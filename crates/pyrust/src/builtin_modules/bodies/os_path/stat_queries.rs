//! Filesystem metadata and canonicalization queries for `os.path`.

use std::path::{Path, PathBuf};

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::Value;

use super::normalize::{host_abspath, host_normpath};
use super::split_join::{fspath_to_str, single_path};

fn system_time_to_epoch_f64(res: std::io::Result<std::time::SystemTime>) -> f64 {
    res.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Strip Windows extended-length verbatim prefixes (`\\?\` / `\\?\UNC\`)
/// returned by `std::fs::canonicalize` so canonical paths match CPython.
pub(crate) fn clean_canonical_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return stripped.to_string();
        }
    }
    s.into_owned()
}

pub(super) fn exists_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let path = single_path(interp, fn_name, args)?;
    Ok(Value::bool_(Path::new(&path).exists()))
}

pub(super) fn lexists_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let path = single_path(interp, fn_name, args)?;
    Ok(Value::bool_(std::fs::symlink_metadata(&path).is_ok()))
}

pub(super) fn islink_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let path = single_path(interp, fn_name, args)?;
    Ok(Value::bool_(
        std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()),
    ))
}

pub(super) fn samefile_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
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
    let p1 = fspath_to_str(interp, fn_name, &args[0].value)?;
    let p2 = fspath_to_str(interp, fn_name, &args[1].value)?;
    let m1 = std::fs::metadata(&p1).map_err(|e| PyError::from_io_error(&e, Some(&p1)))?;
    let m2 = std::fs::metadata(&p2).map_err(|e| PyError::from_io_error(&e, Some(&p2)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Value::bool_(m1.dev() == m2.dev() && m1.ino() == m2.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = (m1, m2);
        let c1 = std::fs::canonicalize(&p1).map_err(|e| PyError::from_io_error(&e, Some(&p1)))?;
        let c2 = std::fs::canonicalize(&p2).map_err(|e| PyError::from_io_error(&e, Some(&p2)))?;
        Ok(Value::bool_(c1 == c2))
    }
}

pub(super) fn isfile_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let path = single_path(interp, fn_name, args)?;
    Ok(Value::bool_(Path::new(&path).is_file()))
}

pub(super) fn isdir_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let path = single_path(interp, fn_name, args)?;
    Ok(Value::bool_(Path::new(&path).is_dir()))
}

pub(super) fn getsize_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let path = single_path(interp, fn_name, args)?;
    let meta = std::fs::metadata(&path).map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
    Ok(Value::int(meta.len() as i64))
}

pub(super) fn getmtime_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let path = single_path(interp, fn_name, args)?;
    let meta = std::fs::metadata(&path).map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
    Ok(Value::float(system_time_to_epoch_f64(meta.modified())))
}

pub(super) fn getatime_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let path = single_path(interp, fn_name, args)?;
    let meta = std::fs::metadata(&path).map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
    Ok(Value::float(system_time_to_epoch_f64(meta.accessed())))
}

pub(super) fn getctime_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let path = single_path(interp, fn_name, args)?;
    let meta = std::fs::metadata(&path).map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let secs = meta.ctime() as f64 + (meta.ctime_nsec() as f64) / 1_000_000_000.0;
        Ok(Value::float(secs))
    }
    #[cfg(not(unix))]
    {
        Ok(Value::float(system_time_to_epoch_f64(meta.created())))
    }
}

pub(super) fn realpath_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut path_arg: Option<&Value> = None;
    let mut strict = false;
    let mut seen_strict = false;
    for arg in args {
        match arg.name.as_deref() {
            None => {
                if path_arg.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() takes 1 positional argument"),
                    ));
                }
                path_arg = Some(&arg.value);
            }
            Some("filename") | Some("path") => {
                if path_arg.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{fn_name}() got multiple values for argument '{}'",
                            arg.name.as_deref().unwrap_or("filename")
                        ),
                    ));
                }
                path_arg = Some(&arg.value);
            }
            Some("strict") => {
                if seen_strict {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'strict'"),
                    ));
                }
                strict = interp.truthy_value(&arg.value)?;
                seen_strict = true;
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
        }
    }
    let Some(val) = path_arg else {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing 1 required positional argument: 'filename'"),
        ));
    };
    let path = fspath_to_str(interp, fn_name, val)?;
    let abs = host_abspath(&path)?;
    match std::fs::canonicalize(&abs) {
        Ok(p) => Ok(Value::string(clean_canonical_path(&p))),
        Err(e) if strict => Err(PyError::from_io_error(&e, Some(&path))),
        Err(_) => {
            // Walk up to the deepest existing ancestor so symlinks in existing
            // parent directories are still resolved when strict=False.
            let mut cur = PathBuf::from(&abs);
            let mut tail: Vec<std::ffi::OsString> = Vec::new();
            while let Some(name) = cur.file_name() {
                tail.push(name.to_os_string());
                if !cur.pop() {
                    break;
                }
                if let Ok(resolved_parent) = std::fs::canonicalize(&cur) {
                    let mut out = PathBuf::from(clean_canonical_path(&resolved_parent));
                    for part in tail.into_iter().rev() {
                        out.push(part);
                    }
                    return Ok(Value::string(host_normpath(&out.to_string_lossy())));
                }
            }
            Ok(Value::string(host_normpath(&abs)))
        }
    }
}
