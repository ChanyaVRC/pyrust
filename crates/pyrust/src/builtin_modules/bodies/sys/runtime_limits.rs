// Runtime recursion, integer digit, switch-interval, and coroutine-tracking
// limits for the `sys` module, plus shared arity/keyword argument validators.

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, get_call_depth,
    get_int_max_str_digits as interpreter_int_max_str_digits, get_recursion_limit,
    reject_keyword_args_expanded, set_int_max_str_digits as set_interpreter_int_max_str_digits,
    set_recursion_limit, value_type_name_str,
};
use crate::value::{Value, ValueKind};

pub(super) fn require_no_args(
    args: &[ExpandedCallArg],
    fn_name: &str,
    display_name: &str,
) -> Result<()> {
    reject_keyword_args_expanded(fn_name, args)?;
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{display_name}() takes no arguments ({} given)", args.len()),
        ));
    }
    Ok(())
}

pub(super) fn require_exact_one_arg<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
    display_name: &str,
) -> Result<&'a Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{display_name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    Ok(&args[0].value)
}

pub(super) fn require_single_named_arg<'a>(
    args: &'a [ExpandedCallArg],
    display_name: &str,
    param_name: &str,
) -> Result<&'a Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{display_name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    if let Some(kw) = args[0].name.as_deref()
        && kw != param_name
    {
        return Err(PyError::named(
            "TypeError",
            format!("'{kw}' is an invalid keyword argument for {display_name}()"),
        ));
    }
    Ok(&args[0].value)
}

pub(super) fn getrecursionlimit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "sys.getrecursionlimit")?;
    Ok(Value::int(get_recursion_limit(interp) as i64))
}

pub(super) fn setrecursionlimit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let limit_val = require_exact_one_arg(args, fn_name, "sys.setrecursionlimit")?;
    // The limit honors the `__index__` protocol (#2022); a non-int raises
    // the canonical TypeError and an overflowing bigint raises
    // `OverflowError: Python int too large to convert to C int`.
    let n = interp.value_to_isize(limit_val, "Python int too large to convert to C int")?;
    if n < 1 {
        return Err(PyError::named(
            "ValueError",
            "recursion limit must be greater or equal than 1".to_string(),
        ));
    }
    let new_limit = n as usize;
    let current_depth = get_call_depth();
    if new_limit <= current_depth {
        return Err(PyError::named(
            "RecursionError",
            format!(
                "cannot set the recursion limit to {new_limit} at the recursion depth {current_depth}: the limit is too low"
            ),
        ));
    }
    set_recursion_limit(interp, new_limit);
    Ok(Value::none())
}

pub(super) fn get_int_max_str_digits(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "sys.get_int_max_str_digits")?;
    Ok(Value::int(interpreter_int_max_str_digits(interp) as i64))
}

pub(super) fn set_int_max_str_digits(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let digits_val = require_exact_one_arg(args, fn_name, "set_int_max_str_digits")?;
    let n = interp.value_to_isize(digits_val, "Python int too large to convert to C int")?;
    if n != 0 && n < pyrust_core::INT_MAX_STR_DIGITS_MIN as i64 {
        return Err(PyError::named(
            "ValueError",
            format!(
                "maxdigits must be 0 or larger than {}",
                pyrust_core::INT_MAX_STR_DIGITS_MIN
            ),
        ));
    }
    set_interpreter_int_max_str_digits(interp, n as usize);
    Ok(Value::none())
}

pub(super) fn getswitchinterval(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "getswitchinterval")?;
    Ok(Value::float(
        interp.features().sys_with(|state| state.switch_interval),
    ))
}

pub(super) fn setswitchinterval(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let interval_val = require_exact_one_arg(args, fn_name, "setswitchinterval")?;
    let interval = match interval_val.kind() {
        ValueKind::Float(f) => f,
        ValueKind::Int(i) => i as f64,
        ValueKind::Bool(b) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        ValueKind::BigInt(b) => {
            use num_traits::ToPrimitive;
            b.to_f64().unwrap_or(f64::INFINITY)
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "must be real number, not {}",
                    value_type_name_str(interval_val)
                ),
            ));
        }
    };
    if interval <= 0.0 || interval.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "switch interval must be strictly positive".to_string(),
        ));
    }
    interp.features().sys_mut().switch_interval = interval;
    Ok(Value::none())
}

pub(super) fn get_coroutine_origin_tracking_depth(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "get_coroutine_origin_tracking_depth")?;
    Ok(Value::int(
        interp
            .features()
            .sys_with(|state| state.coroutine_origin_tracking_depth) as i64,
    ))
}

pub(super) fn set_coroutine_origin_tracking_depth(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let depth_val = require_single_named_arg(args, "set_coroutine_origin_tracking_depth", "depth")?;
    let depth = interp.value_to_isize(depth_val, "Python int too large to convert to C int")?;
    if depth < 0 {
        return Err(PyError::named(
            "ValueError",
            "depth must be >= 0".to_string(),
        ));
    }
    interp.features().sys_mut().coroutine_origin_tracking_depth = depth as usize;
    Ok(Value::none())
}
