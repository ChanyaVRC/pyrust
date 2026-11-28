//! One-time wiring of the canonical ABCs and the two module namespaces.
//!
//! [`ensure_initialized`] adopts the classes of one `module()` build as the
//! canonical set and then does what `ABCMeta.__new__` and the module body of
//! `Lib/_collections_abc.py` do for them: install the metaclass, `__slots__`,
//! `_abc_impl`, the `@abstractmethod` descriptors and `__abstractmethods__`,
//! and every `register()` call, in CPython's order.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};

use crate::ast::BinaryOp;
use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, function_type_singleton, generic_alias_class_singleton,
    lookup_exc_class, make_iterator, object_class_singleton, type_class_singleton, value_class,
};
use crate::value::{PyDict, PyModule, Value, ValueKind};

use super::definitions::{self, Abc, Canonical, ClassRc, MODULE_DOC, canonical};
use super::support::{call, call_method};

/// State of an `@abstractmethod` descriptor wrapping a native method.
pub(crate) struct AbstractMethodState {
    pub(crate) func: Value,
    pub(crate) name: String,
    pub(crate) qualname: String,
}

/// State of an `@abstractmethod` bound to an instance.
pub(crate) struct BoundAbstractMethodState {
    pub(crate) func: Value,
    pub(crate) instance: Value,
}

// ── Canonical initialisation ─────────────────────────────────────────────────

/// Build and wire the canonical ABCs once per GIL group.
pub(crate) fn ensure_initialized(interp: &mut Interpreter) -> Result<()> {
    if definitions::is_initialized() {
        return Ok(());
    }
    let private_types = private_types(interp)?;
    let carrier = super::module();
    let ValueKind::PyModule(module) = carrier.kind() else {
        return Err(internal("module() did not build a module"));
    };
    let class_named = |name: &str| -> Result<ClassRc> {
        match module.borrow().attrs.get(name).map(Value::kind) {
            Some(ValueKind::PyClass(class)) => Ok(Rc::clone(class)),
            _ => Err(internal(&format!("missing class {name}"))),
        }
    };
    let classes = Abc::ALL
        .iter()
        .map(|abc| class_named(abc.name()))
        .collect::<Result<Vec<_>>>()?;
    let abstract_method = class_named("_AbstractMethod")?;
    let bound_abstract_method = class_named("_BoundAbstractMethod")?;
    let callable_alias = class_named("_CallableGenericAlias")?;
    let deprecate_meta = class_named("_DeprecateByteStringMeta")?;
    let marker = call(interp, Value::py_class(object_class_singleton()), &[])?;
    let all = Value::list(
        Abc::EXPORT_ORDER
            .iter()
            .map(|abc| Value::string(abc.name()))
            .collect(),
    );
    definitions::install(Canonical {
        classes,
        abstract_method,
        bound_abstract_method,
        callable_alias,
        deprecate_meta,
        marker,
        all,
        private_types,
    });
    wire_classes(interp)?;
    register_builtin_types(interp)?;
    Ok(())
}

fn internal(message: &str) -> PyError {
    PyError::runtime(format!("internal: collections.abc: {message}"))
}

/// Turn the declared classes into ABCs, in definition order so every base is
/// complete before its subclasses compute `__abstractmethods__`.
fn wire_classes(interp: &mut Interpreter) -> Result<()> {
    let abc_module = interp.load_module("abc")?;
    let abc_meta_value = interp.get_attr(&abc_module, "ABCMeta")?;
    let ValueKind::PyClass(abc_meta) = abc_meta_value.kind() else {
        return Err(internal("abc.ABCMeta is not a class"));
    };
    let abc_meta = Rc::clone(abc_meta);
    let state = canonical();

    // `class _DeprecateByteStringMeta(ABCMeta)`.
    link_class_bases(&state.deprecate_meta, vec![Rc::clone(&abc_meta)]);
    // `class _CallableGenericAlias(GenericAlias)`.
    link_class_bases(&state.callable_alias, vec![generic_alias_class_singleton()]);
    state.callable_alias.borrow_mut().slots = Some(IndexSet::new());
    pyrust_builtins::generic_alias::register_callable_alias_class(&state.callable_alias);

    for abc in Abc::ALL {
        let class = abc.class();
        {
            let mut cls = class.borrow_mut();
            cls.metatype = Some(if abc == Abc::ByteString {
                Rc::clone(&state.deprecate_meta)
            } else {
                Rc::clone(&abc_meta)
            });
            cls.slots = Some(abc.slots().iter().map(|slot| (*slot).to_string()).collect());
            cls.attrs.insert(
                "__doc__".to_string(),
                abc.doc().map_or_else(Value::none, Value::string),
            );
            cls.attrs.insert(
                "__slots__".to_string(),
                Value::tuple(
                    abc.slots()
                        .iter()
                        .map(|slot| Value::string(*slot))
                        .collect(),
                ),
            );
            cls.attrs.insert(
                "_abc_impl".to_string(),
                crate::builtin_modules::abc::new_abc_impl(),
            );
        }
        // `@classmethod` in a Python module: a plain `classmethod` object
        // (with `__func__`) that binds to a `method`, rather than the native
        // `classmethod_descriptor` the class macro installs.
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
        for slot in abc.slots() {
            let descriptor = pyrust_builtins::member_descriptor::member_descriptor(slot, &class);
            class
                .borrow_mut()
                .attrs
                .insert((*slot).to_string(), descriptor);
        }
        for &method in abc.own_abstract_methods() {
            let func = class.borrow().attrs.get(method).cloned();
            let Some(func) = func else {
                return Err(internal(&format!(
                    "{}.{method} is not declared",
                    abc.name()
                )));
            };
            let wrapped = call(
                interp,
                Value::py_class(Rc::clone(&state.abstract_method)),
                &[
                    func,
                    Value::string(method),
                    Value::string(format!("{}.{method}", abc.name())),
                ],
            )?;
            class.borrow_mut().attrs.insert(method.to_string(), wrapped);
        }
        match abc {
            Abc::Set => {
                // `__rand__ = __and__`, `__ror__ = __or__`, `__rxor__ = __xor__`.
                let mut cls = class.borrow_mut();
                for (alias, target) in [
                    ("__rand__", "__and__"),
                    ("__ror__", "__or__"),
                    ("__rxor__", "__xor__"),
                ] {
                    if let Some(method) = cls.attrs.get(target).cloned() {
                        cls.attrs.insert(alias.to_string(), method);
                    }
                }
            }
            Abc::MutableMapping => {
                class
                    .borrow_mut()
                    .attrs
                    .insert("_MutableMapping__marker".to_string(), state.marker.clone());
            }
            Abc::Mapping => {
                pyrust_builtins::abc_marker::register(
                    &class,
                    pyrust_builtins::abc_marker::AbcKind::Mapping,
                );
            }
            _ => {}
        }
        crate::builtin_modules::abc::compute_protocol_abstract_methods(interp, &class)?;
        class.borrow_mut().bump_mutation_version();
    }
    Ok(())
}

/// The type objects `_collections_abc` names at module level, computed the
/// way CPython does (`type(iter(b''))`, `type({}.keys())`, …).
fn private_types(interp: &mut Interpreter) -> Result<Vec<(&'static str, Value)>> {
    let builtin = Value::builtin_function;
    let empty_dict = || Value::dict(PyDict::default());
    let iter_type = |interp: &mut Interpreter, value: Value| -> Result<Value> {
        let iterator = make_iterator(interp, &value)?;
        Ok(value_class(&iterator))
    };
    let bytes_value = Value::bytes(Vec::new());
    let bytearray_value = call(interp, builtin("bytearray"), &[])?;
    let keys = call_method(interp, &empty_dict(), "keys", &[])?;
    let values = call_method(interp, &empty_dict(), "values", &[])?;
    let items = call_method(interp, &empty_dict(), "items", &[])?;
    let reversed_list = call(interp, builtin("reversed"), &[Value::list(Vec::new())])?;
    let empty_range = call(interp, builtin("range"), &[Value::int(0)])?;
    let huge = interp.eval_binary(Value::int(1), BinaryOp::LShift, Value::int(1000))?;
    let long_range = call(interp, builtin("range"), &[huge])?;
    let empty_set = call(interp, builtin("set"), &[])?;
    let empty_zip = call(interp, builtin("zip"), &[])?;
    let type_dict = interp.get_attr(&Value::py_class(type_class_singleton()), "__dict__")?;

    Ok(vec![
        ("bytes_iterator", iter_type(interp, bytes_value)?),
        ("bytearray_iterator", iter_type(interp, bytearray_value)?),
        ("dict_keyiterator", iter_type(interp, keys.clone())?),
        ("dict_valueiterator", iter_type(interp, values.clone())?),
        ("dict_itemiterator", iter_type(interp, items.clone())?),
        ("list_iterator", iter_type(interp, Value::list(Vec::new()))?),
        ("list_reverseiterator", iter_type(interp, reversed_list)?),
        ("range_iterator", iter_type(interp, empty_range)?),
        ("longrange_iterator", iter_type(interp, long_range)?),
        ("set_iterator", iter_type(interp, empty_set)?),
        ("str_iterator", iter_type(interp, Value::string(""))?),
        (
            "tuple_iterator",
            iter_type(interp, Value::tuple(Vec::new()))?,
        ),
        ("zip_iterator", iter_type(interp, empty_zip)?),
        ("dict_keys", value_class(&keys)),
        ("dict_values", value_class(&values)),
        ("dict_items", value_class(&items)),
        ("mappingproxy", value_class(&type_dict)),
        // Generator, coroutine, and async-generator objects report these
        // name tokens as their type.
        ("generator", builtin("generator")),
        ("coroutine", builtin("coroutine")),
        ("async_generator", builtin("async_generator")),
    ])
}

/// `ABC.register(subclass)` through `ABCMeta.register`.
fn register(interp: &mut Interpreter, abc: Abc, subclass: Value) -> Result<()> {
    call_method(interp, &abc.value(), "register", &[subclass])?;
    Ok(())
}

/// The registrations of `Lib/_collections_abc.py`, in source order.
fn register_builtin_types(interp: &mut Interpreter) -> Result<()> {
    let state = canonical();
    let private = |name: &str| -> Value {
        state
            .private_types
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(Value::none)
    };
    let builtin_class = |name: &'static str| -> Value {
        crate::interpreter::primitive_class_by_name(name)
            .map(Value::py_class)
            .unwrap_or_else(|| Value::builtin_function(name))
    };
    register(interp, Abc::Coroutine, private("coroutine"))?;
    register(interp, Abc::AsyncGenerator, private("async_generator"))?;
    for name in [
        "bytes_iterator",
        "bytearray_iterator",
        "dict_keyiterator",
        "dict_valueiterator",
        "dict_itemiterator",
        "list_iterator",
        "list_reverseiterator",
        "range_iterator",
        "longrange_iterator",
        "set_iterator",
        "str_iterator",
        "tuple_iterator",
        "zip_iterator",
    ] {
        register(interp, Abc::Iterator, private(name))?;
    }
    register(interp, Abc::Generator, private("generator"))?;
    register(interp, Abc::Set, builtin_class("frozenset"))?;
    register(interp, Abc::MutableSet, builtin_class("set"))?;
    register(interp, Abc::Mapping, private("mappingproxy"))?;
    register(interp, Abc::KeysView, private("dict_keys"))?;
    register(interp, Abc::ItemsView, private("dict_items"))?;
    register(interp, Abc::ValuesView, private("dict_values"))?;
    register(interp, Abc::MutableMapping, builtin_class("dict"))?;
    for name in ["tuple", "str", "range", "memoryview"] {
        register(interp, Abc::Sequence, builtin_class(name))?;
    }
    for name in ["bytes", "bytearray"] {
        register(interp, Abc::ByteString, builtin_class(name))?;
    }
    for name in ["list", "bytearray"] {
        register(interp, Abc::MutableSequence, builtin_class(name))?;
    }
    Ok(())
}

/// `collections.abc.MutableSequence.register(class)` for a native class
/// (`collections.deque`, `array.array`).
pub(crate) fn register_virtual_mutable_sequence(
    interp: &mut Interpreter,
    class: &ClassRc,
) -> Result<()> {
    ensure_initialized(interp)?;
    register(
        interp,
        Abc::MutableSequence,
        Value::py_class(Rc::clone(class)),
    )
}

/// `collections.abc.MutableSet.register(class)` for a native class
/// (`weakref.WeakSet`).
pub(crate) fn register_virtual_mutable_set(
    interp: &mut Interpreter,
    class: &ClassRc,
) -> Result<()> {
    ensure_initialized(interp)?;
    register(interp, Abc::MutableSet, Value::py_class(Rc::clone(class)))
}

// ── Module namespaces ────────────────────────────────────────────────────────

/// Names `pyrust_module!` builds that are not part of `collections.abc`.
const CARRIER_ONLY: [&str; 6] = [
    "_check_methods",
    "_is_param_expr",
    "_type_repr",
    "_AbstractMethod",
    "_BoundAbstractMethod",
    "_DeprecateByteStringMeta",
];

/// Finish `collections.abc`: export the canonical ABCs in CPython's order —
/// `Lib/collections/abc.py` is `from _collections_abc import *`, `__all__`,
/// and `_CallableGenericAlias` — and drop the carrier-only helpers.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    ensure_initialized(interp)?;
    let state = canonical();
    let mut module = module.borrow_mut();
    for name in CARRIER_ONLY {
        module.attrs.shift_remove(name);
    }
    for abc in Abc::ALL {
        module.attrs.shift_remove(abc.name());
    }
    module.attrs.shift_remove("_CallableGenericAlias");
    module.attrs.shift_remove("__all__");
    // `collections/abc.py` has no docstring.
    module.attrs.insert("__doc__".to_string(), Value::none());
    for abc in Abc::EXPORT_ORDER {
        module.attrs.insert(abc.name().to_string(), abc.value());
    }
    module
        .attrs
        .insert("__all__".to_string(), state.all.clone());
    module.attrs.insert(
        "_CallableGenericAlias".to_string(),
        Value::py_class(Rc::clone(&state.callable_alias)),
    );
    Ok(())
}

/// Populate `_collections_abc` the way its module body leaves it.
pub(crate) fn populate_private_module(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    ensure_initialized(interp)?;
    let state = canonical();
    let abc_module = interp.load_module("abc")?;
    let abc_meta = interp.get_attr(&abc_module, "ABCMeta")?;
    let abstractmethod = interp.get_attr(&abc_module, "abstractmethod")?;
    let sys = interp.load_module("sys")?;
    let function = |name: &'static str| Value::builtin_function(qualified(name));

    let mut entries: Vec<(String, Value)> = vec![
        // The module body rebinds `__name__ = "collections.abc"` (CPython
        // issue #19218) so its classes report that module.
        ("__name__".into(), Value::string("collections.abc")),
        ("__doc__".into(), Value::string(MODULE_DOC)),
        ("ABCMeta".into(), abc_meta),
        ("abstractmethod".into(), abstractmethod),
        ("sys".into(), sys),
        (
            "GenericAlias".into(),
            Value::py_class(generic_alias_class_singleton()),
        ),
        ("EllipsisType".into(), value_class(&Value::ellipsis())),
        (
            "FunctionType".into(),
            Value::py_class(function_type_singleton()),
        ),
        ("__all__".into(), state.all.clone()),
    ];
    for (name, value) in &state.private_types {
        entries.push(((*name).to_string(), value.clone()));
    }
    entries.push(("_check_methods".into(), function("_check_methods")));
    for abc in Abc::ALL {
        entries.push((abc.name().to_string(), abc.value()));
        match abc {
            Abc::Buffer => {
                entries.push((
                    "_CallableGenericAlias".into(),
                    Value::py_class(Rc::clone(&state.callable_alias)),
                ));
                entries.push(("_is_param_expr".into(), function("_is_param_expr")));
                entries.push(("_type_repr".into(), function("_type_repr")));
            }
            Abc::Sequence => entries.push((
                "_DeprecateByteStringMeta".into(),
                Value::py_class(Rc::clone(&state.deprecate_meta)),
            )),
            _ => {}
        }
    }
    let mut module = module.borrow_mut();
    for (name, value) in entries {
        module.attrs.shift_remove(&name);
        module.attrs.insert(name, value);
    }
    Ok(())
}

/// The registry name of a module-level function of this body.
fn qualified(name: &'static str) -> &'static str {
    match name {
        "_check_methods" => "collections.abc._check_methods",
        "_is_param_expr" => "collections.abc._is_param_expr",
        "_type_repr" => "collections.abc._type_repr",
        _ => name,
    }
}

// ── `@abstractmethod` descriptor support ─────────────────────────────────────

/// `_AbstractMethod.__get__(obj)`: a bound abstract method.
pub(crate) fn bind_abstract_method(
    _interp: &mut Interpreter,
    func: Value,
    instance: &Value,
) -> Result<Value> {
    let state = canonical();
    let bound = Value::py_instance(Rc::new(RefCell::new(crate::value::PyInstance {
        class: Rc::clone(&state.bound_abstract_method),
        attrs: crate::value::InstanceAttrs::new(),
    })));
    crate::builtin_modules::macros::native_state::set_native_state(
        &bound,
        BoundAbstractMethodState {
            func,
            instance: instance.clone(),
        },
    )?;
    Ok(bound)
}

/// `func(*([receiver] + args), **kwargs)`.
pub(crate) fn call_with(
    interp: &mut Interpreter,
    func: Value,
    receiver: Option<Value>,
    args: &[Value],
    kwargs: IndexMap<String, Value>,
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = Vec::with_capacity(args.len() + kwargs.len() + 1);
    if let Some(receiver) = receiver {
        expanded.push(ExpandedCallArg {
            name: None,
            value: receiver,
        });
    }
    expanded.extend(
        args.iter()
            .cloned()
            .map(|value| ExpandedCallArg { name: None, value }),
    );
    expanded.extend(kwargs.into_iter().map(|(name, value)| ExpandedCallArg {
        name: Some(name),
        value,
    }));
    interp.call_function_expanded(func, &expanded)
}

// ── `_DeprecateByteStringMeta` ───────────────────────────────────────────────

/// `warnings._deprecated("collections.abc.ByteString", remove=(3, 14))`.
fn warn_byte_string_deprecated(interp: &mut Interpreter) -> Result<()> {
    let warnings = interp.load_module("warnings")?;
    let warn = interp.get_attr(&warnings, "warn")?;
    let category = lookup_exc_class("DeprecationWarning")
        .map(Value::py_class)
        .ok_or_else(|| internal("DeprecationWarning is missing"))?;
    interp.call_function_expanded(
        warn,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string(
                    "'collections.abc.ByteString' is deprecated and slated for removal in \
                     Python 3.14",
                ),
            },
            ExpandedCallArg {
                name: None,
                value: category,
            },
            ExpandedCallArg {
                name: Some("stacklevel".into()),
                value: Value::int(1),
            },
        ],
    )?;
    Ok(())
}

/// `_DeprecateByteStringMeta.__new__(cls, name, bases, namespace, **kwargs)`.
pub(crate) fn deprecate_meta_new(
    interp: &mut Interpreter,
    mcls: &Value,
    args: &[Value],
    kwargs: IndexMap<String, Value>,
) -> Result<Value> {
    if args.first().and_then(Value::as_str) != Some("ByteString") {
        warn_byte_string_deprecated(interp)?;
    }
    let abc_module = interp.load_module("abc")?;
    let abc_meta = interp.get_attr(&abc_module, "ABCMeta")?;
    let new = interp.get_attr(&abc_meta, "__new__")?;
    let mut full = Vec::with_capacity(args.len() + 1);
    full.push(mcls.clone());
    full.extend_from_slice(args);
    call_with(interp, new, None, &full, kwargs)
}

/// `_DeprecateByteStringMeta.__instancecheck__(cls, instance)`.
pub(crate) fn deprecate_meta_instancecheck(
    interp: &mut Interpreter,
    cls: &Value,
    instance: &Value,
) -> Result<Value> {
    warn_byte_string_deprecated(interp)?;
    let abc_module = interp.load_module("abc")?;
    let abc_meta = interp.get_attr(&abc_module, "ABCMeta")?;
    let check = interp.get_attr(&abc_meta, "__instancecheck__")?;
    call(interp, check, &[cls.clone(), instance.clone()])
}
