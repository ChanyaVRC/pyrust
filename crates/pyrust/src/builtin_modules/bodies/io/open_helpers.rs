//! Shared stream-state, closed-file error, argument parsing, and common method
//! helpers for `io.StringIO` and `io.BytesIO`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::{PyBigInt, PyInstance, PyToPrimitive, Value, ValueKind};

pub(super) fn closed_error() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file".to_string())
}

/// Closed-file error with the trailing period. CPython's `_io` module is
/// internally inconsistent about this: `BytesIO` raises every closed-file
/// error with a trailing `.`, and `StringIO.isatty()` / line iteration do
/// too, while `StringIO.read`/`seek`/`tell`/`seekable`/... omit it.
pub(super) fn closed_error_dot() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file.".to_string())
}

/// `io.UnsupportedOperation` — raised by `fileno()` whether the stream is
/// open or closed (CPython raises it before the closed-state check).
pub(super) fn unsupported_fileno() -> PyError {
    PyError::named("io.UnsupportedOperation", "fileno".to_string())
}

pub(super) fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

pub(super) fn is_closed(inst: &Rc<RefCell<PyInstance>>) -> bool {
    matches!(
        inst.borrow().attrs.get_slot("_closed").map(|v| v.kind()),
        Some(ValueKind::Bool(true))
    )
}

pub(super) fn get_pos(inst: &Rc<RefCell<PyInstance>>) -> i64 {
    match inst.borrow().attrs.get_slot("_pos").map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        _ => 0,
    }
}

pub(super) fn set_pos(inst: &Rc<RefCell<PyInstance>>, pos: i64) {
    inst.borrow_mut().attrs.insert_slot("_pos", Value::int(pos));
}

/// Resolve `self` and reject operations on a closed stream (no trailing period).
pub(super) fn open_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    let inst = expect_self(args, fn_name)?;
    if is_closed(&inst) {
        return Err(closed_error());
    }
    Ok(inst)
}

/// Resolve `self` and reject operations on a closed stream (trailing period).
pub(super) fn open_self_dot(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    let inst = expect_self(args, fn_name)?;
    if is_closed(&inst) {
        return Err(closed_error_dot());
    }
    Ok(inst)
}

pub(super) fn require_no_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<()> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    Ok(())
}

pub(super) fn require_one_arg(args: &[ExpandedCallArg], fn_name: &str) -> Result<()> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    Ok(())
}

/// Slice off `self` and enforce the "takes at most 1 argument" arity used by
/// `read` / `readline` / `readlines` / `truncate`.
pub(super) fn user_at_most_one<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
) -> Result<&'a [ExpandedCallArg]> {
    let user = &args[1..];
    if user.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes at most 1 argument"),
        ));
    }
    Ok(user)
}

/// Parse the optional `size` argument of `read` / `readline`: `None`,
/// negative, or omitted means "no limit"; bool/int give the limit.
pub(super) fn parse_optional_size(
    user: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Option<usize>> {
    match user.first().map(|a| a.value.kind()) {
        None | Some(ValueKind::None) => Ok(None),
        Some(ValueKind::Int(n)) if n < 0 => Ok(None),
        // On a narrower host, an oversized read limit is equivalent to an
        // unbounded limit because no backing buffer can reach it.
        Some(ValueKind::Int(n)) => Ok(Some(usize::try_from(n).unwrap_or(usize::MAX))),
        Some(ValueKind::BigInt(n)) if n < &PyBigInt::from(0) => Ok(None),
        Some(ValueKind::BigInt(n)) => Ok(Some(n.to_usize().unwrap_or(usize::MAX))),
        Some(ValueKind::Bool(b)) => Ok(Some(b as usize)),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() size must be an integer"),
        )),
    }
}

pub(super) fn parse_seek_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<(i64, i64)> {
    let user = &args[1..];
    if user.is_empty() || user.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes 1 or 2 arguments"),
        ));
    }
    let offset = parse_io_offset(&user[0].value, fn_name, "offset")?;
    let whence = match user.get(1) {
        None => 0i64,
        Some(arg) => parse_io_whence(&arg.value, fn_name)?,
    };
    Ok((offset, whence))
}

fn parse_io_offset(value: &Value, fn_name: &str, arg_name: &str) -> Result<i64> {
    match value.kind() {
        ValueKind::Int(value) => Ok(value),
        ValueKind::Bool(value) => Ok(value as i64),
        ValueKind::BigInt(value) => value.to_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() {arg_name} must be an integer"),
        )),
    }
}

fn parse_io_whence(value: &Value, fn_name: &str) -> Result<i64> {
    let value = match value.kind() {
        ValueKind::Int(value) => i32::try_from(value).ok(),
        ValueKind::Bool(value) => Some(value as i32),
        ValueKind::BigInt(value) => value.to_i32(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() whence must be an integer"),
            ));
        }
    };
    value.map(i64::from).ok_or_else(|| {
        PyError::named(
            "OverflowError",
            "Python int too large to convert to C int".to_string(),
        )
    })
}

pub(super) fn parse_truncate_arg(
    inst: &Rc<RefCell<PyInstance>>,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<i64> {
    let user = user_at_most_one(args, fn_name)?;
    match user.first().map(|a| a.value.kind()) {
        None | Some(ValueKind::None) => Ok(get_pos(inst)),
        Some(_) => {
            let size = parse_truncate_size(&user[0].value, fn_name)?;
            if size < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("{fn_name}(): negative size"),
                ));
            }
            Ok(size)
        }
    }
}

fn parse_truncate_size(value: &Value, fn_name: &str) -> Result<i64> {
    match value.kind() {
        ValueKind::Int(value) => Ok(value),
        ValueKind::Bool(value) => Ok(value as i64),
        ValueKind::BigInt(value) => value.to_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}(): size must be an integer"),
        )),
    }
}

// ── Shared stream lifecycle / query methods ───────────────────────────────────

pub(super) fn stream_tell(args: &[ExpandedCallArg], fn_name: &str, dot: bool) -> Result<Value> {
    let inst = if dot {
        open_self_dot(args, fn_name)?
    } else {
        open_self(args, fn_name)?
    };
    require_no_args(args, fn_name)?;
    Ok(Value::int(get_pos(&inst)))
}

pub(super) fn stream_close(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    require_no_args(args, fn_name)?;
    inst.borrow_mut()
        .attrs
        .insert_slot("_closed", Value::bool_(true));
    Ok(Value::none())
}

pub(super) fn stream_enter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    Ok(Value::py_instance(inst))
}

pub(super) fn stream_exit(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    inst.borrow_mut()
        .attrs
        .insert_slot("_closed", Value::bool_(true));
    Ok(Value::bool_(false))
}

pub(super) fn stream_bool_query(
    args: &[ExpandedCallArg],
    fn_name: &str,
    dot: bool,
    value: bool,
) -> Result<Value> {
    let _ = if dot {
        open_self_dot(args, fn_name)?
    } else {
        open_self(args, fn_name)?
    };
    Ok(Value::bool_(value))
}

pub(super) fn stream_fileno(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let _ = expect_self(args, fn_name)?;
    Err(unsupported_fileno())
}

pub(super) fn stream_closed(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    Ok(Value::bool_(is_closed(&inst)))
}

pub(super) fn stream_iter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    Ok(Value::py_instance(inst))
}

pub(super) fn stream_repr(
    args: &[ExpandedCallArg],
    fn_name: &str,
    type_label: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if is_closed(&inst) {
        Ok(Value::string(format!("<{type_label} [closed]>")))
    } else {
        Ok(Value::string(format!("<{type_label} object>")))
    }
}
