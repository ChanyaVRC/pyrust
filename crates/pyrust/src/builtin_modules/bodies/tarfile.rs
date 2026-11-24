// `tarfile` module — body for the `tarfile @inject` entry in
// `pyrust_builtin_modules!` (issue #3186).
//
// Pure-Python POSIX ustar / GNU / PAX tar archive reader/writer supporting
// uncompressed tar and gzip-compressed tar (`.tar.gz`), Python 3.12 extraction
// filters, in-memory streams, and filesystem operations.
//
// Reference: <https://docs.python.org/3/library/tarfile.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

const TARFILE_PY_SOURCE: &str = include_str!("tarfile_py.py");

const TARFILE_PY_EXPORTS: [&str; 44] = [
    "AREGTYPE",
    "BLKTYPE",
    "BLOCKSIZE",
    "CHRTYPE",
    "CONTTYPE",
    "CompressionError",
    "DEFAULT_FORMAT",
    "DIRTYPE",
    "ENCODING",
    "EOFHeaderError",
    "EmptyHeaderError",
    "ExtractError",
    "FIFOTYPE",
    "FilterError",
    "GNUTYPE_LONGLINK",
    "GNUTYPE_LONGNAME",
    "GNU_FORMAT",
    "GNU_MAGIC",
    "HeaderError",
    "InvalidHeaderError",
    "LNKTYPE",
    "LinkOutsideDestinationError",
    "NUL",
    "OutsideDestinationError",
    "PAX_FORMAT",
    "POSIX_MAGIC",
    "RECORDSIZE",
    "REGTYPE",
    "ReadError",
    "SYMTYPE",
    "SpecialFileError",
    "StreamError",
    "SubsequentHeaderError",
    "TarError",
    "TarFile",
    "TarInfo",
    "TruncatedHeaderError",
    "USTAR_FORMAT",
    "XGLTYPE",
    "XHDTYPE",
    "AbsoluteLinkError",
    "AbsolutePathError",
    "data_filter",
    "fully_trusted_filter",
];

const TARFILE_PY_EXTRA_EXPORTS: [&str; 4] = ["is_tarfile", "open", "tar_filter", "__all__"];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(TARFILE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("tarfile: exec namespace not a dict".into()))?;
    for name in TARFILE_PY_EXPORTS
        .iter()
        .chain(TARFILE_PY_EXTRA_EXPORTS.iter())
    {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("tarfile"));
                class.error_name = match *name {
                    "TarError" => Some("tarfile.TarError"),
                    "ExtractError" => Some("tarfile.ExtractError"),
                    "ReadError" => Some("tarfile.ReadError"),
                    "CompressionError" => Some("tarfile.CompressionError"),
                    "StreamError" => Some("tarfile.StreamError"),
                    "HeaderError" => Some("tarfile.HeaderError"),
                    "EmptyHeaderError" => Some("tarfile.EmptyHeaderError"),
                    "TruncatedHeaderError" => Some("tarfile.TruncatedHeaderError"),
                    "EOFHeaderError" => Some("tarfile.EOFHeaderError"),
                    "InvalidHeaderError" => Some("tarfile.InvalidHeaderError"),
                    "SubsequentHeaderError" => Some("tarfile.SubsequentHeaderError"),
                    "FilterError" => Some("tarfile.FilterError"),
                    "AbsolutePathError" => Some("tarfile.AbsolutePathError"),
                    "OutsideDestinationError" => Some("tarfile.OutsideDestinationError"),
                    "SpecialFileError" => Some("tarfile.SpecialFileError"),
                    "AbsoluteLinkError" => Some("tarfile.AbsoluteLinkError"),
                    "LinkOutsideDestinationError" => Some("tarfile.LinkOutsideDestinationError"),
                    _ => None,
                };
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
