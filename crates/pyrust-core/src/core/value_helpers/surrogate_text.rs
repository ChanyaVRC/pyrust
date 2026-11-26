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

/// Byte offset of the first CESU-8 surrogate at or after `from`.
///
/// A surrogate is the byte pair `ED [A0-BF]`; `ED [80-9F]` is valid UTF-8
/// (Hangul U+D000..=U+D7FF), so the pair, not the lead byte alone, is what
/// must be matched.  The hot loop tests 32-byte windows with a branch-free
/// compare that LLVM vectorises, stepping 31 bytes so every adjacent pair
/// falls inside some window; Hangul-heavy text therefore costs no more than
/// any other text.  Only a window that contains a hit is rescanned bytewise.
#[inline]
fn find_surrogate_from(bytes: &[u8], from: usize) -> Option<usize> {
    const WINDOW: usize = 32;
    #[inline(always)]
    fn is_pair(lead: u8, next: u8) -> bool {
        (lead == SURROGATE_LEAD) & (next >= 0xA0)
    }
    let mut i = from;
    while let Some(window) = bytes.get(i..i + WINDOW) {
        let window: &[u8; WINDOW] = window.try_into().expect("32-byte window");
        let mut any = false;
        for k in 0..WINDOW - 1 {
            any |= is_pair(window[k], window[k + 1]);
        }
        if any {
            break;
        }
        i += WINDOW - 1;
    }
    let tail = bytes.get(i..)?;
    tail.windows(2)
        .position(|pair| is_pair(pair[0], pair[1]))
        .map(|off| i + off)
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
    let bytes = s.as_bytes();
    let mut seen = 0usize;
    let mut i = 0usize;
    // Skip whole 8-byte words while the target boundary lies beyond them,
    // counting codepoint lead bytes word-at-a-time.
    while let Some(chunk) = bytes.get(i..i + 8) {
        let starts =
            utf8_starts_in_word(u64::from_ne_bytes(chunk.try_into().expect("8-byte chunk")));
        if seen + starts > n {
            break;
        }
        seen += starts;
        i += 8;
    }
    for (j, &b) in bytes[i..].iter().enumerate() {
        if b & 0xC0 != 0x80 {
            if seen == n {
                return i + j;
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
