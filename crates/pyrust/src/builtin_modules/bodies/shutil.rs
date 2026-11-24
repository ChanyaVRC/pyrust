// `shutil` module — body for the `shutil @inject` entry in
// `pyrust_builtin_modules!` (issue #3148).
//
// Minimal native Rust helpers handle streaming file copy, metadata timestamp
// preservation, FIFO detection, and tty terminal-size probing; the public
// CPython 3.12 `shutil` surface lives in `shutil_py.py` and is injected by
// `inject_python_members`.
//
// Reference: <https://docs.python.org/3/library/shutil.html>

use std::cell::RefCell;
use std::io::IsTerminal;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, reject_keyword_args_expanded, value_type_name_str,
};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `shutil` module.
const SHUTIL_PY_SOURCE: &str = include_str!("shutil_py.py");

/// Private native helpers moved into the exec namespace and hidden from
/// `dir(shutil)` so the module surface stays clean.
const SHUTIL_NATIVE_HELPERS: [&str; 4] = [
    "_copyfile_native",
    "_copystat_times",
    "_is_fifo",
    "_query_terminal_size",
];

/// Public attributes exported onto the `shutil` module.
const SHUTIL_PY_EXPORTS: [&str; 18] = [
    "Error",
    "SameFileError",
    "SpecialFileError",
    "ExecError",
    "ReadError",
    "COPY_BUFSIZE",
    "copyfileobj",
    "copyfile",
    "copymode",
    "copystat",
    "copy",
    "copy2",
    "ignore_patterns",
    "copytree",
    "rmtree",
    "move",
    "which",
    "get_terminal_size",
];

/// Execute `SHUTIL_PY_SOURCE` once, wire its wrappers to the native helpers,
/// hide private helper names from `dir(shutil)`, and export the public
/// CPython 3.12 `shutil` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in SHUTIL_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(SHUTIL_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("shutil: exec namespace not a dict".into()))?;
    for name in SHUTIL_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("shutil"));
                class.error_name = match name {
                    "Error" => Some("shutil.Error"),
                    "SameFileError" => Some("shutil.SameFileError"),
                    "SpecialFileError" => Some("shutil.SpecialFileError"),
                    "ExecError" => Some("shutil.ExecError"),
                    "ReadError" => Some("shutil.ReadError"),
                    _ => None,
                };
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

fn require_str(fn_name: &str, value: &Value, what: &str) -> Result<String> {
    match value.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {what} must be str, not {}",
                value_type_name_str(value),
            ),
        )),
    }
}

fn copyfile_native_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let src = require_str(fn_name, &args[0].value, "src")?;
    let dst = require_str(fn_name, &args[1].value, "dst")?;
    let mut reader =
        std::fs::File::open(&src).map_err(|e| PyError::from_io_error(&e, Some(&src)))?;
    let mut writer =
        std::fs::File::create(&dst).map_err(|e| PyError::from_io_error(&e, Some(&dst)))?;
    std::io::copy(&mut reader, &mut writer)
        .map_err(|e| PyError::from_io_error2(&e, Some(&src), Some(&dst)))?;
    Ok(Value::none())
}

fn copystat_times_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 3 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let src = require_str(fn_name, &args[0].value, "src")?;
    let dst = require_str(fn_name, &args[1].value, "dst")?;
    let follow = match args[2].value.kind() {
        ValueKind::Bool(b) => b,
        ValueKind::Int(n) => n != 0,
        ValueKind::None => false,
        _ => true,
    };

    let meta = if follow {
        std::fs::metadata(&src)
    } else {
        std::fs::symlink_metadata(&src)
    }
    .map_err(|e| PyError::from_io_error(&e, Some(&src)))?;

    let atime = meta.accessed().ok();
    let mtime = meta.modified().ok();

    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::time::UNIX_EPOCH;

        fn to_timespec(t: Option<std::time::SystemTime>) -> libc::timespec {
            match t.and_then(|st| st.duration_since(UNIX_EPOCH).ok()) {
                Some(dur) => libc::timespec {
                    tv_sec: dur.as_secs() as libc::time_t,
                    tv_nsec: dur.subsec_nanos() as libc::c_long,
                },
                None => libc::timespec {
                    tv_sec: 0,
                    tv_nsec: libc::UTIME_OMIT,
                },
            }
        }

        let c_dst = CString::new(dst.as_str())
            .map_err(|_| PyError::named("ValueError", "embedded null byte in path".to_string()))?;
        let times = [to_timespec(atime), to_timespec(mtime)];
        let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
        // SAFETY: `c_dst` is NUL-terminated and `times` is a valid 2-element array.
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_dst.as_ptr(), times.as_ptr(), flags) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&err, Some(&dst)));
        }
    }

    #[cfg(not(unix))]
    {
        let dst_path = std::path::Path::new(&dst);
        if !dst_path.is_dir() {
            let mut times = std::fs::FileTimes::new();
            if let Some(a) = atime {
                times = times.set_accessed(a);
            }
            if let Some(m) = mtime {
                times = times.set_modified(m);
            }
            if let Ok(file) = std::fs::OpenOptions::new().write(true).open(dst_path) {
                let _ = file.set_times(times);
            }
        }
    }

    Ok(Value::none())
}

fn is_fifo_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let path = require_str(fn_name, &args[0].value, "path")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let is_fifo = std::fs::metadata(&path).is_ok_and(|m| m.file_type().is_fifo());
        Ok(Value::bool_(is_fifo))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(Value::bool_(false))
    }
}

fn query_terminal_size_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments ({} given)", args.len()),
        ));
    }
    if !std::io::stdout().is_terminal() {
        return Err(PyError::named(
            "OSError",
            "Inappropriate ioctl for device".to_string(),
        ));
    }
    #[cfg(unix)]
    {
        // SAFETY: `winsize` is POD and `TIOCGWINSZ` writes into it.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
        if rc == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
            return Ok(Value::tuple(vec![
                Value::int(ws.ws_col as i64),
                Value::int(ws.ws_row as i64),
            ]));
        }
        Err(PyError::named(
            "OSError",
            "Inappropriate ioctl for device".to_string(),
        ))
    }
    #[cfg(not(unix))]
    {
        Err(PyError::named(
            "OSError",
            "terminal size query unavailable".to_string(),
        ))
    }
}

pyrust_module! {
    fn _copyfile_native(args) -> Result<Value> {
        copyfile_native_impl(FN_NAME, args)
    }

    fn _copystat_times(args) -> Result<Value> {
        copystat_times_impl(FN_NAME, args)
    }

    fn _is_fifo(args) -> Result<Value> {
        is_fifo_impl(FN_NAME, args)
    }

    fn _query_terminal_size(args) -> Result<Value> {
        query_terminal_size_impl(FN_NAME, args)
    }
}
