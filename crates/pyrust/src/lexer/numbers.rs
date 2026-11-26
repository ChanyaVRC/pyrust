/// A number-literal tokenizer error at absolute char index `cur` (CPython
/// `tok->cur`: the reported offset is the 0-based column of `cur`), #3402.
#[cold]
fn number_error(msg: impl Into<String>, cur: usize) -> PyError {
    PyError::syntax(crate::error::SyntaxErrorInfo::at_chars(
        msg,
        cur,
        cur,
        crate::error::SyntaxTextRule::TokenizerLine,
    ))
}

/// Validate underscore placement in a raw digit string (before stripping `_`).
///
/// CPython rules (same across 3.11 / 3.12):
/// - A trailing underscore is a SyntaxError.
/// - Two consecutive underscores (`__`) are a SyntaxError.
/// - A leading underscore after the base prefix (e.g. `0x_FF`) is **valid**.
///
/// `kind` is used only in the error message (e.g. `"decimal"`, `"hexadecimal"`).
/// `raw_start` is the absolute char index of `raw[0]`: CPython's tokenizer
/// stops just past the first `_` that is not followed by a digit, and reports
/// that cursor.
fn validate_underscores(raw: &[char], raw_start: usize, kind: &str) -> Result<()> {
    for (i, &c) in raw.iter().enumerate() {
        if c == '_' && matches!(raw.get(i + 1), None | Some('_')) {
            return Err(number_error(
                format!("invalid {kind} literal"),
                raw_start + i + 1,
            ));
        }
    }
    Ok(())
}

/// Validate underscore placement in a decimal/float literal slice (PEP 515).
///
/// Unlike [`validate_underscores`] (which permits a leading underscore right
/// after a base prefix, as in `0x_FF`), a decimal/float literal requires every
/// `_` to sit **between two ASCII digits**.  This single rule rejects every
/// misplaced case CPython rejects: leading (`_1`), trailing (`1_`), doubled
/// (`1__0`), and any underscore adjacent to `.`, `e`/`E`, or a sign (`1_.5`,
/// `1.5_`, `1.0_e5`, `1.e_5`, `1e_5`, `1e5_`, `1e+_5`).
///
/// `raw` is the full literal slice (digits, `.`, `e`/`E`, `+`/`-`); the value
/// is parsed only after stripping the `_`s.
///
/// `raw_start` is the absolute char index of `raw[0]`.  CPython reports the
/// cursor just past an `_` that is not followed by a digit, or at an `_` that
/// does not follow one.
fn validate_decimal_underscores(raw: &[char], raw_start: usize) -> Result<()> {
    for (i, &c) in raw.iter().enumerate() {
        if c == '_' {
            let prev_digit = i > 0 && raw[i - 1].is_ascii_digit();
            let next_digit = raw.get(i + 1).is_some_and(|n| n.is_ascii_digit());
            if !next_digit {
                return Err(number_error("invalid decimal literal", raw_start + i + 1));
            }
            if !prev_digit {
                return Err(number_error("invalid decimal literal", raw_start + i));
            }
        }
    }
    Ok(())
}

/// Reject leading zeros in a decimal integer literal (CPython rule).
///
/// `raw` is the integer literal slice (decimal digits and `_` only — no `.`,
/// `e`, or `j`).  A literal that starts with `0` is only valid if every digit
/// is `0` (`0`, `00`, `0_0`); any nonzero digit (`0123`, `09`, `0_1`) is a
/// `SyntaxError`.  Underscore placement is assumed already validated.
///
/// `raw_start` is the absolute char index of `raw[0]`; CPython reports the
/// range from just past the literal's first char to just past its first
/// non-zero digit.
fn check_leading_zero(raw: &[char], raw_start: usize) -> Result<()> {
    if raw.first() == Some(&'0')
        && let Some(nonzero) = raw.iter().position(|&c| c.is_ascii_digit() && c != '0')
    {
        return Err(PyError::syntax(crate::error::SyntaxErrorInfo::at_chars(
            "leading zeros in decimal integer literals are not permitted; \
             use an 0o prefix for octal integers",
            raw_start + 1,
            raw_start + nonzero + 1,
            crate::error::SyntaxTextRule::TokenizerLine,
        )));
    }
    Ok(())
}

/// Lex a leading-dot float literal: `.DIGITS[e[+-]DIGITS]` or `.DIGITSj`.
/// `start` points at the `.` character.  The caller has already verified that
/// the character at `start+1` is a decimal digit.
fn lex_leading_dot_float(chars: &[char], start: usize) -> Result<(Token, usize)> {
    let mut pos = start + 1; // skip the leading dot; pos is now at first digit
    while matches!(chars.get(pos), Some('0'..='9' | '_')) {
        pos += 1;
    }
    // Optional exponent (underscores permitted between digits per PEP 515).
    if matches!(chars.get(pos), Some(&'e') | Some(&'E')) {
        pos += 1;
        if matches!(chars.get(pos), Some(&'+') | Some(&'-')) {
            pos += 1;
        }
        while matches!(chars.get(pos), Some('0'..='9' | '_')) {
            pos += 1;
        }
    }
    let raw = &chars[start..pos];
    validate_decimal_underscores(raw, start)?;
    let text: String = raw.iter().filter(|&&c| c != '_').collect();
    let val = text
        .parse::<f64>()
        .map_err(|_| PyError::lex(format!("invalid float '{text}'")))?;
    // Imaginary suffix: .5j
    if matches!(chars.get(pos), Some(&'j') | Some(&'J')) {
        return Ok((Token::Imag(val), pos + 1));
    }
    Ok((Token::Float(val), pos))
}

fn lex_number(chars: &[char], start: usize) -> Result<(Token, usize)> {
    let mut pos = start;
    // Hex
    if chars.get(pos) == Some(&'0') && matches!(chars.get(pos + 1), Some(&'x') | Some(&'X')) {
        pos += 2;
        let hex_start = pos;
        while matches!(
            chars.get(pos),
            Some('0'..='9' | 'a'..='f' | 'A'..='F' | '_')
        ) {
            pos += 1;
        }
        let raw_hex = &chars[hex_start..pos];
        if raw_hex.is_empty() || raw_hex.iter().all(|&c| c == '_') {
            // CPython stops right after the prefix, or after one `_`.
            let cur = hex_start + usize::from(!raw_hex.is_empty());
            return Err(number_error("invalid hexadecimal literal", cur));
        }
        validate_underscores(raw_hex, hex_start, "hexadecimal")?;
        let text: String = raw_hex.iter().filter(|&&c| c != '_').collect();
        return match i64::from_str_radix(&text, 16) {
            Ok(val) => Ok((Token::Int(val), pos)),
            Err(_) => {
                // Overflow: parse as BigInt and store decimal representation.
                let big = BigInt::parse_bytes(text.as_bytes(), 16)
                    .ok_or_else(|| PyError::lex("invalid hexadecimal literal".to_string()))?;
                Ok((Token::BigInt(big.to_string()), pos))
            }
        };
    }
    // Octal
    if chars.get(pos) == Some(&'0') && matches!(chars.get(pos + 1), Some(&'o') | Some(&'O')) {
        pos += 2;
        let oct_start = pos;
        while matches!(chars.get(pos), Some('0'..='7' | '_')) {
            pos += 1;
        }
        let raw_oct = &chars[oct_start..pos];
        if raw_oct.is_empty() || raw_oct.iter().all(|&c| c == '_') {
            // CPython stops right after the prefix, or after one `_`.
            let cur = oct_start + usize::from(!raw_oct.is_empty());
            return Err(number_error("invalid octal literal", cur));
        }
        validate_underscores(raw_oct, oct_start, "octal")?;
        let text: String = raw_oct.iter().filter(|&&c| c != '_').collect();
        return match i64::from_str_radix(&text, 8) {
            Ok(val) => Ok((Token::Int(val), pos)),
            Err(_) => {
                let big = BigInt::parse_bytes(text.as_bytes(), 8)
                    .ok_or_else(|| PyError::lex("invalid octal literal".to_string()))?;
                Ok((Token::BigInt(big.to_string()), pos))
            }
        };
    }
    // Binary
    if chars.get(pos) == Some(&'0') && matches!(chars.get(pos + 1), Some(&'b') | Some(&'B')) {
        pos += 2;
        let bin_start = pos;
        while matches!(chars.get(pos), Some('0'..='1' | '_')) {
            pos += 1;
        }
        let raw_bin = &chars[bin_start..pos];
        if raw_bin.is_empty() || raw_bin.iter().all(|&c| c == '_') {
            // CPython stops right after the prefix, or after one `_`.
            let cur = bin_start + usize::from(!raw_bin.is_empty());
            return Err(number_error("invalid binary literal", cur));
        }
        validate_underscores(raw_bin, bin_start, "binary")?;
        let text: String = raw_bin.iter().filter(|&&c| c != '_').collect();
        return match i64::from_str_radix(&text, 2) {
            Ok(val) => Ok((Token::Int(val), pos)),
            Err(_) => {
                let big = BigInt::parse_bytes(text.as_bytes(), 2)
                    .ok_or_else(|| PyError::lex("invalid binary literal".to_string()))?;
                Ok((Token::BigInt(big.to_string()), pos))
            }
        };
    }

    while matches!(chars.get(pos), Some('0'..='9' | '_')) {
        pos += 1;
    }
    // End of the integer-part digit run; needed for the leading-zero check on a
    // pure decimal integer literal (the `.`/`e`/`j` cases below are floats /
    // complex and are exempt from the leading-zero rule).
    let int_end = pos;

    // Accept DIGITS. (trailing-dot float: `1.`, `1.e5`) as well as DIGITS.DIGITS
    // (standard float: `1.5`).  In CPython, `1.` tokenises as float `1.0` and
    // the subsequent character (whatever it is) is a separate token; `1..` gives
    // float `1.` then a bare `.` dot token.  The only case where we do NOT
    // consume the dot is when the first character of the integer part indicates a
    // non-decimal literal (0x / 0o / 0b) — those are handled above and never
    // reach here.
    if chars.get(pos) == Some(&'.') {
        pos += 1; // consume the dot
        // Optional fractional digit run.  We greedily consume `_` here too (e.g.
        // the leading `_` of `1._5`) so PEP 515 validation over the whole slice
        // can reject misplaced underscores rather than silently splitting them
        // into a separate token.
        while matches!(chars.get(pos), Some('0'..='9' | '_')) {
            pos += 1;
        }
        // Optional exponent (underscores permitted between digits per PEP 515).
        if matches!(chars.get(pos), Some(&'e') | Some(&'E')) {
            pos += 1;
            if matches!(chars.get(pos), Some(&'+') | Some(&'-')) {
                pos += 1;
            }
            while matches!(chars.get(pos), Some('0'..='9' | '_')) {
                pos += 1;
            }
        }
        let raw = &chars[start..pos];
        validate_decimal_underscores(raw, start)?;
        let text: String = raw.iter().filter(|&&c| c != '_').collect();
        let val = text
            .parse::<f64>()
            .map_err(|_| PyError::lex(format!("invalid float '{text}'")))?;
        // Imaginary suffix: 3.14j
        if matches!(chars.get(pos), Some(&'j') | Some(&'J')) {
            return Ok((Token::Imag(val), pos + 1));
        }
        Ok((Token::Float(val), pos))
    } else {
        // Optional exponent on integer-looking floats like 1e5 (underscores
        // permitted between digits per PEP 515).
        if matches!(chars.get(pos), Some(&'e') | Some(&'E')) {
            pos += 1;
            if matches!(chars.get(pos), Some(&'+') | Some(&'-')) {
                pos += 1;
            }
            let exp_start = pos;
            while matches!(chars.get(pos), Some('0'..='9' | '_')) {
                pos += 1;
            }
            if pos > exp_start {
                let raw = &chars[start..pos];
                validate_decimal_underscores(raw, start)?;
                let text: String = raw.iter().filter(|&&c| c != '_').collect();
                let val = text
                    .parse::<f64>()
                    .map_err(|_| PyError::lex(format!("invalid float '{text}'")))?;
                if matches!(chars.get(pos), Some(&'j') | Some(&'J')) {
                    return Ok((Token::Imag(val), pos + 1));
                }
                return Ok((Token::Float(val), pos));
            }
        }
        // Imaginary suffix on bare int: 5j.  Exempt from the leading-zero rule
        // (`01j` is a valid complex literal in CPython).
        if matches!(chars.get(pos), Some(&'j') | Some(&'J')) {
            let raw_imag = &chars[start..pos];
            validate_decimal_underscores(raw_imag, start)?;
            let text: String = raw_imag.iter().filter(|&&c| c != '_').collect();
            let val = text
                .parse::<f64>()
                .map_err(|_| PyError::lex(format!("invalid imaginary literal '{text}j'")))?;
            return Ok((Token::Imag(val), pos + 1));
        }
        let raw_dec = &chars[start..int_end];
        validate_decimal_underscores(raw_dec, start)?;
        check_leading_zero(raw_dec, start)?;
        let text: String = raw_dec.iter().filter(|&&c| c != '_').collect();
        match text.parse::<i64>() {
            Ok(val) => Ok((Token::Int(val), pos)),
            Err(_) => {
                // CPython applies int_max_str_digits while compiling decimal
                // integer literals too. The active Interpreter publishes its
                // policy before exec/eval source parsing; power-of-two literal
                // forms returned above remain exempt.
                pyrust_core::check_int_parse_digits(&text, 10).map_err(|error| {
                    let message = match error.into_kind() {
                        crate::error::PyErrorKind::Named(_, message) => message,
                        other => other.to_string(),
                    };
                    // Located by the scanner loop (`huge_literal_error`, #3402).
                    PyError::lex(format!(
                        "{message} - Consider hexadecimal for huge integer literals \
                         to avoid decimal conversion limits."
                    ))
                })?;
                // Overflow: try parsing as an arbitrary-precision integer.
                let big = text
                    .parse::<BigInt>()
                    .map_err(|_| PyError::lex(format!("invalid integer '{text}'")))?;
                Ok((Token::BigInt(big.to_string()), pos))
            }
        }
    }
}
