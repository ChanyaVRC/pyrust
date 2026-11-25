// `json.scanner` submodule — 100% native Rust implementation of Python 3.12's
// `json.scanner` module (issue #3251).
//
// Provides `NUMBER_RE`, `py_make_scanner`, `make_scanner`, and `c_make_scanner`.
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/json/scanner.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value};

const NUMBER_RE_PATTERN: &str = r"(-?(?:0|[1-9]\d*))(\.\d+)?([eE][-+]?\d+)?";

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

fn raise_stop_iteration(interp: &mut Interpreter, idx: usize) -> PyError {
    if let Some(stop_cls) = crate::interpreter::lookup_exc_class("StopIteration")
        && let Ok(inst) = call_pos(interp, Value::py_class(stop_cls), &[Value::int(idx as i64)])
    {
        return PyError::raised(inst);
    }
    PyError::named("StopIteration", idx.to_string())
}

/// Scan a JSON number starting at Unicode char offset `idx` in `chars`.
/// Returns `Some((integer_part, frac_part_opt, exp_part_opt, end_char_idx))`.
fn scan_number_chars(
    chars: &[char],
    start: usize,
) -> Option<(String, Option<String>, Option<String>, usize)> {
    let n = chars.len();
    let mut i = start;
    if i < n && chars[i] == '-' {
        i += 1;
    }
    if i >= n {
        return None;
    }
    if chars[i] == '0' {
        i += 1;
    } else if chars[i].is_ascii_digit() {
        i += 1;
        while i < n && chars[i].is_ascii_digit() {
            i += 1;
        }
    } else {
        return None;
    }
    let int_end = i;

    let mut frac_start_end = None;
    if i < n && chars[i] == '.' && i + 1 < n && chars[i + 1].is_ascii_digit() {
        let fs = i;
        i += 2;
        while i < n && chars[i].is_ascii_digit() {
            i += 1;
        }
        frac_start_end = Some((fs, i));
    }

    let mut exp_start_end = None;
    if i < n && (chars[i] == 'e' || chars[i] == 'E') {
        let es = i;
        let mut j = i + 1;
        if j < n && (chars[j] == '+' || chars[j] == '-') {
            j += 1;
        }
        if j < n && chars[j].is_ascii_digit() {
            i = j + 1;
            while i < n && chars[i].is_ascii_digit() {
                i += 1;
            }
            exp_start_end = Some((es, i));
        }
    }

    let integer: String = chars[start..int_end].iter().collect();
    let frac = frac_start_end.map(|(a, b)| chars[a..b].iter().collect());
    let exp = exp_start_end.map(|(a, b)| chars[a..b].iter().collect());
    Some((integer, frac, exp, i))
}

fn slice_matches(chars: &[char], idx: usize, expected: &str) -> bool {
    let exp_chars: Vec<char> = expected.chars().collect();
    if idx + exp_chars.len() > chars.len() {
        return false;
    }
    chars[idx..idx + exp_chars.len()] == exp_chars[..]
}

fn scan_once_inner(
    interp: &mut Interpreter,
    scanner_obj: &Value,
    context: &Value,
    s_val: &Value,
    s: &str,
    idx: usize,
) -> Result<Value> {
    let chars: Vec<char> = s.chars().collect();
    if idx >= chars.len() {
        return Err(raise_stop_iteration(interp, idx));
    }

    let decoder_mod = interp.load_module("json.decoder")?;
    let nextchar = chars[idx];

    if nextchar == '"' {
        let parse_string = interp
            .get_attr(context, "parse_string")
            .ok()
            .filter(|v| !v.is_none())
            .unwrap_or_else(|| {
                interp
                    .get_attr(&decoder_mod, "scanstring")
                    .unwrap_or_else(|_| Value::none())
            });
        let strict = interp
            .get_attr(context, "strict")
            .ok()
            .and_then(|v| interp.truthy_value(&v).ok())
            .unwrap_or(true);
        return call_pos(
            interp,
            parse_string,
            &[
                s_val.clone(),
                Value::int((idx + 1) as i64),
                Value::bool_(strict),
            ],
        );
    }

    if nextchar == '{' {
        let parse_object = interp
            .get_attr(context, "parse_object")
            .ok()
            .filter(|v| !v.is_none())
            .unwrap_or_else(|| {
                interp
                    .get_attr(&decoder_mod, "JSONObject")
                    .unwrap_or_else(|_| Value::none())
            });
        let strict = interp
            .get_attr(context, "strict")
            .ok()
            .and_then(|v| interp.truthy_value(&v).ok())
            .unwrap_or(true);
        let object_hook = interp
            .get_attr(context, "object_hook")
            .ok()
            .unwrap_or_else(Value::none);
        let object_pairs_hook = interp
            .get_attr(context, "object_pairs_hook")
            .ok()
            .unwrap_or_else(Value::none);
        let memo = interp
            .get_attr(context, "memo")
            .ok()
            .unwrap_or_else(|| Value::dict(crate::value::PyDict::default()));
        let s_and_end = Value::tuple(vec![s_val.clone(), Value::int((idx + 1) as i64)]);
        return call_pos(
            interp,
            parse_object,
            &[
                s_and_end,
                Value::bool_(strict),
                scanner_obj.clone(),
                object_hook,
                object_pairs_hook,
                memo,
            ],
        );
    }

    if nextchar == '[' {
        let parse_array = interp
            .get_attr(context, "parse_array")
            .ok()
            .filter(|v| !v.is_none())
            .unwrap_or_else(|| {
                interp
                    .get_attr(&decoder_mod, "JSONArray")
                    .unwrap_or_else(|_| Value::none())
            });
        let s_and_end = Value::tuple(vec![s_val.clone(), Value::int((idx + 1) as i64)]);
        return call_pos(interp, parse_array, &[s_and_end, scanner_obj.clone()]);
    }

    if nextchar == 'n' && slice_matches(&chars, idx, "null") {
        return Ok(Value::tuple(vec![
            Value::none(),
            Value::int((idx + 4) as i64),
        ]));
    }
    if nextchar == 't' && slice_matches(&chars, idx, "true") {
        return Ok(Value::tuple(vec![
            Value::bool_(true),
            Value::int((idx + 4) as i64),
        ]));
    }
    if nextchar == 'f' && slice_matches(&chars, idx, "false") {
        return Ok(Value::tuple(vec![
            Value::bool_(false),
            Value::int((idx + 5) as i64),
        ]));
    }

    if let Some((integer, frac, exp, end_idx)) = scan_number_chars(&chars, idx) {
        if frac.is_some() || exp.is_some() {
            let mut full = integer;
            if let Some(f) = frac {
                full.push_str(&f);
            }
            if let Some(e) = exp {
                full.push_str(&e);
            }
            let parse_float = interp
                .get_attr(context, "parse_float")
                .ok()
                .filter(|v| !v.is_none())
                .unwrap_or_else(|| {
                    crate::interpreter::primitive_class_by_name("float")
                        .map(Value::py_class)
                        .unwrap_or_else(|| Value::builtin_function("float"))
                });
            let res = call_pos(interp, parse_float, &[Value::string(full)])?;
            return Ok(Value::tuple(vec![res, Value::int(end_idx as i64)]));
        } else {
            let parse_int = interp
                .get_attr(context, "parse_int")
                .ok()
                .filter(|v| !v.is_none())
                .unwrap_or_else(|| {
                    crate::interpreter::primitive_class_by_name("int")
                        .map(Value::py_class)
                        .unwrap_or_else(|| Value::builtin_function("int"))
                });
            let res = call_pos(interp, parse_int, &[Value::string(integer)])?;
            return Ok(Value::tuple(vec![res, Value::int(end_idx as i64)]));
        }
    }

    let parse_const = |interp: &mut Interpreter, token: &str| -> Result<Value> {
        if let Ok(pc) = interp.get_attr(context, "parse_constant")
            && !pc.is_none()
        {
            return call_pos(interp, pc, &[Value::string(token)]);
        }
        match token {
            "NaN" => interp.get_attr(&decoder_mod, "NaN"),
            "Infinity" => interp.get_attr(&decoder_mod, "PosInf"),
            "-Infinity" => interp.get_attr(&decoder_mod, "NegInf"),
            _ => Err(raise_stop_iteration(interp, idx)),
        }
    };

    if nextchar == 'N' && slice_matches(&chars, idx, "NaN") {
        let res = parse_const(interp, "NaN")?;
        return Ok(Value::tuple(vec![res, Value::int((idx + 3) as i64)]));
    }
    if nextchar == 'I' && slice_matches(&chars, idx, "Infinity") {
        let res = parse_const(interp, "Infinity")?;
        return Ok(Value::tuple(vec![res, Value::int((idx + 8) as i64)]));
    }
    if nextchar == '-' && slice_matches(&chars, idx, "-Infinity") {
        let res = parse_const(interp, "-Infinity")?;
        return Ok(Value::tuple(vec![res, Value::int((idx + 9) as i64)]));
    }

    Err(raise_stop_iteration(interp, idx))
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let re_mod = interp.load_module("re")?;
    let compile_fn = interp.get_attr(&re_mod, "compile")?;
    let flags = interp.get_attr(&re_mod, "VERBOSE")?.as_int().unwrap_or(0)
        | interp.get_attr(&re_mod, "MULTILINE")?.as_int().unwrap_or(0)
        | interp.get_attr(&re_mod, "DOTALL")?.as_int().unwrap_or(0);
    let number_re = call_pos(
        interp,
        compile_fn,
        &[Value::string(NUMBER_RE_PATTERN), Value::int(flags)],
    )?;
    module
        .borrow_mut()
        .insert_attr("NUMBER_RE".to_string(), number_re);
    Ok(())
}

struct ScannerState {
    context: Value,
    depth: usize,
}

pyrust_module! {
    constants {
        "__package__" => Value::string("json"),
        "__all__" => Value::list(vec![Value::string("make_scanner")]),
        "NUMBER_RE" => Value::none(),
    }

    #[native_state(ScannerState)]
    class _Scanner {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, context: PyValue) -> Result<Value> {
            set_state(
                self_val.0,
                ScannerState {
                    context: context.0,
                    depth: 0,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            string: PyStr<'_>,
            idx: PyIndex,
        ) -> Result<Value> {
            let idx_i = idx.expect_i64(FN_NAME, "idx")?;
            let context = with_state_mut(self_val.0, |st| {
                st.depth += 1;
                Ok(st.context.clone())
            })?;
            let s_val = Value::string(string.0.as_ref());
            let result = if idx_i < 0 {
                Err(raise_stop_iteration(_interp, 0))
            } else {
                scan_once_inner(_interp, self_val.0, &context, &s_val, string.0.as_ref(), idx_i as usize)
            };
            let clear_memo = with_state_mut(self_val.0, |st| {
                st.depth = st.depth.saturating_sub(1);
                Ok(st.depth == 0)
            })?;
            if clear_memo
                && let Ok(memo) = _interp.get_attr(&context, "memo")
                && !memo.is_none()
            {
                let _ = call_method(_interp, &memo, "clear", &[]);
            }
            result
        }
    }

    fn py_make_scanner(context: PyValue) -> Result<Value> {
        let scanner_mod = _interp.load_module("json.scanner")?;
        let cls = _interp.get_attr(&scanner_mod, "_Scanner")?;
        call_pos(_interp, cls, &[context.0])
    }

    fn make_scanner(context: PyValue) -> Result<Value> {
        let scanner_mod = _interp.load_module("json.scanner")?;
        let cls = _interp.get_attr(&scanner_mod, "_Scanner")?;
        call_pos(_interp, cls, &[context.0])
    }

    fn c_make_scanner(context: PyValue) -> Result<Value> {
        let scanner_mod = _interp.load_module("json.scanner")?;
        let cls = _interp.get_attr(&scanner_mod, "_Scanner")?;
        call_pos(_interp, cls, &[context.0])
    }
}
