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

use std::path::{Path, PathBuf};

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
        // CPython quirk: any absolute component resets the running path
        // to that component and drops everything that came before.
        let mut out = PathBuf::new();
        for (i, a) in args.iter().enumerate() {
            let s = match a.value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument {} must be str", i + 1),
                )),
            };
            let p = Path::new(&s);
            if p.is_absolute() {
                out = p.to_path_buf();
            } else {
                out.push(p);
            }
        }
        Ok(Value::string(out.to_string_lossy().into_owned()))
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
        // CPython: `dirname(p) == split(p)[0]`.  Rust's `Path::parent`
        // diverges on trailing/repeated slashes (`dirname('/')` should be
        // `'/'`, `dirname('a/')` should be `'a'`), so reuse `posix_split`.
        let (head, _) = posix_split(&path);
        Ok(Value::string(head))
    }

    /// CPython: os.path.basename(path) — final path component, or `""`
    /// if `path` ends with a separator.
    /// <https://docs.python.org/3/library/os.path.html#os.path.basename>
    fn basename(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        // Trailing-separator behaviour: CPython returns `""`, but
        // `Path::file_name()` would return the last real component.  Emulate.
        if path.ends_with('/') || path.ends_with(std::path::MAIN_SEPARATOR) {
            return Ok(Value::string(String::new()));
        }
        let base = Path::new(&path)
            .file_name()
            .map(|os| os.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Value::string(base))
    }

    /// CPython: os.path.abspath(path) — absolute normalised form of `path`.
    /// Uses the process's current working directory; raises on cwd
    /// resolution failure (matches CPython behaviour).
    /// <https://docs.python.org/3/library/os.path.html#os.path.abspath>
    fn abspath(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let p = Path::new(&path);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            let cwd = std::env::current_dir().map_err(|e| {
                PyError::named(
                    "OSError",
                    format!("{FN_NAME}() could not resolve cwd: {e}"),
                )
            })?;
            cwd.join(p)
        };
        Ok(Value::string(abs.to_string_lossy().into_owned()))
    }

    /// CPython: os.path.splitext(path) → (root, ext) — split a path into
    /// `(everything-up-to-the-last-dot, ".ext")`, with no leading dot
    /// counted (so `splitext(".bashrc")` → `(".bashrc", "")`).
    /// <https://docs.python.org/3/library/os.path.html#os.path.splitext>
    fn splitext(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let bytes = path.as_bytes();
        // Walk from the end until we find the directory separator or a
        // dot.  CPython's rule: only the last dot in the basename
        // counts, and a leading-dot basename (`.bashrc`, `..`) is *not*
        // treated as an extension.
        let mut last_sep = None;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'/' || b == std::path::MAIN_SEPARATOR as u8 {
                last_sep = Some(i);
            }
        }
        let basename_start = last_sep.map_or(0, |i| i + 1);
        let basename = &path[basename_start..];
        // Skip leading dots — they don't count as extension separators.
        let leading_dot_end = basename.bytes().take_while(|&b| b == b'.').count();
        let after_dots = &basename[leading_dot_end..];
        match after_dots.rfind('.') {
            Some(rel_dot) => {
                let abs_dot = basename_start + leading_dot_end + rel_dot;
                Ok(Value::tuple(vec![
                    Value::string(path[..abs_dot].to_string()),
                    Value::string(path[abs_dot..].to_string()),
                ]))
            }
            None => Ok(Value::tuple(vec![
                Value::string(path.clone()),
                Value::string(String::new()),
            ])),
        }
    }

    /// CPython: os.path.split(path) → (head, tail) — split into the
    /// directory part and the final component.  `tail` never contains a
    /// slash; if `path` ends with a slash `tail` is empty.  Trailing
    /// slashes are stripped from `head` unless `head` is all slashes
    /// (the root).  POSIX semantics.
    /// <https://docs.python.org/3/library/os.path.html#os.path.split>
    fn split(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let (head, tail) = posix_split(&path);
        Ok(Value::tuple(vec![Value::string(head), Value::string(tail)]))
    }

    /// CPython: os.path.isabs(path) — true if `path` begins with a slash.
    /// <https://docs.python.org/3/library/os.path.html#os.path.isabs>
    fn isabs(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::bool_(path.starts_with('/')))
    }

    /// CPython: os.path.normpath(path) — collapse redundant separators
    /// and up-level references (`.` / `..`).  POSIX semantics, including
    /// the special case where exactly two leading slashes are preserved.
    /// <https://docs.python.org/3/library/os.path.html#os.path.normpath>
    fn normpath(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::string(posix_normpath(&path)))
    }

    /// CPython: os.path.splitdrive(path) → (drive, tail).  On POSIX the
    /// drive is always empty, so this returns `("", path)`.
    /// <https://docs.python.org/3/library/os.path.html#os.path.splitdrive>
    fn splitdrive(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::tuple(vec![Value::string(String::new()), Value::string(path)]))
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
        Ok(Value::string(min[..prefix_len].to_string()))
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
        Ok(Value::string(posix_relpath(&path, &start)?))
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
        Ok(Value::string(posix_expanduser(&path)))
    }

    /// CPython: os.path.realpath(path) — canonical path, resolving
    /// symlinks where possible.  pyrust resolves via the filesystem when
    /// the path exists, otherwise falls back to `abspath` + `normpath`
    /// (which is what posixpath does for non-existent components).
    /// <https://docs.python.org/3/library/os.path.html#os.path.realpath>
    fn realpath(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let abs = posix_abspath(&path)?;
        match std::fs::canonicalize(&abs) {
            Ok(p) => Ok(Value::string(p.to_string_lossy().into_owned())),
            Err(_) => Ok(Value::string(posix_normpath(&abs))),
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
    let mut rel: Vec<&str> = Vec::new();
    for _ in common..start_parts.len() {
        rel.push("..");
    }
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
