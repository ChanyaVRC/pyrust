// `math` module — included into `pub mod math { … }` declared by the
// `pyrust_builtin_modules!` invocation in
// `builtin_modules/mod.rs`.  The macro injects a sibling
// `MODULE_NAME: &str = "math"` constant; the `pyrust_module!` body
// below reads it to compose every function's `FN_NAME` and the
// `PyModule.name`.  No name literal appears in this file.
//
// Reference: <https://docs.python.org/3/library/math.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{float_to_bigint, reject_keyword_args_expanded, value_to_float};
use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "pi"  => Value::float(std::f64::consts::PI),
        "e"   => Value::float(std::f64::consts::E),
        "tau" => Value::float(std::f64::consts::TAU),
        "inf" => Value::float(f64::INFINITY),
        "nan" => Value::float(f64::NAN),
    }

    /// CPython: math.floor(x) → int.  <https://docs.python.org/3/library/math.html#math.floor>
    #[pure]
    fn floor(args) -> Result<Value> {
        let x = single_float(FN_NAME, args)?;
        let f = x.floor();
        if f > i64::MAX as f64 || f < i64::MIN as f64 {
            Ok(float_to_bigint(f))
        } else {
            Ok(Value::int(f as i64))
        }
    }

    /// CPython: math.ceil(x) → int.  <https://docs.python.org/3/library/math.html#math.ceil>
    #[pure]
    fn ceil(args) -> Result<Value> {
        let x = single_float(FN_NAME, args)?;
        let f = x.ceil();
        if f > i64::MAX as f64 || f < i64::MIN as f64 {
            Ok(float_to_bigint(f))
        } else {
            Ok(Value::int(f as i64))
        }
    }

    /// CPython: math.sqrt(x) → float.  <https://docs.python.org/3/library/math.html#math.sqrt>
    #[pure]
    fn sqrt(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.sqrt()))
    }

    /// CPython: math.fabs(x) → float.  <https://docs.python.org/3/library/math.html#math.fabs>
    #[pure]
    fn fabs(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.abs()))
    }

    /// CPython: math.sin(x) → float.  <https://docs.python.org/3/library/math.html#math.sin>
    #[pure]
    fn sin(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.sin()))
    }

    /// CPython: math.cos(x) → float.  <https://docs.python.org/3/library/math.html#math.cos>
    #[pure]
    fn cos(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.cos()))
    }

    /// CPython: math.tan(x) → float.  <https://docs.python.org/3/library/math.html#math.tan>
    #[pure]
    fn tan(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.tan()))
    }

    /// CPython: math.asin(x) → float.  <https://docs.python.org/3/library/math.html#math.asin>
    #[pure]
    fn asin(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.asin()))
    }

    /// CPython: math.acos(x) → float.  <https://docs.python.org/3/library/math.html#math.acos>
    #[pure]
    fn acos(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.acos()))
    }

    /// CPython: math.atan(x) → float.  <https://docs.python.org/3/library/math.html#math.atan>
    #[pure]
    fn atan(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.atan()))
    }

    /// CPython: math.exp(x) → float.  <https://docs.python.org/3/library/math.html#math.exp>
    #[pure]
    fn exp(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.exp()))
    }

    /// CPython: math.log2(x) → float.  <https://docs.python.org/3/library/math.html#math.log2>
    #[pure]
    fn log2(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.log2()))
    }

    /// CPython: math.log10(x) → float.  <https://docs.python.org/3/library/math.html#math.log10>
    #[pure]
    fn log10(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.log10()))
    }

    /// CPython: math.isnan(x) → bool.  <https://docs.python.org/3/library/math.html#math.isnan>
    #[pure]
    fn isnan(args) -> Result<Value> {
        Ok(Value::bool_(single_float(FN_NAME, args)?.is_nan()))
    }

    /// CPython: math.isinf(x) → bool.  <https://docs.python.org/3/library/math.html#math.isinf>
    #[pure]
    fn isinf(args) -> Result<Value> {
        Ok(Value::bool_(single_float(FN_NAME, args)?.is_infinite()))
    }

    /// CPython: math.pow(x, y) → float.  <https://docs.python.org/3/library/math.html#math.pow>
    #[pure]
    fn pow(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes exactly two arguments"
            )));
        }
        let x = value_to_float(&args[0].value, FN_NAME)?;
        let y = value_to_float(&args[1].value, FN_NAME)?;
        Ok(Value::float(x.powf(y)))
    }

    /// CPython: math.atan2(y, x) → float.  <https://docs.python.org/3/library/math.html#math.atan2>
    #[pure]
    fn atan2(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes exactly two arguments"
            )));
        }
        let y = value_to_float(&args[0].value, FN_NAME)?;
        let x = value_to_float(&args[1].value, FN_NAME)?;
        Ok(Value::float(y.atan2(x)))
    }

    /// CPython: math.log(x[, base]) → float.  <https://docs.python.org/3/library/math.html#math.log>
    #[pure]
    fn log(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes one or two arguments"
            )));
        }
        let x = value_to_float(&args[0].value, FN_NAME)?;
        if args.len() == 2 {
            let base = value_to_float(&args[1].value, FN_NAME)?;
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
