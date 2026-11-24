//! Floating-point, exponential, logarithmic, distance, and summation entry
//! points for `math`.

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ConsumerIterator, ExpandedCallArg, Interpreter, reject_keyword_args_expanded, value_to_float,
};
use crate::value::{PyToPrimitive, Value, ValueKind};

use super::coercion::{
    math_arg_to_float, single_float, two_floats, value_to_exp_int, value_to_steps_int,
};
use super::float_algorithms::{
    check_math_overflow, check_math_result, frexp_f64, ieee_remainder, ldexp_f64, nextafter_f64,
    ulp_f64, vector_norm,
};
use super::summation::{FsumState, SumProdFloatState};

pub(super) fn unary_checked(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
    op: fn(f64) -> f64,
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    Ok(Value::float(check_math_result(x, op(x))?))
}

pub(super) fn unary_overflow(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
    op: fn(f64) -> f64,
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    Ok(Value::float(check_math_overflow(x, op(x))?))
}

pub(super) fn unary_raw(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
    op: fn(f64) -> f64,
) -> Result<Value> {
    Ok(Value::float(op(single_float(interp, fn_name, args)?)))
}

pub(super) fn unary_pred(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
    pred: fn(f64) -> bool,
) -> Result<Value> {
    Ok(Value::bool_(pred(single_float(interp, fn_name, args)?)))
}

pub(super) fn pow(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (x, y) = two_floats(interp, fn_name, args)?;
    // CPython special-cases pow(0, negative): division by zero is a domain
    // error (undefined), not a range error, even though the C result is +inf.
    if x == 0.0 && y.is_finite() && y < 0.0 {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    let result = x.powf(y);
    if x.is_finite() && y.is_finite() {
        check_math_result(x, result)?;
    }
    Ok(Value::float(result))
}

pub(super) fn log(
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
    let x = math_arg_to_float(interp, &args[0].value)?;
    if args.len() == 2 {
        let base = math_arg_to_float(interp, &args[1].value)?;
        // Check x before base so log(0, 1) -> ValueError (not ZeroDivisionError).
        let ln_x = check_math_result(x, x.ln())?;
        let ln_base = check_math_result(base, base.ln())?;
        if ln_base == 0.0 {
            return Err(PyError::named(
                "ZeroDivisionError",
                "float division by zero".to_string(),
            ));
        }
        Ok(Value::float(ln_x / ln_base))
    } else {
        Ok(Value::float(check_math_result(x, x.ln())?))
    }
}

pub(super) fn copysign(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (x, y) = two_floats(interp, fn_name, args)?;
    Ok(Value::float(x.copysign(y)))
}

pub(super) fn hypot(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    let mut coords = Vec::with_capacity(args.len());
    for arg in args.iter() {
        coords.push(math_arg_to_float(interp, &arg.value)?);
    }
    Ok(Value::float(vector_norm(&mut coords)))
}

pub(super) fn dist(
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
    let p_items = interp.collect_iterable(&args[0].value)?;
    let q_items = interp.collect_iterable(&args[1].value)?;
    if p_items.len() != q_items.len() {
        return Err(PyError::named(
            "ValueError",
            "both points must have the same number of dimensions".to_string(),
        ));
    }
    let mut diffs = Vec::with_capacity(p_items.len());
    for (pv, qv) in p_items.iter().zip(q_items.iter()) {
        let a = math_arg_to_float(interp, pv)?;
        let b = math_arg_to_float(interp, qv)?;
        diffs.push(a - b);
    }
    Ok(Value::float(vector_norm(&mut diffs)))
}

pub(super) fn fmod(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (x, y) = two_floats(interp, fn_name, args)?;
    let r = x % y;
    if r.is_nan() && !x.is_nan() && !y.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    Ok(Value::float(r))
}

pub(super) fn remainder(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (x, y) = two_floats(interp, fn_name, args)?;
    let r = ieee_remainder(x, y);
    if r.is_nan() && !x.is_nan() && !y.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    Ok(Value::float(r))
}

pub(super) fn modf(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    let (frac, intpart) = if x.is_infinite() {
        (0.0_f64.copysign(x), x)
    } else {
        (x.fract().copysign(x), x.trunc())
    };
    Ok(Value::tuple(vec![
        Value::float(frac),
        Value::float(intpart),
    ]))
}

pub(super) fn frexp(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    let (m, e) = frexp_f64(x);
    Ok(Value::tuple(vec![Value::float(m), Value::int(e as i64)]))
}

pub(super) fn ldexp(
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
    let x = math_arg_to_float(interp, &args[0].value)?;
    let exp = value_to_exp_int(fn_name, &args[1].value)?;
    let r = ldexp_f64(x, exp);
    if r.is_infinite() && x.is_finite() && x != 0.0 {
        return Err(PyError::named(
            "OverflowError",
            "math range error".to_string(),
        ));
    }
    Ok(Value::float(r))
}

pub(super) fn nextafter(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
    for kw in args.iter().filter(|a| a.name.is_some()) {
        let name = kw.name.as_deref().unwrap_or("");
        if name != "steps" {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() got an unexpected keyword argument '{name}'"),
            ));
        }
    }
    if positional.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly two positional arguments"),
        ));
    }
    let x = math_arg_to_float(interp, &positional[0].value)?;
    let y = math_arg_to_float(interp, &positional[1].value)?;
    let steps = match args.iter().find(|a| a.name.as_deref() == Some("steps")) {
        Some(a) => {
            let s = value_to_steps_int(interp, fn_name, &a.value)?;
            if s < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "steps must be a non-negative integer".to_string(),
                ));
            }
            s
        }
        None => 1,
    };
    Ok(Value::float(nextafter_f64(x, y, steps)))
}

pub(super) fn ulp(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    Ok(Value::float(ulp_f64(x)))
}

pub(super) fn isclose(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
    for kw in args.iter().filter(|a| a.name.is_some()) {
        let name = kw.name.as_deref().unwrap_or("");
        if name != "rel_tol" && name != "abs_tol" {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() got an unexpected keyword argument '{name}'"),
            ));
        }
    }
    if positional.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly two positional arguments"),
        ));
    }
    let a = math_arg_to_float(interp, &positional[0].value)?;
    let b = math_arg_to_float(interp, &positional[1].value)?;
    let rel_tol = match args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("rel_tol"))
    {
        Some(arg) => math_arg_to_float(interp, &arg.value)?,
        None => 1e-9,
    };
    let abs_tol = match args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("abs_tol"))
    {
        Some(arg) => math_arg_to_float(interp, &arg.value)?,
        None => 0.0,
    };
    if rel_tol < 0.0 || abs_tol < 0.0 {
        return Err(PyError::named(
            "ValueError",
            "tolerances must be non-negative".to_string(),
        ));
    }
    let result = if a == b {
        true
    } else if a.is_infinite() || b.is_infinite() {
        false
    } else {
        let diff = (b - a).abs();
        diff <= (rel_tol * b).abs() || diff <= (rel_tol * a).abs() || diff <= abs_tol
    };
    Ok(Value::bool_(result))
}

pub(super) fn fsum(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    let mut iterator = ConsumerIterator::new(interp, &args[0].value)?;
    let mut state = FsumState::new();
    while let Some(item) = iterator.next(interp)? {
        state.add(math_arg_to_float(interp, &item)?)?;
    }
    Ok(Value::float(state.finish()?))
}

pub(super) fn sumprod(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("sumprod expected 2 arguments, got {}", args.len()),
        ));
    }
    let mut p_iter = ConsumerIterator::new(interp, &args[0].value)?;
    let mut q_iter = ConsumerIterator::new(interp, &args[1].value)?;

    let exact_int_as_i64 = |v: &Value| match v.kind() {
        ValueKind::Int(value) => Some(value),
        ValueKind::BigInt(value) => value.to_i64(),
        _ => None,
    };
    let is_exact_int_or_bool = |v: &Value| {
        matches!(
            v.kind(),
            ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)
        )
    };
    let is_exact_float = |v: &Value| matches!(v.kind(), ValueKind::Float(_));

    let mut int_path_enabled = true;
    let mut int_total_in_use = false;
    let mut int_total = 0i64;
    let mut float_path_enabled = true;
    let mut float_total_in_use = false;
    let mut float_state = SumProdFloatState::new();
    let mut generic_total = Value::int(0);
    loop {
        let p_item = p_iter.next(interp)?;
        let q_item = q_iter.next(interp)?;
        let (pv, qv) = match (p_item, q_item) {
            (None, None) => break,
            (Some(pv), Some(qv)) => (pv, qv),
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    "Inputs are not the same length".to_string(),
                ));
            }
        };

        if int_path_enabled {
            let next_int_total = exact_int_as_i64(&pv)
                .zip(exact_int_as_i64(&qv))
                .and_then(|(a, b)| a.checked_mul(b))
                .and_then(|product| int_total.checked_add(product));
            if let Some(next_total) = next_int_total {
                int_total = next_total;
                int_total_in_use = true;
                continue;
            }

            int_path_enabled = false;
            if int_total_in_use {
                generic_total =
                    interp.eval_binary(generic_total, BinaryOp::Add, Value::int(int_total))?;
                int_total_in_use = false;
            }
        }

        if float_path_enabled {
            let float_pair = (is_exact_float(&pv)
                && (is_exact_float(&qv) || is_exact_int_or_bool(&qv)))
                || (is_exact_float(&qv) && is_exact_int_or_bool(&pv));
            let accepted = if float_pair {
                match value_to_float(&pv, "__SENTINEL__")
                    .and_then(|a| value_to_float(&qv, "__SENTINEL__").map(|b| (a, b)))
                {
                    Ok((a, b)) => float_state.try_add(a, b),
                    Err(_) => false,
                }
            } else {
                false
            };

            if accepted {
                float_total_in_use = true;
                continue;
            }

            float_path_enabled = false;
            if float_total_in_use {
                generic_total = interp.eval_binary(
                    generic_total,
                    BinaryOp::Add,
                    Value::float(float_state.finish()),
                )?;
                float_total_in_use = false;
            }
        }

        let product = interp.eval_binary(pv, BinaryOp::Mul, qv)?;
        generic_total = interp.eval_binary(generic_total, BinaryOp::Add, product)?;
    }

    if int_path_enabled && int_total_in_use {
        generic_total = interp.eval_binary(generic_total, BinaryOp::Add, Value::int(int_total))?;
    } else if float_path_enabled && float_total_in_use {
        generic_total = interp.eval_binary(
            generic_total,
            BinaryOp::Add,
            Value::float(float_state.finish()),
        )?;
    }
    Ok(generic_total)
}
