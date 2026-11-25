// `fileinput` module — body for the `fileinput @inject` entry in
// `pyrust_builtin_modules!` (issue #3230).
//
// CPython 3.12's `fileinput` is a pure-Python module for iterating over lines
// from multiple input streams (`FileInput`, module-level stateful helpers, and
// `hook_compressed` / `hook_encoded`).
//
// Reference: <https://docs.python.org/3/library/fileinput.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `fileinput` module.
const FILEINPUT_PY_SOURCE: &str = include_str!("fileinput_py.py");

/// Public names exported from `FILEINPUT_PY_SOURCE` onto the `fileinput` module.
const FILEINPUT_PY_EXPORTS: &[&str] = &[
    "__doc__",
    "__all__",
    "_state",
    "input",
    "close",
    "nextfile",
    "filename",
    "lineno",
    "filelineno",
    "fileno",
    "isfirstline",
    "isstdin",
    "FileInput",
    "hook_compressed",
    "hook_encoded",
    "_test",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(FILEINPUT_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("fileinput: exec namespace not a dict".into()))?;
    for name in FILEINPUT_PY_EXPORTS {
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
