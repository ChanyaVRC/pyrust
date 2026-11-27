// `fnmatch` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/fnmatch.py` (issues #3148, #3452).
//
// `translate()` is a line-by-line port of the CPython function and returns the
// exact regular expression string CPython emits (`(?s:...)\Z`, `(?>.*?fixed)`
// atomic groups for interior `*`, escaped set operators, ...).  Alongside the
// regex text the same pass records a small matcher program (`Program`), so
// `fnmatch`, `fnmatchcase` and `filter` match natively on code points without
// going through the (recursive, pure-Python) `re` engine.  The program is a
// faithful interpretation of the regex `translate()` emits: fixed prefix,
// leftmost-first placement of every interior `STAR fixed` pair (the atomic
// group makes that placement final), and a tail anchored with `\Z`.  Matching
// is therefore linear in the subject per fixed segment and can never go
// exponential or hit the recursion limit.
//
// Compiled programs are cached per interpreter in a bounded LRU
// (`functools.lru_cache(maxsize=32768, typed=True)` in CPython 3.12.3).  The
// private `_compile_pattern` helper keeps CPython's contract (it returns
// `re.compile(translate(pat)).match`) and loads `re` only when called.
//
// Reference: <https://docs.python.org/3.12/library/fnmatch.html>

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use pyrust_core::{cesu8_codepoints, push_codepoint};
use pyrust_derive::pyrust_module;

use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBytesLike};
use crate::interpreter::{
    ConsumerIterator, ExpandedCallArg, Interpreter, hash_value_with_interp, value_type_name_str,
};
use crate::value::{Value, ValueKind};

const MODULE_DOC: &str = "Filename matching with shell patterns.\n\n\
fnmatch(FILENAME, PATTERN) matches according to the local convention.\n\
fnmatchcase(FILENAME, PATTERN) always takes case in account.\n\n\
The functions operate by translating the pattern into a regular\n\
expression.  They cache the compiled regular expressions for speed.\n\n\
The function translate(PATTERN) returns a regular expression\n\
corresponding to PATTERN.  (It does not compile it.)\n";

/// `functools.lru_cache(maxsize=32768, typed=True)` on `_compile_pattern`.
const CACHE_MAX: usize = 32768;

// `def`s of `Lib/fnmatch.py`: CPython reports arity errors for these in the
// interpreter's `initialize_locals` wording, not the macro's.
const SIG_FNMATCH: Sig = Sig {
    qualname: "fnmatch",
    params: &["name", "pat"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_FILTER: Sig = Sig {
    qualname: "filter",
    params: &["names", "pat"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_FNMATCHCASE: Sig = Sig {
    qualname: "fnmatchcase",
    params: &["name", "pat"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_TRANSLATE: Sig = Sig {
    qualname: "translate",
    params: &["pat"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_COMPILE_PATTERN: Sig = Sig {
    qualname: "_compile_pattern",
    params: &["pat"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn all_list() -> Value {
    Value::list(
        ["filter", "fnmatch", "fnmatchcase", "translate"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// translate()
// ---------------------------------------------------------------------------

const C_STAR: u32 = '*' as u32;
const C_QUESTION: u32 = '?' as u32;
const C_LBRACKET: u32 = '[' as u32;
const C_RBRACKET: u32 = ']' as u32;
const C_BANG: u32 = '!' as u32;
const C_HYPHEN: u32 = '-' as u32;
const C_BACKSLASH: u32 = '\\' as u32;
const C_CARET: u32 = '^' as u32;

/// One matchable item of a translated pattern.
#[derive(Debug)]
enum Tok {
    /// A literal code point (`re.escape(c)`, `\[`).
    Lit(u32),
    /// `.` — any code point; the surrounding `(?s:...)` makes it match `\n`.
    Any,
    /// `(?!)` — an empty set never matches.
    Never,
    /// `[...]` / `[^...]`.
    Set {
        negated: bool,
        ranges: Vec<(u32, u32)>,
    },
}

impl Tok {
    #[inline]
    fn matches(&self, c: u32) -> bool {
        match self {
            Tok::Lit(l) => *l == c,
            Tok::Any => true,
            Tok::Never => false,
            Tok::Set { negated, ranges } => {
                ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negated
            }
        }
    }
}

/// One element of CPython's first-pass `res` list.
enum Piece {
    Star,
    /// The regex text of the item and its matcher.
    Item(String, Tok),
}

/// `re.escape(c)` of CPython 3.12: only these ASCII characters are escaped.
fn re_escape(c: u32) -> (String, Tok) {
    let mut text = String::new();
    let special = matches!(
        char::from_u32(c),
        Some(
            '(' | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '?'
                | '*'
                | '+'
                | '-'
                | '|'
                | '^'
                | '$'
                | '\\'
                | '.'
                | '&'
                | '~'
                | '#'
                | ' '
                | '\t'
                | '\n'
                | '\r'
                | '\x0b'
                | '\x0c'
        )
    );
    if special {
        text.push('\\');
    }
    push_codepoint(&mut text, c);
    (text, Tok::Lit(c))
}

/// Interpret the body of a `[...]` class exactly as `re` reads the text
/// `translate()` emits: an optional leading `^`, then `\x` escapes, literal
/// characters, and unescaped `-` as the range operator.
fn parse_class(stuff: &[u32]) -> Tok {
    let negated = stuff.first() == Some(&C_CARET);
    let mut p = usize::from(negated);
    let atom = |p: &mut usize| -> u32 {
        if stuff[*p] == C_BACKSLASH && *p + 1 < stuff.len() {
            *p += 2;
            stuff[*p - 1]
        } else {
            *p += 1;
            stuff[*p - 1]
        }
    };
    let mut ranges = Vec::new();
    while p < stuff.len() {
        let lo = atom(&mut p);
        if p + 1 < stuff.len() && stuff[p] == C_HYPHEN {
            p += 1;
            let hi = atom(&mut p);
            ranges.push((lo, hi));
        } else {
            ranges.push((lo, lo));
        }
    }
    Tok::Set { negated, ranges }
}

fn find_hyphen(pat: &[u32], from: usize, to: usize) -> Option<usize> {
    (from..to.min(pat.len())).find(|&k| pat[k] == C_HYPHEN)
}

/// First pass of CPython's `translate()`: the `res` list with `STAR`
/// placeholders.
fn translate_pieces(pat: &[u32]) -> Vec<Piece> {
    let n = pat.len();
    let mut res: Vec<Piece> = Vec::new();
    let mut i = 0;
    while i < n {
        let c = pat[i];
        i += 1;
        if c == C_STAR {
            // compress consecutive `*` into one
            if !matches!(res.last(), Some(Piece::Star)) {
                res.push(Piece::Star);
            }
        } else if c == C_QUESTION {
            res.push(Piece::Item(".".to_string(), Tok::Any));
        } else if c == C_LBRACKET {
            let mut j = i;
            if j < n && pat[j] == C_BANG {
                j += 1;
            }
            if j < n && pat[j] == C_RBRACKET {
                j += 1;
            }
            while j < n && pat[j] != C_RBRACKET {
                j += 1;
            }
            if j >= n {
                res.push(Piece::Item("\\[".to_string(), Tok::Lit(C_LBRACKET)));
                continue;
            }
            let mut stuff: Vec<u32> = Vec::new();
            if !pat[i..j].contains(&C_HYPHEN) {
                for &ch in &pat[i..j] {
                    if ch == C_BACKSLASH {
                        stuff.push(C_BACKSLASH);
                    }
                    stuff.push(ch);
                }
            } else {
                let mut chunks: Vec<Vec<u32>> = Vec::new();
                let mut ci = i;
                let mut k = if pat[i] == C_BANG { i + 2 } else { i + 1 };
                while let Some(found) = find_hyphen(pat, k, j) {
                    chunks.push(pat[ci..found].to_vec());
                    ci = found + 1;
                    k = found + 3;
                }
                let chunk = &pat[ci..j];
                if !chunk.is_empty() {
                    chunks.push(chunk.to_vec());
                } else if let Some(last) = chunks.last_mut() {
                    last.push(C_HYPHEN);
                }
                // Remove empty ranges -- invalid in RE.
                for k in (1..chunks.len()).rev() {
                    if let (Some(&a), Some(&b)) = (chunks[k - 1].last(), chunks[k].first())
                        && a > b
                    {
                        let mut merged = chunks[k - 1][..chunks[k - 1].len() - 1].to_vec();
                        merged.extend_from_slice(&chunks[k][1..]);
                        chunks[k - 1] = merged;
                        chunks.remove(k);
                    }
                }
                // Escape backslashes and hyphens for set difference (--).
                // Hyphens that create ranges shouldn't be escaped.
                for (idx, chunk) in chunks.iter().enumerate() {
                    if idx > 0 {
                        stuff.push(C_HYPHEN);
                    }
                    for &ch in chunk {
                        if ch == C_BACKSLASH || ch == C_HYPHEN {
                            stuff.push(C_BACKSLASH);
                        }
                        stuff.push(ch);
                    }
                }
            }
            // Escape set operations (&&, ~~ and ||).
            let mut escaped: Vec<u32> = Vec::with_capacity(stuff.len());
            for &ch in &stuff {
                if ch == '&' as u32 || ch == '~' as u32 || ch == '|' as u32 {
                    escaped.push(C_BACKSLASH);
                }
                escaped.push(ch);
            }
            let mut stuff = escaped;
            i = j + 1;
            if stuff.is_empty() {
                // Empty range: never match.
                res.push(Piece::Item("(?!)".to_string(), Tok::Never));
            } else if stuff == [C_BANG] {
                // Negated empty range: match any character.
                res.push(Piece::Item(".".to_string(), Tok::Any));
            } else {
                if stuff[0] == C_BANG {
                    stuff[0] = C_CARET;
                } else if stuff[0] == C_CARET || stuff[0] == C_LBRACKET {
                    stuff.insert(0, C_BACKSLASH);
                }
                let mut text = String::from("[");
                for &ch in &stuff {
                    push_codepoint(&mut text, ch);
                }
                text.push(']');
                res.push(Piece::Item(text, parse_class(&stuff)));
            }
        } else {
            let (text, tok) = re_escape(c);
            res.push(Piece::Item(text, tok));
        }
    }
    res
}

/// The matcher: `fixed (STAR fixed)*` split at the stars.
#[derive(Debug)]
struct Program {
    first: Vec<Tok>,
    /// Interior fixed segments (each preceded and followed by a star).
    mids: Vec<Vec<Tok>>,
    /// Tail after the last star; empty when the pattern ends in `*`.
    last: Vec<Tok>,
    has_star: bool,
}

impl Program {
    #[inline]
    fn seg_matches_at(seg: &[Tok], subject: &[u32], at: usize) -> bool {
        seg.iter()
            .zip(&subject[at..at + seg.len()])
            .all(|(t, &c)| t.matches(c))
    }

    fn is_match(&self, subject: &[u32]) -> bool {
        let n = subject.len();
        if !self.has_star {
            return n == self.first.len() && Self::seg_matches_at(&self.first, subject, 0);
        }
        if n < self.first.len() + self.last.len() {
            return false;
        }
        if !Self::seg_matches_at(&self.first, subject, 0) {
            return false;
        }
        let mut pos = self.first.len();
        let tail_start = n - self.last.len();
        for mid in &self.mids {
            // `(?>.*?fixed)`: the leftmost placement, committed (atomic).
            let limit = tail_start;
            let mut found = None;
            let mut p = pos;
            while p + mid.len() <= limit {
                if Self::seg_matches_at(mid, subject, p) {
                    found = Some(p);
                    break;
                }
                p += 1;
            }
            match found {
                Some(p) => pos = p + mid.len(),
                None => return false,
            }
        }
        pos <= tail_start && Self::seg_matches_at(&self.last, subject, tail_start)
    }
}

/// Second pass of CPython's `translate()`: the regex text and the matcher.
fn assemble(pieces: Vec<Piece>) -> (String, Program) {
    let mut regex = String::new();
    let mut first: Vec<Tok> = Vec::new();
    let mut mids: Vec<Vec<Tok>> = Vec::new();
    let mut last: Vec<Tok> = Vec::new();
    let mut has_star = false;
    let mut iter = pieces.into_iter().peekable();
    // Fixed pieces at the start?
    while let Some(Piece::Item(..)) = iter.peek() {
        if let Some(Piece::Item(text, tok)) = iter.next() {
            regex.push_str(&text);
            first.push(tok);
        }
    }
    // Now deal with STAR fixed STAR fixed ...
    while iter.next().is_some() {
        has_star = true;
        if iter.peek().is_none() {
            regex.push_str(".*");
            break;
        }
        let mut fixed_text = String::new();
        let mut fixed: Vec<Tok> = Vec::new();
        while let Some(Piece::Item(..)) = iter.peek() {
            if let Some(Piece::Item(text, tok)) = iter.next() {
                fixed_text.push_str(&text);
                fixed.push(tok);
            }
        }
        if iter.peek().is_none() {
            regex.push_str(".*");
            regex.push_str(&fixed_text);
            last = fixed;
        } else {
            regex.push_str("(?>.*?");
            regex.push_str(&fixed_text);
            regex.push(')');
            mids.push(fixed);
        }
    }
    (
        format!("(?s:{regex})\\Z"),
        Program {
            first,
            mids,
            last,
            has_star,
        },
    )
}

fn translate_codepoints(pat: &[u32]) -> (String, Program) {
    assemble(translate_pieces(pat))
}

// ---------------------------------------------------------------------------
// Pattern cache
// ---------------------------------------------------------------------------

/// `(is_bytes, raw bytes of the pattern)` — `typed=True` keeps `'a'` and
/// `b'a'` apart.
type CacheKey = (bool, Vec<u8>);

/// A bounded LRU of compiled programs (O(log n) per operation).
struct PatternCache {
    map: HashMap<Rc<CacheKey>, (Rc<Program>, u64)>,
    order: BTreeMap<u64, Rc<CacheKey>>,
    tick: u64,
}

impl PatternCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
        }
    }

    fn get(&mut self, key: &CacheKey) -> Option<Rc<Program>> {
        let (program, stamp) = self.map.get_mut(key)?;
        let program = Rc::clone(program);
        let old = *stamp;
        self.tick += 1;
        *stamp = self.tick;
        if let Some(key_rc) = self.order.remove(&old) {
            self.order.insert(self.tick, key_rc);
        }
        Some(program)
    }

    fn insert(&mut self, key: CacheKey, program: Rc<Program>) {
        while self.map.len() >= CACHE_MAX {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            self.map.remove(&oldest);
        }
        self.tick += 1;
        let key = Rc::new(key);
        self.order.insert(self.tick, Rc::clone(&key));
        self.map.insert(key, (program, self.tick));
    }
}

// `gil_local!` justification:
// (a) Why: the compiled-pattern cache holds `Rc` handles that are `!Send` and
//     is shared by every Python thread of one interpreter under the GIL.
// (b) Isolation: independent interpreters on separate OS threads each get
//     their own cache, so programs never leak between them.
pyrust_core::gil_local! {
    static PATTERNS: std::cell::RefCell<PatternCache> =
        std::cell::RefCell::new(PatternCache::new());
}

// ---------------------------------------------------------------------------
// Operand handling
// ---------------------------------------------------------------------------

/// The text of a `str` (or `str` subclass) value.
fn str_operand(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_owned());
    }
    crate::builtin_modules::_string::str_text(v)
}

/// A `bytes` value or a `bytes` subclass instance.
fn exact_bytes_operand(v: &Value) -> Option<Vec<u8>> {
    if let ValueKind::Bytes(rc) = v.kind() {
        return Some(rc.as_ref().clone());
    }
    let backing = crate::interpreter::builtin_data_backing(v)?;
    match backing.kind() {
        ValueKind::Bytes(rc) => Some(rc.as_ref().clone()),
        _ => None,
    }
}

/// Look up `pat`'s compiled program (`_compile_pattern(pat)`); the flag is
/// `isinstance(pat, bytes)`.
fn compile_pattern(interp: &mut Interpreter, pat: &Value) -> Result<(Rc<Program>, bool)> {
    let (is_bytes, raw): (bool, Vec<u8>) = if let Some(s) = str_operand(pat) {
        (false, s.into_bytes())
    } else if let Some(b) = exact_bytes_operand(pat) {
        (true, b)
    } else {
        // `lru_cache` hashes the argument first, then `translate(pat)` asks
        // for its `len()`.
        hash_value_with_interp(interp, pat)?;
        return Err(type_error(format!(
            "object of type '{}' has no len()",
            value_type_name_str(pat)
        )));
    };
    let key: CacheKey = (is_bytes, raw);
    if let Some(program) = PATTERNS.with(|cache| cache.borrow_mut().get(&key)) {
        return Ok((program, is_bytes));
    }
    let codepoints: Vec<u32> = if is_bytes {
        // `str(pat, 'ISO-8859-1')`
        key.1.iter().map(|&b| u32::from(b)).collect()
    } else {
        let text = std::str::from_utf8(&key.1).unwrap_or("");
        cesu8_codepoints(text).collect()
    };
    let program = Rc::new(translate_codepoints(&codepoints).1);
    PATTERNS.with(|cache| cache.borrow_mut().insert(key, Rc::clone(&program)));
    Ok((program, is_bytes))
}

/// `match(name) is not None` for the compiled pattern, with the `TypeError`s
/// `re` raises for a mismatching subject.
fn match_subject(
    interp: &mut Interpreter,
    program: &Program,
    is_bytes: bool,
    name: &Value,
    buf: &mut Vec<u32>,
) -> Result<bool> {
    buf.clear();
    if let Some(s) = name.as_str() {
        if is_bytes {
            return Err(type_error(
                "cannot use a bytes pattern on a string-like object",
            ));
        }
        buf.extend(cesu8_codepoints(s));
    } else if let Some(s) = str_operand(name) {
        if is_bytes {
            return Err(type_error(
                "cannot use a bytes pattern on a string-like object",
            ));
        }
        buf.extend(cesu8_codepoints(&s));
    } else if PyBytesLike::matches(name) {
        let data = PyBytesLike::try_from_value_with_interp(interp, name, "match", "string")?;
        if !is_bytes {
            return Err(type_error(
                "cannot use a string pattern on a bytes-like object",
            ));
        }
        buf.extend(data.as_bytes().iter().map(|&b| u32::from(b)));
    } else {
        return Err(type_error(format!(
            "expected string or bytes-like object, got '{}'",
            value_type_name_str(name)
        )));
    }
    Ok(program.is_match(buf))
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn module_value(interp: &mut Interpreter) -> Result<Value> {
    interp.load_module("fnmatch")
}

/// A module-level import (`os`, `posixpath`): the module attribute when
/// present (CPython resolves `import os` as a module global), otherwise the
/// imported module itself.
fn dep(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = module_value(interp)?;
    match interp.get_attr(&module, name) {
        Ok(value) => Ok(value),
        Err(_) => interp.load_module(name),
    }
}

/// `os.path.normcase(value)`.
fn normcase(interp: &mut Interpreter, value: Value) -> Result<Value> {
    let os = dep(interp, "os")?;
    let path = interp.get_attr(&os, "path")?;
    let normcase = interp.get_attr(&path, "normcase")?;
    call_pos(interp, normcase, &[value])
}

// ---------------------------------------------------------------------------
// translate() entry point
// ---------------------------------------------------------------------------

fn translate_value(pat: &Value) -> Result<Value> {
    if let Some(s) = str_operand(pat) {
        let codepoints: Vec<u32> = cesu8_codepoints(&s).collect();
        return Ok(Value::string(translate_codepoints(&codepoints).0));
    }
    let bytes = exact_bytes_operand(pat).or_else(|| {
        PyBytesLike::matches(pat)
            .then(|| PyBytesLike::try_from_value(pat, "translate", "pat").ok())
            .flatten()
            .map(PyBytesLike::into_vec)
    });
    if let Some(bytes) = bytes {
        // `pat[i]` is an `int`, so the first character reaches `re.escape(int)`.
        if bytes.is_empty() {
            return Ok(Value::string("(?s:)\\Z"));
        }
        return Err(type_error(
            "decoding to str: need a bytes-like object, int found",
        ));
    }
    Err(type_error(format!(
        "object of type '{}' has no len()",
        value_type_name_str(pat)
    )))
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
    }

    // Test whether FILENAME matches PATTERN.
    //
    // Patterns are Unix shell style:
    //
    // *       matches everything
    // ?       matches any single character
    // [seq]   matches any character in seq
    // [!seq]  matches any char not in seq
    //
    // An initial period in FILENAME is not special.
    // Both FILENAME and PATTERN are first case-normalized
    // if the operating system requires it.
    // If you don't want this, use fnmatchcase(FILENAME, PATTERN).
    fn fnmatch(args) -> Result<Value> {
        let mut b = args::bind(&SIG_FNMATCH, args)?;
        let name = b.take_or_none(0);
        let pat = b.take_or_none(1);
        let name = normcase(_interp, name)?;
        let pat = normcase(_interp, pat)?;
        let module = module_value(_interp)?;
        let fnmatchcase = _interp.get_attr(&module, "fnmatchcase")?;
        call_pos(_interp, fnmatchcase, &[name, pat])
    }

    // Construct a list from those elements of the iterable NAMES that match PAT.
    fn filter(args) -> Result<Value> {
        let mut b = args::bind(&SIG_FILTER, args)?;
        let names = b.take_or_none(0);
        let pat = b.take_or_none(1);
        let mut result: Vec<Value> = Vec::new();
        let pat = normcase(_interp, pat)?;
        let (program, is_bytes) = compile_pattern(_interp, &pat)?;
        let os = dep(_interp, "os")?;
        let os_path = _interp.get_attr(&os, "path")?;
        let posixpath = dep(_interp, "posixpath")?;
        // `os.path is posixpath` in CPython.  Here `os.path` is its own
        // native module that mirrors `posixpath` on POSIX, so fall back to
        // `os.name` when the identity check cannot hold.
        let is_posix = if os_path.is_identical_to(&posixpath) {
            true
        } else {
            let os_name = _interp.get_attr(&os, "name")?;
            os_name.as_str() == Some("posix")
        };
        let mut iterator = ConsumerIterator::new(_interp, &names)?;
        let mut buf: Vec<u32> = Vec::new();
        while let Some(name) = iterator.next(_interp)? {
            let hit = if is_posix {
                match_subject(_interp, &program, is_bytes, &name, &mut buf)?
            } else {
                let normalized = normcase(_interp, name.clone())?;
                match_subject(_interp, &program, is_bytes, &normalized, &mut buf)?
            };
            if hit {
                result.push(name);
            }
        }
        Ok(Value::list(result))
    }

    // Test whether FILENAME matches PATTERN, including case.
    //
    // This is a version of fnmatch() which doesn't case-normalize
    // its arguments.
    fn fnmatchcase(args) -> Result<Value> {
        let mut b = args::bind(&SIG_FNMATCHCASE, args)?;
        let name = b.take_or_none(0);
        let pat = b.take_or_none(1);
        let (program, is_bytes) = compile_pattern(_interp, &pat)?;
        let mut buf: Vec<u32> = Vec::new();
        let hit = match_subject(_interp, &program, is_bytes, &name, &mut buf)?;
        Ok(Value::bool_(hit))
    }

    // Translate a shell PATTERN to a regular expression.
    //
    // There is no way to quote meta-characters.
    fn translate(args) -> Result<Value> {
        let mut b = args::bind(&SIG_TRANSLATE, args)?;
        let pat = b.take_or_none(0);
        translate_value(&pat)
    }

    // `re.compile(translate(pat)).match` (CPython caches this with
    // `functools.lru_cache`; the native matchers behind `fnmatchcase` and
    // `filter` use their own bounded cache instead).
    fn _compile_pattern(args) -> Result<Value> {
        let mut b = args::bind(&SIG_COMPILE_PATTERN, args)?;
        let pat = b.take_or_none(0);
        let (regex, is_bytes) = if let Some(s) = str_operand(&pat) {
            let codepoints: Vec<u32> = cesu8_codepoints(&s).collect();
            (translate_codepoints(&codepoints).0, false)
        } else if let Some(raw) = exact_bytes_operand(&pat) {
            let codepoints: Vec<u32> = raw.iter().map(|&b| u32::from(b)).collect();
            (translate_codepoints(&codepoints).0, true)
        } else {
            hash_value_with_interp(_interp, &pat)?;
            return Err(type_error(format!(
                "object of type '{}' has no len()",
                value_type_name_str(&pat)
            )));
        };
        let re = _interp.load_module("re")?;
        let compile = _interp.get_attr(&re, "compile")?;
        let source = if is_bytes {
            // `bytes(res_str, 'ISO-8859-1')`
            Value::bytes(regex.chars().map(|c| c as u32 as u8).collect())
        } else {
            Value::string(regex)
        };
        let compiled = call_pos(_interp, compile, &[source])?;
        _interp.get_attr(&compiled, "match")
    }
}

/// Wire the module-level imports (`os`, `posixpath`) so `fnmatch.os` exists
/// and can be patched, as in `Lib/fnmatch.py`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    for name in ["os", "posixpath"] {
        if let Ok(dep) = interp.load_module(name) {
            module_rc.borrow_mut().insert_attr(name.to_string(), dep);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cps(s: &str) -> Vec<u32> {
        s.chars().map(|c| c as u32).collect()
    }

    fn tr(pat: &str) -> String {
        translate_codepoints(&cps(pat)).0
    }

    fn m(pat: &str, name: &str) -> bool {
        translate_codepoints(&cps(pat)).1.is_match(&cps(name))
    }

    #[test]
    fn translate_matches_cpython_3_12() {
        assert_eq!(tr(""), "(?s:)\\Z");
        assert_eq!(tr("*"), "(?s:.*)\\Z");
        assert_eq!(tr("**"), "(?s:.*)\\Z");
        assert_eq!(tr("a*b"), "(?s:a.*b)\\Z");
        assert_eq!(tr("*a*b*c"), "(?s:(?>.*?a)(?>.*?b).*c)\\Z");
        assert_eq!(tr("a?b"), "(?s:a.b)\\Z");
        assert_eq!(tr("[abc]"), "(?s:[abc])\\Z");
        assert_eq!(tr("[!abc]"), "(?s:[^abc])\\Z");
        assert_eq!(tr("[]a]"), "(?s:[]a])\\Z");
        assert_eq!(tr("[!]]"), "(?s:[^]])\\Z");
        assert_eq!(tr("[a-z]"), "(?s:[a-z])\\Z");
        assert_eq!(tr("[a--z]"), "(?s:[z])\\Z");
        assert_eq!(tr("[a&&b]"), "(?s:[a\\&\\&b])\\Z");
        assert_eq!(tr("[^a]"), "(?s:[\\^a])\\Z");
        assert_eq!(tr("[a\\b]"), "(?s:[a\\\\b])\\Z");
        assert_eq!(tr("[abc"), "(?s:\\[abc)\\Z");
        assert_eq!(tr("[]"), "(?s:\\[\\])\\Z");
        assert_eq!(tr("[z-a]"), "(?s:(?!))\\Z");
        assert_eq!(tr("[!z-a]"), "(?s:.)\\Z");
        assert_eq!(tr("a.b+c"), "(?s:a\\.b\\+c)\\Z");
    }

    #[test]
    fn matching_follows_the_regex() {
        assert!(m("*", ""));
        assert!(m("*.py", "a.b.py"));
        assert!(!m("*.py", "a.pyc"));
        assert!(m("a*b*c", "aXbYbZc"));
        assert!(m("*a*b*", "xxaxxbxx"));
        assert!(!m("*ab", "aab "));
        assert!(m("?", "\n"));
        assert!(m("*", "a\nb"));
        assert!(m("[a-c]x", "bx"));
        assert!(!m("[!a-c]x", "bx"));
        assert!(m("[!a-c]x", "dx"));
        assert!(!m("[z-a]", "a"));
        assert!(m("[!z-a]", "q"));
        assert!(m("a*a", "aa"));
        assert!(!m("a*a", "a"));
        let long = "a".repeat(100);
        let pat = format!("{}b", "*a".repeat(20));
        assert!(!m(&pat, &long));
    }

    #[test]
    fn cache_is_bounded_and_lru() {
        let mut cache = PatternCache::new();
        let program = Rc::new(translate_codepoints(&cps("a")).1);
        for n in 0..(CACHE_MAX + 10) {
            cache.insert((false, n.to_string().into_bytes()), Rc::clone(&program));
        }
        assert_eq!(cache.map.len(), CACHE_MAX);
        assert_eq!(cache.order.len(), CACHE_MAX);
        assert!(cache.get(&(false, b"0".to_vec())).is_none());
        assert!(
            cache
                .get(&(false, (CACHE_MAX + 9).to_string().into_bytes()))
                .is_some()
        );
    }
}
