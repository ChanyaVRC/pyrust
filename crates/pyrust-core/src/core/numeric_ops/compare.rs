//! Cross-type numeric equality and partial ordering for [`NumericSlot`].

use std::cmp::Ordering;

use super::int_float::{bigint_float_cmp, bigint_float_eq, int_float_cmp, int_float_eq};
use super::{NumericCmpOp, NumericSlot};
use crate::errors::Result;
use crate::object_model::{PyBigInt, Value};

pub(super) fn eq_slot(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> bool {
    match (*lhs, *rhs) {
        (NumericSlot::Int(a), NumericSlot::Int(b)) => a == b,
        (NumericSlot::Bool(a), NumericSlot::Bool(b)) => a == b,
        (NumericSlot::Int(a), NumericSlot::Bool(b)) => a == (b as i64),
        (NumericSlot::Bool(a), NumericSlot::Int(b)) => (a as i64) == b,
        (NumericSlot::Float(a), NumericSlot::Float(b)) => a == b,
        (NumericSlot::Int(a), NumericSlot::Float(b))
        | (NumericSlot::Float(b), NumericSlot::Int(a)) => int_float_eq(a, b),
        (NumericSlot::Bool(a), NumericSlot::Float(b))
        | (NumericSlot::Float(b), NumericSlot::Bool(a)) => int_float_eq(a as i64, b),
        (NumericSlot::BigInt(a), NumericSlot::BigInt(b)) => a == b,
        (NumericSlot::BigInt(a), NumericSlot::Int(b))
        | (NumericSlot::Int(b), NumericSlot::BigInt(a)) => *a == PyBigInt::from(b),
        (NumericSlot::BigInt(a), NumericSlot::Bool(b))
        | (NumericSlot::Bool(b), NumericSlot::BigInt(a)) => *a == PyBigInt::from(b as i64),
        (NumericSlot::BigInt(a), NumericSlot::Float(b))
        | (NumericSlot::Float(b), NumericSlot::BigInt(a)) => bigint_float_eq(a, b),
        // Plain component equality — NO NaN bit-equality fallback here,
        // matching CPython (`complex(nan, 0) == complex(nan, 0)` is False).
        (NumericSlot::Complex(ar, ai), NumericSlot::Complex(br, bi)) => ar == br && ai == bi,
        (NumericSlot::Complex(ar, ai), NumericSlot::Float(b))
        | (NumericSlot::Float(b), NumericSlot::Complex(ar, ai)) => ai == 0.0 && ar == b,
        (NumericSlot::Complex(ar, ai), NumericSlot::Int(b))
        | (NumericSlot::Int(b), NumericSlot::Complex(ar, ai)) => ai == 0.0 && int_float_eq(b, ar),
        (NumericSlot::Complex(ar, ai), NumericSlot::Bool(b))
        | (NumericSlot::Bool(b), NumericSlot::Complex(ar, ai)) => {
            ai == 0.0 && int_float_eq(b as i64, ar)
        }
        (NumericSlot::Complex(ar, ai), NumericSlot::BigInt(b))
        | (NumericSlot::BigInt(b), NumericSlot::Complex(ar, ai)) => {
            ai == 0.0 && bigint_float_eq(b, ar)
        }
    }
}

/// Exact partial ordering for non-complex numeric pairs.
///
/// Returns `Err(TypeError)` when either operand is `Complex`, `Ok(None)`
/// when a `NaN` float is present, and `Ok(Some(Ordering))` otherwise.
pub(super) fn partial_cmp_slot(
    lhs: &NumericSlot<'_>,
    op_symbol: &str,
    rhs: &NumericSlot<'_>,
) -> Result<Option<Ordering>> {
    if lhs.is_complex() || rhs.is_complex() {
        return Err(crate::type_err!(
            "'{op_symbol}' not supported between instances of '{}' and '{}'",
            lhs.type_name(),
            rhs.type_name()
        ));
    }
    Ok(match (*lhs, *rhs) {
        (NumericSlot::Int(a), NumericSlot::Int(b)) => Some(a.cmp(&b)),
        (NumericSlot::Bool(a), NumericSlot::Bool(b)) => Some(a.cmp(&b)),
        (NumericSlot::Int(a), NumericSlot::Bool(b)) => Some(a.cmp(&(b as i64))),
        (NumericSlot::Bool(a), NumericSlot::Int(b)) => Some((a as i64).cmp(&b)),
        (NumericSlot::Float(a), NumericSlot::Float(b)) => a.partial_cmp(&b),
        (NumericSlot::Int(a), NumericSlot::Float(b)) => int_float_cmp(a, b),
        (NumericSlot::Float(a), NumericSlot::Int(b)) => int_float_cmp(b, a).map(Ordering::reverse),
        (NumericSlot::Bool(a), NumericSlot::Float(b)) => int_float_cmp(a as i64, b),
        (NumericSlot::Float(a), NumericSlot::Bool(b)) => {
            int_float_cmp(b as i64, a).map(Ordering::reverse)
        }
        (NumericSlot::BigInt(a), NumericSlot::BigInt(b)) => Some(a.cmp(b)),
        (NumericSlot::BigInt(a), NumericSlot::Int(b)) => Some(a.cmp(&PyBigInt::from(b))),
        (NumericSlot::Int(a), NumericSlot::BigInt(b)) => Some(PyBigInt::from(a).cmp(b)),
        (NumericSlot::BigInt(a), NumericSlot::Bool(b)) => Some(a.cmp(&PyBigInt::from(b as i64))),
        (NumericSlot::Bool(a), NumericSlot::BigInt(b)) => Some(PyBigInt::from(a as i64).cmp(b)),
        (NumericSlot::BigInt(a), NumericSlot::Float(b)) => bigint_float_cmp(a, b),
        (NumericSlot::Float(a), NumericSlot::BigInt(b)) => {
            bigint_float_cmp(b, a).map(Ordering::reverse)
        }
        (NumericSlot::Complex(_, _), _) | (_, NumericSlot::Complex(_, _)) => {
            unreachable!("complex operands handled above")
        }
    })
}

pub(super) fn slot_compare(
    lhs: &NumericSlot<'_>,
    op: NumericCmpOp,
    rhs: &NumericSlot<'_>,
) -> Result<bool> {
    match op {
        NumericCmpOp::Eq => Ok(eq_slot(lhs, rhs)),
        NumericCmpOp::Ne => Ok(!eq_slot(lhs, rhs)),
        NumericCmpOp::Lt | NumericCmpOp::Le | NumericCmpOp::Gt | NumericCmpOp::Ge => {
            // NaN is unordered: all of <, <=, >, >= evaluate to False.
            partial_cmp_slot(lhs, op.symbol(), rhs)
                .map(|opt| opt.is_some_and(|ord| op.matches_ordering(ord)))
        }
    }
}

/// Exact numeric equality across all five built-in numeric kinds, or `None`
/// when either operand is non-numeric.
#[inline]
pub fn numeric_eq(lhs: &Value, rhs: &Value) -> Option<bool> {
    let l = NumericSlot::from_value(lhs)?;
    let r = NumericSlot::from_value(rhs)?;
    Some(eq_slot(&l, &r))
}

/// Exact partial ordering across built-in numeric kinds for `sorted()` /
/// `min()` / `max()` and comparison dispatch.
///
/// Returns:
/// - `None` when either operand is non-numeric;
/// - `Some(Err(TypeError))` when either operand is `complex`;
/// - `Some(Ok(None))` when either operand is `NaN`;
/// - `Some(Ok(Some(ord)))` for ordered pairs.
#[inline]
pub fn numeric_partial_cmp(
    lhs: &Value,
    rhs: &Value,
    op_name: &str,
) -> Option<Result<Option<Ordering>>> {
    let l = NumericSlot::from_value(lhs)?;
    let r = NumericSlot::from_value(rhs)?;
    Some(partial_cmp_slot(&l, op_name, &r))
}
