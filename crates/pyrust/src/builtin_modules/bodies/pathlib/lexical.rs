//! POSIX lexical parsing, normalization, component access, and derived paths.
//!
//! This module deliberately does not touch the host filesystem. Operations
//! preserve the receiver's concrete class through `make_path_result`.

use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::{Value, ValueKind};

use super::class_registry::{expect_self, get_path, is_path_instance, make_path_result};

/// Normalize a POSIX path string the same way CPython's `pathlib` does:
///
/// - strip duplicate `/` separators, except the exact `//` prefix;
/// - remove `.` components, but not `..`;
/// - remove trailing slashes;
/// - represent an empty relative result as `"."`.
pub(super) fn normalize_path(path: &str) -> String {
    let (prefix, rest) = if path.starts_with("//") && !path.starts_with("///") {
        ("//", &path[2..])
    } else if let Some(rest) = path.strip_prefix('/') {
        ("/", rest)
    } else {
        ("", path)
    };

    let parts: Vec<&str> = rest
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();

    if parts.is_empty() {
        if prefix.is_empty() {
            ".".to_string()
        } else {
            prefix.to_string()
        }
    } else {
        format!("{}{}", prefix, parts.join("/"))
    }
}

pub(super) fn path_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parts = &args[1..];
    let mut segments: Vec<String> = Vec::new();
    for arg in parts {
        let segment = match arg.value.kind() {
            ValueKind::Str(value) => value.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "argument should be str, not '{}'",
                        pyrust_core::builtin_type_name(&arg.value),
                    ),
                ));
            }
        };
        // An absolute component replaces every preceding component.
        if segment.starts_with('/') {
            segments.clear();
        }
        if !segment.is_empty() {
            segments.push(segment);
        }
    }
    let joined = if segments.is_empty() {
        ".".to_string()
    } else {
        normalize_path(&segments.join("/"))
    };
    inst.borrow_mut()
        .attrs
        .insert("_path", Value::string(&joined));
    Ok(Value::none())
}

pub(super) fn path_truediv(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let lhs = get_path(&inst, fn_name)?;
    let rhs = match args[1].value.kind() {
        ValueKind::Str(value) => value.to_string(),
        ValueKind::PyInstance(other) => {
            if !is_path_instance(other) {
                return Ok(Value::not_implemented());
            }
            get_path(&Rc::clone(other), fn_name)?
        }
        _ => return Ok(Value::not_implemented()),
    };
    let raw = if rhs.starts_with('/') {
        rhs
    } else if rhs.is_empty() {
        lhs
    } else {
        format!("{lhs}/{rhs}")
    };
    Ok(make_path_result(&inst, &normalize_path(&raw)))
}

pub(super) fn path_joinpath(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut current = get_path(&inst, fn_name)?;
    for arg in args.iter().skip(1) {
        let part = match arg.value.kind() {
            ValueKind::Str(value) => value.to_string(),
            ValueKind::PyInstance(other) => {
                if !is_path_instance(other) {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{fn_name}(): argument must be str or Path, not {}",
                            other.borrow().class.borrow().name,
                        ),
                    ));
                }
                get_path(&Rc::clone(other), fn_name)?
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{fn_name}(): argument must be str or Path, not {}",
                        pyrust_core::builtin_type_name(&arg.value),
                    ),
                ));
            }
        };
        if part.starts_with('/') {
            current = part;
        } else if !part.is_empty() {
            if !current.ends_with('/') {
                current.push('/');
            }
            current.push_str(&part);
        }
    }
    Ok(make_path_result(&inst, &normalize_path(&current)))
}

pub(super) fn path_name(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    if is_anchor(&path) {
        return Ok(Value::string(String::new()));
    }
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    Ok(Value::string(name))
}

pub(super) fn path_parent(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    let parent = match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => path[..index].to_string(),
        None => ".".to_string(),
    };
    Ok(make_path_result(&inst, &parent))
}

pub(super) fn path_stem(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    if is_anchor(&path) {
        return Ok(Value::string(String::new()));
    }
    let name = path.rsplit('/').next().unwrap_or(&path);
    let stem = stem(name);
    Ok(Value::string(stem))
}

pub(super) fn path_suffix(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    if is_anchor(&path) {
        return Ok(Value::string(String::new()));
    }
    let name = path.rsplit('/').next().unwrap_or(&path);
    Ok(Value::string(suffix(name)))
}

pub(super) fn path_parts(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    let mut components: Vec<Value> = Vec::new();
    if path.starts_with("//") && !path.starts_with("///") {
        components.push(Value::string("//"));
        for part in path[2..].split('/') {
            if !part.is_empty() {
                components.push(Value::string(part));
            }
        }
    } else if let Some(rest) = path.strip_prefix('/') {
        components.push(Value::string("/"));
        for part in rest.split('/') {
            if !part.is_empty() {
                components.push(Value::string(part));
            }
        }
    } else {
        for part in path.split('/') {
            if !part.is_empty() {
                components.push(Value::string(part));
            }
        }
    }
    Ok(Value::tuple(components))
}

pub(super) fn path_is_absolute(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::bool_(path.starts_with('/')))
}

pub(super) fn path_with_name(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let name = match args[1].value.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): name must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            ));
        }
    };
    let path = get_path(&inst, fn_name)?;
    if name.is_empty() || name.contains('/') {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid name '{name}'"),
        ));
    }
    if is_anchor(&path) {
        return Err(empty_name_error(&path));
    }
    let new_path = match path.rfind('/') {
        Some(index) => format!("{}/{name}", &path[..index]),
        None => name,
    };
    Ok(make_path_result(&inst, &new_path))
}

pub(super) fn path_with_stem(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let new_stem = match args[1].value.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): stem must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            ));
        }
    };
    let path = get_path(&inst, fn_name)?;
    if is_anchor(&path) {
        return Err(empty_name_error(&path));
    }
    let name = path.rsplit('/').next().unwrap_or(&path);
    let new_name = format!("{new_stem}{}", suffix(name));
    let new_path = match path.rfind('/') {
        Some(index) => format!("{}/{new_name}", &path[..index]),
        None => new_name,
    };
    Ok(make_path_result(&inst, &new_path))
}

pub(super) fn path_with_suffix(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let new_suffix = match args[1].value.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): suffix must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            ));
        }
    };
    if !new_suffix.is_empty() && (!new_suffix.starts_with('.') || new_suffix == ".") {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid suffix '{new_suffix}'"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    if is_anchor(&path) {
        return Err(empty_name_error(&path));
    }
    let name = path.rsplit('/').next().unwrap_or(&path);
    let new_name = format!("{}{new_suffix}", stem(name));
    let new_path = match path.rfind('/') {
        Some(index) => format!("{}/{new_name}", &path[..index]),
        None => new_name,
    };
    Ok(make_path_result(&inst, &new_path))
}

fn is_anchor(path: &str) -> bool {
    path == "." || path == "/" || path == "//"
}

fn stem(name: &str) -> &str {
    if name.chars().all(|character| character == '.') {
        name
    } else {
        match name.rfind('.') {
            Some(index) if index > 0 => &name[..index],
            _ => name,
        }
    }
}

fn suffix(name: &str) -> &str {
    if name.chars().all(|character| character == '.') {
        ""
    } else {
        match name.rfind('.') {
            Some(index) if index > 0 => &name[index..],
            _ => "",
        }
    }
}

fn empty_name_error(path: &str) -> PyError {
    PyError::named(
        "ValueError",
        format!("{} has an empty name", repr_path(path)),
    )
}

/// Format a path as a `PosixPath('...')` repr for `ValueError` messages.
fn repr_path(path: &str) -> String {
    format!("PosixPath('{path}')")
}
