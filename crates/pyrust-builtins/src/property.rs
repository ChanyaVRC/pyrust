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
pub fn as_property(value: &Value) -> Option<(Rc<Value>, Rc<Value>, Rc<Value>, Option<u8>)> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<PropertyState>()?;
    Some((
        Rc::clone(&s.fget),
        Rc::clone(&s.fset),
        Rc::clone(&s.fdel),
        s.partial_slot,
    ))
}

fn borrow_state(state: &BuiltinState) -> Option<std::cell::Ref<'_, PropertyState>> {
    let borrow = state.borrow();
    if borrow.downcast_ref::<PropertyState>().is_some() {
        Some(std::cell::Ref::map(borrow, |b| {
            b.downcast_ref::<PropertyState>().unwrap()
        }))
    } else {
        None
    }
}
