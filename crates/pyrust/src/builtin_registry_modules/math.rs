//! `math` module built-ins, migrated to the `#[pyfunction]` registry pattern.
//!
//! Each function below mirrors a CPython `math` callable; see
//! <https://docs.python.org/3/library/math.html> for the canonical
//! signatures.  Argument count and type are checked at call time.

use crate::Interpreter;
use crate::builtin_registry::BuiltinReg;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{float_to_bigint, reject_keyword_args_expanded, value_to_float};
use crate::value::Value;
use pyrust_derive::pyfunction;

// ── Single-argument math fns ─────────────────────────────────────────────────

/// CPython: `math.floor(x)` → int.
/// <https://docs.python.org/3/library/math.html#math.floor>
#[pyfunction(name = "math.floor")]
fn math_floor(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let x = single_float("math.floor", args)?;
    let f = x.floor();
    if f > i64::MAX as f64 || f < i64::MIN as f64 {
        Ok(float_to_bigint(f))
    } else {
        Ok(Value::int(f as i64))
    }
}

/// CPython: `math.ceil(x)` → int.
/// <https://docs.python.org/3/library/math.html#math.ceil>
#[pyfunction(name = "math.ceil")]
fn math_ceil(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let x = single_float("math.ceil", args)?;
    let f = x.ceil();
    if f > i64::MAX as f64 || f < i64::MIN as f64 {
        Ok(float_to_bigint(f))
    } else {
        Ok(Value::int(f as i64))
    }
}

/// CPython: `math.sqrt(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.sqrt>
#[pyfunction(name = "math.sqrt")]
fn math_sqrt(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.sqrt", args)?.sqrt()))
}

/// CPython: `math.fabs(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.fabs>
#[pyfunction(name = "math.fabs")]
fn math_fabs(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.fabs", args)?.abs()))
}

/// CPython: `math.sin(x)` → float (radians).
/// <https://docs.python.org/3/library/math.html#math.sin>
#[pyfunction(name = "math.sin")]
fn math_sin(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.sin", args)?.sin()))
}

/// CPython: `math.cos(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.cos>
#[pyfunction(name = "math.cos")]
fn math_cos(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.cos", args)?.cos()))
}

/// CPython: `math.tan(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.tan>
#[pyfunction(name = "math.tan")]
fn math_tan(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.tan", args)?.tan()))
}

/// CPython: `math.asin(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.asin>
#[pyfunction(name = "math.asin")]
fn math_asin(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.asin", args)?.asin()))
}

/// CPython: `math.acos(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.acos>
#[pyfunction(name = "math.acos")]
fn math_acos(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.acos", args)?.acos()))
}

/// CPython: `math.atan(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.atan>
#[pyfunction(name = "math.atan")]
fn math_atan(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.atan", args)?.atan()))
}

/// CPython: `math.exp(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.exp>
#[pyfunction(name = "math.exp")]
fn math_exp(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.exp", args)?.exp()))
}

/// CPython: `math.log2(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.log2>
#[pyfunction(name = "math.log2")]
fn math_log2(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.log2", args)?.log2()))
}

/// CPython: `math.log10(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.log10>
#[pyfunction(name = "math.log10")]
fn math_log10(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::float(single_float("math.log10", args)?.log10()))
}

/// CPython: `math.isnan(x)` → bool.
/// <https://docs.python.org/3/library/math.html#math.isnan>
#[pyfunction(name = "math.isnan")]
fn math_isnan(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::bool_(single_float("math.isnan", args)?.is_nan()))
}

/// CPython: `math.isinf(x)` → bool.
/// <https://docs.python.org/3/library/math.html#math.isinf>
#[pyfunction(name = "math.isinf")]
fn math_isinf(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    Ok(Value::bool_(
        single_float("math.isinf", args)?.is_infinite(),
    ))
}

// ── Two-argument math fns ────────────────────────────────────────────────────

/// CPython: `math.pow(x, y)` → float.
/// <https://docs.python.org/3/library/math.html#math.pow>
#[pyfunction(name = "math.pow")]
fn math_pow(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded("math.pow", args)?;
    if args.len() != 2 {
        return Err(PyError::Runtime(
            "math.pow() takes exactly two arguments".to_string(),
        ));
    }
    let x = value_to_float(&args[0].value, "math.pow")?;
    let y = value_to_float(&args[1].value, "math.pow")?;
    Ok(Value::float(x.powf(y)))
}

/// CPython: `math.atan2(y, x)` → float.
/// <https://docs.python.org/3/library/math.html#math.atan2>
#[pyfunction(name = "math.atan2")]
fn math_atan2(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded("math.atan2", args)?;
    if args.len() != 2 {
        return Err(PyError::Runtime(
            "math.atan2() takes exactly two arguments".to_string(),
        ));
    }
    let y = value_to_float(&args[0].value, "math.atan2")?;
    let x = value_to_float(&args[1].value, "math.atan2")?;
    Ok(Value::float(y.atan2(x)))
}

/// CPython: `math.log(x[, base])` → float.
/// <https://docs.python.org/3/library/math.html#math.log>
#[pyfunction(name = "math.log")]
fn math_log(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded("math.log", args)?;
    if args.is_empty() || args.len() > 2 {
        return Err(PyError::Runtime(
            "math.log() takes one or two arguments".to_string(),
        ));
    }
    let x = value_to_float(&args[0].value, "math.log")?;
    if args.len() == 2 {
        let base = value_to_float(&args[1].value, "math.log")?;
        Ok(Value::float(x.log(base)))
    } else {
        Ok(Value::float(x.ln()))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Reject kwargs and demand exactly one positional float-coercible arg.
fn single_float(fn_name: &str, args: &[ExpandedCallArg]) -> Result<f64> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::Runtime(format!(
            "{fn_name}() takes exactly one argument"
        )));
    }
    value_to_float(&args[0].value, fn_name)
}

// ── Per-module registration slice ────────────────────────────────────────────

/// Slice of every `math.*` registration in this file.  The central
/// `builtin_registry::REGISTRY` extends from this slice on first lookup.
pub(crate) const REGS: &[BuiltinReg] = &[
    MATH_FLOOR, MATH_CEIL, MATH_SQRT, MATH_FABS, MATH_SIN, MATH_COS, MATH_TAN, MATH_ASIN,
    MATH_ACOS, MATH_ATAN, MATH_EXP, MATH_LOG2, MATH_LOG10, MATH_ISNAN, MATH_ISINF, MATH_POW,
    MATH_ATAN2, MATH_LOG,
];
