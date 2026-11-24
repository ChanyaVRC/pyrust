// `html.entities` module — HTML character entity reference tables (issue #3149).
//
// Exposes `html5`, `name2codepoint`, `codepoint2name`, and `entitydefs` with
// exact CPython 3.12 contents and insertion order.
//
// Reference: <https://docs.python.org/3/library/html.entities.html>

use crate::value::{PyDict, PyKey, Value};
use pyrust_derive::pyrust_module;

#[path = "html_entities/tables.rs"]
mod tables;

fn build_all() -> Value {
    Value::list(vec![
        Value::string("html5"),
        Value::string("name2codepoint"),
        Value::string("codepoint2name"),
        Value::string("entitydefs"),
    ])
}

fn build_name2codepoint() -> Value {
    let mut dict = PyDict::default();
    for &(name, cp) in tables::NAME2CODEPOINT {
        dict.insert(PyKey::str_from(name), Value::int(cp));
    }
    Value::dict(dict)
}

fn build_html5() -> Value {
    let mut dict = PyDict::default();
    for &(name, value) in tables::HTML5 {
        dict.insert(PyKey::str_from(name), Value::string(value));
    }
    Value::dict(dict)
}

fn build_codepoint2name() -> Value {
    let mut dict = PyDict::default();
    for &(name, cp) in tables::NAME2CODEPOINT {
        dict.insert(PyKey::Int(cp), Value::string(name));
    }
    Value::dict(dict)
}

fn build_entitydefs() -> Value {
    let mut dict = PyDict::default();
    for &(name, cp) in tables::NAME2CODEPOINT {
        let ch = char::from_u32(cp as u32)
            .expect("NAME2CODEPOINT entries are valid Unicode scalar values");
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        dict.insert(PyKey::str_from(name), Value::string(s));
    }
    Value::dict(dict)
}

pyrust_module! {
    constants {
        "__all__" => build_all(),
        "name2codepoint" => build_name2codepoint(),
        "html5" => build_html5(),
        "codepoint2name" => build_codepoint2name(),
        "entitydefs" => build_entitydefs(),
    }
}
