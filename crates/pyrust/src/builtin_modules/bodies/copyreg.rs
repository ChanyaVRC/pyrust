// `copyreg` module — body for the `copyreg @inject` entry in
// `pyrust_builtin_modules!` (issue #3208).
//
// Pure-Python implementation of CPython 3.12's `copyreg` module
// (`copyreg_py.py`), shared with `pickle` and `copy`.
//
// Reference: <https://docs.python.org/3/library/copyreg.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `copyreg` module.
const COPYREG_PY_SOURCE: &str = include_str!("copyreg_py.py");

/// Names exported from `COPYREG_PY_SOURCE` onto the `copyreg` module.
const COPYREG_PY_EXPORTS: &[&str] = &[
    "__all__",
    "dispatch_table",
    "pickle",
    "constructor",
    "pickle_complex",
    "pickle_union",
    "_reconstructor",
    "_HEAPTYPE",
    "_new_type",
    "_reduce_ex",
    "__newobj__",
    "__newobj_ex__",
    "_slotnames",
    "_extension_registry",
    "_inverted_registry",
    "_extension_cache",
    "add_extension",
    "remove_extension",
    "clear_extension_cache",
];

/// Execute `COPYREG_PY_SOURCE` once and copy its exported attributes onto the
/// `copyreg` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    ns.dict_insert(
        PyKey::str_from("_native_reconstructor"),
        Value::builtin_function("copyreg._reconstructor"),
    )?;
    ns.dict_insert(
        PyKey::str_from("_native_newobj"),
        Value::builtin_function("copyreg.__newobj__"),
    )?;
    interp.exec_source(COPYREG_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("copyreg: exec namespace not a dict".into()))?;
    for name in COPYREG_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("copyreg"));
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
