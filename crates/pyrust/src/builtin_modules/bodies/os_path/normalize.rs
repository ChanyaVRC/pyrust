//! Path normalization, resolution, and home-expansion helpers for `os.path`.

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{Value, ValueKind};

use super::split_join::{fspath_to_str, fspath_value, nt_join, nt_splitroot};

pub(super) fn normcase_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    let resolved = fspath_value(interp, &args[0].value)?;
    match resolved.kind() {
        ValueKind::Str(s) => {
            if cfg!(windows) {
                Ok(Value::string(s.replace('/', "\\").to_lowercase()))
            } else {
                Ok(Value::string(s))
            }
        }
        ValueKind::Bytes(b) => {
            if cfg!(windows) {
                let normalized: Vec<u8> = b
                    .iter()
                    .map(|&ch| {
                        let ch = if ch == b'/' { b'\\' } else { ch };
                        ch.to_ascii_lowercase()
                    })
                    .collect();
                Ok(Value::bytes(normalized))
            } else {
                Ok(Value::bytes(b.as_ref().clone()))
            }
        }
        _ => unreachable!("fspath_value only returns str or bytes"),
    }
}

pub(super) fn relpath_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.is_empty() || args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes 1 or 2 arguments ({} given)", args.len()),
        ));
    }
    let path = fspath_to_str(interp, fn_name, &args[0].value)?;
    // CPython raises ValueError for an empty `path`.
    if path.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "no path specified".to_string(),
        ));
    }
    let start = match args.get(1) {
        None => ".".to_string(),
        Some(a) if a.value.is_none() => ".".to_string(),
        Some(a) => fspath_to_str(interp, fn_name, &a.value)?,
    };
    Ok(Value::string(host_relpath(&path, &start)?))
}

pub(super) fn expandvars_impl(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    let path = fspath_to_str(interp, fn_name, &args[0].value)?;
    Ok(Value::string(host_expandvars(&path)))
}

pub(super) fn host_expandvars(path: &str) -> String {
    if cfg!(windows) {
        nt_expandvars(path)
    } else {
        posix_expandvars(path)
    }
}

/// POSIX `os.path.expandvars`: expand `$VAR` and `${VAR}` using the live
/// process environment; unknown variables are left unchanged.
pub(super) fn posix_expandvars(path: &str) -> String {
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

/// Windows `ntpath.expandvars`: expand `$VAR`, `${VAR}`, and `%VAR%`
/// (preserving `'...'` quoted spans and translating `$$` -> `$` and `%%` -> `%`).
pub(super) fn nt_expandvars(path: &str) -> String {
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
                // Single-quoted literal span: copy verbatim through closing quote.
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

// ---------------------------------------------------------------------------
// Host-OS dispatch for normalization operations
// ---------------------------------------------------------------------------

pub(super) fn host_isabs(path: &str) -> bool {
    if cfg!(windows) {
        nt_isabs(path)
    } else {
        path.starts_with('/')
    }
}

pub(super) fn host_normpath(path: &str) -> String {
    if cfg!(windows) {
        nt_normpath(path)
    } else {
        posix_normpath(path)
    }
}

pub(super) fn host_abspath(path: &str) -> Result<String> {
    if cfg!(windows) {
        nt_abspath(path)
    } else {
        posix_abspath(path)
    }
}

pub(super) fn host_relpath(path: &str, start: &str) -> Result<String> {
    if cfg!(windows) {
        nt_relpath(path, start)
    } else {
        posix_relpath(path, start)
    }
}

pub(super) fn host_expanduser(path: &str) -> String {
    if cfg!(windows) {
        nt_expanduser(path)
    } else {
        posix_expanduser(path)
    }
}

// ---------------------------------------------------------------------------
// POSIX normalization helpers
// ---------------------------------------------------------------------------

/// POSIX `os.path.normpath`. Mirrors CPython's `posixpath.normpath`, including
/// the "exactly two leading slashes" special case.
pub(super) fn posix_normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    // POSIX: a path beginning with exactly two slashes is implementation
    // defined and preserved; three-or-more collapse to one.
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

/// POSIX `os.path.abspath` — join with cwd if relative, then normpath.
pub(super) fn posix_abspath(path: &str) -> Result<String> {
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
    Ok(posix_normpath(&joined))
}

/// POSIX `os.path.relpath` — relative path from `start` to `path`.
pub(super) fn posix_relpath(path: &str, start: &str) -> Result<String> {
    let abs_path = posix_abspath(path)?;
    let abs_start = posix_abspath(start)?;
    let path_parts: Vec<&str> = abs_path.split('/').filter(|s| !s.is_empty()).collect();
    let start_parts: Vec<&str> = abs_start.split('/').filter(|s| !s.is_empty()).collect();
    // Length of the shared leading component run.
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

/// POSIX `os.path.expanduser` — expand a leading `~` using `$HOME`.
pub(super) fn posix_expanduser(path: &str) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    // Find the end of the user part (`~` or `~user`): up to the first slash.
    let rest_start = path.find('/').unwrap_or(path.len());
    let user_part = &path[1..rest_start];
    if !user_part.is_empty() {
        // `~user` — pyrust does not resolve other users' home dirs.
        return path.to_string();
    }
    let home = match std::env::var_os("HOME") {
        Some(h) => h.to_string_lossy().into_owned(),
        None => return path.to_string(),
    };
    // CPython strips a trailing slash from HOME unless HOME == "/".
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
        // `~` alone -> home (which may be "/").
        if result == "/" {
            // Avoid returning "" when home was "/".
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

// ---------------------------------------------------------------------------
// Windows (ntpath) normalization helpers
// ---------------------------------------------------------------------------

pub(super) fn nt_isabs(s: &str) -> bool {
    // CPython only inspects the first three chars and treats `/` as `\`.
    let head: String = s
        .chars()
        .take(3)
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    let hb: Vec<char> = head.chars().collect();
    // UNC/device (`\\...`) or drive-with-root (`C:\...`). Note the legacy bug:
    // `isabs("/x")` is True because a leading separator matches the first branch.
    hb.first() == Some(&'\\') || (hb.get(1) == Some(&':') && hb.get(2) == Some(&'\\'))
}

pub(super) fn nt_normpath(path: &str) -> String {
    let path: String = path
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    let (drive, root, rest) = nt_splitroot(&path);
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
    // If the path is now empty, substitute '.'.
    if prefix.is_empty() && comps.is_empty() {
        comps.push(".");
    }
    format!("{}{}", prefix, comps.join("\\"))
}

pub(super) fn nt_abspath(path: &str) -> Result<String> {
    if nt_isabs(path) {
        return Ok(nt_normpath(path));
    }
    let cwd = std::env::current_dir()
        .map_err(|e| PyError::named("OSError", format!("could not resolve cwd: {e}")))?;
    let cwd = cwd.to_string_lossy().into_owned();
    Ok(nt_normpath(&nt_join(&[cwd, path.to_string()])))
}

pub(super) fn nt_relpath(path: &str, start: &str) -> Result<String> {
    if path.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "no path specified".to_string(),
        ));
    }
    let start_abs = nt_abspath(&nt_normpath(start))?;
    let path_abs = nt_abspath(&nt_normpath(path))?;
    let (start_drive, _, start_rest) = nt_splitroot(&start_abs);
    let (path_drive, _, path_rest) = nt_splitroot(&path_abs);
    if start_drive.to_lowercase() != path_drive.to_lowercase() {
        return Err(PyError::named(
            "ValueError",
            format!("path is on mount '{path_drive}', start on mount '{start_drive}'"),
        ));
    }
    let start_list: Vec<&str> = start_rest.split('\\').filter(|s| !s.is_empty()).collect();
    let path_list: Vec<&str> = path_rest.split('\\').filter(|s| !s.is_empty()).collect();
    // Case-insensitive component comparison (Windows is case-folding).
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
        Ok(nt_join(&rel))
    }
}

pub(super) fn nt_expanduser(path: &str) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    // End of the user part: up to the first separator.
    let i = path
        .char_indices()
        .skip(1)
        .find(|(_, c)| matches!(c, '\\' | '/'))
        .map(|(idx, _)| idx)
        .unwrap_or(path.len());
    // pyrust does not query the user database, so `~user` is left unchanged.
    if i != 1 {
        return path.to_string();
    }
    // CPython prefers USERPROFILE, then HOMEDRIVE+HOMEPATH.
    let userhome = if let Some(up) = std::env::var_os("USERPROFILE") {
        up.to_string_lossy().into_owned()
    } else if let Some(hp) = std::env::var_os("HOMEPATH") {
        let drive = std::env::var_os("HOMEDRIVE")
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        nt_join(&[drive, hp.to_string_lossy().into_owned()])
    } else {
        return path.to_string();
    };
    format!("{userhome}{}", &path[i..])
}
