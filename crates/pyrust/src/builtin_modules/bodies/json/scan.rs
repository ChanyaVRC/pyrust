//! Native JSON scanning: a byte-level port of `_json.c`'s `scan_once_unicode`
//! / `scanstring_unicode` (the C accelerator CPython's `json` uses by
//! default) plus `json.decoder.py_scanstring`.
//!
//! pyrust stores a `str` as UTF-8 with lone surrogates in CESU-8.  Every JSON
//! token is ASCII and multi-byte sequences never contain ASCII bytes, so the
//! scanner walks bytes and converts byte offsets to the code point indices
//! Python sees only at its boundaries: an index argument, a returned end
//! index, or an error position ([`Doc`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pyrust_core::{cesu8_next_codepoint, push_codepoint, value_from_bigint};

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyBigInt, PyDict, PyKey, Value, ValueKind};

use super::support::{self, C_RECURSION_LIMIT};

// ── Code point ⇄ byte offsets ────────────────────────────────────────────────

/// Code points between two samples of a [`TextIndex`].
const SAMPLE_STRIDE: usize = 64;

/// Code point ⇄ byte offset conversion for one `str`.
///
/// ASCII text maps indices to themselves.  Other text keeps the byte offset
/// of every [`SAMPLE_STRIDE`]-th code point, so a conversion scans at most
/// one stride of bytes instead of the whole prefix.
pub(crate) struct TextIndex {
    /// `None` for ASCII text.
    samples: Option<Vec<usize>>,
    char_len: usize,
}

#[inline]
fn is_lead(b: u8) -> bool {
    b & 0xC0 != 0x80
}

impl TextIndex {
    fn new(text: &str) -> Self {
        if text.is_ascii() {
            return Self {
                samples: None,
                char_len: text.len(),
            };
        }
        let mut samples = Vec::with_capacity(text.len() / SAMPLE_STRIDE + 1);
        let mut count = 0usize;
        for (i, &b) in text.as_bytes().iter().enumerate() {
            if is_lead(b) {
                if count.is_multiple_of(SAMPLE_STRIDE) {
                    samples.push(i);
                }
                count += 1;
            }
        }
        Self {
            samples: Some(samples),
            char_len: count,
        }
    }

    /// `len(s)`.
    pub(crate) fn char_len(&self) -> usize {
        self.char_len
    }

    /// Byte offset of code point `idx`, clamped to `text.len()`.
    pub(crate) fn byte_of(&self, text: &str, idx: usize) -> usize {
        let Some(samples) = &self.samples else {
            return idx.min(text.len());
        };
        if idx >= self.char_len {
            return text.len();
        }
        let mut pos = samples[idx / SAMPLE_STRIDE];
        let mut left = idx % SAMPLE_STRIDE;
        let bytes = text.as_bytes();
        while left > 0 {
            pos += 1;
            while pos < bytes.len() && !is_lead(bytes[pos]) {
                pos += 1;
            }
            left -= 1;
        }
        pos
    }

    /// Code point index of byte offset `byte` (a code point boundary).
    pub(crate) fn char_of(&self, text: &str, byte: usize) -> usize {
        let Some(samples) = &self.samples else {
            return byte;
        };
        let k = samples.partition_point(|&b| b <= byte).saturating_sub(1);
        let start = samples.get(k).copied().unwrap_or(0);
        let end = byte.min(text.len());
        let extra = text.as_bytes()[start..end]
            .iter()
            .filter(|&&b| is_lead(b))
            .count();
        k * SAMPLE_STRIDE + extra
    }
}

/// The index of the document a `py_make_scanner` scan is walking, shared by
/// the nested `JSONObject` / `JSONArray` / scanner calls of that scan.
#[derive(Default)]
struct PyScanCache {
    depth: usize,
    doc: Option<(Value, Rc<TextIndex>)>,
}

// (a) Why `thread_local!`-style storage: a `Value` is `!Send`; the pure-Python
//     scanner re-enters native code once per JSON value with the same `str`,
//     and each step converts several code point indices, so the index built
//     for the first step is reused by the rest of that scan.
// (b) Multi-interpreter / multi-threaded impact: interpreters on one GIL group
//     share the entry, keyed by object identity of an immutable `str`; it is
//     dropped as soon as the outermost `py_make_scanner` call returns, so no
//     document outlives the scan that read it.
pyrust_core::gil_local! {
    static PY_SCAN: RefCell<PyScanCache> = RefCell::new(PyScanCache::default());
}

/// Marks an active `py_make_scanner` call (see [`Doc::indexed`]).
pub(crate) struct PyScanGuard(());

impl PyScanGuard {
    pub(crate) fn enter() -> Self {
        PY_SCAN.with(|cache| cache.borrow_mut().depth += 1);
        PyScanGuard(())
    }
}

impl Drop for PyScanGuard {
    fn drop(&mut self) {
        PY_SCAN.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.depth = cache.depth.saturating_sub(1);
            if cache.depth == 0 {
                cache.doc = None;
            }
        });
    }
}

/// The [`TextIndex`] of the exact `str` object `owner` (whose text is `text`),
/// shared with the rest of an active `py_make_scanner` scan.
fn text_index(owner: &Value, text: &str) -> Rc<TextIndex> {
    let cached = PY_SCAN.with(|cache| {
        let cache = cache.borrow();
        if cache.depth == 0 {
            return None;
        }
        cache
            .doc
            .as_ref()
            .filter(|(doc, _)| doc.is_identical_to(owner))
            .map(|(_, index)| Rc::clone(index))
    });
    if let Some(index) = cached {
        return index;
    }
    let index = Rc::new(TextIndex::new(text));
    PY_SCAN.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.depth > 0 {
            cache.doc = Some((owner.clone(), Rc::clone(&index)));
        }
    });
    index
}

/// `c in ' \t\n\r'`.
#[inline]
pub(crate) fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// Skip JSON whitespace from byte offset `idx`.
#[inline]
pub(crate) fn skip_ws(bytes: &[u8], mut idx: usize) -> usize {
    while idx < bytes.len() && is_ws(bytes[idx]) {
        idx += 1;
    }
    idx
}

/// A document being scanned: the object reported as `JSONDecodeError.doc`
/// and the exact `str` holding its text.
pub(crate) struct Doc {
    pub(crate) value: Value,
    owner: Value,
    /// Sampled offsets for callers that convert many indices (the
    /// pure-Python helpers); `None` converts each index directly, which is
    /// cheapest for the native scanner's few boundary conversions.
    index: Option<Rc<TextIndex>>,
    char_len: std::cell::Cell<Option<usize>>,
}

impl Doc {
    /// `None` unless `value` is a `str` or `str` subclass instance.
    pub(crate) fn new(value: &Value) -> Option<Self> {
        let owner = support::exact_str(value)?;
        Some(Self {
            value: value.clone(),
            owner,
            index: None,
            char_len: std::cell::Cell::new(None),
        })
    }

    /// [`Doc::new`] with a [`TextIndex`], for the pure-Python helpers.
    pub(crate) fn indexed(value: &Value) -> Option<Self> {
        let mut doc = Self::new(value)?;
        let index = text_index(&doc.owner, doc.text());
        doc.char_len.set(Some(index.char_len()));
        doc.index = Some(index);
        Some(doc)
    }

    pub(crate) fn text(&self) -> &str {
        self.owner.as_str().expect("exact str")
    }

    pub(crate) fn char_of(&self, byte: usize) -> usize {
        match &self.index {
            Some(index) => index.char_of(self.text(), byte),
            None => pyrust_core::str_codepoint_count(&self.text()[..byte]),
        }
    }

    pub(crate) fn byte_of(&self, idx: usize) -> usize {
        match &self.index {
            Some(index) => index.byte_of(self.text(), idx),
            None => pyrust_core::str_codepoint_offset(self.text(), idx),
        }
    }

    pub(crate) fn char_len(&self) -> usize {
        if let Some(len) = self.char_len.get() {
            return len;
        }
        let len = pyrust_core::str_codepoint_count(self.text());
        self.char_len.set(Some(len));
        len
    }

    /// `JSONDecodeError(msg, doc, pos)` at code point `pos`.
    pub(crate) fn error_at(&self, interp: &mut Interpreter, msg: &str, pos: i64) -> PyError {
        support::decode_error(interp, msg, &self.value, Value::int(pos))
    }

    /// `JSONDecodeError(msg, doc, pos)` at byte offset `byte`.
    pub(crate) fn error_at_byte(
        &self,
        interp: &mut Interpreter,
        msg: &str,
        byte: usize,
    ) -> PyError {
        self.error_at(interp, msg, self.char_of(byte) as i64)
    }

    /// The code point at index `pos` (`s[pos]`), if in range.
    pub(crate) fn char_at(&self, pos: i64) -> Option<u32> {
        if pos < 0 || pos as usize >= self.char_len() {
            return None;
        }
        let byte = self.byte_of(pos as usize);
        cesu8_next_codepoint(self.text(), byte).map(|(cp, _)| cp)
    }

    /// `WHITESPACE.match(s, pos).end()`.
    pub(crate) fn ws_end(&self, pos: i64) -> i64 {
        let pos = pos.clamp(0, self.char_len() as i64) as usize;
        let start = self.byte_of(pos);
        let end = skip_ws(self.text().as_bytes(), start);
        (pos + (end - start)) as i64
    }
}

// ── scanstring ───────────────────────────────────────────────────────────────

/// A decoded JSON string: a byte range of the document (no escapes) or an
/// owned buffer.
pub(crate) enum Piece {
    Slice(usize, usize),
    Owned(String),
}

impl Piece {
    pub(crate) fn as_str<'a>(&'a self, text: &'a str) -> &'a str {
        match self {
            Piece::Slice(a, b) => &text[*a..*b],
            Piece::Owned(s) => s,
        }
    }

    pub(crate) fn into_value(self, text: &str) -> Value {
        match self {
            Piece::Slice(a, b) => Value::string(&text[a..b]),
            Piece::Owned(s) => Value::string(s),
        }
    }
}

fn hex_digit(b: u8) -> Option<u32> {
    (b as char).to_digit(16).filter(|_| b.is_ascii())
}

fn hex4(bytes: &[u8]) -> Option<u32> {
    bytes
        .iter()
        .try_fold(0u32, |acc, &b| hex_digit(b).map(|d| (acc << 4) | d))
}

/// `_json.c`'s `scanstring_unicode` from byte offset `start` (just after the
/// opening quote).  Returns the decoded string and the byte offset after the
/// closing quote.
pub(crate) fn scanstring_c(
    interp: &mut Interpreter,
    doc: &Doc,
    start: usize,
    strict: bool,
) -> Result<(Piece, usize)> {
    let text = doc.text();
    let b = text.as_bytes();
    let len = b.len();
    let unterminated = |interp: &mut Interpreter| {
        let begin = doc.char_of(start) as i64 - 1;
        doc.error_at(interp, "Unterminated string starting at", begin)
    };
    let mut out: Option<String> = None;
    let mut end = start;
    loop {
        let mut next = end;
        let mut terminator = None;
        while next < len {
            let d = b[next];
            if d == b'"' || d == b'\\' {
                terminator = Some(d);
                break;
            }
            if d <= 0x1f && strict {
                return Err(doc.error_at_byte(interp, "Invalid control character at", next));
            }
            next += 1;
        }
        let Some(terminator) = terminator else {
            return Err(unterminated(interp));
        };
        if terminator == b'"' && out.is_none() {
            return Ok((Piece::Slice(end, next), next + 1));
        }
        let buf = out.get_or_insert_with(String::new);
        buf.push_str(&text[end..next]);
        next += 1;
        if terminator == b'"' {
            end = next;
            break;
        }
        if next == len {
            return Err(unterminated(interp));
        }
        let c = b[next];
        if c != b'u' {
            let ch = match c {
                b'"' => '"',
                b'\\' => '\\',
                b'/' => '/',
                b'b' => '\x08',
                b'f' => '\x0c',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                _ => return Err(doc.error_at_byte(interp, "Invalid \\escape", next - 1)),
            };
            buf.push(ch);
            end = next + 1;
            continue;
        }
        next += 1;
        let end4 = next + 4;
        if end4 >= len {
            return Err(doc.error_at_byte(interp, "Invalid \\uXXXX escape", next - 1));
        }
        let Some(mut cp) = hex4(&b[next..end4]) else {
            return Err(doc.error_at_byte(interp, "Invalid \\uXXXX escape", next - 1));
        };
        end = end4;
        if (0xD800..=0xDBFF).contains(&cp) && end + 6 < len && b[end] == b'\\' && b[end + 1] == b'u'
        {
            let Some(c2) = hex4(&b[end + 2..end + 6]) else {
                return Err(doc.error_at_byte(interp, "Invalid \\uXXXX escape", end + 1));
            };
            if (0xDC00..=0xDFFF).contains(&c2) {
                cp = 0x10000 + (((cp - 0xD800) << 10) | (c2 - 0xDC00));
                end += 6;
            }
        }
        push_codepoint(buf, cp);
    }
    Ok((Piece::Owned(out.unwrap_or_default()), end))
}

/// `repr()` of the one-code-point string `cp`.
fn repr_codepoint(cp: u32) -> String {
    let mut s = String::new();
    push_codepoint(&mut s, cp);
    Value::string(s).repr_raw()
}

/// `json.decoder._decode_uXXXX(s, pos)` with `pos` the byte offset of the
/// `u`; returns the value and the byte offset after its four code points.
fn decode_uxxxx_py(interp: &mut Interpreter, doc: &Doc, pos: usize) -> Result<(i64, usize)> {
    let text = doc.text();
    let invalid =
        |interp: &mut Interpreter| doc.error_at_byte(interp, "Invalid \\uXXXX escape", pos);
    let mut cursor = pos + 1;
    let mut cps = Vec::with_capacity(4);
    while cps.len() < 4 {
        let Some((cp, next)) = cesu8_next_codepoint(text, cursor) else {
            break;
        };
        cps.push(cp);
        cursor = next;
    }
    if cps.len() < 4 || matches!(cps[1], 0x78 | 0x58) {
        return Err(invalid(interp));
    }
    if let Some(value) = hex4(&text.as_bytes()[pos + 1..cursor]).filter(|_| cursor == pos + 5) {
        return Ok((i64::from(value), cursor));
    }
    // `int(esc, 16)` also accepts surrounding whitespace, a sign, underscores
    // and non-ASCII decimal digits; defer to it for anything unusual.
    let int_type = support::builtin(interp, "int")?;
    match support::call(
        interp,
        int_type,
        &[Value::string(&text[pos + 1..cursor]), Value::int(16)],
    ) {
        Ok(value) => match value.kind() {
            ValueKind::Int(n) => Ok((n, cursor)),
            _ => Err(invalid(interp)),
        },
        Err(err) if err.class_name_is("ValueError") => Err(invalid(interp)),
        Err(err) => Err(err),
    }
}

/// `json.decoder.py_scanstring(s, end, strict)` (code point `end`).
pub(crate) fn scanstring_py(
    interp: &mut Interpreter,
    doc: &Doc,
    end: i64,
    strict: bool,
) -> Result<(Value, i64)> {
    let text = doc.text();
    let b = text.as_bytes();
    let len = b.len();
    let begin = end - 1;
    let unterminated =
        |interp: &mut Interpreter| doc.error_at(interp, "Unterminated string starting at", begin);
    if end > doc.char_len() as i64 {
        return Err(unterminated(interp));
    }
    let mut end = doc.byte_of(end.max(0) as usize);
    let mut out = String::new();
    loop {
        let Some(t) = b[end..]
            .iter()
            .position(|&d| d == b'"' || d == b'\\' || d < 0x20)
            .map(|p| end + p)
        else {
            return Err(unterminated(interp));
        };
        out.push_str(&text[end..t]);
        let terminator = b[t];
        end = t + 1;
        if terminator == b'"' {
            break;
        }
        if terminator != b'\\' {
            if strict {
                let msg = format!(
                    "Invalid control character {} at",
                    repr_codepoint(u32::from(terminator))
                );
                return Err(doc.error_at_byte(interp, &msg, end));
            }
            out.push(terminator as char);
            continue;
        }
        if end >= len {
            return Err(unterminated(interp));
        }
        let esc = b[end];
        if esc != b'u' {
            let ch = match esc {
                b'"' => '"',
                b'\\' => '\\',
                b'/' => '/',
                b'b' => '\x08',
                b'f' => '\x0c',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                _ => {
                    let cp = cesu8_next_codepoint(text, end).map_or(0, |(cp, _)| cp);
                    let msg = format!("Invalid \\escape: {}", repr_codepoint(cp));
                    return Err(doc.error_at_byte(interp, &msg, end));
                }
            };
            out.push(ch);
            end += 1;
            continue;
        }
        let (mut uni, after) = decode_uxxxx_py(interp, doc, end)?;
        end = after;
        if (0xD800..=0xDBFF).contains(&uni) && b[end..].starts_with(b"\\u") {
            let (uni2, after2) = decode_uxxxx_py(interp, doc, end + 1)?;
            if (0xDC00..=0xDFFF).contains(&uni2) {
                uni = 0x10000 + (((uni - 0xD800) << 10) | (uni2 - 0xDC00));
                end = after2;
            }
        }
        if !(0..=0x10FFFF).contains(&uni) {
            return Err(PyError::named(
                "ValueError",
                "chr() arg not in range(0x110000)".to_string(),
            ));
        }
        push_codepoint(&mut out, uni as u32);
    }
    Ok((Value::string(out), doc.char_of(end) as i64))
}

// ── The `_json.Scanner` decoder ──────────────────────────────────────────────

/// The decoder context a `_json.Scanner` captures when it is created.
#[derive(Clone)]
pub(crate) struct ScanConfig {
    pub(crate) strict: bool,
    pub(crate) object_hook: Value,
    pub(crate) object_pairs_hook: Value,
    pub(crate) parse_float: Value,
    pub(crate) parse_int: Value,
    pub(crate) parse_constant: Value,
}

impl ScanConfig {
    /// `scanner_new`: read the six attributes `_json.c` caches.
    pub(crate) fn from_context(interp: &mut Interpreter, ctx: &Value) -> Result<Self> {
        let strict = interp.get_attr(ctx, "strict")?;
        let strict = interp.truthy_value(&strict)?;
        Ok(Self {
            strict,
            object_hook: interp.get_attr(ctx, "object_hook")?,
            object_pairs_hook: interp.get_attr(ctx, "object_pairs_hook")?,
            parse_float: interp.get_attr(ctx, "parse_float")?,
            parse_int: interp.get_attr(ctx, "parse_int")?,
            parse_constant: interp.get_attr(ctx, "parse_constant")?,
        })
    }
}

/// Why a scan stopped without a value.
pub(crate) enum ScanError {
    /// `StopIteration(idx)` at byte offset `idx`: no JSON value starts there.
    Stop(usize),
    Raised(PyError),
}

impl From<PyError> for ScanError {
    fn from(err: PyError) -> Self {
        ScanError::Raised(err)
    }
}

type Scanned = std::result::Result<(Value, usize), ScanError>;

/// Whether `value` is the builtin type `name` itself (`float`, `int`).
fn is_builtin_type(value: &Value, name: &str) -> bool {
    match value.kind() {
        ValueKind::PyClass(class) => crate::interpreter::primitive_class_by_name(name)
            .is_some_and(|builtin| Rc::ptr_eq(class, &builtin)),
        ValueKind::BuiltinFunction(builtin) => builtin == name,
        _ => false,
    }
}

struct Scanner<'a> {
    interp: &'a mut Interpreter,
    cfg: &'a ScanConfig,
    doc: &'a Doc,
    float_fast: bool,
    int_fast: bool,
    /// `PyDict_SetDefault(memo, key, key)`: equal keys share one object.
    memo: HashMap<String, Value>,
    depth: usize,
}

/// Nested containers `_json.c` accepts before `_Py_EnterRecursiveCall`
/// fails (a few C frames are already in use when the scan starts).
const MAX_NESTING: usize = C_RECURSION_LIMIT - 3;

/// `scanner(string, idx)` at byte offset `start`: the value and the byte
/// offset where it ends.
pub(crate) fn scan_once(
    interp: &mut Interpreter,
    cfg: &ScanConfig,
    doc: &Doc,
    start: usize,
) -> std::result::Result<(Value, usize), ScanError> {
    let float_fast = is_builtin_type(&cfg.parse_float, "float");
    let int_fast = is_builtin_type(&cfg.parse_int, "int");
    let mut scanner = Scanner {
        interp,
        cfg,
        doc,
        float_fast,
        int_fast,
        memo: HashMap::new(),
        depth: 0,
    };
    scanner.scan(start)
}

impl Scanner<'_> {
    fn error(&mut self, msg: &str, byte: usize) -> ScanError {
        ScanError::Raised(self.doc.error_at_byte(self.interp, msg, byte))
    }

    fn scan(&mut self, idx: usize) -> Scanned {
        let b = self.doc.text().as_bytes();
        if idx >= b.len() {
            return Err(ScanError::Stop(idx));
        }
        let rest = &b[idx..];
        match b[idx] {
            b'"' => {
                let (piece, end) = scanstring_c(self.interp, self.doc, idx + 1, self.cfg.strict)?;
                Ok((piece.into_value(self.doc.text()), end))
            }
            b'{' => self.nested(idx + 1, true),
            b'[' => self.nested(idx + 1, false),
            b'n' if rest.starts_with(b"null") => Ok((Value::none(), idx + 4)),
            b't' if rest.starts_with(b"true") => Ok((Value::bool_(true), idx + 4)),
            b'f' if rest.starts_with(b"false") => Ok((Value::bool_(false), idx + 5)),
            b'N' if rest.starts_with(b"NaN") => self.constant("NaN", idx + 3),
            b'I' if rest.starts_with(b"Infinity") => self.constant("Infinity", idx + 8),
            b'-' if rest.starts_with(b"-Infinity") => self.constant("-Infinity", idx + 9),
            _ => self.number(idx),
        }
    }

    fn nested(&mut self, idx: usize, object: bool) -> Scanned {
        if self.depth >= MAX_NESTING {
            let what = if object { "object" } else { "array" };
            return Err(ScanError::Raised(support::recursion_error(&format!(
                "maximum recursion depth exceeded while decoding a JSON {what} from a unicode string"
            ))));
        }
        self.depth += 1;
        let result = support::with_stack(|| {
            if object {
                self.parse_object(idx)
            } else {
                self.parse_array(idx)
            }
        });
        self.depth -= 1;
        result
    }

    fn key(&mut self, start: usize) -> std::result::Result<(Value, usize), ScanError> {
        let (piece, end) = scanstring_c(self.interp, self.doc, start, self.cfg.strict)?;
        let text = piece.as_str(self.doc.text());
        if let Some(key) = self.memo.get(text) {
            return Ok((key.clone(), end));
        }
        let key = Value::string(text);
        self.memo.insert(text.to_owned(), key.clone());
        Ok((key, end))
    }

    fn parse_object(&mut self, mut idx: usize) -> Scanned {
        let b = self.doc.text().as_bytes();
        let len = b.len();
        let pairs_hook = !self.cfg.object_pairs_hook.is_none();
        let mut pairs: Vec<Value> = Vec::new();
        let mut dict = PyDict::default();
        idx = skip_ws(b, idx);
        if idx >= len || b[idx] != b'}' {
            loop {
                if idx >= len || b[idx] != b'"' {
                    return Err(
                        self.error("Expecting property name enclosed in double quotes", idx)
                    );
                }
                let (key, next) = self.key(idx + 1)?;
                idx = skip_ws(b, next);
                if idx >= len || b[idx] != b':' {
                    return Err(self.error("Expecting ':' delimiter", idx));
                }
                idx = skip_ws(b, idx + 1);
                let (value, next) = self.scan(idx)?;
                if pairs_hook {
                    pairs.push(Value::tuple(vec![key, value]));
                } else {
                    dict.insert(PyKey::Str(key), value);
                }
                idx = skip_ws(b, next);
                if idx < len && b[idx] == b'}' {
                    break;
                }
                if idx >= len || b[idx] != b',' {
                    return Err(self.error("Expecting ',' delimiter", idx));
                }
                idx = skip_ws(b, idx + 1);
            }
        }
        let end = idx + 1;
        if pairs_hook {
            let hook = self.cfg.object_pairs_hook.clone();
            let result = support::call(self.interp, hook, &[Value::list(pairs)])?;
            return Ok((result, end));
        }
        let obj = Value::dict(dict);
        if !self.cfg.object_hook.is_none() {
            let hook = self.cfg.object_hook.clone();
            let result = support::call(self.interp, hook, &[obj])?;
            return Ok((result, end));
        }
        Ok((obj, end))
    }

    fn parse_array(&mut self, mut idx: usize) -> Scanned {
        let b = self.doc.text().as_bytes();
        let len = b.len();
        let mut values = Vec::new();
        idx = skip_ws(b, idx);
        if idx >= len || b[idx] != b']' {
            loop {
                let (value, next) = self.scan(idx)?;
                values.push(value);
                idx = skip_ws(b, next);
                if idx < len && b[idx] == b']' {
                    break;
                }
                if idx >= len || b[idx] != b',' {
                    return Err(self.error("Expecting ',' delimiter", idx));
                }
                idx = skip_ws(b, idx + 1);
            }
        }
        Ok((Value::list(values), idx + 1))
    }

    fn constant(&mut self, name: &str, end: usize) -> Scanned {
        let parse_constant = self.cfg.parse_constant.clone();
        let value = support::call(self.interp, parse_constant, &[Value::string(name)])?;
        Ok((value, end))
    }

    /// `_match_number_unicode`.
    fn number(&mut self, start: usize) -> Scanned {
        let text = self.doc.text();
        let b = text.as_bytes();
        let end_idx = b.len() - 1;
        let mut idx = start;
        if b[idx] == b'-' {
            idx += 1;
            if idx > end_idx {
                return Err(ScanError::Stop(start));
            }
        }
        if (b'1'..=b'9').contains(&b[idx]) {
            idx += 1;
            while idx <= end_idx && b[idx].is_ascii_digit() {
                idx += 1;
            }
        } else if b[idx] == b'0' {
            idx += 1;
        } else {
            return Err(ScanError::Stop(start));
        }
        let mut is_float = false;
        if idx < end_idx && b[idx] == b'.' && b[idx + 1].is_ascii_digit() {
            is_float = true;
            idx += 2;
            while idx <= end_idx && b[idx].is_ascii_digit() {
                idx += 1;
            }
        }
        if idx < end_idx && (b[idx] == b'e' || b[idx] == b'E') {
            let e_start = idx;
            idx += 1;
            if idx < end_idx && (b[idx] == b'-' || b[idx] == b'+') {
                idx += 1;
            }
            while idx <= end_idx && b[idx].is_ascii_digit() {
                idx += 1;
            }
            if b[idx - 1].is_ascii_digit() {
                is_float = true;
            } else {
                idx = e_start;
            }
        }
        let numstr = &text[start..idx];
        let value = if is_float {
            if self.float_fast {
                Value::float(numstr.parse::<f64>().unwrap_or(f64::NAN))
            } else {
                let parse_float = self.cfg.parse_float.clone();
                support::call(self.interp, parse_float, &[Value::string(numstr)])?
            }
        } else if self.int_fast {
            parse_decimal_int(numstr)?
        } else {
            let parse_int = self.cfg.parse_int.clone();
            support::call(self.interp, parse_int, &[Value::string(numstr)])?
        };
        Ok((value, idx))
    }
}

/// `PyLong_FromString(numstr, NULL, 10)` for a JSON integer token.
fn parse_decimal_int(numstr: &str) -> Result<Value> {
    if numstr.len() <= 18
        && let Ok(n) = numstr.parse::<i64>()
    {
        return Ok(Value::int(n));
    }
    let digits = numstr.strip_prefix('-').unwrap_or(numstr);
    pyrust_core::check_int_parse_digits(digits, 10)?;
    if let Ok(n) = numstr.parse::<i64>() {
        return Ok(Value::int(n));
    }
    PyBigInt::parse_bytes(numstr.as_bytes(), 10)
        .map(value_from_bigint)
        .ok_or_else(|| PyError::runtime(format!("json: malformed integer {numstr:?}")))
}
