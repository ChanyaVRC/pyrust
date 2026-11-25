// `getopt` module — body for the `getopt @inject` entry in
// `pyrust_builtin_modules!` (issue #3230).
//
// CPython 3.12's `getopt` is a pure-Python module providing C-style and GNU-style
// command-line option parsing (`getopt`, `gnu_getopt`, `GetoptError`, `error`).
//
// Reference: <https://docs.python.org/3/library/getopt.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `getopt` module.
const GETOPT_PY_SOURCE: &str = include_str!("getopt_py.py");

/// Public names exported from `GETOPT_PY_SOURCE` onto the `getopt` module.
const GETOPT_PY_EXPORTS: &[&str] = &[
    "__doc__",
    "__all__",
    "GetoptError",
    "error",
    "getopt",
    "gnu_getopt",
    "do_longs",
    "long_has_args",
    "do_shorts",
    "short_has_arg",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(GETOPT_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("getopt: exec namespace not a dict".into()))?;
    for name in GETOPT_PY_EXPORTS {
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
