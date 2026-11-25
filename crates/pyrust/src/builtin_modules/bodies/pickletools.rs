// `pickletools` module — 100% native Rust implementation of Python 3.12's
// `pickletools` module (issue #3248).
//
// Provides `ArgumentDescriptor`, `StackObject`, `OpcodeInfo`, all 24 argument
// readers & descriptors, all 20 stack object descriptors, all 68 protocol 0–5
// opcodes (`opcodes`, `code2op`), `genops(pickle)`, `dis(...)`, and
// `optimize(p)`.
//
// Reference: <https://docs.python.org/3/library/pickletools.html>

use std::collections::{HashMap, HashSet};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, ascii_repr_interp, object_class_singleton,
    primitive_class_by_name, render_value_repr, type_class_singleton, value_class,
    value_type_name_str,
};
use crate::value::{PyBigInt, PyDict, PyKey, Value, ValueKind};

fn py_isinstance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> Result<bool> {
    let res = call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls.clone()],
    )?;
    interp.truthy_value(&res)
}

const UP_TO_NEWLINE: i64 = -1;
const TAKEN_FROM_ARGUMENT1: i64 = -2;
const TAKEN_FROM_ARGUMENT4: i64 = -3;
const TAKEN_FROM_ARGUMENT4U: i64 = -4;
const TAKEN_FROM_ARGUMENT8U: i64 = -5;

const FRAME_SIZE_TARGET: usize = 64 * 1024;

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn read_bytes_from_file(interp: &mut Interpreter, f: &Value, n: usize) -> Result<Vec<u8>> {
    let read_fn = interp.get_attr(f, "read")?;
    let val = call_pos(interp, read_fn, &[Value::int(n as i64)])?;
    let bytes = PyBytesLike::try_from_value_with_interp(interp, &val, "read", "data")?;
    Ok(bytes.as_bytes().to_vec())
}

fn readline_bytes_from_file(interp: &mut Interpreter, f: &Value) -> Result<Vec<u8>> {
    let readline_fn = interp.get_attr(f, "readline")?;
    let val = call_pos(interp, readline_fn, &[])?;
    let bytes = PyBytesLike::try_from_value_with_interp(interp, &val, "readline", "data")?;
    Ok(bytes.as_bytes().to_vec())
}

fn value_from_u64(v: u64) -> Value {
    if let Ok(i) = i64::try_from(v) {
        Value::int(i)
    } else {
        Value::bigint(PyBigInt::from(v))
    }
}

fn decode_long_bytes(data: &[u8]) -> Value {
    if data.is_empty() {
        return Value::int(0);
    }
    let bi = PyBigInt::from_signed_bytes_le(data);
    if let Ok(i) = i64::try_from(&bi) {
        Value::int(i)
    } else {
        Value::bigint(bi)
    }
}

fn decode_latin1(data: &[u8]) -> String {
    data.iter().map(|&b| b as char).collect()
}

fn decode_utf8_surrogatepass(interp: &mut Interpreter, data: &[u8]) -> Result<Value> {
    if let Ok(s) = std::str::from_utf8(data) {
        return Ok(Value::string(s));
    }
    let str_cls = primitive_class_by_name("str")
        .map(Value::py_class)
        .unwrap_or_else(|| Value::builtin_function("str"));
    call_pos(
        interp,
        str_cls,
        &[
            Value::bytes(data.to_vec()),
            Value::string("utf-8"),
            Value::string("surrogatepass"),
        ],
    )
}

fn read_stringnl_impl(
    interp: &mut Interpreter,
    f: &Value,
    decode: bool,
    stripquotes: bool,
    encoding: &str,
) -> Result<Value> {
    let mut data = readline_bytes_from_file(interp, f)?;
    if !data.ends_with(b"\n") {
        return Err(PyError::named(
            "ValueError",
            "no newline found when trying to read stringnl",
        ));
    }
    data.pop();

    if stripquotes {
        if data.starts_with(b"\"") {
            if data.len() < 2 || !data.ends_with(b"\"") {
                let r = render_value_repr(interp, &Value::bytes(data))?;
                return Err(PyError::named(
                    "ValueError",
                    format!("strinq quote b'\"' not found at both ends of {r}"),
                ));
            }
            data = data[1..data.len() - 1].to_vec();
        } else if data.starts_with(b"'") {
            if data.len() < 2 || !data.ends_with(b"'") {
                let r = render_value_repr(interp, &Value::bytes(data))?;
                return Err(PyError::named(
                    "ValueError",
                    format!("strinq quote b\"'\" not found at both ends of {r}"),
                ));
            }
            data = data[1..data.len() - 1].to_vec();
        } else {
            let r = render_value_repr(interp, &Value::bytes(data))?;
            return Err(PyError::named(
                "ValueError",
                format!("no string quotes around {r}"),
            ));
        }
    }

    if decode {
        let codecs_mod = interp.load_module("codecs")?;
        let esc_dec = interp.get_attr(&codecs_mod, "escape_decode")?;
        let tup = call_pos(interp, esc_dec, &[Value::bytes(data)])?;
        let raw_bytes = interp.eval_index(&tup, Value::int(0))?;
        let decode_fn = interp.get_attr(&raw_bytes, "decode")?;
        return call_pos(interp, decode_fn, &[Value::string(encoding)]);
    }
    Ok(Value::bytes(data))
}

fn wrap_pickle_source(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let is_bytes_like = matches!(data.kind(), ValueKind::Bytes(_))
        || pyrust_builtins::bytearray::as_bytearray_rc(data).is_some();
    if is_bytes_like {
        let io_mod = interp.load_module("io")?;
        let bytes_io = interp.get_attr(&io_mod, "BytesIO")?;
        return call_pos(interp, bytes_io, std::slice::from_ref(data));
    }
    Ok(data.clone())
}

// ── Static opcode table matching CPython 3.12 ────────────────────────────────

struct OpcodeSpec {
    name: &'static str,
    code: u8,
    arg: Option<&'static str>,
    stack_before: &'static [&'static str],
    stack_after: &'static [&'static str],
    proto: i64,
    doc: &'static str,
}

const OPCODE_SPECS: &[OpcodeSpec] = &[
    OpcodeSpec {
        name: "INT",
        code: b'I',
        arg: Some("decimalnl_short"),
        stack_before: &[],
        stack_after: &["pyinteger_or_bool"],
        proto: 0,
        doc: "Push an integer or bool.",
    },
    OpcodeSpec {
        name: "BININT",
        code: b'J',
        arg: Some("int4"),
        stack_before: &[],
        stack_after: &["pyint"],
        proto: 1,
        doc: "Push a four-byte signed integer.",
    },
    OpcodeSpec {
        name: "BININT1",
        code: b'K',
        arg: Some("uint1"),
        stack_before: &[],
        stack_after: &["pyint"],
        proto: 1,
        doc: "Push a one-byte unsigned integer.",
    },
    OpcodeSpec {
        name: "BININT2",
        code: b'M',
        arg: Some("uint2"),
        stack_before: &[],
        stack_after: &["pyint"],
        proto: 1,
        doc: "Push a two-byte unsigned integer.",
    },
    OpcodeSpec {
        name: "LONG",
        code: b'L',
        arg: Some("decimalnl_long"),
        stack_before: &[],
        stack_after: &["pyint"],
        proto: 0,
        doc: "Push a long integer.",
    },
    OpcodeSpec {
        name: "LONG1",
        code: 0x8a,
        arg: Some("long1"),
        stack_before: &[],
        stack_after: &["pyint"],
        proto: 2,
        doc: "Long integer using one-byte length.",
    },
    OpcodeSpec {
        name: "LONG4",
        code: 0x8b,
        arg: Some("long4"),
        stack_before: &[],
        stack_after: &["pyint"],
        proto: 2,
        doc: "Long integer using four-byte length.",
    },
    OpcodeSpec {
        name: "STRING",
        code: b'S',
        arg: Some("stringnl"),
        stack_before: &[],
        stack_after: &["pybytes_or_str"],
        proto: 0,
        doc: "Push a Python string object.",
    },
    OpcodeSpec {
        name: "BINSTRING",
        code: b'T',
        arg: Some("string4"),
        stack_before: &[],
        stack_after: &["pybytes_or_str"],
        proto: 1,
        doc: "Push a Python string object.",
    },
    OpcodeSpec {
        name: "SHORT_BINSTRING",
        code: b'U',
        arg: Some("string1"),
        stack_before: &[],
        stack_after: &["pybytes_or_str"],
        proto: 1,
        doc: "Push a Python string object.",
    },
    OpcodeSpec {
        name: "BINBYTES",
        code: b'B',
        arg: Some("bytes4"),
        stack_before: &[],
        stack_after: &["pybytes"],
        proto: 3,
        doc: "Push a Python bytes object.",
    },
    OpcodeSpec {
        name: "SHORT_BINBYTES",
        code: b'C',
        arg: Some("bytes1"),
        stack_before: &[],
        stack_after: &["pybytes"],
        proto: 3,
        doc: "Push a Python bytes object.",
    },
    OpcodeSpec {
        name: "BINBYTES8",
        code: 0x8e,
        arg: Some("bytes8"),
        stack_before: &[],
        stack_after: &["pybytes"],
        proto: 4,
        doc: "Push a Python bytes object.",
    },
    OpcodeSpec {
        name: "BYTEARRAY8",
        code: 0x96,
        arg: Some("bytearray8"),
        stack_before: &[],
        stack_after: &["pybytearray"],
        proto: 5,
        doc: "Push a Python bytearray object.",
    },
    OpcodeSpec {
        name: "NEXT_BUFFER",
        code: 0x97,
        arg: None,
        stack_before: &[],
        stack_after: &["pybuffer"],
        proto: 5,
        doc: "Push an out-of-band buffer object.",
    },
    OpcodeSpec {
        name: "READONLY_BUFFER",
        code: 0x98,
        arg: None,
        stack_before: &["pybuffer"],
        stack_after: &["pybuffer"],
        proto: 5,
        doc: "Make an out-of-band buffer object read-only.",
    },
    OpcodeSpec {
        name: "NONE",
        code: b'N',
        arg: None,
        stack_before: &[],
        stack_after: &["pynone"],
        proto: 0,
        doc: "Push None on the stack.",
    },
    OpcodeSpec {
        name: "NEWTRUE",
        code: 0x88,
        arg: None,
        stack_before: &[],
        stack_after: &["pybool"],
        proto: 2,
        doc: "Push True onto the stack.",
    },
    OpcodeSpec {
        name: "NEWFALSE",
        code: 0x89,
        arg: None,
        stack_before: &[],
        stack_after: &["pybool"],
        proto: 2,
        doc: "Push False onto the stack.",
    },
    OpcodeSpec {
        name: "UNICODE",
        code: b'V',
        arg: Some("unicodestringnl"),
        stack_before: &[],
        stack_after: &["pyunicode"],
        proto: 0,
        doc: "Push a Python Unicode string object.",
    },
    OpcodeSpec {
        name: "SHORT_BINUNICODE",
        code: 0x8c,
        arg: Some("unicodestring1"),
        stack_before: &[],
        stack_after: &["pyunicode"],
        proto: 4,
        doc: "Push a Python Unicode string object.",
    },
    OpcodeSpec {
        name: "BINUNICODE",
        code: b'X',
        arg: Some("unicodestring4"),
        stack_before: &[],
        stack_after: &["pyunicode"],
        proto: 1,
        doc: "Push a Python Unicode string object.",
    },
    OpcodeSpec {
        name: "BINUNICODE8",
        code: 0x8d,
        arg: Some("unicodestring8"),
        stack_before: &[],
        stack_after: &["pyunicode"],
        proto: 4,
        doc: "Push a Python Unicode string object.",
    },
    OpcodeSpec {
        name: "FLOAT",
        code: b'F',
        arg: Some("floatnl"),
        stack_before: &[],
        stack_after: &["pyfloat"],
        proto: 0,
        doc: "Newline-terminated decimal float literal.",
    },
    OpcodeSpec {
        name: "BINFLOAT",
        code: b'G',
        arg: Some("float8"),
        stack_before: &[],
        stack_after: &["pyfloat"],
        proto: 1,
        doc: "Float stored in binary form, with 8 bytes of data.",
    },
    OpcodeSpec {
        name: "EMPTY_LIST",
        code: b']',
        arg: None,
        stack_before: &[],
        stack_after: &["pylist"],
        proto: 1,
        doc: "Push an empty list.",
    },
    OpcodeSpec {
        name: "APPEND",
        code: b'a',
        arg: None,
        stack_before: &["pylist", "anyobject"],
        stack_after: &["pylist"],
        proto: 0,
        doc: "Append an object to a list.",
    },
    OpcodeSpec {
        name: "APPENDS",
        code: b'e',
        arg: None,
        stack_before: &["pylist", "markobject", "stackslice"],
        stack_after: &["pylist"],
        proto: 1,
        doc: "Extend a list by a slice of stack objects.",
    },
    OpcodeSpec {
        name: "LIST",
        code: b'l',
        arg: None,
        stack_before: &["markobject", "stackslice"],
        stack_after: &["pylist"],
        proto: 0,
        doc: "Build a list out of the topmost stack slice, after markobject.",
    },
    OpcodeSpec {
        name: "EMPTY_TUPLE",
        code: b')',
        arg: None,
        stack_before: &[],
        stack_after: &["pytuple"],
        proto: 1,
        doc: "Push an empty tuple.",
    },
    OpcodeSpec {
        name: "TUPLE",
        code: b't',
        arg: None,
        stack_before: &["markobject", "stackslice"],
        stack_after: &["pytuple"],
        proto: 0,
        doc: "Build a tuple out of the topmost stack slice, after markobject.",
    },
    OpcodeSpec {
        name: "TUPLE1",
        code: 0x85,
        arg: None,
        stack_before: &["anyobject"],
        stack_after: &["pytuple"],
        proto: 2,
        doc: "Build a one-tuple out of the topmost item on the stack.",
    },
    OpcodeSpec {
        name: "TUPLE2",
        code: 0x86,
        arg: None,
        stack_before: &["anyobject", "anyobject"],
        stack_after: &["pytuple"],
        proto: 2,
        doc: "Build a two-tuple out of the top two items on the stack.",
    },
    OpcodeSpec {
        name: "TUPLE3",
        code: 0x87,
        arg: None,
        stack_before: &["anyobject", "anyobject", "anyobject"],
        stack_after: &["pytuple"],
        proto: 2,
        doc: "Build a three-tuple out of the top three items on the stack.",
    },
    OpcodeSpec {
        name: "EMPTY_DICT",
        code: b'}',
        arg: None,
        stack_before: &[],
        stack_after: &["pydict"],
        proto: 1,
        doc: "Push an empty dict.",
    },
    OpcodeSpec {
        name: "DICT",
        code: b'd',
        arg: None,
        stack_before: &["markobject", "stackslice"],
        stack_after: &["pydict"],
        proto: 0,
        doc: "Build a dict out of the topmost stack slice, after markobject.",
    },
    OpcodeSpec {
        name: "SETITEM",
        code: b's',
        arg: None,
        stack_before: &["pydict", "anyobject", "anyobject"],
        stack_after: &["pydict"],
        proto: 0,
        doc: "Add a key+value pair to an existing dict.",
    },
    OpcodeSpec {
        name: "SETITEMS",
        code: b'u',
        arg: None,
        stack_before: &["pydict", "markobject", "stackslice"],
        stack_after: &["pydict"],
        proto: 1,
        doc: "Add an arbitrary number of key+value pairs to an existing dict.",
    },
    OpcodeSpec {
        name: "EMPTY_SET",
        code: 0x8f,
        arg: None,
        stack_before: &[],
        stack_after: &["pyset"],
        proto: 4,
        doc: "Push an empty set.",
    },
    OpcodeSpec {
        name: "ADDITEMS",
        code: 0x90,
        arg: None,
        stack_before: &["pyset", "markobject", "stackslice"],
        stack_after: &["pyset"],
        proto: 4,
        doc: "Add an arbitrary number of items to an existing set.",
    },
    OpcodeSpec {
        name: "FROZENSET",
        code: 0x91,
        arg: None,
        stack_before: &["markobject", "stackslice"],
        stack_after: &["pyfrozenset"],
        proto: 4,
        doc: "Build a frozenset out of the topmost slice, after markobject.",
    },
    OpcodeSpec {
        name: "POP",
        code: b'0',
        arg: None,
        stack_before: &["anyobject"],
        stack_after: &[],
        proto: 0,
        doc: "Discard the top stack item, shrinking the stack by one item.",
    },
    OpcodeSpec {
        name: "DUP",
        code: b'2',
        arg: None,
        stack_before: &["anyobject"],
        stack_after: &["anyobject", "anyobject"],
        proto: 0,
        doc: "Push the top stack item onto the stack again, duplicating it.",
    },
    OpcodeSpec {
        name: "MARK",
        code: b'(',
        arg: None,
        stack_before: &[],
        stack_after: &["markobject"],
        proto: 0,
        doc: "Push markobject onto the stack.",
    },
    OpcodeSpec {
        name: "POP_MARK",
        code: b'1',
        arg: None,
        stack_before: &["markobject", "stackslice"],
        stack_after: &[],
        proto: 1,
        doc: "Pop all the stack objects at and above the topmost markobject.",
    },
    OpcodeSpec {
        name: "GET",
        code: b'g',
        arg: Some("decimalnl_short"),
        stack_before: &[],
        stack_after: &["anyobject"],
        proto: 0,
        doc: "Read an object from the memo and push it on the stack.",
    },
    OpcodeSpec {
        name: "BINGET",
        code: b'h',
        arg: Some("uint1"),
        stack_before: &[],
        stack_after: &["anyobject"],
        proto: 1,
        doc: "Read an object from the memo and push it on the stack.",
    },
    OpcodeSpec {
        name: "LONG_BINGET",
        code: b'j',
        arg: Some("uint4"),
        stack_before: &[],
        stack_after: &["anyobject"],
        proto: 1,
        doc: "Read an object from the memo and push it on the stack.",
    },
    OpcodeSpec {
        name: "PUT",
        code: b'p',
        arg: Some("decimalnl_short"),
        stack_before: &[],
        stack_after: &[],
        proto: 0,
        doc: "Store the stack top into the memo.  The stack is not popped.",
    },
    OpcodeSpec {
        name: "BINPUT",
        code: b'q',
        arg: Some("uint1"),
        stack_before: &[],
        stack_after: &[],
        proto: 1,
        doc: "Store the stack top into the memo.  The stack is not popped.",
    },
    OpcodeSpec {
        name: "LONG_BINPUT",
        code: b'r',
        arg: Some("uint4"),
        stack_before: &[],
        stack_after: &[],
        proto: 1,
        doc: "Store the stack top into the memo.  The stack is not popped.",
    },
    OpcodeSpec {
        name: "MEMOIZE",
        code: 0x94,
        arg: None,
        stack_before: &["anyobject"],
        stack_after: &["anyobject"],
        proto: 4,
        doc: "Store the stack top into the memo.  The stack is not popped.",
    },
    OpcodeSpec {
        name: "EXT1",
        code: 0x82,
        arg: Some("uint1"),
        stack_before: &[],
        stack_after: &["anyobject"],
        proto: 2,
        doc: "Extension code.",
    },
    OpcodeSpec {
        name: "EXT2",
        code: 0x83,
        arg: Some("uint2"),
        stack_before: &[],
        stack_after: &["anyobject"],
        proto: 2,
        doc: "Extension code.",
    },
    OpcodeSpec {
        name: "EXT4",
        code: 0x84,
        arg: Some("int4"),
        stack_before: &[],
        stack_after: &["anyobject"],
        proto: 2,
        doc: "Extension code.",
    },
    OpcodeSpec {
        name: "GLOBAL",
        code: b'c',
        arg: Some("stringnl_noescape_pair"),
        stack_before: &[],
        stack_after: &["anyobject"],
        proto: 0,
        doc: "Push a global object (module.attr) on the stack.",
    },
    OpcodeSpec {
        name: "STACK_GLOBAL",
        code: 0x93,
        arg: None,
        stack_before: &["pyunicode", "pyunicode"],
        stack_after: &["anyobject"],
        proto: 4,
        doc: "Push a global object (module.attr) on the stack.",
    },
    OpcodeSpec {
        name: "REDUCE",
        code: b'R',
        arg: None,
        stack_before: &["anyobject", "anyobject"],
        stack_after: &["anyobject"],
        proto: 0,
        doc: "Push an object built from a callable and an argument tuple.",
    },
    OpcodeSpec {
        name: "BUILD",
        code: b'b',
        arg: None,
        stack_before: &["anyobject", "anyobject"],
        stack_after: &["anyobject"],
        proto: 0,
        doc: "Finish building an object, via __setstate__ or dict update.",
    },
    OpcodeSpec {
        name: "INST",
        code: b'i',
        arg: Some("stringnl_noescape_pair"),
        stack_before: &["markobject", "stackslice"],
        stack_after: &["anyobject"],
        proto: 0,
        doc: "Build a class instance.",
    },
    OpcodeSpec {
        name: "OBJ",
        code: b'o',
        arg: None,
        stack_before: &["markobject", "anyobject", "stackslice"],
        stack_after: &["anyobject"],
        proto: 1,
        doc: "Build a class instance.",
    },
    OpcodeSpec {
        name: "NEWOBJ",
        code: 0x81,
        arg: None,
        stack_before: &["anyobject", "anyobject"],
        stack_after: &["anyobject"],
        proto: 2,
        doc: "Build an object instance.",
    },
    OpcodeSpec {
        name: "NEWOBJ_EX",
        code: 0x92,
        arg: None,
        stack_before: &["anyobject", "anyobject", "anyobject"],
        stack_after: &["anyobject"],
        proto: 4,
        doc: "Build an object instance.",
    },
    OpcodeSpec {
        name: "PROTO",
        code: 0x80,
        arg: Some("uint1"),
        stack_before: &[],
        stack_after: &[],
        proto: 2,
        doc: "Protocol version indicator.",
    },
    OpcodeSpec {
        name: "STOP",
        code: b'.',
        arg: None,
        stack_before: &["anyobject"],
        stack_after: &[],
        proto: 0,
        doc: "Stop the unpickling machine.",
    },
    OpcodeSpec {
        name: "FRAME",
        code: 0x95,
        arg: Some("uint8"),
        stack_before: &[],
        stack_after: &[],
        proto: 4,
        doc: "Indicate the beginning of a new frame.",
    },
    OpcodeSpec {
        name: "PERSID",
        code: b'P',
        arg: Some("stringnl_noescape"),
        stack_before: &[],
        stack_after: &["anyobject"],
        proto: 0,
        doc: "Push an object identified by a persistent ID.",
    },
    OpcodeSpec {
        name: "BINPERSID",
        code: b'Q',
        arg: None,
        stack_before: &["anyobject"],
        stack_after: &["anyobject"],
        proto: 1,
        doc: "Push an object identified by a persistent ID.",
    },
];

// ── Native iterator state for genops ─────────────────────────────────────────

struct GenopsState {
    stream: Value,
    getpos: Option<Value>,
    code2op: Value,
    done: bool,
}

fn next_op_step(
    interp: &mut Interpreter,
    stream: &Value,
    getpos: &Option<Value>,
    code2op: &Value,
) -> Result<(Value, Value, Value, Value, bool)> {
    let pos = match getpos {
        Some(tell_fn) => call_pos(interp, tell_fn.clone(), &[])?,
        None => Value::none(),
    };
    let code_bytes = read_bytes_from_file(interp, stream, 1)?;
    if code_bytes.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "pickle exhausted before seeing STOP",
        ));
    }
    let ch = code_bytes[0] as char;
    let mut ch_buf = [0u8; 4];
    let code_str = ch.encode_utf8(&mut ch_buf);
    let opcode = code2op
        .as_dict()
        .and_then(|d| d.get(&PyKey::str_from(code_str)).cloned());
    let Some(opcode) = opcode else {
        let pos_s = if pos.is_none() {
            "<unknown>".to_string()
        } else {
            interp.render_value_as_str(&pos)?
        };
        let code_r = render_value_repr(interp, &Value::bytes(code_bytes))?;
        return Err(PyError::named(
            "ValueError",
            format!("at position {pos_s}, opcode {code_r} unknown"),
        ));
    };
    let arg_desc = interp.get_attr(&opcode, "arg")?;
    let arg = if arg_desc.is_none() {
        Value::none()
    } else {
        let reader = interp.get_attr(&arg_desc, "reader")?;
        call_pos(interp, reader, std::slice::from_ref(stream))?
    };
    let end_pos = match getpos {
        Some(tell_fn) => call_pos(interp, tell_fn.clone(), &[])?,
        None => Value::none(),
    };
    let is_stop = code_bytes[0] == b'.';
    Ok((opcode, arg, pos, end_pos, is_stop))
}

fn make_genops_iter(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let stream = wrap_pickle_source(interp, data)?;
    let getpos = match interp.get_attr(&stream, "tell") {
        Ok(v) => Some(v),
        Err(ref e) if e.class_name_is("AttributeError") => None,
        Err(e) => return Err(e),
    };
    let mod_val = interp.load_module("pickletools")?;
    let code2op = interp.get_attr(&mod_val, "code2op")?;
    let iter_cls = interp.get_attr(&mod_val, "_GenopsIterator")?;
    let inst = call_pos(interp, iter_cls, &[])?;
    crate::builtin_modules::macros::native_state::set_native_state(
        &inst,
        GenopsState {
            stream,
            getpos,
            code2op,
            done: false,
        },
    )?;
    Ok(inst)
}

fn write_line(interp: &mut Interpreter, out: &Value, line: &str) -> Result<()> {
    let write_fn = interp.get_attr(out, "write")?;
    call_pos(interp, write_fn, &[Value::string(format!("{line}\n"))])?;
    Ok(())
}

fn encode_put(proto: i64, idx: usize) -> Vec<u8> {
    if proto >= 4 {
        vec![0x94]
    } else if proto >= 1 {
        if idx < 256 {
            vec![b'q', idx as u8]
        } else {
            let mut v = Vec::with_capacity(5);
            v.push(b'r');
            v.extend_from_slice(&(idx as u32).to_le_bytes());
            v
        }
    } else {
        format!("p{idx}\n").into_bytes()
    }
}

fn encode_get(proto: i64, idx: usize) -> Vec<u8> {
    if proto >= 1 {
        if idx < 256 {
            vec![b'h', idx as u8]
        } else {
            let mut v = Vec::with_capacity(5);
            v.push(b'j');
            v.extend_from_slice(&(idx as u32).to_le_bytes());
            v
        }
    } else {
        format!("g{idx}\n").into_bytes()
    }
}

fn commit_frame(out: &mut Vec<u8>, frame_buf: &mut Vec<u8>, force: bool) {
    if !frame_buf.is_empty() && (force || frame_buf.len() >= FRAME_SIZE_TARGET) {
        out.push(0x95);
        out.extend_from_slice(&(frame_buf.len() as u64).to_le_bytes());
        out.extend_from_slice(frame_buf);
        frame_buf.clear();
    }
}

/// Populate `ArgumentDescriptor`, `StackObject`, `opcodes`, `code2op`, and
/// `bytes_types` on a freshly loaded `pickletools` module.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let get_mod = |name: &str| mod_rc.borrow().attrs.get(name).cloned();
    let Some(arg_cls) = get_mod("ArgumentDescriptor") else {
        return;
    };
    let Some(stack_cls) = get_mod("StackObject") else {
        return;
    };
    let Some(op_cls) = get_mod("OpcodeInfo") else {
        return;
    };

    let prim = |name: &str| -> Value {
        primitive_class_by_name(name)
            .map(Value::py_class)
            .unwrap_or_else(|| Value::py_class(object_class_singleton()))
    };
    let none_type = value_class(&Value::none());
    let obj_type = Value::py_class(object_class_singleton());

    // `bytes_types = (bytes, bytearray)`
    let bytes_types = Value::tuple(vec![prim("bytes"), prim("bytearray")]);
    mod_rc
        .borrow_mut()
        .insert_attr("bytes_types".to_string(), bytes_types);

    // 1. ArgumentDescriptor instances
    let arg_specs: [(&str, i64, &str, &str); 24] = [
        ("uint1", 1, "read_uint1", "One-byte unsigned integer."),
        (
            "uint2",
            2,
            "read_uint2",
            "Two-byte unsigned integer, little-endian.",
        ),
        (
            "int4",
            4,
            "read_int4",
            "Four-byte signed integer, little-endian, 2's complement.",
        ),
        (
            "uint4",
            4,
            "read_uint4",
            "Four-byte unsigned integer, little-endian.",
        ),
        (
            "uint8",
            8,
            "read_uint8",
            "Eight-byte unsigned integer, little-endian.",
        ),
        (
            "stringnl",
            UP_TO_NEWLINE,
            "read_stringnl",
            "A newline-terminated string.",
        ),
        (
            "stringnl_noescape",
            UP_TO_NEWLINE,
            "read_stringnl_noescape",
            "A newline-terminated string.",
        ),
        (
            "stringnl_noescape_pair",
            UP_TO_NEWLINE,
            "read_stringnl_noescape_pair",
            "A pair of newline-terminated strings.",
        ),
        (
            "string1",
            TAKEN_FROM_ARGUMENT1,
            "read_string1",
            "A counted string.",
        ),
        (
            "string4",
            TAKEN_FROM_ARGUMENT4,
            "read_string4",
            "A counted string.",
        ),
        (
            "bytes1",
            TAKEN_FROM_ARGUMENT1,
            "read_bytes1",
            "A counted bytes string.",
        ),
        (
            "bytes4",
            TAKEN_FROM_ARGUMENT4U,
            "read_bytes4",
            "A counted bytes string.",
        ),
        (
            "bytes8",
            TAKEN_FROM_ARGUMENT8U,
            "read_bytes8",
            "A counted bytes string.",
        ),
        (
            "bytearray8",
            TAKEN_FROM_ARGUMENT8U,
            "read_bytearray8",
            "A counted bytearray.",
        ),
        (
            "unicodestringnl",
            UP_TO_NEWLINE,
            "read_unicodestringnl",
            "A newline-terminated Unicode string.",
        ),
        (
            "unicodestring1",
            TAKEN_FROM_ARGUMENT1,
            "read_unicodestring1",
            "A counted Unicode string.",
        ),
        (
            "unicodestring4",
            TAKEN_FROM_ARGUMENT4U,
            "read_unicodestring4",
            "A counted Unicode string.",
        ),
        (
            "unicodestring8",
            TAKEN_FROM_ARGUMENT8U,
            "read_unicodestring8",
            "A counted Unicode string.",
        ),
        (
            "decimalnl_short",
            UP_TO_NEWLINE,
            "read_decimalnl_short",
            "A newline-terminated decimal integer literal.",
        ),
        (
            "decimalnl_long",
            UP_TO_NEWLINE,
            "read_decimalnl_long",
            "A newline-terminated decimal integer literal.",
        ),
        (
            "floatnl",
            UP_TO_NEWLINE,
            "read_floatnl",
            "A newline-terminated decimal floating literal.",
        ),
        (
            "float8",
            8,
            "read_float8",
            "An 8-byte binary representation of a float, big-endian.",
        ),
        (
            "long1",
            TAKEN_FROM_ARGUMENT1,
            "read_long1",
            "A binary long, little-endian, using 1-byte size.",
        ),
        (
            "long4",
            TAKEN_FROM_ARGUMENT4,
            "read_long4",
            "A binary representation of a long, little-endian.",
        ),
    ];

    let mut arg_map: HashMap<&'static str, Value> = HashMap::with_capacity(arg_specs.len());
    for (name, n, reader_name, doc) in arg_specs {
        let Some(reader) = get_mod(reader_name) else {
            continue;
        };
        if let Ok(inst) = call_pos(
            interp,
            arg_cls.clone(),
            &[
                Value::string(name),
                Value::int(n),
                reader,
                Value::string(doc),
            ],
        ) {
            mod_rc
                .borrow_mut()
                .insert_attr(name.to_string(), inst.clone());
            arg_map.insert(name, inst);
        }
    }

    // 2. StackObject instances
    let stack_specs: [(&str, &str, Value, &str); 18] = [
        ("pyint", "int", prim("int"), "A Python integer object."),
        (
            "pyinteger_or_bool",
            "int_or_bool",
            Value::tuple(vec![prim("int"), prim("bool")]),
            "A Python integer or boolean object.",
        ),
        ("pybool", "bool", prim("bool"), "A Python boolean object."),
        ("pyfloat", "float", prim("float"), "A Python float object."),
        (
            "pybytes_or_str",
            "bytes_or_str",
            Value::tuple(vec![prim("bytes"), prim("str")]),
            "A Python bytes or (Unicode) string object.",
        ),
        ("pybytes", "bytes", prim("bytes"), "A Python bytes object."),
        (
            "pybytearray",
            "bytearray",
            prim("bytearray"),
            "A Python bytearray object.",
        ),
        (
            "pyunicode",
            "str",
            prim("str"),
            "A Python (Unicode) string object.",
        ),
        ("pynone", "None", none_type, "The Python None object."),
        ("pytuple", "tuple", prim("tuple"), "A Python tuple object."),
        ("pylist", "list", prim("list"), "A Python list object."),
        ("pydict", "dict", prim("dict"), "A Python dict object."),
        ("pyset", "set", prim("set"), "A Python set object."),
        (
            "pyfrozenset",
            "frozenset",
            prim("set"),
            "A Python frozenset object.",
        ),
        (
            "pybuffer",
            "buffer",
            obj_type.clone(),
            "A Python buffer-like object.",
        ),
        (
            "anyobject",
            "any",
            obj_type,
            "Any kind of object whatsoever.",
        ),
        (
            "markobject",
            "mark",
            stack_cls.clone(),
            "'The mark' is a unique object.",
        ),
        (
            "stackslice",
            "stackslice",
            stack_cls.clone(),
            "An object representing a contiguous slice of the stack.",
        ),
    ];

    let mut stack_map: HashMap<&'static str, Value> = HashMap::with_capacity(stack_specs.len());
    for (global_name, obj_name, obtype, doc) in stack_specs {
        if let Ok(inst) = call_pos(
            interp,
            stack_cls.clone(),
            &[Value::string(obj_name), obtype, Value::string(doc)],
        ) {
            mod_rc
                .borrow_mut()
                .insert_attr(global_name.to_string(), inst.clone());
            stack_map.insert(global_name, inst);
        }
    }
    // Aliases matching CPython (`pylong = pyint`, `pystring = pybytes_or_str`).
    if let Some(v) = stack_map.get("pyint").cloned() {
        mod_rc.borrow_mut().insert_attr("pylong".to_string(), v);
    }
    if let Some(v) = stack_map.get("pybytes_or_str").cloned() {
        mod_rc.borrow_mut().insert_attr("pystring".to_string(), v);
    }

    // 3. OpcodeInfo list + code2op dict
    let mut opcodes_list = Vec::with_capacity(OPCODE_SPECS.len());
    let mut code2op_dict = PyDict::default();
    for spec in OPCODE_SPECS {
        let mut ch_buf = [0u8; 4];
        let code_str: &str = (spec.code as char).encode_utf8(&mut ch_buf);
        let arg_val = spec
            .arg
            .and_then(|k| arg_map.get(k).cloned())
            .unwrap_or_else(Value::none);
        let sb: Vec<Value> = spec
            .stack_before
            .iter()
            .filter_map(|k| stack_map.get(k).cloned())
            .collect();
        let sa: Vec<Value> = spec
            .stack_after
            .iter()
            .filter_map(|k| stack_map.get(k).cloned())
            .collect();
        if let Ok(op_inst) = call_pos(
            interp,
            op_cls.clone(),
            &[
                Value::string(spec.name),
                Value::string(code_str),
                arg_val,
                Value::list(sb),
                Value::list(sa),
                Value::int(spec.proto),
                Value::string(spec.doc),
            ],
        ) {
            code2op_dict.insert(PyKey::str_from(code_str), op_inst.clone());
            opcodes_list.push(op_inst);
        }
    }
    let mut m = mod_rc.borrow_mut();
    m.insert_attr("opcodes".to_string(), Value::list(opcodes_list));
    m.insert_attr("code2op".to_string(), Value::dict(code2op_dict));
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("dis"),
            Value::string("genops"),
            Value::string("optimize"),
        ]),
        "UP_TO_NEWLINE" => Value::int(UP_TO_NEWLINE),
        "TAKEN_FROM_ARGUMENT1" => Value::int(TAKEN_FROM_ARGUMENT1),
        "TAKEN_FROM_ARGUMENT4" => Value::int(TAKEN_FROM_ARGUMENT4),
        "TAKEN_FROM_ARGUMENT4U" => Value::int(TAKEN_FROM_ARGUMENT4U),
        "TAKEN_FROM_ARGUMENT8U" => Value::int(TAKEN_FROM_ARGUMENT8U),
    }

    // ── Argument readers ─────────────────────────────────────────────────────

    fn read_uint1(f: PyValueRef<'_>) -> Result<Value> {
        let data = read_bytes_from_file(_interp, f.0, 1)?;
        if let Some(&b) = data.first() {
            Ok(Value::int(b as i64))
        } else {
            Err(PyError::named(
                "ValueError",
                "not enough data in stream to read uint1",
            ))
        }
    }

    fn read_uint2(f: PyValueRef<'_>) -> Result<Value> {
        let data = read_bytes_from_file(_interp, f.0, 2)?;
        if data.len() == 2 {
            Ok(Value::int(u16::from_le_bytes([data[0], data[1]]) as i64))
        } else {
            Err(PyError::named(
                "ValueError",
                "not enough data in stream to read uint2",
            ))
        }
    }

    fn read_int4(f: PyValueRef<'_>) -> Result<Value> {
        let data = read_bytes_from_file(_interp, f.0, 4)?;
        if data.len() == 4 {
            Ok(Value::int(i32::from_le_bytes([data[0], data[1], data[2], data[3]]) as i64))
        } else {
            Err(PyError::named(
                "ValueError",
                "not enough data in stream to read int4",
            ))
        }
    }

    fn read_uint4(f: PyValueRef<'_>) -> Result<Value> {
        let data = read_bytes_from_file(_interp, f.0, 4)?;
        if data.len() == 4 {
            Ok(Value::int(u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as i64))
        } else {
            Err(PyError::named(
                "ValueError",
                "not enough data in stream to read uint4",
            ))
        }
    }

    fn read_uint8(f: PyValueRef<'_>) -> Result<Value> {
        let data = read_bytes_from_file(_interp, f.0, 8)?;
        if data.len() == 8 {
            let v = u64::from_le_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ]);
            Ok(value_from_u64(v))
        } else {
            Err(PyError::named(
                "ValueError",
                "not enough data in stream to read uint8",
            ))
        }
    }

    fn read_stringnl(
        f: PyValueRef<'_>,
        #[default(true.into())] decode: PyBool,
        #[default(true.into())] stripquotes: PyBool,
        #[keyword_only] #[default("latin-1".into())] encoding: PyStr<'_>,
    ) -> Result<Value> {
        read_stringnl_impl(_interp, f.0, *decode, *stripquotes, &encoding)
    }

    fn read_stringnl_noescape(f: PyValueRef<'_>) -> Result<Value> {
        read_stringnl_impl(_interp, f.0, true, false, "utf-8")
    }

    fn read_stringnl_noescape_pair(f: PyValueRef<'_>) -> Result<Value> {
        let a = read_stringnl_impl(_interp, f.0, true, false, "utf-8")?;
        let b = read_stringnl_impl(_interp, f.0, true, false, "utf-8")?;
        let a_s = a.as_str().unwrap_or("");
        let b_s = b.as_str().unwrap_or("");
        Ok(Value::string(format!("{a_s} {b_s}")))
    }

    fn read_string1(f: PyValueRef<'_>) -> Result<Value> {
        let n_bytes = read_bytes_from_file(_interp, f.0, 1)?;
        let Some(&n_u8) = n_bytes.first() else {
            return Err(PyError::named("ValueError", "not enough data in stream to read uint1"));
        };
        let n = n_u8 as usize;
        let data = read_bytes_from_file(_interp, f.0, n)?;
        if data.len() == n {
            Ok(Value::string(decode_latin1(&data)))
        } else {
            Err(PyError::named(
                "ValueError",
                format!("expected {n} bytes in a string1, but only {} remain", data.len()),
            ))
        }
    }

    fn read_string4(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 4)?;
        if hdr.len() != 4 {
            return Err(PyError::named("ValueError", "not enough data in stream to read int4"));
        }
        let n = i32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        if n < 0 {
            return Err(PyError::named(
                "ValueError",
                format!("string4 byte count < 0: {n}"),
            ));
        }
        let n_u = n as usize;
        let data = read_bytes_from_file(_interp, f.0, n_u)?;
        if data.len() == n_u {
            Ok(Value::string(decode_latin1(&data)))
        } else {
            Err(PyError::named(
                "ValueError",
                format!("expected {n_u} bytes in a string4, but only {} remain", data.len()),
            ))
        }
    }

    fn read_bytes1(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 1)?;
        let Some(&n_u8) = hdr.first() else {
            return Err(PyError::named("ValueError", "not enough data in stream to read uint1"));
        };
        let n = n_u8 as usize;
        let data = read_bytes_from_file(_interp, f.0, n)?;
        if data.len() == n {
            Ok(Value::bytes(data))
        } else {
            Err(PyError::named(
                "ValueError",
                format!("expected {n} bytes in a bytes1, but only {} remain", data.len()),
            ))
        }
    }

    fn read_bytes4(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 4)?;
        if hdr.len() != 4 {
            return Err(PyError::named("ValueError", "not enough data in stream to read uint4"));
        }
        let n = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
        let data = read_bytes_from_file(_interp, f.0, n)?;
        if data.len() == n {
            Ok(Value::bytes(data))
        } else {
            Err(PyError::named(
                "ValueError",
                format!("expected {n} bytes in a bytes4, but only {} remain", data.len()),
            ))
        }
    }

    fn read_bytes8(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 8)?;
        if hdr.len() != 8 {
            return Err(PyError::named("ValueError", "not enough data in stream to read uint8"));
        }
        let n_u64 = u64::from_le_bytes([
            hdr[0], hdr[1], hdr[2], hdr[3], hdr[4], hdr[5], hdr[6], hdr[7],
        ]);
        if n_u64 > (isize::MAX as u64) {
            return Err(PyError::named(
                "ValueError",
                format!("bytes8 byte count > sys.maxsize: {n_u64}"),
            ));
        }
        let n = n_u64 as usize;
        let data = read_bytes_from_file(_interp, f.0, n)?;
        if data.len() == n {
            Ok(Value::bytes(data))
        } else {
            Err(PyError::named(
                "ValueError",
                format!("expected {n} bytes in a bytes8, but only {} remain", data.len()),
            ))
        }
    }

    fn read_bytearray8(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 8)?;
        if hdr.len() != 8 {
            return Err(PyError::named("ValueError", "not enough data in stream to read uint8"));
        }
        let n_u64 = u64::from_le_bytes([
            hdr[0], hdr[1], hdr[2], hdr[3], hdr[4], hdr[5], hdr[6], hdr[7],
        ]);
        if n_u64 > (isize::MAX as u64) {
            return Err(PyError::named(
                "ValueError",
                format!("bytearray8 byte count > sys.maxsize: {n_u64}"),
            ));
        }
        let n = n_u64 as usize;
        let data = read_bytes_from_file(_interp, f.0, n)?;
        if data.len() == n {
            Ok(pyrust_builtins::bytearray::bytearray(data))
        } else {
            Err(PyError::named(
                "ValueError",
                format!("expected {n} bytes in a bytearray8, but only {} remain", data.len()),
            ))
        }
    }

    fn read_unicodestringnl(f: PyValueRef<'_>) -> Result<Value> {
        let mut data = readline_bytes_from_file(_interp, f.0)?;
        if !data.ends_with(b"\n") {
            return Err(PyError::named(
                "ValueError",
                "no newline found when trying to read unicodestringnl",
            ));
        }
        data.pop();
        let str_cls = primitive_class_by_name("str")
            .map(Value::py_class)
            .unwrap_or_else(|| Value::builtin_function("str"));
        call_pos(
            _interp,
            str_cls,
            &[Value::bytes(data), Value::string("raw-unicode-escape")],
        )
    }

    fn read_unicodestring1(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 1)?;
        let Some(&n_u8) = hdr.first() else {
            return Err(PyError::named("ValueError", "not enough data in stream to read uint1"));
        };
        let n = n_u8 as usize;
        let data = read_bytes_from_file(_interp, f.0, n)?;
        if data.len() == n {
            decode_utf8_surrogatepass(_interp, &data)
        } else {
            Err(PyError::named(
                "ValueError",
                format!(
                    "expected {n} bytes in a unicodestring1, but only {} remain",
                    data.len()
                ),
            ))
        }
    }

    fn read_unicodestring4(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 4)?;
        if hdr.len() != 4 {
            return Err(PyError::named("ValueError", "not enough data in stream to read uint4"));
        }
        let n = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
        let data = read_bytes_from_file(_interp, f.0, n)?;
        if data.len() == n {
            decode_utf8_surrogatepass(_interp, &data)
        } else {
            Err(PyError::named(
                "ValueError",
                format!(
                    "expected {n} bytes in a unicodestring4, but only {} remain",
                    data.len()
                ),
            ))
        }
    }

    fn read_unicodestring8(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 8)?;
        if hdr.len() != 8 {
            return Err(PyError::named("ValueError", "not enough data in stream to read uint8"));
        }
        let n_u64 = u64::from_le_bytes([
            hdr[0], hdr[1], hdr[2], hdr[3], hdr[4], hdr[5], hdr[6], hdr[7],
        ]);
        if n_u64 > (isize::MAX as u64) {
            return Err(PyError::named(
                "ValueError",
                format!("unicodestring8 byte count > sys.maxsize: {n_u64}"),
            ));
        }
        let n = n_u64 as usize;
        let data = read_bytes_from_file(_interp, f.0, n)?;
        if data.len() == n {
            decode_utf8_surrogatepass(_interp, &data)
        } else {
            Err(PyError::named(
                "ValueError",
                format!(
                    "expected {n} bytes in a unicodestring8, but only {} remain",
                    data.len()
                ),
            ))
        }
    }

    fn read_decimalnl_short(f: PyValueRef<'_>) -> Result<Value> {
        let raw = read_stringnl_impl(_interp, f.0, false, false, "latin-1")?;
        let bytes = match raw.kind() {
            ValueKind::Bytes(b) => b.as_slice(),
            _ => &[],
        };
        if bytes == b"00" {
            return Ok(Value::bool_(false));
        }
        if bytes == b"01" {
            return Ok(Value::bool_(true));
        }
        let int_cls = primitive_class_by_name("int")
            .map(Value::py_class)
            .unwrap_or_else(|| Value::builtin_function("int"));
        call_pos(_interp, int_cls, &[raw])
    }

    fn read_decimalnl_long(f: PyValueRef<'_>) -> Result<Value> {
        let raw = read_stringnl_impl(_interp, f.0, false, false, "latin-1")?;
        let trimmed = match raw.kind() {
            ValueKind::Bytes(b) => b.as_slice().strip_suffix(b"L").unwrap_or(b.as_slice()).to_vec(),
            _ => Vec::new(),
        };
        let int_cls = primitive_class_by_name("int")
            .map(Value::py_class)
            .unwrap_or_else(|| Value::builtin_function("int"));
        call_pos(_interp, int_cls, &[Value::bytes(trimmed)])
    }

    fn read_floatnl(f: PyValueRef<'_>) -> Result<Value> {
        let raw = read_stringnl_impl(_interp, f.0, false, false, "latin-1")?;
        let float_cls = primitive_class_by_name("float")
            .map(Value::py_class)
            .unwrap_or_else(|| Value::builtin_function("float"));
        call_pos(_interp, float_cls, &[raw])
    }

    fn read_float8(f: PyValueRef<'_>) -> Result<Value> {
        let data = read_bytes_from_file(_interp, f.0, 8)?;
        if data.len() == 8 {
            let v = f64::from_be_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ]);
            Ok(Value::float(v))
        } else {
            Err(PyError::named(
                "ValueError",
                "not enough data in stream to read float8",
            ))
        }
    }

    fn read_long1(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 1)?;
        let Some(&n_u8) = hdr.first() else {
            return Err(PyError::named("ValueError", "not enough data in stream to read uint1"));
        };
        let n = n_u8 as usize;
        let data = read_bytes_from_file(_interp, f.0, n)?;
        if data.len() != n {
            return Err(PyError::named(
                "ValueError",
                "not enough data in stream to read long1",
            ));
        }
        Ok(decode_long_bytes(&data))
    }

    fn read_long4(f: PyValueRef<'_>) -> Result<Value> {
        let hdr = read_bytes_from_file(_interp, f.0, 4)?;
        if hdr.len() != 4 {
            return Err(PyError::named("ValueError", "not enough data in stream to read int4"));
        }
        let n = i32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        if n < 0 {
            return Err(PyError::named(
                "ValueError",
                format!("long4 byte count < 0: {n}"),
            ));
        }
        let n_u = n as usize;
        let data = read_bytes_from_file(_interp, f.0, n_u)?;
        if data.len() != n_u {
            return Err(PyError::named(
                "ValueError",
                "not enough data in stream to read long4",
            ));
        }
        Ok(decode_long_bytes(&data))
    }

    // ── Public API: genops, dis, optimize ────────────────────────────────────

    fn genops(pickle: PyValueRef<'_>) -> Result<Value> {
        make_genops_iter(_interp, pickle.0)
    }

    fn dis(
        pickle: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] out: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] memo: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] indentlevel: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] annotate: OptValueRef<'_>,
    ) -> Result<Value> {
        let out_stream = match out.as_value().filter(|v| !v.is_none()) {
            Some(v) => v.clone(),
            None => {
                let sys_mod = _interp.load_module("sys")?;
                _interp.get_attr(&sys_mod, "stdout")?
            }
        };
        let memo_dict = match memo.as_value().filter(|v| !v.is_none()) {
            Some(v) => v.clone(),
            None => Value::dict(PyDict::default()),
        };
        let indent_n = match indentlevel.as_value() {
            Some(v) => PyIndex::try_from_value_with_interp(_interp, v, "dis", "indentlevel")?
                .expect_isize("dis", "indentlevel")?
                .max(0) as usize,
            None => 4usize,
        };
        let annotate_col = match annotate.as_value() {
            Some(v) => PyIndex::try_from_value_with_interp(_interp, v, "dis", "annotate")?
                .expect_isize("dis", "annotate")?
                .max(0) as usize,
            None => 0usize,
        };

        let mod_val = _interp.load_module("pickletools")?;
        let code2op = _interp.get_attr(&mod_val, "code2op")?;
        let markobject = _interp.get_attr(&mod_val, "markobject")?;
        let markobject_id = markobject.object_id().as_int().unwrap_or(0);

        let stream = wrap_pickle_source(_interp, pickle.0)?;
        let getpos = match _interp.get_attr(&stream, "tell") {
            Ok(v) => Some(v),
            Err(ref e) if e.class_name_is("AttributeError") => None,
            Err(e) => return Err(e),
        };

        let mut stack: Vec<Value> = Vec::new();
        let mut markstack: Vec<Value> = Vec::new();
        let mut maxproto: i64 = -1;
        let indentchunk = " ".repeat(indent_n);
        let mut annocol = annotate_col;

        loop {
            let (opcode, arg, pos, _end_pos, is_stop) =
                next_op_step(_interp, &stream, &getpos, &code2op)?;
            let op_name = _interp
                .get_attr(&opcode, "name")?
                .as_str()
                .unwrap_or("")
                .to_owned();
            let op_code_val = _interp.get_attr(&opcode, "code")?;
            let op_proto = _interp.get_attr(&opcode, "proto")?.as_int().unwrap_or(0);
            let op_doc = _interp
                .get_attr(&opcode, "doc")?
                .as_str()
                .unwrap_or("")
                .to_owned();
            let before_val = _interp.get_attr(&opcode, "stack_before")?;
            let after_val = _interp.get_attr(&opcode, "stack_after")?;
            let before = _interp.collect_iterable(&before_val)?;
            let mut after = _interp.collect_iterable(&after_val)?;

            maxproto = maxproto.max(op_proto);

            let code_repr = render_value_repr(_interp, &op_code_val)?;
            let code_inner = if code_repr.len() >= 2 {
                &code_repr[1..code_repr.len() - 1]
            } else {
                code_repr.as_str()
            };

            let mut prefix = String::new();
            if !pos.is_none() {
                let pos_i = pos.as_int().unwrap_or(0);
                prefix = format!("{pos_i:5}: ");
            }
            let mut line = format!(
                "{:<4} {}{}",
                code_inner,
                indentchunk.repeat(markstack.len()),
                op_name
            );

            let mut numtopop = before.len();
            let mut markmsg: Option<String> = None;
            let mut errormsg: Option<String> = None;

            let before_has_mark = before
                .iter()
                .position(|v| v.object_id().as_int() == Some(markobject_id));
            let pop_is_mark = op_name == "POP"
                && stack
                    .last()
                    .is_some_and(|v| v.object_id().as_int() == Some(markobject_id));

            if before_has_mark.is_some() || pop_is_mark {
                if let Some(markpos) = markstack.pop() {
                    if markpos.is_none() {
                        markmsg = Some("(MARK at unknown opcode offset)".to_string());
                    } else {
                        let mp = markpos.as_int().unwrap_or(0);
                        markmsg = Some(format!("(MARK at {mp})"));
                    }
                    while let Some(top) = stack.pop() {
                        if top.object_id().as_int() == Some(markobject_id) {
                            break;
                        }
                    }
                    numtopop = before_has_mark.unwrap_or(0);
                } else {
                    let msg = "no MARK exists on stack".to_string();
                    markmsg = Some(msg.clone());
                    errormsg = Some(msg);
                }
            }

            if matches!(
                op_name.as_str(),
                "PUT" | "BINPUT" | "LONG_BINPUT" | "MEMOIZE"
            ) {
                let memo_idx = if op_name == "MEMOIZE" {
                    let len = memo_dict.as_dict().map_or(0, |d| d.len()) as i64;
                    markmsg = Some(format!("(as {len})"));
                    Value::int(len)
                } else {
                    arg.clone()
                };
                let key = memo_idx.to_key();
                let already_defined = key
                    .as_ref()
                    .and_then(|k| memo_dict.as_dict().map(|d| d.contains_key(k)))
                    .unwrap_or(false);
                if already_defined {
                    let arg_r = render_value_repr(_interp, &arg)?;
                    errormsg = Some(format!("memo key {arg_r} already defined"));
                } else if stack.is_empty() {
                    errormsg = Some("stack is empty -- can't store into memo".to_string());
                } else if stack
                    .last()
                    .is_some_and(|v| v.object_id().as_int() == Some(markobject_id))
                {
                    errormsg = Some("can't store markobject in the memo".to_string());
                } else if let Some(k) = key {
                    memo_dict.dict_insert(k, stack.last().unwrap().clone())?;
                }
            } else if matches!(op_name.as_str(), "GET" | "BINGET" | "LONG_BINGET") {
                let found = arg
                    .to_key()
                    .and_then(|k| memo_dict.as_dict().and_then(|d| d.get(&k).cloned()));
                if let Some(memo_val) = found {
                    after = vec![memo_val];
                } else {
                    let arg_r = render_value_repr(_interp, &arg)?;
                    errormsg = Some(format!("memo key {arg_r} has never been stored into"));
                }
            }

            if !arg.is_none() || markmsg.is_some() {
                if op_name.len() < 10 {
                    line.push_str(&" ".repeat(10 - op_name.len()));
                }
                if !arg.is_none() {
                    let rendered = if matches!(
                        op_name.as_str(),
                        "STRING" | "BINSTRING" | "SHORT_BINSTRING"
                    ) {
                        ascii_repr_interp(_interp, &arg)?
                    } else {
                        render_value_repr(_interp, &arg)?
                    };
                    line.push(' ');
                    line.push_str(&rendered);
                }
                if let Some(m) = markmsg {
                    line.push(' ');
                    line.push_str(&m);
                }
            }

            if annotate_col > 0 {
                let line_len = line.chars().count();
                if annocol > line_len {
                    line.push_str(&" ".repeat(annocol - line_len));
                }
                annocol = line.chars().count();
                if annocol > 50 {
                    annocol = annotate_col;
                }
                let first_doc = op_doc.split('\n').next().unwrap_or("");
                line.push(' ');
                line.push_str(first_doc);
            }

            let full_line = format!("{prefix}{line}");
            write_line(_interp, &out_stream, &full_line)?;

            if let Some(err) = errormsg {
                return Err(PyError::named("ValueError", err));
            }

            if stack.len() < numtopop {
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "tries to pop {numtopop} items from stack with only {} items",
                        stack.len()
                    ),
                ));
            }
            if numtopop > 0 {
                let new_len = stack.len() - numtopop;
                stack.truncate(new_len);
            }
            if after
                .iter()
                .any(|v| v.object_id().as_int() == Some(markobject_id))
            {
                markstack.push(pos);
            }
            stack.extend(after);

            if is_stop {
                break;
            }
        }

        write_line(
            _interp,
            &out_stream,
            &format!("highest protocol among opcodes = {maxproto}"),
        )?;
        if !stack.is_empty() {
            let stack_r = render_value_repr(_interp, &Value::list(stack))?;
            return Err(PyError::named(
                "ValueError",
                format!("stack not empty after STOP: {stack_r}"),
            ));
        }
        Ok(Value::none())
    }

    fn optimize(p: PyBytesLike<'_>) -> Result<Value> {
        let raw = p.as_bytes().to_vec();
        let stream = wrap_pickle_source(_interp, &Value::bytes(raw.clone()))?;
        let getpos = Some(_interp.get_attr(&stream, "tell")?);
        let mod_val = _interp.load_module("pickletools")?;
        let code2op = _interp.get_attr(&mod_val, "code2op")?;

        enum OptOp {
            Put(i64),
            Get(i64),
            Raw(usize, usize),
        }

        let mut oldids: HashSet<i64> = HashSet::new();
        let mut newids: HashMap<i64, Option<usize>> = HashMap::new();
        let mut ops: Vec<OptOp> = Vec::new();
        let mut proto: i64 = 0;
        let mut protoheader: Vec<u8> = Vec::new();

        loop {
            let (opcode, arg, pos, end_pos, is_stop) =
                next_op_step(_interp, &stream, &getpos, &code2op)?;
            let op_name = _interp
                .get_attr(&opcode, "name")?
                .as_str()
                .unwrap_or("")
                .to_owned();
            let op_proto = _interp.get_attr(&opcode, "proto")?.as_int().unwrap_or(0);
            let p_start = pos.as_int().unwrap_or(0) as usize;
            let p_end = end_pos.as_int().unwrap_or(0) as usize;

            if op_name.contains("PUT") {
                let id = arg.as_int().unwrap_or(0);
                oldids.insert(id);
                ops.push(OptOp::Put(id));
            } else if op_name == "MEMOIZE" {
                let idx = oldids.len() as i64;
                oldids.insert(idx);
                ops.push(OptOp::Put(idx));
            } else if op_name.contains("FRAME") {
                // Dropped; re-framed on emit when proto >= 4.
            } else if op_name.contains("GET") {
                if op_proto > proto {
                    proto = op_proto;
                }
                let id = arg.as_int().unwrap_or(0);
                newids.insert(id, None);
                ops.push(OptOp::Get(id));
            } else if op_name == "PROTO" {
                let p_arg = arg.as_int().unwrap_or(0);
                if p_arg > proto {
                    proto = p_arg;
                }
                if p_start == 0 {
                    protoheader = raw[p_start..p_end].to_vec();
                } else {
                    ops.push(OptOp::Raw(p_start, p_end));
                }
            } else {
                ops.push(OptOp::Raw(p_start, p_end));
            }

            if is_stop {
                break;
            }
        }

        let mut out = Vec::with_capacity(raw.len());
        out.extend_from_slice(&protoheader);
        let framing = proto >= 4;
        let mut frame_buf: Vec<u8> = Vec::new();
        let mut next_idx: usize = 0;

        for op in ops {
            let (chunk, frameless): (Vec<u8>, bool) = match op {
                OptOp::Put(old_id) => {
                    if !newids.contains_key(&old_id) {
                        continue;
                    }
                    let data = encode_put(proto, next_idx);
                    newids.insert(old_id, Some(next_idx));
                    next_idx += 1;
                    (data, false)
                }
                OptOp::Get(old_id) => {
                    let Some(Some(mapped)) = newids.get(&old_id) else {
                        return Err(PyError::named(
                            "KeyError",
                            format!("{old_id}"),
                        ));
                    };
                    (encode_get(proto, *mapped), false)
                }
                OptOp::Raw(s, e) => {
                    let slice = raw[s..e].to_vec();
                    let too_big = slice.len() > FRAME_SIZE_TARGET;
                    (slice, too_big)
                }
            };

            if framing {
                commit_frame(&mut out, &mut frame_buf, frameless);
                if frameless {
                    out.extend_from_slice(&chunk);
                } else {
                    frame_buf.extend_from_slice(&chunk);
                }
            } else {
                out.extend_from_slice(&chunk);
            }
        }
        if framing {
            commit_frame(&mut out, &mut frame_buf, true);
        }

        Ok(Value::bytes(out))
    }

    // ── Descriptor & Iterator Classes ────────────────────────────────────────

    class ArgumentDescriptor {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            n: PyIndex,
            reader: PyValue,
            doc: PyStr<'_>,
        ) -> Result<Value> {
            let n_val = n.expect_i64(FN_NAME, "n")?;
            if !(n_val >= 0
                || matches!(
                    n_val,
                    UP_TO_NEWLINE
                        | TAKEN_FROM_ARGUMENT1
                        | TAKEN_FROM_ARGUMENT4
                        | TAKEN_FROM_ARGUMENT4U
                        | TAKEN_FROM_ARGUMENT8U
                ))
            {
                return Err(PyError::named("AssertionError", "invalid ArgumentDescriptor n"));
            }
            _interp.assign_attr(self_val.0.clone(), "name", Value::string(&*name))?;
            _interp.assign_attr(self_val.0.clone(), "n", Value::int(n_val))?;
            _interp.assign_attr(self_val.0.clone(), "reader", reader.0)?;
            _interp.assign_attr(self_val.0.clone(), "doc", Value::string(&*doc))?;
            Ok(Value::none())
        }
    }

    class StackObject {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            obtype: PyValue,
            doc: PyStr<'_>,
        ) -> Result<Value> {
            let type_cls = Value::py_class(type_class_singleton());
            let is_type = py_isinstance(_interp, &obtype.0, &type_cls)?;
            if !is_type {
                if let ValueKind::Tuple(items) = obtype.0.kind() {
                    for item in items.iter() {
                        if !py_isinstance(_interp, item, &type_cls)? {
                            return Err(PyError::named(
                                "AssertionError",
                                "StackObject obtype tuple elements must be types",
                            ));
                        }
                    }
                } else {
                    return Err(PyError::named(
                        "AssertionError",
                        format!(
                            "StackObject obtype must be a type or tuple of types, got '{}'",
                            value_type_name_str(&obtype.0)
                        ),
                    ));
                }
            }
            _interp.assign_attr(self_val.0.clone(), "name", Value::string(&*name))?;
            _interp.assign_attr(self_val.0.clone(), "obtype", obtype.0)?;
            _interp.assign_attr(self_val.0.clone(), "doc", Value::string(&*doc))?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "name")
        }
    }

    class OpcodeInfo {
        #[allow(clippy::too_many_arguments)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            code: PyStr<'_>,
            arg: PyValue,
            stack_before: PyValue,
            stack_after: PyValue,
            proto: PyIndex,
            doc: PyStr<'_>,
        ) -> Result<Value> {
            if code.chars().count() != 1 {
                return Err(PyError::named(
                    "AssertionError",
                    "OpcodeInfo code must be a single character",
                ));
            }
            let proto_val = proto.expect_i64(FN_NAME, "proto")?;
            if !(0..=5).contains(&proto_val) {
                return Err(PyError::named(
                    "AssertionError",
                    "OpcodeInfo proto out of range",
                ));
            }
            _interp.assign_attr(self_val.0.clone(), "name", Value::string(&*name))?;
            _interp.assign_attr(self_val.0.clone(), "code", Value::string(&*code))?;
            _interp.assign_attr(self_val.0.clone(), "arg", arg.0)?;
            _interp.assign_attr(self_val.0.clone(), "stack_before", stack_before.0)?;
            _interp.assign_attr(self_val.0.clone(), "stack_after", stack_after.0)?;
            _interp.assign_attr(self_val.0.clone(), "proto", Value::int(proto_val))?;
            _interp.assign_attr(self_val.0.clone(), "doc", Value::string(&*doc))?;
            Ok(Value::none())
        }
    }

    #[native_state(GenopsState)]
    class _GenopsIterator {
        iter_self;

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (done, stream, getpos, code2op) = with_state(self_val.0, |st| {
                Ok((
                    st.done,
                    st.stream.clone(),
                    st.getpos.clone(),
                    st.code2op.clone(),
                ))
            })?;
            if done {
                return Err(PyError::named("StopIteration", ""));
            }
            let (opcode, arg, pos, _end_pos, is_stop) =
                next_op_step(_interp, &stream, &getpos, &code2op)?;
            if is_stop {
                with_state_mut(self_val.0, |st| {
                    st.done = true;
                    Ok(())
                })?;
            }
            Ok(Value::tuple(vec![opcode, arg, pos]))
        }
    }
}
