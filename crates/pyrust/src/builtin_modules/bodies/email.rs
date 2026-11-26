// `email` package module — body for the `email @inject` entry in
// `pyrust_builtin_modules!` (issue #3211).
//
// Exposes `message_from_string`, `message_from_bytes`, `message_from_file`,
// and `message_from_binary_file` from `email_py.py`. Submodules (`utils`,
// `header`, `policy`, `message`, `parser`, etc.) are loaded lazily on explicit
// import, matching CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/email.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `email` package.
const EMAIL_PY_SOURCE: &str = include_str!("email_py.py");

/// Public names exported from `EMAIL_PY_SOURCE` onto `email`.
const EMAIL_PY_EXPORTS: &[&str] = &[
    "__all__",
    "message_from_string",
    "message_from_bytes",
    "message_from_file",
    "message_from_binary_file",
];

/// Execute `EMAIL_PY_SOURCE` once and copy its exports onto `email`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(EMAIL_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("email: exec namespace not a dict".into()))?;
    for name in EMAIL_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
