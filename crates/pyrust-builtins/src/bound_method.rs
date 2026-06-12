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
//! All three mutable Tier 1 containers (`list`, `dict`, `set`) now share
//! their backing storage on `Value::clone`, so captured bound methods mutate
//! through to the original receiver — matching CPython.  `dict` always
//! worked (Rc-shared `Opaque::Dict`); list and set were brought into line in
//! #305 by routing their storage through `Rc<ListInner>` / `Rc<SetInner>`
//! (see `pyrust_core::ListInner` and `pyrust_core::SetInner`).
//!
//! As a side effect, simple aliasing (`b = a; b.append(x)`) also now
//! propagates for list and set, and `id(a) == id(b)` after the assignment.
//! The direct-call form `obj.method(args)` continues to work for all three
//! types via the `CallMethod` bytecode fast path, which is independent of
//! the bound-method capture path.

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
        let recv_type = pyrust_core::builtin_type_name(&s.receiver);
        // Issue #2397: a bound slot dunder (`[1].__len__`) presents as a CPython
        // `method-wrapper`, including its receiver's identity address.
        if pyrust_core::slot_wrapper_dunder(&format!("{recv_type}.{}", s.name)).is_some() {
            format!(
                "<method-wrapper '{}' of {} object at 0x{:x}>",
                s.name,
                recv_type,
                s.receiver.value_id().unwrap_or(0),
            )
        } else {
            format!("<built-in method {} of {} object>", s.name, recv_type)
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    /// Built-in bound methods are hashable in CPython (`hash([].append)` works).
    /// Hash = FNV-1a of the method name XOR receiver identity (value_id), with
    /// the CPython -1 → -2 sentinel remap applied.
    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<BoundMethodState>()?;
        // FNV-1a over the method name bytes.
        let mut name_hash: u64 = 14695981039346656037u64;
        for b in s.name.as_bytes() {
            name_hash ^= *b as u64;
            name_hash = name_hash.wrapping_mul(1099511628211u64);
        }
        // Receiver identity (stable per Python object; 0 for types without an id).
        let recv_id = s.receiver.value_id().unwrap_or(0) as u64;
        let h = name_hash ^ recv_id;
        // Remap the -1 sentinel (u64::MAX as i64 == -1) to -2 (u64::MAX - 1).
        Some(if h == u64::MAX { u64::MAX - 1 } else { h })
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
/// captured Value.  After #305, `Value::clone` on list/dict/set shares
/// the backing storage with the original, so `as_*_mut` on the returned
/// receiver propagates mutations to the captured Value.
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

/// Issue #2397: returns `true` if `value` is a bound builtin slot dunder
/// (`[1].__len__`), which CPython presents as a `method-wrapper` rather than
/// the generic `builtin_function_or_method`.  Drives `type(...).__name__`.
pub fn is_method_wrapper(value: &Value) -> bool {
    let Some((name, receiver)) = as_bound_method(value) else {
        return false;
    };
    let recv_type = pyrust_core::builtin_type_name(&receiver);
    pyrust_core::slot_wrapper_dunder(&format!("{recv_type}.{name}")).is_some()
}
