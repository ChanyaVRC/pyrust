//! Unary numeric operations and truthiness for [`NumericSlot`].

use num_traits::Zero;

use super::NumericSlot;
use super::int_float::{magnitude, value_from_bigint};
use crate::errors::Result;
use crate::object_model::{PyBigInt, Value};

pub(super) fn slot_negative(slot: &NumericSlot<'_>) -> Result<Value> {
    Ok(match *slot {
        NumericSlot::Int(n) => match n.checked_neg() {
            Some(r) => Value::int(r),
            None => Value::bigint(-PyBigInt::from(n)),
        },
        NumericSlot::Bool(b) => Value::int(if b { -1 } else { 0 }),
        NumericSlot::Float(f) => Value::float(-f),
        NumericSlot::BigInt(b) => value_from_bigint(-b),
        NumericSlot::Complex(re, im) => Value::complex(-re, -im),
    })
}

pub(super) fn slot_positive(slot: &NumericSlot<'_>) -> Result<Value> {
    Ok(match *slot {
        NumericSlot::Int(n) => Value::int(n),
        NumericSlot::Bool(b) => Value::int(if b { 1 } else { 0 }),
        NumericSlot::Float(f) => Value::float(f),
        NumericSlot::BigInt(b) => Value::bigint(b.clone()),
        NumericSlot::Complex(re, im) => Value::complex(re, im),
    })
}

pub(super) fn slot_invert(slot: &NumericSlot<'_>) -> Result<Value> {
    match *slot {
        NumericSlot::Int(n) => Ok(Value::int(!n)),
        NumericSlot::Bool(b) => Ok(Value::int(if b { -2 } else { -1 })),
        NumericSlot::BigInt(b) => Ok(value_from_bigint(!b)),
        NumericSlot::Float(_) | NumericSlot::Complex(_, _) => Err(crate::type_err!(
            "bad operand type for unary ~: '{}'",
            slot.type_name()
        )),
    }
}

pub(super) fn slot_absolute(slot: &NumericSlot<'_>) -> Result<Value> {
    Ok(match *slot {
        NumericSlot::Int(n) => match n.checked_abs() {
            Some(r) => Value::int(r),
            None => Value::bigint(-PyBigInt::from(n)),
        },
        NumericSlot::Bool(b) => Value::int(if b { 1 } else { 0 }),
        NumericSlot::Float(f) => Value::float(f.abs()),
        NumericSlot::BigInt(b) => value_from_bigint(magnitude(b)),
        // Overflow/underflow-safe magnitude (CPython `_Py_c_abs` uses `hypot`).
        NumericSlot::Complex(re, im) => Value::float(re.hypot(im)),
    })
}

pub(super) fn slot_bool(slot: &NumericSlot<'_>) -> bool {
    match *slot {
        NumericSlot::Int(n) => n != 0,
        NumericSlot::Bool(b) => b,
        NumericSlot::Float(f) => f != 0.0,
        NumericSlot::BigInt(b) => !b.is_zero(),
        NumericSlot::Complex(re, im) => re != 0.0 || im != 0.0,
    }
}
