//! Python 3.12 literal decoders (integers, floats, complex, strings, bytes,
//! f-string fragments) for `ast.parse`.

use num_bigint::BigInt;
use num_traits::Num;

use crate::value::Value;

/// Result of decoding a string or bytes literal token.
pub(crate) enum DecodedString {
    Str {
        value: String,
        kind: Option<&'static str>,
    },
    Bytes(Vec<u8>),
}

/// Parse a Python 3.12 `NUMBER` token into an `int`, `BigInt`, `float`, or `complex` `Value`.
pub(crate) fn decode_number(raw: &str) -> Result<Value, String> {
    if raw.ends_with('j') || raw.ends_with('J') {
        let body = &raw[..raw.len() - 1];
        let clean: String = body.chars().filter(|&c| c != '_').collect();
        let im: f64 = clean
            .parse()
            .map_err(|_| format!("invalid imaginary literal: {raw}"))?;
        return Ok(Value::complex(0.0, im));
    }

    // Check base-prefixed integers (0x, 0o, 0b).
    if raw.len() >= 2 && raw.starts_with('0') {
        let second = raw.as_bytes()[1];
        let radix = match second {
            b'x' | b'X' => Some(16),
            b'o' | b'O' => Some(8),
            b'b' | b'B' => Some(2),
            _ => None,
        };
        if let Some(radix) = radix {
            let digits: String = raw[2..].chars().filter(|&c| c != '_').collect();
            if digits.is_empty() {
                return Err(format!("invalid numeric literal: {raw}"));
            }
            if let Ok(v) = i64::from_str_radix(&digits, radix) {
                return Ok(Value::int(v));
            }
            let big = BigInt::from_str_radix(&digits, radix)
                .map_err(|_| format!("invalid numeric literal: {raw}"))?;
            return Ok(Value::bigint(big));
        }
    }

    // Float if contains `.` or `e`/`E`.
    if raw.contains('.') || raw.contains('e') || raw.contains('E') {
        let clean: String = raw.chars().filter(|&c| c != '_').collect();
        let val: f64 = clean
            .parse()
            .map_err(|_| format!("invalid float literal: {raw}"))?;
        return Ok(Value::float(val));
    }

    // Decimal integer — reject leading zeros like `01` (except `0`, `00`, `0_0`).
    let clean: String = raw.chars().filter(|&c| c != '_').collect();
    if clean.len() > 1 && clean.starts_with('0') && clean.bytes().any(|b| b != b'0') {
        return Err(
            "leading zeros in decimal integer literals are not permitted; use an 0o prefix for octal integers"
                .to_string(),
        );
    }
    if let Ok(v) = clean.parse::<i64>() {
        return Ok(Value::int(v));
    }
    let big = BigInt::from_str_radix(&clean, 10)
        .map_err(|_| format!("invalid integer literal: {raw}"))?;
    Ok(Value::bigint(big))
}

/// Decode a Python 3.12 `STRING` token (with prefix and quotes) into `DecodedString`.
pub(crate) fn decode_string_token(raw: &str) -> Result<DecodedString, String> {
    let bytes = raw.as_bytes();
    let mut prefix_end = 0usize;
    while prefix_end < bytes.len() && bytes[prefix_end] != b'\'' && bytes[prefix_end] != b'"' {
        prefix_end += 1;
    }
    let prefix = &raw[..prefix_end];
    let rest = &raw[prefix_end..];

    let mut is_raw = false;
    let mut is_bytes = false;
    let mut is_unicode_prefix = false;
    for ch in prefix.chars() {
        match ch.to_ascii_lowercase() {
            'r' => is_raw = true,
            'b' => is_bytes = true,
            'u' => is_unicode_prefix = true,
            _ => return Err(format!("invalid string prefix: {prefix}")),
        }
    }

    let quote_len = if rest.starts_with("\"\"\"") || rest.starts_with("'''") {
        3
    } else if rest.starts_with('"') || rest.starts_with('\'') {
        1
    } else {
        return Err("invalid string literal".to_string());
    };
    if rest.len() < quote_len * 2 {
        return Err("unterminated string literal".to_string());
    }
    let inner = &rest[quote_len..rest.len() - quote_len];

    if is_bytes {
        let decoded = decode_bytes_inner(inner, is_raw)?;
        Ok(DecodedString::Bytes(decoded))
    } else {
        let decoded = decode_str_inner(inner, is_raw)?;
        let kind = if is_unicode_prefix { Some("u") } else { None };
        Ok(DecodedString::Str {
            value: decoded,
            kind,
        })
    }
}

/// Decode the literal text content of an `FSTRING_MIDDLE` token (which has no
/// surrounding quotes; `is_raw` comes from the enclosing `FSTRING_START`).
pub(crate) fn decode_fstring_middle(text: &str, is_raw: bool) -> Result<String, String> {
    // Normalize \r\n and \r to \n inside multiline f-string text.
    let normalized = normalize_newlines(text);
    if is_raw {
        Ok(normalized)
    } else {
        decode_str_escapes(&normalized)
    }
}

fn normalize_newlines(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

fn decode_str_inner(inner: &str, is_raw: bool) -> Result<String, String> {
    let normalized = normalize_newlines(inner);
    if is_raw {
        Ok(normalized)
    } else {
        decode_str_escapes(&normalized)
    }
}

fn codepoint_to_string_fragment(cp: u32) -> Option<String> {
    if (0xD800..=0xDFFF).contains(&cp) {
        // SAFETY: CESU-8 surrogate encoding matching PyRust's str representation.
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

fn decode_str_escapes(s: &str) -> Result<String, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        if i >= chars.len() {
            out.push('\\');
            break;
        }
        let esc = chars[i];
        match esc {
            '\n' => {
                // Line continuation inside string literal.
                i += 1;
            }
            '\\' => {
                out.push('\\');
                i += 1;
            }
            '\'' => {
                out.push('\'');
                i += 1;
            }
            '"' => {
                out.push('"');
                i += 1;
            }
            'a' => {
                out.push('\x07');
                i += 1;
            }
            'b' => {
                out.push('\x08');
                i += 1;
            }
            'f' => {
                out.push('\x0C');
                i += 1;
            }
            'n' => {
                out.push('\n');
                i += 1;
            }
            'r' => {
                out.push('\r');
                i += 1;
            }
            't' => {
                out.push('\t');
                i += 1;
            }
            'v' => {
                out.push('\x0B');
                i += 1;
            }
            '0'..='7' => {
                let mut val = (esc as u32) - ('0' as u32);
                i += 1;
                for _ in 0..2 {
                    if let Some(&d) = chars.get(i)
                        && ('0'..='7').contains(&d)
                    {
                        val = val * 8 + (d as u32 - '0' as u32);
                        i += 1;
                    } else {
                        break;
                    }
                }
                if let Some(ch) = char::from_u32(val) {
                    out.push(ch);
                } else {
                    return Err("octal escape value out of range".to_string());
                }
            }
            'x' => {
                if i + 2 >= chars.len() {
                    return Err("truncated \\xXX escape".to_string());
                }
                let hex: String = chars[i + 1..=i + 2].iter().collect();
                let v = u8::from_str_radix(&hex, 16)
                    .map_err(|_| format!("invalid \\x escape: \\x{hex}"))?;
                out.push(char::from(v));
                i += 3;
            }
            'u' => {
                if i + 4 >= chars.len() {
                    return Err("truncated \\uXXXX escape".to_string());
                }
                let hex: String = chars[i + 1..=i + 4].iter().collect();
                let cp = u32::from_str_radix(&hex, 16)
                    .map_err(|_| format!("invalid \\u escape: \\u{hex}"))?;
                let frag = codepoint_to_string_fragment(cp)
                    .ok_or_else(|| format!("invalid \\u escape: \\u{hex}"))?;
                out.push_str(&frag);
                i += 5;
            }
            'U' => {
                if i + 8 >= chars.len() {
                    return Err("truncated \\UXXXXXXXX escape".to_string());
                }
                let hex: String = chars[i + 1..=i + 8].iter().collect();
                let cp = u32::from_str_radix(&hex, 16)
                    .map_err(|_| format!("invalid \\U escape: \\U{hex}"))?;
                if cp > 0x10FFFF {
                    return Err("illegal Unicode character".to_string());
                }
                let frag = codepoint_to_string_fragment(cp)
                    .ok_or_else(|| format!("invalid \\U escape: \\U{hex}"))?;
                out.push_str(&frag);
                i += 9;
            }
            'N' => {
                if chars.get(i + 1) != Some(&'{') {
                    return Err("malformed \\N character escape".to_string());
                }
                let mut end = i + 2;
                while end < chars.len() && chars[end] != '}' {
                    end += 1;
                }
                if end >= chars.len() {
                    return Err("malformed \\N character escape".to_string());
                }
                let name: String = chars[i + 2..end].iter().collect();
                let ch = unicode_names2::character(&name)
                    .ok_or_else(|| format!("unknown Unicode character name: {name}"))?;
                out.push(ch);
                i = end + 1;
            }
            other => {
                out.push('\\');
                out.push(other);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn decode_bytes_inner(inner: &str, is_raw: bool) -> Result<Vec<u8>, String> {
    if !inner.is_ascii() {
        return Err("bytes can only contain ASCII literal characters".to_string());
    }
    let normalized = normalize_newlines(inner);
    if is_raw {
        return Ok(normalized.into_bytes());
    }
    let bytes = normalized.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        i += 1;
        if i >= bytes.len() {
            out.push(b'\\');
            break;
        }
        match bytes[i] {
            b'\n' => i += 1,
            b'\\' => {
                out.push(b'\\');
                i += 1;
            }
            b'\'' => {
                out.push(b'\'');
                i += 1;
            }
            b'"' => {
                out.push(b'"');
                i += 1;
            }
            b'a' => {
                out.push(0x07);
                i += 1;
            }
            b'b' => {
                out.push(0x08);
                i += 1;
            }
            b'f' => {
                out.push(0x0C);
                i += 1;
            }
            b'n' => {
                out.push(b'\n');
                i += 1;
            }
            b'r' => {
                out.push(b'\r');
                i += 1;
            }
            b't' => {
                out.push(b'\t');
                i += 1;
            }
            b'v' => {
                out.push(0x0B);
                i += 1;
            }
            b'0'..=b'7' => {
                let mut val = (bytes[i] - b'0') as u32;
                i += 1;
                for _ in 0..2 {
                    if let Some(&d) = bytes.get(i)
                        && (b'0'..=b'7').contains(&d)
                    {
                        val = val * 8 + (d - b'0') as u32;
                        i += 1;
                    } else {
                        break;
                    }
                }
                out.push((val & 0xFF) as u8);
            }
            b'x' => {
                if i + 2 >= bytes.len() {
                    return Err("truncated \\xXX escape".to_string());
                }
                let hex = std::str::from_utf8(&bytes[i + 1..=i + 2]).unwrap_or("");
                let v = u8::from_str_radix(hex, 16)
                    .map_err(|_| format!("invalid \\x escape: \\x{hex}"))?;
                out.push(v);
                i += 3;
            }
            other => {
                out.push(b'\\');
                out.push(other);
                i += 1;
            }
        }
    }
    Ok(out)
}
