// `gzip` module — body for the `gzip @inject` entry in
// `pyrust_builtin_modules!` (issue #3186).
//
// Pure-Python RFC 1952 gzip streaming and one-shot compression/decompression
// built on `zlib`, `io`, `os`, `struct`, and `time`.
//
// Reference: <https://docs.python.org/3/library/gzip.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

const GZIP_PY_SOURCE: &str = include_str!("gzip_py.py");

const GZIP_PY_EXPORTS: [&str; 15] = [
    "BadGzipFile",
    "FCOMMENT",
    "FEXTRA",
    "FHCRC",
    "FNAME",
    "FTEXT",
    "GzipFile",
    "READ",
    "WRITE",
    "_COMPRESS_LEVEL_BEST",
    "_COMPRESS_LEVEL_FAST",
    "_COMPRESS_LEVEL_TRADEOFF",
    "compress",
    "decompress",
    "open",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(GZIP_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("gzip: exec namespace not a dict".into()))?;
    for name in GZIP_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("gzip"));
                if name == "BadGzipFile" {
                    class.error_name = Some("gzip.BadGzipFile");
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
