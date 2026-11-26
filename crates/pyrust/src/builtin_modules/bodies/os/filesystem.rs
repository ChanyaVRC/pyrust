//! Host-filesystem operations exposed by `os`.
//!
//! This module owns path-consuming I/O and recursive traversal. Low-level fd
//! operations live in `fd_ops.rs` and `scandir` lives in `scandir.rs`.

use std::path::Path;

use crate::builtin_modules::genericpath::ops::{fspath_resolve, path_buf_to_bytes};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyPath;
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{Value, ValueKind};

use super::arguments::{require_int, value_is_truthy};
use super::fd_ops;
use super::result_types::make_stat_result_for_path;

fn extract_path_str(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let pv = fspath_resolve(interp, value)?;
    let pb = pv.to_path_buf()?;
    Ok(pb.to_string_lossy().into_owned())
}

fn single_fspath_arg(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<String> {
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
    extract_path_str(interp, &args[0].value)
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

pub(super) fn getcwdb() -> Result<Value> {
    let cwd = std::env::current_dir().map_err(|error| PyError::from_io_error(&error, None))?;
    Ok(Value::bytes(path_buf_to_bytes(&cwd)))
}

pub(super) fn chdir(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let path = single_fspath_arg(interp, fn_name, args)?;
    std::env::set_current_dir(&path)
        .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::none())
}

pub(super) fn listdir(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
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
    let (path_buf, path_disp, is_bytes) = match args.first() {
        Some(arg) if !arg.value.is_none() => {
            let pv = fspath_resolve(interp, &arg.value)?;
            let is_b = pv.is_bytes();
            let pb = pv.to_path_buf()?;
            let disp = pb.to_string_lossy().into_owned();
            (pb, disp, is_b)
        }
        _ => (std::path::PathBuf::from("."), ".".to_string(), false),
    };
    let entries = std::fs::read_dir(&path_buf)
        .map_err(|error| PyError::from_io_error(&error, Some(&path_disp)))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| PyError::from_io_error(&error, Some(&path_disp)))?;
        if is_bytes {
            names.push(Value::bytes(path_buf_to_bytes(Path::new(
                &entry.file_name(),
            ))));
        } else {
            names.push(Value::string(entry.file_name().to_string_lossy()));
        }
    }
    Ok(Value::list(names))
}

pub(super) fn mkdir(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.is_empty() || args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes 1 or 2 arguments ({} given)", args.len()),
        ));
    }
    let path = extract_path_str(interp, &args[0].value)?;
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

pub(super) fn makedirs(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument: 'name'"),
        ));
    }
    let path = extract_path_str(interp, &args[0].value)?;
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

    if !exist_ok && Path::new(&path).exists() {
        return match std::fs::create_dir(&path) {
            Ok(()) => Ok(Value::none()),
            Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
        };
    }
    std::fs::create_dir_all(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::none())
}

pub(super) fn remove(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let path = single_fspath_arg(interp, fn_name, args)?;
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

pub(super) fn unlink(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    remove(interp, args, fn_name)
}

pub(super) fn rmdir(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let path = single_fspath_arg(interp, fn_name, args)?;
    std::fs::remove_dir(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::none())
}

pub(super) fn rename(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
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
    let source = extract_path_str(interp, &args[0].value)?;
    let destination = extract_path_str(interp, &args[1].value)?;
    std::fs::rename(&source, &destination)
        .map_err(|error| PyError::from_io_error2(&error, Some(&source), Some(&destination)))?;
    Ok(Value::none())
}

pub(super) fn replace(src: PyPath, dst: PyPath) -> Result<Value> {
    let s = src.as_path();
    let d = dst.as_path();
    let s_disp = s.to_string_lossy();
    let d_disp = d.to_string_lossy();
    std::fs::rename(s, d)
        .map_err(|error| PyError::from_io_error2(&error, Some(&s_disp), Some(&d_disp)))?;
    Ok(Value::none())
}

pub(super) fn link(src: PyPath, dst: PyPath) -> Result<Value> {
    hard_link_path(src.as_path(), dst.as_path())?;
    Ok(Value::none())
}

/// `os.link(src, dst)` core (default `follow_symlinks=True`), shared with
/// `pathlib.Path.hardlink_to`. CPython calls `link(2)` on POSIX — unlike
/// `std::fs::hard_link`'s flag-less `linkat` — and `CreateHardLinkW` on
/// Windows.
pub(crate) fn hard_link_path(src: &Path, dst: &Path) -> Result<()> {
    let link_error = |error: &std::io::Error| {
        PyError::from_io_error2(
            error,
            Some(&src.to_string_lossy()),
            Some(&dst.to_string_lossy()),
        )
    };
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let embedded_nul = |arg: &str| {
            PyError::named(
                "ValueError",
                format!("link: embedded null character in {arg}"),
            )
        };
        let csrc =
            std::ffi::CString::new(src.as_os_str().as_bytes()).map_err(|_| embedded_nul("src"))?;
        let cdst =
            std::ffi::CString::new(dst.as_os_str().as_bytes()).map_err(|_| embedded_nul("dst"))?;
        // SAFETY: both strings are NUL-terminated and live across the call.
        if unsafe { libc::link(csrc.as_ptr(), cdst.as_ptr()) } != 0 {
            return Err(link_error(&std::io::Error::last_os_error()));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::hard_link(src, dst).map_err(|error| link_error(&error))
    }
}

fn host_split(path: &str) -> (String, String) {
    #[cfg(windows)]
    {
        crate::builtin_modules::ntpath::ops::nt_split_str(path)
    }
    #[cfg(not(windows))]
    {
        crate::builtin_modules::posixpath::ops::posix_split_str(path)
    }
}

pub(super) fn removedirs(name: PyPath) -> Result<Value> {
    let p_str = name.as_path().to_string_lossy().into_owned();
    std::fs::remove_dir(&p_str).map_err(|e| PyError::from_io_error(&e, Some(&p_str)))?;
    let (mut head, mut tail) = host_split(&p_str);
    if tail.is_empty() {
        let (h, t) = host_split(&head);
        head = h;
        tail = t;
    }
    while !head.is_empty() && !tail.is_empty() {
        if std::fs::remove_dir(&head).is_err() {
            break;
        }
        let (h, t) = host_split(&head);
        head = h;
        tail = t;
    }
    Ok(Value::none())
}

pub(super) fn renames(old: PyPath, new: PyPath) -> Result<Value> {
    let old_str = old.as_path().to_string_lossy().into_owned();
    let new_str = new.as_path().to_string_lossy().into_owned();
    let (new_head, new_tail) = host_split(&new_str);
    if !new_head.is_empty() && !new_tail.is_empty() && !Path::new(&new_head).exists() {
        std::fs::create_dir_all(&new_head)
            .map_err(|e| PyError::from_io_error(&e, Some(&new_head)))?;
    }
    std::fs::rename(&old_str, &new_str)
        .map_err(|e| PyError::from_io_error2(&e, Some(&old_str), Some(&new_str)))?;
    let (old_head, old_tail) = host_split(&old_str);
    if !old_head.is_empty() && !old_tail.is_empty() {
        let _ = removedirs(PyPath::from(old_head));
    }
    Ok(Value::none())
}

fn value_to_seconds_ns(v: &Value, fn_name: &str) -> Result<(i64, i64)> {
    match v.kind() {
        ValueKind::Int(n) => Ok((n, 0)),
        ValueKind::Bool(b) => Ok((i64::from(b), 0)),
        ValueKind::Float(f) => {
            if !f.is_finite() {
                return Err(PyError::named(
                    "ValueError",
                    format!("{fn_name}: timestamp out of range"),
                ));
            }
            let secs = f.floor() as i64;
            let nanos = ((f - secs as f64) * 1_000_000_000.0).round() as i64;
            if nanos >= 1_000_000_000 {
                Ok((secs + 1, nanos - 1_000_000_000))
            } else if nanos < 0 {
                Ok((secs - 1, nanos + 1_000_000_000))
            } else {
                Ok((secs, nanos))
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() times must be a 2-tuple of numbers"),
        )),
    }
}

fn value_to_ns_pair(v: &Value, fn_name: &str) -> Result<(i64, i64)> {
    let total_ns = match v.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() ns must be a 2-tuple of ints"),
            ));
        }
    };
    let secs = total_ns.div_euclid(1_000_000_000);
    let nanos = total_ns.rem_euclid(1_000_000_000);
    Ok((secs, nanos))
}

pub(super) fn utime(
    interp: &mut Interpreter,
    path: PyPath,
    times: Option<&Value>,
    ns: Option<&Value>,
    follow_symlinks: bool,
) -> Result<Value> {
    let has_times = times.is_some_and(|v| !v.is_none());
    let has_ns = ns.is_some_and(|v| !v.is_none());
    if has_times && has_ns {
        return Err(PyError::named(
            "ValueError",
            "utime: you may specify either 'times' or 'ns' but not both".to_string(),
        ));
    }

    let timestamps: Option<((i64, i64), (i64, i64))> = if has_times {
        let items = interp.collect_iterable(times.unwrap())?;
        if items.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                "utime: 'times' must be either a tuple of two ints or None".to_string(),
            ));
        }
        Some((
            value_to_seconds_ns(&items[0], "utime")?,
            value_to_seconds_ns(&items[1], "utime")?,
        ))
    } else if has_ns {
        let items = interp.collect_iterable(ns.unwrap())?;
        if items.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                "utime: 'ns' must be a tuple of two ints".to_string(),
            ));
        }
        Some((
            value_to_ns_pair(&items[0], "utime")?,
            value_to_ns_pair(&items[1], "utime")?,
        ))
    } else {
        None
    };

    let p = path.as_path();
    let p_disp = p.to_string_lossy();

    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let cpath = CString::new(p.as_os_str().as_bytes())
            .map_err(|_| PyError::named("ValueError", "embedded null byte".to_string()))?;
        let flags = if follow_symlinks {
            0
        } else {
            libc::AT_SYMLINK_NOFOLLOW
        };
        let rc = if let Some(((a_s, a_ns), (m_s, m_ns))) = timestamps {
            let arr = [
                libc::timespec {
                    tv_sec: a_s as libc::time_t,
                    tv_nsec: a_ns as libc::c_long,
                },
                libc::timespec {
                    tv_sec: m_s as libc::time_t,
                    tv_nsec: m_ns as libc::c_long,
                },
            ];
            // SAFETY: `cpath` is NUL-terminated and `arr` is a 2-element timespec array.
            unsafe { libc::utimensat(libc::AT_FDCWD, cpath.as_ptr(), arr.as_ptr(), flags) }
        } else {
            // SAFETY: NULL timespec sets both timestamps to current time.
            unsafe { libc::utimensat(libc::AT_FDCWD, cpath.as_ptr(), std::ptr::null(), flags) }
        };
        if rc < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                Some(&p_disp),
            ));
        }
        Ok(Value::none())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES,
            OPEN_EXISTING, SetFileTime,
        };

        fn unix_to_filetime(secs: i64, nanos: i64) -> FILETIME {
            const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
            let ticks = (secs + EPOCH_DIFF_SECS)
                .saturating_mul(10_000_000)
                .saturating_add(nanos / 100) as u64;
            FILETIME {
                dwLowDateTime: ticks as u32,
                dwHighDateTime: (ticks >> 32) as u32,
            }
        }

        let (a_ft, m_ft) = match timestamps {
            Some(((a_s, a_ns), (m_s, m_ns))) => {
                (unix_to_filetime(a_s, a_ns), unix_to_filetime(m_s, m_ns))
            }
            None => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                let ft = unix_to_filetime(now.as_secs() as i64, now.subsec_nanos() as i64);
                (ft, ft)
            }
        };

        let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect();
        wide.push(0);
        let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
        if !follow_symlinks {
            flags |= FILE_FLAG_OPEN_REPARSE_POINT;
        }
        // SAFETY: `wide` is NUL-terminated; valid handles are closed before returning.
        unsafe {
            let handle = CreateFileW(
                wide.as_ptr(),
                FILE_WRITE_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                flags,
                std::ptr::null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    Some(&p_disp),
                ));
            }
            let ok = SetFileTime(handle, std::ptr::null(), &a_ft, &m_ft);
            let err = std::io::Error::last_os_error();
            CloseHandle(handle);
            if ok == 0 {
                return Err(PyError::from_io_error(&err, Some(&p_disp)));
            }
        }
        Ok(Value::none())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (timestamps, follow_symlinks, p_disp);
        Err(PyError::named(
            "NotImplementedError",
            "os.utime is not supported on this platform".to_string(),
        ))
    }
}

pub(super) fn walk(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let top = single_fspath_arg(interp, fn_name, args)?;
    let mut results = Vec::new();
    walk_collect(&top, &mut results)?;
    Ok(Value::list(results))
}

pub(super) fn stat(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument 'path'"),
        ));
    }
    let mut follow_symlinks = true;
    for arg in args.iter().skip(1) {
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
                    format!("{fn_name}() takes at most 1 positional argument"),
                ));
            }
        }
    }
    match args[0].value.kind() {
        ValueKind::Int(fd) => return fd_ops::fstat(fd),
        ValueKind::Bool(b) => return fd_ops::fstat(i64::from(b)),
        _ => {}
    }
    let path = extract_path_str(interp, &args[0].value)?;
    let p = Path::new(&path);
    let metadata = if follow_symlinks {
        std::fs::metadata(p)
    } else {
        std::fs::symlink_metadata(p)
    }
    .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(make_stat_result_for_path(p, &metadata, follow_symlinks))
}

pub(super) fn lstat(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let path = single_fspath_arg(interp, fn_name, args)?;
    let p = Path::new(&path);
    let metadata = std::fs::symlink_metadata(p)
        .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(make_stat_result_for_path(p, &metadata, false))
}

pub(super) fn access(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
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
    let path = extract_path_str(interp, &args[0].value)?;
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
        if (mode & 2) != 0 && !meta.is_dir() && meta.permissions().readonly() {
            return Ok(Value::bool_(false));
        }
        Ok(Value::bool_(true))
    }
}

pub(super) fn chmod(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at least 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let path = extract_path_str(interp, &args[0].value)?;
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
    chmod_path(&path, mode, follow_symlinks)?;
    Ok(Value::none())
}

/// CPython's `follow_symlinks_specified()` error for an unsupported
/// `follow_symlinks=False` request.
fn follow_symlinks_unavailable(func: &str) -> PyError {
    PyError::named(
        "NotImplementedError",
        format!("{func}: follow_symlinks unavailable on this platform"),
    )
}

/// `os.chmod(path, mode, follow_symlinks=...)` core, shared with
/// `pathlib.Path.chmod` / `Path.lchmod`.
///
/// Matches CPython 3.12.3 per platform:
/// - Linux: `chmod(2)`, or `fchmodat(AT_SYMLINK_NOFOLLOW)` whose
///   `ENOTSUP`/`EOPNOTSUPP` (a symlink) becomes `NotImplementedError`;
/// - macOS/BSD: `fchmodat(AT_SYMLINK_NOFOLLOW)` changes the link itself;
/// - Windows: only the read-only attribute follows `mode & 0o200`, and
///   `follow_symlinks=False` is always `NotImplementedError`.
pub(crate) fn chmod_path(path: &str, mode: i64, follow_symlinks: bool) -> Result<()> {
    let embedded_nul = || {
        PyError::named(
            "ValueError",
            "chmod: embedded null character in path".to_string(),
        )
    };
    if path.contains('\0') {
        return Err(embedded_nul());
    }
    #[cfg(unix)]
    {
        let cpath = std::ffi::CString::new(path).map_err(|_| embedded_nul())?;
        let raw_mode = (mode & 0o7777) as libc::mode_t;
        // SAFETY: `cpath` is a valid NUL-terminated string for the duration of the call.
        let rc = unsafe {
            if follow_symlinks {
                libc::chmod(cpath.as_ptr(), raw_mode)
            } else {
                libc::fchmodat(
                    libc::AT_FDCWD,
                    cpath.as_ptr(),
                    raw_mode,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            }
        };
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            let code = error.raw_os_error();
            if !follow_symlinks && (code == Some(libc::ENOTSUP) || code == Some(libc::EOPNOTSUPP)) {
                return Err(follow_symlinks_unavailable("chmod"));
            }
            return Err(PyError::from_io_error(&error, Some(path)));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_READONLY, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
            SetFileAttributesW,
        };
        if !follow_symlinks {
            return Err(follow_symlinks_unavailable("chmod"));
        }
        let wide: Vec<u16> = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated and outlives both calls.
        let attrs = unsafe { GetFileAttributesW(wide.as_ptr()) };
        if attrs == INVALID_FILE_ATTRIBUTES {
            let error = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&error, Some(path)));
        }
        let updated = if mode & 0o200 != 0 {
            attrs & !FILE_ATTRIBUTE_READONLY
        } else {
            attrs | FILE_ATTRIBUTE_READONLY
        };
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        if unsafe { SetFileAttributesW(wide.as_ptr(), updated) } == 0 {
            let error = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&error, Some(path)));
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        if !follow_symlinks {
            return Err(follow_symlinks_unavailable("chmod"));
        }
        let meta =
            std::fs::metadata(path).map_err(|error| PyError::from_io_error(&error, Some(path)))?;
        let mut perms = meta.permissions();
        perms.set_readonly((mode & 0o200) == 0);
        std::fs::set_permissions(path, perms)
            .map_err(|error| PyError::from_io_error(&error, Some(path)))
    }
}

pub(super) fn symlink(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at least 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let src = extract_path_str(interp, &args[0].value)?;
    let dst = extract_path_str(interp, &args[1].value)?;
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
    symlink_path(&src, &dst, target_is_directory)?;
    Ok(Value::none())
}

/// `os.symlink(src, dst, target_is_directory)` core, shared with
/// `pathlib.Path.symlink_to`. On Windows a directory symlink is created when
/// requested or when `src` (resolved against `dst`'s parent) is a directory,
/// like CPython's `_check_dirW`.
pub(crate) fn symlink_path(src: &str, dst: &str, target_is_directory: bool) -> Result<()> {
    #[cfg(unix)]
    {
        let _ = target_is_directory;
        std::os::unix::fs::symlink(src, dst)
            .map_err(|error| PyError::from_io_error2(&error, Some(src), Some(dst)))
    }
    #[cfg(windows)]
    {
        let is_dir = target_is_directory || {
            let candidate = if Path::new(src).is_absolute() {
                std::path::PathBuf::from(src)
            } else {
                Path::new(dst)
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(src)
            };
            candidate.is_dir()
        };
        let res = if is_dir {
            std::os::windows::fs::symlink_dir(src, dst)
        } else {
            std::os::windows::fs::symlink_file(src, dst)
        };
        res.map_err(|error| PyError::from_io_error2(&error, Some(src), Some(dst)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (src, dst, target_is_directory);
        Err(PyError::named(
            "NotImplementedError",
            "symlink is not supported on this platform".to_string(),
        ))
    }
}

pub(super) fn readlink(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let path = single_fspath_arg(interp, fn_name, args)?;
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
        let child = Path::new(directory)
            .join(&subdirectory)
            .to_string_lossy()
            .into_owned();
        walk_collect(&child, output)?;
    }
    Ok(())
}
