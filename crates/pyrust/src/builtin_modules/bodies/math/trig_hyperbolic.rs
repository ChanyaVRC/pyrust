//! Trigonometric, angular-conversion, and hyperbolic entry points for `math`.

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

use super::coercion::{single_float, two_floats};
use super::float_algorithms::check_math_result;

pub(super) fn atan2(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (y, x) = two_floats(interp, fn_name, args)?;
    Ok(Value::float(y.atan2(x)))
}

pub(super) fn atanh(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    // atanh(+-1) -> +-inf in C, but CPython reports it as a *domain* error
    // (EDOM), not a range error. Handle it before check_math_result.
    if x.abs() == 1.0 {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    Ok(Value::float(check_math_result(x, x.atanh())?))
}
