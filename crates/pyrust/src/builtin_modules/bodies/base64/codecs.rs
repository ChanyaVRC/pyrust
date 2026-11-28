// Pure byte-level codecs behind `base64`'s Base32 / Ascii85 / Base85
// functions.  They mirror the loops of CPython 3.12.3 `Lib/base64.py` and
// report failures as a [`Failure`] that the module converts into the Python
// exception the source raises.

use crate::error::PyError;
use crate::interpreter::Interpreter;

use super::support::{binascii_error, value_error};

pub(super) const B32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
pub(super) const B32_HEX_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
pub(super) const B85_ALPHABET: &[u8; 85] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";
pub(super) const A85_START: &[u8] = b"<~";
pub(super) const A85_END: &[u8] = b"~>";

/// An exception the Python source raises from a codec loop.
pub(super) enum Failure {
    /// `raise binascii.Error(msg)`.
    Binascii(&'static str),
    /// `raise ValueError(msg)`.
    Value(String),
    /// An exception raised by Python code the codec called back into.
    Py(PyError),
}

impl Failure {
    pub(super) fn into_error(self, interp: &mut Interpreter) -> PyError {
        match self {
            Failure::Binascii(msg) => binascii_error(interp, msg),
            Failure::Value(msg) => value_error(msg),
            Failure::Py(err) => err,
        }
    }
}

impl From<PyError> for Failure {
    fn from(err: PyError) -> Self {
        Failure::Py(err)
    }
}

// ---------------------------------------------------------------------------
// Base32
// ---------------------------------------------------------------------------

/// `_b32encode(alphabet, s)`.
pub(super) fn b32_encode(alphabet: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len().div_ceil(5) * 8);
    for quantum in data.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..quantum.len()].copy_from_slice(quantum);
        let c = buf.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
        for shift in (0..8).rev() {
            out.push(alphabet[((c >> (shift * 5)) & 0x1f) as usize]);
        }
    }
    // Adjust for any leftover partial quanta.
    let leftover = data.len() % 5;
    let n = out.len();
    let pad = match leftover {
        1 => 6,
        2 => 4,
        3 => 3,
        4 => 1,
        _ => 0,
    };
    for slot in &mut out[n - pad..] {
        *slot = b'=';
    }
    out
}

/// `_b32decode` after the argument conversions: `s` is already bytes and
/// `casefold` / `map01` have been applied by the caller.
pub(super) fn b32_decode(alphabet: &[u8], s: &[u8]) -> Result<Vec<u8>, Failure> {
    let mut rev = [0xffu8; 256];
    for (i, &c) in alphabet.iter().enumerate() {
        rev[c as usize] = i as u8;
    }
    // Strip off pad characters from the right.  We need to count the pad
    // characters because this will tell us how many null bytes to remove from
    // the end of the decoded string.
    let l = s.len();
    let stripped_len = s.iter().rposition(|&b| b != b'=').map_or(0, |i| i + 1);
    let body = &s[..stripped_len];
    let padchars = l - stripped_len;
    // Now decode the full quanta.
    let mut decoded: Vec<u8> = Vec::with_capacity(body.len().div_ceil(8) * 5);
    let mut acc: u64 = 0;
    for quanta in body.chunks(8) {
        acc = 0;
        for &c in quanta {
            let v = rev[c as usize];
            if v == 0xff {
                return Err(Failure::Binascii("Non-base32 digit found"));
            }
            acc = (acc << 5) + u64::from(v);
        }
        decoded.extend_from_slice(&acc.to_be_bytes()[3..]);
    }
    // Process the last, partial quanta.
    if !l.is_multiple_of(8) || !matches!(padchars, 0 | 1 | 3 | 4 | 6) {
        return Err(Failure::Binascii("Incorrect padding"));
    }
    if padchars != 0 && !decoded.is_empty() {
        acc <<= 5 * padchars;
        let last = &acc.to_be_bytes()[3..];
        let leftover = (43 - 5 * padchars) / 8; // 1: 4, 3: 3, 4: 2, 6: 1
        let n = decoded.len();
        decoded.truncate(n - 5);
        decoded.extend_from_slice(&last[..leftover]);
    }
    Ok(decoded)
}

// ---------------------------------------------------------------------------
// Ascii85 / Base85
// ---------------------------------------------------------------------------

/// `_85encode(b, chars, chars2, pad, foldnuls, foldspaces)`.
pub(super) fn encode_85(
    data: &[u8],
    chars: &[u8],
    pad: bool,
    foldnuls: bool,
    foldspaces: bool,
) -> Vec<u8> {
    let padding = (4 - data.len() % 4) % 4;
    let mut out = Vec::with_capacity((data.len() + padding) / 4 * 5);
    let mut last_start = 0usize;
    for quad in data.chunks(4) {
        let mut buf = [0u8; 4];
        buf[..quad.len()].copy_from_slice(quad);
        let word = u32::from_be_bytes(buf);
        last_start = out.len();
        if foldnuls && word == 0 {
            out.push(b'z');
        } else if foldspaces && word == 0x2020_2020 {
            out.push(b'y');
        } else {
            let mut digits = [0u8; 5];
            let mut w = word;
            for slot in digits.iter_mut().rev() {
                *slot = chars[(w % 85) as usize];
                w /= 85;
            }
            out.extend_from_slice(&digits);
        }
    }
    if padding != 0 && !pad {
        if &out[last_start..] == b"z" {
            out.truncate(last_start);
            out.extend_from_slice(&[chars[0]; 5]);
        }
        let n = out.len() - last_start;
        out.truncate(last_start + n.saturating_sub(padding));
    }
    out
}

/// `a85encode`'s framing and wrapping after the raw encoding.
pub(super) fn a85_frame(mut result: Vec<u8>, wrapcol: usize, adobe: bool) -> Vec<u8> {
    if adobe {
        let mut framed = A85_START.to_vec();
        framed.append(&mut result);
        result = framed;
    }
    if wrapcol != 0 {
        let wrapcol = wrapcol.max(if adobe { 2 } else { 1 });
        let mut chunks: Vec<&[u8]> = result.chunks(wrapcol).collect();
        if adobe && chunks.last().is_some_and(|c| c.len() + 2 > wrapcol) {
            chunks.push(b"");
        }
        result = chunks.join(&b'\n');
    }
    if adobe {
        result.extend_from_slice(A85_END);
    }
    result
}

/// The decoding loop of `a85decode` after the Adobe framing is stripped.
/// `is_ignored` answers `x in ignorechars`.
pub(super) fn a85_decode(
    data: &[u8],
    foldspaces: bool,
    mut is_ignored: impl FnMut(u8) -> Result<bool, PyError>,
) -> Result<Vec<u8>, Failure> {
    let mut decoded: Vec<u8> = Vec::with_capacity(data.len() / 5 * 4 + 4);
    let mut curr = [0u8; 5];
    let mut n = 0usize;
    for x in data.iter().copied().chain([b'u'; 4]) {
        match x {
            b'!'..=b'u' => {
                curr[n] = x;
                n += 1;
                if n == 5 {
                    let acc = curr
                        .iter()
                        .fold(0u64, |acc, &c| 85 * acc + u64::from(c - 33));
                    if acc > 0xffff_ffff {
                        return Err(Failure::Value("Ascii85 overflow".to_string()));
                    }
                    decoded.extend_from_slice(&(acc as u32).to_be_bytes());
                    n = 0;
                }
            }
            b'z' => {
                if n != 0 {
                    return Err(Failure::Value("z inside Ascii85 5-tuple".to_string()));
                }
                decoded.extend_from_slice(&[0; 4]);
            }
            b'y' if foldspaces => {
                if n != 0 {
                    return Err(Failure::Value("y inside Ascii85 5-tuple".to_string()));
                }
                decoded.extend_from_slice(&[0x20; 4]);
            }
            _ => {
                if is_ignored(x)? {
                    // Skip whitespace.
                    continue;
                }
                return Err(Failure::Value(format!(
                    "Non-Ascii85 digit found: {}",
                    char::from(x)
                )));
            }
        }
    }
    let padding = 4 - n;
    if padding != 0 {
        // Throw away the extra padding.
        let keep = decoded.len().saturating_sub(padding);
        decoded.truncate(keep);
    }
    Ok(decoded)
}

/// `b85decode` after the argument conversion.
pub(super) fn b85_decode(data: &[u8]) -> Result<Vec<u8>, Failure> {
    let mut dec = [0xffu8; 256];
    for (i, &c) in B85_ALPHABET.iter().enumerate() {
        dec[c as usize] = i as u8;
    }
    let padding = (5 - data.len() % 5) % 5;
    let mut out: Vec<u8> = Vec::with_capacity((data.len() + padding) / 5 * 4);
    let total = data.len() + padding;
    for i in (0..total).step_by(5) {
        let mut chunk = [b'~'; 5];
        let end = (i + 5).min(data.len());
        chunk[..end - i].copy_from_slice(&data[i..end]);
        let mut acc: u64 = 0;
        for (j, &c) in chunk.iter().enumerate() {
            let v = dec[c as usize];
            if v == 0xff {
                return Err(Failure::Value(format!(
                    "bad base85 character at position {}",
                    i + j
                )));
            }
            acc = acc * 85 + u64::from(v);
        }
        if acc > 0xffff_ffff {
            return Err(Failure::Value(format!(
                "base85 overflow in hunk starting at byte {i}"
            )));
        }
        out.extend_from_slice(&(acc as u32).to_be_bytes());
    }
    if padding != 0 {
        let keep = out.len().saturating_sub(padding);
        out.truncate(keep);
    }
    Ok(out)
}

/// `chars` table for `a85encode`: `!` .. `u`.
pub(super) fn a85_chars() -> Vec<u8> {
    (33u8..118).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b32_roundtrip_all_lengths() {
        let data: Vec<u8> = (0..40u8).collect();
        for n in 0..data.len() {
            let enc = b32_encode(B32_ALPHABET, &data[..n]);
            assert_eq!(enc.len() % 8, 0);
            let dec = b32_decode(B32_ALPHABET, &enc).ok().unwrap();
            assert_eq!(dec, &data[..n]);
        }
        assert_eq!(b32_encode(B32_ALPHABET, b"a"), b"ME======");
    }

    #[test]
    fn a85_known_vectors() {
        let chars = a85_chars();
        assert_eq!(encode_85(b"\0\0\0\0", &chars, false, true, false), b"z");
        assert_eq!(encode_85(b"\0", &chars, false, true, false), b"!!");
        assert_eq!(encode_85(b"    ", &chars, false, true, true), b"y");
        assert_eq!(a85_frame(b"abc".to_vec(), 0, true), b"<~abc~>");
    }

    #[test]
    fn b85_known_vectors() {
        assert_eq!(
            encode_85(b"hello", B85_ALPHABET, false, false, false),
            b"Xk~0{Zv"
        );
        assert_eq!(b85_decode(b"Xk~0{Zv").ok().unwrap(), b"hello");
    }
}
