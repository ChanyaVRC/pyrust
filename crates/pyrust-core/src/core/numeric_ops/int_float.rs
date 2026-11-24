//! Canonical `int`, `BigInt`, and `float` arithmetic/comparison helpers.

use std::cmp::Ordering;

use num_traits::{FromPrimitive, ToPrimitive, Zero};

use crate::errors::Result;
use crate::object_model::{PyBigInt, PyBigIntSign, PyPow, Value, ValueKind};

/// Fallible `BigInt -> f64`. Raises `OverflowError` (`int too large to convert
/// to float`) when the `BigInt`'s magnitude exceeds `f64`'s finite range.
pub fn bigint_to_float_or_overflow(b: &PyBigInt) -> Result<f64> {
    b.to_f64()
        .filter(|f| f.is_finite())
        .ok_or_else(|| crate::overflow_err!("int too large to convert to float"))
}

/// Python-style modulo for two `i64` values (`b != 0`), where the result has
/// the same sign as the divisor `b`. Handles `i64::MIN % -1 == 0` without panic.
#[inline]
pub fn py_mod_i64(a: i64, b: i64) -> i64 {
    let mut remainder = a.wrapping_rem(b);
    if (remainder > 0 && b < 0) || (remainder < 0 && b > 0) {
        remainder += b;
    }
    remainder
}

/// Port of CPython's `float_divmod` (`Objects/floatobject.c`).
///
/// Returns `(floordiv, mod)` for non-zero divisor `b`, using `fmod` so that
/// infinities and signed zeros propagate identically to CPython. Caller must
/// guard `b == 0.0`.
pub fn float_divmod(a: f64, b: f64) -> (f64, f64) {
    let mut mod_ = a % b;
    let mut div = (a - mod_) / b;
    if mod_ != 0.0 {
        if (b < 0.0) != (mod_ < 0.0) {
            mod_ += b;
            div -= 1.0;
        }
    } else {
        mod_ = 0.0_f64.copysign(b);
    }
    let floordiv = if div != 0.0 {
        let fl = div.floor();
        if div - fl > 0.5 { fl + 1.0 } else { fl }
    } else {
        0.0_f64.copysign(a / b)
    };
    (floordiv, mod_)
}

/// Python-style floor `(quotient, remainder)` for two `PyBigInt`s (`b != 0`).
pub fn bigint_divmod_floor(a: &PyBigInt, b: &PyBigInt) -> (PyBigInt, PyBigInt) {
    let mut q = a / b;
    let mut r = a % b;
    if !r.is_zero() && (r.sign() != b.sign()) {
        q -= 1;
        r += b;
    }
    (q, r)
}

/// Normalize a `PyBigInt` to `Value::int` when it fits in `i64`, otherwise
/// `Value::bigint`.
#[inline]
pub fn value_from_bigint(n: PyBigInt) -> Value {
    match n.to_i64() {
        Some(i) => Value::int(i),
        None => Value::bigint(n),
    }
}

/// Coerce `Int`, `BigInt`, or `Bool` to `PyBigInt`, returning `None` otherwise.
pub fn value_to_bigint(v: &Value) -> Option<PyBigInt> {
    match v.kind() {
        ValueKind::Int(n) => Some(PyBigInt::from(n)),
        ValueKind::BigInt(b) => Some(b.clone()),
        ValueKind::Bool(b) => Some(PyBigInt::from(b as i64)),
        _ => None,
    }
}

/// `a ** b` for non-negative `i64` exponent, promoting to `BigInt` on overflow.
pub fn int_pow_promoting(a: i64, b: i64) -> Value {
    debug_assert!(b >= 0, "int_pow_promoting: caller must guard b < 0");
    let exp = match u32::try_from(b) {
        Ok(e) => e,
        Err(_) => return value_from_bigint(PyPow::pow(PyBigInt::from(a), b as u64)),
    };
    match a.checked_pow(exp) {
        Some(r) => Value::int(r),
        None => Value::bigint(PyPow::pow(PyBigInt::from(a), exp)),
    }
}

/// Exact equality between an `i64` integer and an `f64` float (CPython parity
/// beyond 2^53).
#[inline]
pub fn int_float_eq(i: i64, f: f64) -> bool {
    if !f.is_finite() || f != f.trunc() {
        return false;
    }
    const I64_MAX_PLUS_ONE: f64 = 9_223_372_036_854_775_808.0_f64;
    if f < (i64::MIN as f64) || f >= I64_MAX_PLUS_ONE {
        return false;
    }
    (f as i64) == i
}

/// Exact equality between a `PyBigInt` and an `f64` float.
#[inline]
pub fn bigint_float_eq(big: &PyBigInt, f: f64) -> bool {
    if !f.is_finite() || f != f.trunc() {
        return false;
    }
    match PyBigInt::from_f64(f) {
        Some(f_as_bigint) => f_as_bigint == *big,
        None => false,
    }
}

/// Exact ordering between an `i64` integer and an `f64` float. Returns `None`
/// for `NaN`.
pub fn int_float_cmp(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    const I64_MAX_PLUS_ONE: f64 = 9_223_372_036_854_775_808.0_f64; // 2^63
    if f >= I64_MAX_PLUS_ONE {
        return Some(Ordering::Less);
    }
    if f < (i64::MIN as f64) {
        return Some(Ordering::Greater);
    }
    let trunc = f.trunc();
    let trunc_i = trunc as i64;
    let base = i.cmp(&trunc_i);
    if base != Ordering::Equal || f == trunc {
        Some(base)
    } else if f > 0.0 {
        Some(Ordering::Less)
    } else {
        Some(Ordering::Greater)
    }
}

/// Exact ordering between a `PyBigInt` and an `f64` float. Returns `None` for
/// `NaN`.
pub fn bigint_float_cmp(big: &PyBigInt, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_finite() && f == f.trunc() {
        return PyBigInt::from_f64(f).map(|fi| big.cmp(&fi));
    }
    if f.is_infinite() {
        return if f > 0.0 {
            Some(Ordering::Less)
        } else {
            Some(Ordering::Greater)
        };
    }
    let trunc = f.trunc();
    let base = PyBigInt::from_f64(trunc)
        .map(|ti| big.cmp(&ti))
        .unwrap_or(Ordering::Less);
    if base != Ordering::Equal {
        return Some(base);
    }
    if f > 0.0 {
        Some(Ordering::Less)
    } else {
        Some(Ordering::Greater)
    }
}

pub(super) fn magnitude(b: &PyBigInt) -> PyBigInt {
    if b.sign() == PyBigIntSign::Minus {
        -b
    } else {
        b.clone()
    }
}

/// Correctly-rounded `int / int` true division, mirroring CPython's
/// `long_true_divide` (`Objects/longobject.c`, issue #1923).
pub(super) fn bigint_true_divide(num: &PyBigInt, den: &PyBigInt) -> Result<f64> {
    const MANT_DIG: i64 = 53;
    const MIN_EXP: i64 = -1021;
    const MAX_EXP: i64 = 1024;
    const SUBNORMAL_LSB: i64 = MIN_EXP - MANT_DIG;

    if den.is_zero() {
        return Err(crate::zerodiv_err!("division by zero"));
    }
    let negate = (num.sign() == PyBigIntSign::Minus) != (den.sign() == PyBigIntSign::Minus);
    if num.is_zero() {
        return Ok(if negate { -0.0 } else { 0.0 });
    }
    let a = magnitude(num);
    let b = magnitude(den);
    let diff = a.bits() as i64 - b.bits() as i64;

    if diff > MAX_EXP {
        return Err(crate::overflow_err!(
            "integer division result too large for a float"
        ));
    }
    if diff < SUBNORMAL_LSB - 1 {
        return Ok(if negate { -0.0 } else { 0.0 });
    }

    let l0 = ((diff - 1) - (MANT_DIG - 1)).max(SUBNORMAL_LSB);
    let mut shift = l0 - 2;
    let (mut x, rem) = if shift <= 0 {
        let scaled = &a << ((-shift) as usize);
        (&scaled / &b, &scaled % &b)
    } else {
        let scaled_b = &b << (shift as usize);
        (&a / &scaled_b, &a % &scaled_b)
    };
    let inexact = !rem.is_zero();
    if x.is_zero() {
        return Ok(if negate { -0.0 } else { 0.0 });
    }

    let e = (x.bits() as i64 - 1) + shift;
    let l = (e - (MANT_DIG - 1)).max(SUBNORMAL_LSB);
    let drop = (l - shift) as usize;
    let low = &x & ((PyBigInt::from(1) << drop) - 1);
    x >>= drop;
    let half = PyBigInt::from(1) << (drop - 1);
    let round_up =
        low > half || (low == half && ((&x & PyBigInt::from(1)) == PyBigInt::from(1) || inexact));
    if round_up {
        x += 1;
    }
    shift = l;

    let mantissa = x
        .to_f64()
        .filter(|f| f.is_finite())
        .ok_or_else(|| crate::overflow_err!("integer division result too large for a float"))?;
    let result = ldexp_f64(mantissa, shift);
    if !result.is_finite() {
        return Err(crate::overflow_err!(
            "integer division result too large for a float"
        ));
    }
    Ok(if negate { -result } else { result })
}

fn ldexp_f64(mut m: f64, mut exp: i64) -> f64 {
    if m == 0.0 || !m.is_finite() {
        return m;
    }
    const STEP: i64 = 512;
    const STEP_FACTOR: f64 = 1.340_780_792_994_259_7e154; // 2^512
    while exp > STEP {
        m *= STEP_FACTOR;
        exp -= STEP;
    }
    while exp < -STEP {
        m /= STEP_FACTOR;
        exp += STEP;
    }
    m * 2.0_f64.powi(exp as i32)
}
