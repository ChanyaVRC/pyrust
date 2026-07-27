/// Parse and validate arguments for `exec()` and `eval()`.
///
/// Both accept `(source[, globals[, locals]])`.  Returns
/// `(source_value, globals_option, locals_option)`.
fn parse_exec_eval_args(
    fn_name: &str,
    args: &[crate::interpreter::ExpandedCallArg],
) -> Result<(Value, Option<Value>, Option<Value>)> {
    // Reject keyword arguments.
    if args.iter().any(|a| a.name.is_some()) {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no keyword arguments"),
        ));
    }
    if args.is_empty() || args.len() > 3 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes from 1 to 3 positional arguments ({} given)",
                args.len()
            ),
        ));
    }
    let source_val = args[0].value.clone();
    let globals_opt = args
        .get(1)
        .map(|a| a.value.clone())
        .filter(|v| !matches!(v.kind(), ValueKind::None));
    let locals_opt = args
        .get(2)
        .map(|a| a.value.clone())
        .filter(|v| !matches!(v.kind(), ValueKind::None));
    Ok((source_val, globals_opt, locals_opt))
}

/// CPython injects `__builtins__` into a caller-supplied globals dict the first
/// time `eval()` or `exec()` is called with it (see `PyEval_EvalCode`).  If the
/// dict does not already contain `"__builtins__"`, insert the builtins module.
///
/// Existing values (including `{}` as a deliberate override) are left alone.
fn inject_builtins_into_globals(globals: &Value) {
    use crate::value::StrKey;
    let already_present = globals
        .dict_with(|d| d.contains_key(&StrKey("__builtins__")))
        .unwrap_or(true); // not a dict — leave it alone
    if !already_present {
        let builtins = crate::interpreter::cached_builtins_module();
        let _ = globals.dict_insert(PyKey::str_from("__builtins__"), builtins);
    }
}
