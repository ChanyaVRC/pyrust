//! `slice` built-in type.
//!
//! In CPython, `a[lo:hi:step]` on a `PyInstance` that defines `__getitem__`
//! calls `__getitem__` with a `slice(lo, hi, step)` object.  This module
//! provides the pyrust equivalent as a `BuiltinTypeOps`-backed `BuiltinObject`
//! so that user-defined `__getitem__` methods can inspect `.start`, `.stop`,
//! and `.step` and print/repr the slice value correctly.

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, Value};

pub const TYPE_NAME: &str = "slice";
pub const SLICE_OPS: &SliceOps = &SliceOps;

pub struct SliceState {
    pub start: Value,
    pub stop: Value,
    pub step: Value,
}

pub struct SliceOps;

impl BuiltinTypeOps for SliceOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<SliceState>()
            .expect("SliceOps: bad state");
        format!(
            "slice({}, {}, {})",
            s.start.repr(),
            s.stop.repr(),
            s.step.repr()
        )
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<SliceState>()?;
        match name {
            "start" => Some(s.start.clone()),
            "stop" => Some(s.stop.clone()),
            "step" => Some(s.step.clone()),
            _ => None,
        }
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        use pyrust_core::ValueKind;
        let borrow = state.borrow();
        let s = match borrow.downcast_ref::<SliceState>() {
            Some(s) => s,
            None => return false,
        };
        if let ValueKind::BuiltinObject {
            ops: other_ops,
            state: other_state,
        } = other.kind()
        {
            if other_ops.type_name() != TYPE_NAME {
                return false;
            }
            let other_borrow = other_state.borrow();
            let other_s = match other_borrow.downcast_ref::<SliceState>() {
                Some(s) => s,
                None => return false,
            };
            s.start == other_s.start && s.stop == other_s.stop && s.step == other_s.step
        } else {
            false
        }
    }

    /// `slice` objects are hashable in CPython only when all their components
    /// are hashable.  We return `None` here (unhashable) to match CPython's
    /// common behaviour: `hash(slice(1, 3))` raises `TypeError: unhashable type:
    /// 'slice'`.
    fn hash(&self, _state: &BuiltinState) -> Option<u64> {
        None
    }

    fn setattr(&self, _state: &BuiltinState, name: &str, _value: Value) -> Result<(), PyError> {
        Err(PyError::named(
            "AttributeError",
            format!("readonly attribute '{name}'"),
        ))
    }
}

/// Construct a `slice` value from three optional bounds.
///
/// `None` values are represented as Python `None`.  Matches CPython's
/// `slice(start, stop, step)` constructor where missing args become `None`.
pub fn make_slice(start: Option<Value>, stop: Option<Value>, step: Option<Value>) -> Value {
    let start = start.unwrap_or_else(Value::none);
    let stop = stop.unwrap_or_else(Value::none);
    let step = step.unwrap_or_else(Value::none);
    let state: Box<dyn Any> = Box::new(SliceState { start, stop, step });
    Value::builtin_object(SLICE_OPS, state)
}
