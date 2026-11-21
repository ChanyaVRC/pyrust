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
    /// Attribute name captured by `__set_name__(owner, name)`.  Starts as
    /// `None` until `__set_name__` runs (matching CPython 3.12's
    /// `cached_property.attrname`, issue #2987).
    pub attr_name: Option<String>,
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

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let st = borrow.downcast_ref::<CachedPropertyState>()?;
        match name {
            "func" => Some(st.func.clone()),
            "attrname" => Some(match &st.attr_name {
                Some(attr) => Value::string(attr.clone()),
                None => Value::none(),
            }),
            "__doc__" => Some(match st.func.kind() {
                ValueKind::UserFunction(func) => func.doc.borrow().clone(),
                _ => Value::none(),
            }),
            _ => None,
        }
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        match name {
            // `__set_name__(owner, name)` — invoked by class creation
            // (`make_class_call_set_name`) or explicitly by user code.
            // Mirrors CPython 3.12's `functools.cached_property.__set_name__`.
            "__set_name__" => {
                if args.len() != 2 {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "cached_property.__set_name__() takes 3 positional arguments but {} were given",
                            args.len() + 1
                        ),
                    ));
                }
                let new_name = match args[1].kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "__set_name__() name must be a str".to_string(),
                        ));
                    }
                };
                let mut borrow = state.borrow_mut();
                if let Some(st) = borrow.downcast_mut::<CachedPropertyState>() {
                    match &st.attr_name {
                        None => st.attr_name = Some(new_name),
                        Some(existing) if existing == &new_name => {}
                        Some(existing) => {
                            let existing_repr = Value::string(existing.clone()).repr_raw();
                            let new_repr = Value::string(new_name).repr_raw();
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "Cannot assign the same cached_property to two different names ({existing_repr} and {new_repr})."
                                ),
                            ));
                        }
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

/// Construct a new `cached_property` value with `attrname = None` until
/// `__set_name__(owner, name)` is called.
pub fn cached_property(func: Value) -> Value {
    let state: Box<dyn Any> = Box::new(CachedPropertyState {
        func,
        attr_name: None,
    });
    Value::builtin_object(CACHED_PROPERTY_OPS, state)
}

/// True if `value` is a `functools.cached_property` descriptor.
#[inline]
pub fn is_cached_property(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::BuiltinObject { ops, .. } if builtin_ops_is::<CachedPropertyOps>(ops)
    )
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
