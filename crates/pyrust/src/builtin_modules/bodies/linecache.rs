// `linecache` module — body for the `linecache @inject` entry in
// `pyrust_builtin_modules!` (issue #3229).
//
// Python 3.12's `linecache` is a pure-Python source-line cache backed by
// `os`, `sys`, `functools`, and `tokenize.open` (`cache`, `getline`,
// `getlines`, `clearcache`, `checkcache`, `updatecache`, `lazycache`).
//
// Reference: <https://docs.python.org/3.12/library/linecache.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `linecache` module.
const LINECACHE_PY_SOURCE: &str = include_str!("linecache_py.py");

/// Public names exported from `LINECACHE_PY_SOURCE` onto the `linecache` module.
const LINECACHE_PY_EXPORTS: &[&str] = &[
    "__all__",
    "cache",
    "clearcache",
    "getline",
    "getlines",
    "checkcache",
    "updatecache",
    "lazycache",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(
        LINECACHE_PY_SOURCE,
        "<frozen linecache>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("linecache: exec namespace not a dict".into()))?;
    for &name in LINECACHE_PY_EXPORTS {
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
