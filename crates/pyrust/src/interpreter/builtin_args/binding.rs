// ─── Generic positional+keyword extractor ─────────────────────────────────────
//
// The macro emits, per parameter, code shaped like:
//
//     let path = extract_arg::<PyStr>(
//         args, &positional, FN_NAME, "path",
//         /* pos_index = */ 0,
//         /* kw_allowed = */ true,
//         /* default = */ None,
//     )?;
//
// For a default value, the caller supplies it directly when the slot is
// missing (the macro inlines the literal expression).  This keeps the
// runtime hot path branch-free and lets defaults be arbitrary expressions
// (`PyStr("r".to_string())`, `Value::int(0)`, etc.).

/// Look up the `pos_index`-th positional or `arg_name`-keyword argument.
/// Returns the value to convert, or `None` if absent (caller substitutes
/// the default).
pub(crate) fn locate_arg<'a>(
    args: &'a [ExpandedCallArg],
    positional: &[&'a ExpandedCallArg],
    fn_name: &str,
    arg_name: &str,
    pos_index: usize,
    kw_allowed: bool,
) -> Result<Option<&'a Value>> {
    let pos_match = positional.get(pos_index).map(|a| &a.value);
    let kw_match = if kw_allowed {
        args.iter()
            .find(|a| a.name.as_deref() == Some(arg_name))
            .map(|a| &a.value)
    } else {
        None
    };
    match (pos_match, kw_match) {
        (Some(_), Some(_)) => Err(type_error(format!(
            "{fn_name}() got multiple values for argument '{arg_name}'"
        ))),
        (Some(v), None) => Ok(Some(v)),
        (None, Some(v)) => Ok(Some(v)),
        (None, None) => Ok(None),
    }
}

/// Tightest path through arg validation — for typed signatures whose
/// every parameter is `#[positional_only]` (the all-CPython-builtin
/// shape).  Rejects any keyword argument outright and skips the
/// positional-args collection: when no kwargs are legal, the slice
/// the caller already holds *is* the positional list, indexable
/// directly via `args.get(i)`.
///
/// The macro emits a call to this — plus a direct `args.get(i)` per
/// parameter — instead of the slower
/// `validate_kwargs_and_collect_positional` + `locate_arg` chain when
/// it can prove at compile time that no parameter accepts kwargs.
pub(crate) fn reject_named_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<()> {
    if args.iter().any(|a| a.name.is_some()) {
        return Err(type_error(format!(
            "{fn_name}() takes no keyword arguments"
        )));
    }
    Ok(())
}

/// Build the list of positional args (in source order), checking that every
/// keyword argument is one we recognise.  Returns a `SmallVec` so the
/// common case (≤ 4 args) needs no heap allocation — see
/// [`PositionalArgs`] for the inline-storage rationale.
pub(crate) fn validate_kwargs_and_collect_positional<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
    allowed_kwargs: &[&str],
) -> Result<PositionalArgs<'a>> {
    let mut positional: PositionalArgs<'a> = SmallVec::new();
    for arg in args {
        match &arg.name {
            None => positional.push(arg),
            Some(name) => {
                if !allowed_kwargs.contains(&name.as_str()) {
                    return Err(type_error(format!(
                        "{fn_name}() got an unexpected keyword argument '{name}'"
                    )));
                }
            }
        }
    }
    Ok(positional)
}

/// Bound on positional argument count — emits the CPython-style "too many"
/// error when violated.  Branches the wording on `min == max` so a builtin
/// with no defaults (`min == max == 1`) says `takes 1 positional argument
/// but 2 were given` rather than the nonsensical `takes from 1 to 1
/// positional arguments`.
///
/// Too-few-positional cases are caught downstream by `missing_arg` per
/// parameter (which knows the name); this function intentionally does not
/// check the lower bound.
pub(crate) fn check_positional_count(
    fn_name: &str,
    positional_len: usize,
    min: usize,
    max: usize,
) -> Result<()> {
    if positional_len > max {
        let msg = if min == max {
            let plural = if max == 1 { "argument" } else { "arguments" };
            format!("{fn_name}() takes {max} positional {plural} but {positional_len} were given",)
        } else {
            format!(
                "{fn_name}() takes from {min} to {max} positional arguments but {positional_len} were given",
            )
        };
        return Err(type_error(msg));
    }
    Ok(())
}

/// Construct the "missing required argument" error.  The macro emits a call
/// to this from the per-arg extraction when no value is found and no default
/// is supplied.
pub(crate) fn missing_arg<T>(fn_name: &str, arg_name: &str) -> Result<T> {
    Err(type_error(format!(
        "{fn_name}() missing required argument: '{arg_name}'"
    )))
}
