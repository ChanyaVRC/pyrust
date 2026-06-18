//! `__call__` method-wrapper — produced when `__call__` is looked up on a
//! callable object that does not define its own Python-level `__call__`.
//!
//! Two cases share this wrapper:
//!
//! * **Classes** (issue #2096): every class is callable (to construct
//!   instances) via the metaclass slot `type.__call__`.  CPython surfaces this
//!   as `C.__call__ == <method-wrapper '__call__' of type object at 0x...>`.
//! * **Functions / builtins** (issue #2550): plain functions, lambdas, and
//!   builtin functions are callable, and CPython exposes `f.__call__ ==
//!   <method-wrapper '__call__' of function object at 0x...>` (and
//!   `... of builtin_function_or_method object ...` for builtins) so that
//!   `hasattr(f, '__call__')` is `True`.
//!
//! We model it with a `BuiltinObject` carrying the bound callable plus the
//! owner's type name (for the repr); the interpreter detects it in
//! `call_function_expanded` via [`as_type_call_wrapper`] and re-dispatches the
//! call onto the bound callable (so `C.__call__(...)` behaves exactly like
//! `C(...)`, and `f.__call__(...)` like `f(...)`).

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, ValueKind};

pub struct TypeCallWrapperState {
    /// The callable the wrapper is bound to (`C` in `C.__call__`, `f` in
    /// `f.__call__`).
    pub callable: Value,
    /// The type name of the owner object, used only in `repr`.  CPython reports
    /// `<method-wrapper '__call__' of <owner> object at 0x...>` — `type object`
    /// for a class, `function object` for a function,
    /// `builtin_function_or_method object` for a builtin.
    pub owner: &'static str,
}

pub struct TypeCallWrapperOps;
pub const TYPE_CALL_WRAPPER_OPS: &TypeCallWrapperOps = &TypeCallWrapperOps;
// Matches CPython's `type(C.__call__).__name__`.
pub const TYPE_NAME: &str = "method-wrapper";

impl BuiltinTypeOps for TypeCallWrapperOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        // CPython: `<method-wrapper '__call__' of <owner> object at 0x...>`.
        // The address is implementation-specific; report 0x0 (matching the
        // convention used by pyrust's other lightweight introspection
        // objects, e.g. the `code` object).
        let owner = state
            .borrow()
            .downcast_ref::<TypeCallWrapperState>()
            .map(|s| s.owner)
            .unwrap_or("type");
        format!("<method-wrapper '__call__' of {owner} object at 0x0>")
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<TypeCallWrapperState>()?;
        match name {
            "__name__" => Some(Value::string("__call__")),
            // CPython reports the wrapper's `__qualname__` as
            // `<owner-type>.__call__` (`function.__call__`,
            // `builtin_function_or_method.__call__`, `type.__call__` for a
            // class, `method-wrapper.__call__` for a nested wrapper) while
            // `__name__` stays the bare slot name.
            "__qualname__" => Some(Value::string(format!("{}.__call__", s.owner))),
            // `C.__call__.__self__` is the bound callable.
            "__self__" => Some(s.callable.clone()),
            "__objclass__" => Some(s.callable.clone()),
            _ => None,
        }
    }

    // `call` is not implemented — like `bound_method` / `super_bound_builtin`,
    // the interpreter intercepts via `as_type_call_wrapper` before the trait
    // default, because re-dispatching the call needs the interpreter handle.
}

/// Construct a `type.__call__` wrapper bound to `class` (issue #2096).
pub fn type_call_wrapper(class: Value) -> Value {
    call_wrapper(class, "type")
}

/// Construct a `__call__` wrapper bound to an arbitrary callable, reporting
/// `owner` in its repr (`"function"` / `"builtin_function_or_method"` for
/// issue #2550).
pub fn call_wrapper(callable: Value, owner: &'static str) -> Value {
    let state: Box<dyn Any> = Box::new(TypeCallWrapperState { callable, owner });
    Value::builtin_object(TYPE_CALL_WRAPPER_OPS, state)
}

/// Extract the bound callable from a `__call__` wrapper Value, or `None`.
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
    Some(s.callable.clone())
}
