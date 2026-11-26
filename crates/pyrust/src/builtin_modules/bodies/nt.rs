// Native Rust `nt` module (Windows-only, issue #3262).
//
// Matches CPython 3.12's built-in `nt` module: exports core Windows OS
// primitives shared by identity with `os`, `nt`-internal path/disk/DLL helpers,
// and `nt.environ` as a plain dict snapshot of the process environment.

use num_traits::ToPrimitive;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::os::windows as os_win;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyPath};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

/// Ordered list of attributes mirrored from `os` onto `nt` with preserved
/// object identity (`nt.stat_result is os.stat_result`, etc.).
const NT_MIRRORED_FROM_OS: &[&str] = &[
    "F_OK",
    "R_OK",
    "W_OK",
    "X_OK",
    "O_RDONLY",
    "O_WRONLY",
    "O_RDWR",
    "O_APPEND",
    "O_CREAT",
    "O_EXCL",
    "O_TRUNC",
    "O_BINARY",
    "O_TEXT",
    "error",
    "stat_result",
    "terminal_size",
    "times_result",
    "DirEntry",
    "ScandirIterator",
    "getcwd",
    "getcwdb",
    "chdir",
    "listdir",
    "mkdir",
    "remove",
    "unlink",
    "rmdir",
    "rename",
    "replace",
    "link",
    "stat",
    "lstat",
    "access",
    "chmod",
    "symlink",
    "readlink",
    "fspath",
    "utime",
    "scandir",
    "open",
    "close",
    "read",
    "write",
    "dup",
    "dup2",
    "pipe",
    "lseek",
    "fstat",
    "fsync",
    "ftruncate",
    "truncate",
    "isatty",
    "closerange",
    "get_inheritable",
    "set_inheritable",
    "putenv",
    "unsetenv",
    "getpid",
    "getppid",
    "cpu_count",
    "urandom",
    "strerror",
    "get_terminal_size",
    "system",
    "kill",
    "waitpid",
    "waitstatus_to_exitcode",
    "abort",
    "umask",
    "getlogin",
    "times",
    "startfile",
    "listdrives",
    "listvolumes",
    "listmounts",
    "device_encoding",
];

fn make_nt_environ_dict() -> Value {
    let mut dict = PyDict::default();
    for (k, v) in std::env::vars_os() {
        dict.insert(
            PyKey::str_from(k.to_string_lossy().as_ref()),
            Value::string(v.to_string_lossy()),
        );
    }
    Value::dict(dict)
}

/// Post-load wiring invoked from `builtin_modules/finalization.rs`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(nt_mod) = module.kind() else {
        return;
    };
    if let Ok(os_val) = interp.load_module("os") {
        let mut nt_mut = nt_mod.borrow_mut();
        for &name in NT_MIRRORED_FROM_OS {
            if nt_mut.get_attr_value(name).is_none()
                && let Ok(attr) = interp.get_attr(&os_val, name)
            {
                nt_mut.insert_attr(name.to_string(), attr);
            }
        }
        nt_mut.insert_attr("environ".to_string(), make_nt_environ_dict());
    }
}

fn cookie_from_index(cookie: PyIndex) -> Result<usize> {
    if let Some(i) = cookie.as_i64() {
        return usize::try_from(i).map_err(|_| {
            PyError::named(
                "TypeError",
                "Provided cookie was not returned from os.add_dll_directory".to_string(),
            )
        });
    }
    let bi = cookie.to_bigint();
    bi.to_usize().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "Provided cookie was not returned from os.add_dll_directory".to_string(),
        )
    })
}

pyrust_module! {
    fn _add_dll_directory(#[positional_only] path: PyPath) -> Result<Value> {
        os_win::nt_add_dll_directory(path)
    }

    fn _remove_dll_directory(#[positional_only] cookie: PyIndex) -> Result<Value> {
        let raw = cookie_from_index(cookie)?;
        os_win::nt_remove_dll_directory(raw)
    }

    fn _getfinalpathname(#[positional_only] path: PyPath) -> Result<Value> {
        os_win::nt_getfinalpathname(path)
    }

    fn _getfullpathname(#[positional_only] path: PyPath) -> Result<Value> {
        os_win::nt_getfullpathname(path)
    }

    fn _getdiskusage(path: PyPath) -> Result<Value> {
        os_win::nt_getdiskusage(path)
    }

    fn _getvolumepathname(path: PyPath) -> Result<Value> {
        os_win::nt_getvolumepathname(path)
    }
}
