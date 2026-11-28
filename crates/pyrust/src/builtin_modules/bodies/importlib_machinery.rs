// `importlib.machinery` module — 100% native Rust port of CPython 3.12.3's
// `Lib/importlib/machinery.py` together with the `_bootstrap` /
// `_bootstrap_external` classes it re-exports (issues #3189, #3493); there is
// no Python-source injection.
//
// Public API: `SOURCE_SUFFIXES`, `DEBUG_BYTECODE_SUFFIXES`,
// `OPTIMIZED_BYTECODE_SUFFIXES`, `BYTECODE_SUFFIXES`, `EXTENSION_SUFFIXES`,
// `all_suffixes`, `ModuleSpec`, `BuiltinImporter`, `FrozenImporter`,
// `WindowsRegistryFinder`, `PathFinder`, `FileFinder`, `SourceFileLoader`,
// `SourcelessFileLoader`, `ExtensionFileLoader` and `NamespaceLoader`.  The
// classes keep CPython's `__module__` (`_frozen_importlib` /
// `_frozen_importlib_external`), method kinds, MROs (through the internal
// `_LoaderBasics`, `SourceLoader` and `FileLoader` bases) and private hooks
// (`_get_spec`, `_path_hooks`, `_path_importer_cache`, `_fill_cache`,
// `_NamespacePath`, ...); every method dispatches through `self` / `cls`, so
// subclasses and monkeypatching behave as in CPython.
//
// PyRust specifics:
// - `sys.path_hooks` holds no `FileFinder` hook (the runtime imports from the
//   filesystem natively), so `PathFinder._path_importer_cache` falls back to
//   an uncached `FileFinder` for a directory no hook claims.  Caching `None`
//   for such a directory would hide it from the runtime importer.
// - No module is frozen, and `SourceLoader.get_code` always compiles source:
//   bytecode caches are neither read nor written.  `SourcelessFileLoader`
//   validates the pyc header like CPython but cannot execute CPython bytecode,
//   and `ExtensionFileLoader` cannot load extension modules (`ImportError`).
// - `BuiltinImporter` consults `sys.builtin_module_names` (PyRust's
//   `_imp.is_builtin` also reports its native stdlib modules).
//
// Reference: <https://docs.python.org/3.12/library/importlib.html#module-importlib.machinery>

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::importlib_util::{
    self as util, call, call_kw, call_method, error_is, get_attr_opt, has_attr, repr_of, s, str_of,
    sys_attr, type_error,
};
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, lookup_exc_class};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

#[path = "importlib_machinery/docs.rs"]
mod docs;
#[path = "importlib_machinery/finders.rs"]
mod finders;
#[path = "importlib_machinery/loaders.rs"]
mod loaders;

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

const fn sig_kw(
    qualname: &'static str,
    params: &'static [&'static str],
    defaults: usize,
    kwonly: &'static [&'static str],
) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly,
        var_kw: false,
    }
}

const SIG_ALL_SUFFIXES: Sig = sig("all_suffixes", &[], 0);

const SIG_SPEC_INIT: Sig = sig_kw(
    "ModuleSpec.__init__",
    &["self", "name", "loader"],
    0,
    &["origin", "loader_state", "is_package"],
);
const SIG_SPEC_REPR: Sig = sig("ModuleSpec.__repr__", &["self"], 0);
const SIG_SPEC_EQ: Sig = sig("ModuleSpec.__eq__", &["self", "other"], 0);

const SIG_BUILTIN_FIND_SPEC: Sig = sig(
    "BuiltinImporter.find_spec",
    &["cls", "fullname", "path", "target"],
    2,
);
const SIG_BUILTIN_CREATE_MODULE: Sig = sig("BuiltinImporter.create_module", &["spec"], 0);
const SIG_BUILTIN_EXEC_MODULE: Sig = sig("BuiltinImporter.exec_module", &["module"], 0);
const SIG_BUILTIN_GET_CODE: Sig = sig("BuiltinImporter.get_code", &["self", "fullname"], 0);
const SIG_BUILTIN_GET_SOURCE: Sig = sig("BuiltinImporter.get_source", &["self", "fullname"], 0);
const SIG_BUILTIN_IS_PACKAGE: Sig = sig("BuiltinImporter.is_package", &["self", "fullname"], 0);
const SIG_LOAD_MODULE_SHIM: Sig = sig("_load_module_shim", &["self", "fullname"], 0);

const SIG_FROZEN_FIND_SPEC: Sig = sig(
    "FrozenImporter.find_spec",
    &["cls", "fullname", "path", "target"],
    2,
);
const SIG_FROZEN_CREATE_MODULE: Sig = sig("FrozenImporter.create_module", &["spec"], 0);
const SIG_FROZEN_EXEC_MODULE: Sig = sig("FrozenImporter.exec_module", &["module"], 0);
const SIG_FROZEN_GET_CODE: Sig = sig("FrozenImporter.get_code", &["self", "fullname"], 0);
const SIG_FROZEN_GET_SOURCE: Sig = sig("FrozenImporter.get_source", &["self", "fullname"], 0);
const SIG_FROZEN_IS_PACKAGE: Sig = sig("FrozenImporter.is_package", &["self", "fullname"], 0);
const SIG_FROZEN_LOAD_MODULE: Sig = sig("FrozenImporter.load_module", &["cls", "fullname"], 0);
const SIG_FROZEN_RESOLVE_FILENAME: Sig = sig(
    "FrozenImporter._resolve_filename",
    &["cls", "fullname", "alias", "ispkg"],
    2,
);

const SIG_REGISTRY_OPEN: Sig = sig("WindowsRegistryFinder._open_registry", &["key"], 0);
const SIG_REGISTRY_SEARCH: Sig = sig(
    "WindowsRegistryFinder._search_registry",
    &["cls", "fullname"],
    0,
);
const SIG_REGISTRY_FIND_SPEC: Sig = sig(
    "WindowsRegistryFinder.find_spec",
    &["cls", "fullname", "path", "target"],
    2,
);

const SIG_BASICS_IS_PACKAGE: Sig = sig("_LoaderBasics.is_package", &["self", "fullname"], 0);
const SIG_BASICS_CREATE_MODULE: Sig = sig("_LoaderBasics.create_module", &["self", "spec"], 0);
const SIG_BASICS_EXEC_MODULE: Sig = sig("_LoaderBasics.exec_module", &["self", "module"], 0);
const SIG_BASICS_LOAD_MODULE: Sig = sig("_LoaderBasics.load_module", &["self", "fullname"], 0);

const SIG_SOURCE_PATH_MTIME: Sig = sig("SourceLoader.path_mtime", &["self", "path"], 0);
const SIG_SOURCE_PATH_STATS: Sig = sig("SourceLoader.path_stats", &["self", "path"], 0);
const SIG_SOURCE_CACHE_BYTECODE: Sig = sig(
    "SourceLoader._cache_bytecode",
    &["self", "source_path", "cache_path", "data"],
    0,
);
const SIG_SOURCE_SET_DATA: Sig = sig("SourceLoader.set_data", &["self", "path", "data"], 0);
const SIG_SOURCE_GET_SOURCE: Sig = sig("SourceLoader.get_source", &["self", "fullname"], 0);
const SIG_SOURCE_TO_CODE: Sig = sig_kw(
    "SourceLoader.source_to_code",
    &["self", "data", "path"],
    0,
    &["_optimize"],
);
const SIG_SOURCE_GET_CODE: Sig = sig("SourceLoader.get_code", &["self", "fullname"], 0);

const SIG_FILE_INIT: Sig = sig("FileLoader.__init__", &["self", "fullname", "path"], 0);
const SIG_FILE_EQ: Sig = sig("FileLoader.__eq__", &["self", "other"], 0);
const SIG_FILE_HASH: Sig = sig("FileLoader.__hash__", &["self"], 0);
const SIG_FILE_GET_DATA: Sig = sig("FileLoader.get_data", &["self", "path"], 0);

const SIG_SFL_PATH_STATS: Sig = sig("SourceFileLoader.path_stats", &["self", "path"], 0);
const SIG_SFL_CACHE_BYTECODE: Sig = sig(
    "SourceFileLoader._cache_bytecode",
    &["self", "source_path", "bytecode_path", "data"],
    0,
);
const SIG_SFL_SET_DATA: Sig = sig_kw(
    "SourceFileLoader.set_data",
    &["self", "path", "data"],
    0,
    &["_mode"],
);

const SIG_SOURCELESS_GET_CODE: Sig = sig("SourcelessFileLoader.get_code", &["self", "fullname"], 0);
const SIG_SOURCELESS_GET_SOURCE: Sig =
    sig("SourcelessFileLoader.get_source", &["self", "fullname"], 0);

const SIG_EXT_INIT: Sig = sig("ExtensionFileLoader.__init__", &["self", "name", "path"], 0);
const SIG_EXT_EQ: Sig = sig("ExtensionFileLoader.__eq__", &["self", "other"], 0);
const SIG_EXT_HASH: Sig = sig("ExtensionFileLoader.__hash__", &["self"], 0);
const SIG_EXT_CREATE_MODULE: Sig = sig("ExtensionFileLoader.create_module", &["self", "spec"], 0);
const SIG_EXT_EXEC_MODULE: Sig = sig("ExtensionFileLoader.exec_module", &["self", "module"], 0);
const SIG_EXT_IS_PACKAGE: Sig = sig("ExtensionFileLoader.is_package", &["self", "fullname"], 0);
const SIG_EXT_GET_CODE: Sig = sig("ExtensionFileLoader.get_code", &["self", "fullname"], 0);
const SIG_EXT_GET_SOURCE: Sig = sig("ExtensionFileLoader.get_source", &["self", "fullname"], 0);

const SIG_NSPATH_INIT: Sig = sig(
    "_NamespacePath.__init__",
    &["self", "name", "path", "path_finder"],
    0,
);
const SIG_NSPATH_SELF: Sig = sig("_NamespacePath._recalculate", &["self"], 0);
const SIG_NSPATH_PARENT_NAMES: Sig = sig("_NamespacePath._find_parent_path_names", &["self"], 0);
const SIG_NSPATH_PARENT_PATH: Sig = sig("_NamespacePath._get_parent_path", &["self"], 0);
const SIG_NSPATH_ITER: Sig = sig("_NamespacePath.__iter__", &["self"], 0);
const SIG_NSPATH_GETITEM: Sig = sig("_NamespacePath.__getitem__", &["self", "index"], 0);
const SIG_NSPATH_SETITEM: Sig = sig("_NamespacePath.__setitem__", &["self", "index", "path"], 0);
const SIG_NSPATH_LEN: Sig = sig("_NamespacePath.__len__", &["self"], 0);
const SIG_NSPATH_REPR: Sig = sig("_NamespacePath.__repr__", &["self"], 0);
const SIG_NSPATH_CONTAINS: Sig = sig("_NamespacePath.__contains__", &["self", "item"], 0);
const SIG_NSPATH_APPEND: Sig = sig("_NamespacePath.append", &["self", "item"], 0);

const SIG_NSLOADER_INIT: Sig = sig(
    "NamespaceLoader.__init__",
    &["self", "name", "path", "path_finder"],
    0,
);
const SIG_NSLOADER_IS_PACKAGE: Sig = sig("NamespaceLoader.is_package", &["self", "fullname"], 0);
const SIG_NSLOADER_GET_SOURCE: Sig = sig("NamespaceLoader.get_source", &["self", "fullname"], 0);
const SIG_NSLOADER_GET_CODE: Sig = sig("NamespaceLoader.get_code", &["self", "fullname"], 0);
const SIG_NSLOADER_CREATE_MODULE: Sig = sig("NamespaceLoader.create_module", &["self", "spec"], 0);
const SIG_NSLOADER_EXEC_MODULE: Sig = sig("NamespaceLoader.exec_module", &["self", "module"], 0);
const SIG_NSLOADER_LOAD_MODULE: Sig = sig("NamespaceLoader.load_module", &["self", "fullname"], 0);
const SIG_NSLOADER_RESOURCE_READER: Sig = sig(
    "NamespaceLoader.get_resource_reader",
    &["self", "module"],
    0,
);

const SIG_PATH_INVALIDATE: Sig = sig("PathFinder.invalidate_caches", &[], 0);
const SIG_PATH_HOOKS: Sig = sig("PathFinder._path_hooks", &["path"], 0);
const SIG_PATH_IMPORTER_CACHE: Sig = sig("PathFinder._path_importer_cache", &["cls", "path"], 0);
const SIG_PATH_GET_SPEC: Sig = sig(
    "PathFinder._get_spec",
    &["cls", "fullname", "path", "target"],
    1,
);
const SIG_PATH_FIND_SPEC: Sig = sig(
    "PathFinder.find_spec",
    &["cls", "fullname", "path", "target"],
    2,
);

const SIG_FF_INVALIDATE: Sig = sig("FileFinder.invalidate_caches", &["self"], 0);
const SIG_FF_GET_SPEC: Sig = sig(
    "FileFinder._get_spec",
    &["self", "loader_class", "fullname", "path", "smsl", "target"],
    0,
);
const SIG_FF_FIND_SPEC: Sig = sig("FileFinder.find_spec", &["self", "fullname", "target"], 1);
const SIG_FF_FILL_CACHE: Sig = sig("FileFinder._fill_cache", &["self"], 0);
const SIG_FF_REPR: Sig = sig("FileFinder.__repr__", &["self"], 0);
const SIG_PATH_HOOK_CALL: Sig = sig(
    "FileFinder.path_hook.<locals>.path_hook_for_FileFinder",
    &["path"],
    0,
);

/// `_bootstrap_external.SOURCE_SUFFIXES`.
fn source_suffixes() -> Value {
    #[cfg(windows)]
    let suffixes = vec![s(".py"), s(".pyw")];
    #[cfg(not(windows))]
    let suffixes = vec![s(".py")];
    Value::list(suffixes)
}

/// `_imp.extension_suffixes()`, CPython 3.12's list for this target.
pub(crate) fn extension_suffix_names() -> &'static [&'static str] {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        &[".cp312-win_amd64.pyd", ".pyd"]
    }
    #[cfg(all(windows, target_arch = "aarch64"))]
    {
        &[".cp312-win_arm64.pyd", ".pyd"]
    }
    #[cfg(all(windows, target_arch = "x86"))]
    {
        &[".cp312-win32.pyd", ".pyd"]
    }
    #[cfg(all(
        windows,
        not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86"))
    ))]
    {
        &[".pyd"]
    }
    #[cfg(target_os = "macos")]
    {
        &[".cpython-312-darwin.so", ".abi3.so", ".so"]
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    {
        &[".cpython-312-x86_64-linux-gnu.so", ".abi3.so", ".so"]
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
    {
        &[".cpython-312-aarch64-linux-gnu.so", ".abi3.so", ".so"]
    }
    #[cfg(all(
        unix,
        not(target_os = "macos"),
        not(all(
            target_os = "linux",
            target_env = "gnu",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))
    ))]
    {
        &[".abi3.so", ".so"]
    }
    #[cfg(not(any(windows, unix)))]
    {
        &[".so"]
    }
}

fn extension_suffixes() -> Value {
    Value::list(extension_suffix_names().iter().map(|x| s(x)).collect())
}

// ── Hidden classes ───────────────────────────────────────────────────────────

/// Classes `_bootstrap_external` defines but `importlib.machinery` does not
/// re-export: kept off the module namespace, published on
/// `_frozen_importlib_external`.
pub(crate) const HIDDEN_CLASS_NAMES: &[&str] = &[
    "_LoaderBasics",
    "SourceLoader",
    "FileLoader",
    "_NamespacePath",
    "_FileFinderPathHook",
];

// `gil_local!` justification:
// (a) Why: the hidden classes are `!Send` `Rc` handles created with the
//     module and shared by every Python thread of one interpreter under the
//     GIL.
// (b) Isolation: independent interpreters on separate OS threads each get
//     their own table, so classes never leak between them.
pyrust_core::gil_local! {
    static HIDDEN_CLASSES: RefCell<HashMap<&'static str, Value>> = RefCell::new(HashMap::new());
}

/// One of [`HIDDEN_CLASS_NAMES`], creating `importlib.machinery` on first use.
pub(crate) fn hidden_class(interp: &mut Interpreter, name: &str) -> Result<Value> {
    if let Some(class) = HIDDEN_CLASSES.with(|table| table.borrow().get(name).cloned()) {
        return Ok(class);
    }
    interp.load_module("importlib.machinery")?;
    HIDDEN_CLASSES
        .with(|table| table.borrow().get(name).cloned())
        .ok_or_else(|| PyError::runtime(format!("importlib.machinery: {name} is missing")))
}

// ── Small interpreter helpers ────────────────────────────────────────────────

fn get(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<Value> {
    interp.get_attr(obj, name)
}

fn set(interp: &mut Interpreter, obj: &Value, name: &str, value: Value) -> Result<()> {
    interp.assign_attr(obj.clone(), name, value)
}

fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

fn eq(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    interp.eval_binary(a.clone(), BinaryOp::Eq, b.clone())
}

fn ne(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    let result = interp.eval_binary(a.clone(), BinaryOp::Ne, b.clone())?;
    interp.truthy_value(&result)
}

/// `item in container`.
fn contains(interp: &mut Interpreter, container: &Value, item: &Value) -> Result<bool> {
    let result = interp.eval_binary(item.clone(), BinaryOp::In, container.clone())?;
    interp.truthy_value(&result)
}

fn builtin(name: &'static str) -> Value {
    Value::builtin_function(name)
}

fn machinery_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let machinery = interp.load_module("importlib.machinery")?;
    interp.get_attr(&machinery, name)
}

fn module_attr(interp: &mut Interpreter, module: &str, name: &str) -> Result<Value> {
    let module = interp.load_module(module)?;
    interp.get_attr(&module, name)
}

fn exc_class(name: &str) -> Result<Value> {
    lookup_exc_class(name)
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime(format!("importlib.machinery: {name} is missing")))
}

/// `raise <class_name>(*args, name=..., path=...)` for an `ImportError`.
fn import_error(
    interp: &mut Interpreter,
    message: impl Into<String>,
    name: Option<Value>,
    path: Option<Value>,
) -> PyError {
    let message = message.into();
    let class = match exc_class("ImportError") {
        Ok(class) => class,
        Err(error) => return error,
    };
    let mut keywords = Vec::new();
    if let Some(name) = name {
        keywords.push(("name", name));
    }
    if let Some(path) = path {
        keywords.push(("path", path));
    }
    match call_kw(interp, &class, &[Value::string(message)], &keywords) {
        Ok(exception) => PyError::raised(exception),
        Err(error) => error,
    }
}

/// `raise <class_name>(*args)`.
fn raise_class(interp: &mut Interpreter, class_name: &str, args: &[Value]) -> PyError {
    match exc_class(class_name).and_then(|class| call(interp, &class, args)) {
        Ok(exception) => PyError::raised(exception),
        Err(error) => error,
    }
}

/// `try: f() except (<class_names>) as exc: ...` — `Ok(Err(exc))` when `f`
/// raised one of those classes.
///
/// A handled exception's traceback frames are dropped from the captured
/// chain, so they cannot leak into the next unrelated traceback.
fn attempt_any(
    interp: &mut Interpreter,
    class_names: &[&str],
    f: impl FnOnce(&mut Interpreter) -> Result<Value>,
) -> Result<std::result::Result<Value, Value>> {
    let saved = (pyrust_core::captured_error_frames_len() > 0)
        .then(pyrust_core::clone_captured_error_frames);
    let mut error = match f(interp) {
        Ok(value) => return Ok(Ok(value)),
        Err(error) => error,
    };
    for class_name in class_names {
        let (hit, rest) = error_is(interp, error, class_name)?;
        if let Some(exception) = hit {
            pyrust_core::reset_captured_error_frames();
            for frame in saved.into_iter().flatten().rev() {
                pyrust_core::record_traceback_frame(frame);
            }
            return Ok(Err(exception));
        }
        error = rest;
    }
    Err(error)
}

/// `try: f() except <class_name>: ...` — `Ok(None)` when `f` raised that class.
fn attempt(
    interp: &mut Interpreter,
    class_name: &str,
    f: impl FnOnce(&mut Interpreter) -> Result<Value>,
) -> Result<Option<Value>> {
    Ok(attempt_any(interp, &[class_name], f)?.ok())
}

/// `str.endswith(tuple(suffixes))`.
fn ends_with_any(interp: &mut Interpreter, text: &Value, suffixes: &Value) -> Result<bool> {
    let suffixes = interp.collect_iterable(suffixes)?;
    if let Some(text) = text.as_str()
        && let Some(found) = suffixes
            .iter()
            .map(|suffix| suffix.as_str().map(|x| text.ends_with(x)))
            .collect::<Option<Vec<bool>>>()
    {
        return Ok(found.into_iter().any(|x| x));
    }
    let result = call_method(interp, text, "endswith", &[Value::tuple(suffixes)])?;
    interp.truthy_value(&result)
}

/// `name.rpartition('.')`, as `(head, tail)`.
fn rpartition_dot(interp: &mut Interpreter, name: &Value) -> Result<(Value, Value)> {
    if let Some(text) = name.as_str() {
        return Ok(match text.rsplit_once('.') {
            Some((head, tail)) => (s(head), s(tail)),
            None => (s(""), s(text)),
        });
    }
    let parts = call_method(interp, name, "rpartition", &[s(".")])?;
    let parts = interp.collect_iterable(&parts)?;
    match parts.as_slice() {
        [head, _, tail] => Ok((head.clone(), tail.clone())),
        _ => Err(PyError::runtime(
            "importlib.machinery: rpartition() did not return 3 items".to_string(),
        )),
    }
}

fn new_dict() -> Value {
    Value::dict(PyDict::default())
}

fn dict_set(dict: &Value, key: &str, value: Value) -> Result<()> {
    dict.dict_insert(PyKey::str_from(key), value)?;
    Ok(())
}

/// The positional / keyword arguments of a legacy `(args)` call.
fn split_args(args: &[ExpandedCallArg]) -> (Vec<Value>, Vec<(String, Value)>) {
    let mut positional = Vec::new();
    let mut keywords = Vec::new();
    for arg in args {
        match &arg.name {
            None => positional.push(arg.value.clone()),
            Some(name) => keywords.push((name.clone(), arg.value.clone())),
        }
    }
    (positional, keywords)
}

/// `NamespaceLoader.__new__(NamespaceLoader)` with `loader._path = path`, the
/// loader `_bootstrap._init_module_attrs` builds for a spec without one.
pub(crate) fn bare_namespace_loader(interp: &mut Interpreter, path: Value) -> Result<Value> {
    let class = machinery_attr(interp, "NamespaceLoader")?;
    let object = Value::py_class(crate::interpreter::object_class_singleton());
    let new = interp.get_attr(&object, "__new__")?;
    let loader = call(interp, &new, &[class])?;
    set(interp, &loader, "_path", path)?;
    Ok(loader)
}

/// Classes whose CPython home is `_frozen_importlib` (`_bootstrap`); every
/// other class comes from `_frozen_importlib_external`.
const BOOTSTRAP_CLASSES: &[&str] = &["ModuleSpec", "BuiltinImporter", "FrozenImporter"];

/// `vars(importlib.machinery)` order of CPython 3.12.3's `machinery.py`.
const NAMESPACE_ORDER: &[&str] = &[
    "ModuleSpec",
    "BuiltinImporter",
    "FrozenImporter",
    "SOURCE_SUFFIXES",
    "DEBUG_BYTECODE_SUFFIXES",
    "OPTIMIZED_BYTECODE_SUFFIXES",
    "BYTECODE_SUFFIXES",
    "EXTENSION_SUFFIXES",
    "WindowsRegistryFinder",
    "PathFinder",
    "FileFinder",
    "SourceFileLoader",
    "SourcelessFileLoader",
    "ExtensionFileLoader",
    "NamespaceLoader",
    "all_suffixes",
];

/// Give the classes CPython's `__module__`, move the `_bootstrap_external`
/// internals off the namespace, share the bytecode suffix list and restore
/// `machinery.py`'s namespace order.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let _ = interp;
    let mut module = module.borrow_mut();
    // `DEBUG_BYTECODE_SUFFIXES = OPTIMIZED_BYTECODE_SUFFIXES = BYTECODE_SUFFIXES`.
    if let Some(bytecode) = module.get_attr_value("BYTECODE_SUFFIXES") {
        for alias in ["DEBUG_BYTECODE_SUFFIXES", "OPTIMIZED_BYTECODE_SUFFIXES"] {
            module.insert_attr(alias.to_string(), bytecode.clone());
        }
    }
    let class_names: Vec<String> = module
        .attrs
        .iter()
        .filter(|(_, value)| matches!(value.kind(), ValueKind::PyClass(_)))
        .map(|(name, _)| name.clone())
        .collect();
    for name in class_names {
        let Some(value) = module.get_attr_value(&name) else {
            continue;
        };
        if let ValueKind::PyClass(class) = value.kind() {
            let home = if BOOTSTRAP_CLASSES.contains(&name.as_str()) {
                "_frozen_importlib"
            } else {
                "_frozen_importlib_external"
            };
            class
                .borrow_mut()
                .attrs
                .insert("__module__".to_string(), Value::string(home));
            // `@classmethod` in `_bootstrap*.py`: a plain `classmethod` object
            // (with `__func__`) rather than the native `classmethod_descriptor`
            // the class macro installs.
            let class_methods: Vec<(String, Value)> = class
                .borrow()
                .attrs
                .iter()
                .filter_map(|(name, value)| {
                    pyrust_builtins::classmethod::with_class_method_any(value, |state| {
                        state.wrapped.clone()
                    })
                    .map(|wrapped| (name.clone(), wrapped))
                })
                .collect();
            for (name, wrapped) in class_methods {
                class.borrow_mut().attrs.insert(
                    name,
                    pyrust_builtins::classmethod::class_method_any(wrapped),
                );
            }
        }
    }
    for name in HIDDEN_CLASS_NAMES {
        if let Some(class) = module.attrs.shift_remove(*name) {
            HIDDEN_CLASSES.with(|table| table.borrow_mut().insert(name, class));
        }
    }
    for name in NAMESPACE_ORDER {
        if let Some(value) = module.attrs.shift_remove(*name) {
            module.attrs.insert((*name).to_string(), value);
        }
    }
    Ok(())
}

// ── `_check_name` ────────────────────────────────────────────────────────────

/// The arguments of `_check_name_wrapper(self, name=None)` (the methods it
/// wraps take no further arguments).
fn bind_check_name(qualname: &str, args: &[ExpandedCallArg]) -> Result<(Value, Option<Value>)> {
    let (positional, keywords) = split_args(args);
    if positional.len() > 2 {
        return Err(type_error(format!(
            "{qualname}() takes 2 positional arguments but {} were given",
            positional.len()
        )));
    }
    let mut iter = positional.into_iter();
    let Some(this) = iter.next() else {
        return Err(type_error(
            "_check_name_wrapper() missing 1 required positional argument: 'self'",
        ));
    };
    let mut name = iter.next();
    for (keyword, value) in keywords {
        if keyword != "name" {
            return Err(type_error(format!(
                "{qualname}() got an unexpected keyword argument '{keyword}'"
            )));
        }
        if name.is_some() {
            return Err(type_error(
                "_check_name_wrapper() got multiple values for argument 'name'",
            ));
        }
        name = Some(value);
    }
    Ok((this, name))
}

/// `_check_name`: `name` defaults to `self.name` and must match it.
fn check_name(interp: &mut Interpreter, this: &Value, name: Option<Value>) -> Result<Value> {
    let own = get(interp, this, "name")?;
    let Some(name) = name.filter(|name| !name.is_none()) else {
        return Ok(own);
    };
    if ne(interp, &own, &name)? {
        let own_text = str_of(interp, &own)?;
        let name_text = str_of(interp, &name)?;
        return Err(import_error(
            interp,
            format!("loader for {own_text} cannot handle {name_text}"),
            Some(name),
            None,
        ));
    }
    Ok(name)
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "SOURCE_SUFFIXES" => source_suffixes(),
        "BYTECODE_SUFFIXES" => Value::list(vec![s(".pyc")]),
        "DEBUG_BYTECODE_SUFFIXES" => Value::none(),
        "OPTIMIZED_BYTECODE_SUFFIXES" => Value::none(),
        "EXTENSION_SUFFIXES" => extension_suffixes(),
    }

    /// Returns a list of all recognized module suffixes for this process
    fn all_suffixes(args) -> Result<Value> {
        args::bind(&SIG_ALL_SUFFIXES, args)?;
        let source = machinery_attr(_interp, "SOURCE_SUFFIXES")?;
        let bytecode = machinery_attr(_interp, "BYTECODE_SUFFIXES")?;
        let extension = machinery_attr(_interp, "EXTENSION_SUFFIXES")?;
        let partial = _interp.eval_binary(source, BinaryOp::Add, bytecode)?;
        _interp.eval_binary(partial, BinaryOp::Add, extension)
    }

    class ModuleSpec {
        constants {
            "__doc__" => Value::string(docs::MODULE_SPEC_DOC),
            "__hash__" => Value::none(),
        }

        fn __init__(args) -> Result<Value> {
            loaders::spec_init(_interp, args)?;
            Ok(Value::none())
        }

        fn __repr__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SPEC_REPR, args)?;
            let this = b.take_or_none(0);
            loaders::spec_repr(_interp, &this)
        }

        fn __eq__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SPEC_EQ, args)?;
            let this = b.take_or_none(0);
            let other = b.take_or_none(1);
            loaders::spec_eq(_interp, &this, &other)
        }

        #[getter]
        fn cached(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            loaders::spec_cached(_interp, self_val.0)
        }

        #[setter("cached")]
        fn set_cached(#[positional_only] self_val: PyValueRef<'_>, cached: PyValue) -> Result<Value> {
            set(_interp, self_val.0, "_cached", cached.0)?;
            Ok(Value::none())
        }

        /// The name of the module's parent.
        #[getter]
        fn parent(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            loaders::spec_parent(_interp, self_val.0)
        }

        #[getter]
        fn has_location(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            get(_interp, self_val.0, "_set_fileattr")
        }

        #[setter("has_location")]
        fn set_has_location(
            #[positional_only] self_val: PyValueRef<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let flag = Value::bool_(_interp.truthy_value(&value.0)?);
            set(_interp, self_val.0, "_set_fileattr", flag)?;
            Ok(Value::none())
        }
    }

    class BuiltinImporter {
        constants {
            "__doc__" => Value::string(docs::BUILTIN_IMPORTER_DOC),
            "_ORIGIN" => Value::string("built-in"),
        }

        #[classmethod]
        fn find_spec(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BUILTIN_FIND_SPEC, args)?;
            let cls = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::builtin_find_spec(_interp, &cls, &fullname)
        }

        /// Create a built-in module
        #[staticmethod]
        fn create_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BUILTIN_CREATE_MODULE, args)?;
            let spec = b.take_or_none(0);
            loaders::builtin_create_module(_interp, &spec)
        }

        /// Exec a built-in module
        #[staticmethod]
        fn exec_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BUILTIN_EXEC_MODULE, args)?;
            let module = b.take_or_none(0);
            let imp = _interp.load_module("_imp")?;
            call_method(_interp, &imp, "exec_builtin", &[module])?;
            Ok(Value::none())
        }

        /// Return None as built-in modules do not have code objects.
        #[classmethod]
        fn get_code(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BUILTIN_GET_CODE, args)?;
            let fullname = b.take_or_none(1);
            loaders::requires_builtin(_interp, &fullname)?;
            Ok(Value::none())
        }

        /// Return None as built-in modules do not have source code.
        #[classmethod]
        fn get_source(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BUILTIN_GET_SOURCE, args)?;
            let fullname = b.take_or_none(1);
            loaders::requires_builtin(_interp, &fullname)?;
            Ok(Value::none())
        }

        /// Return False as built-in modules are never packages.
        #[classmethod]
        fn is_package(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BUILTIN_IS_PACKAGE, args)?;
            let fullname = b.take_or_none(1);
            loaders::requires_builtin(_interp, &fullname)?;
            Ok(Value::bool_(false))
        }

        /// Load the specified module into sys.modules and return it.
        ///
        /// This method is deprecated.  Use loader.exec_module() instead.
        #[classmethod]
        fn load_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_LOAD_MODULE_SHIM, args)?;
            let cls = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::load_module_shim(_interp, &cls, &fullname)
        }
    }

    class FrozenImporter {
        constants {
            "__doc__" => Value::string(docs::FROZEN_IMPORTER_DOC),
            "_ORIGIN" => Value::string("frozen"),
            #[cfg(windows)]
            "_SEP" => Value::string("\\"),
            #[cfg(not(windows))]
            "_SEP" => Value::string("/"),
        }

        #[classmethod]
        fn _resolve_filename(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FROZEN_RESOLVE_FILENAME, args)?;
            let cls = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            let alias = b.take_or_none(2);
            let ispkg = b.take_or(3, || Value::bool_(false));
            loaders::frozen_resolve_filename(_interp, &cls, &fullname, &alias, &ispkg)
        }

        #[classmethod]
        fn find_spec(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FROZEN_FIND_SPEC, args)?;
            let cls = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::frozen_find_spec(_interp, &cls, &fullname)
        }

        /// Set __file__, if able.
        #[staticmethod]
        fn create_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FROZEN_CREATE_MODULE, args)?;
            let spec = b.take_or_none(0);
            loaders::frozen_create_module(_interp, &spec)
        }

        #[staticmethod]
        fn exec_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FROZEN_EXEC_MODULE, args)?;
            let module = b.take_or_none(0);
            let spec = get(_interp, &module, "__spec__")?;
            let name = get(_interp, &spec, "name")?;
            Err(loaders::no_frozen_object(_interp, name))
        }

        /// Load a frozen module.
        ///
        /// This method is deprecated.  Use exec_module() instead.
        #[classmethod]
        fn load_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FROZEN_LOAD_MODULE, args)?;
            let cls = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::load_module_shim(_interp, &cls, &fullname)
        }

        /// Return the code object for the frozen module.
        #[classmethod]
        fn get_code(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FROZEN_GET_CODE, args)?;
            let fullname = b.take_or_none(1);
            loaders::requires_frozen(_interp, &fullname)?;
            Err(loaders::no_frozen_object(_interp, fullname))
        }

        /// Return None as frozen modules do not have source code.
        #[classmethod]
        fn get_source(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FROZEN_GET_SOURCE, args)?;
            let fullname = b.take_or_none(1);
            loaders::requires_frozen(_interp, &fullname)?;
            Ok(Value::none())
        }

        /// Return True if the frozen module is a package.
        #[classmethod]
        fn is_package(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FROZEN_IS_PACKAGE, args)?;
            let fullname = b.take_or_none(1);
            loaders::requires_frozen(_interp, &fullname)?;
            let imp = _interp.load_module("_imp")?;
            call_method(_interp, &imp, "is_frozen_package", &[fullname])
        }
    }

    class WindowsRegistryFinder {
        constants {
            "__doc__" => Value::string(docs::WINDOWS_REGISTRY_FINDER_DOC),
            "REGISTRY_KEY" => Value::string(
                "Software\\Python\\PythonCore\\{sys_version}\\Modules\\{fullname}",
            ),
            "REGISTRY_KEY_DEBUG" => Value::string(
                "Software\\Python\\PythonCore\\{sys_version}\\Modules\\{fullname}\\Debug",
            ),
            "DEBUG_BUILD" => Value::bool_(false),
        }

        #[staticmethod]
        fn _open_registry(args) -> Result<Value> {
            let mut b = args::bind(&SIG_REGISTRY_OPEN, args)?;
            let key = b.take_or_none(0);
            finders::registry_open(_interp, &key)
        }

        #[classmethod]
        fn _search_registry(args) -> Result<Value> {
            let mut b = args::bind(&SIG_REGISTRY_SEARCH, args)?;
            let cls = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            finders::registry_search(_interp, &cls, &fullname)
        }

        #[classmethod]
        fn find_spec(args) -> Result<Value> {
            let mut b = args::bind(&SIG_REGISTRY_FIND_SPEC, args)?;
            let cls = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            finders::registry_find_spec(_interp, &cls, &fullname)
        }
    }

    class _LoaderBasics {
        constants {
            "__doc__" => Value::string(docs::LOADER_BASICS_DOC),
        }

        /// Concrete implementation of InspectLoader.is_package by checking if
        /// the path returned by get_filename has a filename of '__init__.py'.
        fn is_package(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASICS_IS_PACKAGE, args)?;
            let this = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::basics_is_package(_interp, &this, &fullname)
        }

        /// Use default semantics for module creation.
        fn create_module(args) -> Result<Value> {
            args::bind(&SIG_BASICS_CREATE_MODULE, args)?;
            Ok(Value::none())
        }

        /// Execute the module.
        fn exec_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASICS_EXEC_MODULE, args)?;
            let this = b.take_or_none(0);
            let module = b.take_or_none(1);
            loaders::basics_exec_module(_interp, &this, &module)?;
            Ok(Value::none())
        }

        /// This method is deprecated.
        fn load_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASICS_LOAD_MODULE, args)?;
            let this = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::load_module_shim(_interp, &this, &fullname)
        }
    }

    class SourceLoader("_LoaderBasics") {
        constants {
            "__doc__" => Value::none(),
        }

        /// Optional method that returns the modification time (an int) for the
        /// specified path (a str).
        ///
        /// Raises OSError when the path cannot be handled.
        fn path_mtime(args) -> Result<Value> {
            args::bind(&SIG_SOURCE_PATH_MTIME, args)?;
            Err(raise_class(_interp, "OSError", &[]))
        }

        /// Optional method returning a metadata dict for the specified
        /// path (a str).
        fn path_stats(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SOURCE_PATH_STATS, args)?;
            let this = b.take_or_none(0);
            let path = b.take_or_none(1);
            let mtime = call_method(_interp, &this, "path_mtime", &[path])?;
            let stats = new_dict();
            dict_set(&stats, "mtime", mtime)?;
            Ok(stats)
        }

        /// Optional method which writes data (bytes) to a file path (a str).
        ///
        /// Implementing this method allows for the writing of bytecode files.
        ///
        /// The source path is needed in order to correctly transfer permissions
        fn _cache_bytecode(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SOURCE_CACHE_BYTECODE, args)?;
            let this = b.take_or_none(0);
            let cache_path = b.take_or_none(2);
            let data = b.take_or_none(3);
            call_method(_interp, &this, "set_data", &[cache_path, data])
        }

        /// Optional method which writes data (bytes) to a file path (a str).
        ///
        /// Implementing this method allows for the writing of bytecode files.
        fn set_data(args) -> Result<Value> {
            args::bind(&SIG_SOURCE_SET_DATA, args)?;
            Ok(Value::none())
        }

        /// Concrete implementation of InspectLoader.get_source.
        fn get_source(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SOURCE_GET_SOURCE, args)?;
            let this = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::source_get_source(_interp, &this, &fullname)
        }

        /// Return the code object compiled from source.
        ///
        /// The 'data' argument can be any object type that compile() supports.
        fn source_to_code(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SOURCE_TO_CODE, args)?;
            let data = b.take_or_none(1);
            let path = b.take_or_none(2);
            let optimize = b.take_or(3, || Value::int(-1));
            loaders::source_to_code(_interp, data, path, optimize)
        }

        /// Concrete implementation of InspectLoader.get_code.
        fn get_code(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SOURCE_GET_CODE, args)?;
            let this = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::source_get_code(_interp, &this, &fullname)
        }
    }

    class FileLoader {
        constants {
            "__doc__" => Value::string(docs::FILE_LOADER_DOC),
        }

        /// Cache the module name and the path to the file found by the
        /// finder.
        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FILE_INIT, args)?;
            let this = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            let path = b.take_or_none(2);
            set(_interp, &this, "name", fullname)?;
            set(_interp, &this, "path", path)?;
            Ok(Value::none())
        }

        fn __eq__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FILE_EQ, args)?;
            let this = b.take_or_none(0);
            let other = b.take_or_none(1);
            loaders::file_loader_eq(_interp, &this, &other)
        }

        fn __hash__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FILE_HASH, args)?;
            let this = b.take_or_none(0);
            loaders::file_loader_hash(_interp, &this)
        }

        /// Load a module from a file.
        ///
        /// This method is deprecated.  Use exec_module() instead.
        fn load_module(args) -> Result<Value> {
            let (this, name) = bind_check_name("FileLoader.load_module", args)?;
            let fullname = check_name(_interp, &this, name)?;
            loaders::load_module_shim(_interp, &this, &fullname)
        }

        /// Return the path to the source file as found by the finder.
        fn get_filename(args) -> Result<Value> {
            let (this, name) = bind_check_name("FileLoader.get_filename", args)?;
            check_name(_interp, &this, name)?;
            get(_interp, &this, "path")
        }

        /// Return the data from path as raw bytes.
        fn get_data(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FILE_GET_DATA, args)?;
            let this = b.take_or_none(0);
            let path = b.take_or_none(1);
            loaders::file_get_data(_interp, &this, &path)
        }

        fn get_resource_reader(args) -> Result<Value> {
            let (this, name) = bind_check_name("FileLoader.get_resource_reader", args)?;
            check_name(_interp, &this, name)?;
            let reader = module_attr(_interp, "importlib.readers", "FileReader")?;
            call(_interp, &reader, &[this])
        }
    }

    class SourceFileLoader("FileLoader", "SourceLoader") {
        constants {
            "__doc__" => Value::string(docs::SOURCE_FILE_LOADER_DOC),
        }

        /// Return the metadata for the path.
        fn path_stats(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SFL_PATH_STATS, args)?;
            let path = b.take_or_none(1);
            loaders::source_file_path_stats(_interp, &path)
        }

        fn _cache_bytecode(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SFL_CACHE_BYTECODE, args)?;
            let this = b.take_or_none(0);
            let source_path = b.take_or_none(1);
            let bytecode_path = b.take_or_none(2);
            let data = b.take_or_none(3);
            let mode = loaders::calc_mode(_interp, &source_path)?;
            let set_data = get(_interp, &this, "set_data")?;
            call_kw(_interp, &set_data, &[bytecode_path, data], &[("_mode", Value::int(mode))])
        }

        /// Write bytes data to a file.
        fn set_data(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SFL_SET_DATA, args)?;
            let path = b.take_or_none(1);
            let data = b.take_or_none(2);
            let mode = b.take_or(3, || Value::int(0o666));
            loaders::source_file_set_data(_interp, &path, &data, &mode)?;
            Ok(Value::none())
        }
    }

    class SourcelessFileLoader("FileLoader", "_LoaderBasics") {
        constants {
            "__doc__" => Value::string(docs::SOURCELESS_FILE_LOADER_DOC),
        }

        fn get_code(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SOURCELESS_GET_CODE, args)?;
            let this = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::sourceless_get_code(_interp, &this, &fullname)
        }

        /// Return None as there is no source code.
        fn get_source(args) -> Result<Value> {
            args::bind(&SIG_SOURCELESS_GET_SOURCE, args)?;
            Ok(Value::none())
        }
    }

    class ExtensionFileLoader("FileLoader", "_LoaderBasics") {
        constants {
            "__doc__" => Value::string(docs::EXTENSION_FILE_LOADER_DOC),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EXT_INIT, args)?;
            let this = b.take_or_none(0);
            let name = b.take_or_none(1);
            let path = b.take_or_none(2);
            set(_interp, &this, "name", name)?;
            set(_interp, &this, "path", path)?;
            Ok(Value::none())
        }

        fn __eq__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EXT_EQ, args)?;
            let this = b.take_or_none(0);
            let other = b.take_or_none(1);
            loaders::file_loader_eq(_interp, &this, &other)
        }

        fn __hash__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EXT_HASH, args)?;
            let this = b.take_or_none(0);
            loaders::file_loader_hash(_interp, &this)
        }

        /// Create an uninitialized extension module
        fn create_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EXT_CREATE_MODULE, args)?;
            let this = b.take_or_none(0);
            let spec = b.take_or_none(1);
            let name = get(_interp, &spec, "name")?;
            Err(loaders::extension_unsupported(_interp, &this, name))
        }

        /// Initialize an extension module
        fn exec_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EXT_EXEC_MODULE, args)?;
            let this = b.take_or_none(0);
            let module = b.take_or_none(1);
            let name = get(_interp, &module, "__name__")?;
            Err(loaders::extension_unsupported(_interp, &this, name))
        }

        /// Return True if the extension module is a package.
        fn is_package(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EXT_IS_PACKAGE, args)?;
            let this = b.take_or_none(0);
            loaders::extension_is_package(_interp, &this)
        }

        /// Return None as an extension module cannot create a code object.
        fn get_code(args) -> Result<Value> {
            args::bind(&SIG_EXT_GET_CODE, args)?;
            Ok(Value::none())
        }

        /// Return None as extension modules have no source code.
        fn get_source(args) -> Result<Value> {
            args::bind(&SIG_EXT_GET_SOURCE, args)?;
            Ok(Value::none())
        }

        /// Return the path to the source file as found by the finder.
        fn get_filename(args) -> Result<Value> {
            let (this, name) = bind_check_name("ExtensionFileLoader.get_filename", args)?;
            check_name(_interp, &this, name)?;
            get(_interp, &this, "path")
        }
    }

    class _NamespacePath {
        constants {
            "__doc__" => Value::string(docs::NAMESPACE_PATH_DOC),
            "_epoch" => Value::int(0),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_INIT, args)?;
            let this = b.take_or_none(0);
            let name = b.take_or_none(1);
            let path = b.take_or_none(2);
            let path_finder = b.take_or_none(3);
            finders::namespace_path_init(_interp, &this, name, path, path_finder)?;
            Ok(Value::none())
        }

        /// Returns a tuple of (parent-module-name, parent-path-attr-name)
        fn _find_parent_path_names(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_PARENT_NAMES, args)?;
            let this = b.take_or_none(0);
            let (parent, attr) = finders::namespace_parent_path_names(_interp, &this)?;
            Ok(Value::tuple(vec![parent, attr]))
        }

        fn _get_parent_path(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_PARENT_PATH, args)?;
            let this = b.take_or_none(0);
            finders::namespace_parent_path(_interp, &this)
        }

        fn _recalculate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_SELF, args)?;
            let this = b.take_or_none(0);
            finders::namespace_recalculate(_interp, &this)
        }

        fn __iter__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_ITER, args)?;
            let this = b.take_or_none(0);
            let path = finders::namespace_recalculate(_interp, &this)?;
            call(_interp, &builtin("iter"), &[path])
        }

        fn __getitem__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_GETITEM, args)?;
            let this = b.take_or_none(0);
            let index = b.take_or_none(1);
            let path = finders::namespace_recalculate(_interp, &this)?;
            crate::builtin_modules::operator::getitem_value(_interp, path, index)
        }

        fn __setitem__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_SETITEM, args)?;
            let this = b.take_or_none(0);
            let index = b.take_or_none(1);
            let path = b.take_or_none(2);
            let own = get(_interp, &this, "_path")?;
            crate::builtin_modules::operator::setitem_value(_interp, own, index, path)?;
            Ok(Value::none())
        }

        fn __len__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_LEN, args)?;
            let this = b.take_or_none(0);
            let path = finders::namespace_recalculate(_interp, &this)?;
            call(_interp, &builtin("len"), &[path])
        }

        fn __repr__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_REPR, args)?;
            let this = b.take_or_none(0);
            let path = get(_interp, &this, "_path")?;
            let repr = repr_of(_interp, &path)?;
            Ok(Value::string(format!("_NamespacePath({repr})")))
        }

        fn __contains__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_CONTAINS, args)?;
            let this = b.take_or_none(0);
            let item = b.take_or_none(1);
            let path = finders::namespace_recalculate(_interp, &this)?;
            Ok(Value::bool_(contains(_interp, &path, &item)?))
        }

        fn append(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSPATH_APPEND, args)?;
            let this = b.take_or_none(0);
            let item = b.take_or_none(1);
            let own = get(_interp, &this, "_path")?;
            call_method(_interp, &own, "append", &[item])?;
            Ok(Value::none())
        }
    }

    class NamespaceLoader {
        constants {
            "__doc__" => Value::none(),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSLOADER_INIT, args)?;
            let this = b.take_or_none(0);
            let name = b.take_or_none(1);
            let path = b.take_or_none(2);
            let path_finder = b.take_or_none(3);
            let class = hidden_class(_interp, "_NamespacePath")?;
            let namespace_path = call(_interp, &class, &[name, path, path_finder])?;
            set(_interp, &this, "_path", namespace_path)?;
            Ok(Value::none())
        }

        fn is_package(args) -> Result<Value> {
            args::bind(&SIG_NSLOADER_IS_PACKAGE, args)?;
            Ok(Value::bool_(true))
        }

        fn get_source(args) -> Result<Value> {
            args::bind(&SIG_NSLOADER_GET_SOURCE, args)?;
            Ok(s(""))
        }

        fn get_code(args) -> Result<Value> {
            args::bind(&SIG_NSLOADER_GET_CODE, args)?;
            call_kw(
                _interp,
                &builtin("compile"),
                &[s(""), s("<string>"), s("exec")],
                &[("dont_inherit", Value::bool_(true))],
            )
        }

        /// Use default semantics for module creation.
        fn create_module(args) -> Result<Value> {
            args::bind(&SIG_NSLOADER_CREATE_MODULE, args)?;
            Ok(Value::none())
        }

        fn exec_module(args) -> Result<Value> {
            args::bind(&SIG_NSLOADER_EXEC_MODULE, args)?;
            Ok(Value::none())
        }

        /// Load a namespace module.
        ///
        /// This method is deprecated.  Use exec_module() instead.
        fn load_module(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSLOADER_LOAD_MODULE, args)?;
            let this = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            loaders::load_module_shim(_interp, &this, &fullname)
        }

        fn get_resource_reader(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NSLOADER_RESOURCE_READER, args)?;
            let this = b.take_or_none(0);
            let path = get(_interp, &this, "_path")?;
            let reader = module_attr(_interp, "importlib.readers", "NamespaceReader")?;
            call(_interp, &reader, &[path])
        }
    }

    class PathFinder {
        constants {
            "__doc__" => Value::string(docs::PATH_FINDER_DOC),
        }

        /// Call the invalidate_caches() method on all path entry finders
        /// stored in sys.path_importer_caches (where implemented).
        #[staticmethod]
        fn invalidate_caches(args) -> Result<Value> {
            args::bind(&SIG_PATH_INVALIDATE, args)?;
            finders::path_finder_invalidate_caches(_interp)?;
            Ok(Value::none())
        }

        /// Search sys.path_hooks for a finder for 'path'.
        #[staticmethod]
        fn _path_hooks(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PATH_HOOKS, args)?;
            let path = b.take_or_none(0);
            finders::path_hooks(_interp, &path)
        }

        /// Get the finder for the path entry from sys.path_importer_cache.
        ///
        /// If the path entry is not in the cache, find the appropriate finder
        /// and cache it. If no finder is available, store None.
        #[classmethod]
        fn _path_importer_cache(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PATH_IMPORTER_CACHE, args)?;
            let cls = b.take_or_none(0);
            let path = b.take_or_none(1);
            finders::path_importer_cache(_interp, &cls, path)
        }

        /// Find the loader or namespace_path for this module/package name.
        #[classmethod]
        fn _get_spec(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PATH_GET_SPEC, args)?;
            let cls = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            let path = b.take_or_none(2);
            let target = b.take_or_none(3);
            finders::path_finder_get_spec(_interp, &cls, &fullname, &path, &target)
        }

        /// Try to find a spec for 'fullname' on sys.path or 'path'.
        ///
        /// The search is based on sys.path_hooks and sys.path_importer_cache.
        #[classmethod]
        fn find_spec(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PATH_FIND_SPEC, args)?;
            let cls = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            let path = b.take_or_none(2);
            let target = b.take_or_none(3);
            finders::path_finder_find_spec(_interp, &cls, &fullname, path, &target)
        }

        /// Find distributions.
        ///
        /// Return an iterable of all Distribution instances capable of
        /// loading the metadata for packages matching ``context.name``
        /// (or all names if ``None`` indicated) along the paths in the list
        /// of directories ``context.path``.
        #[staticmethod]
        fn find_distributions(args) -> Result<Value> {
            let finder = module_attr(_interp, "importlib.metadata", "MetadataPathFinder")?;
            let method = get(_interp, &finder, "find_distributions")?;
            _interp.call_function_expanded(method, args)
        }
    }

    class FileFinder {
        constants {
            "__doc__" => Value::string(docs::FILE_FINDER_DOC),
        }

        /// Initialize with the path to search on and a variable number of
        /// 2-tuples containing the loader and the file suffixes the loader
        /// recognizes.
        fn __init__(args) -> Result<Value> {
            finders::file_finder_init(_interp, args)?;
            Ok(Value::none())
        }

        /// Invalidate the directory mtime.
        fn invalidate_caches(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FF_INVALIDATE, args)?;
            let this = b.take_or_none(0);
            set(_interp, &this, "_path_mtime", Value::int(-1))?;
            Ok(Value::none())
        }

        fn _get_spec(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FF_GET_SPEC, args)?;
            let loader_class = b.take_or_none(1);
            let fullname = b.take_or_none(2);
            let path = b.take_or_none(3);
            let smsl = b.take_or_none(4);
            finders::file_finder_get_spec(_interp, &loader_class, &fullname, &path, smsl)
        }

        /// Try to find a spec for the specified module.
        ///
        /// Returns the matching spec, or None if not found.
        fn find_spec(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FF_FIND_SPEC, args)?;
            let this = b.take_or_none(0);
            let fullname = b.take_or_none(1);
            let target = b.take_or_none(2);
            finders::file_finder_find_spec(_interp, &this, &fullname, &target)
        }

        /// Fill the cache of potential modules and packages for this directory.
        fn _fill_cache(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FF_FILL_CACHE, args)?;
            let this = b.take_or_none(0);
            finders::file_finder_fill_cache(_interp, &this)?;
            Ok(Value::none())
        }

        /// A class method which returns a closure to use on sys.path_hook
        /// which will return an instance using the specified loaders and the path
        /// called on the closure.
        ///
        /// If the path called on the closure is not a directory, ImportError is
        /// raised.
        #[classmethod]
        fn path_hook(args) -> Result<Value> {
            let (positional, keywords) = split_args(args);
            if let Some((keyword, _)) = keywords.first() {
                return Err(type_error(format!(
                    "FileFinder.path_hook() got an unexpected keyword argument '{keyword}'"
                )));
            }
            let mut positional = positional.into_iter();
            let cls = positional.next().unwrap_or_else(Value::none);
            let details = Value::tuple(positional.collect());
            let class = hidden_class(_interp, "_FileFinderPathHook")?;
            call(_interp, &class, &[cls, details])
        }

        fn __repr__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FF_REPR, args)?;
            let this = b.take_or_none(0);
            let path = get(_interp, &this, "path")?;
            let repr = repr_of(_interp, &path)?;
            Ok(Value::string(format!("FileFinder({repr})")))
        }
    }

    /// The closure `FileFinder.path_hook(*loader_details)` returns.
    #[native_state(finders::PathHookState)]
    class _FileFinderPathHook {
        constants {
            "__doc__" => Value::string(docs::PATH_HOOK_FOR_FILE_FINDER_DOC),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            cls: PyValue,
            loader_details: PyValue,
        ) -> Result<Value> {
            let details = _interp.collect_iterable(&loader_details.0)?;
            set_state(self_val.0, finders::PathHookState { cls: cls.0, loader_details: details })?;
            set(_interp, self_val.0, "__name__", s("path_hook_for_FileFinder"))?;
            set(
                _interp,
                self_val.0,
                "__qualname__",
                s("FileFinder.path_hook.<locals>.path_hook_for_FileFinder"),
            )?;
            Ok(Value::none())
        }

        /// Path hook for importlib.machinery.FileFinder.
        fn __call__(args) -> Result<Value> {
            let (mut positional, keywords) = split_args(args);
            if positional.is_empty() {
                return Err(type_error("path_hook_for_FileFinder() missing self"));
            }
            let this = positional.remove(0);
            let rest: Vec<ExpandedCallArg> = positional
                .into_iter()
                .map(|value| ExpandedCallArg { name: None, value })
                .chain(keywords.into_iter().map(|(name, value)| ExpandedCallArg {
                    name: Some(name),
                    value,
                }))
                .collect();
            let mut b = args::bind(&SIG_PATH_HOOK_CALL, &rest)?;
            let path = b.take_or_none(0);
            let (cls, details) = with_state(&this, |state: &finders::PathHookState| {
                Ok((state.cls.clone(), state.loader_details.clone()))
            })?;
            finders::path_hook_call(_interp, &cls, &details, path)
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let id = call(_interp, &builtin("id"), std::slice::from_ref(self_val.0))?;
            let id = id.as_int().unwrap_or_default();
            // CPython's `%p`: zero-padded upper-case hex on Windows.
            #[cfg(windows)]
            let address = format!("0x{id:016X}");
            #[cfg(not(windows))]
            let address = format!("0x{id:x}");
            Ok(Value::string(format!(
                "<function FileFinder.path_hook.<locals>.path_hook_for_FileFinder at {address}>"
            )))
        }
    }
}
