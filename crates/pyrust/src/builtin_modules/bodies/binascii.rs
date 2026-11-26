// `binascii` — 100% native Rust implementation of CPython 3.12's `binascii`
// extension module (issues #3145, #3308).
//
// Hex, Base64, CRC-32, CRC-HQX (CCITT), uuencode, and quoted-printable codecs
// plus the `Error` / `Incomplete` exceptions are declared directly in the
// `pyrust_module!` block below; there is no Python-source injection.
//
// Reference: <https://docs.python.org/3/library/binascii.html>

use std::borrow::Cow;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBytesLike, PyIndex, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, builtin_data_backing, value_type_name_str};
use crate::value::{PyBigInt, PyBigIntSign, Value, ValueKind};
use pyrust_derive::pyrust_module;

// ── Error & buffer helpers ───────────────────────────────────────────────────

/// A `binascii.Error` message produced by a pure codec routine; converted to a
/// real `binascii.Error` instance by [`raise_binascii_error`] at the module
/// boundary.
#[derive(Debug)]
struct CodecError(String);

impl CodecError {
    fn new(msg: impl Into<String>) -> Self {
        CodecError(msg.into())
    }
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type CodecResult<T> = std::result::Result<T, CodecError>;

/// Instantiate `binascii.Error(msg)` from the module's own exception class.
fn raise_binascii_error(interp: &mut Interpreter, err: CodecError) -> PyError {
    if let Ok(module) = interp.load_module("binascii")
        && let Ok(cls) = interp.get_attr(&module, "Error")
        && let ValueKind::PyClass(cls_rc) = cls.kind()
    {
        return PyError::raised(crate::interpreter::instantiate_exception(
            Rc::clone(cls_rc),
            vec![Value::string(&err.0)],
        ));
    }
    PyError::named("ValueError", err.0)
}

fn type_error(msg: String) -> PyError {
    PyError::named("TypeError", msg)
}

/// Resolve a bytes-like argument (`bytes`, `bytearray`, `memoryview`, `array`,
/// PEP 688 exporters, and subclasses), borrowing `bytes` without a copy.
fn extract_bytes_like<'a>(interp: &mut Interpreter, val: &'a Value) -> Result<PyBytesLike<'a>> {
    if !PyBytesLike::matches(val) {
        return Err(type_error(format!(
            "a bytes-like object is required, not '{}'",
            value_type_name_str(val)
        )));
    }
    PyBytesLike::try_from_value_with_interp(interp, val, "binascii", "data")
}

fn ascii_only(s: &str) -> Result<Cow<'_, [u8]>> {
    if !s.is_ascii() {
        return Err(PyError::named(
            "ValueError",
            "string argument should contain only ASCII characters".to_string(),
        ));
    }
    Ok(Cow::Borrowed(s.as_bytes()))
}

/// Resolve an "ASCII string or bytes-like" argument (`a2b_*` input).
fn extract_ascii_buffer<'a>(interp: &mut Interpreter, val: &'a Value) -> Result<Cow<'a, [u8]>> {
    if let ValueKind::Str(s) = val.kind() {
        return ascii_only(s);
    }
    if let Some(backing) = builtin_data_backing(val)
        && let ValueKind::Str(s) = backing.kind()
    {
        return ascii_only(s).map(|b| Cow::Owned(b.into_owned()));
    }
    if !PyBytesLike::matches(val) {
        return Err(type_error(format!(
            "argument should be bytes, buffer or ASCII string, not '{}'",
            value_type_name_str(val)
        )));
    }
    Ok(Cow::Owned(
        PyBytesLike::try_from_value_with_interp(interp, val, "binascii", "data")?.into_vec(),
    ))
}

/// Truthiness of an optional boolean-ish argument (`PyObject_IsTrue`).
fn optional_truthy(interp: &mut Interpreter, arg: &OptValueRef<'_>, default: bool) -> Result<bool> {
    match arg {
        OptionalArg::Provided(v) => interp.truthy_value(v.0),
        OptionalArg::Omitted => Ok(default),
    }
}

fn bigint_low_u32(bi: &PyBigInt) -> u32 {
    let (sign, digits) = bi.to_u64_digits();
    let low = digits.first().copied().unwrap_or(0) as u32;
    if sign == PyBigIntSign::Minus {
        low.wrapping_neg()
    } else {
        low
    }
}

/// `unsigned int` converter with `bitwise=True`: any integer, wrapped to 32 bits.
fn index_to_u32_mask(idx: &PyIndex) -> u32 {
    match idx.as_i64() {
        Some(n) => n as u32,
        None => bigint_low_u32(&idx.to_bigint()),
    }
}

/// `int` (C `int`) converter: `OverflowError` outside the 32-bit signed range.
fn index_to_c_int(idx: &PyIndex) -> Result<i32> {
    idx.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        })
}

fn resolve_index(interp: &mut Interpreter, val: &Value) -> Result<PyIndex> {
    PyIndex::try_from_value_with_interp(interp, val, "binascii", "value")
}

// ── Hex encoding / decoding ──────────────────────────────────────────────────

const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

fn parse_hexlify_sep(sep_val: &Value) -> Result<u8> {
    // Match CPython 3.12 `_Py_strhex_bytes_with_sep` validation order:
    //   1. len(sep) -> TypeError "object of type 'X' has no len()"
    //   2. len != 1 -> ValueError "sep must be length 1."
    //   3. not str/bytes -> TypeError "sep must be str or bytes."
    //   4. str with ord(ch) > 255 -> ValueError "sep must be ASCII."
    //      (CPython checks PyUnicode_KIND == 1BYTE_KIND for bytes output.)
    let sep_len: usize = match sep_val.kind() {
        ValueKind::Str(s) => s.chars().count(),
        ValueKind::Bytes(rc) => rc.len(),
        ValueKind::List(v) => v.len(),
        ValueKind::Tuple(rc) => rc.len(),
        ValueKind::Dict(v) => v.len(),
        ValueKind::Set(v) => v.len(),
        _ => {
            if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(sep_val) {
                snap.len()
            } else if let Some(backing) = builtin_data_backing(sep_val) {
                match backing.kind() {
                    ValueKind::Str(s) => s.chars().count(),
                    ValueKind::Bytes(rc) => rc.len(),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "object of type '{}' has no len()",
                                value_type_name_str(sep_val)
                            ),
                        ));
                    }
                }
            } else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "object of type '{}' has no len()",
                        value_type_name_str(sep_val)
                    ),
                ));
            }
        }
    };
    if sep_len != 1 {
        return Err(PyError::named(
            "ValueError",
            "sep must be length 1.".to_string(),
        ));
    }
    let check_str = |s: &str| -> Result<u8> {
        let ch = s.chars().next().unwrap();
        let cp = ch as u32;
        if cp > 0xFF {
            return Err(PyError::named(
                "ValueError",
                "sep must be ASCII.".to_string(),
            ));
        }
        Ok(cp as u8)
    };
    match sep_val.kind() {
        ValueKind::Str(s) => check_str(s),
        ValueKind::Bytes(rc) => Ok(rc[0]),
        _ => {
            if let Some(backing) = builtin_data_backing(sep_val) {
                match backing.kind() {
                    ValueKind::Str(s) => return check_str(s),
                    ValueKind::Bytes(rc) => return Ok(rc[0]),
                    _ => {}
                }
            }
            Err(PyError::named(
                "TypeError",
                "sep must be str or bytes.".to_string(),
            ))
        }
    }
}

fn encode_hex_bytes(data: &[u8], sep: Option<u8>, bytes_per_sep: i32) -> Vec<u8> {
    let n = data.len();
    if n == 0 {
        return Vec::new();
    }
    let Some(sep_byte) = sep.filter(|_| bytes_per_sep != 0) else {
        let mut out = Vec::with_capacity(n * 2);
        for &b in data {
            out.push(HEX_LOWER[(b >> 4) as usize]);
            out.push(HEX_LOWER[(b & 0x0f) as usize]);
        }
        return out;
    };

    let group_size = bytes_per_sep.unsigned_abs() as usize;
    let num_seps = (n - 1) / group_size;
    let mut out = Vec::with_capacity(n * 2 + num_seps);

    if bytes_per_sep > 0 {
        let rem = n % group_size;
        let first_len = if rem == 0 { group_size } else { rem };
        for (idx, &b) in data.iter().enumerate() {
            if idx >= first_len && (idx - first_len) % group_size == 0 {
                out.push(sep_byte);
            }
            out.push(HEX_LOWER[(b >> 4) as usize]);
            out.push(HEX_LOWER[(b & 0x0f) as usize]);
        }
    } else {
        for (idx, &b) in data.iter().enumerate() {
            if idx > 0 && idx % group_size == 0 {
                out.push(sep_byte);
            }
            out.push(HEX_LOWER[(b >> 4) as usize]);
            out.push(HEX_LOWER[(b & 0x0f) as usize]);
        }
    }
    out
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn decode_hex_bytes(data: &[u8]) -> CodecResult<Vec<u8>> {
    if !data.len().is_multiple_of(2) {
        return Err(CodecError::new("Odd-length string"));
    }
    let mut out = Vec::with_capacity(data.len() / 2);
    for pair in data.as_chunks::<2>().0 {
        let (Some(hi), Some(lo)) = (hex_nibble(pair[0]), hex_nibble(pair[1])) else {
            return Err(CodecError::new("Non-hexadecimal digit found"));
        };
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

// ── Base64 encoding / decoding ───────────────────────────────────────────────

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const fn make_b64_decode_table() -> [u8; 256] {
    let mut table = [0xffu8; 256];
    let mut i = 0usize;
    while i < 64 {
        table[B64_TABLE[i] as usize] = i as u8;
        i += 1;
    }
    table
}

const B64_DECODE: [u8; 256] = make_b64_decode_table();

fn encode_base64(data: &[u8], newline: bool) -> Vec<u8> {
    let n = data.len();
    let encoded_len = n.div_ceil(3) * 4 + usize::from(newline);
    let mut out = Vec::with_capacity(encoded_len);
    let (chunks, rem) = data.as_chunks::<3>();
    for chunk in chunks {
        let v = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        out.push(B64_TABLE[((v >> 18) & 0x3f) as usize]);
        out.push(B64_TABLE[((v >> 12) & 0x3f) as usize]);
        out.push(B64_TABLE[((v >> 6) & 0x3f) as usize]);
        out.push(B64_TABLE[(v & 0x3f) as usize]);
    }
    match rem.len() {
        1 => {
            let v = u32::from(rem[0]) << 16;
            out.push(B64_TABLE[((v >> 18) & 0x3f) as usize]);
            out.push(B64_TABLE[((v >> 12) & 0x3f) as usize]);
            out.push(b'=');
            out.push(b'=');
        }
        2 => {
            let v = (u32::from(rem[0]) << 16) | (u32::from(rem[1]) << 8);
            out.push(B64_TABLE[((v >> 18) & 0x3f) as usize]);
            out.push(B64_TABLE[((v >> 12) & 0x3f) as usize]);
            out.push(B64_TABLE[((v >> 6) & 0x3f) as usize]);
            out.push(b'=');
        }
        _ => {}
    }
    if newline {
        out.push(b'\n');
    }
    out
}

fn decode_base64(data: &[u8], strict_mode: bool) -> CodecResult<Vec<u8>> {
    if strict_mode && !data.is_empty() && data[0] == b'=' {
        return Err(CodecError::new("Leading padding not allowed"));
    }

    let mut out = Vec::with_capacity((data.len() / 4) * 3 + 3);
    let mut quad_pos: usize = 0;
    let mut leftchar: u32 = 0;
    let mut pads: usize = 0;
    let mut padding_started = false;

    for (i, &c) in data.iter().enumerate() {
        if c == b'=' {
            padding_started = true;
            if strict_mode && quad_pos == 0 {
                return Err(CodecError::new("Excess padding not allowed"));
            }
            if quad_pos >= 2 {
                pads += 1;
                if quad_pos + pads >= 4 {
                    if strict_mode && i + 1 < data.len() {
                        return Err(CodecError::new("Excess data after padding"));
                    }
                    if quad_pos == 2 {
                        out.push(((leftchar >> 4) & 0xff) as u8);
                    } else {
                        out.push(((leftchar >> 10) & 0xff) as u8);
                        out.push(((leftchar >> 2) & 0xff) as u8);
                    }
                    quad_pos = 0;
                    break;
                }
            }
            continue;
        }

        let val = B64_DECODE[c as usize];
        if val == 0xff {
            if strict_mode {
                return Err(CodecError::new("Only base64 data is allowed"));
            }
            continue;
        }

        if strict_mode && padding_started {
            return Err(CodecError::new("Discontinuous padding not allowed"));
        }

        pads = 0;
        leftchar = (leftchar << 6) | u32::from(val);
        quad_pos += 1;
        if quad_pos == 4 {
            out.push(((leftchar >> 16) & 0xff) as u8);
            out.push(((leftchar >> 8) & 0xff) as u8);
            out.push((leftchar & 0xff) as u8);
            quad_pos = 0;
            leftchar = 0;
        }
    }

    if quad_pos != 0 {
        if quad_pos == 1 {
            let data_chars = (out.len() / 3) * 4 + 1;
            return Err(CodecError::new(format!(
                "Invalid base64-encoded string: number of data characters ({data_chars}) cannot be 1 more than a multiple of 4"
            )));
        }
        return Err(CodecError::new("Incorrect padding"));
    }

    Ok(out)
}

// ── CRC-32 (IEEE 802.3) & CRC-HQX (CCITT 16-bit) ────────────────────────────

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut c = i;
        let mut j = 0;
        while j < 8 {
            if (c & 1) != 0 {
                c = 0xedb8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            j += 1;
        }
        table[i as usize] = c;
        i += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = make_crc32_table();

fn compute_crc32(data: &[u8], init: u32) -> u32 {
    let mut crc = !init;
    for &b in data {
        crc = CRC32_TABLE[((crc ^ u32::from(b)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

const fn make_crc_hqx_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut j = 0;
        while j < 8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const CRC_HQX_TABLE: [u16; 256] = make_crc_hqx_table();

fn compute_crc_hqx(data: &[u8], init: u16) -> u16 {
    let mut crc = init;
    for &b in data {
        crc = (crc << 8) ^ CRC_HQX_TABLE[(((crc >> 8) as u8) ^ b) as usize];
    }
    crc
}

// ── UU encoding / decoding ───────────────────────────────────────────────────

fn uu_encode_char(val: u8, backtick: bool) -> u8 {
    let v = val & 0x3f;
    if backtick && v == 0 { b'`' } else { v + 0x20 }
}

fn encode_uu(data: &[u8], backtick: bool) -> CodecResult<Vec<u8>> {
    if data.len() > 45 {
        return Err(CodecError::new("At most 45 bytes at once"));
    }
    let mut out = Vec::with_capacity(2 + data.len().div_ceil(3) * 4);
    out.push(uu_encode_char(data.len() as u8, backtick));
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        let v = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(uu_encode_char(((v >> 18) & 0x3f) as u8, backtick));
        out.push(uu_encode_char(((v >> 12) & 0x3f) as u8, backtick));
        out.push(uu_encode_char(((v >> 6) & 0x3f) as u8, backtick));
        out.push(uu_encode_char((v & 0x3f) as u8, backtick));
    }
    out.push(b'\n');
    Ok(out)
}

fn decode_uu(data: &[u8]) -> CodecResult<Vec<u8>> {
    // Match CPython's `binascii_a2b_uu_impl` quirks (including empty input -> 32 NULs).
    let (first, rest) = match data.split_first() {
        Some((&f, r)) => (f, r),
        None => (0u8, &[][..]),
    };
    let bin_len = ((first.wrapping_sub(0x20)) & 0x3f) as usize;
    let mut out = Vec::with_capacity(bin_len);
    let mut idx = 0usize;
    let mut leftbits: u32 = 0;
    let mut leftchar: u32 = 0;

    while out.len() < bin_len {
        let this_ch = if idx < rest.len() {
            let c = rest[idx];
            idx += 1;
            if c == b'\n' || c == b'\r' {
                // Short line: CPython pads missing trailing chars with 0.
                idx = rest.len();
                0u8
            } else {
                let d = c.wrapping_sub(0x20) & 0x3f;
                if !(0x20..=0x60).contains(&c) {
                    return Err(CodecError::new("Illegal char"));
                }
                d
            }
        } else {
            0u8
        };
        leftchar = (leftchar << 6) | u32::from(this_ch);
        leftbits += 6;
        if leftbits >= 8 {
            leftbits -= 8;
            out.push(((leftchar >> leftbits) & 0xff) as u8);
            leftchar &= (1 << leftbits) - 1;
        }
    }

    // Validate remaining characters on the line up to \r / \n.
    while idx < rest.len() {
        let c = rest[idx];
        idx += 1;
        if c == b'\n' || c == b'\r' {
            break;
        }
        if c == b' ' || c == b'`' {
            continue;
        }
        // Check if CPython allows trailing padding chars within the same 4-char group.
        return Err(CodecError::new("Trailing garbage"));
    }

    Ok(out)
}

// ── Quoted-Printable encoding / decoding ─────────────────────────────────────

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
const QP_MAXLINESIZE: usize = 76;

fn qp_push_encoded(out: &mut Vec<u8>, b: u8) {
    out.push(b'=');
    out.push(HEX_UPPER[(b >> 4) as usize]);
    out.push(HEX_UPPER[(b & 0x0f) as usize]);
}

fn encode_qp(data: &[u8], quotetabs: bool, istext: bool, header: bool) -> Vec<u8> {
    let n = data.len();
    // Detect CRLF vs LF line endings when `istext` is True.
    let mut crlf = false;
    for i in 0..n {
        if data[i] == b'\n' {
            if i > 0 && data[i - 1] == b'\r' {
                crlf = true;
            }
            break;
        }
    }
    let eol: &[u8] = if crlf { b"\r\n" } else { b"\n" };

    let mut out = Vec::with_capacity(n + n / 8);
    let mut linelen: usize = 0;
    let mut i = 0usize;

    while i < n {
        let c = data[i];
        if istext && c == b'\n' {
            out.extend_from_slice(eol);
            linelen = 0;
            i += 1;
            continue;
        }
        if istext && c == b'\r' && i + 1 < n && data[i + 1] == b'\n' {
            out.extend_from_slice(eol);
            linelen = 0;
            i += 2;
            continue;
        }

        let next_is_eol = i + 1 == n
            || (istext
                && (data[i + 1] == b'\n'
                    || (data[i + 1] == b'\r' && i + 2 < n && data[i + 2] == b'\n')));

        if header && c == b' ' && !quotetabs && i + 1 < n {
            if linelen >= QP_MAXLINESIZE - 1 && !next_is_eol {
                out.push(b'=');
                out.extend_from_slice(eol);
                linelen = 0;
            }
            out.push(b'_');
            linelen += 1;
            i += 1;
            continue;
        }

        let needs_quote = c > 126
            || c == b'='
            || (header && c == b'_')
            || (c < 32 && !(c == b'\t' && !quotetabs) && !(istext && c == b'\r'))
            || ((c == b' ' || c == b'\t') && (quotetabs || next_is_eol))
            || (c == b'.' && linelen == 0 && next_is_eol);

        if needs_quote {
            if (next_is_eol && linelen + 3 > QP_MAXLINESIZE)
                || (!next_is_eol && linelen + 3 >= QP_MAXLINESIZE)
            {
                out.push(b'=');
                out.extend_from_slice(eol);
                linelen = 0;
            }
            qp_push_encoded(&mut out, c);
            linelen += 3;
        } else {
            if linelen >= QP_MAXLINESIZE - 1 && !next_is_eol {
                out.push(b'=');
                out.extend_from_slice(eol);
                linelen = 0;
            }
            out.push(c);
            linelen += 1;
        }
        i += 1;
    }

    out
}

fn decode_qp(data: &[u8], header: bool) -> Vec<u8> {
    let n = data.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0usize;
    while i < n {
        let c = data[i];
        if c == b'=' {
            if i + 1 == n {
                break;
            }
            if data[i + 1] == b'=' {
                out.push(b'=');
                i += 2;
                continue;
            }
            if data[i + 1] == b'\n' {
                i += 2;
                continue;
            }
            if i + 2 < n && data[i + 1] == b'\r' && data[i + 2] == b'\n' {
                i += 3;
                continue;
            }
            if i + 2 < n
                && let (Some(hi), Some(lo)) = (hex_nibble(data[i + 1]), hex_nibble(data[i + 2]))
            {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
            out.push(b'=');
            i += 1;
        } else if header && c == b'_' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}
// ── Shared function bodies ───────────────────────────────────────────────────

/// `b2a_hex` / `hexlify`: validates `data`, then `bytes_per_sep`, then `sep`,
/// matching Argument Clinic's conversion order followed by
/// `_Py_strhex_bytes_with_sep`.
fn hex_encode(
    interp: &mut Interpreter,
    data: &Value,
    sep: &OptValueRef<'_>,
    bytes_per_sep: &OptValueRef<'_>,
) -> Result<Value> {
    let data = extract_bytes_like(interp, data)?;
    let bps = match bytes_per_sep {
        OptionalArg::Provided(v) => index_to_c_int(&resolve_index(interp, v.0)?)?,
        OptionalArg::Omitted => 1,
    };
    let sep = match sep {
        OptionalArg::Provided(v) => Some(parse_hexlify_sep(v.0)?),
        OptionalArg::Omitted => None,
    };
    Ok(Value::bytes(encode_hex_bytes(&data, sep, bps)))
}

fn hex_decode(interp: &mut Interpreter, hexstr: &Value) -> Result<Value> {
    let data = extract_ascii_buffer(interp, hexstr)?;
    decode_hex_bytes(&data)
        .map(Value::bytes)
        .map_err(|e| raise_binascii_error(interp, e))
}

/// `crc32` / `crc_hqx` shared argument handling: returns the data and the
/// 32-bit-masked initial value (`default` when omitted).
fn crc_args<'a>(
    interp: &mut Interpreter,
    data: &'a Value,
    crc: &OptValueRef<'_>,
    default: u32,
) -> Result<(PyBytesLike<'a>, u32)> {
    let data = extract_bytes_like(interp, data)?;
    let init = match crc {
        OptionalArg::Provided(v) => index_to_u32_mask(&resolve_index(interp, v.0)?),
        OptionalArg::Omitted => default,
    };
    Ok((data, init))
}

/// CPython's `binascii.Error` / `binascii.Incomplete` have `__doc__ = None`;
/// native exception classes only get a `__doc__` entry when they carry one.
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    for name in ["Error", "Incomplete"] {
        let cls = mod_rc.borrow().attrs.get(name).cloned();
        if let Some(cls) = cls
            && let ValueKind::PyClass(cls_rc) = cls.kind()
        {
            cls_rc
                .borrow_mut()
                .attrs
                .entry("__doc__".to_string())
                .or_insert_with(Value::none);
        }
    }
}

// ── Module definition ────────────────────────────────────────────────────────

pyrust_module! {
    constants {
        "__doc__" => Value::string("Conversion between binary data and ASCII"),
    }

    exception Error(ValueError);
    exception Incomplete;

    fn b2a_hex(
        data: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] sep: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] bytes_per_sep: OptValueRef<'_>,
    ) -> Result<Value> {
        hex_encode(_interp, data.0, &sep, &bytes_per_sep)
    }

    fn hexlify(
        data: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] sep: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] bytes_per_sep: OptValueRef<'_>,
    ) -> Result<Value> {
        hex_encode(_interp, data.0, &sep, &bytes_per_sep)
    }

    fn a2b_hex(#[positional_only] hexstr: PyValueRef<'_>) -> Result<Value> {
        hex_decode(_interp, hexstr.0)
    }

    fn unhexlify(#[positional_only] hexstr: PyValueRef<'_>) -> Result<Value> {
        hex_decode(_interp, hexstr.0)
    }

    fn b2a_base64(
        #[positional_only] data: PyValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] newline: OptValueRef<'_>,
    ) -> Result<Value> {
        let data = extract_bytes_like(_interp, data.0)?;
        let newline = optional_truthy(_interp, &newline, true)?;
        Ok(Value::bytes(encode_base64(&data, newline)))
    }

    fn a2b_base64(
        #[positional_only] data: PyValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] strict_mode: OptValueRef<'_>,
    ) -> Result<Value> {
        let data = extract_ascii_buffer(_interp, data.0)?;
        let strict_mode = optional_truthy(_interp, &strict_mode, false)?;
        decode_base64(&data, strict_mode)
            .map(Value::bytes)
            .map_err(|e| raise_binascii_error(_interp, e))
    }

    fn crc32(
        #[positional_only] data: PyValueRef<'_>,
        #[positional_only] #[default(OptionalArg::Omitted)] crc: OptValueRef<'_>,
    ) -> Result<Value> {
        let (data, init) = crc_args(_interp, data.0, &crc, 0)?;
        Ok(Value::int(i64::from(compute_crc32(&data, init))))
    }

    fn crc_hqx(
        #[positional_only] data: PyValueRef<'_>,
        #[positional_only] crc: PyValueRef<'_>,
    ) -> Result<Value> {
        let (data, init) = crc_args(_interp, data.0, &OptionalArg::Provided(crc), 0)?;
        Ok(Value::int(i64::from(compute_crc_hqx(&data, (init & 0xffff) as u16))))
    }

    fn b2a_uu(
        #[positional_only] data: PyValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] backtick: OptValueRef<'_>,
    ) -> Result<Value> {
        let data = extract_bytes_like(_interp, data.0)?;
        let backtick = optional_truthy(_interp, &backtick, false)?;
        encode_uu(&data, backtick)
            .map(Value::bytes)
            .map_err(|e| raise_binascii_error(_interp, e))
    }

    fn a2b_uu(#[positional_only] data: PyValueRef<'_>) -> Result<Value> {
        let data = extract_ascii_buffer(_interp, data.0)?;
        decode_uu(&data)
            .map(Value::bytes)
            .map_err(|e| raise_binascii_error(_interp, e))
    }

    fn b2a_qp(
        data: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] quotetabs: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] istext: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] header: OptValueRef<'_>,
    ) -> Result<Value> {
        let data = extract_bytes_like(_interp, data.0)?;
        let quotetabs = optional_truthy(_interp, &quotetabs, false)?;
        let istext = optional_truthy(_interp, &istext, true)?;
        let header = optional_truthy(_interp, &header, false)?;
        Ok(Value::bytes(encode_qp(&data, quotetabs, istext, header)))
    }

    fn a2b_qp(
        data: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] header: OptValueRef<'_>,
    ) -> Result<Value> {
        let data = extract_ascii_buffer(_interp, data.0)?;
        let header = optional_truthy(_interp, &header, false)?;
        Ok(Value::bytes(decode_qp(&data, header)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2b_base64_strict_rejects_excess_padding_after_full_quad() {
        for sample in [b"YWJj=".as_slice(), b"YWJj==", b"YWJj=a", b"YWJj==a"] {
            let err = decode_base64(sample, true).unwrap_err();
            assert!(
                err.to_string().contains("Excess padding not allowed"),
                "unexpected error for {:?}: {err}",
                sample
            );
        }
    }
}
