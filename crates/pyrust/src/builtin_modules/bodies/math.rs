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
    float_to_bigint, reject_keyword_args_expanded, value_to_float, value_type_name_str,
};
use crate::value::{PyBigInt, PyToPrimitive, Value, ValueKind};
use pyrust_derive::pyrust_module;

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
        // int.__floor__ returns self unchanged — no float coercion needed and
        // coercing a large int to f64 would silently lose precision (e.g.
        // math.floor(2**53+1) must return 2**53+1, not 2**53).
        match val.kind() {
            ValueKind::Int(n) => return Ok(Value::int(n)),
            ValueKind::BigInt(b) => return Ok(Value::bigint(b.clone())),
            ValueKind::Bool(b) => return Ok(Value::int(b as i64)),
            _ => {}
        }
        let x = math_coerce_float(val)?;
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
        match val.kind() {
            ValueKind::Int(n) => return Ok(Value::int(n)),
            ValueKind::BigInt(b) => return Ok(Value::bigint(b.clone())),
            ValueKind::Bool(b) => return Ok(Value::int(b as i64)),
            _ => {}
        }
        let x = math_coerce_float(val)?;
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
    /// Protocol: first try `type(x).__trunc__(x)`.  For `int` / `bool` the
    /// value is returned unchanged (CPython's numeric tower: int.__trunc__
    /// returns self).  For `float` the fractional part is discarded and the
    /// result is an `int`.  For any other type without `__trunc__`, raises
    /// `TypeError: type X doesn't define __trunc__ method`.
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
        match val.kind() {
            // int and bool: trunc(x) == x (already an integer).
            ValueKind::Int(n) => Ok(Value::int(n)),
            ValueKind::BigInt(b) => Ok(Value::bigint(b.clone())),
            ValueKind::Bool(b) => Ok(Value::int(b as i64)),
            // float: truncate toward zero.
            ValueKind::Float(f) => {
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
            _ => Err(PyError::named(
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
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two arguments"),
            ));
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
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly two arguments"),
            ));
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
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes one or two arguments"),
            ));
        }
        let x = value_to_float(&args[0].value, FN_NAME)?;
        if args.len() == 2 {
            let base = value_to_float(&args[1].value, FN_NAME)?;
            Ok(Value::float(x.log(base)))
        } else {
            Ok(Value::float(x.ln()))
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
        let x = value_to_float(&args[0].value, FN_NAME)?;
        let y = value_to_float(&args[1].value, FN_NAME)?;
        Ok(Value::float(x.copysign(y)))
    }

    /// CPython: math.isfinite(x) → bool.  True if x is finite (not inf or nan).
    /// <https://docs.python.org/3/library/math.html#math.isfinite>
    #[pure]
    fn isfinite(args) -> Result<Value> {
        Ok(Value::bool_(single_float(FN_NAME, args)?.is_finite()))
    }

    /// CPython: math.hypot(*coords) → float.  Euclidean distance from origin.
    /// Accepts zero or more positional float arguments.
    /// <https://docs.python.org/3/library/math.html#math.hypot>
    #[pure]
    fn hypot(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let mut sum_sq = 0.0f64;
        for arg in args.iter() {
            let x = value_to_float(&arg.value, FN_NAME)?;
            sum_sq += x * x;
        }
        Ok(Value::float(sum_sq.sqrt()))
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
        let p_items = _interp.collect_iterable(args[0].value.clone())?;
        let q_items = _interp.collect_iterable(args[1].value.clone())?;
        if p_items.len() != q_items.len() {
            return Err(PyError::named(
                "ValueError",
                "both points must have the same number of dimensions".to_string(),
            ));
        }
        let mut sum_sq = 0.0f64;
        for (pv, qv) in p_items.iter().zip(q_items.iter()) {
            let a = value_to_float(pv, FN_NAME)?;
            let b = value_to_float(qv, FN_NAME)?;
            sum_sq += (a - b) * (a - b);
        }
        Ok(Value::float(sum_sq.sqrt()))
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
            let n = value_to_bigint_int(FN_NAME, &arg.value)?;
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
            let n = value_to_bigint_int(FN_NAME, &arg.value)?;
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
        let n_big = value_to_bigint_strict_int(FN_NAME, &args[0].value)?;
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
        let n = value_to_bigint_int(FN_NAME, &args[0].value)?;
        let k = value_to_bigint_int(FN_NAME, &args[1].value)?;
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
        let n = value_to_bigint_int(FN_NAME, &args[0].value)?;
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
            let kv = value_to_bigint_int(FN_NAME, &args[1].value)?;
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
        let items = _interp.collect_iterable(positional[0].value.clone())?;
        let mut acc = start;
        for item in items {
            acc = _interp.eval_binary(acc, BinaryOp::Mul, item)?;
        }
        Ok(acc)
    }
}

// ── Helpers used by the macro-generated bodies ───────────────────────────────

/// Coerce `val` to `f64` for the `math.floor` / `math.ceil` fallback path.
///
/// CPython's error text for non-real types in those functions is
/// `"must be real number, not {type}"`, which differs from the
/// `"math.floor: a float is required, not …"` text that `value_to_float`
/// emits.  We call `value_to_float` and remap any error to the CPython form.
fn math_coerce_float(val: &Value) -> Result<f64> {
    value_to_float(val, "__SENTINEL__").map_err(|_| {
        PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(val)),
        )
    })
}

/// Reject kwargs and demand exactly one positional float-coercible arg.
fn single_float(fn_name: &str, args: &[ExpandedCallArg]) -> Result<f64> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one argument"),
        ));
    }
    value_to_float(&args[0].value, fn_name)
}

/// Extract a `Value` as a `PyBigInt` integer.
/// Accepts `Int`, `BigInt`, and `Bool` (bool is a subclass of int in Python).
/// Rejects `Float` and other types with a `TypeError`.
fn value_to_bigint_int(_fn_name: &str, val: &Value) -> Result<PyBigInt> {
    match val.kind() {
        ValueKind::Int(n) => Ok(PyBigInt::from(n)),
        ValueKind::BigInt(b) => Ok(b.clone()),
        ValueKind::Bool(b) => Ok(PyBigInt::from(b as i64)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

/// Like `value_to_bigint_int` but raises `TypeError` with the exact CPython
/// message for `factorial()`, which does NOT accept floats even when integral.
fn value_to_bigint_strict_int(_fn_name: &str, val: &Value) -> Result<PyBigInt> {
    match val.kind() {
        ValueKind::Int(n) => Ok(PyBigInt::from(n)),
        ValueKind::BigInt(b) => Ok(b.clone()),
        ValueKind::Bool(b) => Ok(PyBigInt::from(b as i64)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
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
