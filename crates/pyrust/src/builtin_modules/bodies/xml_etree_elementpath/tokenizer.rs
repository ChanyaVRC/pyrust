//! `xpath_tokenizer`: a native scanner that reproduces
//! `xpath_tokenizer_re.findall(pattern)` match for match, plus the lazy
//! namespace-resolving generator CPython wraps around it.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use super::{
    call_method, error_is, getitem, render_repr, str_text, syntax_error, truthy, type_error,
    type_name, value_str,
};
use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, ProviderIterator, builtin_data_backing};
use crate::value::{Value, ValueKind};

/// `Py_UNICODE_ISSPACE`, i.e. what `\s` matches in a `str` pattern.
fn is_space(c: char) -> bool {
    matches!(
        c as u32,
        0x09..=0x0D
            | 0x1C..=0x20
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

/// `[^/\[\]\(\)@!=\s]`: the characters a tag token is made of.
fn is_name_char(c: char) -> bool {
    !matches!(c, '/' | '[' | ']' | '(' | ')' | '@' | '!' | '=') && !is_space(c)
}

/// Byte length of the longest prefix of `s` whose chars satisfy `pred`.
fn run_len(s: &str, pred: fn(char) -> bool) -> usize {
    s.char_indices()
        .find(|&(_, c)| !pred(c))
        .map_or(s.len(), |(i, _)| i)
}

/// `xpath_tokenizer_re.findall(pattern)`: `(op, tag)` pairs, where
/// whitespace runs come back as `("", "")`.
///
/// The regex is
/// `('[^']*'|"[^"]*"|::|//?|\.\.|\(\)|!=|[/.*:\[\]\(\)@=])|((?:\{[^}]+\})?[^/\[\]\(\)@!=\s]+)|\s+`;
/// at each position the alternatives are tried in that order and a position
/// where none matches (only a lone `!` can do that) is skipped.
pub(super) fn scan(pattern: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pattern.len() {
        let rest = &pattern[i..];
        let Some(c) = rest.chars().next() else {
            break;
        };
        // Group 1: operators and quoted literals.
        let op_len = match c {
            '\'' | '"' => rest[1..].find(c).map(|j| j + 2),
            ':' if rest.starts_with("::") => Some(2),
            '/' if rest.starts_with("//") => Some(2),
            '.' if rest.starts_with("..") => Some(2),
            '(' if rest.starts_with("()") => Some(2),
            '!' if rest.starts_with("!=") => Some(2),
            '/' | '.' | '*' | ':' | '[' | ']' | '(' | ')' | '@' | '=' => Some(1),
            _ => None,
        };
        if let Some(len) = op_len {
            out.push((rest[..len].to_string(), String::new()));
            i += len;
            continue;
        }
        // Group 2: `(?:\{[^}]+\})?` followed by at least one name char.
        if c == '{'
            && let Some(close) = rest[1..].find('}').map(|j| j + 1)
            && close >= 2
        {
            let name = run_len(&rest[close + 1..], is_name_char);
            if name > 0 {
                let len = close + 1 + name;
                out.push((String::new(), rest[..len].to_string()));
                i += len;
                continue;
            }
        }
        let name = run_len(rest, is_name_char);
        if name > 0 {
            out.push((String::new(), rest[..name].to_string()));
            i += name;
            continue;
        }
        // `\s+`: neither group participates.
        let space = run_len(rest, is_space);
        if space > 0 {
            out.push((String::new(), String::new()));
            i += space;
            continue;
        }
        i += c.len_utf8();
    }
    out
}

/// Live state of one `xpath_tokenizer(pattern, namespaces)` generator.
struct Tokenizer {
    pattern: Value,
    namespaces: Value,
    /// `None` until the generator body starts (first `next()`).
    run: Option<TokenizerRun>,
}

struct TokenizerRun {
    default_namespace: Value,
    tokens: std::vec::IntoIter<(String, String)>,
    parsing_attribute: bool,
}

/// `xpath_tokenizer(pattern, namespaces)`: nothing runs until `next()`.
pub(super) fn make_tokenizer(pattern: Value, namespaces: Value) -> Value {
    let state = Tokenizer {
        pattern,
        namespaces,
        run: None,
    };
    Value::generator(Box::new(ProviderIterator::new(
        Box::new(Some(Some(state))),
        advance_tokenizer,
        "generator",
        "generator",
        None,
    )))
}

fn advance_tokenizer(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    let taken = state_rc
        .borrow_mut()
        .downcast_mut::<Option<Option<Tokenizer>>>()
        .map(Option::take);
    let mut st = match taken {
        Some(Some(Some(st))) => st,
        // Exhausted, or closed by an earlier exception.
        Some(Some(None)) => {
            restore(state_rc, None);
            return Ok(None);
        }
        Some(None) => {
            return Err(PyError::named("ValueError", "generator already executing"));
        }
        None => {
            return Err(PyError::runtime(
                "internal: xpath_tokenizer state corrupted".to_string(),
            ));
        }
    };
    let result = tokenizer_step(interp, &mut st);
    let keep = matches!(result, Ok(Some(_)));
    restore(state_rc, keep.then_some(st));
    result.map_err(|err| interp.pep479_wrap_stop_iteration(err))
}

fn restore(state_rc: &Rc<RefCell<Box<dyn Any>>>, st: Option<Tokenizer>) {
    if let Some(slot) = state_rc
        .borrow_mut()
        .downcast_mut::<Option<Option<Tokenizer>>>()
    {
        *slot = Some(st);
    }
}

fn tokenizer_step(interp: &mut Interpreter, st: &mut Tokenizer) -> Result<Option<Value>> {
    if st.run.is_none() {
        // default_namespace = namespaces.get('') if namespaces else None
        let default_namespace = if truthy(interp, &st.namespaces)? {
            mapping_get_default(interp, &st.namespaces)?
        } else {
            Value::none()
        };
        // xpath_tokenizer_re.findall(pattern)
        let tokens = match str_text(&st.pattern) {
            Some(text) => scan(&text),
            None if is_bytes_like(&st.pattern) => {
                return Err(type_error(
                    "cannot use a string pattern on a bytes-like object",
                ));
            }
            None => {
                return Err(type_error(format!(
                    "expected string or bytes-like object, got '{}'",
                    type_name(&st.pattern)
                )));
            }
        };
        st.run = Some(TokenizerRun {
            default_namespace,
            tokens: tokens.into_iter(),
            parsing_attribute: false,
        });
    }
    let Some(run) = st.run.as_mut() else {
        return Ok(None);
    };
    let Some((op, tag)) = run.tokens.next() else {
        return Ok(None);
    };
    if !tag.is_empty() && !tag.starts_with('{') {
        let token = if let Some((prefix, uri)) = tag.split_once(':') {
            let uri_value = if truthy(interp, &st.namespaces)? {
                mapping_getitem_prefix(interp, &st.namespaces, prefix)?
            } else {
                None
            };
            let Some(uri_value) = uri_value else {
                let shown = render_repr(interp, &Value::string(prefix))?;
                return Err(syntax_error(format!(
                    "prefix {shown} not found in prefix map"
                )));
            };
            let ns = value_str(interp, &uri_value)?;
            pair(&op, &format!("{{{ns}}}{uri}"))
        } else if !run.parsing_attribute && truthy(interp, &run.default_namespace)? {
            let ns = value_str(interp, &run.default_namespace)?;
            pair(&op, &format!("{{{ns}}}{tag}"))
        } else {
            pair(&op, &tag)
        };
        run.parsing_attribute = false;
        return Ok(Some(token));
    }
    run.parsing_attribute = op == "@";
    Ok(Some(pair(&op, &tag)))
}

/// Objects `re` scans through the buffer protocol (`bytes`, `bytearray`,
/// `memoryview`, `array.array`, and `bytes` subclasses).
fn is_bytes_like(value: &Value) -> bool {
    matches!(value.kind(), ValueKind::Bytes(_))
        || matches!(&*type_name(value), "bytearray" | "memoryview" | "array")
        || builtin_data_backing(value)
            .is_some_and(|data| matches!(data.kind(), ValueKind::Bytes(_)))
}

fn pair(op: &str, tag: &str) -> Value {
    Value::tuple(vec![Value::string(op), Value::string(tag)])
}

/// `namespaces.get('')`.
fn mapping_get_default(interp: &mut Interpreter, namespaces: &Value) -> Result<Value> {
    if namespaces.is_dict() {
        return Ok(interp
            .dict_lookup_value(namespaces, &Value::string(""))?
            .map_or_else(Value::none, |(_, value)| value));
    }
    call_method(interp, namespaces, "get", &[Value::string("")])
}

/// `namespaces[prefix]`, with a `KeyError` reported as `None`.
fn mapping_getitem_prefix(
    interp: &mut Interpreter,
    namespaces: &Value,
    prefix: &str,
) -> Result<Option<Value>> {
    let key = Value::string(prefix);
    if namespaces.is_dict() {
        return Ok(interp
            .dict_lookup_value(namespaces, &key)?
            .map(|(_, value)| value));
    }
    match getitem(interp, namespaces, key) {
        Ok(value) => Ok(Some(value)),
        Err(err) if error_is(&err, "KeyError") => Ok(None),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::scan;

    fn ops_or_tags(p: &str) -> Vec<String> {
        scan(p)
            .into_iter()
            .map(|(op, tag)| if op.is_empty() { tag } else { op })
            .collect()
    }

    #[test]
    fn scan_matches_cpython_regex() {
        assert_eq!(ops_or_tags("*"), ["*"]);
        assert_eq!(ops_or_tags("text()"), ["text", "()"]);
        assert_eq!(ops_or_tags("@*"), ["@", "*"]);
        assert_eq!(
            ops_or_tags("para[last()]"),
            ["para", "[", "last", "()", "]"]
        );
        assert_eq!(ops_or_tags("../@lang"), ["..", "/", "@", "lang"]);
        assert_eq!(
            ops_or_tags("a[@b and @c]"),
            ["a", "[", "@", "b", "", "and", "", "@", "c", "]"]
        );
        assert_eq!(
            ops_or_tags(".//{http://spam}egg"),
            [".", "//", "{http://spam}egg"]
        );
        assert_eq!(ops_or_tags("{}*"), ["{}*"]);
        assert_eq!(ops_or_tags("{a}/b"), ["{a}", "/", "b"]);
        assert_eq!(ops_or_tags("a!b"), ["a", "b"]);
        assert_eq!(
            ops_or_tags("[@x='1 2']"),
            ["[", "@", "x", "=", "'1 2'", "]"]
        );
        assert_eq!(ops_or_tags("'open"), ["'open"]);
        assert_eq!(ops_or_tags("a::b"), ["a::b"]);
        assert_eq!(ops_or_tags("::b"), ["::", "b"]);
    }
}
