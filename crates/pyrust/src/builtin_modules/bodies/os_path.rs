// `os.path` module — included into `pub mod os_path { … }` declared by
// the `"os.path" as os_path` entry in `pyrust_builtin_modules!` in
// `builtin_modules/mod.rs`.  This is the first module that exercises
// the dotted-Python-name form of `pyrust_builtin_modules!`: the macro
// injects `MODULE_NAME = "os.path"` and `FN_PREFIX = "os.path."`, so
// the registered names look like `"os.path.join"`.
//
// ## Import forms (all three work)
//
// pyrust's compiler binds `import os.path` to the topmost component
// (the name `os`) to match CPython's package semantics.  The `os`
// parent package — shipped in `bodies/os.rs` — exposes `path` as an
// attribute pointing at this module, so all three import patterns
// resolve correctly:
//
//     import os.path                  # os.path.join(...)
//     import os.path as op            # op.join(...)
//     from os.path import join        # join(...)
//
// Reference: <https://docs.python.org/3/library/os.path.html>

use std::path::Path;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::reject_keyword_args_expanded;
use crate::value::{Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        // `os.sep` lives on `os`, but pyrust doesn't yet ship the parent
        // module; expose the path separator here so callers can build
        // platform-aware paths without depending on `os` proper.
        "sep" => Value::string(std::path::MAIN_SEPARATOR.to_string()),
    }

    /// CPython: os.path.join(path, *paths) — join path components.
    /// <https://docs.python.org/3/library/os.path.html#os.path.join>
    fn join(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() {
            // CPython raises `TypeError` for wrong-arity calls — match
            // that so user `except TypeError:` blocks catch it.
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at least one argument"),
            ));
        }
        let mut parts: Vec<String> = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            match a.value.kind() {
                ValueKind::Str(s) => parts.push(s.to_string()),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument {} must be str", i + 1),
                )),
            }
        }
        Ok(Value::string(host_join(&parts)))
    }

    /// CPython: os.path.exists(path) — true if `path` refers to an
    /// existing path (broken symlinks return False).
    /// <https://docs.python.org/3/library/os.path.html#os.path.exists>
    fn exists(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::bool_(Path::new(&path).exists()))
    }

    /// CPython: os.path.isfile(path) — true if `path` is a regular file.
    /// <https://docs.python.org/3/library/os.path.html#os.path.isfile>
    fn isfile(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::bool_(Path::new(&path).is_file()))
    }

    /// CPython: os.path.isdir(path) — true if `path` is a directory.
    /// <https://docs.python.org/3/library/os.path.html#os.path.isdir>
    fn isdir(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::bool_(Path::new(&path).is_dir()))
    }

    /// CPython: os.path.dirname(path) — directory name of `path` (all
    /// components except the last).  Empty string when `path` has no
    /// directory component.
    /// <https://docs.python.org/3/library/os.path.html#os.path.dirname>
    fn dirname(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        // CPython: `dirname(p) == split(p)[0]`.
        let (head, _) = host_split(&path);
        Ok(Value::string(head))
    }

    /// CPython: os.path.basename(path) — final path component, or `""`
    /// if `path` ends with a separator.
    /// <https://docs.python.org/3/library/os.path.html#os.path.basename>
    fn basename(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        // CPython: `basename(p) == split(p)[1]`.
        let (_, tail) = host_split(&path);
        Ok(Value::string(tail))
    }

    /// CPython: os.path.abspath(path) — absolute normalised form of `path`.
    /// Uses the process's current working directory; raises on cwd
    /// resolution failure (matches CPython behaviour).
    /// <https://docs.python.org/3/library/os.path.html#os.path.abspath>
    fn abspath(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::string(host_abspath(&path)?))
    }

    /// CPython: os.path.splitext(path) → (root, ext) — split a path into
    /// `(everything-up-to-the-last-dot, ".ext")`, with no leading dot
    /// counted (so `splitext(".bashrc")` → `(".bashrc", "")`).
    /// <https://docs.python.org/3/library/os.path.html#os.path.splitext>
    fn splitext(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let (root, ext) = host_splitext(&path);
        Ok(Value::tuple(vec![Value::string(root), Value::string(ext)]))
    }

    /// CPython: os.path.split(path) → (head, tail) — split into the
    /// directory part and the final component.  `tail` never contains a
    /// slash; if `path` ends with a slash `tail` is empty.  Trailing
    /// slashes are stripped from `head` unless `head` is all slashes
    /// (the root).  POSIX semantics.
    /// <https://docs.python.org/3/library/os.path.html#os.path.split>
    fn split(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let (head, tail) = host_split(&path);
        Ok(Value::tuple(vec![Value::string(head), Value::string(tail)]))
    }

    /// CPython: os.path.isabs(path) — true if `path` begins with a slash
    /// (POSIX), or is UNC/device/drive-rooted (Windows ntpath).
    /// <https://docs.python.org/3/library/os.path.html#os.path.isabs>
    fn isabs(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::bool_(host_isabs(&path)))
    }

    /// CPython: os.path.normpath(path) — collapse redundant separators
    /// and up-level references (`.` / `..`).  POSIX semantics, including
    /// the special case where exactly two leading slashes are preserved.
    /// <https://docs.python.org/3/library/os.path.html#os.path.normpath>
    fn normpath(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::string(host_normpath(&path)))
    }

    /// CPython: os.path.splitdrive(path) → (drive, tail).  On POSIX the
    /// drive is always empty, so this returns `("", path)`.  On Windows
    /// (ntpath) it splits off a drive letter (`C:`) or UNC sharepoint.
    /// <https://docs.python.org/3/library/os.path.html#os.path.splitdrive>
    fn splitdrive(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let (drive, tail) = host_splitdrive(&path);
        Ok(Value::tuple(vec![Value::string(drive), Value::string(tail)]))
    }

    /// CPython: os.path.commonprefix(list) — longest common leading
    /// substring (character-by-character, NOT path-component aware) of
    /// the supplied paths.  Returns `""` for an empty list.
    /// <https://docs.python.org/3/library/os.path.html#os.path.commonprefix>
    fn commonprefix(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one argument ({} given)", args.len()),
            ));
        }
        // CPython coerces each element via os.fspath / str; we accept the
        // common case of a list/tuple of str (what the parity tests and
        // real callers use). Non-str elements raise TypeError, mirroring
        // the failure a caller would hit when the items aren't strings.
        let items = _interp.collect_iterable(&args[0].value)?;
        if items.is_empty() {
            return Ok(Value::string(String::new()));
        }
        let mut strs: Vec<String> = Vec::with_capacity(items.len());
        for it in &items {
            match it.kind() {
                ValueKind::Str(s) => strs.push(s.to_string()),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() argument must be a sequence of str, not {}",
                            crate::interpreter::value_type_name_str(it),
                        ),
                    ))
                }
            }
        }
        // Longest common prefix over Unicode scalar values. CPython
        // compares the strings directly (min vs max suffices because
        // string ordering makes the shared prefix bounded by both).
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

    /// CPython: os.path.relpath(path, start=os.curdir) — relative path
    /// from `start` to `path`.  Both are made absolute (via the process
    /// cwd) before the common-prefix computation, matching posixpath.
    /// <https://docs.python.org/3/library/os.path.html#os.path.relpath>
    fn relpath(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes 1 or 2 arguments ({} given)", args.len()),
            ));
        }
        let path = match args[0].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument must be str"),
                ))
            }
        };
        // CPython raises ValueError for an empty `path`.
        if path.is_empty() {
            return Err(PyError::named(
                "ValueError",
                "no path specified".to_string(),
            ));
        }
        let start = match args.get(1).map(|a| a.value.kind()) {
            Some(ValueKind::Str(s)) => s.to_string(),
            None => ".".to_string(),
            Some(_) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument must be str"),
                ))
            }
        };
        Ok(Value::string(host_relpath(&path, &start)?))
    }

    /// CPython: os.path.expanduser(path) — expand a leading `~` (current
    /// user) or `~user` to the corresponding home directory.  Uses the
    /// `HOME` environment variable; if `HOME` is unset or the path has
    /// no leading `~`, the path is returned unchanged.  `~user` is not
    /// resolved (returned unchanged) since pyrust does not query the
    /// password database.
    /// <https://docs.python.org/3/library/os.path.html#os.path.expanduser>
    fn expanduser(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::string(host_expanduser(&path)))
    }

    /// CPython: os.path.realpath(path) — canonical path, resolving
    /// symlinks where possible.  pyrust resolves via the filesystem when
    /// the path exists, otherwise falls back to `abspath` + `normpath`
    /// (which is what posixpath does for non-existent components).
    /// <https://docs.python.org/3/library/os.path.html#os.path.realpath>
    fn realpath(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let abs = host_abspath(&path)?;
        match std::fs::canonicalize(&abs) {
            Ok(p) => Ok(Value::string(p.to_string_lossy())),
            Err(_) => Ok(Value::string(host_normpath(&abs))),
        }
    }
}

/// POSIX `os.path.split` — split at the last slash.  The head has its
/// trailing slashes stripped unless it consists entirely of slashes.
fn posix_split(path: &str) -> (String, String) {
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

/// POSIX `os.path.normpath`.  Mirrors CPython's posixpath.normpath,
/// including the "exactly two leading slashes" special case.
fn posix_normpath(path: &str) -> String {
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
fn posix_abspath(path: &str) -> Result<String> {
    let joined = if path.starts_with('/') {
        path.to_string()
    } else {
        let cwd = std::env::current_dir().map_err(|e| {
            PyError::named("OSError", format!("could not resolve cwd: {e}"))
        })?;
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
fn posix_relpath(path: &str, start: &str) -> Result<String> {
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
fn posix_expanduser(path: &str) -> String {
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
    let mut result = if home.is_empty() { "/".to_string() } else { home };
    let rest = &path[rest_start..];
    if rest.is_empty() {
        // `~` alone → home (which may be "/").
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

/// Common preamble for the single-`str`-arg variants.  Rejects kwargs,
/// requires exactly one positional, and pulls out the string contents
/// (or a TypeError if the caller passed something else).
fn single_path(fn_name: &str, args: &[ExpandedCallArg]) -> Result<String> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        // CPython raises `TypeError` for wrong-arity calls.
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    match args[0].value.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() argument must be str"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Host-OS dispatch
//
// CPython binds `os.path` to `ntpath` when `os.name == 'nt'` (Windows) and to
// `posixpath` otherwise.  pyrust mirrors that: on a Windows host we use the
// `nt_*` helpers (backslash separator, drive-letter / UNC semantics); on every
// other host we keep the existing posix behaviour.  The selector is the same
// `cfg!(windows)` used by `os.name` in `bodies/os.rs`, so the two agree.
//
// The `nt_*` helpers below are pure functions (no platform `cfg`), exercised
// directly by the `#[cfg(test)]` unit tests at the bottom of this file with
// CPython-Windows expected values (verified against `python3.12 -c "import
// ntpath; ..."`, which is importable on Linux).
// ---------------------------------------------------------------------------

fn host_split(path: &str) -> (String, String) {
    if cfg!(windows) {
        nt_split(path)
    } else {
        posix_split(path)
    }
}

fn host_join(parts: &[String]) -> String {
    if cfg!(windows) {
        nt_join(parts)
    } else {
        posix_join(parts)
    }
}

fn host_isabs(path: &str) -> bool {
    if cfg!(windows) {
        nt_isabs(path)
    } else {
        path.starts_with('/')
    }
}

fn host_normpath(path: &str) -> String {
    if cfg!(windows) {
        nt_normpath(path)
    } else {
        posix_normpath(path)
    }
}

fn host_splitdrive(path: &str) -> (String, String) {
    if cfg!(windows) {
        nt_splitdrive(path)
    } else {
        (String::new(), path.to_string())
    }
}

fn host_splitext(path: &str) -> (String, String) {
    // The extension rule is identical on both platforms; only the set of
    // recognised separators differs (ntpath also treats `\` as a separator).
    if cfg!(windows) {
        nt_splitext(path)
    } else {
        splitext_generic(path, '/', None)
    }
}

fn host_abspath(path: &str) -> Result<String> {
    if cfg!(windows) {
        nt_abspath(path)
    } else {
        posix_abspath(path)
    }
}

fn host_relpath(path: &str, start: &str) -> Result<String> {
    if cfg!(windows) {
        nt_relpath(path, start)
    } else {
        posix_relpath(path, start)
    }
}

fn host_expanduser(path: &str) -> String {
    if cfg!(windows) {
        nt_expanduser(path)
    } else {
        posix_expanduser(path)
    }
}

/// POSIX `os.path.join` — any absolute component (one starting with `/`)
/// resets the running result; relative components are appended with a single
/// `/` separator inserted only when the current result does not already end
/// in one.  Mirrors CPython's `posixpath.join`.
fn posix_join(parts: &[String]) -> String {
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
/// separator `altsep`, and the extension separator `.`.  Mirrors CPython's
/// `genericpath._splitext`: the extension starts at the last dot in the final
/// component, ignoring leading dots (`splitext(".bashrc") == (".bashrc", "")`).
fn splitext_generic(path: &str, sep: char, altsep: Option<char>) -> (String, String) {
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

/// ntpath.splitroot → (drive, root, tail).  `drive` is the drive letter
/// (`C:`) or UNC sharepoint (`\\host\share`); `root` is the single separator
/// (`\`) or empty; `tail` is everything after the root.  Separators in the
/// returned strings are normalised to `\`.
fn nt_splitroot(p: &str) -> (String, String, String) {
    let normp: Vec<char> = p.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
    let orig: Vec<char> = p.chars().collect();
    let n = orig.len();
    let take = |a: usize, b: usize| -> String { orig[a..b.min(n)].iter().collect() };

    if normp.first() == Some(&'\\') {
        if normp.get(1) == Some(&'\\') {
            // UNC or device path, e.g. \\server\share or \\?\UNC\server\share
            let upper8: String = normp.iter().take(8).collect::<String>().to_uppercase();
            let start = if upper8 == "\\\\?\\UNC\\" { 8 } else { 2 };
            let index = find_sep(&normp, start);
            let Some(index) = index else {
                return (p.to_string(), String::new(), String::new());
            };
            let index2 = find_sep(&normp, index + 1);
            let Some(index2) = index2 else {
                return (p.to_string(), String::new(), String::new());
            };
            return (take(0, index2), take(index2, index2 + 1), take(index2 + 1, n));
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

fn nt_splitdrive(p: &str) -> (String, String) {
    let (drive, root, tail) = nt_splitroot(p);
    (drive, root + &tail)
}

fn nt_split(p: &str) -> (String, String) {
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

fn nt_isabs(s: &str) -> bool {
    // CPython only inspects the first three chars and treats `/` as `\`.
    let head: String = s.chars().take(3).map(|c| if c == '/' { '\\' } else { c }).collect();
    let hb: Vec<char> = head.chars().collect();
    // UNC/device (`\\…`) or drive-with-root (`C:\…`). Note the legacy bug:
    // `isabs("/x")` is False because a bare leading separator has no drive.
    hb.first() == Some(&'\\') || (hb.get(1) == Some(&':') && hb.get(2) == Some(&'\\'))
}

fn nt_join(parts: &[String]) -> String {
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

fn nt_normpath(path: &str) -> String {
    let path: String = path.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
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

fn nt_abspath(path: &str) -> Result<String> {
    if nt_isabs(path) {
        return Ok(nt_normpath(path));
    }
    let cwd = std::env::current_dir()
        .map_err(|e| PyError::named("OSError", format!("could not resolve cwd: {e}")))?;
    let cwd = cwd.to_string_lossy().into_owned();
    Ok(nt_normpath(&nt_join(&[cwd, path.to_string()])))
}

fn nt_splitext(path: &str) -> (String, String) {
    splitext_generic(path, '\\', Some('/'))
}

fn nt_relpath(path: &str, start: &str) -> Result<String> {
    if path.is_empty() {
        return Err(PyError::named("ValueError", "no path specified".to_string()));
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

fn nt_expanduser(path: &str) -> String {
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

#[cfg(test)]
mod nt_tests {
    //! ntpath-semantics unit tests.  pyrust's CI runs on Linux/WSL where the
    //! `os.path` parity fixture binds the posix arm, so these exercise the
    //! `nt_*` helpers directly.  Expected values are taken verbatim from
    //! CPython 3.12's `ntpath` (importable on Linux:
    //! `python3.12 -c "import ntpath; print(ntpath.split(r'C:\\a\\b'))"`).
    use super::*;

    fn sd(p: &str) -> (String, String) {
        nt_splitdrive(p)
    }
    fn s2(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    #[test]
    fn splitdrive_matches_cpython() {
        assert_eq!(sd("c:/x"), s2("c:", "/x"));
        assert_eq!(sd("//host/share/x"), s2("//host/share", "/x"));
        assert_eq!(sd("a/b"), s2("", "a/b"));
        assert_eq!(sd(r"C:\a"), s2("C:", r"\a"));
        assert_eq!(sd(r"\\?\UNC\srv\shr\x"), s2(r"\\?\UNC\srv\shr", r"\x"));
        assert_eq!(sd("C:"), s2("C:", ""));
        assert_eq!(sd(""), s2("", ""));
    }

    #[test]
    fn split_matches_cpython() {
        assert_eq!(nt_split(r"C:\a\b"), s2(r"C:\a", "b"));
        assert_eq!(nt_split("C:/a/b"), s2("C:/a", "b"));
        assert_eq!(nt_split("a/b"), s2("a", "b"));
        assert_eq!(nt_split(r"C:\"), s2(r"C:\", ""));
        assert_eq!(nt_split(r"\\host\share\x\y"), s2(r"\\host\share\x", "y"));
        assert_eq!(nt_split(r"\\host\share"), s2(r"\\host\share", ""));
        assert_eq!(nt_split(""), s2("", ""));
        assert_eq!(nt_split("/usr/bin"), s2("/usr", "bin"));
        assert_eq!(nt_split(r"a\b\"), s2(r"a\b", ""));
    }

    #[test]
    fn isabs_matches_cpython() {
        assert!(nt_isabs(r"C:\x"));
        assert!(nt_isabs("/x")); // legacy bug: bare separator is "absolute"
        assert!(nt_isabs(r"\\x"));
        assert!(!nt_isabs("C:x"));
        assert!(!nt_isabs("x"));
        assert!(nt_isabs("C:/"));
        assert!(nt_isabs("/"));
        assert!(nt_isabs(r"\\host\share"));
    }

    #[test]
    fn join_matches_cpython() {
        let j = |parts: &[&str]| nt_join(&parts.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(j(&[r"C:\a", "b"]), r"C:\a\b");
        assert_eq!(j(&["a", r"C:\b"]), r"C:\b");
        assert_eq!(j(&["C:", "b"]), "C:b");
        assert_eq!(j(&[r"C:\", "b"]), r"C:\b");
        assert_eq!(j(&["c:/a", "d:/b"]), "d:/b");
        assert_eq!(j(&["/a", "b"]), r"/a\b");
        assert_eq!(j(&["a", "/b"]), "/b");
        assert_eq!(j(&["a", "b", "c"]), r"a\b\c");
        assert_eq!(j(&["//host/share", "a"]), r"//host/share\a");
    }

    #[test]
    fn normpath_matches_cpython() {
        assert_eq!(nt_normpath("a/b"), r"a\b");
        assert_eq!(nt_normpath(r"a\b"), r"a\b");
        assert_eq!(nt_normpath("C:/a/../b"), r"C:\b");
        assert_eq!(nt_normpath("//host/share/a/../b"), r"\\host\share\b");
        assert_eq!(nt_normpath("C:/../x"), r"C:\x");
        assert_eq!(nt_normpath(""), ".");
        assert_eq!(nt_normpath("."), ".");
        assert_eq!(nt_normpath("C://a//b"), r"C:\a\b");
        assert_eq!(nt_normpath(r"C:\..\x"), r"C:\x");
        assert_eq!(nt_normpath("foo/../.."), "..");
        assert_eq!(nt_normpath("/a/b"), r"\a\b");
        assert_eq!(nt_normpath("a/./b/"), r"a\b");
    }

    #[test]
    fn splitext_matches_cpython() {
        assert_eq!(nt_splitext(r"C:\a\b.txt"), s2(r"C:\a\b", ".txt"));
        assert_eq!(nt_splitext("a.b.c"), s2("a.b", ".c"));
        assert_eq!(nt_splitext(".bashrc"), s2(".bashrc", ""));
        assert_eq!(nt_splitext("/x/.y"), s2("/x/.y", ""));
        assert_eq!(nt_splitext("a/b"), s2("a/b", ""));
        assert_eq!(nt_splitext("foo."), s2("foo", "."));
        assert_eq!(nt_splitext(r"C:\a.b\c"), s2(r"C:\a.b\c", ""));
    }

    #[test]
    fn dirname_basename_match_cpython() {
        // dirname == split[0], basename == split[1].
        assert_eq!(nt_split(r"C:\a\b").0, r"C:\a");
        assert_eq!(nt_split(r"C:\a\b").1, "b");
        assert_eq!(nt_split(r"C:\a\").1, "");
        assert_eq!(nt_split(r"C:\a\").0, r"C:\a");
    }

    #[test]
    fn relpath_matches_cpython() {
        // Absolute inputs so the host cwd does not enter the computation.
        assert_eq!(nt_relpath(r"C:\a\b\c", r"C:\a\d").unwrap(), r"..\b\c");
        assert_eq!(nt_relpath(r"C:\a\b", r"C:\a\b").unwrap(), ".");
        assert_eq!(nt_relpath(r"C:\a\b\c", r"C:\a").unwrap(), r"b\c");
        let err = nt_relpath(r"C:\a", r"D:\b").unwrap_err();
        assert!(err.class_name_is("ValueError"));
    }

    #[test]
    fn splitroot_matches_cpython() {
        let r = |p: &str| {
            let (d, r, t) = nt_splitroot(p);
            (d, r, t)
        };
        assert_eq!(r("//server/share/"), ("//server/share".into(), "/".into(), "".into()));
        assert_eq!(r("C:/Users/Barney"), ("C:".into(), "/".into(), "Users/Barney".into()));
        assert_eq!(r("C:///spam///ham"), ("C:".into(), "/".into(), "//spam///ham".into()));
        assert_eq!(r("Windows/notepad"), ("".into(), "".into(), "Windows/notepad".into()));
    }
}
