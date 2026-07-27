/// Decode `raw_unicode_escape` bytes: bytes < 0x100 pass through as Latin-1,
/// `\uHHHH` / `\UHHHHHHHH` are interpreted; backslash is otherwise literal.
fn decode_raw_unicode_escape(bytes: &[u8], errors: &str) -> Result<Value> {
    let mut cps: Vec<u32> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            cps.push(bytes[i] as u32);
            i += 1;
            continue;
        }
        // Count the run of consecutive backslashes; only an odd run followed by
        // 'u'/'U' starts an escape (CPython treats '\\u0041' as literal).
        let bs_start = i;
        let mut j = i;
        while j < bytes.len() && bytes[j] == b'\\' {
            j += 1;
        }
        let bs_count = j - bs_start;
        let next = bytes.get(j).copied();
        let is_escape = bs_count % 2 == 1 && matches!(next, Some(b'u') | Some(b'U'));
        if !is_escape {
            for _ in 0..bs_count {
                cps.push(b'\\' as u32);
            }
            i = j;
            continue;
        }
        // Emit the leading (even) backslashes literally; the last one escapes.
        for _ in 0..(bs_count - 1) {
            cps.push(b'\\' as u32);
        }
        let kind = next.unwrap();
        let digits = if kind == b'u' { 4 } else { 8 };
        let esc_start = j - 1; // position of the escaping backslash
        match parse_hex_escape(bytes, j + 1, digits) {
            Ok(cp) => {
                if cp > 0x10FFFF {
                    return raw_unicode_escape_error(
                        bytes,
                        errors,
                        esc_start,
                        j + 1 + digits,
                        "\\Uxxxxxxxx out of range",
                        &mut cps,
                    );
                }
                cps.push(cp);
                i = j + 1 + digits;
            }
            Err(consumed) => {
                let reason = if kind == b'u' {
                    "truncated \\uXXXX escape"
                } else {
                    "truncated \\UXXXXXXXX escape"
                };
                return raw_unicode_escape_error(
                    bytes,
                    errors,
                    esc_start,
                    j + 1 + consumed,
                    reason,
                    &mut cps,
                );
            }
        }
    }
    Ok(string_from_codepoints(&cps))
}

fn raw_unicode_escape_error(
    bytes: &[u8],
    errors: &str,
    start: usize,
    end: usize,
    reason: &str,
    cps: &mut Vec<u32>,
) -> Result<Value> {
    match errors {
        "strict" => Err(PyError::UnicodeDecodeError {
            encoding: "rawunicodeescape".to_string(),
            object: bytes.to_vec(),
            start,
            end,
            reason: reason.to_string(),
        }),
        "ignore" => {
            let rest = decode_raw_unicode_escape(&bytes[end..], errors)?;
            if let ValueKind::Str(s) = rest.kind() {
                for cp in pyrust_core::cesu8_codepoints(s) {
                    cps.push(cp);
                }
            }
            Ok(string_from_codepoints(cps))
        }
        "replace" => {
            cps.push(0xFFFD);
            let rest = decode_raw_unicode_escape(&bytes[end..], errors)?;
            if let ValueKind::Str(s) = rest.kind() {
                for cp in pyrust_core::cesu8_codepoints(s) {
                    cps.push(cp);
                }
            }
            Ok(string_from_codepoints(cps))
        }
        other => Err(PyError::named(
            "LookupError",
            format!("unknown error handler name '{other}'"),
        )),
    }
}
