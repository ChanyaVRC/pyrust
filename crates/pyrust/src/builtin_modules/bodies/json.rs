// `json` package — 100% native Rust implementation of CPython 3.12.3's
// `Lib/json/__init__.py` (issues #2620, #3413).
//
// Like CPython, `import json` imports `json.decoder` (which imports
// `json.scanner`) and `json.encoder` and re-exports their `JSONDecoder`,
// `JSONDecodeError` and `JSONEncoder` classes (`post_load_init`), then builds
// `_default_encoder` / `_default_decoder`.  `dumps` / `dump` / `loads` /
// `load` keep the pure-Python control flow — the all-defaults fast path
// through the module's `_default_encoder` / `_default_decoder`, otherwise
// `cls(...)` — so `cls=` subclasses and `**kw` behave exactly as in CPython.
//
// The shared machinery lives in `bodies/json/`: `args` (CPython argument
// binding), `scan` (the `_json` scanner and `scanstring`), `encode` (the
// `_json` one-shot encoder and the lazy `_make_iterencode` generator),
// `support` (interpreter glue) and `docs` (CPython's docstrings).
//
// Reference: <https://docs.python.org/3/library/json.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

#[path = "json/args.rs"]
pub(crate) mod args;
#[path = "json/docs.rs"]
pub(crate) mod docs;
#[path = "json/encode.rs"]
pub(crate) mod encode;
#[path = "json/scan.rs"]
pub(crate) mod scan;
#[path = "json/support.rs"]
pub(crate) mod support;

use args::Sig;

/// `json.__all__`.
const ALL: [&str; 7] = [
    "dump",
    "dumps",
    "load",
    "loads",
    "JSONDecoder",
    "JSONDecodeError",
    "JSONEncoder",
];

/// The module namespace after the five module dunders, in CPython's order.
const NAMESPACE_ORDER: [&str; 21] = [
    "__version__",
    "__all__",
    "__author__",
    "scanner",
    "decoder",
    "JSONDecoder",
    "JSONDecodeError",
    "encoder",
    "JSONEncoder",
    "codecs",
    "_default_encoder",
    "dump",
    "dumps",
    "_default_decoder",
    "detect_encoding",
    "load",
    "loads",
    "__file__",
    "__cached__",
    "__builtins__",
    "__path__",
];

/// The keyword-only parameters shared by `dump` and `dumps`.
const ENCODE_KWONLY: &[&str] = &[
    "skipkeys",
    "ensure_ascii",
    "check_circular",
    "allow_nan",
    "cls",
    "indent",
    "separators",
    "default",
    "sort_keys",
];

/// The keyword-only parameters shared by `load` and `loads`.
const DECODE_KWONLY: &[&str] = &[
    "cls",
    "object_hook",
    "parse_float",
    "parse_int",
    "parse_constant",
    "object_pairs_hook",
];

const SIG_DUMPS: Sig = Sig {
    qualname: "dumps",
    params: &["obj"],
    defaults: 0,
    kwonly: ENCODE_KWONLY,
    var_kw: true,
};
const SIG_DUMP: Sig = Sig {
    qualname: "dump",
    params: &["obj", "fp"],
    defaults: 0,
    kwonly: ENCODE_KWONLY,
    var_kw: true,
};
const SIG_LOADS: Sig = Sig {
    qualname: "loads",
    params: &["s"],
    defaults: 0,
    kwonly: DECODE_KWONLY,
    var_kw: true,
};
const SIG_LOAD: Sig = Sig {
    qualname: "load",
    params: &["fp"],
    defaults: 0,
    kwonly: DECODE_KWONLY,
    var_kw: true,
};
const SIG_DETECT_ENCODING: Sig = Sig {
    qualname: "detect_encoding",
    params: &["b"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

/// A global of the `json` module, looked up at call time like the
/// pure-Python functions do.
fn json_global(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("json")?;
    interp.get_attr(&module, name)
}

/// The encoder `dump` / `dumps` use: `None` for the all-defaults fast path
/// (`_default_encoder`), otherwise `cls(skipkeys=..., ..., **kw)`.
fn custom_encoder(
    interp: &mut Interpreter,
    b: &mut args::Bound,
    base: usize,
) -> Result<Option<Value>> {
    let skipkeys = b.take_or(base, || Value::bool_(false));
    let ensure_ascii = b.take_or(base + 1, || Value::bool_(true));
    let check_circular = b.take_or(base + 2, || Value::bool_(true));
    let allow_nan = b.take_or(base + 3, || Value::bool_(true));
    let cls = b.take_or_none(base + 4);
    let indent = b.take_or_none(base + 5);
    let separators = b.take_or_none(base + 6);
    let default = b.take_or_none(base + 7);
    let sort_keys = b.take_or(base + 8, || Value::bool_(false));
    // `not skipkeys and ensure_ascii and check_circular and allow_nan and
    //  cls is None and indent is None and separators is None and
    //  default is None and not sort_keys and not kw`
    let fast = !interp.truthy_value(&skipkeys)?
        && interp.truthy_value(&ensure_ascii)?
        && interp.truthy_value(&check_circular)?
        && interp.truthy_value(&allow_nan)?
        && cls.is_none()
        && indent.is_none()
        && separators.is_none()
        && default.is_none()
        && !interp.truthy_value(&sort_keys)?
        && b.var_kw.is_empty();
    if fast {
        return Ok(None);
    }
    let cls = if cls.is_none() {
        json_global(interp, "JSONEncoder")?
    } else {
        cls
    };
    let mut keywords: Vec<(String, Value)> = vec![
        ("skipkeys".to_string(), skipkeys),
        ("ensure_ascii".to_string(), ensure_ascii),
        ("check_circular".to_string(), check_circular),
        ("allow_nan".to_string(), allow_nan),
        ("indent".to_string(), indent),
        ("separators".to_string(), separators),
        ("default".to_string(), default),
        ("sort_keys".to_string(), sort_keys),
    ];
    keywords.append(&mut b.var_kw);
    support::call_kw(interp, cls, &[], &keywords).map(Some)
}

/// `json.dumps(obj, *, skipkeys=False, ..., **kw)`.
fn dumps_impl(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_DUMPS, call_args)?;
    let obj = b.take_or_none(0);
    let encoder = match custom_encoder(interp, &mut b, 1)? {
        Some(encoder) => encoder,
        None => json_global(interp, "_default_encoder")?,
    };
    support::call_method(interp, &encoder, "encode", &[obj])
}

/// `json.dump(obj, fp, *, skipkeys=False, ..., **kw)`.
fn dump_impl(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_DUMP, call_args)?;
    let obj = b.take_or_none(0);
    let fp = b.take_or_none(1);
    let encoder = match custom_encoder(interp, &mut b, 2)? {
        Some(encoder) => encoder,
        None => json_global(interp, "_default_encoder")?,
    };
    let iterable = support::call_method(interp, &encoder, "iterencode", &[obj])?;
    let iterator = crate::interpreter::make_iterator(interp, &iterable)?;
    loop {
        let chunk = match interp.call_next(&iterator, None) {
            Ok(chunk) => chunk,
            Err(err) if crate::interpreter::is_stop_iteration_error(&err) => break,
            Err(err) => return Err(err),
        };
        support::call_method(interp, &fp, "write", &[chunk])?;
    }
    Ok(Value::none())
}

/// The first four bytes and the length of a `bytes` / `bytearray` value (or
/// subclass instance).
fn bytes_prefix(value: &Value) -> Option<([u8; 4], usize)> {
    let take = |data: &[u8]| {
        let mut head = [0u8; 4];
        let n = data.len().min(4);
        head[..n].copy_from_slice(&data[..n]);
        (head, data.len())
    };
    let value = support::unwrap_backing(value);
    if let ValueKind::Bytes(data) = value.kind() {
        return Some(take(data));
    }
    pyrust_builtins::bytearray::as_bytearray_rc(&value).map(|data| take(&data.borrow()))
}

const BOM_UTF8: &[u8] = b"\xef\xbb\xbf";
const BOM_UTF16_LE: &[u8] = b"\xff\xfe";
const BOM_UTF16_BE: &[u8] = b"\xfe\xff";
const BOM_UTF32_LE: &[u8] = b"\xff\xfe\x00\x00";
const BOM_UTF32_BE: &[u8] = b"\x00\x00\xfe\xff";

/// `detect_encoding` on raw bytes.
fn detect_encoding_bytes(head: &[u8; 4], len: usize) -> &'static str {
    let b = &head[..len.min(4)];
    if b.starts_with(BOM_UTF32_BE) || b.starts_with(BOM_UTF32_LE) {
        return "utf-32";
    }
    if b.starts_with(BOM_UTF16_BE) || b.starts_with(BOM_UTF16_LE) {
        return "utf-16";
    }
    if b.starts_with(BOM_UTF8) {
        return "utf-8-sig";
    }
    if len >= 4 {
        if b[0] == 0 {
            return if b[1] != 0 { "utf-16-be" } else { "utf-32-be" };
        }
        if b[1] == 0 {
            return if b[2] != 0 || b[3] != 0 {
                "utf-16-le"
            } else {
                "utf-32-le"
            };
        }
    } else if len == 2 {
        if b[0] == 0 {
            return "utf-16-be";
        }
        if b[1] == 0 {
            return "utf-16-le";
        }
    }
    "utf-8"
}

/// `json.detect_encoding(b)`: native for `bytes` / `bytearray`, otherwise
/// the pure-Python steps (`b.startswith`, `len(b)`, `b[i]`) on the object.
fn detect_encoding_impl(interp: &mut Interpreter, b: &Value) -> Result<&'static str> {
    if let Some((head, len)) = bytes_prefix(b) {
        return Ok(detect_encoding_bytes(&head, len));
    }
    let startswith = interp.get_attr(b, "startswith")?;
    let bom =
        |items: &[&[u8]]| Value::tuple(items.iter().map(|i| Value::bytes(i.to_vec())).collect());
    for (boms, name) in [
        (bom(&[BOM_UTF32_BE, BOM_UTF32_LE]), "utf-32"),
        (bom(&[BOM_UTF16_BE, BOM_UTF16_LE]), "utf-16"),
        (Value::bytes(BOM_UTF8.to_vec()), "utf-8-sig"),
    ] {
        let hit = support::call(interp, startswith.clone(), &[boms])?;
        if interp.truthy_value(&hit)? {
            return Ok(name);
        }
    }
    let len_fn = support::builtin(interp, "len")?;
    let len = support::call(interp, len_fn, std::slice::from_ref(b))?;
    let len = support::ssize_index(interp, &len)?;
    let item = |interp: &mut Interpreter, i: i64| -> Result<bool> {
        let v = crate::builtin_modules::operator::getitem_value(interp, b.clone(), Value::int(i))?;
        interp.truthy_value(&v)
    };
    if len >= 4 {
        if !item(interp, 0)? {
            return Ok(if item(interp, 1)? {
                "utf-16-be"
            } else {
                "utf-32-be"
            });
        }
        if !item(interp, 1)? {
            return Ok(if item(interp, 2)? || item(interp, 3)? {
                "utf-16-le"
            } else {
                "utf-32-le"
            });
        }
    } else if len == 2 {
        if !item(interp, 0)? {
            return Ok("utf-16-be");
        }
        if !item(interp, 1)? {
            return Ok("utf-16-le");
        }
    }
    Ok("utf-8")
}

/// `isinstance(s, (bytes, bytearray))`.
fn is_bytes_or_bytearray(value: &Value) -> bool {
    bytes_prefix(value).is_some()
}

/// `json.loads(s, *, cls=None, ..., **kw)`.
fn loads_impl(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_LOADS, call_args)?;
    let mut s = b.take_or_none(0);
    let cls = b.take_or_none(1);
    let object_hook = b.take_or_none(2);
    let parse_float = b.take_or_none(3);
    let parse_int = b.take_or_none(4);
    let parse_constant = b.take_or_none(5);
    let object_pairs_hook = b.take_or_none(6);
    let mut kw = std::mem::take(&mut b.var_kw);
    if let Some(text) = support::str_text(&s) {
        if text.starts_with('\u{feff}') {
            return Err(support::decode_error(
                interp,
                "Unexpected UTF-8 BOM (decode using utf-8-sig)",
                &s,
                Value::int(0),
            ));
        }
    } else {
        if !is_bytes_or_bytearray(&s) {
            let name = support::class_name(interp, &s)?;
            return Err(args::type_error(format!(
                "the JSON object must be str, bytes or bytearray, not {name}"
            )));
        }
        let encoding = detect_encoding_impl(interp, &s)?;
        s = support::call_method(
            interp,
            &s,
            "decode",
            &[Value::string(encoding), Value::string("surrogatepass")],
        )?;
    }
    if cls.is_none()
        && object_hook.is_none()
        && parse_int.is_none()
        && parse_float.is_none()
        && parse_constant.is_none()
        && object_pairs_hook.is_none()
        && kw.is_empty()
    {
        let decoder = json_global(interp, "_default_decoder")?;
        return support::call_method(interp, &decoder, "decode", &[s]);
    }
    let cls = if cls.is_none() {
        json_global(interp, "JSONDecoder")?
    } else {
        cls
    };
    for (name, value) in [
        ("object_hook", object_hook),
        ("object_pairs_hook", object_pairs_hook),
        ("parse_float", parse_float),
        ("parse_int", parse_int),
        ("parse_constant", parse_constant),
    ] {
        if !value.is_none() {
            kw.push((name.to_string(), value));
        }
    }
    let decoder = support::call_kw(interp, cls, &[], &kw)?;
    support::call_method(interp, &decoder, "decode", &[s])
}

/// `json.load(fp, *, cls=None, ..., **kw)`: `loads(fp.read(), ...)`.
fn load_impl(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_LOAD, call_args)?;
    let fp = b.take_or_none(0);
    let mut keywords: Vec<(String, Value)> = DECODE_KWONLY
        .iter()
        .enumerate()
        .map(|(i, name)| (name.to_string(), b.take_or_none(i + 1)))
        .collect();
    keywords.append(&mut b.var_kw);
    let text = support::call_method(interp, &fp, "read", &[])?;
    let loads = json_global(interp, "loads")?;
    support::call_kw(interp, loads, &[text], &keywords)
}

/// Post-import hook: `from .decoder import JSONDecoder, JSONDecodeError`,
/// `from .encoder import JSONEncoder`, `import codecs`, the default
/// encoder/decoder, and CPython's namespace order.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let decoder = interp.load_module("json.decoder")?;
    let scanner = interp.load_module("json.scanner")?;
    let json_decoder = interp.get_attr(&decoder, "JSONDecoder")?;
    let json_decode_error = interp.get_attr(&decoder, "JSONDecodeError")?;
    let encoder = interp.load_module("json.encoder")?;
    let json_encoder = interp.get_attr(&encoder, "JSONEncoder")?;
    let codecs = interp.load_module("codecs")?;
    let default_encoder = support::call_kw(
        interp,
        json_encoder.clone(),
        &[],
        &[
            ("skipkeys".to_string(), Value::bool_(false)),
            ("ensure_ascii".to_string(), Value::bool_(true)),
            ("check_circular".to_string(), Value::bool_(true)),
            ("allow_nan".to_string(), Value::bool_(true)),
            ("indent".to_string(), Value::none()),
            ("separators".to_string(), Value::none()),
            ("default".to_string(), Value::none()),
        ],
    )?;
    let default_decoder = support::call_kw(
        interp,
        json_decoder.clone(),
        &[],
        &[
            ("object_hook".to_string(), Value::none()),
            ("object_pairs_hook".to_string(), Value::none()),
        ],
    )?;
    let mut m = module.borrow_mut();
    for (name, value) in [
        ("scanner", scanner),
        ("decoder", decoder),
        ("JSONDecoder", json_decoder),
        ("JSONDecodeError", json_decode_error),
        ("encoder", encoder),
        ("JSONEncoder", json_encoder),
        ("codecs", codecs),
        ("_default_encoder", default_encoder),
        ("_default_decoder", default_decoder),
    ] {
        m.insert_attr(name.to_string(), value);
    }
    for name in NAMESPACE_ORDER {
        if let Some(value) = m.attrs.shift_remove(name) {
            m.attrs.insert(name.to_string(), value);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::JSON_DOC),
        "__package__" => Value::string("json"),
        "__version__" => Value::string("2.0.9"),
        "__all__" => Value::list(ALL.into_iter().map(Value::string).collect()),
        "__author__" => Value::string("Bob Ippolito <bob@redivi.com>"),
    }

    /// Serialize ``obj`` as a JSON formatted stream to ``fp`` (a
    /// ``.write()``-supporting file-like object).
    fn dump(args) -> Result<Value> {
        dump_impl(_interp, args)
    }

    /// Serialize ``obj`` to a JSON formatted ``str``.
    fn dumps(args) -> Result<Value> {
        dumps_impl(_interp, args)
    }

    fn detect_encoding(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DETECT_ENCODING, args)?;
        let data = b.take_or_none(0);
        detect_encoding_impl(_interp, &data).map(Value::string)
    }

    /// Deserialize ``fp`` (a ``.read()``-supporting file-like object containing
    /// a JSON document) to a Python object.
    fn load(args) -> Result<Value> {
        load_impl(_interp, args)
    }

    /// Deserialize ``s`` (a ``str``, ``bytes`` or ``bytearray`` instance
    /// containing a JSON document) to a Python object.
    fn loads(args) -> Result<Value> {
        loads_impl(_interp, args)
    }
}
