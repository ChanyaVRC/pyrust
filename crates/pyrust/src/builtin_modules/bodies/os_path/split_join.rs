//! Path splitting, joining, and common-prefix helpers for `os.path`.

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{Value, ValueKind};

/// Resolve a `str` or `os.PathLike` (returning `str`) via `__fspath__`.
pub(super) fn fspath_to_str(
    interp: &mut Interpreter,
    fn_name: &str,
    value: &Value,
) -> Result<String> {
    if let ValueKind::Str(s) = value.kind() {
        return Ok(s.to_string());
    }
    if let Ok(method) = interp.get_attr(value, "__fspath__") {
        let res = interp.call_function_expanded(method, &[])?;
        return match res.kind() {
            ValueKind::Str(s) => Ok(s.to_string()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): expected __fspath__() to return str, not {}",
                    crate::interpreter::value_type_name_str(&res),
                ),
            )),
        };
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "expected str, bytes or os.PathLike object, not {}",
            crate::interpreter::value_type_name_str(value),
        ),
    ))
}

/// Resolve a `str`, `bytes`, or `os.PathLike` object via `__fspath__`.
pub(super) fn fspath_value(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    if matches!(value.kind(), ValueKind::Str(_) | ValueKind::Bytes(_)) {
        return Ok(value.clone());
    }
    if let Ok(method) = interp.get_attr(value, "__fspath__") {
        let res = interp.call_function_expanded(method, &[])?;
        if matches!(res.kind(), ValueKind::Str(_) | ValueKind::Bytes(_)) {
            return Ok(res);
        }
        return Err(PyError::named(
            "TypeError",
            format!(
                "expected {}.__fspath__() to return str or bytes, not {}",
                crate::interpreter::value_type_name_str(value),
                crate::interpreter::value_type_name_str(&res),
            ),
        ));
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "expected str, bytes or os.PathLike object, not {}",
            crate::interpreter::value_type_name_str(value),
        ),
    ))
}

/// Common preamble for single-path-arg `os.path` functions. Rejects kwargs,
/// requires exactly one positional argument, and extracts the string via `os.fspath`.
pub(super) fn single_path(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<String> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        // CPython raises `TypeError` for wrong-arity calls.
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    fspath_to_str(interp, fn_name, &args[0].value)
}

pub(super) fn join_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes at least one argument"),
        ));
    }
    let mut parts: Vec<String> = Vec::with_capacity(args.len());
    for a in args {
        parts.push(fspath_to_str(interp, fn_name, &a.value)?);
    }
    Ok(Value::string(host_join(&parts)))
}

pub(super) fn commonpath_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    let items = interp.collect_iterable(&args[0].value)?;
    if items.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "commonpath() arg is an empty sequence".to_string(),
        ));
    }
    let mut paths: Vec<String> = Vec::with_capacity(items.len());
    for it in &items {
        paths.push(fspath_to_str(interp, fn_name, it)?);
    }
    Ok(Value::string(host_commonpath(&paths)?))
}

pub(super) fn host_commonpath(paths: &[String]) -> Result<String> {
    if cfg!(windows) {
        nt_commonpath(paths)
    } else {
        posix_commonpath(paths)
    }
}

pub(super) fn posix_commonpath(paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "commonpath() arg is an empty sequence".to_string(),
        ));
    }
    let first_abs = paths[0].starts_with('/');
    if paths.iter().any(|p| p.starts_with('/') != first_abs) {
        return Err(PyError::named(
            "ValueError",
            "Can't mix absolute and relative paths".to_string(),
        ));
    }
    let split_paths: Vec<Vec<&str>> = paths
        .iter()
        .map(|p| {
            p.split('/')
                .filter(|c| !c.is_empty() && *c != ".")
                .collect()
        })
        .collect();
    let s1 = split_paths.iter().min().unwrap();
    let s2 = split_paths.iter().max().unwrap();
    let common_len = s1.iter().zip(s2.iter()).take_while(|(a, b)| a == b).count();
    let prefix = if first_abs { "/" } else { "" };
    Ok(format!("{prefix}{}", s1[..common_len].join("/")))
}

pub(super) fn nt_commonpath(paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "commonpath() arg is an empty sequence".to_string(),
        ));
    }
    let normalized: Vec<String> = paths.iter().map(|p| p.replace('/', "\\")).collect();
    let split: Vec<(String, String, String)> = normalized.iter().map(|p| nt_splitroot(p)).collect();

    let first_drive_lower = split[0].0.to_lowercase();
    if split
        .iter()
        .any(|(d, _, _)| d.to_lowercase() != first_drive_lower)
    {
        return Err(PyError::named(
            "ValueError",
            "Paths don't have the same drive".to_string(),
        ));
    }
    let first_has_root = !split[0].1.is_empty();
    if split
        .iter()
        .any(|(_, r, _)| !r.is_empty() != first_has_root)
    {
        return Err(PyError::named(
            "ValueError",
            "Can't mix absolute and relative paths".to_string(),
        ));
    }

    let first_parts: Vec<&str> = split[0]
        .2
        .split('\\')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    let mut common_len = first_parts.len();
    for (_, _, rest) in &split[1..] {
        let parts: Vec<&str> = rest
            .split('\\')
            .filter(|c| !c.is_empty() && *c != ".")
            .collect();
        let matched = first_parts
            .iter()
            .zip(parts.iter())
            .take_while(|(a, b)| a.to_lowercase() == b.to_lowercase())
            .count();
        common_len = common_len.min(matched);
    }

    let (ref drive, ref root, _) = split[0];
    Ok(format!(
        "{drive}{root}{}",
        first_parts[..common_len].join("\\")
    ))
}

pub(super) fn commonprefix_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    let items = interp.collect_iterable(&args[0].value)?;
    if items.is_empty() {
        return Ok(Value::string(String::new()));
    }
    // Accept str / PathLike or bytes (matching CPython's os.fspath coercion).
    let first = fspath_value(interp, &items[0])?;
    if matches!(first.kind(), ValueKind::Bytes(_)) {
        let mut all_bytes: Vec<Vec<u8>> = Vec::with_capacity(items.len());
        for it in &items {
            let v = fspath_value(interp, it)?;
            match v.kind() {
                ValueKind::Bytes(b) => all_bytes.push(b.to_vec()),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "Can't mix strings and bytes in path components".to_string(),
                    ));
                }
            }
        }
        let min = all_bytes.iter().min().unwrap();
        let max = all_bytes.iter().max().unwrap();
        let prefix_len = min
            .iter()
            .zip(max.iter())
            .take_while(|(a, b)| a == b)
            .count();
        return Ok(Value::bytes(min[..prefix_len].to_vec()));
    }
    let mut strs: Vec<String> = Vec::with_capacity(items.len());
    for it in &items {
        let v = fspath_value(interp, it)?;
        match v.kind() {
            ValueKind::Str(s) => strs.push(s.to_string()),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "Can't mix strings and bytes in path components".to_string(),
                ));
            }
        }
    }
    let min = strs.iter().min().unwrap();
    let max = strs.iter().max().unwrap();
    let prefix_len = min
        .chars()
        .zip(max.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    Ok(Value::string(&min[..prefix_len]))
}

// ---------------------------------------------------------------------------
// Host-OS dispatch for split/join operations
// ---------------------------------------------------------------------------

pub(super) fn host_split(path: &str) -> (String, String) {
    if cfg!(windows) {
        nt_split(path)
    } else {
        posix_split(path)
    }
}

pub(super) fn host_join(parts: &[String]) -> String {
    if cfg!(windows) {
        nt_join(parts)
    } else {
        posix_join(parts)
    }
}

pub(super) fn host_splitdrive(path: &str) -> (String, String) {
    if cfg!(windows) {
        nt_splitdrive(path)
    } else {
        (String::new(), path.to_string())
    }
}

pub(super) fn host_splitext(path: &str) -> (String, String) {
    // The extension rule is identical on both platforms; only the set of
    // recognised separators differs (ntpath also treats `\` as a separator).
    if cfg!(windows) {
        nt_splitext(path)
    } else {
        splitext_generic(path, '/', None)
    }
}

// ---------------------------------------------------------------------------
// POSIX helpers
// ---------------------------------------------------------------------------

/// POSIX `os.path.split` — split at the last slash. The head has its trailing
/// slashes stripped unless it consists entirely of slashes.
pub(super) fn posix_split(path: &str) -> (String, String) {
    let i = path.rfind('/').map_or(0, |i| i + 1);
    let head = &path[..i];
    let tail = &path[i..];
    // Strip trailing slashes from head unless it's all slashes (root).
    let head = if !head.is_empty() && head.bytes().any(|b| b != b'/') {
        head.trim_end_matches('/')
    } else {
        head
    };
    (head.to_string(), tail.to_string())
}

/// POSIX `os.path.join` — any absolute component (one starting with `/`)
/// resets the running result; relative components are appended with a single
/// `/` separator inserted only when the current result does not already end
/// in one. Mirrors CPython's `posixpath.join`.
pub(super) fn posix_join(parts: &[String]) -> String {
    let mut result = parts.first().cloned().unwrap_or_default();
    for p in &parts[1..] {
        if p.starts_with('/') {
            result = p.clone();
        } else if result.is_empty() || result.ends_with('/') {
            result.push_str(p);
        } else {
            result.push('/');
            result.push_str(p);
        }
    }
    result
}

/// Shared `splitext` over a directory separator `sep`, an optional alternate
/// separator `altsep`, and the extension separator `.`. Mirrors CPython's
/// `genericpath._splitext`: the extension starts at the last dot in the final
/// component, ignoring leading dots (`splitext(".bashrc") == (".bashrc", "")`).
pub(super) fn splitext_generic(path: &str, sep: char, altsep: Option<char>) -> (String, String) {
    let bytes = path.as_bytes();
    let sep_b = sep as u8;
    let alt_b = altsep.map(|c| c as u8);
    let mut last_sep: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == sep_b || Some(b) == alt_b {
            last_sep = Some(i);
        }
    }
    let basename_start = last_sep.map_or(0, |i| i + 1);
    let basename = &path[basename_start..];
    let leading_dot_end = basename.bytes().take_while(|&b| b == b'.').count();
    let after_dots = &basename[leading_dot_end..];
    match after_dots.rfind('.') {
        Some(rel_dot) => {
            let abs_dot = basename_start + leading_dot_end + rel_dot;
            (path[..abs_dot].to_string(), path[abs_dot..].to_string())
        }
        None => (path.to_string(), String::new()),
    }
}

// ---------------------------------------------------------------------------
// Windows (ntpath) helpers — backslash separator + drive-letter / UNC paths.
// Both `\` and `/` act as separators; the canonical separator is `\`.
// Reference: CPython Lib/ntpath.py (3.12).
// ---------------------------------------------------------------------------

/// ntpath.splitroot -> (drive, root, tail). `drive` is the drive letter
/// (`C:`) or UNC sharepoint (`\\host\share`); `root` is the single separator
/// (`\`) or empty; `tail` is everything after the root.
pub(super) fn nt_splitroot(p: &str) -> (String, String, String) {
    let normp: Vec<char> = p.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
    let orig: Vec<char> = p.chars().collect();
    let n = orig.len();
    let take = |a: usize, b: usize| -> String { orig[a..b.min(n)].iter().collect() };

    if normp.first() == Some(&'\\') {
        if normp.get(1) == Some(&'\\') {
            // UNC or device path, e.g. \\server\share or \\?\UNC\server\share
            let upper8: String = normp.iter().take(8).collect::<String>().to_uppercase();
            let start = if upper8 == "\\\\?\\UNC\\" { 8 } else { 2 };
            let Some(index) = find_sep(&normp, start) else {
                return (p.to_string(), String::new(), String::new());
            };
            let Some(index2) = find_sep(&normp, index + 1) else {
                return (p.to_string(), String::new(), String::new());
            };
            return (
                take(0, index2),
                take(index2, index2 + 1),
                take(index2 + 1, n),
            );
        }
        // Relative path with root, e.g. \Windows
        return (String::new(), take(0, 1), take(1, n));
    }
    if normp.get(1) == Some(&':') {
        if normp.get(2) == Some(&'\\') {
            // Absolute drive-letter path, e.g. X:\Windows
            return (take(0, 2), take(2, 3), take(3, n));
        }
        // Relative path with drive, e.g. X:Windows
        return (take(0, 2), String::new(), take(2, n));
    }
    // Relative path, e.g. Windows
    (String::new(), String::new(), p.to_string())
}

/// Index of the next `\` (already normalised) at or after `from`.
fn find_sep(chars: &[char], from: usize) -> Option<usize> {
    chars
        .iter()
        .enumerate()
        .skip(from)
        .find(|&(_, &c)| c == '\\')
        .map(|(i, _)| i)
}

pub(super) fn nt_splitdrive(p: &str) -> (String, String) {
    let (drive, root, tail) = nt_splitroot(p);
    (drive, root + &tail)
}

pub(super) fn nt_split(p: &str) -> (String, String) {
    let (d, r, rest) = nt_splitroot(p);
    let chars: Vec<char> = rest.chars().collect();
    // Index just past the last separator (`\` or `/`).
    let mut i = chars.len();
    while i > 0 && !matches!(chars[i - 1], '\\' | '/') {
        i -= 1;
    }
    let head: String = chars[..i].iter().collect();
    let tail: String = chars[i..].iter().collect();
    let head = head.trim_end_matches(['\\', '/']);
    (format!("{d}{r}{head}"), tail)
}

pub(super) fn nt_join(parts: &[String]) -> String {
    if parts.is_empty() {
        return String::new();
    }
    let (mut result_drive, mut result_root, mut result_path) = nt_splitroot(&parts[0]);
    for p in &parts[1..] {
        let (p_drive, p_root, p_path) = nt_splitroot(p);
        if !p_root.is_empty() {
            // Second path is absolute.
            if !p_drive.is_empty() || result_drive.is_empty() {
                result_drive = p_drive;
            }
            result_root = p_root;
            result_path = p_path;
            continue;
        } else if !p_drive.is_empty() && p_drive != result_drive {
            if p_drive.to_lowercase() != result_drive.to_lowercase() {
                // Different drives — ignore the first path entirely.
                result_drive = p_drive;
                result_root = p_root;
                result_path = p_path;
                continue;
            }
            // Same drive, different case.
            result_drive = p_drive;
        }
        // Second path is relative to the first.
        if !result_path.is_empty() && !ends_with_sep(&result_path) {
            result_path.push('\\');
        }
        result_path.push_str(&p_path);
    }
    // Add a separator between a UNC/drive and a non-absolute path.
    if !result_path.is_empty()
        && result_root.is_empty()
        && !result_drive.is_empty()
        && !ends_with_colon_or_sep(&result_drive)
    {
        return format!("{result_drive}\\{result_path}");
    }
    format!("{result_drive}{result_root}{result_path}")
}

fn ends_with_sep(s: &str) -> bool {
    s.ends_with(['\\', '/'])
}

fn ends_with_colon_or_sep(s: &str) -> bool {
    s.ends_with([':', '\\', '/'])
}

pub(super) fn nt_splitext(path: &str) -> (String, String) {
    splitext_generic(path, '\\', Some('/'))
}
