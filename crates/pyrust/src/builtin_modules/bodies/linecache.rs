// `linecache` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/linecache.py` (issues #3229, #3445).
//
// `cache` is the live, mutable module dict (`{filename: (size, mtime, lines,
// fullname)}` or `{filename: (lazy_loader_thunk,)}`).  Every function resolves
// it through the module attribute on each call, exactly as the CPython source
// resolves the `cache` global, so external code (`traceback`, `doctest`,
// `inspect`, user tests) that mutates, clears or rebinds `linecache.cache` is
// seen immediately.  The functions likewise reach each other (`getline` ->
// `getlines` -> `updatecache` -> `lazycache`, `getlines` -> `clearcache`)
// through the module attributes, so patching one of them behaves like CPython.
//
// Reference: <https://docs.python.org/3.12/library/linecache.html>

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, Value, ValueKind};

const MODULE_DOC: &str = "Cache lines from Python source files.\n\n\
This is intended to read lines from modules imported -- hence if a filename\n\
is not found, it will look down the module search path for a file by\n\
that name.\n";

fn all_list() -> Value {
    Value::list(
        ["getline", "clearcache", "checkcache", "lazycache"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, args)
}

fn module_value(interp: &mut Interpreter) -> Result<Value> {
    interp.load_module("linecache")
}

/// The live `cache` global of the module.
fn cache_value(interp: &mut Interpreter) -> Result<Value> {
    let module = module_value(interp)?;
    interp.get_attr(&module, "cache")
}

/// A module-level dependency (`os`, `sys`, `functools`, `tokenize`): the module
/// attribute when present (CPython resolves `import os` as a module global),
/// otherwise the imported module itself.
fn dep(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = module_value(interp)?;
    match interp.get_attr(&module, name) {
        Ok(value) => Ok(value),
        Err(_) => interp.load_module(name),
    }
}

/// Call another public function of this module through the module attribute.
fn call_module_fn(interp: &mut Interpreter, name: &str, args: &[Value]) -> Result<Value> {
    let module = module_value(interp)?;
    let callee = interp.get_attr(&module, name)?;
    call_pos(interp, callee, args)
}

fn cache_contains(interp: &mut Interpreter, cache: &Value, key: &Value) -> Result<bool> {
    let found = call_method(interp, cache, "__contains__", std::slice::from_ref(key))?;
    interp.truthy_value(&found)
}

fn getitem(interp: &mut Interpreter, target: &Value, key: Value) -> Result<Value> {
    call_method(interp, target, "__getitem__", &[key])
}

fn cache_pop(interp: &mut Interpreter, cache: &Value, key: &Value) -> Result<()> {
    call_method(interp, cache, "pop", &[key.clone(), Value::none()])?;
    Ok(())
}

fn empty_lines() -> Value {
    Value::list(Vec::new())
}

fn is_os_error(err: &PyError) -> bool {
    err.class_name_is("OSError")
        || [
            "FileNotFoundError",
            "PermissionError",
            "FileExistsError",
            "IsADirectoryError",
            "NotADirectoryError",
            "InterruptedError",
            "BlockingIOError",
            "ChildProcessError",
            "ProcessLookupError",
            "TimeoutError",
            "ConnectionError",
            "BrokenPipeError",
            "ConnectionAbortedError",
            "ConnectionRefusedError",
            "ConnectionResetError",
        ]
        .iter()
        .any(|name| err.class_name_is(name))
}

fn is_import_error(err: &PyError) -> bool {
    err.class_name_is("ImportError") || err.class_name_is("ModuleNotFoundError")
}

/// Run `f`.  An error accepted by `swallow` is handed back as `Ok(Err(err))`
/// after the traceback frames recorded while it unwound are dropped, so the
/// swallowed exception (CPython's `except` clause) does not leak frames into a
/// later, unrelated exception.  Any other error propagates unchanged.
fn try_swallow<T>(
    f: impl FnOnce() -> Result<T>,
    swallow: impl FnOnce(&PyError) -> bool,
) -> Result<std::result::Result<T, PyError>> {
    let saved = pyrust_core::clone_captured_error_frames();
    match f() {
        Ok(value) => Ok(Ok(value)),
        Err(err) if swallow(&err) => {
            pyrust_core::reset_captured_error_frames();
            for frame in saved.into_iter().rev() {
                pyrust_core::record_traceback_frame(frame);
            }
            Ok(Err(err))
        }
        Err(err) => Err(err),
    }
}

/// `getattr(obj, name, None)`: only `AttributeError` is swallowed.
fn getattr_opt(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<Option<Value>> {
    let attr = try_swallow(
        || interp.get_attr(obj, name),
        |err| err.class_name_is("AttributeError"),
    )?;
    Ok(attr.ok())
}

fn os_stat(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let os = dep(interp, "os")?;
    call_method(interp, &os, "stat", std::slice::from_ref(path))
}

/// `not filename or (filename.startswith('<') and filename.endswith('>'))`.
fn is_pseudo_filename(interp: &mut Interpreter, filename: &Value) -> Result<bool> {
    if !interp.truthy_value(filename)? {
        return Ok(true);
    }
    if let Some(s) = filename.as_str() {
        return Ok(s.starts_with('<') && s.ends_with('>'));
    }
    let starts = call_method(interp, filename, "startswith", &[Value::string("<")])?;
    if !interp.truthy_value(&starts)? {
        return Ok(false);
    }
    let ends = call_method(interp, filename, "endswith", &[Value::string(">")])?;
    interp.truthy_value(&ends)
}

fn opt_or_none(arg: &OptValueRef<'_>) -> Value {
    match arg {
        OptionalArg::Provided(v) => v.0.clone(),
        OptionalArg::Omitted => Value::none(),
    }
}

/// `cache[filename][index]` style access for an entry tuple.
fn entry_item(interp: &mut Interpreter, entry: &Value, index: i64) -> Result<Value> {
    getitem(interp, entry, Value::int(index))
}

fn unpack_error(got: usize, expected: usize) -> PyError {
    if got < expected {
        PyError::named(
            "ValueError",
            format!("not enough values to unpack (expected {expected}, got {got})"),
        )
    } else {
        PyError::named(
            "ValueError",
            format!("too many values to unpack (expected {expected})"),
        )
    }
}

/// Body of `updatecache` once `os.stat` failed for the literal filename.
enum StatFallback {
    /// The lazy PEP 302 loader produced the final answer.
    Done(Value),
    /// Fell through to the `sys.path` search; carries the stat result and the
    /// resolved full name.
    Found { stat: Value, fullname: Value },
    /// Nothing found: `return []`.
    Missing,
}

fn stat_fallback(
    interp: &mut Interpreter,
    filename: &Value,
    module_globals: &Value,
) -> Result<StatFallback> {
    let basename = filename.clone();

    // Realise a lazy loader based lookup if there is one, otherwise try to
    // look up right now.
    let registered = call_module_fn(
        interp,
        "lazycache",
        &[filename.clone(), module_globals.clone()],
    )?;
    if interp.truthy_value(&registered)? {
        let cache = cache_value(interp)?;
        let entry = getitem(interp, &cache, filename.clone())?;
        let thunk = entry_item(interp, &entry, 0)?;
        let loaded = try_swallow(
            || call_pos(interp, thunk, &[]),
            |err| is_import_error(err) || is_os_error(err),
        )?;
        match loaded {
            Err(_) => {}
            Ok(data) => {
                if data.is_none() {
                    // No luck, the PEP 302 loader cannot find the source for
                    // this module.
                    return Ok(StatFallback::Done(empty_lines()));
                }
                let size = interp.object_len(&data)?;
                let split = call_method(interp, &data, "splitlines", &[])?;
                let mut lines = Vec::new();
                for line in interp.collect_iterable(&split)? {
                    lines.push(interp.eval_binary(line, BinaryOp::Add, Value::string("\n"))?);
                }
                let lines = Value::list(lines);
                let cache = cache_value(interp)?;
                call_method(
                    interp,
                    &cache,
                    "__setitem__",
                    &[
                        filename.clone(),
                        Value::tuple(vec![
                            Value::int(size),
                            Value::none(),
                            lines,
                            filename.clone(),
                        ]),
                    ],
                )?;
                let cache = cache_value(interp)?;
                let entry = getitem(interp, &cache, filename.clone())?;
                return Ok(StatFallback::Done(entry_item(interp, &entry, 2)?));
            }
        }
    }

    // Try looking through the module search path, which is only useful when
    // handling a relative filename.
    let os = dep(interp, "os")?;
    let path_mod = interp.get_attr(&os, "path")?;
    let isabs = call_method(interp, &path_mod, "isabs", std::slice::from_ref(filename))?;
    if interp.truthy_value(&isabs)? {
        return Ok(StatFallback::Missing);
    }
    let sys = dep(interp, "sys")?;
    let sys_path = interp.get_attr(&sys, "path")?;
    let dirs = interp.collect_iterable(&sys_path)?;
    for dirname in dirs {
        let os = dep(interp, "os")?;
        let path_mod = interp.get_attr(&os, "path")?;
        let joined = try_swallow(
            || call_method(interp, &path_mod, "join", &[dirname, basename.clone()]),
            |err| err.class_name_is("TypeError") || err.class_name_is("AttributeError"),
        )?;
        // Not sufficiently string-like to do anything useful with.
        let Ok(fullname) = joined else {
            continue;
        };
        if let Ok(stat) = try_swallow(|| os_stat(interp, &fullname), is_os_error)? {
            return Ok(StatFallback::Found { stat, fullname });
        }
    }
    Ok(StatFallback::Missing)
}

/// `with tokenize.open(fullname) as fp: lines = fp.readlines()`
fn read_source_lines(interp: &mut Interpreter, fullname: &Value) -> Result<Value> {
    let tokenize = dep(interp, "tokenize")?;
    let open = interp.get_attr(&tokenize, "open")?;
    let opened = call_pos(interp, open, std::slice::from_ref(fullname))?;
    let fp = call_method(interp, &opened, "__enter__", &[])?;
    let read = call_method(interp, &fp, "readlines", &[]);
    let exit_args = [Value::none(), Value::none(), Value::none()];
    match read {
        Ok(lines) => {
            call_method(interp, &opened, "__exit__", &exit_args)?;
            Ok(lines)
        }
        Err(err) => {
            let _ = call_method(interp, &opened, "__exit__", &exit_args);
            Err(err)
        }
    }
}

fn updatecache_impl(
    interp: &mut Interpreter,
    filename: &Value,
    module_globals: &Value,
) -> Result<Value> {
    let cache = cache_value(interp)?;
    if cache_contains(interp, &cache, filename)? {
        let entry = getitem(interp, &cache, filename.clone())?;
        if interp.object_len(&entry)? != 1 {
            cache_pop(interp, &cache, filename)?;
        }
    }
    if is_pseudo_filename(interp, filename)? {
        return Ok(empty_lines());
    }

    let (stat, fullname) = match try_swallow(|| os_stat(interp, filename), is_os_error)? {
        Ok(stat) => (stat, filename.clone()),
        Err(_) => match stat_fallback(interp, filename, module_globals)? {
            StatFallback::Done(lines) => return Ok(lines),
            StatFallback::Found { stat, fullname } => (stat, fullname),
            StatFallback::Missing => return Ok(empty_lines()),
        },
    };

    let read = try_swallow(
        || read_source_lines(interp, &fullname),
        |err| {
            is_os_error(err)
                || err.class_name_is("UnicodeDecodeError")
                || err.class_name_is("SyntaxError")
        },
    )?;
    let Ok(lines) = read else {
        return Ok(empty_lines());
    };
    if interp.truthy_value(&lines)? {
        let last = getitem(interp, &lines, Value::int(-1))?;
        let ends = call_method(interp, &last, "endswith", &[Value::string("\n")])?;
        if !interp.truthy_value(&ends)? {
            let fixed = interp.eval_binary(last, BinaryOp::Add, Value::string("\n"))?;
            call_method(interp, &lines, "__setitem__", &[Value::int(-1), fixed])?;
        }
    }
    let size = interp.get_attr(&stat, "st_size")?;
    let mtime = interp.get_attr(&stat, "st_mtime")?;
    let cache = cache_value(interp)?;
    call_method(
        interp,
        &cache,
        "__setitem__",
        &[
            filename.clone(),
            Value::tuple(vec![size, mtime, lines.clone(), fullname]),
        ],
    )?;
    Ok(lines)
}

fn lazycache_impl(
    interp: &mut Interpreter,
    filename: &Value,
    module_globals: &Value,
) -> Result<Value> {
    let cache = cache_value(interp)?;
    if cache_contains(interp, &cache, filename)? {
        let entry = getitem(interp, &cache, filename.clone())?;
        return Ok(Value::bool_(interp.object_len(&entry)? == 1));
    }
    if is_pseudo_filename(interp, filename)? {
        return Ok(Value::bool_(false));
    }
    // Try for a __loader__, if available.
    if interp.truthy_value(module_globals)? {
        let has_name = call_method(
            interp,
            module_globals,
            "__contains__",
            &[Value::string("__name__")],
        )?;
        if interp.truthy_value(&has_name)? {
            let spec = call_method(interp, module_globals, "get", &[Value::string("__spec__")])?;
            let spec_name = getattr_opt(interp, &spec, "name")?;
            let name = match spec_name {
                Some(n) if interp.truthy_value(&n)? => n,
                _ => getitem(interp, module_globals, Value::string("__name__"))?,
            };
            let mut loader = getattr_opt(interp, &spec, "loader")?.unwrap_or_else(Value::none);
            if loader.is_none() {
                loader = call_method(
                    interp,
                    module_globals,
                    "get",
                    &[Value::string("__loader__")],
                )?;
            }
            let get_source = getattr_opt(interp, &loader, "get_source")?;
            if interp.truthy_value(&name)?
                && let Some(get_source) = get_source
                && interp.truthy_value(&get_source)?
            {
                let functools = dep(interp, "functools")?;
                let partial = interp.get_attr(&functools, "partial")?;
                let get_lines = call_pos(interp, partial, &[get_source, name])?;
                let cache = cache_value(interp)?;
                call_method(
                    interp,
                    &cache,
                    "__setitem__",
                    &[filename.clone(), Value::tuple(vec![get_lines])],
                )?;
                return Ok(Value::bool_(true));
            }
        }
    }
    Ok(Value::bool_(false))
}

/// Wire the CPython module-level imports (`functools`, `os`, `sys`,
/// `tokenize`) so `linecache.os` and friends exist and can be patched.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    for name in ["functools", "sys", "os", "tokenize"] {
        if let Ok(dep) = interp.load_module(name) {
            module_rc.borrow_mut().insert_attr(name.to_string(), dep);
        }
    }
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
        "cache" => Value::dict(PyDict::default()),
    }

    /// Clear the cache entirely.
    fn clearcache() -> Result<Value> {
        let cache = cache_value(_interp)?;
        call_method(_interp, &cache, "clear", &[])?;
        Ok(Value::none())
    }

    /// Get a line for a Python source file from the cache.
    /// Update the cache if it doesn't contain an entry for this file already.
    fn getline(
        filename: PyValueRef<'_>,
        lineno: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] module_globals: OptValueRef<'_>,
    ) -> Result<Value> {
        let lines = call_module_fn(
            _interp,
            "getlines",
            &[filename.0.clone(), opt_or_none(&module_globals)],
        )?;
        let len = _interp.object_len(&lines)?;
        let lineno = lineno.0;
        let in_range = if let Some(n) = lineno.as_int() {
            1 <= n && n <= len
        } else {
            let lower = _interp.eval_binary(Value::int(1), BinaryOp::Le, lineno.clone())?;
            if _interp.truthy_value(&lower)? {
                let upper = _interp.eval_binary(lineno.clone(), BinaryOp::Le, Value::int(len))?;
                _interp.truthy_value(&upper)?
            } else {
                false
            }
        };
        if in_range {
            let index = _interp.eval_binary(lineno.clone(), BinaryOp::Sub, Value::int(1))?;
            return getitem(_interp, &lines, index);
        }
        Ok(Value::string(""))
    }

    /// Get the lines for a Python source file from the cache.
    /// Update the cache if it doesn't contain an entry for this file already.
    fn getlines(
        filename: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] module_globals: OptValueRef<'_>,
    ) -> Result<Value> {
        let cache = cache_value(_interp)?;
        if cache_contains(_interp, &cache, filename.0)? {
            let entry = getitem(_interp, &cache, filename.0.clone())?;
            if _interp.object_len(&entry)? != 1 {
                let entry = getitem(_interp, &cache, filename.0.clone())?;
                return entry_item(_interp, &entry, 2);
            }
        }
        let updated = try_swallow(
            || {
                call_module_fn(
                    _interp,
                    "updatecache",
                    &[filename.0.clone(), opt_or_none(&module_globals)],
                )
            },
            |err| err.class_name_is("MemoryError"),
        )?;
        match updated {
            Ok(lines) => Ok(lines),
            Err(_) => {
                call_module_fn(_interp, "clearcache", &[])?;
                Ok(empty_lines())
            }
        }
    }

    /// Discard cache entries that are out of date.
    /// (This is not checked upon each call!)
    fn checkcache(#[default(OptionalArg::Omitted)] filename: OptValueRef<'_>) -> Result<Value> {
        let cache = cache_value(_interp)?;
        let filenames: Vec<Value> = match &filename {
            OptionalArg::Provided(v) if !v.0.is_none() => {
                if cache_contains(_interp, &cache, v.0)? {
                    vec![v.0.clone()]
                } else {
                    return Ok(Value::none());
                }
            }
            _ => {
                let keys = call_method(_interp, &cache, "keys", &[])?;
                _interp.collect_iterable(&keys)?
            }
        };
        for fname in filenames {
            let cache = cache_value(_interp)?;
            let entry = getitem(_interp, &cache, fname.clone())?;
            if _interp.object_len(&entry)? == 1 {
                // lazy cache entry, leave it lazy.
                continue;
            }
            let parts = _interp.collect_iterable(&entry)?;
            if parts.len() != 4 {
                return Err(unpack_error(parts.len(), 4));
            }
            let size = parts[0].clone();
            let mtime = parts[1].clone();
            let fullname = parts[3].clone();
            if mtime.is_none() {
                // no-op for files loaded via a __loader__
                continue;
            }
            let Ok(stat) = try_swallow(|| os_stat(_interp, &fullname), is_os_error)? else {
                cache_pop(_interp, &cache, &fname)?;
                continue;
            };
            let st_size = _interp.get_attr(&stat, "st_size")?;
            let size_differs = _interp.eval_binary(size, BinaryOp::Ne, st_size)?;
            let mut stale = _interp.truthy_value(&size_differs)?;
            if !stale {
                let st_mtime = _interp.get_attr(&stat, "st_mtime")?;
                let mtime_differs = _interp.eval_binary(mtime, BinaryOp::Ne, st_mtime)?;
                stale = _interp.truthy_value(&mtime_differs)?;
            }
            if stale {
                cache_pop(_interp, &cache, &fname)?;
            }
        }
        Ok(Value::none())
    }

    /// Update a cache entry and return its list of lines.
    /// If something's wrong, print a message, discard the cache entry,
    /// and return an empty list.
    fn updatecache(
        filename: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] module_globals: OptValueRef<'_>,
    ) -> Result<Value> {
        updatecache_impl(_interp, filename.0, &opt_or_none(&module_globals))
    }

    /// Seed the cache for filename with module_globals.
    ///
    /// The module loader will be asked for the source only when getlines is
    /// called, not immediately.
    ///
    /// If there is an entry in the cache already, it is not altered.
    ///
    /// :return: True if a lazy load is registered in the cache,
    ///     otherwise False. To register such a load a module loader with a
    ///     get_source method must be found, the filename must be a cacheable
    ///     filename, and the filename must not be already cached.
    fn lazycache(filename: PyValueRef<'_>, module_globals: PyValueRef<'_>) -> Result<Value> {
        lazycache_impl(_interp, filename.0, module_globals.0)
    }
}
