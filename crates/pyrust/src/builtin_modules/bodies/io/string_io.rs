//! `io.StringIO` method entry points (argument validation + delegation to
//! `string_buffer`).

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

use super::open_helpers::{
    expect_self, open_self, open_self_dot, parse_optional_size, parse_seek_args,
    parse_truncate_arg, require_no_args, require_one_arg, user_at_most_one,
};
use super::string_buffer;

pub(super) fn init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    // Accept at most 2 user args: initial_value and newline.
    if user.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at most 2 arguments ({} given)",
                user.len()
            ),
        ));
    }
    let initial = match user.first() {
        Some(a) => match a.value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{fn_name}() initial_value must be str, not {}",
                        pyrust_core::builtin_type_name(&a.value)
                    ),
                ));
            }
        },
        None => String::new(),
    };
    // newline must be None or str (value is accepted-and-ignored).
    if let Some(nl) = user.get(1) {
        match nl.value.kind() {
            ValueKind::None | ValueKind::Str(_) => {}
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{fn_name}() newline must be str or None, not {}",
                        pyrust_core::builtin_type_name(&nl.value)
                    ),
                ));
            }
        }
    }
    string_buffer::initialize(&inst, &initial);
    Ok(Value::none())
}

pub(super) fn read(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self(args, fn_name)?;
    let user = user_at_most_one(args, fn_name)?;
    let size = parse_optional_size(user, fn_name)?;
    Ok(Value::string(string_buffer::read(&inst, size, fn_name)?))
}

pub(super) fn readline(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self(args, fn_name)?;
    let user = user_at_most_one(args, fn_name)?;
    let size_limit = parse_optional_size(user, fn_name)?;
    Ok(Value::string(string_buffer::read_line(
        &inst, size_limit, fn_name,
    )?))
}

/// CPython's StringIO.readlines closed-file error carries the trailing period.
pub(super) fn readlines(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    user_at_most_one(args, fn_name)?;
    let lines = string_buffer::read_lines(&inst, fn_name)?
        .into_iter()
        .map(Value::string)
        .collect();
    Ok(Value::list(lines))
}

pub(super) fn write(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self(args, fn_name)?;
    require_one_arg(args, fn_name)?;
    let s = match args[1].value.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() argument must be str"),
            ));
        }
    };
    let n_written = string_buffer::write(&inst, &s)?;
    Ok(Value::int(n_written as i64))
}

pub(super) fn getvalue(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self(args, fn_name)?;
    require_no_args(args, fn_name)?;
    Ok(Value::string(string_buffer::contents(&inst, fn_name)?))
}

pub(super) fn seek(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self(args, fn_name)?;
    let (offset, whence) = parse_seek_args(args, fn_name)?;
    Ok(Value::int(string_buffer::seek(
        &inst, offset, whence, fn_name,
    )?))
}

pub(super) fn truncate(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self(args, fn_name)?;
    let size = parse_truncate_arg(&inst, args, fn_name)?;
    let new_len = string_buffer::truncate(&inst, size, fn_name)?;
    Ok(Value::int(new_len))
}

pub(super) fn writelines(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = open_self_dot(args, fn_name)?;
    require_one_arg(args, fn_name)?;
    // CPython's writelines delegates each element to `write` as it iterates, so
    // a non-str element mid-iterable leaves the already written prefix in the
    // buffer (it is NOT atomic).
    let items = interp.collect_iterable(&args[1].value)?;
    for item in &items {
        match item.kind() {
            ValueKind::Str(s) => {
                string_buffer::write(&inst, s)?;
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "string argument expected, got '{}'",
                        pyrust_core::builtin_type_name(item)
                    ),
                ));
            }
        }
    }
    Ok(Value::none())
}

/// CPython's `StringIO.flush()` does NOT raise on a closed stream.
pub(super) fn flush(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let _ = expect_self(args, fn_name)?;
    Ok(Value::none())
}

/// CPython's `StringIO.__next__` closed-file error omits the trailing period.
pub(super) fn next(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = open_self(args, fn_name)?;
    match string_buffer::next_line(&inst, fn_name)? {
        Some(line) => Ok(Value::string(line)),
        None => Err(PyError::named("StopIteration", String::new())),
    }
}
