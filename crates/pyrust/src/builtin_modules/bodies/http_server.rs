// `http.server` module — body for the `"http.server" as http_server @inject`
// entry in `pyrust_builtin_modules!` (issue #3207).
//
// Pure-Python HTTP server classes (`HTTPServer`, `ThreadingHTTPServer`,
// `BaseHTTPRequestHandler`, `SimpleHTTPRequestHandler`) built directly on
// PyRust's `socket`, `selectors`, `http`, `http.client`, `html`, and
// `urllib.parse` modules.
//
// Reference: <https://docs.python.org/3/library/http.server.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `http.server` module.
const HTTP_SERVER_PY_SOURCE: &str = include_str!("http_server_py.py");

/// Public names exported from `HTTP_SERVER_PY_SOURCE` onto `http.server`.
const HTTP_SERVER_PY_EXPORTS: &[&str] = &[
    "__all__",
    "__version__",
    "DEFAULT_ERROR_MESSAGE",
    "DEFAULT_ERROR_CONTENT_TYPE",
    "HTTPServer",
    "ThreadingHTTPServer",
    "BaseHTTPRequestHandler",
    "SimpleHTTPRequestHandler",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(HTTP_SERVER_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("http.server: exec namespace not a dict".into()))?;
    for &name in HTTP_SERVER_PY_EXPORTS {
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
