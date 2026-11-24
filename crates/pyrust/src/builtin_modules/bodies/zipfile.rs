// `zipfile` module — body for the `zipfile @inject` entry in
// `pyrust_builtin_modules!` (issue #3186).
//
// Pure-Python ZIP archive reader/writer supporting ZIP_STORED, ZIP_DEFLATED,
// ZIP64, in-memory file-like objects, and filesystem extraction.
//
// Reference: <https://docs.python.org/3/library/zipfile.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

const ZIPFILE_PY_SOURCE: &str = include_str!("zipfile_py.py");

const ZIPFILE_PY_EXPORTS: [&str; 14] = [
    "BadZipFile",
    "BadZipfile",
    "LargeZipFile",
    "ZIP64_LIMIT",
    "ZIP_BZIP2",
    "ZIP_DEFLATED",
    "ZIP_FILECOUNT_LIMIT",
    "ZIP_LZMA",
    "ZIP_MAX_COMMENT",
    "ZIP_STORED",
    "ZipFile",
    "ZipInfo",
    "error",
    "is_zipfile",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(ZIPFILE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("zipfile: exec namespace not a dict".into()))?;
    for name in ZIPFILE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("zipfile"));
                if name == "BadZipFile" {
                    class.error_name = Some("zipfile.BadZipFile");
                } else if name == "LargeZipFile" {
                    class.error_name = Some("zipfile.LargeZipFile");
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
