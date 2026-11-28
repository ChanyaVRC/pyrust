//! Interpreter plumbing for the native `random` module: calls, operators,
//! conversions and the exception helpers the CPython source gets for free.

use num_bigint::BigInt;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

pub(super) fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

pub(super) fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

pub(super) fn index_error(msg: impl Into<String>) -> PyError {
    PyError::named("IndexError", msg.into())
}

fn positional(pos: &[Value]) -> Vec<ExpandedCallArg> {
    pos.iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect()
}

/// `callee(*pos)`.
pub(super) fn call(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    interp.call_function_expanded(callee, &positional(pos))
}

/// `callee(*pos, **kw)`.
pub(super) fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut args = positional(pos);
    args.extend(kw.iter().map(|(name, value)| ExpandedCallArg {
        name: Some((*name).to_string()),
        value: value.clone(),
    }));
    interp.call_function_expanded(callee, &args)
}

/// `target.name(*pos)`.
pub(super) fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call(interp, callee, pos)
}

/// `builtins.<name>(*pos)`.
pub(super) fn builtin(
    interp: &mut Interpreter,
    name: &'static str,
    pos: &[Value],
) -> Result<Value> {
    call(interp, Value::builtin_function(name), pos)
}

/// `left <op> right` through the interpreter's operator protocol.
pub(super) fn binop(
    interp: &mut Interpreter,
    left: &Value,
    op: BinaryOp,
    right: &Value,
) -> Result<Value> {
    interp.eval_binary(left.clone(), op, right.clone())
}

pub(super) fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

/// `left <op> right` as a bool, with an exact native path for two real
/// numbers that convert to `f64` without rounding.
pub(super) fn compare(
    interp: &mut Interpreter,
    left: &Value,
    op: BinaryOp,
    right: &Value,
) -> Result<bool> {
    if let (Some(a), Some(b)) = (exact_int(left), exact_int(right)) {
        return Ok(match op {
            BinaryOp::Lt => a < b,
            BinaryOp::Le => a <= b,
            BinaryOp::Gt => a > b,
            BinaryOp::Ge => a >= b,
            BinaryOp::Eq => a == b,
            _ => a != b,
        });
    }
    if let (Some(a), Some(b)) = (float_operand(left), float_operand(right)) {
        return Ok(match op {
            BinaryOp::Lt => a < b,
            BinaryOp::Le => a <= b,
            BinaryOp::Gt => a > b,
            BinaryOp::Ge => a >= b,
            BinaryOp::Eq => a == b,
            _ => a != b,
        });
    }
    let result = binop(interp, left, op, right)?;
    truthy(interp, &result)
}

/// Largest magnitude of an `int` operand the float fast paths accept: sums and
/// differences of two such values are still exact in `f64`.
const EXACT_INT_LIMIT: i64 = 1 << 52;

/// An exact `float`, or an exact `int` / `bool` that converts to `f64`
/// without rounding and whose sums/differences stay exact: the operands for
/// which native `f64` arithmetic gives the same result as Python's.
pub(super) fn float_operand(value: &Value) -> Option<f64> {
    match value.kind() {
        ValueKind::Float(f) => Some(f),
        ValueKind::Int(n) if (-EXACT_INT_LIMIT..=EXACT_INT_LIMIT).contains(&n) => Some(n as f64),
        ValueKind::Bool(b) => Some(f64::from(u8::from(b))),
        _ => None,
    }
}

/// An exact `int` / `bool` that fits `i64`.
pub(super) fn exact_int(value: &Value) -> Option<i64> {
    match value.kind() {
        ValueKind::Int(n) => Some(n),
        ValueKind::Bool(b) => Some(i64::from(b)),
        _ => None,
    }
}

/// The value of an `int` (exact, `bool`, or an `int` subclass instance).
pub(super) fn int_value_of(value: &Value) -> Option<BigInt> {
    match value.kind() {
        ValueKind::Int(n) => Some(BigInt::from(n)),
        ValueKind::Bool(b) => Some(BigInt::from(u8::from(b))),
        ValueKind::BigInt(n) => Some(n.clone()),
        ValueKind::PyInstance(_) => {
            crate::interpreter::builtin_data_backing(value).and_then(|b| match b.kind() {
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => int_value_of(&b),
                _ => None,
            })
        }
        _ => None,
    }
}

/// `isinstance(value, int)`.
pub(super) fn is_int(value: &Value) -> bool {
    int_value_of(value).is_some()
}

pub(super) fn bigint_value(n: BigInt) -> Value {
    pyrust_core::value_from_bigint(n)
}

/// `operator.index(value)` as a `BigInt`.
pub(super) fn index(interp: &mut Interpreter, value: &Value) -> Result<BigInt> {
    if let Some(n) = exact_int(value) {
        return Ok(BigInt::from(n));
    }
    let index = interp.value_to_index(value, |v| {
        type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            pyrust_core::builtin_type_name(v)
        ))
    })?;
    Ok(int_value_of(&index).unwrap_or_default())
}

/// `len(value)`.
pub(super) fn length(interp: &mut Interpreter, value: &Value) -> Result<usize> {
    match value.kind() {
        ValueKind::List(items) => return Ok(items.len()),
        ValueKind::Tuple(items) => return Ok(items.len()),
        _ => {}
    }
    let n = builtin(interp, "len", std::slice::from_ref(value))?;
    Ok(exact_int(&n).map_or(0, |n| n.max(0) as usize))
}

/// `obj[key]`.
pub(super) fn getitem(interp: &mut Interpreter, obj: &Value, key: Value) -> Result<Value> {
    crate::builtin_modules::operator::getitem_value(interp, obj.clone(), key)
}

/// `obj[i]` for a non-negative index, without the interpreter for an exact
/// `list` / `tuple`.
pub(super) fn getitem_at(interp: &mut Interpreter, obj: &Value, i: usize) -> Result<Value> {
    match obj.kind() {
        ValueKind::List(items) if i < items.len() => return Ok(items[i].clone()),
        ValueKind::Tuple(items) if i < items.len() => return Ok(items[i].clone()),
        _ => {}
    }
    getitem(interp, obj, Value::int(i as i64))
}

/// `obj[key] = value`.
pub(super) fn setitem(
    interp: &mut Interpreter,
    obj: &Value,
    key: Value,
    value: Value,
) -> Result<()> {
    crate::builtin_modules::operator::setitem_value(interp, obj.clone(), key, value)?;
    Ok(())
}

/// `f"{value}"`.
pub(super) fn format_value(interp: &mut Interpreter, value: &Value) -> Result<String> {
    if let Some(n) = int_value_of(value)
        .filter(|_| matches!(value.kind(), ValueKind::Int(_) | ValueKind::BigInt(_)))
    {
        return Ok(n.to_string());
    }
    let text = builtin(interp, "format", std::slice::from_ref(value))?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

/// `str(value)`.
pub(super) fn str_value(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let text = builtin(interp, "str", std::slice::from_ref(value))?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

/// `repr(value)`.
pub(super) fn repr_value(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let text = builtin(interp, "repr", std::slice::from_ref(value))?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

/// A bare `raise <name>` (no arguments).
pub(super) fn bare_exception(interp: &mut Interpreter, name: &'static str) -> PyError {
    let Some(class) = crate::interpreter::lookup_exc_class(name) else {
        return PyError::named(name, String::new());
    };
    match call(interp, Value::py_class(class), &[]) {
        Ok(instance) => PyError::raised(instance),
        Err(err) => err,
    }
}

/// `warnings.warn(message)`.
pub(super) fn warn(interp: &mut Interpreter, message: &str) -> Result<()> {
    let warnings = interp.load_module("warnings")?;
    let warn = interp.get_attr(&warnings, "warn")?;
    call(interp, warn, &[Value::string(message)])?;
    Ok(())
}

/// Run `f`.  An error accepted by `swallow` is handed back as `Ok(Err(err))`
/// after the traceback frames recorded while it unwound are dropped, so the
/// swallowed exception (CPython's `except` clause) does not leak frames into a
/// later, unrelated exception.  Any other error propagates unchanged.
pub(super) fn try_swallow<T>(
    f: impl FnOnce() -> Result<T>,
    swallow: impl FnOnce(&PyError) -> bool,
) -> Result<std::result::Result<T, PyError>> {
    let saved = pyrust_core::clone_captured_error_frames();
    match f() {
        Ok(value) => Ok(Ok(value)),
        Err(err) if swallow(&err) => {
            pyrust_core::reset_captured_error_frames();
            for frame in saved.into_iter().rev() {
                pyrust_core::record_traceback_frame(frame);
            }
            Ok(Err(err))
        }
        Err(err) => Err(err),
    }
}
