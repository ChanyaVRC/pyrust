// `http` package module — body for the `http @inject` entry in
// `pyrust_builtin_modules!` (issue #3207).
//
// Exposes `HTTPStatus` and `HTTPMethod` from `http_py.py`. Submodules
// (`client`, `server`, `cookies`, `cookiejar`) are loaded lazily on explicit
// import, matching CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/http.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `http` module.
const HTTP_PY_SOURCE: &str = include_str!("http_py.py");

/// Public names exported from `HTTP_PY_SOURCE` onto `http`.
const HTTP_PY_EXPORTS: &[&str] = &["__all__", "HTTPStatus", "HTTPMethod"];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(HTTP_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("http: exec namespace not a dict".into()))?;
    for &name in HTTP_PY_EXPORTS {
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
