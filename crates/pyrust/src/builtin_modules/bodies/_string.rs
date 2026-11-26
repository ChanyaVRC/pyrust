// `_string` internal module — 100% native-Rust port of CPython 3.12's PEP 3101
// format-string parser helpers (issues #3250, #3401).
//
// Exposes `formatter_parser(format_string)` and
// `formatter_field_name_split(field_name)`.  Both are faithful ports of the
// `MarkupIterator` / `FieldNameIterator` state machines in CPython's
// `Objects/stringlib/unicode_format.h`: the returned `formatteriterator` and
// `fieldnameiterator` objects are lazy, so a malformed tail raises only when
// iteration reaches it and every diagnostic uses CPython's wording.
//
// The scanners work on byte offsets.  Every character they branch on is ASCII,
// and pyrust strings are (CESU-8 extended) UTF-8, so a non-ASCII byte never
// matches a delimiter and every slice boundary falls on a code point boundary.
//
// `string.Formatter` (in `bodies/string.rs`) reuses these entry points, exactly
// like CPython's `Lib/string.py` calls into `_string`.
//
// Reference: CPython 3.12 `Objects/stringlib/unicode_format.h`

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use pyrust_core::{cesu8_next_codepoint, push_codepoint};
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter, ProviderIterator, value_type_name_str};
use crate::value::{Value, ValueKind};

fn value_error(msg: &str) -> PyError {
    PyError::named("ValueError", msg.to_string())
}

/// The text of an exact `str` or a `str` subclass instance.
pub(crate) fn str_text(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_owned());
    }
    if let ValueKind::PyInstance(inst) = v.kind()
        && let Some(data) = inst.borrow().attrs.builtin_data()
    {
        return data.as_str().map(str::to_owned);
    }
    None
}

/// `PyUnicode_Check` guard shared by both entry points.
fn expect_str(v: &Value) -> Result<String> {
    str_text(v).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!("expected str, got {}", value_type_name_str(v)),
        )
    })
}

/// Read the code point starting at byte `pos` and return it with the offset
/// just past it.
fn read_codepoint(s: &str, pos: usize) -> (u32, usize) {
    cesu8_next_codepoint(s, pos).unwrap_or((u32::from(s.as_bytes()[pos]), pos + 1))
}

fn codepoint_string(cp: u32) -> String {
    let mut out = String::new();
    push_codepoint(&mut out, cp);
    out
}

/// `get_integer`: the value of a run of Unicode decimal digits, `None` when the
/// run is empty or holds a non-decimal code point, and CPython's overflow error
/// once the value cannot fit a `Py_ssize_t`.
fn get_integer(s: &str) -> Result<Option<i64>> {
    if s.is_empty() {
        return Ok(None);
    }
    let mut accumulator: i64 = 0;
    let mut pos = 0;
    while pos < s.len() {
        let (cp, next) = read_codepoint(s, pos);
        pos = next;
        let digit = if cp < 0x80 {
            (cp as u8).is_ascii_digit().then(|| cp - u32::from(b'0'))
        } else {
            crate::builtin_modules::unicodedata::decimal_value(cp)
        };
        let Some(digit) = digit else {
            return Ok(None);
        };
        let digit = i64::from(digit);
        if accumulator > (i64::MAX - digit) / 10 {
            return Err(value_error("Too many decimal digits in format string"));
        }
        accumulator = accumulator * 10 + digit;
    }
    Ok(Some(accumulator))
}

// ── MarkupIterator (formatter_parser) ────────────────────────────────────────

/// One `(literal_text, field_name, format_spec, conversion)` item produced by
/// [`MarkupCursor::next_item`].  Ranges are byte offsets into the source.
pub(crate) struct MarkupItem {
    pub(crate) literal: (usize, usize),
    pub(crate) field: Option<MarkupField>,
}

pub(crate) struct MarkupField {
    pub(crate) name: (usize, usize),
    /// `None` when the field has no format spec (rendered as `''`).
    pub(crate) spec: Option<(usize, usize)>,
    pub(crate) conversion: Option<u32>,
}

/// Cursor over a format string, advancing exactly like CPython's
/// `MarkupIterator`: an error leaves the cursor where the C scanner stops.
pub(crate) struct MarkupCursor {
    pub(crate) text: String,
    pos: usize,
}

impl MarkupCursor {
    pub(crate) fn new(text: String) -> Self {
        MarkupCursor { text, pos: 0 }
    }

    /// `MarkupIterator_next`: `Ok(None)` once the input is exhausted.
    pub(crate) fn next_item(&mut self) -> Result<Option<MarkupItem>> {
        let bytes = self.text.as_bytes();
        let end = bytes.len();
        if self.pos >= end {
            return Ok(None);
        }
        let start = self.pos;
        let mut c = 0u8;
        let mut markup_follows = false;
        // Parse up to the first '{' or '}', or the end of the string.
        while self.pos < end {
            c = bytes[self.pos];
            self.pos += 1;
            if c == b'{' || c == b'}' {
                markup_follows = true;
                break;
            }
        }
        let at_end = self.pos >= end;
        let mut len = self.pos - start;
        if c == b'}' && (at_end || bytes[self.pos] != b'}') {
            return Err(value_error("Single '}' encountered in format string"));
        }
        if at_end && c == b'{' {
            return Err(value_error("Single '{' encountered in format string"));
        }
        if !at_end {
            if bytes[self.pos] == c {
                // An escaped '{{' or '}}': no markup follows this literal.
                self.pos += 1;
                markup_follows = false;
            } else {
                len -= 1;
            }
        }
        let literal = (start, start + len);
        if !markup_follows {
            return Ok(Some(MarkupItem {
                literal,
                field: None,
            }));
        }
        let field = self.parse_field()?;
        Ok(Some(MarkupItem {
            literal,
            field: Some(field),
        }))
    }

    /// `parse_field`: the replacement field after its opening '{'.
    fn parse_field(&mut self) -> Result<MarkupField> {
        let text = self.text.as_str();
        let bytes = text.as_bytes();
        let end = bytes.len();
        let mut c = 0u8;
        // The field name runs to the end of the string, ':', '!' or '}'.
        let name_start = self.pos;
        while self.pos < end {
            c = bytes[self.pos];
            self.pos += 1;
            match c {
                b'{' => return Err(value_error("unexpected '{' in field name")),
                b'[' => {
                    while self.pos < end && bytes[self.pos] != b']' {
                        self.pos += 1;
                    }
                }
                b'}' | b':' | b'!' => break,
                _ => {}
            }
        }
        let name = (name_start, (self.pos - 1).max(name_start));
        if c == b'!' || c == b':' {
            let mut conversion = None;
            if c == b'!' {
                // There must be another character present.
                if self.pos >= end {
                    return Err(value_error(
                        "end of string while looking for conversion specifier",
                    ));
                }
                let (cp, next) = read_codepoint(text, self.pos);
                self.pos = next;
                conversion = Some(cp);
                if self.pos < end {
                    let (follow, next) = read_codepoint(text, self.pos);
                    self.pos = next;
                    if follow == u32::from(b'}') {
                        return Ok(MarkupField {
                            name,
                            spec: None,
                            conversion,
                        });
                    }
                    if follow != u32::from(b':') {
                        return Err(value_error("expected ':' after conversion specifier"));
                    }
                }
            }
            let spec_start = self.pos;
            let mut count = 1usize;
            while self.pos < end {
                let b = bytes[self.pos];
                self.pos += 1;
                if b == b'{' {
                    count += 1;
                } else if b == b'}' {
                    count -= 1;
                    if count == 0 {
                        return Ok(MarkupField {
                            name,
                            spec: Some((spec_start, self.pos - 1)),
                            conversion,
                        });
                    }
                }
            }
            return Err(value_error("unmatched '{' in format spec"));
        }
        if c != b'}' {
            return Err(value_error("expected '}' before end of string"));
        }
        Ok(MarkupField {
            name,
            spec: None,
            conversion: None,
        })
    }

    /// The Python tuple for `item`, as `formatteriter_next` builds it.
    pub(crate) fn item_tuple(&self, item: &MarkupItem) -> Value {
        let slice = |(a, b): (usize, usize)| Value::string(&self.text[a..b]);
        let literal = slice(item.literal);
        match &item.field {
            None => Value::tuple(vec![literal, Value::none(), Value::none(), Value::none()]),
            Some(field) => Value::tuple(vec![
                literal,
                slice(field.name),
                field.spec.map_or_else(|| Value::string(""), slice),
                field
                    .conversion
                    .map_or_else(Value::none, |cp| Value::string(codepoint_string(cp))),
            ]),
        }
    }
}

fn advance_formatter(
    _interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    let mut state = state_rc.borrow_mut();
    let cursor = state
        .downcast_mut::<MarkupCursor>()
        .ok_or_else(|| PyError::runtime("internal: formatteriterator state corrupted".into()))?;
    match cursor.next_item()? {
        Some(item) => Ok(Some(cursor.item_tuple(&item))),
        None => Ok(None),
    }
}

/// `_string.formatter_parser(format_string)`: a lazy `formatteriterator`.
pub(crate) fn formatter_parser_value(format_string: &Value) -> Result<Value> {
    let text = expect_str(format_string)?;
    Ok(Value::generator(Box::new(ProviderIterator::new(
        Box::new(MarkupCursor::new(text)),
        advance_formatter,
        "formatteriterator",
        "formatteriterator",
        None,
    ))))
}

// ── FieldNameIterator (formatter_field_name_split) ───────────────────────────

/// One `(is_attr, key)` step of a field name's `.attr` / `[key]` tail.
pub(crate) enum FieldStep {
    Attr(String),
    Item(Value),
}

/// Cursor over the tail of a field name, advancing exactly like CPython's
/// `FieldNameIterator`.
pub(crate) struct FieldNameCursor {
    text: String,
    pos: usize,
}

impl FieldNameCursor {
    /// `FieldNameIterator_next`: `Ok(None)` once the tail is exhausted.
    pub(crate) fn next_step(&mut self) -> Result<Option<FieldStep>> {
        let bytes = self.text.as_bytes();
        let end = bytes.len();
        if self.pos >= end {
            return Ok(None);
        }
        let (c, next) = read_codepoint(&self.text, self.pos);
        self.pos = next;
        let step = if c == u32::from(b'.') {
            let start = self.pos;
            while self.pos < end && bytes[self.pos] != b'[' && bytes[self.pos] != b'.' {
                self.pos += 1;
            }
            if start == self.pos {
                return Err(value_error("Empty attribute in format string"));
            }
            FieldStep::Attr(self.text[start..self.pos].to_string())
        } else if c == u32::from(b'[') {
            let start = self.pos;
            let mut bracket_seen = false;
            while self.pos < end {
                let b = bytes[self.pos];
                self.pos += 1;
                if b == b']' {
                    bracket_seen = true;
                    break;
                }
            }
            if !bracket_seen {
                return Err(value_error("Missing ']' in format string"));
            }
            let key = &self.text[start..self.pos - 1];
            let index = get_integer(key)?;
            if key.is_empty() {
                return Err(value_error("Empty attribute in format string"));
            }
            FieldStep::Item(index.map_or_else(|| Value::string(key), Value::int))
        } else {
            return Err(value_error(
                "Only '.' or '[' may follow ']' in format field specifier",
            ));
        };
        Ok(Some(step))
    }
}

/// `field_name_split`: the leading argument reference (an `int` when it is a
/// run of decimal digits, else a `str`) and a cursor over the rest.
pub(crate) fn split_field_name(text: String) -> Result<(Value, FieldNameCursor)> {
    let bytes = text.as_bytes();
    let first_end = bytes
        .iter()
        .position(|&b| b == b'.' || b == b'[')
        .unwrap_or(bytes.len());
    let first_text = &text[..first_end];
    let first = match get_integer(first_text)? {
        Some(index) => Value::int(index),
        None => Value::string(first_text),
    };
    Ok((
        first,
        FieldNameCursor {
            text,
            pos: first_end,
        },
    ))
}

/// [`split_field_name`] behind the `formatter_field_name_split` type check.
pub(crate) fn split_field_name_value(field_name: &Value) -> Result<(Value, FieldNameCursor)> {
    split_field_name(expect_str(field_name)?)
}

fn advance_field_name(
    _interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    let mut state = state_rc.borrow_mut();
    let cursor = state
        .downcast_mut::<FieldNameCursor>()
        .ok_or_else(|| PyError::runtime("internal: fieldnameiterator state corrupted".into()))?;
    Ok(cursor.next_step()?.map(|step| match step {
        FieldStep::Attr(name) => Value::tuple(vec![Value::bool_(true), Value::string(name)]),
        FieldStep::Item(key) => Value::tuple(vec![Value::bool_(false), key]),
    }))
}

/// `_string.formatter_field_name_split(field_name)`: `(first, rest)` with a
/// lazy `fieldnameiterator` as `rest`.
pub(crate) fn field_name_split_value(field_name: &Value) -> Result<Value> {
    let (first, cursor) = split_field_name_value(field_name)?;
    let rest = Value::generator(Box::new(ProviderIterator::new(
        Box::new(cursor),
        advance_field_name,
        "fieldnameiterator",
        "fieldnameiterator",
        None,
    )));
    Ok(Value::tuple(vec![first, rest]))
}

pyrust_module! {
    constants {
        "__doc__" => Value::string("string helper module"),
    }

    // Declared in CPython's `_string_methods` table order.
    fn formatter_field_name_split(#[positional_only] field_name: PyValue) -> Result<Value> {
        let _ = &_interp;
        field_name_split_value(&field_name.0)
    }

    fn formatter_parser(#[positional_only] format_string: PyValue) -> Result<Value> {
        let _ = &_interp;
        formatter_parser_value(&format_string.0)
    }
}
