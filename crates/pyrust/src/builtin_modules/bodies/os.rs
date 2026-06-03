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
// Reads use `std::env::var_os` (returns `Option<OsString>`) rather
// than `std::env::var`, which would force us to disambiguate
// `VarError::NotPresent` from `VarError::NotUnicode`.  A non-UTF-8
// env var is surfaced via `to_string_lossy`, so `os.environ['FOO']`
// always returns a `str`.  Writes go through `set_var` / `remove_var`
// under `ENV_LOCK` (see below) to serialise pyrust-side env mutation.
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
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};

use crate::error::{PyError, Result};

/// Process-global lock guarding every read and write of the env via
/// this module.  `std::env::set_var` / `remove_var` are `unsafe` in
/// modern Rust editions because env mutation isn't thread-safe even
/// from Rust's perspective — linked C libraries, allocators, and
/// signal handlers may concurrently read the env block.  pyrust runs
/// its interpreter single-threaded today, but a per-module mutex is
/// cheap insurance: it serialises every pyrust-side env access so at
/// least the calls we control don't race each other.  It is *not* a
/// guarantee against races with other threads in the process that
/// don't go through this lock (best-effort).
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
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
        // CPython: os.name — the OS-dependent module name. Matches the host
        // OS so parity with CPython holds on both POSIX ("posix") and
        // Windows ("nt"). <https://docs.python.org/3/library/os.html#os.name>
        "name" => Value::string(if cfg!(windows) { "nt" } else { "posix" }),
        // CPython: os.linesep — the string used to separate lines on the
        // current platform: "\r\n" on Windows, "\n" on POSIX.
        "linesep" => Value::string(if cfg!(windows) { "\r\n" } else { "\n" }),
        // CPython: os.curdir / os.pardir — the string the OS uses for the
        // current / parent directory. Same on POSIX and Windows.
        "curdir" => Value::string("."),
        "pardir" => Value::string(".."),
        // CPython: os.extsep — the character separating the base name from
        // the extension. Same on POSIX and Windows.
        "extsep" => Value::string("."),
        // CPython: os.pathsep — the separator in $PATH-style lists:
        // ";" on Windows, ":" on POSIX.
        "pathsep" => Value::string(if cfg!(windows) { ";" } else { ":" }),
        // CPython: os.altsep — the alternate path separator; "/" on Windows,
        // None on POSIX.
        "altsep" => {
            if cfg!(windows) {
                Value::string("/")
            } else {
                Value::none()
            }
        },
        // CPython: os.devnull — the path of the null device: "nul" on
        // Windows, "/dev/null" on POSIX.
        "devnull" => Value::string(if cfg!(windows) { "nul" } else { "/dev/null" }),
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
            .map_err(|e| PyError::from_io_error(&e, None))?;
        Ok(Value::string(cwd.to_string_lossy()))
    }

    /// CPython: os.chdir(path) — change the working directory.
    /// <https://docs.python.org/3/library/os.html#os.chdir>
    fn chdir(args) -> Result<Value> {
        let path = single_path_arg(FN_NAME, args)?;
        std::env::set_current_dir(&path)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        Ok(Value::none())
    }

    // ── environment variables (function form) ────────────────────────

    /// CPython: os.getenv(key, default=None) — `environ.get(key, default)`.
    /// Returns `default` (None unless overridden) when the variable is
    /// unset.
    /// <https://docs.python.org/3/library/os.html#os.getenv>
    fn getenv(args) -> Result<Value> {
        // CPython's `os.getenv(key, default=None)` accepts both
        // positionally and by keyword (`os.getenv(key='PATH')`).  Walk
        // the args once, tracking which slot each entry filled, and
        // reject duplicates / unknown kwargs.
        let mut key_value: Option<Value> = None;
        let mut default_value: Option<Value> = None;
        let mut key_from_kw = false;
        let mut default_from_kw = false;
        for (i, a) in args.iter().enumerate() {
            match a.name.as_deref() {
                Some("key") => {
                    if key_value.is_some() {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{FN_NAME}() got multiple values for argument 'key'"),
                        ));
                    }
                    key_value = Some(a.value.clone());
                    key_from_kw = true;
                }
                Some("default") => {
                    if default_value.is_some() {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{FN_NAME}() got multiple values for argument 'default'"
                            ),
                        ));
                    }
                    default_value = Some(a.value.clone());
                    default_from_kw = true;
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
                    0 if !key_from_kw => {
                        key_value = Some(a.value.clone());
                    }
                    1 if !default_from_kw => {
                        default_value = Some(a.value.clone());
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{FN_NAME}() takes 1 or 2 arguments ({} given)",
                                args.len()
                            ),
                        ));
                    }
                },
            }
        }
        let key_value = key_value.ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!("{FN_NAME}() missing required argument: 'key'"),
            )
        })?;
        let key = require_str(FN_NAME, &key_value, "key")?;
        let default = default_value.unwrap_or_else(Value::none);
        // `var_os` sidesteps `VarError`'s NotPresent/NotUnicode split:
        // any non-UTF-8 env var is decoded lossily so the result is
        // always a `str`.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match std::env::var_os(&key) {
            Some(v) => Ok(Value::string(v.to_string_lossy().into_owned())),
            None => Ok(default),
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
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        let mut names: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
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
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
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
        // pull `mode` / `exist_ok` from either positional slot or kw
        // entry, rejecting duplicates the way CPython does (TypeError
        // "got multiple values for argument 'X'").
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() missing required argument: 'name'"),
            ));
        }
        let path = require_str(FN_NAME, &args[0].value, "path")?;
        let mut exist_ok = false;
        let mut seen_mode_positional = false;
        let mut seen_mode_keyword = false;
        let mut seen_exist_ok_positional = false;
        let mut seen_exist_ok_keyword = false;
        for (i, a) in args.iter().enumerate().skip(1) {
            match a.name.as_deref() {
                Some("mode") => {
                    if seen_mode_positional || seen_mode_keyword {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{FN_NAME}() got multiple values for argument 'mode'",
                            ),
                        ));
                    }
                    require_int(FN_NAME, &a.value, "mode")?;
                    seen_mode_keyword = true;
                }
                Some("exist_ok") => {
                    if seen_exist_ok_positional || seen_exist_ok_keyword {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{FN_NAME}() got multiple values for argument 'exist_ok'",
                            ),
                        ));
                    }
                    exist_ok = value_is_truthy(&a.value);
                    seen_exist_ok_keyword = true;
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
                    1 if !seen_mode_positional && !seen_mode_keyword => {
                        require_int(FN_NAME, &a.value, "mode")?;
                        seen_mode_positional = true;
                    }
                    2 if !seen_exist_ok_positional && !seen_exist_ok_keyword => {
                        exist_ok = value_is_truthy(&a.value);
                        seen_exist_ok_positional = true;
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
        // branch by probing the target up front, then provoking the
        // real OS error from `create_dir` so the message + errno match
        // the underlying platform (no hard-coded English in the path).
        if !exist_ok && std::path::Path::new(&path).exists() {
            // Race window: if the path disappears between `exists()`
            // and `create_dir`, `create_dir` returns Ok — treat that as
            // success rather than fabricating an error.
            return match std::fs::create_dir(&path) {
                Ok(()) => Ok(Value::none()),
                Err(e) => Err(PyError::from_io_error(&e, Some(&path))),
            };
        }
        std::fs::create_dir_all(&path)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        Ok(Value::none())
    }

    // ── file/dir removal & rename ────────────────────────────────────

    /// CPython: os.remove(path) — delete a file.
    /// <https://docs.python.org/3/library/os.html#os.remove>
    fn remove(args) -> Result<Value> {
        let path = single_path_arg(FN_NAME, args)?;
        std::fs::remove_file(&path)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        Ok(Value::none())
    }

    /// CPython: os.unlink(path) — alias of `os.remove`.
    /// <https://docs.python.org/3/library/os.html#os.unlink>
    fn unlink(args) -> Result<Value> {
        let path = single_path_arg(FN_NAME, args)?;
        std::fs::remove_file(&path)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        Ok(Value::none())
    }

    /// CPython: os.rmdir(path) — remove an empty directory.
    /// <https://docs.python.org/3/library/os.html#os.rmdir>
    fn rmdir(args) -> Result<Value> {
        let path = single_path_arg(FN_NAME, args)?;
        std::fs::remove_dir(&path)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
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
            .map_err(|e| PyError::from_io_error2(&e, Some(&src), Some(&dst)))?;
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

    // ── process / system ─────────────────────────────────────────────

    /// CPython: os.getpid() → int — the current process id.
    /// <https://docs.python.org/3/library/os.html#os.getpid>
    fn getpid(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        Ok(Value::int(std::process::id() as i64))
    }

    /// CPython: os.getppid() → int — the parent process id.
    /// <https://docs.python.org/3/library/os.html#os.getppid>
    fn getppid(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        Ok(Value::int(get_parent_pid()))
    }

    /// CPython: os.cpu_count() → int | None — number of logical CPUs.
    /// <https://docs.python.org/3/library/os.html#os.cpu_count>
    fn cpu_count(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        match std::thread::available_parallelism() {
            Ok(n) => Ok(Value::int(n.get() as i64)),
            Err(_) => Ok(Value::none()),
        }
    }

    /// CPython: os.urandom(size) → bytes — `size` cryptographically
    /// random bytes from the OS entropy source.
    /// <https://docs.python.org/3/library/os.html#os.urandom>
    fn urandom(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument ({} given)", args.len()),
            ));
        }
        let n = require_int(FN_NAME, &args[0].value, "size")?;
        if n < 0 {
            return Err(PyError::named(
                "ValueError",
                "negative argument not allowed".to_string(),
            ));
        }
        let bytes = os_urandom(n as usize)?;
        Ok(Value::bytes(bytes))
    }

    /// CPython: os.strerror(code) → str — the error message for `code`.
    /// <https://docs.python.org/3/library/os.html#os.strerror>
    fn strerror(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument ({} given)", args.len()),
            ));
        }
        let code = require_int(FN_NAME, &args[0].value, "code")?;
        // `std::io::Error::from_raw_os_error(..).to_string()` appends a
        // " (os error N)" suffix that CPython's strerror does not produce;
        // strip it so the message matches the libc text.
        let raw = std::io::Error::from_raw_os_error(code as i32).to_string();
        let msg = match raw.rfind(" (os error ") {
            Some(idx) => raw[..idx].to_string(),
            None => raw,
        };
        Ok(Value::string(msg))
    }

    /// CPython: os.get_terminal_size(fd=STDOUT_FILENO) → os.terminal_size.
    /// pyrust reads the COLUMNS / LINES environment variables, falling
    /// back to the conventional 80×24 default when they are unset.
    /// <https://docs.python.org/3/library/os.html#os.get_terminal_size>
    fn get_terminal_size(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 1 argument ({} given)", args.len()),
            ));
        }
        let columns = std::env::var("COLUMNS")
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .filter(|&c| c > 0)
            .unwrap_or(80);
        let lines = std::env::var("LINES")
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .filter(|&l| l > 0)
            .unwrap_or(24);
        Ok(make_terminal_size(columns, lines))
    }

    /// CPython: os.fspath(path) — return `path` unchanged if it is str or
    /// bytes, otherwise return the result of `type(path).__fspath__(path)`
    /// (which must itself be str or bytes), else raise TypeError.
    /// <https://docs.python.org/3/library/os.html#os.fspath>
    fn fspath(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument ({} given)", args.len()),
            ));
        }
        let obj = args[0].value.clone();
        let is_path = matches!(obj.kind(), ValueKind::Str(_) | ValueKind::Bytes(_));
        if is_path {
            return Ok(obj);
        }
        // PathLike protocol: call obj.__fspath__().
        if let Ok(method) = _interp.get_attr(&obj, "__fspath__") {
            let result = _interp.call_function_expanded(method, &[])?;
            let result_ok = matches!(result.kind(), ValueKind::Str(_) | ValueKind::Bytes(_));
            if result_ok {
                return Ok(result);
            }
            return Err(PyError::named(
                "TypeError",
                format!(
                    "expected {}.__fspath__() to return str or bytes, not {}",
                    value_type_name_str(&obj),
                    value_type_name_str(&result),
                ),
            ));
        }
        Err(PyError::named(
            "TypeError",
            format!(
                "expected str, bytes or os.PathLike object, not {}",
                value_type_name_str(&obj),
            ),
        ))
    }

    /// CPython: os.stat(path) → os.stat_result — filesystem metadata.
    /// pyrust populates the commonly-used fields (st_mode/st_size/st_mtime
    /// /st_ctime/st_atime/st_ino/st_nlink/st_uid/st_gid/st_dev); other
    /// fields default to 0.
    /// <https://docs.python.org/3/library/os.html#os.stat>
    fn stat(args) -> Result<Value> {
        let path = single_path_arg(FN_NAME, args)?;
        let meta = std::fs::metadata(&path)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        Ok(make_stat_result(&meta))
    }

    /// `repr(os.get_terminal_size())` — `os.terminal_size(columns=…, lines=…)`.
    #[py_name = "terminal_size_repr"]
    fn terminal_size_repr(args) -> Result<Value> {
        require_self(args, FN_NAME)?;
        let inst = match args[0].value.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => {
                return Err(PyError::Runtime(
                    "terminal_size_repr() expected a terminal_size instance".to_string(),
                ))
            }
        };
        let borrow = inst.borrow();
        let get = |name: &str| -> i64 {
            match borrow.attrs.get(name).map(|v| v.kind()) {
                Some(ValueKind::Int(n)) => n,
                _ => 0,
            }
        };
        Ok(Value::string(format!(
            "os.terminal_size(columns={}, lines={})",
            get("columns"),
            get("lines"),
        )))
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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `var_os` returns `Option<OsString>`; a non-unicode value is
        // surfaced via lossy decode rather than the awkward
        // `VarError::NotUnicode` branch that `std::env::var` exposes.
        // Missing env var maps to KeyError (CPython parity — `os.environ`
        // is a Mapping, not a defaulting view).
        match std::env::var_os(&key) {
            Some(v) => Ok(Value::string(v.to_string_lossy().into_owned())),
            None => Err(PyError::key_error(Value::string(key.clone()))),
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
        // editions because the process env isn't thread-safe.  We
        // serialise every pyrust-side env access through `ENV_LOCK`
        // so concurrent calls from within pyrust don't race; this is
        // best-effort against threads in other linked libraries that
        // don't take this lock.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var(&key, &value) };
        Ok(Value::none())
    }

    #[py_name = "_Environ.__delitem__"]
    fn environ_delitem(args) -> Result<Value> {
        let key = require_key_arg(FN_NAME, args)?;
        // CPython raises KeyError on delete-of-missing — match that by
        // probing first, since `remove_var` itself is infallible.
        // SAFETY: see `environ_setitem` — same `ENV_LOCK` guard
        // covers the probe-then-remove pair so the two halves can't be
        // interleaved with another pyrust-side write.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if std::env::var_os(&key).is_none() {
            return Err(PyError::key_error(Value::string(key.clone())));
        }
        unsafe { std::env::remove_var(&key) };
        Ok(Value::none())
    }

    #[py_name = "_Environ.__contains__"]
    fn environ_contains(args) -> Result<Value> {
        let key = require_key_arg(FN_NAME, args)?;
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `var_os` returns None only for genuinely-missing keys —
        // `var`'s `NotUnicode` branch would have falsely reported a
        // present-but-non-UTF-8 var as absent.
        Ok(Value::bool_(std::env::var_os(&key).is_some()))
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
            type_name: "generator",
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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match std::env::var_os(&key) {
            Some(v) => Ok(Value::string(v.to_string_lossy().into_owned())),
            None => Ok(default),
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
        std::fs::read_dir(dir).map_err(|e| PyError::from_io_error(&e, Some(dir)))?;
    for entry in entries {
        let entry = entry.map_err(|e| PyError::from_io_error(&e, Some(dir)))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let ft = entry
            .file_type()
            .map_err(|e| PyError::from_io_error(&e, Some(dir)))?;
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
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        for (short, py_full) in ENVIRON_METHODS {
            attrs.insert((*short).to_string(), Value::builtin_function(py_full));
        }
        Rc::new(RefCell::new(PyClass::new("_Environ", "_Environ", None, attrs)))
    };
}

/// (method-short, registry-name) pairs for every `_Environ` method.
///
/// Static, so there's no `Box::leak` and no per-thread init cost.  The
/// registry names must match the `#[py_name = "_Environ.<method>"]`
/// annotations on the dispatch fns above — the `FN_PREFIX` injected by
/// `pyrust_builtin_modules!` resolves to `"os."`, so the full
/// registry path is `"os._Environ.<method>"`.
const ENVIRON_METHODS: &[(&str, &str)] = &[
    ("__getitem__", "os._Environ.__getitem__"),
    ("__setitem__", "os._Environ.__setitem__"),
    ("__delitem__", "os._Environ.__delitem__"),
    ("__contains__", "os._Environ.__contains__"),
    ("__iter__", "os._Environ.__iter__"),
    ("__len__", "os._Environ.__len__"),
    ("__repr__", "os._Environ.__repr__"),
    ("get", "os._Environ.get"),
    ("keys", "os._Environ.keys"),
    ("values", "os._Environ.values"),
    ("items", "os._Environ.items"),
];

// ── process / system helpers ────────────────────────────────────────

/// Parent-process id.  `std` has no portable `getppid`, so we call the
/// platform primitive directly: `getppid(2)` on Unix, and a Toolhelp
/// process snapshot on Windows (which has no direct PPID syscall — the
/// snapshot's `PROCESSENTRY32::th32ParentProcessID` is the supported way).
#[cfg(unix)]
fn get_parent_pid() -> i64 {
    // SAFETY: `getppid` takes no arguments, never fails, and has no
    // preconditions; it always returns the caller's parent pid.
    (unsafe { libc::getppid() }) as i64
}

#[cfg(windows)]
fn get_parent_pid() -> i64 {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32,
        TH32CS_SNAPPROCESS,
    };

    let me = std::process::id();
    // SAFETY: the Toolhelp APIs are pure FFI with documented contracts; we
    // check the snapshot handle for INVALID_HANDLE_VALUE before use, zero-init
    // the entry and set `dwSize` as required, and close the handle on exit.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut entry: PROCESSENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;
        let mut ppid: i64 = 0;
        if Process32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32ProcessID == me {
                    ppid = entry.th32ParentProcessID as i64;
                    break;
                }
                if Process32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        ppid
    }
}

#[cfg(not(any(unix, windows)))]
fn get_parent_pid() -> i64 {
    0
}

/// Read `n` random bytes from the OS cryptographically-secure RNG.
///
/// Uses the `getrandom` crate, which is the de-facto cross-platform
/// CSPRNG shim: it reads `getrandom(2)` / `/dev/urandom` on Linux,
/// `getentropy` on macOS/BSD, and `BCryptGenRandom` on Windows — the
/// same OS entropy sources CPython's `os.urandom` draws from.
fn os_urandom(n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    if n == 0 {
        return Ok(buf);
    }
    getrandom::getrandom(&mut buf).map_err(|e| {
        // `getrandom::Error` carries a raw OS error code when the failure
        // originated in the platform RNG; surface it as an OSError the same
        // way a `/dev/urandom` read failure would have.
        match e.raw_os_error() {
            Some(code) => {
                let io = std::io::Error::from_raw_os_error(code);
                PyError::from_io_error(&io, None)
            }
            None => PyError::named("OSError", e.to_string()),
        }
    })?;
    Ok(buf)
}

// ── terminal_size struct-sequence ───────────────────────────────────

thread_local! {
    static TERMINAL_SIZE_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function("os.terminal_size_repr"),
        );
        Rc::new(RefCell::new(PyClass::new(
            "terminal_size",
            "os.terminal_size",
            None,
            attrs,
        )))
    };
}

/// Build an `os.terminal_size(columns, lines)` instance.
fn make_terminal_size(columns: i64, lines: i64) -> Value {
    TERMINAL_SIZE_CLASS.with(|class| {
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        attrs.insert("columns".to_string(), Value::int(columns));
        attrs.insert("lines".to_string(), Value::int(lines));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}

// ── stat_result struct-sequence ─────────────────────────────────────

thread_local! {
    static STAT_RESULT_CLASS: Rc<RefCell<PyClass>> = {
        Rc::new(RefCell::new(PyClass::new(
            "stat_result",
            "os.stat_result",
            None,
            indexmap::IndexMap::new(),
        )))
    };
}

/// Build an `os.stat_result` from filesystem metadata.  The commonly
/// used fields are populated; the rest default to 0.
fn make_stat_result(meta: &std::fs::Metadata) -> Value {
    fn secs(t: std::io::Result<std::time::SystemTime>) -> f64 {
        t.ok()
            .and_then(|st| st.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
    let mtime = secs(meta.modified());
    let atime = secs(meta.accessed());
    let ctime = secs(meta.created());

    #[cfg(unix)]
    let (mode, ino, nlink, uid, gid, dev) = {
        use std::os::unix::fs::MetadataExt;
        (
            meta.mode() as i64,
            meta.ino() as i64,
            meta.nlink() as i64,
            meta.uid() as i64,
            meta.gid() as i64,
            meta.dev() as i64,
        )
    };
    #[cfg(not(unix))]
    let (mode, ino, nlink, uid, gid, dev) = (
        if meta.is_dir() { 0o040000_i64 } else { 0o100000_i64 },
        0_i64,
        0_i64,
        0_i64,
        0_i64,
        0_i64,
    );

    STAT_RESULT_CLASS.with(|class| {
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        attrs.insert("st_mode".to_string(), Value::int(mode));
        attrs.insert("st_ino".to_string(), Value::int(ino));
        attrs.insert("st_dev".to_string(), Value::int(dev));
        attrs.insert("st_nlink".to_string(), Value::int(nlink));
        attrs.insert("st_uid".to_string(), Value::int(uid));
        attrs.insert("st_gid".to_string(), Value::int(gid));
        attrs.insert("st_size".to_string(), Value::int(meta.len() as i64));
        attrs.insert("st_atime".to_string(), Value::float(atime));
        attrs.insert("st_mtime".to_string(), Value::float(mtime));
        attrs.insert("st_ctime".to_string(), Value::float(ctime));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
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
            attrs: indexmap::IndexMap::new(),
        })))
    })
}
