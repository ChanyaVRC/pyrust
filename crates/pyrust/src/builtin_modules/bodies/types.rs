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
//     `mappingproxy` class singleton; built-in constructor dispatch owns
//     argument validation.
//   * `SimpleNamespace` — a plain Python class injected from `types_py.py`.
//
// The `MODULE_NAME` constant is injected by the `pyrust_builtin_modules!`
// plumbing; no module-name literal appears in this file.
//
// Reference: <https://docs.python.org/3/library/types.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::interpreter::{function_type_singleton, method_type_singleton, primitive_class_by_name};
use crate::value::{PyKey, Value};
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

/// `Value` for the `method` type singleton (`type(obj.method)`).  Returns the
/// proper `method` `PyClass` so `type(C().m) is types.MethodType` holds and the
/// repr is `<class 'method'>` (issue #1528 made `method` a real class).
fn method_type_value() -> Value {
    Value::py_class(method_type_singleton())
}

/// `Value` for a primitive type whose class singleton is registered in
/// `primitive_class_by_name` (`ellipsis`, `NotImplementedType`).  These resolve
/// to the same `PyClass` `type(...)` / `type(NotImplemented)` return, so
/// identity holds and the repr is `<class 'ellipsis'>` etc.  Falls back to the
/// matching value sentinel if the class singleton is somehow unavailable.
fn primitive_type_value(name: &str, fallback: Value) -> Value {
    match primitive_class_by_name(name) {
        Some(rc) => Value::py_class(rc),
        None => fallback,
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
        // CPython: types.MethodType — `type(obj.method)`; the type of bound
        // methods.  This is the real `method` class, so
        // `type(C().m) is types.MethodType`.
        "MethodType"          => method_type_value(),
        // CPython: types.GeneratorType — `type(x for x in [])`.  `type()` of a
        // generator returns the `generator` builtin-type sentinel, which is a
        // by-name singleton, so identity holds.
        "GeneratorType"       => Value::builtin_function("generator"),
        // CPython: types.CoroutineType — `type(async_def_call())`.
        "CoroutineType"       => Value::builtin_function("coroutine"),
        // CPython: types.AsyncGeneratorType — type of `async def` generators.
        "AsyncGeneratorType"  => Value::builtin_function("async_generator"),
        // CPython: types.UnionType — `type(int | str)` (PEP 604).  `type()` of a
        // union value reports the `types.UnionType` name tag, so identity holds
        // and `typing.get_origin(int | str) is types.UnionType`.
        "UnionType"           => Value::builtin_function(
            pyrust_builtins::union_type::TYPE_NAME,
        ),
        // CPython: types.EllipsisType — `type(...)`; the real `ellipsis` class.
        "EllipsisType"        => primitive_type_value("ellipsis", Value::ellipsis()),
        // CPython: types.NotImplementedType — `type(NotImplemented)`; the real
        // `NotImplementedType` class.
        "NotImplementedType"  => primitive_type_value(
            "NotImplementedType",
            Value::not_implemented(),
        ),
    }
}

/// Execute `TYPES_PY_SOURCE` once and copy its public names onto the module's
/// attribute map.  Called from the `types` post-load `@inject` hook in
/// `env.rs::load_module` (mirrors `operator` / `string`).
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(TYPES_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("types: exec namespace not a dict".into()))?;
    for name in TYPES_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(())
}
