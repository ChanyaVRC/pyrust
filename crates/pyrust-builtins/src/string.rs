use indexmap::IndexMap;
use pyrust_core::{
    PyError, PyKey, Result, Value, ValueKind, builtin_type_name, expect_arg_count,
    extract_fill_char, extract_int, extract_optional_int,
};
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

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
];

/// Returns `true` if `method` is the name of a built-in `str` method.
/// Used by `hasattr` / `getattr` to validate attribute names without
/// invoking the method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

pub fn call(method: &str, src: &Value, args: Vec<Value>) -> Result<Value> {
    let s: &str = src.as_str().unwrap();
    let args = args.as_slice();
    match method {
        // Common Sequence Operations (via char indexing)
        "index" => str_index(s, args),
        "count" => str_count(s, args),
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
            let prefix = match args[0].kind() {
                ValueKind::Str(p) => p,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "removeprefix() argument must be str, not {}",
                            builtin_type_name(&args[0])
                        ),
                    ));
                }
            };
            Ok(str_removeprefix(s, prefix))
        }
        "removesuffix" => {
            expect_arg_count(args, 1, 1, "removesuffix")?;
            // CPython: "removesuffix() argument must be str, not <type>"
            let suffix = match args[0].kind() {
                ValueKind::Str(p) => p,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "removesuffix() argument must be str, not {}",
                            builtin_type_name(&args[0])
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
            Ok(str_center(s, width, fill))
        }
        "ljust" => {
            expect_arg_count(args, 1, 2, "ljust")?;
            let width = extract_int(&args[0], "ljust", "width")?;
            let fill = extract_fill_char(args)?;
            Ok(str_ljust(s, width, fill))
        }
        "rjust" => {
            expect_arg_count(args, 1, 2, "rjust")?;
            let width = extract_int(&args[0], "rjust", "width")?;
            let fill = extract_fill_char(args)?;
            Ok(str_rjust(s, width, fill))
        }
        "zfill" => {
            expect_arg_count(args, 1, 1, "zfill")?;
            let width = extract_int(&args[0], "zfill", "width")?;
            Ok(str_zfill(s, width))
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
            let tabsize = match extract_optional_int(args, 0)? {
                Some(n) => n,
                None => 8,
            };
            Ok(str_expandtabs(s, tabsize))
        }
        // Case
        "upper" => Ok(Value::string(s.to_uppercase())),
        "lower" => Ok(Value::string(s.to_lowercase())),
        "casefold" => Ok(Value::string(unicode_casefold(s))),
        "capitalize" => Ok(Value::string(capitalize(s))),
        "swapcase" => Ok(Value::string(swapcase(s))),
        "title" => Ok(Value::string(titlecase(s))),
        // Searching
        "find" => str_find(s, args, false),
        "rfind" => str_rfind(s, args, false),
        "rindex" => str_rfind(s, args, true),
        // Replacement
        "replace" => str_replace(s, args),
        // Testing
        "startswith" => str_startswith(s, args),
        "endswith" => str_endswith(s, args),
        "isdigit" => Ok(Value::bool_(
            !s.is_empty() && s.chars().all(is_python_digit),
        )),
        "isalpha" => Ok(Value::bool_(
            !s.is_empty()
                && if s.is_ascii() {
                    s.bytes().all(|b| b.is_ascii_alphabetic())
                } else {
                    s.chars().all(is_python_alpha)
                },
        )),
        "isalnum" => Ok(Value::bool_(
            !s.is_empty()
                && if s.is_ascii() {
                    s.bytes().all(|b| b.is_ascii_alphanumeric())
                } else {
                    s.chars().all(|c| c.is_alphanumeric())
                },
        )),
        "isspace" => Ok(Value::bool_(
            !s.is_empty()
                && if s.is_ascii() {
                    s.bytes()
                        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
                } else {
                    s.chars().all(|c| c.is_whitespace())
                },
        )),
        "isdecimal" => Ok(Value::bool_(
            !s.is_empty()
                && if s.is_ascii() {
                    s.bytes().all(|b| b.is_ascii_digit())
                } else {
                    s.chars()
                        .all(|c| c.general_category() == GeneralCategory::DecimalNumber)
                },
        )),
        "isnumeric" => Ok(Value::bool_(
            !s.is_empty()
                && if s.is_ascii() {
                    s.bytes().all(|b| b.is_ascii_digit())
                } else {
                    s.chars().all(is_python_numeric)
                },
        )),
        "islower" => Ok(Value::bool_(str_islower(s))),
        "isupper" => Ok(Value::bool_(str_isupper(s))),
        "istitle" => Ok(Value::bool_(str_istitle(s))),
        "isascii" => Ok(Value::bool_(s.is_ascii())),
        "isidentifier" => Ok(Value::bool_(str_isidentifier(s))),
        "isprintable" => Ok(Value::bool_(if s.is_ascii() {
            // Printable ASCII: 0x20 (space) through 0x7e (~). DEL (0x7f) is not printable.
            s.bytes().all(|b| b >= 0x20 && b < 0x7f)
        } else {
            s.chars().all(is_printable)
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
        _ => Err(PyError::Runtime(format!(
            "'str' object has no attribute '{method}'"
        ))),
    }
}

// ─── Method implementations ───────────────────────────────────────────────────

fn str_center(s: &str, width: i64, fill: char) -> Value {
    let width = width.max(0) as usize;
    let char_len = s.chars().count();
    if char_len >= width {
        return Value::string(s);
    }
    let marg = width - char_len;
    // CPython formula: left = marg//2 + (marg & width & 1)
    let left_pad = marg / 2 + (marg & width & 1);
    let right_pad = marg - left_pad;
    let mut out = String::with_capacity(s.len() + marg * fill.len_utf8());
    for _ in 0..left_pad {
        out.push(fill);
    }
    out.push_str(s);
    for _ in 0..right_pad {
        out.push(fill);
    }
    Value::string(out)
}

fn str_ljust(s: &str, width: i64, fill: char) -> Value {
    let width = width.max(0) as usize;
    let char_len = s.chars().count();
    if char_len >= width {
        return Value::string(s);
    }
    let pad = width - char_len;
    let mut out = String::with_capacity(s.len() + pad * fill.len_utf8());
    out.push_str(s);
    for _ in 0..pad {
        out.push(fill);
    }
    Value::string(out)
}

fn str_rjust(s: &str, width: i64, fill: char) -> Value {
    let width = width.max(0) as usize;
    let char_len = s.chars().count();
    if char_len >= width {
        return Value::string(s);
    }
    let pad = width - char_len;
    let mut out = String::with_capacity(s.len() + pad * fill.len_utf8());
    for _ in 0..pad {
        out.push(fill);
    }
    out.push_str(s);
    Value::string(out)
}

fn str_zfill(s: &str, width: i64) -> Value {
    let width = width.max(0) as usize;
    let char_len = s.chars().count();
    if char_len >= width {
        return Value::string(s);
    }
    let pad = width - char_len;
    let mut out = String::with_capacity(s.len() + pad);
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
    Value::string(out)
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
    // CPython coerces keepends via the standard truth protocol — any value is
    // accepted.  Delegate to Value::truthy() which covers all ValueKind arms
    // (including Dict, Set, BigInt, Range, Complex, BuiltinObject, etc.).
    let keepends = args.first().map_or(false, |v| v.truthy());
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

/// CPython `str.isprintable` semantics: printable unless in Control, Format,
/// Surrogate, PrivateUse, Unassigned, or Separator (except ASCII space U+0020).
fn is_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    !matches!(
        c.general_category(),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::SpaceSeparator
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
    )
}

/// Unicode full case-folding (CaseFolding.txt status F and S).
/// Handles multi-char expansions (ß→ss, ligatures) that Rust's `to_lowercase` misses.
fn unicode_casefold(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'ß' => out.push_str("ss"),
            'ẞ' => out.push_str("ss"),
            'ﬀ' => out.push_str("ff"),
            'ﬁ' => out.push_str("fi"),
            'ﬂ' => out.push_str("fl"),
            'ﬃ' => out.push_str("ffi"),
            'ﬄ' => out.push_str("ffl"),
            'ﬅ' | 'ﬆ' => out.push_str("st"),
            _ => {
                for lc in c.to_lowercase() {
                    out.push(lc);
                }
            }
        }
    }
    out
}

fn swapcase(s: &str) -> String {
    if s.is_ascii() {
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

fn titlecase(s: &str) -> String {
    if s.is_ascii() {
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
                out.extend(c.to_uppercase());
            }
            prev_cased = true;
        } else {
            out.push(c);
            prev_cased = false;
        }
    }
    out
}

fn str_islower(s: &str) -> bool {
    if s.is_ascii() {
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
    let mut has_cased = false;
    for c in s.chars() {
        if c.is_uppercase() {
            return false;
        }
        if c.is_lowercase() {
            has_cased = true;
        }
    }
    has_cased
}

fn str_isupper(s: &str) -> bool {
    if s.is_ascii() {
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
    let mut has_cased = false;
    for c in s.chars() {
        if c.is_lowercase() {
            return false;
        }
        if c.is_uppercase() {
            has_cased = true;
        }
    }
    has_cased
}

fn str_istitle(s: &str) -> bool {
    if s.is_ascii() {
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
    let mut prev_cased = false;
    let mut has_cased = false;
    for c in s.chars() {
        if c.is_uppercase() {
            if prev_cased {
                return false; // uppercase after cased (must follow non-cased)
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

fn str_isidentifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.is_ascii() {
        let mut bytes = s.bytes();
        let first = bytes.next().unwrap();
        if !first.is_ascii_alphabetic() && first != b'_' {
            return false;
        }
        return bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// Python's str.isnumeric(): includes Nd (decimal), No (other number like fractions,
/// superscript), and Nl (letter number). For ASCII this is the same as isdigit.
fn is_python_numeric(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::DecimalNumber
            | GeneralCategory::OtherNumber
            | GeneralCategory::LetterNumber
    )
}

// ─────────────────────────────────────────────────────────────────────────────

/// Python's str.isdigit(): Unicode Nd (DecimalNumber) category plus superscript/subscript digits.
fn is_python_digit(c: char) -> bool {
    // Nd covers all decimal digit scripts (Arabic-Indic, Devanagari, etc.)
    if c.general_category() == GeneralCategory::DecimalNumber {
        return true;
    }
    // Superscript/subscript digits have Numeric_Type=Digit but category No (OtherNumber)
    matches!(c as u32,
        0x00B2 | 0x00B3 | 0x00B9        // ²³¹
        | 0x2070 | 0x2074..=0x2079      // ⁰⁴⁵⁶⁷⁸⁹
        | 0x2080..=0x2089) // ₀₁₂₃₄₅₆₇₈₉
}

/// Python's str.isalpha(): Unicode general category L* (Letter).
fn is_python_alpha(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
    )
}

fn str_index(s: &str, args: &[Value]) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "must be str, not {}",
                    builtin_type_name(args.first().unwrap())
                ),
            ));
        }
        None => {
            return Err(PyError::named(
                "TypeError",
                "str.index() requires a str argument".to_string(),
            ));
        }
    };
    let Some((start, end)) = str_slice_args(s, args)? else {
        return Err(PyError::named(
            "ValueError",
            "substring not found".to_string(),
        ));
    };
    let haystack = &s[start..end];
    match haystack.find(sub) {
        Some(byte_pos) => {
            let char_pos = s[..start + byte_pos].chars().count();
            Ok(Value::int(char_pos as i64))
        }
        None => Err(PyError::named(
            "ValueError",
            "substring not found".to_string(),
        )),
    }
}

fn str_count(s: &str, args: &[Value]) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "must be str, not {}",
                    builtin_type_name(args.first().unwrap())
                ),
            ));
        }
        None => {
            return Err(PyError::named(
                "TypeError",
                "str.count() requires a str argument".to_string(),
            ));
        }
    };
    let Some((start, end)) = str_slice_args(s, args)? else {
        // Inverted window: CPython returns 0 for all substrings including empty.
        return Ok(Value::int(0));
    };
    if sub.is_empty() {
        let haystack = &s[start..end];
        return Ok(Value::int((haystack.chars().count() + 1) as i64));
    }
    let haystack = &s[start..end];
    let n = haystack.match_indices(sub).count();
    Ok(Value::int(n as i64))
}

fn str_find(s: &str, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "must be str, not {}",
                    builtin_type_name(args.first().unwrap())
                ),
            ));
        }
        None => {
            return Err(PyError::named(
                "TypeError",
                "str.find() requires a str argument".to_string(),
            ));
        }
    };
    let Some((start, end)) = str_slice_args(s, args)? else {
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
        Some(byte_pos) => {
            let char_pos = s[..start + byte_pos].chars().count();
            Ok(Value::int(char_pos as i64))
        }
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

fn str_rfind(s: &str, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "must be str, not {}",
                    builtin_type_name(args.first().unwrap())
                ),
            ));
        }
        None => {
            return Err(PyError::named(
                "TypeError",
                "str.rfind() requires a str argument".to_string(),
            ));
        }
    };
    let Some((start, end)) = str_slice_args(s, args)? else {
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
        Some(byte_pos) => {
            let char_pos = s[..start + byte_pos].chars().count();
            Ok(Value::int(char_pos as i64))
        }
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

fn join(sep: &str, args: &[Value]) -> Result<Value> {
    let iterable = args
        .first()
        .ok_or_else(|| PyError::Runtime("str.join() requires 1 argument".to_string()))?;
    let parts: Vec<String> = match iterable.kind() {
        ValueKind::List(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| match v.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        builtin_type_name(v),
                    ),
                )),
            })
            .collect::<Result<_>>()?,
        ValueKind::Tuple(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| match v.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        builtin_type_name(v),
                    ),
                )),
            })
            .collect::<Result<_>>()?,
        ValueKind::Str(s) => s
            .chars()
            .map(|c| Ok(c.to_string()))
            .collect::<Result<_>>()?,
        ValueKind::Dict(d) => d
            .keys()
            .enumerate()
            .map(|(i, k)| match k {
                PyKey::Str(s) => Ok(s.as_str().unwrap_or("").to_owned()),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        pykey_type_name(k),
                    ),
                )),
            })
            .collect::<Result<_>>()?,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "can only join an iterable".to_string(),
            ));
        }
    };
    Ok(Value::string(parts.join(sep)))
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
                    builtin_type_name(&args[0])
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
                    builtin_type_name(&args[1])
                ),
            ));
        }
    };
    let count = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        None => -1,
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
    if count < 0 {
        Ok(Value::string(s.replace(old, new)))
    } else {
        Ok(Value::string(s.replacen(old, new, count as usize)))
    }
}

fn str_startswith(s: &str, args: &[Value]) -> Result<Value> {
    // str_slice_args returns None for an inverted window (start > end).
    // For a Str prefix, that is an immediate False.
    // For a Tuple, CPython still validates element types even on an inverted
    // window — TypeError takes priority over the inverted-range short-circuit.
    let window = str_slice_args(s, args)?;
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
                        if let Some((start, end)) = window {
                            if s[start..end].starts_with(p) {
                                return Ok(Value::bool_(true));
                            }
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

fn str_endswith(s: &str, args: &[Value]) -> Result<Value> {
    // str_slice_args returns None for an inverted window (start > end).
    // For a Str suffix, that is an immediate False.
    // For a Tuple, CPython still validates element types even on an inverted
    // window — TypeError takes priority over the inverted-range short-circuit.
    let window = str_slice_args(s, args)?;
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
                        if let Some((start, end)) = window {
                            if s[start..end].ends_with(p) {
                                return Ok(Value::bool_(true));
                            }
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
            out.extend(first.to_uppercase());
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

/// Convert char-based start/end args (args[1], args[2]) to byte offsets.
///
/// Returns `Ok(None)` when the requested window is inverted (`start > stop`
/// after clamping to string bounds). Callers must treat `None` as an empty
/// search range (return -1 / 0 / raise ValueError as appropriate).
/// This matches CPython's `adjust_indices` contract — an inverted window is
/// distinct from a zero-length equal window (`start == stop`), which is
/// represented as `Some((n, n))`.
fn str_slice_args(s: &str, args: &[Value]) -> Result<Option<(usize, usize)>> {
    // Fast path: no start/end args — common case for find/startswith/etc.
    let has_start = args.get(1).is_some();
    let has_end = args.get(2).is_some();
    if !has_start && !has_end {
        return Ok(Some((0, s.len())));
    }

    // ASCII fast path: char index == byte index, no scanning needed
    if s.is_ascii() {
        let byte_len = s.len();
        let start_char = match args.get(1).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => normalise_char_idx(i, byte_len).min(byte_len),
            Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, byte_len).min(byte_len),
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
        return Ok(Some((start_char, end_char)));
    }

    // Unicode: single scan for char_len + both byte positions
    let char_len = s.chars().count();
    let start_char = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_char_idx(i, char_len),
        Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, char_len),
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
                        builtin_type_name(v)
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
                        builtin_type_name(v)
                    ),
                ));
            }
        },
    };
    encode_str_to_bytes(s, encoding, errors)
}

/// Format a single Unicode codepoint the way CPython does in
/// `UnicodeEncodeError` messages: `\xXX` for `< 0x100`, `\uXXXX` for
/// `< 0x10000`, otherwise `\UXXXXXXXX`.
fn format_codepoint_repr(cp: u32) -> String {
    if cp < 0x100 {
        format!("\\x{:02x}", cp)
    } else if cp < 0x10000 {
        format!("\\u{:04x}", cp)
    } else {
        format!("\\U{:08x}", cp)
    }
}

/// Encode a Python `str` to `bytes`.
///
/// Supports `utf-8`, `ascii`, `latin-1` (and CPython aliases).
/// Other encoding names raise `LookupError: unknown encoding: <name>`.
///
/// `errors="strict"` raises `UnicodeEncodeError` on unencodable characters;
/// `"ignore"` silently drops them; `"replace"` substitutes `b'?'`.
pub fn encode_str_to_bytes(source: &str, encoding: &str, errors: &str) -> Result<Value> {
    fn normalize(name: &str) -> String {
        name.to_ascii_lowercase().replace('_', "-")
    }
    let canonical = normalize(encoding);

    enum Handler {
        Strict,
        Ignore,
        Replace,
    }

    fn resolve_handler(errors: &str) -> Result<Handler> {
        match errors {
            "strict" => Ok(Handler::Strict),
            "ignore" => Ok(Handler::Ignore),
            "replace" => Ok(Handler::Replace),
            other => Err(PyError::named(
                "LookupError",
                format!("unknown error handler name '{other}'"),
            )),
        }
    }

    fn encode_single_byte_codec(
        source: &str,
        codec_name: &str,
        fits: impl Fn(u32) -> bool,
        range_label: &str,
        errors: &str,
    ) -> Result<Value> {
        let chars: Vec<char> = source.chars().collect();
        let mut out = Vec::with_capacity(source.len());
        let mut idx = 0usize;
        while idx < chars.len() {
            let cp = chars[idx] as u32;
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
                    Handler::Strict => {
                        let run_start = idx;
                        let mut run_end = idx + 1;
                        while run_end < chars.len() && !fits(chars[run_end] as u32) {
                            run_end += 1;
                        }
                        let run_len = run_end - run_start;
                        let msg = if run_len == 1 {
                            format!(
                                "'{codec_name}' codec can't encode character '{}' in position {run_start}: ordinal not in range({range_label})",
                                format_codepoint_repr(cp),
                            )
                        } else {
                            format!(
                                "'{codec_name}' codec can't encode characters in position {run_start}-{}: ordinal not in range({range_label})",
                                run_end - 1,
                            )
                        };
                        return Err(PyError::named("UnicodeEncodeError", msg));
                    }
                }
            }
        }
        Ok(Value::bytes(out))
    }

    match canonical.as_str() {
        "utf-8" | "utf8" | "u8" | "utf" => Ok(Value::bytes(source.as_bytes().to_vec())),
        "ascii" | "us-ascii" | "646" => {
            encode_single_byte_codec(source, "ascii", |cp| cp < 0x80, "128", errors)
        }
        "latin-1" | "iso-8859-1" | "8859" | "cp819" | "latin1" | "l1" => {
            encode_single_byte_codec(source, "latin-1", |cp| cp < 0x100, "256", errors)
        }
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {encoding}"),
        )),
    }
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

    let mut table: IndexMap<PyKey, Value> = IndexMap::new();

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
                        format!(
                            "keys in translate table must be strings or integers, not {}",
                            pykey_type_name(k)
                        ),
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
                    format!(
                        "maketrans argument 1 must be str, not {}",
                        builtin_type_name(&args[0])
                    ),
                ));
            }
        };
        let y = match args[1].kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "maketrans argument 2 must be str, not {}",
                        builtin_type_name(&args[1])
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
                            "maketrans argument 3 must be str, not {}",
                            builtin_type_name(&args[2])
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
                "translate() takes exactly one argument ({} given)",
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
                    let code = n as u32;
                    let replacement = char::from_u32(code).ok_or_else(|| {
                        PyError::named(
                            "ValueError",
                            "character mapping must return integer, None or str".to_string(),
                        )
                    })?;
                    out.push(replacement);
                }
                ValueKind::Bool(b) => {
                    let code = b as u32;
                    let replacement = char::from_u32(code).ok_or_else(|| {
                        PyError::named(
                            "ValueError",
                            "character mapping must return integer, None or str".to_string(),
                        )
                    })?;
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
