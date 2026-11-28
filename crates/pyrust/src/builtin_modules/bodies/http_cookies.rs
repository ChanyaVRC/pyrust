// `http.cookies` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/http/cookies.py` (issues #3207, #3478).
//
// The module mirrors the CPython source structure: `CookieError`, the
// `_quote` / `_unquote` / `_getdate` helpers, the `Morsel` dict subclass, the
// `BaseCookie` container with its `__parse_string` parser and `SimpleCookie`.
// Like the Python source, every step reaches the overridable hooks through
// ordinary lookups (`self.value_encode`, `self.OutputString`, `self.__set`,
// `M.set`, `time.time`, ...), so subclasses and monkeypatching behave exactly
// as in CPython.
//
// The compiled-regex module globals of the Python source (`_CookiePattern`,
// `_OctalPatt`, `_QuotePatt`) and the `re` import are not exposed: the
// parser and the unquoting loop are hand-written scanners that reproduce the
// regexes' backtracking order (see `match_cookie`), so `import http.cookies`
// does not have to import the (pure-Python) `re` module.  `_is_legal_key` is
// the one exception: it builds its `re` pattern lazily when called from
// Python, the native code uses a character scan instead.
//
// Reference: <https://docs.python.org/3.12/library/http.cookies.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValueRef;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyClass, PyDict, PyKey, PySet, Value, ValueKind};

#[path = "http_cookies/docs.rs"]
mod docs;

type ClassRc = Rc<RefCell<PyClass>>;

// Per-interpreter state (`gil_local!`):
// (a) Why: native code raises `CookieError` from helpers that have no module
//     reference.
// (b) Every generation is kept (a re-import after `del sys.modules[...]`
//     creates new classes); the newest one is used, like CPython's module
//     global lookup after a reload.
pyrust_core::gil_local! {
    static COOKIE_ERRORS: RefCell<Vec<ClassRc>> = const { RefCell::new(Vec::new()) };
}

fn cookie_error(msg: impl Into<String>) -> PyError {
    match COOKIE_ERRORS.with(|errors| errors.borrow().last().cloned()) {
        Some(class) => PyError::class(class, msg),
        None => PyError::runtime("http.cookies is not initialised".to_string()),
    }
}

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

// The `def`s of `Lib/http/cookies.py`: CPython reports arity errors for these
// in the interpreter's `initialize_locals` wording, not the macro's.
const SIG_QUOTE: Sig = sig("_quote", &["str"], 0);
const SIG_UNQUOTE: Sig = sig("_unquote", &["str"], 0);
const SIG_GETDATE: Sig = sig("_getdate", &["future", "weekdayname", "monthname"], 3);
const SIG_MORSEL_INIT: Sig = sig("Morsel.__init__", &["self"], 0);
const SIG_MORSEL_SETITEM: Sig = sig("Morsel.__setitem__", &["self", "K", "V"], 0);
const SIG_MORSEL_SETDEFAULT: Sig = sig("Morsel.setdefault", &["self", "key", "val"], 1);
const SIG_MORSEL_EQ: Sig = sig("Morsel.__eq__", &["self", "morsel"], 0);
const SIG_MORSEL_COPY: Sig = sig("Morsel.copy", &["self"], 0);
const SIG_MORSEL_UPDATE: Sig = sig("Morsel.update", &["self", "values"], 0);
const SIG_MORSEL_IS_RESERVED: Sig = sig("Morsel.isReservedKey", &["self", "K"], 0);
const SIG_MORSEL_SET: Sig = sig("Morsel.set", &["self", "key", "val", "coded_val"], 0);
const SIG_MORSEL_GETSTATE: Sig = sig("Morsel.__getstate__", &["self"], 0);
const SIG_MORSEL_SETSTATE: Sig = sig("Morsel.__setstate__", &["self", "state"], 0);
const SIG_MORSEL_OUTPUT: Sig = sig("Morsel.output", &["self", "attrs", "header"], 2);
const SIG_MORSEL_REPR: Sig = sig("Morsel.__repr__", &["self"], 0);
const SIG_MORSEL_JS_OUTPUT: Sig = sig("Morsel.js_output", &["self", "attrs"], 1);
const SIG_MORSEL_OUTPUT_STRING: Sig = sig("Morsel.OutputString", &["self", "attrs"], 1);
const SIG_BASE_VALUE_DECODE: Sig = sig("BaseCookie.value_decode", &["self", "val"], 0);
const SIG_BASE_VALUE_ENCODE: Sig = sig("BaseCookie.value_encode", &["self", "val"], 0);
const SIG_BASE_INIT: Sig = sig("BaseCookie.__init__", &["self", "input"], 1);
const SIG_BASE_SET: Sig = sig(
    "BaseCookie.__set",
    &["self", "key", "real_value", "coded_value"],
    0,
);
const SIG_BASE_SETITEM: Sig = sig("BaseCookie.__setitem__", &["self", "key", "value"], 0);
const SIG_BASE_OUTPUT: Sig = sig("BaseCookie.output", &["self", "attrs", "header", "sep"], 3);
const SIG_BASE_REPR: Sig = sig("BaseCookie.__repr__", &["self"], 0);
const SIG_BASE_JS_OUTPUT: Sig = sig("BaseCookie.js_output", &["self", "attrs"], 1);
const SIG_BASE_LOAD: Sig = sig("BaseCookie.load", &["self", "rawdata"], 0);
const SIG_BASE_PARSE: Sig = sig("BaseCookie.__parse_string", &["self", "str", "patt"], 1);
const SIG_SIMPLE_VALUE_DECODE: Sig = sig("SimpleCookie.value_decode", &["self", "val"], 0);
const SIG_SIMPLE_VALUE_ENCODE: Sig = sig("SimpleCookie.value_encode", &["self", "val"], 0);

/// `string.ascii_letters + string.digits + "!#$%&'*+-.^_`|~:"`.
const LEGAL_CHARS: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&'*+-.^_`|~:";
/// `_LegalChars + ' ()/<=>?@[]{}'`.
const UNESCAPED_CHARS: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&'*+-.^_`|~: ()/<=>?@[]{}";
const LEGAL_KEY_CHARS: &str = r"\w\d!#%&'~_`><@,:/\$\*\+\-\.\^\|\)\(\?\}\{\=";
const LEGAL_VALUE_CHARS: &str = r"\w\d!#%&'~_`><@,:/\$\*\+\-\.\^\|\)\(\?\}\{\=\[\]";

const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn all_list() -> Value {
    Value::list(
        ["CookieError", "BaseCookie", "SimpleCookie"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

fn weekday_names() -> Value {
    Value::list(WEEKDAY_NAMES.into_iter().map(Value::string).collect())
}

fn month_names() -> Value {
    let mut names = vec![Value::none()];
    names.extend(MONTH_NAMES.into_iter().map(Value::string));
    Value::list(names)
}

/// `_Translator`: the `str.translate` table `_quote` uses.
fn translator() -> Value {
    let mut table = PyDict::default();
    for n in 0u32..256 {
        let is_unescaped = char::from_u32(n).is_some_and(|c| UNESCAPED_CHARS.contains(c));
        if !is_unescaped {
            let key = Value::int(i64::from(n)).to_key().expect("int key");
            table.insert(key, Value::string(format!("\\{n:03o}")));
        }
    }
    let quote = Value::int(i64::from(b'"')).to_key().expect("int key");
    table.insert(quote, Value::string("\\\""));
    let backslash = Value::int(i64::from(b'\\')).to_key().expect("int key");
    table.insert(backslash, Value::string("\\\\"));
    Value::dict(table)
}

/// `Morsel._reserved`.
fn reserved_dict() -> Value {
    let mut reserved = PyDict::default();
    for (key, name) in [
        ("expires", "expires"),
        ("path", "Path"),
        ("comment", "Comment"),
        ("domain", "Domain"),
        ("max-age", "Max-Age"),
        ("secure", "Secure"),
        ("httponly", "HttpOnly"),
        ("version", "Version"),
        ("samesite", "SameSite"),
    ] {
        reserved.insert(PyKey::str_from(key), Value::string(name));
    }
    Value::dict(reserved)
}

/// `Morsel._flags`.
fn flags_set() -> Value {
    let mut flags = PySet::default();
    flags.insert(PyKey::str_from("secure"));
    flags.insert(PyKey::str_from("httponly"));
    Value::set(flags)
}

// ---------------------------------------------------------------------------
// Interpreter plumbing
// ---------------------------------------------------------------------------

fn call_pos(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, pos)
}

fn call_builtin(interp: &mut Interpreter, name: &'static str, pos: &[Value]) -> Result<Value> {
    call_pos(interp, Value::builtin_function(name), pos)
}

/// A module global of `http.cookies` (`Morsel`, `_getdate`, ...), looked up at
/// call time like a Python global.
fn global(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("http.cookies")?;
    interp.get_attr(&module, name)
}

/// `dict.<method>(*args)`.
fn dict_base_call(interp: &mut Interpreter, method: &str, args: &[Value]) -> Result<Value> {
    let dict_cls = crate::interpreter::primitive_class_by_name("dict")
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime("missing dict class".to_string()))?;
    let function = interp.get_attr(&dict_cls, method)?;
    call_pos(interp, function, args)
}

fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

/// `a == b` as the Python value (not coerced to bool).
fn eq_value(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    interp.eval_binary(a.clone(), BinaryOp::Eq, b.clone())
}

/// `item in container`.
fn contains(interp: &mut Interpreter, item: &Value, container: &Value) -> Result<bool> {
    let result = interp.eval_binary(item.clone(), BinaryOp::In, container.clone())?;
    truthy(interp, &result)
}

fn is_exact_str(value: &Value) -> bool {
    matches!(value.kind(), ValueKind::Str(_))
}

/// `str(value)`.
fn str_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    if is_exact_str(value)
        && let Some(text) = value.as_str()
    {
        return Ok(text.to_string());
    }
    let result = call_builtin(interp, "str", std::slice::from_ref(value))?;
    Ok(result.as_str().unwrap_or_default().to_string())
}

/// `repr(value)`.
fn repr_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let result = call_builtin(interp, "repr", std::slice::from_ref(value))?;
    Ok(result.as_str().unwrap_or_default().to_string())
}

/// `isinstance(value, cls)`.
fn is_instance(interp: &mut Interpreter, value: &Value, cls: &Value) -> Result<bool> {
    let result = call_builtin(interp, "isinstance", &[value.clone(), cls.clone()])?;
    truthy(interp, &result)
}

fn primitive(name: &str) -> Result<Value> {
    crate::interpreter::primitive_class_by_name(name)
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime(format!("missing {name} class")))
}

/// `isinstance(value, int)` (`bool` and `int` subclasses included).
fn is_int(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    let int = primitive("int")?;
    is_instance(interp, value, &int)
}

/// `isinstance(value, str)`.
fn is_str(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    let string = primitive("str")?;
    is_instance(interp, value, &string)
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

/// `a, b = value`.
fn unpack_pair(interp: &mut Interpreter, value: &Value) -> Result<(Value, Value)> {
    let mut items = interp.collect_iterable(value)?;
    match items.len() {
        2 => {
            let second = items.pop().expect("two items");
            let first = items.pop().expect("two items");
            Ok((first, second))
        }
        n if n < 2 => Err(value_error(format!(
            "not enough values to unpack (expected 2, got {n})"
        ))),
        _ => Err(value_error("too many values to unpack (expected 2)")),
    }
}

/// `sorted(obj.items())`, unpacked into `(key, value)` pairs.
fn sorted_items(interp: &mut Interpreter, obj: &Value) -> Result<Vec<(Value, Value)>> {
    let items = call_method(interp, obj, "items", &[])?;
    let sorted = call_builtin(interp, "sorted", &[items])?;
    let entries = interp.collect_iterable(&sorted)?;
    let mut pairs = Vec::with_capacity(entries.len());
    for entry in &entries {
        pairs.push(unpack_pair(interp, entry)?);
    }
    Ok(pairs)
}

/// `"Invalid attribute %r" % (key,)`.
fn invalid_attribute(interp: &mut Interpreter, key: &Value) -> PyError {
    match repr_of(interp, key) {
        Ok(text) => cookie_error(format!("Invalid attribute {text}")),
        Err(err) => err,
    }
}

/// `key.lower()`.
fn lower(interp: &mut Interpreter, key: &Value) -> Result<Value> {
    call_method(interp, key, "lower", &[])
}

// ---------------------------------------------------------------------------
// _quote / _unquote / _getdate
// ---------------------------------------------------------------------------

fn is_legal_char(c: char) -> bool {
    LEGAL_CHARS.contains(c)
}

/// `_is_legal_key(value)` for the native callers.  Exact strings are scanned;
/// anything else goes through the module's `_is_legal_key`, which raises
/// `re`'s own `TypeError`.
fn is_legal_key(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if is_exact_str(value)
        && let Some(text) = value.as_str()
    {
        return Ok(!text.is_empty() && text.chars().all(is_legal_char));
    }
    let function = global(interp, "_is_legal_key")?;
    let result = call_pos(interp, function, std::slice::from_ref(value))?;
    truthy(interp, &result)
}

/// `_quote(str)`.
fn quote_impl(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    if value.is_none() || is_legal_key(interp, value)? {
        return Ok(value.clone());
    }
    let table = global(interp, "_Translator")?;
    let translated = call_method(interp, value, "translate", &[table])?;
    let opening = interp.eval_binary(Value::string("\""), BinaryOp::Add, translated)?;
    interp.eval_binary(opening, BinaryOp::Add, Value::string("\""))
}

/// The body of `_unquote` once the surrounding double quotes are removed:
/// `\ooo` octal escapes (first digit 0-3) and `\<char>` quote pairs.  A
/// backslash followed by a newline (or at the end) is kept literally, like
/// the `[\\].` regex that does not match `\n`.
fn unquote_body(chars: &[char]) -> String {
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let mut found = None;
        let mut p = i;
        while p + 1 < n {
            if chars[p] == '\\' && chars[p + 1] != '\n' {
                found = Some(p);
                break;
            }
            p += 1;
        }
        let Some(p) = found else {
            out.extend(&chars[i..]);
            break;
        };
        out.extend(&chars[i..p]);
        let octal = p + 3 < n
            && matches!(chars[p + 1], '0'..='3')
            && matches!(chars[p + 2], '0'..='7')
            && matches!(chars[p + 3], '0'..='7');
        if octal {
            let digit = |c: char| c.to_digit(8).unwrap_or(0);
            let code = digit(chars[p + 1]) * 64 + digit(chars[p + 2]) * 8 + digit(chars[p + 3]);
            out.push(char::from_u32(code).unwrap_or('\u{0}'));
            i = p + 4;
        } else {
            out.push(chars[p + 1]);
            i = p + 2;
        }
    }
    out
}

/// `_unquote(str)`.
fn unquote_impl(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    if value.is_none() {
        return Ok(value.clone());
    }
    let length = call_builtin(interp, "len", std::slice::from_ref(value))?;
    if length.as_int().is_some_and(|len| len < 2) {
        return Ok(value.clone());
    }
    if is_exact_str(value)
        && let Some(text) = value.as_str()
    {
        let chars: Vec<char> = text.chars().collect();
        if chars.first() != Some(&'"') || chars.last() != Some(&'"') {
            return Ok(value.clone());
        }
        return Ok(Value::string(unquote_body(&chars[1..chars.len() - 1])));
    }
    let quote = Value::string("\"");
    let first =
        crate::builtin_modules::operator::getitem_value(interp, value.clone(), Value::int(0))?;
    let last =
        crate::builtin_modules::operator::getitem_value(interp, value.clone(), Value::int(-1))?;
    for end in [first, last] {
        let differs = interp.eval_binary(end, BinaryOp::Ne, quote.clone())?;
        if truthy(interp, &differs)? {
            return Ok(value.clone());
        }
    }
    // A non-string sequence that starts and ends with `'"'`: `re.sub` rejects it.
    let text = str_of(interp, value)?;
    let chars: Vec<char> = text.chars().collect();
    Ok(Value::string(unquote_body(&chars[1..chars.len() - 1])))
}

/// `_getdate(future, weekdayname, monthname)`.
fn getdate_impl(
    interp: &mut Interpreter,
    future: Value,
    weekdayname: Value,
    monthname: Value,
) -> Result<Value> {
    let time = interp.load_module("time")?;
    let gmtime = interp.get_attr(&time, "gmtime")?;
    let clock = interp.get_attr(&time, "time")?;
    let now = call_pos(interp, clock, &[])?;
    let when = interp.eval_binary(now, BinaryOp::Add, future)?;
    let parts = call_pos(interp, gmtime, &[when])?;
    let fields = interp.collect_iterable(&parts)?;
    if fields.len() != 9 {
        return Err(if fields.len() < 9 {
            value_error(format!(
                "not enough values to unpack (expected 9, got {})",
                fields.len()
            ))
        } else {
            value_error("too many values to unpack (expected 9)")
        });
    }
    let int = |i: usize| fields[i].as_int().unwrap_or(0);
    let (year, month, day, hh, mm, ss, wd) =
        (int(0), int(1), int(2), int(3), int(4), int(5), int(6));
    let weekday =
        crate::builtin_modules::operator::getitem_value(interp, weekdayname, Value::int(wd))?;
    let weekday = str_of(interp, &weekday)?;
    let month_name =
        crate::builtin_modules::operator::getitem_value(interp, monthname, Value::int(month))?;
    let month_name = str_of(interp, &month_name)?;
    Ok(Value::string(format!(
        "{weekday}, {day:02} {month_name:>3} {year:>4} {hh:02}:{mm:02}:{ss:02} GMT"
    )))
}

// ---------------------------------------------------------------------------
// Morsel
// ---------------------------------------------------------------------------

/// A new `Morsel()` of the newest `http.cookies.Morsel`.
fn new_morsel(interp: &mut Interpreter) -> Result<Value> {
    let class = global(interp, "Morsel")?;
    call_pos(interp, class, &[])
}

fn morsel_init(interp: &mut Interpreter, this: &Value) -> Result<()> {
    for name in ["_key", "_value", "_coded_value"] {
        interp.assign_attr(this.clone(), name, Value::none())?;
    }
    let reserved = interp.get_attr(this, "_reserved")?;
    for key in interp.collect_iterable(&reserved)? {
        dict_base_call(
            interp,
            "__setitem__",
            &[this.clone(), key, Value::string("")],
        )?;
    }
    Ok(())
}

/// `K = K.lower(); if K not in self._reserved: raise CookieError(...)`.
fn checked_attribute(interp: &mut Interpreter, this: &Value, key: &Value) -> Result<Value> {
    let key = lower(interp, key)?;
    let reserved = interp.get_attr(this, "_reserved")?;
    if !contains(interp, &key, &reserved)? {
        return Err(invalid_attribute(interp, &key));
    }
    Ok(key)
}

fn morsel_setitem(interp: &mut Interpreter, this: &Value, key: &Value, value: Value) -> Result<()> {
    let key = checked_attribute(interp, this, key)?;
    dict_base_call(interp, "__setitem__", &[this.clone(), key, value])?;
    Ok(())
}

fn morsel_setdefault(
    interp: &mut Interpreter,
    this: &Value,
    key: &Value,
    value: Value,
) -> Result<Value> {
    let key = checked_attribute(interp, this, key)?;
    dict_base_call(interp, "setdefault", &[this.clone(), key, value])
}

fn morsel_eq(interp: &mut Interpreter, this: &Value, other: &Value) -> Result<Value> {
    let morsel = global(interp, "Morsel")?;
    if !is_instance(interp, other, &morsel)? {
        return Ok(Value::not_implemented());
    }
    let mut result = dict_base_call(interp, "__eq__", &[this.clone(), other.clone()])?;
    for name in ["_value", "_key", "_coded_value"] {
        if !truthy(interp, &result)? {
            return Ok(result);
        }
        let mine = interp.get_attr(this, name)?;
        let theirs = interp.get_attr(other, name)?;
        result = eq_value(interp, &mine, &theirs)?;
    }
    Ok(result)
}

fn morsel_copy(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let morsel = new_morsel(interp)?;
    dict_base_call(interp, "update", &[morsel.clone(), this.clone()])?;
    let target = interp.get_attr(&morsel, "__dict__")?;
    // A live instance `__dict__` is not accepted as `update()`'s argument, so
    // hand over a snapshot of it.
    let source = interp.get_attr(this, "__dict__")?;
    let snapshot = call_method(interp, &source, "copy", &[])?;
    call_method(interp, &target, "update", &[snapshot])?;
    Ok(morsel)
}

fn morsel_update(interp: &mut Interpreter, this: &Value, values: Value) -> Result<()> {
    let mut data = PyDict::default();
    let as_dict = call_builtin(interp, "dict", &[values])?;
    let items = call_method(interp, &as_dict, "items", &[])?;
    for entry in interp.collect_iterable(&items)? {
        let (key, value) = unpack_pair(interp, &entry)?;
        let key = lower(interp, &key)?;
        let reserved = interp.get_attr(this, "_reserved")?;
        if !contains(interp, &key, &reserved)? {
            return Err(invalid_attribute(interp, &key));
        }
        let py_key = interp.value_to_pykey(&key)?;
        data.insert(py_key, value);
    }
    dict_base_call(interp, "update", &[this.clone(), Value::dict(data)])?;
    Ok(())
}

fn morsel_set(
    interp: &mut Interpreter,
    this: &Value,
    key: &Value,
    value: Value,
    coded_value: Value,
) -> Result<()> {
    let lowered = lower(interp, key)?;
    let reserved = interp.get_attr(this, "_reserved")?;
    if contains(interp, &lowered, &reserved)? {
        let text = repr_of(interp, key)?;
        return Err(cookie_error(format!(
            "Attempt to set a reserved key {text}"
        )));
    }
    if !is_legal_key(interp, key)? {
        let text = repr_of(interp, key)?;
        return Err(cookie_error(format!("Illegal key {text}")));
    }
    interp.assign_attr(this.clone(), "_key", key.clone())?;
    interp.assign_attr(this.clone(), "_value", value)?;
    interp.assign_attr(this.clone(), "_coded_value", coded_value)?;
    Ok(())
}

fn morsel_getstate(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let mut state = PyDict::default();
    for (name, attr) in [
        ("key", "_key"),
        ("value", "_value"),
        ("coded_value", "_coded_value"),
    ] {
        let value = interp.get_attr(this, attr)?;
        state.insert(PyKey::str_from(name), value);
    }
    Ok(Value::dict(state))
}

fn morsel_setstate(interp: &mut Interpreter, this: &Value, state: &Value) -> Result<()> {
    for (name, attr) in [
        ("key", "_key"),
        ("value", "_value"),
        ("coded_value", "_coded_value"),
    ] {
        let value = crate::builtin_modules::operator::getitem_value(
            interp,
            state.clone(),
            Value::string(name),
        )?;
        interp.assign_attr(this.clone(), attr, value)?;
    }
    Ok(())
}

/// `self.OutputString(attrs)`.
fn morsel_output_string(interp: &mut Interpreter, this: &Value, attrs: Value) -> Result<Value> {
    let mut result: Vec<String> = Vec::new();
    let key = interp.get_attr(this, "key")?;
    let coded_value = interp.get_attr(this, "coded_value")?;
    let key = str_of(interp, &key)?;
    let coded_value = str_of(interp, &coded_value)?;
    result.push(format!("{key}={coded_value}"));

    let reserved = interp.get_attr(this, "_reserved")?;
    let attrs = if attrs.is_none() {
        reserved.clone()
    } else {
        attrs
    };
    let empty = Value::string("");
    for (key, value) in sorted_items(interp, this)? {
        let is_empty = eq_value(interp, &value, &empty)?;
        if truthy(interp, &is_empty)? {
            continue;
        }
        if !contains(interp, &key, &attrs)? {
            continue;
        }
        let key_name = key.as_str().map(str::to_string).unwrap_or_default();
        let name =
            crate::builtin_modules::operator::getitem_value(interp, reserved.clone(), key.clone())?;
        let name_text = str_of(interp, &name)?;
        if key_name == "expires" && is_int(interp, &value)? {
            let getdate = global(interp, "_getdate")?;
            let date = call_pos(interp, getdate, &[value])?;
            let date = str_of(interp, &date)?;
            result.push(format!("{name_text}={date}"));
            continue;
        }
        if key_name == "max-age" && is_int(interp, &value)? {
            let number = call_builtin(interp, "int", &[value])?;
            let number = str_of(interp, &number)?;
            result.push(format!("{name_text}={number}"));
            continue;
        }
        if key_name == "comment" && is_str(interp, &value)? {
            let quote = global(interp, "_quote")?;
            let quoted = call_pos(interp, quote, &[value])?;
            let quoted = str_of(interp, &quoted)?;
            result.push(format!("{name_text}={quoted}"));
            continue;
        }
        let flags = interp.get_attr(this, "_flags")?;
        if contains(interp, &key, &flags)? {
            if truthy(interp, &value)? {
                result.push(name_text);
            }
            continue;
        }
        let value_text = str_of(interp, &value)?;
        result.push(format!("{name_text}={value_text}"));
    }
    Ok(Value::string(result.join("; ")))
}

fn morsel_output(
    interp: &mut Interpreter,
    this: &Value,
    attrs: Value,
    header: &Value,
) -> Result<Value> {
    let output = call_method(interp, this, "OutputString", &[attrs])?;
    let header = str_of(interp, header)?;
    let output = str_of(interp, &output)?;
    Ok(Value::string(format!("{header} {output}")))
}

fn morsel_repr(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let class = interp.get_attr(this, "__class__")?;
    let name = interp.get_attr(&class, "__name__")?;
    let name = str_of(interp, &name)?;
    let output = call_method(interp, this, "OutputString", &[])?;
    let output = str_of(interp, &output)?;
    Ok(Value::string(format!("<{name}: {output}>")))
}

fn morsel_js_output(interp: &mut Interpreter, this: &Value, attrs: Value) -> Result<Value> {
    let output = call_method(interp, this, "OutputString", &[attrs])?;
    let escaped = call_method(
        interp,
        &output,
        "replace",
        &[Value::string("\""), Value::string("\\\"")],
    )?;
    let escaped = str_of(interp, &escaped)?;
    Ok(Value::string(format!(
        "\n        <script type=\"text/javascript\">\n        <!-- begin hiding\n        document.cookie = \"{escaped}\";\n        // end hiding -->\n        </script>\n        "
    )))
}

/// `Morsel.__class_getitem__ = classmethod(types.GenericAlias)`.
fn class_getitem(args: &[ExpandedCallArg]) -> Result<Value> {
    // `types.GenericAlias(cls, item)`: the class is bound as its first argument.
    if args.iter().any(|arg| arg.name.is_some()) {
        return Err(PyError::named(
            "TypeError",
            "GenericAlias takes no keyword arguments".to_string(),
        ));
    }
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("GenericAlias expected 2 arguments, got {}", args.len()),
        ));
    }
    let (cls, rest) = (&args[0], &args[1..]);
    let item = &rest[0].value;
    let type_arguments = if matches!(item.kind(), ValueKind::Tuple(_)) {
        item.clone()
    } else {
        Value::tuple(vec![item.clone()])
    };
    Ok(pyrust_builtins::generic_alias::generic_alias(
        cls.value.clone(),
        type_arguments,
    ))
}

// ---------------------------------------------------------------------------
// The cookie-string scanner (`_CookiePattern`)
// ---------------------------------------------------------------------------

/// `\s` under `re.ASCII`.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// `\w` under `re.ASCII`.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `[_LegalKeyChars]`.
fn is_key_char(c: char) -> bool {
    is_word(c) || "!#%&'~`><@,:/$*+-.^|)(?}{=".contains(c)
}

/// `[_LegalValueChars]`.
fn is_value_char(c: char) -> bool {
    is_key_char(c) || c == '[' || c == ']'
}

/// One `_CookiePattern.match(...)`: the character ranges of the `key` group
/// and of the `val` group (`None` when the optional value group did not
/// participate), and the end of the whole match.
#[derive(Debug, PartialEq, Eq)]
struct CookieMatch {
    key: (usize, usize),
    val: Option<(usize, usize)>,
    end: usize,
}

/// `\s*(\s+|;|$)` at `pos`: the end of the match, if any.  The greedy `\s*`
/// takes the whole whitespace run, so `;` or the end of the string are tried
/// first, and a non-empty run is accepted by backtracking into `\s+`.
fn match_tail(s: &[char], pos: usize) -> Option<usize> {
    let n = s.len();
    let mut p = pos;
    while p < n && is_space(s[p]) {
        p += 1;
    }
    if p < n && s[p] == ';' {
        Some(p + 1)
    } else if p == n || p > pos {
        Some(p)
    } else {
        None
    }
}

/// `"(?:[^\\"]|\\.)*"` at `pos`: the end of the quoted string.
fn match_quoted(s: &[char], pos: usize) -> Option<usize> {
    let n = s.len();
    if pos >= n || s[pos] != '"' {
        return None;
    }
    let mut i = pos + 1;
    while i < n {
        match s[i] {
            '"' => return Some(i + 1),
            '\\' => {
                if i + 1 >= n || s[i + 1] == '\n' {
                    return None;
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    None
}

/// `\w{3},\s[\w\d\s-]{9,11}\s[\d:]{8}\sGMT` at `pos`, trying the greedy
/// middle run from its longest length down to its shortest.  Yields every
/// candidate end in the regex's priority order.
fn match_expires_date(s: &[char], pos: usize) -> Vec<usize> {
    let n = s.len();
    let at = |i: usize| if i < n { Some(s[i]) } else { None };
    let mut ends = Vec::new();
    if !(0..3).all(|k| at(pos + k).is_some_and(is_word))
        || at(pos + 3) != Some(',')
        || !at(pos + 4).is_some_and(is_space)
    {
        return ends;
    }
    let mid_start = pos + 5;
    let mut run = 0;
    while run < 11 && at(mid_start + run).is_some_and(|c| is_word(c) || is_space(c) || c == '-') {
        run += 1;
    }
    let mut mid = run;
    while mid >= 9 {
        let t = mid_start + mid;
        let digits = (0..8).all(|k| at(t + 1 + k).is_some_and(|c| c.is_ascii_digit() || c == ':'));
        if at(t).is_some_and(is_space)
            && digits
            && at(t + 9).is_some_and(is_space)
            && at(t + 10) == Some('G')
            && at(t + 11) == Some('M')
            && at(t + 12) == Some('T')
        {
            ends.push(t + 13);
        }
        mid -= 1;
    }
    ends
}

/// The `val` group at `pos` followed by the tail: its range and the end of
/// the whole match.
fn match_value(s: &[char], pos: usize) -> Option<((usize, usize), usize)> {
    if let Some(close) = match_quoted(s, pos)
        && let Some(end) = match_tail(s, close)
    {
        return Some(((pos, close), end));
    }
    for close in match_expires_date(s, pos) {
        if let Some(end) = match_tail(s, close) {
            return Some(((pos, close), end));
        }
    }
    let mut longest = pos;
    while longest < s.len() && is_value_char(s[longest]) {
        longest += 1;
    }
    (pos..=longest)
        .rev()
        .find_map(|close| match_tail(s, close).map(|end| ((pos, close), end)))
}

/// `\s*=\s*(?P<val>...)` followed by the tail, after a key ending at `key_end`.
fn match_value_group(s: &[char], key_end: usize) -> Option<((usize, usize), usize)> {
    let n = s.len();
    let mut p = key_end;
    while p < n && is_space(s[p]) {
        p += 1;
    }
    if p >= n || s[p] != '=' {
        return None;
    }
    p += 1;
    let mut after = p;
    while after < n && is_space(s[after]) {
        after += 1;
    }
    (p..=after)
        .rev()
        .find_map(|value_start| match_value(s, value_start))
}

/// `_CookiePattern.match(s, start)`.
fn match_cookie(s: &[char], start: usize) -> Option<CookieMatch> {
    let n = s.len();
    let mut key_start = start;
    while key_start < n && is_space(s[key_start]) {
        key_start += 1;
    }
    let mut key_end = key_start;
    loop {
        if key_end >= n || !is_key_char(s[key_end]) {
            return None;
        }
        key_end += 1;
        if let Some((val, end)) = match_value_group(s, key_end) {
            return Some(CookieMatch {
                key: (key_start, key_end),
                val: Some(val),
                end,
            });
        }
        if let Some(end) = match_tail(s, key_end) {
            return Some(CookieMatch {
                key: (key_start, key_end),
                val: None,
                end,
            });
        }
    }
}

enum Parsed {
    Attribute(String, Value),
    KeyValue(String, Value),
}

/// `BaseCookie.__parse_string`.
fn parse_string(interp: &mut Interpreter, this: &Value, text: &str) -> Result<()> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let slice = |range: (usize, usize)| -> String { chars[range.0..range.1].iter().collect() };
    let morsel = global(interp, "Morsel")?;
    let reserved = interp.get_attr(&morsel, "_reserved")?;
    let flags = interp.get_attr(&morsel, "_flags")?;
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut morsel_seen = false;
    let mut i = 0;
    while i < n {
        let Some(found) = match_cookie(&chars, i) else {
            break;
        };
        let key = slice(found.key);
        let value = found.val.map(slice);
        i = found.end;
        let lowered = Value::string(key.to_lowercase());
        if let Some(attribute) = key.strip_prefix('$') {
            if !morsel_seen {
                continue;
            }
            let value = value.map_or_else(Value::none, Value::string);
            parsed.push(Parsed::Attribute(attribute.to_string(), value));
        } else if contains(interp, &lowered, &reserved)? {
            if !morsel_seen {
                return Ok(());
            }
            match value {
                None => {
                    if contains(interp, &lowered, &flags)? {
                        parsed.push(Parsed::Attribute(key, Value::bool_(true)));
                    } else {
                        return Ok(());
                    }
                }
                Some(value) => {
                    let value = unquote_impl(interp, &Value::string(value))?;
                    parsed.push(Parsed::Attribute(key, value));
                }
            }
        } else if let Some(value) = value {
            let decoded = call_method(interp, this, "value_decode", &[Value::string(value)])?;
            parsed.push(Parsed::KeyValue(key, decoded));
            morsel_seen = true;
        } else {
            return Ok(());
        }
    }

    let mut current: Option<Value> = None;
    for item in parsed {
        match item {
            Parsed::Attribute(key, value) => {
                let Some(target) = &current else {
                    return Err(PyError::named("AssertionError", String::new()));
                };
                let setitem = interp.get_attr(target, "__setitem__")?;
                call_pos(interp, setitem, &[Value::string(key), value])?;
            }
            Parsed::KeyValue(key, decoded) => {
                let (real_value, coded_value) = unpack_pair(interp, &decoded)?;
                call_method(
                    interp,
                    this,
                    "_BaseCookie__set",
                    &[Value::string(key.clone()), real_value, coded_value],
                )?;
                current = Some(crate::builtin_modules::operator::getitem_value(
                    interp,
                    this.clone(),
                    Value::string(key),
                )?);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// BaseCookie
// ---------------------------------------------------------------------------

fn base_set(
    interp: &mut Interpreter,
    this: &Value,
    key: Value,
    real_value: Value,
    coded_value: Value,
) -> Result<()> {
    let fresh = new_morsel(interp)?;
    let morsel = call_method(interp, this, "get", &[key.clone(), fresh])?;
    call_method(
        interp,
        &morsel,
        "set",
        &[key.clone(), real_value, coded_value],
    )?;
    dict_base_call(interp, "__setitem__", &[this.clone(), key, morsel])?;
    Ok(())
}

fn base_setitem(interp: &mut Interpreter, this: &Value, key: Value, value: Value) -> Result<()> {
    let morsel = global(interp, "Morsel")?;
    if is_instance(interp, &value, &morsel)? {
        dict_base_call(interp, "__setitem__", &[this.clone(), key, value])?;
        return Ok(());
    }
    let encoded = call_method(interp, this, "value_encode", &[value])?;
    let (real_value, coded_value) = unpack_pair(interp, &encoded)?;
    call_method(
        interp,
        this,
        "_BaseCookie__set",
        &[key, real_value, coded_value],
    )?;
    Ok(())
}

fn base_output(
    interp: &mut Interpreter,
    this: &Value,
    attrs: Value,
    header: Value,
    sep: &Value,
) -> Result<Value> {
    let mut result = Vec::new();
    for (_key, value) in sorted_items(interp, this)? {
        result.push(call_method(
            interp,
            &value,
            "output",
            &[attrs.clone(), header.clone()],
        )?);
    }
    call_method(interp, sep, "join", &[Value::list(result)])
}

fn base_repr(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let mut parts = Vec::new();
    for (key, value) in sorted_items(interp, this)? {
        let key = str_of(interp, &key)?;
        let inner = interp.get_attr(&value, "value")?;
        let inner = repr_of(interp, &inner)?;
        parts.push(format!("{key}={inner}"));
    }
    let class = interp.get_attr(this, "__class__")?;
    let name = interp.get_attr(&class, "__name__")?;
    let name = str_of(interp, &name)?;
    Ok(Value::string(format!("<{name}: {}>", parts.join(" "))))
}

fn base_js_output(interp: &mut Interpreter, this: &Value, attrs: Value) -> Result<Value> {
    let mut result = String::new();
    for (_key, value) in sorted_items(interp, this)? {
        let part = call_method(interp, &value, "js_output", std::slice::from_ref(&attrs))?;
        result.push_str(&str_of(interp, &part)?);
    }
    Ok(Value::string(result))
}

fn base_load(interp: &mut Interpreter, this: &Value, rawdata: &Value) -> Result<()> {
    if is_str(interp, rawdata)? {
        let text = str_of(interp, rawdata)?;
        let parse = interp.get_attr(this, "_BaseCookie__parse_string")?;
        call_pos(interp, parse, &[Value::string(text)])?;
        return Ok(());
    }
    let items = call_method(interp, rawdata, "items", &[])?;
    for entry in interp.collect_iterable(&items)? {
        let (key, value) = unpack_pair(interp, &entry)?;
        let setitem = interp.get_attr(this, "__setitem__")?;
        call_pos(interp, setitem, &[key, value])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Module wiring
// ---------------------------------------------------------------------------

/// Wire what the CPython source gets from its imports and class bodies: the
/// `string` / `types` globals, the `''.join` shorthands, `Morsel.__ne__ =
/// object.__ne__` and the `__str__ = output` aliases, and record the
/// `CookieError` class for native raises.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    for name in ["string", "types"] {
        if let Ok(dep) = interp.load_module(name) {
            module_rc.borrow_mut().insert_attr(name.to_string(), dep);
        }
    }
    for (name, separator) in [
        ("_nulljoin", ""),
        ("_semispacejoin", "; "),
        ("_spacejoin", " "),
    ] {
        if let Ok(join) = interp.get_attr(&Value::string(separator), "join") {
            module_rc.borrow_mut().insert_attr(name.to_string(), join);
        }
    }
    let error = module_rc.borrow().attrs.get("CookieError").cloned();
    if let Some(error) = error
        && let ValueKind::PyClass(error) = error.kind()
    {
        COOKIE_ERRORS.with(|errors| errors.borrow_mut().push(error.clone()));
        // `class CookieError(Exception): pass` has no docstring.
        error
            .borrow_mut()
            .attrs
            .insert("__doc__".to_string(), Value::none());
    }
    let object_ne = primitive("object")
        .ok()
        .and_then(|object| interp.get_attr(&object, "__ne__").ok());
    for class_name in ["Morsel", "BaseCookie"] {
        let class = module_rc.borrow().attrs.get(class_name).cloned();
        let Some(class) = class else { continue };
        let ValueKind::PyClass(class_rc) = class.kind() else {
            continue;
        };
        let output = class_rc.borrow().attrs.get("output").cloned();
        if let Some(output) = output {
            class_rc
                .borrow_mut()
                .attrs
                .insert("__str__".to_string(), output);
        }
        if class_name == "Morsel"
            && let Some(object_ne) = &object_ne
        {
            class_rc
                .borrow_mut()
                .attrs
                .insert("__ne__".to_string(), object_ne.clone());
        }
        class_rc.borrow().bump_mutation_version();
    }
}

pyrust_module! {
    exception CookieError;

    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "__all__" => all_list(),
        "_LegalChars" => Value::string(LEGAL_CHARS),
        "_UnescapedChars" => Value::string(UNESCAPED_CHARS),
        "_Translator" => translator(),
        "_weekdayname" => weekday_names(),
        "_monthname" => month_names(),
        "_LegalKeyChars" => Value::string(LEGAL_KEY_CHARS),
        "_LegalValueChars" => Value::string(LEGAL_VALUE_CHARS),
    }

    /// `_is_legal_key = re.compile('[%s]+' % re.escape(_LegalChars)).fullmatch`.
    fn _is_legal_key(args) -> Result<Value> {
        let re = _interp.load_module("re")?;
        let escape = _interp.get_attr(&re, "escape")?;
        let legal = global(_interp, "_LegalChars")?;
        let escaped = call_pos(_interp, escape, &[legal])?;
        let escaped = str_of(_interp, &escaped)?;
        let compile = _interp.get_attr(&re, "compile")?;
        let pattern = call_pos(_interp, compile, &[Value::string(format!("[{escaped}]+"))])?;
        let fullmatch = _interp.get_attr(&pattern, "fullmatch")?;
        _interp.call_function_expanded(fullmatch, args)
    }

    fn _quote(args) -> Result<Value> {
        let mut b = args::bind(&SIG_QUOTE, args)?;
        let value = b.take_or_none(0);
        quote_impl(_interp, &value)
    }

    fn _unquote(args) -> Result<Value> {
        let mut b = args::bind(&SIG_UNQUOTE, args)?;
        let value = b.take_or_none(0);
        unquote_impl(_interp, &value)
    }

    fn _getdate(args) -> Result<Value> {
        let mut b = args::bind(&SIG_GETDATE, args)?;
        let future = b.take_or(0, || Value::int(0));
        let weekdayname = match b.take(1) {
            Some(names) => names,
            None => global(_interp, "_weekdayname")?,
        };
        let monthname = match b.take(2) {
            Some(names) => names,
            None => global(_interp, "_monthname")?,
        };
        getdate_impl(_interp, future, weekdayname, monthname)
    }

    class Morsel("dict") {
        constants {
            "__doc__" => Value::string(docs::MORSEL_DOC),
            "_reserved" => reserved_dict(),
            "_flags" => flags_set(),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_INIT, args)?;
            let this = b.take_or_none(0);
            morsel_init(_interp, &this)?;
            Ok(Value::none())
        }

        #[getter]
        fn key(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_key")
        }

        #[getter]
        fn value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_value")
        }

        #[getter]
        fn coded_value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_coded_value")
        }

        fn __setitem__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_SETITEM, args)?;
            let this = b.take_or_none(0);
            let key = b.take_or_none(1);
            let value = b.take_or_none(2);
            morsel_setitem(_interp, &this, &key, value)?;
            Ok(Value::none())
        }

        fn setdefault(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_SETDEFAULT, args)?;
            let this = b.take_or_none(0);
            let key = b.take_or_none(1);
            let value = b.take_or_none(2);
            morsel_setdefault(_interp, &this, &key, value)
        }

        fn __eq__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_EQ, args)?;
            let this = b.take_or_none(0);
            let other = b.take_or_none(1);
            morsel_eq(_interp, &this, &other)
        }

        fn copy(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_COPY, args)?;
            let this = b.take_or_none(0);
            morsel_copy(_interp, &this)
        }

        fn update(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_UPDATE, args)?;
            let this = b.take_or_none(0);
            let values = b.take_or_none(1);
            morsel_update(_interp, &this, values)?;
            Ok(Value::none())
        }

        #[py_name = "isReservedKey"]
        fn is_reserved_key(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_IS_RESERVED, args)?;
            let this = b.take_or_none(0);
            let key = b.take_or_none(1);
            let lowered = lower(_interp, &key)?;
            let reserved = _interp.get_attr(&this, "_reserved")?;
            Ok(Value::bool_(contains(_interp, &lowered, &reserved)?))
        }

        fn set(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_SET, args)?;
            let this = b.take_or_none(0);
            let key = b.take_or_none(1);
            let value = b.take_or_none(2);
            let coded_value = b.take_or_none(3);
            morsel_set(_interp, &this, &key, value, coded_value)?;
            Ok(Value::none())
        }

        fn __getstate__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_GETSTATE, args)?;
            let this = b.take_or_none(0);
            morsel_getstate(_interp, &this)
        }

        fn __setstate__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_SETSTATE, args)?;
            let this = b.take_or_none(0);
            let state = b.take_or_none(1);
            morsel_setstate(_interp, &this, &state)?;
            Ok(Value::none())
        }

        fn output(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_OUTPUT, args)?;
            let this = b.take_or_none(0);
            let attrs = b.take_or_none(1);
            let header = b.take_or(2, || Value::string("Set-Cookie:"));
            morsel_output(_interp, &this, attrs, &header)
        }

        fn __repr__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_REPR, args)?;
            let this = b.take_or_none(0);
            morsel_repr(_interp, &this)
        }

        fn js_output(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_JS_OUTPUT, args)?;
            let this = b.take_or_none(0);
            let attrs = b.take_or_none(1);
            morsel_js_output(_interp, &this, attrs)
        }

        #[py_name = "OutputString"]
        fn output_string(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MORSEL_OUTPUT_STRING, args)?;
            let this = b.take_or_none(0);
            let attrs = b.take_or_none(1);
            morsel_output_string(_interp, &this, attrs)
        }

        #[classmethod]
        fn __class_getitem__(args) -> Result<Value> {
            class_getitem(args)
        }
    }

    class BaseCookie("dict") {
        constants {
            "__doc__" => Value::string(docs::BASE_COOKIE_DOC),
        }

        fn value_decode(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_VALUE_DECODE, args)?;
            let _this = b.take_or_none(0);
            let value = b.take_or_none(1);
            Ok(Value::tuple(vec![value.clone(), value]))
        }

        fn value_encode(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_VALUE_ENCODE, args)?;
            let _this = b.take_or_none(0);
            let value = b.take_or_none(1);
            let text = Value::string(str_of(_interp, &value)?);
            Ok(Value::tuple(vec![text.clone(), text]))
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_INIT, args)?;
            let this = b.take_or_none(0);
            let input = b.take_or_none(1);
            if truthy(_interp, &input)? {
                call_method(_interp, &this, "load", &[input])?;
            }
            Ok(Value::none())
        }

        #[py_name = "_BaseCookie__set"]
        fn base_cookie_set(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_SET, args)?;
            let this = b.take_or_none(0);
            let key = b.take_or_none(1);
            let real_value = b.take_or_none(2);
            let coded_value = b.take_or_none(3);
            base_set(_interp, &this, key, real_value, coded_value)?;
            Ok(Value::none())
        }

        fn __setitem__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_SETITEM, args)?;
            let this = b.take_or_none(0);
            let key = b.take_or_none(1);
            let value = b.take_or_none(2);
            base_setitem(_interp, &this, key, value)?;
            Ok(Value::none())
        }

        fn output(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_OUTPUT, args)?;
            let this = b.take_or_none(0);
            let attrs = b.take_or_none(1);
            let header = b.take_or(2, || Value::string("Set-Cookie:"));
            let sep = b.take_or(3, || Value::string("\r\n"));
            base_output(_interp, &this, attrs, header, &sep)
        }

        fn __repr__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_REPR, args)?;
            let this = b.take_or_none(0);
            base_repr(_interp, &this)
        }

        fn js_output(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_JS_OUTPUT, args)?;
            let this = b.take_or_none(0);
            let attrs = b.take_or_none(1);
            base_js_output(_interp, &this, attrs)
        }

        fn load(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_LOAD, args)?;
            let this = b.take_or_none(0);
            let rawdata = b.take_or_none(1);
            base_load(_interp, &this, &rawdata)?;
            Ok(Value::none())
        }

        #[py_name = "_BaseCookie__parse_string"]
        fn base_cookie_parse_string(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BASE_PARSE, args)?;
            let this = b.take_or_none(0);
            let text = b.take_or_none(1);
            let text = str_of(_interp, &text)?;
            parse_string(_interp, &this, &text)?;
            Ok(Value::none())
        }
    }

    class SimpleCookie("BaseCookie") {
        constants {
            "__doc__" => Value::string(docs::SIMPLE_COOKIE_DOC),
        }

        fn value_decode(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SIMPLE_VALUE_DECODE, args)?;
            let _this = b.take_or_none(0);
            let value = b.take_or_none(1);
            let decoded = unquote_impl(_interp, &value)?;
            Ok(Value::tuple(vec![decoded, value]))
        }

        fn value_encode(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SIMPLE_VALUE_ENCODE, args)?;
            let _this = b.take_or_none(0);
            let value = b.take_or_none(1);
            let text = Value::string(str_of(_interp, &value)?);
            let quoted = quote_impl(_interp, &text)?;
            Ok(Value::tuple(vec![text, quoted]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn scanner_matches_key_value_pairs_and_flags() {
        let s = chars("a=b; Secure; c=\"d e\"");
        let first = match_cookie(&s, 0).expect("first");
        assert_eq!(first.key, (0, 1));
        assert_eq!(first.val, Some((2, 3)));
        assert_eq!(first.end, 4);
        let second = match_cookie(&s, first.end).expect("second");
        assert_eq!(second.val, None);
        assert_eq!(
            &s[second.key.0..second.key.1].iter().collect::<String>(),
            "Secure"
        );
        let third = match_cookie(&s, second.end).expect("third");
        assert_eq!(third.val.map(|(a, b)| b - a), Some(5));
    }

    #[test]
    fn scanner_rejects_illegal_characters() {
        assert_eq!(match_cookie(&chars("a=b\"c"), 0), None);
        assert_eq!(match_cookie(&chars("\"a\"=b"), 0), None);
    }

    #[test]
    fn scanner_matches_the_expires_date_shape() {
        let s = chars("expires=Wed, 09 Jun 2021 10:18:14 GMT; Path=/");
        let found = match_cookie(&s, 0).expect("expires");
        let (a, b) = found.val.expect("val");
        assert_eq!(
            s[a..b].iter().collect::<String>(),
            "Wed, 09 Jun 2021 10:18:14 GMT"
        );
    }

    #[test]
    fn unquote_handles_octal_and_pairs() {
        assert_eq!(unquote_body(&chars("a\\012b\\\"c\\\\d")), "a\nb\"c\\d");
        assert_eq!(unquote_body(&chars("a\\\nb")), "a\\\nb");
        assert_eq!(unquote_body(&chars("\\400")), "400");
    }
}
