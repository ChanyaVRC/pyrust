// `html` package module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/html/__init__.py` (issues #3149, #3446).
//
// * `escape(s, quote=True)` follows the CPython source's `str.replace` chain.
//   An exact `str` takes a single-pass fast path; anything else (a `str`
//   subclass overriding `replace`, a non-`str`) goes through the same
//   `s.replace(...)` method calls the source performs, so the errors match.
// * `unescape(s)` scans `_charref`'s grammar by hand
//   (`&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\f <&#;]{1,32};?)`) and resolves
//   each reference like `_replace_charref`, using the shared, immutable
//   `html.entities.html5` table from `html_entities/tables.rs` (indexed once
//   per process into a hash map; no interpreter state is cached).
// * `post_load_init` binds what the CPython source gets from its imports and
//   module-level assignments: `_re`, `_html5`, `_charref` (the compiled
//   pattern), the `html.entities` submodule, `_invalid_charrefs` and
//   `_invalid_codepoints`.
//
// Reference: <https://docs.python.org/3/library/html.html>

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::html_entities::tables::HTML5;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyDict, PyKey, PyModule, PySet, Value};

// `def`s of `Lib/html/__init__.py`: CPython reports arity errors for these in
// the interpreter's `initialize_locals` wording, not the macro's.
const SIG_ESCAPE: Sig = Sig {
    qualname: "escape",
    params: &["s", "quote"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_UNESCAPE: Sig = Sig {
    qualname: "unescape",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_REPLACE_CHARREF: Sig = Sig {
    qualname: "_replace_charref",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

const MODULE_DOC: &str = "\nGeneral functions for HTML manipulation.\n";

/// `_charref`'s pattern, character for character.
const CHARREF_PATTERN: &str = "&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\\t\\n\\f <&#;]{1,32};?)";

/// Decimal digits above which `int(str)` may hit the interpreter's
/// int/str conversion limit (its smallest settable value is 640), in which
/// case the run is handed to the real `int()` so the error matches CPython.
const INT_STR_LIMIT_PROBE: usize = 640;

/// Code points 0x80..=0x9f remapped by the HTML5 numeric-reference rules
/// (Windows-1252 for the assigned slots, the C1 control itself otherwise).
const C1_REPLACEMENTS: [u32; 32] = [
    0x20ac, 0x81, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x8d, 0x017d, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014,
    0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0x9d, 0x017e, 0x0178,
];

fn html5_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| HTML5.iter().copied().collect())
}

fn all_list() -> Value {
    Value::list(vec![Value::string("escape"), Value::string("unescape")])
}

// ── escape ───────────────────────────────────────────────────────────────────

fn escape_str(s: &str, quote: bool) -> Option<String> {
    let needs = |b: u8| matches!(b, b'&' | b'<' | b'>') || (quote && matches!(b, b'"' | b'\''));
    if !s.bytes().any(needs) {
        return None;
    }
    let mut out = String::with_capacity(s.len() + s.len() / 8 + 8);
    let mut last = 0;
    for (i, b) in s.bytes().enumerate() {
        if !needs(b) {
            continue;
        }
        out.push_str(&s[last..i]);
        out.push_str(match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            _ => "&#x27;",
        });
        last = i + 1;
    }
    out.push_str(&s[last..]);
    Some(out)
}

fn call_replace(interp: &mut Interpreter, target: &Value, old: &str, new: &str) -> Result<Value> {
    let callee = interp.get_attr(target, "replace")?;
    interp.call_function_expanded(
        callee,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string(old),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(new),
            },
        ],
    )
}

fn escape_impl(interp: &mut Interpreter, s: &Value, quote: &Value) -> Result<Value> {
    if let Some(text) = s.as_str() {
        let quote = interp.truthy_value(quote)?;
        return Ok(match escape_str(text, quote) {
            Some(out) => Value::string(out),
            None => s.clone(),
        });
    }
    // Duck-typed path: exactly the `s.replace(...)` chain of the source.
    let mut s = call_replace(interp, s, "&", "&amp;")?;
    s = call_replace(interp, &s, "<", "&lt;")?;
    s = call_replace(interp, &s, ">", "&gt;")?;
    if interp.truthy_value(quote)? {
        s = call_replace(interp, &s, "\"", "&quot;")?;
        s = call_replace(interp, &s, "'", "&#x27;")?;
    }
    Ok(s)
}

// ── unescape ─────────────────────────────────────────────────────────────────

/// Whether `b` ends a named reference (`[^\t\n\f <&#;]` complement).
#[inline]
fn is_named_stop(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | 0x0c | b' ' | b'<' | b'&' | b'#' | b';')
}

#[inline]
fn is_char_start(b: u8) -> bool {
    b & 0xC0 != 0x80
}

/// A numeric reference's code point: `Some(n)` when it fits in `u32`
/// (saturating otherwise, since anything above 0x10ffff maps to U+FFFD).
fn parse_numeric(digits: &str, radix: u32) -> u32 {
    let sig = digits.trim_start_matches('0');
    if sig.is_empty() {
        return 0;
    }
    // 0x10ffff has 6 hex / 7 decimal digits; any longer significant run is
    // already out of range.
    let max_len = if radix == 16 { 6 } else { 7 };
    if sig.len() > max_len {
        return u32::MAX;
    }
    u32::from_str_radix(sig, radix).unwrap_or(u32::MAX)
}

/// `_replace_charref_str` for a numeric reference's code point.
fn numeric_replacement(num: u32, out: &mut String) {
    match num {
        0 => out.push('\u{fffd}'),
        0x0d => out.push('\r'),
        0x80..=0x9f => {
            let cp = C1_REPLACEMENTS[(num - 0x80) as usize];
            out.push(char::from_u32(cp).expect("C1 table holds scalar values"));
        }
        0xd800..=0xdfff => out.push('\u{fffd}'),
        n if n > 0x10ffff => out.push('\u{fffd}'),
        // `_invalid_codepoints`: C0 controls (but not tab/LF/FF/CR), DEL,
        // the noncharacter block and the last two code points of each plane.
        0x1..=0x8 | 0xb | 0xe..=0x1f | 0x7f | 0xfdd0..=0xfdef => {}
        n if n & 0xfffe == 0xfffe => {}
        n => out.push(char::from_u32(n).expect("range-checked scalar value")),
    }
}

/// `_replace_charref_str` for a named reference (`name` includes any `;`).
fn named_replacement(name: &str, out: &mut String) {
    let map = html5_map();
    if let Some(value) = map.get(name) {
        out.push_str(value);
        return;
    }
    // Longest matching prefix of at least two characters, then the rest.
    let bytes = name.as_bytes();
    let mut chars = 0usize;
    let mut boundaries: Vec<usize> = Vec::with_capacity(bytes.len());
    for (i, &b) in bytes.iter().enumerate() {
        if is_char_start(b) {
            boundaries.push(i);
            chars += 1;
        }
    }
    // Prefix lengths `len(s) - 1` down to 2 (in characters).
    for prefix_chars in (2..chars).rev() {
        let end = boundaries[prefix_chars];
        if let Some(value) = map.get(&name[..end]) {
            out.push_str(value);
            out.push_str(&name[end..]);
            return;
        }
    }
    out.push('&');
    out.push_str(name);
}

/// Resolve every character reference of `s`; `None` when there are none.
/// Decimal runs long enough to be subject to the int/str conversion limit are
/// reported through `long_decimal` so the caller can invoke `int()` on them.
fn unescape_str(
    s: &str,
    long_decimal: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Option<String>> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut out = String::new();
    let mut last = 0usize;
    let mut pos = 0usize;
    let mut replaced = false;
    while let Some(rel) = bytes[pos..].iter().position(|&b| b == b'&') {
        let amp = pos + rel;
        let start = amp + 1;
        if start >= n {
            break;
        }
        let mut end = 0usize;
        // (kind, digits_start, digits_end)
        enum Ref {
            Dec(usize, usize),
            Hex(usize, usize),
            Named,
        }
        let mut kind = Ref::Named;
        if bytes[start] == b'#' {
            if start + 1 < n && matches!(bytes[start + 1], b'x' | b'X') {
                let ds = start + 2;
                let mut j = ds;
                while j < n && bytes[j].is_ascii_hexdigit() {
                    j += 1;
                }
                if j > ds {
                    kind = Ref::Hex(ds, j);
                    end = if j < n && bytes[j] == b';' { j + 1 } else { j };
                }
            } else {
                let ds = start + 1;
                let mut j = ds;
                while j < n && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > ds {
                    kind = Ref::Dec(ds, j);
                    end = if j < n && bytes[j] == b';' { j + 1 } else { j };
                }
            }
        } else {
            let mut j = start;
            let mut count = 0usize;
            while j < n {
                let b = bytes[j];
                if is_named_stop(b) {
                    break;
                }
                if is_char_start(b) {
                    if count == 32 {
                        break;
                    }
                    count += 1;
                }
                j += 1;
            }
            if j > start {
                end = if j < n && bytes[j] == b';' { j + 1 } else { j };
            }
        }
        if end == 0 {
            pos = amp + 1;
            continue;
        }
        out.push_str(&s[last..amp]);
        match kind {
            Ref::Dec(ds, de) => {
                if de - ds > INT_STR_LIMIT_PROBE {
                    long_decimal(&s[ds..de])?;
                }
                numeric_replacement(parse_numeric(&s[ds..de], 10), &mut out);
            }
            Ref::Hex(ds, de) => numeric_replacement(parse_numeric(&s[ds..de], 16), &mut out),
            Ref::Named => named_replacement(&s[start..end], &mut out),
        }
        replaced = true;
        last = end;
        pos = end;
    }
    if !replaced {
        return Ok(None);
    }
    out.push_str(&s[last..]);
    Ok(Some(out))
}

/// `int(digits)` — raises exactly what CPython raises when the digit run
/// exceeds `sys.get_int_max_str_digits()`.
fn check_int_str_limit(interp: &mut Interpreter, digits: &str) -> Result<()> {
    let builtins = interp.load_module("builtins")?;
    let int_type = interp.get_attr(&builtins, "int")?;
    interp.call_function_expanded(
        int_type,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(digits),
        }],
    )?;
    Ok(())
}

fn unescape_impl(interp: &mut Interpreter, s: &Value) -> Result<Value> {
    if let Some(text) = s.as_str() {
        if !text.as_bytes().contains(&b'&') {
            return Ok(s.clone());
        }
        let resolved = unescape_str(text, &mut |digits| check_int_str_limit(interp, digits))?;
        return Ok(match resolved {
            Some(out) => Value::string(out),
            None => s.clone(),
        });
    }
    // `'&' not in s` with the real `in` operator (so non-strings raise the
    // same errors), then `_charref.sub` which only accepts text.
    if !crate::builtin_modules::mp_rt::py_contains(interp, s.clone(), Value::string("&"))? {
        return Ok(s.clone());
    }
    let Some(text) = crate::builtin_modules::_string::str_text(s) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "expected string or bytes-like object, got '{}'",
                value_type_name_str(s)
            ),
        ));
    };
    let resolved = unescape_str(&text, &mut |digits| check_int_str_limit(interp, digits))?;
    // `re.sub` hands back a plain `str` (never the subclass) once the
    // subject was scanned.
    Ok(Value::string(resolved.unwrap_or(text)))
}

/// `_replace_charref(s)`: `_replace_charref_str(s.group(1))`.
fn replace_charref_impl(interp: &mut Interpreter, m: &Value) -> Result<Value> {
    let group = interp.get_attr(m, "group")?;
    let name = interp.call_function_expanded(
        group,
        &[ExpandedCallArg {
            name: None,
            value: Value::int(1),
        }],
    )?;
    let Some(text) = crate::builtin_modules::_string::str_text(&name) else {
        return Err(PyError::named(
            "TypeError",
            "'NoneType' object is not subscriptable".to_string(),
        ));
    };
    let bytes = text.as_bytes();
    let mut out = String::new();
    if bytes.first() == Some(&b'#') {
        let hex = matches!(bytes.get(1), Some(b'x' | b'X'));
        let digits = text[if hex { 2 } else { 1 }..].trim_end_matches(';');
        if !hex && digits.len() > INT_STR_LIMIT_PROBE {
            check_int_str_limit(interp, digits)?;
        }
        numeric_replacement(parse_numeric(digits, if hex { 16 } else { 10 }), &mut out);
    } else {
        named_replacement(&text, &mut out);
    }
    Ok(Value::string(out))
}

// ── module members ───────────────────────────────────────────────────────────

fn build_invalid_charrefs() -> Value {
    let mut dict = PyDict::default();
    dict.insert(PyKey::Int(0x00), Value::string("\u{fffd}"));
    dict.insert(PyKey::Int(0x0d), Value::string("\r"));
    for (i, &cp) in C1_REPLACEMENTS.iter().enumerate() {
        let ch = char::from_u32(cp).expect("C1 table holds scalar values");
        dict.insert(PyKey::Int(0x80 + i as i64), Value::string(ch.to_string()));
    }
    Value::dict(dict)
}

fn build_invalid_codepoints() -> Value {
    let mut set = PySet::default();
    let mut add = |cp: i64| {
        set.insert(PyKey::Int(cp));
    };
    (0x1..=0x8).for_each(&mut add);
    (0xe..=0x1f).for_each(&mut add);
    (0x7f..=0x9f).for_each(&mut add);
    (0xfdd0..=0xfdef).for_each(&mut add);
    add(0xb);
    for plane in 0..=0x10i64 {
        add((plane << 16) | 0xfffe);
        add((plane << 16) | 0xffff);
    }
    Value::set(set)
}

/// Bind the names the CPython source gets from its imports and module-level
/// assignments once the module is in the import cache.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let re = interp.load_module("re")?;
    let entities = interp.load_module("html.entities")?;
    let html5 = interp.get_attr(&entities, "html5")?;
    let compile = interp.get_attr(&re, "compile")?;
    let charref = interp.call_function_expanded(
        compile,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(CHARREF_PATTERN),
        }],
    )?;
    let mut module = module.borrow_mut();
    module.insert_attr("_re".to_string(), re);
    module.insert_attr("_html5".to_string(), html5);
    module.insert_attr("entities".to_string(), entities);
    module.insert_attr("_charref".to_string(), charref);
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
        "__package__" => Value::string("html"),
        "_invalid_charrefs" => build_invalid_charrefs(),
        "_invalid_codepoints" => build_invalid_codepoints(),
    }

    // Replace special characters "&", "<" and ">" to HTML-safe sequences.
    // If the optional flag quote is true (the default), the quotation mark
    // characters, both double quote (") and single quote (') characters are
    // also translated.
    fn escape(args) -> Result<Value> {
        let mut b = args::bind(&SIG_ESCAPE, args)?;
        let s = b.take_or_none(0);
        let quote = b.take_or(1, || Value::bool_(true));
        escape_impl(_interp, &s, &quote)
    }

    // Convert all named and numeric character references (e.g. &gt;, &#62;,
    // &x3e;) in the string s to the corresponding unicode characters.
    fn unescape(args) -> Result<Value> {
        let mut b = args::bind(&SIG_UNESCAPE, args)?;
        let s = b.take_or_none(0);
        unescape_impl(_interp, &s)
    }

    fn _replace_charref(args) -> Result<Value> {
        let mut b = args::bind(&SIG_REPLACE_CHARREF, args)?;
        let s = b.take_or_none(0);
        replace_charref_impl(_interp, &s)
    }
}
