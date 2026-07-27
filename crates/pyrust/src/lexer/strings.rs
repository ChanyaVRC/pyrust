fn lex_string(chars: &[char], start: usize, raw: bool) -> Result<(Token, usize)> {
    let quote = chars[start];

    // Triple-quoted strings
    if chars.get(start + 1) == Some(&quote) && chars.get(start + 2) == Some(&quote) {
        let content_start = start + 3;
        let mut pos = content_start;
        let mut out = String::new();
        loop {
            if pos + 2 < chars.len()
                && chars[pos] == quote
                && chars[pos + 1] == quote
                && chars[pos + 2] == quote
            {
                return Ok((Token::Str(out), pos + 3));
            }
            match chars.get(pos) {
                None => {
                    return Err(PyError::Lex(
                        "unterminated triple-quoted string".to_string(),
                    ));
                }
                Some(&'\\') if raw => {
                    // In raw mode: backslash is kept literally.
                    // A backslash before the quote character prevents the quote from
                    // ending the string, but the backslash itself is included in the output.
                    // A backslash at end of input is a syntax error.
                    let next = chars.get(pos + 1).copied().ok_or_else(|| {
                        PyError::Lex(
                            "EOL while scanning string literal (trailing backslash in raw string)"
                                .to_string(),
                        )
                    })?;
                    out.push('\\');
                    out.push(next);
                    pos += 2;
                }
                Some(&'\\') => {
                    pos += 1;
                    let escaped = chars
                        .get(pos)
                        .copied()
                        .ok_or_else(|| PyError::Lex("unterminated escape".to_string()))?;
                    // \<newline> inside a triple-quoted string: skip both
                    // (line continuation within the string literal, same as CPython).
                    // Also handle CRLF (\r\n): \<CR><LF> drops all three characters.
                    if escaped == '\n' {
                        pos += 1;
                    } else if escaped == '\r' {
                        pos += 1;
                        // If \r is followed by \n (CRLF), skip the \n too.
                        if chars.get(pos) == Some(&'\n') {
                            pos += 1;
                        }
                    } else {
                        let (s, next_pos) = parse_escape(chars, pos, content_start)?;
                        out.push_str(&s);
                        pos = next_pos;
                    }
                }
                Some(&'\r') => {
                    // Normalize CR and CRLF to LF inside triple-quoted strings,
                    // matching CPython's universal-newline handling (tokenize.c).
                    pos += 1;
                    if chars.get(pos) == Some(&'\n') {
                        pos += 1;
                    }
                    out.push('\n');
                }
                Some(&c) => {
                    out.push(c);
                    pos += 1;
                }
            }
        }
    }

    let content_start = start + 1;
    let mut pos = content_start;
    let mut out = String::new();

    while let Some(c) = chars.get(pos).copied() {
        if c == quote {
            return Ok((Token::Str(out), pos + 1));
        }
        if c == '\\' {
            if raw {
                // In raw mode: backslash is kept literally.
                // A backslash before the quote character prevents the quote from
                // ending the string, but the backslash itself is included in the output.
                // A backslash at end of input is a syntax error.
                let next = chars.get(pos + 1).copied().ok_or_else(|| {
                    PyError::Lex(
                        "EOL while scanning string literal (trailing backslash in raw string)"
                            .to_string(),
                    )
                })?;
                out.push('\\');
                out.push(next);
                pos += 2;
                continue;
            }
            pos += 1;
            let escaped = chars
                .get(pos)
                .copied()
                .ok_or_else(|| PyError::Lex("EOL while scanning string literal".to_string()))?;
            // \<newline>: line continuation — consume backslash and newline, add nothing.
            // Also handle CRLF: \<CR><LF> drops all three characters.
            if escaped == '\n' {
                pos += 1;
            } else if escaped == '\r' {
                pos += 1;
                if chars.get(pos) == Some(&'\n') {
                    pos += 1;
                }
            } else {
                let (s, next_pos) = parse_escape(chars, pos, content_start)?;
                out.push_str(&s);
                pos = next_pos;
            }
            continue;
        }
        out.push(c);
        pos += 1;
    }

    Err(PyError::Lex("unterminated string literal".to_string()))
}
