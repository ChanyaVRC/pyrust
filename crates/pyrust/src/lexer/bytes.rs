fn lex_bytes(chars: &[char], start: usize, raw: bool) -> Result<(Token, usize)> {
    let quote = chars[start];

    // Triple-quoted bytes literals: b"""...""" or b'''...'''
    if chars.get(start + 1) == Some(&quote) && chars.get(start + 2) == Some(&quote) {
        let mut pos = start + 3;
        let mut out: Vec<u8> = Vec::new();
        loop {
            if pos + 2 < chars.len()
                && chars[pos] == quote
                && chars[pos + 1] == quote
                && chars[pos + 2] == quote
            {
                return Ok((Token::Bytes(out), pos + 3));
            }
            match chars.get(pos) {
                None => {
                    return Err(PyError::Lex(
                        "unterminated triple-quoted bytes literal".to_string(),
                    ));
                }
                Some(&'\\') if raw => {
                    // In raw mode: backslash is kept literally.
                    let next = chars.get(pos + 1).copied().ok_or_else(|| {
                        PyError::Lex(
                            "EOL while scanning bytes literal (trailing backslash in raw bytes)"
                                .to_string(),
                        )
                    })?;
                    if (next as u32) > 0x7f {
                        return Err(PyError::Lex(format!(
                            "bytes can only contain ASCII literal characters (got {next:?})"
                        )));
                    }
                    out.push(b'\\');
                    out.push(next as u8);
                    pos += 2;
                }
                Some(&'\\') => {
                    pos += 1;
                    let esc = chars
                        .get(pos)
                        .copied()
                        .ok_or_else(|| PyError::Lex("unterminated bytes escape".to_string()))?;
                    let mapped = match esc {
                        'n' => 0x0a,
                        't' => 0x09,
                        'r' => 0x0d,
                        '\\' => 0x5c,
                        '\'' => 0x27,
                        '"' => 0x22,
                        'a' => 0x07,
                        'b' => 0x08,
                        'f' => 0x0c,
                        'v' => 0x0b,
                        '0'..='7' => {
                            let mut val = esc as u32 - '0' as u32;
                            if let Some(&d) = chars.get(pos + 1)
                                && ('0'..='7').contains(&d)
                            {
                                val = val * 8 + (d as u32 - '0' as u32);
                                pos += 1;
                                if let Some(&d2) = chars.get(pos + 1)
                                    && ('0'..='7').contains(&d2)
                                {
                                    val = val * 8 + (d2 as u32 - '0' as u32);
                                    pos += 1;
                                }
                            }
                            // CPython 3.12: values > 0xFF emit SyntaxWarning and truncate
                            // to the low byte. pyrust omits the warning for now.
                            (val & 0xFF) as u8
                        }
                        'x' => {
                            let hi = chars
                                .get(pos + 1)
                                .copied()
                                .ok_or_else(|| PyError::Lex("incomplete \\x escape".to_string()))?;
                            let lo = chars
                                .get(pos + 2)
                                .copied()
                                .ok_or_else(|| PyError::Lex("incomplete \\x escape".to_string()))?;
                            let v = u8::from_str_radix(&format!("{hi}{lo}"), 16)
                                .map_err(|_| PyError::Lex("invalid \\x escape".to_string()))?;
                            pos += 2;
                            v
                        }
                        other => {
                            // \<newline>: line continuation — both characters dropped.
                            if other == '\n' {
                                pos += 1;
                                continue;
                            }
                            if other == '\r' {
                                pos += 1;
                                if chars.get(pos) == Some(&'\n') {
                                    pos += 1;
                                }
                                continue;
                            }
                            // Unrecognised escape: keep backslash + char verbatim
                            // (matches CPython 3.12 which emits a SyntaxWarning).
                            if (other as u32) > 0x7f {
                                return Err(PyError::Lex(format!(
                                    "bytes can only contain ASCII literal characters (got {other:?})"
                                )));
                            }
                            out.push(b'\\');
                            out.push(other as u8);
                            pos += 1;
                            continue;
                        }
                    };
                    out.push(mapped);
                    pos += 1;
                }
                Some(&'\r') => {
                    // Normalize CR and CRLF to LF, matching CPython's universal-newline
                    // handling for triple-quoted literals.
                    pos += 1;
                    if chars.get(pos) == Some(&'\n') {
                        pos += 1;
                    }
                    out.push(b'\n');
                }
                Some(&c) => {
                    if (c as u32) > 0x7f {
                        return Err(PyError::Lex(format!(
                            "bytes can only contain ASCII literal characters (got {c:?})"
                        )));
                    }
                    out.push(c as u8);
                    pos += 1;
                }
            }
        }
    }

    let mut pos = start + 1;
    let mut out: Vec<u8> = Vec::new();
    while let Some(c) = chars.get(pos).copied() {
        if c == quote {
            return Ok((Token::Bytes(out), pos + 1));
        }
        if c == '\\' {
            if raw {
                // In raw mode: backslash is kept literally.
                // A backslash before the quote character prevents the quote from
                // ending the string, but the backslash itself is included in the output.
                // A backslash at end of input is a syntax error.
                let next = chars.get(pos + 1).copied().ok_or_else(|| {
                    PyError::Lex(
                        "EOL while scanning bytes literal (trailing backslash in raw bytes)"
                            .to_string(),
                    )
                })?;
                if (next as u32) > 0x7f {
                    return Err(PyError::Lex(format!(
                        "bytes can only contain ASCII literal characters (got {next:?})"
                    )));
                }
                out.push(b'\\');
                out.push(next as u8);
                pos += 2;
                continue;
            }
            pos += 1;
            let esc = chars
                .get(pos)
                .copied()
                .ok_or_else(|| PyError::Lex("unterminated bytes escape".to_string()))?;
            let mapped = match esc {
                'n' => 0x0a,
                't' => 0x09,
                'r' => 0x0d,
                '\\' => 0x5c,
                '\'' => 0x27,
                '"' => 0x22,
                'a' => 0x07,
                'b' => 0x08,
                'f' => 0x0c,
                'v' => 0x0b,
                '0'..='7' => {
                    // \ooo — 1 to 3 octal digits; produces values 0x00–0xFF.
                    // CPython 3.12: values > 0xFF emit SyntaxWarning and truncate to
                    // the low byte. pyrust omits the warning for now.
                    let mut val = esc as u32 - '0' as u32;
                    if let Some(&d) = chars.get(pos + 1)
                        && ('0'..='7').contains(&d)
                    {
                        val = val * 8 + (d as u32 - '0' as u32);
                        pos += 1;
                        if let Some(&d2) = chars.get(pos + 1)
                            && ('0'..='7').contains(&d2)
                        {
                            val = val * 8 + (d2 as u32 - '0' as u32);
                            pos += 1;
                        }
                    }
                    (val & 0xFF) as u8
                }
                'x' => {
                    // \xNN — two hex digits
                    let hi = chars
                        .get(pos + 1)
                        .copied()
                        .ok_or_else(|| PyError::Lex("incomplete \\x escape".to_string()))?;
                    let lo = chars
                        .get(pos + 2)
                        .copied()
                        .ok_or_else(|| PyError::Lex("incomplete \\x escape".to_string()))?;
                    let v = u8::from_str_radix(&format!("{hi}{lo}"), 16)
                        .map_err(|_| PyError::Lex("invalid \\x escape".to_string()))?;
                    pos += 2;
                    v
                }
                other => {
                    // \<newline> (and \<CR> / \<CR><LF>) is a line continuation:
                    // both characters are dropped, matching CPython 3.12 behaviour.
                    if other == '\n' {
                        pos += 1;
                        continue;
                    }
                    if other == '\r' {
                        pos += 1;
                        // Skip the \n of a CRLF pair if present.
                        if chars.get(pos) == Some(&'\n') {
                            pos += 1;
                        }
                        continue;
                    }
                    // Unrecognised escape: CPython 3.12 keeps the backslash and
                    // the character verbatim (emitting a SyntaxWarning which pyrust
                    // omits for now).
                    if (other as u32) > 0x7f {
                        return Err(PyError::Lex(format!(
                            "bytes can only contain ASCII literal characters (got {other:?})"
                        )));
                    }
                    out.push(b'\\');
                    out.push(other as u8);
                    pos += 1;
                    continue;
                }
            };
            out.push(mapped);
            pos += 1;
            continue;
        }
        // Non-ASCII characters not allowed in bytes literals
        if (c as u32) > 0x7f {
            return Err(PyError::Lex(format!(
                "bytes can only contain ASCII literal characters (got {c:?})"
            )));
        }
        out.push(c as u8);
        pos += 1;
    }
    Err(PyError::Lex("unterminated bytes literal".to_string()))
}
