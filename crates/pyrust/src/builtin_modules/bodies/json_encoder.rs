// `json.encoder` submodule — 100% native Rust implementation of Python 3.12's
// `json.encoder` module (issues #3251, #3413).
//
// Defines the canonical `JSONEncoder` class (re-exported by `json`, like
// CPython's `from .encoder import JSONEncoder`), plus `encode_basestring`,
// `encode_basestring_ascii`, their `py_` / `c_` aliases, `INFINITY` and
// `ESCAPE_DCT`.  The encoding algorithms live in `bodies/json/encode.rs`:
// `encode()` takes `_json.c`'s one-shot path and `iterencode()` returns a
// lazy generator with `_make_iterencode`'s exact chunking.  Like the
// pure-Python class, every method dispatches through `self`, so subclasses
// may override `default`, `encode` or `iterencode`.
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/json/encoder.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::json::{docs, encode, support};
use crate::error::Result;
use crate::interpreter::builtin_args::PyStr;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

fn make_escape_dct() -> Value {
    let mut d = PyDict::default();
    for (k, v) in [
        ("\\", "\\\\"),
        ("\"", "\\\""),
        ("\x08", "\\b"),
        ("\x0c", "\\f"),
        ("\n", "\\n"),
        ("\r", "\\r"),
        ("\t", "\\t"),
    ] {
        d.insert(PyKey::str_from(k), Value::string(v));
    }
    for i in 0u8..0x20 {
        let key = (i as char).to_string();
        if !d.contains_key(&PyKey::str_from(&key)) {
            d.insert(PyKey::str_from(&key), Value::string(format!("\\u{i:04x}")));
        }
    }
    Value::dict(d)
}

const SIG_INIT: Sig = Sig {
    qualname: "JSONEncoder.__init__",
    params: &["self"],
    defaults: 0,
    kwonly: &[
        "skipkeys",
        "ensure_ascii",
        "check_circular",
        "allow_nan",
        "sort_keys",
        "indent",
        "separators",
        "default",
    ],
    var_kw: false,
};
const SIG_DEFAULT: Sig = Sig {
    qualname: "JSONEncoder.default",
    params: &["self", "o"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_ENCODE: Sig = Sig {
    qualname: "JSONEncoder.encode",
    params: &["self", "o"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_ITERENCODE: Sig = Sig {
    qualname: "JSONEncoder.iterencode",
    params: &["self", "o", "_one_shot"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};

/// `JSONEncoder.__init__`.
fn encoder_init(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_INIT, call_args)?;
    let this = b.take_or_none(0);
    let values = [
        ("skipkeys", b.take_or(1, || Value::bool_(false))),
        ("ensure_ascii", b.take_or(2, || Value::bool_(true))),
        ("check_circular", b.take_or(3, || Value::bool_(true))),
        ("allow_nan", b.take_or(4, || Value::bool_(true))),
        ("sort_keys", b.take_or(5, || Value::bool_(false))),
    ];
    let indent = b.take_or_none(6);
    let separators = b.take_or_none(7);
    let default = b.take_or_none(8);
    for (name, value) in values {
        interp.assign_attr(this.clone(), name, value)?;
    }
    interp.assign_attr(this.clone(), "indent", indent.clone())?;
    if !separators.is_none() {
        let (item_separator, key_separator) = support::unpack2(interp, &separators)?;
        interp.assign_attr(this.clone(), "item_separator", item_separator)?;
        interp.assign_attr(this.clone(), "key_separator", key_separator)?;
    } else if !indent.is_none() {
        interp.assign_attr(this.clone(), "item_separator", Value::string(","))?;
    }
    if !default.is_none() {
        interp.assign_attr(this, "default", default)?;
    }
    Ok(Value::none())
}

/// `JSONEncoder.encode`.
fn encoder_encode(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_ENCODE, call_args)?;
    let this = b.take_or_none(0);
    let o = b.take_or_none(1);
    if let Some(text) = support::str_text(&o) {
        let ensure_ascii = interp.get_attr(&this, "ensure_ascii")?;
        let ensure_ascii = interp.truthy_value(&ensure_ascii)?;
        return Ok(Value::string(encode::escape(&text, ensure_ascii)));
    }
    let iterencode = interp.get_attr(&this, "iterencode")?;
    let chunks = support::call_kw(
        interp,
        iterencode,
        &[o],
        &[("_one_shot".to_string(), Value::bool_(true))],
    )?;
    let backing = support::unwrap_backing(&chunks);
    let items: Vec<Value> = match backing.kind() {
        ValueKind::List(items) => items.clone(),
        ValueKind::Tuple(items) => items.to_vec(),
        _ => support::collect(interp, &chunks)?,
    };
    support::join_chunks(&items)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let re_mod = interp.load_module("re")?;
    let mut m = module.borrow_mut();
    // `from _json import encode_basestring as c_encode_basestring`, then
    // `encode_basestring = (c_encode_basestring or py_encode_basestring)`.
    for (alias, source) in [
        ("c_encode_basestring", "encode_basestring"),
        ("c_encode_basestring_ascii", "encode_basestring_ascii"),
    ] {
        if let Some(value) = m.get_attr_value(source) {
            m.insert_attr(alias.to_string(), value);
        }
    }
    m.insert_attr("re".to_string(), re_mod);
    let order = [
        "re",
        "c_encode_basestring_ascii",
        "c_encode_basestring",
        "ESCAPE_DCT",
        "INFINITY",
        "py_encode_basestring",
        "encode_basestring",
        "py_encode_basestring_ascii",
        "encode_basestring_ascii",
        "JSONEncoder",
    ];
    for name in order {
        if let Some(value) = m.attrs.shift_remove(name) {
            m.attrs.insert(name.to_string(), value);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::JSON_ENCODER_MODULE_DOC),
        "__package__" => Value::string("json"),
        "ESCAPE_DCT" => make_escape_dct(),
        "INFINITY" => Value::float(f64::INFINITY),
    }

    // `_json.encode_basestring_ascii` / `_json.encode_basestring`, bound as
    // `c_encode_basestring_ascii` / `c_encode_basestring` by `post_load_init`.
    #[py_name = "encode_basestring_ascii"]
    fn c_encode_basestring_ascii(#[positional_only] s: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::string(encode::escape(s.0.as_ref(), true)))
    }

    #[py_name = "encode_basestring"]
    fn c_encode_basestring(#[positional_only] s: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::string(encode::escape(s.0.as_ref(), false)))
    }

    /// Return a JSON representation of a Python string
    fn py_encode_basestring(s: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::string(encode::escape(s.0.as_ref(), false)))
    }

    /// Return an ASCII-only JSON representation of a Python string
    fn py_encode_basestring_ascii(s: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::string(encode::escape(s.0.as_ref(), true)))
    }

    class JSONEncoder {
        constants {
            "__doc__" => Value::string(docs::JSON_ENCODER_DOC),
            "item_separator" => Value::string(", "),
            "key_separator" => Value::string(": "),
        }

        fn __init__(args) -> Result<Value> {
            encoder_init(_interp, args)
        }

        fn default(args) -> Result<Value> {
            let mut b = args::bind(&SIG_DEFAULT, args)?;
            let o = b.take_or_none(1);
            let name = support::class_name(_interp, &o)?;
            Err(args::type_error(format!(
                "Object of type {name} is not JSON serializable"
            )))
        }

        fn encode(args) -> Result<Value> {
            encoder_encode(_interp, args)
        }

        fn iterencode(args) -> Result<Value> {
            let mut b = args::bind(&SIG_ITERENCODE, args)?;
            let this = b.take_or_none(0);
            let o = b.take_or_none(1);
            let one_shot = match b.take(2) {
                Some(flag) => _interp.truthy_value(&flag)?,
                None => false,
            };
            encode::iterencode(_interp, &this, o, one_shot)
        }
    }
}
