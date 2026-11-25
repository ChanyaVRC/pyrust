// `pkgutil` module — body for the `pkgutil @inject` entry in
// `pyrust_builtin_modules!` (issue #3229).
//
// Python 3.12's `pkgutil` provides package discovery, importer resolution,
// package resource access, dotted-name resolution, and namespace path
// extension (`ModuleInfo`, `iter_modules`, `walk_packages`,
// `iter_importer_modules`, `get_importer`, `iter_importers`, `get_loader`,
// `find_loader`, `get_data`, `resolve_name`, `extend_path`, `read_code`).
//
// Reference: <https://docs.python.org/3.12/library/pkgutil.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `pkgutil` module.
const PKGUTIL_PY_SOURCE: &str = include_str!("pkgutil_py.py");

/// Public names exported from `PKGUTIL_PY_SOURCE` onto the `pkgutil` module.
const PKGUTIL_PY_EXPORTS: &[&str] = &[
    "__all__",
    "ModuleInfo",
    "read_code",
    "walk_packages",
    "iter_modules",
    "iter_importer_modules",
    "get_importer",
    "iter_importers",
    "get_loader",
    "find_loader",
    "extend_path",
    "get_data",
    "resolve_name",
    "_ZipImporter",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(
        PKGUTIL_PY_SOURCE,
        "<frozen pkgutil>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("pkgutil: exec namespace not a dict".into()))?;
    for &name in PKGUTIL_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("pkgutil"));
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
