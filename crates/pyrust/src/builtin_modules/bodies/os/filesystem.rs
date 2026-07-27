//! Host-filesystem operations exposed by `os`.
//!
//! This module owns path-consuming I/O and recursive traversal. It does not
//! construct Python-facing result classes or access process-environment state.

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, reject_keyword_args_expanded};
use crate::value::Value;

use super::arguments::{require_int, require_str, single_path_arg, value_is_truthy};
use super::result_types::make_stat_result;

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
    // `mode` remains accepted and ignored. Applying it correctly requires a
    // platform-specific implementation that honors umask semantics.
    if let Some(mode) = args.get(1) {
        require_int(fn_name, &mode.value, "mode")?;
    }
    std::fs::create_dir(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
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
