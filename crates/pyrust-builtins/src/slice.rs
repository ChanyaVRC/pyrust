//! `slice` built-in type.
//!
//! In CPython, `a[lo:hi:step]` on a `PyInstance` that defines `__getitem__`
//! calls `__getitem__` with a `slice(lo, hi, step)` object.  This module
//! provides the pyrust equivalent as a `BuiltinTypeOps`-backed `BuiltinObject`
//! so that user-defined `__getitem__` methods can inspect `.start`, `.stop`,
//! and `.step` and print/repr the slice value correctly.

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, PyKey, Value, py_hash_pykey};

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

    /// `slice` objects are hashable in CPython 3.12 when all their components
    /// are hashable (CPython changed slice to be hashable in 3.12; the common
    /// case of `slice(int_or_None, int_or_None, int_or_None)` is always
    /// hashable).
    ///
    /// Algorithm matches CPython 3.12 Objects/sliceobject.c::slice_hash: the same
    /// xxHash kernel as tuplehash but without the final length-mixing XOR step
    /// (i.e. just the per-element accumulation over 3 items with PRIME5 seed).
    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<SliceState>()?;
        let hstart = component_hash(&s.start)?;
        let hstop = component_hash(&s.stop)?;
        let hstep = component_hash(&s.step)?;
        Some(slice_hash_xxh([hstart, hstop, hstep]) as u64)
    }

    /// Expose `slice` as a hashable key so it can be used in sets/dicts.
    fn to_key(&self, state: &BuiltinState) -> Option<PyKey> {
        let hash = self.hash(state)?;
        let value = Value::builtin_object_shared(SLICE_OPS, state.clone());
        Some(PyKey::Object { hash, value })
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

/// Hash a single slice component using the CPython-compatible hash from `py_hash_pykey`.
///
/// Returns `None` if the component is unhashable (e.g. a list), making the
/// whole slice unhashable — matching CPython's "hashable if components are
/// hashable" contract.
fn component_hash(v: &Value) -> Option<i64> {
    let key = v.to_key()?;
    Some(py_hash_pykey(&key))
}

/// CPython 3.12 slice hash kernel (Objects/sliceobject.c::slice_hash).
///
/// Same xxHash per-element accumulation as tuplehash but WITHOUT the final
/// `acc += n ^ (PRIME5 ^ 3527539)` length-mixing step.  Accepts exactly 3
/// elements: (start_hash, stop_hash, step_hash).
fn slice_hash_xxh(component_hashes: [i64; 3]) -> i64 {
    const PRIME1: u64 = 11400714785074694791;
    const PRIME2: u64 = 14029467366897019727;
    const PRIME5: u64 = 2870177450012600261;

    #[inline(always)]
    fn xxstep(acc: u64, lane: u64) -> u64 {
        let acc = acc.wrapping_add(lane.wrapping_mul(PRIME2));
        let acc = (acc << 31) | (acc >> 33); // rotl31
        acc.wrapping_mul(PRIME1)
    }

    let mut acc: u64 = PRIME5;
    for h in component_hashes {
        acc = xxstep(acc, h as u64);
    }
    if acc == u64::MAX {
        acc = 1546275796;
    }
    acc as i64
}
