// `runpy` module — body for the `runpy @inject` entry in
// `pyrust_builtin_modules!` (issue #3229).
//
// Python 3.12's `runpy` locates and executes Python modules and filesystem
// paths (`.py` scripts, directories with `__main__.py`, and zip archives with
// `__main__.py`) via `run_module` and `run_path`.
//
// Reference: <https://docs.python.org/3.12/library/runpy.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `runpy` module.
const RUNPY_PY_SOURCE: &str = include_str!("runpy_py.py");

/// Public and compatibility names exported from `RUNPY_PY_SOURCE`.
const RUNPY_PY_EXPORTS: &[&str] = &[
    "__all__",
    "run_module",
    "run_path",
    "_run_code",
    "_run_module_code",
    "_run_module_as_main",
    "_TempModule",
    "_ModifiedArgv0",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(RUNPY_PY_SOURCE, "<frozen runpy>", Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("runpy: exec namespace not a dict".into()))?;
    for &name in RUNPY_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("runpy"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
