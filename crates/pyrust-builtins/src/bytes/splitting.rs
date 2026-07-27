fn bytes_split(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let (sep_opt, maxsplit) = bytes_split_args(args)?;
    let parts = match sep_opt {
        None => bytes_split_whitespace(bytes, maxsplit, false),
        Some(sep) => {
            if sep.is_empty() {
                return Err(PyError::named("ValueError", "empty separator".to_string()));
            }
            bytes_split_by_sep(bytes, sep, maxsplit, false)
        }
    };
    Ok(Value::list(
        parts
            .into_iter()
            .map(|b| Value::bytes(b.to_vec()))
            .collect(),
    ))
}

fn bytes_rsplit(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let (sep_opt, maxsplit) = bytes_split_args(args)?;
    let parts = match sep_opt {
        None => bytes_split_whitespace(bytes, maxsplit, true),
        Some(sep) => {
            if sep.is_empty() {
                return Err(PyError::named("ValueError", "empty separator".to_string()));
            }
            bytes_split_by_sep(bytes, sep, maxsplit, true)
        }
    };
    Ok(Value::list(
        parts
            .into_iter()
            .map(|b| Value::bytes(b.to_vec()))
            .collect(),
    ))
}

/// Parse (sep, maxsplit) from bytes split/rsplit args.
fn bytes_split_args(args: &[Value]) -> Result<(Option<&[u8]>, i64)> {
    let sep = match args.first() {
        Some(v) => match v.kind() {
            ValueKind::Bytes(rc) => Some(rc.as_slice()),
            ValueKind::None => None,
            // CPython 3.12 names the offending separator type (issue #2044).
            // bytearray / bytes-subclass separators are pre-coerced to `Bytes`
            // by the interpreter, so only genuinely-wrong types reach here.
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "a bytes-like object is required, not '{}'",
                        pyrust_core::builtin_type_name(v)
                    ),
                ));
            }
        },
        None => None,
    };
    let maxsplit: i64 = match args.get(1).map(|v| v.kind()) {
        None => -1,
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            ));
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.split() maxsplit must be an integer".to_string(),
            ));
        }
    };
    Ok((sep, maxsplit))
}

/// Split on ASCII whitespace (consecutive whitespace is a single separator).
/// Mirrors CPython's `bytes.split()` with no sep argument.
fn bytes_split_whitespace<'a>(bytes: &'a [u8], maxsplit: i64, reverse: bool) -> Vec<&'a [u8]> {
    if reverse {
        let max = if maxsplit < 0 {
            usize::MAX
        } else {
            maxsplit as usize
        };
        let mut parts: Vec<&'a [u8]> = Vec::new();
        let mut end = bytes.len();
        let mut splits = 0;
        // Trim trailing whitespace.
        while end > 0 && is_ascii_whitespace(bytes[end - 1]) {
            end -= 1;
        }
        while end > 0 && splits < max {
            // Scan backwards to find start of the token.
            let token_end = end;
            let mut start = token_end;
            while start > 0 && !is_ascii_whitespace(bytes[start - 1]) {
                start -= 1;
            }
            parts.push(&bytes[start..token_end]);
            splits += 1;
            // Skip whitespace.
            end = start;
            while end > 0 && is_ascii_whitespace(bytes[end - 1]) {
                end -= 1;
            }
        }
        // Any remaining bytes before end go into the last (leftmost) chunk.
        if end > 0 {
            parts.push(&bytes[..end]);
        }
        parts.reverse();
        parts
    } else {
        let max = if maxsplit < 0 {
            usize::MAX
        } else {
            maxsplit as usize
        };
        let mut parts: Vec<&'a [u8]> = Vec::new();
        let mut i = 0;
        let len = bytes.len();
        // Skip leading whitespace.
        while i < len && is_ascii_whitespace(bytes[i]) {
            i += 1;
        }
        let mut splits = 0;
        while i < len {
            if splits >= max {
                // Remaining bytes as the last element.
                parts.push(&bytes[i..]);
                break;
            }
            // Collect non-whitespace token.
            let start = i;
            while i < len && !is_ascii_whitespace(bytes[i]) {
                i += 1;
            }
            parts.push(&bytes[start..i]);
            splits += 1;
            // Skip whitespace.
            while i < len && is_ascii_whitespace(bytes[i]) {
                i += 1;
            }
        }
        parts
    }
}

/// Split by a literal separator.
fn bytes_split_by_sep<'a>(
    bytes: &'a [u8],
    sep: &[u8],
    maxsplit: i64,
    reverse: bool,
) -> Vec<&'a [u8]> {
    let max = if maxsplit < 0 {
        usize::MAX
    } else {
        maxsplit as usize
    };
    if reverse {
        let mut parts: Vec<&'a [u8]> = Vec::new();
        let mut end = bytes.len();
        let mut splits = 0;
        while splits < max {
            match rfind_subsequence(&bytes[..end], sep) {
                Some(pos) => {
                    parts.push(&bytes[pos + sep.len()..end]);
                    end = pos;
                    splits += 1;
                }
                None => break,
            }
        }
        parts.push(&bytes[..end]);
        parts.reverse();
        parts
    } else {
        let mut parts: Vec<&'a [u8]> = Vec::new();
        let mut start = 0;
        let mut splits = 0;
        while splits < max {
            match find_subsequence(&bytes[start..], sep) {
                Some(pos) => {
                    parts.push(&bytes[start..start + pos]);
                    start += pos + sep.len();
                    splits += 1;
                }
                None => break,
            }
        }
        parts.push(&bytes[start..]);
        parts
    }
}

// ---------------------------------------------------------------------------
// splitlines
// ---------------------------------------------------------------------------

fn bytes_splitlines(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let keepends =
        crate::method_signature::normalized_optional_bool("bytes", "splitlines", "keepends", args)?;
    // bytes.splitlines() only recognises \r\n, \r, \n.
    let len = bytes.len();
    let mut lines: Vec<Value> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < len {
        let eol_len = match bytes[i] {
            b'\n' => 1,
            b'\r' => {
                if i + 1 < len && bytes[i + 1] == b'\n' {
                    2
                } else {
                    1
                }
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let end = if keepends { i + eol_len } else { i };
        lines.push(Value::bytes(bytes[start..end].to_vec()));
        i += eol_len;
        start = i;
    }
    // Trailing non-empty segment (no trailing newline).
    if start < len {
        lines.push(Value::bytes(bytes[start..].to_vec()));
    }
    Ok(Value::list(lines))
}

// ---------------------------------------------------------------------------
// join
// ---------------------------------------------------------------------------

fn bytes_join(sep: &[u8], args: &[Value]) -> Result<Value> {
    let iterable = args.first().ok_or_else(|| {
        PyError::named("TypeError", "bytes.join() requires 1 argument".to_string())
    })?;

    /// Extract byte items from a slice of Values, all must be bytes-like.
    fn collect_items(vals: &[Value]) -> Result<Vec<Vec<u8>>> {
        vals.iter()
            .enumerate()
            .map(|(i, v)| match v.kind() {
                ValueKind::Bytes(rc) => Ok(rc.as_slice().to_vec()),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected a bytes-like object, {} found",
                        pyrust_core::builtin_type_name(v),
                    ),
                )),
            })
            .collect()
    }

    let items: Vec<Vec<u8>> = match iterable.kind() {
        ValueKind::List(list_items) => collect_items(&list_items)?,
        ValueKind::Tuple(tuple_items) => collect_items(tuple_items)?,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "can only join an iterable of bytes-like objects".to_string(),
            ));
        }
    };

    if items.is_empty() {
        return Ok(Value::bytes(vec![]));
    }
    let total_len =
        items.iter().map(|b| b.len()).sum::<usize>() + sep.len() * (items.len().saturating_sub(1));
    let mut out: Vec<u8> = Vec::with_capacity(total_len);
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(sep);
        }
        out.extend_from_slice(item);
    }
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// title / capitalize
// ---------------------------------------------------------------------------
