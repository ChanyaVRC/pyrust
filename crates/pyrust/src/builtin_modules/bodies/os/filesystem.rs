//! Host-filesystem operations exposed by `os`.
//!
//! This module owns path-consuming I/O and recursive traversal. It does not
//! construct Python-facing result classes or access process-environment state.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, reject_keyword_args_expanded, value_type_name_str};
use crate::value::{Value, ValueKind};

use super::arguments::{require_int, require_str, single_path_arg, value_is_truthy};
use super::result_types::make_stat_result;

/// Standard POSIX / Windows file-open flag constants exposed on `os`.
pub(crate) const O_RDONLY: i64 = 0x0000;
pub(crate) const O_WRONLY: i64 = 0x0001;
pub(crate) const O_RDWR: i64 = 0x0002;
pub(crate) const O_APPEND: i64 = 0x0008;
pub(crate) const O_CREAT: i64 = 0x0100;
pub(crate) const O_TRUNC: i64 = 0x0200;
pub(crate) const O_EXCL: i64 = 0x0400;

/// Process-local table of open OS file handles backing integer file descriptors
/// returned by `tempfile.mkstemp()` and consumed by `os.read` / `os.write` /
/// `os.close`.
static FD_TABLE: LazyLock<Mutex<HashMap<i64, std::fs::File>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_FD: AtomicI64 = AtomicI64::new(3);

/// Register an open `std::fs::File` in the descriptor table and return its fd.
pub(crate) fn register_fd(file: std::fs::File) -> i64 {
    let fd = NEXT_FD.fetch_add(1, Ordering::Relaxed);
    let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
    table.insert(fd, file);
    fd
}

/// Duplicate an open `std::fs::File` registered in the descriptor table.
pub(crate) fn clone_fd(fd: i64) -> std::io::Result<std::fs::File> {
    let table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
    match table.get(&fd) {
        Some(file) => file.try_clone(),
        None => Err(ebadf_error()),
    }
}

/// Remove and drop an fd from the descriptor table, returning `EBADF` if absent.
pub(crate) fn close_fd_raw(fd: i64) -> std::io::Result<()> {
    let file = {
        let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        table.remove(&fd)
    };
    if file.is_some() {
        Ok(())
    } else {
        Err(ebadf_error())
    }
}

fn ebadf_error() -> std::io::Error {
    #[cfg(unix)]
    {
        std::io::Error::from_raw_os_error(libc::EBADF)
    }
    #[cfg(not(unix))]
    {
        std::io::Error::from_raw_os_error(9)
    }
}

pub(super) fn getcwd(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    let cwd = std::env::current_dir().map_err(|error| PyError::from_io_error(&error, None))?;
    Ok(Value::string(cwd.to_string_lossy()))
}

pub(super) fn chdir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let path = single_path_arg(fn_name, args)?;
    std::env::set_current_dir(&path)
        .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::none())
}

pub(super) fn listdir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at most 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let path = match args.first() {
        Some(arg) => require_str(fn_name, &arg.value, "path")?,
        None => ".".to_string(),
    };
    let entries =
        std::fs::read_dir(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
        names.push(Value::string(entry.file_name().to_string_lossy()));
    }
    Ok(Value::list(names))
}

pub(super) fn mkdir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.is_empty() || args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes 1 or 2 arguments ({} given)", args.len()),
        ));
    }
    let path = require_str(fn_name, &args[0].value, "path")?;
    let mode = match args.get(1) {
        Some(mode) => Some(require_int(fn_name, &mode.value, "mode")?),
        None => None,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        if let Some(m) = mode {
            builder.mode((m & 0o7777) as u32);
        }
        builder
            .create(&path)
            .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        std::fs::create_dir(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    }
    Ok(Value::none())
}

pub(super) fn makedirs(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument: 'name'"),
        ));
    }
    let path = require_str(fn_name, &args[0].value, "path")?;
    let mut exist_ok = false;
    let mut seen_mode_positional = false;
    let mut seen_mode_keyword = false;
    let mut seen_exist_ok_positional = false;
    let mut seen_exist_ok_keyword = false;

    for (index, arg) in args.iter().enumerate().skip(1) {
        match arg.name.as_deref() {
            Some("mode") => {
                if seen_mode_positional || seen_mode_keyword {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'mode'"),
                    ));
                }
                require_int(fn_name, &arg.value, "mode")?;
                seen_mode_keyword = true;
            }
            Some("exist_ok") => {
                if seen_exist_ok_positional || seen_exist_ok_keyword {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'exist_ok'"),
                    ));
                }
                exist_ok = value_is_truthy(&arg.value);
                seen_exist_ok_keyword = true;
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => match index {
                1 if !seen_mode_positional && !seen_mode_keyword => {
                    require_int(fn_name, &arg.value, "mode")?;
                    seen_mode_positional = true;
                }
                2 if !seen_exist_ok_positional && !seen_exist_ok_keyword => {
                    exist_ok = value_is_truthy(&arg.value);
                    seen_exist_ok_positional = true;
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() takes at most 3 arguments"),
                    ));
                }
            },
        }
    }

    if !exist_ok && std::path::Path::new(&path).exists() {
        return match std::fs::create_dir(&path) {
            Ok(()) => Ok(Value::none()),
            Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
        };
    }
    std::fs::create_dir_all(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::none())
}

pub(super) fn remove(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let path = single_path_arg(fn_name, args)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink_dir()) {
            std::fs::remove_dir(&path)
                .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
            return Ok(Value::none());
        }
    }
    std::fs::remove_file(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::none())
}

pub(super) fn unlink(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    remove(args, fn_name)
}

pub(super) fn rmdir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let path = single_path_arg(fn_name, args)?;
    std::fs::remove_dir(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::none())
}

pub(super) fn rename(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
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
    let source = require_str(fn_name, &args[0].value, "src")?;
    let destination = require_str(fn_name, &args[1].value, "dst")?;
    std::fs::rename(&source, &destination)
        .map_err(|error| PyError::from_io_error2(&error, Some(&source), Some(&destination)))?;
    Ok(Value::none())
}

pub(super) fn walk(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let top = single_path_arg(fn_name, args)?;
    let mut results = Vec::new();
    walk_collect(&top, &mut results)?;
    Ok(Value::list(results))
}

pub(super) fn stat(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let path = single_path_arg(fn_name, args)?;
    let metadata =
        std::fs::metadata(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(make_stat_result(&metadata))
}

pub(super) fn lstat(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let path = single_path_arg(fn_name, args)?;
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(make_stat_result(&metadata))
}

pub(super) fn open_path(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing required positional arguments"),
        ));
    }
    let path = require_str(fn_name, &args[0].value, "path")?;
    let flags = require_int(fn_name, &args[1].value, "flags")?;
    let mode = match args.get(2) {
        Some(arg) => require_int(fn_name, &arg.value, "mode")?,
        None => 0o777,
    };
    let access = flags & 0x3;
    let mut opts = std::fs::OpenOptions::new();
    match access {
        O_WRONLY => {
            opts.write(true);
        }
        O_RDWR => {
            opts.read(true).write(true);
        }
        _ => {
            opts.read(true);
        }
    }
    if (flags & O_APPEND) != 0 {
        opts.append(true);
    }
    if (flags & O_TRUNC) != 0 {
        opts.truncate(true);
    }
    if (flags & O_CREAT) != 0 && (flags & O_EXCL) != 0 {
        opts.create_new(true);
    } else if (flags & O_CREAT) != 0 {
        opts.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode((mode & 0o7777) as u32);
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    let file = opts
        .open(&path)
        .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::int(register_fd(file)))
}

pub(super) fn close(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
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
    let fd = require_int(fn_name, &args[0].value, "fd")?;
    close_fd_raw(fd).map_err(|error| PyError::from_io_error(&error, None))?;
    Ok(Value::none())
}

pub(super) fn read_fd(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
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
    let fd = require_int(fn_name, &args[0].value, "fd")?;
    let n = require_int(fn_name, &args[1].value, "n")?;
    if n < 0 {
        return Err(PyError::named("OSError", "Invalid argument".to_string()));
    }
    let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
    let file = table
        .get_mut(&fd)
        .ok_or_else(|| PyError::from_io_error(&ebadf_error(), None))?;
    let mut buf = vec![0u8; n as usize];
    let count = file
        .read(&mut buf)
        .map_err(|error| PyError::from_io_error(&error, None))?;
    buf.truncate(count);
    Ok(Value::bytes(buf))
}

pub(super) fn write_fd(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
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
    let fd = require_int(fn_name, &args[0].value, "fd")?;
    let data: Vec<u8> = match args[1].value.kind() {
        ValueKind::Bytes(b) => b.as_ref().clone(),
        _ => {
            if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&args[1].value) {
                snap
            } else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "a bytes-like object is required, not '{}'",
                        value_type_name_str(&args[1].value)
                    ),
                ));
            }
        }
    };
    let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
    let file = table
        .get_mut(&fd)
        .ok_or_else(|| PyError::from_io_error(&ebadf_error(), None))?;
    let written = file
        .write(&data)
        .map_err(|error| PyError::from_io_error(&error, None))?;
    Ok(Value::int(written as i64))
}

pub(super) fn access(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
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
    let path = require_str(fn_name, &args[0].value, "path")?;
    let mode = require_int(fn_name, &args[1].value, "mode")?;
    #[cfg(unix)]
    {
        use std::ffi::CString;
        let Ok(cpath) = CString::new(path) else {
            return Ok(Value::bool_(false));
        };
        // SAFETY: `cpath` is a NUL-terminated string valid for the call.
        let ok = unsafe { libc::access(cpath.as_ptr(), mode as libc::c_int) == 0 };
        Ok(Value::bool_(ok))
    }
    #[cfg(not(unix))]
    {
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(Value::bool_(false));
        };
        // W_OK == 2: non-directory files marked read-only fail W_OK on Windows.
        if (mode & 2) != 0 && !meta.is_dir() && meta.permissions().readonly() {
            return Ok(Value::bool_(false));
        }
        Ok(Value::bool_(true))
    }
}

pub(super) fn chmod(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at least 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let path = require_str(fn_name, &args[0].value, "path")?;
    let mode = require_int(fn_name, &args[1].value, "mode")?;
    let mut follow_symlinks = true;
    for arg in args.iter().skip(2) {
        match arg.name.as_deref() {
            Some("follow_symlinks") => follow_symlinks = value_is_truthy(&arg.value),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() takes at most 2 positional arguments"),
                ));
            }
        }
    }
    let _ = follow_symlinks;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode((mode & 0o7777) as u32);
        std::fs::set_permissions(&path, perms)
            .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    }
    #[cfg(not(unix))]
    {
        let meta = std::fs::metadata(&path)
            .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
        let mut perms = meta.permissions();
        perms.set_readonly((mode & 0o200) == 0);
        std::fs::set_permissions(&path, perms)
            .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    }
    Ok(Value::none())
}

pub(super) fn symlink(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at least 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let src = require_str(fn_name, &args[0].value, "src")?;
    let dst = require_str(fn_name, &args[1].value, "dst")?;
    let mut target_is_directory = false;
    for (idx, arg) in args.iter().enumerate().skip(2) {
        match arg.name.as_deref() {
            Some("target_is_directory") => target_is_directory = value_is_truthy(&arg.value),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None if idx == 2 => target_is_directory = value_is_truthy(&arg.value),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() takes at most 3 arguments"),
                ));
            }
        }
    }
    #[cfg(unix)]
    {
        let _ = target_is_directory;
        std::os::unix::fs::symlink(&src, &dst)
            .map_err(|error| PyError::from_io_error2(&error, Some(&src), Some(&dst)))?;
    }
    #[cfg(windows)]
    {
        let is_dir = target_is_directory || {
            let candidate = if std::path::Path::new(&src).is_absolute() {
                std::path::PathBuf::from(&src)
            } else {
                std::path::Path::new(&dst)
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join(&src)
            };
            candidate.is_dir()
        };
        let res = if is_dir {
            std::os::windows::fs::symlink_dir(&src, &dst)
        } else {
            std::os::windows::fs::symlink_file(&src, &dst)
        };
        res.map_err(|error| PyError::from_io_error2(&error, Some(&src), Some(&dst)))?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (src, dst, target_is_directory);
        return Err(PyError::named(
            "NotImplementedError",
            "symlink is not supported on this platform".to_string(),
        ));
    }
    Ok(Value::none())
}

pub(super) fn readlink(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let path = single_path_arg(fn_name, args)?;
    let target =
        std::fs::read_link(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::string(target.to_string_lossy()))
}

/// Recursive eager traversal used by the current `os.walk` implementation.
fn walk_collect(directory: &str, output: &mut Vec<Value>) -> Result<()> {
    let mut subdirectories = Vec::new();
    let mut files = Vec::new();
    let entries = std::fs::read_dir(directory)
        .map_err(|error| PyError::from_io_error(&error, Some(directory)))?;

    for entry in entries {
        let entry = entry.map_err(|error| PyError::from_io_error(&error, Some(directory)))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry
            .file_type()
            .map_err(|error| PyError::from_io_error(&error, Some(directory)))?;
        if file_type.is_dir() {
            subdirectories.push(name);
        } else {
            files.push(name);
        }
    }

    output.push(Value::tuple(vec![
        Value::string(directory),
        Value::list(subdirectories.iter().cloned().map(Value::string).collect()),
        Value::list(files.into_iter().map(Value::string).collect()),
    ]));

    for subdirectory in subdirectories {
        let child = std::path::Path::new(directory)
            .join(&subdirectory)
            .to_string_lossy()
            .into_owned();
        walk_collect(&child, output)?;
    }
    Ok(())
}
