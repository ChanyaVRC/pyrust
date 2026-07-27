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
use crate::interpreter::{
    ExpandedCallArg, builtin_data_backing, float_to_bigint, reject_keyword_args_expanded,
    value_to_float, value_type_name_str,
};
use crate::value::{PyBigInt, PyToPrimitive, Value, ValueKind};
use pyrust_derive::pyrust_module;

// Keep the macro-generated registration/API surface in this file. Private
// implementation modules expose only their helper entry points to this parent,
// so moving algorithms cannot change Python-visible registration or visibility.
#[path = "math/coercion.rs"]
mod coercion;
#[path = "math/float_algorithms.rs"]
mod float_algorithms;
#[path = "math/integer_algorithms.rs"]
mod integer_algorithms;
#[path = "math/special_functions.rs"]
mod special_functions;
#[path = "math/summation.rs"]
mod summation;

use self::coercion::{
    math_arg_to_float, math_integral_exact, single_float, try_math_dunder, two_floats,
    value_to_exp_int, value_to_steps_int,
};
use self::float_algorithms::{
    check_math_overflow, check_math_result, frexp_f64, ieee_remainder, ldexp_f64, nextafter_f64,
    ulp_f64, vector_norm,
};
use self::integer_algorithms::{
    bigint_gcd, bigint_lcm, bigint_to_int_value, value_to_bigint_int, value_to_bigint_strict_int,
};
use self::special_functions::{m_erf, m_erfc, m_lgamma, m_tgamma};
use self::summation::{fsum_impl, sumprod_floats};

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
            ValueKind::PyInstance(_) => match builtin_data_backing(val).as_ref().map(|b| b.kind()) {
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
    fn sqrt(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.sqrt())?))
    }

    /// CPython: math.fabs(x) → float.  <https://docs.python.org/3/library/math.html#math.fabs>
    fn fabs(args) -> Result<Value> {
        Ok(Value::float(single_float(_interp, FN_NAME, args)?.abs()))
    }

    /// CPython: math.sin(x) → float.  <https://docs.python.org/3/library/math.html#math.sin>
    fn sin(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.sin())?))
    }

    /// CPython: math.cos(x) → float.  <https://docs.python.org/3/library/math.html#math.cos>
    fn cos(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.cos())?))
    }

    /// CPython: math.tan(x) → float.  <https://docs.python.org/3/library/math.html#math.tan>
    fn tan(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.tan())?))
    }

    /// CPython: math.asin(x) → float.  <https://docs.python.org/3/library/math.html#math.asin>
    fn asin(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.asin())?))
    }

    /// CPython: math.acos(x) → float.  <https://docs.python.org/3/library/math.html#math.acos>
    fn acos(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.acos())?))
    }

    /// CPython: math.atan(x) → float.  <https://docs.python.org/3/library/math.html#math.atan>
    fn atan(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.atan())?))
    }

    /// CPython: math.exp(x) → float.  <https://docs.python.org/3/library/math.html#math.exp>
    fn exp(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.exp())?))
    }

    /// CPython: math.log2(x) → float.  <https://docs.python.org/3/library/math.html#math.log2>
    fn log2(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.log2())?))
    }

    /// CPython: math.log10(x) → float.  <https://docs.python.org/3/library/math.html#math.log10>
    fn log10(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.log10())?))
    }

    /// CPython: math.isnan(x) → bool.  <https://docs.python.org/3/library/math.html#math.isnan>
    fn isnan(args) -> Result<Value> {
        Ok(Value::bool_(single_float(_interp, FN_NAME, args)?.is_nan()))
    }

    /// CPython: math.isinf(x) → bool.  <https://docs.python.org/3/library/math.html#math.isinf>
    fn isinf(args) -> Result<Value> {
        Ok(Value::bool_(single_float(_interp, FN_NAME, args)?.is_infinite()))
    }

    /// CPython: math.pow(x, y) → float.  <https://docs.python.org/3/library/math.html#math.pow>
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
    fn isfinite(args) -> Result<Value> {
        Ok(Value::bool_(single_float(_interp, FN_NAME, args)?.is_finite()))
    }

    /// CPython: math.hypot(*coords) → float.  Euclidean distance from origin.
    /// Accepts zero or more positional float arguments.
    /// <https://docs.python.org/3/library/math.html#math.hypot>
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
        let k_small = k.clone().min(n.clone() - k);
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
    fn degrees(args) -> Result<Value> {
        Ok(Value::float(single_float(_interp, FN_NAME, args)?.to_degrees()))
    }

    /// CPython: math.radians(x) → float.  Convert degrees to radians.
    /// <https://docs.python.org/3/library/math.html#math.radians>
    fn radians(args) -> Result<Value> {
        Ok(Value::float(single_float(_interp, FN_NAME, args)?.to_radians()))
    }

    /// CPython: math.sinh(x) → float.  Hyperbolic sine.
    /// <https://docs.python.org/3/library/math.html#math.sinh>
    fn sinh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_overflow(x, x.sinh())?))
    }

    /// CPython: math.cosh(x) → float.  Hyperbolic cosine.
    /// <https://docs.python.org/3/library/math.html#math.cosh>
    fn cosh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_overflow(x, x.cosh())?))
    }

    /// CPython: math.tanh(x) → float.  Hyperbolic tangent.
    /// <https://docs.python.org/3/library/math.html#math.tanh>
    fn tanh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.tanh())?))
    }

    /// CPython: math.asinh(x) → float.  Inverse hyperbolic sine.
    /// <https://docs.python.org/3/library/math.html#math.asinh>
    fn asinh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.asinh())?))
    }

    /// CPython: math.acosh(x) → float.  Inverse hyperbolic cosine.
    /// Domain: x >= 1; acosh(x<1) → ValueError ("math domain error").
    /// <https://docs.python.org/3/library/math.html#math.acosh>
    fn acosh(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.acosh())?))
    }

    /// CPython: math.atanh(x) → float.  Inverse hyperbolic tangent.
    /// Domain: -1 < x < 1; atanh(±1) → ValueError, |x|>1 → ValueError.
    /// <https://docs.python.org/3/library/math.html#math.atanh>
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
    fn expm1(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_overflow(x, x.exp_m1())?))
    }

    /// CPython: math.log1p(x) → float.  Compute ln(1+x) accurately for small x.
    /// Domain: x > -1; log1p(-1) → ValueError, log1p(x<-1) → ValueError.
    /// <https://docs.python.org/3/library/math.html#math.log1p>
    fn log1p(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.ln_1p())?))
    }

    /// CPython: math.exp2(x) → float.  Compute 2**x.
    /// <https://docs.python.org/3/library/math.html#math.exp2>
    fn exp2(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_overflow(x, x.exp2())?))
    }

    /// CPython: math.cbrt(x) → float.  Cube root (defined for negative x).
    /// Note: results can differ from CPython by one ULP for non-perfect-cube
    /// inputs (e.g. cbrt(27)) because Rust's `f64::cbrt` and the cbrt CPython
    /// 3.12 links round the last bit differently.  Exact cubes, 0, ±inf and NaN
    /// agree.  <https://docs.python.org/3/library/math.html#math.cbrt>
    fn cbrt(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(check_math_result(x, x.cbrt())?))
    }

    /// CPython: math.fmod(x, y) → float.  C library fmod: result has the sign of
    /// x.  fmod(x, 0) and fmod(±inf, y) raise ValueError ("math domain error").
    /// <https://docs.python.org/3/library/math.html#math.fmod>
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
    fn frexp(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        let (m, e) = frexp_f64(x);
        Ok(Value::tuple(vec![Value::float(m), Value::int(e as i64)]))
    }

    /// CPython: math.ldexp(x, i) → float.  Compute x * 2**i.  Overflow raises
    /// OverflowError ("math range error").  `i` must be an integer.
    /// <https://docs.python.org/3/library/math.html#math.ldexp>
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
    fn ulp(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(ulp_f64(x)))
    }

    /// CPython: math.isqrt(n) → int.  Integer square root (floor of the exact
    /// square root).  Works on arbitrary-precision ints.  Negative n raises
    /// ValueError; non-integers raise TypeError.
    /// <https://docs.python.org/3/library/math.html#math.isqrt>
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

    /// CPython: math.fsum(iterable) → float.  Full-precision (exactly rounded)
    /// sum of an iterable of floats, using Shewchuk's algorithm.
    /// <https://docs.python.org/3/library/math.html#math.fsum>
    fn fsum(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            // CPython: "math.fsum() takes exactly one argument (N given)".
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        }
        let items = _interp.collect_iterable(&args[0].value)?;
        let mut values = Vec::with_capacity(items.len());
        for item in &items {
            values.push(math_arg_to_float(_interp, item)?);
        }
        Ok(Value::float(fsum_impl(&values)?))
    }

    /// CPython: math.sumprod(p, q) → number.  Sum of products of two iterables
    /// (Python 3.12+).  Returns an exact int when every element of both
    /// iterables is integral, a compensated-summation float for float/int
    /// elements, and otherwise falls back to the generic `*`/`+` operators.
    /// Raises ValueError if the iterables differ in length.
    /// <https://docs.python.org/3/library/math.html#math.sumprod>
    fn sumprod(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            // CPython (Argument Clinic, bare name): "sumprod expected 2
            // arguments, got N" — distinct from the single_float wording.
            return Err(PyError::named(
                "TypeError",
                format!("sumprod expected 2 arguments, got {}", args.len()),
            ));
        }
        let p_items = _interp.collect_iterable(&args[0].value)?;
        let q_items = _interp.collect_iterable(&args[1].value)?;
        if p_items.len() != q_items.len() {
            return Err(PyError::named(
                "ValueError",
                "Inputs are not the same length".to_string(),
            ));
        }
        // CPython's math_sumprod_impl picks a path per pair (Modules/mathmodule.c):
        //  (1) both exact ints  -> exact integer accumulation (BigInt here),
        //  (2) an exact float / exact-int pair -> triple-double float sum,
        //  (3) anything else (e.g. an object with __mul__) -> the generic
        //      `*` / `+` operators.
        // We classify whole iterables up front: all-int -> (1); all
        // float-path-eligible (exact float or exact int) -> (2); otherwise the
        // generic operator path (3), which — like CPython — raises TypeError for
        // objects that define neither `__mul__` nor coercion.
        let is_int =
            |v: &Value| matches!(v.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_));
        let is_flt_eligible = |v: &Value| {
            matches!(
                v.kind(),
                ValueKind::Float(_) | ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        };
        if p_items.iter().chain(q_items.iter()).all(is_int) {
            // (1) Exact integer path -> exact `int` (sumprod([1,2,3],[4,5,6]) == 32).
            let mut acc = PyBigInt::from(0i64);
            for (pv, qv) in p_items.iter().zip(q_items.iter()) {
                let a = value_to_bigint_int(_interp, FN_NAME, pv)?;
                let b = value_to_bigint_int(_interp, FN_NAME, qv)?;
                acc += a * b;
            }
            return bigint_to_int_value(acc);
        }
        if p_items.iter().chain(q_items.iter()).all(is_flt_eligible) {
            // (2) Float path: triple-double compensated sum of products.
            let mut p = Vec::with_capacity(p_items.len());
            let mut q = Vec::with_capacity(q_items.len());
            for v in &p_items {
                p.push(value_to_float(v, "__SENTINEL__")?);
            }
            for v in &q_items {
                q.push(value_to_float(v, "__SENTINEL__")?);
            }
            return Ok(Value::float(sumprod_floats(&p, &q)));
        }
        // (3) Generic path: accumulate via the `*` and `+` operators, exactly as
        // CPython falls back to PyNumber_Multiply / PyNumber_Add.  Objects that
        // define neither raise TypeError here, matching CPython.
        let mut acc = Value::int(0);
        for (pv, qv) in p_items.iter().zip(q_items.iter()) {
            let prod = _interp.eval_binary(pv.clone(), BinaryOp::Mul, qv.clone())?;
            acc = _interp.eval_binary(acc, BinaryOp::Add, prod)?;
        }
        Ok(acc)
    }

    /// CPython: math.gamma(x) → float.  The Gamma function.  Poles at
    /// non-positive integers raise ValueError ("math domain error").
    /// <https://docs.python.org/3/library/math.html#math.gamma>
    fn gamma(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(m_tgamma(x)?))
    }

    /// CPython: math.lgamma(x) → float.  Natural log of |Gamma(x)|.  Poles at
    /// non-positive integers raise ValueError ("math domain error").
    /// <https://docs.python.org/3/library/math.html#math.lgamma>
    fn lgamma(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(m_lgamma(x)?))
    }

    /// CPython: math.erf(x) → float.  The error function.
    /// <https://docs.python.org/3/library/math.html#math.erf>
    fn erf(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(m_erf(x)))
    }

    /// CPython: math.erfc(x) → float.  The complementary error function.
    /// <https://docs.python.org/3/library/math.html#math.erfc>
    fn erfc(args) -> Result<Value> {
        let x = single_float(_interp, FN_NAME, args)?;
        Ok(Value::float(m_erfc(x)))
    }
}
