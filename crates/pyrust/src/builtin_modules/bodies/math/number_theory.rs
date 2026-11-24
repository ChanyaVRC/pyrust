//! Number-theoretic and rounding entry points for `math` (`floor`, `ceil`,
//! `trunc`, `gcd`, `lcm`, `factorial`, `comb`, `perm`, `prod`, `isqrt`).

use num_traits::Signed;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ConsumerIterator, ExpandedCallArg, Interpreter, builtin_data_backing, float_to_bigint,
    reject_keyword_args_expanded, value_type_name_str,
};
use crate::value::{PyBigInt, PyToPrimitive, Value, ValueKind};

use super::coercion::{math_arg_to_float, math_integral_exact, try_math_dunder};
use super::integer_algorithms::{
    bigint_gcd, bigint_lcm, bigint_to_int_value, value_to_bigint_int, value_to_bigint_strict_int,
};

fn rounded_float_to_int(x: f64, round_fn: fn(f64) -> f64) -> Result<Value> {
    // Guard nan/inf before rounding — NaN comparisons are always false so the
    // range check below would silently fall through to `as i64`.
    if x.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "cannot convert float NaN to integer".to_string(),
        ));
    }
    if x.is_infinite() {
        return Err(PyError::named(
            "OverflowError",
            "cannot convert float infinity to integer".to_string(),
        ));
    }
    let f = round_fn(x);
    if f >= i64::MAX as f64 || f < i64::MIN as f64 {
        float_to_bigint(f)
    } else {
        Ok(Value::int(f as i64))
    }
}

fn floor_or_ceil(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
    dunder: &str,
    round_fn: fn(f64) -> f64,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    let val = &args[0].value;
    if let Some(r) = try_math_dunder(interp, val, dunder) {
        return r;
    }
    if let Some(n) = math_integral_exact(val) {
        return Ok(n);
    }
    let x = math_arg_to_float(interp, val)?;
    rounded_float_to_int(x, round_fn)
}

pub(super) fn floor(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    floor_or_ceil(interp, fn_name, args, "__floor__", f64::floor)
}

pub(super) fn ceil(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    floor_or_ceil(interp, fn_name, args, "__ceil__", f64::ceil)
}

pub(super) fn trunc(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    let val = &args[0].value;
    if let Some(r) = try_math_dunder(interp, val, "__trunc__") {
        return r;
    }
    if let Some(n) = math_integral_exact(val) {
        return Ok(n);
    }
    // float and float subclasses (inherited float.__trunc__): truncate toward
    // zero. Unlike floor/ceil, trunc does NOT fall back to __index__/__float__.
    let f = match val.kind() {
        ValueKind::Float(f) => Some(f),
        ValueKind::PyInstance(_) => match builtin_data_backing(val).as_ref().map(|b| b.kind()) {
            Some(ValueKind::Float(f)) => Some(f),
            _ => None,
        },
        _ => None,
    };
    match f {
        Some(f) => rounded_float_to_int(f, f64::trunc),
        None => Err(PyError::named(
            "TypeError",
            format!(
                "type {} doesn't define __trunc__ method",
                value_type_name_str(val)
            ),
        )),
    }
}

pub(super) fn gcd(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    let mut result = PyBigInt::from(0i64);
    for arg in args.iter() {
        let n = value_to_bigint_int(interp, fn_name, &arg.value)?;
        result = bigint_gcd(result, n);
    }
    bigint_to_int_value(result)
}

pub(super) fn lcm(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    let mut result = PyBigInt::from(1i64);
    for arg in args.iter() {
        let n = value_to_bigint_int(interp, fn_name, &arg.value)?;
        result = bigint_lcm(result, n);
    }
    bigint_to_int_value(result)
}

pub(super) fn factorial(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    let n_big = value_to_bigint_strict_int(interp, fn_name, &args[0].value)?;
    if n_big.is_negative() {
        return Err(PyError::named(
            "ValueError",
            "factorial() not defined for negative values".to_string(),
        ));
    }
    let n_u64: u64 = n_big.to_u64().ok_or_else(|| {
        PyError::named(
            "OverflowError",
            "factorial() argument is too large".to_string(),
        )
    })?;
    let mut acc = PyBigInt::from(1i64);
    for i in 2..=n_u64 {
        acc *= PyBigInt::from(i);
    }
    bigint_to_int_value(acc)
}

pub(super) fn comb(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly two arguments"),
        ));
    }
    let n = value_to_bigint_int(interp, fn_name, &args[0].value)?;
    let k = value_to_bigint_int(interp, fn_name, &args[1].value)?;
    if n.is_negative() {
        return Err(PyError::named(
            "ValueError",
            "n must be a non-negative integer".to_string(),
        ));
    }
    if k.is_negative() {
        return Err(PyError::named(
            "ValueError",
            "k must be a non-negative integer".to_string(),
        ));
    }
    if k > n {
        return Ok(Value::int(0));
    }
    let k_small = k.clone().min(n.clone() - k);
    let k_u64: u64 = k_small.to_u64().ok_or_else(|| {
        PyError::named("OverflowError", "comb() argument is too large".to_string())
    })?;
    let mut result = PyBigInt::from(1i64);
    for i in 0..k_u64 {
        result *= n.clone() - PyBigInt::from(i);
        result /= PyBigInt::from(i + 1);
    }
    bigint_to_int_value(result)
}

pub(super) fn perm(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.is_empty() || args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes one or two arguments"),
        ));
    }
    let n = value_to_bigint_int(interp, fn_name, &args[0].value)?;
    let k_explicit = args.len() == 2 && !matches!(args[1].value.kind(), ValueKind::None);
    if n.is_negative() {
        let msg = if k_explicit {
            "n must be a non-negative integer".to_string()
        } else {
            "factorial() not defined for negative values".to_string()
        };
        return Err(PyError::named("ValueError", msg));
    }
    let k = if k_explicit {
        let kv = value_to_bigint_int(interp, fn_name, &args[1].value)?;
        if kv.is_negative() {
            return Err(PyError::named(
                "ValueError",
                "k must be a non-negative integer".to_string(),
            ));
        }
        kv
    } else {
        n.clone()
    };
    if k > n {
        return Ok(Value::int(0));
    }
    let k_u64: u64 = k.to_u64().ok_or_else(|| {
        PyError::named("OverflowError", "perm() argument is too large".to_string())
    })?;
    let mut result = PyBigInt::from(1i64);
    for i in 0..k_u64 {
        result *= n.clone() - PyBigInt::from(i);
    }
    bigint_to_int_value(result)
}

pub(super) fn prod(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
    let keyword: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_some()).collect();
    if positional.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one positional argument"),
        ));
    }
    for kw in &keyword {
        let name = kw.name.as_deref().unwrap_or("");
        if name != "start" {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() got an unexpected keyword argument '{name}'"),
            ));
        }
    }
    let start = keyword
        .first()
        .map(|a| a.value.clone())
        .unwrap_or_else(|| Value::int(1));
    let mut iterator = ConsumerIterator::new(interp, &positional[0].value)?;
    let mut acc = start;
    while let Some(item) = iterator.next(interp)? {
        acc = interp.eval_binary(acc, BinaryOp::Mul, item)?;
    }
    Ok(acc)
}

pub(super) fn isqrt(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    let n = value_to_bigint_int(interp, fn_name, &args[0].value)?;
    if n.is_negative() {
        return Err(PyError::named(
            "ValueError",
            "isqrt() argument must be nonnegative".to_string(),
        ));
    }
    bigint_to_int_value(n.sqrt())
}
