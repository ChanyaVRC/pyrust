//! `classmethod` and `staticmethod` descriptor wrappers.
//!
//! This module provides two sets of `BuiltinObject` types:
//!
//! ## Any-value wrappers (for non-`UserFunction` arguments)
//!
//! CPython 3.12 accepts any object as the argument to `classmethod()` or
//! `staticmethod()`.  pyrust's existing path uses `Value::class_method` /
//! `Value::static_method` which require `Rc<UserFunction>`.  When the
//! argument is not a `UserFunction`, we wrap it in one of these
//! `BuiltinObject` types instead.
//!
//! - `ClassMethodAny` — descriptor wrapping an arbitrary value.  Exposes
//!   `__func__` and `__get__(instance, owner)`.
//! - `StaticMethodAny` — same for `staticmethod`.
//!
//! ## `__get__` binder objects (for `UserFunction` classmethods)
//!
//! When the user explicitly calls `cm.__get__(instance, owner)` where `cm` is
//! a `UserFunction` classmethod or staticmethod, the attribute lookup returns a
//! `ClassMethodGetBinder` / `StaticMethodGetBinder`.  The interpreter's
//! `call_function_expanded` recognises these and applies the descriptor
//! binding.

use std::any::Any;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, UserFunction, Value, ValueKind};

// ─── staticmethod any ────────────────────────────────────────────────────────

/// State for a `staticmethod` wrapping an arbitrary (non-`UserFunction`) value.
pub struct StaticMethodAnyState {
    pub wrapped: Value,
}

pub struct StaticMethodAnyOps;
pub const STATIC_METHOD_ANY_OPS: &StaticMethodAnyOps = &StaticMethodAnyOps;
pub const STATIC_TYPE_NAME: &str = "staticmethod";

impl BuiltinTypeOps for StaticMethodAnyOps {
    fn type_name(&self) -> &'static str {
        STATIC_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<StaticMethodAnyState>()
            .expect("StaticMethodAnyState");
        format!("<staticmethod({})>", s.wrapped.repr())
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        if name == "__func__" {
            let borrow = state.borrow();
            let s = borrow.downcast_ref::<StaticMethodAnyState>()?;
            return Some(s.wrapped.clone());
        }
        None
    }

    fn has_method(&self, name: &str) -> bool {
        name == "__get__"
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value, PyError> {
        if name != "__get__" {
            return Err(PyError::named(
                "AttributeError",
                format!("'staticmethod' object has no attribute '{name}'"),
            ));
        }
        // CPython 3.12: __get__(None, None) is invalid — instance and owner
        // cannot both be None.
        let instance = args.first().cloned().unwrap_or_else(Value::none);
        let owner = args.get(1).cloned().unwrap_or_else(Value::none);
        if matches!(instance.kind(), ValueKind::None) && matches!(owner.kind(), ValueKind::None) {
            return Err(PyError::named(
                "TypeError",
                "__get__(None, None) is invalid".to_string(),
            ));
        }
        // staticmethod.__get__(instance, owner) — returns the wrapped value
        // directly, ignoring both arguments (CPython Data Model §3.3.2).
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<StaticMethodAnyState>()
            .expect("StaticMethodAnyState");
        Ok(s.wrapped.clone())
    }
}

/// Construct a `staticmethod` descriptor wrapping an arbitrary `Value`.
pub fn static_method_any(wrapped: Value) -> Value {
    let state: Box<dyn Any> = Box::new(StaticMethodAnyState { wrapped });
    Value::builtin_object(STATIC_METHOD_ANY_OPS, state)
}

/// Return `Some(wrapped)` if `value` is a non-function staticmethod wrapper,
/// cloning the inner value.  Returns `None` for all other value kinds.
pub fn as_static_method_any(value: &Value) -> Option<Value> {
    with_static_method_any(value, |s| s.wrapped.clone())
}

/// Run `f` with a borrow of the underlying [`StaticMethodAnyState`].
pub fn with_static_method_any<R>(
    value: &Value,
    f: impl FnOnce(&StaticMethodAnyState) -> R,
) -> Option<R> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != STATIC_TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<StaticMethodAnyState>()?;
    Some(f(s))
}

// ─── classmethod any ─────────────────────────────────────────────────────────

/// State for a `classmethod` wrapping an arbitrary (non-`UserFunction`) value.
pub struct ClassMethodAnyState {
    pub wrapped: Value,
}

pub struct ClassMethodAnyOps;
pub const CLASS_METHOD_ANY_OPS: &ClassMethodAnyOps = &ClassMethodAnyOps;
pub const CLASS_TYPE_NAME: &str = "classmethod";

impl BuiltinTypeOps for ClassMethodAnyOps {
    fn type_name(&self) -> &'static str {
        CLASS_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ClassMethodAnyState>()
            .expect("ClassMethodAnyState");
        format!("<classmethod({})>", s.wrapped.repr())
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        if name == "__func__" {
            let borrow = state.borrow();
            let s = borrow.downcast_ref::<ClassMethodAnyState>()?;
            return Some(s.wrapped.clone());
        }
        None
    }

    fn has_method(&self, name: &str) -> bool {
        name == "__get__"
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value, PyError> {
        if name != "__get__" {
            return Err(PyError::named(
                "AttributeError",
                format!("'classmethod' object has no attribute '{name}'"),
            ));
        }
        // classmethod.__get__(instance, owner)
        // args[0] = instance (None or PyInstance — ignored for classmethod)
        // args[1] = owner (the owning class)
        //
        // CPython 3.12: __get__(None, None) is invalid — instance and owner
        // cannot both be None.
        let instance = args.first().cloned().unwrap_or_else(Value::none);
        let owner = args.get(1).cloned().unwrap_or_else(Value::none);
        if matches!(instance.kind(), ValueKind::None) && matches!(owner.kind(), ValueKind::None) {
            return Err(PyError::named(
                "TypeError",
                "__get__(None, None) is invalid".to_string(),
            ));
        }

        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ClassMethodAnyState>()
            .expect("ClassMethodAnyState");
        let wrapped = s.wrapped.clone();
        drop(borrow);
        let class_rc = match owner.kind() {
            ValueKind::PyClass(c) => Some(Rc::clone(c)),
            _ => None,
        };

        if let Some(class_rc) = class_rc {
            // If the wrapped value is a UserFunction we can form a proper
            // ClassBoundMethod so calling the result prepends cls.
            let user_fn = match wrapped.kind() {
                ValueKind::UserFunction(f) => Some(Rc::clone(f)),
                _ => None,
            };
            if let Some(f) = user_fn {
                return Ok(Value::class_bound_method(f, class_rc));
            }
        }

        // Non-callable wrapped value, or no recognisable owner class: return
        // the wrapped value directly.  CPython returns a method object
        // wrapping the non-callable, but pyrust's ClassBoundMethod variant
        // requires a UserFunction.  This is the next-closest approximation and
        // covers the non-callable descriptor case without crashing.
        Ok(wrapped)
    }
}

/// Construct a `classmethod` descriptor wrapping an arbitrary `Value`.
pub fn class_method_any(wrapped: Value) -> Value {
    let state: Box<dyn Any> = Box::new(ClassMethodAnyState { wrapped });
    Value::builtin_object(CLASS_METHOD_ANY_OPS, state)
}

/// Return `Some(wrapped)` if `value` is a non-function classmethod wrapper,
/// cloning the inner value.  Returns `None` for all other value kinds.
pub fn as_class_method_any(value: &Value) -> Option<Value> {
    with_class_method_any(value, |s| s.wrapped.clone())
}

/// Run `f` with a borrow of the underlying [`ClassMethodAnyState`].
pub fn with_class_method_any<R>(
    value: &Value,
    f: impl FnOnce(&ClassMethodAnyState) -> R,
) -> Option<R> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != CLASS_TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<ClassMethodAnyState>()?;
    Some(f(s))
}

// ─── __get__ binders for UserFunction classmethod / staticmethod ──────────────

/// Returned by `classmethod.__get__` when the descriptor wraps a
/// `UserFunction`.  Calling this value (via the interpreter's
/// `call_function_expanded` guard arm) creates a `ClassBoundMethod`.
pub struct ClassMethodGetBinder {
    pub func: Rc<UserFunction>,
}

pub struct ClassMethodGetBinderOps;
pub const CLASS_METHOD_GET_BINDER_OPS: &ClassMethodGetBinderOps = &ClassMethodGetBinderOps;
pub const CLASS_BINDER_TYPE_NAME: &str = "classmethod_get_binder";

impl BuiltinTypeOps for ClassMethodGetBinderOps {
    fn type_name(&self) -> &'static str {
        CLASS_BINDER_TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<classmethod.__get__ binder>".to_string()
    }
}

/// Construct a `classmethod.__get__` binder wrapping a `UserFunction`.
pub fn class_method_get_binder(func: Rc<UserFunction>) -> Value {
    let state: Box<dyn Any> = Box::new(ClassMethodGetBinder { func });
    Value::builtin_object(CLASS_METHOD_GET_BINDER_OPS, state)
}

/// Extract the `Rc<UserFunction>` from a `ClassMethodGetBinder` value, or
/// return `None` if the value is not one.
pub fn as_class_method_get_binder(value: &Value) -> Option<Rc<UserFunction>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != CLASS_BINDER_TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<ClassMethodGetBinder>()?;
    Some(Rc::clone(&s.func))
}

/// Returned by `staticmethod.__get__` when the descriptor wraps a
/// `UserFunction`.  Calling this value returns the underlying plain function.
pub struct StaticMethodGetBinder {
    pub func: Rc<UserFunction>,
}

pub struct StaticMethodGetBinderOps;
pub const STATIC_METHOD_GET_BINDER_OPS: &StaticMethodGetBinderOps = &StaticMethodGetBinderOps;
pub const STATIC_BINDER_TYPE_NAME: &str = "staticmethod_get_binder";

impl BuiltinTypeOps for StaticMethodGetBinderOps {
    fn type_name(&self) -> &'static str {
        STATIC_BINDER_TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<staticmethod.__get__ binder>".to_string()
    }
}

/// Construct a `staticmethod.__get__` binder wrapping a `UserFunction`.
pub fn static_method_get_binder(func: Rc<UserFunction>) -> Value {
    let state: Box<dyn Any> = Box::new(StaticMethodGetBinder { func });
    Value::builtin_object(STATIC_METHOD_GET_BINDER_OPS, state)
}

/// Extract the `Rc<UserFunction>` from a `StaticMethodGetBinder` value, or
/// return `None` if the value is not one.
pub fn as_static_method_get_binder(value: &Value) -> Option<Rc<UserFunction>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != STATIC_BINDER_TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<StaticMethodGetBinder>()?;
    Some(Rc::clone(&s.func))
}
