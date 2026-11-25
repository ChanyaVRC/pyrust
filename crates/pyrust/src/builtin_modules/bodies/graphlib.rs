// `graphlib` module — body for the `graphlib @inject` entry in
// `pyrust_builtin_modules!` (issue #3208).
//
// Pure-Python implementation of CPython 3.12's `graphlib` module
// (`graphlib_py.py`): `TopologicalSorter` and `CycleError`.
//
// Reference: <https://docs.python.org/3/library/graphlib.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `graphlib` module.
const GRAPHLIB_PY_SOURCE: &str = include_str!("graphlib_py.py");

/// Names exported from `GRAPHLIB_PY_SOURCE` onto the `graphlib` module.
const GRAPHLIB_PY_EXPORTS: &[&str] = &[
    "__all__",
    "TopologicalSorter",
    "CycleError",
    "_NODE_OUT",
    "_NODE_DONE",
    "_NodeInfo",
];

/// Execute `GRAPHLIB_PY_SOURCE` once and copy its exported attributes onto the
/// `graphlib` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(GRAPHLIB_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("graphlib: exec namespace not a dict".into()))?;
    for name in GRAPHLIB_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("graphlib"));
            }
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
