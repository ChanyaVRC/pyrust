use pyrust_core::{PyError, PyKey, Result, Value, ValueKind};
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

pub fn call(method: &str, src: &Value, args: &[Value]) -> Result<Value> {
    let s: &str = src.as_str().unwrap();
    match method {
        // Common Sequence Operations (via char indexing)
        "index" => str_index(s, args),
        "count" => str_count(s, args),
        // Splitting / joining
        "split" => split(src, s, args),
        "rsplit" => rsplit(src, s, args),
        "join" => join(s, args),
        "splitlines" => str_splitlines(s, args),
        "partition" => str_partition(s, args),
        "rpartition" => str_rpartition(s, args),
        // Stripping
        "strip" => Ok(Value::string(strip_chars(s, args, true, true))),
        "lstrip" => Ok(Value::string(strip_chars(s, args, true, false))),
        "rstrip" => Ok(Value::string(strip_chars(s, args, false, true))),
        // Prefix/suffix removal
        "removeprefix" => str_removeprefix(s, args),
        "removesuffix" => str_removesuffix(s, args),
        // Justification / padding
        "center" => str_center(s, args),
        "ljust" => str_ljust(s, args),
        "rjust" => str_rjust(s, args),
        "zfill" => str_zfill(s, args),
        "expandtabs" => str_expandtabs(s, args),
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
            !s.is_empty() && s.chars().all(is_python_alpha),
        )),
        "isalnum" => Ok(Value::bool_(
            !s.is_empty() && s.chars().all(|c| c.is_alphanumeric()),
        )),
        "isspace" => Ok(Value::bool_(
            !s.is_empty() && s.chars().all(|c| c.is_whitespace()),
        )),
        "isdecimal" => Ok(Value::bool_(
            !s.is_empty()
                && s.chars()
                    .all(|c| c.general_category() == GeneralCategory::DecimalNumber),
        )),
        "isnumeric" => Ok(Value::bool_(
            !s.is_empty() && s.chars().all(is_python_numeric),
        )),
        "islower" => Ok(Value::bool_(str_islower(s))),
        "isupper" => Ok(Value::bool_(str_isupper(s))),
        "istitle" => Ok(Value::bool_(str_istitle(s))),
        "isascii" => Ok(Value::bool_(s.is_ascii())),
        "isidentifier" => Ok(Value::bool_(str_isidentifier(s))),
        "isprintable" => Ok(Value::bool_(s.chars().all(is_printable))),
        _ => Err(PyError::Runtime(format!(
            "'str' object has no attribute '{method}'"
        ))),
    }
}

// ─── New method implementations ──────────────────────────────────────────────

fn str_require_str_arg<'a>(args: &'a [Value], method: &str) -> Result<&'a str> {
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(s)) => Ok(s),
        _ => Err(PyError::Runtime(format!(
            "str.{method}() requires a str argument"
        ))),
    }
}

fn str_require_int_arg(args: &[Value], method: &str) -> Result<i64> {
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => Ok(n),
        Some(ValueKind::Bool(b)) => Ok(b as i64),
        _ => Err(PyError::Runtime(format!(
            "str.{method}() requires an integer argument"
        ))),
    }
}

fn fill_char_arg(args: &[Value]) -> Result<char> {
    match args.get(1).map(|v| v.kind()) {
        None => Ok(' '),
        Some(ValueKind::Str(s)) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(c),
                _ => Err(PyError::Named(
                    "TypeError".to_string(),
                    "The fill character must be exactly one character long".to_string(),
                )),
            }
        }
        _ => Err(PyError::Named(
            "TypeError".to_string(),
            "The fill character must be a str".to_string(),
        )),
    }
}

fn str_center(s: &str, args: &[Value]) -> Result<Value> {
    let width = str_require_int_arg(args, "center")?.max(0) as usize;
    let fill = fill_char_arg(args)?;
    let char_len = s.chars().count();
    if char_len >= width {
        return Ok(Value::string(s));
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
    Ok(Value::string(out))
}

fn str_ljust(s: &str, args: &[Value]) -> Result<Value> {
    let width = str_require_int_arg(args, "ljust")?.max(0) as usize;
    let fill = fill_char_arg(args)?;
    let char_len = s.chars().count();
    if char_len >= width {
        return Ok(Value::string(s));
    }
    let pad = width - char_len;
    let mut out = String::with_capacity(s.len() + pad * fill.len_utf8());
    out.push_str(s);
    for _ in 0..pad {
        out.push(fill);
    }
    Ok(Value::string(out))
}

fn str_rjust(s: &str, args: &[Value]) -> Result<Value> {
    let width = str_require_int_arg(args, "rjust")?.max(0) as usize;
    let fill = fill_char_arg(args)?;
    let char_len = s.chars().count();
    if char_len >= width {
        return Ok(Value::string(s));
    }
    let pad = width - char_len;
    let mut out = String::with_capacity(s.len() + pad * fill.len_utf8());
    for _ in 0..pad {
        out.push(fill);
    }
    out.push_str(s);
    Ok(Value::string(out))
}

fn str_zfill(s: &str, args: &[Value]) -> Result<Value> {
    let width = str_require_int_arg(args, "zfill")?.max(0) as usize;
    let char_len = s.chars().count();
    if char_len >= width {
        return Ok(Value::string(s));
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
    Ok(Value::string(out))
}

fn str_expandtabs(s: &str, args: &[Value]) -> Result<Value> {
    let tabsize = match args.first().map(|v| v.kind()) {
        None => 8i64,
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        _ => {
            return Err(PyError::Runtime(
                "str.expandtabs() tabsize must be an integer".to_string(),
            ));
        }
    };
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
    Ok(Value::string(out))
}

fn str_partition(s: &str, args: &[Value]) -> Result<Value> {
    let sep = str_require_str_arg(args, "partition")?;
    if sep.is_empty() {
        return Err(PyError::Named(
            "ValueError".to_string(),
            "empty separator".to_string(),
        ));
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

fn str_rpartition(s: &str, args: &[Value]) -> Result<Value> {
    let sep = str_require_str_arg(args, "rpartition")?;
    if sep.is_empty() {
        return Err(PyError::Named(
            "ValueError".to_string(),
            "empty separator".to_string(),
        ));
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
    let keepends = match args.first().map(|v| v.kind()) {
        None => false,
        Some(ValueKind::Bool(b)) => b,
        Some(ValueKind::Int(n)) => n != 0,
        _ => {
            return Err(PyError::Runtime(
                "str.splitlines() keepends must be bool or int".to_string(),
            ));
        }
    };
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

fn str_removeprefix(s: &str, args: &[Value]) -> Result<Value> {
    let prefix = str_require_str_arg(args, "removeprefix")?;
    if s.starts_with(prefix) {
        Ok(Value::string(&s[prefix.len()..]))
    } else {
        Ok(Value::string(s))
    }
}

fn str_removesuffix(s: &str, args: &[Value]) -> Result<Value> {
    let suffix = str_require_str_arg(args, "removesuffix")?;
    if s.ends_with(suffix) {
        Ok(Value::string(&s[..s.len() - suffix.len()]))
    } else {
        Ok(Value::string(s))
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
        _ => {
            return Err(PyError::Runtime(
                "str.index() requires a str argument".to_string(),
            ));
        }
    };
    let (start, end) = str_slice_args(s, args)?;
    let haystack = &s[start..end];
    match haystack.find(sub) {
        Some(byte_pos) => {
            let char_pos = s[..start + byte_pos].chars().count();
            Ok(Value::int(char_pos as i64))
        }
        None => Err(PyError::Runtime("substring not found".to_string())),
    }
}

fn str_count(s: &str, args: &[Value]) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        _ => {
            return Err(PyError::Runtime(
                "str.count() requires a str argument".to_string(),
            ));
        }
    };
    if sub.is_empty() {
        let (start, end) = str_slice_args(s, args)?;
        let haystack = &s[start..end];
        return Ok(Value::int((haystack.chars().count() + 1) as i64));
    }
    let (start, end) = str_slice_args(s, args)?;
    let haystack = &s[start..end];
    let n = haystack.match_indices(sub).count();
    Ok(Value::int(n as i64))
}

fn str_find(s: &str, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        _ => {
            return Err(PyError::Runtime(
                "str.find() requires a str argument".to_string(),
            ));
        }
    };
    let (start, end) = str_slice_args(s, args)?;
    let haystack = &s[start..end];
    match haystack.find(sub) {
        Some(byte_pos) => {
            let char_pos = s[..start + byte_pos].chars().count();
            Ok(Value::int(char_pos as i64))
        }
        None => {
            if raise_on_miss {
                Err(PyError::Runtime("substring not found".to_string()))
            } else {
                Ok(Value::int(-1))
            }
        }
    }
}

fn str_rfind(s: &str, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        _ => {
            return Err(PyError::Runtime(
                "str.rfind() requires a str argument".to_string(),
            ));
        }
    };
    let (start, end) = str_slice_args(s, args)?;
    let haystack = &s[start..end];
    match haystack.rfind(sub) {
        Some(byte_pos) => {
            let char_pos = s[..start + byte_pos].chars().count();
            Ok(Value::int(char_pos as i64))
        }
        None => {
            if raise_on_miss {
                Err(PyError::Runtime("substring not found".to_string()))
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
                return Err(PyError::Named(
                    "ValueError".to_string(),
                    "empty separator".to_string(),
                ));
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
            // For rsplit with no sep, reverse the whitespace split
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
                return Err(PyError::Named(
                    "ValueError".to_string(),
                    "empty separator".to_string(),
                ));
            }
            if maxsplit < 0 {
                let cap = s.len() / sep_str.len() + 1;
                let mut parts = Vec::with_capacity(cap);
                for p in s.split(sep_str) {
                    let off = subslice_offset(s, p);
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts.reverse();
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

fn join(sep: &str, args: &[Value]) -> Result<Value> {
    let iterable = args
        .first()
        .ok_or_else(|| PyError::Runtime("str.join() requires 1 argument".to_string()))?;
    let parts: Vec<String> = match iterable.kind() {
        ValueKind::List(items) => items
            .iter()
            .map(|v| match v.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(PyError::Runtime("sequence item must be str".to_string())),
            })
            .collect::<Result<_>>()?,
        ValueKind::Tuple(items) => items
            .iter()
            .map(|v| match v.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(PyError::Runtime("sequence item must be str".to_string())),
            })
            .collect::<Result<_>>()?,
        ValueKind::Str(s) => s
            .chars()
            .map(|c| Ok(c.to_string()))
            .collect::<Result<_>>()?,
        ValueKind::Dict(d) => d
            .keys()
            .map(|k| match k {
                PyKey::Str(s) => Ok(s.clone()),
                _ => Err(PyError::Runtime("sequence item must be str".to_string())),
            })
            .collect::<Result<_>>()?,
        _ => {
            return Err(PyError::Runtime(
                "str.join() argument must be iterable".to_string(),
            ));
        }
    };
    Ok(Value::string(parts.join(sep)))
}

fn str_replace(s: &str, args: &[Value]) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::Runtime(
            "str.replace() requires 2 arguments".to_string(),
        ));
    }
    let old: &str = match args[0].kind() {
        ValueKind::Str(s) => s,
        _ => {
            return Err(PyError::Runtime(
                "str.replace() argument 1 must be str".to_string(),
            ));
        }
    };
    let new: &str = match args[1].kind() {
        ValueKind::Str(s) => s,
        _ => {
            return Err(PyError::Runtime(
                "str.replace() argument 2 must be str".to_string(),
            ));
        }
    };
    let count = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        None => -1,
        _ => {
            return Err(PyError::Runtime(
                "str.replace() count must be int".to_string(),
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
    let (start, end) = str_slice_args(s, args)?;
    let slice = &s[start..end];
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(p)) => Ok(Value::bool_(slice.starts_with(p))),
        Some(ValueKind::Tuple(prefixes)) => Ok(Value::bool_(prefixes.iter().any(|pv| {
            if let ValueKind::Str(p) = pv.kind() {
                slice.starts_with(p)
            } else {
                false
            }
        }))),
        _ => Err(PyError::Runtime(
            "str.startswith() first arg must be str or a tuple of str".to_string(),
        )),
    }
}

fn str_endswith(s: &str, args: &[Value]) -> Result<Value> {
    let (start, end) = str_slice_args(s, args)?;
    let slice = &s[start..end];
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(p)) => Ok(Value::bool_(slice.ends_with(p))),
        Some(ValueKind::Tuple(suffixes)) => Ok(Value::bool_(suffixes.iter().any(|sv| {
            if let ValueKind::Str(p) = sv.kind() {
                slice.ends_with(p)
            } else {
                false
            }
        }))),
        _ => Err(PyError::Runtime(
            "str.endswith() first arg must be str or a tuple of str".to_string(),
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

fn strip_chars(s: &str, args: &[Value], left: bool, right: bool) -> String {
    let chars_arg: Option<&str> = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(c)) => Some(c),
        Some(ValueKind::None) | None => None,
        _ => None,
    };
    match chars_arg {
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
    }
}

/// Parse (sep, maxsplit) from split/rsplit args.
fn split_args<'a>(args: &'a [Value]) -> Result<(Option<&'a str>, i64)> {
    let sep = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(s)) => Some(s),
        Some(ValueKind::None) | None => None,
        _ => {
            return Err(PyError::Runtime(
                "split() separator must be str or None".to_string(),
            ));
        }
    };
    let maxsplit = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        None => -1,
        _ => return Err(PyError::Runtime("split() maxsplit must be int".to_string())),
    };
    Ok((sep, maxsplit))
}

/// Convert char-based start/end args (args[1], args[2]) to byte offsets.
fn str_slice_args(s: &str, args: &[Value]) -> Result<(usize, usize)> {
    // Fast path: no start/end args — common case for find/startswith/etc.
    let has_start = args.get(1).is_some();
    let has_end = args.get(2).is_some();
    if !has_start && !has_end {
        return Ok((0, s.len()));
    }

    // ASCII fast path: char index == byte index, no scanning needed
    if s.is_ascii() {
        let byte_len = s.len();
        let start_char = match args.get(1).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => normalise_char_idx(i, byte_len).min(byte_len),
            Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, byte_len).min(byte_len),
            None => 0,
            _ => {
                return Err(PyError::Runtime(
                    "slice indices must be integers".to_string(),
                ));
            }
        };
        let end_char = match args.get(2).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => normalise_char_idx(i, byte_len).min(byte_len),
            Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, byte_len).min(byte_len),
            None => byte_len,
            _ => {
                return Err(PyError::Runtime(
                    "slice indices must be integers".to_string(),
                ));
            }
        };
        return Ok((start_char, end_char));
    }

    // Unicode: single scan for char_len + both byte positions
    let char_len = s.chars().count();
    let start_char = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_char_idx(i, char_len),
        Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, char_len),
        None => 0,
        _ => {
            return Err(PyError::Runtime(
                "slice indices must be integers".to_string(),
            ));
        }
    };
    let end_char = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_char_idx(i, char_len).min(char_len),
        Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, char_len).min(char_len),
        None => char_len,
        _ => {
            return Err(PyError::Runtime(
                "slice indices must be integers".to_string(),
            ));
        }
    };
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
    Ok((start_byte, end_byte))
}

fn normalise_char_idx(idx: i64, len: usize) -> usize {
    if idx < 0 {
        let from_end = (-idx) as usize;
        if from_end > len { 0 } else { len - from_end }
    } else {
        idx as usize
    }
}
