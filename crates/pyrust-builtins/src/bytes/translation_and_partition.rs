fn bytes_translate(bytes: &[u8], args: &[Value], kwargs: &PyDict) -> Result<Value> {
    // CPython signature: bytes.translate(table, /, delete=b'')
    // table may be None or a 256-byte mapping (bytes).
    // When table is None: just delete bytes in the delete set.
    // When table is provided: map each byte through the table, then delete.

    // CPython counts all arguments (positional + keyword) and raises if the total
    // exceeds 2 (table + delete).  This fires before the duplicate-delete or
    // unknown-keyword checks.
    let total_args = args.len() + kwargs.len();
    if total_args > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "translate() takes at most 2 arguments ({} given)",
                total_args
            ),
        ));
    }

    // Reject unrecognised keyword arguments.
    for key in kwargs.keys() {
        if let PyKey::Str(s) = key {
            let name = s.as_str().unwrap_or("");
            if name != "delete" {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{name}' is an invalid keyword argument for translate()"),
                ));
            }
        }
    }

    let table_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "translate() takes at least 1 positional argument (0 given)".to_string(),
        )
    })?;
    let table: Option<&[u8]> = match table_val.kind() {
        ValueKind::None => None,
        ValueKind::Bytes(rc) => {
            let s = rc.as_slice();
            if s.len() != 256 {
                return Err(PyError::named(
                    "ValueError",
                    "translation table must be 256 characters long".to_string(),
                ));
            }
            Some(s)
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(table_val)
                ),
            ));
        }
    };

    // `delete` may come from args[1] (positional) or from the `delete=` keyword argument.
    // The total-args guard above ensures they can't both be set simultaneously.
    let kw_delete = kwargs.get(&StrKey("delete"));
    let delete_val = args.get(1).or(kw_delete);
    let delete: &[u8] = match delete_val.map(|v| v.kind()) {
        None => &[],
        Some(ValueKind::Bytes(rc)) => rc.as_slice(),
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(delete_val.unwrap())
                ),
            ));
        }
    };

    // Build a 256-entry boolean table so each delete-membership check is O(1)
    // instead of O(|delete|), making the overall loop O(n) rather than O(n × |delete|).
    let mut delete_table = [false; 256];
    for &b in delete {
        delete_table[b as usize] = true;
    }

    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if delete_table[b as usize] {
            continue;
        }
        let mapped = match table {
            None => b,
            Some(t) => t[b as usize],
        };
        out.push(mapped);
    }
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// partition / rpartition
// ---------------------------------------------------------------------------

fn bytes_partition(bytes: &[u8], args: &[Value], reverse: bool) -> Result<Value> {
    let name = if reverse { "rpartition" } else { "partition" };
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "bytes.{name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    let sep_val = &args[0];
    let sep: &[u8] = match sep_val.kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
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
        rfind_subsequence(bytes, sep)
    } else {
        find_subsequence(bytes, sep)
    };
    let parts = match found {
        Some(pos) => {
            let before = Value::bytes(bytes[..pos].to_vec());
            let mid = Value::bytes(sep.to_vec());
            let after = Value::bytes(bytes[pos + sep.len()..].to_vec());
            vec![before, mid, after]
        }
        None => {
            if reverse {
                // rpartition not found: (b'', b'', original)
                vec![
                    Value::bytes(vec![]),
                    Value::bytes(vec![]),
                    Value::bytes(bytes.to_vec()),
                ]
            } else {
                // partition not found: (original, b'', b'')
                vec![
                    Value::bytes(bytes.to_vec()),
                    Value::bytes(vec![]),
                    Value::bytes(vec![]),
                ]
            }
        }
    };
    Ok(Value::tuple(parts))
}

// ---------------------------------------------------------------------------
// istitle
// ---------------------------------------------------------------------------

fn bytes_istitle(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    // A byte sequence is titlecased if each word (sequence of alpha bytes) starts
    // with an uppercase letter and the rest are lowercase.  Non-alpha bytes act as
    // word separators.
    let mut prev_was_alpha = false;
    let mut has_alpha = false;
    for &b in bytes {
        if b.is_ascii_alphabetic() {
            has_alpha = true;
            if prev_was_alpha {
                // Continuation of a word — must be lowercase.
                if b.is_ascii_uppercase() {
                    return false;
                }
            } else {
                // Start of a new word — must be uppercase.
                if b.is_ascii_lowercase() {
                    return false;
                }
            }
            prev_was_alpha = true;
        } else {
            prev_was_alpha = false;
        }
    }
    has_alpha
}
