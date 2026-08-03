//! Descriptor objects for numeric-tower read-only properties and methods on
//! `int` and `float` class objects.
//!
//! In CPython 3.12, accessing `int.real`, `int.imag`, `int.numerator`,
//! `int.denominator` returns a `getset_descriptor`, while `int.conjugate`
//! returns a `method_descriptor`.  These objects are descriptors: their
//! `__get__` method, when called with an instance, returns the same value as
//! direct instance attribute access.
//!
//! pyrust exposes these as `BuiltinObject` values so that `type(int.real).__name__`
//! returns `"getset_descriptor"` and `repr(int.real)` matches CPython's output.
//!
//! Calling a `getset_descriptor` directly raises `TypeError` (not callable),
//! matching CPython.  A `method_descriptor` is callable with an explicit
//! instance argument (e.g. `int.conjugate(5)` → 5).

use std::any::Any;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use pyrust_core::{
    BuiltinState, BuiltinTypeOps, PyBigInt, PyClass, PyToPrimitive, Value, ValueKind,
    builtin_ops_is,
};

// ── scalar / range read-only attribute values ───────────────────────────────
//
// Single source of truth for the *value* returned by the numeric-tower
// read-only properties (`real`/`imag`/`numerator`/`denominator`), `complex`
// `.real`/`.imag`, and `range` `.start`/`.stop`/`.step`.  `get_attr` (and the
// int/float-subclass path) delegate here instead of inlining per-type `match`
// arms, so the type-specific knowledge lives next to the rest of the numeric
// builtin logic rather than in the interpreter's attribute dispatcher.

/// `int` / `bool` / `BigInt` / `float` numeric-tower read-only properties:
/// `real`, `imag`, `numerator`, `denominator`.  Returns `None` when `value` is
/// not one of those numeric kinds or `name` is not such a property.
///
/// `bool` follows CPython: `True.real == 1` is a plain `int`, not a `bool`.
/// `float` exposes only `real`/`imag` (no `numerator`/`denominator`).
pub fn numeric_tower_attr(value: &Value, name: &str) -> Option<Value> {
    match value.kind() {
        ValueKind::Bool(b) => match name {
            "real" | "numerator" => Some(Value::int(b as i64)),
            "imag" => Some(Value::int(0)),
            "denominator" => Some(Value::int(1)),
            _ => None,
        },
        ValueKind::Int(_) | ValueKind::BigInt(_) => match name {
            "real" | "numerator" => Some(value.clone()),
            "imag" => Some(Value::int(0)),
            "denominator" => Some(Value::int(1)),
            _ => None,
        },
        ValueKind::Float(_) => match name {
            "real" => Some(value.clone()),
            "imag" => Some(Value::float(0.0)),
            _ => None,
        },
        _ => None,
    }
}

/// `complex` attribute access: the `.real` / `.imag` read-only properties and
/// the `.conjugate` method.  Returns `None` when `value` is not a `complex` or
/// `name` is not one of those attributes.
///
/// `conjugate` yields a *bound* method (not a value); it is handled here rather
/// than via the generic `has_method` path because the latter resolves it to an
/// unbound `method_descriptor` for `complex`.
pub fn complex_attr(value: &Value, name: &str) -> Option<Value> {
    let ValueKind::Complex(re, im) = value.kind() else {
        return None;
    };
    match name {
        "real" => Some(Value::float(re)),
        "imag" => Some(Value::float(im)),
        "conjugate" => Some(crate::bound_method::bound_method(
            "conjugate",
            value.clone(),
        )),
        _ => None,
    }
}

/// `range` `.start` / `.stop` / `.step` read-only properties (issue #1807).
/// Returns `None` when `value` is not a `range` or `name` is not one of them.
pub fn range_attr(value: &Value, name: &str) -> Option<Value> {
    match value.kind() {
        ValueKind::Range { start, stop, step } => match name {
            "start" => Some(Value::int(start)),
            "stop" => Some(Value::int(stop)),
            "step" => Some(Value::int(step)),
            _ => None,
        },
        // Arbitrary-precision range (#2118): start/stop/step are full ints.
        // Normalize each to the inline `int` form when it happens to fit i64
        // (e.g. `range(0, 10**20).step` is the plain int `1`).
        ValueKind::BigRange { start, stop, step } => {
            let normalize = |n: &PyBigInt| match n.to_i64() {
                Some(i) => Value::int(i),
                None => Value::bigint(n.clone()),
            };
            match name {
                "start" => Some(normalize(start)),
                "stop" => Some(normalize(stop)),
                "step" => Some(normalize(step)),
                _ => None,
            }
        }
        _ => None,
    }
}

// ── getset_descriptor ────────────────────────────────────────────────────────

pub struct GetsetDescriptorState {
    pub attr_name: &'static str,
    pub class_name: &'static str,
    owner: Option<Weak<RefCell<PyClass>>>,
    dict_view_kind: Option<crate::dict_views::DictViewKind>,
}

pub struct DictViewMappingDescriptorInfo {
    pub class_name: &'static str,
    pub view_kind: crate::dict_views::DictViewKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictViewMappingDescriptorMethod {
    Get,
    Set,
    Delete,
}

impl DictViewMappingDescriptorMethod {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "__get__" => Some(Self::Get),
            "__set__" => Some(Self::Set),
            "__delete__" => Some(Self::Delete),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Get => "__get__",
            Self::Set => "__set__",
            Self::Delete => "__delete__",
        }
    }
}

pub struct GetsetDescriptorOps;
pub const GETSET_DESCRIPTOR_OPS: &GetsetDescriptorOps = &GetsetDescriptorOps;
pub const GETSET_TYPE_NAME: &str = "getset_descriptor";

impl BuiltinTypeOps for GetsetDescriptorOps {
    fn type_name(&self) -> &'static str {
        GETSET_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<GetsetDescriptorState>()
            .expect("getset_descriptor state");
        format!(
            "<attribute '{}' of '{}' objects>",
            s.attr_name, s.class_name
        )
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let descriptor = borrow.downcast_ref::<GetsetDescriptorState>()?;
        match name {
            "__name__" => Some(Value::string(descriptor.attr_name)),
            "__objclass__" => descriptor
                .owner
                .as_ref()
                .and_then(Weak::upgrade)
                .map(Value::py_class),
            "__doc__" if descriptor.dict_view_kind.is_some() => {
                Some(Value::string("dictionary that this view refers to"))
            }
            _ => None,
        }
    }
}

/// Construct a `getset_descriptor` value for the given attribute and class name.
pub fn getset_descriptor(attr_name: &'static str, class_name: &'static str) -> Value {
    let state: Box<dyn Any> = Box::new(GetsetDescriptorState {
        attr_name,
        class_name,
        owner: None,
        dict_view_kind: None,
    });
    Value::builtin_object(GETSET_DESCRIPTOR_OPS, state)
}

/// Construct the read-only `mapping` descriptor owned by a plain dictionary
/// view class. Ordered view classes inherit this exact descriptor from their
/// corresponding plain base.
pub fn dict_view_mapping_descriptor(
    owner: &Rc<RefCell<PyClass>>,
    view_kind: crate::dict_views::DictViewKind,
) -> Value {
    let class_name = match view_kind {
        crate::dict_views::DictViewKind::Keys => crate::dict_views::DICT_KEYS_TYPE_NAME,
        crate::dict_views::DictViewKind::Items => crate::dict_views::DICT_ITEMS_TYPE_NAME,
        crate::dict_views::DictViewKind::Values => crate::dict_views::DICT_VALUES_TYPE_NAME,
    };
    let state: Box<dyn Any> = Box::new(GetsetDescriptorState {
        attr_name: "mapping",
        class_name,
        owner: Some(Rc::downgrade(owner)),
        dict_view_kind: Some(view_kind),
    });
    Value::builtin_object(GETSET_DESCRIPTOR_OPS, state)
}

/// Extract the `(attr_name, class_name)` from a `getset_descriptor` Value,
/// or `None` if the value is not a getset_descriptor.
pub fn as_getset_descriptor(value: &Value) -> Option<(&'static str, &'static str)> {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<GetsetDescriptorOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<GetsetDescriptorState>()?;
    Some((s.attr_name, s.class_name))
}

pub fn as_dict_view_mapping_descriptor(value: &Value) -> Option<DictViewMappingDescriptorInfo> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<GetsetDescriptorOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let descriptor = borrow.downcast_ref::<GetsetDescriptorState>()?;
    Some(DictViewMappingDescriptorInfo {
        class_name: descriptor.class_name,
        view_kind: descriptor.dict_view_kind?,
    })
}

pub fn dict_view_mapping_descriptor_method(
    value: &Value,
    name: &str,
) -> Option<DictViewMappingDescriptorMethod> {
    let method = DictViewMappingDescriptorMethod::from_name(name)?;
    as_dict_view_mapping_descriptor(value)?;
    Some(method)
}

// ── method_descriptor ────────────────────────────────────────────────────────

pub struct MethodDescriptorState {
    pub attr_name: &'static str,
    pub class_name: &'static str,
}

pub struct MethodDescriptorOps;
pub const METHOD_DESCRIPTOR_OPS: &MethodDescriptorOps = &MethodDescriptorOps;
pub const METHOD_DESCRIPTOR_TYPE_NAME: &str = "method_descriptor";

impl BuiltinTypeOps for MethodDescriptorOps {
    fn type_name(&self) -> &'static str {
        METHOD_DESCRIPTOR_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<MethodDescriptorState>()
            .expect("method_descriptor state");
        format!("<method '{}' of '{}' objects>", s.attr_name, s.class_name)
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }
}

/// Construct a `method_descriptor` value for the given attribute and class name.
pub fn method_descriptor(attr_name: &'static str, class_name: &'static str) -> Value {
    let state: Box<dyn Any> = Box::new(MethodDescriptorState {
        attr_name,
        class_name,
    });
    Value::builtin_object(METHOD_DESCRIPTOR_OPS, state)
}

/// Extract the `(attr_name, class_name)` from a `method_descriptor` Value,
/// or `None` if the value is not a method_descriptor.
pub fn as_method_descriptor(value: &Value) -> Option<(&'static str, &'static str)> {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<MethodDescriptorOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<MethodDescriptorState>()?;
    Some((s.attr_name, s.class_name))
}
