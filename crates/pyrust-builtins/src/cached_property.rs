//! `functools.cached_property` descriptor.
//!
//! Lives here as a `BuiltinObject` whose descriptor-protocol invocation
//! is dispatched from the interpreter's `get_attr` path — the same hook
//! that handles `property`.  This module exposes the state struct and
//! accessors; the actual descriptor invocation (calling the wrapped
//! function with the instance and stashing the result in
//! `instance.__dict__[name]`) lives in
//! `pyrust/src/interpreter/runtime/attributes.rs` because it needs interpreter
//! access to dispatch the wrapped callable.
//!
//! ## Why a `BuiltinObject` (not a `class { … }` block in functools.rs)
//!
//! pyrust's descriptor protocol invokes only `property` from
//! `get_attr` — extending it to recognise an arbitrary user-class
//! `__get__` method would balloon the hot attribute-lookup path.
//! `cached_property` plugs into the same fast lane as `property` by
//! reusing this BuiltinObject pattern.
//!
//! ## Attribute-name capture
//!
//! CPython's `cached_property` learns the attribute name through
//! `__set_name__(owner, name)`, called by class-body machinery at class
//! creation time.  pyrust doesn't dispatch `__set_name__` yet, so we
//! fall back to the wrapped function's own name (`func.__name__`,
//! captured at decoration time).  That matches the common case
//! `@cached_property def x(self): …` where the desired attribute name
//! and the function name agree.

use std::any::Any;

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, PyError, Result, Value, ValueKind, builtin_ops_is,
};

/// State for a `cached_property` descriptor.
pub struct CachedPropertyState {
    /// The wrapped accessor function.  Called with `self` on first
    /// access; the result is then stashed into the instance's __dict__
    /// under `attr_name` so subsequent reads bypass this descriptor.
    pub func: Value,
    /// Attribute name used to stash the cached value.  Captured at
    /// decoration time from `func.__name__` (pyrust doesn't yet
    /// dispatch `__set_name__`, which is how CPython captures it).
    pub attr_name: String,
}

pub struct CachedPropertyOps;
pub const CACHED_PROPERTY_OPS: &CachedPropertyOps = &CachedPropertyOps;
pub const TYPE_NAME: &str = "cached_property";

impl BuiltinTypeOps for CachedPropertyOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<functools.cached_property object>".to_string()
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        match name {
            // `__set_name__(owner, name)` — invoked by class-body
            // machinery in CPython to capture the attribute name.
            // pyrust doesn't dispatch it automatically, but accepting
            // the explicit call lets user code that explicitly invokes
            // it (often as a workaround for class-body decorator-scope
            // limitations) round-trip cleanly.  We *do* honour the
            // captured name here so the descriptor stashes under that
            // attribute if it differs from the wrapped function's own
            // name.
            "__set_name__" => {
                if args.len() != 2 {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "__set_name__() takes exactly 2 arguments, got {}",
                            args.len()
                        ),
                    ));
                }
                if let ValueKind::Str(s) = args[1].kind() {
                    let mut borrow = state.borrow_mut();
                    if let Some(st) = borrow.downcast_mut::<CachedPropertyState>() {
                        st.attr_name = s.to_string();
                    }
                }
                Ok(Value::none())
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'{}' object has no attribute '{name}'", TYPE_NAME),
            )),
        }
    }

    fn has_method(&self, name: &str) -> bool {
        name == "__set_name__"
    }
}

/// Construct a new `cached_property` value.  `attr_name` is the slot
/// used in the instance's `__dict__` to stash the computed value.
pub fn cached_property(func: Value, attr_name: String) -> Value {
    let state: Box<dyn Any> = Box::new(CachedPropertyState { func, attr_name });
    Value::builtin_object(CACHED_PROPERTY_OPS, state)
}

/// Run `f` with a borrow of the underlying [`CachedPropertyState`].
/// Returns `Some(f(state))` if `value` is a `cached_property`, or
/// `None` otherwise.
pub fn with_cached_property<R>(
    value: &Value,
    f: impl FnOnce(&CachedPropertyState) -> R,
) -> Option<R> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<CachedPropertyOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<CachedPropertyState>()?;
    Some(f(s))
}
