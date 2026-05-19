use pyrust_core::{PyError, Result, Value, ValueKind};

/// Common Sequence Operations — index and count.
/// These apply to both list (Vec<Value>) and tuple (Vec<Value>).
pub fn seq_index(items: &[Value], args: &[Value], type_name: &str) -> Result<Value> {
    let target = args.first().ok_or_else(|| {
        PyError::Runtime(format!("{type_name}.index() requires at least 1 argument"))
    })?;
    let start = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_index(i, items.len()),
        Some(ValueKind::Bool(b)) => normalise_index(b as i64, items.len()),
        Some(_) => {
            return Err(PyError::Runtime(format!(
                "{type_name}.index() slice indices must be integers"
            )));
        }
        None => 0,
    };
    let stop = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_index(i, items.len()).min(items.len()),
        Some(ValueKind::Bool(b)) => normalise_index(b as i64, items.len()).min(items.len()),
        Some(_) => {
            return Err(PyError::Runtime(format!(
                "{type_name}.index() slice indices must be integers"
            )));
        }
        None => items.len(),
    };
    for (i, item) in items[start..stop].iter().enumerate() {
        if item == target {
            return Ok(Value::int((start + i) as i64));
        }
    }
    Err(PyError::named(
        "ValueError",
        format!("{target} is not in {type_name}"),
    ))
}

pub fn seq_count(items: &[Value], args: &[Value], type_name: &str) -> Result<Value> {
    let target = args
        .first()
        .ok_or_else(|| PyError::Runtime(format!("{type_name}.count() requires 1 argument")))?;
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
