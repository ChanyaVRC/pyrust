// `json.decoder` submodule — 100% native Rust implementation of Python 3.12's
// `json.decoder` module (issues #3251, #3413).
//
// Defines the canonical `JSONDecoder` and `JSONDecodeError` classes
// (re-exported by `json`, like CPython's `from .decoder import ...`), plus
// `scanstring` (`_json.scanstring`, also bound as `c_scanstring`),
// `py_scanstring`, `JSONObject`, `JSONArray`, `WHITESPACE`, `WHITESPACE_STR`,
// `BACKSLASH`, `FLAGS`, `NaN`, `PosInf`, `NegInf` and `_CONSTANTS`.
// `JSONDecoder` decodes through `self.scan_once` — by default the native
// `_json.Scanner` from `json.scanner.make_scanner` — and, like the
// pure-Python class, dispatches `decode` → `self.raw_decode` → `self.scan_once`
// so subclasses may override either step.
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/json/decoder.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::json::scan::{self, Doc, ScanError};
use crate::builtin_modules::json::{docs, support};
use crate::builtin_modules::json_scanner;
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

const WHITESPACE_STR_VAL: &str = " \t\n\r";

fn make_backslash_dict() -> Value {
    let mut d = PyDict::default();
    for (k, v) in [
        ("\"", "\""),
        ("\\", "\\"),
        ("/", "/"),
        ("b", "\x08"),
        ("f", "\x0c"),
        ("n", "\n"),
        ("r", "\r"),
        ("t", "\t"),
    ] {
        d.insert(PyKey::str_from(k), Value::string(v));
    }
    Value::dict(d)
}

const SIG_ERROR_INIT: Sig = Sig {
    qualname: "JSONDecodeError.__init__",
    params: &["self", "msg", "doc", "pos"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_ERROR_REDUCE: Sig = Sig {
    qualname: "JSONDecodeError.__reduce__",
    params: &["self"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_DECODER_INIT: Sig = Sig {
    qualname: "JSONDecoder.__init__",
    params: &["self"],
    defaults: 0,
    kwonly: &[
        "object_hook",
        "parse_float",
        "parse_int",
        "parse_constant",
        "strict",
        "object_pairs_hook",
    ],
    var_kw: false,
};
const SIG_DECODE: Sig = Sig {
    qualname: "JSONDecoder.decode",
    params: &["self", "s", "_w"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_RAW_DECODE: Sig = Sig {
    qualname: "JSONDecoder.raw_decode",
    params: &["self", "s", "idx"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_PY_SCANSTRING: Sig = Sig {
    qualname: "py_scanstring",
    params: &["s", "end", "strict", "_b", "_m"],
    defaults: 3,
    kwonly: &[],
    var_kw: false,
};
const SIG_JSON_OBJECT: Sig = Sig {
    qualname: "JSONObject",
    params: &[
        "s_and_end",
        "strict",
        "scan_once",
        "object_hook",
        "object_pairs_hook",
        "memo",
        "_w",
        "_ws",
    ],
    defaults: 3,
    kwonly: &[],
    var_kw: false,
};
const SIG_JSON_ARRAY: Sig = Sig {
    qualname: "JSONArray",
    params: &["s_and_end", "scan_once", "_w", "_ws"],
    defaults: 2,
    kwonly: &[],
    var_kw: false,
};

// ── JSONDecodeError ──────────────────────────────────────────────────────────

/// `(doc.count('\n', 0, pos) + 1, pos - doc.rfind('\n', 0, pos))` for a
/// `str` document and an `int` position.
fn line_col(doc: &Doc, pos: i64) -> (i64, i64) {
    let n = doc.char_len() as i64;
    let end = if pos < 0 {
        (pos + n).max(0)
    } else {
        pos.min(n)
    };
    let prefix = &doc.text()[..doc.byte_of(end as usize)];
    let lineno = prefix.bytes().filter(|&b| b == b'\n').count() as i64 + 1;
    // Byte search: a `char` pattern would decode CESU-8 surrogates.
    let last = prefix
        .as_bytes()
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(-1, |b| doc.char_of(b) as i64);
    (lineno, pos - last)
}

/// `JSONDecodeError.__init__(self, msg, doc, pos)`.
fn decode_error_init(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_ERROR_INIT, call_args)?;
    let this = b.take_or_none(0);
    let msg = b.take_or_none(1);
    let doc = b.take_or_none(2);
    let pos = b.take_or_none(3);
    let native = match (Doc::new(&doc), pos.kind()) {
        (Some(text), ValueKind::Int(p)) => Some((text, p)),
        _ => None,
    };
    let (lineno, colno) = match native {
        Some((text, p)) => {
            let (line, col) = line_col(&text, p);
            (Value::int(line), Value::int(col))
        }
        None => {
            let newline = Value::string("\n");
            let count = support::call_method(
                interp,
                &doc,
                "count",
                &[newline.clone(), Value::int(0), pos.clone()],
            )?;
            let lineno = interp.eval_binary(count, BinaryOp::Add, Value::int(1))?;
            let found = support::call_method(
                interp,
                &doc,
                "rfind",
                &[newline, Value::int(0), pos.clone()],
            )?;
            let colno = interp.eval_binary(pos.clone(), BinaryOp::Sub, found)?;
            (lineno, colno)
        }
    };
    let errmsg = match (msg.as_str(), lineno.kind(), colno.kind(), pos.kind()) {
        (Some(m), ValueKind::Int(l), ValueKind::Int(c), ValueKind::Int(p)) => {
            Value::string(format!("{m}: line {l} column {c} (char {p})"))
        }
        _ => interp.eval_binary(
            Value::string("%s: line %d column %d (char %d)"),
            BinaryOp::Mod,
            Value::tuple(vec![
                msg.clone(),
                lineno.clone(),
                colno.clone(),
                pos.clone(),
            ]),
        )?,
    };
    // `ValueError.__init__(self, errmsg)`
    interp.assign_attr(this.clone(), "args", Value::tuple(vec![errmsg]))?;
    for (name, value) in [
        ("msg", msg),
        ("doc", doc),
        ("pos", pos),
        ("lineno", lineno),
        ("colno", colno),
    ] {
        interp.assign_attr(this.clone(), name, value)?;
    }
    Ok(Value::none())
}

// ── scanstring ───────────────────────────────────────────────────────────────

/// `_json.scanstring` at code point `end`, with its bounds check.
fn c_scanstring_at(
    interp: &mut Interpreter,
    doc: &Doc,
    end: i64,
    strict: bool,
) -> Result<(Value, i64)> {
    if end < 0 || end > doc.char_len() as i64 {
        return Err(PyError::named(
            "ValueError",
            "end is out of bounds".to_string(),
        ));
    }
    let start = doc.byte_of(end as usize);
    let (piece, next) = scan::scanstring_c(interp, doc, start, strict)?;
    Ok((piece.into_value(doc.text()), doc.char_of(next) as i64))
}

/// `_json.scanstring(s, end, strict=True)` (`PyArg_ParseTuple` "On|p").
fn c_scanstring_call(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    if call_args.iter().any(|a| a.name.is_some()) {
        return Err(args::type_error("scanstring() takes no keyword arguments"));
    }
    let n = call_args.len();
    if n < 2 {
        return Err(args::type_error(format!(
            "scanstring() takes at least 2 arguments ({n} given)"
        )));
    }
    if n > 3 {
        return Err(args::type_error(format!(
            "scanstring() takes at most 3 arguments ({n} given)"
        )));
    }
    let s = &call_args[0].value;
    let end = support::ssize_index(interp, &call_args[1].value)?;
    let strict = match call_args.get(2) {
        Some(arg) => interp.truthy_value(&arg.value)?,
        None => true,
    };
    let Some(doc) = Doc::new(s) else {
        return Err(args::type_error(format!(
            "first argument must be a string, not {}",
            support::type_name(s)
        )));
    };
    let (value, next) = c_scanstring_at(interp, &doc, end, strict)?;
    Ok(Value::tuple(vec![value, Value::int(next)]))
}

/// The `str` a `re` pattern method would accept as its subject.
fn pattern_subject(value: &Value) -> Result<Doc> {
    if let Some(doc) = Doc::new(value) {
        return Ok(doc);
    }
    let bytes_like = matches!(value.kind(), ValueKind::Bytes(_))
        || pyrust_builtins::bytearray::as_bytearray_rc(value).is_some();
    Err(args::type_error(if bytes_like {
        "cannot use a string pattern on a bytes-like object".to_string()
    } else {
        format!(
            "expected string or bytes-like object, got '{}'",
            support::type_name(value)
        )
    }))
}

/// `json.decoder.py_scanstring(s, end, strict=True)`.
fn py_scanstring_call(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_PY_SCANSTRING, call_args)?;
    let s = b.take_or_none(0);
    let end = b.take_or_none(1);
    let strict = match b.take(2) {
        Some(flag) => interp.truthy_value(&flag)?,
        None => true,
    };
    let doc = pattern_subject(&s)?;
    let end = support::ssize_index(interp, &end)?;
    let (value, next) = scan::scanstring_py(interp, &doc, end, strict)?;
    Ok(Value::tuple(vec![value, Value::int(next)]))
}

// ── JSONObject / JSONArray ───────────────────────────────────────────────────

/// `c in WHITESPACE_STR` for a code point.
fn is_ws_cp(cp: u32) -> bool {
    matches!(cp, 0x20 | 0x09 | 0x0A | 0x0D)
}

/// `nextchar in _ws` for `nextchar = s[end:end + 1]` (`''` is in every str).
fn slice_in_ws(cp: Option<u32>) -> bool {
    cp.is_none_or(is_ws_cp)
}

fn s_and_end(interp: &mut Interpreter, value: &Value) -> Result<(Value, Doc, i64)> {
    let (s, end) = support::unpack2(interp, value)?;
    pattern_subject(&s)?;
    let doc = Doc::indexed(&s).expect("checked str");
    let end = support::ssize_index(interp, &end)?;
    Ok((s, doc, end))
}

/// `value, end = scan_once(s, end)` with `StopIteration` turned into
/// `JSONDecodeError("Expecting value", s, err.value)`.
fn call_scan_once(
    interp: &mut Interpreter,
    scan_once: &Value,
    s: &Value,
    end: i64,
) -> Result<(Value, i64)> {
    match support::call(interp, scan_once.clone(), &[s.clone(), Value::int(end)]) {
        Ok(result) => {
            let (value, end) = support::unpack2(interp, &result)?;
            Ok((value, support::ssize_index(interp, &end)?))
        }
        Err(err) => match support::stop_iteration_value(interp, &err) {
            Some(pos) => Err(support::decode_error(interp, "Expecting value", s, pos)),
            None => Err(err),
        },
    }
}

/// `json.decoder.JSONObject(s_and_end, strict, scan_once, object_hook,
/// object_pairs_hook, memo=None)`.
fn json_object_call(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_JSON_OBJECT, call_args)?;
    let (s, doc, mut end) = s_and_end(interp, &b.take_or_none(0))?;
    let strict = b.take_or_none(1);
    let strict = interp.truthy_value(&strict)?;
    let scan_once = b.take_or_none(2);
    let object_hook = b.take_or_none(3);
    let object_pairs_hook = b.take_or_none(4);
    let memo = match b.take(5) {
        Some(memo) if !memo.is_none() => memo,
        _ => Value::dict(PyDict::default()),
    };
    let result = |obj: Value, end: i64| Ok(Value::tuple(vec![obj, Value::int(end)]));
    let mut pairs: Vec<Value> = Vec::new();
    let mut nextchar = doc.char_at(end);
    if nextchar != Some(u32::from(b'"')) {
        if slice_in_ws(nextchar) {
            end = doc.ws_end(end);
            nextchar = doc.char_at(end);
        }
        if nextchar == Some(u32::from(b'}')) {
            if !object_pairs_hook.is_none() {
                let obj = support::call(interp, object_pairs_hook, &[Value::list(pairs)])?;
                return result(obj, end + 1);
            }
            let mut obj = Value::dict(PyDict::default());
            if !object_hook.is_none() {
                obj = support::call(interp, object_hook, &[obj])?;
            }
            return result(obj, end + 1);
        } else if nextchar != Some(u32::from(b'"')) {
            return Err(doc.error_at(
                interp,
                "Expecting property name enclosed in double quotes",
                end,
            ));
        }
    }
    end += 1;
    loop {
        let (key, next) = c_scanstring_at(interp, &doc, end, strict)?;
        end = next;
        let key = support::call_method(interp, &memo, "setdefault", &[key.clone(), key])?;
        if doc.char_at(end) != Some(u32::from(b':')) {
            end = doc.ws_end(end);
            if doc.char_at(end) != Some(u32::from(b':')) {
                return Err(doc.error_at(interp, "Expecting ':' delimiter", end));
            }
        }
        end += 1;
        if doc.char_at(end).is_some_and(is_ws_cp) {
            end += 1;
            if doc.char_at(end).is_some_and(is_ws_cp) {
                end = doc.ws_end(end + 1);
            }
        }
        let (value, next) = call_scan_once(interp, &scan_once, &s, end)?;
        end = next;
        pairs.push(Value::tuple(vec![key, value]));
        let mut nextchar = doc.char_at(end);
        if nextchar.is_some_and(is_ws_cp) {
            end = doc.ws_end(end + 1);
            nextchar = doc.char_at(end);
        }
        end += 1;
        if nextchar == Some(u32::from(b'}')) {
            break;
        }
        if nextchar != Some(u32::from(b',')) {
            return Err(doc.error_at(interp, "Expecting ',' delimiter", end - 1));
        }
        end = doc.ws_end(end);
        let nextchar = doc.char_at(end);
        end += 1;
        if nextchar != Some(u32::from(b'"')) {
            return Err(doc.error_at(
                interp,
                "Expecting property name enclosed in double quotes",
                end - 1,
            ));
        }
    }
    if !object_pairs_hook.is_none() {
        let obj = support::call(interp, object_pairs_hook, &[Value::list(pairs)])?;
        return result(obj, end);
    }
    let dict_type = support::builtin(interp, "dict")?;
    let mut obj = support::call(interp, dict_type, &[Value::list(pairs)])?;
    if !object_hook.is_none() {
        obj = support::call(interp, object_hook, &[obj])?;
    }
    result(obj, end)
}

/// `json.decoder.JSONArray(s_and_end, scan_once)`.
fn json_array_call(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_JSON_ARRAY, call_args)?;
    let (s, doc, mut end) = s_and_end(interp, &b.take_or_none(0))?;
    let scan_once = b.take_or_none(1);
    let mut values = Vec::new();
    let mut nextchar = doc.char_at(end);
    if slice_in_ws(nextchar) {
        end = doc.ws_end(end + 1);
        nextchar = doc.char_at(end);
    }
    if nextchar == Some(u32::from(b']')) {
        return Ok(Value::tuple(vec![Value::list(values), Value::int(end + 1)]));
    }
    loop {
        let (value, next) = call_scan_once(interp, &scan_once, &s, end)?;
        end = next;
        values.push(value);
        let mut nextchar = doc.char_at(end);
        if slice_in_ws(nextchar) {
            end = doc.ws_end(end + 1);
            nextchar = doc.char_at(end);
        }
        end += 1;
        if nextchar == Some(u32::from(b']')) {
            break;
        }
        if nextchar != Some(u32::from(b',')) {
            return Err(doc.error_at(interp, "Expecting ',' delimiter", end - 1));
        }
        if doc.char_at(end).is_some_and(is_ws_cp) {
            end += 1;
            if doc.char_at(end).is_some_and(is_ws_cp) {
                end = doc.ws_end(end + 1);
            }
        }
    }
    Ok(Value::tuple(vec![Value::list(values), Value::int(end)]))
}

// ── JSONDecoder ──────────────────────────────────────────────────────────────

/// `json.decoder`'s module object.
fn decoder_module(interp: &mut Interpreter) -> Result<Value> {
    interp.load_module("json.decoder")
}

/// `value or fallback()`.
fn value_or(
    interp: &mut Interpreter,
    value: Value,
    fallback: impl FnOnce(&mut Interpreter) -> Result<Value>,
) -> Result<Value> {
    if interp.truthy_value(&value)? {
        Ok(value)
    } else {
        fallback(interp)
    }
}

/// `JSONDecoder.__init__`.
fn decoder_init(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_DECODER_INIT, call_args)?;
    let this = b.take_or_none(0);
    let object_hook = b.take_or_none(1);
    let parse_float = b.take_or_none(2);
    let parse_int = b.take_or_none(3);
    let parse_constant = b.take_or_none(4);
    let strict = b.take_or(5, || Value::bool_(true));
    let object_pairs_hook = b.take_or_none(6);
    let module = decoder_module(interp)?;
    let parse_float = value_or(interp, parse_float, |interp| {
        support::builtin(interp, "float")
    })?;
    let parse_int = value_or(interp, parse_int, |interp| support::builtin(interp, "int"))?;
    let parse_constant = value_or(interp, parse_constant, |interp| {
        let constants = interp.get_attr(&module, "_CONSTANTS")?;
        interp.get_attr(&constants, "__getitem__")
    })?;
    let parse_object = interp.get_attr(&module, "JSONObject")?;
    let parse_array = interp.get_attr(&module, "JSONArray")?;
    let parse_string = interp.get_attr(&module, "scanstring")?;
    for (name, value) in [
        ("object_hook", object_hook),
        ("parse_float", parse_float),
        ("parse_int", parse_int),
        ("parse_constant", parse_constant),
        ("strict", strict),
        ("object_pairs_hook", object_pairs_hook),
        ("parse_object", parse_object),
        ("parse_array", parse_array),
        ("parse_string", parse_string),
        ("memo", Value::dict(PyDict::default())),
    ] {
        interp.assign_attr(this.clone(), name, value)?;
    }
    let scanner = interp.load_module("json.scanner")?;
    let make_scanner = interp.get_attr(&scanner, "make_scanner")?;
    let scan_once = support::call(interp, make_scanner, std::slice::from_ref(&this))?;
    interp.assign_attr(this, "scan_once", scan_once)?;
    Ok(Value::none())
}

/// `WHITESPACE.match(s, pos).end()` for `decode`'s default `_w`.
fn whitespace_end(interp: &mut Interpreter, s: &Value, pos: &Value) -> Result<Value> {
    let doc = pattern_subject(s)?;
    let pos = support::ssize_index(interp, pos)?;
    Ok(Value::int(doc.ws_end(pos)))
}

/// `_w(s, pos).end()` for a caller-supplied `_w`.
fn custom_whitespace_end(
    interp: &mut Interpreter,
    w: &Value,
    s: &Value,
    pos: Value,
) -> Result<Value> {
    let m = support::call(interp, w.clone(), &[s.clone(), pos])?;
    support::call_method(interp, &m, "end", &[])
}

/// `JSONDecoder.decode(self, s, _w=WHITESPACE.match)`.
fn decoder_decode(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_DECODE, call_args)?;
    let this = b.take_or_none(0);
    let s = b.take_or_none(1);
    let w = b.take(2);
    let start = match &w {
        Some(w) => custom_whitespace_end(interp, w, &s, Value::int(0))?,
        None => whitespace_end(interp, &s, &Value::int(0))?,
    };
    let raw_decode = interp.get_attr(&this, "raw_decode")?;
    let decoded = support::call_kw(
        interp,
        raw_decode,
        std::slice::from_ref(&s),
        &[("idx".to_string(), start)],
    )?;
    let (obj, end) = support::unpack2(interp, &decoded)?;
    let end = match &w {
        Some(w) => custom_whitespace_end(interp, w, &s, end)?,
        None => whitespace_end(interp, &s, &end)?,
    };
    let len = match Doc::new(&s) {
        Some(doc) => Value::int(doc.char_len() as i64),
        None => {
            let len_fn = support::builtin(interp, "len")?;
            support::call(interp, len_fn, std::slice::from_ref(&s))?
        }
    };
    let ne = interp.eval_binary(end.clone(), BinaryOp::Ne, len)?;
    if interp.truthy_value(&ne)? {
        return Err(support::decode_error(interp, "Extra data", &s, end));
    }
    Ok(obj)
}

/// `JSONDecoder.raw_decode(self, s, idx=0)`.
fn decoder_raw_decode(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_RAW_DECODE, call_args)?;
    let this = b.take_or_none(0);
    let s = b.take_or_none(1);
    let idx = b.take_or(2, || Value::int(0));
    let scan_once = interp.get_attr(&this, "scan_once")?;
    let scanned = if json_scanner::is_c_scanner(&scan_once) {
        let cfg = json_scanner::c_scanner_config(&scan_once)?;
        let idx = support::ssize_index(interp, &idx)?;
        match json_scanner::c_scan(interp, &cfg, &s, idx) {
            Ok((obj, end)) => Ok(Value::tuple(vec![obj, Value::int(end)])),
            Err(ScanError::Stop(pos)) => {
                return Err(support::decode_error(
                    interp,
                    "Expecting value",
                    &s,
                    Value::int(pos as i64),
                ));
            }
            Err(ScanError::Raised(err)) => Err(err),
        }
    } else {
        support::call(interp, scan_once, &[s.clone(), idx])
    };
    match scanned {
        Ok(result) => {
            let (obj, end) = support::unpack2(interp, &result)?;
            Ok(Value::tuple(vec![obj, end]))
        }
        Err(err) => match support::stop_iteration_value(interp, &err) {
            Some(pos) => Err(support::decode_error(interp, "Expecting value", &s, pos)),
            None => Err(err),
        },
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // `NaN = float('nan')` etc. are created once, so every `NaN` token decodes
    // to the same object (`_CONSTANTS.__getitem__` hands these back).
    let nan = Value::float(f64::NAN);
    let pos_inf = Value::float(f64::INFINITY);
    let neg_inf = Value::float(f64::NEG_INFINITY);
    let mut constants = PyDict::default();
    constants.insert(PyKey::str_from("-Infinity"), neg_inf.clone());
    constants.insert(PyKey::str_from("Infinity"), pos_inf.clone());
    constants.insert(PyKey::str_from("NaN"), nan.clone());
    {
        let mut m = module.borrow_mut();
        m.insert_attr("NaN".to_string(), nan);
        m.insert_attr("PosInf".to_string(), pos_inf);
        m.insert_attr("NegInf".to_string(), neg_inf);
        m.insert_attr("_CONSTANTS".to_string(), Value::dict(constants));
        if let Some(scanstring) = m.get_attr_value("scanstring") {
            m.insert_attr("c_scanstring".to_string(), scanstring);
        }
    }
    let re_mod = interp.load_module("re")?;
    let compile_fn = interp.get_attr(&re_mod, "compile")?;
    // `FLAGS = re.VERBOSE | re.MULTILINE | re.DOTALL` (a `re.RegexFlag`).
    let verbose = interp.get_attr(&re_mod, "VERBOSE")?;
    let multiline = interp.get_attr(&re_mod, "MULTILINE")?;
    let dotall = interp.get_attr(&re_mod, "DOTALL")?;
    let flags = interp.eval_binary(verbose, BinaryOp::BitOr, multiline)?;
    let flags = interp.eval_binary(flags, BinaryOp::BitOr, dotall)?;
    let ws_re = support::call(
        interp,
        compile_fn,
        &[Value::string(r"[ \t\n\r]*"), flags.clone()],
    )?;
    let scanner_mod = interp.load_module("json.scanner")?;
    let mut m = module.borrow_mut();
    m.insert_attr("re".to_string(), re_mod);
    m.insert_attr("scanner".to_string(), scanner_mod);
    m.insert_attr("FLAGS".to_string(), flags);
    m.insert_attr("WHITESPACE".to_string(), ws_re);
    for name in [
        "re",
        "scanner",
        "c_scanstring",
        "__all__",
        "FLAGS",
        "NaN",
        "PosInf",
        "NegInf",
        "JSONDecodeError",
        "_CONSTANTS",
        "BACKSLASH",
        "py_scanstring",
        "scanstring",
        "WHITESPACE",
        "WHITESPACE_STR",
        "JSONObject",
        "JSONArray",
        "JSONDecoder",
    ] {
        if let Some(value) = m.attrs.shift_remove(name) {
            m.attrs.insert(name.to_string(), value);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::JSON_DECODER_MODULE_DOC),
        "__package__" => Value::string("json"),
        "__all__" => Value::list(vec![
            Value::string("JSONDecoder"),
            Value::string("JSONDecodeError"),
        ]),
        "WHITESPACE_STR" => Value::string(WHITESPACE_STR_VAL),
        "BACKSLASH" => make_backslash_dict(),
    }

    class JSONDecodeError("ValueError") {
        constants {
            "__doc__" => Value::string(docs::JSON_DECODE_ERROR_DOC),
        }

        fn __init__(args) -> Result<Value> {
            decode_error_init(_interp, args)
        }

        fn __reduce__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_ERROR_REDUCE, args)?;
            let this = b.take_or_none(0);
            let cls = _interp.get_attr(&this, "__class__")?;
            let msg = _interp.get_attr(&this, "msg")?;
            let doc = _interp.get_attr(&this, "doc")?;
            let pos = _interp.get_attr(&this, "pos")?;
            Ok(Value::tuple(vec![cls, Value::tuple(vec![msg, doc, pos])]))
        }
    }

    // `_json.scanstring` (also bound as `c_scanstring` by `post_load_init`).
    fn scanstring(args) -> Result<Value> {
        c_scanstring_call(_interp, args)
    }

    /// Scan the string s for a JSON string. End is the index of the
    /// character in s after the quote that started the JSON string.
    /// Unescapes all valid JSON string escape sequences and raises ValueError
    /// on attempt to decode an invalid string. If strict is False then literal
    /// control characters are allowed in the string.
    ///
    /// Returns a tuple of the decoded string and the index of the character in s
    /// after the end quote.
    fn py_scanstring(args) -> Result<Value> {
        py_scanstring_call(_interp, args)
    }

    #[py_name = "JSONObject"]
    fn json_object(args) -> Result<Value> {
        json_object_call(_interp, args)
    }

    #[py_name = "JSONArray"]
    fn json_array(args) -> Result<Value> {
        json_array_call(_interp, args)
    }

    class JSONDecoder {
        constants {
            "__doc__" => Value::string(docs::JSON_DECODER_DOC),
        }

        fn __init__(args) -> Result<Value> {
            decoder_init(_interp, args)
        }

        fn decode(args) -> Result<Value> {
            decoder_decode(_interp, args)
        }

        fn raw_decode(args) -> Result<Value> {
            decoder_raw_decode(_interp, args)
        }
    }
}
