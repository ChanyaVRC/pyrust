// `stat` module — body for the `stat @inject` entry in
// `pyrust_builtin_modules!` (issue #3209).
//
// Provides Python 3.12 file mode constants (`ST_*`, `S_IF*`, `S_IR*`/`S_IW*`/
// `S_IX*`, `S_ISUID`/`S_ISGID`/`S_ISVTX`/`S_ENFMT`, `UF_*`, `SF_*`,
// `FILE_ATTRIBUTE_*`, `IO_REPARSE_TAG_*`), mode predicates (`S_IFMT`, `S_IMODE`,
// `S_ISDIR`, `S_ISCHR`, `S_ISBLK`, `S_ISREG`, `S_ISFIFO`, `S_ISLNK`,
// `S_ISSOCK`, `S_ISDOOR`, `S_ISPORT`, `S_ISWHT`), and `filemode()`.
//
// Reference: <https://docs.python.org/3.12/library/stat.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `stat` module.
const STAT_PY_SOURCE: &str = include_str!("stat_py.py");

/// Public and compatibility names exported from `STAT_PY_SOURCE` onto the
/// `stat` module.
const STAT_PY_EXPORTS: &[&str] = &[
    "ST_MODE",
    "ST_INO",
    "ST_DEV",
    "ST_NLINK",
    "ST_UID",
    "ST_GID",
    "ST_SIZE",
    "ST_ATIME",
    "ST_MTIME",
    "ST_CTIME",
    "S_IMODE",
    "S_IFMT",
    "S_IFDIR",
    "S_IFCHR",
    "S_IFBLK",
    "S_IFREG",
    "S_IFIFO",
    "S_IFLNK",
    "S_IFSOCK",
    "S_IFDOOR",
    "S_IFPORT",
    "S_IFWHT",
    "S_ISDIR",
    "S_ISCHR",
    "S_ISBLK",
    "S_ISREG",
    "S_ISFIFO",
    "S_ISLNK",
    "S_ISSOCK",
    "S_ISDOOR",
    "S_ISPORT",
    "S_ISWHT",
    "S_ISUID",
    "S_ISGID",
    "S_ENFMT",
    "S_ISVTX",
    "S_IREAD",
    "S_IWRITE",
    "S_IEXEC",
    "S_IRWXU",
    "S_IRUSR",
    "S_IWUSR",
    "S_IXUSR",
    "S_IRWXG",
    "S_IRGRP",
    "S_IWGRP",
    "S_IXGRP",
    "S_IRWXO",
    "S_IROTH",
    "S_IWOTH",
    "S_IXOTH",
    "UF_NODUMP",
    "UF_IMMUTABLE",
    "UF_APPEND",
    "UF_OPAQUE",
    "UF_NOUNLINK",
    "UF_COMPRESSED",
    "UF_HIDDEN",
    "SF_ARCHIVED",
    "SF_IMMUTABLE",
    "SF_APPEND",
    "SF_NOUNLINK",
    "SF_SNAPSHOT",
    "_filemode_table",
    "filemode",
    "FILE_ATTRIBUTE_ARCHIVE",
    "FILE_ATTRIBUTE_COMPRESSED",
    "FILE_ATTRIBUTE_DEVICE",
    "FILE_ATTRIBUTE_DIRECTORY",
    "FILE_ATTRIBUTE_ENCRYPTED",
    "FILE_ATTRIBUTE_HIDDEN",
    "FILE_ATTRIBUTE_INTEGRITY_STREAM",
    "FILE_ATTRIBUTE_NORMAL",
    "FILE_ATTRIBUTE_NOT_CONTENT_INDEXED",
    "FILE_ATTRIBUTE_NO_SCRUB_DATA",
    "FILE_ATTRIBUTE_OFFLINE",
    "FILE_ATTRIBUTE_READONLY",
    "FILE_ATTRIBUTE_REPARSE_POINT",
    "FILE_ATTRIBUTE_SPARSE_FILE",
    "FILE_ATTRIBUTE_SYSTEM",
    "FILE_ATTRIBUTE_TEMPORARY",
    "FILE_ATTRIBUTE_VIRTUAL",
    "IO_REPARSE_TAG_SYMLINK",
    "IO_REPARSE_TAG_MOUNT_POINT",
    "IO_REPARSE_TAG_APPEXECLINK",
];

/// Execute `STAT_PY_SOURCE` once and copy its exported attributes onto the
/// `stat` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(STAT_PY_SOURCE, "<frozen stat>", Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("stat: exec namespace not a dict".into()))?;
    for &name in STAT_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    drop(dict);
    Ok(Some(ns))
}

pyrust_module! {}
