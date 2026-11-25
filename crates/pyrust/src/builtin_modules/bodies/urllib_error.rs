// `urllib.error` module — body for the `"urllib.error" as urllib_error @inject`
// entry in `pyrust_builtin_modules!` (issue #3223).
//
// Pure-Python exception hierarchy (`URLError`, `HTTPError`,
// `ContentTooShortError`) aligned with CPython 3.12's `Lib/urllib/error.py`.
//
// Reference: <https://docs.python.org/3/library/urllib.error.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `urllib.error` module.
const URLLIB_ERROR_PY_SOURCE: &str = include_str!("urllib_error_py.py");

/// Public names exported from `URLLIB_ERROR_PY_SOURCE`.
const URLLIB_ERROR_PY_EXPORTS: &[&str] =
    &["__all__", "URLError", "HTTPError", "ContentTooShortError"];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(URLLIB_ERROR_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("urllib.error: exec namespace not a dict".into()))?;
    for &name in URLLIB_ERROR_PY_EXPORTS {
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
