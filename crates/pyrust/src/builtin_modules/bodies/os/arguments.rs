//! Shared argument validation for the `os` registration adapters.
//!
//! These helpers understand only PyRust's call representation and primitive
//! Python coercions. Host filesystem, environment, and process operations
//! belong to their respective implementation modules.

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, reject_keyword_args_expanded, value_type_name_str};
use crate::value::{Value, ValueKind};

/// Pull exactly one positional path argument.
pub(super) fn single_path_arg(fn_name: &str, args: &[ExpandedCallArg]) -> Result<String> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    require_str(fn_name, &args[0].value, "path")
}

/// Coerce a value to a Rust string, preserving the existing `os` diagnostics.
pub(super) fn require_str(fn_name: &str, value: &Value, what: &str) -> Result<String> {
    match value.kind() {
        ValueKind::Str(string) => Ok(string.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {what} must be str, not {}",
                value_type_name_str(value),
            ),
        )),
    }
}

/// Require an integer-shaped value for integer `os` parameters.
pub(super) fn require_int(fn_name: &str, value: &Value, what: &str) -> Result<i64> {
    match value.kind() {
        ValueKind::Int(number) => Ok(number),
        ValueKind::Bool(boolean) => Ok(boolean as i64),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {what} must be int, not {}",
                value_type_name_str(value),
            ),
        )),
    }
}

/// Internal guard for private `_Environ` methods.
pub(super) fn require_self(args: &[ExpandedCallArg], fn_name: &str) -> Result<()> {
    if args.is_empty() {
        Err(PyError::Runtime(format!(
            "internal: {fn_name}() called without self",
        )))
    } else {
        Ok(())
    }
}

/// Pull the one user-visible key argument after `_Environ`'s receiver.
pub(super) fn require_key_arg(fn_name: &str, args: &[ExpandedCallArg]) -> Result<String> {
    require_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name} takes exactly 1 argument ({} given)",
                args.len() - 1
            ),
        ));
    }
    require_str(fn_name, &args[1].value, "key")
}

/// Validate a receiver-only `_Environ` method call.
pub(super) fn require_no_user_args(
    args: &[ExpandedCallArg],
    fn_name: &str,
    method: &str,
) -> Result<()> {
    require_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{method}() takes no arguments ({} given)", args.len() - 1),
        ));
    }
    Ok(())
}

/// Minimal truthiness needed by `os.makedirs(exist_ok=...)`.
pub(super) fn value_is_truthy(value: &Value) -> bool {
    match value.kind() {
        ValueKind::Bool(boolean) => boolean,
        ValueKind::Int(number) => number != 0,
        ValueKind::None => false,
        _ => true,
    }
}
