//! Python-compatible bytes-literal rendering for `bytearray.__repr__`.

/// Render a byte slice as a Python bytes literal (`b'...'`).  Used for
/// `bytearray.__repr__`, which wraps this in `bytearray(...)`.
pub(super) fn bytearray_bytes_repr(bytes: &[u8]) -> String {
    let has_single = bytes.contains(&b'\'');
    let has_double = bytes.contains(&b'"');
    let q = if has_single && !has_double { '"' } else { '\'' };
    let mut out = String::with_capacity(bytes.len() + 3);
    out.push('b');
    out.push(q);
    for &b in bytes {
        match b {
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x5c => out.push_str("\\\\"),
            b'\'' if q == '\'' => out.push_str("\\'"),
            b'"' if q == '"' => out.push_str("\\\""),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(q);
    out
}
