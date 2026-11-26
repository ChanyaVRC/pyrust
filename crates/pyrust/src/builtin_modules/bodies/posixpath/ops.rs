//! Pure POSIX path semantics backing both `posixpath` and Unix `os.path`.
//!
//! All path-string operations support `str`, `bytes`, and `os.PathLike` inputs
//! matching CPython 3.12 `Lib/posixpath.py`.

use std::path::PathBuf;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::Value;

use crate::builtin_modules::genericpath::ops::{
    PathValue, clean_canonical_path, fspath_resolve, path_buf_to_bytes, splitext_value,
};

// ── Pure string / byte POSIX algorithms ──────────────────────────────────────

pub(crate) fn posix_isabs_str(path: &str) -> bool {
    path.starts_with('/')
}

pub(crate) fn posix_isabs_bytes(path: &[u8]) -> bool {
    path.starts_with(b"/")
}

pub(crate) fn posix_split_str(path: &str) -> (String, String) {
    let i = path.rfind('/').map_or(0, |idx| idx + 1);
    let head = &path[..i];
    let tail = &path[i..];
    let head = if !head.is_empty() && head.bytes().any(|b| b != b'/') {
        head.trim_end_matches('/')
    } else {
        head
    };
    (head.to_string(), tail.to_string())
}

pub(crate) fn posix_split_bytes(path: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let i = path
        .iter()
        .rposition(|&b| b == b'/')
        .map_or(0, |idx| idx + 1);
    let mut head = &path[..i];
    let tail = &path[i..];
    if !head.is_empty() && head.iter().any(|&b| b != b'/') {
        while head.last() == Some(&b'/') {
            head = &head[..head.len() - 1];
        }
    }
    (head.to_vec(), tail.to_vec())
}

/// CPython 3.12 `posixpath.splitroot(p)` -> `(drive, root, tail)`.
/// `drive` is always empty; `root` is `""`, `"/"`, or `"//"` (exactly two
/// leading slashes are preserved per POSIX; three or more collapse to `"/"`
/// with the extra slashes kept in `tail`).
pub(crate) fn posix_splitroot_str(path: &str) -> (String, String, String) {
    if !path.starts_with('/') {
        (String::new(), String::new(), path.to_string())
    } else if !path.starts_with("//") || path.starts_with("///") {
        (String::new(), "/".to_string(), path[1..].to_string())
    } else {
        (String::new(), "//".to_string(), path[2..].to_string())
    }
}

pub(crate) fn posix_splitroot_bytes(path: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    if !path.starts_with(b"/") {
        (Vec::new(), Vec::new(), path.to_vec())
    } else if !path.starts_with(b"//") || path.starts_with(b"///") {
        (Vec::new(), b"/".to_vec(), path[1..].to_vec())
    } else {
        (Vec::new(), b"//".to_vec(), path[2..].to_vec())
    }
}

pub(crate) fn posix_join_str(parts: &[String]) -> String {
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

pub(crate) fn posix_join_bytes(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut result = parts.first().cloned().unwrap_or_default();
    for p in &parts[1..] {
        if p.starts_with(b"/") {
            result = p.clone();
        } else if result.is_empty() || result.ends_with(b"/") {
            result.extend_from_slice(p);
        } else {
            result.push(b'/');
            result.extend_from_slice(p);
        }
    }
    result
}

pub(crate) fn posix_normpath_str(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let initial_slashes = if path.starts_with('/') {
        if path.starts_with("//") && !path.starts_with("///") {
            2
        } else {
            1
        }
    } else {
        0
    };
    let mut new_comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".."
            || (initial_slashes == 0 && new_comps.is_empty())
            || new_comps.last() == Some(&"..")
        {
            new_comps.push(comp);
        } else if !new_comps.is_empty() {
            new_comps.pop();
        }
    }
    let mut result = "/".repeat(initial_slashes);
    result.push_str(&new_comps.join("/"));
    if result.is_empty() {
        ".".to_string()
    } else {
        result
    }
}

pub(crate) fn posix_normpath_bytes(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return b".".to_vec();
    }
    let initial_slashes = if path.starts_with(b"/") {
        if path.starts_with(b"//") && !path.starts_with(b"///") {
            2
        } else {
            1
        }
    } else {
        0
    };
    let mut new_comps: Vec<&[u8]> = Vec::new();
    for comp in path.split(|&b| b == b'/') {
        if comp.is_empty() || comp == b"." {
            continue;
        }
        if comp != b".."
            || (initial_slashes == 0 && new_comps.is_empty())
            || new_comps.last() == Some(&b"..".as_slice())
        {
            new_comps.push(comp);
        } else if !new_comps.is_empty() {
            new_comps.pop();
        }
    }
    let mut result = vec![b'/'; initial_slashes];
    for (idx, comp) in new_comps.iter().enumerate() {
        if idx > 0 {
            result.push(b'/');
        }
        result.extend_from_slice(comp);
    }
    if result.is_empty() {
        b".".to_vec()
    } else {
        result
    }
}

pub(crate) fn posix_abspath_str(path: &str) -> Result<String> {
    let joined = if path.starts_with('/') {
        path.to_string()
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| PyError::named("OSError", format!("could not resolve cwd: {e}")))?;
        let mut s = cwd.to_string_lossy().into_owned();
        if !s.ends_with('/') {
            s.push('/');
        }
        s.push_str(path);
        s
    };
    Ok(posix_normpath_str(&joined))
}

pub(crate) fn posix_abspath_bytes(path: &[u8]) -> Result<Vec<u8>> {
    let joined = if path.starts_with(b"/") {
        path.to_vec()
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| PyError::named("OSError", format!("could not resolve cwd: {e}")))?;
        let mut s = path_buf_to_bytes(&cwd);
        if !s.ends_with(b"/") {
            s.push(b'/');
        }
        s.extend_from_slice(path);
        s
    };
    Ok(posix_normpath_bytes(&joined))
}

pub(crate) fn posix_relpath_str(path: &str, start: &str) -> Result<String> {
    if path.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "no path specified".to_string(),
        ));
    }
    let abs_path = posix_abspath_str(path)?;
    let abs_start = posix_abspath_str(start)?;
    let path_parts: Vec<&str> = abs_path.split('/').filter(|s| !s.is_empty()).collect();
    let start_parts: Vec<&str> = abs_start.split('/').filter(|s| !s.is_empty()).collect();
    let common = path_parts
        .iter()
        .zip(start_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut rel: Vec<&str> = vec![".."; start_parts.len() - common];
    rel.extend_from_slice(&path_parts[common..]);
    if rel.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(rel.join("/"))
    }
}

pub(crate) fn posix_relpath_bytes(path: &[u8], start: &[u8]) -> Result<Vec<u8>> {
    if path.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "no path specified".to_string(),
        ));
    }
    let abs_path = posix_abspath_bytes(path)?;
    let abs_start = posix_abspath_bytes(start)?;
    let path_parts: Vec<&[u8]> = abs_path
        .split(|&b| b == b'/')
        .filter(|s| !s.is_empty())
        .collect();
    let start_parts: Vec<&[u8]> = abs_start
        .split(|&b| b == b'/')
        .filter(|s| !s.is_empty())
        .collect();
    let common = path_parts
        .iter()
        .zip(start_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut rel: Vec<&[u8]> = vec![b"..".as_slice(); start_parts.len() - common];
    rel.extend_from_slice(&path_parts[common..]);
    if rel.is_empty() {
        Ok(b".".to_vec())
    } else {
        let mut out = Vec::new();
        for (i, part) in rel.iter().enumerate() {
            if i > 0 {
                out.push(b'/');
            }
            out.extend_from_slice(part);
        }
        Ok(out)
    }
}

pub(crate) fn posix_commonpath_str(paths: &[String]) -> Result<String> {
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

pub(crate) fn posix_commonpath_bytes(paths: &[Vec<u8>]) -> Result<Vec<u8>> {
    if paths.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "commonpath() arg is an empty sequence".to_string(),
        ));
    }
    let first_abs = paths[0].starts_with(b"/");
    if paths.iter().any(|p| p.starts_with(b"/") != first_abs) {
        return Err(PyError::named(
            "ValueError",
            "Can't mix absolute and relative paths".to_string(),
        ));
    }
    let split_paths: Vec<Vec<&[u8]>> = paths
        .iter()
        .map(|p| {
            p.split(|&b| b == b'/')
                .filter(|c| !c.is_empty() && *c != b".")
                .collect()
        })
        .collect();
    let s1 = split_paths.iter().min().unwrap();
    let s2 = split_paths.iter().max().unwrap();
    let common_len = s1.iter().zip(s2.iter()).take_while(|(a, b)| a == b).count();
    let mut out = Vec::new();
    if first_abs {
        out.push(b'/');
    }
    for (idx, part) in s1[..common_len].iter().enumerate() {
        if idx > 0 {
            out.push(b'/');
        }
        out.extend_from_slice(part);
    }
    Ok(out)
}

#[cfg(unix)]
fn lookup_user_home_unix(username: &str) -> Option<String> {
    use std::ffi::{CStr, CString};
    let cname = CString::new(username).ok()?;
    // SAFETY: `getpwnam` returns either null or a pointer to a static passwd
    // struct; we copy `pw_dir` immediately before returning.
    unsafe {
        let pw = libc::getpwnam(cname.as_ptr());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())
    }
}

/// `pwd.getpwuid(os.getuid()).pw_dir`, the `expanduser` fallback when `HOME`
/// is unset.
#[cfg(unix)]
fn lookup_current_user_home_unix() -> Option<String> {
    use std::ffi::CStr;
    // SAFETY: `getpwuid` returns either null or a pointer to a static passwd
    // struct; we copy `pw_dir` immediately before returning.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())
    }
}

pub(crate) fn posix_expanduser_str(path: &str) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    let rest_start = path.find('/').unwrap_or(path.len());
    let user_part = &path[1..rest_start];
    let home = if user_part.is_empty() {
        match std::env::var_os("HOME") {
            Some(h) => h.to_string_lossy().into_owned(),
            #[cfg(unix)]
            None => match lookup_current_user_home_unix() {
                Some(h) => h,
                None => return path.to_string(),
            },
            #[cfg(not(unix))]
            None => return path.to_string(),
        }
    } else {
        #[cfg(unix)]
        {
            match lookup_user_home_unix(user_part) {
                Some(h) => h,
                None => return path.to_string(),
            }
        }
        #[cfg(not(unix))]
        {
            return path.to_string();
        }
    };
    let home = if home.len() > 1 {
        home.trim_end_matches('/').to_string()
    } else {
        home
    };
    let mut result = if home.is_empty() {
        "/".to_string()
    } else {
        home
    };
    let rest = &path[rest_start..];
    if rest.is_empty() {
        if result == "/" {
            return "/".to_string();
        }
        result
    } else {
        if result == "/" {
            result.clear();
        }
        result.push_str(rest);
        result
    }
}

pub(crate) fn posix_expandvars_str(path: &str) -> String {
    if !path.contains('$') {
        return path.to_string();
    }
    let chars: Vec<char> = path.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < len {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if i + 1 >= len {
            out.push('$');
            break;
        }
        if chars[i + 1] == '{' {
            let start = i + 2;
            let mut end = start;
            while end < len && chars[end] != '}' {
                end += 1;
            }
            if end < len && chars[end] == '}' {
                let name: String = chars[start..end].iter().collect();
                if let Ok(val) = std::env::var(&name) {
                    out.push_str(&val);
                } else {
                    out.push_str("${");
                    out.push_str(&name);
                    out.push('}');
                }
                i = end + 1;
            } else {
                out.push('$');
                i += 1;
            }
        } else if chars[i + 1].is_ascii_alphanumeric() || chars[i + 1] == '_' {
            let start = i + 1;
            let mut end = start;
            while end < len && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let name: String = chars[start..end].iter().collect();
            if let Ok(val) = std::env::var(&name) {
                out.push_str(&val);
            } else {
                out.push('$');
                out.push_str(&name);
            }
            i = end;
        } else {
            out.push('$');
            i += 1;
        }
    }
    out
}

// ── Value-level adapters shared by `posixpath` and Unix `os.path` ────────────

pub(crate) fn normcase(interp: &mut Interpreter, s: &Value) -> Result<Value> {
    Ok(fspath_resolve(interp, s)?.into_value())
}

pub(crate) fn isabs(interp: &mut Interpreter, s: &Value) -> Result<Value> {
    let ok = match fspath_resolve(interp, s)? {
        PathValue::Str(st) => posix_isabs_str(&st),
        PathValue::Bytes(b) => posix_isabs_bytes(&b),
    };
    Ok(Value::bool_(ok))
}

pub(crate) fn join(interp: &mut Interpreter, first: &Value, rest: &[Value]) -> Result<Value> {
    let first_pv = fspath_resolve(interp, first)?;
    match first_pv {
        PathValue::Str(s) => {
            let mut parts = Vec::with_capacity(1 + rest.len());
            parts.push(s);
            for item in rest {
                match fspath_resolve(interp, item)? {
                    PathValue::Str(p) => parts.push(p),
                    PathValue::Bytes(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "Can't mix strings and bytes in path components".to_string(),
                        ));
                    }
                }
            }
            Ok(Value::string(posix_join_str(&parts)))
        }
        PathValue::Bytes(b) => {
            let mut parts = Vec::with_capacity(1 + rest.len());
            parts.push(b);
            for item in rest {
                match fspath_resolve(interp, item)? {
                    PathValue::Bytes(p) => parts.push(p),
                    PathValue::Str(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "Can't mix strings and bytes in path components".to_string(),
                        ));
                    }
                }
            }
            Ok(Value::bytes(posix_join_bytes(&parts)))
        }
    }
}

pub(crate) fn split(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => {
            let (h, t) = posix_split_str(&s);
            Ok(Value::tuple(vec![Value::string(h), Value::string(t)]))
        }
        PathValue::Bytes(b) => {
            let (h, t) = posix_split_bytes(&b);
            Ok(Value::tuple(vec![Value::bytes(h), Value::bytes(t)]))
        }
    }
}

pub(crate) fn splitroot(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => {
            let (d, r, t) = posix_splitroot_str(&s);
            Ok(Value::tuple(vec![
                Value::string(d),
                Value::string(r),
                Value::string(t),
            ]))
        }
        PathValue::Bytes(b) => {
            let (d, r, t) = posix_splitroot_bytes(&b);
            Ok(Value::tuple(vec![
                Value::bytes(d),
                Value::bytes(r),
                Value::bytes(t),
            ]))
        }
    }
}

pub(crate) fn splitext(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    let pv = fspath_resolve(interp, p)?;
    Ok(splitext_value(pv, b'/', None, b'.'))
}

pub(crate) fn splitdrive(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::tuple(vec![
            Value::string(String::new()),
            Value::string(s),
        ])),
        PathValue::Bytes(b) => Ok(Value::tuple(vec![
            Value::bytes(Vec::new()),
            Value::bytes(b),
        ])),
    }
}

pub(crate) fn basename(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(posix_split_str(&s).1)),
        PathValue::Bytes(b) => Ok(Value::bytes(posix_split_bytes(&b).1)),
    }
}

pub(crate) fn dirname(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(posix_split_str(&s).0)),
        PathValue::Bytes(b) => Ok(Value::bytes(posix_split_bytes(&b).0)),
    }
}

pub(crate) fn normpath(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(posix_normpath_str(&s))),
        PathValue::Bytes(b) => Ok(Value::bytes(posix_normpath_bytes(&b))),
    }
}

pub(crate) fn abspath(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(posix_abspath_str(&s)?)),
        PathValue::Bytes(b) => Ok(Value::bytes(posix_abspath_bytes(&b)?)),
    }
}

pub(crate) fn realpath(interp: &mut Interpreter, filename: &Value, strict: bool) -> Result<Value> {
    let pv = fspath_resolve(interp, filename)?;
    let is_bytes = pv.is_bytes();
    let path_str = pv.to_lossy_string();
    let abs = posix_abspath_str(&path_str)?;
    let resolved = match std::fs::canonicalize(&abs) {
        Ok(p) => clean_canonical_path(&p),
        Err(e) if strict => return Err(PyError::from_io_error(&e, Some(&path_str))),
        Err(_) => {
            let mut cur = PathBuf::from(&abs);
            let mut tail: Vec<std::ffi::OsString> = Vec::new();
            let mut found = None;
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
                    found = Some(posix_normpath_str(&out.to_string_lossy()));
                    break;
                }
            }
            found.unwrap_or_else(|| posix_normpath_str(&abs))
        }
    };
    if is_bytes {
        Ok(Value::bytes(resolved.into_bytes()))
    } else {
        Ok(Value::string(resolved))
    }
}

pub(crate) fn relpath(
    interp: &mut Interpreter,
    path: &Value,
    start: Option<&Value>,
) -> Result<Value> {
    let pv = fspath_resolve(interp, path)?;
    match pv {
        PathValue::Str(p_str) => {
            let start_str = match start {
                None => ".".to_string(),
                Some(v) if v.is_none() => ".".to_string(),
                Some(v) => match fspath_resolve(interp, v)? {
                    PathValue::Str(s) => s,
                    PathValue::Bytes(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "Can't mix strings and bytes in path components".to_string(),
                        ));
                    }
                },
            };
            Ok(Value::string(posix_relpath_str(&p_str, &start_str)?))
        }
        PathValue::Bytes(p_bytes) => {
            let start_bytes = match start {
                None => b".".to_vec(),
                Some(v) if v.is_none() => b".".to_vec(),
                Some(v) => match fspath_resolve(interp, v)? {
                    PathValue::Bytes(b) => b,
                    PathValue::Str(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "Can't mix strings and bytes in path components".to_string(),
                        ));
                    }
                },
            };
            Ok(Value::bytes(posix_relpath_bytes(&p_bytes, &start_bytes)?))
        }
    }
}

pub(crate) fn commonpath(interp: &mut Interpreter, paths_val: &Value) -> Result<Value> {
    let items = interp.collect_iterable(paths_val)?;
    if items.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "commonpath() arg is an empty sequence".to_string(),
        ));
    }
    let first = fspath_resolve(interp, &items[0])?;
    match first {
        PathValue::Str(s0) => {
            let mut all = Vec::with_capacity(items.len());
            all.push(s0);
            for it in &items[1..] {
                match fspath_resolve(interp, it)? {
                    PathValue::Str(s) => all.push(s),
                    PathValue::Bytes(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "Can't mix strings and bytes in path components".to_string(),
                        ));
                    }
                }
            }
            Ok(Value::string(posix_commonpath_str(&all)?))
        }
        PathValue::Bytes(b0) => {
            let mut all = Vec::with_capacity(items.len());
            all.push(b0);
            for it in &items[1..] {
                match fspath_resolve(interp, it)? {
                    PathValue::Bytes(b) => all.push(b),
                    PathValue::Str(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "Can't mix strings and bytes in path components".to_string(),
                        ));
                    }
                }
            }
            Ok(Value::bytes(posix_commonpath_bytes(&all)?))
        }
    }
}

pub(crate) fn expanduser(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(posix_expanduser_str(&s))),
        PathValue::Bytes(b) => {
            let s = String::from_utf8_lossy(&b);
            Ok(Value::bytes(posix_expanduser_str(&s).into_bytes()))
        }
    }
}

pub(crate) fn expandvars(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(posix_expandvars_str(&s))),
        PathValue::Bytes(b) => {
            let s = String::from_utf8_lossy(&b);
            Ok(Value::bytes(posix_expandvars_str(&s).into_bytes()))
        }
    }
}

pub(crate) fn ismount(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let pv = fspath_resolve(interp, path)?;
    let Ok(pb) = pv.to_path_buf() else {
        return Ok(Value::bool_(false));
    };
    let Ok(s1) = std::fs::symlink_metadata(&pb) else {
        return Ok(Value::bool_(false));
    };
    if s1.file_type().is_symlink() {
        return Ok(Value::bool_(false));
    }
    let parent = pb.join("..");
    let Ok(s2) = std::fs::symlink_metadata(&parent) else {
        return Ok(Value::bool_(false));
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Value::bool_(s1.dev() != s2.dev() || s1.ino() == s2.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = (s1, s2);
        let Ok(c1) = std::fs::canonicalize(&pb) else {
            return Ok(Value::bool_(false));
        };
        let Ok(c2) = std::fs::canonicalize(&parent) else {
            return Ok(Value::bool_(false));
        };
        Ok(Value::bool_(c1 == c2))
    }
}
