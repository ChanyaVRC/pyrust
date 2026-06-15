//! An "unbound method descriptor" — produced when an inherited or
//! locally-defined method is accessed *unbound* on a non-primitive subclass of
//! a builtin primitive (e.g. `OrderedDict.clear`, `OrderedDict.move_to_end`).
//!
//! In CPython these are C-implemented `method_descriptor`s owned by the
//! subclass type, so calling them with an unrelated receiver
//! (`OrderedDict.clear({1: 2})`) raises
//! `TypeError: descriptor 'clear' for 'collections.OrderedDict' objects
//! doesn't apply to a 'dict' object`.  pyrust resolves the same access to the
//! *shared* primitive sentinel (`BuiltinFunction("dict.clear")`) or to the
//! Python `UserFunction`, neither of which records the owning class — so the
//! receiver-class guard is impossible at call time.
//!
//! This wrapper closes that gap: it carries `(owning_class, callable)`.  The
//! interpreter detects it in `call_function_expanded` via
//! [`as_unbound_method_descriptor`], validates that the first positional
//! argument is an instance of the owning class (or a subclass), and only then
//! re-dispatches to the underlying `callable`.

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, ValueKind};

pub struct UnboundMethodDescriptorState {
    /// The class the descriptor was accessed on (e.g. the `OrderedDict`
    /// `PyClass` value).  Used both for the receiver-class check and for the
    /// `descriptor '<m>' for '<owner>' objects ...` error wording.
    pub owning_class: Value,
    /// The bare method name (e.g. `"clear"`).
    pub method: String,
    /// The underlying callable to re-dispatch to once the receiver passes the
    /// class check — a `BuiltinFunction("dict.clear")` sentinel or the Python
    /// `UserFunction`.
    pub callable: Value,
}

pub struct UnboundMethodDescriptorOps;
pub const UNBOUND_METHOD_DESCRIPTOR_OPS: &UnboundMethodDescriptorOps = &UnboundMethodDescriptorOps;
// CPython exposes these as `method_descriptor` objects.
pub const TYPE_NAME: &str = "method_descriptor";

impl BuiltinTypeOps for UnboundMethodDescriptorOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<UnboundMethodDescriptorState>()
            .expect("unbound method descriptor state");
        let owner = owner_display_name(&s.owning_class);
        format!("<method '{}' of '{}' objects>", s.method, owner)
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<UnboundMethodDescriptorState>()?;
        match name {
            "__name__" => Some(Value::string(s.method.clone())),
            "__qualname__" => {
                let owner = match s.owning_class.kind() {
                    ValueKind::PyClass(c) => c.borrow().qualname.clone(),
                    _ => return None,
                };
                Some(Value::string(format!("{owner}.{}", s.method)))
            }
            "__objclass__" => Some(s.owning_class.clone()),
            _ => None,
        }
    }
}

/// CPython's `tp_name`-style display name for the owning class:
/// `<module>.<qualname>`, dropping the module prefix when it is `builtins` or
/// absent.  `OrderedDict` → `collections.OrderedDict`.
fn owner_display_name(owning_class: &Value) -> String {
    let ValueKind::PyClass(c) = owning_class.kind() else {
        return "?".to_string();
    };
    let borrowed = c.borrow();
    let qualname = borrowed.qualname.clone();
    let module = borrowed
        .attrs
        .get("__module__")
        .and_then(|m| match m.kind() {
            ValueKind::Str(s) => Some(s.to_string()),
            _ => None,
        });
    match module {
        Some(m) if m != "builtins" && !m.is_empty() => format!("{m}.{qualname}"),
        _ => qualname,
    }
}

/// Construct an unbound-method-descriptor Value.
pub fn unbound_method_descriptor(
    owning_class: Value,
    method: impl Into<String>,
    callable: Value,
) -> Value {
    let state: Box<dyn Any> = Box::new(UnboundMethodDescriptorState {
        owning_class,
        method: method.into(),
        callable,
    });
    Value::builtin_object(UNBOUND_METHOD_DESCRIPTOR_OPS, state)
}

/// Extract `(owning_class, method, callable)` from an unbound-method-descriptor
/// Value, or `None`.
pub fn as_unbound_method_descriptor(value: &Value) -> Option<(Value, String, Value)> {
    let ValueKind::BuiltinObject { state, .. } = value.kind() else {
        return None;
    };
    // Match by downcasting to the (unique) state struct rather than by
    // `type_name`, which deliberately collides with the numeric
    // `method_descriptor` so both surface as `method_descriptor` to Python.
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<UnboundMethodDescriptorState>()?;
    Some((s.owning_class.clone(), s.method.clone(), s.callable.clone()))
}
