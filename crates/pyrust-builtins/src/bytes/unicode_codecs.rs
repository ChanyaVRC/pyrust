/// Decode UTF-8 bytes that are known to contain at least one invalid sequence,
/// applying the specified error handler.
fn decode_utf8_with_errors(bytes: &[u8], errors: &str, codec_name: &str) -> Result<Value> {
    match errors {
        "strict" => {
            let e = std::str::from_utf8(bytes).unwrap_err();
            let start = e.valid_up_to();
            let end = start + e.error_len().unwrap_or(bytes.len() - start);
            let reason = if e.error_len().is_none() {
                "unexpected end of data"
            } else {
                let b = bytes[start];
                // CPython 3.12 reports "invalid continuation byte" when the
                // byte at `start` is a valid multi-byte sequence start
                // (0xC2..=0xF4) but the bytes that follow are not valid
                // continuation bytes.  All other cases are "invalid start byte".
                if matches!(b, 0xC2..=0xF4) {
                    "invalid continuation byte"
                } else {
                    "invalid start byte"
                }
            };
            Err(PyError::UnicodeDecodeError {
                encoding: codec_name.to_string(),
                object: bytes.to_vec(),
                start,
                end,
                reason: reason.to_string(),
            })
        }
        "ignore" => Ok(Value::string(bytes_decode_utf8_ignore(bytes))),
        "replace" => Ok(Value::string(String::from_utf8_lossy(bytes).as_ref())),
        "backslashreplace" => Ok(Value::string(bytes_decode_utf8_backslashreplace(bytes))),
        "surrogateescape" => Ok(Value::string(bytes_decode_utf8_surrogateescape(bytes))),
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown error handler name '{errors}'"),
        )),
    }
}

/// Decode ASCII bytes that contain at least one byte > 0x7F, applying the
/// specified error handler.
fn decode_ascii_with_errors(bytes: &[u8], errors: &str) -> Result<Value> {
    match errors {
        "strict" => {
            let i = bytes.iter().position(|&b| b > 0x7F).unwrap();
            Err(PyError::UnicodeDecodeError {
                encoding: "ascii".to_string(),
                object: bytes.to_vec(),
                start: i,
                end: i + 1,
                reason: "ordinal not in range(128)".to_string(),
            })
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
        "backslashreplace" => {
            let mut out = String::with_capacity(bytes.len());
            for &b in bytes {
                if b <= 0x7F {
                    out.push(b as char);
                } else {
                    use std::fmt::Write as _;
                    let _ = write!(out, "\\x{:02x}", b);
                }
            }
            Ok(Value::string(&out))
        }
        "surrogateescape" => {
            // Each byte > 0x7F maps to the lone surrogate U+DC80 + byte.
            let mut out = String::with_capacity(bytes.len());
            for &b in bytes {
                if b <= 0x7F {
                    out.push(b as char);
                } else {
                    push_surrogate_escape(&mut out, b);
                }
            }
            Ok(Value::string(&out))
        }
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown error handler name '{errors}'"),
        )),
    }
}

/// Push the CESU-8 encoding of the lone surrogate codepoint U+DC00 | b into
/// `out`.  Pyrust uses CESU-8 to represent surrogate codepoints throughout.
#[inline]
fn push_surrogate_escape(out: &mut String, b: u8) {
    // surrogateescape maps byte b (0x80..=0xFF) to U+DC80..=U+DCFF.
    // DC80 = 0xDC80, so codepoint = 0xDC00 | (b & 0x7F) only for 0x80..=0xFF:
    // U+DC80 + (b - 0x80) = 0xDC80 + b - 0x80 = 0xDC00 + b.
    let cp: u32 = 0xDC00u32 | (b as u32);
    // CESU-8 for a surrogate codepoint (0xD800..=0xDFFF):
    // Safety: we hold &mut String exclusively and push a valid CESU-8 triplet.
    unsafe {
        out.as_mut_vec().extend_from_slice(&[
            0xE0 | (cp >> 12) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]);
    }
}

/// Incrementally decode `bytes` as UTF-8, invoking `on_invalid` once per byte of
/// every invalid sequence. Valid UTF-8 runs pass through unchanged. This is the
/// single shared scaffold behind the `backslashreplace` / `surrogateescape` /
/// `ignore` error handlers, which differ only in `on_invalid`.
fn bytes_decode_utf8_with(bytes: &[u8], mut on_invalid: impl FnMut(&mut String, u8)) -> String {
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
                // SAFETY: `from_utf8` reports `valid_up_to` as the length of the
                // well-formed UTF-8 prefix of `bytes[i..]`, so `bytes[i..i +
                // valid_up_to]` is valid UTF-8 and `from_utf8_unchecked` is sound.
                out.push_str(unsafe { std::str::from_utf8_unchecked(&bytes[i..i + valid_up_to]) });
                // Hand each byte of the invalid run to the handler. `error_len()`
                // is `None` for a truncated trailing sequence, treated as 1 byte.
                let skip = e.error_len().unwrap_or(1);
                for j in 0..skip {
                    on_invalid(&mut out, bytes[i + valid_up_to + j]);
                }
                i += valid_up_to + skip;
            }
        }
    }
    out
}

/// Decode UTF-8 with `backslashreplace`: each invalid byte `b` becomes `\xNN`.
/// Valid UTF-8 bytes pass through unchanged.
fn bytes_decode_utf8_backslashreplace(bytes: &[u8]) -> String {
    bytes_decode_utf8_with(bytes, |out, b| {
        use std::fmt::Write as _;
        let _ = write!(out, "\\x{:02x}", b);
    })
}

/// Decode UTF-8 with `surrogateescape`: each invalid byte `b` becomes the lone
/// surrogate U+DC80 + (b - 0x80) (stored as CESU-8).  Valid UTF-8 passes through.
fn bytes_decode_utf8_surrogateescape(bytes: &[u8]) -> String {
    bytes_decode_utf8_with(bytes, push_surrogate_escape)
}

/// Decode a little-endian UTF-16 byte slice (no BOM) into a Python string,
/// applying the specified error handler on invalid sequences.
///
/// See `decode_utf16` for the `original_bytes`/`bom_offset` contract.
fn decode_utf16_le(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
) -> Result<Value> {
    decode_utf16(bytes, original_bytes, bom_offset, errors, false)
}

/// Decode a big-endian UTF-16 byte slice (no BOM) into a Python string,
/// applying the specified error handler on invalid sequences.
///
/// See `decode_utf16` for the `original_bytes`/`bom_offset` contract.
fn decode_utf16_be(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
) -> Result<Value> {
    decode_utf16(bytes, original_bytes, bom_offset, errors, true)
}

/// Decode a UTF-16 byte slice (no BOM) into a Python string, applying the
/// specified error handler on invalid sequences. `big_endian` selects the
/// byte order, and thus the `utf-16-le`/`utf-16-be` codec name reported in
/// any `UnicodeDecodeError`.
///
/// `original_bytes` is the full original byte sequence passed by the caller
/// (may include a BOM prefix).  `bom_offset` is the number of BOM bytes that
/// were stripped before `bytes` was derived from `original_bytes`.  Both are
/// forwarded to `decode_utf16_units` so that any `UnicodeDecodeError` carries
/// the full original bytes and correct start/end offsets — matching CPython's
/// behaviour (see issues #1781, #1813).
fn decode_utf16(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
    big_endian: bool,
) -> Result<Value> {
    let codec_name = if big_endian { "utf-16-be" } else { "utf-16-le" };
    let to_u16 = if big_endian {
        u16::from_be_bytes
    } else {
        u16::from_le_bytes
    };
    if !bytes.len().is_multiple_of(2) {
        // Truncated: odd number of bytes.
        let trunc_start = bom_offset + bytes.len() - 1;
        let trunc_end = bom_offset + bytes.len();
        match errors {
            "ignore" => {
                // Drop the trailing byte and decode the rest.
                let units: Vec<u16> = bytes[..bytes.len() - 1]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| to_u16([c[0], c[1]]))
                    .collect();
                return decode_utf16_units(&units, original_bytes, bom_offset, codec_name, errors);
            }
            "replace" => {
                let units: Vec<u16> = bytes[..bytes.len() - 1]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| to_u16([c[0], c[1]]))
                    .collect();
                let mut s = decode_utf16_units_to_string(
                    &units,
                    original_bytes,
                    bom_offset,
                    codec_name,
                    errors,
                )?;
                s.push('\u{FFFD}');
                return Ok(Value::string(&s));
            }
            "backslashreplace" => {
                let units: Vec<u16> = bytes[..bytes.len() - 1]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| to_u16([c[0], c[1]]))
                    .collect();
                let mut s = decode_utf16_units_to_string(
                    &units,
                    original_bytes,
                    bom_offset,
                    codec_name,
                    errors,
                )?;
                use std::fmt::Write as _;
                let _ = write!(s, "\\x{:02x}", bytes[bytes.len() - 1]);
                return Ok(Value::string(&s));
            }
            "strict" | "surrogateescape" => {
                return Err(PyError::UnicodeDecodeError {
                    encoding: codec_name.to_string(),
                    object: original_bytes.to_vec(),
                    start: trunc_start,
                    end: trunc_end,
                    reason: "truncated data".to_string(),
                });
            }
            _ => {
                return Err(PyError::named(
                    "LookupError",
                    format!("unknown error handler name '{errors}'"),
                ));
            }
        }
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| to_u16([c[0], c[1]]))
        .collect();
    decode_utf16_units(&units, original_bytes, bom_offset, codec_name, errors)
}

/// Decode a slice of UTF-16 code units into a Python string, returning a `Value`.
///
/// `raw_bytes` is the full original byte sequence (including any BOM prefix).
/// `bom_offset` is the number of BOM bytes preceding the encoded units, used to
/// adjust `start`/`end` in `UnicodeDecodeError` — matching CPython's behaviour.
fn decode_utf16_units(
    units: &[u16],
    raw_bytes: &[u8],
    bom_offset: usize,
    codec_name: &str,
    errors: &str,
) -> Result<Value> {
    let s = decode_utf16_units_to_string(units, raw_bytes, bom_offset, codec_name, errors)?;
    Ok(Value::string(&s))
}

/// Inner helper: decode UTF-16 code units into a `String`, applying the error handler.
///
/// `raw_bytes` is the full original byte sequence (including any BOM prefix).
/// `bom_offset` adjusts `start`/`end` in `UnicodeDecodeError` and the byte slice
/// used by `backslashreplace` so that offsets are relative to `raw_bytes`.
///
/// This is factored out so the truncation-handling paths in `decode_utf16_le`/`_be`
/// can decode the valid prefix and then append their own substitution.
fn decode_utf16_units_to_string(
    units: &[u16],
    raw_bytes: &[u8],
    bom_offset: usize,
    codec_name: &str,
    errors: &str,
) -> Result<String> {
    // Validate the error handler name upfront for the non-strict paths, so that
    // an unknown handler always raises LookupError (matching CPython's behaviour
    // where the handler is validated regardless of whether any error occurs).
    // We do this check inside the error arms below; see the `_` arm.
    let mut out = String::with_capacity(units.len());
    let mut iter = units.iter().copied().enumerate();
    while let Some((i, u)) = iter.next() {
        match u {
            // High surrogate: expect a following low surrogate.
            0xD800..=0xDBFF => {
                let next = iter.next();
                match next {
                    Some((_, low)) if (0xDC00..=0xDFFF).contains(&low) => {
                        let cp = 0x10000 + ((u as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
                        out.push(char::from_u32(cp).expect("valid surrogate pair"));
                    }
                    // End of stream after a high surrogate: no low surrogate follows.
                    None => match errors {
                        "replace" => out.push('\u{FFFD}'),
                        "ignore" => {}
                        "backslashreplace" => {
                            // Emit the two raw bytes of the high surrogate unit.
                            use std::fmt::Write as _;
                            let byte_start = bom_offset + i * 2;
                            let unit_bytes = &raw_bytes[byte_start..byte_start + 2];
                            for &b in unit_bytes {
                                let _ = write!(out, "\\x{b:02x}");
                            }
                        }
                        _ => {
                            // "strict", "surrogateescape", and any unknown handler.
                            if errors != "strict" && errors != "surrogateescape" {
                                return Err(PyError::named(
                                    "LookupError",
                                    format!("unknown error handler name '{errors}'"),
                                ));
                            }
                            return Err(PyError::UnicodeDecodeError {
                                encoding: codec_name.to_string(),
                                object: raw_bytes.to_vec(),
                                start: bom_offset + i * 2,
                                end: bom_offset + i * 2 + 2,
                                reason: "unexpected end of data".to_string(),
                            });
                        }
                    },
                    // A non-low-surrogate unit follows the high surrogate.
                    Some((j, _)) => match errors {
                        "replace" => {
                            // Replace the bad high surrogate; re-process the next unit.
                            out.push('\u{FFFD}');
                            // Put j back — we can't un-advance an iterator, so decode
                            // the next unit directly from its value.
                            let next_u = units[j];
                            match next_u {
                                0xD800..=0xDBFF => {
                                    // Another high surrogate: will be handled next iteration —
                                    // but we already consumed it. Push a replacement for it too
                                    // only if it itself has no following low (which we can't
                                    // check here). This is subtle: CPython replaces only the
                                    // first bad unit and then continues, so we do the same by
                                    // pushing back via a sub-decode of the remaining slice.
                                    // Simple approach: just re-run from j onward.
                                    let sub = decode_utf16_units_to_string(
                                        &units[j..],
                                        raw_bytes,
                                        bom_offset + j * 2,
                                        codec_name,
                                        errors,
                                    )?;
                                    out.push_str(&sub);
                                    return Ok(out);
                                }
                                0xDC00..=0xDFFF => {
                                    // Lone low surrogate — replace it too.
                                    out.push('\u{FFFD}');
                                }
                                _ => {
                                    out.push(
                                        char::from_u32(next_u as u32)
                                            .expect("BMP codepoint is valid"),
                                    );
                                }
                            }
                        }
                        "ignore" => {
                            // Skip the bad high surrogate; re-process the next unit.
                            let next_u = units[j];
                            match next_u {
                                0xD800..=0xDFFF => {
                                    let sub = decode_utf16_units_to_string(
                                        &units[j..],
                                        raw_bytes,
                                        bom_offset + j * 2,
                                        codec_name,
                                        errors,
                                    )?;
                                    out.push_str(&sub);
                                    return Ok(out);
                                }
                                _ => {
                                    out.push(
                                        char::from_u32(next_u as u32)
                                            .expect("BMP codepoint is valid"),
                                    );
                                }
                            }
                        }
                        "backslashreplace" => {
                            use std::fmt::Write as _;
                            let byte_start = bom_offset + i * 2;
                            let unit_bytes = &raw_bytes[byte_start..byte_start + 2];
                            for &b in unit_bytes {
                                let _ = write!(out, "\\x{b:02x}");
                            }
                            // Re-process the unit that followed.
                            let sub = decode_utf16_units_to_string(
                                &units[j..],
                                raw_bytes,
                                bom_offset + j * 2,
                                codec_name,
                                errors,
                            )?;
                            out.push_str(&sub);
                            return Ok(out);
                        }
                        _ => {
                            if errors != "strict" && errors != "surrogateescape" {
                                return Err(PyError::named(
                                    "LookupError",
                                    format!("unknown error handler name '{errors}'"),
                                ));
                            }
                            return Err(PyError::UnicodeDecodeError {
                                encoding: codec_name.to_string(),
                                object: raw_bytes.to_vec(),
                                start: bom_offset + i * 2,
                                end: bom_offset + i * 2 + 2,
                                reason: "illegal UTF-16 surrogate".to_string(),
                            });
                        }
                    },
                }
            }
            // Lone low surrogate: invalid.
            0xDC00..=0xDFFF => match errors {
                "replace" => out.push('\u{FFFD}'),
                "ignore" => {}
                "backslashreplace" => {
                    use std::fmt::Write as _;
                    let byte_start = bom_offset + i * 2;
                    let unit_bytes = &raw_bytes[byte_start..byte_start + 2];
                    for &b in unit_bytes {
                        let _ = write!(out, "\\x{b:02x}");
                    }
                }
                _ => {
                    if errors != "strict" && errors != "surrogateescape" {
                        return Err(PyError::named(
                            "LookupError",
                            format!("unknown error handler name '{errors}'"),
                        ));
                    }
                    return Err(PyError::UnicodeDecodeError {
                        encoding: codec_name.to_string(),
                        object: raw_bytes.to_vec(),
                        start: bom_offset + i * 2,
                        end: bom_offset + i * 2 + 2,
                        reason: "illegal encoding".to_string(),
                    });
                }
            },
            // BMP character.
            _ => {
                out.push(char::from_u32(u as u32).expect("BMP codepoint is valid"));
            }
        }
    }
    Ok(out)
}

/// Decode a little-endian UTF-32 byte slice (no BOM) into a Python string,
/// applying the specified error handler on invalid sequences.
///
/// See `decode_utf32` for the chunk-first rationale and the
/// `original_bytes`/`bom_offset` contract.
fn decode_utf32_le(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
) -> Result<Value> {
    decode_utf32(bytes, original_bytes, bom_offset, errors, false)
}

/// Decode a big-endian UTF-32 byte slice (no BOM) into a Python string,
/// applying the specified error handler on invalid sequences.
///
/// See `decode_utf32` for the chunk-first rationale and the
/// `original_bytes`/`bom_offset` contract.
fn decode_utf32_be(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
) -> Result<Value> {
    decode_utf32(bytes, original_bytes, bom_offset, errors, true)
}

/// Decode a UTF-32 byte slice (no BOM) into a Python string, applying the
/// specified error handler on invalid sequences. `big_endian` selects the
/// byte order, and thus the `utf-32-le`/`utf-32-be` codec name reported in
/// any `UnicodeDecodeError`.
///
/// CPython processes complete 4-byte chunks first (reporting "code point not in
/// range" on any invalid chunk) and only then reports "truncated data" for any
/// trailing bytes that don't form a complete chunk.  The early-truncation guard
/// is therefore removed in favour of checking the remainder after all full
/// chunks have been decoded successfully.
///
/// `original_bytes` is the full original byte sequence (including any BOM prefix).
/// `bom_offset` is the number of BOM bytes preceding `bytes` so that error
/// `start`/`end` offsets index into `original_bytes` — matching CPython's behaviour
/// (see issues #1781, #1813).
fn decode_utf32(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
    big_endian: bool,
) -> Result<Value> {
    let codec_name = if big_endian { "utf-32-be" } else { "utf-32-le" };
    let to_u32 = if big_endian {
        u32::from_be_bytes
    } else {
        u32::from_le_bytes
    };
    let (chunks, remainder) = bytes.as_chunks::<4>();
    let mut out = String::with_capacity(bytes.len() / 4);
    for (i, chunk) in chunks.iter().enumerate() {
        let cp = to_u32([chunk[0], chunk[1], chunk[2], chunk[3]]);
        match char::from_u32(cp) {
            Some(c) => out.push(c),
            None => match errors {
                "replace" => out.push('\u{FFFD}'),
                "ignore" => {}
                "backslashreplace" => {
                    use std::fmt::Write as _;
                    for &b in chunk {
                        let _ = write!(out, "\\x{b:02x}");
                    }
                }
                _ => {
                    if errors != "strict" && errors != "surrogateescape" {
                        return Err(PyError::named(
                            "LookupError",
                            format!("unknown error handler name '{errors}'"),
                        ));
                    }
                    return Err(PyError::UnicodeDecodeError {
                        encoding: codec_name.to_string(),
                        object: original_bytes.to_vec(),
                        start: bom_offset + i * 4,
                        end: bom_offset + i * 4 + 4,
                        reason: "code point not in range(0x110000)".to_string(),
                    });
                }
            },
        }
    }
    if !remainder.is_empty() {
        let n = bytes.len() - remainder.len();
        match errors {
            "replace" => out.push('\u{FFFD}'),
            "ignore" => {}
            "backslashreplace" => {
                use std::fmt::Write as _;
                for &b in remainder {
                    let _ = write!(out, "\\x{b:02x}");
                }
            }
            _ => {
                if errors != "strict" && errors != "surrogateescape" {
                    return Err(PyError::named(
                        "LookupError",
                        format!("unknown error handler name '{errors}'"),
                    ));
                }
                return Err(PyError::UnicodeDecodeError {
                    encoding: codec_name.to_string(),
                    object: original_bytes.to_vec(),
                    start: bom_offset + n,
                    end: bom_offset + bytes.len(),
                    reason: "truncated data".to_string(),
                });
            }
        }
    }
    Ok(Value::string(&out))
}

/// Decode UTF-8 bytes, skipping any invalid byte sequences (errors='ignore').
fn bytes_decode_utf8_ignore(bytes: &[u8]) -> String {
    bytes_decode_utf8_with(bytes, |_out, _b| {})
}

// ---------------------------------------------------------------------------
// startswith / endswith
// ---------------------------------------------------------------------------
