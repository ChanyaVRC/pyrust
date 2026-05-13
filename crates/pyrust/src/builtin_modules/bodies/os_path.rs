// `os.path` module — included into `pub mod os_path { … }` declared by
// the `"os.path" as os_path` entry in `pyrust_builtin_modules!` in
// `builtin_modules/mod.rs`.  This is the first module that exercises
// the dotted-Python-name form of `pyrust_builtin_modules!`: the macro
// injects `MODULE_NAME = "os.path"` and `FN_PREFIX = "os.path."`, so
// the registered names look like `"os.path.join"`.
//
// ## Import quirk
//
// pyrust's compiler binds `import os.path` to the *first* component of
// the dotted name — i.e. the name `os` — to match CPython's package
// semantics.  But pyrust does not yet ship an `os` parent package, so
// `import os.path` followed by `os.path.join(...)` fails with
// `AttributeError: module 'os.path' has no attribute 'path'`.  Use one
// of the working forms instead:
//
//     import os.path as op
//     op.join('a', 'b')
//
//     from os.path import join
//     join('a', 'b')
//
// Proper `os`-as-package support is a separate concern (it requires the
// import path to build a stub parent module on first dotted-builtin
// load); tracked outside this PR.
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
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes at least one argument",
            )));
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
        let p = Path::new(&path);
        let dir = match p.parent() {
            Some(parent) => parent.to_string_lossy().into_owned(),
            None => String::new(),
        };
        Ok(Value::string(dir))
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
}

/// Common preamble for the single-`str`-arg variants.  Rejects kwargs,
/// requires exactly one positional, and pulls out the string contents
/// (or a TypeError if the caller passed something else).
fn single_path(fn_name: &str, args: &[ExpandedCallArg]) -> Result<String> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::Runtime(format!(
            "{fn_name}() takes exactly one argument",
        )));
    }
    match args[0].value.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() argument must be str"),
        )),
    }
}
