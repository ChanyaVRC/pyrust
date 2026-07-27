/// Like `extract_bytes_arg` but also accepts an integer byte value (0..=255),
/// converting it to a one-byte owned `Vec<u8>` needle.  CPython's `find` and
/// `count` accept integer sub-sequences via this path.  Returns `Err(ValueError)`
/// when the integer is outside `0..=255`.
fn extract_bytes_or_int_arg(arg: &Value) -> Result<std::borrow::Cow<'_, [u8]>> {
    match arg.kind() {
        ValueKind::Bytes(rc) => Ok(std::borrow::Cow::Borrowed(rc.as_slice())),
        ValueKind::Int(n) => {
            let b = u8::try_from(n).map_err(|_| {
                PyError::named("ValueError", "byte must be in range(0, 256)".to_string())
            })?;
            Ok(std::borrow::Cow::Owned(vec![b]))
        }
        ValueKind::Bool(b) => Ok(std::borrow::Cow::Owned(vec![b as u8])),
        // A BigInt is by definition outside 0..=255; CPython reports the same
        // ValueError as an out-of-range plain int rather than a TypeError.
        ValueKind::BigInt(_) => Err(PyError::named(
            "ValueError",
            "byte must be in range(0, 256)".to_string(),
        )),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "argument should be integer or bytes-like object, not '{}'",
                pyrust_core::builtin_type_name(arg),
            ),
        )),
    }
}

fn bytes_startswith(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let prefix_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.startswith() requires at least 1 argument".to_string(),
        )
    })?;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        return Ok(Value::bool_(false));
    };
    let haystack = &bytes[start..end];
    match prefix_val.kind() {
        ValueKind::Bytes(rc) => Ok(Value::bool_(haystack.starts_with(rc.as_slice()))),
        ValueKind::Tuple(prefixes) => {
            for pv in prefixes.iter() {
                match pv.kind() {
                    ValueKind::Bytes(rc) if haystack.starts_with(rc.as_slice()) => {
                        return Ok(Value::bool_(true));
                    }
                    ValueKind::Bytes(_) => {}
                    // Non-bytes tuple element: CPython names the offending
                    // element's type in the canonical message (issue #2044).
                    // bytearray / bytes-subclass elements are pre-coerced to
                    // `Bytes` by the interpreter, so only genuinely-wrong types
                    // reach here.
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "a bytes-like object is required, not '{}'",
                                pyrust_core::builtin_type_name(pv)
                            ),
                        ));
                    }
                }
            }
            Ok(Value::bool_(false))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "startswith first arg must be bytes or a tuple of bytes, not {}",
                pyrust_core::builtin_type_name(prefix_val)
            ),
        )),
    }
}

fn bytes_endswith(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let suffix_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.endswith() requires at least 1 argument".to_string(),
        )
    })?;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        return Ok(Value::bool_(false));
    };
    let haystack = &bytes[start..end];
    match suffix_val.kind() {
        ValueKind::Bytes(rc) => Ok(Value::bool_(haystack.ends_with(rc.as_slice()))),
        ValueKind::Tuple(suffixes) => {
            for sv in suffixes.iter() {
                match sv.kind() {
                    ValueKind::Bytes(rc) if haystack.ends_with(rc.as_slice()) => {
                        return Ok(Value::bool_(true));
                    }
                    ValueKind::Bytes(_) => {}
                    // Non-bytes tuple element: CPython names the offending
                    // element's type in the canonical message (issue #2044).
                    // bytearray / bytes-subclass elements are pre-coerced to
                    // `Bytes` by the interpreter, so only genuinely-wrong types
                    // reach here.
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "a bytes-like object is required, not '{}'",
                                pyrust_core::builtin_type_name(sv)
                            ),
                        ));
                    }
                }
            }
            Ok(Value::bool_(false))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "endswith first arg must be bytes or a tuple of bytes, not {}",
                pyrust_core::builtin_type_name(suffix_val)
            ),
        )),
    }
}

// ---------------------------------------------------------------------------
// find
// ---------------------------------------------------------------------------

fn bytes_find(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.find() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython returns -1 even for empty sub.
        return Ok(Value::int(-1));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: empty sub in a valid (possibly empty) range returns start.
        return Ok(Value::int(start as i64));
    }

    match find_subsequence(haystack, sub) {
        Some(pos) => Ok(Value::int((start + pos) as i64)),
        None => Ok(Value::int(-1)),
    }
}

// ---------------------------------------------------------------------------
// rfind
// ---------------------------------------------------------------------------

fn bytes_rfind(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.rfind() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython returns -1 even for empty sub.
        return Ok(Value::int(-1));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: empty sub rfind in a valid range returns end offset.
        return Ok(Value::int(end as i64));
    }

    match rfind_subsequence(haystack, sub) {
        Some(pos) => Ok(Value::int((start + pos) as i64)),
        None => Ok(Value::int(-1)),
    }
}

// ---------------------------------------------------------------------------
// index
// ---------------------------------------------------------------------------

fn bytes_index(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.index() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython raises ValueError.
        return Err(PyError::named(
            "ValueError",
            "subsection not found".to_string(),
        ));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: empty sub in a valid (possibly empty) range returns start.
        return Ok(Value::int(start as i64));
    }

    match find_subsequence(haystack, sub) {
        Some(pos) => Ok(Value::int((start + pos) as i64)),
        None => Err(PyError::named(
            "ValueError",
            "subsection not found".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// rindex
// ---------------------------------------------------------------------------

fn bytes_rindex(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.rindex() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython raises ValueError.
        return Err(PyError::named(
            "ValueError",
            "subsection not found".to_string(),
        ));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: empty sub rindex in a valid range returns end offset.
        return Ok(Value::int(end as i64));
    }

    match rfind_subsequence(haystack, sub) {
        Some(pos) => Ok(Value::int((start + pos) as i64)),
        None => Err(PyError::named(
            "ValueError",
            "subsection not found".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// count
// ---------------------------------------------------------------------------

fn bytes_count(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.count() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython returns 0 even for empty sub.
        return Ok(Value::int(0));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: count of empty sub = len(haystack) + 1.
        return Ok(Value::int((haystack.len() as i64) + 1));
    }

    let mut count: i64 = 0;
    let mut i = 0;
    while i + sub.len() <= haystack.len() {
        if &haystack[i..i + sub.len()] == sub {
            count += 1;
            i += sub.len();
        } else {
            i += 1;
        }
    }
    Ok(Value::int(count))
}

// ---------------------------------------------------------------------------
