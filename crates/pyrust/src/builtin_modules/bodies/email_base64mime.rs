// `email.base64mime` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Base64 content transfer encoding per RFCs 2045-2047.
//
// Reference: <https://docs.python.org/3/library/email.base64mime.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBytesLike, PyIndex, PyStr, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard RFC 4648 base64 encoding without trailing newline.
pub(crate) fn b64_encode_bytes(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let (chunks, rem) = data.as_chunks::<3>();
    for c in chunks {
        let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32);
        out.push(B64_ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64_ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(B64_ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push(B64_ALPHABET[(n & 0x3F) as usize] as char);
    }
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(B64_ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(B64_ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(B64_ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(B64_ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push(B64_ALPHABET[((n >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

pub(crate) fn b64_header_length_usize(len: usize) -> usize {
    let groups_of_3 = len / 3;
    let leftover = len % 3;
    groups_of_3 * 4 + if leftover != 0 { 4 } else { 0 }
}

pub(crate) fn b64_body_encode_bytes(data: &[u8], maxlinelen: usize, eol: &str) -> Result<String> {
    if data.is_empty() {
        return Ok(String::new());
    }
    let max_unencoded = (maxlinelen * 3) / 4;
    if max_unencoded == 0 {
        return Err(PyError::named(
            "ValueError",
            "maxlinelen must be at least 4".to_string(),
        ));
    }
    let mut out = String::new();
    for chunk in data.chunks(max_unencoded) {
        out.push_str(&b64_encode_bytes(chunk));
        out.push_str(eol);
    }
    Ok(out)
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn decode_impl(interp: &mut Interpreter, string: &Value) -> Result<Value> {
    if let Some(s) = string.as_str() {
        if s.is_empty() {
            return Ok(Value::bytes(Vec::new()));
        }
        let enc_method = interp.get_attr(string, "encode")?;
        let raw_bytes = call_pos(interp, enc_method, &[Value::string("raw-unicode-escape")])?;
        let binascii = interp.load_module("binascii")?;
        let a2b = interp.get_attr(&binascii, "a2b_base64")?;
        return call_pos(interp, a2b, &[raw_bytes]);
    }
    let bytes_like = PyBytesLike::try_from_value_with_interp(interp, string, "decode", "string")?;
    if bytes_like.is_empty() {
        return Ok(Value::bytes(Vec::new()));
    }
    let binascii = interp.load_module("binascii")?;
    let a2b = interp.get_attr(&binascii, "a2b_base64")?;
    call_pos(interp, a2b, &[Value::bytes(bytes_like.as_bytes().to_vec())])
}

fn all_list() -> Value {
    Value::list(vec![
        Value::string("body_decode"),
        Value::string("body_encode"),
        Value::string("decode"),
        Value::string("decodestring"),
        Value::string("header_encode"),
        Value::string("header_length"),
    ])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
        "__all__" => all_list(),
        "CRLF" => Value::string("\r\n"),
        "NL" => Value::string("\n"),
        "EMPTYSTRING" => Value::string(""),
        "MISC_LEN" => Value::int(7),
    }

    fn header_length(bytearray: PyValueRef<'_>) -> Result<Value> {
        let len_val = call_pos(
            _interp,
            Value::builtin_function("len"),
            std::slice::from_ref(bytearray.0),
        )?;
        let n = len_val.as_int().unwrap_or(0).max(0) as usize;
        Ok(Value::int(b64_header_length_usize(n) as i64))
    }

    fn body_length(bytearray: PyValueRef<'_>) -> Result<Value> {
        let len_val = call_pos(
            _interp,
            Value::builtin_function("len"),
            std::slice::from_ref(bytearray.0),
        )?;
        let n = len_val.as_int().unwrap_or(0).max(0) as usize;
        Ok(Value::int(b64_header_length_usize(n) as i64))
    }

    fn header_encode(
        header_bytes: PyValueRef<'_>,
        #[default("iso-8859-1".into())] charset: PyStr<'_>,
    ) -> Result<Value> {
        let cs = charset.0.as_ref();
        let raw_bytes: Vec<u8> = if let Some(s) = header_bytes.0.as_str() {
            if s.is_empty() {
                return Ok(Value::string(""));
            }
            let enc_fn = _interp.get_attr(header_bytes.0, "encode")?;
            let b_val = call_pos(_interp, enc_fn, &[Value::string(cs)])?;
            let bl = PyBytesLike::try_from_value_with_interp(_interp, &b_val, FN_NAME, "header_bytes")?;
            bl.as_bytes().to_vec()
        } else {
            let bl = PyBytesLike::try_from_value_with_interp(
                _interp,
                header_bytes.0,
                FN_NAME,
                "header_bytes",
            )?;
            if bl.is_empty() {
                return Ok(Value::string(""));
            }
            bl.as_bytes().to_vec()
        };
        let encoded = b64_encode_bytes(&raw_bytes);
        Ok(Value::string(format!("=?{cs}?b?{encoded}?=")))
    }

    fn body_encode(
        s: PyValueRef<'_>,
        #[default(76i64.into())] maxlinelen: PyIndex,
        #[default("\n".into())] eol: PyStr<'_>,
    ) -> Result<Value> {
        let max_len = maxlinelen.expect_i64(FN_NAME, "maxlinelen")?;
        if max_len < 0 {
            return Err(PyError::named(
                "ValueError",
                "maxlinelen must be non-negative".to_string(),
            ));
        }
        let raw_bytes: Vec<u8> = if let Some(st) = s.0.as_str() {
            if st.is_empty() {
                return Ok(Value::string(""));
            }
            let mut v = Vec::with_capacity(st.len());
            for ch in st.chars() {
                let code = ch as u32;
                if code > 255 {
                    return Err(PyError::named(
                        "UnicodeEncodeError",
                        "body_encode expects bytes-like object".to_string(),
                    ));
                }
                v.push(code as u8);
            }
            v
        } else {
            let bl = PyBytesLike::try_from_value_with_interp(_interp, s.0, FN_NAME, "s")?;
            if bl.is_empty() {
                return Ok(Value::string(""));
            }
            bl.as_bytes().to_vec()
        };
        let encoded = b64_body_encode_bytes(&raw_bytes, max_len as usize, eol.0.as_ref())?;
        Ok(Value::string(encoded))
    }

    fn decode(string: PyValueRef<'_>) -> Result<Value> {
        decode_impl(_interp, string.0)
    }

    fn body_decode(string: PyValueRef<'_>) -> Result<Value> {
        decode_impl(_interp, string.0)
    }

    fn decodestring(string: PyValueRef<'_>) -> Result<Value> {
        decode_impl(_interp, string.0)
    }
}
