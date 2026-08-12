fn min_max_primitive_slice(items: &[Value], is_max: bool) -> Option<Result<Value>> {
    if items.is_empty()
        || items
            .iter()
            .any(|value| matches!(value.kind(), ValueKind::PyInstance(_)))
    {
        return None;
    }
    let op = if is_max { ">" } else { "<" };
    let mut best = &items[0];
    for value in &items[1..] {
        match compare_values_with_op(value, best, op) {
            Ok(ordering) => {
                let take = if is_max {
                    ordering == std::cmp::Ordering::Greater
                } else {
                    ordering == std::cmp::Ordering::Less
                };
                if take {
                    best = value;
                }
            }
            // The interpreter-free comparator cannot see a PyInstance nested
            // inside a list/tuple.  Retry only that TypeError through
            // `min_max_compare`; semantic failures such as RecursionError
            // must propagate without a second comparison pass.
            Err(error) => {
                if matches!(&error, PyError::Named(class, _) if class.as_ref() == "TypeError") {
                    return None;
                }
                return Some(Err(error));
            }
        }
    }
    Some(Ok(best.clone()))
}

fn min_max_compare(
    interp: &mut crate::Interpreter,
    candidate: &Value,
    best: &Value,
    is_max: bool,
) -> Result<std::cmp::Ordering> {
    // Comparison protocol ownership stays in the runtime.  Its first branch
    // is still the interpreter-free primitive comparator; only a failed
    // concrete sequence comparison enters user-dispatching lexicography.
    if is_max {
        interp.richcmp_order_gt(candidate, best)
    } else {
        interp.richcmp_order(candidate, best)
    }
}

fn min_max_impl(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    is_max: bool,
    fn_name: &str,
) -> Result<Value> {
    let positional: Vec<&ExpandedCallArg> = args.iter().filter(|arg| arg.name.is_none()).collect();
    if positional.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name} expected at least 1 argument, got 0"),
        ));
    }
    let key_fn = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("key"))
        .map(|arg| arg.value.clone())
        .filter(|value| !value.is_none());
    let default_value = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("default"))
        .map(|arg| arg.value.clone());
    for arg in args.iter().filter(|arg| arg.name.is_some()) {
        if arg.name.as_deref() != Some("key") && arg.name.as_deref() != Some("default") {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' is an invalid keyword argument for {fn_name}()",
                    arg.name.as_ref().unwrap()
                ),
            ));
        }
    }
    if key_fn.is_none()
        && positional.len() == 1
        && let Some(result) = positional[0]
            .value
            .list_with(|items| min_max_primitive_slice(items, is_max))
        && let Some(output) = result
    {
        return output;
    }
    if positional.len() > 1 && default_value.is_some() {
        return Err(PyError::named(
            "TypeError",
            format!("Cannot specify a default for {fn_name}() with multiple positional arguments"),
        ));
    }
    if let Some(key_fn) = key_fn.as_ref()
        && positional.len() == 1
    {
        let mut iterator = ConsumerIterator::new(interp, &positional[0].value)?;
        let Some(mut best_value) = iterator.next(interp)? else {
            if let Some(default) = default_value {
                return Ok(default);
            }
            return Err(PyError::named(
                "ValueError",
                format!("{fn_name}() iterable argument is empty"),
            ));
        };
        let mut best_key = interp.call_function_expanded(
            key_fn.clone(),
            &[ExpandedCallArg {
                name: None,
                value: best_value.clone(),
            }],
        )?;
        while let Some(candidate_value) = iterator.next(interp)? {
            let candidate_key = interp.call_function_expanded(
                key_fn.clone(),
                &[ExpandedCallArg {
                    name: None,
                    value: candidate_value.clone(),
                }],
            )?;
            let ordering = min_max_compare(interp, &candidate_key, &best_key, is_max)?;
            if (is_max && ordering == std::cmp::Ordering::Greater)
                || (!is_max && ordering == std::cmp::Ordering::Less)
            {
                best_key = candidate_key;
                best_value = candidate_value;
            }
        }
        return Ok(best_value);
    }
    let items = if positional.len() == 1 {
        interp.collect_iterable(&positional[0].value)?
    } else {
        positional.iter().map(|arg| arg.value.clone()).collect()
    };
    if items.is_empty() {
        if let Some(default) = default_value {
            return Ok(default);
        }
        return Err(PyError::named(
            "ValueError",
            format!("{fn_name}() iterable argument is empty"),
        ));
    }
    if let Some(key_fn) = key_fn {
        let mut keyed = Vec::with_capacity(items.len());
        for value in items {
            let key = interp.call_function_expanded(
                key_fn.clone(),
                &[ExpandedCallArg {
                    name: None,
                    value: value.clone(),
                }],
            )?;
            keyed.push((key, value));
        }
        let mut result_error = None;
        let result = keyed
            .into_iter()
            .reduce(|best, candidate| {
                if result_error.is_some() {
                    return best;
                }
                let comparison = min_max_compare(interp, &candidate.0, &best.0, is_max);
                match comparison {
                    Ok(ordering)
                        if (is_max && ordering == std::cmp::Ordering::Greater)
                            || (!is_max && ordering == std::cmp::Ordering::Less) =>
                    {
                        candidate
                    }
                    Ok(_) => best,
                    Err(error) => {
                        result_error = Some(error);
                        best
                    }
                }
            })
            .unwrap();
        if let Some(error) = result_error {
            return Err(error);
        }
        Ok(result.1)
    } else {
        let mut result_error = None;
        let result = items
            .into_iter()
            .reduce(|best, candidate| {
                if result_error.is_some() {
                    return best;
                }
                let comparison = min_max_compare(interp, &candidate, &best, is_max);
                match comparison {
                    Ok(ordering)
                        if (is_max && ordering == std::cmp::Ordering::Greater)
                            || (!is_max && ordering == std::cmp::Ordering::Less) =>
                    {
                        candidate
                    }
                    Ok(_) => best,
                    Err(error) => {
                        result_error = Some(error);
                        best
                    }
                }
            })
            .unwrap();
        if let Some(error) = result_error {
            return Err(error);
        }
        Ok(result)
    }
}
