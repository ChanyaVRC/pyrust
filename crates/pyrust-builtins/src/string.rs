use pyrust_core::{
    PyBigIntSign, PyDict, PyError, PyKey, Result, Value, ValueKind, builtin_type_name,
    cesu8_codepoints, cp_is_printable, expect_arg_count, extract_fill_char, extract_int,
    extract_optional_int, py_value_display_name,
};
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use crate::unicode_data;

/// Compute the byte offset of a subslice `sub` within its parent `parent`.
///
/// `sub` must be a contiguous subslice of `parent` (i.e. produced by Rust's
/// `split`, `split_whitespace`, `trim_*`, etc. applied to `parent`).  The
/// assertion is a safety net; in correct code it always holds.
#[inline(always)]
fn subslice_offset(parent: &str, sub: &str) -> usize {
    let off = sub.as_ptr() as usize - parent.as_ptr() as usize;
    debug_assert!(
        off + sub.len() <= parent.len(),
        "subslice_offset: sub ({off}..{}) is outside parent (..{})",
        off + sub.len(),
        parent.len()
    );
    off
}

/// Canonical list of method names dispatched by `call`.
/// Single source of truth for `has_method` and the drift-guard test.
///
/// Note: `format` is listed here so that `hasattr(s, "format")` and
/// `getattr(s, "format")` work correctly for str instances.  The bound-method
/// dispatch in `calls.rs` intercepts `"format"` before `call_str_method` to
/// route kwargs through `format_str_template`; the arm in `call` below is a
/// drift-guard stub that is never reached at runtime.
/// `format_map` is also listed and intercepted in `call_str_method` before
/// the fall-through to `pyrust_builtins::string::call`.
pub const METHODS: &[&str] = &[
    "__iter__",
    "index",
    "count",
    "split",
    "rsplit",
    "join",
    "splitlines",
    "partition",
    "rpartition",
    "strip",
    "lstrip",
    "rstrip",
    "removeprefix",
    "removesuffix",
    "center",
    "ljust",
    "rjust",
    "zfill",
    "expandtabs",
    "upper",
    "lower",
    "casefold",
    "capitalize",
    "swapcase",
    "title",
    "find",
    "rfind",
    "rindex",
    "replace",
    "format",
    "format_map",
    "startswith",
    "endswith",
    "isdigit",
    "isalpha",
    "isalnum",
    "isspace",
    "isdecimal",
    "isnumeric",
    "islower",
    "isupper",
    "istitle",
    "isascii",
    "isidentifier",
    "isprintable",
    "encode",
    "translate",
    "__getnewargs__",
];

/// Returns `true` if `method` is the name of a built-in `str` method.
/// Used by `hasattr` / `getattr` to validate attribute names without
/// invoking the method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Returns `true` if `method` must be evaluated directly by the VM dispatcher
/// rather than delegated to `call_str_method` / `call` below.
///
/// `format` needs keyword arguments and the interpreter's
/// `format_str_template`; `format_map` routes through
/// `format_str_template_map`; `maketrans` is a staticmethod whose receiver is
/// discarded and forwarded to `str_maketrans`.  The interpreter-free `call`
/// arm for `format`/`format_map` is a drift-guard stub that is never reached
/// at runtime.  The VM dispatcher queries this predicate to route them
/// upstream.  Single source of truth for the carve-out (see
/// `crates/pyrust-builtins/README.md`).
pub fn requires_vm_template(method: &str) -> bool {
    matches!(method, "format" | "format_map" | "maketrans")
}

pub fn call(method: &str, src: &Value, args: &[Value]) -> Result<Value> {
    let s: &str = src.as_str().unwrap();
    match method {
        // Common Sequence Operations (via char indexing).  ASCII-ness is cached
        // O(1) on the string header (#2124), so the find/index/count fast paths
        // no longer rescan the whole string on every call.
        "index" => str_index(s, src.str_is_ascii(), args),
        "count" => str_count(s, src.str_is_ascii(), args),
        // Splitting / joining
        "split" => split(src, s, args),
        "rsplit" => rsplit(src, s, args),
        "join" => join(s, args),
        "splitlines" => str_splitlines(s, args),
        "partition" => {
            expect_arg_count(args, 1, 1, "partition")?;
            // CPython: "must be str, not <T>" (no param name in the message)
            let sep = match args[0].kind() {
                ValueKind::Str(s) => s,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("must be str, not {}", builtin_type_name(&args[0])),
                    ));
                }
            };
            str_partition(s, sep)
        }
        "rpartition" => {
            expect_arg_count(args, 1, 1, "rpartition")?;
            let sep = match args[0].kind() {
                ValueKind::Str(s) => s,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("must be str, not {}", builtin_type_name(&args[0])),
                    ));
                }
            };
            str_rpartition(s, sep)
        }
        // Stripping
        "strip" => Ok(Value::string(strip_chars(s, args, true, true, "strip")?)),
        "lstrip" => Ok(Value::string(strip_chars(s, args, true, false, "lstrip")?)),
        "rstrip" => Ok(Value::string(strip_chars(s, args, false, true, "rstrip")?)),
        // Prefix/suffix removal
        "removeprefix" => {
            expect_arg_count(args, 1, 1, "removeprefix")?;
            // CPython: "removeprefix() argument must be str, not <type>"
            // None displays as "None" not "NoneType" in this message.
            let prefix = match args[0].kind() {
                ValueKind::Str(p) => p,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "removeprefix() argument must be str, not {}",
                            py_value_display_name(&args[0])
                        ),
                    ));
                }
            };
            Ok(str_removeprefix(s, prefix))
        }
        "removesuffix" => {
            expect_arg_count(args, 1, 1, "removesuffix")?;
            // CPython: "removesuffix() argument must be str, not <type>"
            // None displays as "None" not "NoneType" in this message.
            let suffix = match args[0].kind() {
                ValueKind::Str(p) => p,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "removesuffix() argument must be str, not {}",
                            py_value_display_name(&args[0])
                        ),
                    ));
                }
            };
            Ok(str_removesuffix(s, suffix))
        }
        // Justification / padding
        "center" => {
            expect_arg_count(args, 1, 2, "center")?;
            let width = extract_int(&args[0], "center", "width")?;
            let fill = extract_fill_char(args)?;
            str_center(s, src.str_is_ascii(), width, fill)
        }
        "ljust" => {
            expect_arg_count(args, 1, 2, "ljust")?;
            let width = extract_int(&args[0], "ljust", "width")?;
            let fill = extract_fill_char(args)?;
            str_ljust(s, src.str_is_ascii(), width, fill)
        }
        "rjust" => {
            expect_arg_count(args, 1, 2, "rjust")?;
            let width = extract_int(&args[0], "rjust", "width")?;
            let fill = extract_fill_char(args)?;
            str_rjust(s, src.str_is_ascii(), width, fill)
        }
        "zfill" => {
            expect_arg_count(args, 1, 1, "zfill")?;
            let width = extract_int(&args[0], "zfill", "width")?;
            str_zfill(s, src.str_is_ascii(), width)
        }
        "expandtabs" => {
            // expandtabs() takes at most 1 argument (<got> given)
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "expandtabs() takes at most 1 argument ({} given)",
                        args.len()
                    ),
                ));
            }
            let tabsize = extract_optional_int(args, 0)?.unwrap_or(8);
            Ok(str_expandtabs(s, tabsize))
        }
        // Case
        "upper" => Ok(Value::string(if src.str_is_ascii() {
            s.to_ascii_uppercase()
        } else {
            s.to_uppercase()
        })),
        "lower" => Ok(Value::string(if src.str_is_ascii() {
            s.to_ascii_lowercase()
        } else {
            s.to_lowercase()
        })),
        "casefold" => Ok(Value::string(unicode_casefold(s, src.str_is_ascii()))),
        "capitalize" => Ok(Value::string(capitalize(s))),
        "swapcase" => Ok(Value::string(swapcase(s, src.str_is_ascii()))),
        "title" => Ok(Value::string(titlecase(s, src.str_is_ascii()))),
        // Searching
        "find" => str_find(s, src.str_is_ascii(), args, false),
        "rfind" => str_rfind(s, src.str_is_ascii(), args, false),
        "rindex" => str_rfind(s, src.str_is_ascii(), args, true),
        // Replacement
        "replace" => str_replace(s, args),
        // Testing
        "startswith" => str_startswith(s, src.str_is_ascii(), args),
        "endswith" => str_endswith(s, src.str_is_ascii(), args),
        "isdigit" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    // is_python_digit includes superscript/subscript No codepoints which
                    // are all non-ASCII, so pure ASCII strings can shortcut with is_ascii_digit.
                    s.bytes().all(|b| b.is_ascii_digit())
                } else {
                    // Use cesu8_codepoints so surrogate bytes don't reach chars().
                    // char::from_u32 returns None for surrogates; None → false → all() fails.
                    cesu8_codepoints(s).all(|n| char::from_u32(n).is_some_and(is_python_digit))
                },
        )),
        "isalpha" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(|b| b.is_ascii_alphabetic())
                } else {
                    cesu8_codepoints(s).all(|n| char::from_u32(n).is_some_and(is_python_alpha))
                },
        )),
        "isalnum" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(|b| b.is_ascii_alphanumeric())
                } else {
                    cesu8_codepoints(s).all(|n| char::from_u32(n).is_some_and(is_python_alnum))
                },
        )),
        "isspace" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(is_python_space_ascii)
                } else {
                    cesu8_codepoints(s).all(|n| char::from_u32(n).is_some_and(is_python_space))
                },
        )),
        "isdecimal" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(|b| b.is_ascii_digit())
                } else {
                    cesu8_codepoints(s).all(|n| {
                        char::from_u32(n).is_some_and(|c| {
                            // general_category tracks a newer Unicode than CPython
                            // 3.12 (Unicode 15.0); codepoints assigned in 16.0+ were
                            // Cn in 15.0 and must not count as decimal.
                            !unicode_data::is_assigned_after_15_0(c)
                                && c.general_category() == GeneralCategory::DecimalNumber
                        })
                    })
                },
        )),
        "isnumeric" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(|b| b.is_ascii_digit())
                } else {
                    cesu8_codepoints(s)
                        .all(|n| char::from_u32(n).is_some_and(unicode_data::is_numeric))
                },
        )),
        "islower" => Ok(Value::bool_(str_islower(s, src.str_is_ascii()))),
        "isupper" => Ok(Value::bool_(str_isupper(s, src.str_is_ascii()))),
        "istitle" => Ok(Value::bool_(str_istitle(s, src.str_is_ascii()))),
        "isascii" => Ok(Value::bool_(src.str_is_ascii())),
        "isidentifier" => Ok(Value::bool_(str_isidentifier(s, src.str_is_ascii()))),
        "isprintable" => Ok(Value::bool_(if src.str_is_ascii() {
            // Printable ASCII: 0x20 (space) through 0x7e (~). DEL (0x7f) is not printable.
            s.bytes().all(|b| (0x20..0x7f).contains(&b))
        } else {
            // Use cesu8_codepoints to handle surrogate bytes without invoking
            // chars(), which panics in debug builds on surrogate byte sequences.
            cesu8_codepoints(s).all(cp_is_printable)
        })),
        "encode" => str_encode(s, args),
        "translate" => str_translate(s, args),
        // `format` is intercepted by the bound-method dispatch in `calls.rs`
        // and routed through `format_str_template` (which handles kwargs).
        // This arm exists solely to satisfy the drift-guard test that verifies
        // every entry in METHODS has a dispatch arm; it is never reached at
        // runtime.
        "format" => Err(PyError::named(
            "TypeError",
            format!(
                "descriptor 'format' of 'str' object needs an argument ({} given)",
                args.len()
            ),
        )),
        // `format_map` is intercepted by `call_str_method` in the interpreter
        // and routed through `format_str_template_map` (which needs `&mut Interpreter`).
        // This arm exists solely to satisfy the drift-guard test that verifies every
        // entry in METHODS has a dispatch arm; it will never be reached at runtime.
        "format_map" => Err(PyError::named(
            "TypeError",
            format!(
                "str.format_map() takes exactly one argument ({} given)",
                args.len()
            ),
        )),
        // Intercepted upstream in vm.rs / calls.rs; sentinel for drift guard.
        "__iter__" => Err(PyError::named(
            "TypeError",
            "'str' __iter__ must be dispatched by the interpreter",
        )),
        // __getnewargs__ supports the pickle protocol: it returns a 1-tuple
        // containing the str itself, i.e. 'hello'.__getnewargs__() == ('hello',).
        "__getnewargs__" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "str.__getnewargs__() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            Ok(Value::tuple(vec![src.clone()]))
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'str' object has no attribute '{method}'"),
        )),
    }
}

// ─── Method implementations ───────────────────────────────────────────────────

/// Try to reserve `additional` bytes in `out`, mapping allocation failure to
/// `MemoryError` rather than panicking, mirroring CPython's behaviour.
#[inline]
fn try_reserve_str(out: &mut String, additional: usize) -> Result<()> {
    out.try_reserve(additional)
        .map_err(|_| PyError::named("MemoryError", ""))
}

fn str_center(s: &str, is_ascii: bool, width: i64, fill: char) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = if is_ascii { s.len() } else { s.chars().count() };
    if char_len >= width {
        return Ok(Value::string(s));
    }
    let marg = width - char_len;
    // CPython formula: left = marg//2 + (marg & width & 1)
    let left_pad = marg / 2 + (marg & width & 1);
    let right_pad = marg - left_pad;
    let fill_bytes = marg.saturating_mul(fill.len_utf8());
    let total = s.len().saturating_add(fill_bytes);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    for _ in 0..left_pad {
        out.push(fill);
    }
    out.push_str(s);
    for _ in 0..right_pad {
        out.push(fill);
    }
    Ok(Value::string(out))
}

fn str_ljust(s: &str, is_ascii: bool, width: i64, fill: char) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = if is_ascii { s.len() } else { s.chars().count() };
    if char_len >= width {
        return Ok(Value::string(s));
    }
    let pad = width - char_len;
    let fill_bytes = pad.saturating_mul(fill.len_utf8());
    let total = s.len().saturating_add(fill_bytes);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    out.push_str(s);
    for _ in 0..pad {
        out.push(fill);
    }
    Ok(Value::string(out))
}

fn str_rjust(s: &str, is_ascii: bool, width: i64, fill: char) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = if is_ascii { s.len() } else { s.chars().count() };
    if char_len >= width {
        return Ok(Value::string(s));
    }
    let pad = width - char_len;
    let fill_bytes = pad.saturating_mul(fill.len_utf8());
    let total = s.len().saturating_add(fill_bytes);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    for _ in 0..pad {
        out.push(fill);
    }
    out.push_str(s);
    Ok(Value::string(out))
}

fn str_zfill(s: &str, is_ascii: bool, width: i64) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = if is_ascii { s.len() } else { s.chars().count() };
    if char_len >= width {
        return Ok(Value::string(s));
    }
    let pad = width - char_len;
    let total = s.len().saturating_add(pad);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    // Preserve leading sign character
    let mut chars = s.chars();
    match chars.next() {
        Some(c @ ('+' | '-')) => {
            out.push(c);
            for _ in 0..pad {
                out.push('0');
            }
            out.push_str(chars.as_str());
        }
        Some(first) => {
            for _ in 0..pad {
                out.push('0');
            }
            out.push(first);
            out.push_str(chars.as_str());
        }
        None => {
            for _ in 0..pad {
                out.push('0');
            }
        }
    }
    Ok(Value::string(out))
}

fn str_expandtabs(s: &str, tabsize: i64) -> Value {
    let tabsize = tabsize.max(0) as usize;
    let mut out = String::with_capacity(s.len());
    let mut col: usize = 0;
    for c in s.chars() {
        match c {
            '\t' => {
                if tabsize == 0 {
                    // tab with size 0 is removed
                } else {
                    let spaces = tabsize - (col % tabsize);
                    for _ in 0..spaces {
                        out.push(' ');
                    }
                    col += spaces;
                }
            }
            '\n' | '\r' => {
                out.push(c);
                col = 0;
            }
            _ => {
                out.push(c);
                col += 1;
            }
        }
    }
    Value::string(out)
}

fn str_partition(s: &str, sep: &str) -> Result<Value> {
    if sep.is_empty() {
        return Err(PyError::named("ValueError", "empty separator".to_string()));
    }
    let (before, found_sep, after) = match s.find(sep) {
        Some(pos) => (&s[..pos], sep, &s[pos + sep.len()..]),
        None => (s, "", ""),
    };
    Ok(Value::tuple(vec![
        Value::string(before),
        Value::string(found_sep),
        Value::string(after),
    ]))
}

fn str_rpartition(s: &str, sep: &str) -> Result<Value> {
    if sep.is_empty() {
        return Err(PyError::named("ValueError", "empty separator".to_string()));
    }
    let (before, found_sep, after) = match s.rfind(sep) {
        Some(pos) => (&s[..pos], sep, &s[pos + sep.len()..]),
        None => ("", "", s),
    };
    Ok(Value::tuple(vec![
        Value::string(before),
        Value::string(found_sep),
        Value::string(after),
    ]))
}

fn str_splitlines(s: &str, args: &[Value]) -> Result<Value> {
    // splitlines() takes at most 1 argument (<got> given)
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "splitlines() takes at most 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    // CPython coerces keepends via the standard truth protocol — any value is
    // accepted.  Delegate to Value::truthy_raw() which covers all ValueKind arms
    // (including Dict, Set, BigInt, Range, Complex, BuiltinObject, etc.).
    let keepends = args.first().is_some_and(|v| v.truthy_raw());
    let mut lines: Vec<Value> = Vec::new();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut start = 0;
    let mut i = 0;
    while i < len {
        let b = bytes[i];
        // Detect line endings: \r\n, \r, \n, \x0b, \x0c, \x1c, \x1d, \x1e, \x85,  ,
        let eol_len: usize;
        let is_eol = match b {
            b'\n' | b'\x0b' | b'\x0c' | b'\x1c' | b'\x1d' | b'\x1e' => {
                eol_len = 1;
                true
            }
            b'\r' => {
                if i + 1 < len && bytes[i + 1] == b'\n' {
                    eol_len = 2;
                } else {
                    eol_len = 1;
                }
                true
            }
            0xC2 if i + 1 < len && bytes[i + 1] == 0x85 => {
                // U+0085 NEXT LINE encoded as UTF-8: 0xC2 0x85
                eol_len = 2;
                true
            }
            0xE2 if i + 2 < len
                && bytes[i + 1] == 0x80
                && (bytes[i + 2] == 0xA8 || bytes[i + 2] == 0xA9) =>
            {
                // U+2028 LINE SEPARATOR / U+2029 PARAGRAPH SEPARATOR: 0xE2 0x80 0xA8/0xA9
                eol_len = 3;
                true
            }
            _ => {
                eol_len = 0;
                false
            }
        };
        if is_eol {
            let end = if keepends { i + eol_len } else { i };
            lines.push(Value::string(&s[start..end]));
            i += eol_len;
            start = i;
        } else {
            i += 1;
        }
    }
    // Trailing non-empty segment (no trailing newline)
    if start < len {
        lines.push(Value::string(&s[start..]));
    }
    Ok(Value::list(lines))
}

fn str_removeprefix(s: &str, prefix: &str) -> Value {
    if let Some(stripped) = s.strip_prefix(prefix) {
        Value::string(stripped)
    } else {
        Value::string(s)
    }
}

fn str_removesuffix(s: &str, suffix: &str) -> Value {
    if let Some(stripped) = s.strip_suffix(suffix) {
        Value::string(stripped)
    } else {
        Value::string(s)
    }
}

/// Unicode full case-folding (CaseFolding.txt status F and S).
/// Handles multi-char expansions (ß→ss, ligatures) that Rust's `to_lowercase` misses.
fn unicode_casefold(s: &str, is_ascii: bool) -> String {
    if is_ascii {
        return s.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        // CaseFolding.txt full folding: for most characters the fold equals the
        // lowercase mapping, so use to_lowercase and override only the documented
        // exceptions (µ→μ, ς→σ, ß→ss, ﬆ→st, Cherokee, …) where fold ≠ lowercase.
        match unicode_data::casefold_exception(c) {
            Some(folded) => out.push_str(folded),
            None => out.extend(c.to_lowercase()),
        }
    }
    out
}

fn swapcase(s: &str, is_ascii: bool) -> String {
    if is_ascii {
        return s
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else if c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_uppercase() {
            out.extend(c.to_lowercase());
        } else if c.is_lowercase() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn titlecase(s: &str, is_ascii: bool) -> String {
    if is_ascii {
        let mut out = String::with_capacity(s.len());
        let mut prev_cased = false;
        for c in s.chars() {
            if c.is_ascii_alphabetic() {
                if prev_cased {
                    out.push(c.to_ascii_lowercase());
                } else {
                    out.push(c.to_ascii_uppercase());
                }
                prev_cased = true;
            } else {
                out.push(c);
                prev_cased = false;
            }
        }
        return out;
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                push_titlecase(&mut out, c);
            }
            prev_cased = true;
        } else {
            out.push(c);
            prev_cased = false;
        }
    }
    out
}

/// Push the Unicode titlecase form of `c` onto `out`. Unlike `char::to_uppercase`,
/// this maps Lt digraphs to their titlecase form (ǆ→ǅ, ǳ→ǲ, …) and applies the
/// SpecialCasing titlecase entries (ß→Ss, ﬀ→Ff, …); for all other characters the
/// titlecase mapping equals the uppercase mapping.
fn push_titlecase(out: &mut String, c: char) {
    match unicode_data::to_titlecase(c) {
        Some(t) => out.push_str(t),
        None => out.extend(c.to_uppercase()),
    }
}

fn str_islower(s: &str, is_ascii: bool) -> bool {
    if is_ascii {
        let mut has_cased = false;
        for b in s.bytes() {
            if b.is_ascii_uppercase() {
                return false;
            }
            if b.is_ascii_lowercase() {
                has_cased = true;
            }
        }
        return has_cased;
    }
    // cesu8_codepoints: surrogate codepoints have no case; treat as uncased separator.
    let mut has_cased = false;
    for n in cesu8_codepoints(s) {
        let Some(c) = char::from_u32(n) else { continue };
        // char::is_*case tracks a newer Unicode than CPython 3.12 (Unicode 15.0);
        // codepoints assigned in 16.0+ were Cn in 15.0 and have no case.
        if unicode_data::is_assigned_after_15_0(c) {
            continue;
        }
        if c.is_uppercase() {
            return false;
        }
        if c.is_lowercase() {
            has_cased = true;
        }
    }
    has_cased
}

fn str_isupper(s: &str, is_ascii: bool) -> bool {
    if is_ascii {
        let mut has_cased = false;
        for b in s.bytes() {
            if b.is_ascii_lowercase() {
                return false;
            }
            if b.is_ascii_uppercase() {
                has_cased = true;
            }
        }
        return has_cased;
    }
    // cesu8_codepoints: surrogate codepoints have no case; treat as uncased separator.
    let mut has_cased = false;
    for n in cesu8_codepoints(s) {
        let Some(c) = char::from_u32(n) else { continue };
        // char::is_*case tracks a newer Unicode than CPython 3.12 (Unicode 15.0);
        // codepoints assigned in 16.0+ were Cn in 15.0 and have no case.
        if unicode_data::is_assigned_after_15_0(c) {
            continue;
        }
        if c.is_lowercase() {
            return false;
        }
        if c.is_uppercase() {
            has_cased = true;
        }
    }
    has_cased
}

fn str_istitle(s: &str, is_ascii: bool) -> bool {
    if is_ascii {
        let mut prev_cased = false;
        let mut has_cased = false;
        for b in s.bytes() {
            if b.is_ascii_uppercase() {
                if prev_cased {
                    return false;
                }
                prev_cased = true;
                has_cased = true;
            } else if b.is_ascii_lowercase() {
                if !prev_cased {
                    return false;
                }
                prev_cased = true;
                has_cased = true;
            } else {
                prev_cased = false;
            }
        }
        return has_cased;
    }
    // cesu8_codepoints: surrogate codepoints have no case; treat as uncased separator.
    let mut prev_cased = false;
    let mut has_cased = false;
    for n in cesu8_codepoints(s) {
        let c = match char::from_u32(n) {
            Some(c) => c,
            None => {
                prev_cased = false;
                continue;
            }
        };
        // char::is_*case / general_category track a newer Unicode than CPython
        // 3.12 (Unicode 15.0); codepoints assigned in 16.0+ were Cn in 15.0, so
        // treat them as uncased separators.
        if unicode_data::is_assigned_after_15_0(c) {
            prev_cased = false;
            continue;
        }
        // CPython's unicode_istitle treats titlecase (Lt) characters like
        // uppercase: they must start a word (follow a non-cased character).
        // Rust's char::is_uppercase covers only Lu, so test Lt explicitly.
        if c.is_uppercase() || c.general_category() == GeneralCategory::TitlecaseLetter {
            if prev_cased {
                return false; // uppercase/titlecase after cased (must follow non-cased)
            }
            prev_cased = true;
            has_cased = true;
        } else if c.is_lowercase() {
            if !prev_cased {
                return false; // lowercase after non-cased
            }
            prev_cased = true;
            has_cased = true;
        } else {
            prev_cased = false;
        }
    }
    has_cased
}

fn str_isidentifier(s: &str, is_ascii: bool) -> bool {
    if s.is_empty() {
        return false;
    }
    if is_ascii {
        let mut bytes = s.bytes();
        let first = bytes.next().unwrap();
        if !first.is_ascii_alphabetic() && first != b'_' {
            return false;
        }
        return bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
    }
    // Use cesu8_codepoints to avoid chars() panicking on surrogate bytes.
    // A surrogate codepoint is not a valid identifier character → return false.
    //
    // Python identifiers use the Unicode XID_Start / XID_Continue properties
    // (plus `_`), not is_alphabetic / is_alphanumeric. Combining marks (Mn/Mc)
    // are XID_Continue but not alphanumeric; superscripts (²) are alphanumeric
    // but not XID_Continue.
    let mut codepoints = cesu8_codepoints(s);
    let first = match codepoints.next().and_then(char::from_u32) {
        Some(c) => c,
        None => return false, // empty or surrogate first codepoint
    };
    if !unicode_data::is_xid_start(first) {
        return false;
    }
    codepoints.all(|n| char::from_u32(n).is_some_and(unicode_data::is_xid_continue))
}

/// ASCII whitespace per Python's `str.isspace()` / `Py_UNICODE_ISSPACE`. In
/// addition to the usual ` \t\n\r\x0b\x0c`, CPython treats the C0 information
/// separators `\x1c`–`\x1f` (bidirectional class B/S) as whitespace.
#[inline]
fn is_python_space_ascii(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c | 0x1c..=0x1f)
}

/// Python's `str.isspace()`: the fixed whitespace set used by CPython's
/// `Py_UNICODE_ISSPACE` (Unicode 15.0). This differs from Rust's
/// `char::is_whitespace`, which omits `\x1c`–`\x1f` and `\x85`.
fn is_python_space(c: char) -> bool {
    matches!(
        c as u32,
        0x09..=0x0D
            | 0x1C..=0x1F
            | 0x20
            | 0x85
            | 0xA0
            | 0x1680
            | 0x2000..=0x200A
            | 0x2028
            | 0x2029
            | 0x202F
            | 0x205F
            | 0x3000
    )
}

/// Python's `str.isalnum()`: a character is alphanumeric when it is alphabetic
/// (`isalpha`), or has Numeric_Type Decimal (`isdecimal`), Digit (`isdigit`), or
/// Numeric (`isnumeric`). Symbol categories such as circled letters (So) are
/// none of these and are correctly excluded.
fn is_python_alnum(c: char) -> bool {
    is_python_alpha(c) || is_python_digit(c) || unicode_data::is_numeric(c)
}

// ─────────────────────────────────────────────────────────────────────────────

/// Python's str.isdigit(): Unicode Nd (DecimalNumber) category plus all codepoints with
/// Numeric_Type=Digit (category No). Authoritative list from CPython 3.12 / Unicode 15.
fn is_python_digit(c: char) -> bool {
    // Nd covers all decimal digit scripts (Arabic-Indic, Devanagari, etc.).
    // `general_category` tracks a newer Unicode than CPython 3.12 (Unicode 15.0),
    // so skip codepoints assigned in 16.0+ (Cn in 15.0) to keep parity.
    if !unicode_data::is_assigned_after_15_0(c)
        && c.general_category() == GeneralCategory::DecimalNumber
    {
        return true;
    }
    // Remaining codepoints with Numeric_Type=Digit (category No) per Unicode 15 / CPython 3.12.
    matches!(
        c as u32,
        0x00B2 | 0x00B3 | 0x00B9           // superscript 2, 3, 1
        | 0x1369..=0x1371                   // Ethiopic digits 1–9
        | 0x19DA                            // New Tai Lue Tham Digit One
        | 0x2070 | 0x2074..=0x2079         // superscript 0, 4–9
        | 0x2080..=0x2089                   // subscript 0–9
        | 0x2460..=0x2468                   // circled digits 1–9
        | 0x2474..=0x247C                   // parenthesized digits 1–9
        | 0x2488..=0x2490                   // digit full-stop 1–9
        | 0x24EA                            // circled digit 0
        | 0x24F5..=0x24FD                   // double circled digits 1–9
        | 0x24FF                            // negative circled digit 0
        | 0x2776..=0x277E                   // dingbat negative circled digits 1–9
        | 0x2780..=0x2788                   // dingbat circled sans-serif digits 1–9
        | 0x278A..=0x2792                   // dingbat negative circled sans-serif digits 1–9
        | 0x10A40..=0x10A43                 // Kharoshthi digits 1–4
        | 0x10E60..=0x10E68                 // Rumi digits 1–9
        | 0x11052..=0x1105A                 // Brahmi numbers 1–9
        | 0x1F100..=0x1F10A                 // digit full-stop/comma 0–9
    )
}

/// Python's str.isalpha(): Unicode general category L* (Letter).
///
/// `general_category` tracks a newer Unicode database than CPython 3.12
/// (Unicode 15.0); codepoints assigned in Unicode 16.0+ were `Cn` in 15.0, so
/// they must classify as non-alphabetic to stay byte-identical to python3.12.
fn is_python_alpha(c: char) -> bool {
    !unicode_data::is_assigned_after_15_0(c)
        && matches!(
            c.general_category(),
            GeneralCategory::UppercaseLetter
                | GeneralCategory::LowercaseLetter
                | GeneralCategory::TitlecaseLetter
                | GeneralCategory::ModifierLetter
                | GeneralCategory::OtherLetter
        )
}

/// Validate that the first argument to a `str` search method is itself a `str`,
/// returning the borrowed substring. `method` is the bare method name (e.g.
/// `"index"`) threaded into the missing-argument error message.
fn require_str_arg<'a>(args: &'a [Value], method: &str) -> Result<&'a str> {
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => Ok(sub),
        Some(_) => Err(PyError::named(
            "TypeError",
            format!(
                "must be str, not {}",
                builtin_type_name(args.first().unwrap())
            ),
        )),
        None => Err(PyError::named(
            "TypeError",
            format!("str.{method}() requires a str argument"),
        )),
    }
}

fn str_index(s: &str, is_ascii: bool, args: &[Value]) -> Result<Value> {
    let sub = require_str_arg(args, "index")?;
    let Some((start, end)) = str_slice_args(s, is_ascii, args)? else {
        return Err(PyError::named(
            "ValueError",
            "substring not found".to_string(),
        ));
    };
    let haystack = &s[start..end];
    match haystack.find(sub) {
        Some(byte_pos) => Ok(Value::int(
            byte_to_char_idx(s, is_ascii, start + byte_pos) as i64
        )),
        None => Err(PyError::named(
            "ValueError",
            "substring not found".to_string(),
        )),
    }
}

fn str_count(s: &str, is_ascii: bool, args: &[Value]) -> Result<Value> {
    let sub = require_str_arg(args, "count")?;
    let Some((start, end)) = str_slice_args(s, is_ascii, args)? else {
        // Inverted window: CPython returns 0 for all substrings including empty.
        return Ok(Value::int(0));
    };
    if sub.is_empty() {
        let haystack = &s[start..end];
        // ASCII: char count == byte count (#2032).  A substring of an all-ASCII
        // string is all-ASCII, so the cached whole-string flag applies directly.
        let count = if is_ascii || haystack.is_ascii() {
            haystack.len()
        } else {
            haystack.chars().count()
        };
        return Ok(Value::int((count + 1) as i64));
    }
    let haystack = &s[start..end];
    let n = haystack.match_indices(sub).count();
    Ok(Value::int(n as i64))
}

fn str_find(s: &str, is_ascii: bool, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = require_str_arg(args, "find")?;
    let Some((start, end)) = str_slice_args(s, is_ascii, args)? else {
        if raise_on_miss {
            return Err(PyError::named(
                "ValueError",
                "substring not found".to_string(),
            ));
        }
        return Ok(Value::int(-1));
    };
    let haystack = &s[start..end];
    match haystack.find(sub) {
        Some(byte_pos) => Ok(Value::int(
            byte_to_char_idx(s, is_ascii, start + byte_pos) as i64
        )),
        None => {
            if raise_on_miss {
                Err(PyError::named(
                    "ValueError",
                    "substring not found".to_string(),
                ))
            } else {
                Ok(Value::int(-1))
            }
        }
    }
}

fn str_rfind(s: &str, is_ascii: bool, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = require_str_arg(args, "rfind")?;
    let Some((start, end)) = str_slice_args(s, is_ascii, args)? else {
        if raise_on_miss {
            return Err(PyError::named(
                "ValueError",
                "substring not found".to_string(),
            ));
        }
        return Ok(Value::int(-1));
    };
    let haystack = &s[start..end];
    match haystack.rfind(sub) {
        Some(byte_pos) => Ok(Value::int(
            byte_to_char_idx(s, is_ascii, start + byte_pos) as i64
        )),
        None => {
            if raise_on_miss {
                Err(PyError::named(
                    "ValueError",
                    "substring not found".to_string(),
                ))
            } else {
                Ok(Value::int(-1))
            }
        }
    }
}

fn split(src: &Value, s: &str, args: &[Value]) -> Result<Value> {
    let (sep, maxsplit) = split_args(args)?;
    let parts: Vec<Value> = match sep {
        None => {
            if maxsplit < 0 {
                // Heuristic capacity (avg word ~4 chars) avoids Vec realloc in one pass
                let mut parts = Vec::with_capacity(s.len() / 4 + 1);
                for p in s.split_whitespace() {
                    let off = subslice_offset(s, p);
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                let n = maxsplit as usize;
                // Python's whitespace split: consecutive whitespace treated as one
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    return Ok(Value::list(vec![]));
                }
                let mut out = Vec::new();
                let mut remaining = s;
                for _ in 0..n {
                    let t = remaining.trim_start();
                    if t.is_empty() {
                        break;
                    }
                    match t.find(char::is_whitespace) {
                        None => {
                            let off = subslice_offset(s, t);
                            out.push(src.string_slice(off, off + t.len()));
                            remaining = "";
                            break;
                        }
                        Some(pos) => {
                            let off = subslice_offset(s, t);
                            out.push(src.string_slice(off, off + pos));
                            remaining = &t[pos..];
                        }
                    }
                }
                let tail = remaining.trim_start();
                if !tail.is_empty() {
                    let off = subslice_offset(s, tail);
                    out.push(src.string_slice(off, off + tail.len()));
                }
                return Ok(Value::list(out));
            }
        }
        Some(sep_str) => {
            if sep_str.is_empty() {
                return Err(PyError::named("ValueError", "empty separator".to_string()));
            }
            if maxsplit < 0 {
                let cap = s.len() / sep_str.len() + 1;
                let mut parts = Vec::with_capacity(cap);
                for p in s.split(sep_str) {
                    let off = subslice_offset(s, p);
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                s.splitn(maxsplit as usize + 1, sep_str)
                    .map(|p| {
                        let off = subslice_offset(s, p);
                        src.string_slice(off, off + p.len())
                    })
                    .collect()
            }
        }
    };
    Ok(Value::list(parts))
}

fn rsplit(src: &Value, s: &str, args: &[Value]) -> Result<Value> {
    let (sep, maxsplit) = split_args(args)?;
    let parts: Vec<Value> = match sep {
        None => {
            // No sep: rsplit with no maxsplit is identical to split (left-to-right).
            if maxsplit < 0 {
                let mut parts = Vec::with_capacity(s.len() / 4 + 1);
                for p in s.split_whitespace() {
                    let off = subslice_offset(s, p);
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                let n = maxsplit as usize;
                let mut out = Vec::new();
                let mut remaining = s;
                for _ in 0..n {
                    let t = remaining.trim_end();
                    if t.is_empty() {
                        break;
                    }
                    match t.rfind(char::is_whitespace) {
                        None => {
                            let off = subslice_offset(s, t);
                            out.push(src.string_slice(off, off + t.len()));
                            remaining = "";
                            break;
                        }
                        Some(pos) => {
                            let tail = &t[pos + 1..];
                            let off = subslice_offset(s, tail);
                            out.push(src.string_slice(off, off + tail.len()));
                            remaining = &t[..pos];
                        }
                    }
                }
                let head = remaining.trim_end();
                if !head.is_empty() {
                    let off = subslice_offset(s, head);
                    out.push(src.string_slice(off, off + head.len()));
                }
                out.reverse();
                return Ok(Value::list(out));
            }
        }
        Some(sep_str) => {
            if sep_str.is_empty() {
                return Err(PyError::named("ValueError", "empty separator".to_string()));
            }
            if maxsplit < 0 {
                let cap = s.len() / sep_str.len() + 1;
                let mut parts = Vec::with_capacity(cap);
                for p in s.split(sep_str) {
                    let off = subslice_offset(s, p);
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                let mut parts: Vec<Value> = s
                    .rsplitn(maxsplit as usize + 1, sep_str)
                    .map(|p| {
                        let off = subslice_offset(s, p);
                        src.string_slice(off, off + p.len())
                    })
                    .collect();
                parts.reverse();
                parts
            }
        }
    };
    Ok(Value::list(parts))
}

/// Return the CPython type name for a `PyKey` variant — used in join's
/// "sequence item N: expected str instance, X found" error messages.
/// `PyKey::Object` stores the original `Value`, so we derive the name
/// via `builtin_type_name` rather than hardcoding "object" (#576 Copilot
/// review: use the runtime class name, e.g. "MyKey").
fn pykey_type_name(k: &PyKey) -> std::borrow::Cow<'static, str> {
    match k {
        PyKey::Int(_) | PyKey::BigInt(_) => std::borrow::Cow::Borrowed("int"),
        PyKey::Float(_) => std::borrow::Cow::Borrowed("float"),
        PyKey::Bool(_) => std::borrow::Cow::Borrowed("bool"),
        PyKey::Str(_) => std::borrow::Cow::Borrowed("str"),
        PyKey::None => std::borrow::Cow::Borrowed("NoneType"),
        PyKey::Ellipsis => std::borrow::Cow::Borrowed("ellipsis"),
        PyKey::FrozenSet(_) => std::borrow::Cow::Borrowed("frozenset"),
        PyKey::Tuple(_) => std::borrow::Cow::Borrowed("tuple"),
        PyKey::Bytes(_) => std::borrow::Cow::Borrowed("bytes"),
        PyKey::Complex(_, _) => std::borrow::Cow::Borrowed("complex"),
        PyKey::Object { value, .. } => builtin_type_name(value),
    }
}

/// Build the joined string from an iterator that yields each element as a
/// validated `&str`. The single pass validates every element (so a non-str
/// element raises before any work) and stashes the borrowed slices, summing
/// their byte lengths. The result is then filled directly into the string's
/// backing buffer via `string_from_fill` — no intermediate `String`, so the
/// joined bytes are touched exactly once (the copy) instead of three times
/// (push into a `String`, an `is_ascii` rescan, then the final memcpy).
fn join_borrowed<'a, I>(sep: &str, parts: I) -> Result<Value>
where
    I: ExactSizeIterator<Item = Result<&'a str>>,
{
    let n = parts.len();
    if n == 0 {
        return Ok(Value::string(String::new()));
    }
    // Validate every element up front, stashing the borrowed slice and summing
    // byte lengths.  Borrowing keeps the build pass infallible; a SmallVec keeps
    // the common small join off the heap (no allocation for up to 16 parts).
    let mut slices: smallvec::SmallVec<[&str; 16]> = smallvec::SmallVec::with_capacity(n);
    let mut body_len = 0usize;
    for part in parts {
        let s = part?;
        body_len += s.len();
        slices.push(s);
    }
    let total = body_len + sep.len() * (n - 1);
    // For short results the ASCII scan is cache-hot and benefits later index /
    // find / slice ops, so compute it eagerly during the copy.  For large
    // results that scan would roughly double the bytes touched, so leave the
    // flag uncomputed (`None`) and let `str_is_ascii` resolve it lazily if ever
    // queried.  256 bytes keeps the common small join eager.
    let eager_ascii = total <= 256;
    let sep_ascii = sep.is_ascii();
    // SAFETY: every slice is a validated `&str` and `sep` is a `&str`, so the
    // bytes written are valid UTF-8, and we write exactly `total` bytes.
    Ok(unsafe {
        Value::string_from_fill(total, |buf| {
            let mut off = 0usize;
            let mut all_ascii = sep_ascii;
            for (i, s) in slices.iter().enumerate() {
                if i != 0 {
                    buf[off..off + sep.len()].copy_from_slice(sep.as_bytes());
                    off += sep.len();
                }
                let b = s.as_bytes();
                buf[off..off + b.len()].copy_from_slice(b);
                off += b.len();
                if eager_ascii {
                    all_ascii &= s.is_ascii();
                }
            }
            eager_ascii.then_some(all_ascii)
        })
    })
}

fn join(sep: &str, args: &[Value]) -> Result<Value> {
    let iterable = args
        .first()
        .ok_or_else(|| PyError::Runtime("str.join() requires 1 argument".to_string()))?;
    // Borrow each element as &str (no owned String per element); the result
    // string is allocated exactly once. The borrow guard from `kind()` must
    // stay alive across the build, so do it inside each arm.
    match iterable.kind() {
        ValueKind::List(items) => join_borrowed(
            sep,
            items.iter().enumerate().map(|(i, v)| match v.kind() {
                ValueKind::Str(s) => Ok(s),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        builtin_type_name(v),
                    ),
                )),
            }),
        ),
        ValueKind::Tuple(items) => join_borrowed(
            sep,
            items.iter().enumerate().map(|(i, v)| match v.kind() {
                ValueKind::Str(s) => Ok(s),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        builtin_type_name(v),
                    ),
                )),
            }),
        ),
        ValueKind::Dict(d) => join_borrowed(
            sep,
            d.keys().enumerate().map(|(i, k)| match k {
                PyKey::Str(s) => Ok(s.as_str().unwrap_or("")),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        pykey_type_name(k),
                    ),
                )),
            }),
        ),
        ValueKind::Str(s) => {
            // Iterating a str yields single chars; join them with `sep`
            // between each, allocating the result once.
            let n = s.chars().count();
            if n == 0 {
                return Ok(Value::string(String::new()));
            }
            let total = s.len() + sep.len() * (n - 1);
            let mut out = String::with_capacity(total);
            for (i, c) in s.chars().enumerate() {
                if i != 0 {
                    out.push_str(sep);
                }
                out.push(c);
            }
            Ok(Value::string(out))
        }
        _ => Err(PyError::named(
            "TypeError",
            "can only join an iterable".to_string(),
        )),
    }
}

fn str_replace(s: &str, args: &[Value]) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            format!("replace expected at least 2 arguments, got {}", args.len()),
        ));
    }
    let old: &str = match args[0].kind() {
        ValueKind::Str(s) => s,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "replace() argument 1 must be str, not {}",
                    py_value_display_name(&args[0])
                ),
            ));
        }
    };
    let new: &str = match args[1].kind() {
        ValueKind::Str(s) => s,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "replace() argument 2 must be str, not {}",
                    py_value_display_name(&args[1])
                ),
            ));
        }
    };
    let count = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        None => -1,
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            ));
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    builtin_type_name(&args[2])
                ),
            ));
        }
    };
    let max = if count < 0 {
        usize::MAX
    } else {
        count as usize
    };
    Ok(Value::string(replace_fill(s, old, new, max)))
}

/// Single-pass `str.replace`/`replacen` that seeds the result buffer with
/// `s.len()` capacity.  Rust's `str::replace` starts from an empty `String` and
/// reallocates as it grows (several allocation events per call); most replaces
/// keep the length close to the source, so one up-front reservation avoids those
/// intermediate reallocations without the extra counting pass a *precise* size
/// would need.  Semantics are identical to `s.replacen(from, to, max)`
/// (`max == usize::MAX` for replace-all), including empty-`from` behaviour.
fn replace_fill(s: &str, from: &str, to: &str, max: usize) -> String {
    if max == 0 {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let mut last_end = 0;
    for (start, part) in s.match_indices(from).take(max) {
        result.push_str(&s[last_end..start]);
        result.push_str(to);
        last_end = start + part.len();
    }
    result.push_str(&s[last_end..]);
    result
}

fn str_startswith(s: &str, is_ascii: bool, args: &[Value]) -> Result<Value> {
    // str_slice_args returns None for an inverted window (start > end).
    // For a Str prefix, that is an immediate False.
    // For a Tuple, CPython still validates element types even on an inverted
    // window — TypeError takes priority over the inverted-range short-circuit.
    let window = str_slice_args(s, is_ascii, args)?;
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(p)) => {
            let Some((start, end)) = window else {
                return Ok(Value::bool_(false));
            };
            Ok(Value::bool_(s[start..end].starts_with(p)))
        }
        Some(ValueKind::Tuple(prefixes)) => {
            for pv in prefixes.iter() {
                match pv.kind() {
                    ValueKind::Str(p) => {
                        if let Some((start, end)) = window
                            && s[start..end].starts_with(p)
                        {
                            return Ok(Value::bool_(true));
                        }
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "tuple for startswith must only contain str, not {}",
                                builtin_type_name(pv)
                            ),
                        ));
                    }
                }
            }
            Ok(Value::bool_(false))
        }
        Some(_) => Err(PyError::named(
            "TypeError",
            format!(
                "startswith first arg must be str or a tuple of str, not {}",
                builtin_type_name(args.first().unwrap())
            ),
        )),
        None => Err(PyError::named(
            "TypeError",
            "startswith() takes at least 1 argument (0 given)".to_string(),
        )),
    }
}

fn str_endswith(s: &str, is_ascii: bool, args: &[Value]) -> Result<Value> {
    // str_slice_args returns None for an inverted window (start > end).
    // For a Str suffix, that is an immediate False.
    // For a Tuple, CPython still validates element types even on an inverted
    // window — TypeError takes priority over the inverted-range short-circuit.
    let window = str_slice_args(s, is_ascii, args)?;
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(p)) => {
            let Some((start, end)) = window else {
                return Ok(Value::bool_(false));
            };
            Ok(Value::bool_(s[start..end].ends_with(p)))
        }
        Some(ValueKind::Tuple(suffixes)) => {
            for sv in suffixes.iter() {
                match sv.kind() {
                    ValueKind::Str(p) => {
                        if let Some((start, end)) = window
                            && s[start..end].ends_with(p)
                        {
                            return Ok(Value::bool_(true));
                        }
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "tuple for endswith must only contain str, not {}",
                                builtin_type_name(sv)
                            ),
                        ));
                    }
                }
            }
            Ok(Value::bool_(false))
        }
        Some(_) => Err(PyError::named(
            "TypeError",
            format!(
                "endswith first arg must be str or a tuple of str, not {}",
                builtin_type_name(args.first().unwrap())
            ),
        )),
        None => Err(PyError::named(
            "TypeError",
            "endswith() takes at least 1 argument (0 given)".to_string(),
        )),
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = String::with_capacity(s.len());
            // CPython titlecases the first character (so Lt digraphs become their
            // titlecase form, e.g. "ǆabc".capitalize() == "ǅabc"), then lowercases
            // the remainder.
            push_titlecase(&mut out, first);
            out.extend(chars.as_str().chars().flat_map(char::to_lowercase));
            out
        }
    }
}

fn strip_chars(s: &str, args: &[Value], left: bool, right: bool, method: &str) -> Result<String> {
    let chars_arg: Option<&str> = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(c)) => Some(c),
        Some(ValueKind::None) | None => None,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!("{method} arg must be None or str"),
            ));
        }
    };
    Ok(match chars_arg {
        None => {
            let mut result = s;
            if left {
                result = result.trim_start();
            }
            if right {
                result = result.trim_end();
            }
            result.to_string()
        }
        Some(chars) => {
            let mut result = s;
            if left {
                result = result.trim_start_matches(|c: char| chars.contains(c));
            }
            if right {
                result = result.trim_end_matches(|c: char| chars.contains(c));
            }
            result.to_string()
        }
    })
}

/// Parse (sep, maxsplit) from split/rsplit args.
fn split_args(args: &[Value]) -> Result<(Option<&str>, i64)> {
    let sep = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(s)) => Some(s),
        Some(ValueKind::None) | None => None,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "must be str or None, not {}",
                    builtin_type_name(args.first().unwrap())
                ),
            ));
        }
    };
    let maxsplit = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        None => -1,
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            ));
        }
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    builtin_type_name(&args[1])
                ),
            ));
        }
    };
    Ok((sep, maxsplit))
}

/// Convert a byte offset into a char (code-point) index within `s`.
///
/// ASCII fast path (#2032): when the prefix `s[..byte_off]` is all-ASCII the
/// char index equals the byte offset, so no scan is needed.  `is_ascii()` is
/// SIMD-accelerated and far cheaper than decoding via `chars().count()`.
#[inline]
fn byte_to_char_idx(s: &str, is_ascii: bool, byte_off: usize) -> usize {
    // When the whole string is ASCII (cached, #2124) the prefix is too, so the
    // char index is the byte offset with no scan.  Otherwise fall back to the
    // prefix `is_ascii()` check before decoding.
    if is_ascii {
        return byte_off;
    }
    let prefix = &s[..byte_off];
    if prefix.is_ascii() {
        byte_off
    } else {
        prefix.chars().count()
    }
}

/// Convert char-based start/end args (args[1], args[2]) to byte offsets.
///
/// Returns `Ok(None)` when the requested window is inverted (`start > stop`
/// after clamping to string bounds). Callers must treat `None` as an empty
/// search range (return -1 / 0 / raise ValueError as appropriate).
/// This matches CPython's `adjust_indices` contract — an inverted window is
/// distinct from a zero-length equal window (`start == stop`), which is
/// represented as `Some((n, n))`.
fn str_slice_args(s: &str, is_ascii: bool, args: &[Value]) -> Result<Option<(usize, usize)>> {
    // Fast path: no start/end args — common case for find/startswith/etc.
    let has_start = args.get(1).is_some();
    let has_end = args.get(2).is_some();
    if !has_start && !has_end {
        return Ok(Some((0, s.len())));
    }

    // ASCII fast path: char index == byte index, no scanning needed.  `is_ascii`
    // is the O(1) cached flag (#2124) when available; otherwise the caller passes
    // `s.is_ascii()` directly.
    if is_ascii {
        let byte_len = s.len();
        // Do NOT clamp start before the inverted-window check: if the caller
        // passes start > len(s), that must produce None (not found / 0 count),
        // not a zero-length window at the end.  Mirror the Unicode path which
        // defers the start clamp until after the end_char < start_char test.
        let start_char = match args.get(1).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => normalise_char_idx(i, byte_len),
            Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, byte_len),
            Some(ValueKind::BigInt(n)) => bigint_start_idx(n, byte_len),
            Some(ValueKind::None) | None => 0,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "slice indices must be integers or None or have an __index__ method",
                ));
            }
        };
        let end_char = match args.get(2).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => normalise_char_idx(i, byte_len).min(byte_len),
            Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, byte_len).min(byte_len),
            Some(ValueKind::BigInt(n)) => bigint_end_idx(n, byte_len),
            Some(ValueKind::None) | None => byte_len,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "slice indices must be integers or None or have an __index__ method",
                ));
            }
        };
        if end_char < start_char {
            return Ok(None);
        }
        return Ok(Some((start_char.min(byte_len), end_char)));
    }

    // Unicode: single scan for char_len + both byte positions
    let char_len = s.chars().count();
    let start_char = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_char_idx(i, char_len),
        Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, char_len),
        Some(ValueKind::BigInt(n)) => bigint_start_idx(n, char_len),
        Some(ValueKind::None) | None => 0,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or None or have an __index__ method",
            ));
        }
    };
    let end_char = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_char_idx(i, char_len).min(char_len),
        Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, char_len).min(char_len),
        Some(ValueKind::BigInt(n)) => bigint_end_idx(n, char_len),
        Some(ValueKind::None) | None => char_len,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or None or have an __index__ method",
            ));
        }
    };
    // Inverted window: start > stop after normalisation — signal to caller.
    if end_char < start_char {
        return Ok(None);
    }
    // Clamp start_char to char_len so the single-pass loop terminates correctly.
    let start_char = start_char.min(char_len);
    // Single pass to find both byte positions
    let mut start_byte = s.len();
    let mut end_byte = s.len();
    for (i, (b, _)) in s.char_indices().enumerate() {
        if i == start_char {
            start_byte = b;
        }
        if i == end_char {
            end_byte = b;
            break;
        }
    }
    Ok(Some((start_byte, end_byte)))
}

fn normalise_char_idx(idx: i64, len: usize) -> usize {
    if idx < 0 {
        let from_end = (-idx) as usize;
        len.saturating_sub(from_end)
    } else {
        idx as usize
    }
}

/// Normalise a `BigInt` `start` bound for a search window. A BigInt never fits
/// in an index range, so CPython clamps it: a negative one to the start (`0`), a
/// positive one to just past the end (`len + 1`) so the inverted-window check in
/// the caller reports "not found" rather than a zero-length window (#2688).
fn bigint_start_idx(n: &pyrust_core::PyBigInt, len: usize) -> usize {
    match n.sign() {
        PyBigIntSign::Minus => 0,
        _ => len + 1,
    }
}

/// Normalise a `BigInt` `end` bound for a search window: a negative one clamps to
/// the start (`0`), a positive one to the end (`len`) (#2688).
fn bigint_end_idx(n: &pyrust_core::PyBigInt, len: usize) -> usize {
    match n.sign() {
        PyBigIntSign::Minus => 0,
        _ => len,
    }
}

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

/// `str.encode(encoding='utf-8', errors='strict')`
///
/// Positional args have the same semantics as keyword args; the caller
/// (`str_merge_kwargs`) normalises keyword forms into positional slots
/// before this function is reached.
fn str_encode(s: &str, args: &[Value]) -> Result<Value> {
    if args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("encode() takes at most 2 arguments ({} given)", args.len()),
        ));
    }
    let encoding: &str = match args.first() {
        None => "utf-8",
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "encode() argument 'encoding' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };
    let errors: &str = match args.get(1) {
        None => "strict",
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "encode() argument 'errors' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };
    encode_str_to_bytes(s, encoding, errors)
}

/// Encode a Python `str` to `bytes`.
///
/// Supports `utf-8`, `ascii`, `latin-1` (and CPython aliases).
/// Other encoding names raise `LookupError: unknown encoding: <name>`.
///
/// `errors="strict"` raises `UnicodeEncodeError` on unencodable characters;
/// `"ignore"` silently drops them; `"replace"` substitutes `b'?'`;
/// `"backslashreplace"` substitutes `\xHH`, `\uHHHH`, or `\UHHHHHHHH`;
/// `"xmlcharrefreplace"` substitutes `&#NNN;` (decimal codepoint);
/// `"namereplace"` substitutes `\N{NAME}`, falling back to the backslash form.
pub fn encode_str_to_bytes(source: &str, encoding: &str, errors: &str) -> Result<Value> {
    fn normalize(name: &str) -> String {
        name.to_ascii_lowercase().replace('_', "-")
    }
    let canonical = normalize(encoding);

    enum Handler {
        Strict,
        Ignore,
        Replace,
        BackslashReplace,
        XmlCharRefReplace,
        NameReplace,
        SurrogatePass,
    }

    fn resolve_handler(errors: &str) -> Result<Handler> {
        match errors {
            "strict" => Ok(Handler::Strict),
            "ignore" => Ok(Handler::Ignore),
            "replace" => Ok(Handler::Replace),
            "backslashreplace" => Ok(Handler::BackslashReplace),
            "xmlcharrefreplace" => Ok(Handler::XmlCharRefReplace),
            "namereplace" => Ok(Handler::NameReplace),
            "surrogatepass" => Ok(Handler::SurrogatePass),
            other => Err(PyError::named(
                "LookupError",
                format!("unknown error handler name '{other}'"),
            )),
        }
    }

    /// True for a lone surrogate codepoint (U+D800..=U+DFFF).
    fn is_surrogate(cp: u32) -> bool {
        (0xD800..=0xDFFF).contains(&cp)
    }

    /// Produce the backslash-escape bytes for a single unencodable codepoint.
    /// `\xHH` for cp < 0x100, `\uHHHH` for cp < 0x10000, `\UHHHHHHHH` otherwise.
    fn backslash_escape_bytes(cp: u32) -> Vec<u8> {
        if cp < 0x100 {
            format!("\\x{:02x}", cp).into_bytes()
        } else if cp < 0x10000 {
            format!("\\u{:04x}", cp).into_bytes()
        } else {
            format!("\\U{:08x}", cp).into_bytes()
        }
    }

    fn encode_single_byte_codec(
        source: &str,
        codec_name: &str,
        fits: impl Fn(u32) -> bool,
        range_label: &str,
        errors: &str,
    ) -> Result<Value> {
        // Iterate codepoints via cesu8_codepoints so surrogate bytes never reach
        // char::chars() (a debug-abort hazard on the CESU-8 strings pyrust stores).
        let cps: Vec<u32> = cesu8_codepoints(source).collect();
        let mut out = Vec::with_capacity(source.len());
        let mut idx = 0usize;
        while idx < cps.len() {
            let cp = cps[idx];
            if fits(cp) {
                out.push(cp as u8);
                idx += 1;
            } else {
                match resolve_handler(errors)? {
                    Handler::Ignore => {
                        idx += 1;
                    }
                    Handler::Replace => {
                        out.push(b'?');
                        idx += 1;
                    }
                    Handler::BackslashReplace => {
                        out.extend_from_slice(&backslash_escape_bytes(cp));
                        idx += 1;
                    }
                    Handler::XmlCharRefReplace => {
                        out.extend_from_slice(format!("&#{};", cp).as_bytes());
                        idx += 1;
                    }
                    Handler::NameReplace => {
                        // Surrogates (from_u32 == None) have no name; fall back to
                        // the backslash escape form.
                        let replacement = match char::from_u32(cp).and_then(unicode_names2::name) {
                            Some(name) => format!("\\N{{{}}}", name).into_bytes(),
                            None => backslash_escape_bytes(cp),
                        };
                        out.extend_from_slice(&replacement);
                        idx += 1;
                    }
                    // For single-byte codecs CPython treats surrogatepass like
                    // strict (the surrogate still doesn't fit the byte range).
                    Handler::Strict | Handler::SurrogatePass => {
                        let run_start = idx;
                        let mut run_end = idx + 1;
                        while run_end < cps.len() && !fits(cps[run_end]) {
                            run_end += 1;
                        }
                        return Err(PyError::UnicodeEncodeError {
                            encoding: codec_name.to_string(),
                            object: source.to_string(),
                            start: run_start,
                            end: run_end,
                            reason: format!("ordinal not in range({range_label})"),
                        });
                    }
                }
            }
        }
        Ok(Value::bytes(out))
    }

    /// Surrogate-aware UTF-N encoder.
    ///
    /// Iterates codepoints via `cesu8_codepoints` (never `str::chars`, which would
    /// abort on the CESU-8 surrogate bytes pyrust stores), encoding non-surrogate
    /// codepoints with `encode_cp` and applying the resolved `errors` handler to
    /// lone surrogates.  `surrogate_bytes` produces the raw UTF-N bytes emitted by
    /// the `surrogatepass` handler.  Error positions are codepoint indices,
    /// matching CPython; consecutive surrogates are coalesced into one error run.
    fn encode_utf_surrogate_aware(
        source: &str,
        codec_name: &str,
        bom: &[u8],
        mut encode_cp: impl FnMut(u32, &mut Vec<u8>),
        surrogate_bytes: impl Fn(u32) -> Vec<u8>,
        errors: &str,
    ) -> Result<Value> {
        let cps: Vec<u32> = cesu8_codepoints(source).collect();
        let mut out = Vec::with_capacity(bom.len() + source.len());
        out.extend_from_slice(bom);
        let mut idx = 0usize;
        while idx < cps.len() {
            let cp = cps[idx];
            if !is_surrogate(cp) {
                encode_cp(cp, &mut out);
                idx += 1;
                continue;
            }
            match resolve_handler(errors)? {
                Handler::SurrogatePass => {
                    out.extend_from_slice(&surrogate_bytes(cp));
                    idx += 1;
                }
                Handler::Ignore => {
                    idx += 1;
                }
                Handler::Replace => {
                    out.push(b'?');
                    idx += 1;
                }
                Handler::BackslashReplace => {
                    out.extend_from_slice(&backslash_escape_bytes(cp));
                    idx += 1;
                }
                Handler::XmlCharRefReplace => {
                    out.extend_from_slice(format!("&#{};", cp).as_bytes());
                    idx += 1;
                }
                Handler::NameReplace => {
                    // Surrogates have no Unicode name; fall back to the backslash form.
                    out.extend_from_slice(&backslash_escape_bytes(cp));
                    idx += 1;
                }
                Handler::Strict => {
                    let run_start = idx;
                    let mut run_end = idx + 1;
                    while run_end < cps.len() && is_surrogate(cps[run_end]) {
                        run_end += 1;
                    }
                    return Err(PyError::UnicodeEncodeError {
                        encoding: codec_name.to_string(),
                        object: source.to_string(),
                        start: run_start,
                        end: run_end,
                        reason: "surrogates not allowed".to_string(),
                    });
                }
            }
        }
        Ok(Value::bytes(out))
    }

    // UTF-16 surrogatepass: emit the surrogate as a single 16-bit code unit.
    fn utf16_surrogate_bytes(cp: u32, big_endian: bool) -> Vec<u8> {
        let unit = cp as u16;
        if big_endian {
            unit.to_be_bytes().to_vec()
        } else {
            unit.to_le_bytes().to_vec()
        }
    }

    // Encode a non-surrogate scalar codepoint into UTF-16 code units (LE or BE).
    fn encode_cp_utf16(cp: u32, big_endian: bool, out: &mut Vec<u8>) {
        let c = char::from_u32(cp).expect("non-surrogate codepoint is a valid char");
        let mut buf = [0u16; 2];
        for unit in c.encode_utf16(&mut buf) {
            if big_endian {
                out.extend_from_slice(&unit.to_be_bytes());
            } else {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        }
    }

    match canonical.as_str() {
        "utf-8" | "utf8" | "u8" | "utf" => {
            // Fast path: a string with no 0xED byte cannot contain a CESU-8 lone
            // surrogate, so its bytes are already valid UTF-8 — copy them directly.
            if !source.as_bytes().contains(&0xED) {
                return Ok(Value::bytes(source.as_bytes().to_vec()));
            }
            encode_utf_surrogate_aware(
                source,
                "utf-8",
                b"",
                |cp, out| {
                    let c = char::from_u32(cp).expect("non-surrogate codepoint is a valid char");
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                },
                |cp| pyrust_core::cesu8_encode_codepoint(cp).into_bytes(),
                errors,
            )
        }
        // UTF-8-SIG: prepend U+FEFF BOM (EF BB BF) then UTF-8 encoded content.
        "utf-8-sig" => {
            if !source.as_bytes().contains(&0xED) {
                let mut out = Vec::with_capacity(3 + source.len());
                out.extend_from_slice(b"\xef\xbb\xbf");
                out.extend_from_slice(source.as_bytes());
                return Ok(Value::bytes(out));
            }
            encode_utf_surrogate_aware(
                source,
                "utf-8-sig",
                b"\xef\xbb\xbf",
                |cp, out| {
                    let c = char::from_u32(cp).expect("non-surrogate codepoint is a valid char");
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                },
                |cp| pyrust_core::cesu8_encode_codepoint(cp).into_bytes(),
                errors,
            )
        }
        "ascii" | "us-ascii" | "646" => {
            encode_single_byte_codec(source, "ascii", |cp| cp < 0x80, "128", errors)
        }
        "latin-1" | "iso-8859-1" | "8859" | "cp819" | "latin1" | "l1" => {
            encode_single_byte_codec(source, "latin-1", |cp| cp < 0x100, "256", errors)
        }
        "cp1252" | "windows-1252" => encode_cp1252(source, errors),
        "unicode-escape" => Ok(Value::bytes(encode_unicode_escape(source))),
        "raw-unicode-escape" => Ok(Value::bytes(encode_raw_unicode_escape(source))),
        "utf-7" => Ok(Value::bytes(encode_utf7(source))),
        // UTF-16 with LE BOM: \xff\xfe followed by LE-encoded code units.
        // "utf16" is the no-separator alias (normalize replaces _ with - but not
        // nothing, so "utf16" stays as-is and must be listed separately).
        "utf-16" | "utf16" => encode_utf_surrogate_aware(
            source,
            "utf-16",
            b"\xff\xfe",
            |cp, out| encode_cp_utf16(cp, false, out),
            |cp| utf16_surrogate_bytes(cp, false),
            errors,
        ),
        "utf-16-le" => encode_utf_surrogate_aware(
            source,
            "utf-16-le",
            b"",
            |cp, out| encode_cp_utf16(cp, false, out),
            |cp| utf16_surrogate_bytes(cp, false),
            errors,
        ),
        "utf-16-be" => encode_utf_surrogate_aware(
            source,
            "utf-16-be",
            b"",
            |cp, out| encode_cp_utf16(cp, true, out),
            |cp| utf16_surrogate_bytes(cp, true),
            errors,
        ),
        // UTF-32 with LE BOM: \xff\xfe\x00\x00 followed by LE-encoded code points.
        "utf-32" | "utf32" => encode_utf_surrogate_aware(
            source,
            "utf-32",
            b"\xff\xfe\x00\x00",
            |cp, out| out.extend_from_slice(&cp.to_le_bytes()),
            |cp| cp.to_le_bytes().to_vec(),
            errors,
        ),
        "utf-32-le" => encode_utf_surrogate_aware(
            source,
            "utf-32-le",
            b"",
            |cp, out| out.extend_from_slice(&cp.to_le_bytes()),
            |cp| cp.to_le_bytes().to_vec(),
            errors,
        ),
        "utf-32-be" => encode_utf_surrogate_aware(
            source,
            "utf-32-be",
            b"",
            |cp, out| out.extend_from_slice(&cp.to_be_bytes()),
            |cp| cp.to_be_bytes().to_vec(),
            errors,
        ),
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {encoding}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Additional codecs: cp1252, unicode_escape, raw_unicode_escape, utf-7
// ---------------------------------------------------------------------------

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

/// `str.maketrans(x[, y[, z]])` — static method.
///
/// 1-arg form: x must be a dict mapping (int ordinal | single-char str | None key) → replacement.
/// 2-arg form: x and y must be equal-length strings; returns {ord(c): ord(d) for c,d in zip(x,y)}.
/// 3-arg form: same as 2-arg, plus {ord(c): None for c in z}.
///
/// Returns a dict with integer keys (codepoint ordinals).
pub fn str_maketrans(args: &[Value]) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "maketrans expected at least 1 argument, got 0".to_string(),
        ));
    }
    if args.len() > 3 {
        return Err(PyError::named(
            "TypeError",
            format!("maketrans expected at most 3 arguments, got {}", args.len()),
        ));
    }

    let mut table: PyDict = PyDict::default();

    if args.len() == 1 {
        // 1-arg form: x must be a dict
        let dict = match args[0].kind() {
            ValueKind::Dict(d) => d,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "if you give only one argument to maketrans it must be a dict".to_string(),
                ));
            }
        };
        for (k, v) in dict.iter() {
            let ordinal: i64 = match k {
                PyKey::Int(n) => *n,
                PyKey::Bool(b) => *b as i64,
                PyKey::Str(sv) => {
                    let s = sv.as_str().unwrap_or("");
                    let mut chars = s.chars();
                    let first = chars.next().ok_or_else(|| {
                        PyError::named(
                            "ValueError",
                            "string keys in translate table must be of length 1".to_string(),
                        )
                    })?;
                    if chars.next().is_some() {
                        return Err(PyError::named(
                            "ValueError",
                            "string keys in translate table must be of length 1".to_string(),
                        ));
                    }
                    first as i64
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "keys in translate table must be strings or integers".to_string(),
                    ));
                }
            };
            table.insert(PyKey::Int(ordinal), v.clone());
        }
    } else {
        // 2-arg or 3-arg form: x and y must be equal-length strings
        let x = match args[0].kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "first maketrans argument must be a string if there is a second argument"
                        .to_string(),
                ));
            }
        };
        let y = match args[1].kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "maketrans() argument 2 must be str, not {}",
                        py_value_display_name(&args[1])
                    ),
                ));
            }
        };
        let x_chars: Vec<char> = x.chars().collect();
        let y_chars: Vec<char> = y.chars().collect();
        if x_chars.len() != y_chars.len() {
            return Err(PyError::named(
                "ValueError",
                "the first two maketrans arguments must have equal length".to_string(),
            ));
        }
        for (cx, cy) in x_chars.iter().zip(y_chars.iter()) {
            table.insert(PyKey::Int(*cx as i64), Value::int(*cy as i64));
        }
        if args.len() == 3 {
            let z = match args[2].kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "maketrans() argument 3 must be str, not {}",
                            py_value_display_name(&args[2])
                        ),
                    ));
                }
            };
            for cz in z.chars() {
                table.insert(PyKey::Int(cz as i64), Value::none());
            }
        }
    }

    Ok(Value::dict(table))
}

/// `str.translate(table)` — instance method.
///
/// Iterates over the Unicode codepoints of self. For each codepoint `cp`,
/// looks up `cp` in `table` (a dict with int keys, e.g. from `str.maketrans`):
/// - absent → keep character as-is
/// - `None`  → delete character
/// - `int`   → replace with `chr(int)`
/// - `str`   → replace with that string
fn str_translate(s: &str, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "str.translate() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    let dict = match args[0].kind() {
        ValueKind::Dict(d) => d,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "translate() argument must be a dict or mapping, not {}",
                    builtin_type_name(&args[0])
                ),
            ));
        }
    };

    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as i64;
        match dict.get(&PyKey::Int(cp)) {
            None => out.push(c),
            Some(v) => match v.kind() {
                ValueKind::None => { /* delete */ }
                ValueKind::Int(n) => {
                    if !(0..=0x10FFFF).contains(&n) {
                        return Err(PyError::named(
                            "ValueError",
                            "character mapping must be in range(0x110000)".to_string(),
                        ));
                    }
                    let cp = n as u32;
                    if (0xD800..=0xDFFF).contains(&cp) {
                        // Lone surrogates are not Unicode scalar values, so
                        // char::from_u32 rejects them. CPython's str type freely
                        // stores lone surrogates; match that by writing the
                        // CESU-8 three-byte sequence directly into the buffer.
                        // SAFETY: we hold &mut String's backing Vec exclusively,
                        // and the three bytes we push are a well-formed CESU-8
                        // encoding of a surrogate codepoint. Every other write to
                        // `out` goes through safe String methods, so the rest of
                        // the buffer is valid UTF-8. The combined byte sequence is
                        // the same representation pyrust uses for surrogate-
                        // containing strings throughout the runtime.
                        unsafe {
                            out.as_mut_vec().extend_from_slice(&[
                                0xE0 | (cp >> 12) as u8,
                                0x80 | ((cp >> 6) & 0x3F) as u8,
                                0x80 | (cp & 0x3F) as u8,
                            ]);
                        }
                    } else {
                        // Non-surrogate codepoints in 0..=0x10FFFF are valid
                        // Unicode scalar values; from_u32 is safe here.
                        let replacement = char::from_u32(cp)
                            .expect("non-surrogate in 0..=0x10FFFF is a valid char");
                        out.push(replacement);
                    }
                }
                ValueKind::BigInt(_) => {
                    // A BigInt is always outside the valid codepoint range
                    // 0..=0x10FFFF, so it can never be a legal mapping value.
                    return Err(PyError::named(
                        "ValueError",
                        "character mapping must be in range(0x110000)".to_string(),
                    ));
                }
                ValueKind::Bool(b) => {
                    // bool is a subclass of int; False=0 (NUL), True=1 (SOH)
                    let replacement =
                        char::from_u32(b as u32).expect("0 and 1 are valid codepoints");
                    out.push(replacement);
                }
                ValueKind::Str(repl) => {
                    out.push_str(repl);
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "character mapping must return integer, None or str".to_string(),
                    ));
                }
            },
        }
    }
    Ok(Value::string(out))
}
