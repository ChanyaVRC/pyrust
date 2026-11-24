//! Special-function entry points (`gamma`, `lgamma`, `erf`, `erfc`) for `math`.

use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

use super::coercion::single_float;
use super::special_functions::{m_erf, m_erfc, m_lgamma, m_tgamma};

pub(super) fn gamma(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    Ok(Value::float(m_tgamma(x)?))
}

pub(super) fn lgamma(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    Ok(Value::float(m_lgamma(x)?))
}

pub(super) fn erf(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    Ok(Value::float(m_erf(x)))
}

pub(super) fn erfc(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let x = single_float(interp, fn_name, args)?;
    Ok(Value::float(m_erfc(x)))
}
