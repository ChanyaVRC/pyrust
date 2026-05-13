// `os` module — parent package for `os.path` plus the most-used
// filesystem and environment helpers (CPython parity).
//
// The original landing of this file (PR #327) only exposed `os.sep`
// and `os.path`.  Issue #328 expands the surface to the small but
// load-bearing set of working-directory, environment-variable, and
// filesystem mutators that show up in roughly every non-trivial
// Python script: `getcwd`, `chdir`, `getenv`, `environ`, `listdir`,
// `mkdir` / `makedirs`, `remove` / `unlink`, `rmdir`, `rename`, `walk`.
//
// ## `os.environ` design
//
// `os.environ` is a dict-like *instance*, not a class.  CPython
// implements it as a `MutableMapping` view over the process env
// (`os.environ['HOME'] = '/x'` calls `putenv` immediately).  We mirror
// that with a private `_Environ` class declared via top-level
// `pyrust_module!` fns (annotated with `#[py_name = "_Environ.<method>"]`),
// then build a singleton `PyInstance` of that class at module-init
// time.  The methods route every read/write/delete through
// `std::env::var` / `set_var` / `remove_var` — there is no shadow
// HashMap, so changes pyrust makes are visible to subprocesses and
// vice-versa.
//
// `_Environ` is intentionally *not* itself exported on the `os`
// module (the user sees only `os.environ`).  Building it this way
// rather than via `pyrust_module!`'s `class { … }` block sidesteps a
// macro-ordering wrinkle: the `class { … }` form only inserts the
// PyClass into the module's attr map, with no hook for adding an
// instance afterwards.  The manual approach keeps the dispatch fns
// registered through the macro (so they participate in the normal
// builtin-function lookup path) while letting us hand-build the
// class + singleton in one step.
//
// ## Submodule identity (carried over from PR #327)
//
// The `path` constant evaluates `super::os_path::module()`, which
// builds a fresh `os.path` PyModule every `os.module()` call.  That
// would diverge from CPython (`os.path is direct_os_path` should be
// True for any two import forms), so `Interpreter::load_module` in
// `runtime/env.rs` post-processes the result, replacing submodule-
// shaped attrs with their module-cache entries on first import.
//
// ## OSError surfacing
//
// Every filesystem operation that talks to `std::fs` surfaces its
// `io::Error` as a Python-level `OSError` carrying the underlying
// message.  CPython does the same (the message text differs slightly
// per-OS, which the parity tests work around by avoiding error-path
// assertions on platform-specific wording).
//
// Reference: <https://docs.python.org/3/library/os.html>

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::reject_keyword_args_expanded;
use crate::interpreter::{NativeIterFrame, value_type_name_str};
use crate::value::{PyClass, PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "sep" => Value::string(std::path::MAIN_SEPARATOR.to_string()),
        // Submodule binding — exposed so `import os.path; os.path.join(...)`
        // resolves the `path` attribute on the `os` package value.
        "path" => super::os_path::module(),
        // Singleton dict-like view of the process environment.  Built
        // from `ENVIRON_CLASS` (see below) every time `module()` runs;
        // the class is process-global, the instance per-module-build.
        // Each invocation gets a fresh PyInstance Rc so attribute
        // writes (`os.environ.foo = …` if the user reaches for it)
        // don't leak across re-imports.
        "environ" => make_environ_instance(),
    }

    // ── working directory ────────────────────────────────────────────

    /// CPython: os.getcwd() → str — the current working directory.
    /// <https://docs.python.org/3/library/os.html#os.getcwd>
    fn getcwd(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments"),
            ));
        }
        let cwd = std::env::current_dir()
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        Ok(Value::string(cwd.to_string_lossy()))
    }

    /// CPython: os.chdir(path) — change the working directory.
    /// <https://docs.python.org/3/library/os.html#os.chdir>
    fn chdir(args) -> Result<Value> {
        let path = single_path_arg(FN_NAME, args)?;
        std::env::set_current_dir(&path)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        Ok(Value::none())
    }

    // ── environment variables (function form) ────────────────────────

    /// CPython: os.getenv(key, default=None) — `environ.get(key, default)`.
    /// Returns `default` (None unless overridden) when the variable is
    /// unset.
    /// <https://docs.python.org/3/library/os.html#os.getenv>
    fn getenv(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes 1 or 2 arguments ({} given)", args.len()),
            ));
        }
        let key = require_str(FN_NAME, &args[0].value, "key")?;
        let default = args
            .get(1)
            .map(|a| a.value.clone())
            .unwrap_or_else(Value::none);
        match std::env::var(&key) {
            Ok(v) => Ok(Value::string(v)),
            Err(_) => Ok(default),
        }
    }

    // ── directory listing ────────────────────────────────────────────

    /// CPython: os.listdir(path='.') → list[str] — names in `path`,
    /// excluding `.` and `..`.  Order is filesystem-defined; callers
    /// that care about determinism (e.g. parity tests) sort the result
    /// themselves — `listdir` stays a thin wrapper.
    /// <https://docs.python.org/3/library/os.html#os.listdir>
    fn listdir(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 1 argument ({} given)", args.len()),
            ));
        }
        let path = match args.first() {
            Some(a) => require_str(FN_NAME, &a.value, "path")?,
            None => ".".to_string(),
        };
        let entries = std::fs::read_dir(&path)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        let mut names: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| PyError::named("OSError", e.to_string()))?;
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        Ok(Value::list(names.into_iter().map(Value::string).collect()))
    }

    // ── directory creation ───────────────────────────────────────────

    /// CPython: os.mkdir(path, mode=0o777) — create one directory.
    /// `mode` is accepted but ignored on Windows (matches CPython); on
    /// Unix we don't currently apply it either — the umask-respecting
    /// default of `std::fs::create_dir` is close enough for the parity
    /// tests, which don't probe st_mode.
    /// <https://docs.python.org/3/library/os.html#os.mkdir>
    fn mkdir(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes 1 or 2 arguments ({} given)", args.len()),
            ));
        }
        let path = require_str(FN_NAME, &args[0].value, "path")?;
        // `mode` accepted-and-ignored (CPython on Windows does the same).
        // TODO: apply `mode` on Unix via `OpenOptionsExt::mode`; current behaviour falls back to the umask default.
        if let Some(m) = args.get(1) {
            require_int(FN_NAME, &m.value, "mode")?;
        }
        std::fs::create_dir(&path)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        Ok(Value::none())
    }

    /// CPython: os.makedirs(path, mode=0o777, exist_ok=False).  Honour
    /// `exist_ok` — when False, raise OSError on an existing target;
    /// when True, suppress the directory-already-exists case but still
    /// surface other I/O failures.
    /// <https://docs.python.org/3/library/os.html#os.makedirs>
    fn makedirs(args) -> Result<Value> {
        // Keyword-arg shape (`exist_ok=True`) is common — preserve it.
        // The fast positional-only path stays a tight loop, and we
        // pull `exist_ok` out of either position 2 or a kw entry.
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() missing required argument: 'name'"),
            ));
        }
        let path = require_str(FN_NAME, &args[0].value, "path")?;
        let mut exist_ok = false;
        let mut seen_mode = false;
        for (i, a) in args.iter().enumerate().skip(1) {
            match a.name.as_deref() {
                Some("mode") => {
                    require_int(FN_NAME, &a.value, "mode")?;
                    seen_mode = true;
                }
                Some("exist_ok") => {
                    exist_ok = value_is_truthy(&a.value);
                }
                Some(other) => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() got an unexpected keyword argument '{other}'",
                        ),
                    ));
                }
                None => match i {
                    1 if !seen_mode => {
                        require_int(FN_NAME, &a.value, "mode")?;
                        seen_mode = true;
                    }
                    2 => {
                        exist_ok = value_is_truthy(&a.value);
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{FN_NAME}() takes at most 3 arguments"),
                        ));
                    }
                },
            }
        }
        // `exist_ok=True` + path exists is the only case we suppress.
        // `create_dir_all` already returns Ok on pre-existing
        // directories, but CPython raises FileExistsError when
        // `exist_ok=False` and the leaf already exists — recreate that
        // branch by probing the target up front.
        if !exist_ok && std::path::Path::new(&path).exists() {
            return Err(PyError::named(
                "OSError",
                format!("[Errno 17] File exists: '{path}'"),
            ));
        }
        std::fs::create_dir_all(&path)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        Ok(Value::none())
    }

    // ── file/dir removal & rename ────────────────────────────────────

    /// CPython: os.remove(path) — delete a file.
    /// <https://docs.python.org/3/library/os.html#os.remove>
    fn remove(args) -> Result<Value> {
        let path = single_path_arg(FN_NAME, args)?;
        std::fs::remove_file(&path)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        Ok(Value::none())
    }

    /// CPython: os.unlink(path) — alias of `os.remove`.
    /// <https://docs.python.org/3/library/os.html#os.unlink>
    fn unlink(args) -> Result<Value> {
        let path = single_path_arg(FN_NAME, args)?;
        std::fs::remove_file(&path)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        Ok(Value::none())
    }

    /// CPython: os.rmdir(path) — remove an empty directory.
    /// <https://docs.python.org/3/library/os.html#os.rmdir>
    fn rmdir(args) -> Result<Value> {
        let path = single_path_arg(FN_NAME, args)?;
        std::fs::remove_dir(&path)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        Ok(Value::none())
    }

    /// CPython: os.rename(src, dst) — rename / move.
    /// <https://docs.python.org/3/library/os.html#os.rename>
    fn rename(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 2 arguments ({} given)", args.len()),
            ));
        }
        let src = require_str(FN_NAME, &args[0].value, "src")?;
        let dst = require_str(FN_NAME, &args[1].value, "dst")?;
        std::fs::rename(&src, &dst)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        Ok(Value::none())
    }

    /// CPython: os.walk(top) — recursive walk yielding (dirpath,
    /// dirnames, filenames) tuples.  Eager landing per the issue: we
    /// materialise the entire walk up front and return a list, which
    /// is iter-compatible (`for d, ds, fs in os.walk(...)`) without
    /// requiring lazy generator plumbing.
    ///
    /// `top` itself is the first entry.  Subdirectories are descended
    /// in `read_dir` order, which diverges from CPython's
    /// "topdown=True" default in ordering only — the structure of the
    /// yielded tuples is identical, and tests should sort before
    /// comparing.
    /// <https://docs.python.org/3/library/os.html#os.walk>
    fn walk(args) -> Result<Value> {
        let top = single_path_arg(FN_NAME, args)?;
        let mut results: Vec<Value> = Vec::new();
        walk_collect(&top, &mut results)?;
        Ok(Value::list(results))
    }

    // ── _Environ dunders + dict-like API ────────────────────────────
    //
    // These register at "os._Environ.<method>" via `#[py_name]` so the
    // private class isn't visible on the module surface but its
    // builtin-function dispatch routes through the standard
    // `lookup_class_attr` + `invoke_class_method` machinery.  Every
    // method's first arg is `self` (the singleton instance); user args
    // start at `args[1..]`, mirroring the convention `class { … }`
    // blocks use elsewhere.

    #[py_name = "_Environ.__getitem__"]
    fn environ_getitem(args) -> Result<Value> {
        let key = require_key_arg(FN_NAME, args)?;
        match std::env::var(&key) {
            Ok(v) => Ok(Value::string(v)),
            // Missing env var maps to KeyError (CPython parity — `os.environ`
            // is a Mapping, not a defaulting view).
            Err(_) => Err(PyError::named("KeyError", format!("'{key}'"))),
        }
    }

    #[py_name = "_Environ.__setitem__"]
    fn environ_setitem(args) -> Result<Value> {
        require_self(args, FN_NAME)?;
        if args.len() != 3 {
            return Err(PyError::named(
                "TypeError",
                format!("__setitem__ takes 2 arguments ({} given)", args.len() - 1),
            ));
        }
        let key = require_str(FN_NAME, &args[1].value, "key")?;
        let value = require_str(FN_NAME, &args[2].value, "value")?;
        // SAFETY: `std::env::set_var` is marked unsafe in newer Rust
        // editions because the process env isn't thread-safe.  pyrust
        // runs the interpreter single-threaded, so the soundness
        // precondition holds.  Mirror CPython's straight-through write.
        unsafe { std::env::set_var(&key, &value) };
        Ok(Value::none())
    }

    #[py_name = "_Environ.__delitem__"]
    fn environ_delitem(args) -> Result<Value> {
        let key = require_key_arg(FN_NAME, args)?;
        // CPython raises KeyError on delete-of-missing — match that by
        // probing first, since `remove_var` itself is infallible.
        if std::env::var(&key).is_err() {
            return Err(PyError::named("KeyError", format!("'{key}'")));
        }
        unsafe { std::env::remove_var(&key) };
        Ok(Value::none())
    }

    #[py_name = "_Environ.__contains__"]
    fn environ_contains(args) -> Result<Value> {
        let key = require_key_arg(FN_NAME, args)?;
        Ok(Value::bool_(std::env::var(&key).is_ok()))
    }

    #[py_name = "_Environ.__iter__"]
    fn environ_iter(args) -> Result<Value> {
        require_self(args, FN_NAME)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("__iter__ takes no arguments ({} given)", args.len() - 1),
            ));
        }
        // Snapshot keys at call time.  `std::env::vars()` walks a
        // C-side env block; we materialise it once so the iterator
        // value is independent of later `set_var` / `remove_var`
        // calls, matching CPython's view semantics (a dict view
        // iterating during mutation would also see the snapshot, not
        // the mutated set).
        let keys: Vec<Value> = std::env::vars().map(|(k, _)| Value::string(k)).collect();
        Ok(Value::generator(Box::new(NativeIterFrame {
            items: keys,
            pos: 0,
        })))
    }

    #[py_name = "_Environ.__len__"]
    fn environ_len(args) -> Result<Value> {
        require_self(args, FN_NAME)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("__len__ takes no arguments ({} given)", args.len() - 1),
            ));
        }
        Ok(Value::int(std::env::vars().count() as i64))
    }

    #[py_name = "_Environ.__repr__"]
    fn environ_repr(args) -> Result<Value> {
        require_self(args, FN_NAME)?;
        // CPython renders the full env dump as `environ({...})`.  We
        // deliberately don't reproduce the dump because process env
        // contents differ across machines and would never match in
        // parity tests under the harness.  Instead we surface the
        // entry count, which is at least informative in interactive
        // use without breaking parity (the count itself isn't compared
        // by any parity test).
        let n = std::env::vars().count();
        Ok(Value::string(format!("environ({{...{n} entries...}})")))
    }

    #[py_name = "_Environ.get"]
    fn environ_get(args) -> Result<Value> {
        require_self(args, FN_NAME)?;
        if args.len() < 2 || args.len() > 3 {
            return Err(PyError::named(
                "TypeError",
                format!("get() takes 1 or 2 arguments ({} given)", args.len() - 1),
            ));
        }
        let key = require_str(FN_NAME, &args[1].value, "key")?;
        let default = args
            .get(2)
            .map(|a| a.value.clone())
            .unwrap_or_else(Value::none);
        match std::env::var(&key) {
            Ok(v) => Ok(Value::string(v)),
            Err(_) => Ok(default),
        }
    }

    #[py_name = "_Environ.keys"]
    fn environ_keys(args) -> Result<Value> {
        require_no_user_args(args, FN_NAME, "keys")?;
        let keys: Vec<Value> = std::env::vars().map(|(k, _)| Value::string(k)).collect();
        Ok(Value::list(keys))
    }

    #[py_name = "_Environ.values"]
    fn environ_values(args) -> Result<Value> {
        require_no_user_args(args, FN_NAME, "values")?;
        let values: Vec<Value> = std::env::vars().map(|(_, v)| Value::string(v)).collect();
        Ok(Value::list(values))
    }

    #[py_name = "_Environ.items"]
    fn environ_items(args) -> Result<Value> {
        require_no_user_args(args, FN_NAME, "items")?;
        let pairs: Vec<Value> = std::env::vars()
            .map(|(k, v)| Value::tuple(vec![Value::string(k), Value::string(v)]))
            .collect();
        Ok(Value::list(pairs))
    }
}

// ── shared arg helpers ──────────────────────────────────────────────

/// Pull exactly one positional `path` arg, rejecting kwargs and
/// wrong-arity calls — the shape used by `chdir`, `remove`, `rmdir`,
/// `walk`, etc.
fn single_path_arg(fn_name: &str, args: &[ExpandedCallArg]) -> Result<String> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument ({} given)", args.len()),
        ));
    }
    require_str(fn_name, &args[0].value, "path")
}

/// Coerce a Value to a String, raising TypeError if it isn't a str.
fn require_str(fn_name: &str, v: &Value, what: &str) -> Result<String> {
    match v.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {what} must be str, not {}",
                value_type_name_str(v),
            ),
        )),
    }
}

/// Coerce a Value to an i64; only used for accepted-and-ignored mode
/// arguments, so the value itself is dropped on the floor — this is
/// purely a TypeError guard so the caller's API contract matches
/// CPython's "mode must be an int" signature.
fn require_int(fn_name: &str, v: &Value, what: &str) -> Result<i64> {
    match v.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(b as i64),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {what} must be int, not {}",
                value_type_name_str(v),
            ),
        )),
    }
}

/// Internal-bug guard: every `_Environ.*` method has `self` at args[0].
/// Returns an Err if the dispatcher somehow handed us an empty arg
/// list (which would indicate a pyrust bug, not user error).
fn require_self(args: &[ExpandedCallArg], fn_name: &str) -> Result<()> {
    if args.is_empty() {
        Err(PyError::Runtime(format!(
            "internal: {fn_name}() called without self",
        )))
    } else {
        Ok(())
    }
}

/// Common shape for `_Environ` dunders that take exactly one user arg
/// (the key): require `self` at args[0], `key` at args[1], pull the
/// key as a String.
fn require_key_arg(fn_name: &str, args: &[ExpandedCallArg]) -> Result<String> {
    require_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name} takes exactly 1 argument ({} given)",
                args.len() - 1
            ),
        ));
    }
    require_str(fn_name, &args[1].value, "key")
}

/// `keys()` / `values()` / `items()` take only `self`.  Centralised
/// arity check so all three share one diagnostic.
fn require_no_user_args(args: &[ExpandedCallArg], fn_name: &str, method: &str) -> Result<()> {
    require_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{method}() takes no arguments ({} given)",
                args.len() - 1
            ),
        ));
    }
    Ok(())
}

/// Treat any non-zero / non-empty value as truthy for the
/// `exist_ok=…` flag.  We accept the common shapes (bool, int) plus
/// fall back to non-None for everything else — matches CPython's
/// `bool(exist_ok)` coercion in `makedirs`.
fn value_is_truthy(v: &Value) -> bool {
    match v.kind() {
        ValueKind::Bool(b) => b,
        ValueKind::Int(n) => n != 0,
        ValueKind::None => false,
        _ => true,
    }
}

// ── walk() implementation ───────────────────────────────────────────

/// Recursive eager walk.  For each visited directory we push a single
/// `(dirpath, [subdirs], [files])` tuple to `out` and then descend
/// into each subdir.  Errors from `read_dir` surface as `OSError`;
/// CPython's `walk` actually swallows `read_dir` errors and continues
/// silently (you have to opt in via the `onerror` arg), but the parity
/// tests don't exercise that path so the simpler propagation is fine
/// for the initial landing.
fn walk_collect(dir: &str, out: &mut Vec<Value>) -> Result<()> {
    let mut subdirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let entries =
        std::fs::read_dir(dir).map_err(|e| PyError::named("OSError", e.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|e| PyError::named("OSError", e.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let ft = entry
            .file_type()
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        if ft.is_dir() {
            subdirs.push(name);
        } else {
            files.push(name);
        }
    }
    out.push(Value::tuple(vec![
        Value::string(dir),
        Value::list(subdirs.iter().cloned().map(Value::string).collect()),
        Value::list(files.into_iter().map(Value::string).collect()),
    ]));
    for sub in subdirs {
        // Build child path with the platform separator so the yielded
        // dirpath looks like CPython's (`top/sub` on POSIX, `top\sub`
        // on Windows).  `Path::join` handles trailing-separator edge
        // cases (and the case where `dir` was passed in with the
        // non-native separator) without splicing `MAIN_SEPARATOR`
        // ourselves and risking a doubled separator.
        let sub_path = std::path::Path::new(dir)
            .join(&sub)
            .to_string_lossy()
            .into_owned();
        walk_collect(&sub_path, out)?;
    }
    Ok(())
}

// ── `_Environ` class + singleton instance ───────────────────────────

// The `_Environ` `PyClass` is built once per interpreter thread and
// reused for every `os.environ` instance on that thread.  Each method
// attr is a `Value::builtin_function(...)` carrying the registry name
// we registered above via `#[py_name = "_Environ.<m>"]` — the
// interpreter's `lookup_class_attr` + `invoke_class_method` path
// picks them up exactly like a `class { … }` block would.
//
// `Value` is `!Send` (it transitively holds `Rc`s), so we cache the
// class in a `thread_local!` rather than a `static LazyLock`.  pyrust
// is single-threaded for now, but the same shape works when/if a
// per-thread interpreter lands.
//
// `FN_PREFIX` resolves to `"os."` here (injected by
// `pyrust_builtin_modules!`), so the registered names are
// `"os._Environ.__getitem__"` etc.
thread_local! {
    static ENVIRON_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: HashMap<String, Value> = HashMap::new();
        for (short, py_full) in environ_method_table() {
            attrs.insert(short.to_string(), Value::builtin_function(py_full));
        }
        Rc::new(RefCell::new(PyClass {
            name: "_Environ".to_string(),
            base: None,
            attrs,
        }))
    };
}

/// (method-short, registry-name) pairs for every `_Environ` method.
///
/// The full names are leaked once via `Box::leak` so we can hand them
/// to `Value::builtin_function`, which wants `&'static str`.  Cost is
/// one tiny per-method allocation across the whole process lifetime;
/// the alternative (string-literal `&'static`s baked in here) would
/// double the source of truth — once in the `#[py_name]` annotations
/// above, once here.
fn environ_method_table() -> Vec<(&'static str, &'static str)> {
    let shorts = [
        "__getitem__",
        "__setitem__",
        "__delitem__",
        "__contains__",
        "__iter__",
        "__len__",
        "__repr__",
        "get",
        "keys",
        "values",
        "items",
    ];
    shorts
        .iter()
        .map(|s| {
            let full: &'static str = Box::leak(format!("os._Environ.{s}").into_boxed_str());
            (*s, full)
        })
        .collect()
}

/// Construct the `os.environ` singleton.  Called from the constants
/// block on every `module()` invocation, which the interpreter calls
/// at most once per process (the module-cache memoises the result).
/// The PyClass underneath is global; only the per-module PyInstance
/// is reallocated, which is cheap.
fn make_environ_instance() -> Value {
    ENVIRON_CLASS.with(|class| {
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs: HashMap::new(),
        })))
    })
}
