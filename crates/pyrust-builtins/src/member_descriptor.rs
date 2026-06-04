//! `member_descriptor` objects for `__slots__` (issues #2084 / #2076).
//!
//! In CPython, declaring `class S: __slots__ = ('x',)` installs a
//! `member_descriptor` in the class namespace for each slot name:
//!
//! ```python
//! type(S.x).__name__   # 'member_descriptor'
//! repr(S.x)            # "<member 'x' of 'S' objects>"
//! 'x' in S.__dict__    # True
//! 'x' in dir(S)        # True
//! ```
//!
//! The descriptor is a *data descriptor* (defines `__get__`, `__set__` and
//! `__delete__`): reading an unset slot raises `AttributeError`, assignment
//! stores into the instance's slot storage, and deletion clears it.  It is the
//! mechanism that lets a slotted instance store attributes without a per-
//! instance `__dict__`.
//!
//! pyrust exposes these as `BuiltinObject` values so that
//! `type(S.x).__name__ == "member_descriptor"` and `repr(S.x)` matches CPython.
//! The descriptor carries the owning class name (for `repr`) and the slot name
//! (for instance storage lookup); the interpreter intercepts get/set/delete on
//! a `member_descriptor` data descriptor and routes them to the instance's
//! slot storage.

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value};

pub struct MemberDescriptorState {
    /// The slot name (e.g. `"x"`), used both for `repr` and to key the
    /// instance's slot storage.
    pub attr_name: String,
    /// The owning class name (e.g. `"S"`), used only for `repr`.
    pub class_name: String,
}

pub struct MemberDescriptorOps;
pub const MEMBER_DESCRIPTOR_OPS: &MemberDescriptorOps = &MemberDescriptorOps;
pub const MEMBER_DESCRIPTOR_TYPE_NAME: &str = "member_descriptor";

impl BuiltinTypeOps for MemberDescriptorOps {
    fn type_name(&self) -> &'static str {
        MEMBER_DESCRIPTOR_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<MemberDescriptorState>()
            .expect("member_descriptor state");
        format!("<member '{}' of '{}' objects>", s.attr_name, s.class_name)
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }
}

/// Construct a `member_descriptor` value for the given slot / class name.
pub fn member_descriptor(attr_name: &str, class_name: &str) -> Value {
    let state: Box<dyn Any> = Box::new(MemberDescriptorState {
        attr_name: attr_name.to_string(),
        class_name: class_name.to_string(),
    });
    Value::builtin_object(MEMBER_DESCRIPTOR_OPS, state)
}

/// Extract the slot name from a `member_descriptor` Value, or `None` if the
/// value is not a `member_descriptor`.
pub fn as_member_descriptor(value: &Value) -> Option<String> {
    as_member_descriptor_full(value).map(|(slot, _)| slot)
}

/// Extract the `(slot_name, class_name)` from a `member_descriptor` Value, or
/// `None` if the value is not a `member_descriptor`.
pub fn as_member_descriptor_full(value: &Value) -> Option<(String, String)> {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != MEMBER_DESCRIPTOR_TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<MemberDescriptorState>()?;
    Some((s.attr_name.clone(), s.class_name.clone()))
}
