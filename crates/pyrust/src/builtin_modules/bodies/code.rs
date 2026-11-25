// `code` module — body for the `code @inject` entry in
// `pyrust_builtin_modules!` (issue #3228).
//
// Pure-Python implementation of CPython 3.12's `code` module (`code_py.py`):
// `InteractiveInterpreter`, `InteractiveConsole`, `interact`, and
// `compile_command`.
//
// Reference: <https://docs.python.org/3/library/code.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `code` module.
const CODE_PY_SOURCE: &str = include_str!("code_py.py");

/// Names exported from `CODE_PY_SOURCE` onto the `code` module.
const CODE_PY_EXPORTS: &[&str] = &[
    "__all__",
    "CommandCompiler",
    "compile_command",
    "InteractiveInterpreter",
    "InteractiveConsole",
    "interact",
];

/// Execute `CODE_PY_SOURCE` once and copy its exported attributes onto the
/// `code` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(CODE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("code: exec namespace not a dict".into()))?;
    for name in CODE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                // Only stamp classes defined in `code`, not re-exported `CommandCompiler`.
                if *name != "CommandCompiler" {
                    cls_rc
                        .borrow_mut()
                        .attrs
                        .insert("__module__".to_string(), Value::string("code"));
                }
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
