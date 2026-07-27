fn bytes_replace(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            "bytes.replace() requires at least 2 arguments".to_string(),
        ));
    }
    let old: &[u8] = match args[0].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[0])
                ),
            ));
        }
    };
    let new: &[u8] = match args[1].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[1])
                ),
            ));
        }
    };
    let count: i64 = match args.get(2).map(|v| v.kind()) {
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
                "bytes.replace() count must be an integer".to_string(),
            ));
        }
    };

    if count == 0 {
        return Ok(Value::bytes(bytes.to_vec()));
    }

    let max_replacements: usize = if count < 0 {
        usize::MAX
    } else {
        count as usize
    };

    if old.is_empty() {
        // CPython inserts `new` between every byte and at start/end when old is empty.
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len() + new.len() * (bytes.len() + 1));
        let mut replacements = 0usize;
        if replacements < max_replacements {
            out.extend_from_slice(new);
            replacements += 1;
        }
        for &b in bytes {
            out.push(b);
            if replacements < max_replacements {
                out.extend_from_slice(new);
                replacements += 1;
            }
        }
        return Ok(Value::bytes(out));
    }

    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut replacements = 0usize;
    while i + old.len() <= bytes.len() {
        if replacements >= max_replacements {
            break;
        }
        if bytes[i..].starts_with(old) {
            out.extend_from_slice(new);
            i += old.len();
            replacements += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // Append the remaining bytes.
    out.extend_from_slice(&bytes[i..]);
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// strip / lstrip / rstrip
// ---------------------------------------------------------------------------

/// ASCII whitespace bytes: space, tab, newline, carriage return, vertical tab, form feed.
fn is_ascii_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Linear membership avoids building a bitmap for short `chars`.
const STRIP_LINEAR_SCAN_MAX: usize = 8;

fn bytes_strip(bytes: &[u8], args: &[Value], left: bool, right: bool) -> Result<Value> {
    let chars_owned: Option<std::borrow::Cow<'_, [u8]>> = match args.first() {
        None => None,
        Some(v) => match v.kind() {
            ValueKind::None => None,
            ValueKind::Bytes(rc) => Some(std::borrow::Cow::Borrowed(rc.as_slice())),
            // Accept any bytes-like object (bytearray, ...) per CPython; reject
            // genuinely-wrong types (str, int, ...) with CPython's message.
            _ => match crate::bytearray::as_bytearray_snapshot(v) {
                Some(data) => Some(std::borrow::Cow::Owned(data)),
                None => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "a bytes-like object is required, not '{}'",
                            pyrust_core::builtin_type_name(v)
                        ),
                    ));
                }
            },
        },
    };
    let chars_arg: Option<&[u8]> = chars_owned.as_deref();
    let mut start = 0;
    let mut end = bytes.len();
    match chars_arg {
        None => {
            if left {
                while start < end && is_ascii_whitespace(bytes[start]) {
                    start += 1;
                }
            }
            if right {
                while end > start && is_ascii_whitespace(bytes[end - 1]) {
                    end -= 1;
                }
            }
        }
        Some(chars) => {
            if chars.is_empty() {
                // Nothing can be stripped.
            } else if chars.len() == 1 {
                let needle = chars[0];
                if left {
                    while start < end && bytes[start] == needle {
                        start += 1;
                    }
                }
                if right {
                    while end > start && bytes[end - 1] == needle {
                        end -= 1;
                    }
                }
            } else if chars.len() <= STRIP_LINEAR_SCAN_MAX {
                if left {
                    while start < end && chars.contains(&bytes[start]) {
                        start += 1;
                    }
                }
                if right {
                    while end > start && chars.contains(&bytes[end - 1]) {
                        end -= 1;
                    }
                }
            } else {
                let mut bitmap = [0u64; 4];
                for &byte in chars {
                    bitmap[(byte >> 6) as usize] |= 1u64 << (byte & 63);
                }
                let contains = |byte: u8| bitmap[(byte >> 6) as usize] & (1u64 << (byte & 63)) != 0;
                if left {
                    while start < end && contains(bytes[start]) {
                        start += 1;
                    }
                }
                if right {
                    while end > start && contains(bytes[end - 1]) {
                        end -= 1;
                    }
                }
            }
        }
    }
    Ok(Value::bytes(bytes[start..end].to_vec()))
}

// ---------------------------------------------------------------------------
// removeprefix / removesuffix
// ---------------------------------------------------------------------------

fn bytes_removeprefix(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "bytes.removeprefix() takes exactly one argument (0 given)".to_string(),
        ));
    }
    let prefix: &[u8] = match args[0].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[0])
                ),
            ));
        }
    };
    if bytes.starts_with(prefix) {
        Ok(Value::bytes(bytes[prefix.len()..].to_vec()))
    } else {
        Ok(Value::bytes(bytes.to_vec()))
    }
}

fn bytes_removesuffix(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "bytes.removesuffix() takes exactly one argument (0 given)".to_string(),
        ));
    }
    let suffix: &[u8] = match args[0].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[0])
                ),
            ));
        }
    };
    if bytes.ends_with(suffix) {
        Ok(Value::bytes(bytes[..bytes.len() - suffix.len()].to_vec()))
    } else {
        Ok(Value::bytes(bytes.to_vec()))
    }
}

// ---------------------------------------------------------------------------
// split / rsplit
// ---------------------------------------------------------------------------
