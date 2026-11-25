// `netrc` module — body for the `netrc @inject` entry in
// `pyrust_builtin_modules!` (issue #3230).
//
// CPython 3.12's `netrc` is a pure-Python module providing an object-oriented
// parser for `.netrc` credential files (`netrc` and `NetrcParseError`).
//
// Reference: <https://docs.python.org/3/library/netrc.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `netrc` module.
const NETRC_PY_SOURCE: &str = include_str!("netrc_py.py");

/// Public names exported from `NETRC_PY_SOURCE` onto the `netrc` module.
const NETRC_PY_EXPORTS: &[&str] = &[
    "__doc__",
    "__all__",
    "NetrcParseError",
    "_netrclex",
    "netrc",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(NETRC_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("netrc: exec namespace not a dict".into()))?;
    for name in NETRC_PY_EXPORTS {
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
