// `tomllib` module — body for the `tomllib @inject` entry in
// `pyrust_builtin_modules!` (issue #3184, PEP 680).
//
// CPython 3.11+'s `tomllib` is a pure-Python TOML 1.0.0 parser exposing
// `loads`, `load`, and `TOMLDecodeError`. We ship a pure-Python implementation
// (`tomllib_py.py`) exec'd once at first import and injected onto the module
// by `inject_python_members`.
//
// Reference: <https://docs.python.org/3/library/tomllib.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `tomllib` module.
const TOMLLIB_PY_SOURCE: &str = include_str!("tomllib_py.py");

/// Public names exported onto the `tomllib` module.
const TOMLLIB_PY_EXPORTS: [&str; 3] = ["loads", "load", "TOMLDecodeError"];

/// Execute `TOMLLIB_PY_SOURCE` once and copy its public names onto the
/// `tomllib` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(TOMLLIB_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("tomllib: exec namespace not a dict".into()))?;
    for name in TOMLLIB_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("tomllib"));
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
