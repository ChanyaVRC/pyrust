// `types` module — body for the `types` entry in `pyrust_builtin_modules!`.
//
// CPython's `types` module exposes the type objects for runtime objects that
// have no built-in name binding (functions, modules, mapping proxies, …) plus
// a handful of helpers.  We implement the commonly-used subset:
//
//   * Type-object constants (`NoneType`, `FunctionType`, `LambdaType`,
//     `ModuleType`, `BuiltinFunctionType`, `GenericAlias`).  These reference
//     the interpreter's internal type singletons / sentinels so that, e.g.,
//     `type(len) is types.BuiltinFunctionType` and
//     `type(lambda: 0) is types.FunctionType` hold.
//   * `MappingProxyType` — the real `mappingproxy` primitive class.  In
//     CPython `types.MappingProxyType` *is* the type, so
//     `type(int.__dict__) is types.MappingProxyType` holds and calling it
//     constructs a proxy.  We bind the constant to the interpreter's
//     `mappingproxy` class singleton; the construction call is intercepted in
//     `calls.rs` and dispatched to `construct_mapping_proxy` below.
//   * `SimpleNamespace` — a plain Python class injected from `types_py.py`.
//
// The `MODULE_NAME` constant is injected by the `pyrust_builtin_modules!`
// plumbing; no module-name literal appears in this file.
//
// Reference: <https://docs.python.org/3/library/types.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::Interpreter;
use crate::interpreter::{function_type_singleton, primitive_class_by_name, value_type_name_str};
use crate::value::{PyDict, PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source members (`SimpleNamespace`) injected at first import.
const TYPES_PY_SOURCE: &str = include_str!("types_py.py");

/// Public names defined by `TYPES_PY_SOURCE` to copy onto the module.
const TYPES_PY_EXPORTS: &[&str] = &["SimpleNamespace"];

/// `Value` for the `function` type singleton (`type(lambda: 0)`).  Used for
/// both `FunctionType` and `LambdaType` (CPython aliases them to the same
/// object).
fn function_type_value() -> Value {
    Value::py_class(function_type_singleton())
}

/// `Value` for the `NoneType` primitive class (`type(None)`).
fn none_type_value() -> Value {
    match primitive_class_by_name("NoneType") {
        Some(rc) => Value::py_class(rc),
        None => Value::none(),
    }
}

/// `Value` for the `mappingproxy` primitive class (`type(int.__dict__)`).  This
/// is the *same object* `type(...)` of a live proxy resolves to, so
/// `type(int.__dict__) is types.MappingProxyType` holds.  Falls back to the
/// native `MappingProxyType` builtin sentinel if the class singleton is
/// somehow unavailable (it never is at runtime).
fn mapping_proxy_type_value() -> Value {
    match primitive_class_by_name("mappingproxy") {
        Some(rc) => Value::py_class(rc),
        None => Value::builtin_function("MappingProxyType"),
    }
}

/// Build a `mappingproxy` from `MappingProxyType(...)` / `mappingproxy(...)`
/// call arguments.  Shared by the `types` constant's call interception in
/// `calls.rs`.  CPython 3.12 (Argument Clinic): the single `mapping` parameter
/// is positional-or-keyword, so `mappingproxy(d)` and `mappingproxy(mapping=d)`
/// both work, while a 2nd argument or a wrong keyword raises the `mappingproxy`
/// wording.  `mapping` must be a mapping (we accept `dict`).
pub(crate) fn construct_mapping_proxy(args: &[ExpandedCallArg]) -> Result<Value> {
    // More than one argument (positional and/or keyword) is always rejected.
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "mappingproxy() takes at most 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    // Resolve the single `mapping` argument: a lone positional, or the keyword
    // `mapping=`.  A missing positional / wrong keyword name is "missing
    // required argument 'mapping' (pos 1)" (CPython ignores the stray keyword
    // and reports the unfilled positional).
    let mapping = match args.first() {
        Some(a) if a.name.as_deref().is_none_or(|n| n == "mapping") => &a.value,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "mappingproxy() missing required argument 'mapping' (pos 1)".to_string(),
            ));
        }
    };
    match mapping.get_dict_rc() {
        Some(rc) => Ok(pyrust_builtins::mapping_proxy::mapping_proxy_dict(
            Rc::clone(rc),
        )),
        None => Err(PyError::named(
            "TypeError",
            format!(
                "mappingproxy() argument must be a mapping, not {}",
                value_type_name_str(mapping)
            ),
        )),
    }
}

pyrust_module! {
    constants {
        // CPython: types.NoneType — `type(None)`.
        "NoneType"            => none_type_value(),
        // CPython: types.FunctionType — `type(lambda: 0)`; the type of
        // user-defined functions.
        "FunctionType"        => function_type_value(),
        // CPython: types.LambdaType — alias of FunctionType (same object).
        "LambdaType"          => function_type_value(),
        // CPython: types.ModuleType — `type(sys)`.
        "ModuleType"          => Value::builtin_function("module"),
        // CPython: types.BuiltinFunctionType — `type(len)`.  Built-in
        // functions and methods share this type; `BuiltinMethodType` is the
        // same object.
        "BuiltinFunctionType" => Value::builtin_function("builtin_function_or_method"),
        "BuiltinMethodType"   => Value::builtin_function("builtin_function_or_method"),
        // CPython: types.GenericAlias — `type(list[int])`.  This must be the
        // SAME object `type(list[int])` returns (issue #2733), so identity
        // (`type(list[int]) is types.GenericAlias`) holds.  Calling it
        // constructs an alias (intercepted in `calls.rs`).
        "GenericAlias"        => Value::py_class(crate::interpreter::generic_alias_class_singleton()),
        // CPython: types.MappingProxyType — `type(int.__dict__)`; the real
        // `mappingproxy` class.  Calling it constructs a proxy (intercepted in
        // `calls.rs`), and `type(int.__dict__) is types.MappingProxyType`.
        "MappingProxyType"    => mapping_proxy_type_value(),
    }
}

/// Execute `TYPES_PY_SOURCE` once and copy its public names onto the module's
/// attribute map.  Called from the `types` post-load `@inject` hook in
/// `env.rs::load_module` (mirrors `operator` / `string`).
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(TYPES_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("types: exec namespace not a dict".into()))?;
    for name in TYPES_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            // `SimpleNamespace`'s repr / unhashable-type message qualify with
            // `__module__`; the exec namespace defaults `__module__` to
            // `__main__`, so override it to `types` to match CPython.
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("types"));
            }
            module.borrow_mut().attrs.insert(name.to_string(), val.clone());
        }
    }
    Ok(())
}
