// `collections.abc` module — abstract base class stubs.
//
// This module exposes the standard ABC names from `collections.abc`
// as `PyClass` objects so that:
//
//   from collections.abc import Sequence, Mapping, Iterable, ...
//
// works without ImportError, and `isinstance([], Sequence)` etc.
// return the same values as CPython 3.12.
//
// ## Design
//
// Each ABC is a per-thread `PyClass` singleton with no methods.
// `isinstance` checks work because on first module load we register
// each ABC as an `extra_base` on the relevant primitive class(es).
// `class_is_subclass_of` walks `base` and `extra_bases`, so
// `isinstance([], Sequence)` → `list`'s extra_bases contains
// `Sequence` → `True`.
//
// The `Callable` ABC is special: it covers user functions, bound
// methods, classes (callable via `type`), and built-in functions
// (`len`, `print`).  We register it on `function_type_singleton()`,
// `method_type_singleton()`, and `type_class_singleton()`.
// `isinstance_single` has an extra arm that fires when the expected
// class's name is "Callable" and the value is a builtin-function kind.
//
// ## Registration
//
// `register_abc_extra_bases()` is called once per thread from
// `make_abc_module()` (the expression in the `constants` block).
// It adds each ABC Rc to the `extra_bases` of the relevant primitive
// class singletons so subsequent `isinstance` calls work without any
// additional dispatch overhead.
//
// Reference: <https://docs.python.org/3/library/collections.abc.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::value::{PyClass, Value};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

// ── ABC class singletons ──────────────────────────────────────────────────────
//
// One thread-local per ABC.  Each is an empty PyClass with no methods —
// `isinstance` works purely through the `extra_bases` registration path.

macro_rules! abc_class {
    ($name:expr) => {
        Rc::new(RefCell::new(PyClass {
            name: $name.to_string(),
            qualname: $name.to_string(),
            base: None,
            extra_bases: vec![],
            attrs: IndexMap::new(),
            mutation_version: std::cell::Cell::new(0),
            subclasses: std::cell::RefCell::new(vec![]),
            metatype: None,
        }))
    };
}

thread_local! {
    static ABC_CONTAINER:        Rc<RefCell<PyClass>> = abc_class!("Container");
    static ABC_HASHABLE:         Rc<RefCell<PyClass>> = abc_class!("Hashable");
    static ABC_ITERABLE:         Rc<RefCell<PyClass>> = abc_class!("Iterable");
    static ABC_ITERATOR:         Rc<RefCell<PyClass>> = abc_class!("Iterator");
    static ABC_REVERSIBLE:       Rc<RefCell<PyClass>> = abc_class!("Reversible");
    static ABC_GENERATOR:        Rc<RefCell<PyClass>> = abc_class!("Generator");
    static ABC_SIZED:            Rc<RefCell<PyClass>> = abc_class!("Sized");
    static ABC_CALLABLE:         Rc<RefCell<PyClass>> = abc_class!("Callable");
    static ABC_SEQUENCE:         Rc<RefCell<PyClass>> = abc_class!("Sequence");
    static ABC_MUTABLE_SEQUENCE: Rc<RefCell<PyClass>> = abc_class!("MutableSequence");
    static ABC_SET:              Rc<RefCell<PyClass>> = abc_class!("Set");
    static ABC_MUTABLE_SET:      Rc<RefCell<PyClass>> = abc_class!("MutableSet");
    static ABC_MAPPING:          Rc<RefCell<PyClass>> = abc_class!("Mapping");
    static ABC_MUTABLE_MAPPING:  Rc<RefCell<PyClass>> = abc_class!("MutableMapping");
    static ABC_MAPPING_VIEW:     Rc<RefCell<PyClass>> = abc_class!("MappingView");
    static ABC_KEYS_VIEW:        Rc<RefCell<PyClass>> = abc_class!("KeysView");
    static ABC_ITEMS_VIEW:       Rc<RefCell<PyClass>> = abc_class!("ItemsView");
    static ABC_VALUES_VIEW:      Rc<RefCell<PyClass>> = abc_class!("ValuesView");
    static ABC_AWAITABLE:        Rc<RefCell<PyClass>> = abc_class!("Awaitable");
    static ABC_COROUTINE:        Rc<RefCell<PyClass>> = abc_class!("Coroutine");
    static ABC_ASYNC_ITERABLE:   Rc<RefCell<PyClass>> = abc_class!("AsyncIterable");
    static ABC_ASYNC_ITERATOR:   Rc<RefCell<PyClass>> = abc_class!("AsyncIterator");
    static ABC_ASYNC_GENERATOR:  Rc<RefCell<PyClass>> = abc_class!("AsyncGenerator");
    static ABC_BUFFER:           Rc<RefCell<PyClass>> = abc_class!("Buffer");

    // Once-per-thread flag: `true` after `register_abc_extra_bases` has run.
    static ABC_REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Public accessor: return the Callable ABC PyClass singleton.
/// Used by `isinstance_single` in builtins.rs to handle
/// `isinstance(len, Callable)` for built-in function values.
pub(crate) fn callable_abc_class() -> Rc<RefCell<PyClass>> {
    ABC_CALLABLE.with(Rc::clone)
}

pyrust_module! {
    constants {
        "Container"        => make_abc_module(),
        "Hashable"         => Value::py_class(ABC_HASHABLE.with(Rc::clone)),
        "Iterable"         => Value::py_class(ABC_ITERABLE.with(Rc::clone)),
        "Iterator"         => Value::py_class(ABC_ITERATOR.with(Rc::clone)),
        "Reversible"       => Value::py_class(ABC_REVERSIBLE.with(Rc::clone)),
        "Generator"        => Value::py_class(ABC_GENERATOR.with(Rc::clone)),
        "Sized"            => Value::py_class(ABC_SIZED.with(Rc::clone)),
        "Callable"         => Value::py_class(ABC_CALLABLE.with(Rc::clone)),
        "Sequence"         => Value::py_class(ABC_SEQUENCE.with(Rc::clone)),
        "MutableSequence"  => Value::py_class(ABC_MUTABLE_SEQUENCE.with(Rc::clone)),
        "Set"              => Value::py_class(ABC_SET.with(Rc::clone)),
        "MutableSet"       => Value::py_class(ABC_MUTABLE_SET.with(Rc::clone)),
        "Mapping"          => Value::py_class(ABC_MAPPING.with(Rc::clone)),
        "MutableMapping"   => Value::py_class(ABC_MUTABLE_MAPPING.with(Rc::clone)),
        "MappingView"      => Value::py_class(ABC_MAPPING_VIEW.with(Rc::clone)),
        "KeysView"         => Value::py_class(ABC_KEYS_VIEW.with(Rc::clone)),
        "ItemsView"        => Value::py_class(ABC_ITEMS_VIEW.with(Rc::clone)),
        "ValuesView"       => Value::py_class(ABC_VALUES_VIEW.with(Rc::clone)),
        "Awaitable"        => Value::py_class(ABC_AWAITABLE.with(Rc::clone)),
        "Coroutine"        => Value::py_class(ABC_COROUTINE.with(Rc::clone)),
        "AsyncIterable"    => Value::py_class(ABC_ASYNC_ITERABLE.with(Rc::clone)),
        "AsyncIterator"    => Value::py_class(ABC_ASYNC_ITERATOR.with(Rc::clone)),
        "AsyncGenerator"   => Value::py_class(ABC_ASYNC_GENERATOR.with(Rc::clone)),
        "Buffer"           => Value::py_class(ABC_BUFFER.with(Rc::clone))
    }
}

// `make_abc_module` is called for the "Container" constant to trigger
// registration once.  It returns the Container PyClass value and, as a side
// effect, calls `register_abc_extra_bases()` if this is the first call on
// this thread.
fn make_abc_module() -> Value {
    if !ABC_REGISTERED.with(|f| f.get()) {
        register_abc_extra_bases();
        ABC_REGISTERED.with(|f| f.set(true));
    }
    Value::py_class(ABC_CONTAINER.with(Rc::clone))
}

/// Register each ABC as an `extra_base` on the relevant primitive class(es).
///
/// Called once per thread on first module load.  Mutates `extra_bases`
/// on the per-thread primitive class singletons so that
/// `class_is_subclass_of(list_class, sequence_class)` returns `true`.
///
/// CPython registration table (from Lib/_collections_abc.py):
///
///   Hashable:        int, float, complex, str, bytes, tuple, frozenset,
///                    bool, NoneType, NotImplementedType, ellipsis
///   Iterable:        str, bytes, list, tuple, dict, set, frozenset,
///                    range-like (not yet), bool(no)
///   Container:       str, bytes, list, tuple, dict, set, frozenset
///   Sized:           str, bytes, list, tuple, dict, set, frozenset
///   Reversible:      str, bytes, list, tuple (not set, frozenset, dict)
///   Sequence:        str, bytes, list, tuple
///   MutableSequence: list
///   Set:             set, frozenset
///   MutableSet:      set
///   Mapping:         dict
///   MutableMapping:  dict
///   Callable:        function, method, type  (+ BuiltinFunction — handled
///                    via name check in isinstance_single)
fn register_abc_extra_bases() {
    use crate::interpreter::{
        function_type_singleton, method_type_singleton, object_class_singleton,
        primitive_class_by_name, type_class_singleton,
    };

    // Helper: add `abc` to `prim`'s extra_bases, and add `prim` to
    // `abc`'s subclasses list so the relationship is discoverable.
    fn link(prim: &Rc<RefCell<PyClass>>, abc: &Rc<RefCell<PyClass>>) {
        // Avoid duplicates if called more than once (safety belt).
        let already = prim
            .borrow()
            .extra_bases
            .iter()
            .any(|b| Rc::ptr_eq(b, abc));
        if already {
            return;
        }
        prim.borrow_mut()
            .extra_bases
            .push(Rc::clone(abc));
        abc.borrow()
            .subclasses
            .borrow_mut()
            .push(Rc::downgrade(prim));
    }

    // Resolve ABCs once.
    let container       = ABC_CONTAINER.with(Rc::clone);
    let hashable        = ABC_HASHABLE.with(Rc::clone);
    let iterable        = ABC_ITERABLE.with(Rc::clone);
    let reversible      = ABC_REVERSIBLE.with(Rc::clone);
    let sized           = ABC_SIZED.with(Rc::clone);
    let callable        = ABC_CALLABLE.with(Rc::clone);
    let sequence        = ABC_SEQUENCE.with(Rc::clone);
    let mut_sequence    = ABC_MUTABLE_SEQUENCE.with(Rc::clone);
    let set_abc         = ABC_SET.with(Rc::clone);
    let mut_set         = ABC_MUTABLE_SET.with(Rc::clone);
    let mapping         = ABC_MAPPING.with(Rc::clone);
    let mut_mapping     = ABC_MUTABLE_MAPPING.with(Rc::clone);

    // Resolve primitive classes.
    macro_rules! prim {
        ($name:expr) => {
            match primitive_class_by_name($name) {
                Some(cls) => cls,
                None => return, // defensive; should never happen
            }
        };
    }

    let list_cls      = prim!("list");
    let tuple_cls     = prim!("tuple");
    let str_cls       = prim!("str");
    let bytes_cls     = prim!("bytes");
    let dict_cls      = prim!("dict");
    let set_cls       = prim!("set");
    let frozenset_cls = prim!("frozenset");
    let int_cls       = prim!("int");
    let float_cls     = prim!("float");
    let complex_cls   = prim!("complex");
    let bool_cls      = prim!("bool");
    let none_cls      = prim!("NoneType");

    // ── Sequence ────────────────────────────────────────────────────────────
    // list, tuple, str, bytes are Sequences.
    for cls in [&list_cls, &tuple_cls, &str_cls, &bytes_cls] {
        link(cls, &sequence);
    }

    // ── MutableSequence ─────────────────────────────────────────────────────
    // Only list.
    link(&list_cls, &mut_sequence);

    // ── Reversible ──────────────────────────────────────────────────────────
    // list, tuple, str, bytes, dict (not set, frozenset in CPython).
    // dict became Reversible in Python 3.8 (dict keys preserve insertion order).
    for cls in [&list_cls, &tuple_cls, &str_cls, &bytes_cls, &dict_cls] {
        link(cls, &reversible);
    }

    // ── Set ─────────────────────────────────────────────────────────────────
    // set and frozenset.
    for cls in [&set_cls, &frozenset_cls] {
        link(cls, &set_abc);
    }

    // ── MutableSet ──────────────────────────────────────────────────────────
    // Only set (frozenset is immutable).
    link(&set_cls, &mut_set);

    // ── Mapping ─────────────────────────────────────────────────────────────
    // dict.
    link(&dict_cls, &mapping);

    // ── MutableMapping ──────────────────────────────────────────────────────
    // dict.
    link(&dict_cls, &mut_mapping);

    // ── Container ───────────────────────────────────────────────────────────
    // str, bytes, list, tuple, dict, set, frozenset.
    for cls in [
        &str_cls, &bytes_cls, &list_cls, &tuple_cls,
        &dict_cls, &set_cls, &frozenset_cls,
    ] {
        link(cls, &container);
    }

    // ── Sized ───────────────────────────────────────────────────────────────
    // str, bytes, list, tuple, dict, set, frozenset.
    for cls in [
        &str_cls, &bytes_cls, &list_cls, &tuple_cls,
        &dict_cls, &set_cls, &frozenset_cls,
    ] {
        link(cls, &sized);
    }

    // ── Iterable ────────────────────────────────────────────────────────────
    // str, bytes, list, tuple, dict, set, frozenset.
    for cls in [
        &str_cls, &bytes_cls, &list_cls, &tuple_cls,
        &dict_cls, &set_cls, &frozenset_cls,
    ] {
        link(cls, &iterable);
    }

    // ── Hashable ────────────────────────────────────────────────────────────
    // int, float, complex, str, bytes, tuple, frozenset, bool, NoneType.
    // (list, dict, set are NOT hashable)
    for cls in [
        &int_cls, &float_cls, &complex_cls, &str_cls, &bytes_cls,
        &tuple_cls, &frozenset_cls, &bool_cls, &none_cls,
    ] {
        link(cls, &hashable);
    }

    // ── Callable ────────────────────────────────────────────────────────────
    // function, method, type.  BuiltinFunction values are handled via
    // name check in isinstance_single (builtins.rs).
    let fn_type  = function_type_singleton();
    let meth_type = method_type_singleton();
    let type_cls = type_class_singleton();
    let _obj_cls = object_class_singleton(); // ensure singleton initialised
    for cls in [&fn_type, &meth_type, &type_cls] {
        link(cls, &callable);
    }
}
