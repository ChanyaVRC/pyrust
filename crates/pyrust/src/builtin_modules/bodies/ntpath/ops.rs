//! Pure Windows/NT path semantics backing both `ntpath` and Windows `os.path`.
//!
//! All path-string operations support `str`, `bytes`, and `os.PathLike` inputs
//! matching CPython 3.12 `Lib/ntpath.py`.

use std::path::PathBuf;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::Value;

use crate::builtin_modules::genericpath::ops::{
    PathValue, clean_canonical_path, fspath_resolve, path_buf_to_bytes, splitext_value,
};

// ── Pure NT string algorithms ────────────────────────────────────────────────

fn find_sep_chars(chars: &[char], from: usize) -> Option<usize> {
    chars
        .iter()
        .enumerate()
        .skip(from)
        .find(|&(_, &c)| c == '\\')
        .map(|(i, _)| i)
}

fn find_sep_bytes(bytes: &[u8], from: usize) -> Option<usize> {
    bytes
        .iter()
        .enumerate()
        .skip(from)
        .find(|&(_, &b)| b == b'\\')
        .map(|(i, _)| i)
}

/// CPython 3.12 `ntpath.splitroot(p)` -> `(drive, root, tail)`.
pub(crate) fn nt_splitroot_str(p: &str) -> (String, String, String) {
    let normp: Vec<char> = p.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
    let orig: Vec<char> = p.chars().collect();
    let n = orig.len();
    let take = |a: usize, b: usize| -> String { orig[a..b.min(n)].iter().collect() };

    if normp.first() == Some(&'\\') {
        if normp.get(1) == Some(&'\\') {
            // UNC or device path, e.g. \\server\share or \\?\UNC\server\share
            let upper8: String = normp.iter().take(8).collect::<String>().to_uppercase();
            let start = if upper8 == "\\\\?\\UNC\\" { 8 } else { 2 };
            let Some(index) = find_sep_chars(&normp, start) else {
                return (p.to_string(), String::new(), String::new());
            };
            let Some(index2) = find_sep_chars(&normp, index + 1) else {
                return (p.to_string(), String::new(), String::new());
            };
            return (
                take(0, index2),
                take(index2, index2 + 1),
                take(index2 + 1, n),
            );
        }
        return (String::new(), take(0, 1), take(1, n));
    }
    if normp.get(1) == Some(&':') {
        if normp.get(2) == Some(&'\\') {
            return (take(0, 2), take(2, 3), take(3, n));
        }
        return (take(0, 2), String::new(), take(2, n));
    }
    (String::new(), String::new(), p.to_string())
}

pub(crate) fn nt_splitroot_bytes(p: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let normp: Vec<u8> = p
        .iter()
        .map(|&b| if b == b'/' { b'\\' } else { b })
        .collect();
    let n = p.len();
    let take = |a: usize, b: usize| -> Vec<u8> { p[a..b.min(n)].to_vec() };

    if normp.first() == Some(&b'\\') {
        if normp.get(1) == Some(&b'\\') {
            let is_unc_ext = normp.len() >= 8 && normp[..8].eq_ignore_ascii_case(b"\\\\?\\UNC\\");
            let start = if is_unc_ext { 8 } else { 2 };
            let Some(index) = find_sep_bytes(&normp, start) else {
                return (p.to_vec(), Vec::new(), Vec::new());
            };
            let Some(index2) = find_sep_bytes(&normp, index + 1) else {
                return (p.to_vec(), Vec::new(), Vec::new());
            };
            return (
                take(0, index2),
                take(index2, index2 + 1),
                take(index2 + 1, n),
            );
        }
        return (Vec::new(), take(0, 1), take(1, n));
    }
    if normp.get(1) == Some(&b':') {
        if normp.get(2) == Some(&b'\\') {
            return (take(0, 2), take(2, 3), take(3, n));
        }
        return (take(0, 2), Vec::new(), take(2, n));
    }
    (Vec::new(), Vec::new(), p.to_vec())
}

pub(crate) fn nt_splitdrive_str(p: &str) -> (String, String) {
    let (drive, root, tail) = nt_splitroot_str(p);
    (drive, root + &tail)
}

pub(crate) fn nt_splitdrive_bytes(p: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (drive, mut root, tail) = nt_splitroot_bytes(p);
    root.extend_from_slice(&tail);
    (drive, root)
}

pub(crate) fn nt_split_str(p: &str) -> (String, String) {
    let (d, r, rest) = nt_splitroot_str(p);
    let chars: Vec<char> = rest.chars().collect();
    let mut i = chars.len();
    while i > 0 && !matches!(chars[i - 1], '\\' | '/') {
        i -= 1;
    }
    let head: String = chars[..i].iter().collect();
    let tail: String = chars[i..].iter().collect();
    let head = head.trim_end_matches(['\\', '/']);
    (format!("{d}{r}{head}"), tail)
}

pub(crate) fn nt_split_bytes(p: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut d, r, rest) = nt_splitroot_bytes(p);
    let mut i = rest.len();
    while i > 0 && !matches!(rest[i - 1], b'\\' | b'/') {
        i -= 1;
    }
    let mut head = &rest[..i];
    let tail = rest[i..].to_vec();
    while head.last().is_some_and(|&b| matches!(b, b'\\' | b'/')) {
        head = &head[..head.len() - 1];
    }
    d.extend_from_slice(&r);
    d.extend_from_slice(head);
    (d, tail)
}

pub(crate) fn nt_isabs_str(s: &str) -> bool {
    let head: Vec<char> = s
        .chars()
        .take(3)
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    head.first() == Some(&'\\') || (head.get(1) == Some(&':') && head.get(2) == Some(&'\\'))
}

pub(crate) fn nt_isabs_bytes(s: &[u8]) -> bool {
    let head: Vec<u8> = s
        .iter()
        .take(3)
        .map(|&b| if b == b'/' { b'\\' } else { b })
        .collect();
    head.first() == Some(&b'\\') || (head.get(1) == Some(&b':') && head.get(2) == Some(&b'\\'))
}

fn ends_with_sep_str(s: &str) -> bool {
    s.ends_with(['\\', '/'])
}

fn ends_with_colon_or_sep_str(s: &str) -> bool {
    s.ends_with([':', '\\', '/'])
}

pub(crate) fn nt_join_str(parts: &[String]) -> String {
    if parts.is_empty() {
        return String::new();
    }
    let (mut result_drive, mut result_root, mut result_path) = nt_splitroot_str(&parts[0]);
    for p in &parts[1..] {
        let (p_drive, p_root, p_path) = nt_splitroot_str(p);
        if !p_root.is_empty() {
            if !p_drive.is_empty() || result_drive.is_empty() {
                result_drive = p_drive;
            }
            result_root = p_root;
            result_path = p_path;
            continue;
        } else if !p_drive.is_empty() && p_drive != result_drive {
            if p_drive.to_lowercase() != result_drive.to_lowercase() {
                result_drive = p_drive;
                result_root = p_root;
                result_path = p_path;
                continue;
            }
            result_drive = p_drive;
        }
        if !result_path.is_empty() && !ends_with_sep_str(&result_path) {
            result_path.push('\\');
        }
        result_path.push_str(&p_path);
    }
    if !result_path.is_empty()
        && result_root.is_empty()
        && !result_drive.is_empty()
        && !ends_with_colon_or_sep_str(&result_drive)
    {
        return format!("{result_drive}\\{result_path}");
    }
    format!("{result_drive}{result_root}{result_path}")
}

pub(crate) fn nt_join_bytes(parts: &[Vec<u8>]) -> Vec<u8> {
    if parts.is_empty() {
        return Vec::new();
    }
    let (mut result_drive, mut result_root, mut result_path) = nt_splitroot_bytes(&parts[0]);
    for p in &parts[1..] {
        let (p_drive, p_root, p_path) = nt_splitroot_bytes(p);
        if !p_root.is_empty() {
            if !p_drive.is_empty() || result_drive.is_empty() {
                result_drive = p_drive;
            }
            result_root = p_root;
            result_path = p_path;
            continue;
        } else if !p_drive.is_empty() && p_drive != result_drive {
            if !p_drive.eq_ignore_ascii_case(&result_drive) {
                result_drive = p_drive;
                result_root = p_root;
                result_path = p_path;
                continue;
            }
            result_drive = p_drive;
        }
        if !result_path.is_empty()
            && !result_path
                .last()
                .is_some_and(|&b| matches!(b, b'\\' | b'/'))
        {
            result_path.push(b'\\');
        }
        result_path.extend_from_slice(&p_path);
    }
    if !result_path.is_empty()
        && result_root.is_empty()
        && !result_drive.is_empty()
        && !result_drive
            .last()
            .is_some_and(|&b| matches!(b, b':' | b'\\' | b'/'))
    {
        let mut out = result_drive;
        out.push(b'\\');
        out.extend_from_slice(&result_path);
        return out;
    }
    let mut out = result_drive;
    out.extend_from_slice(&result_root);
    out.extend_from_slice(&result_path);
    out
}

pub(crate) fn nt_normpath_str(path: &str) -> String {
    let path: String = path
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    // Preserve verbatim extended-length / device paths (\\?\ and \\.\).
    if path.starts_with(r"\\.\") || path.starts_with(r"\\?\") {
        return path;
    }
    let (drive, root, rest) = nt_splitroot_str(&path);
    let prefix = format!("{drive}{root}");
    let has_root = !root.is_empty();
    let mut comps: Vec<&str> = rest.split('\\').collect();
    let mut i = 0;
    while i < comps.len() {
        if comps[i].is_empty() || comps[i] == "." {
            comps.remove(i);
        } else if comps[i] == ".." {
            if i > 0 && comps[i - 1] != ".." {
                comps.remove(i);
                comps.remove(i - 1);
                i -= 1;
            } else if i == 0 && has_root {
                comps.remove(i);
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    if prefix.is_empty() && comps.is_empty() {
        comps.push(".");
    }
    format!("{}{}", prefix, comps.join("\\"))
}

pub(crate) fn nt_normpath_bytes(path: &[u8]) -> Vec<u8> {
    let path: Vec<u8> = path
        .iter()
        .map(|&b| if b == b'/' { b'\\' } else { b })
        .collect();
    if path.starts_with(b"\\\\.\\") || path.starts_with(b"\\\\?\\") {
        return path;
    }
    let (mut prefix, root, rest) = nt_splitroot_bytes(&path);
    let has_root = !root.is_empty();
    prefix.extend_from_slice(&root);
    let mut comps: Vec<&[u8]> = rest.split(|&b| b == b'\\').collect();
    let mut i = 0;
    while i < comps.len() {
        if comps[i].is_empty() || comps[i] == b"." {
            comps.remove(i);
        } else if comps[i] == b".." {
            if i > 0 && comps[i - 1] != b".." {
                comps.remove(i);
                comps.remove(i - 1);
                i -= 1;
            } else if i == 0 && has_root {
                comps.remove(i);
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    if prefix.is_empty() && comps.is_empty() {
        comps.push(b".");
    }
    for (idx, comp) in comps.iter().enumerate() {
        if idx > 0 {
            prefix.push(b'\\');
        }
        prefix.extend_from_slice(comp);
    }
    prefix
}

pub(crate) fn nt_abspath_str(path: &str) -> Result<String> {
    if nt_isabs_str(path) {
        return Ok(nt_normpath_str(path));
    }
    let cwd = std::env::current_dir()
        .map_err(|e| PyError::named("OSError", format!("could not resolve cwd: {e}")))?;
    let cwd = cwd.to_string_lossy().into_owned();
    Ok(nt_normpath_str(&nt_join_str(&[cwd, path.to_string()])))
}

pub(crate) fn nt_abspath_bytes(path: &[u8]) -> Result<Vec<u8>> {
    if nt_isabs_bytes(path) {
        return Ok(nt_normpath_bytes(path));
    }
    let cwd = std::env::current_dir()
        .map_err(|e| PyError::named("OSError", format!("could not resolve cwd: {e}")))?;
    let cwd_bytes = path_buf_to_bytes(&cwd);
    Ok(nt_normpath_bytes(&nt_join_bytes(&[
        cwd_bytes,
        path.to_vec(),
    ])))
}

pub(crate) fn nt_relpath_str(path: &str, start: &str) -> Result<String> {
    if path.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "no path specified".to_string(),
        ));
    }
    let start_abs = nt_abspath_str(&nt_normpath_str(start))?;
    let path_abs = nt_abspath_str(&nt_normpath_str(path))?;
    let (start_drive, _, start_rest) = nt_splitroot_str(&start_abs);
    let (path_drive, _, path_rest) = nt_splitroot_str(&path_abs);
    if start_drive.to_lowercase() != path_drive.to_lowercase() {
        return Err(PyError::named(
            "ValueError",
            format!("path is on mount '{path_drive}', start on mount '{start_drive}'"),
        ));
    }
    let start_list: Vec<&str> = start_rest.split('\\').filter(|s| !s.is_empty()).collect();
    let path_list: Vec<&str> = path_rest.split('\\').filter(|s| !s.is_empty()).collect();
    let common = start_list
        .iter()
        .zip(path_list.iter())
        .take_while(|(a, b)| a.to_lowercase() == b.to_lowercase())
        .count();
    let mut rel: Vec<String> = vec!["..".to_string(); start_list.len() - common];
    rel.extend(path_list[common..].iter().map(|s| s.to_string()));
    if rel.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(nt_join_str(&rel))
    }
}

pub(crate) fn nt_commonpath_str(paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "commonpath() arg is an empty sequence".to_string(),
        ));
    }
    let normalized: Vec<String> = paths.iter().map(|p| p.replace('/', "\\")).collect();
    let split: Vec<(String, String, String)> =
        normalized.iter().map(|p| nt_splitroot_str(p)).collect();

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

pub(crate) fn nt_expanduser_str(path: &str) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    let i = path
        .char_indices()
        .skip(1)
        .find(|(_, c)| matches!(c, '\\' | '/'))
        .map(|(idx, _)| idx)
        .unwrap_or(path.len());
    if i != 1 {
        return path.to_string();
    }
    let userhome = if let Some(up) = std::env::var_os("USERPROFILE") {
        up.to_string_lossy().into_owned()
    } else if let Some(hp) = std::env::var_os("HOMEPATH") {
        let drive = std::env::var_os("HOMEDRIVE")
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        nt_join_str(&[drive, hp.to_string_lossy().into_owned()])
    } else {
        return path.to_string();
    };
    format!("{userhome}{}", &path[i..])
}

pub(crate) fn nt_expandvars_str(path: &str) -> String {
    if !path.contains('$') && !path.contains('%') {
        return path.to_string();
    }
    let chars: Vec<char> = path.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < len {
        match chars[i] {
            '\'' => {
                out.push('\'');
                i += 1;
                while i < len {
                    let c = chars[i];
                    out.push(c);
                    i += 1;
                    if c == '\'' {
                        break;
                    }
                }
            }
            '%' => {
                if i + 1 < len && chars[i + 1] == '%' {
                    out.push('%');
                    i += 2;
                    continue;
                }
                let start = i + 1;
                let mut end = start;
                while end < len && chars[end] != '%' {
                    end += 1;
                }
                if end < len && chars[end] == '%' {
                    let name: String = chars[start..end].iter().collect();
                    if let Ok(val) = std::env::var(&name) {
                        out.push_str(&val);
                    } else {
                        out.push('%');
                        out.push_str(&name);
                        out.push('%');
                    }
                    i = end + 1;
                } else {
                    out.push('%');
                    i += 1;
                }
            }
            '$' => {
                if i + 1 < len && chars[i + 1] == '$' {
                    out.push('$');
                    i += 2;
                    continue;
                }
                if i + 1 < len && chars[i + 1] == '{' {
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
                } else if i + 1 < len
                    && (chars[i + 1].is_ascii_alphanumeric() || chars[i + 1] == '_')
                {
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
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Platform check for Windows NTFS directory junctions (`IO_REPARSE_TAG_MOUNT_POINT`).
pub(crate) fn path_is_junction(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FindClose, FindFirstFileW, WIN32_FIND_DATAW,
        };
        const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;

        let s = path.to_string_lossy();
        let trimmed = s.trim_end_matches(['\\', '/']);
        if trimmed.is_empty() || trimmed.ends_with(':') {
            return false;
        }
        let mut wide: Vec<u16> = std::ffi::OsStr::new(trimmed).encode_wide().collect();
        wide.push(0);
        // SAFETY: `wide` is NUL-terminated; `FindFirstFileW` writes into `data`
        // and any valid search handle is closed via `FindClose`.
        unsafe {
            let mut data: WIN32_FIND_DATAW = std::mem::zeroed();
            let handle = FindFirstFileW(wide.as_ptr(), &mut data);
            if handle == INVALID_HANDLE_VALUE {
                return false;
            }
            FindClose(handle);
            (data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0
                && data.dwReserved0 == IO_REPARSE_TAG_MOUNT_POINT
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

// ── Value-level adapters shared by `ntpath` and Windows `os.path` ────────────

pub(crate) fn normcase(interp: &mut Interpreter, s: &Value) -> Result<Value> {
    match fspath_resolve(interp, s)? {
        PathValue::Str(st) => Ok(Value::string(st.replace('/', "\\").to_lowercase())),
        PathValue::Bytes(b) => {
            let out: Vec<u8> = b
                .iter()
                .map(|&ch| {
                    let ch = if ch == b'/' { b'\\' } else { ch };
                    ch.to_ascii_lowercase()
                })
                .collect();
            Ok(Value::bytes(out))
        }
    }
}

pub(crate) fn isabs(interp: &mut Interpreter, s: &Value) -> Result<Value> {
    let ok = match fspath_resolve(interp, s)? {
        PathValue::Str(st) => nt_isabs_str(&st),
        PathValue::Bytes(b) => nt_isabs_bytes(&b),
    };
    Ok(Value::bool_(ok))
}

pub(crate) fn join(interp: &mut Interpreter, first: &Value, rest: &[Value]) -> Result<Value> {
    match fspath_resolve(interp, first)? {
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
            Ok(Value::string(nt_join_str(&parts)))
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
            Ok(Value::bytes(nt_join_bytes(&parts)))
        }
    }
}

pub(crate) fn split(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => {
            let (h, t) = nt_split_str(&s);
            Ok(Value::tuple(vec![Value::string(h), Value::string(t)]))
        }
        PathValue::Bytes(b) => {
            let (h, t) = nt_split_bytes(&b);
            Ok(Value::tuple(vec![Value::bytes(h), Value::bytes(t)]))
        }
    }
}

pub(crate) fn splitroot(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => {
            let (d, r, t) = nt_splitroot_str(&s);
            Ok(Value::tuple(vec![
                Value::string(d),
                Value::string(r),
                Value::string(t),
            ]))
        }
        PathValue::Bytes(b) => {
            let (d, r, t) = nt_splitroot_bytes(&b);
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
    Ok(splitext_value(pv, b'\\', Some(b'/'), b'.'))
}

pub(crate) fn splitdrive(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => {
            let (d, t) = nt_splitdrive_str(&s);
            Ok(Value::tuple(vec![Value::string(d), Value::string(t)]))
        }
        PathValue::Bytes(b) => {
            let (d, t) = nt_splitdrive_bytes(&b);
            Ok(Value::tuple(vec![Value::bytes(d), Value::bytes(t)]))
        }
    }
}

pub(crate) fn basename(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(nt_split_str(&s).1)),
        PathValue::Bytes(b) => Ok(Value::bytes(nt_split_bytes(&b).1)),
    }
}

pub(crate) fn dirname(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(nt_split_str(&s).0)),
        PathValue::Bytes(b) => Ok(Value::bytes(nt_split_bytes(&b).0)),
    }
}

pub(crate) fn normpath(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(nt_normpath_str(&s))),
        PathValue::Bytes(b) => Ok(Value::bytes(nt_normpath_bytes(&b))),
    }
}

pub(crate) fn abspath(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(nt_abspath_str(&s)?)),
        PathValue::Bytes(b) => Ok(Value::bytes(nt_abspath_bytes(&b)?)),
    }
}

pub(crate) fn realpath(interp: &mut Interpreter, path: &Value, strict: bool) -> Result<Value> {
    let pv = fspath_resolve(interp, path)?;
    let is_bytes = pv.is_bytes();
    let path_str = pv.to_lossy_string();
    let abs = nt_abspath_str(&path_str)?;
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
                    found = Some(nt_normpath_str(&out.to_string_lossy()));
                    break;
                }
            }
            found.unwrap_or_else(|| nt_normpath_str(&abs))
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
            Ok(Value::string(nt_relpath_str(&p_str, &start_str)?))
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
            let p_str = String::from_utf8_lossy(&p_bytes).into_owned();
            let s_str = String::from_utf8_lossy(&start_bytes).into_owned();
            Ok(Value::bytes(nt_relpath_str(&p_str, &s_str)?.into_bytes()))
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
            Ok(Value::string(nt_commonpath_str(&all)?))
        }
        PathValue::Bytes(b0) => {
            let mut all = Vec::with_capacity(items.len());
            all.push(String::from_utf8_lossy(&b0).into_owned());
            for it in &items[1..] {
                match fspath_resolve(interp, it)? {
                    PathValue::Bytes(b) => all.push(String::from_utf8_lossy(&b).into_owned()),
                    PathValue::Str(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "Can't mix strings and bytes in path components".to_string(),
                        ));
                    }
                }
            }
            Ok(Value::bytes(nt_commonpath_str(&all)?.into_bytes()))
        }
    }
}

pub(crate) fn expanduser(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(nt_expanduser_str(&s))),
        PathValue::Bytes(b) => {
            let s = String::from_utf8_lossy(&b);
            Ok(Value::bytes(nt_expanduser_str(&s).into_bytes()))
        }
    }
}

pub(crate) fn expandvars(interp: &mut Interpreter, p: &Value) -> Result<Value> {
    match fspath_resolve(interp, p)? {
        PathValue::Str(s) => Ok(Value::string(nt_expandvars_str(&s))),
        PathValue::Bytes(b) => {
            let s = String::from_utf8_lossy(&b);
            Ok(Value::bytes(nt_expandvars_str(&s).into_bytes()))
        }
    }
}

pub(crate) fn isjunction(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let pv = fspath_resolve(interp, path)?;
    let Ok(pb) = pv.to_path_buf() else {
        return Ok(Value::bool_(false));
    };
    Ok(Value::bool_(path_is_junction(&pb)))
}

pub(crate) fn isdevdrive(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let pv = fspath_resolve(interp, path)?;
    let Ok(_pb) = pv.to_path_buf() else {
        return Ok(Value::bool_(false));
    };
    // Standard NTFS/FAT/POSIX volumes are not Windows ReFS Dev Drives; CPython
    // also returns False whenever the volume cannot be queried or is not a Dev
    // Drive.
    Ok(Value::bool_(false))
}

pub(crate) fn ismount(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let pv = fspath_resolve(interp, path)?;
    let raw = pv.to_lossy_string();
    if raw.as_bytes().contains(&0) {
        return Ok(Value::bool_(false));
    }
    let abs = match nt_abspath_str(&raw) {
        Ok(a) => a,
        Err(_) => return Ok(Value::bool_(false)),
    };
    let (drive, root, rest) = nt_splitroot_str(&abs);
    if !rest.is_empty() {
        #[cfg(windows)]
        {
            if path_is_junction(std::path::Path::new(&abs)) {
                return Ok(Value::bool_(true));
            }
            // Check volume mount point via GetVolumePathNameW.
            use std::os::windows::ffi::{OsStrExt, OsStringExt};
            use windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW;
            let mut wide: Vec<u16> = std::ffi::OsStr::new(&abs).encode_wide().collect();
            wide.push(0);
            let mut buf = vec![0u16; wide.len() + 260];
            // SAFETY: both buffers are valid pointers with explicit length.
            let ok =
                unsafe { GetVolumePathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
            if ok != 0 {
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                let vol = std::ffi::OsString::from_wide(&buf[..end])
                    .to_string_lossy()
                    .trim_end_matches(['\\', '/'])
                    .to_lowercase();
                let target = abs.trim_end_matches(['\\', '/']).to_lowercase();
                if !vol.is_empty() && vol == target {
                    return Ok(Value::bool_(true));
                }
            }
        }
        return Ok(Value::bool_(false));
    }
    // UNC root (\\server\share or \\server\share\) or drive/root (C:\, /, \).
    let drive_chars: Vec<char> = drive.chars().collect();
    if drive_chars.len() >= 2
        && matches!(drive_chars[0], '\\' | '/')
        && matches!(drive_chars[1], '\\' | '/')
    {
        return Ok(Value::bool_(true));
    }
    Ok(Value::bool_(!root.is_empty()))
}
