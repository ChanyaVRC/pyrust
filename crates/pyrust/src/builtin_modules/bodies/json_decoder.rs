// `json.decoder` submodule — 100% native Rust implementation of Python 3.12's
// `json.decoder` module (issue #3251).
//
// Exposes `JSONDecoder`, `JSONDecodeError`, `WHITESPACE`, `WHITESPACE_STR`,
// `BACKSLASH`, `NaN`, `PosInf`, `NegInf`, `scanstring`, `py_scanstring`,
// `JSONObject`, and `JSONArray`. Shares the canonical `JSONDecoder` and
// `JSONDecodeError` classes with `json`.
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/json/decoder.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyBool, PyIndex, PyStr, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value};

const WHITESPACE_STR_VAL: &str = " \t\n\r";

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_pos(interp, f, args)
}

fn raise_decode_error(interp: &mut Interpreter, msg: &str, doc: &str, pos: usize) -> PyError {
    if let Ok(dec_mod) = interp.load_module("json.decoder")
        && let Ok(cls) = interp.get_attr(&dec_mod, "JSONDecodeError")
        && let Ok(inst) = call_pos(
            interp,
            cls,
            &[
                Value::string(msg),
                Value::string(doc),
                Value::int(pos as i64),
            ],
        )
    {
        return PyError::raised(inst);
    }
    PyError::named("ValueError", format!("{msg}: char {pos}"))
}

fn parse_u4(chars: &[char], idx: usize, doc: &str, interp: &mut Interpreter) -> Result<u32> {
    if idx + 4 > chars.len() {
        return Err(raise_decode_error(
            interp,
            "Invalid \\uXXXX escape",
            doc,
            idx.saturating_sub(1),
        ));
    }
    let mut acc: u32 = 0;
    for &ch in &chars[idx..idx + 4] {
        let digit = ch.to_digit(16).ok_or_else(|| {
            raise_decode_error(interp, "Invalid \\uXXXX escape", doc, idx.saturating_sub(1))
        })?;
        acc = (acc << 4) | digit;
    }
    Ok(acc)
}

/// Core implementation of `json.decoder.scanstring(s, end, strict=True)`.
/// `end` is the character index immediately after the opening `"`.
fn scanstring_impl(
    interp: &mut Interpreter,
    s: &str,
    end: usize,
    strict: bool,
) -> Result<(String, usize)> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let begin = end.saturating_sub(1);
    let mut idx = end;
    let mut out = String::new();

    while idx < n {
        let ch = chars[idx];
        if ch == '"' {
            return Ok((out, idx + 1));
        }
        if ch == '\\' {
            idx += 1;
            if idx >= n {
                return Err(raise_decode_error(
                    interp,
                    "Unterminated string starting at",
                    s,
                    begin,
                ));
            }
            let esc = chars[idx];
            match esc {
                '"' => {
                    out.push('"');
                    idx += 1;
                }
                '\\' => {
                    out.push('\\');
                    idx += 1;
                }
                '/' => {
                    out.push('/');
                    idx += 1;
                }
                'b' => {
                    out.push('\x08');
                    idx += 1;
                }
                'f' => {
                    out.push('\x0c');
                    idx += 1;
                }
                'n' => {
                    out.push('\n');
                    idx += 1;
                }
                'r' => {
                    out.push('\r');
                    idx += 1;
                }
                't' => {
                    out.push('\t');
                    idx += 1;
                }
                'u' => {
                    let mut uni = parse_u4(&chars, idx + 1, s, interp)?;
                    idx += 5;
                    if (0xd800..=0xdbff).contains(&uni)
                        && idx + 6 <= n
                        && chars[idx] == '\\'
                        && chars[idx + 1] == 'u'
                    {
                        let uni2 = parse_u4(&chars, idx + 2, s, interp)?;
                        if (0xdc00..=0xdfff).contains(&uni2) {
                            uni = 0x10000 + (((uni - 0xd800) << 10) | (uni2 - 0xdc00));
                            idx += 6;
                        }
                    }
                    let decoded_ch = char::from_u32(uni).ok_or_else(|| {
                        raise_decode_error(
                            interp,
                            "Invalid \\uXXXX escape",
                            s,
                            idx.saturating_sub(5),
                        )
                    })?;
                    out.push(decoded_ch);
                }
                other => {
                    let msg = format!("Invalid \\escape: '{other}'");
                    return Err(raise_decode_error(interp, &msg, s, idx));
                }
            }
        } else {
            if strict && (ch as u32) < 0x20 {
                let msg = format!("Invalid control character {:?} at", ch.to_string());
                return Err(raise_decode_error(interp, &msg, s, idx));
            }
            out.push(ch);
            idx += 1;
        }
    }

    Err(raise_decode_error(
        interp,
        "Unterminated string starting at",
        s,
        begin,
    ))
}

fn skip_ws(chars: &[char], mut idx: usize) -> usize {
    while idx < chars.len() && matches!(chars[idx], ' ' | '\t' | '\n' | '\r') {
        idx += 1;
    }
    idx
}

fn extract_s_and_end(interp: &mut Interpreter, val: &Value) -> Result<(String, usize)> {
    let items = interp.collect_iterable(val)?;
    if items.len() != 2 {
        return Err(PyError::named(
            "ValueError",
            "expected 2-tuple (s, end)".to_string(),
        ));
    }
    let s = items[0]
        .as_str()
        .ok_or_else(|| PyError::named("TypeError", "expected str".to_string()))?
        .to_string();
    let end = items[1]
        .as_int()
        .ok_or_else(|| PyError::named("TypeError", "expected int".to_string()))?;
    Ok((s, end.max(0) as usize))
}

fn is_stop_iteration(interp: &mut Interpreter, err: &PyError) -> Option<usize> {
    if !err.class_name_is("StopIteration") {
        return None;
    }
    let exc = interp.materialize_pyerror(err.clone()).ok()?;
    let val = interp.get_attr(&exc, "value").ok()?;
    val.as_int().map(|n| n.max(0) as usize)
}

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

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // Share canonical classes from `json` so `json.decoder.JSONDecoder is json.JSONDecoder`
    // and `json.decoder.JSONDecodeError is json.JSONDecodeError`.
    let json_mod = interp.load_module("json")?;
    for attr in ["JSONDecoder", "JSONDecodeError"] {
        if let Ok(v) = interp.get_attr(&json_mod, attr) {
            module.borrow_mut().insert_attr(attr.to_string(), v);
        }
    }
    // Compile `WHITESPACE` regex.
    let re_mod = interp.load_module("re")?;
    let compile_fn = interp.get_attr(&re_mod, "compile")?;
    let flags = interp.get_attr(&re_mod, "VERBOSE")?.as_int().unwrap_or(0)
        | interp.get_attr(&re_mod, "MULTILINE")?.as_int().unwrap_or(0)
        | interp.get_attr(&re_mod, "DOTALL")?.as_int().unwrap_or(0);
    let ws_re = call_pos(
        interp,
        compile_fn,
        &[Value::string(r"[ \t\n\r]*"), Value::int(flags)],
    )?;
    module
        .borrow_mut()
        .insert_attr("WHITESPACE".to_string(), ws_re);
    let scanner_mod = interp.load_module("json.scanner")?;
    module
        .borrow_mut()
        .insert_attr("scanner".to_string(), scanner_mod);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("json"),
        "__all__" => Value::list(vec![
            Value::string("JSONDecoder"),
            Value::string("JSONDecodeError"),
        ]),
        "WHITESPACE_STR" => Value::string(WHITESPACE_STR_VAL),
        "WHITESPACE" => Value::none(),
        "BACKSLASH" => make_backslash_dict(),
        "NaN" => Value::float(f64::NAN),
        "PosInf" => Value::float(f64::INFINITY),
        "NegInf" => Value::float(f64::NEG_INFINITY),
        "JSONDecoder" => Value::none(),
        "JSONDecodeError" => Value::none(),
    }

    fn scanstring(
        s: PyStr<'_>,
        end: PyIndex,
        #[default(true.into())] strict: PyBool,
    ) -> Result<Value> {
        let end_i = end.expect_i64(FN_NAME, "end")?;
        if end_i < 0 {
            return Err(PyError::named("ValueError", "end is out of bounds".to_string()));
        }
        let (decoded, next_idx) = scanstring_impl(_interp, s.0.as_ref(), end_i as usize, strict.0)?;
        Ok(Value::tuple(vec![
            Value::string(decoded),
            Value::int(next_idx as i64),
        ]))
    }

    fn py_scanstring(
        s: PyStr<'_>,
        end: PyIndex,
        #[default(true.into())] strict: PyBool,
    ) -> Result<Value> {
        let end_i = end.expect_i64(FN_NAME, "end")?;
        if end_i < 0 {
            return Err(PyError::named("ValueError", "end is out of bounds".to_string()));
        }
        let (decoded, next_idx) = scanstring_impl(_interp, s.0.as_ref(), end_i as usize, strict.0)?;
        Ok(Value::tuple(vec![
            Value::string(decoded),
            Value::int(next_idx as i64),
        ]))
    }

    fn c_scanstring(
        s: PyStr<'_>,
        end: PyIndex,
        #[default(true.into())] strict: PyBool,
    ) -> Result<Value> {
        let end_i = end.expect_i64(FN_NAME, "end")?;
        if end_i < 0 {
            return Err(PyError::named("ValueError", "end is out of bounds".to_string()));
        }
        let (decoded, next_idx) = scanstring_impl(_interp, s.0.as_ref(), end_i as usize, strict.0)?;
        Ok(Value::tuple(vec![
            Value::string(decoded),
            Value::int(next_idx as i64),
        ]))
    }

    #[py_name = "JSONObject"]
    fn json_object(
        s_and_end: PyValue,
        strict: PyBool,
        scan_once: PyValue,
        object_hook: PyValue,
        object_pairs_hook: PyValue,
        #[default(OptionalArg::Omitted)] memo: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] _w: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] _ws: OptValueRef<'_>,
    ) -> Result<Value> {
        let (s, end) = extract_s_and_end(_interp, &s_and_end.0)?;
        let chars: Vec<char> = s.chars().collect();
        let n = chars.len();
        let mut idx = skip_ws(&chars, end);
        let memo_opt = memo.as_value().filter(|v| !v.is_none());

        let finish = |interp: &mut Interpreter,
                      pairs: Vec<(Value, Value)>,
                      end_pos: usize|
         -> Result<Value> {
            if !object_pairs_hook.0.is_none() {
                let pair_list = Value::list(
                    pairs
                        .into_iter()
                        .map(|(k, v)| Value::tuple(vec![k, v]))
                        .collect(),
                );
                let result = call_pos(interp, object_pairs_hook.0.clone(), &[pair_list])?;
                return Ok(Value::tuple(vec![result, Value::int(end_pos as i64)]));
            }
            let dict_val = Value::dict(PyDict::default());
            for (k, v) in pairs {
                call_method(interp, &dict_val, "__setitem__", &[k, v])?;
            }
            let result = if !object_hook.0.is_none() {
                call_pos(interp, object_hook.0.clone(), &[dict_val])?
            } else {
                dict_val
            };
            Ok(Value::tuple(vec![result, Value::int(end_pos as i64)]))
        };

        if idx < n && chars[idx] == '}' {
            return finish(_interp, Vec::new(), idx + 1);
        }

        let mut pairs = Vec::new();
        let s_val = Value::string(&s);
        loop {
            if idx >= n || chars[idx] != '"' {
                return Err(raise_decode_error(
                    _interp,
                    "Expecting property name enclosed in double quotes",
                    &s,
                    idx,
                ));
            }
            let (key_str, next_idx) = scanstring_impl(_interp, &s, idx + 1, strict.0)?;
            let mut key_val = Value::string(key_str);
            if let Some(m) = memo_opt {
                key_val = call_method(_interp, m, "setdefault", &[key_val.clone(), key_val])?;
            }
            idx = skip_ws(&chars, next_idx);
            if idx >= n || chars[idx] != ':' {
                return Err(raise_decode_error(_interp, "Expecting ':' delimiter", &s, idx));
            }
            idx = skip_ws(&chars, idx + 1);
            let scanned = match call_pos(
                _interp,
                scan_once.0.clone(),
                &[s_val.clone(), Value::int(idx as i64)],
            ) {
                Ok(v) => v,
                Err(err) => {
                    if let Some(pos) = is_stop_iteration(_interp, &err) {
                        return Err(raise_decode_error(_interp, "Expecting value", &s, pos));
                    }
                    return Err(err);
                }
            };
            let parts = _interp.collect_iterable(&scanned)?;
            if parts.len() != 2 {
                return Err(PyError::named("ValueError", "scanner must return (value, end)".to_string()));
            }
            pairs.push((key_val, parts[0].clone()));
            idx = parts[1].as_int().unwrap_or(0).max(0) as usize;
            idx = skip_ws(&chars, idx);
            if idx >= n {
                return Err(raise_decode_error(_interp, "Expecting ',' delimiter", &s, idx));
            }
            let ch = chars[idx];
            if ch == '}' {
                return finish(_interp, pairs, idx + 1);
            }
            if ch != ',' {
                return Err(raise_decode_error(_interp, "Expecting ',' delimiter", &s, idx));
            }
            idx = skip_ws(&chars, idx + 1);
        }
    }

    #[py_name = "JSONArray"]
    fn json_array(
        s_and_end: PyValue,
        scan_once: PyValue,
        #[default(OptionalArg::Omitted)] _w: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] _ws: OptValueRef<'_>,
    ) -> Result<Value> {
        let (s, end) = extract_s_and_end(_interp, &s_and_end.0)?;
        let chars: Vec<char> = s.chars().collect();
        let n = chars.len();
        let mut idx = skip_ws(&chars, end);
        let mut values = Vec::new();
        if idx < n && chars[idx] == ']' {
            return Ok(Value::tuple(vec![
                Value::list(values),
                Value::int((idx + 1) as i64),
            ]));
        }
        let s_val = Value::string(&s);
        loop {
            let scanned = match call_pos(
                _interp,
                scan_once.0.clone(),
                &[s_val.clone(), Value::int(idx as i64)],
            ) {
                Ok(v) => v,
                Err(err) => {
                    if let Some(pos) = is_stop_iteration(_interp, &err) {
                        return Err(raise_decode_error(_interp, "Expecting value", &s, pos));
                    }
                    return Err(err);
                }
            };
            let parts = _interp.collect_iterable(&scanned)?;
            if parts.len() != 2 {
                return Err(PyError::named("ValueError", "scanner must return (value, end)".to_string()));
            }
            values.push(parts[0].clone());
            idx = parts[1].as_int().unwrap_or(0).max(0) as usize;
            idx = skip_ws(&chars, idx);
            if idx >= n {
                return Err(raise_decode_error(_interp, "Expecting ',' delimiter", &s, idx));
            }
            let ch = chars[idx];
            if ch == ']' {
                return Ok(Value::tuple(vec![
                    Value::list(values),
                    Value::int((idx + 1) as i64),
                ]));
            }
            if ch != ',' {
                return Err(raise_decode_error(_interp, "Expecting ',' delimiter", &s, idx));
            }
            idx = skip_ws(&chars, idx + 1);
        }
    }
}
