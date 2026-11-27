//! Native JSON encoding.
//!
//! `JSONEncoder.iterencode` picks one of two CPython 3.12 strategies:
//!
//! * `_json.c`'s `c_make_encoder` for the one-shot, `indent is None` call
//!   `JSONEncoder.encode` makes — [`encode_one_shot`] writes the whole
//!   document into one buffer and returns it as a 1-tuple.
//! * `json.encoder._make_iterencode` everywhere else — [`iterencode_lazy`]
//!   returns a lazy generator yielding exactly the chunks the pure-Python
//!   generator yields, in the same order and with the same side effects
//!   (`default()` calls, errors) at the same `next()` step, so `json.dump`
//!   writes the same pieces to its file.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use pyrust_core::cesu8_next_codepoint;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, ProviderIterator, builtin_data_backing, key_ref_to_value};
use crate::value::{Value, ValueKind};

use super::support::{self, C_RECURSION_LIMIT};

// ── String escaping ──────────────────────────────────────────────────────────

const HEX: &[u8; 16] = b"0123456789abcdef";

fn push_u_escape(out: &mut String, cp: u32) {
    out.push_str("\\u");
    for shift in [12, 8, 4, 0] {
        out.push(HEX[((cp >> shift) & 0xF) as usize] as char);
    }
}

/// Append the JSON string literal for `s`: `_json.c`'s `escape_unicode`
/// (`encode_basestring`) or, with `ensure_ascii`, `ascii_escape_unicode`
/// (`encode_basestring_ascii`).  Lone surrogates (CESU-8) are escaped like
/// any other non-ASCII code point, or copied through unchanged.
pub(crate) fn escape_into(out: &mut String, s: &str, ensure_ascii: bool) {
    out.reserve(s.len() + 2);
    out.push('"');
    let b = s.as_bytes();
    let mut run = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c >= 0x80 {
            if !ensure_ascii {
                i += 1;
                continue;
            }
            out.push_str(&s[run..i]);
            let (cp, next) = cesu8_next_codepoint(s, i).unwrap_or((0xFFFD, b.len()));
            if cp >= 0x10000 {
                let v = cp - 0x10000;
                push_u_escape(out, 0xD800 | (v >> 10));
                push_u_escape(out, 0xDC00 | (v & 0x3FF));
            } else {
                push_u_escape(out, cp);
            }
            i = next;
            run = i;
            continue;
        }
        let short = match c {
            b'"' => Some("\\\""),
            b'\\' => Some("\\\\"),
            b'\n' => Some("\\n"),
            b'\r' => Some("\\r"),
            b'\t' => Some("\\t"),
            0x08 => Some("\\b"),
            0x0c => Some("\\f"),
            _ => None,
        };
        if short.is_none() && c >= 0x20 && !(ensure_ascii && c == 0x7f) {
            i += 1;
            continue;
        }
        out.push_str(&s[run..i]);
        match short {
            Some(esc) => out.push_str(esc),
            None => push_u_escape(out, u32::from(c)),
        }
        i += 1;
        run = i;
    }
    out.push_str(&s[run..]);
    out.push('"');
}

/// `encode_basestring(s)` / `encode_basestring_ascii(s)`.
pub(crate) fn escape(s: &str, ensure_ascii: bool) -> String {
    let mut out = String::new();
    escape_into(&mut out, s, ensure_ascii);
    out
}

// ── Value classification ─────────────────────────────────────────────────────

/// What the encoders see in a value: CPython's `PyUnicode_Check` /
/// `PyLong_Check` / ... tests, which accept subclasses and then work on the
/// builtin storage.
enum Class {
    Null,
    Bool(bool),
    Str(Value),
    Int(Value),
    Float(f64),
    /// A `list` / `tuple` (or subclass) with its builtin storage.
    Seq(Value),
    /// A `dict` (or subclass) with its builtin storage; `exact` for `dict`.
    Dict {
        storage: Value,
        exact: bool,
    },
    Other,
}

fn classify(value: &Value) -> Class {
    match value.kind() {
        ValueKind::None => Class::Null,
        ValueKind::Bool(b) => Class::Bool(b),
        ValueKind::Int(_) | ValueKind::BigInt(_) => Class::Int(value.clone()),
        ValueKind::Float(f) => Class::Float(f),
        ValueKind::Str(_) => Class::Str(value.clone()),
        ValueKind::List(_) | ValueKind::Tuple(_) => Class::Seq(value.clone()),
        ValueKind::Dict(_) => Class::Dict {
            storage: value.clone(),
            exact: true,
        },
        ValueKind::PyInstance(_) => {
            let Some(backing) = builtin_data_backing(value) else {
                return Class::Other;
            };
            match backing.kind() {
                ValueKind::Str(_) => Class::Str(backing.clone()),
                ValueKind::Int(_) | ValueKind::BigInt(_) => Class::Int(backing.clone()),
                ValueKind::Float(f) => Class::Float(f),
                ValueKind::List(_) | ValueKind::Tuple(_) => Class::Seq(backing.clone()),
                ValueKind::Dict(_) => Class::Dict {
                    storage: backing.clone(),
                    exact: false,
                },
                _ => Class::Other,
            }
        }
        _ => Class::Other,
    }
}

/// `int.__repr__(value)` for an exact `int`.
fn int_repr(value: &Value) -> Result<String> {
    match value.kind() {
        ValueKind::Int(n) => Ok(n.to_string()),
        ValueKind::BigInt(n) => {
            pyrust_core::check_int_str_conversion(value)?;
            Ok(n.to_string())
        }
        _ => Ok(value.repr_raw()),
    }
}

/// `floatstr` / `encoder_encode_float`.
fn float_str(
    interp: &mut Interpreter,
    f: f64,
    original: &Value,
    allow_nan: bool,
) -> Result<String> {
    if f.is_finite() {
        return Ok(Value::float(f).repr_raw());
    }
    if !allow_nan {
        let shown = match original.kind() {
            ValueKind::Float(_) => Value::float(f).repr_raw(),
            _ => crate::interpreter::render_value_repr(interp, original)?,
        };
        return Err(PyError::named(
            "ValueError",
            format!("Out of range float values are not JSON compliant: {shown}"),
        ));
    }
    Ok(if f > 0.0 {
        "Infinity"
    } else if f < 0.0 {
        "-Infinity"
    } else {
        "NaN"
    }
    .to_string())
}

/// `id(value)` as a marker key.
fn marker_id(value: &Value) -> i64 {
    value.object_id().as_int().unwrap_or(0)
}

/// Item `i` of a `list` / `tuple` storage, re-read on every step like
/// `PySequence_Fast_GET_ITEM` / `for value in lst`.
fn seq_item(seq: &Value, i: usize) -> Option<Value> {
    match seq.kind() {
        ValueKind::List(items) => items.get(i).cloned(),
        ValueKind::Tuple(items) => items.get(i).cloned(),
        _ => None,
    }
}

fn seq_is_empty(seq: &Value) -> bool {
    match seq.kind() {
        ValueKind::List(items) => items.is_empty(),
        ValueKind::Tuple(items) => items.is_empty(),
        _ => true,
    }
}

fn dict_is_empty(storage: &Value) -> bool {
    match storage.kind() {
        ValueKind::Dict(d) => d.is_empty(),
        _ => true,
    }
}

/// A snapshot of a dict's `(key, value)` pairs from its storage.
fn dict_pairs(storage: &Value) -> Vec<(Value, Value)> {
    match storage.kind() {
        ValueKind::Dict(d) => d
            .iter()
            .map(|(k, v)| (key_ref_to_value(k), v.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// `dct.items()` as a list of pairs, optionally `sorted()`.  An exact `dict`
/// is read from its storage; a subclass goes through its `items()` method.
fn dict_items(
    interp: &mut Interpreter,
    dict: &Value,
    storage: &Value,
    exact: bool,
    sort: bool,
) -> Result<Vec<Value>> {
    let mut items: Vec<Value> = if exact {
        dict_pairs(storage)
            .into_iter()
            .map(|(k, v)| Value::tuple(vec![k, v]))
            .collect()
    } else {
        let view = support::call_method(interp, dict, "items", &[])?;
        support::collect(interp, &view)?
    };
    if sort {
        let sorted = support::builtin(interp, "sorted")?;
        let result = support::call(interp, sorted, &[Value::list(items)])?;
        items = support::collect(interp, &result)?;
    }
    Ok(items)
}

/// The encoder's view of a dict key (`encoder_encode_key_value` /
/// `_iterencode_dict`): its JSON text, or `None` to skip it.
fn key_text(
    interp: &mut Interpreter,
    key: &Value,
    allow_nan: bool,
    skipkeys: bool,
    type_name: impl FnOnce(&mut Interpreter) -> Result<String>,
) -> Result<Option<String>> {
    Ok(Some(match classify(key) {
        Class::Str(s) => s.as_str().unwrap_or_default().to_string(),
        Class::Float(f) => float_str(interp, f, key, allow_nan)?,
        Class::Bool(true) => "true".to_string(),
        Class::Bool(false) => "false".to_string(),
        Class::Null => "null".to_string(),
        Class::Int(v) => int_repr(&v)?,
        _ if skipkeys => return Ok(None),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "keys must be str, int, float, bool or None, not {}",
                    type_name(interp)?
                ),
            ));
        }
    }))
}

fn circular() -> PyError {
    PyError::named("ValueError", "Circular reference detected".to_string())
}

// ── `c_make_encoder` ─────────────────────────────────────────────────────────

/// The settings `JSONEncoder.iterencode` reads from the encoder.
pub(crate) struct Options {
    pub(crate) check_circular: bool,
    pub(crate) default_fn: Value,
    pub(crate) ensure_ascii: bool,
    pub(crate) allow_nan: bool,
    pub(crate) sort_keys: bool,
    pub(crate) skipkeys: bool,
}

/// `c_make_encoder(...)(o, 0)`: the whole document as one string.
pub(crate) fn encode_one_shot(
    interp: &mut Interpreter,
    opts: &Options,
    key_separator: &str,
    item_separator: &str,
    o: &Value,
) -> Result<String> {
    let mut encoder = OneShot {
        interp,
        opts,
        key_separator,
        item_separator,
        markers: opts.check_circular.then(HashSet::new),
        depth: 0,
    };
    let mut out = String::new();
    encoder.encode(&mut out, o)?;
    Ok(out)
}

/// Nested containers `_json.c` accepts before `_Py_EnterRecursiveCall`
/// fails (a few C frames are already in use when encoding starts).
const MAX_NESTING: usize = C_RECURSION_LIMIT - 3;

struct OneShot<'a> {
    interp: &'a mut Interpreter,
    opts: &'a Options,
    key_separator: &'a str,
    item_separator: &'a str,
    markers: Option<HashSet<i64>>,
    depth: usize,
}

impl OneShot<'_> {
    fn enter_marker(&mut self, obj: &Value) -> Result<Option<i64>> {
        let Some(markers) = &mut self.markers else {
            return Ok(None);
        };
        let id = marker_id(obj);
        if !markers.insert(id) {
            return Err(circular());
        }
        Ok(Some(id))
    }

    fn leave_marker(&mut self, id: Option<i64>) {
        if let (Some(markers), Some(id)) = (&mut self.markers, id) {
            markers.remove(&id);
        }
    }

    /// `_Py_EnterRecursiveCall(" while encoding a JSON object")` around `f`.
    fn recurse(&mut self, f: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        if self.depth >= MAX_NESTING {
            return Err(support::recursion_error(
                "maximum recursion depth exceeded while encoding a JSON object",
            ));
        }
        self.depth += 1;
        let result = support::with_stack(|| f(self));
        self.depth -= 1;
        result
    }

    /// `encoder_listencode_obj`.
    fn encode(&mut self, out: &mut String, obj: &Value) -> Result<()> {
        match classify(obj) {
            Class::Null => out.push_str("null"),
            Class::Bool(true) => out.push_str("true"),
            Class::Bool(false) => out.push_str("false"),
            Class::Str(s) => {
                escape_into(out, s.as_str().unwrap_or_default(), self.opts.ensure_ascii)
            }
            Class::Int(v) => out.push_str(&int_repr(&v)?),
            Class::Float(f) => {
                let text = float_str(self.interp, f, obj, self.opts.allow_nan)?;
                out.push_str(&text);
            }
            Class::Seq(seq) => self.recurse(|this| this.encode_list(out, obj, &seq))?,
            Class::Dict { storage, exact } => {
                self.recurse(|this| this.encode_dict(out, obj, &storage, exact))?;
            }
            Class::Other => {
                let id = self.enter_marker(obj)?;
                let default_fn = self.opts.default_fn.clone();
                let newobj = support::call(self.interp, default_fn, std::slice::from_ref(obj))?;
                self.recurse(|this| this.encode(out, &newobj))?;
                self.leave_marker(id);
            }
        }
        Ok(())
    }

    /// `encoder_listencode_list`.
    fn encode_list(&mut self, out: &mut String, obj: &Value, seq: &Value) -> Result<()> {
        if seq_is_empty(seq) {
            out.push_str("[]");
            return Ok(());
        }
        let id = self.enter_marker(obj)?;
        out.push('[');
        let mut i = 0;
        while let Some(item) = seq_item(seq, i) {
            if i > 0 {
                out.push_str(self.item_separator);
            }
            self.encode(out, &item)?;
            i += 1;
        }
        out.push(']');
        self.leave_marker(id);
        Ok(())
    }

    /// `encoder_listencode_dict`.
    fn encode_dict(
        &mut self,
        out: &mut String,
        obj: &Value,
        storage: &Value,
        exact: bool,
    ) -> Result<()> {
        if dict_is_empty(storage) {
            out.push_str("{}");
            return Ok(());
        }
        let id = self.enter_marker(obj)?;
        out.push('{');
        let mut first = true;
        if self.opts.sort_keys || !exact {
            let items = dict_items(self.interp, obj, storage, exact, self.opts.sort_keys)?;
            for item in items {
                let pair = match item.kind() {
                    ValueKind::Tuple(pair) if pair.len() == 2 => (pair[0].clone(), pair[1].clone()),
                    _ => {
                        return Err(PyError::named(
                            "ValueError",
                            "items must return 2-tuples".to_string(),
                        ));
                    }
                };
                self.encode_key_value(out, &mut first, &pair.0, &pair.1)?;
            }
        } else {
            for (key, value) in dict_pairs(storage) {
                self.encode_key_value(out, &mut first, &key, &value)?;
            }
        }
        out.push('}');
        self.leave_marker(id);
        Ok(())
    }

    /// `encoder_encode_key_value`.
    fn encode_key_value(
        &mut self,
        out: &mut String,
        first: &mut bool,
        key: &Value,
        value: &Value,
    ) -> Result<()> {
        let Some(text) = key_text(
            self.interp,
            key,
            self.opts.allow_nan,
            self.opts.skipkeys,
            |_| Ok(support::type_name(key)),
        )?
        else {
            return Ok(());
        };
        if *first {
            *first = false;
        } else {
            out.push_str(self.item_separator);
        }
        escape_into(out, &text, self.opts.ensure_ascii);
        out.push_str(self.key_separator);
        self.encode(out, value)
    }
}

// ── `JSONEncoder.iterencode` ─────────────────────────────────────────────────

fn truthy_attr(interp: &mut Interpreter, this: &Value, name: &str) -> Result<bool> {
    let value = interp.get_attr(this, name)?;
    interp.truthy_value(&value)
}

/// `make_encoder()`'s `U` conversion of a separator argument.
fn separator_text(value: &Value, position: usize) -> Result<String> {
    support::str_text(value)
        .map(std::borrow::Cow::into_owned)
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "make_encoder() argument {position} must be str, not {}",
                    support::type_name(value)
                ),
            )
        })
}

/// `JSONEncoder.iterencode(self, o, _one_shot)`.
pub(crate) fn iterencode(
    interp: &mut Interpreter,
    this: &Value,
    o: Value,
    one_shot: bool,
) -> Result<Value> {
    let check_circular = truthy_attr(interp, this, "check_circular")?;
    let ensure_ascii = truthy_attr(interp, this, "ensure_ascii")?;
    let allow_nan = interp.get_attr(this, "allow_nan")?;
    let indent = interp.get_attr(this, "indent")?;
    let default_fn = interp.get_attr(this, "default")?;
    let key_separator = interp.get_attr(this, "key_separator")?;
    let item_separator = interp.get_attr(this, "item_separator")?;
    let sort_keys = interp.get_attr(this, "sort_keys")?;
    let skipkeys = interp.get_attr(this, "skipkeys")?;
    if one_shot && indent.is_none() {
        let key_text = separator_text(&key_separator, 5)?;
        let item_text = separator_text(&item_separator, 6)?;
        let opts = Options {
            check_circular,
            default_fn,
            ensure_ascii,
            sort_keys: interp.truthy_value(&sort_keys)?,
            skipkeys: interp.truthy_value(&skipkeys)?,
            allow_nan: interp.truthy_value(&allow_nan)?,
        };
        let text = encode_one_shot(interp, &opts, &key_text, &item_text, &o)?;
        return Ok(Value::tuple(vec![Value::string(text)]));
    }
    let opts = Options {
        check_circular,
        default_fn,
        ensure_ascii,
        allow_nan: interp.truthy_value(&allow_nan)?,
        sort_keys: interp.truthy_value(&sort_keys)?,
        skipkeys: interp.truthy_value(&skipkeys)?,
    };
    // `if _indent is not None and not isinstance(_indent, str): _indent = ' ' * _indent`
    let indent = if indent.is_none() {
        None
    } else if let Some(text) = support::str_text(&indent) {
        Some(text.into_owned())
    } else {
        let spaces = interp.eval_binary(Value::string(" "), BinaryOp::Mul, indent)?;
        Some(
            support::str_text(&spaces)
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default(),
        )
    };
    Ok(iterencode_lazy(
        opts,
        indent,
        key_separator,
        item_separator,
        o,
    ))
}

// ── `_make_iterencode` ───────────────────────────────────────────────────────

/// Where a `_iterencode_list` generator resumes.
enum ListStep {
    Start,
    Next,
    /// `yield buf` was the last chunk; resume into `yield from chunks`.
    Child(Value),
    Close,
    CloseBracket,
}

struct ListFrame {
    obj: Value,
    seq: Value,
    level: usize,
    index: usize,
    newline_indent: Option<String>,
    separator: Value,
    marker: Option<i64>,
    step: ListStep,
}

/// Where a `_iterencode_dict` generator resumes.
enum DictStep {
    Start,
    Open,
    Items,
    Next,
    Key(String, Value),
    KeySeparator(Value),
    Value(Value),
    Close,
    CloseBrace,
}

struct DictFrame {
    obj: Value,
    storage: Value,
    exact: bool,
    level: usize,
    newline_indent: Option<String>,
    item_separator: Value,
    marker: Option<i64>,
    items: Vec<Value>,
    index: usize,
    first: bool,
    step: DictStep,
}

enum Frame {
    /// `_iterencode(value, level)` before its first step.
    Value(Value, usize),
    List(ListFrame),
    Dict(DictFrame),
    /// `_iterencode`'s `default()` branch, waiting on `yield from`.
    Default(Option<i64>),
}

/// The state of one `_make_iterencode(...)(o, 0)` generator.
struct LazyEncoder {
    opts: Options,
    indent: Option<String>,
    key_separator: Value,
    item_separator: Value,
    markers: Option<HashSet<i64>>,
    stack: Vec<Frame>,
    /// Generator frames allowed by `sys.getrecursionlimit()` in this step.
    frame_budget: usize,
}

enum Slot {
    Idle(Box<LazyEncoder>),
    Running,
    Done,
}

/// `_make_iterencode(...)(o, 0)` for an encoder whose settings were read by
/// `JSONEncoder.iterencode`.
pub(crate) fn iterencode_lazy(
    opts: Options,
    indent: Option<String>,
    key_separator: Value,
    item_separator: Value,
    o: Value,
) -> Value {
    let markers = opts.check_circular.then(HashSet::new);
    let encoder = LazyEncoder {
        opts,
        indent,
        key_separator,
        item_separator,
        markers,
        stack: vec![Frame::Value(o, 0)],
        frame_budget: 0,
    };
    Value::generator(Box::new(ProviderIterator::new(
        Box::new(RefCell::new(Slot::Idle(Box::new(encoder)))),
        advance_lazy,
        "generator",
        "generator",
        None,
    )))
}

fn advance_lazy(
    interp: &mut Interpreter,
    state: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    let taken = {
        let state = state.borrow();
        let slot = state
            .downcast_ref::<RefCell<Slot>>()
            .ok_or_else(|| PyError::runtime("internal: json encoder state corrupted".into()))?;
        std::mem::replace(&mut *slot.borrow_mut(), Slot::Running)
    };
    let mut encoder = match taken {
        Slot::Idle(encoder) => encoder,
        Slot::Running => {
            return Err(PyError::named(
                "ValueError",
                "generator already executing".to_string(),
            ));
        }
        Slot::Done => {
            restore(state, Slot::Done);
            return Ok(None);
        }
    };
    let result = encoder.next_chunk(interp);
    let next = match &result {
        Ok(Some(_)) => Slot::Idle(encoder),
        _ => Slot::Done,
    };
    restore(state, next);
    result
}

fn restore(state: &Rc<RefCell<Box<dyn Any>>>, slot: Slot) {
    if let Some(cell) = state.borrow().downcast_ref::<RefCell<Slot>>() {
        *cell.borrow_mut() = slot;
    }
}

/// `buf + text` (a `str` fast path; anything else goes through `+`).
fn concat(interp: &mut Interpreter, buf: &Value, text: &str) -> Result<Value> {
    if let Some(head) = buf.as_str() {
        let mut out = String::with_capacity(head.len() + text.len());
        out.push_str(head);
        out.push_str(text);
        return Ok(Value::string(out));
    }
    interp.eval_binary(buf.clone(), BinaryOp::Add, Value::string(text))
}

/// The JSON text of a scalar, or `None` for a container / other object.
fn scalar_text(
    interp: &mut Interpreter,
    value: &Value,
    class: &Class,
    opts: &Options,
) -> Result<Option<String>> {
    Ok(Some(match class {
        Class::Str(s) => escape(s.as_str().unwrap_or_default(), opts.ensure_ascii),
        Class::Null => "null".to_string(),
        Class::Bool(true) => "true".to_string(),
        Class::Bool(false) => "false".to_string(),
        Class::Int(v) => int_repr(v)?,
        Class::Float(f) => float_str(interp, *f, value, opts.allow_nan)?,
        _ => return Ok(None),
    }))
}

impl LazyEncoder {
    fn newline_indent(&self, level: usize) -> Option<String> {
        self.indent.as_ref().map(|indent| {
            let mut s = String::with_capacity(1 + indent.len() * level);
            s.push('\n');
            for _ in 0..level {
                s.push_str(indent);
            }
            s
        })
    }

    fn enter_marker(&mut self, obj: &Value) -> Result<Option<i64>> {
        let Some(markers) = &mut self.markers else {
            return Ok(None);
        };
        let id = marker_id(obj);
        if !markers.insert(id) {
            return Err(circular());
        }
        Ok(Some(id))
    }

    fn leave_marker(&mut self, id: Option<i64>) {
        if let (Some(markers), Some(id)) = (&mut self.markers, id) {
            markers.remove(&id);
        }
    }

    /// Push a nested generator frame, failing like CPython once the chain of
    /// generator frames reaches the interpreter's recursion limit.
    fn push(&mut self, frame: Frame) -> Result<()> {
        if self.stack.len() >= self.frame_budget {
            return Err(support::recursion_error("maximum recursion depth exceeded"));
        }
        self.stack.push(frame);
        Ok(())
    }

    /// Run the generator to its next `yield`.
    fn next_chunk(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        // Each nested `_iterencode*` generator is one more Python frame on
        // top of the consumer's call chain.
        self.frame_budget = crate::interpreter::get_recursion_limit(interp)
            .saturating_sub(crate::interpreter::get_call_depth() + 3);
        loop {
            let Some(frame) = self.stack.pop() else {
                return Ok(None);
            };
            match frame {
                Frame::Value(value, level) => {
                    if let Some(chunk) = self.start_value(interp, value, level)? {
                        return Ok(Some(chunk));
                    }
                }
                Frame::Default(marker) => self.leave_marker(marker),
                Frame::List(frame) => {
                    if let Some(chunk) = self.step_list(interp, frame)? {
                        return Ok(Some(chunk));
                    }
                }
                Frame::Dict(frame) => {
                    if let Some(chunk) = self.step_dict(interp, frame)? {
                        return Ok(Some(chunk));
                    }
                }
            }
        }
    }

    /// `_iterencode(o, level)` up to its first `yield`.
    fn start_value(
        &mut self,
        interp: &mut Interpreter,
        value: Value,
        level: usize,
    ) -> Result<Option<Value>> {
        let class = classify(&value);
        if let Some(text) = scalar_text(interp, &value, &class, &self.opts)? {
            return Ok(Some(Value::string(text)));
        }
        match class {
            Class::Seq(seq) => {
                let frame = ListFrame {
                    obj: value,
                    seq,
                    level,
                    index: 0,
                    newline_indent: None,
                    separator: Value::none(),
                    marker: None,
                    step: ListStep::Start,
                };
                self.push(Frame::List(frame))?;
            }
            Class::Dict { storage, exact } => {
                let frame = DictFrame {
                    obj: value,
                    storage,
                    exact,
                    level,
                    newline_indent: None,
                    item_separator: Value::none(),
                    marker: None,
                    items: Vec::new(),
                    index: 0,
                    first: true,
                    step: DictStep::Start,
                };
                self.push(Frame::Dict(frame))?;
            }
            _ => {
                let marker = self.enter_marker(&value)?;
                let default_fn = self.opts.default_fn.clone();
                let newobj = support::call(interp, default_fn, &[value])?;
                self.push(Frame::Default(marker))?;
                self.stack.push(Frame::Value(newobj, level));
            }
        }
        Ok(None)
    }

    /// One resumption of `_iterencode_list`; `None` when it handed control
    /// to a child frame or finished without yielding.
    fn step_list(&mut self, interp: &mut Interpreter, mut f: ListFrame) -> Result<Option<Value>> {
        loop {
            match std::mem::replace(&mut f.step, ListStep::Next) {
                ListStep::Start => {
                    if seq_is_empty(&f.seq) {
                        return Ok(Some(Value::string("[]")));
                    }
                    f.marker = self.enter_marker(&f.obj)?;
                    if self.indent.is_some() {
                        f.level += 1;
                    }
                    f.newline_indent = self.newline_indent(f.level);
                    f.separator = match &f.newline_indent {
                        Some(nl) => concat(interp, &self.item_separator, nl)?,
                        None => self.item_separator.clone(),
                    };
                }
                ListStep::Next => {
                    let Some(value) = seq_item(&f.seq, f.index) else {
                        f.step = ListStep::Close;
                        continue;
                    };
                    let buf = if f.index == 0 {
                        let mut open = String::from("[");
                        if let Some(nl) = &f.newline_indent {
                            open.push_str(nl);
                        }
                        Value::string(open)
                    } else {
                        f.separator.clone()
                    };
                    f.index += 1;
                    let class = classify(&value);
                    if let Some(text) = scalar_text(interp, &value, &class, &self.opts)? {
                        let chunk = concat(interp, &buf, &text)?;
                        self.stack.push(Frame::List(f));
                        return Ok(Some(chunk));
                    }
                    f.step = ListStep::Child(value);
                    self.stack.push(Frame::List(f));
                    return Ok(Some(buf));
                }
                ListStep::Child(value) => {
                    let level = f.level;
                    self.stack.push(Frame::List(f));
                    self.stack.push(Frame::Value(value, level));
                    return Ok(None);
                }
                ListStep::Close => {
                    f.step = ListStep::CloseBracket;
                    if f.newline_indent.is_some() {
                        let closing = self.newline_indent(f.level - 1).unwrap_or_default();
                        self.stack.push(Frame::List(f));
                        return Ok(Some(Value::string(closing)));
                    }
                }
                ListStep::CloseBracket => {
                    self.leave_marker(f.marker);
                    return Ok(Some(Value::string("]")));
                }
            }
        }
    }

    /// One resumption of `_iterencode_dict`.
    fn step_dict(&mut self, interp: &mut Interpreter, mut f: DictFrame) -> Result<Option<Value>> {
        loop {
            match std::mem::replace(&mut f.step, DictStep::Next) {
                DictStep::Start => {
                    if dict_is_empty(&f.storage) {
                        return Ok(Some(Value::string("{}")));
                    }
                    f.marker = self.enter_marker(&f.obj)?;
                    f.step = DictStep::Open;
                    self.stack.push(Frame::Dict(f));
                    return Ok(Some(Value::string("{")));
                }
                DictStep::Open => {
                    f.step = DictStep::Items;
                    if self.indent.is_some() {
                        f.level += 1;
                        f.newline_indent = self.newline_indent(f.level);
                        let nl = f.newline_indent.clone().unwrap_or_default();
                        f.item_separator = concat(interp, &self.item_separator, &nl)?;
                        self.stack.push(Frame::Dict(f));
                        return Ok(Some(Value::string(nl)));
                    }
                    f.item_separator = self.item_separator.clone();
                }
                DictStep::Items => {
                    f.items = dict_items(interp, &f.obj, &f.storage, f.exact, self.opts.sort_keys)?;
                }
                DictStep::Next => {
                    let Some(item) = f.items.get(f.index).cloned() else {
                        f.step = DictStep::Close;
                        continue;
                    };
                    f.index += 1;
                    let (key, value) = support::unpack2(interp, &item)?;
                    let allow_nan = self.opts.allow_nan;
                    let skipkeys = self.opts.skipkeys;
                    let Some(text) = key_text(interp, &key, allow_nan, skipkeys, |interp| {
                        support::class_name(interp, &key)
                    })?
                    else {
                        continue;
                    };
                    let encoded = escape(&text, self.opts.ensure_ascii);
                    if f.first {
                        f.first = false;
                        f.step = DictStep::KeySeparator(value);
                        self.stack.push(Frame::Dict(f));
                        return Ok(Some(Value::string(encoded)));
                    }
                    f.step = DictStep::Key(encoded, value);
                    let sep = f.item_separator.clone();
                    self.stack.push(Frame::Dict(f));
                    return Ok(Some(sep));
                }
                DictStep::Key(encoded, value) => {
                    f.step = DictStep::KeySeparator(value);
                    self.stack.push(Frame::Dict(f));
                    return Ok(Some(Value::string(encoded)));
                }
                DictStep::KeySeparator(value) => {
                    f.step = DictStep::Value(value);
                    let sep = self.key_separator.clone();
                    self.stack.push(Frame::Dict(f));
                    return Ok(Some(sep));
                }
                DictStep::Value(value) => {
                    let class = classify(&value);
                    if let Some(text) = scalar_text(interp, &value, &class, &self.opts)? {
                        self.stack.push(Frame::Dict(f));
                        return Ok(Some(Value::string(text)));
                    }
                    let level = f.level;
                    self.stack.push(Frame::Dict(f));
                    self.stack.push(Frame::Value(value, level));
                    return Ok(None);
                }
                DictStep::Close => {
                    f.step = DictStep::CloseBrace;
                    if f.newline_indent.is_some() {
                        let closing = self.newline_indent(f.level - 1).unwrap_or_default();
                        self.stack.push(Frame::Dict(f));
                        return Ok(Some(Value::string(closing)));
                    }
                }
                DictStep::CloseBrace => {
                    self.leave_marker(f.marker);
                    return Ok(Some(Value::string("}")));
                }
            }
        }
    }
}
