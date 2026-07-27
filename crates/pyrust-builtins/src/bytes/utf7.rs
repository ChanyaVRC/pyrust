/// Modified-UTF-7 base64 alphabet value for a byte, or `None` if not a base64
/// character.
fn utf7_base64_value(b: u8) -> Option<u32> {
    match b {
        b'A'..=b'Z' => Some((b - b'A') as u32),
        b'a'..=b'z' => Some((b - b'a' + 26) as u32),
        b'0'..=b'9' => Some((b - b'0' + 52) as u32),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode a run of modified-UTF-7 base64 characters into UTF-16 code units.
///
/// Bits are accumulated 6-per-char and a 16-bit unit is emitted every 16 bits.
/// On a malformed shift sequence returns the CPython error reason:
/// - "partial character in shift sequence" when >= 6 unused bits remain (a whole
///   base64 char's worth that can't complete a unit);
/// - "non-zero padding bits in shift sequence" when the leftover (< 6) padding
///   bits are not all zero.
///
/// Always returns the complete (16-bit) units decoded; `Err(reason)` indicates a
/// malformed tail, but the already-decoded `units` are still returned so the
/// non-strict error handlers can keep them (matching CPython, e.g.
/// `b'+ABC-'.decode('utf-7','replace') == '\x10�'`).
fn utf7_base64_decode(b64: &[u8]) -> (Vec<u16>, Option<&'static str>) {
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    let mut units: Vec<u16> = Vec::new();
    for &c in b64 {
        // Caller only passes valid base64 chars.
        let v = match utf7_base64_value(c) {
            Some(v) => v,
            None => return (units, Some("partial character in shift sequence")),
        };
        acc = (acc << 6) | v;
        nbits += 6;
        if nbits >= 16 {
            nbits -= 16;
            units.push(((acc >> nbits) & 0xFFFF) as u16);
        }
    }
    if nbits >= 6 {
        return (units, Some("partial character in shift sequence"));
    }
    if nbits > 0 && (acc & ((1 << nbits) - 1)) != 0 {
        return (units, Some("non-zero padding bits in shift sequence"));
    }
    (units, None)
}

/// Decode modified UTF-7.  Direct bytes pass through; `+...-` sections are
/// base64-decoded to UTF-16BE code units (`+-` is a literal `+`).
fn decode_utf7(bytes: &[u8], errors: &str) -> Result<Value> {
    let mut cps: Vec<u32> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'+' {
            cps.push(b as u32);
            i += 1;
            continue;
        }
        // `+` begins a shifted section.  `+-` is a literal `+`.
        if bytes.get(i + 1) == Some(&b'-') {
            cps.push('+' as u32);
            i += 2;
            continue;
        }
        let section_start = i;
        i += 1;
        let mut b64: Vec<u8> = Vec::new();
        while i < bytes.len() && utf7_base64_value(bytes[i]).is_some() {
            b64.push(bytes[i]);
            i += 1;
        }
        let (units, err) = utf7_base64_decode(&b64);
        // Combine UTF-16 units (surrogate pairs → scalar; lone surrogate kept).
        utf7_units_to_codepoints(&units, &mut cps);
        if let Some(reason) = err {
            // CPython's error span includes a terminating '-' if present.
            let end = if bytes.get(i) == Some(&b'-') {
                i + 1
            } else {
                i
            };
            match errors {
                "strict" => {
                    return Err(PyError::UnicodeDecodeError {
                        encoding: "utf7".to_string(),
                        object: bytes.to_vec(),
                        start: section_start,
                        end,
                        reason: reason.to_string(),
                    });
                }
                "ignore" => {}
                "replace" => cps.push(0xFFFD),
                other => {
                    return Err(PyError::named(
                        "LookupError",
                        format!("unknown error handler name '{other}'"),
                    ));
                }
            }
        }
        // Consume a terminating `-` if present (explicit shift-out).
        if bytes.get(i) == Some(&b'-') {
            i += 1;
        }
    }
    Ok(string_from_codepoints(&cps))
}

/// Combine decoded UTF-16 units into codepoints, joining valid surrogate pairs
/// and keeping lone surrogates as-is.
fn utf7_units_to_codepoints(units: &[u16], cps: &mut Vec<u32>) {
    let mut k = 0usize;
    while k < units.len() {
        let u = units[k];
        if (0xD800..=0xDBFF).contains(&u) && k + 1 < units.len() {
            let low = units[k + 1];
            if (0xDC00..=0xDFFF).contains(&low) {
                let cp = 0x10000 + ((u as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
                cps.push(cp);
                k += 2;
                continue;
            }
        }
        cps.push(u as u32);
        k += 1;
    }
}
