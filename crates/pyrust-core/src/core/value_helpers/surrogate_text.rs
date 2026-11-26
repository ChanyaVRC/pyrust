// Surrogate-safe text primitives (issue #3315).
//
// pyrust stores a Python `str` as a Rust `&str`, except that a lone UTF-16
// surrogate (U+D800..=U+DFFF) is written as its three-byte CESU-8 sequence
// `ED [A0-BF] [80-BF]`.  Those bytes are not valid UTF-8, so every std API
// that decodes a `char` (`chars()`, `char_indices()`, `trim*()`,
// `split_whitespace()`, `to_lowercase()`, a `char`/closure `Pattern`, an
// empty-needle `&str` `Pattern`, ...) builds a `char` from a surrogate via
// `char::from_u32_unchecked`: undefined behaviour, which debug builds catch
// as a process-aborting precondition failure.
//
// The helpers below never materialise a `char` for a surrogate.  Byte-level
// std APIs stay sound on these strings: slicing at codepoint boundaries,
// `as_bytes()`, and non-empty `&str` substring search (`find`, `split`,
// `match_indices`, `starts_with`, ...), because CESU-8 surrogates keep
// UTF-8's self-synchronising lead/continuation byte structure.

/// Lead byte of every CESU-8 surrogate sequence (and of U+D000..=U+D7FF).
const SURROGATE_LEAD: u8 = 0xED;

/// Position of the first `0xED` byte in `bytes`: a word-at-a-time scan so the
/// surrogate check stays cheap on long non-ASCII text.
#[inline]
fn find_surrogate_lead(bytes: &[u8]) -> Option<usize> {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    const PATTERN: u64 = LO * SURROGATE_LEAD as u64;
    let mut i = 0;
    while let Some(chunk) = bytes.get(i..i + 8) {
        let word = u64::from_le_bytes(chunk.try_into().expect("8-byte chunk")) ^ PATTERN;
        // Exact "some byte of `word` is zero" test, i.e. some byte was 0xED.
        if word.wrapping_sub(LO) & !word & HI != 0 {
            break;
        }
        i += 8;
    }
    bytes[i..]
        .iter()
        .position(|&b| b == SURROGATE_LEAD)
        .map(|off| i + off)
}

/// Byte offset of the first CESU-8 surrogate at or after `from`.
#[inline]
fn find_surrogate_from(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while let Some(off) = find_surrogate_lead(&bytes[i..]) {
        let pos = i + off;
        // `ED 80..=9F` is valid UTF-8 (U+D000..=U+D7FF); `ED A0..=BF` is a
        // surrogate.
        if bytes.get(pos + 1).is_some_and(|&b| b >= 0xA0) {
            return Some(pos);
        }
        i = pos + 1;
    }
    None
}

/// Returns `true` when `s` holds at least one lone surrogate.
///
/// A `false` result means `s` is genuine UTF-8, so every `char`-decoding std
/// API is sound on it.  ASCII and most non-ASCII text are rejected by one
/// word-at-a-time scan for the `0xED` lead byte.
#[inline]
pub fn str_has_surrogates(s: &str) -> bool {
    find_surrogate_from(s.as_bytes(), 0).is_some()
}

/// One run of a string as split by [`str_segments`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrSegment<'a> {
    /// A maximal surrogate-free run: genuine UTF-8, safe for every std
    /// `char` API.
    Text(&'a str),
    /// One lone surrogate: `raw` is its three CESU-8 bytes, `cp` its value.
    Surrogate { raw: &'a str, cp: u32 },
}

/// Iterator returned by [`str_segments`].
#[derive(Clone, Debug)]
pub struct StrSegments<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Iterator for StrSegments<'a> {
    type Item = StrSegment<'a>;

    #[inline]
    fn next(&mut self) -> Option<StrSegment<'a>> {
        let s = self.s;
        let bytes = s.as_bytes();
        let pos = self.pos;
        if pos >= bytes.len() {
            return None;
        }
        if bytes[pos] == SURROGATE_LEAD && bytes.get(pos + 1).is_some_and(|&b| b >= 0xA0) {
            let end = (pos + 3).min(bytes.len());
            self.pos = end;
            let cp = cesu8_next_codepoint(s, pos).map_or(0xFFFD, |(cp, _)| cp);
            return Some(StrSegment::Surrogate {
                raw: &s[pos..end],
                cp,
            });
        }
        let end = find_surrogate_from(bytes, pos).unwrap_or(bytes.len());
        self.pos = end;
        Some(StrSegment::Text(&s[pos..end]))
    }
}

/// Split `s` into surrogate-free UTF-8 runs and individual lone surrogates.
///
/// A surrogate-free string yields exactly one `Text` segment covering all of
/// `s`, so a method that runs its existing `char`-based code per `Text`
/// segment behaves identically on ordinary strings.
#[inline]
pub fn str_segments(s: &str) -> StrSegments<'_> {
    StrSegments { s, pos: 0 }
}

/// Apply `map_text` to every surrogate-free run of `s`, copying lone
/// surrogates through unchanged, and return the concatenation.
///
/// This is the shape of every context-free (or surrogate-delimited) case
/// mapping: CPython leaves surrogates unchanged and treats them as uncased,
/// non-case-ignorable characters, which is exactly a segment boundary.
pub fn map_str_segments(s: &str, mut map_text: impl FnMut(&str, &mut String)) -> String {
    let mut out = String::with_capacity(s.len());
    for seg in str_segments(s) {
        match seg {
            StrSegment::Text(text) => map_text(text, &mut out),
            StrSegment::Surrogate { raw, .. } => out.push_str(raw),
        }
    }
    out
}

/// Decode the codepoint that ends at byte offset `end`, returning it and its
/// starting byte offset.  The reverse counterpart of [`cesu8_next_codepoint`].
#[inline]
pub fn cesu8_prev_codepoint(s: &str, end: usize) -> Option<(u32, usize)> {
    let bytes = s.as_bytes();
    if end == 0 || end > bytes.len() {
        return None;
    }
    let mut start = end - 1;
    while start > 0 && end - start < 4 && bytes[start] & 0xC0 == 0x80 {
        start -= 1;
    }
    cesu8_next_codepoint(s, start).map(|(cp, _)| (cp, start))
}

/// Number of Python codepoints in `s` (a lone surrogate counts as one).
#[inline]
pub fn str_codepoint_count(s: &str) -> usize {
    utf8_codepoint_count(s.as_bytes())
}

/// Byte offset of the codepoint boundary `n` codepoints into `s`, clamped to
/// `s.len()`.
#[inline]
pub fn str_codepoint_offset(s: &str, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut seen = 0usize;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if b & 0xC0 != 0x80 {
            if seen == n {
                return i;
            }
            seen += 1;
        }
    }
    s.len()
}

/// The first `n` codepoints of `s` (all of `s` when it is shorter).
#[inline]
pub fn str_codepoint_prefix(s: &str, n: usize) -> &str {
    &s[..str_codepoint_offset(s, n)]
}

/// Append codepoint `cp` (any value in `0..=0x10FFFF`, surrogates included)
/// to `out` in pyrust's string representation.
///
/// # Panics
///
/// Panics when `cp > 0x10FFFF`; callers validate the Python-visible range
/// (`range(0x110000)`) first.
#[inline]
pub fn push_codepoint(out: &mut String, cp: u32) {
    if let Some(c) = char::from_u32(cp) {
        out.push(c);
        return;
    }
    assert!(
        (0xD800..=0xDFFF).contains(&cp),
        "codepoint {cp:#x} is outside range(0x110000)"
    );
    // SAFETY: the three bytes are the well-formed CESU-8 encoding of a lone
    // surrogate, the representation pyrust uses for surrogate-bearing strings
    // throughout the runtime (see `cesu8_encode_codepoint`).
    unsafe {
        out.as_mut_vec().extend_from_slice(&[
            0xE0 | (cp >> 12) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]);
    }
}
