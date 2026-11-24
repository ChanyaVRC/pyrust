//! `io.BytesIO` method entry points (argument validation + delegation to
//! `bytes_buffer`).

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

use super::bytes_buffer;
use super::open_helpers::{
    expect_self, open_self_dot, parse_optional_size, parse_seek_args, parse_truncate_arg,
    require_no_args, require_one_arg, user_at_most_one,
};

pub(super) fn init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
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
        Some(a) => match crate::interpreter::try_extract_buffer_bytes(interp, &a.value)? {
            Some(bytes) => bytes,
            None => {
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

pub(super) fn write(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    require_one_arg(args, fn_name)?;
    let data: Vec<u8> = match crate::interpreter::try_extract_buffer_bytes(interp, &args[1].value)?
    {
        Some(bytes) => bytes,
        None => {
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

pub(super) fn getbuffer(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    require_no_args(args, fn_name)?;
    let (data, exports) = bytes_buffer::buffer_handle(&inst, fn_name)?;
    pyrust_builtins::memoryview::from_mutable_buffer(
        data,
        Some(exports),
        Value::py_instance(inst),
        "B",
        false,
    )
}

pub(super) fn close(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    require_no_args(args, fn_name)?;
    bytes_buffer::check_no_exports(&inst)?;
    inst.borrow_mut()
        .attrs
        .insert_slot("_closed", Value::bool_(true));
    Ok(Value::none())
}

pub(super) fn exit(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    bytes_buffer::check_no_exports(&inst)?;
    inst.borrow_mut()
        .attrs
        .insert_slot("_closed", Value::bool_(true));
    Ok(Value::bool_(false))
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
        let bytes = crate::interpreter::extract_buffer_bytes(interp, item)?;
        bytes_buffer::write(&inst, &bytes)?;
    }
    Ok(Value::none())
}

pub(super) fn readinto(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    require_one_arg(args, fn_name)?;
    let target =
        crate::interpreter::acquire_writable_buffer(interp, &args[1].value).map_err(|e| {
            if e.class_name_is("TypeError")
                && e.to_string().contains("read-write bytes-like object")
            {
                PyError::named(
                    "TypeError",
                    format!(
                        "readinto() argument must be read-write bytes-like object, not {}",
                        pyrust_core::builtin_type_name(&args[1].value)
                    ),
                )
            } else {
                e
            }
        })?;
    let read_res = {
        let mut dst = target.data.borrow_mut();
        bytes_buffer::read_into(
            &inst,
            &mut dst[target.offset..target.offset + target.len],
            fn_name,
        )
    };
    let rel_res = target.release(interp);
    let n = read_res?;
    rel_res?;
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
