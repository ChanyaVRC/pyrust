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
// Each ABC is a per-thread `PyClass` singleton.  `isinstance` checks work
// through three mechanisms:
//
// 1. **`__instancecheck__` / `__subclasshook__` method dispatch**: each ABC
//    stores `__instancecheck__`, `__subclasshook__`, and `__subclasscheck__`
//    as `BuiltinFunction` attrs.  The `isinstance` builtin detects these and
//    calls them via the interpreter, mirroring CPython's
//    `ABCMeta.__instancecheck__` → `__subclasshook__` → `_check_methods`
//    chain.  This enables:
//      - `hasattr(Iterable, '__instancecheck__')` → True
//      - `Iterable.__instancecheck__(Foo())` → True (direct call)
//      - `issubclass(UserClass, Iterable)` → structural check (fixes #1799)
//
// 2. **`abc_subclasshook`** (internal helper): the shared Rust implementation
//    used by `__subclasshook__` and `__instancecheck__` for the structural
//    check.  Walks the class MRO for user instances, or uses the hardcoded
//    primitive protocol table for built-in values.
//
// 3. **`extra_bases` registration** (legacy / complex ABCs): on first module
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

use crate::error::{PyError, Result};
use crate::interpreter::{class_is_subclass_of, ExpandedCallArg};
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

/// `__subclasshook__`-style structural subtyping check for an *instance*.
///
/// Used internally by the `__instancecheck__` and `__subclasshook__` builtins.
/// Mirrors CPython's `ABCMeta.__instancecheck__` → `__subclasshook__`
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
            for &method in required {
                if !primitive_value_has_dunder(value, method) {
                    return Some(false);
                }
            }
            Some(true)
        }
    }
}

/// `__subclasshook__`-style structural subtyping check for a *class* (used by
/// `issubclass`).  Checks whether the class `C` has all required methods in
/// its MRO.  Returns `NotImplemented` (as `None`) when a method is absent —
/// this allows the caller to fall back to the `extra_bases` / MRO check
/// (`class_is_subclass_of`), which is the source of truth for primitive types
/// like `list` whose methods are not stored in the `PyClass` attrs dict.
///
/// Mirrors CPython's `_check_methods`: absent → `NotImplemented`, explicit
/// `None` (e.g., `__hash__ = None`) → `False`.
fn abc_subclasshook_for_class(abc_name: &str, class: &Rc<RefCell<PyClass>>) -> Option<bool> {
    let required = required_methods(abc_name)?;
    for &method in required {
        match class_mro_has_method(class, method) {
            Some(true) => {} // present and non-None; continue
            Some(false) => return Some(false), // explicitly None → hard False
            None => return None,              // absent → NotImplemented; let caller fall back
        }
    }
    Some(true)
}

// ── Registry names for the three ABC dunder methods ─────────────────────────
//
// These must match the `fn <short_name>(args)` declarations inside the
// `pyrust_module!` below, prefixed with the module's `FN_PREFIX`
// ("collections.abc.").

const ABC_INSTANCECHECK_FN:   &str = "collections.abc.__instancecheck__";
const ABC_SUBCLASSHOOK_FN:    &str = "collections.abc.__subclasshook__";
const ABC_SUBCLASSCHECK_FN:   &str = "collections.abc.__subclasscheck__";

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

    /// ABC.__instancecheck__(cls, instance) — called by the `isinstance` builtin
    /// when `cls` is an ABC with this method in its attrs.
    ///
    /// Protocol:
    ///   1. Call cls.__subclasshook__(type(instance)).
    ///   2. If the result is True or False, return it.
    ///   3. Otherwise fall through to class_is_subclass_of (MRO / extra_bases).
    ///
    /// args[0] = cls (the ABC class value), args[1] = instance.
    #[py_name = "__instancecheck__"]
    fn abc_instancecheck(args) -> Result<Value> {
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 2 arguments ({} given)", args.len()),
            ));
        }
        let cls_val  = &args[0].value;
        let instance = &args[1].value;

        let abc_name = match cls_val.kind() {
            ValueKind::PyClass(rc) => rc.borrow().name.clone(),
            _ => return Err(PyError::named("TypeError",
                format!("{FN_NAME}() first argument must be a class"))),
        };

        // Step 1: structural hook (instance-based).
        if let Some(result) = abc_subclasshook(&abc_name, instance) {
            return Ok(Value::bool_(result));
        }

        // Step 2: MRO / extra_bases fallback.
        let actual_class = match instance.kind() {
            ValueKind::PyInstance(inst) => Some(Rc::clone(&inst.borrow().class)),
            ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
                Some(crate::interpreter::method_type_singleton())
            }
            ValueKind::UserFunction(_) => Some(crate::interpreter::function_type_singleton()),
            ValueKind::PyClass(cls_rc) => {
                let meta = cls_rc.borrow().metatype.clone();
                Some(meta.unwrap_or_else(crate::interpreter::type_class_singleton))
            }
            _ => crate::interpreter::primitive_class_for_value(instance),
        };

        if let (Some(actual), ValueKind::PyClass(expected_rc)) = (actual_class, cls_val.kind()) {
            return Ok(Value::bool_(class_is_subclass_of(&actual, expected_rc)));
        }

        Ok(Value::bool_(false))
    }

    /// ABC.__subclasshook__(cls, C) — classmethod-style structural check.
    ///
    /// Called directly or via `__subclasscheck__`.  Returns True/False if this
    /// ABC has a structural hook for `C`, or NotImplemented if not applicable.
    ///
    /// args[0] = cls (the ABC class value), args[1] = C (class to check).
    #[py_name = "__subclasshook__"]
    fn abc_subclasshook_method(args) -> Result<Value> {
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 2 arguments ({} given)", args.len()),
            ));
        }
        let cls_val = &args[0].value;
        let c_val   = &args[1].value;

        let abc_name = match cls_val.kind() {
            ValueKind::PyClass(rc) => rc.borrow().name.clone(),
            _ => return Err(PyError::named("TypeError",
                format!("{FN_NAME}() first argument must be a class"))),
        };

        match c_val.kind() {
            ValueKind::PyClass(c_rc) => {
                match abc_subclasshook_for_class(&abc_name, c_rc) {
                    Some(result) => Ok(Value::bool_(result)),
                    None => Ok(Value::not_implemented()),
                }
            }
            _ => Ok(Value::not_implemented()),
        }
    }

    /// ABC.__subclasscheck__(cls, subclass) — called by the `issubclass` builtin
    /// when `cls` is an ABC with this method in its attrs.
    ///
    /// Protocol:
    ///   1. Call cls.__subclasshook__(subclass).
    ///   2. If the result is True or False, return it.
    ///   3. Otherwise fall through to class_is_subclass_of (MRO / extra_bases).
    ///
    /// args[0] = cls (the ABC class value), args[1] = subclass.
    #[py_name = "__subclasscheck__"]
    fn abc_subclasscheck(args) -> Result<Value> {
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 2 arguments ({} given)", args.len()),
            ));
        }
        let cls_val      = &args[0].value;
        let subclass_val = &args[1].value;

        let abc_name = match cls_val.kind() {
            ValueKind::PyClass(rc) => rc.borrow().name.clone(),
            _ => return Err(PyError::named("TypeError",
                format!("{FN_NAME}() first argument must be a class"))),
        };

        match subclass_val.kind() {
            ValueKind::PyClass(c_rc) => {
                // Step 1: structural hook (class-based).
                if let Some(result) = abc_subclasshook_for_class(&abc_name, c_rc) {
                    return Ok(Value::bool_(result));
                }
                // Step 2: MRO / extra_bases fallback.
                let expected_rc = match cls_val.kind() {
                    ValueKind::PyClass(rc) => rc,
                    _ => unreachable!(),
                };
                Ok(Value::bool_(class_is_subclass_of(c_rc, expected_rc)))
            }
            _ => Ok(Value::bool_(false)),
        }
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

/// Register each ABC as an `extra_base` on the relevant primitive class(es),
/// and add `__instancecheck__`, `__subclasshook__`, and `__subclasscheck__`
/// as `BuiltinFunction` attrs on every ABC class.
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
/// handled by `__instancecheck__` / `__subclasscheck__` for all value kinds.
/// The `extra_bases` entries are kept for the primitive class singletons so
/// that `issubclass(list, Iterable)` etc. still works via the class MRO.
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
///   Callable:        function, method, type  (+ BuiltinFunction / BuiltinObject
///                    bound methods — now handled via __instancecheck__)
fn register_abc_extra_bases() {
    use crate::interpreter::{
        function_type_singleton, method_type_singleton, object_class_singleton,
        primitive_class_by_name, range_class_singleton, type_class_singleton,
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

    // Add __instancecheck__, __subclasshook__, __subclasscheck__ to an ABC as
    // BuiltinFunction values.  When accessed on a PyClass via env.rs::get_attr,
    // these are intercepted by `is_builtin_classmethod` and wrapped as
    // `super_bound_builtin(fn_name, cls)`, so that direct access
    // `Iterable.__instancecheck__` returns a bound callable where cls=Iterable.
    // This means `Iterable.__instancecheck__(x)` is called as
    // `abc_instancecheck([Iterable, x])` after the super_bound_builtin dispatch
    // prepends the receiver.
    fn add_dunder_methods(abc: &Rc<RefCell<PyClass>>) {
        let mut borrowed = abc.borrow_mut();
        borrowed.attrs.entry("__instancecheck__".to_string())
            .or_insert_with(|| Value::builtin_function(ABC_INSTANCECHECK_FN));
        borrowed.attrs.entry("__subclasshook__".to_string())
            .or_insert_with(|| Value::builtin_function(ABC_SUBCLASSHOOK_FN));
        borrowed.attrs.entry("__subclasscheck__".to_string())
            .or_insert_with(|| Value::builtin_function(ABC_SUBCLASSCHECK_FN));
    }

    // Resolve ABCs once.
    let container       = ABC_CONTAINER.with(Rc::clone);
    let hashable        = ABC_HASHABLE.with(Rc::clone);
    let iterable        = ABC_ITERABLE.with(Rc::clone);
    let iterator        = ABC_ITERATOR.with(Rc::clone);
    let reversible      = ABC_REVERSIBLE.with(Rc::clone);
    let generator       = ABC_GENERATOR.with(Rc::clone);
    let sized           = ABC_SIZED.with(Rc::clone);
    let callable        = ABC_CALLABLE.with(Rc::clone);
    let sequence        = ABC_SEQUENCE.with(Rc::clone);
    let mut_sequence    = ABC_MUTABLE_SEQUENCE.with(Rc::clone);
    let set_abc         = ABC_SET.with(Rc::clone);
    let mut_set         = ABC_MUTABLE_SET.with(Rc::clone);
    let mapping         = ABC_MAPPING.with(Rc::clone);
    let mut_mapping     = ABC_MUTABLE_MAPPING.with(Rc::clone);
    let mapping_view    = ABC_MAPPING_VIEW.with(Rc::clone);
    let keys_view       = ABC_KEYS_VIEW.with(Rc::clone);
    let items_view      = ABC_ITEMS_VIEW.with(Rc::clone);
    let values_view     = ABC_VALUES_VIEW.with(Rc::clone);
    let awaitable       = ABC_AWAITABLE.with(Rc::clone);
    let coroutine       = ABC_COROUTINE.with(Rc::clone);
    let async_iterable  = ABC_ASYNC_ITERABLE.with(Rc::clone);
    let async_iterator  = ABC_ASYNC_ITERATOR.with(Rc::clone);
    let async_generator = ABC_ASYNC_GENERATOR.with(Rc::clone);
    let buffer          = ABC_BUFFER.with(Rc::clone);

    // Add __instancecheck__ / __subclasshook__ / __subclasscheck__ to every ABC.
    for abc in [
        &container, &hashable, &iterable, &iterator, &reversible, &generator,
        &sized, &callable, &sequence, &mut_sequence, &set_abc, &mut_set,
        &mapping, &mut_mapping, &mapping_view, &keys_view, &items_view,
        &values_view, &awaitable, &coroutine, &async_iterable, &async_iterator,
        &async_generator, &buffer,
    ] {
        add_dunder_methods(abc);
    }

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

    // ── Singleton class objects ──────────────────────────────────────────────
    let fn_type   = function_type_singleton();
    let meth_type = method_type_singleton();
    let type_cls  = type_class_singleton();
    let range_cls = range_class_singleton();
    let _obj_cls  = object_class_singleton(); // ensure singleton initialised

    // ── Sequence ────────────────────────────────────────────────────────────
    // list, tuple, str, bytes, bytearray, range are Sequences.
    // CPython: Sequence.register(range) in Lib/_collections_abc.py (issue #1800).
    for cls in [&list_cls, &tuple_cls, &str_cls, &bytes_cls, &bytearray_cls, &range_cls] {
        link(cls, &sequence);
    }

    // ── MutableSequence ─────────────────────────────────────────────────────
    // list and bytearray.
    for cls in [&list_cls, &bytearray_cls] {
        link(cls, &mut_sequence);
    }

    // ── Reversible ──────────────────────────────────────────────────────────
    // list, tuple, str, bytes, dict, bytearray, range
    // (not set, frozenset in CPython).
    // dict became Reversible in Python 3.8 (dict keys preserve insertion order).
    // CPython: Reversible.register(range) in Lib/_collections_abc.py (issue #1800).
    for cls in [
        &list_cls, &tuple_cls, &str_cls, &bytes_cls, &dict_cls, &bytearray_cls, &range_cls,
    ] {
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
    // str, bytes, list, tuple, dict, set, frozenset, bytearray, range.
    for cls in [
        &str_cls, &bytes_cls, &list_cls, &tuple_cls,
        &dict_cls, &set_cls, &frozenset_cls, &bytearray_cls, &range_cls,
    ] {
        link(cls, &container);
    }

    // ── Sized ───────────────────────────────────────────────────────────────
    // str, bytes, list, tuple, dict, set, frozenset, bytearray, range.
    for cls in [
        &str_cls, &bytes_cls, &list_cls, &tuple_cls,
        &dict_cls, &set_cls, &frozenset_cls, &bytearray_cls, &range_cls,
    ] {
        link(cls, &sized);
    }

    // ── Iterable ────────────────────────────────────────────────────────────
    // str, bytes, list, tuple, dict, set, frozenset, bytearray, range.
    for cls in [
        &str_cls, &bytes_cls, &list_cls, &tuple_cls,
        &dict_cls, &set_cls, &frozenset_cls, &bytearray_cls, &range_cls,
    ] {
        link(cls, &iterable);
    }

    // ── Hashable ────────────────────────────────────────────────────────────
    // int, float, complex, str, bytes, tuple, frozenset, bool, NoneType, range.
    // Also: user-defined functions and bound methods (issue #1793) — they are
    // hashable by identity in CPython.  The structural hook already returns
    // Some(true) for instances, but `issubclass(type(f), Hashable)` goes through
    // the extra_bases MRO path, so we must link the function/method class here.
    // (list, dict, set are NOT hashable)
    for cls in [
        &int_cls, &float_cls, &complex_cls, &str_cls, &bytes_cls,
        &tuple_cls, &frozenset_cls, &bool_cls, &none_cls,
        &range_cls, &fn_type, &meth_type,
    ] {
        link(cls, &hashable);
    }

    // ── Callable ────────────────────────────────────────────────────────────
    // function, method, type.  BuiltinFunction and BuiltinObject bound methods
    // are handled via __instancecheck__ using the primitive protocol table.
    for cls in [&fn_type, &meth_type, &type_cls] {
        link(cls, &callable);
    }
}
