//! CPython-compatible argument-count and keyword diagnostics for the `struct`
//! module functions and `Struct` methods.
//!
//! CPython implements these entry points with a mix of Argument Clinic,
//! `METH_FASTCALL`, and hand-written checks, so each one has its own wording.
//! The helpers below reproduce those messages exactly (3.12.3).

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;

use super::format::raise_struct_error;

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

/// Split `args` into positional values and keyword arguments.
pub(super) fn split_args(args: &[ExpandedCallArg]) -> (Vec<&Value>, Vec<(&str, &Value)>) {
    let mut positional = Vec::with_capacity(args.len());
    let mut keywords = Vec::new();
    for arg in args {
        match arg.name.as_deref() {
            Some(name) => keywords.push((name, &arg.value)),
            None => positional.push(&arg.value),
        }
    }
    (positional, keywords)
}

/// Reject any keyword argument with CPython's `X() takes no keyword arguments`.
pub(super) fn no_keywords(label: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if args.iter().any(|arg| arg.name.is_some()) {
        return Err(type_error(format!("{label}() takes no keyword arguments")));
    }
    Ok(())
}

/// `label() takes exactly one argument (N given)` for a one-argument callable.
pub(super) fn exactly_one<'a>(label: &str, args: &'a [ExpandedCallArg]) -> Result<&'a Value> {
    no_keywords(label, args)?;
    match args {
        [only] => Ok(&only.value),
        _ => Err(type_error(format!(
            "{label}() takes exactly one argument ({} given)",
            args.len()
        ))),
    }
}

/// `unpack` / `iter_unpack` style: `label expected 2 arguments, got N`.
pub(super) fn exactly_two<'a>(
    label: &str,
    fn_label: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, &'a Value)> {
    no_keywords(fn_label, args)?;
    match args {
        [first, second] => Ok((&first.value, &second.value)),
        _ => Err(type_error(format!(
            "{label} expected 2 arguments, got {}",
            args.len()
        ))),
    }
}

/// `_clearcache()` accepts no arguments at all.
pub(super) fn no_arguments(label: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if args.is_empty() {
        return Ok(());
    }
    Err(type_error(format!(
        "{label}() takes no arguments ({} given)",
        args.len()
    )))
}

/// Parsed `unpack_from` arguments.
pub(super) struct UnpackFromArgs<'a> {
    pub(super) buffer: &'a Value,
    pub(super) offset: Option<&'a Value>,
}

/// Parse `unpack_from(buffer, offset=0)` as an Argument Clinic signature.
///
/// `positional_only` is the number of leading positional-only parameters
/// before `buffer` (1 for `struct.unpack_from(format, /, buffer, offset=0)`,
/// 0 for the `Struct.unpack_from(buffer, offset=0)` method).  `positional`
/// includes those leading positional-only values.
pub(super) fn parse_unpack_from<'a>(
    positional: &[&'a Value],
    keywords: &[(&str, &'a Value)],
    positional_only: usize,
) -> Result<UnpackFromArgs<'a>> {
    const NAME: &str = "unpack_from";
    let total = positional.len();
    if total < positional_only {
        return Err(type_error(format!(
            "{NAME}() takes at least {positional_only} positional argument{} ({total} given)",
            if positional_only == 1 { "" } else { "s" }
        )));
    }
    let max = positional_only + 2;
    // CPython's clinic parser counts keyword arguments toward the limit.
    let given = total + keywords.len();
    if given > max {
        return Err(type_error(format!(
            "{NAME}() takes at most {max} arguments ({given} given)"
        )));
    }
    let mut buffer = positional.get(positional_only).copied();
    let mut offset = positional.get(positional_only + 1).copied();
    for (name, value) in keywords {
        let (slot, pos) = match *name {
            "buffer" => (&mut buffer, positional_only + 1),
            "offset" => (&mut offset, positional_only + 2),
            other => {
                return Err(type_error(format!(
                    "'{other}' is an invalid keyword argument for {NAME}()"
                )));
            }
        };
        if slot.is_some() {
            return Err(type_error(format!(
                "argument for {NAME}() given by name ('{name}') and position ({pos})"
            )));
        }
        *slot = Some(*value);
    }
    let Some(buffer) = buffer else {
        return Err(type_error(format!(
            "{NAME}() missing required argument 'buffer' (pos {})",
            positional_only + 1
        )));
    };
    Ok(UnpackFromArgs { buffer, offset })
}

/// Pull the `buffer` and `offset` leading arguments of `pack_into`.
///
/// CPython raises `struct.error` (not `TypeError`) when they are missing.
pub(super) fn pack_into_head<'a>(
    positional: &[&'a Value],
    error_cls: &Value,
) -> Result<(&'a Value, &'a Value)> {
    let Some(buffer) = positional.first().copied() else {
        return Err(raise_struct_error(
            error_cls,
            "pack_into expected buffer argument",
        ));
    };
    let Some(offset) = positional.get(1).copied() else {
        return Err(raise_struct_error(
            error_cls,
            "pack_into expected offset argument",
        ));
    };
    Ok((buffer, offset))
}

/// Parse `Struct.__init__(format)`, which accepts `format` positionally or by
/// keyword.
pub(super) fn parse_struct_init<'a>(
    positional: &[&'a Value],
    keywords: &[(&str, &'a Value)],
) -> Result<&'a Value> {
    const NAME: &str = "Struct";
    if positional.len() > 1 {
        return Err(type_error(format!(
            "{NAME}() takes at most 1 argument ({} given)",
            positional.len()
        )));
    }
    let mut format = positional.first().copied();
    for (name, value) in keywords {
        if *name != "format" {
            if format.is_none() {
                // CPython reports the missing parameter before the stray keyword.
                continue;
            }
            return Err(type_error(format!(
                "'{name}' is an invalid keyword argument for {NAME}()"
            )));
        }
        if format.is_some() {
            return Err(type_error(format!(
                "argument for {NAME}() given by name ('format') and position (1)"
            )));
        }
        format = Some(*value);
    }
    format.ok_or_else(|| {
        type_error(format!(
            "{NAME}() missing required argument 'format' (pos 1)"
        ))
    })
}
