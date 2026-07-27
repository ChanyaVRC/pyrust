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
    /// Per-captured-method `__module__` slot. CPython initializes built-in
    /// bound methods to None, permits arbitrary assignment, and resets the
    /// slot to None on deletion.
    pub module: Value,
}

pub struct BoundMethodOps;
pub const BOUND_METHOD_OPS: &BoundMethodOps = &BoundMethodOps;
pub const TYPE_NAME: &str = "builtin_function_or_method";

#[inline]
fn has_bound_method_ops(ops: &dyn BuiltinTypeOps) -> bool {
    pyrust_core::builtin_ops_is::<BoundMethodOps>(ops)
}

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
        if pyrust_core::builtin_callable_presentation(&format!("{recv_type}.{}", s.name))
            .is_wrapper_descriptor()
        {
            format!(
                "<method-wrapper '{}' of {} object at 0x{:x}>",
                s.name,
                recv_type,
                s.receiver.value_id().unwrap_or(0),
            )
        } else {
            // Issue #2422: a bound builtin method (`[1].append`) reports its
            // receiver's identity address, matching CPython's
            // `<built-in method append of list object at 0x...>` (the bound
            // method's `__self__` is the receiver, so `id(__self__)`).
            format!(
                "<built-in method {} of {} object at 0x{:x}>",
                s.name,
                recv_type,
                s.receiver.value_id().unwrap_or(0),
            )
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    /// CPython compares captured built-in methods by `(function, self)`,
    /// rather than by the transient bound-method allocation. The Rust name is
    /// the stable function identity for Tier 1 methods; the receiver must use
    /// object identity, not Python value equality (`1` and `True` compare
    /// equal as values but are not the same receiver).
    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let borrow = state.borrow();
        let Some(method) = borrow.downcast_ref::<BoundMethodState>() else {
            return false;
        };
        let ValueKind::BuiltinObject {
            state: other_state, ..
        } = other.kind()
        else {
            return false;
        };
        let other_borrow = other_state.borrow();
        let Some(other_method) = other_borrow.downcast_ref::<BoundMethodState>() else {
            return false;
        };
        method.name == other_method.name
            && same_receiver_identity(&method.receiver, &other_method.receiver)
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

fn same_receiver_identity(left: &Value, right: &Value) -> bool {
    match (left.kind(), right.kind()) {
        (ValueKind::None, ValueKind::None)
        | (ValueKind::NotImplemented, ValueKind::NotImplemented)
        | (ValueKind::Ellipsis, ValueKind::Ellipsis) => true,
        (ValueKind::Bool(a), ValueKind::Bool(b)) => a == b,
        (ValueKind::Int(a), ValueKind::Int(b)) => a == b,
        (ValueKind::Float(a), ValueKind::Float(b)) => a.to_bits() == b.to_bits(),
        (ValueKind::Complex(ar, ai), ValueKind::Complex(br, bi)) => {
            ar.to_bits() == br.to_bits() && ai.to_bits() == bi.to_bits()
        }
        (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) => Rc::ptr_eq(a, b),
        (ValueKind::PyClass(a), ValueKind::PyClass(b)) => Rc::ptr_eq(a, b),
        (ValueKind::UserFunction(a), ValueKind::UserFunction(b)) => Rc::ptr_eq(a, b),
        (ValueKind::BuiltinFunction(_), ValueKind::BuiltinFunction(_)) => {
            match (left.as_function_rc(), right.as_function_rc()) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            }
        }
        (ValueKind::Generator(a), ValueKind::Generator(b)) => Rc::ptr_eq(a, b),
        (ValueKind::Bytes(a), ValueKind::Bytes(b)) => Rc::ptr_eq(a, b),
        (ValueKind::PyModule(a), ValueKind::PyModule(b)) => Rc::ptr_eq(a, b),
        (ValueKind::Str(_), ValueKind::Str(_))
        | (ValueKind::BigInt(_), ValueKind::BigInt(_))
        | (ValueKind::List(_), ValueKind::List(_))
        | (ValueKind::Set(_), ValueKind::Set(_))
        | (ValueKind::Dict(_), ValueKind::Dict(_))
        | (ValueKind::Tuple(_), ValueKind::Tuple(_))
        | (ValueKind::BoundMethod { .. }, ValueKind::BoundMethod { .. })
        | (ValueKind::ClassBoundMethod { .. }, ValueKind::ClassBoundMethod { .. })
        | (ValueKind::SuperProxy { .. }, ValueKind::SuperProxy { .. })
        | (ValueKind::SuperProxyClass { .. }, ValueKind::SuperProxyClass { .. })
        | (ValueKind::SuperProxyUnbound { .. }, ValueKind::SuperProxyUnbound { .. })
        | (ValueKind::BuiltinObject { .. }, ValueKind::BuiltinObject { .. }) => {
            matches!(
                (left.value_id(), right.value_id()),
                (Some(a), Some(b)) if a == b
            )
        }
        _ => false,
    }
}

/// Construct a bound-method Value pointing at `receiver.name`.
pub fn bound_method(name: impl Into<String>, receiver: Value) -> Value {
    let state: Box<dyn Any> = Box::new(BoundMethodState {
        name: Rc::new(name.into()),
        receiver,
        module: Value::none(),
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
    if !has_bound_method_ops(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<BoundMethodState>()?;
    Some((Rc::clone(&s.name), s.receiver.clone()))
}

/// Read the per-object `__module__` slot of a captured built-in method.
pub fn module_value(value: &Value) -> Option<Value> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !has_bound_method_ops(ops) {
        return None;
    }
    let borrow = state.borrow();
    let method = borrow.downcast_ref::<BoundMethodState>()?;
    Some(method.module.clone())
}

/// Assign the per-object `__module__` slot of a captured built-in method.
/// Returns false when `value` is another BuiltinObject category.
pub fn set_module(value: &Value, module: Value) -> bool {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return false;
    };
    if !has_bound_method_ops(ops) {
        return false;
    }
    let mut borrow = state.borrow_mut();
    let Some(method) = borrow.downcast_mut::<BoundMethodState>() else {
        return false;
    };
    method.module = module;
    true
}

/// Returns true if `value` is a bound-method built-in.  Mirrors what
/// CPython exposes as `isinstance(x, builtin_function_or_method)`.
pub fn is_bound_method(value: &Value) -> bool {
    matches!(value.kind(), ValueKind::BuiltinObject { ops, .. } if has_bound_method_ops(ops))
}

/// Issue #2397: returns `true` if `value` is a bound builtin slot dunder
/// (`[1].__len__`), which CPython presents as a `method-wrapper` rather than
/// the generic `builtin_function_or_method`.  Drives `type(...).__name__`.
pub fn is_method_wrapper(value: &Value) -> bool {
    let Some((name, receiver)) = as_bound_method(value) else {
        return false;
    };
    let recv_type = pyrust_core::builtin_type_name(&receiver);
    pyrust_core::builtin_callable_presentation(&format!("{recv_type}.{name}"))
        .is_wrapper_descriptor()
}
