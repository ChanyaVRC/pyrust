// `types` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/types.py` (issues #2970, #3460); there is no Python-source injection.
//
// CPython's `types` module exposes the type objects for runtime objects that
// have no built-in name binding (functions, modules, mapping proxies, …) plus
// the class-creation helpers (`new_class`, `resolve_bases`, `prepare_class`,
// `get_original_bases`), `DynamicClassAttribute`, the `coroutine` decorator
// and `SimpleNamespace`.
//
//   * Type-object constants (`NoneType`, `FunctionType`, `LambdaType`,
//     `ModuleType`, `BuiltinFunctionType`, `GenericAlias`, …).  These
//     reference the interpreter's internal type singletons / sentinels so
//     that, e.g., `type(len) is types.BuiltinFunctionType` and
//     `type(lambda: 0) is types.FunctionType` hold.
//   * `MappingProxyType` — the real `mappingproxy` primitive class.
//   * `new_class` / `resolve_bases` / `prepare_class` / `_calculate_meta` /
//     `get_original_bases` follow the CPython source statement by statement
//     and resolve the sibling helpers through the live module namespace, so
//     monkeypatching `types.resolve_bases` is seen by `new_class` exactly as
//     on CPython.
//   * `DynamicClassAttribute` and `_GeneratorWrapper` keep their state in
//     ordinary instance attributes (`fget`, `overwrite_doc`,
//     `_GeneratorWrapper__wrapped`, …) so `vars()` and subclasses (`enum`
//     derives its `property` from `DynamicClassAttribute`) see what they see
//     on CPython.
//   * `SimpleNamespace` is the real C-level type: `__module__ == "types"`,
//     `namespace(...)` repr with a recursion guard, `__eq__` / `__ne__` on
//     `__dict__`, `__reduce__`, unhashable.
//   * `coroutine` marks generator functions `CO_ITERABLE_COROUTINE` in place
//     and wraps every other callable in a metadata-preserving callable that
//     passes native coroutines / marked generators through and wraps
//     generator-protocol objects in `_GeneratorWrapper`.  The wrapper's class
//     (`_CoroutineWrapper`, standing in for CPython's `wrapped` closure) is a
//     hidden helper class removed from the module namespace.
//
// The `MODULE_NAME` constant is injected by the `pyrust_builtin_modules!`
// plumbing; no module-name literal appears in this file.
//
// Reference: <https://docs.python.org/3.12/library/types.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::operator::{getitem_value, setitem_value};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, function_type_singleton, lookup_exc_class, method_type_singleton,
    primitive_class_by_name, render_value_repr, value_class,
};
use crate::value::{GeneratorKind, InstanceAttrs, PyClass, PyInstance, PyModule, Value, ValueKind};

const MODULE_DOC: &str =
    "\nDefine names for built-in types that aren't directly accessible as a builtin.\n";

const DYNAMIC_CLASS_ATTRIBUTE_DOC: &str = "Route attribute access on a class to __getattr__.\n\n    \
This is a descriptor, used to define attributes that act differently when\n    \
accessed through an instance and through a class.  Instance access remains\n    \
normal, but access to an attribute through a class will be routed to the\n    \
class's __getattr__ method; this is done by raising AttributeError.\n\n    \
This allows one to have properties active on an instance, and have virtual\n    \
attributes on the class with the same name.  (Enum used this between Python\n    \
versions 3.4 - 3.9 .)\n\n    \
Subclass from this to use a different method of accessing virtual attributes\n    \
and still be treated properly by the inspect module. (Enum uses this since\n    \
Python 3.10 .)\n\n    ";

const SIMPLE_NAMESPACE_DOC: &str =
    "A simple attribute-based namespace.\n\nSimpleNamespace(**kwargs)";

/// `types.__all__` (CPython builds it from the public globals in definition
/// order).
const ALL_NAMES: &[&str] = &[
    "FunctionType",
    "LambdaType",
    "CodeType",
    "MappingProxyType",
    "SimpleNamespace",
    "CellType",
    "GeneratorType",
    "CoroutineType",
    "AsyncGeneratorType",
    "MethodType",
    "BuiltinFunctionType",
    "BuiltinMethodType",
    "WrapperDescriptorType",
    "MethodWrapperType",
    "MethodDescriptorType",
    "ClassMethodDescriptorType",
    "ModuleType",
    "TracebackType",
    "FrameType",
    "GetSetDescriptorType",
    "MemberDescriptorType",
    "new_class",
    "resolve_bases",
    "prepare_class",
    "get_original_bases",
    "DynamicClassAttribute",
    "coroutine",
    "GenericAlias",
    "UnionType",
    "EllipsisType",
    "NoneType",
    "NotImplementedType",
];

/// Name of the hidden helper class `coroutine` instantiates for the wrapper
/// path.
const COROUTINE_WRAPPER_CLASS: &str = "_CoroutineWrapper";

// Thread-local justification:
// (a) Why `thread_local!`: holds a strong `Rc<RefCell<PyClass>>` (`!Send`) to
//     the hidden `_CoroutineWrapper` helper class of the newest imported
//     `types` module, which `coroutine()` instantiates without a module
//     attribute to find it by.
// (b) Multi-interpreter / multi-threaded impact: each interpreter thread
//     keeps the class of its own `types` import; separate threads are
//     completely isolated.
pyrust_core::gil_local! {
    /// The hidden `_CoroutineWrapper` class of the newest prepared module.
    static COROUTINE_WRAPPER: RefCell<Option<Rc<RefCell<PyClass>>>> =
        const { RefCell::new(None) };
}

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

const SIG_NEW_CLASS: Sig = sig("new_class", &["name", "bases", "kwds", "exec_body"], 3);
const SIG_RESOLVE_BASES: Sig = sig("resolve_bases", &["bases"], 0);
const SIG_PREPARE_CLASS: Sig = sig("prepare_class", &["name", "bases", "kwds"], 2);
const SIG_CALCULATE_META: Sig = sig("_calculate_meta", &["meta", "bases"], 0);
const SIG_COROUTINE: Sig = sig("coroutine", &["func"], 0);
const SIG_DCA_INIT: Sig = sig(
    "DynamicClassAttribute.__init__",
    &["self", "fget", "fset", "fdel", "doc"],
    4,
);
const SIG_DCA_GET: Sig = sig(
    "DynamicClassAttribute.__get__",
    &["self", "instance", "ownerclass"],
    1,
);
const SIG_DCA_SET: Sig = sig(
    "DynamicClassAttribute.__set__",
    &["self", "instance", "value"],
    0,
);
const SIG_DCA_DELETE: Sig = sig("DynamicClassAttribute.__delete__", &["self", "instance"], 0);
const SIG_DCA_GETTER: Sig = sig("DynamicClassAttribute.getter", &["self", "fget"], 0);
const SIG_DCA_SETTER: Sig = sig("DynamicClassAttribute.setter", &["self", "fset"], 0);
const SIG_DCA_DELETER: Sig = sig("DynamicClassAttribute.deleter", &["self", "fdel"], 0);
const SIG_GW_INIT: Sig = sig("_GeneratorWrapper.__init__", &["self", "gen"], 0);
const SIG_GW_SEND: Sig = sig("_GeneratorWrapper.send", &["self", "val"], 0);
const SIG_GW_THROW: Sig = sig("_GeneratorWrapper.throw", &["self", "tp"], 0);
const SIG_GW_CLOSE: Sig = sig("_GeneratorWrapper.close", &["self"], 0);
const SIG_GW_NEXT: Sig = sig("_GeneratorWrapper.__next__", &["self"], 0);
const SIG_GW_ITER: Sig = sig("_GeneratorWrapper.__iter__", &["self"], 0);
const SIG_GW_AWAIT: Sig = sig("_GeneratorWrapper.__await__", &["self"], 0);
const SIG_NS_SELF: Sig = sig("SimpleNamespace", &["self"], 0);
const SIG_NS_COMPARE: Sig = sig("SimpleNamespace.__eq__", &["self", "other"], 0);

fn type_error(message: impl Into<String>) -> PyError {
    PyError::named("TypeError", message.into())
}

fn attribute_error(message: impl Into<String>) -> PyError {
    PyError::named("AttributeError", message.into())
}

fn call_pos(interp: &mut Interpreter, callee: Value, values: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = values
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_builtin(interp: &mut Interpreter, name: &'static str, values: &[Value]) -> Result<Value> {
    call_pos(interp, Value::builtin_function(name), values)
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    values: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, values)
}

/// `callee(*positional, **kwds)` with `kwds` a dict; a non-`str` key is the
/// `TypeError` CPython raises when unpacking it into keyword arguments.
fn call_with_dict_kwargs(
    interp: &mut Interpreter,
    callee: Value,
    positional: &[Value],
    kwds: &Value,
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = positional
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    let items = call_method(interp, kwds, "items", &[])?;
    for item in interp.collect_iterable(&items)? {
        let pair = interp.collect_iterable(&item)?;
        let [key, value] = <[Value; 2]>::try_from(pair)
            .map_err(|_| PyError::runtime("types: dict item is not a pair".into()))?;
        let Some(name) = key.as_str() else {
            return Err(type_error("keywords must be strings"));
        };
        expanded.push(ExpandedCallArg {
            name: Some(name.to_string()),
            value,
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

/// A module global of `types`, resolved live.
fn module_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module(MODULE_NAME)?;
    interp.get_attr(&module, name)
}

fn is_same(interp: &mut Interpreter, left: &Value, right: &Value) -> Result<bool> {
    let result = interp.eval_binary(left.clone(), BinaryOp::Is, right.clone())?;
    interp.truthy_value(&result)
}

fn is_instance(interp: &mut Interpreter, value: &Value, class: &Value) -> Result<bool> {
    let result = call_builtin(interp, "isinstance", &[value.clone(), class.clone()])?;
    interp.truthy_value(&result)
}

fn is_subclass(interp: &mut Interpreter, class: &Value, base: &Value) -> Result<bool> {
    let result = call_builtin(interp, "issubclass", &[class.clone(), base.clone()])?;
    interp.truthy_value(&result)
}

/// `hasattr(obj, name)`: only `AttributeError` counts as "absent".
fn has_attr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    match interp.get_attr(obj, name) {
        Ok(_) => Ok(true),
        Err(ref err) if err.class_name_is("AttributeError") => Ok(false),
        Err(err) => Err(err),
    }
}

/// `getattr(obj, name, default)`: only `AttributeError` counts as "absent".
fn get_attr_or(interp: &mut Interpreter, obj: &Value, name: &str, default: Value) -> Result<Value> {
    match interp.get_attr(obj, name) {
        Ok(value) => Ok(value),
        Err(ref err) if err.class_name_is("AttributeError") => Ok(default),
        Err(err) => Err(err),
    }
}

fn builtin_value(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let builtins = interp.load_module("builtins")?;
    interp.get_attr(&builtins, name)
}

fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

/// `obj.__class__`.
fn dunder_class(interp: &mut Interpreter, obj: &Value) -> Result<Value> {
    interp.get_attr(obj, "__class__")
}

/// The `self` receiver of a native method.
fn self_of(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    args.first()
        .map(|arg| arg.value.clone())
        .ok_or_else(|| PyError::runtime(format!("internal: {fn_name}() missing self")))
}

/// A bare `AttributeError()` (no arguments), as `DynamicClassAttribute`
/// raises to route a class-level access to `__getattr__`.
fn bare_attribute_error(interp: &mut Interpreter) -> PyError {
    let Some(class) = lookup_exc_class("AttributeError") else {
        return attribute_error("");
    };
    match call_pos(interp, Value::py_class(class), &[]) {
        Ok(instance) => PyError::raised(instance),
        Err(error) => error,
    }
}

// ---------------------------------------------------------------------------
// new_class / resolve_bases / prepare_class / _calculate_meta / get_original_bases
// ---------------------------------------------------------------------------

/// `resolve_bases(bases)`: PEP 560 `__mro_entries__` resolution.
fn resolve_bases_impl(interp: &mut Interpreter, bases: &Value) -> Result<Value> {
    let originals = interp.collect_iterable(bases)?;
    let mut new_bases = originals.clone();
    let type_class = builtin_value(interp, "type")?;
    let tuple_class = builtin_value(interp, "tuple")?;
    let mut updated = false;
    let mut shift = 0isize;
    for (index, base) in originals.iter().enumerate() {
        if is_instance(interp, base, &type_class)? {
            continue;
        }
        if !has_attr(interp, base, "__mro_entries__")? {
            continue;
        }
        let entries_fn = interp.get_attr(base, "__mro_entries__")?;
        let new_base = call_pos(interp, entries_fn, std::slice::from_ref(bases))?;
        updated = true;
        if !is_instance(interp, &new_base, &tuple_class)? {
            return Err(type_error("__mro_entries__ must return a tuple"));
        }
        let entries = interp.collect_iterable(&new_base)?;
        let start = (index as isize + shift) as usize;
        let count = entries.len();
        new_bases.splice(start..start + 1, entries);
        shift += count as isize - 1;
    }
    if !updated {
        return Ok(bases.clone());
    }
    Ok(Value::tuple(new_bases))
}

/// `_calculate_meta(meta, bases)`: the most derived metaclass.
fn calculate_meta_impl(interp: &mut Interpreter, meta: &Value, bases: &Value) -> Result<Value> {
    let mut winner = meta.clone();
    for base in interp.collect_iterable(bases)? {
        let base_meta = value_class(&base);
        if is_subclass(interp, &winner, &base_meta)? {
            continue;
        }
        if is_subclass(interp, &base_meta, &winner)? {
            winner = base_meta;
            continue;
        }
        return Err(type_error(
            "metaclass conflict: the metaclass of a derived class must be a (non-strict) \
             subclass of the metaclasses of all its bases",
        ));
    }
    Ok(winner)
}

/// `prepare_class(name, bases, kwds)` → `(meta, ns, kwds)`.
fn prepare_class_impl(
    interp: &mut Interpreter,
    name: &Value,
    bases: &Value,
    kwds: &Value,
) -> Result<Value> {
    let kwds = if kwds.is_none() {
        Value::dict(crate::value::PyDict::default())
    } else {
        // Don't alter the provided mapping.
        call_builtin(interp, "dict", std::slice::from_ref(kwds))?
    };
    let metaclass_key = Value::string("metaclass");
    let contains = call_method(
        interp,
        &kwds,
        "__contains__",
        std::slice::from_ref(&metaclass_key),
    )?;
    let mut meta = if truthy(interp, &contains)? {
        call_method(interp, &kwds, "pop", std::slice::from_ref(&metaclass_key))?
    } else if truthy(interp, bases)? {
        let first = getitem_value(interp, bases.clone(), Value::int(0))?;
        value_class(&first)
    } else {
        builtin_value(interp, "type")?
    };
    let type_class = builtin_value(interp, "type")?;
    if is_instance(interp, &meta, &type_class)? {
        // When meta is a type, we first determine the most-derived metaclass
        // instead of invoking the initial candidate directly.
        let calculate = module_attr(interp, "_calculate_meta")?;
        meta = call_pos(interp, calculate, &[meta, bases.clone()])?;
    }
    let ns = if has_attr(interp, &meta, "__prepare__")? {
        let prepare = interp.get_attr(&meta, "__prepare__")?;
        call_with_dict_kwargs(interp, prepare, &[name.clone(), bases.clone()], &kwds)?
    } else {
        Value::dict(crate::value::PyDict::default())
    };
    Ok(Value::tuple(vec![meta, ns, kwds]))
}

// ---------------------------------------------------------------------------
// coroutine
// ---------------------------------------------------------------------------

/// `CO_COROUTINE | CO_ITERABLE_COROUTINE`.
const CO_COROUTINE_FLAGS: i64 = 0x180;
/// `CO_GENERATOR`.
const CO_GENERATOR: i64 = 0x20;

/// The post-call logic of `coroutine`'s `wrapped`: pass native coroutines and
/// marked generators through, wrap generator-protocol objects.
fn wrap_coroutine_result(interp: &mut Interpreter, coro: Value) -> Result<Value> {
    let coro_class = dunder_class(interp, &coro)?;
    let coroutine_type = module_attr(interp, "CoroutineType")?;
    let generator_type = module_attr(interp, "GeneratorType")?;
    if is_same(interp, &coro_class, &coroutine_type)? {
        // 'coro' is a native coroutine object.
        return Ok(coro);
    }
    if is_same(interp, &coro_class, &generator_type)? {
        let marked = match coro.kind() {
            ValueKind::Generator(cell) => cell.is_iterable_coroutine(),
            _ => false,
        };
        if marked {
            // 'coro' is an iterable coroutine.
            return Ok(coro);
        }
    }
    let abc = interp.load_module("_collections_abc")?;
    let generator_abc = interp.get_attr(&abc, "Generator")?;
    let coroutine_abc = interp.get_attr(&abc, "Coroutine")?;
    // PyRust stores built-in iterators, generators, coroutines and async
    // generators behind one internal value variant; only a real plain
    // generator belongs on the `_GeneratorWrapper` path.  Protocol objects
    // stay eligible and are filtered by the ABC checks.
    let candidate = match coro.kind() {
        ValueKind::Generator(cell) => cell.kind() == GeneratorKind::Generator,
        _ => true,
    };
    if candidate
        && is_instance(interp, &coro, &generator_abc)?
        && !is_instance(interp, &coro, &coroutine_abc)?
    {
        // 'coro' is either a pure Python generator iterator, or it implements
        // collections.abc.Generator (and does not implement
        // collections.abc.Coroutine).
        let wrapper = module_attr(interp, "_GeneratorWrapper")?;
        return call_pos(interp, wrapper, &[coro]);
    }
    // 'coro' is either an instance of collections.abc.Coroutine or some other
    // object -- pass it through.
    Ok(coro)
}

/// `functools.update_wrapper(wrapper, wrapped)` (what `@functools.wraps` does).
fn update_wrapper(interp: &mut Interpreter, wrapper: &Value, wrapped: &Value) -> Result<()> {
    let functools = interp.load_module("functools")?;
    let update = interp.get_attr(&functools, "update_wrapper")?;
    call_pos(interp, update, &[wrapper.clone(), wrapped.clone()])?;
    Ok(())
}

/// `coroutine(func)`.
fn coroutine_impl(interp: &mut Interpreter, func: Value) -> Result<Value> {
    let is_callable = call_builtin(interp, "callable", std::slice::from_ref(&func))?;
    if !truthy(interp, &is_callable)? {
        return Err(type_error("types.coroutine() expects a callable"));
    }
    let function_type = module_attr(interp, "FunctionType")?;
    let func_class = dunder_class(interp, &func)?;
    if is_same(interp, &func_class, &function_type)? {
        let code = get_attr_or(interp, &func, "__code__", Value::none())?;
        let code_class = dunder_class(interp, &code)?;
        let code_type = module_attr(interp, "CodeType")?;
        if is_same(interp, &code_class, &code_type)? {
            let flags = interp.get_attr(&code, "co_flags")?;
            let flags = flags.as_int().unwrap_or(0);
            // Check if 'func' is a coroutine function.
            if flags & CO_COROUTINE_FLAGS != 0 {
                return Ok(func);
            }
            // Check if 'func' is a generator function: mark it
            // `CO_ITERABLE_COROUTINE` (on this function object only).
            if flags & CO_GENERATOR != 0 {
                if let ValueKind::UserFunction(function) = func.kind() {
                    function.iterable_coroutine.set(true);
                }
                return Ok(func);
            }
        }
    }
    // The following code is primarily to support functions that return
    // generator-like objects (for instance generators compiled with Cython).
    // Delay the functools / _collections_abc imports like CPython does.
    interp.load_module("functools")?;
    interp.load_module("_collections_abc")?;
    let class = COROUTINE_WRAPPER
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| PyError::runtime("types: _CoroutineWrapper class not prepared".into()))?;
    let wrapped = call_pos(interp, Value::py_class(class), std::slice::from_ref(&func))?;
    update_wrapper(interp, &wrapped, &func)?;
    Ok(wrapped)
}

/// State of a `_CoroutineWrapper`: the decorated callable and, once bound as a
/// method, the receiver.
struct CoroutineWrapperState {
    func: Value,
    bound: Option<Value>,
}

// ---------------------------------------------------------------------------
// Type-object constants
// ---------------------------------------------------------------------------

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

fn all_list() -> Value {
    Value::list(ALL_NAMES.iter().map(|name| Value::string(*name)).collect())
}

// ---------------------------------------------------------------------------
// DynamicClassAttribute helpers
// ---------------------------------------------------------------------------

/// `DynamicClassAttribute.__init__(self, fget=None, fset=None, fdel=None,
/// doc=None)`.
fn dca_init(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut bound = args::bind(&SIG_DCA_INIT, args)?;
    let this = bound.take_or_none(0);
    let fget = bound.take_or_none(1);
    let fset = bound.take_or_none(2);
    let fdel = bound.take_or_none(3);
    let doc = bound.take_or_none(4);
    interp.assign_attr(this.clone(), "fget", fget.clone())?;
    interp.assign_attr(this.clone(), "fset", fset)?;
    interp.assign_attr(this.clone(), "fdel", fdel)?;
    // next two lines make DynamicClassAttribute act the same as property
    let doc_value = if truthy(interp, &doc)? {
        doc.clone()
    } else {
        interp.get_attr(&fget, "__doc__")?
    };
    interp.assign_attr(this.clone(), "__doc__", doc_value)?;
    interp.assign_attr(this.clone(), "overwrite_doc", Value::bool_(doc.is_none()))?;
    // support for abstract methods
    let abstract_flag = get_attr_or(interp, &fget, "__isabstractmethod__", Value::bool_(false))?;
    let abstract_flag = truthy(interp, &abstract_flag)?;
    interp.assign_attr(this, "__isabstractmethod__", Value::bool_(abstract_flag))?;
    Ok(Value::none())
}

/// Shared tail of `getter` / `setter` / `deleter`:
/// `result = type(self)(*ctor_args); result.overwrite_doc = self.overwrite_doc`.
fn dca_copy(interp: &mut Interpreter, this: &Value, ctor_args: &[Value]) -> Result<Value> {
    let class = value_class(this);
    let result = call_pos(interp, class, ctor_args)?;
    let overwrite_doc = interp.get_attr(this, "overwrite_doc")?;
    interp.assign_attr(result.clone(), "overwrite_doc", overwrite_doc)?;
    Ok(result)
}

// ---------------------------------------------------------------------------
// SimpleNamespace helpers
// ---------------------------------------------------------------------------

/// `SimpleNamespace.__repr__`.
fn namespace_repr(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let class = value_class(this);
    let namespace_class = module_attr(interp, "SimpleNamespace")?;
    let name = if is_same(interp, &class, &namespace_class)? {
        "namespace".to_string()
    } else {
        let class_name = interp.get_attr(&class, "__name__")?;
        class_name.as_str().unwrap_or_default().to_string()
    };
    let identity = match this.kind() {
        ValueKind::PyInstance(inst) => Rc::as_ptr(inst) as usize as i64,
        _ => 0,
    };
    let Some(_guard) = pyrust_core::enter_repr_cycle_guard(identity) else {
        return Ok(Value::string(format!("{name}(...)")));
    };
    let dict = interp.get_attr(this, "__dict__")?;
    let items = call_method(interp, &dict, "items", &[])?;
    let mut pieces = Vec::new();
    for item in interp.collect_iterable(&items)? {
        let pair = interp.collect_iterable(&item)?;
        let [key, value] = <[Value; 2]>::try_from(pair)
            .map_err(|_| PyError::runtime("types: dict item is not a pair".into()))?;
        // Only non-empty string keys are rendered.
        let Some(key) = key.as_str().filter(|key| !key.is_empty()) else {
            continue;
        };
        let rendered = render_value_repr(interp, &value)?;
        pieces.push(format!("{key}={rendered}"));
    }
    Ok(Value::string(format!("{name}({})", pieces.join(", "))))
}

/// `SimpleNamespace.__eq__` / `__ne__`: compare the `__dict__`s of two
/// namespaces; anything else is `NotImplemented`.
fn namespace_compare(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    op: BinaryOp,
) -> Result<Value> {
    let mut bound = args::bind(&SIG_NS_COMPARE, args)?;
    let this = bound.take_or_none(0);
    let other = bound.take_or_none(1);
    let namespace_class = module_attr(interp, "SimpleNamespace")?;
    if !is_instance(interp, &this, &namespace_class)?
        || !is_instance(interp, &other, &namespace_class)?
    {
        return Ok(Value::not_implemented());
    }
    // PyRust's instance `__dict__` is an identity-proxied view whose `==`
    // does not consult the values' `__eq__`; compare real dict snapshots.
    let left = interp.get_attr(&this, "__dict__")?;
    let left = call_method(interp, &left, "copy", &[])?;
    let right = interp.get_attr(&other, "__dict__")?;
    let right = call_method(interp, &right, "copy", &[])?;
    interp.eval_binary(left, op, right)
}

pyrust_module! {
    constants {
        "__doc__"             => Value::string(MODULE_DOC),
        "__all__"             => all_list(),
        // CPython: types.NoneType — `type(None)`.
        "NoneType"            => none_type_value(),
        // CPython: types.FunctionType — `type(lambda: 0)`; the type of
        // user-defined functions.
        "FunctionType"        => function_type_value(),
        // CPython: types.LambdaType — alias of FunctionType (same object).
        "LambdaType"          => function_type_value(),
        // CPython runtime-object types already modelled by pyrust's built-in
        // type-name tokens.  These aliases must be the exact objects returned
        // by `type()` for representative runtime values.
        "CodeType"            => Value::builtin_function(pyrust_builtins::code::TYPE_NAME),
        "CellType"            => Value::builtin_function(pyrust_builtins::cell::TYPE_NAME),
        "WrapperDescriptorType" => Value::builtin_function("wrapper_descriptor"),
        "MethodWrapperType"   => Value::builtin_function(
            pyrust_builtins::type_call_wrapper::TYPE_NAME,
        ),
        "MethodDescriptorType" => Value::builtin_function(
            pyrust_builtins::unbound_method_descriptor::TYPE_NAME,
        ),
        "ClassMethodDescriptorType" => Value::builtin_function(
            pyrust_builtins::classmethod::NATIVE_CLASS_METHOD_DESCRIPTOR_TYPE_NAME,
        ),
        // CPython: types.ModuleType — `type(sys)` (issue #2970).
        "ModuleType"          => Value::py_class(crate::interpreter::module_type_singleton()),
        "TracebackType"       => Value::builtin_function(pyrust_builtins::traceback::TYPE_NAME),
        "FrameType"           => Value::builtin_function(pyrust_builtins::frame::TYPE_NAME),
        "GetSetDescriptorType" => Value::builtin_function(
            pyrust_builtins::numeric_attrs_descriptor::GETSET_TYPE_NAME,
        ),
        "MemberDescriptorType" => Value::builtin_function(
            pyrust_builtins::member_descriptor::MEMBER_DESCRIPTOR_TYPE_NAME,
        ),
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

    /// Create a class object dynamically using the appropriate metaclass.
    fn new_class(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_NEW_CLASS, args)?;
        let name = bound.take_or_none(0);
        let bases = bound.take_or(1, || Value::tuple(Vec::new()));
        let kwds = bound.take_or_none(2);
        let exec_body = bound.take_or_none(3);
        let resolve = module_attr(_interp, "resolve_bases")?;
        let resolved_bases = call_pos(_interp, resolve, std::slice::from_ref(&bases))?;
        let prepare = module_attr(_interp, "prepare_class")?;
        let prepared = call_pos(
            _interp,
            prepare,
            &[name.clone(), resolved_bases.clone(), kwds],
        )?;
        let [meta, ns, kwds] = <[Value; 3]>::try_from(_interp.collect_iterable(&prepared)?)
            .map_err(|_| PyError::runtime("types: prepare_class must return a 3-tuple".into()))?;
        if !exec_body.is_none() {
            call_pos(_interp, exec_body, std::slice::from_ref(&ns))?;
        }
        if !is_same(_interp, &resolved_bases, &bases)? {
            setitem_value(_interp, ns.clone(), Value::string("__orig_bases__"), bases)?;
        }
        call_with_dict_kwargs(_interp, meta, &[name, resolved_bases, ns], &kwds)
    }

    /// Resolve MRO entries dynamically as specified by PEP 560.
    fn resolve_bases(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_RESOLVE_BASES, args)?;
        let bases = bound.take_or_none(0);
        resolve_bases_impl(_interp, &bases)
    }

    /// Call the __prepare__ method of the appropriate metaclass.
    ///
    /// Returns (metaclass, namespace, kwds) as a 3-tuple
    ///
    /// *metaclass* is the appropriate metaclass
    /// *namespace* is the prepared class namespace
    /// *kwds* is an updated copy of the passed in kwds argument with any
    /// 'metaclass' entry removed. If no kwds argument is passed in, this will
    /// be an empty dict.
    fn prepare_class(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_PREPARE_CLASS, args)?;
        let name = bound.take_or_none(0);
        let bases = bound.take_or(1, || Value::tuple(Vec::new()));
        let kwds = bound.take_or_none(2);
        prepare_class_impl(_interp, &name, &bases, &kwds)
    }

    /// Calculate the most derived metaclass.
    fn _calculate_meta(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_CALCULATE_META, args)?;
        let meta = bound.take_or_none(0);
        let bases = bound.take_or_none(1);
        calculate_meta_impl(_interp, &meta, &bases)
    }

    /// Return the class's "original" bases prior to modification by
    /// `__mro_entries__`.
    fn get_original_bases(args) -> Result<Value> {
        if let Some(named) = args.iter().find(|arg| arg.name.is_some()) {
            return Err(type_error(format!(
                "get_original_bases() got some positional-only arguments passed as keyword \
                 arguments: '{}'",
                named.name.as_deref().unwrap_or_default()
            )));
        }
        let mut bound = args::bind(
            &sig("get_original_bases", &["cls"], 0),
            args,
        )?;
        let cls = bound.take_or_none(0);
        let lookup = (|| -> Result<Value> {
            let class_dict = _interp.get_attr(&cls, "__dict__")?;
            let get = _interp.get_attr(&class_dict, "get")?;
            let fallback = _interp.get_attr(&cls, "__bases__")?;
            call_pos(_interp, get, &[Value::string("__orig_bases__"), fallback])
        })();
        match lookup {
            Err(ref error) if error.class_name_is("AttributeError") => {
                let type_name = value_class(&cls);
                let type_name = _interp.get_attr(&type_name, "__name__")?;
                let rendered = render_value_repr(_interp, &type_name)?;
                Err(type_error(format!(
                    "Expected an instance of type, not {rendered}"
                )))
            }
            other => other,
        }
    }

    /// Convert regular generator function to a coroutine.
    fn coroutine(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_COROUTINE, args)?;
        let func = bound.take_or_none(0);
        coroutine_impl(_interp, func)
    }

    // ── DynamicClassAttribute ────────────────────────────────────────────────

    class DynamicClassAttribute {
        constants {
            "__doc__" => Value::string(DYNAMIC_CLASS_ATTRIBUTE_DOC),
        }

        fn __init__(args) -> Result<Value> {
            dca_init(_interp, args)
        }

        fn __get__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_DCA_GET, args)?;
            let this = bound.take_or_none(0);
            let instance = bound.take_or_none(1);
            if instance.is_none() {
                let is_abstract = _interp.get_attr(&this, "__isabstractmethod__")?;
                if truthy(_interp, &is_abstract)? {
                    return Ok(this);
                }
                return Err(bare_attribute_error(_interp));
            }
            let fget = _interp.get_attr(&this, "fget")?;
            if fget.is_none() {
                return Err(attribute_error("unreadable attribute"));
            }
            call_pos(_interp, fget, &[instance])
        }

        fn __set__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_DCA_SET, args)?;
            let this = bound.take_or_none(0);
            let instance = bound.take_or_none(1);
            let value = bound.take_or_none(2);
            let fset = _interp.get_attr(&this, "fset")?;
            if fset.is_none() {
                return Err(attribute_error("can't set attribute"));
            }
            call_pos(_interp, fset, &[instance, value])?;
            Ok(Value::none())
        }

        fn __delete__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_DCA_DELETE, args)?;
            let this = bound.take_or_none(0);
            let instance = bound.take_or_none(1);
            let fdel = _interp.get_attr(&this, "fdel")?;
            if fdel.is_none() {
                return Err(attribute_error("can't delete attribute"));
            }
            call_pos(_interp, fdel, &[instance])?;
            Ok(Value::none())
        }

        fn getter(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_DCA_GETTER, args)?;
            let this = bound.take_or_none(0);
            let fget = bound.take_or_none(1);
            let overwrite = _interp.get_attr(&this, "overwrite_doc")?;
            let fdoc = if truthy(_interp, &overwrite)? {
                _interp.get_attr(&fget, "__doc__")?
            } else {
                Value::none()
            };
            let doc = if truthy(_interp, &fdoc)? {
                fdoc
            } else {
                _interp.get_attr(&this, "__doc__")?
            };
            let fset = _interp.get_attr(&this, "fset")?;
            let fdel = _interp.get_attr(&this, "fdel")?;
            dca_copy(_interp, &this, &[fget, fset, fdel, doc])
        }

        fn setter(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_DCA_SETTER, args)?;
            let this = bound.take_or_none(0);
            let fset = bound.take_or_none(1);
            let fget = _interp.get_attr(&this, "fget")?;
            let fdel = _interp.get_attr(&this, "fdel")?;
            let doc = _interp.get_attr(&this, "__doc__")?;
            dca_copy(_interp, &this, &[fget, fset, fdel, doc])
        }

        fn deleter(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_DCA_DELETER, args)?;
            let this = bound.take_or_none(0);
            let fdel = bound.take_or_none(1);
            let fget = _interp.get_attr(&this, "fget")?;
            let fset = _interp.get_attr(&this, "fset")?;
            let doc = _interp.get_attr(&this, "__doc__")?;
            dca_copy(_interp, &this, &[fget, fset, fdel, doc])
        }
    }

    // ── _GeneratorWrapper ────────────────────────────────────────────────────

    class _GeneratorWrapper {
        fn __init__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_GW_INIT, args)?;
            let this = bound.take_or_none(0);
            let gen_value = bound.take_or_none(1);
            _interp.assign_attr(this.clone(), "_GeneratorWrapper__wrapped", gen_value.clone())?;
            let generator_type = module_attr(_interp, "GeneratorType")?;
            let gen_class = dunder_class(_interp, &gen_value)?;
            let is_gen = is_same(_interp, &gen_class, &generator_type)?;
            _interp.assign_attr(this.clone(), "_GeneratorWrapper__isgen", Value::bool_(is_gen))?;
            let name = get_attr_or(_interp, &gen_value, "__name__", Value::none())?;
            _interp.assign_attr(this.clone(), "__name__", name)?;
            let qualname = get_attr_or(_interp, &gen_value, "__qualname__", Value::none())?;
            _interp.assign_attr(this, "__qualname__", qualname)?;
            Ok(Value::none())
        }

        fn send(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_GW_SEND, args)?;
            let this = bound.take_or_none(0);
            let val = bound.take_or_none(1);
            let wrapped = _interp.get_attr(&this, "_GeneratorWrapper__wrapped")?;
            call_method(_interp, &wrapped, "send", &[val])
        }

        fn throw(args) -> Result<Value> {
            // `def throw(self, tp, *rest)`: bind `self` / `tp` (by position or
            // keyword) and forward every further positional argument.
            let mut head: Vec<ExpandedCallArg> = Vec::new();
            let mut rest: Vec<Value> = Vec::new();
            let mut positional = 0usize;
            for arg in args {
                if arg.name.is_none() {
                    positional += 1;
                    if positional > 2 {
                        rest.push(arg.value.clone());
                        continue;
                    }
                }
                head.push(arg.clone());
            }
            let mut bound = args::bind(&SIG_GW_THROW, &head)?;
            let this = bound.take_or_none(0);
            let tp = bound.take_or_none(1);
            let wrapped = _interp.get_attr(&this, "_GeneratorWrapper__wrapped")?;
            let throw = _interp.get_attr(&wrapped, "throw")?;
            let mut call_args = vec![tp];
            call_args.extend(rest);
            call_pos(_interp, throw, &call_args)
        }

        fn close(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_GW_CLOSE, args)?;
            let this = bound.take_or_none(0);
            let wrapped = _interp.get_attr(&this, "_GeneratorWrapper__wrapped")?;
            call_method(_interp, &wrapped, "close", &[])
        }

        #[getter]
        fn gi_code(#[positional_only] this: PyValueRef<'_>) -> Result<Value> {
            let wrapped = _interp.get_attr(this.0, "_GeneratorWrapper__wrapped")?;
            _interp.get_attr(&wrapped, "gi_code")
        }

        #[getter]
        fn gi_frame(#[positional_only] this: PyValueRef<'_>) -> Result<Value> {
            let wrapped = _interp.get_attr(this.0, "_GeneratorWrapper__wrapped")?;
            _interp.get_attr(&wrapped, "gi_frame")
        }

        #[getter]
        fn gi_running(#[positional_only] this: PyValueRef<'_>) -> Result<Value> {
            let wrapped = _interp.get_attr(this.0, "_GeneratorWrapper__wrapped")?;
            _interp.get_attr(&wrapped, "gi_running")
        }

        #[getter]
        fn gi_yieldfrom(#[positional_only] this: PyValueRef<'_>) -> Result<Value> {
            let wrapped = _interp.get_attr(this.0, "_GeneratorWrapper__wrapped")?;
            _interp.get_attr(&wrapped, "gi_yieldfrom")
        }

        #[getter]
        fn cr_code(#[positional_only] this: PyValueRef<'_>) -> Result<Value> {
            let wrapped = _interp.get_attr(this.0, "_GeneratorWrapper__wrapped")?;
            _interp.get_attr(&wrapped, "gi_code")
        }

        #[getter]
        fn cr_frame(#[positional_only] this: PyValueRef<'_>) -> Result<Value> {
            let wrapped = _interp.get_attr(this.0, "_GeneratorWrapper__wrapped")?;
            _interp.get_attr(&wrapped, "gi_frame")
        }

        #[getter]
        fn cr_running(#[positional_only] this: PyValueRef<'_>) -> Result<Value> {
            let wrapped = _interp.get_attr(this.0, "_GeneratorWrapper__wrapped")?;
            _interp.get_attr(&wrapped, "gi_running")
        }

        #[getter]
        fn cr_await(#[positional_only] this: PyValueRef<'_>) -> Result<Value> {
            let wrapped = _interp.get_attr(this.0, "_GeneratorWrapper__wrapped")?;
            _interp.get_attr(&wrapped, "gi_yieldfrom")
        }

        fn __next__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_GW_NEXT, args)?;
            let this = bound.take_or_none(0);
            let wrapped = _interp.get_attr(&this, "_GeneratorWrapper__wrapped")?;
            call_builtin(_interp, "next", &[wrapped])
        }

        fn __iter__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_GW_ITER, args)?;
            let this = bound.take_or_none(0);
            let is_gen = _interp.get_attr(&this, "_GeneratorWrapper__isgen")?;
            if truthy(_interp, &is_gen)? {
                return _interp.get_attr(&this, "_GeneratorWrapper__wrapped");
            }
            Ok(this)
        }

        fn __await__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_GW_AWAIT, args)?;
            let this = bound.take_or_none(0);
            let is_gen = _interp.get_attr(&this, "_GeneratorWrapper__isgen")?;
            if truthy(_interp, &is_gen)? {
                return _interp.get_attr(&this, "_GeneratorWrapper__wrapped");
            }
            Ok(this)
        }
    }

    // ── The hidden callable `coroutine` returns for non-generator functions ──

    #[native_state(CoroutineWrapperState)]
    class _CoroutineWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                CoroutineWrapperState {
                    func: func.0,
                    bound: None,
                },
            )?;
            Ok(Value::none())
        }

        /// `wrapped(*args, **kwargs)`.
        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] call_args: &[Value],
            #[varkwargs] call_kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let (func, bound) =
                with_state(self_val.0, |st| Ok((st.func.clone(), st.bound.clone())))?;
            let mut expanded: Vec<ExpandedCallArg> = Vec::new();
            if let Some(receiver) = bound {
                expanded.push(ExpandedCallArg { name: None, value: receiver });
            }
            expanded.extend(
                call_args
                    .iter()
                    .cloned()
                    .map(|value| ExpandedCallArg { name: None, value }),
            );
            expanded.extend(call_kwargs.iter().map(|(name, value)| ExpandedCallArg {
                name: Some(name.clone()),
                value: value.clone(),
            }));
            let coro = _interp.call_function_expanded(func, &expanded)?;
            wrap_coroutine_result(_interp, coro)
        }

        /// Decorated callables bind as methods like the function CPython
        /// returns.
        fn __get__(
            #[positional_only] self_val: PyValueRef<'_>,
            obj: PyValue,
            #[default(OptionalArg::Omitted)] _objtype: OptValueRef<'_>,
        ) -> Result<Value> {
            if obj.0.is_none() {
                return Ok(self_val.0.clone());
            }
            let ValueKind::PyInstance(instance) = self_val.0.kind() else {
                return Ok(self_val.0.clone());
            };
            let func = with_state(self_val.0, |st| Ok(st.func.clone()))?;
            let (class, attrs): (Rc<RefCell<PyClass>>, InstanceAttrs) = {
                let borrowed = instance.borrow();
                (Rc::clone(&borrowed.class), borrowed.attrs.clone())
            };
            let bound_instance =
                Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })));
            set_state(
                &bound_instance,
                CoroutineWrapperState {
                    func,
                    bound: Some(obj.0),
                },
            )?;
            Ok(bound_instance)
        }
    }

    // ── SimpleNamespace ──────────────────────────────────────────────────────

    class SimpleNamespace {
        constants {
            "__doc__" => Value::string(SIMPLE_NAMESPACE_DOC),
            "__hash__" => Value::none(),
        }

        fn __init__(args) -> Result<Value> {
            let this = self_of(args, "SimpleNamespace.__init__")?;
            if args.iter().filter(|arg| arg.name.is_none()).count() > 1 {
                return Err(type_error("no positional arguments expected"));
            }
            let dict = _interp.get_attr(&this, "__dict__")?;
            for arg in args.iter().skip(1) {
                if let Some(name) = &arg.name {
                    setitem_value(
                        _interp,
                        dict.clone(),
                        Value::string(name),
                        arg.value.clone(),
                    )?;
                }
            }
            Ok(Value::none())
        }

        fn __repr__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_NS_SELF, args)?;
            let this = bound.take_or_none(0);
            namespace_repr(_interp, &this)
        }

        fn __eq__(args) -> Result<Value> {
            namespace_compare(_interp, args, BinaryOp::Eq)
        }

        fn __ne__(args) -> Result<Value> {
            namespace_compare(_interp, args, BinaryOp::Ne)
        }

        fn __lt__(args) -> Result<Value> {
            namespace_compare(_interp, args, BinaryOp::Lt)
        }

        fn __le__(args) -> Result<Value> {
            namespace_compare(_interp, args, BinaryOp::Le)
        }

        fn __gt__(args) -> Result<Value> {
            namespace_compare(_interp, args, BinaryOp::Gt)
        }

        fn __ge__(args) -> Result<Value> {
            namespace_compare(_interp, args, BinaryOp::Ge)
        }

        fn __reduce__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_NS_SELF, args)?;
            let this = bound.take_or_none(0);
            // CPython returns the live `ns.__dict__`; PyRust's instance
            // `__dict__` is an identity-proxied view that `copy` / `pickle`
            // state application does not merge, so hand over a real dict.
            let dict = _interp.get_attr(&this, "__dict__")?;
            let dict = call_method(_interp, &dict, "copy", &[])?;
            Ok(Value::tuple(vec![
                value_class(&this),
                Value::tuple(Vec::new()),
                dict,
            ]))
        }
    }
}

/// Post-import wiring: tag `SimpleNamespace` with CPython's static-type name
/// (`types.SimpleNamespace`) for error messages and hide the `_CoroutineWrapper`
/// helper class from the module namespace.
pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let namespace = module.borrow().get_attr_value("SimpleNamespace");
    if let Some(value) = namespace
        && let ValueKind::PyClass(class) = value.kind()
    {
        class.borrow_mut().error_name = Some("types.SimpleNamespace");
    }
    let wrapper = module.borrow().get_attr_value(COROUTINE_WRAPPER_CLASS);
    if let Some(value) = wrapper
        && let ValueKind::PyClass(class) = value.kind()
    {
        COROUTINE_WRAPPER.with(|slot| *slot.borrow_mut() = Some(Rc::clone(class)));
    }
    module
        .borrow_mut()
        .attrs
        .shift_remove(COROUTINE_WRAPPER_CLASS);
    Ok(())
}
