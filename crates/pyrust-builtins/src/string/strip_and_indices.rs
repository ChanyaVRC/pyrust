fn capitalize(s: &str) -> String {
    // CPython titlecases the first character (so Lt digraphs become their
    // titlecase form, e.g. "ǆabc".capitalize() == "ǅabc"), then lowercases the
    // remainder.  Lone surrogates have no case mapping and are copied through,
    // so only the surrogate-free runs reach the `char` APIs (#3315).
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for seg in str_segments(s) {
        match seg {
            // A leading surrogate is "titlecased" to itself.
            StrSegment::Surrogate { raw, .. } => out.push_str(raw),
            StrSegment::Text(text) => {
                let mut chars = text.chars();
                if first && let Some(c) = chars.next() {
                    push_titlecase(&mut out, c);
                }
                out.extend(chars.flat_map(char::to_lowercase));
            }
        }
        first = false;
    }
    out
}

/// Linear membership avoids building a hash set for short `chars`.
const STRIP_LINEAR_SCAN_MAX: usize = 8;

/// Byte range of `s` left after stripping the codepoints matching `pred` from
/// the requested ends.
///
/// Walks codepoints inward from each end with the surrogate-safe cursor
/// decoders, so the cost is proportional to what is stripped and a lone
/// surrogate is an ordinary codepoint (std's `trim*` would decode it into an
/// invalid `char`, #3315).  ASCII bytes decode in one comparison.
#[inline]
fn strip_span(s: &str, left: bool, right: bool, pred: impl Fn(u32) -> bool) -> (usize, usize) {
    let mut start = 0;
    let mut end = s.len();
    if left {
        while let Some((cp, next)) = cesu8_next_codepoint(s, start) {
            if !pred(cp) {
                break;
            }
            start = next;
        }
    }
    if right {
        while end > start {
            match cesu8_prev_codepoint(s, end) {
                Some((cp, prev)) if pred(cp) => end = prev,
                _ => break,
            }
        }
    }
    (start, end)
}

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
    let (start, end) = match chars_arg {
        // CPython strips `Py_UNICODE_ISSPACE` characters, the same set as
        // `str.isspace()` (it includes `\x1c`–`\x1f`, unlike Rust's `trim`).
        None => strip_span(s, left, right, is_python_space_cp),
        // Nothing can be stripped.
        Some("") => (0, s.len()),
        Some(chars) if chars.len() == 1 => {
            // A one-byte string is necessarily ASCII. Keep this common case
            // below the membership-set setup cost.
            let needle = u32::from(chars.as_bytes()[0]);
            strip_span(s, left, right, |cp| cp == needle)
        }
        Some(chars) if chars.is_ascii() => {
            let mut bitmap = [0u64; 2];
            for byte in chars.bytes() {
                bitmap[(byte >> 6) as usize] |= 1u64 << (byte & 63);
            }
            strip_span(s, left, right, |cp| {
                cp < 0x80 && bitmap[(cp >> 6) as usize] & (1u64 << (cp & 63)) != 0
            })
        }
        Some(chars) => {
            // Collect codepoints (not `char`s) so lone surrogates in `chars`
            // are matchable members too.
            let set: smallvec::SmallVec<[u32; STRIP_LINEAR_SCAN_MAX]> =
                cesu8_codepoints(chars).collect();
            if set.len() <= STRIP_LINEAR_SCAN_MAX {
                strip_span(s, left, right, |cp| set.contains(&cp))
            } else {
                let set: HashSet<u32> = set.into_iter().collect();
                strip_span(s, left, right, |cp| set.contains(&cp))
            }
        }
    };
    if end - start == s.len() {
        return Ok(src.clone());
    }
    Ok(src.string_slice(start, end))
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
        str_codepoint_count(prefix)
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

    // Unicode: count codepoints by their lead bytes (never decoding a `char`,
    // so lone surrogates are safe, #3315), then one scan for both byte offsets.
    let char_len = str_codepoint_count(s);
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
    // Single pass over the codepoint lead bytes to find both byte positions.
    let mut start_byte = s.len();
    let mut end_byte = s.len();
    let mut i = 0usize;
    for (b, &byte) in s.as_bytes().iter().enumerate() {
        if byte & 0xC0 == 0x80 {
            continue;
        }
        if i == start_char {
            start_byte = b;
        }
        if i == end_char {
            end_byte = b;
            break;
        }
        i += 1;
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
