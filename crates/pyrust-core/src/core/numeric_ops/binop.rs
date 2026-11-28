//! Binary arithmetic, bitwise, and shift operations for [`NumericSlot`].

use num_traits::{ToPrimitive, Zero};

use super::complex::{complex_div, complex_pow};
use super::int_float::{
    bigint_divmod_floor, bigint_true_divide, float_divmod, int_pow_promoting, py_mod_i64,
    value_from_bigint,
};
use super::{NumericBinOp, NumericSlot};
use crate::errors::Result;
use crate::object_model::{PyBigInt, PyBigIntSign, PyPow, Value};

/// Shared numeric promotion path for closed binary arithmetic (`+`, `-`, `*`):
/// complex -> checked `i64` (with `BigInt` fallback) -> `f64` -> `BigInt`.
#[inline]
fn promote_closed_arith(
    lhs: &NumericSlot<'_>,
    rhs: &NumericSlot<'_>,
    complex_op: impl FnOnce((f64, f64), (f64, f64)) -> (f64, f64),
    i64_checked_op: impl FnOnce(i64, i64) -> Option<i64>,
    f64_op: impl FnOnce(f64, f64) -> f64,
    bigint_op: impl FnOnce(PyBigInt, PyBigInt) -> PyBigInt,
) -> Result<Value> {
    if lhs.is_complex() || rhs.is_complex() {
        let a = lhs.to_complex()?;
        let b = rhs.to_complex()?;
        let (re, im) = complex_op(a, b);
        return Ok(Value::complex(re, im));
    }
    match (lhs.as_i64_coerced(), rhs.as_i64_coerced()) {
        (Some(a), Some(b)) => Ok(match i64_checked_op(a, b) {
            Some(res) => Value::int(res),
            None => Value::bigint(bigint_op(PyBigInt::from(a), PyBigInt::from(b))),
        }),
        _ => match (*lhs, *rhs) {
            (NumericSlot::Float(a), NumericSlot::Float(b)) => Ok(Value::float(f64_op(a, b))),
            (NumericSlot::Float(a), _) => rhs.to_float().map(|b| Value::float(f64_op(a, b))),
            (_, NumericSlot::Float(b)) => lhs.to_float().map(|a| Value::float(f64_op(a, b))),
            _ => Ok(value_from_bigint(bigint_op(
                lhs.to_bigint(),
                rhs.to_bigint(),
            ))),
        },
    }
}

/// Shared promotion path for bitwise binary operators (`&`, `|`, `^`).
#[inline]
fn promote_bitwise(
    op: NumericBinOp,
    lhs: &NumericSlot<'_>,
    rhs: &NumericSlot<'_>,
    bool_op: impl FnOnce(bool, bool) -> bool,
    i64_op: impl FnOnce(i64, i64) -> i64,
    bigint_op: impl FnOnce(PyBigInt, PyBigInt) -> PyBigInt,
) -> Result<Value> {
    if lhs.is_float_or_complex() || rhs.is_float_or_complex() {
        return Err(lhs.unsupported_binop(op, rhs));
    }
    if let (NumericSlot::Bool(a), NumericSlot::Bool(b)) = (*lhs, *rhs) {
        return Ok(Value::bool_(bool_op(a, b)));
    }
    if let (Some(a), Some(b)) = (lhs.as_i64_coerced(), rhs.as_i64_coerced()) {
        return Ok(Value::int(i64_op(a, b)));
    }
    Ok(value_from_bigint(bigint_op(
        lhs.to_bigint(),
        rhs.to_bigint(),
    )))
}

#[inline]
fn bigint_pow_nonneg(base: PyBigInt, exp: &PyBigInt) -> Result<Value> {
    match exp.to_u64_digits().1.as_slice() {
        [e] => Ok(value_from_bigint(PyPow::pow(base, *e))),
        [] => Ok(Value::int(1)),
        _ => Err(crate::overflow_err!("exponent too large for ** to compute")),
    }
}

pub(super) fn slot_add(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    promote_closed_arith(
        lhs,
        rhs,
        |a, b| (a.0 + b.0, a.1 + b.1),
        i64::checked_add,
        |a, b| a + b,
        |a, b| a + b,
    )
}

pub(super) fn slot_sub(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    promote_closed_arith(
        lhs,
        rhs,
        |a, b| (a.0 - b.0, a.1 - b.1),
        i64::checked_sub,
        |a, b| a - b,
        |a, b| a - b,
    )
}

pub(super) fn slot_mul(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    promote_closed_arith(
        lhs,
        rhs,
        // (ar+ai*j) * (br+bi*j) = (ar*br - ai*bi) + (ar*bi + ai*br)j
        |a, b| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0),
        i64::checked_mul,
        |a, b| a * b,
        |a, b| a * b,
    )
}

pub(super) fn slot_div(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    if lhs.is_complex() || rhs.is_complex() {
        let a = lhs.to_complex()?;
        let b = rhs.to_complex()?;
        return complex_div(a, b);
    }
    match (lhs.as_i64_coerced(), rhs.as_i64_coerced()) {
        // Small int/int fast path: when both operands are exactly
        // representable as f64 (|n| < 2^53), IEEE-754 division is itself
        // correctly rounded and matches CPython byte-for-byte — no BigInt
        // allocation. Larger magnitudes route through the exact divider.
        (Some(a), Some(b)) => {
            if b == 0 {
                Err(crate::zerodiv_err!("division by zero"))
            } else if a.unsigned_abs() < (1 << 53) && b.unsigned_abs() < (1 << 53) {
                Ok(Value::float(a as f64 / b as f64))
            } else {
                bigint_true_divide(&PyBigInt::from(a), &PyBigInt::from(b)).map(Value::float)
            }
        }
        _ if !matches!(*lhs, NumericSlot::Float(_)) && !matches!(*rhs, NumericSlot::Float(_)) => {
            bigint_true_divide(&lhs.to_bigint(), &rhs.to_bigint()).map(Value::float)
        }
        // At least one Float operand: CPython divides as doubles and the
        // ZeroDivisionError wording is "float division by zero".
        _ => {
            let a = lhs.to_float()?;
            let b = rhs.to_float()?;
            if b == 0.0 {
                return Err(crate::zerodiv_err!("float division by zero"));
            }
            Ok(Value::float(a / b))
        }
    }
}

pub(super) fn slot_floordiv(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    if lhs.is_complex() || rhs.is_complex() {
        return Err(lhs.unsupported_binop(NumericBinOp::FloorDiv, rhs));
    }
    match (lhs.as_i64_coerced(), rhs.as_i64_coerced()) {
        (Some(a), Some(b)) => {
            if b == 0 {
                Err(crate::zerodiv_err!("integer division or modulo by zero"))
            } else {
                let modulo = py_mod_i64(a, b);
                // `a - modulo` / `a_adj / b` can overflow near i64::MIN;
                // promote to BigInt for exact floor division (#485).
                if let Some(q) = a.checked_sub(modulo).and_then(|a_adj| a_adj.checked_div(b)) {
                    Ok(Value::int(q))
                } else {
                    let (q, _) = bigint_divmod_floor(&PyBigInt::from(a), &PyBigInt::from(b));
                    Ok(Value::bigint(q))
                }
            }
        }
        _ if !matches!(*lhs, NumericSlot::Float(_)) && !matches!(*rhs, NumericSlot::Float(_)) => {
            let a = lhs.to_bigint();
            let b = rhs.to_bigint();
            if b.is_zero() {
                Err(crate::zerodiv_err!("integer division or modulo by zero"))
            } else {
                let (q, _) = bigint_divmod_floor(&a, &b);
                Ok(value_from_bigint(q))
            }
        }
        // At least one Float operand: CPython fmod-based float_divmod so
        // infinities/signed zeros match and `a // b == divmod(a, b)[0]` (#2025).
        _ => {
            let a = lhs.to_float()?;
            let b = rhs.to_float()?;
            if b == 0.0 {
                return Err(crate::zerodiv_err!("float floor division by zero"));
            }
            let (div, _) = float_divmod(a, b);
            Ok(Value::float(div))
        }
    }
}

pub(super) fn slot_mod(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    if lhs.is_complex() || rhs.is_complex() {
        return Err(lhs.unsupported_binop(NumericBinOp::Mod, rhs));
    }
    match (lhs.as_i64_coerced(), rhs.as_i64_coerced()) {
        (Some(a), Some(b)) => {
            if b == 0 {
                Err(crate::zerodiv_err!("integer modulo by zero"))
            } else {
                Ok(Value::int(py_mod_i64(a, b)))
            }
        }
        _ if !matches!(*lhs, NumericSlot::Float(_)) && !matches!(*rhs, NumericSlot::Float(_)) => {
            let a = lhs.to_bigint();
            let b = rhs.to_bigint();
            if b.is_zero() {
                Err(crate::zerodiv_err!("integer modulo by zero"))
            } else {
                let (_, rem) = bigint_divmod_floor(&a, &b);
                Ok(value_from_bigint(rem))
            }
        }
        _ => {
            let a = lhs.to_float()?;
            let b = rhs.to_float()?;
            if b == 0.0 {
                return Err(crate::zerodiv_err!("float modulo"));
            }
            let mut rem = a % b;
            if rem == 0.0 {
                // CPython float_rem: zero result copies sign of divisor.
                rem = rem.copysign(b);
            } else if rem.signum() != b.signum() {
                rem += b;
            }
            Ok(Value::float(rem))
        }
    }
}

/// CPython's `float_pow` message when the platform `pow` overflows:
/// `PyErr_SetFromErrno(PyExc_OverflowError)` with `errno == ERANGE`, whose
/// text is the C library's `strerror(ERANGE)`.
#[cfg(any(target_os = "linux", target_os = "android"))]
const FLOAT_POW_OVERFLOW: &str = "(34, 'Numerical result out of range')";
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const FLOAT_POW_OVERFLOW: &str = "(34, 'Result too large')";

/// Whether `a ** b == result` is a range error: finite operands whose power
/// overflowed (`_Py_ADJUST_ERANGE1` sets `ERANGE` for an infinite result).
fn float_pow_overflows(a: f64, b: f64, result: f64) -> bool {
    result.is_infinite() && a.is_finite() && b.is_finite() && a != 0.0
}

/// The `OverflowError` itself, out of line so the error formatting does not
/// grow the numeric dispatch that LTO inlines into the interpreter loop.
#[cold]
#[inline(never)]
fn float_pow_overflow_error() -> crate::errors::PyError {
    crate::overflow_err!("{}", FLOAT_POW_OVERFLOW)
}

pub(super) fn slot_pow(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    if lhs.is_complex() || rhs.is_complex() {
        let (zr, zi) = lhs.to_complex()?;
        let (wr, wi) = rhs.to_complex()?;
        return complex_pow(zr, zi, wr, wi);
    }
    match (lhs.as_i64_coerced(), rhs.as_i64_coerced()) {
        // Integer ** non-negative integer stays in the int domain
        // (BigInt promotion on overflow, #421).
        (Some(a), Some(b)) if b >= 0 => Ok(int_pow_promoting(a, b)),
        (None, Some(b)) if matches!(*lhs, NumericSlot::BigInt(_)) && b >= 0 => {
            Ok(value_from_bigint(PyPow::pow(lhs.to_bigint(), b as u64)))
        }
        (_, None)
            if !matches!(*lhs, NumericSlot::Float(_))
                && matches!(
                    *rhs,
                    NumericSlot::BigInt(big) if big.sign() != PyBigIntSign::Minus
                ) =>
        {
            let NumericSlot::BigInt(exp) = *rhs else {
                unreachable!()
            };
            bigint_pow_nonneg(lhs.to_bigint(), exp)
        }
        // Float base/exponent, or integer with a negative exponent:
        // float exponentiation (with CPython's negative-real -> complex
        // promotion and the 0.0 ** negative ZeroDivisionError).
        _ => {
            let a = lhs.to_float()?;
            let b = rhs.to_float()?;
            if a == 0.0 && b < 0.0 && b.is_finite() {
                return Err(crate::zerodiv_err!(
                    "0.0 cannot be raised to a negative power"
                ));
            }
            let result = a.powf(b);
            if a < 0.0 && b.is_finite() && result.is_nan() {
                let abs_val = a.abs().powf(b);
                let angle = std::f64::consts::PI * b;
                Ok(Value::complex(abs_val * angle.cos(), abs_val * angle.sin()))
            } else if float_pow_overflows(a, b, result) {
                Err(float_pow_overflow_error())
            } else {
                Ok(Value::float(result))
            }
        }
    }
}

pub(super) fn slot_and(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    promote_bitwise(
        NumericBinOp::BitAnd,
        lhs,
        rhs,
        |a, b| a & b,
        |a, b| a & b,
        |a, b| a & b,
    )
}

pub(super) fn slot_or(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    promote_bitwise(
        NumericBinOp::BitOr,
        lhs,
        rhs,
        |a, b| a | b,
        |a, b| a | b,
        |a, b| a | b,
    )
}

pub(super) fn slot_xor(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    promote_bitwise(
        NumericBinOp::BitXor,
        lhs,
        rhs,
        |a, b| a ^ b,
        |a, b| a ^ b,
        |a, b| a ^ b,
    )
}

enum ShiftCount {
    Fits(usize),
    Saturated,
}

const MAX_SHIFT: usize = 1 << 30;

fn shift_count(slot: &NumericSlot<'_>) -> Result<ShiftCount> {
    match *slot {
        NumericSlot::Int(n) => {
            if n < 0 {
                Err(crate::value_err!("negative shift count"))
            } else {
                Ok(usize::try_from(n).map_or(ShiftCount::Saturated, ShiftCount::Fits))
            }
        }
        NumericSlot::Bool(b) => Ok(ShiftCount::Fits(b as usize)),
        NumericSlot::BigInt(big) => match big.sign() {
            PyBigIntSign::Minus => Err(crate::value_err!("negative shift count")),
            PyBigIntSign::NoSign => Ok(ShiftCount::Fits(0)),
            PyBigIntSign::Plus => Ok(big
                .to_usize()
                .map_or(ShiftCount::Saturated, ShiftCount::Fits)),
        },
        NumericSlot::Float(_) | NumericSlot::Complex(_, _) => {
            unreachable!("shift_count called on Float/Complex")
        }
    }
}

pub(super) fn slot_lshift(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    if lhs.is_float_or_complex() || rhs.is_float_or_complex() {
        return Err(lhs.unsupported_binop(NumericBinOp::LShift, rhs));
    }
    let a = lhs.to_bigint();
    match shift_count(rhs)? {
        ShiftCount::Fits(n) => {
            if n > MAX_SHIFT && !a.is_zero() {
                return Err(crate::overflow_err!("too many digits in integer"));
            }
            Ok(value_from_bigint(a << n))
        }
        ShiftCount::Saturated => {
            if a.is_zero() {
                Ok(Value::int(0))
            } else {
                Err(crate::overflow_err!("too many digits in integer"))
            }
        }
    }
}

pub(super) fn slot_rshift(lhs: &NumericSlot<'_>, rhs: &NumericSlot<'_>) -> Result<Value> {
    if lhs.is_float_or_complex() || rhs.is_float_or_complex() {
        return Err(lhs.unsupported_binop(NumericBinOp::RShift, rhs));
    }
    let a = lhs.to_bigint();
    match shift_count(rhs)? {
        ShiftCount::Fits(n) => Ok(value_from_bigint(a >> n)),
        ShiftCount::Saturated => Ok(Value::int(if a.sign() == PyBigIntSign::Minus {
            -1
        } else {
            0
        })),
    }
}
