// `quopri` module — body for the `quopri @inject` entry in
// `pyrust_builtin_modules!` (issue #3230).
//
// CPython 3.12's `quopri` is a pure-Python module implementing RFC 1521/1522
// quoted-printable transport encoding (`encode`, `decode`, `encodestring`,
// `decodestring`) backed by `binascii.b2a_qp` / `binascii.a2b_qp`.
//
// Reference: <https://docs.python.org/3/library/quopri.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `quopri` module.
const QUOPRI_PY_SOURCE: &str = include_str!("quopri_py.py");

/// Public names exported from `QUOPRI_PY_SOURCE` onto the `quopri` module.
const QUOPRI_PY_EXPORTS: &[&str] = &[
    "__doc__",
    "__all__",
    "ESCAPE",
    "MAXLINESIZE",
    "HEX",
    "EMPTYSTRING",
    "a2b_qp",
    "b2a_qp",
    "needsquoting",
    "quote",
    "encode",
    "encodestring",
    "decode",
    "decodestring",
    "ishex",
    "unhex",
    "main",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(QUOPRI_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("quopri: exec namespace not a dict".into()))?;
    for name in QUOPRI_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
