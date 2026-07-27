// Argument coercion and state decoding for `islice`.

/// Read the (`_iter`, `_skip`, `_remaining_stop`, `_step`) state.
fn read_islice_state(
    inst: &Rc<std::cell::RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<(Value, i64, Option<i64>, i64)> {
    let attrs = inst.borrow();
    let iter = attrs
        .attrs
        .get("_iter")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    let skip = match attrs.attrs.get("_skip").map(|value| value.kind()) {
        Some(ValueKind::Int(value)) => value,
        _ => return Err(internal(fn_name)),
    };
    let remaining = match attrs.attrs.get("_remaining_stop").map(|value| value.kind()) {
        Some(ValueKind::Int(value)) => Some(value),
        Some(ValueKind::None) | None => None,
        _ => return Err(internal(fn_name)),
    };
    let step = match attrs.attrs.get("_step").map(|value| value.kind()) {
        Some(ValueKind::Int(value)) => value,
        _ => return Err(internal(fn_name)),
    };
    Ok((iter, skip, remaining, step))
}

/// Extract a non-negative `i64` (or `None`) from an `islice` slot.
fn slice_arg(
    interp: &mut crate::Interpreter,
    _fn_name: &str,
    value: &Value,
    slot: &str,
) -> Result<Option<i64>> {
    // CPython's evaluate_slice_index honors __index__, but itertools.islice
    // clears coercion failures and replaces them with slot-specific
    // ValueErrors.
    let resolved = match value.kind() {
        ValueKind::None => return Ok(None),
        ValueKind::Int(_) | ValueKind::Bool(_) => Ok(value.clone()),
        ValueKind::PyInstance(_) => interp.value_to_index(value, |_| {
            PyError::named("__pyrust_NotIndex__", String::new())
        }),
        _ => Err(PyError::named("__pyrust_NotIndex__", String::new())),
    };
    let indices_message =
        || "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.".to_string();
    let slot_error = |coerced_negative_in_range: bool| {
        let message = match slot {
            "step" => "Step for islice() must be a positive integer or None.".to_string(),
            // A successfully coerced negative stop <= -2 gets the generic
            // indices message. Coercion failure, overflow, and -1 use the
            // stop-specific message.
            "stop" if !coerced_negative_in_range => {
                "Stop argument for islice() must be None or an integer: 0 <= x <= sys.maxsize."
                    .to_string()
            }
            _ => indices_message(),
        };
        PyError::named("ValueError", message)
    };
    match resolved {
        Ok(resolved) => match resolved.kind() {
            ValueKind::Int(number) if number >= 0 => Ok(Some(number)),
            ValueKind::Bool(value) => Ok(Some(value as i64)),
            ValueKind::Int(number) => Err(slot_error(number <= -2)),
            ValueKind::BigInt(_) => Err(slot_error(false)),
            _ => unreachable!("value_to_index guarantees an integer"),
        },
        Err(_) => Err(slot_error(false)),
    }
}
