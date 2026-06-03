// `math` module — included into `pub mod math { … }` declared by the
// `pyrust_builtin_modules!` invocation in
// `builtin_modules/mod.rs`.  The macro injects a sibling
// `MODULE_NAME: &str = "math"` constant; the `pyrust_module!` body
// below reads it to compose every function's `FN_NAME` and the
// `PyModule.name`.  No name literal appears in this file.
//
// Reference: <https://docs.python.org/3/library/math.html>

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{
    float_to_bigint, instance_builtin_data, invoke_class_method, lookup_class_attr,
    reject_keyword_args_expanded, value_to_float, value_type_name_str,
};
use crate::value::{PyBigInt, PyToPrimitive, Value, ValueKind};
use pyrust_derive::pyrust_module;
use std::rc::Rc;

/// Check for a unary dunder method (`__floor__`, `__ceil__`, `__trunc__`) on
/// a `PyInstance` value, calling it if present, or returning `None` so the
/// caller can fall back to the numeric conversion path.
///
/// This mirrors CPython's `Modules/mathmodule.c` protocol for those three
/// functions: dunder dispatch is attempted first; if the method is absent the
/// float-coercion path is used instead.
fn try_math_dunder(
    interp: &mut crate::Interpreter,
    val: &Value,
    method: &str,
) -> Option<Result<Value>> {
    interp.try_dunder_unary(val, method)
}

pyrust_module! {
    constants {
        "pi"  => Value::float(std::f64::consts::PI),
        "e"   => Value::float(std::f64::consts::E),
        "tau" => Value::float(std::f64::consts::TAU),
        "inf" => Value::float(f64::INFINITY),
        "nan" => Value::float(f64::NAN),
    }

    /// CPython: math.floor(x) → int.  <https://docs.python.org/3/library/math.html#math.floor>
    ///
    /// Protocol: first try `type(x).__floor__(x)`; if absent, fall back to
    /// float coercion and apply `f64::floor`.  Mirrors CPython's
    /// `math_floor_impl` in `Modules/mathmodule.c`.
    fn floor(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one argument"),
            ));
        }
        let val = &args[0].value;
        if let Some(r) = try_math_dunder(_interp, val, "__floor__") {
            return r;
        }
        // int.__floor__ returns self unchanged (covers concrete ints and int
        // subclasses) — no float coercion needed, and coercing a large int to
        // f64 would silently lose precision (e.g. math.floor(2**53+1) must
        // return 2**53+1, not 2**53).
        if let Some(n) = math_integral_exact(val) {
            return Ok(n);
        }
        // Otherwise coerce via __float__/__index__ (CPython's PyFloat_AsDouble)
        // and apply f64::floor. A value reached via __index__ is rounded
        // through f64 here, matching CPython (it does not preserve precision).
        let x = math_arg_to_float(_interp, val)?;
        // Guard nan/inf before applying floor — NaN comparisons are always false
        // so the range check below would silently fall through to `as i64`.
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
        let f = x.floor();
        if f >= i64::MAX as f64 || f < i64::MIN as f64 {
            float_to_bigint(f)
        } else {
            Ok(Value::int(f as i64))
        }
    }

    /// CPython: math.ceil(x) → int.  <https://docs.python.org/3/library/math.html#math.ceil>
    ///
    /// Protocol: first try `type(x).__ceil__(x)`; if absent, fall back to
    /// float coercion and apply `f64::ceil`.  Mirrors CPython's
    /// `math_ceil_impl` in `Modules/mathmodule.c`.
    fn ceil(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one argument"),
            ));
        }
        let val = &args[0].value;
        if let Some(r) = try_math_dunder(_interp, val, "__ceil__") {
            return r;
        }
        // int.__ceil__ returns self unchanged — same precision reasoning as
        // floor above.
        if let Some(n) = math_integral_exact(val) {
            return Ok(n);
        }
        // Otherwise coerce via __float__/__index__ and apply f64::ceil.
        let x = math_arg_to_float(_interp, val)?;
        // Guard nan/inf before applying ceil — same reasoning as floor above.
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
        let f = x.ceil();
        if f >= i64::MAX as f64 || f < i64::MIN as f64 {
            float_to_bigint(f)
        } else {
            Ok(Value::int(f as i64))
        }
    }

    /// CPython: math.trunc(x) → Integral.  <https://docs.python.org/3/library/math.html#math.trunc>
    ///
    /// Protocol: first try `type(x).__trunc__(x)`.  For `int` / `bool` and `int`
    /// subclasses the value is returned unchanged (CPython's numeric tower:
    /// int.__trunc__ returns self).  For `float` and `float` subclasses the
    /// fractional part is discarded and the result is an `int` (inherited
    /// float.__trunc__).  Unlike floor/ceil, trunc does NOT fall back to
    /// __index__/__float__ on plain objects: any other type without __trunc__
    /// raises `TypeError: type X doesn't define __trunc__ method`.
    fn trunc(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one argument"),
            ));
        }
        let val = &args[0].value;
        // Try user-defined __trunc__ first (covers PyInstance and any class
        // that defines the dunder).
        if let Some(r) = try_math_dunder(_interp, val, "__trunc__") {
            return r;
        }
        // int and int subclasses: trunc(x) == x (already an integer), returned
        // exactly without an f64 round-trip.
        if let Some(n) = math_integral_exact(val) {
            return Ok(n);
        }
        // float and float subclasses (inherited float.__trunc__): truncate the
        // backing value toward zero. Other types fall through to the TypeError.
        let f = match val.kind() {
            ValueKind::Float(f) => Some(f),
            ValueKind::PyInstance(inst) => match instance_builtin_data(inst).as_ref().map(|b| b.kind()) {
                Some(ValueKind::Float(f)) => Some(f),
                _ => None,
            },
            _ => None,
        };
        match f {
            Some(f) => {
                // Guard nan/inf before trunc — NaN comparisons are always false
                // so the range check below would silently fall through to `as i64`.
                if f.is_nan() {
                    return Err(PyError::named(
                        "ValueError",
                        "cannot convert float NaN to integer".to_string(),
                    ));
                }
                if f.is_infinite() {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot convert float infinity to integer".to_string(),
                    ));
                }
                let t = f.trunc();
                if t >= i64::MAX as f64 || t < i64::MIN as f64 {
                    float_to_bigint(t)
                } else {
                    Ok(Value::int(t as i64))
                }
            }
            // Everything else: raise CPython's exact TypeError message.
            None => Err(PyError::named(
                "TypeError",
                format!(
                    "type {} doesn't define __trunc__ method",
                    value_type_name_str(val)
                ),
            )),
        }
    }

    /// CPython: math.sqrt(x) → float.  <https://docs.python.org/3/library/math.html#math.sqrt>
    #[pure]
    fn sqrt(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.sqrt())?))
    }

    /// CPython: math.fabs(x) → float.  <https://docs.python.org/3/library/math.html#math.fabs>
    #[pure]
    fn fabs(args) -> Result<Value> {
        Ok(Value::float(single_float(_interp, FN_NAME, args)?.abs()))
    }

    /// CPython: math.sin(x) → float.  <https://docs.python.org/3/library/math.html#math.sin>
    #[pure]
    fn sin(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.sin())?))
    }

    /// CPython: math.cos(x) → float.  <https://docs.python.org/3/library/math.html#math.cos>
    #[pure]
    fn cos(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.cos())?))
    }

    /// CPython: math.tan(x) → float.  <https://docs.python.org/3/library/math.html#math.tan>
    #[pure]
    fn tan(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.tan())?))
    }

    /// CPython: math.asin(x) → float.  <https://docs.python.org/3/library/math.html#math.asin>
    #[pure]
    fn asin(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.asin())?))
    }

    /// CPython: math.acos(x) → float.  <https://docs.python.org/3/library/math.html#math.acos>
    #[pure]
    fn acos(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.acos())?))
    }

    /// CPython: math.atan(x) → float.  <https://docs.python.org/3/library/math.html#math.atan>
    #[pure]
    fn atan(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.atan())?))
    }

    /// CPython: math.exp(x) → float.  <https://docs.python.org/3/library/math.html#math.exp>
    #[pure]
    fn exp(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.exp())?))
    }

    /// CPython: math.log2(x) → float.  <https://docs.python.org/3/library/math.html#math.log2>
    #[pure]
    fn log2(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.log2())?))
    }

    /// CPython: math.log10(x) → float.  <https://docs.python.org/3/library/math.html#math.log10>
    #[pure]
    fn log10(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.log10())?))
    }

    /// CPython: math.isnan(x) → bool.  <https://docs.python.org/3/library/math.html#math.isnan>
    #[pure]
    fn isnan(args) -> Result<Value> {
        Ok(Value::bool_(single_float(_interp, FN_NAME, args)?.is_nan()))
    }

    /// CPython: math.isinf(x) → bool.  <https://docs.python.org/3/library/math.html#math.isinf>
    #[pure]
    fn isinf(args) -> Result<Value> {
        Ok(Value::bool_(single_float(_interp, FN_NAME, args)?.is_infinite()))
    }

    /// CPython: math.pow(x, y) → float.  <https://docs.python.org/3/library/math.html#math.pow>
    #[pure]
    fn pow(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two arguments"),
            ));
        }
        let x = math_arg_to_float(_interp, &args[0].value)?;
        let y = math_arg_to_float(_interp, &args[1].value)?;
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

    /// CPython: math.atan2(y, x) → float.  <https://docs.python.org/3/library/math.html#math.atan2>
    #[pure]
    fn atan2(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two arguments"),
            ));
        }
        let y = math_arg_to_float(_interp, &args[0].value)?;
        let x = math_arg_to_float(_interp, &args[1].value)?;
        Ok(Value::float(y.atan2(x)))
    }

    /// CPython: math.log(x[, base]) → float.  <https://docs.python.org/3/library/math.html#math.log>
    #[pure]
    fn log(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes one or two arguments"),
            ));
        }
        let x = math_arg_to_float(_interp, &args[0].value)?;
        if args.len() == 2 {
            let base = math_arg_to_float(_interp, &args[1].value)?;
            // Mirror CPython's two-arg log: compute ln(x) / ln(base).
            // This order matters: if x is out-of-domain, raise ValueError first;
            // only then check the base, so that log(0, 1) → ValueError (not ZeroDivisionError).
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

    /// CPython: math.copysign(x, y) → float.  Copy sign of y onto magnitude of |x|.
    /// <https://docs.python.org/3/library/math.html#math.copysign>
    #[pure]
    fn copysign(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two arguments"),
            ));
        }
        let x = math_arg_to_float(_interp, &args[0].value)?;
        let y = math_arg_to_float(_interp, &args[1].value)?;
        Ok(Value::float(x.copysign(y)))
    }

    /// CPython: math.isfinite(x) → bool.  True if x is finite (not inf or nan).
    /// <https://docs.python.org/3/library/math.html#math.isfinite>
    #[pure]
    fn isfinite(args) -> Result<Value> {
        Ok(Value::bool_(single_float(_interp, FN_NAME, args)?.is_finite()))
    }

    /// CPython: math.hypot(*coords) → float.  Euclidean distance from origin.
    /// Accepts zero or more positional float arguments.
    /// <https://docs.python.org/3/library/math.html#math.hypot>
    #[pure]
    fn hypot(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let mut coords = Vec::with_capacity(args.len());
        for arg in args.iter() {
            coords.push(math_arg_to_float(_interp, &arg.value)?);
        }
        Ok(Value::float(vector_norm(&mut coords)))
    }

    /// CPython: math.dist(p, q) → float.  Euclidean distance between two points.
    /// Both sequences must have the same length.  Any iterable is accepted.
    /// <https://docs.python.org/3/library/math.html#math.dist>
    fn dist(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two arguments"),
            ));
        }
        let p_items = _interp.collect_iterable(&args[0].value)?;
        let q_items = _interp.collect_iterable(&args[1].value)?;
        if p_items.len() != q_items.len() {
            return Err(PyError::named(
                "ValueError",
                "both points must have the same number of dimensions".to_string(),
            ));
        }
        let mut diffs = Vec::with_capacity(p_items.len());
        for (pv, qv) in p_items.iter().zip(q_items.iter()) {
            let a = math_arg_to_float(_interp, pv)?;
            let b = math_arg_to_float(_interp, qv)?;
            diffs.push(a - b);
        }
        Ok(Value::float(vector_norm(&mut diffs)))
    }

    /// CPython: math.gcd(*integers) → int.  Greatest common divisor.
    /// Variadic (0 or more args); result is always non-negative.
    /// math.gcd() → 0, math.gcd(n) → abs(n).
    /// <https://docs.python.org/3/library/math.html#math.gcd>
    #[pure]
    fn gcd(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let mut result = PyBigInt::from(0i64);
        for arg in args.iter() {
            let n = value_to_bigint_int(_interp, FN_NAME, &arg.value)?;
            result = bigint_gcd(result, n);
        }
        bigint_to_int_value(result)
    }

    /// CPython: math.lcm(*integers) → int.  Least common multiple.
    /// Variadic (0 or more args); math.lcm() → 1.
    /// <https://docs.python.org/3/library/math.html#math.lcm>
    #[pure]
    fn lcm(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let mut result = PyBigInt::from(1i64);
        for arg in args.iter() {
            let n = value_to_bigint_int(_interp, FN_NAME, &arg.value)?;
            result = bigint_lcm(result, n);
        }
        bigint_to_int_value(result)
    }

    /// CPython: math.factorial(n) → int.  Factorial of a non-negative integer.
    /// Raises TypeError for floats; ValueError for negative values.
    /// <https://docs.python.org/3/library/math.html#math.factorial>
    #[pure]
    fn factorial(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one argument"),
            ));
        }
        let n_big = value_to_bigint_strict_int(_interp, FN_NAME, &args[0].value)?;
        use num_traits::Signed;
        if n_big.is_negative() {
            return Err(PyError::named(
                "ValueError",
                "factorial() not defined for negative values".to_string(),
            ));
        }
        // Convert to u64 for iteration; CPython allows arbitrarily large
        // factorials, but practically n must fit in memory.
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

    /// CPython: math.comb(n, k) → int.  Binomial coefficient n choose k.
    /// Returns 0 when k > n. Raises ValueError if n < 0 or k < 0.
    /// <https://docs.python.org/3/library/math.html#math.comb>
    #[pure]
    fn comb(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two arguments"),
            ));
        }
        let n = value_to_bigint_int(_interp, FN_NAME, &args[0].value)?;
        let k = value_to_bigint_int(_interp, FN_NAME, &args[1].value)?;
        use num_traits::Signed;
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
        // comb(n, k) = n! / (k! * (n-k)!)  — computed iteratively to avoid
        // materialising full factorials.  Use the smaller of k and n-k.
        let k_small = k.clone().min(n.clone() - k.clone());
        let k_u64: u64 = k_small.to_u64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "comb() argument is too large".to_string(),
            )
        })?;
        let mut result = PyBigInt::from(1i64);
        for i in 0..k_u64 {
            result *= n.clone() - PyBigInt::from(i);
            result /= PyBigInt::from(i + 1);
        }
        bigint_to_int_value(result)
    }

    /// CPython: math.perm(n, k=None) → int.  Number of ways to choose k items
    /// from n without repetition and with order.  perm(n) == factorial(n).
    /// <https://docs.python.org/3/library/math.html#math.perm>
    #[pure]
    fn perm(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes one or two arguments"),
            ));
        }
        let n = value_to_bigint_int(_interp, FN_NAME, &args[0].value)?;
        // Whether k was explicitly supplied (not None) changes the error message
        // CPython uses for negative n: when k is omitted/None it delegates to
        // factorial's message; when k is an integer it uses perm's own wording.
        let k_explicit = args.len() == 2 && !matches!(args[1].value.kind(), ValueKind::None);
        use num_traits::Signed;
        if n.is_negative() {
            let msg = if k_explicit {
                "n must be a non-negative integer".to_string()
            } else {
                "factorial() not defined for negative values".to_string()
            };
            return Err(PyError::named("ValueError", msg));
        }
        let k = if k_explicit {
            let kv = value_to_bigint_int(_interp, FN_NAME, &args[1].value)?;
            if kv.is_negative() {
                return Err(PyError::named(
                    "ValueError",
                    "k must be a non-negative integer".to_string(),
                ));
            }
            kv
        } else {
            // perm(n) or perm(n, None): k defaults to n
            n.clone()
        };
        if k > n {
            return Ok(Value::int(0));
        }
        // P(n, k) = n * (n-1) * ... * (n-k+1)
        let k_u64: u64 = k.to_u64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "perm() argument is too large".to_string(),
            )
        })?;
        let mut result = PyBigInt::from(1i64);
        for i in 0..k_u64 {
            result *= n.clone() - PyBigInt::from(i);
        }
        bigint_to_int_value(result)
    }

    /// CPython: math.prod(iterable, *, start=1) → number.  Product of all elements.
    /// <https://docs.python.org/3/library/math.html#math.prod>
    fn prod(args) -> Result<Value> {
        // Separate positional from keyword args.
        let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
        let keyword: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_some()).collect();
        if positional.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one positional argument"),
            ));
        }
        for kw in &keyword {
            let name = kw.name.as_deref().unwrap_or("");
            if name != "start" {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() got an unexpected keyword argument '{name}'"),
                ));
            }
        }
        let start = keyword
            .first()
            .map(|a| a.value.clone())
            .unwrap_or_else(|| Value::int(1));
        let items = _interp.collect_iterable(&positional[0].value)?;
        let mut acc = start;
        for item in items {
            acc = _interp.eval_binary(acc, BinaryOp::Mul, item)?;
        }
        Ok(acc)
    }

    /// CPython: math.degrees(x) → float.  Convert radians to degrees.
    /// <https://docs.python.org/3/library/math.html#math.degrees>
    #[pure]
    fn degrees(args) -> Result<Value> {
        Ok(Value::float(single_float(_interp, FN_NAME, args)?.to_degrees()))
    }

    /// CPython: math.radians(x) → float.  Convert degrees to radians.
    /// <https://docs.python.org/3/library/math.html#math.radians>
    #[pure]
    fn radians(args) -> Result<Value> {
        Ok(Value::float(single_float(_interp, FN_NAME, args)?.to_radians()))
    }

    /// CPython: math.sinh(x) → float.  Hyperbolic sine.
    /// <https://docs.python.org/3/library/math.html#math.sinh>
    #[pure]
    fn sinh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_overflow(x, x.sinh())?))
    }

    /// CPython: math.cosh(x) → float.  Hyperbolic cosine.
    /// <https://docs.python.org/3/library/math.html#math.cosh>
    #[pure]
    fn cosh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_overflow(x, x.cosh())?))
    }

    /// CPython: math.tanh(x) → float.  Hyperbolic tangent.
    /// <https://docs.python.org/3/library/math.html#math.tanh>
    #[pure]
    fn tanh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.tanh())?))
    }

    /// CPython: math.asinh(x) → float.  Inverse hyperbolic sine.
    /// <https://docs.python.org/3/library/math.html#math.asinh>
    #[pure]
    fn asinh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.asinh())?))
    }

    /// CPython: math.acosh(x) → float.  Inverse hyperbolic cosine.
    /// Domain: x >= 1; acosh(x<1) → ValueError ("math domain error").
    /// <https://docs.python.org/3/library/math.html#math.acosh>
    #[pure]
    fn acosh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.acosh())?))
    }

    /// CPython: math.atanh(x) → float.  Inverse hyperbolic tangent.
    /// Domain: -1 < x < 1; atanh(±1) → ValueError, |x|>1 → ValueError.
    /// <https://docs.python.org/3/library/math.html#math.atanh>
    #[pure]
    fn atanh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        // atanh(±1) → ±inf in C, but CPython reports it as a *domain* error
        // (EDOM), not a range error.  Handle it before the generic
        // check_math_result (which would map +inf to OverflowError).
        if x.abs() == 1.0 {
            return Err(PyError::named("ValueError", "math domain error".to_string()));
        }
        Ok(Value::float(check_math_result(x, x.atanh())?))
    }

    /// CPython: math.expm1(x) → float.  Compute e**x - 1 accurately for small x.
    /// <https://docs.python.org/3/library/math.html#math.expm1>
    #[pure]
    fn expm1(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_overflow(x, x.exp_m1())?))
    }

    /// CPython: math.log1p(x) → float.  Compute ln(1+x) accurately for small x.
    /// Domain: x > -1; log1p(-1) → ValueError, log1p(x<-1) → ValueError.
    /// <https://docs.python.org/3/library/math.html#math.log1p>
    #[pure]
    fn log1p(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.ln_1p())?))
    }

    /// CPython: math.exp2(x) → float.  Compute 2**x.
    /// <https://docs.python.org/3/library/math.html#math.exp2>
    #[pure]
    fn exp2(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_overflow(x, x.exp2())?))
    }

    /// CPython: math.cbrt(x) → float.  Cube root (defined for negative x).
    /// Note: results can differ from CPython by one ULP for non-perfect-cube
    /// inputs (e.g. cbrt(27)) because Rust's `f64::cbrt` and the cbrt CPython
    /// 3.12 links round the last bit differently.  Exact cubes, 0, ±inf and NaN
    /// agree.  <https://docs.python.org/3/library/math.html#math.cbrt>
    #[pure]
    fn cbrt(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.cbrt())?))
    }

    /// CPython: math.fmod(x, y) → float.  C library fmod: result has the sign of
    /// x.  fmod(x, 0) and fmod(±inf, y) raise ValueError ("math domain error").
    /// <https://docs.python.org/3/library/math.html#math.fmod>
    #[pure]
    fn fmod(args) -> Result<Value> {
        let (x, y) = two_floats(_interp, FN_NAME, args)?;
        // Rust's `%` on f64 implements C fmod semantics (truncated remainder,
        // sign of the dividend).
        let r = x % y;
        // CPython raises "math domain error" when the C result is NaN but
        // neither input was NaN (covers fmod(x, 0), fmod(inf, y), fmod(inf,inf)).
        if r.is_nan() && !x.is_nan() && !y.is_nan() {
            return Err(PyError::named("ValueError", "math domain error".to_string()));
        }
        Ok(Value::float(r))
    }

    /// CPython: math.remainder(x, y) → float.  IEEE 754 remainder: x - n*y where
    /// n is the integer nearest x/y (ties to even).  Degenerate finite inputs
    /// (e.g. remainder(inf, 1), remainder(1, 0)) raise ValueError.
    /// <https://docs.python.org/3/library/math.html#math.remainder>
    #[pure]
    fn remainder(args) -> Result<Value> {
        let (x, y) = two_floats(_interp, FN_NAME, args)?;
        let r = ieee_remainder(x, y);
        if r.is_nan() && !x.is_nan() && !y.is_nan() {
            return Err(PyError::named("ValueError", "math domain error".to_string()));
        }
        Ok(Value::float(r))
    }

    /// CPython: math.modf(x) → (frac, int).  Fractional and integer parts, both
    /// floats, each carrying the sign of x.
    /// <https://docs.python.org/3/library/math.html#math.modf>
    #[pure]
    fn modf(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        // CPython returns (frac, intpart). For ±inf the integer part is ±inf and
        // the fractional part is ±0.0 with the sign of x; f64::fract() yields NaN
        // for inf, so handle it explicitly.
        let (frac, intpart) = if x.is_infinite() {
            (0.0_f64.copysign(x), x)
        } else {
            // f64::fract() drops the sign on a zero fractional part (e.g.
            // (-0.0).fract() == 0.0), but CPython's modf keeps the sign of x.
            (x.fract().copysign(x), x.trunc())
        };
        Ok(Value::tuple(vec![Value::float(frac), Value::float(intpart)]))
    }

    /// CPython: math.frexp(x) → (m, e).  Decompose x into m * 2**e with
    /// 0.5 <= |m| < 1 (or m == 0).  For 0/±inf/NaN, e == 0 and m == x.
    /// <https://docs.python.org/3/library/math.html#math.frexp>
    #[pure]
    fn frexp(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        let (m, e) = frexp_f64(x);
        Ok(Value::tuple(vec![Value::float(m), Value::int(e as i64)]))
    }

    /// CPython: math.ldexp(x, i) → float.  Compute x * 2**i.  Overflow raises
    /// OverflowError ("math range error").  `i` must be an integer.
    /// <https://docs.python.org/3/library/math.html#math.ldexp>
    #[pure]
    fn ldexp(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two arguments"),
            ));
        }
        let x = math_arg_to_float(_interp, &args[0].value)?;
        let exp = value_to_exp_int(FN_NAME, &args[1].value)?;
        let r = ldexp_f64(x, exp);
        // ldexp of a finite non-zero x that overflows to ±inf is a range error.
        if r.is_infinite() && x.is_finite() && x != 0.0 {
            return Err(PyError::named("OverflowError", "math range error".to_string()));
        }
        Ok(Value::float(r))
    }

    /// CPython: math.nextafter(x, y, *, steps=1) → float.  The float `steps`
    /// representable values after x in the direction of y.
    /// <https://docs.python.org/3/library/math.html#math.nextafter>
    #[pure]
    fn nextafter(args) -> Result<Value> {
        let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
        for kw in args.iter().filter(|a| a.name.is_some()) {
            let name = kw.name.as_deref().unwrap_or("");
            if name != "steps" {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() got an unexpected keyword argument '{name}'"),
                ));
            }
        }
        if positional.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two positional arguments"),
            ));
        }
        let x = math_arg_to_float(_interp, &positional[0].value)?;
        let y = math_arg_to_float(_interp, &positional[1].value)?;
        let steps = match args.iter().find(|a| a.name.as_deref() == Some("steps")) {
            Some(a) => {
                let s = value_to_steps_int(_interp, FN_NAME, &a.value)?;
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

    /// CPython: math.ulp(x) → float.  Value of the least significant bit of x.
    /// ulp(nan)=nan, ulp(±inf)=inf, ulp(0)=smallest subnormal.
    /// <https://docs.python.org/3/library/math.html#math.ulp>
    #[pure]
    fn ulp(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(ulp_f64(x)))
    }

    /// CPython: math.isqrt(n) → int.  Integer square root (floor of the exact
    /// square root).  Works on arbitrary-precision ints.  Negative n raises
    /// ValueError; non-integers raise TypeError.
    /// <https://docs.python.org/3/library/math.html#math.isqrt>
    #[pure]
    fn isqrt(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one argument"),
            ));
        }
        let n = value_to_bigint_int(_interp, FN_NAME, &args[0].value)?;
        use num_traits::Signed;
        if n.is_negative() {
            return Err(PyError::named(
                "ValueError",
                "isqrt() argument must be nonnegative".to_string(),
            ));
        }
        // num-bigint's exact integer sqrt (floor) — no float rounding error.
        bigint_to_int_value(n.sqrt())
    }

    /// CPython: math.isclose(a, b, *, rel_tol=1e-09, abs_tol=0.0) → bool.
    /// <https://docs.python.org/3/library/math.html#math.isclose>
    #[pure]
    fn isclose(args) -> Result<Value> {
        let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
        for kw in args.iter().filter(|a| a.name.is_some()) {
            let name = kw.name.as_deref().unwrap_or("");
            if name != "rel_tol" && name != "abs_tol" {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() got an unexpected keyword argument '{name}'"),
                ));
            }
        }
        if positional.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two positional arguments"),
            ));
        }
        let a = math_arg_to_float(_interp, &positional[0].value)?;
        let b = math_arg_to_float(_interp, &positional[1].value)?;
        let rel_tol = match args.iter().find(|arg| arg.name.as_deref() == Some("rel_tol")) {
            Some(arg) => math_arg_to_float(_interp, &arg.value)?,
            None => 1e-9,
        };
        let abs_tol = match args.iter().find(|arg| arg.name.as_deref() == Some("abs_tol")) {
            Some(arg) => math_arg_to_float(_interp, &arg.value)?,
            None => 0.0,
        };
        if rel_tol < 0.0 || abs_tol < 0.0 {
            return Err(PyError::named(
                "ValueError",
                "tolerances must be non-negative".to_string(),
            ));
        }
        // CPython's algorithm (Modules/mathmodule.c, math_isclose_impl).
        let result = if a == b {
            true
        } else if a.is_infinite() || b.is_infinite() {
            // ±inf is only "close" to an identical infinity (handled by a == b).
            false
        } else {
            let diff = (b - a).abs();
            diff <= (rel_tol * b).abs() || diff <= (rel_tol * a).abs() || diff <= abs_tol
        };
        Ok(Value::bool_(result))
    }
}

// ── Helpers used by the macro-generated bodies ───────────────────────────────

/// Post-call result checker matching CPython's `math_1` errno/fpclassify logic.
///
/// Mirrors CPython's two-condition check in `Modules/mathmodule.c`:
/// - `isinf(r) && isfinite(x)` and `r > 0` → `OverflowError: math range error`
/// - `isinf(r) && isfinite(x)` and `r < 0` → `ValueError: math domain error` (e.g. log(0) → -inf)
/// - `isnan(r) && !isnan(x)` → `ValueError: math domain error`
///
/// The last condition catches inputs that are ±∞ (not NaN) but produce NaN
/// in the underlying C math function, such as `sin(inf)` / `cos(inf)` / `tan(inf)`.
/// Those are domain errors even though the input itself is not finite.
#[inline]
fn check_math_result(arg: f64, result: f64) -> Result<f64> {
    if result.is_infinite() && arg.is_finite() {
        if result > 0.0 {
            return Err(PyError::named(
                "OverflowError",
                "math range error".to_string(),
            ));
        }
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    if result.is_nan() && !arg.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    Ok(result)
}

/// Result-checker for functions whose only way to produce an infinite result
/// from a finite argument is *overflow* (the exponential / hyperbolic-growth
/// family: exp2, expm1, sinh, cosh).  Unlike `check_math_result`, a `-inf`
/// result here is a range error, not a domain error: `sinh(-1000)` overflows
/// to `-inf` and CPython reports `OverflowError` ("math range error"), whereas
/// `check_math_result` maps every `-inf` to `ValueError` (correct only for the
/// logarithmic pole at `log(0)` / `log1p(-1)`).
fn check_math_overflow(arg: f64, result: f64) -> Result<f64> {
    if result.is_infinite() && arg.is_finite() {
        return Err(PyError::named(
            "OverflowError",
            "math range error".to_string(),
        ));
    }
    if result.is_nan() && !arg.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    Ok(result)
}

/// Interpreter-aware coercion of a math float-argument to `f64`, matching
/// CPython's `PyFloat_AsDouble` / `Modules/mathmodule.c` argument handling.
///
/// Concrete `int` / `float` / `bool` / `BigInt` go through `value_to_float`
/// directly (the common case — no `_interp` work).  A `PyInstance` is coerced
/// like CPython's `PyFloat_AsDouble`:
///   1. a `float` subclass uses its primitive backing value directly — the
///      `PyFloat_Check` fast path, which *bypasses* any user `__float__`
///      override (CPython quirk: `math.sqrt(MyFloat(16))` ignores a
///      `__float__` that returns something else),
///   2. otherwise a user `__float__` is dispatched (must return an exact
///      `float` — CPython rejects `int`/`bool` returns with a TypeError),
///   3. otherwise, for an `int` subclass, its integer backing is used — this
///      is CPython's inherited `int.__float__` (`nb_float`), which ranks above
///      a user `__index__` override,
///   4. otherwise `__index__` is dispatched (math accepts `__index__` for
///      float arguments on plain objects).
///
/// `__float__` (and the int subclass's inherited `nb_float`) is preferred over
/// `__index__`, matching CPython's `nb_float`-before-`nb_index` order.
/// Anything with neither protocol raises
/// `TypeError: must be real number, not <type>`.
fn math_arg_to_float(interp: &mut crate::Interpreter, val: &Value) -> Result<f64> {
    if let ValueKind::PyInstance(inst) = val.kind() {
        let inst_rc = Rc::clone(inst);
        let class = Rc::clone(&inst_rc.borrow().class);
        let backing = instance_builtin_data(&inst_rc);
        // (1) float subclass: use the backing float directly (PyFloat_Check
        // fast path bypasses any __float__ override).
        if let Some(ref b) = backing {
            if matches!(b.kind(), ValueKind::Float(_)) {
                return value_to_float(b, "__SENTINEL__");
            }
        }
        let self_val = Value::py_instance(Rc::clone(&inst_rc));
        // (2) __float__ — must return an exact float (not int/bool).
        if let Some(method) = lookup_class_attr(&class, "__float__") {
            let result = invoke_class_method(interp, method, self_val, &[])?;
            return if let ValueKind::Float(f) = result.kind() {
                Ok(f)
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "{}.__float__ returned non-float (type {})",
                        class.borrow().name,
                        value_type_name_str(&result),
                    ),
                ))
            };
        }
        // (3) int subclass: the inherited int.__float__ (nb_float) uses the
        // backing value and ranks above a user __index__ override.
        if let Some(ref b) = backing {
            if matches!(
                b.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            ) {
                return value_to_float(b, "__SENTINEL__");
            }
        }
        // (4) __index__ — math accepts it for float arguments on plain objects.
        if let Some(method) = lookup_class_attr(&class, "__index__") {
            let result = invoke_class_method(interp, method, self_val, &[])?;
            return match result.kind() {
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
                    value_to_float(&result, "__SENTINEL__")
                }
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "__index__ returned non-int (type {})",
                        value_type_name_str(&result)
                    ),
                )),
            };
        }
        return Err(PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(val)),
        ));
    }
    // Concrete numeric kinds delegate to `value_to_float`, which preserves the
    // OverflowError for a BigInt too large to fit in f64.  Only genuinely
    // non-numeric kinds get the "must be real number" TypeError — remapping
    // every error here would mask that OverflowError.
    match val.kind() {
        ValueKind::Int(_) | ValueKind::Float(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
            value_to_float(val, "__SENTINEL__")
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(val)),
        )),
    }
}

/// If `val` is already integral — a concrete `int`/`bool`/`BigInt`, or an
/// `int` subclass instance — return that exact integer `Value` so
/// `math.floor`/`ceil`/`trunc` can hand it back unchanged.
///
/// CPython's `math_floor_impl`/`math_ceil_impl`/`math_trunc_impl` short-circuit
/// any `PyLong` (including subclasses) to avoid the f64 round-trip that would
/// silently drop precision for large integers (e.g. `math.floor(I(2**60+1))`).
/// An `int` subclass's exact value is used even when the subclass defines a
/// `__float__` override — matching CPython, where the inherited integer path
/// wins over `__float__` for floor/ceil/trunc.
fn math_integral_exact(val: &Value) -> Option<Value> {
    match val.kind() {
        ValueKind::Int(n) => Some(Value::int(n)),
        ValueKind::BigInt(b) => Some(Value::bigint(b.clone())),
        ValueKind::Bool(b) => Some(Value::int(b as i64)),
        ValueKind::PyInstance(inst) => match instance_builtin_data(inst).as_ref().map(|b| b.kind()) {
            Some(ValueKind::Int(n)) => Some(Value::int(n)),
            Some(ValueKind::BigInt(b)) => Some(Value::bigint(b.clone())),
            Some(ValueKind::Bool(b)) => Some(Value::int(b as i64)),
            _ => None,
        },
        _ => None,
    }
}

/// Reject kwargs and demand exactly one positional float-coercible arg.
fn single_float(
    interp: &mut crate::Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<f64> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    math_arg_to_float(interp, &args[0].value)
}

/// Reject kwargs and demand exactly two positional float-coercible args.
fn two_floats(
    interp: &mut crate::Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<(f64, f64)> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly two arguments"),
        ));
    }
    let x = math_arg_to_float(interp, &args[0].value)?;
    let y = math_arg_to_float(interp, &args[1].value)?;
    Ok((x, y))
}

/// Coerce an integer argument (the exponent of `ldexp` / `steps` of
/// `nextafter`) to `i32`, clamping to `i32::MIN/MAX` so an overflowing
/// magnitude saturates `ldexp` to ±inf (caught as a range error) or to 0.
/// Rejects floats with CPython's exact `ldexp` wording; accepts bool.
fn value_to_exp_int(_fn_name: &str, val: &Value) -> Result<i32> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
        ValueKind::Bool(b) => Ok(b as i32),
        ValueKind::BigInt(b) => {
            use num_traits::Signed;
            Ok(b.to_i32()
                .unwrap_or(if b.is_negative() { i32::MIN } else { i32::MAX }))
        }
        _ => Err(PyError::named(
            "TypeError",
            "Expected an int as second argument to ldexp.".to_string(),
        )),
    }
}

/// Coerce the `steps` argument of `nextafter` to `i64`, saturating an
/// out-of-range magnitude (the result is bounded by the distance to `y`
/// anyway, so a saturated huge value still lands exactly on `y`).
/// Rejects floats with TypeError; accepts bool (int subclass).
fn value_to_steps_int(interp: &mut crate::Interpreter, _fn_name: &str, val: &Value) -> Result<i64> {
    // `steps` honors the `__index__` protocol (#2022); a non-int raises the
    // canonical TypeError.  A bigint magnitude saturates to i64::MIN/MAX (the
    // result is bounded by the distance to `y`, so a saturated huge value still
    // lands exactly on `y` — matching CPython).
    let resolved = interp.value_to_index(val, |v| {
        PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(v)
            ),
        )
    })?;
    match resolved.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(b as i64),
        ValueKind::BigInt(b) => {
            use num_traits::Signed;
            Ok(b.to_i64()
                .unwrap_or(if b.is_negative() { i64::MIN } else { i64::MAX }))
        }
        _ => unreachable!("value_to_index guarantees an integer"),
    }
}

/// IEEE 754 remainder, matching CPython's `m_remainder` (Modules/mathmodule.c):
/// the nearest-integer-ties-to-even remainder of `x` by `y`.  Returns NaN for
/// the degenerate finite cases (`y == 0`) and the infinite-`x` case, which the
/// callers translate into a `ValueError`.
fn ieee_remainder(x: f64, y: f64) -> f64 {
    if x.is_finite() && y.is_finite() {
        if y == 0.0 {
            return f64::NAN;
        }
        let absx = x.abs();
        let absy = y.abs();
        let m = absx % absy;
        let c = absy - m;
        let r = if m < c {
            m
        } else if m > c {
            -c
        } else {
            // Exact halfway: pick the value that makes the quotient even.
            m - 2.0 * ((0.5 * (absx - m)) % absy)
        };
        return 1.0_f64.copysign(x) * r;
    }
    if x.is_nan() {
        return x;
    }
    if y.is_nan() {
        return y;
    }
    if x.is_infinite() {
        return f64::NAN;
    }
    // x finite, y infinite: remainder is x unchanged.
    x
}

/// Decompose `x` into `(m, e)` with `x == m * 2**e` and `0.5 <= |m| < 1`,
/// matching C `frexp` / CPython `math.frexp`.  For `0`, `±inf`, and `NaN` the
/// exponent is `0` and `m == x`.
fn frexp_f64(x: f64) -> (f64, i32) {
    if x == 0.0 || !x.is_finite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let raw_exp = ((bits >> 52) & 0x7ff) as i32;
    if raw_exp == 0 {
        // Subnormal: scale up by 2**64 to normalise, then adjust the exponent.
        let (m, e) = frexp_f64(x * (2f64).powi(64));
        return (m, e - 64);
    }
    // Normalised: exponent is biased by 1022 so the mantissa lands in [0.5, 1).
    let e = raw_exp - 1022;
    let m = f64::from_bits((bits & !(0x7ffu64 << 52)) | (1022u64 << 52));
    (m, e)
}

/// Compute `x * 2**exp`, matching C `ldexp` / CPython `math.ldexp`.
fn ldexp_f64(x: f64, exp: i32) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    // Scale in bounded steps so we keep correct gradual underflow into the
    // subnormal range: a single `2f64.powi(exp)` underflows to 0 once
    // `exp < -1074` even when `x * 2**exp` is still a representable subnormal.
    // 2**±1000 is always finite-and-normal, so multiplying by it never loses a
    // bit; the residual `|exp| <= 1000` step lands the result exactly.
    let mut e = exp;
    let mut r = x;
    while e > 1000 {
        r *= 2f64.powi(1000);
        e -= 1000;
        if r.is_infinite() {
            return r;
        }
    }
    while e < -1000 {
        r *= 2f64.powi(-1000);
        e += 1000;
        if r == 0.0 {
            return r;
        }
    }
    r * 2f64.powi(e)
}

/// Overflow/underflow-safe Euclidean norm of `vec`, used by `math.hypot` and
/// `math.dist`.  A faithful port of CPython 3.12's `vector_norm`
/// (`Modules/mathmodule.c`): it scales by the largest magnitude so the squares
/// never overflow, then accumulates the sum of scaled squares with a
/// double-length (compensated) running total plus a final differential
/// correction step.  This reproduces CPython's result byte-for-byte.
///
/// `vec` is taken by `&mut` because the subnormal-rescaling branch rewrites it
/// in place, mirroring the C code.
fn vector_norm(vec: &mut [f64]) -> f64 {
    // Algorithm 1.1: compensated sum of two doubles with |a| >= |b|.
    fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
        let x = a + b;
        let y = (a - x) + b;
        (x, y)
    }
    // Algorithm 3.5: error-free transformation of a product (uses fused mul-add).
    fn dl_mul(x: f64, y: f64) -> (f64, f64) {
        let z = x * y;
        let zz = x.mul_add(y, -z);
        (z, zz)
    }

    let n = vec.len();
    let mut max = 0.0f64;
    let mut found_nan = false;
    for v in vec.iter_mut() {
        *v = v.abs();
        if *v > max {
            max = *v;
        }
        if v.is_nan() {
            found_nan = true;
        }
    }
    if max.is_infinite() {
        return max;
    }
    if found_nan {
        return f64::NAN;
    }
    if max == 0.0 || n <= 1 {
        return max;
    }
    let (_, max_e) = frexp_f64(max);
    if max_e < -1023 {
        // ldexp(1.0, -max_e) would overflow; convert subnormals to normals
        // by dividing through by DBL_MIN, recurse, then scale the result back.
        for v in vec.iter_mut() {
            *v /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(vec);
    }
    let scale = ldexp_f64(1.0, -max_e);
    let mut csum = 1.0f64;
    let mut frac1 = 0.0f64;
    let mut frac2 = 0.0f64;
    for &v in vec.iter() {
        let x = v * scale; // lossless scaling
        let pr = dl_mul(x, x); // lossless squaring
        let sm = dl_fast_sum(csum, pr.0); // lossless addition
        csum = sm.0;
        frac1 += pr.1; // lossy addition
        frac2 += sm.1; // lossy addition
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let pr = dl_mul(-h, h);
    let sm = dl_fast_sum(csum, pr.0);
    csum = sm.0;
    frac1 += pr.1;
    frac2 += sm.1;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h); // differential correction
    h / scale
}

/// The `steps`-th representable double after `x` toward `y`, matching CPython's
/// `math.nextafter`.  `steps` is assumed non-negative (validated by the caller).
///
/// Done with direct bit arithmetic (not a step loop): consecutive doubles have
/// consecutive sign-magnitude bit patterns, so we map each value to a totally
/// ordered `i64` key, advance by `steps`, and map back.  This is O(1) and
/// matches CPython for arbitrarily large `steps`.
fn nextafter_f64(x: f64, y: f64, steps: i64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == y {
        // CPython returns y here (preserves -0.0 vs 0.0 of the target).
        return y;
    }
    if steps == 0 {
        return x;
    }
    // Map each f64 to a monotonic i64 key so that incrementing the key by 1
    // advances to the next representable double.  Positive floats keep their
    // bit pattern (a non-negative key); negative floats map to the negation of
    // their magnitude (an ordered negative key).  ±0.0 both map to key 0, which
    // is correct: a downward step from +0.0 and from -0.0 both reach -smallest.
    let to_ordered = |b: u64| -> i64 {
        if b & 0x8000_0000_0000_0000 == 0 {
            b as i64
        } else {
            -((b & 0x7fff_ffff_ffff_ffff) as i64)
        }
    };
    let from_ordered = |k: i64| -> u64 {
        if k >= 0 {
            k as u64
        } else {
            k.unsigned_abs() | 0x8000_0000_0000_0000
        }
    };
    let kx = to_ordered(x.to_bits()) as i128;
    let ky = to_ordered(y.to_bits()) as i128;
    // Move toward y by at most |ky - kx| steps; saturate at y.
    let dir: i128 = if ky > kx { 1 } else { -1 };
    let dist = (ky - kx).unsigned_abs();
    let advance = (steps as u128).min(dist) as i128;
    let kr = kx + dir * advance;
    f64::from_bits(from_ordered(kr as i64))
}

/// `math.ulp(x)`: the value of the least-significant bit of `x`.
fn ulp_f64(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let ax = x.abs();
    if ax.is_infinite() {
        return ax;
    }
    let up = ax.next_up();
    if up.is_infinite() {
        // At the largest finite magnitude, step downward instead.
        ax - ax.next_down()
    } else {
        up - ax
    }
}

/// Extract a `Value` as a `PyBigInt` integer for math's integer-taking
/// functions (`gcd` / `lcm` / `comb` / `perm` / `factorial` / `isqrt`).
///
/// Accepts `Int`, `BigInt`, and `Bool` (bool is a subclass of int).  For a
/// `PyInstance` it follows CPython's `__index__` protocol: an `int` subclass
/// yields its primitive backing, otherwise `type(x).__index__(x)` is
/// dispatched (which must return an `int`).  `__float__` is NOT consulted —
/// CPython's integer-argument functions reject float-only objects with
/// `'X' object cannot be interpreted as an integer`.
fn value_to_bigint_int(
    interp: &mut crate::Interpreter,
    _fn_name: &str,
    val: &Value,
) -> Result<PyBigInt> {
    // Route the int / int-subclass / `__index__` resolution through the shared
    // index protocol (#2022), then widen to `PyBigInt` (math functions need
    // arbitrary precision).  `__float__` is NOT consulted — CPython's
    // integer-argument functions reject float-only objects with the canonical
    // `'X' object cannot be interpreted as an integer`.
    let resolved = interp.value_to_index(val, |v| {
        PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(v)
            ),
        )
    })?;
    match resolved.kind() {
        ValueKind::Int(n) => Ok(PyBigInt::from(n)),
        ValueKind::BigInt(b) => Ok(b.clone()),
        ValueKind::Bool(b) => Ok(PyBigInt::from(b as i64)),
        _ => unreachable!("value_to_index guarantees an integer"),
    }
}

/// Integer coercion for `factorial()`.  CPython's `factorial` accepts the same
/// arguments as the other integer functions (int / int-subclass / `__index__`)
/// and rejects floats — so this delegates to `value_to_bigint_int`.
fn value_to_bigint_strict_int(
    interp: &mut crate::Interpreter,
    fn_name: &str,
    val: &Value,
) -> Result<PyBigInt> {
    value_to_bigint_int(interp, fn_name, val)
}

/// Euclidean GCD for `PyBigInt`.  Result is always non-negative.
fn bigint_gcd(mut a: PyBigInt, mut b: PyBigInt) -> PyBigInt {
    use num_traits::Signed;
    a = a.abs();
    b = b.abs();
    while b != PyBigInt::from(0i64) {
        let t = b.clone();
        b = a % b;
        a = t;
    }
    a
}

/// LCM for `PyBigInt`.  `lcm(a, b) = |a * b| / gcd(a, b)`.
/// When either argument is zero, returns zero.
fn bigint_lcm(a: PyBigInt, b: PyBigInt) -> PyBigInt {
    use num_traits::Signed;
    let ab = a.abs() * b.abs();
    if ab == PyBigInt::from(0i64) {
        return PyBigInt::from(0i64);
    }
    let g = bigint_gcd(a, b);
    ab / g
}

/// Convert a `PyBigInt` back to the smallest `Value` representation:
/// `Value::int` when it fits in `i64`, `Value::bigint` otherwise.
fn bigint_to_int_value(n: PyBigInt) -> Result<Value> {
    if let Some(i) = n.to_i64() {
        Ok(Value::int(i))
    } else {
        Ok(Value::bigint(n))
    }
}
