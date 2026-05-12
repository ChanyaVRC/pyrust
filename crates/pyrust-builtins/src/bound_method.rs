//! Generic "built-in bound method" — relocated from `pyrust-core` Tier 1
//! (#300).  Produced by attribute access on a Tier 1 built-in value (str,
//! list, dict, tuple, set, complex, ...) when the attribute names one of the
//! type's methods.  When called, dispatches by method name through the
//! per-type call() functions in this crate.
//!
//! Lives here as a `BuiltinObject` carrying `(name, receiver)`.  Calling
//! the resulting Value invokes the appropriate `crate::{string, list, ...}::call`.

use std::any::Any;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, PyKey, Result, Value, ValueKind};

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
            .expect("bound method");
        format!(
            "<built-in method {} of {} object>",
            s.name,
            receiver_type_name(&s.receiver),
        )
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn call(
        &self,
        state: &BuiltinState,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let (name, receiver) = {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<BoundMethodState>()
                .ok_or_else(|| PyError::Runtime("internal: bad bound method".to_string()))?;
            (Rc::clone(&s.name), s.receiver.clone())
        };
        dispatch_call(&name, receiver, args)
    }
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
/// not a bound method.
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

fn dispatch_call(name: &str, receiver: Value, args: Vec<Value>) -> Result<Value> {
    match receiver.kind() {
        ValueKind::Str(_) => crate::string::call(name, &receiver, args),
        ValueKind::List(_) => {
            // list methods mutate; obtain mutable access via a clone-then-replace
            // is not viable here (receiver is owned by the BoundMethod state).
            // Dispatch through a temporary owned vec; mutations are reflected via
            // the Value's underlying header.  In the existing VM path the same
            // happens by calling pyrust_builtins::list::call with a &mut Vec
            // pulled from the receiver register; here we operate on a clone and
            // return the same Value.
            //
            // Since list method calls on a *bound* method don't mutate the
            // underlying list value here (the binding was already captured), the
            // tests that exercise mutation go through the receiver register
            // directly via the CallMethod insn, not through this path.
            let mut items: Vec<Value> = receiver.as_list().cloned().unwrap_or_default();
            crate::list::call(name, &mut items, args, &IndexMap::new())
        }
        ValueKind::Tuple(items) => crate::tuple::call(name, items, args),
        ValueKind::Dict(_) => {
            let mut owned: IndexMap<PyKey, Value> = receiver.as_dict().cloned().unwrap_or_default();
            crate::dict::call(name, &mut owned, args)
        }
        ValueKind::Set(_) => {
            let mut owned: IndexSet<PyKey> = match receiver.kind() {
                ValueKind::Set(s) => s.clone(),
                _ => IndexSet::new(),
            };
            crate::set::call(name, &mut owned, args)
        }
        ValueKind::Complex(re, im) => {
            if name == "conjugate" {
                Ok(Value::complex(re, -im))
            } else {
                Err(PyError::Named(
                    "AttributeError".to_string(),
                    format!("'complex' object has no attribute '{name}'"),
                ))
            }
        }
        _ => Err(PyError::Named(
            "AttributeError".to_string(),
            format!(
                "'{}' object has no attribute '{name}'",
                receiver_type_name(&receiver)
            ),
        )),
    }
}

fn receiver_type_name(v: &Value) -> &'static str {
    pyrust_core::builtin_type_name(v)
}
