//! Host-filesystem and interpreter-facing `Path` operations.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, NativeIterFrame};
use crate::value::{PyClass, Value, ValueKind};

use super::class_registry::{
    expect_self, get_path, make_path_classmethod_result, make_path_instance_for_class,
    make_path_result,
};

pub(super) fn path_exists(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::bool_(std::path::Path::new(&path).exists()))
}

pub(super) fn path_is_file(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::bool_(std::path::Path::new(&path).is_file()))
}

pub(super) fn path_is_dir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::bool_(std::path::Path::new(&path).is_dir()))
}

pub(super) fn path_read_text(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    // Accept up to two extra args (encoding and errors) but ignore them.
    if args.len() > 3 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes at most 2 arguments"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => Ok(Value::string(contents)),
        Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
    }
}

pub(super) fn path_write_text(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() < 2 || args.len() > 4 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes 1 to 3 arguments"),
        ));
    }
    let data = match args[1].value.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): data must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            ));
        }
    };
    let path = get_path(&inst, fn_name)?;
    let char_count = data.chars().count();
    std::fs::write(&path, data.as_bytes())
        .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::int(char_count as i64))
}

pub(super) fn path_mkdir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut parents = false;
    let mut exist_ok = false;
    // Positional slots are mode, parents, and exist_ok. Mode is accepted but
    // ignored because `std::fs` does not expose umask control.
    let mut pos_index: usize = 0;
    let mut seen_mode_kw = false;
    let mut seen_parents_kw = false;
    let mut seen_exist_ok_kw = false;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("mode") => {
                if seen_mode_kw {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'mode'"),
                    ));
                }
                seen_mode_kw = true;
            }
            Some("parents") => {
                if seen_parents_kw {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'parents'"),
                    ));
                }
                parents = is_truthy_flag(&arg.value);
                seen_parents_kw = true;
            }
            Some("exist_ok") => {
                if seen_exist_ok_kw {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'exist_ok'"),
                    ));
                }
                exist_ok = is_truthy_flag(&arg.value);
                seen_exist_ok_kw = true;
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => {
                match pos_index {
                    0 => {}
                    1 => parents = is_truthy_flag(&arg.value),
                    2 => exist_ok = is_truthy_flag(&arg.value),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{fn_name}() takes at most 3 arguments"),
                        ));
                    }
                }
                pos_index += 1;
            }
        }
    }
    let path = get_path(&inst, fn_name)?;
    let result = if parents {
        std::fs::create_dir_all(&path)
    } else {
        std::fs::create_dir(&path)
    };
    match result {
        Ok(()) => Ok(Value::none()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && exist_ok => {
            Ok(Value::none())
        }
        Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
    }
}

pub(super) fn path_cwd(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let cwd = std::env::current_dir().map_err(|error| PyError::from_io_error(&error, None))?;
    let path = cwd.to_string_lossy();
    make_path_classmethod_result(interp, args, path.into_owned())
}

pub(super) fn path_home(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| {
            PyError::named(
                "RuntimeError",
                "Could not determine home directory".to_string(),
            )
        })?;
    make_path_classmethod_result(interp, args, home)
}

pub(super) fn path_resolve(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut strict = false;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("strict") => strict = is_truthy_flag(&arg.value),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => strict = is_truthy_flag(&arg.value),
        }
    }
    let path = get_path(&inst, fn_name)?;
    match std::fs::canonicalize(&path) {
        Ok(resolved) => Ok(make_path_result(&inst, &resolved.to_string_lossy())),
        Err(error) => {
            if strict {
                Err(PyError::from_io_error(&error, Some(&path)))
            } else {
                let raw = std::path::Path::new(&path);
                let absolute = if raw.is_absolute() {
                    raw.to_path_buf()
                } else {
                    std::env::current_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                        .join(raw)
                };
                Ok(make_path_result(&inst, &absolute.to_string_lossy()))
            }
        }
    }
}

pub(super) fn path_read_bytes(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Value::bytes(bytes)),
        Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
    }
}

pub(super) fn path_write_bytes(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let data = match args[1].value.kind() {
        ValueKind::Bytes(bytes) => bytes.to_vec(),
        ValueKind::BuiltinObject { .. } => {
            match pyrust_builtins::bytearray::as_bytearray_snapshot(&args[1].value) {
                Some(bytes) => bytes,
                None => return Err(bytes_like_type_error(fn_name, &args[1].value)),
            }
        }
        _ => return Err(bytes_like_type_error(fn_name, &args[1].value)),
    };
    let path = get_path(&inst, fn_name)?;
    let len = data.len();
    std::fs::write(&path, &data).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::int(len as i64))
}

pub(super) fn path_open(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    let mut mode = "r".to_string();
    let mut encoding: Option<String> = None;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("mode") => {
                if let ValueKind::Str(value) = arg.value.kind() {
                    mode = value.to_string();
                }
            }
            Some("encoding") => {
                if let ValueKind::Str(value) = arg.value.kind() {
                    encoding = Some(value.to_string());
                }
            }
            Some("buffering") | Some("errors") | Some("newline") => {}
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => {
                if let ValueKind::Str(value) = arg.value.kind() {
                    mode = value.to_string();
                }
            }
        }
    }
    pyrust_builtins::file::open(&path, &mode, encoding.as_deref(), true)
}

pub(super) fn path_unlink(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut missing_ok = false;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("missing_ok") => missing_ok = is_truthy_flag(&arg.value),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => missing_ok = is_truthy_flag(&arg.value),
        }
    }
    let path = get_path(&inst, fn_name)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Value::none()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && missing_ok => {
            Ok(Value::none())
        }
        Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
    }
}

pub(super) fn path_iterdir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    let entries =
        std::fs::read_dir(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    let mut items: Vec<Value> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
        let child_path = entry.path().to_string_lossy().into_owned();
        items.push(make_path_result(&inst, &child_path));
    }
    Ok(Value::generator(Box::new(NativeIterFrame::new(
        items,
        "generator",
    ))))
}

pub(super) fn path_glob(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let pattern = match args[1].value.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): pattern must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            ));
        }
    };
    let base = get_path(&inst, fn_name)?;
    let result_class = Rc::clone(&inst.borrow().class);
    let items = glob_collect(&base, &pattern, &result_class)?;
    Ok(Value::generator(Box::new(NativeIterFrame::new(
        items,
        "generator",
    ))))
}

fn is_truthy_flag(value: &Value) -> bool {
    matches!(value.kind(), ValueKind::Bool(true))
        || matches!(value.kind(), ValueKind::Int(number) if number != 0)
}

fn bytes_like_type_error(fn_name: &str, value: &Value) -> PyError {
    PyError::named(
        "TypeError",
        format!(
            "{fn_name}(): data must be bytes-like, not {}",
            pyrust_core::builtin_type_name(value),
        ),
    )
}

/// Collect paths under `base` matching `pattern` as `result_class` instances.
fn glob_collect(
    base: &str,
    pattern: &str,
    result_class: &Rc<RefCell<PyClass>>,
) -> Result<Vec<Value>> {
    let parts: Vec<&str> = pattern.split('/').collect();
    let base_path = std::path::Path::new(base);
    let has_recursive = parts.contains(&"**");
    let mut results: Vec<std::path::PathBuf> = Vec::new();

    if has_recursive {
        let all = collect_all_descendants(base_path);
        for candidate in &all {
            let relative = candidate.strip_prefix(base_path).unwrap_or(candidate);
            let relative_string = relative.to_string_lossy();
            if glob_pattern_matches(pattern, &relative_string) {
                results.push(candidate.clone());
            }
        }
    } else {
        let mut current_dirs: Vec<std::path::PathBuf> = vec![base_path.to_path_buf()];
        for (index, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            let is_last = index == parts.len() - 1;
            let mut next: Vec<std::path::PathBuf> = Vec::new();
            for dir in &current_dirs {
                let entries = match std::fs::read_dir(dir) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_string = name.to_string_lossy();
                    if glob_component_matches(part, &name_string) {
                        let child = entry.path();
                        if is_last {
                            results.push(child);
                        } else if child.is_dir() {
                            next.push(child);
                        }
                    }
                }
            }
            if !is_last {
                current_dirs = next;
            }
        }
    }

    Ok(results
        .into_iter()
        .map(|path| make_path_instance_for_class(Rc::clone(result_class), &path.to_string_lossy()))
        .collect())
}

fn collect_all_descendants(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let sub = collect_all_descendants(&path);
            out.push(path);
            out.extend(sub);
        } else {
            out.push(path);
        }
    }
    out
}

fn glob_pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();
    glob_parts_match(&pattern_parts, &path_parts)
}

fn glob_parts_match(pattern: &[&str], path: &[&str]) -> bool {
    match (pattern.first(), path.first()) {
        (None, None) => true,
        (None, _) => false,
        (Some(&"**"), _) => {
            let rest = &pattern[1..];
            for skip in 0..=path.len() {
                if glob_parts_match(rest, &path[skip..]) {
                    return true;
                }
            }
            false
        }
        (Some(_), None) => pattern.iter().all(|part| *part == "**"),
        (Some(component), Some(value)) => {
            glob_component_matches(component, value) && glob_parts_match(&pattern[1..], &path[1..])
        }
    }
}

fn glob_component_matches(pattern: &str, name: &str) -> bool {
    glob_match(pattern.as_bytes(), name.as_bytes())
}

fn glob_match(pattern: &[u8], value: &[u8]) -> bool {
    match (pattern.first(), value.first()) {
        (None, None) => true,
        (None, _) => false,
        (Some(b'*'), _) => {
            for skip in 0..=value.len() {
                if glob_match(&pattern[1..], &value[skip..]) {
                    return true;
                }
            }
            false
        }
        (Some(b'?'), Some(_)) => glob_match(&pattern[1..], &value[1..]),
        (Some(b'?'), None) => false,
        (Some(b'['), _) => {
            let close = pattern[1..].iter().position(|&byte| byte == b']');
            let close = match close {
                Some(index) => index + 1,
                None => {
                    if value.first() == Some(&b'[') {
                        return glob_match(&pattern[1..], &value[1..]);
                    }
                    return false;
                }
            };
            let class = &pattern[1..close];
            let rest = &pattern[close + 1..];
            let character = match value.first() {
                Some(&character) => character,
                None => return false,
            };
            char_class_matches(class, character) && glob_match(rest, &value[1..])
        }
        (Some(&pattern_byte), Some(&value_byte)) => {
            pattern_byte == value_byte && glob_match(&pattern[1..], &value[1..])
        }
        (Some(_), None) => false,
    }
}

fn char_class_matches(class: &[u8], character: u8) -> bool {
    let (negated, class) = if class.len() > 1 && class[0] == b'!' {
        (true, &class[1..])
    } else {
        (false, class)
    };

    let mut index = 0;
    let mut matched = false;
    while index < class.len() {
        if index + 2 < class.len() && class[index + 1] == b'-' {
            if character >= class[index] && character <= class[index + 2] {
                matched = true;
            }
            index += 3;
        } else {
            if class[index] == character {
                matched = true;
            }
            index += 1;
        }
    }

    if negated { !matched } else { matched }
}
