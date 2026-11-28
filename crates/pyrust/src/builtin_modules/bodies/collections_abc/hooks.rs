//! CPython's `_check_methods` and the structural `__subclasshook__`s of the
//! one-trick-pony ABCs (`Hashable`, `Iterable`, `Sized`, …).
//!
//! `_check_methods(C, *methods)` walks `C.__mro__` and inspects each class
//! `__dict__`. The runtime's built-in types do not all expose their slots in a
//! class dictionary yet (and several are still `BuiltinFunction` name tokens
//! rather than class objects), so the lookups below combine the class
//! dictionaries with the method tables of those built-in types.

use std::rc::Rc;

use crate::error::Result;
use crate::interpreter::{
    Interpreter, class_hash_inherits_builtin_none, function_type_singleton, lookup_class_attr,
    method_type_singleton, type_class_singleton,
};
use crate::value::{Value, ValueKind};

use super::definitions::{ClassRc, abc_of};

/// How one method name resolves along a class's MRO.
enum Lookup {
    Found,
    /// Explicitly set to `None` (`__hash__ = None`, `__reversed__ = None`).
    Blocked,
    Missing,
}

/// `cls.__subclasshook__(C)` for the ABCs that define one.
pub(super) fn subclasshook(
    interp: &mut Interpreter,
    cls: &Value,
    subclass: &Value,
) -> Result<Value> {
    let ValueKind::PyClass(class) = cls.kind() else {
        return Ok(Value::not_implemented());
    };
    let Some(methods) = abc_of(class).and_then(|abc| abc.hook_methods()) else {
        return Ok(Value::not_implemented());
    };
    check_methods(interp, subclass, methods)
}

/// `_check_methods(C, *methods)`: `True` when every method resolves to a
/// non-`None` value, otherwise `NotImplemented`.
pub(super) fn check_methods(
    interp: &mut Interpreter,
    subclass: &Value,
    methods: &[&str],
) -> Result<Value> {
    let resolved = match subclass.kind() {
        ValueKind::PyClass(class) => methods
            .iter()
            .all(|method| matches!(class_lookup(class, method), Lookup::Found)),
        ValueKind::BuiltinFunction(token) => {
            methods.iter().all(|method| token_has_method(token, method))
        }
        _ => {
            // `C.__mro__` on a non-class raises `AttributeError`; an object
            // that does provide an MRO is inspected entry by entry.
            let mro = interp.get_attr(subclass, "__mro__")?;
            let mro = interp.collect_iterable(&mro)?;
            methods.iter().all(|method| {
                for base in &mro {
                    if let ValueKind::PyClass(base) = base.kind() {
                        match base.borrow().attrs.get(*method) {
                            Some(value) => return !value.is_none(),
                            None => continue,
                        }
                    }
                }
                false
            })
        }
    };
    Ok(if resolved {
        Value::bool_(true)
    } else {
        Value::not_implemented()
    })
}

fn class_lookup(class: &ClassRc, method: &str) -> Lookup {
    if method == "__hash__" && class_hash_inherits_builtin_none(class) {
        return Lookup::Blocked;
    }
    match lookup_class_attr(class, method) {
        Some(value) if value.is_none() => Lookup::Blocked,
        Some(_) => Lookup::Found,
        None if builtin_class_has_method(class, method) => Lookup::Found,
        None => Lookup::Missing,
    }
}

/// Slots of built-in class objects that their class dictionary does not
/// (yet) expose.
fn builtin_class_has_method(class: &ClassRc, method: &str) -> bool {
    let is = |candidate: ClassRc| Rc::ptr_eq(class, &candidate);
    if is(function_type_singleton()) || is(method_type_singleton()) {
        return matches!(method, "__call__" | "__get__" | "__hash__");
    }
    if is(type_class_singleton()) {
        return matches!(method, "__call__" | "__hash__");
    }
    false
}

/// Whether the built-in type a `BuiltinFunction` name token stands for
/// defines `method`.
///
/// Every such type keeps `object.__hash__`; the remaining slots are those of
/// CPython's generator, coroutine, iterator, and callable types.
fn token_has_method(token: &str, method: &str) -> bool {
    if method == "__hash__" {
        return true;
    }
    match token {
        "generator" | "coroutine_wrapper" => {
            matches!(method, "__iter__" | "__next__" | "send" | "throw" | "close")
        }
        "coroutine" => matches!(method, "__await__" | "send" | "throw" | "close"),
        "async_generator" => matches!(
            method,
            "__aiter__" | "__anext__" | "asend" | "athrow" | "aclose"
        ),
        "async_generator_asend" | "async_generator_athrow" | "anext_awaitable" => matches!(
            method,
            "__await__" | "__iter__" | "__next__" | "send" | "throw" | "close"
        ),
        "builtin_function_or_method"
        | "method_descriptor"
        | "wrapper_descriptor"
        | "method-wrapper"
        | "classmethod_descriptor"
        | "staticmethod" => method == "__call__",
        "map"
        | "filter"
        | "zip"
        | "enumerate"
        | "reversed"
        | "iterator"
        | "callable_iterator"
        | "list_iterator"
        | "list_reverseiterator"
        | "tuple_iterator"
        | "str_iterator"
        | "str_ascii_iterator"
        | "bytes_iterator"
        | "bytearray_iterator"
        | "range_iterator"
        | "longrange_iterator"
        | "set_iterator"
        | "dict_keyiterator"
        | "dict_valueiterator"
        | "dict_itemiterator"
        | "dict_reversekeyiterator"
        | "dict_reversevalueiterator"
        | "dict_reverseitemiterator"
        | "odict_iterator"
        | "memory_iterator" => matches!(method, "__iter__" | "__next__"),
        _ => false,
    }
}
