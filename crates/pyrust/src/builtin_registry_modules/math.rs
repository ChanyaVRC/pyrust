//! `math` module built-ins — declared via the file-scoped
//! `pyrust_module!` macro.
//!
//! Each `fn name(args)` here is expanded to `fn math_name(_interp, args)`
//! with the unified dispatch signature, paired with a sibling
//! `BuiltinReg` whose Python-level name is `"math.name"`.  The macro
//! also generates `REGS: &[BuiltinReg]` (consumed by the central
//! registry) and `module()` (consumed by the interpreter's
//! `load_module` path), so this file is the single source of truth for
//! every `math.*` callable.
//!
//! Reference: <https://docs.python.org/3/library/math.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{float_to_bigint, reject_keyword_args_expanded, value_to_float};
use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    name = "math",

    constants {
        "pi"  => Value::float(std::f64::consts::PI),
        "e"   => Value::float(std::f64::consts::E),
        "tau" => Value::float(std::f64::consts::TAU),
        "inf" => Value::float(f64::INFINITY),
        "nan" => Value::float(f64::NAN),
    }

    /// CPython: math.floor(x) → int.  <https://docs.python.org/3/library/math.html#math.floor>
    fn floor(args) -> Result<Value> {
        let x = single_float("math.floor", args)?;
        let f = x.floor();
        if f > i64::MAX as f64 || f < i64::MIN as f64 {
            Ok(float_to_bigint(f))
        } else {
            Ok(Value::int(f as i64))
        }
    }

    /// CPython: math.ceil(x) → int.  <https://docs.python.org/3/library/math.html#math.ceil>
    fn ceil(args) -> Result<Value> {
        let x = single_float("math.ceil", args)?;
        let f = x.ceil();
        if f > i64::MAX as f64 || f < i64::MIN as f64 {
            Ok(float_to_bigint(f))
        } else {
            Ok(Value::int(f as i64))
        }
    }

    /// CPython: math.sqrt(x) → float.  <https://docs.python.org/3/library/math.html#math.sqrt>
    fn sqrt(args) -> Result<Value> {
        Ok(Value::float(single_float("math.sqrt", args)?.sqrt()))
    }

    /// CPython: math.fabs(x) → float.  <https://docs.python.org/3/library/math.html#math.fabs>
    fn fabs(args) -> Result<Value> {
        Ok(Value::float(single_float("math.fabs", args)?.abs()))
    }

    /// CPython: math.sin(x) → float.  <https://docs.python.org/3/library/math.html#math.sin>
    fn sin(args) -> Result<Value> {
        Ok(Value::float(single_float("math.sin", args)?.sin()))
    }

    /// CPython: math.cos(x) → float.  <https://docs.python.org/3/library/math.html#math.cos>
    fn cos(args) -> Result<Value> {
        Ok(Value::float(single_float("math.cos", args)?.cos()))
    }

    /// CPython: math.tan(x) → float.  <https://docs.python.org/3/library/math.html#math.tan>
    fn tan(args) -> Result<Value> {
        Ok(Value::float(single_float("math.tan", args)?.tan()))
    }

    /// CPython: math.asin(x) → float.  <https://docs.python.org/3/library/math.html#math.asin>
    fn asin(args) -> Result<Value> {
        Ok(Value::float(single_float("math.asin", args)?.asin()))
    }

    /// CPython: math.acos(x) → float.  <https://docs.python.org/3/library/math.html#math.acos>
    fn acos(args) -> Result<Value> {
        Ok(Value::float(single_float("math.acos", args)?.acos()))
    }

    /// CPython: math.atan(x) → float.  <https://docs.python.org/3/library/math.html#math.atan>
    fn atan(args) -> Result<Value> {
        Ok(Value::float(single_float("math.atan", args)?.atan()))
    }

    /// CPython: math.exp(x) → float.  <https://docs.python.org/3/library/math.html#math.exp>
    fn exp(args) -> Result<Value> {
        Ok(Value::float(single_float("math.exp", args)?.exp()))
    }

    /// CPython: math.log2(x) → float.  <https://docs.python.org/3/library/math.html#math.log2>
    fn log2(args) -> Result<Value> {
        Ok(Value::float(single_float("math.log2", args)?.log2()))
    }

    /// CPython: math.log10(x) → float.  <https://docs.python.org/3/library/math.html#math.log10>
    fn log10(args) -> Result<Value> {
        Ok(Value::float(single_float("math.log10", args)?.log10()))
    }

    /// CPython: math.isnan(x) → bool.  <https://docs.python.org/3/library/math.html#math.isnan>
    fn isnan(args) -> Result<Value> {
        Ok(Value::bool_(single_float("math.isnan", args)?.is_nan()))
    }

    /// CPython: math.isinf(x) → bool.  <https://docs.python.org/3/library/math.html#math.isinf>
    fn isinf(args) -> Result<Value> {
        Ok(Value::bool_(single_float("math.isinf", args)?.is_infinite()))
    }

    /// CPython: math.pow(x, y) → float.  <https://docs.python.org/3/library/math.html#math.pow>
    fn pow(args) -> Result<Value> {
        reject_keyword_args_expanded("math.pow", args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime("math.pow() takes exactly two arguments".to_string()));
        }
        let x = value_to_float(&args[0].value, "math.pow")?;
        let y = value_to_float(&args[1].value, "math.pow")?;
        Ok(Value::float(x.powf(y)))
    }

    /// CPython: math.atan2(y, x) → float.  <https://docs.python.org/3/library/math.html#math.atan2>
    fn atan2(args) -> Result<Value> {
        reject_keyword_args_expanded("math.atan2", args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime("math.atan2() takes exactly two arguments".to_string()));
        }
        let y = value_to_float(&args[0].value, "math.atan2")?;
        let x = value_to_float(&args[1].value, "math.atan2")?;
        Ok(Value::float(y.atan2(x)))
    }

    /// CPython: math.log(x[, base]) → float.  <https://docs.python.org/3/library/math.html#math.log>
    fn log(args) -> Result<Value> {
        reject_keyword_args_expanded("math.log", args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::Runtime("math.log() takes one or two arguments".to_string()));
        }
        let x = value_to_float(&args[0].value, "math.log")?;
        if args.len() == 2 {
            let base = value_to_float(&args[1].value, "math.log")?;
            Ok(Value::float(x.log(base)))
        } else {
            Ok(Value::float(x.ln()))
        }
    }
}

// ── Helpers used by the macro-generated bodies ───────────────────────────────

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
