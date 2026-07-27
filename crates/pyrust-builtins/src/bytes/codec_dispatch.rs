/// Decode `bytes` using the given `encoding` and `errors` handler.
///
/// Shared implementation for `bytes.decode()` and the 2/3-arg form of
/// `str(bytes, encoding[, errors])`.
///
/// Supported encodings: `utf-8` (and aliases), `utf-8-sig`, `latin-1` (and
/// aliases), `ascii`, `utf-16` (LE/BE/BOM), `utf-32` (LE/BE/BOM).
/// Supported error handlers: `strict`, `replace`, `ignore`,
/// `backslashreplace`, `surrogateescape`.
pub fn decode_bytes(bytes: &[u8], encoding: &str, errors: &str) -> Result<Value> {
    // Normalise encoding name (strip hyphens/underscores, lowercase).
    let enc_norm: String = encoding
        .to_ascii_lowercase()
        .chars()
        .filter(|&c| c != '-' && c != '_')
        .collect();

    match enc_norm.as_str() {
        "utf8" => {
            // Fast path: if all bytes are valid UTF-8 the error handler is never
            // invoked, so we must not validate its name (CPython is lazy here).
            match std::str::from_utf8(bytes) {
                Ok(s) => Ok(Value::string(s)),
                Err(_) => decode_utf8_with_errors(bytes, errors, "utf-8"),
            }
        }
        // UTF-8-SIG: strip leading BOM (U+FEFF encoded as EF BB BF) if present.
        "utf8sig" => {
            let payload = if bytes.starts_with(b"\xef\xbb\xbf") {
                &bytes[3..]
            } else {
                bytes
            };
            match std::str::from_utf8(payload) {
                Ok(s) => Ok(Value::string(s)),
                Err(_) => decode_utf8_with_errors(payload, errors, "utf-8-sig"),
            }
        }
        // Latin-1 and its many aliases: byte N → Unicode code point N.
        // This encoding never fails, so the error handler is never invoked.
        "latin1" | "iso88591" | "iso8859" | "l1" | "cp819" | "latin" => {
            let s: String = bytes.iter().map(|&b| b as char).collect();
            Ok(Value::string(&s))
        }
        "ascii" => {
            // Find the first non-ASCII byte, if any.
            let has_bad = bytes.iter().any(|&b| b > 0x7F);
            if !has_bad {
                // All bytes are valid ASCII; error handler is never invoked.
                // SAFETY: all bytes validated as ASCII (≤ 0x7F, valid UTF-8).
                Ok(Value::string(unsafe {
                    std::str::from_utf8_unchecked(bytes)
                }))
            } else {
                decode_ascii_with_errors(bytes, errors)
            }
        }
        // UTF-16 with BOM detection: first two bytes are the BOM (\xff\xfe for LE,
        // \xfe\xff for BE).  If absent, default to little-endian (matches x86/x64/ARM64).
        //
        // `bytes` (the full original slice including the BOM) and the BOM byte count are
        // passed as `original_bytes`/`bom_offset` so that any UnicodeDecodeError carries
        // the full original bytes as `.object` with `start`/`end` adjusted past the BOM —
        // matching CPython's behaviour (see issues #1781, #1813).
        "utf16" => {
            if bytes.starts_with(b"\xff\xfe") {
                decode_utf16_le(&bytes[2..], bytes, 2, errors)
            } else if bytes.starts_with(b"\xfe\xff") {
                decode_utf16_be(&bytes[2..], bytes, 2, errors)
            } else {
                decode_utf16_le(bytes, bytes, 0, errors)
            }
        }
        "utf16le" => decode_utf16_le(bytes, bytes, 0, errors),
        "utf16be" => decode_utf16_be(bytes, bytes, 0, errors),
        // UTF-32 with BOM detection: first four bytes are the BOM.
        "utf32" => {
            if bytes.starts_with(b"\xff\xfe\x00\x00") {
                decode_utf32_le(&bytes[4..], bytes, 4, errors)
            } else if bytes.starts_with(b"\x00\x00\xfe\xff") {
                decode_utf32_be(&bytes[4..], bytes, 4, errors)
            } else {
                decode_utf32_le(bytes, bytes, 0, errors)
            }
        }
        "utf32le" => decode_utf32_le(bytes, bytes, 0, errors),
        "utf32be" => decode_utf32_be(bytes, bytes, 0, errors),
        "cp1252" | "windows1252" => decode_cp1252(bytes, errors),
        "unicodeescape" => decode_unicode_escape(bytes, errors),
        "rawunicodeescape" => decode_raw_unicode_escape(bytes, errors),
        "utf7" => decode_utf7(bytes, errors),
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {encoding}"),
        )),
    }
}

/// Build a Python `str` from codepoints that may include lone surrogates,
/// using the CESU-8-aware encoder so `char::from_u32` is never called on a
/// surrogate (which would abort in debug builds).
fn string_from_codepoints(cps: &[u32]) -> Value {
    let mut s = String::new();
    for &cp in cps {
        s.push_str(&pyrust_core::cesu8_encode_codepoint(cp));
    }
    Value::string(s)
}

/// Decode CP1252 (Windows-1252) bytes, honouring the `errors` handler.
/// Undefined bytes (0x81/0x8D/0x8F/0x90/0x9D) raise with CPython's `charmap`
/// reason "character maps to <undefined>".
fn decode_cp1252(bytes: &[u8], errors: &str) -> Result<Value> {
    let mut out = String::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        let b = bytes[idx];
        if let Some(cp) = crate::string::cp1252_decode_codepoint(b) {
            // cp1252 maps only to scalar values, never surrogates.
            out.push(char::from_u32(cp).expect("cp1252 maps to a scalar value"));
            idx += 1;
            continue;
        }
        match errors {
            "ignore" => idx += 1,
            "replace" => {
                out.push('\u{FFFD}');
                idx += 1;
            }
            "strict" => {
                return Err(PyError::UnicodeDecodeError {
                    encoding: "charmap".to_string(),
                    object: bytes.to_vec(),
                    start: idx,
                    end: idx + 1,
                    reason: "character maps to <undefined>".to_string(),
                });
            }
            other => {
                return Err(PyError::named(
                    "LookupError",
                    format!("unknown error handler name '{other}'"),
                ));
            }
        }
    }
    Ok(Value::string(out))
}

/// Parse exactly `n` hex digits starting at `start`.
///
/// Returns `Ok(value)` when `n` valid hex digits are present, otherwise
/// `Err(consumed)` where `consumed` is the count of leading valid hex digits
/// (so callers can report CPython's truncated-escape end position).
fn parse_hex_escape(bytes: &[u8], start: usize, n: usize) -> std::result::Result<u32, usize> {
    let mut v: u32 = 0;
    for k in 0..n {
        match bytes.get(start + k).and_then(|b| (*b as char).to_digit(16)) {
            Some(d) => v = v * 16 + d,
            None => return Err(k),
        }
    }
    Ok(v)
}
