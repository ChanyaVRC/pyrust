/// Encode a Unicode codepoint into a `String` fragment for a string literal.
///
/// Non-surrogate codepoints in `0..=0x10FFFF` are valid Unicode scalar values
/// and go through `char::from_u32`.  Lone surrogates (`0xD800..=0xDFFF`) are
/// not valid scalar values (Rust `char` rejects them), but pyrust's `str`
/// freely stores them — `chr(0xdc80)` works.  We encode a lone surrogate as
/// the same three-byte CESU-8 sequence that `chr_from_code_point`
/// (builtins.rs) uses, so a `\u`/`\U` surrogate escape produces a string
/// byte-identical to the corresponding `chr()` result.
fn codepoint_to_string_fragment(cp: u32) -> Option<String> {
    if (0xD800..=0xDFFF).contains(&cp) {
        // SAFETY: the three bytes are a well-formed CESU-8 encoding of a
        // surrogate codepoint, matching the representation pyrust uses for
        // surrogate-containing strings throughout the runtime.
        let s = unsafe {
            String::from_utf8_unchecked(vec![
                0xE0 | (cp >> 12) as u8,
                0x80 | ((cp >> 6) & 0x3F) as u8,
                0x80 | (cp & 0x3F) as u8,
            ])
        };
        Some(s)
    } else {
        char::from_u32(cp).map(|ch| ch.to_string())
    }
}

/// Parse a string escape sequence starting at `pos` (the character after the
/// backslash) and return `(resulting_str, next_pos)` where `next_pos` is the
/// index of the first character not consumed by this escape.
///
/// `content_start` is the index in `chars` of the first character after the
/// opening quote(s) of the string literal.  It is used to compute accurate
/// byte positions for `\N` error messages (matching CPython 3.12 output).
///
/// Supports single-character escapes (`\n`, `\t`, ...), octal escapes
/// (`\ooo`, 1–3 digits, value 0–511 as a Unicode codepoint), and `\xNN` hex
/// escapes (exactly two hex digits, producing U+0000–U+00FF).
///
/// For unrecognized escape sequences (e.g. `\z`), CPython 3.12 emits a
/// `DeprecationWarning` and preserves the two-character sequence `\<char>`
/// verbatim.  pyrust matches that behaviour (without emitting the warning).
fn parse_escape(chars: &[char], pos: usize, content_start: usize) -> Result<(String, usize)> {
    let c = *chars
        .get(pos)
        .ok_or_else(|| PyError::Lex("unterminated escape sequence".to_string()))?;
    match c {
        'n' => Ok(("\n".to_string(), pos + 1)),
        't' => Ok(("\t".to_string(), pos + 1)),
        'r' => Ok(("\r".to_string(), pos + 1)),
        '\\' => Ok(("\\".to_string(), pos + 1)),
        '\'' => Ok(("'".to_string(), pos + 1)),
        '"' => Ok(("\"".to_string(), pos + 1)),
        'a' => Ok(("\x07".to_string(), pos + 1)),
        'b' => Ok(("\x08".to_string(), pos + 1)),
        'f' => Ok(("\x0C".to_string(), pos + 1)),
        'v' => Ok(("\x0B".to_string(), pos + 1)),
        '0'..='7' => {
            // \ooo — 1 to 3 octal digits; in string literals produces U+0000–U+01FF
            // (CPython 3.12 accepts values > 0xFF as Unicode codepoints with a warning).
            let d1 = c as u32 - '0' as u32;
            let mut val = d1;
            let mut end = pos + 1;
            if let Some(&d) = chars.get(end)
                && ('0'..='7').contains(&d)
            {
                val = val * 8 + (d as u32 - '0' as u32);
                end += 1;
                if let Some(&d2) = chars.get(end)
                    && ('0'..='7').contains(&d2)
                {
                    val = val * 8 + (d2 as u32 - '0' as u32);
                    end += 1;
                }
            }
            let ch = char::from_u32(val).ok_or_else(|| {
                PyError::Lex(format!("octal escape value out of range: \\{val:o}"))
            })?;
            Ok((ch.to_string(), end))
        }
        'x' => {
            // \xNN — exactly two hex digits; produces U+0000–U+00FF.
            let hi = chars.get(pos + 1).copied().ok_or_else(|| {
                PyError::Lex("incomplete \\x escape (need 2 hex digits)".to_string())
            })?;
            let lo = chars.get(pos + 2).copied().ok_or_else(|| {
                PyError::Lex("incomplete \\x escape (need 2 hex digits)".to_string())
            })?;
            let v = u8::from_str_radix(&format!("{hi}{lo}"), 16)
                .map_err(|_| PyError::Lex(format!("invalid \\x escape: \\x{hi}{lo}")))?;
            Ok((char::from(v).to_string(), pos + 3))
        }
        'u' => {
            // \uNNNN — exactly four hex digits; BMP codepoint (surrogates forbidden).
            let digits: String = (1..=4)
                .map(|i| {
                    chars.get(pos + i).copied().ok_or_else(|| {
                        PyError::Lex("incomplete \\u escape (need 4 hex digits)".to_string())
                    })
                })
                .collect::<Result<_>>()?;
            let codepoint = u32::from_str_radix(&digits, 16)
                .map_err(|_| PyError::Lex(format!("invalid \\u escape: \\u{digits}")))?;
            // Lone surrogates (U+D800–U+DFFF) are stored using pyrust's
            // surrogate-aware encoding (same as `chr()`), matching CPython
            // which keeps lone surrogates in `str` (fixes #1893).
            let frag = codepoint_to_string_fragment(codepoint)
                .ok_or_else(|| PyError::Lex(format!("invalid \\u escape: U+{codepoint:04X}")))?;
            Ok((frag, pos + 5))
        }
        'U' => {
            // \UNNNNNNNN — exactly eight hex digits; full Unicode range (surrogates and >0x10FFFF forbidden).
            let digits: String = (1..=8)
                .map(|i| {
                    chars.get(pos + i).copied().ok_or_else(|| {
                        PyError::Lex("incomplete \\U escape (need 8 hex digits)".to_string())
                    })
                })
                .collect::<Result<_>>()?;
            let codepoint = u32::from_str_radix(&digits, 16)
                .map_err(|_| PyError::Lex(format!("invalid \\U escape: \\U{digits}")))?;
            // Keep the genuine out-of-range check; only the surrogate
            // rejection is lifted (fixes #1893).
            if codepoint > 0x10FFFF {
                return Err(PyError::Lex(format!(
                    "invalid \\U escape: codepoint U+{codepoint:08X} out of range"
                )));
            }
            // Lone surrogates are stored using pyrust's surrogate-aware
            // encoding (same as `chr()`), matching CPython.
            let frag = codepoint_to_string_fragment(codepoint)
                .ok_or_else(|| PyError::Lex(format!("invalid \\U escape: U+{codepoint:08X}")))?;
            Ok((frag, pos + 9))
        }
        'N' => {
            // \N{Unicode name} — look up character by Unicode name.
            //
            // Byte positions in error messages are relative to the string
            // content (after the opening quote), matching CPython 3.12.
            let bs_byte: usize = chars[content_start..pos - 1]
                .iter()
                .map(|c| c.len_utf8())
                .sum();
            if chars.get(pos + 1) != Some(&'{') {
                return Err(PyError::Lex(format!(
                    "(unicode error) 'unicodeescape' codec can't decode bytes in position \
                     {bs_byte}-{}: malformed \\N character escape",
                    bs_byte + 1
                )));
            }
            let mut end = pos + 2;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            if end >= chars.len() {
                // Unterminated \N{ — report up to the last name char scanned,
                // excluding the string-terminating quote/newline that caused us
                // to overshoot.  Unicode character names never contain quote
                // chars or newlines, so stopping at any of those is safe.
                let name_bytes: usize = chars[pos + 2..]
                    .iter()
                    .take_while(|&&c| c != '}' && c != '\'' && c != '"' && c != '\n' && c != '\r')
                    .map(|c| c.len_utf8())
                    .sum();
                let end_byte = bs_byte + 2 + name_bytes;
                return Err(PyError::Lex(format!(
                    "(unicode error) 'unicodeescape' codec can't decode bytes in position \
                     {bs_byte}-{end_byte}: malformed \\N character escape"
                )));
            }
            let name: String = chars[pos + 2..end].iter().collect();
            if name.is_empty() {
                return Err(PyError::Lex(format!(
                    "(unicode error) 'unicodeescape' codec can't decode bytes in position \
                     {bs_byte}-{}: malformed \\N character escape",
                    bs_byte + 2
                )));
            }
            let end_byte: usize = chars[content_start..end].iter().map(|c| c.len_utf8()).sum();
            let ch = unicode_names2::character(&name).ok_or_else(|| {
                PyError::Lex(format!(
                    "(unicode error) 'unicodeescape' codec can't decode bytes in position \
                     {bs_byte}-{end_byte}: unknown Unicode character name"
                ))
            })?;
            Ok((ch.to_string(), end + 1))
        }
        // Unrecognized escape: CPython 3.12 preserves the backslash + character
        // verbatim and emits a DeprecationWarning.  pyrust accepts the sequence
        // without emitting the warning (warning infrastructure not yet wired).
        other => Ok((format!("\\{other}"), pos + 1)),
    }
}
