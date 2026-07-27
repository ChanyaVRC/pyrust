/// CP1252 (Windows-1252) is identical to Latin-1 except in 0x80..=0x9F, where it
/// maps to printable characters (the five undefined slots are `None`).  This
/// table holds the 0x80..=0x9F → Unicode codepoint mapping; `None` means the
/// byte is undefined (encoding/decoding raises).
const CP1252_HIGH: [Option<u32>; 32] = [
    Some(0x20AC), // 0x80 €
    None,         // 0x81
    Some(0x201A), // 0x82 ‚
    Some(0x0192), // 0x83 ƒ
    Some(0x201E), // 0x84 „
    Some(0x2026), // 0x85 …
    Some(0x2020), // 0x86 †
    Some(0x2021), // 0x87 ‡
    Some(0x02C6), // 0x88 ˆ
    Some(0x2030), // 0x89 ‰
    Some(0x0160), // 0x8A Š
    Some(0x2039), // 0x8B ‹
    Some(0x0152), // 0x8C Œ
    None,         // 0x8D
    Some(0x017D), // 0x8E Ž
    None,         // 0x8F
    None,         // 0x90
    Some(0x2018), // 0x91 ‘
    Some(0x2019), // 0x92 ’
    Some(0x201C), // 0x93 “
    Some(0x201D), // 0x94 ”
    Some(0x2022), // 0x95 •
    Some(0x2013), // 0x96 –
    Some(0x2014), // 0x97 —
    Some(0x02DC), // 0x98 ˜
    Some(0x2122), // 0x99 ™
    Some(0x0161), // 0x9A š
    Some(0x203A), // 0x9B ›
    Some(0x0153), // 0x9C œ
    None,         // 0x9D
    Some(0x017E), // 0x9E ž
    Some(0x0178), // 0x9F Ÿ
];

/// Map a Unicode codepoint to its CP1252 byte, or `None` if it is not
/// representable in CP1252.
fn cp1252_encode_byte(cp: u32) -> Option<u8> {
    // 0x00..=0x7F and 0xA0..=0xFF map straight through (== Latin-1).
    if cp < 0x80 || (0xA0..=0xFF).contains(&cp) {
        return Some(cp as u8);
    }
    // Search the high table for a matching codepoint.
    for (i, slot) in CP1252_HIGH.iter().enumerate() {
        if *slot == Some(cp) {
            return Some(0x80 + i as u8);
        }
    }
    None
}

/// Map a CP1252 byte to its Unicode codepoint, or `None` if the byte is
/// undefined.
pub fn cp1252_decode_codepoint(byte: u8) -> Option<u32> {
    if !(0x80..0xA0).contains(&byte) {
        Some(byte as u32)
    } else {
        CP1252_HIGH[(byte - 0x80) as usize]
    }
}

/// Encode a string to CP1252, honouring the `errors` handler (mirrors CPython's
/// `charmap` codec: undefined characters raise with reason
/// "character maps to <undefined>").
fn encode_cp1252(source: &str, errors: &str) -> Result<Value> {
    let cps: Vec<u32> = cesu8_codepoints(source).collect();
    let mut out = Vec::with_capacity(source.len());
    let mut idx = 0usize;
    while idx < cps.len() {
        let cp = cps[idx];
        if let Some(b) = cp1252_encode_byte(cp) {
            out.push(b);
            idx += 1;
            continue;
        }
        match errors {
            "ignore" => idx += 1,
            "replace" => {
                out.push(b'?');
                idx += 1;
            }
            "backslashreplace" => {
                out.extend_from_slice(&escape_codepoint_backslash(cp));
                idx += 1;
            }
            "xmlcharrefreplace" => {
                out.extend_from_slice(format!("&#{};", cp).as_bytes());
                idx += 1;
            }
            "strict" => {
                let run_start = idx;
                let mut run_end = idx + 1;
                while run_end < cps.len() && cp1252_encode_byte(cps[run_end]).is_none() {
                    run_end += 1;
                }
                return Err(PyError::UnicodeEncodeError {
                    encoding: "charmap".to_string(),
                    object: source.to_string(),
                    start: run_start,
                    end: run_end,
                    reason: "character maps to <undefined>".to_string(),
                });
            }
            other => {
                return Err(PyError::named(
                    "LookupError",
                    format!("unknown error handler name '{other}'"),
                ));
            }
        }
    }
    Ok(Value::bytes(out))
}

/// `\xHH` / `\uHHHH` / `\UHHHHHHHH` escape bytes for one codepoint.
fn escape_codepoint_backslash(cp: u32) -> Vec<u8> {
    if cp < 0x100 {
        format!("\\x{:02x}", cp).into_bytes()
    } else if cp < 0x10000 {
        format!("\\u{:04x}", cp).into_bytes()
    } else {
        format!("\\U{:08x}", cp).into_bytes()
    }
}

/// `str.encode('unicode_escape')` — Python string-escape representation.
///
/// Printable ASCII (0x20..=0x7E) emits literally except backslash (`\\`).
/// `\n`/`\t`/`\r` get their short escapes; all other codepoints become
/// `\xHH` / `\uHHHH` / `\UHHHHHHHH`.  Always succeeds (no error handler needed),
/// matching CPython.
fn encode_unicode_escape(source: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(source.len());
    for cp in cesu8_codepoints(source) {
        match cp {
            0x5C => out.extend_from_slice(b"\\\\"),
            0x0A => out.extend_from_slice(b"\\n"),
            0x09 => out.extend_from_slice(b"\\t"),
            0x0D => out.extend_from_slice(b"\\r"),
            0x20..=0x7E => out.push(cp as u8),
            _ => out.extend_from_slice(&escape_codepoint_backslash(cp)),
        }
    }
    out
}

/// `str.encode('raw_unicode_escape')` — like Latin-1, but codepoints >= 0x100
/// become `\uHHHH` / `\UHHHHHHHH` (bytes 0x00..=0xFF pass through raw).
fn encode_raw_unicode_escape(source: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(source.len());
    for cp in cesu8_codepoints(source) {
        if cp < 0x100 {
            out.push(cp as u8);
        } else if cp < 0x10000 {
            out.extend_from_slice(format!("\\u{:04x}", cp).as_bytes());
        } else {
            out.extend_from_slice(format!("\\U{:08x}", cp).as_bytes());
        }
    }
    out
}

const UTF7_B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// True for the UTF-7 "direct" character set (encoded as a single byte): the
/// whitespace controls `\t \n \r`, and printable ASCII 0x20..=0x7E except
/// `+` (0x2B), `\` (0x5C), and `~` (0x7E).  Matches CPython's encoder output.
fn utf7_is_direct(cp: u32) -> bool {
    matches!(cp, 0x09 | 0x0A | 0x0D)
        || (0x20..=0x7E).contains(&cp) && cp != 0x2B && cp != 0x5C && cp != 0x7E
}

/// True if `cp` is a modified-base64 alphabet byte (`[A-Za-z0-9+/]`).  Used to
/// decide whether a shifted section needs an explicit `-` shift-out before the
/// next direct character (CPython only emits `-` when the following byte could
/// otherwise be misread as continuing the base64 run).
fn utf7_is_b64(cp: u32) -> bool {
    matches!(cp, 0x41..=0x5A | 0x61..=0x7A | 0x30..=0x39) || cp == 0x2B || cp == 0x2F
}

/// `str.encode('utf-7')`.  Direct characters pass through; runs of other
/// characters are base64-encoded (of their UTF-16BE code units) inside `+...`.
/// A bare `+` becomes `+-`.  The closing `-` shift-out is emitted only when the
/// following byte is a base64 char or `-` (or at end of string), matching
/// CPython byte-for-byte.  A `+` encountered while already inside a shifted
/// section is folded into the running base64 (CPython does not break the run).
fn encode_utf7(source: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(source.len());
    let cps: Vec<u32> = cesu8_codepoints(source).collect();
    // Pending base64 bit accumulator for the active shifted section.
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    let mut shifted = false;

    // Flush the active shifted section, deciding the trailing `-` from `next`
    // (the codepoint that terminates the run, or `None` at end of string).
    fn close_shift(out: &mut Vec<u8>, acc: &mut u32, nbits: &mut u32, next: Option<u32>) {
        if *nbits > 0 {
            out.push(UTF7_B64[((*acc << (6 - *nbits)) & 0x3F) as usize]);
        }
        *acc = 0;
        *nbits = 0;
        // CPython emits the shift-out `-` at end of string, or when the next
        // direct char is itself a base64 char or `-`.
        let emit_dash = match next {
            None => true,
            Some(c) => c == 0x2D || utf7_is_b64(c),
        };
        if emit_dash {
            out.push(b'-');
        }
    }

    let push_unit = |out: &mut Vec<u8>, acc: &mut u32, nbits: &mut u32, unit: u16| {
        *acc = (*acc << 16) | unit as u32;
        *nbits += 16;
        while *nbits >= 6 {
            *nbits -= 6;
            out.push(UTF7_B64[((*acc >> *nbits) & 0x3F) as usize]);
        }
    };

    let mut idx = 0usize;
    while idx < cps.len() {
        let cp = cps[idx];
        // A direct char (when not already shifted) is emitted literally; if a
        // shifted section is open it must be closed first.
        if utf7_is_direct(cp) {
            if shifted {
                close_shift(&mut out, &mut acc, &mut nbits, Some(cp));
                shifted = false;
            }
            out.push(cp as u8);
            idx += 1;
            continue;
        }
        // A `+` outside a shifted section is the literal `+-`; inside a shifted
        // section it is just another code unit folded into the run.
        if cp == 0x2B && !shifted {
            out.extend_from_slice(b"+-");
            idx += 1;
            continue;
        }
        if !shifted {
            out.push(b'+');
            shifted = true;
        }
        // Surrogate codepoints encode as their own 16-bit unit; scalars may
        // produce a surrogate pair.
        if (0xD800..=0xDFFF).contains(&cp) {
            push_unit(&mut out, &mut acc, &mut nbits, cp as u16);
        } else if let Some(ch) = char::from_u32(cp) {
            let mut buf = [0u16; 2];
            for u in ch.encode_utf16(&mut buf) {
                push_unit(&mut out, &mut acc, &mut nbits, *u);
            }
        }
        idx += 1;
    }
    if shifted {
        close_shift(&mut out, &mut acc, &mut nbits, None);
    }
    out
}

// ---------------------------------------------------------------------------
// maketrans / translate
// ---------------------------------------------------------------------------
