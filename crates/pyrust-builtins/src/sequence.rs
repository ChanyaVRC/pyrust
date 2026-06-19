use pyrust_core::{PyBigInt, PyBigIntSign, PyError, PyToPrimitive, Result, Value, ValueKind};

/// Common Sequence Operations — index and count.
/// These apply to both list (Vec<Value>) and tuple (Vec<Value>).
pub fn seq_index(items: &[Value], args: &[Value], type_name: &str) -> Result<Value> {
    let target = args
        .first()
        .ok_or_else(|| PyError::named("TypeError", "index expected at least 1 argument, got 0"))?;
    let len = items.len();
    let start = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_index(i, len).min(len),
        Some(ValueKind::Bool(b)) => normalise_index(b as i64, len).min(len),
        Some(ValueKind::BigInt(b)) => normalise_bigint_index(b, len).min(len),
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or have an __index__ method",
            ));
        }
        None => 0,
    };
    let stop = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_index(i, len).min(len),
        Some(ValueKind::Bool(b)) => normalise_index(b as i64, len).min(len),
        Some(ValueKind::BigInt(b)) => normalise_bigint_index(b, len).min(len),
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or have an __index__ method",
            ));
        }
        None => len,
    };
    // An inverted window (start > stop after normalisation) must be treated as empty,
    // matching CPython's semantics: the search yields zero iterations and falls through
    // to the ValueError below.
    let stop = stop.max(start);
    for (i, item) in items[start..stop].iter().enumerate() {
        // Identity short-circuit (CPython `PyObject_RichCompareBool`): a NaN
        // searching for itself matches even though `==` is False.
        if item == target || item.is_identical_nan(target) {
            return Ok(Value::int((start + i) as i64));
        }
    }
    // CPython's error messages differ between list and tuple:
    //   list  → "{repr(x)} is not in list"
    //   tuple → "tuple.index(x): x not in tuple"
    let msg = if type_name == "tuple" {
        "tuple.index(x): x not in tuple".to_string()
    } else {
        format!("{} is not in {type_name}", target.repr_raw())
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
    // Identity short-circuit (CPython `PyObject_RichCompareBool`): a NaN
    // counting itself matches even though `==` is False.
    let n = items
        .iter()
        .filter(|v| **v == *target || v.is_identical_nan(target))
        .count();
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

/// Clamp a `BigInt` index into `[0, len]`.
///
/// A positive BigInt larger than `len` clamps to `len` (past-the-end).
/// A negative BigInt with magnitude larger than `len` clamps to `0`.
/// This matches CPython's `PySlice_AdjustIndices` behaviour for large ints.
pub fn normalise_bigint_index(idx: &PyBigInt, len: usize) -> usize {
    match idx.sign() {
        PyBigIntSign::Minus => {
            // Negative: try to convert to i64; if it doesn't fit it must be
            // more negative than any valid sequence length → clamp to 0.
            PyToPrimitive::to_i64(idx)
                .map(|i| normalise_index(i, len))
                .unwrap_or(0)
        }
        _ => {
            // Zero or positive: if it fits in usize use it (capped at len by
            // the caller's `.min(len)`); otherwise it's past the end → len.
            PyToPrimitive::to_usize(idx).unwrap_or(len)
        }
    }
}
