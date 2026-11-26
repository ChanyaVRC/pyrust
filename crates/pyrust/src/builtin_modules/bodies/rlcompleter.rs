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

/// Execute `RLCOMPLETER_PY_SOURCE` directly inside the module's live namespace
/// so module attribute mutations (such as `rlcompleter._readline_available`)
/// remain visible to `Completer` methods, matching CPython.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = module.borrow_mut().attach_live_namespace_with_dunders();
    interp.exec_source(RLCOMPLETER_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("rlcompleter: exec namespace not a dict".into()))?;
    for name in RLCOMPLETER_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name))
            && let ValueKind::PyClass(cls_rc) = val.kind()
        {
            cls_rc
                .borrow_mut()
                .attrs
                .insert("__module__".to_string(), Value::string("rlcompleter"));
        }
    }
    Ok(None)
}

pyrust_module! {}
