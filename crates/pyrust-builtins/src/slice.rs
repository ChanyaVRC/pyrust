//! `slice` built-in type.
//!
//! In CPython, `a[lo:hi:step]` on a `PyInstance` that defines `__getitem__`
//! calls `__getitem__` with a `slice(lo, hi, step)` object.  This module
//! provides the pyrust equivalent as a `BuiltinTypeOps`-backed `BuiltinObject`
//! so that user-defined `__getitem__` methods can inspect `.start`, `.stop`,
//! and `.step` and print/repr the slice value correctly.

use std::any::Any;

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, PyBigIntSign, PyError, PyToPrimitive, Result, Value, ValueKind,
};

pub const TYPE_NAME: &str = "slice";
pub const SLICE_OPS: &SliceOps = &SliceOps;

/// Method names exposed by `slice` for `dir()`.
pub const METHODS: &[&str] = &["indices"];

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

    // `slice` does not implement `hash` or `to_key` here.  Hashing goes
    // entirely through `hash_value_with_interp` in the interpreter, which
    // recurses into each component with full error propagation.  That path
    // surfaces the correct per-component error message (e.g.
    // "unhashable type: 'list'" when a bound is a list) instead of the
    // misleading "unhashable type: 'slice'" that `component_hash` used to
    // produce.  `value_to_pykey` in expr.rs stores the resulting hash in a
    // `PyKey::Object` so dict/set lookups remain consistent with `hash()`.
    //
    // All call sites that previously called `Value::to_key()` directly
    // (Counter, defaultdict, require_key in collections.rs) were updated to
    // use `interp.value_to_pykey()` in PR #905, closing the parity gap
    // that was documented here.

    fn setattr(&self, _state: &BuiltinState, name: &str, _value: Value) -> Result<()> {
        Err(PyError::named(
            "AttributeError",
            format!("readonly attribute '{name}'"),
        ))
    }

    fn has_method(&self, name: &str) -> bool {
        name == "indices"
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        if name != "indices" {
            return Err(PyError::named(
                "AttributeError",
                format!("'slice' object has no attribute '{name}'"),
            ));
        }
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<SliceState>()
            .expect("SliceOps::call_method: bad state");
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "slice.indices() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        }
        let length = slice_index_from_value(&args[0])?;
        if length < 0 {
            return Err(PyError::named(
                "ValueError",
                "length should not be negative".to_string(),
            ));
        }
        let (start, stop, step) = compute_indices(length, &s.start, &s.stop, &s.step)?;
        Ok(Value::tuple(vec![
            Value::int(start),
            Value::int(stop),
            Value::int(step),
        ]))
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

/// Return the `(start, stop, step)` triple of a slice `Value`, or `None` if the
/// value is not a slice object.  Used by the interpreter's ordering comparison
/// to compare two slices as `(start, stop, step)` tuples, matching CPython's
/// `slice_richcompare` (issue #2127).
pub fn slice_fields(value: &Value) -> Option<(Value, Value, Value)> {
    if let ValueKind::BuiltinObject { ops, state } = value.kind() {
        if ops.type_name() == TYPE_NAME {
            let borrow = state.borrow();
            let s = borrow.downcast_ref::<SliceState>()?;
            return Some((s.start.clone(), s.stop.clone(), s.step.clone()));
        }
    }
    None
}

/// Convert the `length` argument of `slice.indices()` to an `i64`.
///
/// Accepts `int`, `bool` (as 0/1), and `BigInt` (clamped to `i64` range).
/// Any other type raises `TypeError` with the `__index__`-style message,
/// matching CPython's `PyArg_ParseTuple` behaviour for the length argument.
fn slice_index_from_value(value: &Value) -> Result<i64> {
    match value.kind() {
        ValueKind::Int(i) => Ok(i),
        ValueKind::Bool(b) => Ok(if b { 1 } else { 0 }),
        ValueKind::BigInt(big) => Ok(match PyToPrimitive::to_i64(big) {
            Some(i) => i,
            None => match big.sign() {
                PyBigIntSign::Minus => i64::MIN,
                _ => i64::MAX,
            },
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(value)
            ),
        )),
    }
}

/// Convert a slice-bound value (start, stop, or step) to an `i64`.
///
/// Same integer coercion as `slice_index_from_value`, but raises the
/// CPython-matching error message from `_PyEval_SliceIndex` when the
/// value is not an integer type:
/// `"slice indices must be integers or None or have an __index__ method"`
fn slice_bound_from_value(value: &Value) -> Result<i64> {
    match value.kind() {
        ValueKind::Int(i) => Ok(i),
        ValueKind::Bool(b) => Ok(if b { 1 } else { 0 }),
        ValueKind::BigInt(big) => Ok(match PyToPrimitive::to_i64(big) {
            Some(i) => i,
            None => match big.sign() {
                PyBigIntSign::Minus => i64::MIN,
                _ => i64::MAX,
            },
        }),
        _ => Err(PyError::named(
            "TypeError",
            "slice indices must be integers or None or have an __index__ method".to_string(),
        )),
    }
}

/// Compute `(start, stop, step)` for `slice.indices(length)`.
///
/// Implements the same algorithm as CPython's `PySlice_Unpack` +
/// `PySlice_AdjustIndices` (`Objects/sliceobject.c`).  `length` must be
/// non-negative (caller's responsibility).
fn compute_indices(length: i64, lo: &Value, hi: &Value, st: &Value) -> Result<(i64, i64, i64)> {
    // Step.
    let step = if st.is_none() {
        1
    } else {
        let s = slice_bound_from_value(st)?;
        if s == 0 {
            return Err(PyError::named(
                "ValueError",
                "slice step cannot be zero".to_string(),
            ));
        }
        s
    };

    // Resolve a bound value and clamp it to the valid range for this step direction.
    // For step > 0: valid range is [0, length].
    // For step < 0: valid range is [-1, length-1].
    let clamp_bound = |v: &Value, default_pos: i64, default_neg: i64| -> Result<i64> {
        if v.is_none() {
            return Ok(if step > 0 { default_pos } else { default_neg });
        }
        let raw = slice_bound_from_value(v)?;
        // Resolve negative index relative to length.
        let resolved = if raw < 0 { raw + length } else { raw };
        Ok(if step > 0 {
            resolved.clamp(0, length)
        } else {
            resolved.clamp(-1, length - 1)
        })
    };

    let start = clamp_bound(lo, 0, length - 1)?;
    let stop = clamp_bound(hi, length, -1)?;

    Ok((start, stop, step))
}
