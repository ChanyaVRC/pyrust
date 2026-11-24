//! Filesystem metadata and canonicalization queries for `os.path`.

use std::path::Path;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;

use super::normalize::{host_abspath, host_normpath};
use super::split_join::single_path;

pub(super) fn exists_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let path = single_path(fn_name, args)?;
    Ok(Value::bool_(Path::new(&path).exists()))
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
