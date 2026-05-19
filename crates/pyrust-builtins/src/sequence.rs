use pyrust_core::{PyError, Result, Value, ValueKind};

/// Common Sequence Operations — index and count.
/// These apply to both list (Vec<Value>) and tuple (Vec<Value>).
pub fn seq_index(items: &[Value], args: &[Value], type_name: &str) -> Result<Value> {
    let target = args
        .first()
        .ok_or_else(|| PyError::named("TypeError", "index expected at least 1 argument, got 0"))?;
    let start = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_index(i, items.len()).min(items.len()),
        Some(ValueKind::Bool(b)) => normalise_index(b as i64, items.len()).min(items.len()),
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or have an __index__ method",
            ));
        }
        None => 0,
    };
    let stop = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_index(i, items.len()).min(items.len()),
        Some(ValueKind::Bool(b)) => normalise_index(b as i64, items.len()).min(items.len()),
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or have an __index__ method",
            ));
        }
        None => items.len(),
    };
    // An inverted window (start > stop after normalisation) must be treated as empty,
    // matching CPython's semantics: the search yields zero iterations and falls through
    // to the ValueError below.
    let stop = stop.max(start);
    for (i, item) in items[start..stop].iter().enumerate() {
        if item == target {
            return Ok(Value::int((start + i) as i64));
        }
    }
    // CPython's error messages differ between list and tuple:
    //   list  → "{x} is not in list"
    //   tuple → "tuple.index(x): x not in tuple"
    let msg = if type_name == "tuple" {
        "tuple.index(x): x not in tuple".to_string()
    } else {
        format!("{target} is not in {type_name}")
    };
    Err(PyError::named("ValueError", msg))
}

pub fn seq_count(items: &[Value], args: &[Value], type_name: &str) -> Result<Value> {
    let target = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!("{type_name}.count() takes exactly one argument (0 given)"),
        )
    })?;
    let n = items.iter().filter(|v| *v == target).count();
    Ok(Value::int(n as i64))
}

/// Clamp a possibly-negative index into `[0, len]`.
pub fn normalise_index(idx: i64, len: usize) -> usize {
    if idx < 0 {
        let from_end = (-idx) as usize;
        len.saturating_sub(from_end)
    } else {
        idx as usize
    }
}
