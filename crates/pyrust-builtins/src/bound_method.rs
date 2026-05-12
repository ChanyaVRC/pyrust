//! Generic "built-in bound method" — relocated from `pyrust-core` Tier 1
//! (#300).  Produced by attribute access on a Tier 1 built-in value (str,
//! list, dict, tuple, set, complex, ...) when the attribute names one of the
//! type's methods.
//!
//! Lives here as a `BuiltinObject` carrying `(name, receiver)`.  Calling the
//! resulting Value is dispatched **by the interpreter** through
//! [`as_bound_method`] before the generic trait `call` path is reached, so the
//! interpreter can take a mutable handle to the receiver register and have
//! mutations on list/dict/set propagate.  The trait `call` is intentionally
//! not implemented — bound methods on mutable containers need a mutable
//! receiver, which only the interpreter can provide.

use std::any::Any;
use std::rc::Rc;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, ValueKind};

pub struct BoundMethodState {
    pub name: Rc<String>,
    pub receiver: Value,
}

pub struct BoundMethodOps;
pub const BOUND_METHOD_OPS: &BoundMethodOps = &BoundMethodOps;
pub const TYPE_NAME: &str = "builtin_function_or_method";

impl BuiltinTypeOps for BoundMethodOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<BoundMethodState>()
            .expect("bound method state");
        format!(
            "<built-in method {} of {} object>",
            s.name,
            pyrust_core::builtin_type_name(&s.receiver),
        )
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    // `call` is intentionally *not* implemented.  Bound methods on mutable
    // Tier 1 containers (list, dict, set) require a mutable handle to the
    // receiver register that only the interpreter has — calling through this
    // trait path would silently clone the receiver and discard mutations.
    // The interpreter intercepts via `as_bound_method` before reaching the
    // default trait `call`, so this path returns the trait default's
    // `TypeError: '...' object is not callable` only if something bypasses
    // the interpreter's dispatch.
}

/// Construct a bound-method Value pointing at `receiver.name`.
pub fn bound_method(name: impl Into<String>, receiver: Value) -> Value {
    let state: Box<dyn Any> = Box::new(BoundMethodState {
        name: Rc::new(name.into()),
        receiver,
    });
    Value::builtin_object(BOUND_METHOD_OPS, state)
}

/// Extract `(name, receiver)` from a bound-method Value, or None if it's
/// not a bound method.  The returned `receiver` is a clone of the captured
/// Value — callers that need mutation on mutable Tier 1 containers must
/// obtain `&mut Vec<Value>` / `&mut IndexMap` / `&mut IndexSet` via the
/// `Value::as_*_mut` accessors on the returned receiver.
pub fn as_bound_method(value: &Value) -> Option<(Rc<String>, Value)> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<BoundMethodState>()?;
    Some((Rc::clone(&s.name), s.receiver.clone()))
}

/// Returns true if `value` is a bound-method built-in.  Mirrors what
/// CPython exposes as `isinstance(x, builtin_function_or_method)`.
pub fn is_bound_method(value: &Value) -> bool {
    matches!(value.kind(), ValueKind::BuiltinObject { ops, .. } if ops.type_name() == TYPE_NAME)
}
