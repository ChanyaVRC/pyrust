//! A "super-bound builtin" — produced by SuperProxy attribute access when the
//! resolved parent-class attribute is a `BuiltinFunction` sentinel (e.g.
//! `list.__init__`).
//!
//! CPython binds the instance to the descriptor at `super().__init__` access
//! time.  We replicate this with a `BuiltinObject` carrying `(fn_name,
//! instance)`.  The interpreter detects it in `call_function_expanded` via
//! [`as_super_bound_builtin`] and prepends the instance to the args slice
//! before calling the registry dispatch — identical to what
//! `invoke_class_method` does for the normal `__init__` call path.

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, ValueKind};

pub struct SuperBoundBuiltinState {
    /// Registry name of the builtin (e.g. `"list.__init__"`).
    pub fn_name: String,
    /// The instance that `super()` was invoked on.
    pub instance: Value,
}

pub struct SuperBoundBuiltinOps;
pub const SUPER_BOUND_BUILTIN_OPS: &SuperBoundBuiltinOps = &SuperBoundBuiltinOps;
// Internal tag; never exposed to Python — CPython shows these as
// `builtin_function_or_method` but we need a distinct marker internally.
pub const TYPE_NAME: &str = "super_bound_builtin";

impl BuiltinTypeOps for SuperBoundBuiltinOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<SuperBoundBuiltinState>()
            .expect("super bound builtin state");
        format!("<built-in method {}>", s.fn_name)
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    /// Expose `__name__`, `__qualname__`, `__self__`, `__module__`, and
    /// `__doc__` so that e.g. `int.__init_subclass__.__name__` works.
    /// CPython exposes these on `builtin_function_or_method` objects.
    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<SuperBoundBuiltinState>()?;
        match name {
            "__name__" => {
                // "object.__init_subclass__" → "__init_subclass__"
                let bare = s.fn_name.rsplit('.').next().unwrap_or(&s.fn_name);
                Some(Value::string(bare))
            }
            "__qualname__" => {
                // Bind to the instance's class name if available.
                // E.g. for `int.__init_subclass__`, returns "int.__init_subclass__".
                let cls_name = match s.instance.kind() {
                    ValueKind::PyClass(c) => c.borrow().name.clone(),
                    _ => pyrust_core::builtin_type_name(&s.instance).into_owned(),
                };
                let bare = s.fn_name.rsplit('.').next().unwrap_or(&s.fn_name);
                Some(Value::string(format!("{cls_name}.{bare}")))
            }
            "__self__" => Some(s.instance.clone()),
            // CPython's classmethod_descriptor.__module__ is None (unlike
            // top-level builtin_function_or_method which returns "builtins").
            "__module__" => Some(Value::none()),
            "__doc__" => Some(Value::none()),
            _ => None,
        }
    }

    // `call` is not implemented — same reason as `bound_method`: we need the
    // interpreter handle to call the registry dispatch.  The interpreter
    // intercepts via `as_super_bound_builtin` before the trait default.
}

/// Construct a super-bound-builtin Value carrying `fn_name` and `instance`.
pub fn super_bound_builtin(fn_name: String, instance: Value) -> Value {
    let state: Box<dyn Any> = Box::new(SuperBoundBuiltinState { fn_name, instance });
    Value::builtin_object(SUPER_BOUND_BUILTIN_OPS, state)
}

/// Extract `(fn_name, instance)` from a super-bound-builtin Value, or None.
pub fn as_super_bound_builtin(value: &Value) -> Option<(String, Value)> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<SuperBoundBuiltinState>()?;
    Some((s.fn_name.clone(), s.instance.clone()))
}
