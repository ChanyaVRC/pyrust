//! `type.__call__` bound method-wrapper — produced when `__call__` is looked up
//! on a class object that does not define its own `__call__` in its MRO
//! (issue #2096).
//!
//! Every class is callable (to construct instances) via the metaclass slot
//! `type.__call__`.  CPython surfaces this as `C.__call__ ==
//! <method-wrapper '__call__' of type object at 0x...>` — a `method-wrapper`
//! bound to the class — so that `hasattr(C, '__call__')` agrees with
//! `callable(C)`.  We model it with a `BuiltinObject` carrying the bound class;
//! the interpreter detects it in `call_function_expanded` via
//! [`as_type_call_wrapper`] and re-dispatches the call onto the class itself
//! (so `C.__call__(...)` behaves exactly like `C(...)`).

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, ValueKind};

pub struct TypeCallWrapperState {
    /// The class object the wrapper is bound to (`C` in `C.__call__`).
    pub class: Value,
}

pub struct TypeCallWrapperOps;
pub const TYPE_CALL_WRAPPER_OPS: &TypeCallWrapperOps = &TypeCallWrapperOps;
// Matches CPython's `type(C.__call__).__name__`.
pub const TYPE_NAME: &str = "method-wrapper";

impl BuiltinTypeOps for TypeCallWrapperOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        // CPython: `<method-wrapper '__call__' of type object at 0x...>`.
        // The address is implementation-specific; report 0x0 (matching the
        // convention used by pyrust's other lightweight introspection
        // objects, e.g. the `code` object).
        "<method-wrapper '__call__' of type object at 0x0>".to_string()
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<TypeCallWrapperState>()?;
        match name {
            "__name__" | "__qualname__" => Some(Value::string("__call__")),
            // `C.__call__.__self__` is the bound class.
            "__self__" => Some(s.class.clone()),
            "__objclass__" => Some(s.class.clone()),
            _ => None,
        }
    }

    // `call` is not implemented — like `bound_method` / `super_bound_builtin`,
    // the interpreter intercepts via `as_type_call_wrapper` before the trait
    // default, because constructing the instance needs the interpreter handle.
}

/// Construct a `type.__call__` wrapper bound to `class`.
pub fn type_call_wrapper(class: Value) -> Value {
    let state: Box<dyn Any> = Box::new(TypeCallWrapperState { class });
    Value::builtin_object(TYPE_CALL_WRAPPER_OPS, state)
}

/// Extract the bound class from a `type.__call__` wrapper Value, or `None`.
pub fn as_type_call_wrapper(value: &Value) -> Option<Value> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    // The state downcast is the authoritative discriminator (`method-wrapper`
    // is a name pyrust could plausibly reuse for other slot wrappers later), so
    // match on it rather than on `type_name`.
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<TypeCallWrapperState>()?;
    Some(s.class.clone())
}
