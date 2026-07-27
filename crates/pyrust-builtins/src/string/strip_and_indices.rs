fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = String::with_capacity(s.len());
            // CPython titlecases the first character (so Lt digraphs become their
            // titlecase form, e.g. "ǆabc".capitalize() == "ǅabc"), then lowercases
            // the remainder.
            push_titlecase(&mut out, first);
            out.extend(chars.as_str().chars().flat_map(char::to_lowercase));
            out
        }
    }
}

/// Linear membership avoids building a bitmap/hash set for short `chars`.
const STRIP_LINEAR_SCAN_MAX: usize = 8;

fn strip_chars(
    src: &Value,
    s: &str,
    args: &[Value],
    left: bool,
    right: bool,
    method: &str,
) -> Result<Value> {
    let chars_arg: Option<&str> = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(c)) => Some(c),
        Some(ValueKind::None) | None => None,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!("{method} arg must be None or str"),
            ));
        }
    };
    let result = match chars_arg {
        None => {
            let mut result = s;
            if left {
                result = result.trim_start();
            }
            if right {
                result = result.trim_end();
            }
            result
        }
        Some(chars) => {
            let mut result = s;
            if chars.is_empty() {
                // Nothing can be stripped.
            } else if chars.len() == 1 {
                // A one-byte UTF-8 string is necessarily ASCII. Keep this
                // common case below the adaptive-set setup cost.
                let needle = chars.as_bytes()[0] as char;
                if left {
                    result = result.trim_start_matches(needle);
                }
                if right {
                    result = result.trim_end_matches(needle);
                }
            } else if chars.len() <= STRIP_LINEAR_SCAN_MAX
                || chars.chars().nth(STRIP_LINEAR_SCAN_MAX).is_none()
            {
                if left {
                    result = result.trim_start_matches(|c: char| chars.contains(c));
                }
                if right {
                    result = result.trim_end_matches(|c: char| chars.contains(c));
                }
            } else if chars.is_ascii() {
                let mut bitmap = [0u64; 2];
                for byte in chars.bytes() {
                    bitmap[(byte >> 6) as usize] |= 1u64 << (byte & 63);
                }
                let contains = |c: char| {
                    c.is_ascii() && bitmap[(c as u8 >> 6) as usize] & (1u64 << (c as u8 & 63)) != 0
                };
                if left {
                    result = result.trim_start_matches(contains);
                }
                if right {
                    result = result.trim_end_matches(contains);
                }
            } else {
                let set: HashSet<char> = chars.chars().collect();
                if left {
                    result = result.trim_start_matches(|c: char| set.contains(&c));
                }
                if right {
                    result = result.trim_end_matches(|c: char| set.contains(&c));
                }
            }
            result
        }
    };
    if result.len() == s.len() {
        return Ok(src.clone());
    }
    let start = result.as_ptr() as usize - s.as_ptr() as usize;
    Ok(src.string_slice(start, start + result.len()))
}

/// Parse (sep, maxsplit) from split/rsplit args.
fn split_args(args: &[Value]) -> Result<(Option<&str>, i64)> {
    let sep = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(s)) => Some(s),
        Some(ValueKind::None) | None => None,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "must be str or None, not {}",
                    builtin_type_name(args.first().unwrap())
                ),
            ));
        }
    };
    let maxsplit = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        None => -1,
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            ));
        }
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    builtin_type_name(&args[1])
                ),
            ));
        }
    };
    Ok((sep, maxsplit))
}

/// Convert a byte offset into a char (code-point) index within `s`.
///
/// ASCII fast path (#2032): when the prefix `s[..byte_off]` is all-ASCII the
/// char index equals the byte offset, so no scan is needed.  `is_ascii()` is
/// SIMD-accelerated and far cheaper than decoding via `chars().count()`.
#[inline]
fn byte_to_char_idx(s: &str, is_ascii: bool, byte_off: usize) -> usize {
    // When the whole string is ASCII (cached, #2124) the prefix is too, so the
    // char index is the byte offset with no scan.  Otherwise fall back to the
    // prefix `is_ascii()` check before decoding.
    if is_ascii {
        return byte_off;
    }
    let prefix = &s[..byte_off];
    if prefix.is_ascii() {
        byte_off
    } else {
        prefix.chars().count()
    }
}

/// Convert char-based start/end args (args[1], args[2]) to byte offsets.
///
/// Returns `Ok(None)` when the requested window is inverted (`start > stop`
/// after clamping to string bounds). Callers must treat `None` as an empty
/// search range (return -1 / 0 / raise ValueError as appropriate).
/// This matches CPython's `adjust_indices` contract — an inverted window is
/// distinct from a zero-length equal window (`start == stop`), which is
/// represented as `Some((n, n))`.
fn str_slice_args(s: &str, is_ascii: bool, args: &[Value]) -> Result<Option<(usize, usize)>> {
    // Fast path: no start/end args — common case for find/startswith/etc.
    let has_start = args.get(1).is_some();
    let has_end = args.get(2).is_some();
    if !has_start && !has_end {
        return Ok(Some((0, s.len())));
    }

    // ASCII fast path: char index == byte index, no scanning needed.  `is_ascii`
    // is the O(1) cached flag (#2124) when available; otherwise the caller passes
    // `s.is_ascii()` directly.
    if is_ascii {
        let byte_len = s.len();
        // Do NOT clamp start before the inverted-window check: if the caller
        // passes start > len(s), that must produce None (not found / 0 count),
        // not a zero-length window at the end.  Mirror the Unicode path which
        // defers the start clamp until after the end_char < start_char test.
        let start_char = match args.get(1).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => normalise_char_idx(i, byte_len),
            Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, byte_len),
            Some(ValueKind::BigInt(n)) => bigint_start_idx(n, byte_len),
            Some(ValueKind::None) | None => 0,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "slice indices must be integers or None or have an __index__ method",
                ));
            }
        };
        let end_char = match args.get(2).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => normalise_char_idx(i, byte_len).min(byte_len),
            Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, byte_len).min(byte_len),
            Some(ValueKind::BigInt(n)) => bigint_end_idx(n, byte_len),
            Some(ValueKind::None) | None => byte_len,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "slice indices must be integers or None or have an __index__ method",
                ));
            }
        };
        if end_char < start_char {
            return Ok(None);
        }
        return Ok(Some((start_char.min(byte_len), end_char)));
    }

    // Unicode: single scan for char_len + both byte positions
    let char_len = s.chars().count();
    let start_char = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_char_idx(i, char_len),
        Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, char_len),
        Some(ValueKind::BigInt(n)) => bigint_start_idx(n, char_len),
        Some(ValueKind::None) | None => 0,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or None or have an __index__ method",
            ));
        }
    };
    let end_char = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_char_idx(i, char_len).min(char_len),
        Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, char_len).min(char_len),
        Some(ValueKind::BigInt(n)) => bigint_end_idx(n, char_len),
        Some(ValueKind::None) | None => char_len,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or None or have an __index__ method",
            ));
        }
    };
    // Inverted window: start > stop after normalisation — signal to caller.
    if end_char < start_char {
        return Ok(None);
    }
    // Clamp start_char to char_len so the single-pass loop terminates correctly.
    let start_char = start_char.min(char_len);
    // Single pass to find both byte positions
    let mut start_byte = s.len();
    let mut end_byte = s.len();
    for (i, (b, _)) in s.char_indices().enumerate() {
        if i == start_char {
            start_byte = b;
        }
        if i == end_char {
            end_byte = b;
            break;
        }
    }
    Ok(Some((start_byte, end_byte)))
}

fn normalise_char_idx(idx: i64, len: usize) -> usize {
    if idx < 0 {
        let from_end = (-idx) as usize;
        len.saturating_sub(from_end)
    } else {
        idx as usize
    }
}

/// Normalise a `BigInt` `start` bound for a search window. A BigInt never fits
/// in an index range, so CPython clamps it: a negative one to the start (`0`), a
/// positive one to just past the end (`len + 1`) so the inverted-window check in
/// the caller reports "not found" rather than a zero-length window (#2688).
fn bigint_start_idx(n: &pyrust_core::PyBigInt, len: usize) -> usize {
    match n.sign() {
        PyBigIntSign::Minus => 0,
        _ => len + 1,
    }
}

/// Normalise a `BigInt` `end` bound for a search window: a negative one clamps to
/// the start (`0`), a positive one to the end (`len`) (#2688).
fn bigint_end_idx(n: &pyrust_core::PyBigInt, len: usize) -> usize {
    match n.sign() {
        PyBigIntSign::Minus => 0,
        _ => len,
    }
}

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------
