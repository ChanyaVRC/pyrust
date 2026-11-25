// `codeop` module — body for the `codeop @inject` entry in
// `pyrust_builtin_modules!` (issue #3228).
//
// Pure-Python implementation of CPython 3.12's `codeop` module
// (`codeop_py.py`): `compile_command`, `Compile`, and `CommandCompiler`.
//
// Reference: <https://docs.python.org/3/library/codeop.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `codeop` module.
const CODEOP_PY_SOURCE: &str = include_str!("codeop_py.py");

/// Names exported from `CODEOP_PY_SOURCE` onto the `codeop` module.
const CODEOP_PY_EXPORTS: &[&str] = &[
    "__all__",
    "PyCF_DONT_IMPLY_DEDENT",
    "PyCF_ALLOW_INCOMPLETE_INPUT",
    "_features",
    "_maybe_compile",
    "_is_syntax_error",
    "_compile",
    "compile_command",
    "Compile",
    "CommandCompiler",
];

/// Execute `CODEOP_PY_SOURCE` once and copy its exported attributes onto the
/// `codeop` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(CODEOP_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("codeop: exec namespace not a dict".into()))?;
    for name in CODEOP_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("codeop"));
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
