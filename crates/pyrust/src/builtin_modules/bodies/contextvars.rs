// `contextvars` module — body for the `contextvars @inject` entry in
// `pyrust_builtin_modules!` (issue #3184, PEP 567).
//
// Provides `ContextVar`, `Token`, `Context` (virtually registered as a
// `collections.abc.Mapping`), and `copy_context`, exec'd once at first import
// from `contextvars_py.py` and injected onto the module by
// `inject_python_members`.
//
// Reference: <https://docs.python.org/3/library/contextvars.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `contextvars` module.
const CONTEXTVARS_PY_SOURCE: &str = include_str!("contextvars_py.py");

/// Public names exported onto the `contextvars` module.
const CONTEXTVARS_PY_EXPORTS: [&str; 4] = ["Context", "ContextVar", "Token", "copy_context"];

/// Execute `CONTEXTVARS_PY_SOURCE` once and copy its public names onto the
/// `contextvars` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(CONTEXTVARS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("contextvars: exec namespace not a dict".into()))?;
    for name in CONTEXTVARS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("contextvars"));
                if name == "Context" {
                    super::collections_abc::register_virtual_mapping(cls_rc);
                }
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
