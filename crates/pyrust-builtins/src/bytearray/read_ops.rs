//! Result adaptation and bytearray-specific snapshot read operations.

use pyrust_core::{PyError, Result, Value, ValueKind};

use super::{ByteArrayState, bytearray};

/// Convert a `Value::bytes(...)` result to `bytearray(...)`.  Panics if
/// `v` is not a bytes value — callers guarantee this.
pub(super) fn bytes_val_to_bytearray(v: Value) -> Value {
    match v.kind() {
        ValueKind::Bytes(rc) => bytearray(rc.as_slice().to_vec()),
        _ => panic!("bytes_val_to_bytearray: expected bytes value"),
    }
}

/// Convert a `Value::list` of `Value::bytes` items to a `Value::list` of
/// `bytearray` items (for split/rsplit/splitlines return types).
pub(super) fn bytes_list_to_bytearray_list(v: Value) -> Value {
    // Collect into a snapshot first to avoid holding a kind() borrow.
    let snapshot: Option<Vec<Value>> = match v.kind() {
        ValueKind::List(items) => Some(
            items
                .iter()
                .map(|item| match item.kind() {
                    ValueKind::Bytes(rc) => bytearray(rc.as_slice().to_vec()),
                    _ => item.clone(),
                })
                .collect(),
        ),
        _ => None,
    };
    match snapshot {
        Some(out) => Value::list(out),
        None => v,
    }
}

/// `bytearray.partition` / `bytearray.rpartition` — returns a 3-tuple of
/// bytearray values.
pub(super) fn bytearray_partition(bytes: &[u8], args: &[Value], reverse: bool) -> Result<Value> {
    let name = if reverse { "rpartition" } else { "partition" };
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "bytearray.{name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    let sep_val = &args[0];
    let sep: Vec<u8> = match sep_val.kind() {
        ValueKind::Bytes(rc) => rc.as_slice().to_vec(),
        ValueKind::BuiltinObject { ops, state }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
        {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<ByteArrayState>()
                .expect("bytearray state");
            s.data.borrow().clone()
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(sep_val)
                ),
            ));
        }
    };
    if sep.is_empty() {
        return Err(PyError::named("ValueError", "empty separator".to_string()));
    }
    let found = if reverse {
        crate::bytes::rfind_subsequence(bytes, &sep)
    } else {
        crate::bytes::find_subsequence(bytes, &sep)
    };
    let parts = match found {
        Some(pos) => {
            let before = bytearray(bytes[..pos].to_vec());
            let mid = bytearray(sep.clone());
            let after = bytearray(bytes[pos + sep.len()..].to_vec());
            vec![before, mid, after]
        }
        None => {
            if reverse {
                vec![
                    bytearray(vec![]),
                    bytearray(vec![]),
                    bytearray(bytes.to_vec()),
                ]
            } else {
                vec![
                    bytearray(bytes.to_vec()),
                    bytearray(vec![]),
                    bytearray(vec![]),
                ]
            }
        }
    };
    Ok(Value::tuple(parts))
}

/// `bytearray.join(iterable)` — join elements of an iterable of bytes-like
/// objects using this bytearray as separator.
pub(super) fn bytearray_join(sep: &[u8], args: &[Value]) -> Result<Value> {
    let iterable = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "join() takes exactly one argument (0 given)".to_string(),
        )
    })?;
    // Collect elements from the iterable.
    let items = pyrust_core::iter_values_via_registry(iterable).map_err(|_| {
        PyError::named(
            "TypeError",
            format!(
                "can only join an iterable, not '{}'",
                pyrust_core::builtin_type_name(iterable)
            ),
        )
    })?;
    let mut out: Vec<u8> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(sep);
        }
        match item.kind() {
            ValueKind::Bytes(rc) => out.extend_from_slice(rc.as_slice()),
            ValueKind::BuiltinObject { ops, state }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
            {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<ByteArrayState>()
                    .expect("bytearray state");
                out.extend_from_slice(&s.data.borrow());
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {}: expected a bytes-like object, {} found",
                        i,
                        pyrust_core::builtin_type_name(item)
                    ),
                ));
            }
        }
    }
    Ok(bytearray(out))
}

/// Title-case a byte slice (same logic as bytes).
pub(super) fn bytes_title(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut prev_was_alpha = false;
    for &b in bytes {
        if b.is_ascii_alphabetic() {
            if prev_was_alpha {
                out.push(b.to_ascii_lowercase());
            } else {
                out.push(b.to_ascii_uppercase());
            }
            prev_was_alpha = true;
        } else {
            out.push(b);
            prev_was_alpha = false;
        }
    }
    out
}

/// Capitalize a byte slice (same logic as bytes).
pub(super) fn bytes_capitalize(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = bytes.iter().map(|b| b.to_ascii_lowercase()).collect();
    if let Some(first) = out.first_mut() {
        *first = first.to_ascii_uppercase();
    }
    out
}
