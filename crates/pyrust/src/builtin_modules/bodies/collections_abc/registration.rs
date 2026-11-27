//! ABC inheritance hierarchy wiring, virtual primitive registrations, and
//! Python mixin injection for `collections.abc`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    Interpreter, function_type_singleton, method_type_singleton, object_class_singleton,
    primitive_class_by_name, range_class_singleton, type_class_singleton,
};
use crate::value::{PyClass, PyKey, Value, ValueKind};

use super::definitions::{
    Abc, are_mixins_installed, is_registered, mark_mixins_installed, mark_registered,
    register_virtual_subclass,
};

const COLLECTIONS_ABC_PY_SOURCE: &str = include_str!("../collections_abc_py.py");

const ABC_INSTANCECHECK_FN: &str = "collections.abc.__instancecheck__";
const ABC_SUBCLASSHOOK_FN: &str = "collections.abc.__subclasshook__";
const ABC_SUBCLASSCHECK_FN: &str = "collections.abc.__subclasscheck__";

/// Ensure the canonical ABC hierarchy and virtual registrations have been
/// initialised once on this thread.
pub(super) fn ensure_abc_initialized() {
    if !is_registered() {
        init_abc_hierarchy_and_registry();
        mark_registered();
    }
}

/// Trigger one-time initialisation and return the canonical `Container` ABC.
pub(super) fn make_abc_module() -> Value {
    ensure_abc_initialized();
    Value::py_class(Abc::Container.class())
}

/// Register a class (e.g. `collections.deque`) as a virtual subclass of
/// `collections.abc.MutableSequence`.
pub(crate) fn register_virtual_mutable_sequence(class: &Rc<RefCell<PyClass>>) {
    ensure_abc_initialized();
    let mut_seq = Abc::MutableSequence.class();
    register_virtual_subclass(&mut_seq, class);
}

/// Register a class (e.g. `weakref.WeakSet`) as a virtual subclass of
/// `collections.abc.MutableSet`.
pub(crate) fn register_virtual_mutable_set(class: &Rc<RefCell<PyClass>>) {
    ensure_abc_initialized();
    let mut_set = Abc::MutableSet.class();
    register_virtual_subclass(&mut_set, class);
}

fn set_bases(sub: &Rc<RefCell<PyClass>>, bases: &[&Rc<RefCell<PyClass>>]) {
    let Some((first, rest)) = bases.split_first() else {
        return;
    };
    {
        let mut borrowed = sub.borrow_mut();
        borrowed.base = Some(Rc::clone(first));
        borrowed.extra_bases = rest.iter().map(|b| Rc::clone(b)).collect();
    }
    for parent in bases {
        let parent_borrowed = parent.borrow();
        let mut subs = parent_borrowed.subclasses.borrow_mut();
        if !subs.iter().any(|w| {
            w.upgrade()
                .is_some_and(|existing| Rc::ptr_eq(&existing, sub))
        }) {
            subs.push(Rc::downgrade(sub));
        }
    }
}

fn add_dunder_methods(abc: &Rc<RefCell<PyClass>>) {
    let mut borrowed = abc.borrow_mut();
    for (dunder, target) in [
        ("__instancecheck__", ABC_INSTANCECHECK_FN),
        ("__subclasshook__", ABC_SUBCLASSHOOK_FN),
        ("__subclasscheck__", ABC_SUBCLASSCHECK_FN),
    ] {
        borrowed.attrs.entry(dunder.to_string()).or_insert_with(|| {
            pyrust_builtins::classmethod::class_method_any(Value::builtin_function(target))
        });
    }
}

fn init_abc_hierarchy_and_registry() {
    let container = Abc::Container.class();
    let hashable = Abc::Hashable.class();
    let iterable = Abc::Iterable.class();
    let iterator = Abc::Iterator.class();
    let reversible = Abc::Reversible.class();
    let generator = Abc::Generator.class();
    let sized = Abc::Sized.class();
    let collection = Abc::Collection.class();
    let callable = Abc::Callable.class();
    let sequence = Abc::Sequence.class();
    let mut_sequence = Abc::MutableSequence.class();
    let set_abc = Abc::Set.class();
    let mut_set = Abc::MutableSet.class();
    let mapping = Abc::Mapping.class();
    let mut_mapping = Abc::MutableMapping.class();
    let mapping_view = Abc::MappingView.class();
    let keys_view = Abc::KeysView.class();
    let items_view = Abc::ItemsView.class();
    let values_view = Abc::ValuesView.class();
    let awaitable = Abc::Awaitable.class();
    let coroutine = Abc::Coroutine.class();
    let async_iterable = Abc::AsyncIterable.class();
    let async_iterator = Abc::AsyncIterator.class();
    let async_generator = Abc::AsyncGenerator.class();
    let buffer_abc = Abc::Buffer.class();
    let byte_string = Abc::ByteString.class();

    // Real ABC-to-ABC inheritance hierarchy (CPython 3.12).
    set_bases(&coroutine, &[&awaitable]);
    set_bases(&async_iterator, &[&async_iterable]);
    set_bases(&async_generator, &[&async_iterator]);
    set_bases(&iterator, &[&iterable]);
    set_bases(&reversible, &[&iterable]);
    set_bases(&generator, &[&iterator]);
    set_bases(&collection, &[&sized, &iterable, &container]);
    set_bases(&set_abc, &[&collection]);
    set_bases(&mut_set, &[&set_abc]);
    set_bases(&mapping, &[&collection]);
    set_bases(&mut_mapping, &[&mapping]);
    set_bases(&mapping_view, &[&sized]);
    set_bases(&keys_view, &[&mapping_view, &set_abc]);
    set_bases(&items_view, &[&mapping_view, &set_abc]);
    set_bases(&values_view, &[&mapping_view, &collection]);
    set_bases(&sequence, &[&reversible, &collection]);
    set_bases(&mut_sequence, &[&sequence]);
    // Deprecated since 3.12 (slated for removal in 3.14) but still exported;
    // `typing.ByteString` uses it as its origin.
    set_bases(&byte_string, &[&sequence]);

    // CPython defines `__class_getitem__ = classmethod(GenericAlias)` directly
    // on these ABCs; every other generic ABC (`Sequence`, `Mapping`, …) and
    // their subclasses inherit it, so `Sequence[int]` is a `types.GenericAlias`
    // (issue #3326). `Hashable`, `Sized`, and `Buffer` stay unsubscriptable;
    // `Callable` needs CPython's dedicated `_CallableGenericAlias`.
    for (abc, sentinel) in [
        (&awaitable, "Awaitable.__class_getitem__"),
        (&async_iterable, "AsyncIterable.__class_getitem__"),
        (&iterable, "Iterable.__class_getitem__"),
        (&container, "Container.__class_getitem__"),
        (&mapping_view, "MappingView.__class_getitem__"),
    ] {
        abc.borrow_mut().attrs.insert(
            "__class_getitem__".to_string(),
            Value::builtin_function(sentinel),
        );
    }

    pyrust_builtins::abc_marker::register(&mapping, pyrust_builtins::abc_marker::AbcKind::Mapping);

    for abc in Abc::ALL {
        add_dunder_methods(&abc.class());
    }

    // Virtual subclass registrations (CPython Lib/_collections_abc.py).
    let _obj_cls = object_class_singleton();
    let fn_type = function_type_singleton();
    let meth_type = method_type_singleton();
    let type_cls = type_class_singleton();
    let range_cls = range_class_singleton();

    let reg_prim = |abc: &Rc<RefCell<PyClass>>, name: &str| {
        if let Some(cls) = primitive_class_by_name(name) {
            register_virtual_subclass(abc, &cls);
        }
    };

    reg_prim(&mut_mapping, "dict");
    reg_prim(&mapping, "mappingproxy");
    reg_prim(&reversible, "dict");
    reg_prim(&reversible, "mappingproxy");
    reg_prim(&mut_sequence, "list");
    reg_prim(&mut_sequence, "bytearray");
    reg_prim(&sequence, "tuple");
    reg_prim(&sequence, "str");
    reg_prim(&sequence, "bytes");
    reg_prim(&sequence, "memoryview");
    register_virtual_subclass(&sequence, &range_cls);
    reg_prim(&byte_string, "bytes");
    reg_prim(&byte_string, "bytearray");
    reg_prim(&buffer_abc, "bytes");
    reg_prim(&buffer_abc, "bytearray");
    reg_prim(&buffer_abc, "memoryview");
    reg_prim(&mut_set, "set");
    reg_prim(&set_abc, "frozenset");

    for name in [
        "int",
        "float",
        "complex",
        "str",
        "bytes",
        "memoryview",
        "tuple",
        "frozenset",
        "bool",
        "NoneType",
        "NotImplementedType",
        "ellipsis",
    ] {
        reg_prim(&hashable, name);
    }
    register_virtual_subclass(&hashable, &range_cls);
    register_virtual_subclass(&hashable, &fn_type);
    register_virtual_subclass(&hashable, &meth_type);
    register_virtual_subclass(&hashable, &type_cls);

    register_virtual_subclass(&callable, &fn_type);
    register_virtual_subclass(&callable, &meth_type);
    register_virtual_subclass(&callable, &type_cls);
}

const MIXIN_TARGETS: [Abc; 12] = [
    Abc::Iterator,
    Abc::Generator,
    Abc::MappingView,
    Abc::KeysView,
    Abc::ItemsView,
    Abc::ValuesView,
    Abc::Mapping,
    Abc::MutableMapping,
    Abc::Set,
    Abc::MutableSet,
    Abc::Sequence,
    Abc::MutableSequence,
];

/// Populate all canonical `collections.abc` ABC singletons onto `module`
/// (used by `_collections_abc` without eagerly importing `collections.abc`).
pub(crate) fn populate_abc_exports(module: &Rc<RefCell<crate::value::PyModule>>) {
    ensure_abc_initialized();
    let mut m = module.borrow_mut();
    for abc in Abc::ALL {
        m.insert_attr(abc.name().to_string(), Value::py_class(abc.class()));
    }
}

/// Install the concrete Python mixin methods from `collections_abc_py.py` onto
/// the canonical `collections.abc` `PyClass` singletons once per thread.
pub(crate) fn ensure_mixins_initialized(interp: &mut Interpreter) -> Result<()> {
    ensure_abc_initialized();
    if are_mixins_installed() {
        return Ok(());
    }

    let ns = Value::dict(crate::value::PyDict::default());
    ns.dict_insert(
        PyKey::str_from("__name__"),
        Value::string("collections.abc"),
    )?;
    interp.exec_source(COLLECTIONS_ABC_PY_SOURCE, Some(ns.clone()), None)?;

    {
        let dict = ns
            .as_dict()
            .ok_or_else(|| PyError::runtime("collections.abc: exec namespace not a dict".into()))?;
        for abc in MIXIN_TARGETS {
            if let Some(carrier_val) = dict.get(&PyKey::str_from(abc.name()))
                && let ValueKind::PyClass(carrier_cls) = carrier_val.kind()
            {
                let copied: Vec<(String, Value)> = carrier_cls
                    .borrow()
                    .attrs
                    .iter()
                    .filter(|(k, _)| {
                        !matches!(
                            k.as_str(),
                            "__module__"
                                | "__doc__"
                                | "__dict__"
                                | "__weakref__"
                                | "__annotations__"
                        )
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let target = abc.class();
                let mut target_mut = target.borrow_mut();
                for (k, v) in copied {
                    target_mut.attrs.insert(k, v);
                }
                target_mut.bump_mutation_version();
            }
        }
    }

    // Rebind every ABC name in the exec namespace to the canonical singleton so
    // mixin bodies resolve the real canonical classes.
    for abc in Abc::ALL {
        ns.dict_insert(PyKey::str_from(abc.name()), Value::py_class(abc.class()))?;
    }

    mark_mixins_installed();
    Ok(())
}

/// Post-load `@inject` entry point for `collections.abc`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    _module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    ensure_mixins_initialized(interp)?;
    Ok(None)
}
