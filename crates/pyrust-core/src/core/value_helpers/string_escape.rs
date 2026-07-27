/// Choose the quote character for a string repr.  CPython prefers single
/// quotes; it switches to double quotes when the string contains a single
/// quote but no double quote (avoids backslash escapes in the common case).
fn repr_quote(s: &str) -> char {
    if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    }
}

/// Iterate over the codepoints of a string that may contain CESU-8-encoded
/// lone surrogates (0xD800–0xDFFF).  Rust's `str::chars()` calls
/// `char::from_u32_unchecked` internally; in debug builds that triggers an
/// undefined-behaviour precondition check and aborts when the decoded value is
/// a surrogate.  This iterator decodes the bytes manually and yields `u32`
/// values directly, avoiding `char` entirely for the surrogate range.
pub fn cesu8_codepoints(s: &str) -> impl Iterator<Item = u32> + '_ {
    let mut byte_offset = 0;
    std::iter::from_fn(move || {
        let (codepoint, next_offset) = cesu8_next_codepoint(s, byte_offset)?;
        byte_offset = next_offset;
        Some(codepoint)
    })
}

/// Decode one codepoint at `byte_offset`, returning the codepoint and the next
/// byte offset.  This is the cursor form of [`cesu8_codepoints`], used by lazy
/// string iterators so they do not materialise every one-character `Value`
/// before yielding the first item.
pub fn cesu8_next_codepoint(s: &str, byte_offset: usize) -> Option<(u32, usize)> {
    let bytes = s.as_bytes();
    let b0 = *bytes.get(byte_offset)?;
    let (cp, width) = if b0 < 0x80 {
        (b0 as u32, 1usize)
    } else if b0 < 0xE0 {
        let b1 = *bytes.get(byte_offset + 1)?;
        (((b0 & 0x1F) as u32) << 6 | (b1 & 0x3F) as u32, 2)
    } else if b0 < 0xF0 {
        let b1 = *bytes.get(byte_offset + 1)?;
        let b2 = *bytes.get(byte_offset + 2)?;
        (
            ((b0 & 0x0F) as u32) << 12 | ((b1 & 0x3F) as u32) << 6 | (b2 & 0x3F) as u32,
            3,
        )
    } else {
        let b1 = *bytes.get(byte_offset + 1)?;
        let b2 = *bytes.get(byte_offset + 2)?;
        let b3 = *bytes.get(byte_offset + 3)?;
        (
            ((b0 & 0x07) as u32) << 18
                | ((b1 & 0x3F) as u32) << 12
                | ((b2 & 0x3F) as u32) << 6
                | (b3 & 0x3F) as u32,
            4,
        )
    };
    Some((cp, byte_offset + width))
}

/// Encode a single codepoint into a one-character `String`, mirroring the
/// representation produced by [`cesu8_codepoints`].  Lone surrogates
/// (0xD800–0xDFFF) are written as their three-byte CESU-8 sequence directly,
/// since `char::from_u32` rejects them; every other value in 0..=0x10FFFF is a
/// valid Unicode scalar and goes through `char`.  This is the inverse of one
/// step of [`cesu8_codepoints`] and the surrogate-safe replacement for
/// `char::to_string()` when iterating a string that may hold lone surrogates.
pub fn cesu8_encode_codepoint(cp: u32) -> String {
    if (0xD800..=0xDFFF).contains(&cp) {
        // SAFETY: the three bytes are a well-formed CESU-8 encoding of a
        // surrogate codepoint, matching the representation pyrust uses for
        // surrogate-containing strings throughout the runtime.
        unsafe {
            String::from_utf8_unchecked(vec![
                0xE0 | (cp >> 12) as u8,
                0x80 | ((cp >> 6) & 0x3F) as u8,
                0x80 | (cp & 0x3F) as u8,
            ])
        }
    } else {
        char::from_u32(cp)
            .expect("non-surrogate codepoint is a valid char")
            .to_string()
    }
}

fn escape_str(s: &str, quote: char) -> String {
    let quote_u32 = quote as u32;
    let mut out = String::with_capacity(s.len());
    for n in cesu8_codepoints(s) {
        match n {
            0x5C => out.push_str("\\\\"), // '\\'
            0x0A => out.push_str("\\n"),  // '\n'
            0x09 => out.push_str("\\t"),  // '\t'
            0x0D => out.push_str("\\r"),  // '\r'
            n if n == quote_u32 => {
                out.push('\\');
                // SAFETY: quote is a valid char (it is either '\'' or '"').
                out.push(quote);
            }
            n if !cp_is_printable(n) => {
                if n <= 0xFF {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n <= 0xFFFF {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            n => {
                // n is printable and not a surrogate (surrogates are non-printable).
                // SAFETY: printable codepoints are not surrogates, so from_u32 is safe.
                if let Some(c) = char::from_u32(n) {
                    out.push(c);
                } else {
                    // Unreachable in well-formed input; escape as fallback.
                    out.push_str(&format!("\\u{n:04x}"));
                }
            }
        }
    }
    out
}

/// Returns `true` when codepoint `n` is considered "printable" by Python's
/// `str.isprintable()` / CPython's `Py_UNICODE_ISPRINTABLE`.
///
/// CPython considers a character non-printable when its Unicode general
/// category is one of: Cc (control), Cf (format), Cs (surrogate),
/// Co (private-use), Cn (unassigned), Zl/Zp (line/paragraph separators),
/// or any Zs (space separator) except ASCII space (U+0020).
///
/// Accepts a raw `u32` codepoint so that surrogate codepoints
/// (which are not valid Rust `char` values) can be tested without invoking
/// undefined behaviour via `char::from_u32_unchecked`.
#[inline]
pub fn cp_is_printable(n: u32) -> bool {
    if n == 0x20 {
        return true; // ASCII space is printable
    }
    // Lone surrogates (Cs category) are never printable.
    if (0xD800..=0xDFFF).contains(&n) {
        return false;
    }
    // For non-surrogate codepoints in the valid Unicode scalar range, delegate
    // to the char-based general_category lookup.
    match char::from_u32(n) {
        None => false, // out of Unicode range → not printable
        Some(c) => !matches!(
            c.general_category(),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
                | GeneralCategory::Unassigned
                | GeneralCategory::SpaceSeparator
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        ),
    }
}
