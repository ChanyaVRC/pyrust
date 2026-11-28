// `importlib.util` module — 100% native Rust port of CPython 3.12.3's
// `Lib/importlib/util.py` together with the `_bootstrap` /
// `_bootstrap_external` helpers it re-exports (issues #3189, #3470); there is
// no Python-source injection.
//
// Public API: `MAGIC_NUMBER`, `cache_from_source`, `source_from_cache`,
// `decode_source`, `resolve_name`, `find_spec`, `spec_from_loader`,
// `spec_from_file_location`, `module_from_spec`, `source_hash`, `LazyLoader`
// and the `Loader` ABC.  The private names CPython's test-suite and third
// party code reach for (`_RAW_MAGIC_NUMBER`, `_resolve_name`, `_find_spec`,
// `_find_spec_from_path`, `_init_module_attrs`, `_LazyModule`,
// `_incompatible_extension_module_restrictions`) keep their signatures.
//
// The path arithmetic of `cache_from_source` / `source_from_cache` /
// `spec_from_file_location` is `_bootstrap_external`'s own replacement for
// `os.path` (`_path_split`, `_path_join`, `_path_abspath`), ported per
// platform, so `__pycache__` paths are byte-identical to CPython on both
// Windows and POSIX.  `decode_source` goes through `tokenize.detect_encoding`
// and mirrors `IncrementalNewlineDecoder(None, True).decode(text)`, including
// the dropped trailing `\r`.  `source_hash` is `_imp.source_hash` (SipHash-1-3,
// keyed with the raw pyc magic number).
//
// PyRust specifics kept from the previous implementation:
// - `sys.meta_path` is empty here, so `_find_spec` falls back to the built-in
//   module table and then to `importlib.machinery.PathFinder`.
// - `find_spec` synthesises a spec for modules the runtime loaded natively
//   whose `__spec__` slot is still `None` (CPython would raise `ValueError`).
// - A module object created by `types.ModuleType` cannot change its class, so
//   `module_from_spec` builds a `_LazyModule` directly for a `LazyLoader`
//   spec and `LazyLoader.exec_module` falls back to eager execution for a
//   module that cannot be retyped.
//
// Reference: <https://docs.python.org/3.12/library/importlib.html#module-importlib.util>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::genericpath::ops::{PathValue, fspath_resolve};
use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::macros::link_class_bases;
use crate::builtin_modules::py_compile::siphash13;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBytesLike, PyValue, PyValueRef};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, class_is_subclass_of, lookup_exc_class, render_value_repr,
};
use crate::value::{PyClass, PyDict, PyKey, PyModule, Value, ValueKind};

#[cfg(windows)]
use crate::builtin_modules::ntpath::ops::nt_splitroot_str;

const MODULE_DOC: &str = "Utility code for constructing importers, etc.";

/// `_bootstrap_external._RAW_MAGIC_NUMBER`: `3531 | ord("\r") << 16 | ord("\n") << 24`.
const RAW_MAGIC_NUMBER: u64 = 3531 | (b'\r' as u64) << 16 | (b'\n' as u64) << 24;

const PYCACHE: &str = "__pycache__";
const OPT: &str = "opt-";

#[cfg(windows)]
const PATH_SEPARATORS: &str = "\\/";
#[cfg(not(windows))]
const PATH_SEPARATORS: &str = "/";
#[cfg(windows)]
pub(crate) const PATH_SEP: char = '\\';
#[cfg(not(windows))]
pub(crate) const PATH_SEP: char = '/';

// The `def`s of `util.py` / `_bootstrap*.py`: CPython reports arity errors for
// these in the interpreter's `initialize_locals` wording.
const SIG_CACHE_FROM_SOURCE: Sig = Sig {
    qualname: "cache_from_source",
    params: &["path", "debug_override"],
    defaults: 1,
    kwonly: &["optimization"],
    var_kw: false,
};
const SIG_SOURCE_FROM_CACHE: Sig = Sig {
    qualname: "source_from_cache",
    params: &["path"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_SOURCE_HASH: Sig = Sig {
    qualname: "source_hash",
    params: &["source_bytes"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_DECODE_SOURCE: Sig = Sig {
    qualname: "decode_source",
    params: &["source_bytes"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_RESOLVE_NAME: Sig = Sig {
    qualname: "resolve_name",
    params: &["name", "package"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_PRIVATE_RESOLVE_NAME: Sig = Sig {
    qualname: "_resolve_name",
    params: &["name", "package", "level"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_SPEC_FROM_LOADER: Sig = Sig {
    qualname: "spec_from_loader",
    params: &["name", "loader"],
    defaults: 0,
    kwonly: &["origin", "is_package"],
    var_kw: false,
};
const SIG_SPEC_FROM_FILE_LOCATION: Sig = Sig {
    qualname: "spec_from_file_location",
    params: &["name", "location"],
    defaults: 1,
    kwonly: &["loader", "submodule_search_locations"],
    var_kw: false,
};
const SIG_INIT_MODULE_ATTRS: Sig = Sig {
    qualname: "_init_module_attrs",
    params: &["spec", "module"],
    defaults: 0,
    kwonly: &["override"],
    var_kw: false,
};
const SIG_MODULE_FROM_SPEC: Sig = Sig {
    qualname: "module_from_spec",
    params: &["spec"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_FIND_SPEC_PRIVATE: Sig = Sig {
    qualname: "_find_spec",
    params: &["name", "path", "target"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_FIND_SPEC_FROM_PATH: Sig = Sig {
    qualname: "_find_spec_from_path",
    params: &["name", "path"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_FIND_SPEC: Sig = Sig {
    qualname: "find_spec",
    params: &["name", "package"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};

// ── Small interpreter helpers ────────────────────────────────────────────────

pub(crate) fn type_error(message: impl Into<String>) -> PyError {
    PyError::named("TypeError", message.into())
}

fn value_error(message: impl Into<String>) -> PyError {
    PyError::named("ValueError", message.into())
}

pub(crate) fn s(text: &str) -> Value {
    Value::string(text)
}

pub(crate) fn call(interp: &mut Interpreter, callee: &Value, values: &[Value]) -> Result<Value> {
    call_kw(interp, callee, values, &[])
}

pub(crate) fn call_kw(
    interp: &mut Interpreter,
    callee: &Value,
    values: &[Value],
    keywords: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = values
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    expanded.extend(keywords.iter().map(|(name, value)| ExpandedCallArg {
        name: Some((*name).to_string()),
        value: value.clone(),
    }));
    interp.call_function_expanded(callee.clone(), &expanded)
}

pub(crate) fn call_method(
    interp: &mut Interpreter,
    receiver: &Value,
    name: &str,
    values: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(receiver, name)?;
    call(interp, &callee, values)
}

/// Whether `error` is an instance of the named builtin exception class
/// (`except OSError:`), plus an equivalent error to re-raise.
pub(crate) fn error_is(
    interp: &mut Interpreter,
    error: PyError,
    name: &str,
) -> Result<(Option<Value>, PyError)> {
    let exception = interp.materialize_pyerror(error)?;
    let hit = exception.as_py_instance_rc().is_some_and(|instance| {
        let class = Rc::clone(&instance.borrow().class);
        lookup_exc_class(name).is_some_and(|expected| class_is_subclass_of(&class, &expected))
    });
    Ok((hit.then(|| exception.clone()), PyError::raised(exception)))
}

/// `getattr(obj, name)` that treats `AttributeError` as "absent".
pub(crate) fn get_attr_opt(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
) -> Result<Option<Value>> {
    match interp.get_attr(obj, name) {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            let (hit, error) = error_is(interp, error, "AttributeError")?;
            if hit.is_some() { Ok(None) } else { Err(error) }
        }
    }
}

/// `getattr(obj, name, None)`.
fn get_attr_or_none(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<Value> {
    Ok(get_attr_opt(interp, obj, name)?.unwrap_or_else(Value::none))
}

/// `hasattr(obj, name)`.
pub(crate) fn has_attr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    Ok(get_attr_opt(interp, obj, name)?.is_some())
}

/// `try: obj.name = value() except AttributeError: pass`.
fn try_set_attr(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
    value: impl FnOnce(&mut Interpreter) -> Result<Value>,
) -> Result<()> {
    let result = value(interp).and_then(|v| interp.assign_attr(obj.clone(), name, v));
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            let (hit, error) = error_is(interp, error, "AttributeError")?;
            if hit.is_some() { Ok(()) } else { Err(error) }
        }
    }
}

/// `raise <error> from None`.
fn from_none(interp: &mut Interpreter, error: PyError) -> PyError {
    let Ok(exception) = interp.materialize_pyerror(error) else {
        return PyError::runtime("importlib.util: could not build exception".to_string());
    };
    let _ = interp.assign_attr(exception.clone(), "__cause__", Value::none());
    let _ = interp.assign_attr(
        exception.clone(),
        "__suppress_context__",
        Value::bool_(true),
    );
    PyError::raised(exception)
}

/// `raise <error> from <cause>`.
pub(crate) fn from_cause(interp: &mut Interpreter, error: PyError, cause: Value) -> PyError {
    let Ok(exception) = interp.materialize_pyerror(error) else {
        return PyError::runtime("importlib.util: could not build exception".to_string());
    };
    let _ = interp.assign_attr(exception.clone(), "__cause__", cause);
    let _ = interp.assign_attr(
        exception.clone(),
        "__suppress_context__",
        Value::bool_(true),
    );
    PyError::raised(exception)
}

pub(crate) fn repr_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    render_value_repr(interp, value)
}

/// `str(value)`.
pub(crate) fn str_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }
    let Some(class) = crate::interpreter::primitive_class_by_name("str") else {
        return Err(PyError::runtime("importlib.util: no str class".to_string()));
    };
    let result = call(interp, &Value::py_class(class), std::slice::from_ref(value))?;
    Ok(result.as_str().unwrap_or_default().to_string())
}

pub(crate) fn sys_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let sys = interp.load_module("sys")?;
    interp.get_attr(&sys, name)
}

fn machinery_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let machinery = interp.load_module("importlib.machinery")?;
    interp.get_attr(&machinery, name)
}

fn util_class(interp: &mut Interpreter, name: &str) -> Result<Rc<RefCell<PyClass>>> {
    let util = interp.load_module("importlib.util")?;
    let class = interp.get_attr(&util, name)?;
    match class.kind() {
        ValueKind::PyClass(class) => Ok(Rc::clone(class)),
        _ => Err(PyError::runtime(format!(
            "importlib.util: {name} is not a class"
        ))),
    }
}

/// `name in sys.modules`.
fn in_sys_modules(interp: &mut Interpreter, name: &Value) -> Result<bool> {
    let modules = sys_attr(interp, "modules")?;
    let found = call_method(interp, &modules, "__contains__", std::slice::from_ref(name))?;
    interp.truthy_value(&found)
}

/// `sys.modules[name]`.
fn sys_modules_get(interp: &mut Interpreter, name: &Value) -> Result<Value> {
    let modules = sys_attr(interp, "modules")?;
    call_method(interp, &modules, "__getitem__", std::slice::from_ref(name))
}

fn new_dict() -> Value {
    Value::dict(PyDict::default())
}

fn dict_set(dict: &Value, key: &str, value: Value) -> Result<()> {
    dict.dict_insert(PyKey::str_from(key), value)?;
    Ok(())
}

/// `name.startswith('.')` for a `str` (any other receiver goes through the
/// attribute protocol, so a non-string raises CPython's `AttributeError`).
fn starts_with_dot(interp: &mut Interpreter, name: &Value) -> Result<bool> {
    if let Some(text) = name.as_str() {
        return Ok(text.starts_with('.'));
    }
    let result = call_method(interp, name, "startswith", &[s(".")])?;
    interp.truthy_value(&result)
}

// ── `_bootstrap_external` path helpers ───────────────────────────────────────

fn is_sep(c: char) -> bool {
    PATH_SEPARATORS.contains(c)
}

/// `_path_split(path)` — `os.path.split` for a single trailing component.
pub(crate) fn path_split(path: &str) -> (&str, &str) {
    match path.rfind(is_sep) {
        None => ("", path),
        Some(i) => (&path[..i], &path[i + 1..]),
    }
}

/// `_path_join(*path_parts)` on Windows.
#[cfg(windows)]
pub(crate) fn path_join(parts: &[&str]) -> String {
    if parts.is_empty() {
        return String::new();
    }
    if parts.len() == 1 {
        return parts[0].to_string();
    }
    let mut root = String::new();
    let mut path: Vec<String> = Vec::new();
    for part in parts {
        let (drive, rooted, tail) = nt_splitroot_str(part);
        let new_root = format!("{drive}{rooted}");
        if new_root.starts_with(is_sep) || new_root.ends_with(is_sep) {
            let stripped = new_root.trim_end_matches(is_sep);
            if !stripped.is_empty() {
                root = stripped.to_string();
            }
            path = vec![format!("{PATH_SEP}{tail}")];
        } else if new_root.ends_with(':') {
            if root.to_lowercase() != new_root.to_lowercase() {
                // Drive-relative paths are resolved by the OS: reset the tail
                // but add no separator prefix.
                root = new_root;
                path = vec![tail];
            } else {
                path.push(tail);
            }
        } else {
            if !new_root.is_empty() {
                root = new_root;
            }
            path.push(tail);
        }
    }
    let path: Vec<String> = path
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.trim_end_matches(is_sep).to_string())
        .collect();
    if path.len() == 1 && path[0].is_empty() {
        // Avoid losing the root's trailing separator when joining with nothing.
        return format!("{root}{PATH_SEP}");
    }
    format!("{root}{}", path.join(&PATH_SEP.to_string()))
}

/// `_path_join(*path_parts)` on POSIX.
#[cfg(not(windows))]
pub(crate) fn path_join(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .map(|part| part.trim_end_matches(is_sep))
        .collect::<Vec<_>>()
        .join(&PATH_SEP.to_string())
}

#[cfg(windows)]
pub(crate) fn path_isabs(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let (drive, rooted, _) = nt_splitroot_str(path);
    let root = format!("{drive}{rooted}").replace('/', "\\");
    root.chars().count() > 1 && (root.starts_with("\\\\") || root.ends_with('\\'))
}

#[cfg(not(windows))]
pub(crate) fn path_isabs(path: &str) -> bool {
    path.starts_with(is_sep)
}

/// `_path_abspath(path)` — a cheap `os.path.abspath` that never normalises.
pub(crate) fn path_abspath(interp: &mut Interpreter, path: &str) -> Result<String> {
    if path_isabs(path) {
        return Ok(path.to_string());
    }
    let mut path = path;
    for sep in PATH_SEPARATORS.chars() {
        path = path.strip_prefix(&format!(".{sep}")).unwrap_or(path);
    }
    let os = interp.load_module("os")?;
    let cwd = call_method(interp, &os, "getcwd", &[])?;
    let cwd = cwd.as_str().unwrap_or_default().to_string();
    Ok(path_join(&[&cwd, path]))
}

/// `os.fspath(path)` narrowed to the `str` the path helpers accept.
///
/// CPython feeds a `bytes` path to `_path_split`, whose `path.rfind('/')`
/// raises this `TypeError`.
pub(crate) fn fspath_str(interp: &mut Interpreter, path: &Value) -> Result<String> {
    match fspath_resolve(interp, path)? {
        PathValue::Str(text) => Ok(text),
        PathValue::Bytes(_) => Err(type_error(
            "argument should be integer or bytes-like object, not 'str'",
        )),
    }
}

/// `isalnum()` of a Python `str`.
fn is_alnum(text: &str) -> bool {
    !text.is_empty() && text.chars().all(char::is_alphanumeric)
}

fn cache_tag(interp: &mut Interpreter) -> Result<Option<String>> {
    let implementation = sys_attr(interp, "implementation")?;
    let tag = interp.get_attr(&implementation, "cache_tag")?;
    Ok(tag.as_str().map(str::to_string))
}

fn pycache_prefix(interp: &mut Interpreter) -> Result<Option<String>> {
    let prefix = sys_attr(interp, "pycache_prefix")?;
    Ok(prefix.as_str().map(str::to_string))
}

// ── `cache_from_source` / `source_from_cache` / `decode_source` ──────────────

pub(crate) fn cache_from_source_impl(
    interp: &mut Interpreter,
    path: &Value,
    debug_override: &Value,
    optimization: Value,
) -> Result<String> {
    let mut optimization = optimization;
    if !debug_override.is_none() {
        let warnings = interp.load_module("warnings")?;
        let warn = interp.get_attr(&warnings, "warn")?;
        let category = lookup_exc_class("DeprecationWarning")
            .map(Value::py_class)
            .ok_or_else(|| PyError::runtime("DeprecationWarning is missing".to_string()))?;
        call(
            interp,
            &warn,
            &[
                s("the debug_override parameter is deprecated; use 'optimization' instead"),
                category,
            ],
        )?;
        if !optimization.is_none() {
            return Err(type_error(
                "debug_override or optimization must be set to None",
            ));
        }
        optimization = if interp.truthy_value(debug_override)? {
            s("")
        } else {
            Value::int(1)
        };
    }
    let path = fspath_str(interp, path)?;
    let (head, tail) = path_split(&path);
    let (base, sep, rest) = match tail.rfind('.') {
        Some(i) => (&tail[..i], ".", &tail[i + 1..]),
        None => ("", "", tail),
    };
    let Some(tag) = cache_tag(interp)? else {
        return Err(PyError::named(
            "NotImplementedError",
            "sys.implementation.cache_tag is None",
        ));
    };
    let mut almost_filename = format!("{}{sep}{tag}", if base.is_empty() { rest } else { base });
    if optimization.is_none() {
        let flags = sys_attr(interp, "flags")?;
        let level = interp.get_attr(&flags, "optimize")?;
        optimization = if level.as_int() == Some(0) {
            s("")
        } else {
            level
        };
    }
    let optimization = str_of(interp, &optimization)?;
    if !optimization.is_empty() {
        if !is_alnum(&optimization) {
            let repr = repr_of(interp, &s(&optimization))?;
            return Err(value_error(format!("{repr} is not alphanumeric")));
        }
        almost_filename = format!("{almost_filename}.{OPT}{optimization}");
    }
    let filename = format!("{almost_filename}.pyc");
    if let Some(prefix) = pycache_prefix(interp)? {
        let mut head = path_abspath(interp, head)?;
        let mut chars = head.chars();
        if let (Some(first), Some(second)) = (chars.next(), chars.next())
            && second == ':'
            && !is_sep(first)
        {
            head = head[first.len_utf8() + second.len_utf8()..].to_string();
        }
        return Ok(path_join(&[
            &prefix,
            head.trim_start_matches(is_sep),
            &filename,
        ]));
    }
    Ok(path_join(&[head, PYCACHE, &filename]))
}

fn source_from_cache_impl(interp: &mut Interpreter, path: &Value) -> Result<String> {
    if cache_tag(interp)?.is_none() {
        return Err(PyError::named(
            "NotImplementedError",
            "sys.implementation.cache_tag is None",
        ));
    }
    let path = fspath_str(interp, path)?;
    let (head, pycache_filename) = path_split(&path);
    let mut head = head.to_string();
    let mut found_in_pycache_prefix = false;
    if let Some(prefix) = pycache_prefix(interp)? {
        let stripped = prefix.trim_end_matches(is_sep);
        if head.starts_with(&format!("{stripped}{PATH_SEP}")) {
            head = head[stripped.len()..].to_string();
            found_in_pycache_prefix = true;
        }
    }
    if !found_in_pycache_prefix {
        let (parent, pycache) = path_split(&head);
        if pycache != PYCACHE {
            let repr = repr_of(interp, &s(&path))?;
            return Err(value_error(format!(
                "{PYCACHE} not bottom-level directory in {repr}"
            )));
        }
        head = parent.to_string();
    }
    let dot_count = pycache_filename.matches('.').count();
    if dot_count != 2 && dot_count != 3 {
        let repr = repr_of(interp, &s(pycache_filename))?;
        return Err(value_error(format!("expected only 2 or 3 dots in {repr}")));
    }
    if dot_count == 3 {
        let segments: Vec<&str> = pycache_filename.rsplitn(3, '.').collect();
        // `rsplit('.', 2)[-2]`: the segment between the last two dots.
        let optimization = segments[1];
        let Some(opt_level) = optimization.strip_prefix(OPT) else {
            let repr = repr_of(interp, &s(OPT))?;
            return Err(value_error(format!(
                "optimization portion of filename does not start with {repr}"
            )));
        };
        if !is_alnum(opt_level) {
            let repr = repr_of(interp, &s(optimization))?;
            return Err(value_error(format!(
                "optimization level {repr} is not an alphanumeric value"
            )));
        }
    }
    let base_filename = pycache_filename
        .split_once('.')
        .map_or(pycache_filename, |(base, _)| base);
    Ok(path_join(&[&head, &format!("{base_filename}.py")]))
}

/// What `IncrementalNewlineDecoder(None, True).decode(text)` returns for a
/// non-final chunk: a trailing `\r` is held back, `\r\n` and `\r` become `\n`.
fn translate_newlines(text: &str) -> String {
    let text = text.strip_suffix('\r').unwrap_or(text);
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub(crate) fn decode_source_impl(interp: &mut Interpreter, source_bytes: &Value) -> Result<Value> {
    let io = interp.load_module("io")?;
    let bytes_io = interp.get_attr(&io, "BytesIO")?;
    // `_io.BytesIO(None)` is an empty stream, and a `str` is refused with the
    // buffer protocol's wording.
    if source_bytes.as_str().is_some() {
        return Err(type_error("a bytes-like object is required, not 'str'"));
    }
    let initial = if source_bytes.is_none() {
        Vec::new()
    } else {
        vec![source_bytes.clone()]
    };
    let stream = call(interp, &bytes_io, &initial)?;
    let readline = interp.get_attr(&stream, "readline")?;
    let tokenize = interp.load_module("tokenize")?;
    let detect = interp.get_attr(&tokenize, "detect_encoding")?;
    let detected = call(interp, &detect, &[readline])?;
    let detected = interp.collect_iterable(&detected)?;
    let encoding = detected.first().cloned().unwrap_or_else(Value::none);
    let decoded = call_method(interp, source_bytes, "decode", &[encoding])?;
    match decoded.as_str() {
        Some(text) => Ok(Value::string(translate_newlines(text))),
        None => Ok(decoded),
    }
}

// ── `resolve_name` ───────────────────────────────────────────────────────────

/// `_resolve_name(name, package, level)`.
fn resolve_name_level(
    interp: &mut Interpreter,
    name: &Value,
    package: &Value,
    level: usize,
) -> Result<Value> {
    // `bits = package.rsplit('.', level - 1)`; `len(bits) < level` means the
    // package has fewer than `level` components.
    let base = match package.as_str() {
        Some(text) => {
            let mut base = text;
            for _ in 1..level {
                match base.rfind('.') {
                    Some(i) => base = &base[..i],
                    None => {
                        return Err(PyError::import_error(
                            "ImportError",
                            "attempted relative import beyond top-level package",
                            None,
                        ));
                    }
                }
            }
            s(base)
        }
        None => {
            let bits = call_method(
                interp,
                package,
                "rsplit",
                &[s("."), Value::int(level as i64 - 1)],
            )?;
            let bits = interp.collect_iterable(&bits)?;
            if bits.len() < level {
                return Err(PyError::import_error(
                    "ImportError",
                    "attempted relative import beyond top-level package",
                    None,
                ));
            }
            bits[0].clone()
        }
    };
    if interp.truthy_value(name)? {
        let base = str_of(interp, &base)?;
        let name = str_of(interp, name)?;
        Ok(Value::string(format!("{base}.{name}")))
    } else {
        Ok(base)
    }
}

fn resolve_name_impl(interp: &mut Interpreter, name: &Value, package: &Value) -> Result<Value> {
    if !starts_with_dot(interp, name)? {
        return Ok(name.clone());
    }
    if !interp.truthy_value(package)? {
        let repr = repr_of(interp, name)?;
        return Err(PyError::import_error(
            "ImportError",
            format!("no package specified for {repr} (required for relative module names)"),
            None,
        ));
    }
    let text = name.as_str().unwrap_or_default();
    let level = text.chars().take_while(|c| *c == '.').count();
    resolve_name_level(interp, &s(&text[level..]), package, level)
}

// ── Spec construction ────────────────────────────────────────────────────────

fn module_spec(
    interp: &mut Interpreter,
    name: &Value,
    loader: &Value,
    origin: Value,
    is_package: Option<Value>,
) -> Result<Value> {
    let module_spec = machinery_attr(interp, "ModuleSpec")?;
    let mut keywords = vec![("origin", origin)];
    if let Some(is_package) = is_package {
        keywords.push(("is_package", is_package));
    }
    call_kw(
        interp,
        &module_spec,
        &[name.clone(), loader.clone()],
        &keywords,
    )
}

fn builtin_spec(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let importer = machinery_attr(interp, "BuiltinImporter")?;
    module_spec(
        interp,
        &s(name),
        &importer,
        s("built-in"),
        Some(Value::bool_(crate::builtin_modules::is_builtin_package(
            name,
        ))),
    )
}

/// `spec_from_file_location`; `submodule_search_locations` is `None` for the
/// `_POPULATE` default.
pub(crate) fn spec_from_file_location_impl(
    interp: &mut Interpreter,
    name: &Value,
    location: Option<Value>,
    loader: Value,
    submodule_search_locations: Option<Value>,
) -> Result<Value> {
    let location = match location {
        None => {
            let mut location = s("<unknown>");
            if has_attr(interp, &loader, "get_filename")? {
                match call_method(interp, &loader, "get_filename", std::slice::from_ref(name)) {
                    Ok(found) => location = found,
                    Err(error) => {
                        let (hit, error) = error_is(interp, error, "ImportError")?;
                        if hit.is_none() {
                            return Err(error);
                        }
                    }
                }
            }
            location
        }
        Some(location) => {
            let path = fspath_str(interp, &location)?;
            match path_abspath(interp, &path) {
                Ok(absolute) => Value::string(absolute),
                Err(error) => {
                    let (hit, error) = error_is(interp, error, "OSError")?;
                    if hit.is_none() {
                        return Err(error);
                    }
                    Value::string(path)
                }
            }
        }
    };

    let spec = module_spec(interp, name, &loader, location.clone(), None)?;
    interp.assign_attr(spec.clone(), "_set_fileattr", Value::bool_(true))?;

    let mut loader = loader;
    if loader.is_none() {
        let location_text = location.as_str().ok_or_else(|| {
            PyError::attribute_error(
                format!(
                    "'{}' object has no attribute 'endswith'",
                    crate::interpreter::value_type_name_str(&location)
                ),
                Some("endswith".to_string()),
                Some(location.clone()),
            )
        })?;
        // `_get_supported_file_loaders()`: extension, source, bytecode.
        let supported = [
            ("ExtensionFileLoader", "EXTENSION_SUFFIXES"),
            ("SourceFileLoader", "SOURCE_SUFFIXES"),
            ("SourcelessFileLoader", "BYTECODE_SUFFIXES"),
        ];
        let mut found = None;
        for (loader_class, suffixes) in supported {
            let suffixes = machinery_attr(interp, suffixes)?;
            let suffixes = interp.collect_iterable(&suffixes)?;
            if suffixes
                .iter()
                .any(|suffix| suffix.as_str().is_some_and(|x| location_text.ends_with(x)))
            {
                found = Some(loader_class);
                break;
            }
        }
        let Some(loader_class) = found else {
            return Ok(Value::none());
        };
        let loader_class = machinery_attr(interp, loader_class)?;
        loader = call(interp, &loader_class, &[name.clone(), location.clone()])?;
        interp.assign_attr(spec.clone(), "loader", loader.clone())?;
    }

    match submodule_search_locations {
        None => {
            if has_attr(interp, &loader, "is_package")? {
                match call_method(interp, &loader, "is_package", std::slice::from_ref(name)) {
                    Ok(is_package) => {
                        if interp.truthy_value(&is_package)? {
                            interp.assign_attr(
                                spec.clone(),
                                "submodule_search_locations",
                                Value::list(Vec::new()),
                            )?;
                        }
                    }
                    Err(error) => {
                        let (hit, error) = error_is(interp, error, "ImportError")?;
                        if hit.is_none() {
                            return Err(error);
                        }
                    }
                }
            }
        }
        Some(locations) => {
            interp.assign_attr(spec.clone(), "submodule_search_locations", locations)?;
        }
    }

    let locations = interp.get_attr(&spec, "submodule_search_locations")?;
    let is_empty_list = interp.eval_binary(
        locations.clone(),
        crate::ast::BinaryOp::Eq,
        Value::list(Vec::new()),
    )?;
    if interp.truthy_value(&is_empty_list)? && interp.truthy_value(&location)? {
        let location_text = str_of(interp, &location)?;
        let dirname = path_split(&location_text).0.to_string();
        call_method(interp, &locations, "append", &[Value::string(dirname)])?;
    }
    Ok(spec)
}

pub(crate) fn spec_from_loader_impl(
    interp: &mut Interpreter,
    name: &Value,
    loader: &Value,
    origin: Value,
    is_package: Value,
) -> Result<Value> {
    let origin = if origin.is_none() {
        get_attr_or_none(interp, loader, "_ORIGIN")?
    } else {
        origin
    };
    if !interp.truthy_value(&origin)? && has_attr(interp, loader, "get_filename")? {
        if is_package.is_none() {
            return spec_from_file_location_impl(interp, name, None, loader.clone(), None);
        }
        let search = if interp.truthy_value(&is_package)? {
            Value::list(Vec::new())
        } else {
            Value::none()
        };
        return spec_from_file_location_impl(interp, name, None, loader.clone(), Some(search));
    }

    let mut is_package = is_package;
    if is_package.is_none() {
        if has_attr(interp, loader, "is_package")? {
            match call_method(interp, loader, "is_package", std::slice::from_ref(name)) {
                Ok(value) => is_package = value,
                Err(error) => {
                    let (hit, error) = error_is(interp, error, "ImportError")?;
                    if hit.is_none() {
                        return Err(error);
                    }
                    is_package = Value::none();
                }
            }
        } else {
            is_package = Value::bool_(false);
        }
    }
    module_spec(interp, name, loader, origin, Some(is_package))
}

// ── Module construction ──────────────────────────────────────────────────────

pub(crate) fn init_module_attrs_impl(
    interp: &mut Interpreter,
    spec: &Value,
    module: &Value,
    override_: bool,
) -> Result<()> {
    // `__name__`
    if override_ || get_attr_or_none(interp, module, "__name__")?.is_none() {
        try_set_attr(interp, module, "__name__", |interp| {
            interp.get_attr(spec, "name")
        })?;
    }
    // `__loader__`
    if override_ || get_attr_or_none(interp, module, "__loader__")?.is_none() {
        let mut loader = interp.get_attr(spec, "loader")?;
        if loader.is_none() {
            // A backward compatibility hack.
            let locations = interp.get_attr(spec, "submodule_search_locations")?;
            if !locations.is_none() {
                // `NamespaceLoader.__new__(NamespaceLoader)` with `_path` set
                // directly: `__init__` would wrap the locations again.
                loader = crate::builtin_modules::importlib_machinery::bare_namespace_loader(
                    interp, locations,
                )?;
                interp.assign_attr(spec.clone(), "loader", loader.clone())?;
                // Namespace packages have no location, but their
                // `__spec__.origin` is `None`, so `__file__` is too.
                interp.assign_attr(module.clone(), "__file__", Value::none())?;
            }
        }
        try_set_attr(interp, module, "__loader__", |_| Ok(loader))?;
    }
    // `__package__`
    if override_ || get_attr_or_none(interp, module, "__package__")?.is_none() {
        try_set_attr(interp, module, "__package__", |interp| {
            interp.get_attr(spec, "parent")
        })?;
    }
    // `__spec__`
    try_set_attr(interp, module, "__spec__", |_| Ok(spec.clone()))?;
    // `__path__`
    if override_ || get_attr_or_none(interp, module, "__path__")?.is_none() {
        let locations = interp.get_attr(spec, "submodule_search_locations")?;
        if !locations.is_none() {
            try_set_attr(interp, module, "__path__", |_| Ok(locations))?;
        }
    }
    // `__file__` / `__cached__`
    let has_location = interp.get_attr(spec, "has_location")?;
    if interp.truthy_value(&has_location)? {
        if override_ || get_attr_or_none(interp, module, "__file__")?.is_none() {
            try_set_attr(interp, module, "__file__", |interp| {
                interp.get_attr(spec, "origin")
            })?;
        }
        if override_ || get_attr_or_none(interp, module, "__cached__")?.is_none() {
            let cached = interp.get_attr(spec, "cached")?;
            if !cached.is_none() {
                try_set_attr(interp, module, "__cached__", |_| Ok(cached))?;
            }
        }
    }
    Ok(())
}

fn new_module(interp: &mut Interpreter, spec: &Value, name: &Value) -> Result<Value> {
    // A `LazyLoader` swaps the module's class in `exec_module`; a plain
    // `types.ModuleType` cannot be retyped here, so start from `_LazyModule`
    // (which behaves as a plain module until `loader_state` is installed).
    let loader = interp.get_attr(spec, "loader")?;
    let lazy_class = util_class(interp, "LazyLoader")?;
    let is_lazy = loader
        .as_py_instance_rc()
        .is_some_and(|instance| class_is_subclass_of(&instance.borrow().class, &lazy_class));
    let class = if is_lazy {
        util_class(interp, "_LazyModule")?
    } else {
        crate::interpreter::module_type_singleton()
    };
    let module = call(interp, &Value::py_class(class), std::slice::from_ref(name))?;
    if is_lazy {
        // `exec(code, module.__dict__)` needs a real dict: give the instance
        // one as its live backing store.
        let proxy = interp.get_attr(&module, "__dict__")?;
        let items = call_method(interp, &proxy, "items", &[])?;
        let namespace = new_dict();
        for item in interp.collect_iterable(&items)? {
            let pair = interp.collect_iterable(&item)?;
            if let (Some(key), Some(value)) = (pair.first(), pair.get(1)) {
                call_method(
                    interp,
                    &namespace,
                    "__setitem__",
                    &[key.clone(), value.clone()],
                )?;
            }
        }
        interp.assign_attr(module.clone(), "__dict__", namespace)?;
    }
    Ok(module)
}

pub(crate) fn module_from_spec_impl(interp: &mut Interpreter, spec: &Value) -> Result<Value> {
    // Typically loaders will not implement `create_module()`.
    let mut module = Value::none();
    let loader = interp.get_attr(spec, "loader")?;
    if has_attr(interp, &loader, "create_module")? {
        // If `create_module()` returns `None` default module creation is used.
        let loader = interp.get_attr(spec, "loader")?;
        module = call_method(interp, &loader, "create_module", std::slice::from_ref(spec))?;
    } else {
        let loader = interp.get_attr(spec, "loader")?;
        if has_attr(interp, &loader, "exec_module")? {
            return Err(PyError::import_error(
                "ImportError",
                "loaders that define exec_module() must also define create_module()",
                None,
            ));
        }
    }
    if module.is_none() {
        let name = interp.get_attr(spec, "name")?;
        module = new_module(interp, spec, &name)?;
    }
    init_module_attrs_impl(interp, spec, &module, false)?;
    Ok(module)
}

// ── Spec lookup ──────────────────────────────────────────────────────────────

/// A spec for a module the runtime loaded natively whose `__spec__` slot is
/// still `None`.  `None` for what looks user-constructed (a bare
/// `types.ModuleType` or `__main__`), so `ValueError` fires as in CPython.
fn synthesize_loaded_module_spec(
    interp: &mut Interpreter,
    fullname: &str,
    module: &Value,
) -> Result<Option<Value>> {
    if fullname == "__main__" {
        return Ok(None);
    }
    if crate::builtin_modules::is_builtin_module(fullname) {
        return builtin_spec(interp, fullname).map(Some);
    }
    let file_attr = get_attr_or_none(interp, module, "__file__")?;
    let pkg_path = get_attr_or_none(interp, module, "__path__")?;
    let name = s(fullname);
    if let Some(file) = file_attr.as_str()
        && !file.is_empty()
    {
        let locations = if pkg_path.is_none() {
            Value::none()
        } else {
            Value::list(interp.collect_iterable(&pkg_path)?)
        };
        return spec_from_file_location_impl(
            interp,
            &name,
            Some(file_attr.clone()),
            Value::none(),
            Some(locations),
        )
        .map(Some);
    }
    if file_attr.is_none() && !pkg_path.is_none() {
        let locations = Value::list(interp.collect_iterable(&pkg_path)?);
        let namespace_loader = machinery_attr(interp, "NamespaceLoader")?;
        // `_NamespacePath` recalculates through `PathFinder._get_spec`.
        let path_finder = machinery_attr(interp, "PathFinder")?;
        let path_finder = interp.get_attr(&path_finder, "_get_spec")?;
        let loader = call(
            interp,
            &namespace_loader,
            &[name.clone(), locations.clone(), path_finder],
        )?;
        let spec = module_spec(interp, &name, &loader, Value::none(), None)?;
        interp.assign_attr(spec.clone(), "submodule_search_locations", locations)?;
        return Ok(Some(spec));
    }
    Ok(None)
}

/// The `__spec__` of an already imported module (`sys.modules[fullname]`).
fn loaded_module_spec(
    interp: &mut Interpreter,
    name: &Value,
    fullname: &Value,
    module: &Value,
    synthesize: bool,
) -> Result<Value> {
    if module.is_none() {
        return Ok(Value::none());
    }
    let spec = match interp.get_attr(module, "__spec__") {
        Ok(spec) => spec,
        Err(error) => {
            let (hit, error) = error_is(interp, error, "AttributeError")?;
            if hit.is_none() {
                return Err(error);
            }
            let name = str_of(interp, name)?;
            let error = value_error(format!("{name}.__spec__ is not set"));
            return Err(from_none(interp, error));
        }
    };
    if !spec.is_none() {
        return Ok(spec);
    }
    if synthesize
        && let Some(text) = fullname.as_str()
        && let Some(spec) = synthesize_loaded_module_spec(interp, text, module)?
    {
        return Ok(spec);
    }
    let name = str_of(interp, name)?;
    Err(value_error(format!("{name}.__spec__ is None")))
}

fn find_spec_private_impl(
    interp: &mut Interpreter,
    name: &Value,
    path: &Value,
    target: &Value,
) -> Result<Value> {
    if !interp.truthy_value(name)? {
        return Ok(Value::none());
    }
    let meta_path = sys_attr(interp, "meta_path")?;
    if meta_path.is_none() {
        return Err(PyError::import_error(
            "ImportError",
            "sys.meta_path is None, Python is likely shutting down",
            None,
        ));
    }
    // We check `sys.modules` here for the reload case.
    let is_reload = in_sys_modules(interp, name)?;
    for finder in interp.collect_iterable(&meta_path)? {
        let Some(find_spec) = get_attr_opt(interp, &finder, "find_spec")? else {
            continue;
        };
        let spec = call(
            interp,
            &find_spec,
            &[name.clone(), path.clone(), target.clone()],
        )?;
        if spec.is_none() {
            continue;
        }
        // The parent import may have already imported this module.
        if !is_reload && in_sys_modules(interp, name)? {
            let module = sys_modules_get(interp, name)?;
            return match get_attr_opt(interp, &module, "__spec__")? {
                None => Ok(spec),
                Some(loaded) if loaded.is_none() => Ok(spec),
                Some(loaded) => Ok(loaded),
            };
        }
        return Ok(spec);
    }

    if let Some(text) = name.as_str()
        && crate::builtin_modules::is_builtin_module(text)
    {
        return builtin_spec(interp, text);
    }
    let path_finder = machinery_attr(interp, "PathFinder")?;
    call_method(
        interp,
        &path_finder,
        "find_spec",
        &[name.clone(), path.clone(), target.clone()],
    )
}

fn find_spec_from_path_impl(interp: &mut Interpreter, name: &Value, path: &Value) -> Result<Value> {
    if !in_sys_modules(interp, name)? {
        return find_spec_private_impl(interp, name, path, &Value::none());
    }
    let module = sys_modules_get(interp, name)?;
    loaded_module_spec(interp, name, name, &module, false)
}

fn find_spec_impl(interp: &mut Interpreter, name: &Value, package: &Value) -> Result<Value> {
    let fullname = if starts_with_dot(interp, name)? {
        resolve_name_impl(interp, name, package)?
    } else {
        name.clone()
    };
    let Some(fullname_text) = fullname.as_str().map(str::to_string) else {
        return Err(type_error("find_spec() module name must be a string"));
    };
    if fullname_text.is_empty() {
        return Ok(Value::none());
    }
    if in_sys_modules(interp, &fullname)? {
        let module = sys_modules_get(interp, &fullname)?;
        return loaded_module_spec(interp, name, &fullname, &module, true);
    }
    let parent_name = fullname_text
        .rsplit_once('.')
        .map_or("", |(parent, _)| parent)
        .to_string();
    if parent_name.is_empty() {
        return find_spec_private_impl(interp, &fullname, &Value::none(), &Value::none());
    }
    let import = Value::builtin_function("__import__");
    let parent = call_kw(
        interp,
        &import,
        &[s(&parent_name)],
        &[("fromlist", Value::list(vec![s("__path__")]))],
    )?;
    // Importing the parent may have populated `fullname` in `sys.modules`.
    if in_sys_modules(interp, &fullname)? {
        let module = sys_modules_get(interp, &fullname)?;
        return loaded_module_spec(interp, name, &fullname, &module, true);
    }
    if crate::builtin_modules::is_builtin_module(&fullname_text) {
        return builtin_spec(interp, &fullname_text);
    }
    let parent_path = match interp.get_attr(&parent, "__path__") {
        Ok(path) => path,
        Err(error) => {
            let (hit, error) = error_is(interp, error, "AttributeError")?;
            let Some(cause) = hit else {
                return Err(error);
            };
            if crate::builtin_modules::is_builtin_package(&parent_name) {
                return Ok(Value::none());
            }
            let repr_parent = repr_of(interp, &s(&parent_name))?;
            let repr_full = repr_of(interp, &fullname)?;
            let error = PyError::import_error(
                "ModuleNotFoundError",
                format!(
                    "__path__ attribute not found on {repr_parent} while trying to find {repr_full}"
                ),
                Some(fullname_text),
            );
            return Err(from_cause(interp, error, cause));
        }
    };
    find_spec_private_impl(interp, &fullname, &parent_path, &Value::none())
}

// ── Lazy loading ─────────────────────────────────────────────────────────────

/// `object.__getattribute__(obj, name)`.
fn object_getattribute(interp: &mut Interpreter, obj: &Value, name: &Value) -> Result<Value> {
    let object = Value::py_class(crate::interpreter::object_class_singleton());
    let raw = interp.get_attr(&object, "__getattribute__")?;
    call(interp, &raw, &[obj.clone(), name.clone()])
}

fn attr_name(attr: &Value) -> Result<&str> {
    attr.as_str().ok_or_else(|| {
        type_error(format!(
            "attribute name must be string, not '{}'",
            crate::interpreter::value_type_name_str(attr)
        ))
    })
}

/// `_LazyModule.__getattribute__`: trigger the load, then return `attr`.
fn lazy_getattribute(interp: &mut Interpreter, module: &Value, attr: &Value) -> Result<Value> {
    let attr_text = attr_name(attr)?.to_string();
    let spec = object_getattribute(interp, module, &s("__spec__"))?;
    let loader_state = if spec.is_none() {
        Value::none()
    } else {
        interp.get_attr(&spec, "loader_state")?
    };
    if loader_state.is_none() {
        // No lazy load is pending: a plain module.
        return object_getattribute(interp, module, attr);
    }
    let lock = call_method(interp, &loader_state, "__getitem__", &[s("lock")])?;
    call_method(interp, &lock, "__enter__", &[])?;
    let loaded = lazy_load_locked(interp, module, &spec, &loader_state, attr);
    let exit = call_method(
        interp,
        &lock,
        "__exit__",
        &[Value::none(), Value::none(), Value::none()],
    );
    let early = loaded?;
    exit?;
    if let Some(early) = early {
        return Ok(early);
    }
    interp.get_attr(module, &attr_text)
}

/// The body of `with loader_state['lock']:`.  `Some(value)` is the early
/// `return` of a re-entrant call.
fn lazy_load_locked(
    interp: &mut Interpreter,
    module: &Value,
    spec: &Value,
    loader_state: &Value,
    attr: &Value,
) -> Result<Option<Value>> {
    // Only the first thread to get the lock should trigger the load and reset
    // the module's class.  The rest can now `getattr()`.
    let lazy_class = util_class(interp, "_LazyModule")?;
    let still_lazy = module
        .as_py_instance_rc()
        .is_some_and(|instance| Rc::ptr_eq(&instance.borrow().class, &lazy_class));
    if !still_lazy {
        return Ok(None);
    }
    // Reentrant calls from the same thread must be allowed to proceed without
    // triggering the load again (`exec_module()` and self-referential imports).
    let is_loading = call_method(interp, loader_state, "__getitem__", &[s("is_loading")])?;
    if interp.truthy_value(&is_loading)? {
        return object_getattribute(interp, module, attr).map(Some);
    }
    call_method(
        interp,
        loader_state,
        "__setitem__",
        &[s("is_loading"), Value::bool_(true)],
    )?;

    let module_dict = object_getattribute(interp, module, &s("__dict__"))?;
    // All module metadata must be gathered from `__spec__` in order to avoid
    // using mutated values.  Get the original name to make sure no object
    // substitution occurred in `sys.modules`.
    let original_name = interp.get_attr(spec, "name")?;
    // Figure out exactly what attributes were mutated between the creation of
    // the module and now.
    let attrs_then = call_method(interp, loader_state, "__getitem__", &[s("__dict__")])?;
    let items = call_method(interp, &module_dict, "items", &[])?;
    let attrs_updated = new_dict();
    for item in interp.collect_iterable(&items)? {
        let pair = interp.collect_iterable(&item)?;
        let (Some(key), Some(value)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        // Code that set an attribute may have kept a reference to the assigned
        // object, making identity more important than equality.
        let known = call_method(
            interp,
            &attrs_then,
            "__contains__",
            std::slice::from_ref(key),
        )?;
        let changed = if interp.truthy_value(&known)? {
            let then = call_method(
                interp,
                &attrs_then,
                "__getitem__",
                std::slice::from_ref(key),
            )?;
            !then.is_identical_to(value)
        } else {
            true
        };
        if changed {
            call_method(
                interp,
                &attrs_updated,
                "__setitem__",
                &[key.clone(), value.clone()],
            )?;
        }
    }
    let loader = interp.get_attr(spec, "loader")?;
    call_method(interp, &loader, "exec_module", std::slice::from_ref(module))?;
    // If `exec_module()` was used directly there is no guarantee the module
    // object was put into `sys.modules`.
    if in_sys_modules(interp, &original_name)? {
        let current = sys_modules_get(interp, &original_name)?;
        if !module.is_identical_to(&current) {
            let repr = repr_of(interp, &original_name)?;
            return Err(value_error(format!(
                "module object for {repr} substituted in sys.modules during a lazy load"
            )));
        }
    }
    // Update after loading since that's what would happen in an eager loading
    // situation.
    call_method(interp, &module_dict, "update", &[attrs_updated])?;
    // Finally, stop triggering this method.
    interp.assign_attr(
        module.clone(),
        "__class__",
        Value::py_class(crate::interpreter::module_type_singleton()),
    )?;
    Ok(None)
}

struct FactoryState {
    cls: Value,
    loader: Value,
}

/// `LazyLoader.__check_eager_loader`.
fn check_eager_loader(interp: &mut Interpreter, loader: &Value) -> Result<()> {
    if has_attr(interp, loader, "exec_module")? {
        Ok(())
    } else {
        Err(type_error("loader must define exec_module()"))
    }
}

/// `-1 if disable_check else 1`, the `override` argument of
/// `_imp._override_multi_interp_extensions_check`.
fn restrictions_override(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let disable = interp.get_attr(this, "disable_check")?;
    Ok(Value::int(if interp.truthy_value(&disable)? {
        -1
    } else {
        1
    }))
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        // `_bootstrap_external.MAGIC_NUMBER`: pyc magic 3531 followed by `\r\n`.
        "MAGIC_NUMBER" => Value::bytes(crate::builtin_modules::py_compile::PYC_MAGIC_NUMBER.to_vec()),
        "_RAW_MAGIC_NUMBER" => Value::int(RAW_MAGIC_NUMBER as i64),
    }

    /// Return the hash of *source_bytes* as used in hash-based pyc files.
    fn source_hash(args) -> Result<Value> {
        let mut b = args::bind(&SIG_SOURCE_HASH, args)?;
        let source_bytes = b.take_or_none(0);
        let bytes = PyBytesLike::try_from_value_with_interp(_interp, &source_bytes, "", "")
            .map_err(|_| {
                type_error(format!(
                    "a bytes-like object is required, not '{}'",
                    crate::interpreter::value_type_name_str(&source_bytes)
                ))
            })?;
        Ok(Value::bytes(
            siphash13(RAW_MAGIC_NUMBER, bytes.as_bytes())
                .to_le_bytes()
                .to_vec(),
        ))
    }

    /// Given the path to a .py file, return the path to its .pyc file.
    ///
    /// The .py file does not need to exist; this simply returns the path to the
    /// .pyc file calculated as if the .py file were imported.
    fn cache_from_source(args) -> Result<Value> {
        let mut b = args::bind(&SIG_CACHE_FROM_SOURCE, args)?;
        let path = b.take_or_none(0);
        let debug_override = b.take_or_none(1);
        let optimization = b.take_or_none(2);
        let result = cache_from_source_impl(_interp, &path, &debug_override, optimization)?;
        Ok(Value::string(result))
    }

    /// Given the path to a .pyc. file, return the path to its .py file.
    ///
    /// The .pyc file does not need to exist; this simply returns the path to
    /// the .py file calculated to correspond to the .pyc file.  If path does
    /// not conform to PEP 3147/488 format, ValueError will be raised. If
    /// sys.implementation.cache_tag is None then NotImplementedError is raised.
    fn source_from_cache(args) -> Result<Value> {
        let mut b = args::bind(&SIG_SOURCE_FROM_CACHE, args)?;
        let path = b.take_or_none(0);
        Ok(Value::string(source_from_cache_impl(_interp, &path)?))
    }

    /// Decode bytes representing source code and return the string.
    ///
    /// Universal newline support is used in the decoding.
    fn decode_source(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DECODE_SOURCE, args)?;
        let source_bytes = b.take_or_none(0);
        decode_source_impl(_interp, &source_bytes)
    }

    /// Resolve a relative module name to an absolute one.
    fn resolve_name(args) -> Result<Value> {
        let mut b = args::bind(&SIG_RESOLVE_NAME, args)?;
        let name = b.take_or_none(0);
        let package = b.take_or_none(1);
        resolve_name_impl(_interp, &name, &package)
    }

    /// Resolve a relative module name to an absolute one.
    fn _resolve_name(args) -> Result<Value> {
        let mut b = args::bind(&SIG_PRIVATE_RESOLVE_NAME, args)?;
        let name = b.take_or_none(0);
        let package = b.take_or_none(1);
        let level = b.take_or_none(2);
        let level = level.as_int().ok_or_else(|| {
            type_error("'level' must be an int")
        })?;
        if level < 1 {
            // `package.rsplit('.', level - 1)` with a negative maxsplit splits
            // everything, so `len(bits) < level` is never true.
            let base = str_of(_interp, &package)?;
            return if _interp.truthy_value(&name)? {
                let name = str_of(_interp, &name)?;
                Ok(Value::string(format!("{base}.{name}")))
            } else {
                Ok(Value::string(base))
            };
        }
        resolve_name_level(_interp, &name, &package, level as usize)
    }

    /// Return a module spec based on various loader methods.
    fn spec_from_loader(args) -> Result<Value> {
        let mut b = args::bind(&SIG_SPEC_FROM_LOADER, args)?;
        let name = b.take_or_none(0);
        let loader = b.take_or_none(1);
        let origin = b.take_or_none(2);
        let is_package = b.take_or_none(3);
        spec_from_loader_impl(_interp, &name, &loader, origin, is_package)
    }

    /// Return a module spec based on a file location.
    ///
    /// To indicate that the module is a package, set
    /// submodule_search_locations to a list of directory paths.  An
    /// empty list is sufficient, though its not otherwise useful to the
    /// import system.
    ///
    /// The loader must take a spec as its only __init__() arg.
    fn spec_from_file_location(args) -> Result<Value> {
        let mut b = args::bind(&SIG_SPEC_FROM_FILE_LOCATION, args)?;
        let name = b.take_or_none(0);
        let location = b.take(1).filter(|value| !value.is_none());
        let loader = b.take_or_none(2);
        let submodule_search_locations = b.take(3);
        spec_from_file_location_impl(_interp, &name, location, loader, submodule_search_locations)
    }

    fn _init_module_attrs(args) -> Result<Value> {
        let mut b = args::bind(&SIG_INIT_MODULE_ATTRS, args)?;
        let spec = b.take_or_none(0);
        let module = b.take_or_none(1);
        let override_ = b.take_or(2, || Value::bool_(false));
        let override_ = _interp.truthy_value(&override_)?;
        init_module_attrs_impl(_interp, &spec, &module, override_)?;
        Ok(module)
    }

    /// Create a module based on the provided spec.
    fn module_from_spec(args) -> Result<Value> {
        let mut b = args::bind(&SIG_MODULE_FROM_SPEC, args)?;
        let spec = b.take_or_none(0);
        module_from_spec_impl(_interp, &spec)
    }

    /// Find a module's spec.
    fn _find_spec(args) -> Result<Value> {
        let mut b = args::bind(&SIG_FIND_SPEC_PRIVATE, args)?;
        let name = b.take_or_none(0);
        let path = b.take_or_none(1);
        let target = b.take_or_none(2);
        find_spec_private_impl(_interp, &name, &path, &target)
    }

    /// Return the spec for the specified module.
    ///
    /// First, sys.modules is checked to see if the module was already imported.
    /// If so, then sys.modules[name].__spec__ is returned. If that happens to be
    /// set to None, then ValueError is raised. If the module is not in
    /// sys.modules, then sys.meta_path is searched for a suitable spec with the
    /// value of 'path' given to the finders. None is returned if no spec could
    /// be found.
    ///
    /// Dotted names do not have their parent packages implicitly imported. You
    /// will most likely need to explicitly import all parent packages in the
    /// proper order for a submodule to get the correct spec.
    fn _find_spec_from_path(args) -> Result<Value> {
        let mut b = args::bind(&SIG_FIND_SPEC_FROM_PATH, args)?;
        let name = b.take_or_none(0);
        let path = b.take_or_none(1);
        find_spec_from_path_impl(_interp, &name, &path)
    }

    /// Return the spec for the specified module.
    ///
    /// First, sys.modules is checked to see if the module was already imported.
    /// If so, then sys.modules[name].__spec__ is returned. If that happens to be
    /// set to None, then ValueError is raised. If the module is not in
    /// sys.modules, then sys.meta_path is searched for a suitable spec with the
    /// value of 'path' given to the finders. None is returned if no spec could
    /// be found.
    ///
    /// If the name is for submodule (contains a dot), the parent module is
    /// automatically imported.
    ///
    /// The name and package arguments work the same as importlib.import_module().
    /// In other words, relative module names (with leading dots) work.
    fn find_spec(args) -> Result<Value> {
        let mut b = args::bind(&SIG_FIND_SPEC, args)?;
        let name = b.take_or_none(0);
        let package = b.take_or_none(1);
        find_spec_impl(_interp, &name, &package)
    }

    /// A subclass of the module type which triggers loading upon attribute access.
    class _LazyModule(crate::interpreter::module_type_singleton()) {
        /// Trigger the load of the module and return the attribute.
        fn __getattribute__(
            #[positional_only] self_val: PyValueRef<'_>,
            attr: PyValue,
        ) -> Result<Value> {
            lazy_getattribute(_interp, self_val.0, &attr.0)
        }

        /// Trigger the load and then perform the deletion.
        fn __delattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            attr: PyValue,
        ) -> Result<Value> {
            // To trigger the load and raise an exception if the attribute
            // doesn't exist.
            lazy_getattribute(_interp, self_val.0, &attr.0)?;
            let name = attr_name(&attr.0)?.to_string();
            _interp.delete_attr(self_val.0.clone(), &name)?;
            Ok(Value::none())
        }
    }

    /// A loader that creates a module which defers loading until attribute access.
    class LazyLoader {
        #[staticmethod]
        fn _LazyLoader__check_eager_loader(loader: PyValue) -> Result<Value> {
            check_eager_loader(_interp, &loader.0)?;
            Ok(Value::none())
        }

        /// Construct a callable which returns the eager loader made lazy.
        #[classmethod]
        fn factory(#[positional_only] cls: PyValueRef<'_>, loader: PyValue) -> Result<Value> {
            check_eager_loader(_interp, &loader.0)?;
            let factory_class = util_class(_interp, "_LazyLoaderFactory")?;
            call(
                _interp,
                &Value::py_class(factory_class),
                &[cls.0.clone(), loader.0],
            )
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            loader: PyValue,
        ) -> Result<Value> {
            check_eager_loader(_interp, &loader.0)?;
            _interp.assign_attr(self_val.0.clone(), "loader", loader.0)?;
            Ok(Value::none())
        }

        fn create_module(
            #[positional_only] self_val: PyValueRef<'_>,
            spec: PyValue,
        ) -> Result<Value> {
            let loader = _interp.get_attr(self_val.0, "loader")?;
            call_method(_interp, &loader, "create_module", &[spec.0])
        }

        /// Make the module load lazily.
        fn exec_module(
            #[positional_only] self_val: PyValueRef<'_>,
            module: PyValue,
        ) -> Result<Value> {
            let module = module.0;
            let loader = _interp.get_attr(self_val.0, "loader")?;
            let spec = _interp.get_attr(&module, "__spec__")?;
            _interp.assign_attr(spec.clone(), "loader", loader.clone())?;
            _interp.assign_attr(module.clone(), "__loader__", loader.clone())?;
            // Don't need to worry about deep-copying as trying to set an
            // attribute on an object would have triggered the load.
            let loader_state = new_dict();
            let module_dict = _interp.get_attr(&module, "__dict__")?;
            let snapshot = call_method(_interp, &module_dict, "copy", &[])?;
            dict_set(&loader_state, "__dict__", snapshot)?;
            let module_class = _interp.get_attr(&module, "__class__")?;
            dict_set(&loader_state, "__class__", module_class)?;
            let threading = _interp.load_module("threading")?;
            let rlock = _interp.get_attr(&threading, "RLock")?;
            let lock = call(_interp, &rlock, &[])?;
            dict_set(&loader_state, "lock", lock)?;
            dict_set(&loader_state, "is_loading", Value::bool_(false))?;
            _interp.assign_attr(spec.clone(), "loader_state", loader_state)?;
            let lazy_class = util_class(_interp, "_LazyModule")?;
            _interp.assign_attr(
                module.clone(),
                "__class__",
                Value::py_class(Rc::clone(&lazy_class)),
            )?;
            let retyped = module
                .as_py_instance_rc()
                .is_some_and(|instance| class_is_subclass_of(&instance.borrow().class, &lazy_class));
            if !retyped {
                // A module object whose class cannot change would stay empty
                // forever: execute eagerly instead.
                _interp.assign_attr(spec, "loader_state", Value::none())?;
                call_method(_interp, &loader, "exec_module", &[module])?;
            }
            Ok(Value::none())
        }
    }

    /// `LazyLoader.factory(loader)`'s callable: `lambda *args, **kwargs:
    /// cls(loader(*args, **kwargs))`.
    #[native_state(FactoryState)]
    class _LazyLoaderFactory {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            cls: PyValue,
            loader: PyValue,
        ) -> Result<Value> {
            set_state(self_val.0, FactoryState { cls: cls.0, loader: loader.0 })?;
            Ok(Value::none())
        }

        fn __call__(
            #[state] state: &FactoryState,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut expanded: Vec<ExpandedCallArg> = args
                .iter()
                .cloned()
                .map(|value| ExpandedCallArg { name: None, value })
                .collect();
            expanded.extend(kwargs.into_iter().map(|(name, value)| ExpandedCallArg {
                name: Some(name),
                value,
            }));
            let eager = _interp.call_function_expanded(state.loader.clone(), &expanded)?;
            call(_interp, &state.cls, &[eager])
        }
    }

    /// A context manager that can temporarily skip the compatibility check.
    ///
    /// NOTE: This function is meant to accommodate an unusual case; one
    /// which is likely to eventually go away.  There's is a pretty good
    /// chance this is not what you were looking for.
    class _incompatible_extension_module_restrictions {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut expanded: Vec<ExpandedCallArg> = std::iter::once(self_val.0.clone())
                .chain(args.iter().cloned())
                .map(|value| ExpandedCallArg { name: None, value })
                .collect();
            expanded.extend(kwargs.into_iter().map(|(name, value)| ExpandedCallArg {
                name: Some(name),
                value,
            }));
            let bound = args::bind(
                &Sig {
                    qualname: "_incompatible_extension_module_restrictions.__init__",
                    params: &["self"],
                    defaults: 0,
                    kwonly: &["disable_check"],
                    var_kw: false,
                },
                &expanded,
            );
            let mut bound = bound?;
            let Some(disable_check) = bound.take(1) else {
                return Err(type_error(
                    "_incompatible_extension_module_restrictions.__init__() missing 1 required keyword-only argument: 'disable_check'",
                ));
            };
            let disable_check = Value::bool_(_interp.truthy_value(&disable_check)?);
            _interp.assign_attr(self_val.0.clone(), "disable_check", disable_check)?;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let override_ = restrictions_override(_interp, self_val.0)?;
            let imp = _interp.load_module("_imp")?;
            let old = call_method(
                _interp,
                &imp,
                "_override_multi_interp_extensions_check",
                &[override_],
            )?;
            _interp.assign_attr(self_val.0.clone(), "old", old)?;
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            let old = _interp.get_attr(self_val.0, "old")?;
            _interp.delete_attr(self_val.0.clone(), "old")?;
            let imp = _interp.load_module("_imp")?;
            call_method(
                _interp,
                &imp,
                "_override_multi_interp_extensions_check",
                &[old],
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn r#override(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            restrictions_override(_interp, self_val.0)
        }
    }
}

/// Wire `Loader` (`importlib.abc.Loader`, `importlib._abc.Loader` in CPython)
/// onto the module and make it the base of `LazyLoader`, with the ABC
/// bookkeeping a `class LazyLoader(Loader)` statement would give it.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let abc = interp.load_module("importlib.abc")?;
    let loader = interp.get_attr(&abc, "Loader")?;
    let ValueKind::PyClass(loader_class) = loader.kind() else {
        return Err(PyError::runtime(
            "importlib.util: importlib.abc.Loader is not a class".to_string(),
        ));
    };
    let lazy_loader = module.borrow().get_attr_value("LazyLoader");
    if let Some(lazy_loader) = lazy_loader
        && let ValueKind::PyClass(lazy_class) = lazy_loader.kind()
    {
        link_class_bases(lazy_class, vec![Rc::clone(loader_class)]);
        let metatype = loader_class.borrow().metatype.clone();
        let mut lazy = lazy_class.borrow_mut();
        lazy.metatype = metatype;
        lazy.attrs.insert(
            "__abstractmethods__".to_string(),
            pyrust_builtins::frozenset::frozenset(crate::value::PySet::default()),
        );
        lazy.attrs.insert(
            "_abc_impl".to_string(),
            crate::builtin_modules::abc::new_abc_impl(),
        );
    }
    module
        .borrow_mut()
        .insert_attr("Loader".to_string(), loader.clone());
    Ok(())
}
