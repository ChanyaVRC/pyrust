//! `io.BytesIO` method entry points (argument validation + delegation to
//! `bytes_buffer`).

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

use super::bytes_buffer;
use super::open_helpers::{
    expect_self, open_self_dot, parse_optional_size, parse_seek_args, parse_truncate_arg,
    require_no_args, require_one_arg, user_at_most_one,
};

pub(super) fn init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    if user.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at most 1 argument ({} given)",
                user.len()
            ),
        ));
    }
    let initial: Vec<u8> = match user.first() {
        Some(a) => match a.value.kind() {
            ValueKind::Bytes(b) => b.to_vec(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{fn_name}() initial_bytes must be a bytes-like object, not {}",
                        pyrust_core::builtin_type_name(&a.value)
                    ),
                ));
            }
        },
        None => Vec::new(),
    };
    bytes_buffer::initialize(&inst, initial);
    Ok(Value::none())
}

pub(super) fn read(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    let user = user_at_most_one(args, fn_name)?;
    let size = parse_optional_size(user, fn_name)?;
    Ok(Value::bytes(bytes_buffer::read(&inst, size, fn_name)?))
}

pub(super) fn readline(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    let user = user_at_most_one(args, fn_name)?;
    let size_limit = parse_optional_size(user, fn_name)?;
    Ok(Value::bytes(bytes_buffer::read_line(
        &inst, size_limit, fn_name,
    )?))
}

pub(super) fn readlines(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    user_at_most_one(args, fn_name)?;
    let lines = bytes_buffer::read_lines(&inst, fn_name)?
        .into_iter()
        .map(Value::bytes)
        .collect();
    Ok(Value::list(lines))
}

pub(super) fn write(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    require_one_arg(args, fn_name)?;
    let data: Vec<u8> = match args[1].value.kind() {
        ValueKind::Bytes(b) => b.to_vec(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() argument must be bytes-like object"),
            ));
        }
    };
    let n_written = bytes_buffer::write(&inst, &data)?;
    Ok(Value::int(n_written as i64))
}

pub(super) fn getvalue(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    require_no_args(args, fn_name)?;
    Ok(Value::bytes(bytes_buffer::contents(&inst, fn_name)?))
}

pub(super) fn seek(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    let (offset, whence) = parse_seek_args(args, fn_name)?;
    Ok(Value::int(bytes_buffer::seek(
        &inst, offset, whence, fn_name,
    )?))
}

pub(super) fn truncate(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    let size = parse_truncate_arg(&inst, args, fn_name)?;
    let new_len = bytes_buffer::truncate(&inst, size, fn_name)?;
    Ok(Value::int(new_len))
}

pub(super) fn writelines(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    require_one_arg(args, fn_name)?;
    // CPython delegates each element to `write` as it iterates, so a non-bytes
    // element mid-iterable leaves the written prefix in the buffer.
    let items = interp.collect_iterable(&args[1].value)?;
    for item in &items {
        match item.kind() {
            ValueKind::Bytes(b) => {
                bytes_buffer::write(&inst, b)?;
            }
            _ => match pyrust_builtins::bytearray::as_bytearray_snapshot(item) {
                Some(b) => {
                    bytes_buffer::write(&inst, &b)?;
                }
                None => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "a bytes-like object is required, not '{}'",
                            pyrust_core::builtin_type_name(item)
                        ),
                    ));
                }
            },
        }
    }
    Ok(Value::none())
}

pub(super) fn readinto(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    require_one_arg(args, fn_name)?;
    let target = match pyrust_builtins::bytearray::as_bytearray_rc(&args[1].value) {
        Some(rc) => rc,
        None => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "readinto() argument must be read-write bytes-like object, not {}",
                    pyrust_core::builtin_type_name(&args[1].value)
                ),
            ));
        }
    };
    let mut dst = target.borrow_mut();
    let n = bytes_buffer::read_into(&inst, &mut dst, fn_name)?;
    Ok(Value::int(n as i64))
}

/// Unlike `StringIO`, CPython's `BytesIO.flush()` DOES raise on a closed stream.
pub(super) fn flush(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let _ = open_self_dot(args, fn_name)?;
    Ok(Value::none())
}

pub(super) fn next(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    match bytes_buffer::next_line(&inst, fn_name)? {
        Some(line) => Ok(Value::bytes(line)),
        None => Err(PyError::named("StopIteration", String::new())),
    }
}
