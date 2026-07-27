// Callable detection and Python-semantic cache key construction.

/// Decide whether `lru_cache` should interpret its first positional argument
/// as the function to wrap for the bare `@lru_cache` form.
fn is_callable(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::UserFunction(_)
            | ValueKind::BuiltinFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. }
            | ValueKind::PyClass(_)
    )
}

/// Build the same logical key shape as CPython's private `_make_key`.
fn build_key(interp: &mut Interpreter, args: &[ExpandedCallArg], typed: bool) -> Result<PyKey> {
    // Exact int/str single arguments use CPython's direct fast-path key.
    // Check this before splitting positional and keyword arguments so the
    // common LRU-hit case does not allocate either temporary Vec.
    if !typed
        && let [argument] = args
        && argument.name.is_none()
        && matches!(
            argument.value.kind(),
            ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Str(_)
        )
    {
        return interp.value_to_pykey(&argument.value);
    }

    let positional: Vec<&Value> = args
        .iter()
        .filter(|argument| argument.name.is_none())
        .map(|argument| &argument.value)
        .collect();
    let keywords: Vec<(&str, &Value)> = args
        .iter()
        .filter_map(|argument| argument.name.as_deref().map(|name| (name, &argument.value)))
        .collect();

    let mut parts = Vec::with_capacity(
        positional.len()
            + usize::from(!keywords.is_empty())
            + keywords.len() * 2
            + if typed {
                positional.len() + keywords.len()
            } else {
                0
            },
    );
    parts.extend(positional.iter().map(|value| (*value).clone()));
    if !keywords.is_empty() {
        // An unreachable builtin-function identity separates positional
        // values from the keyword suffix without a forgeable string token.
        parts.push(Value::builtin_function("__pyrust_functools_lru_kwd_mark"));
        for (name, value) in &keywords {
            // Keyword insertion order intentionally participates in the key.
            parts.push(Value::string(name));
            parts.push((*value).clone());
        }
    }
    if typed {
        parts.extend(positional.iter().map(|value| value_class(value)));
        parts.extend(keywords.iter().map(|(_, value)| value_class(value)));
    }
    interp.value_to_pykey(&Value::tuple(parts))
}
