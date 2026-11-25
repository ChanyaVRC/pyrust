// `http.cookies` module — body for the `"http.cookies" as http_cookies @inject`
// entry in `pyrust_builtin_modules!` (issue #3207).
//
// Pure-Python implementation of Python 3.12's `http.cookies` (`CookieError`,
// `Morsel`, `BaseCookie`, `SimpleCookie`).
//
// Reference: <https://docs.python.org/3/library/http.cookies.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `http.cookies` module.
const HTTP_COOKIES_PY_SOURCE: &str = include_str!("http_cookies_py.py");

/// Public names exported from `HTTP_COOKIES_PY_SOURCE` onto `http.cookies`.
const HTTP_COOKIES_PY_EXPORTS: &[&str] = &[
    "__all__",
    "CookieError",
    "Morsel",
    "BaseCookie",
    "SimpleCookie",
    "_quote",
    "_unquote",
    "_getdate",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(HTTP_COOKIES_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("http.cookies: exec namespace not a dict".into()))?;
    for &name in HTTP_COOKIES_PY_EXPORTS {
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
