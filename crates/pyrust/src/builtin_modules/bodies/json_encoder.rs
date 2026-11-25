// `json.encoder` submodule — 100% native Rust implementation of Python 3.12's
// `json.encoder` module (issue #3251).
//
// Exposes `JSONEncoder`, `encode_basestring`, `encode_basestring_ascii`,
// `py_encode_basestring`, `py_encode_basestring_ascii`, `INFINITY`, and
// `ESCAPE_DCT`. Shares the canonical `JSONEncoder` class with `json`.
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/json/encoder.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::PyStr;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value};

fn encode_string_impl(s: &str, ensure_ascii: bool) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if ensure_ascii && (c as u32) > 0x7f => {
                use std::fmt::Write;
                let cp = c as u32;
                if cp > 0xffff {
                    let v = cp - 0x10000;
                    let hi = 0xd800 + (v >> 10);
                    let lo = 0xdc00 + (v & 0x3ff);
                    let _ = write!(out, "\\u{hi:04x}\\u{lo:04x}");
                } else {
                    let _ = write!(out, "\\u{cp:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

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

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let json_mod = interp.load_module("json")?;
    if let Ok(cls) = interp.get_attr(&json_mod, "JSONEncoder") {
        module
            .borrow_mut()
            .insert_attr("JSONEncoder".to_string(), cls);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("json"),
        "__all__" => Value::list(vec![Value::string("JSONEncoder")]),
        "INFINITY" => Value::float(f64::INFINITY),
        "ESCAPE_DCT" => make_escape_dct(),
        "JSONEncoder" => Value::none(),
    }

    fn encode_basestring(s: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(encode_string_impl(s.0.as_ref(), false)))
    }

    fn py_encode_basestring(s: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(encode_string_impl(s.0.as_ref(), false)))
    }

    fn c_encode_basestring(s: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(encode_string_impl(s.0.as_ref(), false)))
    }

    fn encode_basestring_ascii(s: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(encode_string_impl(s.0.as_ref(), true)))
    }

    fn py_encode_basestring_ascii(s: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(encode_string_impl(s.0.as_ref(), true)))
    }

    fn c_encode_basestring_ascii(s: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(encode_string_impl(s.0.as_ref(), true)))
    }
}
