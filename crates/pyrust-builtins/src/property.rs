//! `@property` descriptor.
//!
//! Eliminated from `pyrust-core`'s Tier 1 (#294): three-callable descriptor
//! with no compile-time-specializable payload.  Lives here as a
//! `BuiltinObject` whose ops dispatch the descriptor-protocol calls.

use std::any::Any;
use std::rc::Rc;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, ValueKind};

/// Property descriptor state.  Each slot is either a callable `Value` or
/// `Value::none()` (meaning "not set").
pub struct PropertyState {
    pub fget: Rc<Value>,
    pub fset: Rc<Value>,
    pub fdel: Rc<Value>,
    /// If `Some(slot)`, this descriptor is the intermediate result of
    /// `prop.setter` / `prop.deleter` / `prop.getter`: calling it with a
    /// function returns a new Property with that slot replaced.
    /// `slot` is 0=fget, 1=fset, 2=fdel.
    pub partial_slot: Option<u8>,
}

pub struct PropertyOps;
pub const PROPERTY_OPS: &PropertyOps = &PropertyOps;
pub const TYPE_NAME: &str = "property";

impl BuiltinTypeOps for PropertyOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        if borrow_state(state).is_some_and(|s| s.partial_slot.is_some()) {
            "<property accessor partial>".to_string()
        } else {
            "<property object>".to_string()
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }
}

/// Construct a new `@property` value.  Pass `Value::none()` for any
/// accessor that is not set.
pub fn property(fget: Value, fset: Value, fdel: Value) -> Value {
    let state: Box<dyn Any> = Box::new(PropertyState {
        fget: Rc::new(fget),
        fset: Rc::new(fset),
        fdel: Rc::new(fdel),
        partial_slot: None,
    });
    Value::builtin_object(PROPERTY_OPS, state)
}

/// Returned by `prop.setter(fn)` — creates a new Property with fset replaced.
pub fn property_setter_partial(fget: Value, fdel: Value) -> Value {
    accessor_partial(1, fget, Value::none(), fdel)
}

/// Returned by `prop.deleter(fn)` — creates a new Property with fdel replaced.
pub fn property_deleter_partial(fget: Value, fset: Value) -> Value {
    accessor_partial(2, fget, fset, Value::none())
}

/// Returned by `prop.getter(fn)` — creates a new Property with fget replaced.
pub fn property_getter_partial(fset: Value, fdel: Value) -> Value {
    accessor_partial(0, Value::none(), fset, fdel)
}

fn accessor_partial(slot: u8, fget: Value, fset: Value, fdel: Value) -> Value {
    let state: Box<dyn Any> = Box::new(PropertyState {
        fget: Rc::new(fget),
        fset: Rc::new(fset),
        fdel: Rc::new(fdel),
        partial_slot: Some(slot),
    });
    Value::builtin_object(PROPERTY_OPS, state)
}

/// Extract the property fields (fget, fset, fdel, partial_slot) from a value.
/// Returns None if the value isn't a property.
///
/// Note: clones all three `Rc<Value>` slots. Prefer [`with_property`] when
/// only a subset of slots is needed — see #304.
pub fn as_property(value: &Value) -> Option<(Rc<Value>, Rc<Value>, Rc<Value>, Option<u8>)> {
    with_property(value, |s| {
        (
            Rc::clone(&s.fget),
            Rc::clone(&s.fset),
            Rc::clone(&s.fdel),
            s.partial_slot,
        )
    })
}

/// Run `f` with a borrow of the underlying [`PropertyState`].  Returns
/// `Some(f(state))` if `value` is a `property`, or `None` otherwise.
///
/// Callers should clone only the slots they need inside the closure — this
/// avoids the unconditional 3 × `Rc::clone` cost of [`as_property`].
pub fn with_property<R>(value: &Value, f: impl FnOnce(&PropertyState) -> R) -> Option<R> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<PropertyState>()?;
    Some(f(s))
}

/// Return `Some(partial_slot)` if `value` is a `property`, or `None`.
///
/// Cheaper than [`as_property`] when only the partial-slot flag is needed:
/// no `Rc::clone` calls on the accessor slots.
pub fn property_partial_slot(value: &Value) -> Option<Option<u8>> {
    with_property(value, |s| s.partial_slot)
}

fn borrow_state(state: &BuiltinState) -> Option<std::cell::Ref<'_, PropertyState>> {
    std::cell::Ref::filter_map(state.borrow(), |b| b.downcast_ref::<PropertyState>()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_property_extracts_fget_only() {
        let getter = Value::builtin_function("len");
        let prop = property(getter.clone(), Value::none(), Value::none());
        let fget = with_property(&prop, |s| Rc::clone(&s.fget)).expect("is property");
        // fget should be the getter, others untouched.
        assert!(!fget.is_none());
        assert_eq!(
            with_property(&prop, |s| s.partial_slot),
            Some(None),
            "plain property has no partial_slot",
        );
    }

    #[test]
    fn with_property_returns_none_for_non_property() {
        let not_a_property = Value::int(42);
        assert!(with_property(&not_a_property, |_| ()).is_none());
        assert!(property_partial_slot(&not_a_property).is_none());
    }

    #[test]
    fn property_partial_slot_distinguishes_kinds() {
        let plain = property(Value::none(), Value::none(), Value::none());
        assert_eq!(property_partial_slot(&plain), Some(None));

        let setter_partial = property_setter_partial(Value::none(), Value::none());
        assert_eq!(property_partial_slot(&setter_partial), Some(Some(1)));

        let getter_partial = property_getter_partial(Value::none(), Value::none());
        assert_eq!(property_partial_slot(&getter_partial), Some(Some(0)));

        let deleter_partial = property_deleter_partial(Value::none(), Value::none());
        assert_eq!(property_partial_slot(&deleter_partial), Some(Some(2)));
    }

    #[test]
    fn as_property_still_returns_all_four() {
        let getter = Value::builtin_function("len");
        let setter = Value::builtin_function("id");
        let prop = property(getter, setter, Value::none());
        let (fget, fset, fdel, slot) = as_property(&prop).expect("is property");
        assert!(!fget.is_none());
        assert!(!fset.is_none());
        assert!(fdel.is_none());
        assert_eq!(slot, None);
    }
}
