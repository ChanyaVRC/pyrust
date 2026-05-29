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
// `isinstance` checks work through two mechanisms:
//
// 1. **Structural subtyping via `abc_subclasshook`**: when `isinstance(x, ABC)`
//    is called, `isinstance_single` calls `abc_subclasshook(abc_name, x)`.
//    For user-defined instances, this walks the class MRO checking whether
//    each required abstract method is present and non-None.  For built-in
//    values, a hardcoded protocol table covers the known dunders.
//    Returns `Some(true)`/`Some(false)` to short-circuit, `None` to fall
//    through to the `extra_bases` / MRO check.
//
//    This mirrors CPython's `__subclasshook__` + `_check_methods` mechanism
//    from `Lib/_collections_abc.py`.
//
// 2. **`extra_bases` registration** (legacy / complex ABCs): on first module
//    load, `register_abc_extra_bases()` links each ABC that lacks a direct
//    structural hook (Sequence, MutableSequence, Set, MutableSet, Mapping,
//    MutableMapping) into the `extra_bases` of the relevant primitive class
//    singletons.  The Callable ABC is also registered here for user-defined
//    functions and classes.
//
// ## Which ABCs have structural hooks (mirrors CPython)?
//
//   Hashable:       __hash__
//   Iterable:       __iter__
//   Iterator:       __iter__ + __next__
//   Reversible:     __reversed__ + __iter__
//   Generator:      __iter__ + __next__ + send + throw + close
//   Sized:          __len__
//   Container:      __contains__
//   Callable:       __call__
//   Buffer:         __buffer__
//   Awaitable:      __await__
//   Coroutine:      __await__ + send + throw + close
//   AsyncIterable:  __aiter__
//   AsyncIterator:  __anext__ + __aiter__
//   AsyncGenerator: __aiter__ + __anext__ + asend + athrow + aclose
//
// ## Primitive protocol table
//
//   type       __iter__ __len__ __contains__ __hash__ __reversed__ __call__
//   str          yes     yes      yes          yes       yes
//   bytes        yes     yes      yes          yes       yes
//   list         yes     yes      yes          no        yes
//   tuple        yes     yes      yes          yes       yes
//   dict         yes     yes      yes          no
//   set          yes     yes      yes          no
//   frozenset    yes     yes      yes          yes
//   int/bool     no      no       no           yes
//   float        no      no       no           yes
//   complex      no      no       no           yes
//   NoneType     no      no       no           yes
//   bytearray    yes     yes      yes          no        yes
//   range        yes     yes      yes          yes       yes
//   generator    yes     no       no           no
//   BuiltinFn    no      no       no           yes       yes
//   BoundMethod  no      no       no           yes       yes
//
// Reference: <https://docs.python.org/3/library/collections.abc.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::value::{PyClass, Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

// ── ABC class singletons ──────────────────────────────────────────────────────
//
// One thread-local per ABC.

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

/// Public accessor: return the Hashable ABC PyClass singleton.
/// Used by `isinstance_single` in builtins.rs to handle
/// `isinstance(len, Hashable)` for built-in function and bound-method values.
pub(crate) fn hashable_abc_class() -> Rc<RefCell<PyClass>> {
    ABC_HASHABLE.with(Rc::clone)
}

// ── Required-method table (mirrors CPython's __subclasshook__ impls) ──────────
//
// Each entry maps an ABC name to the slice of method names that must all be
// present and non-None on a class for `isinstance(x, ABC)` to return True.
// ABCs that lack a direct structural hook (Sequence, Mapping, Set, …) are NOT
// in this table; they fall through to the `extra_bases` / MRO path.

const ABC_REQUIRED_METHODS: &[(&str, &[&str])] = &[
    ("Hashable",        &["__hash__"]),
    ("Iterable",        &["__iter__"]),
    ("Iterator",        &["__iter__", "__next__"]),
    ("Reversible",      &["__reversed__", "__iter__"]),
    ("Generator",       &["__iter__", "__next__", "send", "throw", "close"]),
    ("Sized",           &["__len__"]),
    ("Container",       &["__contains__"]),
    ("Callable",        &["__call__"]),
    ("Buffer",          &["__buffer__"]),
    ("Awaitable",       &["__await__"]),
    ("Coroutine",       &["__await__", "send", "throw", "close"]),
    ("AsyncIterable",   &["__aiter__"]),
    ("AsyncIterator",   &["__anext__", "__aiter__"]),
    ("AsyncGenerator",  &["__aiter__", "__anext__", "asend", "athrow", "aclose"]),
];

/// Required methods for an ABC, or `None` if the ABC has no structural hook.
fn required_methods(abc_name: &str) -> Option<&'static [&'static str]> {
    ABC_REQUIRED_METHODS
        .iter()
        .find(|(name, _)| *name == abc_name)
        .map(|(_, methods)| *methods)
}

// ── Primitive protocol support table ─────────────────────────────────────────
//
// Maps (value type tag, dunder name) → bool.  Covers only the dunders that
// appear in `ABC_REQUIRED_METHODS`.  Used for non-instance values where there
// is no class MRO to walk.

fn primitive_value_has_dunder(value: &Value, dunder: &str) -> bool {
    match value.kind() {
        // str: iterable, sized, contains, hashable, reversed, no __call__
        ValueKind::Str(_) => matches!(
            dunder,
            "__iter__" | "__len__" | "__contains__" | "__hash__" | "__reversed__"
        ),

        // bytes: same as str
        ValueKind::Bytes(_) => matches!(
            dunder,
            "__iter__" | "__len__" | "__contains__" | "__hash__" | "__reversed__"
        ),

        // list: iterable, sized, contains, reversed, NOT hashable (__hash__ = None)
        ValueKind::List(_) => matches!(
            dunder,
            "__iter__" | "__len__" | "__contains__" | "__reversed__"
        ),

        // tuple: iterable, sized, contains, hashable, reversed
        ValueKind::Tuple(_) => matches!(
            dunder,
            "__iter__" | "__len__" | "__contains__" | "__hash__" | "__reversed__"
        ),

        // dict: iterable, sized, contains, reversed, NOT hashable
        ValueKind::Dict(_) => matches!(
            dunder,
            "__iter__" | "__len__" | "__contains__" | "__reversed__"
        ),

        // set: iterable, sized, contains, NOT hashable, NOT reversed
        ValueKind::Set(_) => matches!(dunder, "__iter__" | "__len__" | "__contains__"),

        // int (and BigInt): hashable only, not iterable/sized/contains
        ValueKind::Int(_) | ValueKind::BigInt(_) => dunder == "__hash__",

        // bool is a subclass of int — same protocol support
        ValueKind::Bool(_) => dunder == "__hash__",

        // float: hashable only
        ValueKind::Float(_) => dunder == "__hash__",

        // complex: hashable only
        ValueKind::Complex(_, _) => dunder == "__hash__",

        // None: hashable only (NoneType is hashable in CPython)
        ValueKind::None => dunder == "__hash__",

        // NotImplemented, Ellipsis: hashable
        ValueKind::NotImplemented | ValueKind::Ellipsis => dunder == "__hash__",

        // range: iterable, sized, contains, hashable, reversed
        ValueKind::Range { .. } => matches!(
            dunder,
            "__iter__" | "__len__" | "__contains__" | "__hash__" | "__reversed__"
        ),

        // generator objects: have __iter__, __next__, send, throw, close
        ValueKind::Generator(_) => matches!(
            dunder,
            "__iter__" | "__next__" | "send" | "throw" | "close"
        ),

        // BuiltinObject variants
        ValueKind::BuiltinObject { ops, .. } => {
            let type_name = ops.type_name();
            if type_name == "bytearray" {
                // bytearray: iterable, sized, contains, reversed, NOT hashable
                matches!(
                    dunder,
                    "__iter__" | "__len__" | "__contains__" | "__reversed__"
                )
            } else if type_name == "frozenset" {
                // frozenset: iterable, sized, contains, hashable, NOT reversed
                matches!(
                    dunder,
                    "__iter__" | "__len__" | "__contains__" | "__hash__"
                )
            } else if type_name == pyrust_builtins::bound_method::TYPE_NAME {
                // Built-in bound methods are callable and hashable
                matches!(dunder, "__call__" | "__hash__")
            } else {
                false
            }
        }

        // BuiltinFunction (`len`, `print`, …): callable and hashable
        ValueKind::BuiltinFunction(_) => matches!(dunder, "__call__" | "__hash__"),

        // UserFunction (lambdas, def'd functions): callable and hashable
        ValueKind::UserFunction(_) => matches!(dunder, "__call__" | "__hash__"),

        // BoundMethod / ClassBoundMethod: callable and hashable
        ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
            matches!(dunder, "__call__" | "__hash__")
        }

        // PyClass values (class objects): callable (__call__ = construct) and hashable
        ValueKind::PyClass(_) => matches!(dunder, "__call__" | "__hash__"),

        // PyInstance: handled separately via MRO walk — don't fall through here.
        // PyModule, SuperProxy, and any future variants: no protocol support.
        _ => false,
    }
}

// ── MRO method check (mirrors CPython's _check_methods) ──────────────────────
//
// Walks the class chain looking for `name`.  Returns:
//   Some(true)  — found in MRO and the value is not None
//   Some(false) — found in MRO but the value IS None (explicitly excluded,
//                 e.g. `__hash__ = None` on list-like user types)
//   None        — not found anywhere in the MRO (= NotImplemented in CPython)
//
// The `object` fallback in `lookup_class_attr` ensures that user classes
// without an explicit `__hash__` still resolve to `object.__hash__`, so they
// are correctly treated as Hashable by default.

fn class_mro_has_method(class: &Rc<RefCell<PyClass>>, name: &str) -> Option<bool> {
    use crate::interpreter::lookup_class_attr;
    match lookup_class_attr(class, name) {
        Some(v) if v.is_none() => Some(false),
        Some(_) => Some(true),
        None => None,
    }
}

// ── Public structural subtyping hook ─────────────────────────────────────────

/// `__subclasshook__`-style structural subtyping check.
///
/// Called by `isinstance_single` in `builtins.rs` when the expected class is
/// a known ABC.  Mirrors CPython's `ABCMeta.__instancecheck__` → `__subclasshook__`
/// → `_check_methods` chain.
///
/// Returns:
///   `Some(true)`  — all required methods present; `isinstance` should return True.
///   `Some(false)` — a required method is explicitly None; isinstance returns False.
///   `None`        — ABC not in our structural table, or check inconclusive;
///                   fall through to the `extra_bases` / MRO path.
pub(crate) fn abc_subclasshook(abc_name: &str, value: &Value) -> Option<bool> {
    let required = required_methods(abc_name)?;

    match value.kind() {
        // User-defined instances: walk the class MRO.
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            for &method in required {
                match class_mro_has_method(&class, method) {
                    Some(true) => {} // present and non-None; continue
                    Some(false) => return Some(false), // explicitly None
                    None => return Some(false), // method absent from entire MRO
                }
            }
            Some(true)
        }

        // All other value kinds (primitives, BuiltinFunction, Generator, …):
        // use the hardcoded protocol table.
        _ => {
            // Special case: BuiltinFunction and BuiltinObject bound methods are
            // already handled by dedicated arms in isinstance_single for Callable
            // and Hashable.  For other ABCs they correctly return false via the
            // primitive_value_has_dunder table.
            for &method in required {
                if !primitive_value_has_dunder(value, method) {
                    return Some(false);
                }
            }
            Some(true)
        }
    }
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
/// This handles the complex ABCs that do NOT have direct structural hooks
/// (Sequence, MutableSequence, Set, MutableSet, Mapping, MutableMapping),
/// as well as Callable for user-defined functions / classes.
///
/// The simple ABCs (Hashable, Iterable, Sized, Container, etc.) are now
/// handled by `abc_subclasshook` for all value kinds, so they no longer need
/// entries here.  The legacy entries are kept for the primitive class singletons
/// so that `issubclass(list, Iterable)` etc. still works via the class MRO —
/// the `extra_bases` on the primitive classes are still the source of truth
/// for class-vs-class subclass relationships.
///
/// CPython registration table (from Lib/_collections_abc.py):
///
///   Hashable:        int, float, complex, str, bytes, tuple, frozenset,
///                    bool, NoneType, NotImplementedType, ellipsis
///   Iterable:        str, bytes, list, tuple, dict, set, frozenset, bytearray
///   Container:       str, bytes, list, tuple, dict, set, frozenset, bytearray
///   Sized:           str, bytes, list, tuple, dict, set, frozenset, bytearray
///   Reversible:      str, bytes, list, tuple, dict, bytearray
///   Sequence:        str, bytes, list, tuple, bytearray
///   MutableSequence: list, bytearray
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

    let list_cls        = prim!("list");
    let tuple_cls       = prim!("tuple");
    let str_cls         = prim!("str");
    let bytes_cls       = prim!("bytes");
    let bytearray_cls   = prim!("bytearray");
    let dict_cls        = prim!("dict");
    let set_cls         = prim!("set");
    let frozenset_cls   = prim!("frozenset");
    let int_cls         = prim!("int");
    let float_cls       = prim!("float");
    let complex_cls     = prim!("complex");
    let bool_cls        = prim!("bool");
    let none_cls        = prim!("NoneType");

    // ── Sequence ────────────────────────────────────────────────────────────
    // list, tuple, str, bytes, bytearray are Sequences.
    for cls in [&list_cls, &tuple_cls, &str_cls, &bytes_cls, &bytearray_cls] {
        link(cls, &sequence);
    }

    // ── MutableSequence ─────────────────────────────────────────────────────
    // list and bytearray.
    for cls in [&list_cls, &bytearray_cls] {
        link(cls, &mut_sequence);
    }

    // ── Reversible ──────────────────────────────────────────────────────────
    // list, tuple, str, bytes, dict, bytearray
    // (not set, frozenset in CPython).
    // dict became Reversible in Python 3.8 (dict keys preserve insertion order).
    for cls in [&list_cls, &tuple_cls, &str_cls, &bytes_cls, &dict_cls, &bytearray_cls] {
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
    // str, bytes, list, tuple, dict, set, frozenset, bytearray.
    for cls in [
        &str_cls, &bytes_cls, &list_cls, &tuple_cls,
        &dict_cls, &set_cls, &frozenset_cls, &bytearray_cls,
    ] {
        link(cls, &container);
    }

    // ── Sized ───────────────────────────────────────────────────────────────
    // str, bytes, list, tuple, dict, set, frozenset, bytearray.
    for cls in [
        &str_cls, &bytes_cls, &list_cls, &tuple_cls,
        &dict_cls, &set_cls, &frozenset_cls, &bytearray_cls,
    ] {
        link(cls, &sized);
    }

    // ── Iterable ────────────────────────────────────────────────────────────
    // str, bytes, list, tuple, dict, set, frozenset, bytearray.
    for cls in [
        &str_cls, &bytes_cls, &list_cls, &tuple_cls,
        &dict_cls, &set_cls, &frozenset_cls, &bytearray_cls,
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
