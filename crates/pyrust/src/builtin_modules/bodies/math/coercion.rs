//! Python numeric-protocol coercion used by the math API.

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, builtin_data_backing, invoke_class_method, lookup_value_special_method,
    normalize_float_slot_result, reject_keyword_args_expanded, value_to_float, value_type_name_str,
};
use crate::value::{PyToPrimitive, Value, ValueKind};

/// Check for a unary dunder method (`__floor__`, `__ceil__`, `__trunc__`) on
/// a `PyInstance` value, calling it if present, or returning `None` so the
/// caller can fall back to the numeric conversion path.
///
/// This mirrors CPython's `Modules/mathmodule.c` protocol for those three
/// functions: dunder dispatch is attempted first; if the method is absent the
/// float-coercion path is used instead.
pub(super) fn try_math_dunder(
    interp: &mut crate::Interpreter,
    val: &Value,
    method: &str,
) -> Option<Result<Value>> {
    interp.try_dunder_unary(val, method)
}

/// Interpreter-aware coercion of a math float-argument to `f64`, matching
/// CPython's `PyFloat_AsDouble` / `Modules/mathmodule.c` argument handling.
///
/// Concrete `int` / `float` / `bool` / `BigInt` go through `value_to_float`
/// directly (the common case — no `_interp` work). A Python instance, or a
/// class object with numeric metaclass slots, is coerced like CPython's
/// `PyFloat_AsDouble`:
///   1. a `float` subclass uses its primitive backing value directly — the
///      `PyFloat_Check` fast path, which *bypasses* any user `__float__`
///      override (CPython quirk: `math.sqrt(MyFloat(16))` ignores a
///      `__float__` that returns something else),
///   2. otherwise a user `__float__` is dispatched (a float subclass is
///      normalized to plain `float`; CPython rejects `int`/`bool` returns),
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
pub(super) fn math_arg_to_float(interp: &mut crate::Interpreter, val: &Value) -> Result<f64> {
    if matches!(val.kind(), ValueKind::PyInstance(_) | ValueKind::PyClass(_)) {
        let backing = builtin_data_backing(val);
        // (1) float subclass: use the backing float directly (PyFloat_Check
        // fast path bypasses any __float__ override).
        if let Some(ref b) = backing
            && matches!(b.kind(), ValueKind::Float(_))
        {
            return value_to_float(b, "__SENTINEL__");
        }
        // (2) __float__ — must return an exact float (not int/bool).
        if let Some(method) = lookup_value_special_method(val, "__float__").transpose()? {
            let result = invoke_class_method(interp, method, val.clone(), &[])?;
            return if let Some(normalized) = normalize_float_slot_result(&result) {
                let ValueKind::Float(value) = normalized.kind() else {
                    unreachable!("normalize_float_slot_result guarantees float")
                };
                Ok(value)
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "{}.__float__ returned non-float (type {})",
                        value_type_name_str(val),
                        value_type_name_str(&result),
                    ),
                ))
            };
        }
        // (3) int subclass: the inherited int.__float__ (nb_float) uses the
        // backing value and ranks above a user __index__ override.
        if let Some(ref b) = backing
            && matches!(
                b.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return value_to_float(b, "__SENTINEL__");
        }
        // (4) __index__ — math accepts it for float arguments on plain objects.
        if let Some(result) = interp.try_value_to_index(val)? {
            return value_to_float(&result, "__SENTINEL__");
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
pub(super) fn math_integral_exact(val: &Value) -> Option<Value> {
    match val.kind() {
        ValueKind::Int(n) => Some(Value::int(n)),
        ValueKind::BigInt(b) => Some(Value::bigint(b.clone())),
        ValueKind::Bool(b) => Some(Value::int(b as i64)),
        ValueKind::PyInstance(_) => match builtin_data_backing(val).as_ref().map(|b| b.kind()) {
            Some(ValueKind::Int(n)) => Some(Value::int(n)),
            Some(ValueKind::BigInt(b)) => Some(Value::bigint(b.clone())),
            Some(ValueKind::Bool(b)) => Some(Value::int(b as i64)),
            _ => None,
        },
        _ => None,
    }
}

/// Reject kwargs and demand exactly one positional float-coercible arg.
pub(super) fn single_float(
    interp: &mut crate::Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<f64> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        // CPython: "math.<fn>() takes exactly one argument (N given)".
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    math_arg_to_float(interp, &args[0].value)
}

/// Reject kwargs and demand exactly two positional float-coercible args.
pub(super) fn two_floats(
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
pub(super) fn value_to_exp_int(_fn_name: &str, val: &Value) -> Result<i32> {
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
pub(super) fn value_to_steps_int(
    interp: &mut crate::Interpreter,
    _fn_name: &str,
    val: &Value,
) -> Result<i64> {
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
