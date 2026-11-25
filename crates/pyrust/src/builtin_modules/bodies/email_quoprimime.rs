// `email.quoprimime` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Quoted-printable content transfer encoding per RFCs 2045-2047.
//
// Reference: <https://docs.python.org/3/library/email.quoprimime.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyBytesLike, PyIndex, PyStr, PyValueRef};
use crate::value::Value;

#[inline]
fn is_header_safe_literal(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'!' | b'*' | b'+' | b'/')
}

#[inline]
fn is_body_safe_literal(b: u8) -> bool {
    (0x20..=0x7E).contains(&b) && b != b'=' || b == b'\t'
}

fn push_hex_escape(out: &mut String, b: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('=');
    out.push(HEX[(b >> 4) as usize] as char);
    out.push(HEX[(b & 0x0F) as usize] as char);
}

#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn qp_header_length_bytes(data: &[u8]) -> usize {
    data.iter()
        .map(|&b| {
            if is_header_safe_literal(b) || b == b' ' {
                1
            } else {
                3
            }
        })
        .sum()
}

pub(crate) fn qp_body_length_bytes(data: &[u8]) -> usize {
    data.iter()
        .map(|&b| if is_body_safe_literal(b) { 1 } else { 3 })
        .sum()
}

pub(crate) fn qp_header_encode_bytes(data: &[u8], charset: &str) -> String {
    if data.is_empty() {
        return String::new();
    }
    let mut encoded = String::with_capacity(data.len() * 2);
    for &b in data {
        if is_header_safe_literal(b) {
            encoded.push(b as char);
        } else if b == b' ' {
            encoded.push('_');
        } else {
            push_hex_escape(&mut encoded, b);
        }
    }
    format!("=?{charset}?q?{encoded}?=")
}

/// Exact port of CPython 3.12's `email.quoprimime.body_encode`.
pub(crate) fn qp_body_encode_str(body: &str, maxlinelen: i64, eol: &str) -> Result<String> {
    if maxlinelen < 4 {
        return Err(PyError::named(
            "ValueError",
            "maxlinelen must be at least 4".to_string(),
        ));
    }
    if body.is_empty() {
        return Ok(String::new());
    }

    // Translate characters via _QUOPRI_BODY_ENCODE_MAP (\r and \n stay literal).
    let mut translated = String::with_capacity(body.len() + body.len() / 8);
    for ch in body.chars() {
        let code = ch as u32;
        if code < 256 {
            let b = code as u8;
            if is_body_safe_literal(b) || b == b'\r' || b == b'\n' {
                translated.push(b as char);
            } else {
                push_hex_escape(&mut translated, b);
            }
        } else {
            translated.push(ch);
        }
    }

    let soft_break = format!("={eol}");
    let max_len = maxlinelen as usize;
    let maxlinelen1 = max_len - 1;

    // Python str.splitlines() semantics (splitting on \r\n, \n, \r, etc.)
    let mut lines: Vec<&str> = Vec::new();
    {
        let bytes = translated.as_bytes();
        let n = bytes.len();
        let mut i = 0;
        let mut line_start = 0;
        while i < n {
            if bytes[i] == b'\r' {
                lines.push(&translated[line_start..i]);
                if i + 1 < n && bytes[i + 1] == b'\n' {
                    i += 2;
                } else {
                    i += 1;
                }
                line_start = i;
            } else if bytes[i] == b'\n' {
                lines.push(&translated[line_start..i]);
                i += 1;
                line_start = i;
            } else {
                i += 1;
            }
        }
        if line_start < n {
            lines.push(&translated[line_start..n]);
        }
    }

    let mut encoded_body: Vec<String> = Vec::with_capacity(lines.len() + 1);
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        let mut start: usize = 0;
        let laststart: isize = (len as isize) - 1 - (max_len as isize);
        while (start as isize) <= laststart {
            let stop = start + maxlinelen1;
            if chars[stop - 2] == '=' {
                let mut piece: String = chars[start..stop - 1].iter().collect();
                // Wait: in CPython `line[start:stop - 1]` already ends with '='!
                if !piece.ends_with('=') {
                    piece.push('=');
                }
                encoded_body.push(piece);
                start = stop - 2;
            } else if chars[stop - 1] == '=' {
                let piece: String = chars[start..stop].iter().collect();
                encoded_body.push(piece);
                start = stop - 1;
            } else {
                let mut piece: String = chars[start..stop].iter().collect();
                piece.push('=');
                encoded_body.push(piece);
                start = stop;
            }
        }

        if let Some(&last_ch) = chars.last()
            && (last_ch == ' ' || last_ch == '\t')
        {
            let room = (start as isize) - laststart;
            let q = if room >= 3 {
                let mut esc = String::with_capacity(3);
                push_hex_escape(&mut esc, last_ch as u8);
                esc
            } else if room == 2 {
                format!("{last_ch}{soft_break}")
            } else {
                let mut esc = String::with_capacity(3);
                push_hex_escape(&mut esc, last_ch as u8);
                format!("{soft_break}{esc}")
            };
            let mut piece: String = chars[start..len - 1].iter().collect();
            piece.push_str(&q);
            encoded_body.push(piece);
        } else {
            let piece: String = chars[start..].iter().collect();
            encoded_body.push(piece);
        }
    }

    if translated.ends_with('\r') || translated.ends_with('\n') {
        encoded_body.push(String::new());
    }

    Ok(encoded_body.join(eol))
}

/// Exact port of CPython 3.12's `email.quoprimime.decode`.
pub(crate) fn qp_decode_str(encoded: &str, eol: &str) -> String {
    if encoded.is_empty() {
        return String::new();
    }
    let mut decoded = String::with_capacity(encoded.len());

    // Split into lines (matching Python str.splitlines())
    let mut lines: Vec<&str> = Vec::new();
    {
        let bytes = encoded.as_bytes();
        let n = bytes.len();
        let mut i = 0;
        let mut line_start = 0;
        while i < n {
            if bytes[i] == b'\r' {
                lines.push(&encoded[line_start..i]);
                if i + 1 < n && bytes[i + 1] == b'\n' {
                    i += 2;
                } else {
                    i += 1;
                }
                line_start = i;
            } else if bytes[i] == b'\n' {
                lines.push(&encoded[line_start..i]);
                i += 1;
                line_start = i;
            } else {
                i += 1;
            }
        }
        if line_start < n {
            lines.push(&encoded[line_start..n]);
        }
    }

    for raw_line in lines {
        let line = raw_line.trim_end();
        if line.is_empty() {
            decoded.push_str(eol);
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let n = chars.len();
        let mut i = 0;
        while i < n {
            let c = chars[i];
            if c != '=' {
                decoded.push(c);
                i += 1;
            } else if i + 1 == n {
                i += 1;
                continue;
            } else if i + 2 < n
                && chars[i + 1].is_ascii()
                && chars[i + 2].is_ascii()
                && let (Some(h1), Some(h2)) =
                    (hex_val(chars[i + 1] as u8), hex_val(chars[i + 2] as u8))
            {
                decoded.push(((h1 << 4) | h2) as char);
                i += 3;
            } else {
                decoded.push(c);
                i += 1;
            }
            if i == n {
                decoded.push_str(eol);
            }
        }
    }

    if !encoded.ends_with('\r') && !encoded.ends_with('\n') && decoded.ends_with(eol) {
        decoded.pop();
    }
    decoded
}

pub(crate) fn qp_header_decode_str(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '_' {
            out.push(' ');
            i += 1;
        } else if c == '='
            && i + 2 < n
            && chars[i + 1].is_ascii()
            && chars[i + 2].is_ascii()
            && let (Some(h1), Some(h2)) = (hex_val(chars[i + 1] as u8), hex_val(chars[i + 2] as u8))
        {
            out.push(((h1 << 4) | h2) as char);
            i += 3;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn extract_octet(val: i64, fn_name: &str) -> Result<u8> {
    if !(0..=255).contains(&val) {
        return Err(PyError::named(
            "IndexError",
            format!("{fn_name}: octet out of range 0..255: {val}"),
        ));
    }
    Ok(val as u8)
}

fn all_list() -> Value {
    Value::list(vec![
        Value::string("body_decode"),
        Value::string("body_encode"),
        Value::string("body_length"),
        Value::string("decode"),
        Value::string("decodestring"),
        Value::string("header_decode"),
        Value::string("header_encode"),
        Value::string("header_length"),
        Value::string("quote"),
        Value::string("unquote"),
    ])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
        "__all__" => all_list(),
        "CRLF" => Value::string("\r\n"),
        "NL" => Value::string("\n"),
        "EMPTYSTRING" => Value::string(""),
    }

    fn header_check(octet: PyIndex) -> Result<Value> {
        let b = extract_octet(octet.expect_i64(FN_NAME, "octet")?, FN_NAME)?;
        Ok(Value::bool_(!is_header_safe_literal(b)))
    }

    fn body_check(octet: PyIndex) -> Result<Value> {
        let b = extract_octet(octet.expect_i64(FN_NAME, "octet")?, FN_NAME)?;
        Ok(Value::bool_(!is_body_safe_literal(b)))
    }

    fn header_length(bytearray: PyBytesLike<'_>) -> Result<Value> {
        Ok(Value::int(qp_header_length_bytes(bytearray.as_bytes()) as i64))
    }

    fn body_length(bytearray: PyBytesLike<'_>) -> Result<Value> {
        Ok(Value::int(qp_body_length_bytes(bytearray.as_bytes()) as i64))
    }

    fn unquote(s: PyStr<'_>) -> Result<Value> {
        let chars: Vec<char> = s.0.chars().collect();
        if chars.len() < 3 {
            return Err(PyError::named(
                "ValueError",
                format!("invalid literal for int() with base 16: {:?}", s.0.as_ref()),
            ));
        }
        let sub: String = chars[1..3].iter().collect();
        let val = u32::from_str_radix(&sub, 16).map_err(|_| {
            PyError::named(
                "ValueError",
                format!("invalid literal for int() with base 16: {sub:?}"),
            )
        })?;
        let ch = char::from_u32(val)
            .ok_or_else(|| PyError::named("ValueError", "chr() arg not in range".to_string()))?;
        Ok(Value::string(ch.to_string()))
    }

    fn quote(c: PyStr<'_>) -> Result<Value> {
        let mut chars = c.0.chars();
        let Some(ch) = chars.next() else {
            return Err(PyError::named(
                "TypeError",
                "ord() expected a character, but string of length 0 found".to_string(),
            ));
        };
        if chars.next().is_some() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "ord() expected a character, but string of length {} found",
                    c.0.chars().count()
                ),
            ));
        }
        let code = ch as u32;
        if code > 255 {
            return Err(PyError::named(
                "IndexError",
                "list index out of range".to_string(),
            ));
        }
        let mut out = String::with_capacity(3);
        push_hex_escape(&mut out, code as u8);
        Ok(Value::string(out))
    }

    fn header_encode(
        header_bytes: PyBytesLike<'_>,
        #[default("iso-8859-1".into())] charset: PyStr<'_>,
    ) -> Result<Value> {
        Ok(Value::string(qp_header_encode_bytes(
            header_bytes.as_bytes(),
            charset.0.as_ref(),
        )))
    }

    fn body_encode(
        body: PyStr<'_>,
        #[default(76i64.into())] maxlinelen: PyIndex,
        #[default("\n".into())] eol: PyStr<'_>,
    ) -> Result<Value> {
        let max_len = maxlinelen.expect_i64(FN_NAME, "maxlinelen")?;
        Ok(Value::string(qp_body_encode_str(
            body.0.as_ref(),
            max_len,
            eol.0.as_ref(),
        )?))
    }

    fn decode(
        encoded: PyValueRef<'_>,
        #[default("\n".into())] eol: PyStr<'_>,
    ) -> Result<Value> {
        let s = encoded.0.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "decode() expected a string".to_string(),
            )
        })?;
        Ok(Value::string(qp_decode_str(s, eol.0.as_ref())))
    }

    fn body_decode(
        encoded: PyValueRef<'_>,
        #[default("\n".into())] eol: PyStr<'_>,
    ) -> Result<Value> {
        let s = encoded.0.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "body_decode() expected a string".to_string(),
            )
        })?;
        Ok(Value::string(qp_decode_str(s, eol.0.as_ref())))
    }

    fn decodestring(
        encoded: PyValueRef<'_>,
        #[default("\n".into())] eol: PyStr<'_>,
    ) -> Result<Value> {
        let s = encoded.0.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "decodestring() expected a string".to_string(),
            )
        })?;
        Ok(Value::string(qp_decode_str(s, eol.0.as_ref())))
    }

    fn header_decode(s: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(qp_header_decode_str(s.0.as_ref())))
    }
}
