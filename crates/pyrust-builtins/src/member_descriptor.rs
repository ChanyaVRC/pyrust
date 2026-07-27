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
//! The class dictionary stores a cycle-free descriptor with a weak owner.
//! Whenever Python exposes that descriptor, [`export_member_descriptor`]
//! returns a cached detached twin which strongly retains the owner. The class
//! never stores that twin, so this matches CPython's owner lifetime without a
//! class → descriptor → class `Rc` cycle.

use std::any::Any;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use pyrust_core::{
    BuiltinState, BuiltinTypeOps, MemberSlotId, PyClass, Value, ValueKind, builtin_ops_is,
};

enum MemberDescriptorOwner {
    Internal(Weak<RefCell<PyClass>>),
    Exported(Rc<RefCell<PyClass>>),
}

impl MemberDescriptorOwner {
    fn get(&self) -> Option<Rc<RefCell<PyClass>>> {
        match self {
            Self::Internal(owner) => owner.upgrade(),
            Self::Exported(owner) => Some(Rc::clone(owner)),
        }
    }
}

pub struct MemberDescriptorState {
    /// The slot name (e.g. `"x"`), used for presentation and lookup.
    pub attr_name: String,
    /// Physical storage identity. Same-named descriptors in different layout
    /// layers must never alias.
    pub slot_id: MemberSlotId,
    /// Internal descriptors use a weak owner; exported detached descriptors
    /// use a strong owner and are never stored back on that class.
    owner: MemberDescriptorOwner,
    /// Cached detached state. This is weak, so dropping every external
    /// descriptor immediately releases its strong owner.
    exported_state: Weak<RefCell<Box<dyn Any>>>,
    /// Display fallback used only if the owner has otherwise been dropped.
    pub fallback_owner_name: String,
}

pub struct MemberDescriptorOps;
pub const MEMBER_DESCRIPTOR_OPS: &MemberDescriptorOps = &MemberDescriptorOps;
pub const MEMBER_DESCRIPTOR_TYPE_NAME: &str = "member_descriptor";

pub struct MemberDescriptorInfo {
    pub attr_name: String,
    pub owner_name: String,
    pub owner: Option<Rc<RefCell<PyClass>>>,
    pub slot_id: MemberSlotId,
}

impl BuiltinTypeOps for MemberDescriptorOps {
    fn type_name(&self) -> &'static str {
        MEMBER_DESCRIPTOR_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<MemberDescriptorState>()
            .expect("member_descriptor state");
        let owner_name = s
            .owner
            .get()
            .map(|owner| owner.borrow().name.clone())
            .unwrap_or_else(|| s.fallback_owner_name.clone());
        format!("<member '{}' of '{}' objects>", s.attr_name, owner_name)
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<MemberDescriptorState>()?;
        match name {
            "__name__" => Some(Value::string(s.attr_name.clone())),
            "__objclass__" => s.owner.get().map(Value::py_class),
            _ => None,
        }
    }
}

/// Construct a `member_descriptor` value for the given slot / owning class.
pub fn member_descriptor(attr_name: &str, owner: &Rc<RefCell<PyClass>>) -> Value {
    let state: Box<dyn Any> = Box::new(MemberDescriptorState {
        attr_name: attr_name.to_string(),
        slot_id: MemberSlotId::fresh(),
        owner: MemberDescriptorOwner::Internal(Rc::downgrade(owner)),
        exported_state: Weak::new(),
        fallback_owner_name: owner.borrow().name.clone(),
    });
    Value::builtin_object(MEMBER_DESCRIPTOR_OPS, state)
}

/// Extract the slot name from a `member_descriptor` Value, or `None` if the
/// value is not a `member_descriptor`.
pub fn as_member_descriptor(value: &Value) -> Option<String> {
    as_member_descriptor_full(value).map(|info| info.attr_name)
}

/// Return whether `value` is a native member descriptor without borrowing or
/// cloning its descriptor state.
#[inline]
pub fn is_member_descriptor(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::BuiltinObject { ops, .. } if builtin_ops_is::<MemberDescriptorOps>(ops)
    )
}

/// Return the Python-exposed form of a member descriptor.
///
/// The internal class-dictionary value holds its owner weakly. The first
/// external read creates a detached state with a strong owner and caches only a
/// weak pointer to it; concurrent/repeated reads therefore preserve descriptor
/// identity, while the detached state and owner are released as soon as the
/// last external descriptor is dropped.
pub fn export_member_descriptor(value: &Value) -> Option<Value> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<MemberDescriptorOps>(ops) {
        return None;
    }
    let state = Rc::clone(state);
    let (attr_name, slot_id, owner, fallback_owner_name, cached) = {
        let borrow = state.borrow();
        let descriptor = borrow.downcast_ref::<MemberDescriptorState>()?;
        if matches!(&descriptor.owner, MemberDescriptorOwner::Exported(_)) {
            return Some(value.clone());
        }
        (
            descriptor.attr_name.clone(),
            descriptor.slot_id,
            descriptor.owner.get()?,
            descriptor.fallback_owner_name.clone(),
            descriptor.exported_state.upgrade(),
        )
    };
    if let Some(cached) = cached {
        return Some(Value::builtin_object_shared(MEMBER_DESCRIPTOR_OPS, cached));
    }

    let exported_state: BuiltinState = Rc::new(RefCell::new(Box::new(MemberDescriptorState {
        attr_name,
        slot_id,
        owner: MemberDescriptorOwner::Exported(owner),
        exported_state: Weak::new(),
        fallback_owner_name,
    })));
    {
        let mut borrow = state.borrow_mut();
        let descriptor = borrow
            .downcast_mut::<MemberDescriptorState>()
            .expect("member_descriptor state");
        descriptor.exported_state = Rc::downgrade(&exported_state);
    }
    Some(Value::builtin_object_shared(
        MEMBER_DESCRIPTOR_OPS,
        exported_state,
    ))
}

/// Extract the presentation and physical identity of a `member_descriptor`.
///
/// `owner_identity` is `None` only when both the class and every instance of it
/// have been dropped; callers must reject any receiver in that case.
pub fn as_member_descriptor_full(value: &Value) -> Option<MemberDescriptorInfo> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<MemberDescriptorOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<MemberDescriptorState>()?;
    let owner = s.owner.get();
    let owner_name = owner
        .as_ref()
        .map(|owner| owner.borrow().name.clone())
        .unwrap_or_else(|| s.fallback_owner_name.clone());
    Some(MemberDescriptorInfo {
        attr_name: s.attr_name.clone(),
        owner_name,
        owner,
        slot_id: s.slot_id,
    })
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;

    #[test]
    fn exported_descriptor_retains_owner_without_a_class_cycle() {
        let owner = Rc::new(RefCell::new(PyClass::new(
            "Owner",
            "Owner",
            None,
            IndexMap::new(),
        )));
        let internal = member_descriptor("slot", &owner);
        owner
            .borrow_mut()
            .attrs
            .insert("slot".to_string(), internal.clone());
        let owner_weak = Rc::downgrade(&owner);

        let first = export_member_descriptor(&internal).expect("member descriptor");
        let second = export_member_descriptor(&internal).expect("cached member descriptor");
        let (
            ValueKind::BuiltinObject {
                state: first_state, ..
            },
            ValueKind::BuiltinObject {
                state: second_state,
                ..
            },
        ) = (first.kind(), second.kind())
        else {
            panic!("exports must be builtin objects");
        };
        assert!(Rc::ptr_eq(first_state, second_state));

        drop(owner);
        assert!(
            owner_weak.upgrade().is_some(),
            "an exported descriptor must retain its owner"
        );
        drop(first);
        drop(second);
        assert!(
            owner_weak.upgrade().is_none(),
            "dropping every export must break the owner/class-dict cycle"
        );
        assert!(
            as_member_descriptor_full(&internal)
                .unwrap()
                .owner
                .is_none()
        );
    }
}
