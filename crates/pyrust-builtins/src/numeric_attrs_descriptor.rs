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

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value};

// ── getset_descriptor ────────────────────────────────────────────────────────

pub struct GetsetDescriptorState {
    pub attr_name: &'static str,
    pub class_name: &'static str,
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
}

/// Construct a `getset_descriptor` value for the given attribute and class name.
pub fn getset_descriptor(attr_name: &'static str, class_name: &'static str) -> Value {
    let state: Box<dyn Any> = Box::new(GetsetDescriptorState {
        attr_name,
        class_name,
    });
    Value::builtin_object(GETSET_DESCRIPTOR_OPS, state)
}

/// Extract the `(attr_name, class_name)` from a `getset_descriptor` Value,
/// or `None` if the value is not a getset_descriptor.
pub fn as_getset_descriptor(value: &Value) -> Option<(&'static str, &'static str)> {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != GETSET_TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<GetsetDescriptorState>()?;
    Some((s.attr_name, s.class_name))
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
    if ops.type_name() != METHOD_DESCRIPTOR_TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<MethodDescriptorState>()?;
    Some((s.attr_name, s.class_name))
}
