//! Generic "built-in bound method" — relocated from `pyrust-core` Tier 1
//! (#300).  Produced by attribute access on a Tier 1 built-in value (str,
//! list, dict, tuple, set, complex, ...) when the attribute names one of the
//! type's methods.
//!
//! Lives here as a `BuiltinObject` carrying `(name, receiver)`.  Calling the
//! resulting Value is dispatched **by the interpreter** through
//! [`as_bound_method`] before the generic trait `call` path is reached.  The
//! trait `call` is intentionally not implemented — see the mutation-propagation
//! caveat below.
//!
//! # Mutation propagation (issue #305)
//!
//! Bound methods on mutable Tier 1 containers diverge from CPython depending
//! on the underlying storage of the receiver:
//!
//! - `dict`: storage is `Rc<RefCell<IndexMap>>` inside `Opaque::Dict`, so
//!   `Value::clone` shares it.  Mutations via a captured bound method (e.g.
//!   `m = d.update; m(...)`) propagate to the original.  **Works as CPython.**
//! - `list`: storage is a `Vec<Value>` in a NaN-boxed pool header.
//!   `Value::clone` allocates a fresh header with a deep-copied vector, so a
//!   captured bound method's receiver is a *value copy* — `m = lst.append;
//!   m(4)` would silently no-op against the original.  Rather than silently
//!   diverge from CPython, the interpreter's bound-method dispatch arm
//!   (`crates/pyrust/src/interpreter/runtime/calls.rs`) raises a clear
//!   `TypeError` for captured *mutating* list methods.  Read-only captured
//!   methods (`count`, `index`, `copy`) work as expected — they don't depend
//!   on mutation propagation.
//! - `set`: storage is a plain `IndexSet` inside `Opaque::Set`; same story
//!   as list — captured mutating set methods raise `TypeError`, read-only
//!   ones (`union`, `isdisjoint`, …) work fine.
//!
//! This divergence is part of pyrust's broader value-vs-reference semantics
//! gap for list/set (basic `b = a; b.append(x)` also fails to alias).  The
//! direct call form `obj.method(args)` works for all three types via the
//! `CallMethod` bytecode fast path, which hands the VM a mutable register
//! reference and bypasses bound-method capture entirely.
//!
//! Fixing the captured-bound-method case structurally for list/set requires
//! Rc-sharing the backing storage (a NaN-box layout change for list, an
//! `Opaque::Set` rewrap for set).  That's a larger refactor than is
//! warranted while the broader value-semantics gap is still open; the
//! explicit `TypeError` is the pragmatic middle ground — it stops the silent
//! divergence and points users at the working call form.  Tracked in #305.

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
/// not a bound method.  The returned `receiver` is a `Value::clone` of the
/// captured Value.
///
/// **Caveat (issue #305):** for list and set receivers, `Value::clone`
/// deep-copies the backing storage, so `as_*_mut` on the returned receiver
/// mutates a private copy — the mutation will not be visible through the
/// original Value the caller bound the method on.  The interpreter's
/// dispatch arm guards against this by raising `TypeError` for captured
/// *mutating* list/set methods (see the module-level docs).  For dict
/// receivers, storage is `Rc<RefCell<_>>`-shared inside `Opaque::Dict`, so
/// mutations propagate correctly.
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
