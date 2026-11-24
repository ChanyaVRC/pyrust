//! Structural subtyping hooks (`__instancecheck__`, `__subclasshook__`,
//! `__subclasscheck__`) and primitive protocol lookup for `collections.abc`.

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_core::GeneratorKind;

use crate::error::{PyError, Result};
use crate::interpreter::{
    DictViewClass, ExpandedCallArg, class_hash_inherits_builtin_none, class_is_subclass_of,
    is_async_gen_asend_value, lookup_class_attr,
};
use crate::value::{PyClass, Value, ValueKind};

use super::definitions::{
    canonical_structural_abc_name, dict_view_instance_is_virtual_abc, dict_view_is_virtual_abc,
    is_virtual_subclass, required_methods,
};

fn primitive_value_has_dunder(value: &Value, dunder: &str) -> bool {
    match value.kind() {
        ValueKind::Str(_)
        | ValueKind::Tuple(_)
        | ValueKind::Range { .. }
        | ValueKind::BigRange { .. } => matches!(
            dunder,
            "__iter__" | "__len__" | "__contains__" | "__hash__" | "__reversed__"
        ),

        ValueKind::Bytes(_) => matches!(
            dunder,
            "__iter__" | "__len__" | "__contains__" | "__hash__" | "__reversed__" | "__buffer__"
        ),

        ValueKind::List(_) | ValueKind::Dict(_) => matches!(
            dunder,
            "__iter__" | "__len__" | "__contains__" | "__reversed__"
        ),

        ValueKind::Set(_) => matches!(dunder, "__iter__" | "__len__" | "__contains__"),

        ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Bool(_)
        | ValueKind::Float(_)
        | ValueKind::Complex(_, _)
        | ValueKind::None
        | ValueKind::NotImplemented
        | ValueKind::Ellipsis => dunder == "__hash__",

        ValueKind::Generator(cell) => match cell.kind() {
            GeneratorKind::Generator => {
                matches!(dunder, "__iter__" | "__next__" | "send" | "throw" | "close")
            }
            GeneratorKind::Coroutine => {
                matches!(dunder, "__await__" | "send" | "throw" | "close")
            }
            GeneratorKind::AsyncGenerator => matches!(
                dunder,
                "__aiter__" | "__anext__" | "asend" | "athrow" | "aclose"
            ),
            GeneratorKind::Iterator => {
                if cell.try_borrow().is_ok_and(|state| {
                    state
                        .downcast_ref::<crate::interpreter::AnextAwaitable>()
                        .is_some()
                }) {
                    matches!(
                        dunder,
                        "__await__" | "__iter__" | "__next__" | "send" | "throw" | "close"
                    )
                } else if is_async_gen_asend_value(value) {
                    matches!(dunder, "__await__" | "send" | "throw" | "close")
                } else {
                    matches!(dunder, "__iter__" | "__next__")
                }
            }
        },

        ValueKind::BuiltinObject { ops, .. } => {
            if let Some(kind) = pyrust_builtins::dict_views::view_kind(value) {
                match kind {
                    pyrust_builtins::dict_views::DictViewKind::Values => {
                        matches!(dunder, "__iter__" | "__len__" | "__reversed__" | "__hash__")
                    }
                    pyrust_builtins::dict_views::DictViewKind::Keys
                    | pyrust_builtins::dict_views::DictViewKind::Items => matches!(
                        dunder,
                        "__iter__" | "__len__" | "__reversed__" | "__contains__"
                    ),
                }
            } else if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) {
                matches!(
                    dunder,
                    "__iter__" | "__len__" | "__contains__" | "__reversed__" | "__buffer__"
                )
            } else if ops.canonical_class_tag()
                == Some(pyrust_core::CanonicalClassTag::MappingProxy)
            {
                matches!(
                    dunder,
                    "__iter__" | "__len__" | "__contains__" | "__reversed__"
                )
            } else if pyrust_builtins::memoryview::is_memoryview_ops(ops) {
                matches!(
                    dunder,
                    "__iter__"
                        | "__len__"
                        | "__contains__"
                        | "__hash__"
                        | "__reversed__"
                        | "__buffer__"
                )
            } else if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) {
                matches!(dunder, "__iter__" | "__len__" | "__contains__" | "__hash__")
            } else if pyrust_builtins::bound_method::is_bound_method(value) {
                matches!(dunder, "__call__" | "__hash__")
            } else {
                false
            }
        }

        ValueKind::BuiltinFunction(_)
        | ValueKind::UserFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::PyClass(_) => matches!(dunder, "__call__" | "__hash__"),

        _ => false,
    }
}

fn class_mro_has_method(class: &Rc<RefCell<PyClass>>, name: &str) -> Option<bool> {
    if name == "__hash__" && class_hash_inherits_builtin_none(class) {
        return Some(false);
    }
    match lookup_class_attr(class, name) {
        Some(v) if v.is_none() => Some(false),
        Some(_) => Some(true),
        None => None,
    }
}

/// `__subclasshook__`-style structural subtyping check for an *instance*.
fn abc_subclasshook(abc_name: &str, value: &Value) -> Option<bool> {
    let required = required_methods(abc_name)?;
    match value.kind() {
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            abc_subclasshook_for_class(abc_name, &class)
        }
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

fn abc_subclasshook_for_class(abc_name: &str, class: &Rc<RefCell<PyClass>>) -> Option<bool> {
    let required = required_methods(abc_name)?;
    for &method in required {
        match class_mro_has_method(class, method) {
            Some(true) => {}
            Some(false) => return Some(false),
            None => return None,
        }
    }
    Some(true)
}

fn abc_subclasshook_for_builtin_token(abc_name: &str, type_name: &str) -> Option<bool> {
    let required = required_methods(abc_name)?;
    let has_dunder = |dunder: &str| match type_name {
        "generator" => Some(matches!(
            dunder,
            "__iter__" | "__next__" | "send" | "throw" | "close"
        )),
        "coroutine" | "async_generator_asend" | "async_generator_athrow" => {
            Some(matches!(dunder, "__await__" | "send" | "throw" | "close"))
        }
        "anext_awaitable" => Some(matches!(
            dunder,
            "__await__" | "__iter__" | "__next__" | "send" | "throw" | "close"
        )),
        "async_generator" => Some(matches!(
            dunder,
            "__aiter__" | "__anext__" | "asend" | "athrow" | "aclose"
        )),
        "map"
        | "filter"
        | "zip"
        | "enumerate"
        | "iterator"
        | "callable_iterator"
        | "list_iterator"
        | "list_reverseiterator"
        | "tuple_iterator"
        | "str_iterator"
        | "str_ascii_iterator"
        | "bytes_iterator"
        | "range_iterator"
        | "longrange_iterator"
        | "set_iterator"
        | "dict_keyiterator"
        | "dict_valueiterator"
        | "dict_itemiterator"
        | "dict_reversekeyiterator"
        | "dict_reversevalueiterator"
        | "dict_reverseitemiterator" => Some(matches!(dunder, "__iter__" | "__next__")),
        _ => None,
    };
    for &method in required {
        if !has_dunder(method)? {
            return None;
        }
    }
    Some(true)
}

fn parse_abc_check_args<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
) -> Result<(Rc<RefCell<PyClass>>, &'a Value)> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let abc_class = match args[0].value.kind() {
        ValueKind::PyClass(rc) => Rc::clone(rc),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() first argument must be a class"),
            ));
        }
    };
    Ok((abc_class, &args[1].value))
}

pub(super) fn instancecheck(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let (abc_class, instance) = parse_abc_check_args(args, fn_name)?;

    if dict_view_instance_is_virtual_abc(instance, &abc_class) {
        return Ok(Value::bool_(true));
    }

    if let Some(abc_name) = canonical_structural_abc_name(&abc_class)
        && let Some(result) = abc_subclasshook(abc_name, instance)
    {
        return Ok(Value::bool_(result));
    }

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
        _ => crate::interpreter::value_class_object(instance),
    };

    if let Some(actual) = actual_class {
        return Ok(Value::bool_(
            class_is_subclass_of(&actual, &abc_class) || is_virtual_subclass(&actual, &abc_class),
        ));
    }

    Ok(Value::bool_(false))
}

pub(super) fn subclasshook_method(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let (abc_class, c_val) = parse_abc_check_args(args, fn_name)?;
    let Some(abc_name) = canonical_structural_abc_name(&abc_class) else {
        return Ok(Value::not_implemented());
    };
    match c_val.kind() {
        ValueKind::PyClass(c_rc) => match abc_subclasshook_for_class(abc_name, c_rc) {
            Some(result) => Ok(Value::bool_(result)),
            None => Ok(Value::not_implemented()),
        },
        ValueKind::BuiltinFunction(type_name) => {
            match abc_subclasshook_for_builtin_token(abc_name, type_name) {
                Some(result) => Ok(Value::bool_(result)),
                None => Ok(Value::not_implemented()),
            }
        }
        _ => Ok(Value::not_implemented()),
    }
}

pub(super) fn subclasscheck(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let (abc_class, subclass_val) = parse_abc_check_args(args, fn_name)?;
    match subclass_val.kind() {
        ValueKind::PyClass(c_rc) => {
            if DictViewClass::from_class(c_rc)
                .is_some_and(|kind| dict_view_is_virtual_abc(kind, &abc_class))
            {
                return Ok(Value::bool_(true));
            }
            if let Some(abc_name) = canonical_structural_abc_name(&abc_class)
                && let Some(result) = abc_subclasshook_for_class(abc_name, c_rc)
            {
                return Ok(Value::bool_(result));
            }
            Ok(Value::bool_(
                class_is_subclass_of(c_rc, &abc_class) || is_virtual_subclass(c_rc, &abc_class),
            ))
        }
        ValueKind::BuiltinFunction(type_name) => {
            if let Some(abc_name) = canonical_structural_abc_name(&abc_class)
                && let Some(result) = abc_subclasshook_for_builtin_token(abc_name, type_name)
            {
                return Ok(Value::bool_(result));
            }
            Ok(Value::bool_(false))
        }
        _ => Ok(Value::bool_(false)),
    }
}
