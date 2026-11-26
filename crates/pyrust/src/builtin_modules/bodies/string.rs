// `string` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/string.py` (issues #2515, #3401).
//
// Included into `pub mod string { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs`.
//
// * The character-class constants, `__all__`, and `capwords` are plain native
//   members.
// * `Template` keeps CPython's class protocol: `delimiter`, `idpattern`,
//   `braceidpattern`, and `flags` are class attributes, and the native
//   `__init_subclass__` compiles `pattern` with `re` for the base class and for
//   every subclass (honouring a `pattern` defined in the subclass body).
//   `substitute` / `safe_substitute` / `is_valid` / `get_identifiers` go
//   through `self.pattern.sub(...)` / `.finditer(...)` like CPython, except
//   when `self.pattern` is a pattern this module compiled from the default
//   identifier syntax: those are scanned natively with identical results.
// * `Formatter` dispatches every hook (`vformat`, `_vformat`, `parse`,
//   `get_field`, `get_value`, `check_unused_args`, `format_field`,
//   `convert_field`) through `self`, so subclasses can override any of them,
//   and parses with the `_string` scanners exactly as CPython does.
//
// Reference: <https://docs.python.org/3/library/string.html>

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use indexmap::IndexMap;
use pyrust_core::{WeakTarget, str_codepoint_count, str_codepoint_offset};
use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::_string::{
    FieldStep, formatter_parser_value, split_field_name_value, str_text,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, c3_linearize_classes, is_stop_iteration_error, lookup_class_attr,
    lookup_exc_class, make_iterator, render_instance_str, render_value_repr,
};
use crate::value::{
    InstanceAttrs, PyClass, PyDict, PyInstance, PyKey, PyModule, PySet, Value, ValueKind,
};

const MODULE_DOC: &str = "A collection of string constants.\n\
\n\
Public module variables:\n\
\n\
whitespace -- a string containing all ASCII whitespace\n\
ascii_lowercase -- a string containing all ASCII lowercase letters\n\
ascii_uppercase -- a string containing all ASCII uppercase letters\n\
ascii_letters -- a string containing all ASCII letters\n\
digits -- a string containing all ASCII decimal digits\n\
hexdigits -- a string containing all ASCII hexadecimal digits\n\
octdigits -- a string containing all ASCII octal digits\n\
punctuation -- a string containing all ASCII punctuation characters\n\
printable -- a string containing all ASCII characters considered printable\n\
\n";

const TEMPLATE_DOC: &str = "A string class for supporting $-substitutions.";

const ALL: [&str; 12] = [
    "ascii_letters",
    "ascii_lowercase",
    "ascii_uppercase",
    "capwords",
    "digits",
    "hexdigits",
    "octdigits",
    "printable",
    "punctuation",
    "whitespace",
    "Formatter",
    "Template",
];

/// `Template.idpattern`: ASCII identifiers matched case-insensitively under the
/// default `flags` (bpo-31672).
const DEFAULT_IDPATTERN: &str = "(?a:[_a-z][_a-z0-9]*)";

/// The module namespace in CPython's definition order (after the dunders).
const NAMESPACE_ORDER: [&str; 17] = [
    "__all__",
    "_string",
    "whitespace",
    "ascii_lowercase",
    "ascii_uppercase",
    "ascii_letters",
    "digits",
    "hexdigits",
    "octdigits",
    "punctuation",
    "printable",
    "capwords",
    "_re",
    "_ChainMap",
    "_sentinel_dict",
    "Template",
    "Formatter",
];

/// Hidden callable passed to `pattern.sub` (CPython's nested `convert`).
const CONVERTER_CLASS_NAME: &str = "_TemplateConverter";

/// `re` flag bits that cannot change how the default `Template` pattern
/// matches: IGNORECASE, MULTILINE, DOTALL, UNICODE, VERBOSE, ASCII.
const RE_IGNORECASE: i64 = 2;
const RE_VERBOSE: i64 = 64;
const FAST_FLAG_MASK: i64 = 2 | 8 | 16 | 32 | 64 | 256;

const SWITCH_NUMBERING: &str =
    "cannot switch from manual field specification to automatic field numbering";

type ClassRc = Rc<RefCell<PyClass>>;

// ── Generation-local registries ──────────────────────────────────────────────

/// The pieces of one imported `string` generation that its methods need.
struct TemplateGeneration {
    /// That generation's `Template` (the `super()` anchor of
    /// `__init_subclass__`).
    class: Weak<RefCell<PyClass>>,
    /// The native `Template._invalid` as installed, to detect overrides.
    native_invalid: Value,
    /// The `_sentinel_dict` default of `substitute` / `safe_substitute`.
    sentinel: Value,
    /// The hidden `_TemplateConverter` class.
    converter: ClassRc,
}

/// A compiled `Template.pattern` whose matching the native scanner reproduces:
/// `re.escape(delimiter)` followed by the default `idpattern` alternatives.
#[derive(Clone)]
struct FastSpec {
    delimiter: Rc<str>,
    ignorecase: bool,
}

// Thread-local justification:
// (a) Why `thread_local!`-style storage: the registries hold `!Send` `Rc` /
//     `Weak` class handles and pattern objects that the module's methods must
//     reach without a receiver-owned slot: `__init_subclass__` needs its own
//     generation's `Template` for `super()`, `substitute` needs the hidden
//     converter class and the `_sentinel_dict` identity, and the native
//     scanner needs to recognise the patterns it may stand in for.
// (b) Multi-interpreter / multi-threaded impact: every import generation is
//     registered (weakly where possible) and dead entries are pruned, so a
//     re-import never invalidates live classes or patterns; separate threads
//     are completely isolated.
pyrust_core::gil_local! {
    static TEMPLATE_GENERATIONS: RefCell<Vec<TemplateGeneration>> =
        const { RefCell::new(Vec::new()) };
    static FAST_PATTERNS: RefCell<Vec<(WeakTarget, FastSpec)>> =
        const { RefCell::new(Vec::new()) };
}

fn with_generations<R>(f: impl FnOnce(&mut Vec<TemplateGeneration>) -> R) -> R {
    TEMPLATE_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.class.strong_count() > 0);
        f(&mut generations)
    })
}

fn fast_spec_for(pattern: &Value) -> Option<FastSpec> {
    FAST_PATTERNS.with(|patterns| {
        patterns.borrow().iter().find_map(|(weak, spec)| {
            weak.upgrade()
                .filter(|live| live.is_identical_to(pattern))
                .map(|_| spec.clone())
        })
    })
}

fn register_fast_pattern(pattern: &Value, spec: FastSpec) {
    let Some((weak, _)) = WeakTarget::downgrade_value(pattern) else {
        return;
    };
    FAST_PATTERNS.with(|patterns| {
        let mut patterns = patterns.borrow_mut();
        patterns.retain(|(entry, _)| entry.upgrade().is_some());
        if !patterns.iter().any(|(entry, _)| {
            entry
                .upgrade()
                .is_some_and(|live| live.is_identical_to(pattern))
        }) {
            patterns.push((weak, spec));
        }
    });
}

// ── Generic interpreter helpers ──────────────────────────────────────────────

fn positional(args: &[Value]) -> Vec<ExpandedCallArg> {
    args.iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect()
}

fn call(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    interp.call_function_expanded(callee, &positional(args))
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call(interp, callee, args)
}

fn call_method_kw(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    args: &[Value],
    kwargs: Vec<(&str, Value)>,
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    let mut expanded = positional(args);
    expanded.extend(kwargs.into_iter().map(|(key, value)| ExpandedCallArg {
        name: Some(key.to_string()),
        value,
    }));
    interp.call_function_expanded(callee, &expanded)
}

fn builtin(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let builtins = interp.load_module("builtins")?;
    interp.get_attr(&builtins, name)
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

/// Raise `exc_name(*args)` as a real exception instance (multi-arg payloads).
fn raise_with_args(interp: &mut Interpreter, exc_name: &str, args: &[Value]) -> PyError {
    let Some(class) = lookup_exc_class(exc_name) else {
        return PyError::runtime(format!("string: missing builtin {exc_name}"));
    };
    match call(interp, Value::py_class(class), args) {
        Ok(exc) => PyError::raised(exc),
        Err(err) => err,
    }
}

/// `str(value)`.
fn py_str(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    if value.is_str() {
        return Ok(value.clone());
    }
    Ok(Value::string(render_instance_str(interp, value)?))
}

/// Pull the next item of `iterator`, or `None` once it is exhausted.
fn next_item(interp: &mut Interpreter, iterator: &Value) -> Result<Option<Value>> {
    match interp.call_next(iterator, None) {
        Ok(item) => Ok(Some(item)),
        Err(err) if is_stop_iteration_error(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Decompose a generic `'T' object is not iterable` TypeError into `T`.
fn not_iterable_type_name(err: &PyError) -> Option<String> {
    match err.kind() {
        pyrust_core::PyErrorKind::Named(class_name, message)
            if class_name.as_ref() == "TypeError" =>
        {
            message
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix("' object is not iterable"))
                .map(str::to_owned)
        }
        _ => None,
    }
}

/// `a, b, ... = value` with CPython's unpacking diagnostics.
fn unpack(interp: &mut Interpreter, value: &Value, expected: usize) -> Result<Vec<Value>> {
    let too_few = |got: usize| {
        value_error(format!(
            "not enough values to unpack (expected {expected}, got {got})"
        ))
    };
    let too_many = || value_error(format!("too many values to unpack (expected {expected})"));
    if let Some(items) = value.as_tuple() {
        return match items.len().cmp(&expected) {
            std::cmp::Ordering::Equal => Ok(items.to_vec()),
            std::cmp::Ordering::Less => Err(too_few(items.len())),
            std::cmp::Ordering::Greater => Err(too_many()),
        };
    }
    let iterator =
        make_iterator(interp, value).map_err(|err| match not_iterable_type_name(&err) {
            Some(type_name) => type_error(format!("cannot unpack non-iterable {type_name} object")),
            None => err,
        })?;
    let mut items = Vec::with_capacity(expected);
    while items.len() < expected {
        match next_item(interp, &iterator)? {
            Some(item) => items.push(item),
            None => return Err(too_few(items.len())),
        }
    }
    if next_item(interp, &iterator)?.is_some() {
        return Err(too_many());
    }
    Ok(items)
}

/// `value == text` for a `str` literal on the right.
fn eq_str(interp: &mut Interpreter, value: &Value, text: &str) -> Result<bool> {
    if let Some(s) = value.as_str() {
        return Ok(s == text);
    }
    let res = interp.eval_binary(value.clone(), BinaryOp::Eq, Value::string(text))?;
    interp.truthy_value(&res)
}

/// `''.join(parts)`, natively when every part is an exact `str`.
fn join_parts(interp: &mut Interpreter, parts: Vec<Value>) -> Result<Value> {
    if parts.iter().all(Value::is_str) {
        let mut out = String::new();
        for part in &parts {
            out.push_str(part.as_str().unwrap_or_default());
        }
        return Ok(Value::string(out));
    }
    call_method(interp, &Value::string(""), "join", &[Value::list(parts)])
}

fn class_of(value: &Value) -> Option<ClassRc> {
    match value.kind() {
        ValueKind::PyClass(class) => Some(Rc::clone(class)),
        _ => None,
    }
}

// ── capwords ─────────────────────────────────────────────────────────────────

/// `(sep or ' ').join(map(str.capitalize, s.split(sep)))`.
fn capwords_impl(interp: &mut Interpreter, s: &Value, sep: &Value) -> Result<Value> {
    let joiner = if interp.truthy_value(sep)? {
        sep.clone()
    } else {
        Value::string(" ")
    };
    let join = interp.get_attr(&joiner, "join")?;
    let str_type = builtin(interp, "str")?;
    let capitalize = interp.get_attr(&str_type, "capitalize")?;
    let words = call_method(interp, s, "split", std::slice::from_ref(sep))?;
    let iterator = make_iterator(interp, &words)?;
    let mut capitalized = Vec::new();
    while let Some(word) = next_item(interp, &iterator)? {
        capitalized.push(call(interp, capitalize.clone(), &[word])?);
    }
    call(interp, join, &[Value::list(capitalized)])
}

// ── Template ─────────────────────────────────────────────────────────────────

/// `pattern` as CPython's `Template.__init_subclass__` spells it.
fn template_pattern_source(delim: &str, id: &str, bid: &str) -> String {
    let mut out = String::new();
    out.push_str("\n            ");
    out.push_str(delim);
    out.push_str("(?:\n              (?P<escaped>");
    out.push_str(delim);
    out.push_str(")  |   # Escape sequence of two delimiters\n              (?P<named>");
    out.push_str(id);
    out.push_str(")       |   # delimiter and a Python identifier\n              {(?P<braced>");
    out.push_str(bid);
    out.push_str(")} |   # delimiter and a braced identifier\n");
    out.push_str("              (?P<invalid>)             # Other ill-formed delimiter exprs\n");
    out.push_str("            )\n            ");
    out
}

/// `f"{value}"`.
fn format_text(interp: &mut Interpreter, value: &Value) -> Result<String> {
    if let Some(s) = value.as_str() {
        return Ok(s.to_owned());
    }
    let formatted = call(
        interp,
        Value::builtin_function("format"),
        &[value.clone(), Value::string("")],
    )?;
    Ok(str_text(&formatted).unwrap_or_default())
}

/// The generation's `Template` that `cls` derives from.
fn template_base_of(cls: &ClassRc) -> Option<ClassRc> {
    let mro = c3_linearize_classes(cls);
    with_generations(|generations| {
        mro.into_iter().find(|class| {
            generations
                .iter()
                .any(|generation| generation.class.as_ptr() == Rc::as_ptr(class))
        })
    })
}

/// `Template.__init_subclass__(cls)`: recompile `cls.pattern`.
fn init_subclass_impl(interp: &mut Interpreter, cls: &Value) -> Result<()> {
    let Some(cls_rc) = class_of(cls) else {
        return Err(type_error("Template.__init_subclass__() requires a class"));
    };
    // super().__init_subclass__()
    if let Some(template) = template_base_of(&cls_rc) {
        let super_type = builtin(interp, "super")?;
        let proxy = call(
            interp,
            super_type,
            &[Value::py_class(template), cls.clone()],
        )?;
        call_method(interp, &proxy, "__init_subclass__", &[])?;
    }
    let re_mod = interp.load_module("re")?;
    let mut fast_delimiter: Option<Rc<str>> = None;
    let pattern = if cls_rc.borrow().attrs.contains_key("pattern") {
        interp.get_attr(cls, "pattern")?
    } else {
        let delimiter = interp.get_attr(cls, "delimiter")?;
        let delim = call_method(interp, &re_mod, "escape", std::slice::from_ref(&delimiter))?;
        let id = interp.get_attr(cls, "idpattern")?;
        let brace_id = interp.get_attr(cls, "braceidpattern")?;
        let bid = if interp.truthy_value(&brace_id)? {
            brace_id
        } else {
            interp.get_attr(cls, "idpattern")?
        };
        let delim_text = format_text(interp, &delim)?;
        let id_text = format_text(interp, &id)?;
        let bid_text = format_text(interp, &bid)?;
        if id_text == DEFAULT_IDPATTERN
            && bid_text == DEFAULT_IDPATTERN
            && let Some(literal) = delimiter.as_str()
            && !literal.is_empty()
        {
            fast_delimiter = Some(Rc::from(literal));
        }
        Value::string(template_pattern_source(&delim_text, &id_text, &bid_text))
    };
    let compile = interp.get_attr(&re_mod, "compile")?;
    let flags = interp.get_attr(cls, "flags")?;
    let verbose = interp.get_attr(&re_mod, "VERBOSE")?;
    let flags = interp.eval_binary(flags, BinaryOp::BitOr, verbose)?;
    let compiled = call(interp, compile, &[pattern, flags.clone()])?;
    interp.assign_attr(cls.clone(), "pattern", compiled.clone())?;
    if let (Some(delimiter), Some(bits)) = (fast_delimiter, flags.as_int())
        && bits & !FAST_FLAG_MASK == 0
        && bits & RE_VERBOSE != 0
    {
        let ignorecase = bits & RE_IGNORECASE != 0;
        // Under IGNORECASE a cased delimiter would also match its other case
        // forms; leave those to `re`.
        let literal_match = !ignorecase
            || delimiter
                .chars()
                .all(|c| c.is_ascii() && !c.is_ascii_alphabetic());
        if literal_match {
            register_fast_pattern(
                &compiled,
                FastSpec {
                    delimiter,
                    ignorecase,
                },
            );
        }
    }
    Ok(())
}

/// True when `self._invalid` resolves to the native `Template._invalid`.
fn invalid_hook_is_native(this: &Value) -> bool {
    let ValueKind::PyInstance(inst) = this.kind() else {
        return false;
    };
    let class = {
        let inst = inst.borrow();
        if inst.attrs.contains_key("_invalid") {
            return false;
        }
        Rc::clone(&inst.class)
    };
    let Some(found) = lookup_class_attr(&class, "_invalid") else {
        return false;
    };
    with_generations(|generations| {
        generations
            .iter()
            .any(|generation| generation.native_invalid.is_identical_to(&found))
    })
}

fn is_sentinel(value: &Value) -> bool {
    with_generations(|generations| {
        generations
            .iter()
            .any(|generation| generation.sentinel.is_identical_to(value))
    })
}

/// Python's `str.splitlines` line boundaries.
fn is_line_break(cp: u32) -> bool {
    matches!(
        cp,
        0x0A | 0x0B | 0x0C | 0x0D | 0x1C | 0x1D | 0x1E | 0x85 | 0x2028 | 0x2029
    )
}

/// `(lineno, colno)` of `Template._invalid` for `self.template[:i]`.
fn invalid_position(text: &str, i: i64) -> (i64, i64) {
    let total = str_codepoint_count(text) as i64;
    let end = if i < 0 {
        (total + i).max(0)
    } else {
        i.min(total)
    };
    let prefix = &text[..str_codepoint_offset(text, end as usize)];
    if prefix.is_empty() {
        return (1, 1);
    }
    let codepoints: Vec<u32> = pyrust_core::cesu8_codepoints(prefix).collect();
    let mut lines = 0i64;
    let mut line_start = 0usize;
    let mut previous_start = 0usize;
    let mut k = 0usize;
    while k < codepoints.len() {
        let cp = codepoints[k];
        k += 1;
        if is_line_break(cp) {
            if cp == 0x0D && codepoints.get(k) == Some(&0x0A) {
                k += 1;
            }
            lines += 1;
            previous_start = line_start;
            line_start = k;
        }
    }
    let last_len = if line_start < codepoints.len() {
        lines += 1;
        codepoints.len() - line_start
    } else {
        codepoints.len() - previous_start
    };
    let before_last = (codepoints.len() - last_len) as i64;
    (lines, i - before_last)
}

fn invalid_placeholder_error(lineno: i64, colno: i64) -> PyError {
    value_error(format!(
        "Invalid placeholder in string: line {lineno}, col {colno}"
    ))
}

/// `Template._invalid(self, mo)` for an arbitrary `template` / start value.
fn invalid_impl(interp: &mut Interpreter, this: &Value, mo: &Value) -> Result<Value> {
    let i = call_method(interp, mo, "start", &[Value::string("invalid")])?;
    let template = interp.get_attr(this, "template")?;
    if let (Some(text), Some(index)) = (str_text(&template), i.as_int()) {
        let (lineno, colno) = invalid_position(&text, index);
        return Err(invalid_placeholder_error(lineno, colno));
    }
    // Generic object protocol path for exotic `template` / `start` values.
    let slice_type = builtin(interp, "slice")?;
    let upto = call(interp, slice_type.clone(), &[Value::none(), i.clone()])?;
    let prefix = interp.eval_index(&template, upto)?;
    let lines = call_method_kw(
        interp,
        &prefix,
        "splitlines",
        &[],
        vec![("keepends", Value::bool_(true))],
    )?;
    let (lineno, colno) = if !interp.truthy_value(&lines)? {
        (Value::int(1), Value::int(1))
    } else {
        let head = call(interp, slice_type, &[Value::none(), Value::int(-1)])?;
        let head = interp.eval_index(&lines, head)?;
        let joined = call_method(interp, &Value::string(""), "join", &[head])?;
        let len = Value::builtin_function("len");
        let joined_len = call(interp, len.clone(), &[joined])?;
        let colno = interp.eval_binary(i, BinaryOp::Sub, joined_len)?;
        (call(interp, len, &[lines])?, colno)
    };
    let message = interp.eval_binary(
        Value::string("Invalid placeholder in string: line %d, col %d"),
        BinaryOp::Mod,
        Value::tuple(vec![lineno, colno]),
    )?;
    Err(raise_with_args(interp, "ValueError", &[message]))
}

fn unrecognized_group_error(interp: &mut Interpreter, this: &Value) -> PyError {
    match interp.get_attr(this, "pattern") {
        Ok(pattern) => raise_with_args(
            interp,
            "ValueError",
            &[
                Value::string("Unrecognized named group in pattern"),
                pattern,
            ],
        ),
        Err(err) => err,
    }
}

/// Build the mapping `substitute` / `safe_substitute` look names up in.
fn resolve_mapping(
    interp: &mut Interpreter,
    mapping: OptValueRef<'_>,
    kws: IndexMap<String, Value>,
) -> Result<Value> {
    let kws_dict = Value::dict(PyDict::default());
    let has_kws = !kws.is_empty();
    for (key, value) in kws {
        kws_dict.dict_insert(PyKey::str_from(&key), value)?;
    }
    match mapping {
        OptionalArg::Omitted => Ok(kws_dict),
        OptionalArg::Provided(mapping) if is_sentinel(mapping.0) => Ok(kws_dict),
        OptionalArg::Provided(mapping) if !has_kws => Ok(mapping.0.clone()),
        OptionalArg::Provided(mapping) => {
            let collections = interp.load_module("collections")?;
            let chain_map = interp.get_attr(&collections, "ChainMap")?;
            call(interp, chain_map, &[kws_dict, mapping.0.clone()])
        }
    }
}

/// `str(mapping[name])`.
fn lookup_named(interp: &mut Interpreter, mapping: &Value, name: Value) -> Result<Value> {
    let value = interp.eval_index(mapping, name)?;
    py_str(interp, &value)
}

/// The output of a native `pattern.sub`, mirroring `_sre`'s piece list: empty
/// literal runs and `None` replacements are dropped, and a non-`str`
/// replacement fails only at the final join, naming its piece index.
struct SubOutput {
    text: String,
    pieces: usize,
    bad_piece: Option<(usize, String)>,
}

impl SubOutput {
    fn new(capacity: usize) -> Self {
        SubOutput {
            text: String::with_capacity(capacity),
            pieces: 0,
            bad_piece: None,
        }
    }

    fn literal(&mut self, text: &str) {
        if !text.is_empty() {
            self.text.push_str(text);
            self.pieces += 1;
        }
    }

    /// A replacement that is the matched text itself (`mo.group()`).
    fn matched(&mut self, text: &str) {
        self.text.push_str(text);
        self.pieces += 1;
    }

    fn replacement(&mut self, value: &Value) {
        if value.is_none() {
            return;
        }
        match str_text(value) {
            Some(text) => self.text.push_str(&text),
            None => {
                if self.bad_piece.is_none() {
                    let type_name = crate::interpreter::value_type_name_str(value);
                    self.bad_piece = Some((self.pieces, type_name.to_string()));
                }
            }
        }
        self.pieces += 1;
    }

    fn finish(self) -> Result<Value> {
        if let Some((index, type_name)) = self.bad_piece {
            return Err(type_error(format!(
                "sequence item {index}: expected str instance, {type_name} found"
            )));
        }
        Ok(Value::string(self.text))
    }
}

/// End of the identifier starting at `start`, if one starts there.
fn identifier_end(bytes: &[u8], start: usize, ignorecase: bool) -> Option<usize> {
    let is_start =
        |b: u8| b == b'_' || b.is_ascii_lowercase() || (ignorecase && b.is_ascii_uppercase());
    let first = *bytes.get(start)?;
    if !is_start(first) {
        return None;
    }
    let mut end = start + 1;
    while end < bytes.len() && (is_start(bytes[end]) || bytes[end].is_ascii_digit()) {
        end += 1;
    }
    Some(end)
}

/// One match of the default pattern at a delimiter occurrence.
enum Placeholder {
    /// `$$`; the match ends at `end`.
    Escaped { end: usize },
    /// `$name` / `${name}`: identifier byte range and match end.
    Named { name: (usize, usize), end: usize },
    /// A bare delimiter; the `invalid` group starts at `start`.
    Invalid { start: usize },
}

/// Scan the default pattern's match at the delimiter found at `pos`.
fn classify(text: &str, spec: &FastSpec, pos: usize) -> Placeholder {
    let bytes = text.as_bytes();
    let delimiter = spec.delimiter.as_bytes();
    let after = pos + delimiter.len();
    if bytes[after..].starts_with(delimiter) {
        return Placeholder::Escaped {
            end: after + delimiter.len(),
        };
    }
    if let Some(end) = identifier_end(bytes, after, spec.ignorecase) {
        return Placeholder::Named {
            name: (after, end),
            end,
        };
    }
    if bytes.get(after) == Some(&b'{')
        && let Some(end) = identifier_end(bytes, after + 1, spec.ignorecase)
        && bytes.get(end) == Some(&b'}')
    {
        return Placeholder::Named {
            name: (after + 1, end),
            end: end + 1,
        };
    }
    Placeholder::Invalid { start: after }
}

/// Native `pattern.sub(convert, template)` for a [`FastSpec`] pattern.
fn fast_substitute(
    interp: &mut Interpreter,
    this: &Value,
    spec: &FastSpec,
    text: &str,
    mapping: &Value,
    safe: bool,
) -> Result<Value> {
    let mut out = SubOutput::new(text.len());
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find(&*spec.delimiter) {
        let pos = cursor + offset;
        out.literal(&text[cursor..pos]);
        match classify(text, spec, pos) {
            Placeholder::Escaped { end } => {
                let delimiter = interp.get_attr(this, "delimiter")?;
                out.replacement(&delimiter);
                cursor = end;
            }
            Placeholder::Named { name, end } => {
                let key = Value::string(&text[name.0..name.1]);
                match lookup_named(interp, mapping, key) {
                    Ok(value) => out.replacement(&value),
                    Err(err) if safe && err.class_name_is("KeyError") => {
                        out.matched(&text[pos..end]);
                    }
                    Err(err) => return Err(err),
                }
                cursor = end;
            }
            Placeholder::Invalid { start } => {
                if !safe {
                    let index = str_codepoint_count(&text[..start]) as i64;
                    let (lineno, colno) = invalid_position(text, index);
                    return Err(invalid_placeholder_error(lineno, colno));
                }
                out.matched(&text[pos..start]);
                cursor = start;
            }
        }
    }
    out.literal(&text[cursor..]);
    out.finish()
}

/// `Template.substitute` / `Template.safe_substitute`.
fn substitute_impl(
    interp: &mut Interpreter,
    this: &Value,
    mapping: OptValueRef<'_>,
    kws: IndexMap<String, Value>,
    safe: bool,
) -> Result<Value> {
    let mapping = resolve_mapping(interp, mapping, kws)?;
    let pattern = interp.get_attr(this, "pattern")?;
    let mut template = None;
    if let Some(spec) = fast_spec_for(&pattern)
        && (safe || invalid_hook_is_native(this))
    {
        let value = interp.get_attr(this, "template")?;
        if let Some(text) = str_text(&value) {
            return fast_substitute(interp, this, &spec, &text, &mapping, safe);
        }
        template = Some(value);
    }
    let sub = interp.get_attr(&pattern, "sub")?;
    let template = match template {
        Some(value) => value,
        None => interp.get_attr(this, "template")?,
    };
    let converter = new_converter(this.clone(), mapping, safe)?;
    call(interp, sub, &[converter, template])
}

/// The `convert(mo)` callback of `substitute` / `safe_substitute`.
fn convert_impl(
    interp: &mut Interpreter,
    this: &Value,
    mapping: &Value,
    safe: bool,
    mo: &Value,
) -> Result<Value> {
    let named = call_method(interp, mo, "group", &[Value::string("named")])?;
    let named = if interp.truthy_value(&named)? {
        named
    } else {
        call_method(interp, mo, "group", &[Value::string("braced")])?
    };
    if !named.is_none() {
        return match lookup_named(interp, mapping, named) {
            Err(err) if safe && err.class_name_is("KeyError") => {
                call_method(interp, mo, "group", &[])
            }
            other => other,
        };
    }
    let escaped = call_method(interp, mo, "group", &[Value::string("escaped")])?;
    if !escaped.is_none() {
        return interp.get_attr(this, "delimiter");
    }
    let invalid = call_method(interp, mo, "group", &[Value::string("invalid")])?;
    if !invalid.is_none() {
        if safe {
            return call_method(interp, mo, "group", &[]);
        }
        call_method(interp, this, "_invalid", std::slice::from_ref(mo))?;
    }
    Err(unrecognized_group_error(interp, this))
}

struct ConverterState {
    template: Value,
    mapping: Value,
    safe: bool,
}

fn new_converter(template: Value, mapping: Value, safe: bool) -> Result<Value> {
    let class = with_generations(|generations| {
        generations
            .last()
            .map(|generation| Rc::clone(&generation.converter))
    })
    .ok_or_else(|| PyError::runtime("string: Template converter is not initialised".into()))?;
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::default(),
    })));
    crate::builtin_modules::macros::native_state::set_native_state(
        &inst,
        ConverterState {
            template,
            mapping,
            safe,
        },
    )?;
    Ok(inst)
}

/// What `is_valid` / `get_identifiers` see in one match.
enum MatchKind {
    Named(Value),
    Escaped,
    Invalid,
    Unrecognized,
}

/// Classify a `re.Match` of a custom pattern like CPython's loops do.
fn match_kind(interp: &mut Interpreter, mo: &Value, for_identifiers: bool) -> Result<MatchKind> {
    let group = |interp: &mut Interpreter, name: &str| {
        call_method(interp, mo, "group", &[Value::string(name)])
    };
    if for_identifiers {
        let named = group(interp, "named")?;
        let named = if interp.truthy_value(&named)? {
            named
        } else {
            group(interp, "braced")?
        };
        if !named.is_none() {
            return Ok(MatchKind::Named(named));
        }
        if !group(interp, "invalid")?.is_none() {
            return Ok(MatchKind::Invalid);
        }
        if !group(interp, "escaped")?.is_none() {
            return Ok(MatchKind::Escaped);
        }
        return Ok(MatchKind::Unrecognized);
    }
    if !group(interp, "invalid")?.is_none() {
        return Ok(MatchKind::Invalid);
    }
    if group(interp, "named")?.is_none()
        && group(interp, "braced")?.is_none()
        && group(interp, "escaped")?.is_none()
    {
        return Ok(MatchKind::Unrecognized);
    }
    Ok(MatchKind::Escaped)
}

/// Shared walk of `is_valid` (`for_identifiers == false`) and
/// `get_identifiers` (`true`).
fn scan_template(interp: &mut Interpreter, this: &Value, for_identifiers: bool) -> Result<Value> {
    let pattern = interp.get_attr(this, "pattern")?;
    let mut ids: Vec<Value> = Vec::new();
    let mut template = None;
    if let Some(spec) = fast_spec_for(&pattern) {
        let value = interp.get_attr(this, "template")?;
        if let Some(text) = str_text(&value) {
            let mut cursor = 0usize;
            while let Some(offset) = text[cursor..].find(&*spec.delimiter) {
                let pos = cursor + offset;
                match classify(&text, &spec, pos) {
                    Placeholder::Escaped { end } => cursor = end,
                    Placeholder::Named { name, end } => {
                        let name = &text[name.0..name.1];
                        if for_identifiers && !ids.iter().any(|id| id.as_str() == Some(name)) {
                            ids.push(Value::string(name));
                        }
                        cursor = end;
                    }
                    Placeholder::Invalid { start } => {
                        if !for_identifiers {
                            return Ok(Value::bool_(false));
                        }
                        cursor = start;
                    }
                }
            }
            return Ok(if for_identifiers {
                Value::list(ids)
            } else {
                Value::bool_(true)
            });
        }
        template = Some(value);
    }
    let finditer = interp.get_attr(&pattern, "finditer")?;
    let template = match template {
        Some(value) => value,
        None => interp.get_attr(this, "template")?,
    };
    let matches = call(interp, finditer, &[template])?;
    let iterator = make_iterator(interp, &matches)?;
    while let Some(mo) = next_item(interp, &iterator)? {
        match match_kind(interp, &mo, for_identifiers)? {
            MatchKind::Named(named) => {
                let mut seen = false;
                for id in &ids {
                    let res = interp.eval_binary(id.clone(), BinaryOp::Eq, named.clone())?;
                    if interp.truthy_value(&res)? {
                        seen = true;
                        break;
                    }
                }
                if !seen {
                    ids.push(named);
                }
            }
            MatchKind::Invalid if !for_identifiers => return Ok(Value::bool_(false)),
            MatchKind::Invalid | MatchKind::Escaped => {}
            MatchKind::Unrecognized => return Err(unrecognized_group_error(interp, this)),
        }
    }
    Ok(if for_identifiers {
        Value::list(ids)
    } else {
        Value::bool_(true)
    })
}

// ── Formatter ────────────────────────────────────────────────────────────────

/// `recursion_depth < 0`.
fn is_negative(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if let Some(n) = value.as_int() {
        return Ok(n < 0);
    }
    let res = interp.eval_binary(value.clone(), BinaryOp::Lt, Value::int(0))?;
    interp.truthy_value(&res)
}

/// `field_name.isdigit()`.
fn is_digit_name(interp: &mut Interpreter, field_name: &Value) -> Result<bool> {
    if let Some(s) = field_name.as_str()
        && s.is_ascii()
    {
        return Ok(!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
    }
    let res = call_method(interp, field_name, "isdigit", &[])?;
    interp.truthy_value(&res)
}

/// `Formatter._vformat`.
#[allow(clippy::too_many_arguments)]
fn vformat_inner(
    interp: &mut Interpreter,
    this: &Value,
    format_string: Value,
    args: &Value,
    kwargs: &Value,
    used_args: &Value,
    recursion_depth: &Value,
    auto_arg_index: Value,
) -> Result<Value> {
    if is_negative(interp, recursion_depth)? {
        return Err(value_error("Max string recursion exceeded"));
    }
    let mut auto_arg_index = auto_arg_index;
    let mut result = Vec::new();
    let parsed = call_method(interp, this, "parse", &[format_string])?;
    let iterator = make_iterator(interp, &parsed)?;
    while let Some(item) = next_item(interp, &iterator)? {
        let mut parts = unpack(interp, &item, 4)?.into_iter();
        let (Some(literal_text), Some(field_name), Some(format_spec), Some(conversion)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(PyError::runtime("string: unpack arity".into()));
        };
        // output the literal text
        if interp.truthy_value(&literal_text)? {
            result.push(literal_text);
        }
        if field_name.is_none() {
            continue;
        }
        // handle arg indexing when empty field_names are given.
        let mut field_name = field_name;
        if eq_str(interp, &field_name, "")? {
            if auto_arg_index.is_bool() && !auto_arg_index.as_bool() {
                return Err(value_error(SWITCH_NUMBERING));
            }
            field_name = py_str(interp, &auto_arg_index)?;
            auto_arg_index = interp.eval_binary(auto_arg_index, BinaryOp::Add, Value::int(1))?;
        } else if is_digit_name(interp, &field_name)? {
            if interp.truthy_value(&auto_arg_index)? {
                return Err(value_error(SWITCH_NUMBERING));
            }
            // disable auto arg incrementing, if it gets used later on, then
            // an exception will be raised
            auto_arg_index = Value::bool_(false);
        }
        // given the field_name, find the object it references and the
        // argument it came from
        let found = call_method(
            interp,
            this,
            "get_field",
            &[field_name, args.clone(), kwargs.clone()],
        )?;
        let mut found = unpack(interp, &found, 2)?.into_iter();
        let (Some(obj), Some(arg_used)) = (found.next(), found.next()) else {
            return Err(PyError::runtime("string: unpack arity".into()));
        };
        call_method(interp, used_args, "add", &[arg_used])?;
        // do any conversion on the resulting object
        let obj = call_method(interp, this, "convert_field", &[obj, conversion])?;
        // expand the format spec, if needed
        let depth = interp.eval_binary(recursion_depth.clone(), BinaryOp::Sub, Value::int(1))?;
        let expanded = call_method_kw(
            interp,
            this,
            "_vformat",
            &[
                format_spec,
                args.clone(),
                kwargs.clone(),
                used_args.clone(),
                depth,
            ],
            vec![("auto_arg_index", auto_arg_index)],
        )?;
        let mut expanded = unpack(interp, &expanded, 2)?.into_iter();
        let (Some(format_spec), Some(next_index)) = (expanded.next(), expanded.next()) else {
            return Err(PyError::runtime("string: unpack arity".into()));
        };
        auto_arg_index = next_index;
        // format the object and append to the result
        result.push(call_method(
            interp,
            this,
            "format_field",
            &[obj, format_spec],
        )?);
    }
    let joined = join_parts(interp, result)?;
    Ok(Value::tuple(vec![joined, auto_arg_index]))
}

/// `Formatter.vformat`.
fn vformat_impl(
    interp: &mut Interpreter,
    this: &Value,
    format_string: Value,
    args: Value,
    kwargs: Value,
) -> Result<Value> {
    let used_args = Value::set(PySet::default());
    let res = call_method(
        interp,
        this,
        "_vformat",
        &[
            format_string,
            args.clone(),
            kwargs.clone(),
            used_args.clone(),
            Value::int(2),
        ],
    )?;
    let mut parts = unpack(interp, &res, 2)?.into_iter();
    let result = parts.next().unwrap_or_else(Value::none);
    call_method(
        interp,
        this,
        "check_unused_args",
        &[used_args, args, kwargs],
    )?;
    Ok(result)
}

/// `isinstance(key, int)`.
fn is_int_instance(interp: &mut Interpreter, key: &Value) -> Result<bool> {
    if key.is_int() || key.is_bool() || key.as_int().is_some() {
        return Ok(true);
    }
    if key.is_str() || key.is_none() || key.is_float() {
        return Ok(false);
    }
    let int_type = builtin(interp, "int")?;
    let res = call(
        interp,
        Value::builtin_function("isinstance"),
        &[key.clone(), int_type],
    )?;
    interp.truthy_value(&res)
}

/// `Formatter.get_field`.
fn get_field_impl(
    interp: &mut Interpreter,
    this: &Value,
    field_name: &Value,
    args: Value,
    kwargs: Value,
) -> Result<Value> {
    let (first, mut rest) = split_field_name_value(field_name)?;
    let mut obj = call_method(interp, this, "get_value", &[first.clone(), args, kwargs])?;
    // loop through the rest of the field_name, doing getattr or getitem as
    // needed
    while let Some(step) = rest.next_step()? {
        obj = match step {
            FieldStep::Attr(name) => interp.get_attr(&obj, &name)?,
            FieldStep::Item(key) => interp.eval_index(&obj, key)?,
        };
    }
    Ok(Value::tuple(vec![obj, first]))
}

/// `Formatter.convert_field`.
fn convert_field_impl(interp: &mut Interpreter, value: Value, conversion: &Value) -> Result<Value> {
    if conversion.is_none() {
        return Ok(value);
    }
    if eq_str(interp, conversion, "s")? {
        return py_str(interp, &value);
    }
    if eq_str(interp, conversion, "r")? {
        return Ok(Value::string(render_value_repr(interp, &value)?));
    }
    if eq_str(interp, conversion, "a")? {
        return call(interp, Value::builtin_function("ascii"), &[value]);
    }
    let shown = py_str(interp, conversion)?;
    Err(value_error(format!(
        "Unknown conversion specifier {}",
        str_text(&shown).unwrap_or_default()
    )))
}

// ── Module wiring ────────────────────────────────────────────────────────────

/// Finish the module after it is built: CPython's `from collections import
/// ChainMap`, `import re`, `import _string`, `Template.flags`,
/// `Template.__init_subclass__()`, the hidden converter, and the namespace
/// order.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let string_helpers = interp.load_module("_string")?;
    let re_mod = interp.load_module("re")?;
    let collections = interp.load_module("collections")?;
    let chain_map = interp.get_attr(&collections, "ChainMap")?;
    let ignorecase = interp.get_attr(&re_mod, "IGNORECASE")?;

    let (template_val, converter, sentinel) = {
        let mut module = module.borrow_mut();
        let converter = module.attrs.shift_remove(CONVERTER_CLASS_NAME);
        (
            module.attrs.get("Template").cloned(),
            converter,
            module.attrs.get("_sentinel_dict").cloned(),
        )
    };
    let (Some(template_val), Some(converter), Some(sentinel)) = (template_val, converter, sentinel)
    else {
        return Err(PyError::runtime("string: module members missing".into()));
    };
    let (Some(template), Some(converter)) = (class_of(&template_val), class_of(&converter)) else {
        return Err(PyError::runtime("string: Template is not a class".into()));
    };
    {
        let mut template = template.borrow_mut();
        // `flags = _re.IGNORECASE`, keeping the attribute's declaration slot.
        template.attrs.insert("flags".to_string(), ignorecase);
        // CPython defines `__init_subclass__` right after `flags`.
        if let Some(hook) = template.attrs.shift_remove("__init_subclass__")
            && let Some(index) = template.attrs.get_index_of("flags")
        {
            template
                .attrs
                .shift_insert(index + 1, "__init_subclass__".to_string(), hook);
        }
    }
    // A docstring-less class gets `__doc__ = None` last, after its methods.
    if let Some(formatter) = module.borrow().attrs.get("Formatter").and_then(class_of) {
        let mut formatter = formatter.borrow_mut();
        if let Some(doc) = formatter.attrs.shift_remove("__doc__") {
            formatter.attrs.insert("__doc__".to_string(), doc);
        }
    }
    let native_invalid = template
        .borrow()
        .attrs
        .get("_invalid")
        .cloned()
        .unwrap_or_else(Value::none);
    with_generations(|generations| {
        generations.push(TemplateGeneration {
            class: Rc::downgrade(&template),
            native_invalid,
            sentinel,
            converter,
        });
    });
    // Initialize Template.pattern.  __init_subclass__() is automatically
    // called only for subclasses, not for the Template class itself.
    init_subclass_impl(interp, &template_val)?;

    let mut module = module.borrow_mut();
    let mut ordered = Vec::with_capacity(NAMESPACE_ORDER.len());
    for name in NAMESPACE_ORDER {
        let value = match name {
            "_string" => Some(string_helpers.clone()),
            "_re" => Some(re_mod.clone()),
            "_ChainMap" => Some(chain_map.clone()),
            _ => module.attrs.shift_remove(name),
        };
        ordered.push((name, value));
    }
    for (name, value) in ordered {
        if let Some(value) = value {
            module.attrs.insert(name.to_string(), value);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => Value::list(ALL.into_iter().map(Value::string).collect()),
        // Some strings for ctype-style character classification
        "whitespace"      => Value::string(" \t\n\r\x0b\x0c"),
        "ascii_lowercase" => Value::string("abcdefghijklmnopqrstuvwxyz"),
        "ascii_uppercase" => Value::string("ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        "ascii_letters"   => Value::string("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        "digits"          => Value::string("0123456789"),
        "hexdigits"       => Value::string("0123456789abcdefABCDEF"),
        "octdigits"       => Value::string("01234567"),
        "punctuation"     => Value::string("!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~"),
        "printable"       => Value::string(
            "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\
             !\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~ \t\n\r\x0b\x0c"
        ),
        "_sentinel_dict"  => Value::dict(PyDict::default()),
    }

    // Capitalize the words in a string, e.g. " aBc  dEf " -> "Abc Def".
    fn capwords(s: PyValue, #[default(PyValue(Value::none()))] sep: PyValue) -> Result<Value> {
        capwords_impl(_interp, &s.0, &sep.0)
    }

    class Template {
        constants {
            "__doc__" => Value::string(TEMPLATE_DOC),
            "delimiter" => Value::string("$"),
            // r'[a-z]' matches to non-ASCII letters when used with IGNORECASE,
            // but without the ASCII flag.  We can't add re.ASCII to flags
            // because of backward compatibility.  So we use the ?a local flag
            // and [a-z] pattern.  See https://bugs.python.org/issue31672
            "idpattern" => Value::string(DEFAULT_IDPATTERN),
            "braceidpattern" => Value::none(),
            // Replaced by `re.IGNORECASE` in `post_load_init`.
            "flags" => Value::int(RE_IGNORECASE),
        }

        #[classmethod]
        fn __init_subclass__(
            #[positional_only] cls: PyValueRef<'_>,
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            // `def __init_subclass__(cls):` takes no class keywords.
            if let Some(name) = kwargs.keys().next() {
                return Err(type_error(format!(
                    "Template.__init_subclass__() got an unexpected keyword argument '{name}'"
                )));
            }
            init_subclass_impl(_interp, cls.0)?;
            Ok(Value::none())
        }

        fn __init__(#[positional_only] self_val: PyValueRef<'_>, template: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "template", template.0)?;
            Ok(Value::none())
        }

        fn _invalid(#[positional_only] self_val: PyValueRef<'_>, mo: PyValue) -> Result<Value> {
            invalid_impl(_interp, self_val.0, &mo.0)
        }

        fn substitute(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only]
            #[default(OptionalArg::Omitted)]
            mapping: OptValueRef<'_>,
            #[varkwargs] kws: IndexMap<String, Value>,
        ) -> Result<Value> {
            substitute_impl(_interp, self_val.0, mapping, kws, false)
        }

        fn safe_substitute(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only]
            #[default(OptionalArg::Omitted)]
            mapping: OptValueRef<'_>,
            #[varkwargs] kws: IndexMap<String, Value>,
        ) -> Result<Value> {
            substitute_impl(_interp, self_val.0, mapping, kws, true)
        }

        fn is_valid(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            scan_template(_interp, self_val.0, false)
        }

        fn get_identifiers(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            scan_template(_interp, self_val.0, true)
        }
    }

    #[native_state(ConverterState)]
    class _TemplateConverter {
        fn __call__(#[positional_only] self_val: PyValueRef<'_>, #[positional_only] mo: PyValue) -> Result<Value> {
            let (template, mapping, safe) = with_state(self_val.0, |st: &ConverterState| {
                Ok((st.template.clone(), st.mapping.clone(), st.safe))
            })?;
            convert_impl(_interp, &template, &mapping, safe, &mo.0)
        }
    }

    // The Formatter class: see PEP 3101 for details and purpose of this class.
    class Formatter {
        constants {
            "__doc__" => Value::none(),
        }

        fn format(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only]
            #[default(OptionalArg::Omitted)]
            format_string: OptValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let OptionalArg::Provided(format_string) = format_string else {
                return Err(type_error(
                    "Formatter.format() missing 1 required positional argument: 'format_string'",
                ));
            };
            let kwargs_dict = Value::dict(PyDict::default());
            for (key, value) in kwargs {
                kwargs_dict.dict_insert(PyKey::str_from(&key), value)?;
            }
            call_method(
                _interp,
                self_val.0,
                "vformat",
                &[format_string.0.clone(), Value::tuple(args.to_vec()), kwargs_dict],
            )
        }

        fn vformat(
            #[positional_only] self_val: PyValueRef<'_>,
            format_string: PyValue,
            args: PyValue,
            kwargs: PyValue,
        ) -> Result<Value> {
            vformat_impl(_interp, self_val.0, format_string.0, args.0, kwargs.0)
        }

        fn _vformat(
            #[positional_only] self_val: PyValueRef<'_>,
            format_string: PyValue,
            args: PyValue,
            kwargs: PyValue,
            used_args: PyValue,
            recursion_depth: PyValue,
            #[default(PyValue(Value::int(0)))] auto_arg_index: PyValue,
        ) -> Result<Value> {
            vformat_inner(
                _interp,
                self_val.0,
                format_string.0,
                &args.0,
                &kwargs.0,
                &used_args.0,
                &recursion_depth.0,
                auto_arg_index.0,
            )
        }

        fn get_value(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            args: PyValue,
            kwargs: PyValue,
        ) -> Result<Value> {
            let _ = self_val;
            if is_int_instance(_interp, &key.0)? {
                _interp.eval_index(&args.0, key.0)
            } else {
                _interp.eval_index(&kwargs.0, key.0)
            }
        }

        fn check_unused_args(
            #[positional_only] self_val: PyValueRef<'_>,
            used_args: PyValue,
            args: PyValue,
            kwargs: PyValue,
        ) -> Result<Value> {
            let _ = (self_val, used_args, args, kwargs, &_interp);
            Ok(Value::none())
        }

        fn format_field(
            #[positional_only] self_val: PyValueRef<'_>,
            value: PyValue,
            format_spec: PyValue,
        ) -> Result<Value> {
            let _ = self_val;
            call(_interp, Value::builtin_function("format"), &[value.0, format_spec.0])
        }

        fn convert_field(
            #[positional_only] self_val: PyValueRef<'_>,
            value: PyValue,
            conversion: PyValue,
        ) -> Result<Value> {
            let _ = self_val;
            convert_field_impl(_interp, value.0, &conversion.0)
        }

        // returns an iterable that contains tuples of the form:
        // (literal_text, field_name, format_spec, conversion)
        fn parse(#[positional_only] self_val: PyValueRef<'_>, format_string: PyValue) -> Result<Value> {
            let _ = (self_val, &_interp);
            formatter_parser_value(&format_string.0)
        }

        // given a field_name, find the object it references.
        fn get_field(
            #[positional_only] self_val: PyValueRef<'_>,
            field_name: PyValue,
            args: PyValue,
            kwargs: PyValue,
        ) -> Result<Value> {
            get_field_impl(_interp, self_val.0, &field_name.0, args.0, kwargs.0)
        }
    }
}
