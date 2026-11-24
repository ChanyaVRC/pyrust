//! Python numeric-protocol coercion used by the `cmath` module.

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, builtin_data_backing, coerce_subclass_backing, invoke_class_method,
    lookup_value_special_method, normalize_complex_slot_result, normalize_float_slot_result,
    reject_keyword_args_expanded, value_to_float, value_type_name_str,
};
use crate::value::{Value, ValueKind};

/// Coerce a Python argument to `f64`, matching CPython's `PyFloat_AsDouble`.
pub(super) fn cmath_arg_to_float(interp: &mut crate::Interpreter, val: &Value) -> Result<f64> {
    if matches!(val.kind(), ValueKind::PyInstance(_) | ValueKind::PyClass(_)) {
        let backing = builtin_data_backing(val);
        if let Some(ref b) = backing
            && matches!(b.kind(), ValueKind::Float(_))
        {
            return value_to_float(b, "__SENTINEL__");
        }
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
        if let Some(ref b) = backing
            && matches!(
                b.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return value_to_float(b, "__SENTINEL__");
        }
        if let Some(result) = interp.try_value_to_index(val)? {
            return value_to_float(&result, "__SENTINEL__");
        }
        return Err(PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(val)),
        ));
    }
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

/// Coerce a Python argument to `(f64, f64)`, matching CPython's `PyComplex_AsCComplex`.
///
/// Order:
/// 1. Plain `complex` -> `(re, im)`.
/// 2. Complex subclass without user `__complex__` override -> backing `(re, im)`.
/// 3. User `__complex__` method -> normalized complex `(re, im)`.
/// 4. Fall back to `cmath_arg_to_float` -> `(f, 0.0)`.
pub(super) fn cmath_arg_to_complex(
    interp: &mut crate::Interpreter,
    val: &Value,
) -> Result<(f64, f64)> {
    match val.kind() {
        ValueKind::Complex(re, im) => Ok((re, im)),
        ValueKind::Int(_) | ValueKind::Float(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
            Ok((value_to_float(val, "__SENTINEL__")?, 0.0))
        }
        ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
            let complex_backing = coerce_subclass_backing(val, &["__complex__"]);
            if let Some(backing) = &complex_backing
                && let ValueKind::Complex(re, im) = backing.kind()
            {
                return Ok((re, im));
            }
            if complex_backing.is_none()
                && let Some(method) = lookup_value_special_method(val, "__complex__").transpose()?
            {
                let result = invoke_class_method(interp, method, val.clone(), &[])?;
                let normalized = normalize_complex_slot_result(&result).ok_or_else(|| {
                    PyError::named(
                        "TypeError",
                        format!(
                            "__complex__ returned non-complex (type {})",
                            value_type_name_str(&result),
                        ),
                    )
                })?;
                let ValueKind::Complex(re, im) = normalized.kind() else {
                    unreachable!("normalize_complex_slot_result guarantees complex")
                };
                return Ok((re, im));
            }
            Ok((cmath_arg_to_float(interp, val)?, 0.0))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(val)),
        )),
    }
}

/// Reject kwargs and require exactly one positional complex-coercible argument.
pub(super) fn single_complex(
    interp: &mut crate::Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<(f64, f64)> {
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
    cmath_arg_to_complex(interp, &args[0].value)
}

/// Reject kwargs and require exactly two positional float-coercible arguments.
pub(super) fn two_floats(
    interp: &mut crate::Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<(f64, f64)> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let r = cmath_arg_to_float(interp, &args[0].value)?;
    let phi = cmath_arg_to_float(interp, &args[1].value)?;
    Ok((r, phi))
}
