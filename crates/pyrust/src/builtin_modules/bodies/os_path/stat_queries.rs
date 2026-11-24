//! Filesystem metadata and canonicalization queries for `os.path`.

use std::path::Path;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, reject_keyword_args_expanded};
use crate::value::{Value, ValueKind};

use super::normalize::{host_abspath, host_normpath};
use super::split_join::single_path;

pub(super) fn exists_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let path = single_path(fn_name, args)?;
    Ok(Value::bool_(Path::new(&path).exists()))
}

pub(super) fn lexists_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let path = single_path(fn_name, args)?;
    Ok(Value::bool_(std::fs::symlink_metadata(&path).is_ok()))
}

pub(super) fn islink_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let path = single_path(fn_name, args)?;
    Ok(Value::bool_(
        std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()),
    ))
}

pub(super) fn samefile_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
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
    let p1 = match args[0].value.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() argument must be str"),
            ));
        }
    };
    let p2 = match args[1].value.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() argument must be str"),
            ));
        }
    };
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

pub(super) fn isfile_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let path = single_path(fn_name, args)?;
    Ok(Value::bool_(Path::new(&path).is_file()))
}

pub(super) fn isdir_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let path = single_path(fn_name, args)?;
    Ok(Value::bool_(Path::new(&path).is_dir()))
}

pub(super) fn realpath_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let path = single_path(fn_name, args)?;
    let abs = host_abspath(&path)?;
    match std::fs::canonicalize(&abs) {
        Ok(p) => Ok(Value::string(p.to_string_lossy())),
        Err(_) => Ok(Value::string(host_normpath(&abs))),
    }
}
