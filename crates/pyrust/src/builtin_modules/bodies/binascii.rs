// `binascii` module — body for the `binascii @inject` entry in
// `pyrust_builtin_modules!` (issue #3145).
//
// Native Rust implementations for hex, Base64, CRC-32, CRC-HQX (CCITT),
// uuencode, and quoted-printable codecs live here; the public Python surface
// (`Error`, `Incomplete`, and exact CPython 3.12 function signatures) is
// injected from `binascii_py.py` by `inject_python_members`.
//
// Reference: <https://docs.python.org/3/library/binascii.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, reject_keyword_args_expanded,
    value_type_name_str,
};
use crate::value::{PyBigInt, PyBigIntSign, PyKey, PyToPrimitive, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the public `binascii` surface.
const BINASCII_PY_SOURCE: &str = include_str!("binascii_py.py");

/// Private native helpers seeded into the injected exec namespace and removed
/// from `binascii.__dict__` after injection so `dir(binascii)` matches CPython.
const BINASCII_NATIVE_HELPERS: [&str; 10] = [
    "_b2a_hex",
    "_a2b_hex",
    "_b2a_base64",
    "_a2b_base64",
    "_crc32",
    "_crc_hqx",
    "_b2a_uu",
    "_a2b_uu",
    "_b2a_qp",
    "_a2b_qp",
];

/// Public names exported from `BINASCII_PY_SOURCE` onto the `binascii` module.
const BINASCII_PY_EXPORTS: [&str; 14] = [
    "Error",
    "Incomplete",
    "a2b_base64",
    "a2b_hex",
    "a2b_qp",
    "a2b_uu",
    "b2a_base64",
    "b2a_hex",
    "b2a_qp",
    "b2a_uu",
    "crc32",
    "crc_hqx",
    "hexlify",
    "unhexlify",
];

/// Execute `BINASCII_PY_SOURCE` once, wire its wrappers to the native helpers,
/// hide the internal helper names from `dir(binascii)`, and export the public
/// CPython 3.12 `binascii` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in BINASCII_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(BINASCII_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("binascii: exec namespace not a dict".into()))?;
    for name in BINASCII_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("binascii"));
                class.error_name = match name {
                    "Error" => Some("binascii.Error"),
                    "Incomplete" => Some("binascii.Incomplete"),
                    _ => None,
                };
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Error & buffer helpers ───────────────────────────────────────────────────

fn raise_binascii_error(error_cls: &Value, msg: impl Into<String>) -> PyError {
    let msg = msg.into();
    if let ValueKind::PyClass(cls_rc) = error_cls.kind() {
        return PyError::Raised(crate::interpreter::instantiate_exception(
            Rc::clone(cls_rc),
            vec![Value::string(&msg)],
        ));
    }
    PyError::Runtime(msg)
}

fn try_extract_bytes_like(val: &Value) -> Option<Vec<u8>> {
    if let ValueKind::Bytes(rc) = val.kind() {
        return Some(rc.as_ref().clone());
    }
    if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
        return Some(snap);
    }
    if let Some(backing) = builtin_data_backing(val) {
        if let ValueKind::Bytes(rc) = backing.kind() {
            return Some(rc.as_ref().clone());
        }
        if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
            return Some(snap);
        }
    }
    // Support `array.array` instances (backed by `self._buf` bytearray).
    if let ValueKind::PyInstance(inst_rc) = val.kind() {
        let inst = inst_rc.borrow();
        let is_array = {
            let cls = inst.class.borrow();
            cls.name == "array"
                && cls
                    .attrs
                    .get("__module__")
                    .and_then(|v| v.as_str())
                    .is_some_and(|m| m == "array")
        };
        if is_array
            && let Some(buf) = inst.attrs.get("_buf")
            && let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(buf)
        {
            return Some(snap);
        }
    }
    None
}

fn extract_bytes_like(val: &Value) -> Result<Vec<u8>> {
    try_extract_bytes_like(val).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(val)
            ),
        )
    })
}

fn extract_ascii_buffer(val: &Value) -> Result<Vec<u8>> {
    if let ValueKind::Str(s) = val.kind() {
        if !s.is_ascii() {
            return Err(PyError::named(
                "ValueError",
                "string argument should contain only ASCII characters".to_string(),
            ));
        }
        return Ok(s.as_bytes().to_vec());
    }
    if let Some(backing) = builtin_data_backing(val)
        && let ValueKind::Str(s) = backing.kind()
    {
        if !s.is_ascii() {
            return Err(PyError::named(
                "ValueError",
                "string argument should contain only ASCII characters".to_string(),
            ));
        }
        return Ok(s.as_bytes().to_vec());
    }
    try_extract_bytes_like(val).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "argument should be bytes, buffer or ASCII string, not '{}'",
                value_type_name_str(val)
            ),
        )
    })
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

fn resolve_index_value(interp: &mut Interpreter, val: &Value) -> Result<Value> {
    match val.kind() {
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => Ok(val.clone()),
        ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
            if let Some(indexed) = interp.try_value_to_index(val)? {
                Ok(indexed)
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        value_type_name_str(val)
                    ),
                ))
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_u32_mask(interp: &mut Interpreter, val: &Value) -> Result<u32> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Int(n) => Ok(n as u32),
        ValueKind::Bool(b) => Ok(u32::from(b)),
        ValueKind::BigInt(bi) => Ok(bigint_low_u32(bi)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_c_int(interp: &mut Interpreter, val: &Value) -> Result<i32> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Bool(b) => Ok(i32::from(b)),
        ValueKind::Int(n) => i32::try_from(n).map_err(|_| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        }),
        ValueKind::BigInt(bi) => bi.to_i32().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
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

fn decode_hex_bytes(data: &[u8], error_cls: &Value) -> Result<Vec<u8>> {
    if !data.len().is_multiple_of(2) {
        return Err(raise_binascii_error(error_cls, "Odd-length string"));
    }
    let mut out = Vec::with_capacity(data.len() / 2);
    for pair in data.as_chunks::<2>().0 {
        let (Some(hi), Some(lo)) = (hex_nibble(pair[0]), hex_nibble(pair[1])) else {
            return Err(raise_binascii_error(
                error_cls,
                "Non-hexadecimal digit found",
            ));
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

fn decode_base64(data: &[u8], strict_mode: bool, error_cls: &Value) -> Result<Vec<u8>> {
    if strict_mode && !data.is_empty() && data[0] == b'=' {
        return Err(raise_binascii_error(
            error_cls,
            "Leading padding not allowed",
        ));
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
                return Err(raise_binascii_error(
                    error_cls,
                    "Excess padding not allowed",
                ));
            }
            if quad_pos >= 2 {
                pads += 1;
                if quad_pos + pads >= 4 {
                    if strict_mode && i + 1 < data.len() {
                        return Err(raise_binascii_error(error_cls, "Excess data after padding"));
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
                return Err(raise_binascii_error(
                    error_cls,
                    "Only base64 data is allowed",
                ));
            }
            continue;
        }

        if strict_mode && padding_started {
            return Err(raise_binascii_error(
                error_cls,
                "Discontinuous padding not allowed",
            ));
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
            return Err(raise_binascii_error(
                error_cls,
                format!(
                    "Invalid base64-encoded string: number of data characters ({data_chars}) cannot be 1 more than a multiple of 4"
                ),
            ));
        }
        return Err(raise_binascii_error(error_cls, "Incorrect padding"));
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

fn encode_uu(data: &[u8], backtick: bool, error_cls: &Value) -> Result<Vec<u8>> {
    if data.len() > 45 {
        return Err(raise_binascii_error(error_cls, "At most 45 bytes at once"));
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

fn decode_uu(data: &[u8], error_cls: &Value) -> Result<Vec<u8>> {
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
                    return Err(raise_binascii_error(error_cls, "Illegal char"));
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
        return Err(raise_binascii_error(error_cls, "Trailing garbage"));
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
            out.push(b'\n');
            linelen = 0;
            i += 1;
            continue;
        }
        if istext && c == b'\r' && i + 1 < n && data[i + 1] == b'\n' {
            out.extend_from_slice(b"\r\n");
            linelen = 0;
            i += 2;
            continue;
        }

        let next_is_eol = i + 1 == n
            || (istext
                && (data[i + 1] == b'\n'
                    || (data[i + 1] == b'\r' && i + 2 < n && data[i + 2] == b'\n')));

        if header && c == b' ' {
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
            || (c < 32 && !(c == b'\t' && !quotetabs))
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
            if i + 1 < n && data[i + 1] == b'\n' {
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

// ── Native module helpers ────────────────────────────────────────────────────

pyrust_module! {
    /// `_b2a_hex(data, sep, has_sep, bytes_per_sep)` -> `bytes`.
    fn _b2a_hex(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let data = extract_bytes_like(&args[0].value)?;
        let has_sep = _interp.truthy_value(&args[2].value)?;
        // CPython validates `bytes_per_sep` unconditionally before `sep`.
        let bps = extract_c_int(_interp, &args[3].value)?;
        let sep = if has_sep {
            Some(parse_hexlify_sep(&args[1].value)?)
        } else {
            None
        };
        Ok(Value::bytes(encode_hex_bytes(&data, sep, bps)))
    }

    /// `_a2b_hex(error_cls, hexstr)` -> `bytes`.
    fn _a2b_hex(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let data = extract_ascii_buffer(&args[1].value)?;
        Ok(Value::bytes(decode_hex_bytes(&data, error_cls)?))
    }

    /// `_b2a_base64(data, newline)` -> `bytes`.
    fn _b2a_base64(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let data = extract_bytes_like(&args[0].value)?;
        let newline = _interp.truthy_value(&args[1].value)?;
        Ok(Value::bytes(encode_base64(&data, newline)))
    }

    /// `_a2b_base64(error_cls, data, strict_mode)` -> `bytes`.
    fn _a2b_base64(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let data = extract_ascii_buffer(&args[1].value)?;
        let strict_mode = _interp.truthy_value(&args[2].value)?;
        Ok(Value::bytes(decode_base64(&data, strict_mode, error_cls)?))
    }

    /// `_crc32(data, value)` -> `int` (unsigned 32-bit CRC-32).
    fn _crc32(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let data = extract_bytes_like(&args[0].value)?;
        let init = extract_u32_mask(_interp, &args[1].value)?;
        Ok(Value::int(i64::from(compute_crc32(&data, init))))
    }

    /// `_crc_hqx(data, value)` -> `int` (unsigned 16-bit CRC-CCITT).
    fn _crc_hqx(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let data = extract_bytes_like(&args[0].value)?;
        let init = (extract_u32_mask(_interp, &args[1].value)? & 0xffff) as u16;
        Ok(Value::int(i64::from(compute_crc_hqx(&data, init))))
    }

    /// `_b2a_uu(error_cls, data, backtick)` -> `bytes`.
    fn _b2a_uu(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let data = extract_bytes_like(&args[1].value)?;
        let backtick = _interp.truthy_value(&args[2].value)?;
        Ok(Value::bytes(encode_uu(&data, backtick, error_cls)?))
    }

    /// `_a2b_uu(error_cls, data)` -> `bytes`.
    fn _a2b_uu(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let data = extract_ascii_buffer(&args[1].value)?;
        Ok(Value::bytes(decode_uu(&data, error_cls)?))
    }

    /// `_b2a_qp(data, quotetabs, istext, header)` -> `bytes`.
    fn _b2a_qp(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let data = extract_bytes_like(&args[0].value)?;
        let quotetabs = _interp.truthy_value(&args[1].value)?;
        let istext = _interp.truthy_value(&args[2].value)?;
        let header = _interp.truthy_value(&args[3].value)?;
        Ok(Value::bytes(encode_qp(&data, quotetabs, istext, header)))
    }

    /// `_a2b_qp(data, header)` -> `bytes`.
    fn _a2b_qp(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let data = extract_ascii_buffer(&args[0].value)?;
        let header = _interp.truthy_value(&args[1].value)?;
        Ok(Value::bytes(decode_qp(&data, header)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2b_base64_strict_rejects_excess_padding_after_full_quad() {
        for sample in [b"YWJj=".as_slice(), b"YWJj==", b"YWJj=a", b"YWJj==a"] {
            let err = decode_base64(sample, true, &Value::none()).unwrap_err();
            assert!(
                err.to_string().contains("Excess padding not allowed"),
                "unexpected error for {:?}: {err}",
                sample
            );
        }
    }
}
