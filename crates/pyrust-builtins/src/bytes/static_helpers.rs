/// `bytes.expandtabs(tabsize=8)`
///
/// Return a copy where tab characters (`\t`, 0x09) are replaced by spaces up
/// to the next tab stop (multiples of `tabsize`).  `\n` and `\r` reset the
/// column counter.  Non-ASCII bytes are treated as single-column characters
/// (matching CPython's behaviour: bytes are opaque octets, not Unicode).
fn bytes_expandtabs(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "expandtabs() takes at most 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let tabsize: i64 = match args.first().map(|v| v.kind()) {
        None => 8,
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C int",
            ));
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    pyrust_core::builtin_type_name(args.first().unwrap())
                ),
            ));
        }
    };
    let tabsize = tabsize.max(0) as usize;
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut col: usize = 0;
    for &b in bytes {
        match b {
            b'\t' => {
                if tabsize > 0 {
                    let spaces = tabsize - (col % tabsize);
                    out.extend(std::iter::repeat_n(b' ', spaces));
                    col += spaces;
                }
                // tabsize == 0: tab is silently removed (col unchanged)
            }
            b'\n' | b'\r' => {
                out.push(b);
                col = 0;
            }
            _ => {
                out.push(b);
                col += 1;
            }
        }
    }
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// fromhex
// ---------------------------------------------------------------------------

/// `bytes.fromhex(string)` — classmethod.
///
/// Decodes a hex string to bytes.  Whitespace between hex pairs is allowed
/// (including leading and trailing whitespace); whitespace within a pair is
/// not.  Raises `ValueError` for invalid hex digits or odd-length non-empty
/// tokens; raises `TypeError` for non-string input.
pub fn bytes_fromhex(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c.is_ascii_whitespace() {
            continue;
        }
        // First hex digit of a pair.
        let hi = c.to_digit(16).ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!("non-hexadecimal number found in fromhex() arg at position {i}"),
            )
        })?;
        // Second hex digit of the pair — must follow immediately (no whitespace allowed mid-pair).
        let (j, c2) = chars.next().ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!(
                    "non-hexadecimal number found in fromhex() arg at position {}",
                    i + 1
                ),
            )
        })?;
        let lo = c2.to_digit(16).ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!("non-hexadecimal number found in fromhex() arg at position {j}"),
            )
        })?;
        out.push((hi * 16 + lo) as u8);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// maketrans
// ---------------------------------------------------------------------------

/// `bytes.maketrans(from_bytes, to_bytes)` — static method.
///
/// Builds a 256-byte translation table where each byte value `from_bytes[i]`
/// maps to `to_bytes[i]`; all other byte values map to themselves.
/// Both arguments must be bytes of the same length.
///
/// CPython implements this as `staticmethod`, so it is callable both as
/// `bytes.maketrans(...)` and on an instance (`b''.maketrans(...)`).
pub fn bytes_maketrans(args: &[Value]) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("maketrans expected 2 arguments, got {}", args.len()),
        ));
    }
    // Accept any bytes-like object (bytes, bytearray, subclasses) per CPython.
    let from_owned: std::borrow::Cow<'_, [u8]> = match args[0].kind() {
        ValueKind::Bytes(rc) => std::borrow::Cow::Borrowed(rc.as_slice()),
        _ => match crate::bytearray::as_bytearray_snapshot(&args[0]) {
            Some(data) => std::borrow::Cow::Owned(data),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "a bytes-like object is required, not '{}'",
                        pyrust_core::builtin_type_name(&args[0])
                    ),
                ));
            }
        },
    };
    let to_owned: std::borrow::Cow<'_, [u8]> = match args[1].kind() {
        ValueKind::Bytes(rc) => std::borrow::Cow::Borrowed(rc.as_slice()),
        _ => match crate::bytearray::as_bytearray_snapshot(&args[1]) {
            Some(data) => std::borrow::Cow::Owned(data),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "a bytes-like object is required, not '{}'",
                        pyrust_core::builtin_type_name(&args[1])
                    ),
                ));
            }
        },
    };
    let from: &[u8] = from_owned.as_ref();
    let to: &[u8] = to_owned.as_ref();
    if from.len() != to.len() {
        return Err(PyError::named(
            "ValueError",
            "maketrans arguments must have same length".to_string(),
        ));
    }
    // Build the identity table then apply the mapping.
    let mut table: Vec<u8> = (0u8..=255).collect();
    for (&f, &t) in from.iter().zip(to.iter()) {
        table[f as usize] = t;
    }
    Ok(Value::bytes(table))
}
