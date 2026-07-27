/// Decode `unicode_escape` bytes: interpret Python string escapes
/// (`\n \t \r \a \b \f \v \0`, octal, `\xHH`, `\uHHHH`, `\UHHHHHHHH`, `\\`,
/// `\'`, `\"`); unknown escapes keep the backslash.
fn decode_unicode_escape(bytes: &[u8], errors: &str) -> Result<Value> {
    let mut cps: Vec<u32> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            cps.push(bytes[i] as u32);
            i += 1;
            continue;
        }
        let esc_start = i;
        match bytes.get(i + 1).copied() {
            None => {
                return unicode_escape_error(
                    bytes,
                    errors,
                    esc_start,
                    esc_start + 1,
                    "\\ at end of string",
                    &mut cps,
                );
            }
            Some(c) => match c {
                b'\n' => i += 2, // line continuation: nothing emitted
                b'\\' => {
                    cps.push(b'\\' as u32);
                    i += 2;
                }
                b'\'' => {
                    cps.push(b'\'' as u32);
                    i += 2;
                }
                b'"' => {
                    cps.push(b'"' as u32);
                    i += 2;
                }
                b'a' => {
                    cps.push(0x07);
                    i += 2;
                }
                b'b' => {
                    cps.push(0x08);
                    i += 2;
                }
                b'f' => {
                    cps.push(0x0C);
                    i += 2;
                }
                b'n' => {
                    cps.push(0x0A);
                    i += 2;
                }
                b'r' => {
                    cps.push(0x0D);
                    i += 2;
                }
                b't' => {
                    cps.push(0x09);
                    i += 2;
                }
                b'v' => {
                    cps.push(0x0B);
                    i += 2;
                }
                b'0'..=b'7' => {
                    // Octal escape: up to 3 digits.
                    let mut val: u32 = 0;
                    let mut k = i + 1;
                    let mut count = 0;
                    while k < bytes.len() && count < 3 && (b'0'..=b'7').contains(&bytes[k]) {
                        val = val * 8 + (bytes[k] - b'0') as u32;
                        k += 1;
                        count += 1;
                    }
                    cps.push(val);
                    i = k;
                }
                b'x' => match parse_hex_escape(bytes, i + 2, 2) {
                    Ok(cp) => {
                        cps.push(cp);
                        i += 4;
                    }
                    Err(consumed) => {
                        return unicode_escape_error(
                            bytes,
                            errors,
                            esc_start,
                            i + 2 + consumed,
                            "truncated \\xXX escape",
                            &mut cps,
                        );
                    }
                },
                b'u' => match parse_hex_escape(bytes, i + 2, 4) {
                    Ok(cp) => {
                        cps.push(cp);
                        i += 6;
                    }
                    Err(consumed) => {
                        return unicode_escape_error(
                            bytes,
                            errors,
                            esc_start,
                            i + 2 + consumed,
                            "truncated \\uXXXX escape",
                            &mut cps,
                        );
                    }
                },
                b'U' => match parse_hex_escape(bytes, i + 2, 8) {
                    Ok(cp) => {
                        if cp > 0x10FFFF {
                            return unicode_escape_error(
                                bytes,
                                errors,
                                esc_start,
                                esc_start + 10,
                                "illegal Unicode character",
                                &mut cps,
                            );
                        }
                        cps.push(cp);
                        i += 10;
                    }
                    Err(consumed) => {
                        return unicode_escape_error(
                            bytes,
                            errors,
                            esc_start,
                            i + 2 + consumed,
                            "truncated \\UXXXXXXXX escape",
                            &mut cps,
                        );
                    }
                },
                b'N' => {
                    // \N{NAME} — named character escape.
                    if bytes.get(i + 2) != Some(&b'{') {
                        return unicode_escape_error(
                            bytes,
                            errors,
                            esc_start,
                            esc_start + 2,
                            "malformed \\N character escape",
                            &mut cps,
                        );
                    }
                    match bytes[i + 3..].iter().position(|&b| b == b'}') {
                        None => {
                            return unicode_escape_error(
                                bytes,
                                errors,
                                esc_start,
                                bytes.len(),
                                "malformed \\N character escape",
                                &mut cps,
                            );
                        }
                        Some(rel) => {
                            let name_end = i + 3 + rel;
                            let name = std::str::from_utf8(&bytes[i + 3..name_end]).ok();
                            match name.and_then(unicode_names2::character) {
                                Some(ch) => {
                                    cps.push(ch as u32);
                                    i = name_end + 1;
                                }
                                None => {
                                    return unicode_escape_error(
                                        bytes,
                                        errors,
                                        esc_start,
                                        name_end + 1,
                                        "unknown Unicode character name",
                                        &mut cps,
                                    );
                                }
                            }
                        }
                    }
                }
                _ => {
                    // Unknown escape: keep the backslash and the char literally.
                    cps.push(b'\\' as u32);
                    cps.push(c as u32);
                    i += 2;
                }
            },
        }
    }
    Ok(string_from_codepoints(&cps))
}

fn unicode_escape_error(
    bytes: &[u8],
    errors: &str,
    start: usize,
    end: usize,
    reason: &str,
    cps: &mut Vec<u32>,
) -> Result<Value> {
    match errors {
        "strict" => Err(PyError::UnicodeDecodeError {
            encoding: "unicodeescape".to_string(),
            object: bytes.to_vec(),
            start,
            end,
            reason: reason.to_string(),
        }),
        "ignore" => {
            let rest = decode_unicode_escape(&bytes[end..], errors)?;
            if let ValueKind::Str(s) = rest.kind() {
                for cp in pyrust_core::cesu8_codepoints(s) {
                    cps.push(cp);
                }
            }
            Ok(string_from_codepoints(cps))
        }
        "replace" => {
            cps.push(0xFFFD);
            let rest = decode_unicode_escape(&bytes[end..], errors)?;
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
