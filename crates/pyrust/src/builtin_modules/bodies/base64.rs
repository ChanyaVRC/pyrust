// `base64` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/base64.py` (issues #3145, #3487).
//
// Like the Python source, the Base64 functions delegate to `binascii`
// (`b2a_base64` / `a2b_base64` / `hexlify` / `unhexlify`, looked up on the
// module's `binascii` global at call time) and the public functions call each
// other through the module namespace, so patching `base64.b64encode` or
// `base64.binascii` behaves as in CPython.  Base32, Base16, Ascii85 and
// Base85 and the file-object helpers `encode` / `decode` / `main` are ported
// statement by statement.
//
// Layout:
// - `base64/support.rs`: argument conversion, call helpers, exception builders
// - `base64/codecs.rs`: byte-level Base32 / Ascii85 / Base85 codecs
// - `base64/docs.rs`: docstrings generated from the reference interpreter
//
// Reference: <https://docs.python.org/3.12/library/base64.html>

#[path = "base64/codecs.rs"]
mod codecs;
#[path = "base64/docs.rs"]
mod docs;
#[path = "base64/support.rs"]
mod support;

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use self::codecs::{
    A85_END, A85_START, B32_ALPHABET, B32_HEX_ALPHABET, B85_ALPHABET, a85_chars, a85_decode,
    a85_frame, b32_decode, b32_encode, b85_decode, encode_85,
};
use self::support::{
    assertion_error, binascii_call, binascii_error, builtin_call, bytes_from_buffer,
    bytes_from_decode_data, call_method, call_pos, contains, len_of, module_attr, repr_of,
    translate, translation_table, type_error, value_error,
};
use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Bound, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBytesLike};
use crate::interpreter::{ConsumerIterator, ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyModule, Value, ValueKind};

const MAXLINESIZE: usize = 76;
const MAXBINSIZE: usize = (MAXLINESIZE / 4) * 3;

const SIG_B64ENCODE: Sig = Sig {
    qualname: "b64encode",
    params: &["s", "altchars"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_B64DECODE: Sig = Sig {
    qualname: "b64decode",
    params: &["s", "altchars", "validate"],
    defaults: 2,
    kwonly: &[],
    var_kw: false,
};
const SIG_STANDARD_B64ENCODE: Sig = Sig {
    qualname: "standard_b64encode",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_STANDARD_B64DECODE: Sig = Sig {
    qualname: "standard_b64decode",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_URLSAFE_B64ENCODE: Sig = Sig {
    qualname: "urlsafe_b64encode",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_URLSAFE_B64DECODE: Sig = Sig {
    qualname: "urlsafe_b64decode",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_B32ENCODE: Sig = Sig {
    qualname: "b32encode",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_B32DECODE: Sig = Sig {
    qualname: "b32decode",
    params: &["s", "casefold", "map01"],
    defaults: 2,
    kwonly: &[],
    var_kw: false,
};
const SIG_B32HEXENCODE: Sig = Sig {
    qualname: "b32hexencode",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_B32HEXDECODE: Sig = Sig {
    qualname: "b32hexdecode",
    params: &["s", "casefold"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_B16ENCODE: Sig = Sig {
    qualname: "b16encode",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_B16DECODE: Sig = Sig {
    qualname: "b16decode",
    params: &["s", "casefold"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_A85ENCODE: Sig = Sig {
    qualname: "a85encode",
    params: &["b"],
    defaults: 0,
    kwonly: &["foldspaces", "wrapcol", "pad", "adobe"],
    var_kw: false,
};
const SIG_A85DECODE: Sig = Sig {
    qualname: "a85decode",
    params: &["b"],
    defaults: 0,
    kwonly: &["foldspaces", "adobe", "ignorechars"],
    var_kw: false,
};
const SIG_B85ENCODE: Sig = Sig {
    qualname: "b85encode",
    params: &["b", "pad"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_B85DECODE: Sig = Sig {
    qualname: "b85decode",
    params: &["b"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_ENCODE: Sig = Sig {
    qualname: "encode",
    params: &["input", "output"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_DECODE: Sig = Sig {
    qualname: "decode",
    params: &["input", "output"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_BYTES_FROM_DECODE_DATA: Sig = Sig {
    qualname: "_bytes_from_decode_data",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_INPUT_TYPE_CHECK: Sig = Sig {
    qualname: "_input_type_check",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_ENCODEBYTES: Sig = Sig {
    qualname: "encodebytes",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_DECODEBYTES: Sig = Sig {
    qualname: "decodebytes",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_MAIN: Sig = Sig {
    qualname: "main",
    params: &[],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

/// The module namespace in the source's definition order.
const NAMESPACE_ORDER: &[&str] = &[
    "__doc__",
    "re",
    "struct",
    "binascii",
    "__all__",
    "bytes_types",
    "_bytes_from_decode_data",
    "b64encode",
    "b64decode",
    "standard_b64encode",
    "standard_b64decode",
    "_urlsafe_encode_translation",
    "_urlsafe_decode_translation",
    "urlsafe_b64encode",
    "urlsafe_b64decode",
    "_B32_ENCODE_DOCSTRING",
    "_B32_DECODE_DOCSTRING",
    "_B32_DECODE_MAP01_DOCSTRING",
    "_b32alphabet",
    "_b32hexalphabet",
    "b32encode",
    "b32decode",
    "b32hexencode",
    "b32hexdecode",
    "b16encode",
    "b16decode",
    "_A85START",
    "_A85END",
    "a85encode",
    "a85decode",
    "_b85alphabet",
    "b85encode",
    "b85decode",
    "MAXLINESIZE",
    "MAXBINSIZE",
    "encode",
    "decode",
    "_input_type_check",
    "encodebytes",
    "decodebytes",
    "main",
];

fn all_list() -> Value {
    Value::list(
        [
            "encode",
            "decode",
            "encodebytes",
            "decodebytes",
            "b64encode",
            "b64decode",
            "b32encode",
            "b32decode",
            "b32hexencode",
            "b32hexdecode",
            "b16encode",
            "b16decode",
            "b85encode",
            "b85decode",
            "a85encode",
            "a85decode",
            "standard_b64encode",
            "standard_b64decode",
            "urlsafe_b64encode",
            "urlsafe_b64decode",
        ]
        .into_iter()
        .map(Value::string)
        .collect(),
    )
}

/// `__doc__` of the `base64` callable `registry_key` (`base64.b64encode`, ...).
pub(crate) fn native_doc(registry_key: &str) -> Option<Option<&'static str>> {
    docs::function_doc(registry_key.strip_prefix("base64.")?)
}

/// `bytes.maketrans(from, to)` for the 256-byte `_urlsafe_*_translation`.
fn urlsafe_table(from: &[u8], to: &[u8]) -> Value {
    Value::bytes(translation_table(from, to).to_vec())
}

fn bytes_of(interp: &mut Interpreter, value: &Value, what: &str) -> Result<Vec<u8>> {
    if !PyBytesLike::matches(value) {
        return Err(type_error(format!(
            "a bytes-like object is required, not '{}'",
            value_type_name_str(value)
        )));
    }
    Ok(PyBytesLike::try_from_value_with_interp(interp, value, "base64", what)?.into_vec())
}

// ---------------------------------------------------------------------------
// Base64
// ---------------------------------------------------------------------------

fn b64encode_impl(interp: &mut Interpreter, s: Value, altchars: Value) -> Result<Value> {
    let encoded = binascii_call(
        interp,
        "b2a_base64",
        &[s],
        &[("newline", Value::bool_(false))],
    )?;
    if altchars.is_none() {
        return Ok(encoded);
    }
    // assert len(altchars) == 2, repr(altchars)
    if len_of(interp, &altchars)? != 2 {
        let repr = repr_of(interp, &altchars)?;
        return Err(assertion_error(repr));
    }
    // bytes.maketrans(b'+/', altchars)
    let alt = bytes_of(interp, &altchars, "altchars")?;
    let table = translation_table(b"+/", &alt);
    let data = bytes_of(interp, &encoded, "s")?;
    Ok(Value::bytes(translate(&data, &table)))
}

fn b64decode_impl(
    interp: &mut Interpreter,
    s: &Value,
    altchars: &Value,
    validate: Value,
) -> Result<Value> {
    let mut data = bytes_from_decode_data(interp, s)?;
    if !altchars.is_none() {
        let alt = bytes_from_decode_data(interp, altchars)?;
        if alt.len() != 2 {
            let repr = repr_of(interp, &Value::bytes(alt))?;
            return Err(assertion_error(repr));
        }
        data = translate(&data, &translation_table(&alt, b"+/"));
    }
    binascii_call(
        interp,
        "a2b_base64",
        &[Value::bytes(data)],
        &[("strict_mode", validate)],
    )
}

fn call_module_func(interp: &mut Interpreter, name: &str, args: &[Value]) -> Result<Value> {
    let func = module_attr(interp, name)?;
    call_pos(interp, func, args)
}

// ---------------------------------------------------------------------------
// Base32
// ---------------------------------------------------------------------------

fn b32decode_impl(
    interp: &mut Interpreter,
    alphabet: &[u8],
    s: &Value,
    casefold: &Value,
    map01: &Value,
) -> Result<Value> {
    let mut data = bytes_from_decode_data(interp, s)?;
    if !data.len().is_multiple_of(8) {
        return Err(binascii_error(interp, "Incorrect padding"));
    }
    // Handle section 2.4 zero and one mapping.  The flag map01 will be either
    // None, or the character to map the digit 1 (one) to.  It should be
    // either L (el) or I (eye).
    if !map01.is_none() {
        let mapped = bytes_from_decode_data(interp, map01)?;
        if mapped.len() != 1 {
            let repr = repr_of(interp, &Value::bytes(mapped))?;
            return Err(assertion_error(repr));
        }
        data = translate(&data, &translation_table(b"01", &[b'O', mapped[0]]));
    }
    if interp.truthy_value(casefold)? {
        data.make_ascii_uppercase();
    }
    match b32_decode(alphabet, &data) {
        Ok(decoded) => Ok(Value::bytes(decoded)),
        Err(failure) => Err(failure.into_error(interp)),
    }
}

// ---------------------------------------------------------------------------
// Ascii85 / Base85
// ---------------------------------------------------------------------------

fn a85encode_impl(interp: &mut Interpreter, b: &mut Bound) -> Result<Value> {
    let data_arg = b.take_or_none(0);
    let foldspaces = b.take_or(1, || Value::bool_(false));
    let wrapcol = b.take_or(2, || Value::int(0));
    let pad = b.take_or(3, || Value::bool_(false));
    let adobe = b.take_or(4, || Value::bool_(false));

    let data = bytes_from_buffer(interp, &data_arg)?;
    let pad = interp.truthy_value(&pad)?;
    let foldspaces = interp.truthy_value(&foldspaces)?;
    let result = encode_85(&data, &a85_chars(), pad, true, foldspaces);

    let adobe = interp.truthy_value(&adobe)?;
    let mut wrap = 0usize;
    if interp.truthy_value(&wrapcol)? {
        // wrapcol = max(2 if adobe else 1, wrapcol)
        let floor = Value::int(if adobe { 2 } else { 1 });
        let col = builtin_call(interp, "max", &[floor, wrapcol])?;
        wrap = match col.as_int() {
            Some(n) => n.max(0) as usize,
            None if value_type_name_str(&col) == "int" => usize::MAX,
            None => {
                return Err(type_error(format!(
                    "'{}' object cannot be interpreted as an integer",
                    value_type_name_str(&col)
                )));
            }
        };
    }
    Ok(Value::bytes(a85_frame(result, wrap, adobe)))
}

fn a85decode_impl(interp: &mut Interpreter, b: &mut Bound) -> Result<Value> {
    let data_arg = b.take_or_none(0);
    let foldspaces = b.take_or(1, || Value::bool_(false));
    let adobe = b.take_or(2, || Value::bool_(false));
    let ignorechars = b.take_or(3, || Value::bytes(vec![b' ', b'\t', b'\n', b'\r', 0x0b]));

    let mut data = bytes_from_decode_data(interp, &data_arg)?;
    if interp.truthy_value(&adobe)? {
        if !data.ends_with(A85_END) {
            return Err(value_error(
                "Ascii85 encoded byte sequences must end with b'~>'",
            ));
        }
        let end = data.len() - 2;
        let start = if data.starts_with(A85_START) { 2 } else { 0 };
        data = if start < end {
            data[start..end].to_vec()
        } else {
            Vec::new()
        };
    }
    let foldspaces = interp.truthy_value(&foldspaces)?;
    let decoded = if PyBytesLike::matches(&ignorechars) {
        let set = PyBytesLike::try_from_value_with_interp(
            interp,
            &ignorechars,
            "a85decode",
            "ignorechars",
        )?
        .into_vec();
        a85_decode(&data, foldspaces, |x| Ok(set.contains(&x)))
    } else {
        a85_decode(&data, foldspaces, |x| {
            contains(interp, Value::int(i64::from(x)), &ignorechars)
        })
    };
    match decoded {
        Ok(out) => Ok(Value::bytes(out)),
        Err(failure) => Err(failure.into_error(interp)),
    }
}

// ---------------------------------------------------------------------------
// Legacy interface
// ---------------------------------------------------------------------------

fn is_type_error(interp: &mut Interpreter, err: &PyError) -> Result<bool> {
    let exc = interp.materialize_pyerror(err.clone())?;
    let Some(cls) = crate::interpreter::lookup_exc_class("TypeError") else {
        return Ok(false);
    };
    let r = builtin_call(interp, "isinstance", &[exc, Value::py_class(cls)])?;
    interp.truthy_value(&r)
}

/// `_input_type_check(s)`.
fn input_type_check(interp: &mut Interpreter, s: &Value) -> Result<()> {
    if matches!(s.kind(), ValueKind::Bytes(_)) {
        return Ok(());
    }
    let class_name = value_type_name_str(s);
    let m = match builtin_call(interp, "memoryview", std::slice::from_ref(s)) {
        Ok(m) => m,
        Err(err) => {
            if !is_type_error(interp, &err)? {
                return Err(err);
            }
            let cause = interp.materialize_pyerror(err)?;
            let new = interp.materialize_pyerror(type_error(format!(
                "expected bytes-like object, not {class_name}"
            )))?;
            let _ = interp.assign_attr(new.clone(), "__cause__", cause);
            let _ = interp.assign_attr(new.clone(), "__suppress_context__", Value::bool_(true));
            return Err(PyError::raised(new));
        }
    };
    let format = interp.get_attr(&m, "format")?;
    let ndim = interp.get_attr(&m, "ndim")?;
    let result = (|| -> Result<()> {
        if !matches!(format.as_str(), Some("c" | "b" | "B")) {
            let repr = repr_of(interp, &format)?;
            return Err(type_error(format!(
                "expected single byte elements, not {repr} from {class_name}"
            )));
        }
        if ndim.as_int() != Some(1) {
            return Err(type_error(format!(
                "expected 1-D data, not {}-D data from {class_name}",
                ndim.as_int().unwrap_or(0)
            )));
        }
        Ok(())
    })();
    let _ = call_method(interp, &m, "release", &[]);
    result
}

/// `encode(input, output)`.
fn encode_file(interp: &mut Interpreter, input: &Value, output: &Value) -> Result<()> {
    let max = Value::int(MAXBINSIZE as i64);
    loop {
        let mut s = call_method(interp, input, "read", std::slice::from_ref(&max))?;
        if !interp.truthy_value(&s)? {
            break;
        }
        loop {
            let have = len_of(interp, &s)?;
            if have >= MAXBINSIZE {
                break;
            }
            let ns = call_method(
                interp,
                input,
                "read",
                &[Value::int((MAXBINSIZE - have) as i64)],
            )?;
            if !interp.truthy_value(&ns)? {
                break;
            }
            s = interp.eval_binary(s, BinaryOp::Add, ns)?;
        }
        let line = binascii_call(interp, "b2a_base64", &[s], &[])?;
        call_method(interp, output, "write", &[line])?;
    }
    Ok(())
}

/// `decode(input, output)`.
fn decode_file(interp: &mut Interpreter, input: &Value, output: &Value) -> Result<()> {
    loop {
        let line = call_method(interp, input, "readline", &[])?;
        if !interp.truthy_value(&line)? {
            break;
        }
        let s = binascii_call(interp, "a2b_base64", &[line], &[])?;
        call_method(interp, output, "write", &[s])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// main()
// ---------------------------------------------------------------------------

fn is_instance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> Result<bool> {
    let r = builtin_call(interp, "isinstance", &[obj.clone(), cls.clone()])?;
    interp.truthy_value(&r)
}

/// `sys.stdout = sys.stderr; print(*lines); sys.exit(2)`.
fn usage_exit(interp: &mut Interpreter, sys: &Value, lines: &[Value]) -> Result<Value> {
    let stderr = interp.get_attr(sys, "stderr")?;
    interp.assign_attr(sys.clone(), "stdout", stderr)?;
    for line in lines {
        builtin_call(interp, "print", std::slice::from_ref(line))?;
    }
    call_method(interp, sys, "exit", &[Value::int(2)])
}

fn run_main(interp: &mut Interpreter) -> Result<Value> {
    let getopt = interp.load_module("getopt")?;
    let sys = interp.load_module("sys")?;

    let argv = interp.get_attr(&sys, "argv")?;
    let argv0 = {
        let first =
            crate::builtin_modules::operator::getitem_value(interp, argv.clone(), Value::int(0))?;
        interp.render_value_as_str(&first)?
    };
    let usage = format!(
        "usage: {argv0} [-h|-d|-e|-u] [file|-]\n        -h: print this help message and exit\n        -d, -u: decode\n        -e: encode (default)"
    );
    // `sys.argv[1:]`
    let slice = crate::interpreter::make_slice_value(Some(Value::int(1)), None, None);
    let argv_tail = crate::builtin_modules::operator::getitem_value(interp, argv, slice)?;

    let getopt_fn = interp.get_attr(&getopt, "getopt")?;
    let parsed = match call_pos(interp, getopt_fn, &[argv_tail, Value::string("hdeu")]) {
        Ok(v) => v,
        Err(err) => {
            let exc = interp.materialize_pyerror(err.clone())?;
            let error_cls = interp.get_attr(&getopt, "error")?;
            if !is_instance(interp, &exc, &error_cls)? {
                return Err(err);
            }
            return usage_exit(interp, &sys, &[exc, Value::string(usage)]);
        }
    };
    let opts = parsed
        .as_tuple()
        .and_then(|items| items.first().cloned())
        .unwrap_or_else(Value::none);
    let args = parsed
        .as_tuple()
        .and_then(|items| items.get(1).cloned())
        .unwrap_or_else(Value::none);

    let mut func = "encode";
    let mut opts_iter = ConsumerIterator::new(interp, &opts)?;
    while let Some(pair) = opts_iter.next(interp)? {
        let o = pair
            .as_tuple()
            .and_then(|items| items.first().cloned())
            .unwrap_or_else(Value::none);
        match o.as_str() {
            Some("-e") => func = "encode",
            Some("-d") | Some("-u") => func = "decode",
            Some("-h") => {
                builtin_call(interp, "print", &[Value::string(usage)])?;
                return Ok(Value::none());
            }
            _ => {}
        }
    }

    let first_arg = if interp.truthy_value(&args)? {
        let first = crate::builtin_modules::operator::getitem_value(interp, args, Value::int(0))?;
        Some(first)
    } else {
        None
    };
    let stdout = interp.get_attr(&sys, "stdout")?;
    let outbuf = interp.get_attr(&stdout, "buffer")?;
    match first_arg {
        Some(path) if path.as_str() != Some("-") => {
            let fp = builtin_call(interp, "open", &[path, Value::string("rb")])?;
            let target = module_attr(interp, func)?;
            let result = call_pos(interp, target, &[fp.clone(), outbuf]);
            let closed = call_method(interp, &fp, "close", &[]);
            result?;
            closed?;
        }
        _ => {
            let stdin = interp.get_attr(&sys, "stdin")?;
            let inbuf = interp.get_attr(&stdin, "buffer")?;
            let target = module_attr(interp, func)?;
            call_pos(interp, target, &[inbuf, outbuf])?;
        }
    }
    Ok(Value::none())
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "__all__" => all_list(),
        "_urlsafe_encode_translation" => urlsafe_table(b"+/", b"-_"),
        "_urlsafe_decode_translation" => urlsafe_table(b"-_", b"+/"),
        "_B32_ENCODE_DOCSTRING" => Value::string(docs::_B32_ENCODE_DOCSTRING),
        "_B32_DECODE_DOCSTRING" => Value::string(docs::_B32_DECODE_DOCSTRING),
        "_B32_DECODE_MAP01_DOCSTRING" => Value::string(docs::_B32_DECODE_MAP01_DOCSTRING),
        "_b32alphabet" => Value::bytes(B32_ALPHABET.to_vec()),
        "_b32hexalphabet" => Value::bytes(B32_HEX_ALPHABET.to_vec()),
        "_A85START" => Value::bytes(A85_START.to_vec()),
        "_A85END" => Value::bytes(A85_END.to_vec()),
        "_b85alphabet" => Value::bytes(B85_ALPHABET.to_vec()),
        "MAXLINESIZE" => Value::int(MAXLINESIZE as i64),
        "MAXBINSIZE" => Value::int(MAXBINSIZE as i64),
    }

    fn _bytes_from_decode_data(args) -> Result<Value> {
        let mut b = args::bind(&SIG_BYTES_FROM_DECODE_DATA, args)?;
        let s = b.take_or_none(0);
        let converted = Value::bytes(bytes_from_decode_data(_interp, &s)?);
        // `bytes` / `bytearray` arguments are returned as they are.
        Ok(if support::is_bytes_type_instance(&s) { s } else { converted })
    }

    fn b64encode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B64ENCODE, args)?;
        let s = b.take_or_none(0);
        let altchars = b.take_or_none(1);
        b64encode_impl(_interp, s, altchars)
    }

    fn b64decode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B64DECODE, args)?;
        let s = b.take_or_none(0);
        let altchars = b.take_or_none(1);
        let validate = b.take_or(2, || Value::bool_(false));
        b64decode_impl(_interp, &s, &altchars, validate)
    }

    fn standard_b64encode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_STANDARD_B64ENCODE, args)?;
        let s = b.take_or_none(0);
        call_module_func(_interp, "b64encode", &[s])
    }

    fn standard_b64decode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_STANDARD_B64DECODE, args)?;
        let s = b.take_or_none(0);
        call_module_func(_interp, "b64decode", &[s])
    }

    fn urlsafe_b64encode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_URLSAFE_B64ENCODE, args)?;
        let s = b.take_or_none(0);
        let encoded = call_module_func(_interp, "b64encode", &[s])?;
        let data = bytes_of(_interp, &encoded, "s")?;
        Ok(Value::bytes(translate(&data, &translation_table(b"+/", b"-_"))))
    }

    fn urlsafe_b64decode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_URLSAFE_B64DECODE, args)?;
        let s = b.take_or_none(0);
        let data = bytes_from_decode_data(_interp, &s)?;
        let data = translate(&data, &translation_table(b"-_", b"+/"));
        call_module_func(_interp, "b64decode", &[Value::bytes(data)])
    }

    // Encode the bytes-like objects using base32 and return a bytes object.
    fn b32encode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B32ENCODE, args)?;
        let s = b.take_or_none(0);
        let data = bytes_from_buffer(_interp, &s)?;
        Ok(Value::bytes(b32_encode(B32_ALPHABET, &data)))
    }

    fn b32decode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B32DECODE, args)?;
        let s = b.take_or_none(0);
        let casefold = b.take_or(1, || Value::bool_(false));
        let map01 = b.take_or_none(2);
        b32decode_impl(_interp, B32_ALPHABET, &s, &casefold, &map01)
    }

    fn b32hexencode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B32HEXENCODE, args)?;
        let s = b.take_or_none(0);
        let data = bytes_from_buffer(_interp, &s)?;
        Ok(Value::bytes(b32_encode(B32_HEX_ALPHABET, &data)))
    }

    fn b32hexdecode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B32HEXDECODE, args)?;
        let s = b.take_or_none(0);
        let casefold = b.take_or(1, || Value::bool_(false));
        b32decode_impl(_interp, B32_HEX_ALPHABET, &s, &casefold, &Value::none())
    }

    fn b16encode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B16ENCODE, args)?;
        let s = b.take_or_none(0);
        let hex = binascii_call(_interp, "hexlify", &[s], &[])?;
        let mut data = bytes_of(_interp, &hex, "s")?;
        data.make_ascii_uppercase();
        Ok(Value::bytes(data))
    }

    fn b16decode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B16DECODE, args)?;
        let s = b.take_or_none(0);
        let casefold = b.take_or(1, || Value::bool_(false));
        let mut data = bytes_from_decode_data(_interp, &s)?;
        if _interp.truthy_value(&casefold)? {
            data.make_ascii_uppercase();
        }
        if data.iter().any(|c| !matches!(c, b'0'..=b'9' | b'A'..=b'F')) {
            return Err(binascii_error(_interp, "Non-base16 digit found"));
        }
        binascii_call(_interp, "unhexlify", &[Value::bytes(data)], &[])
    }

    fn a85encode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_A85ENCODE, args)?;
        a85encode_impl(_interp, &mut b)
    }

    fn a85decode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_A85DECODE, args)?;
        a85decode_impl(_interp, &mut b)
    }

    fn b85encode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B85ENCODE, args)?;
        let data_arg = b.take_or_none(0);
        let pad = b.take_or(1, || Value::bool_(false));
        let data = bytes_from_buffer(_interp, &data_arg)?;
        let pad = _interp.truthy_value(&pad)?;
        Ok(Value::bytes(encode_85(&data, B85_ALPHABET, pad, false, false)))
    }

    fn b85decode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_B85DECODE, args)?;
        let data_arg = b.take_or_none(0);
        let data = bytes_from_decode_data(_interp, &data_arg)?;
        match b85_decode(&data) {
            Ok(out) => Ok(Value::bytes(out)),
            Err(failure) => Err(failure.into_error(_interp)),
        }
    }

    // Encode a file; input and output are binary files.
    fn encode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_ENCODE, args)?;
        let input = b.take_or_none(0);
        let output = b.take_or_none(1);
        encode_file(_interp, &input, &output)?;
        Ok(Value::none())
    }

    // Decode a file; input and output are binary files.
    fn decode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DECODE, args)?;
        let input = b.take_or_none(0);
        let output = b.take_or_none(1);
        decode_file(_interp, &input, &output)?;
        Ok(Value::none())
    }

    fn _input_type_check(args) -> Result<Value> {
        let mut b = args::bind(&SIG_INPUT_TYPE_CHECK, args)?;
        let s = b.take_or_none(0);
        input_type_check(_interp, &s)?;
        Ok(Value::none())
    }

    fn encodebytes(args) -> Result<Value> {
        let mut b = args::bind(&SIG_ENCODEBYTES, args)?;
        let s = b.take_or_none(0);
        input_type_check(_interp, &s)?;
        let data = bytes_of(_interp, &s, "s")?;
        let mut pieces: Vec<u8> = Vec::with_capacity(data.len() / MAXBINSIZE * (MAXLINESIZE + 1) + 80);
        for chunk in data.chunks(MAXBINSIZE) {
            let line = binascii_call(_interp, "b2a_base64", &[Value::bytes(chunk.to_vec())], &[])?;
            pieces.extend_from_slice(&bytes_of(_interp, &line, "s")?);
        }
        Ok(Value::bytes(pieces))
    }

    fn decodebytes(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DECODEBYTES, args)?;
        let s = b.take_or_none(0);
        input_type_check(_interp, &s)?;
        binascii_call(_interp, "a2b_base64", &[s], &[])
    }

    // Small main program.
    fn main(args) -> Result<Value> {
        args::bind(&SIG_MAIN, args)?;
        run_main(_interp)
    }
}

/// Post-import wiring: `import re, struct, binascii`, `bytes_types`, and the
/// source's namespace order.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let re = interp.load_module("re")?;
    let struct_mod = interp.load_module("struct")?;
    let binascii = interp.load_module("binascii")?;
    let builtins = interp.load_module("builtins")?;
    let bytes_cls = interp.get_attr(&builtins, "bytes")?;
    let bytearray_cls = interp.get_attr(&builtins, "bytearray")?;
    let mut module = module.borrow_mut();
    module.insert_attr("re".to_string(), re);
    module.insert_attr("struct".to_string(), struct_mod);
    module.insert_attr("binascii".to_string(), binascii);
    module.insert_attr(
        "bytes_types".to_string(),
        Value::tuple(vec![bytes_cls, bytearray_cls]),
    );
    for name in NAMESPACE_ORDER {
        if let Some(value) = module.attrs.shift_remove(*name) {
            module.attrs.insert((*name).to_string(), value);
        }
    }
    Ok(())
}
