use pyrust_core::{PyError, Result, Value, ValueKind};

/// Canonical list of method names dispatched by `call`.
/// Single source of truth for `has_method` and the drift-guard test.
pub const METHODS: &[&str] = &[
    "hex",
    "decode",
    "startswith",
    "endswith",
    "find",
    "count",
    "upper",
    "lower",
];

/// Returns `true` if `method` is the name of a built-in `bytes` method.
/// Used by `hasattr` / `getattr` to validate attribute names without
/// invoking the method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

pub fn call(method: &str, receiver: &Value, args: &[Value]) -> Result<Value> {
    let bytes: &[u8] = match receiver.kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "expected bytes receiver, got {}",
                    pyrust_core::builtin_type_name(receiver)
                ),
            ));
        }
    };
    match method {
        "hex" => bytes_hex(bytes, args),
        "decode" => bytes_decode(bytes, args),
        "startswith" => bytes_startswith(bytes, args),
        "endswith" => bytes_endswith(bytes, args),
        "find" => bytes_find(bytes, args),
        "count" => bytes_count(bytes, args),
        "upper" => Ok(Value::bytes(
            bytes.iter().map(|b| b.to_ascii_uppercase()).collect(),
        )),
        "lower" => Ok(Value::bytes(
            bytes.iter().map(|b| b.to_ascii_lowercase()).collect(),
        )),
        _ => Err(PyError::named(
            "AttributeError",
            format!("'bytes' object has no attribute '{method}'"),
        )),
    }
}

// ---------------------------------------------------------------------------
// hex
// ---------------------------------------------------------------------------

/// `bytes.hex(sep=None, bytes_per_sep=1)`
///
/// Without a separator: return a hex string of all bytes.
/// With a separator string: insert it between every `bytes_per_sep` bytes
/// (default 1, positive means group from left, negative from right — only
/// the simple case of one-byte groups is implemented here, matching CPython
/// 3.12's most common usage).
fn bytes_hex(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.is_empty() {
        // Fast path: no separator.
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
        }
        return Ok(Value::string(out));
    }

    // args[0] = sep (str), args[1] = bytes_per_sep (int, default 1)
    let sep: &str = match args[0].kind() {
        ValueKind::Str(s) => s,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.hex() separator must be a str".to_string(),
            ));
        }
    };

    // Validate separator: CPython requires exactly one ASCII character.
    let sep_chars = sep.chars().count();
    if sep_chars != 1 {
        return Err(PyError::named(
            "ValueError",
            "sep must be length 1.".to_string(),
        ));
    }
    // CPython 3.12 also requires the separator to be ASCII.
    if !sep.is_ascii() {
        return Err(PyError::named(
            "ValueError",
            "sep must be ASCII.".to_string(),
        ));
    }

    let bytes_per_sep: i64 = match args.get(1).map(|v| v.kind()) {
        None => 1,
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.hex() bytes_per_sep must be an integer".to_string(),
            ));
        }
    };

    if bytes_per_sep == 0 {
        // CPython 3.12: bytes_per_sep=0 means "no separator" — returns plain hex.
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
        }
        return Ok(Value::string(out));
    }

    // Group the hex nibbles according to bytes_per_sep.
    //
    // CPython semantics (from the C source):
    //   Positive bytes_per_sep: separator inserted from the RIGHT — rightmost
    //   groups have exactly `bytes_per_sep` bytes; the leftmost group gets the
    //   remainder (may be shorter).  e.g. b'\x00\x01\x02\x03\x04'.hex(':', 2)
    //   → "00:0102:0304"
    //
    //   Negative bytes_per_sep: separator inserted from the LEFT — leftmost
    //   groups have exactly `|bytes_per_sep|` bytes; the rightmost group gets
    //   the remainder.  e.g. b'\x00\x01\x02\x03\x04'.hex(':', -2)
    //   → "0001:0203:04"
    let n = bytes.len();
    if n == 0 {
        return Ok(Value::string("".to_string()));
    }

    // Render all bytes as hex pairs first.
    let hex_pairs: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let group_size = bytes_per_sep.unsigned_abs() as usize;
    let mut groups: Vec<String> = Vec::new();

    if bytes_per_sep > 0 {
        // Group from right: the first group may be smaller (the remainder).
        let rem = n % group_size;
        let mut i = 0;
        if rem > 0 {
            groups.push(hex_pairs[0..rem].concat());
            i = rem;
        }
        while i < n {
            let end = i + group_size;
            groups.push(hex_pairs[i..end].concat());
            i = end;
        }
    } else {
        // Group from left: the last group may be smaller (the remainder).
        let mut i = 0;
        while i < n {
            let end = (i + group_size).min(n);
            groups.push(hex_pairs[i..end].concat());
            i = end;
        }
    }

    Ok(Value::string(groups.join(sep)))
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

fn bytes_decode(bytes: &[u8], args: &[Value]) -> Result<Value> {
    // Signature: decode(encoding='utf-8', errors='strict')
    let encoding: &str = match args.first().map(|v| v.kind()) {
        None => "utf-8",
        Some(ValueKind::Str(s)) => s,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.decode() encoding must be a str".to_string(),
            ));
        }
    };

    let errors: &str = match args.get(1).map(|v| v.kind()) {
        None => "strict",
        Some(ValueKind::Str(s)) => s,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.decode() errors must be a str".to_string(),
            ));
        }
    };

    // Normalise encoding name (strip hyphens/underscores, lowercase).
    let enc_norm: String = encoding
        .to_ascii_lowercase()
        .chars()
        .filter(|&c| c != '-' && c != '_')
        .collect();

    match enc_norm.as_str() {
        "utf8" => match errors {
            "strict" => match std::str::from_utf8(bytes) {
                Ok(s) => Ok(Value::string(s)),
                Err(e) => Err(PyError::named(
                    "UnicodeDecodeError",
                    format!(
                        "'utf-8' codec can't decode byte 0x{:02x} in position {}: invalid start byte",
                        bytes[e.valid_up_to()],
                        e.valid_up_to()
                    ),
                )),
            },
            "ignore" => {
                let s = bytes_decode_utf8_ignore(bytes);
                Ok(Value::string(&s))
            }
            "replace" => Ok(Value::string(String::from_utf8_lossy(bytes).as_ref())),
            _ => Err(PyError::named(
                "LookupError",
                format!("unknown error handler name '{errors}'"),
            )),
        },
        "latin1" | "iso88591" | "iso8859" => {
            // Latin-1: byte N maps directly to Unicode code point N.
            let s: String = bytes.iter().map(|&b| b as char).collect();
            Ok(Value::string(&s))
        }
        "ascii" => {
            match errors {
                "strict" => {
                    // Every byte must be in 0x00..=0x7F.
                    for (i, &b) in bytes.iter().enumerate() {
                        if b > 0x7F {
                            return Err(PyError::named(
                                "UnicodeDecodeError",
                                format!(
                                    "'ascii' codec can't decode byte 0x{b:02x} in position {i}: ordinal not in range(128)"
                                ),
                            ));
                        }
                    }
                    // SAFETY: all bytes validated as ASCII.
                    Ok(Value::string(unsafe {
                        std::str::from_utf8_unchecked(bytes)
                    }))
                }
                "ignore" => {
                    let s: String = bytes
                        .iter()
                        .filter(|&&b| b <= 0x7F)
                        .map(|&b| b as char)
                        .collect();
                    Ok(Value::string(&s))
                }
                "replace" => {
                    let s: String = bytes
                        .iter()
                        .map(|&b| if b <= 0x7F { b as char } else { '\u{FFFD}' })
                        .collect();
                    Ok(Value::string(&s))
                }
                _ => Err(PyError::named(
                    "LookupError",
                    format!("unknown error handler name '{errors}'"),
                )),
            }
        }
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {encoding}"),
        )),
    }
}

/// Decode UTF-8 bytes, skipping any invalid byte sequences (errors='ignore').
fn bytes_decode_utf8_ignore(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match std::str::from_utf8(&bytes[i..]) {
            Ok(s) => {
                out.push_str(s);
                break;
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                // SAFETY: validated by from_utf8.
                out.push_str(unsafe { std::str::from_utf8_unchecked(&bytes[i..i + valid_up_to]) });
                // Skip the invalid byte(s): error_len() is None for truncated sequence.
                let skip = e.error_len().unwrap_or(1);
                i += valid_up_to + skip;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// startswith / endswith
// ---------------------------------------------------------------------------

fn extract_bytes_arg<'a>(arg: &'a Value) -> Result<&'a [u8]> {
    match arg.kind() {
        ValueKind::Bytes(rc) => Ok(rc.as_slice()),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
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
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "a bytes-like object is required, not a non-bytes item in the tuple"
                                .to_string(),
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
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "a bytes-like object is required, not a non-bytes item in the tuple"
                                .to_string(),
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
    let sub = extract_bytes_arg(sub_val)?;
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
// count
// ---------------------------------------------------------------------------

fn bytes_count(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.count() requires at least 1 argument".to_string(),
        )
    })?;
    let sub = extract_bytes_arg(sub_val)?;
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
// Helpers
// ---------------------------------------------------------------------------

/// Parse start/end slice args (args[1], args[2]) into byte offsets, clamped to
/// `[0, len]`.  Returns `Ok(Some((start, end)))` for a valid (possibly empty)
/// range, or `Ok(None)` when `end < start` after clamping — an inverted range
/// that CPython treats as "empty with no match" (find → -1, count → 0,
/// startswith/endswith → False).
fn bytes_slice_args(len: usize, args: &[Value]) -> Result<Option<(usize, usize)>> {
    let start: usize = match args.get(1).map(|v| v.kind()) {
        None => 0,
        Some(ValueKind::Int(i)) => normalise_idx(i, len),
        Some(ValueKind::Bool(b)) => normalise_idx(b as i64, len),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers".to_string(),
            ));
        }
    };
    let end: usize = match args.get(2).map(|v| v.kind()) {
        None => len,
        Some(ValueKind::Int(i)) => normalise_idx(i, len).min(len),
        Some(ValueKind::Bool(b)) => normalise_idx(b as i64, len).min(len),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers".to_string(),
            ));
        }
    };
    let start = start.min(len);
    // Don't clamp end up to start: an inverted range (end < start) must be
    // distinguished from a genuinely empty range (end == start) because
    // CPython returns -1/0/False for inverted ranges but a valid index for
    // empty ranges at the boundary.
    if end < start {
        Ok(None)
    } else {
        Ok(Some((start, end)))
    }
}

fn normalise_idx(idx: i64, len: usize) -> usize {
    if idx < 0 {
        let from_end = (-idx) as usize;
        len.saturating_sub(from_end)
    } else {
        idx as usize
    }
}

/// Naive substring search: find first occurrence of `sub` in `haystack`,
/// returning the starting byte index, or `None` if not found.
fn find_subsequence(haystack: &[u8], sub: &[u8]) -> Option<usize> {
    if sub.len() > haystack.len() {
        return None;
    }
    haystack.windows(sub.len()).position(|w| w == sub)
}
