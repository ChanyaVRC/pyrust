// `mimetypes` module — body for the `mimetypes @inject` entry in
// `pyrust_builtin_modules!` (issue #3211).
//
// Pure-Python implementation of CPython 3.12's `mimetypes` module
// (`mimetypes_py.py`) with the full standard CPython built-in MIME type tables
// (`types_map`, `common_types`, `encodings_map`, `suffix_map`) so
// `guess_type` / `guess_extension` / `guess_all_extensions` work
// deterministically without depending on host `/etc/mime.types`.
//
// Reference: <https://docs.python.org/3/library/mimetypes.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `mimetypes` module.
const MIMETYPES_PY_SOURCE: &str = include_str!("mimetypes_py.py");

/// Public names exported from `MIMETYPES_PY_SOURCE` onto `mimetypes`.
const MIMETYPES_PY_EXPORTS: &[&str] = &[
    "__all__",
    "knownfiles",
    "inited",
    "MimeTypes",
    "guess_type",
    "guess_all_extensions",
    "guess_extension",
    "add_type",
    "init",
    "read_mime_types",
    "suffix_map",
    "encodings_map",
    "types_map",
    "common_types",
];

/// Execute `MIMETYPES_PY_SOURCE` once and copy its exports onto `mimetypes`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(MIMETYPES_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("mimetypes: exec namespace not a dict".into()))?;
    for name in MIMETYPES_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("mimetypes"));
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
