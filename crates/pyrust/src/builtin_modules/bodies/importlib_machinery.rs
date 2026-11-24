// `importlib.machinery` module — body for the `"importlib.machinery" as
// importlib_machinery @inject` entry in `pyrust_builtin_modules!` (issue #3189).
//
// Exposes Python 3.12's `ModuleSpec`, `SOURCE_SUFFIXES`, `BYTECODE_SUFFIXES`,
// `DEBUG_BYTECODE_SUFFIXES`, `OPTIMIZED_BYTECODE_SUFFIXES`,
// `EXTENSION_SUFFIXES`, `ALL_SUFFIXES`, `all_suffixes`, `SourceFileLoader`,
// `SourcelessFileLoader`, `ExtensionFileLoader`, `BuiltinImporter`,
// `FrozenImporter`, `NamespaceLoader`, `FileFinder`, and `PathFinder`,
// injected from `importlib_machinery_py.py`.
//
// Reference: <https://docs.python.org/3/library/importlib.html#module-importlib.machinery>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for `importlib.machinery`.
const IMPORTLIB_MACHINERY_PY_SOURCE: &str = include_str!("importlib_machinery_py.py");

/// Public and internal names exported onto `importlib.machinery`.
const IMPORTLIB_MACHINERY_PY_EXPORTS: &[&str] = &[
    "SOURCE_SUFFIXES",
    "DEBUG_BYTECODE_SUFFIXES",
    "OPTIMIZED_BYTECODE_SUFFIXES",
    "BYTECODE_SUFFIXES",
    "EXTENSION_SUFFIXES",
    "ALL_SUFFIXES",
    "all_suffixes",
    "ModuleSpec",
    "BuiltinImporter",
    "FrozenImporter",
    "SourceFileLoader",
    "SourcelessFileLoader",
    "ExtensionFileLoader",
    "NamespaceLoader",
    "FileFinder",
    "PathFinder",
    "_cache_from_source",
    "_source_from_cache",
    "_decode_source",
    "_get_cached",
];

/// Execute `IMPORTLIB_MACHINERY_PY_SOURCE` once and copy its exports onto
/// `importlib.machinery`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(IMPORTLIB_MACHINERY_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("importlib.machinery: exec namespace not a dict".into()))?;
    for name in IMPORTLIB_MACHINERY_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc.borrow_mut().attrs.insert(
                    "__module__".to_string(),
                    Value::string("importlib.machinery"),
                );
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
