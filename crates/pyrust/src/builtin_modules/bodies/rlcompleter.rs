// `rlcompleter` module — body for the `rlcompleter @inject` entry in
// `pyrust_builtin_modules!` (issue #3228).
//
// Pure-Python implementation of CPython 3.12's `rlcompleter` module
// (`rlcompleter_py.py`): `Completer` and `get_class_members`.
//
// Reference: <https://docs.python.org/3/library/rlcompleter.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `rlcompleter` module.
const RLCOMPLETER_PY_SOURCE: &str = include_str!("rlcompleter_py.py");

/// Names exported from `RLCOMPLETER_PY_SOURCE` onto the `rlcompleter` module.
const RLCOMPLETER_PY_EXPORTS: &[&str] = &[
    "__all__",
    "Completer",
    "get_class_members",
    "_readline_available",
];

/// Execute `RLCOMPLETER_PY_SOURCE` once and copy its exported attributes onto
/// the `rlcompleter` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(RLCOMPLETER_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("rlcompleter: exec namespace not a dict".into()))?;
    for name in RLCOMPLETER_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("rlcompleter"));
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
