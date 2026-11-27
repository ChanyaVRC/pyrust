//! Interpreter glue shared by the native `json` package: calls, `str`
//! extraction, `JSONDecodeError` construction and small protocol helpers.

use std::borrow::Cow;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, builtin_data_backing, value_type_name_str};
use crate::value::{Value, ValueKind};

/// Call `callee(*positional, **keywords)`.
pub(crate) fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    positional: &[Value],
    keywords: &[(String, Value)],
) -> Result<Value> {
    interp.call_function_expanded(callee, &super::args::expand(positional, keywords))
}

/// Call `callee(*positional)`.
pub(crate) fn call(interp: &mut Interpreter, callee: Value, positional: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = positional
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

/// Call `target.name(*positional)`.
pub(crate) fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    positional: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call(interp, callee, positional)
}

/// `getattr(builtins, name)`.
pub(crate) fn builtin(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let builtins = interp.load_module("builtins")?;
    interp.get_attr(&builtins, name)
}

/// `Py_TYPE(obj)->tp_name` as used in CPython's messages.
pub(crate) fn type_name(value: &Value) -> String {
    value_type_name_str(value).into_owned()
}

/// `obj.__class__.__name__`, as the pure-Python modules spell type names.
pub(crate) fn class_name(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let cls = interp.get_attr(value, "__class__")?;
    let name = interp.get_attr(&cls, "__name__")?;
    Ok(str_text(&name).map_or_else(|| type_name(value), Cow::into_owned))
}

/// The text of an exact `str` or a `str` subclass instance (`PyUnicode_Check`).
pub(crate) fn str_text(value: &Value) -> Option<Cow<'_, str>> {
    if let Some(text) = value.as_str() {
        return Some(Cow::Borrowed(text));
    }
    if !matches!(value.kind(), ValueKind::PyInstance(_)) {
        return None;
    }
    let backing = builtin_data_backing(value)?;
    backing.as_str().map(|text| Cow::Owned(text.to_owned()))
}

/// The exact `str` object holding the text of a `str` / `str` subclass value.
pub(crate) fn exact_str(value: &Value) -> Option<Value> {
    if value.as_str().is_some() {
        return Some(value.clone());
    }
    if !matches!(value.kind(), ValueKind::PyInstance(_)) {
        return None;
    }
    builtin_data_backing(value).filter(|backing| backing.as_str().is_some())
}

/// Builtin backing of a builtin-subclass instance, or the value itself.
pub(crate) fn unwrap_backing(value: &Value) -> Cow<'_, Value> {
    if matches!(value.kind(), ValueKind::PyInstance(_))
        && let Some(backing) = builtin_data_backing(value)
    {
        return Cow::Owned(backing);
    }
    Cow::Borrowed(value)
}

/// `a, b = value` for a two-target unpacking.
pub(crate) fn unpack2(interp: &mut Interpreter, value: &Value) -> Result<(Value, Value)> {
    let items: Vec<Value> = match value.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        ValueKind::List(items) => items.clone(),
        _ => match interp.collect_iterable(value) {
            Ok(items) => items,
            Err(err) if err.class_name_is("TypeError") && !is_iterable(interp, value) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("cannot unpack non-iterable {} object", type_name(value)),
                ));
            }
            Err(err) => return Err(err),
        },
    };
    match items.len() {
        2 => {
            let mut it = items.into_iter();
            Ok((it.next().expect("two items"), it.next().expect("two items")))
        }
        n if n > 2 => Err(PyError::named(
            "ValueError",
            "too many values to unpack (expected 2)".to_string(),
        )),
        n => Err(PyError::named(
            "ValueError",
            format!("not enough values to unpack (expected 2, got {n})"),
        )),
    }
}

fn is_iterable(interp: &mut Interpreter, value: &Value) -> bool {
    interp.get_attr(value, "__iter__").is_ok() || interp.get_attr(value, "__getitem__").is_ok()
}

/// `''.join(chunks)` with `str.join`'s `TypeError` wording.
pub(crate) fn join_chunks(chunks: &[Value]) -> Result<Value> {
    if let [only] = chunks
        && only.as_str().is_some()
    {
        return Ok(only.clone());
    }
    let mut out = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        match str_text(chunk) {
            Some(text) => out.push_str(&text),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        type_name(chunk)
                    ),
                ));
            }
        }
    }
    Ok(Value::string(out))
}

/// `list(iterable)`.
pub(crate) fn collect(interp: &mut Interpreter, iterable: &Value) -> Result<Vec<Value>> {
    interp.collect_iterable(iterable)
}

/// `StopIteration.value` when `err` is a `StopIteration`.
pub(crate) fn stop_iteration_value(interp: &mut Interpreter, err: &PyError) -> Option<Value> {
    if !crate::interpreter::is_stop_iteration_error(err) {
        return None;
    }
    let exc = interp.materialize_pyerror(err.clone()).ok()?;
    Some(
        interp
            .get_attr(&exc, "value")
            .unwrap_or_else(|_| Value::none()),
    )
}

/// `raise StopIteration(idx)`.
pub(crate) fn stop_iteration(interp: &mut Interpreter, idx: Value) -> PyError {
    if let Some(cls) = crate::interpreter::lookup_exc_class("StopIteration")
        && let Ok(inst) = call(interp, Value::py_class(cls), std::slice::from_ref(&idx))
    {
        return PyError::raised(inst);
    }
    PyError::named("StopIteration", idx.repr_raw())
}

/// `json.decoder.JSONDecodeError(msg, doc, pos)`, looked up on the module at
/// raise time like `_json.c`'s `raise_errmsg`.
pub(crate) fn decode_error(
    interp: &mut Interpreter,
    msg: &str,
    doc: &Value,
    pos: Value,
) -> PyError {
    let made = (|| -> Result<Value> {
        let decoder = interp.load_module("json.decoder")?;
        let cls = interp.get_attr(&decoder, "JSONDecodeError")?;
        call(interp, cls, &[Value::string(msg), doc.clone(), pos])
    })();
    match made {
        Ok(inst) => PyError::raised(inst),
        Err(err) => err,
    }
}

/// `PyNumber_AsSsize_t(value, PyExc_OverflowError)` after `__index__`
/// (the C `n` argument format).
pub(crate) fn ssize_index(interp: &mut Interpreter, value: &Value) -> Result<i64> {
    use num_traits::ToPrimitive;
    let resolved = interp.value_to_index(value, |v| {
        PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(v)
            ),
        )
    })?;
    match resolved.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        ValueKind::BigInt(n) => n.to_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(value)
            ),
        )),
    }
}

/// `RecursionError(message)`.
pub(crate) fn recursion_error(message: &str) -> PyError {
    PyError::named("RecursionError", message.to_string())
}

/// `Py_C_RECURSION_LIMIT` of CPython 3.12, which bounds `_json`'s nesting.
#[cfg(windows)]
pub(crate) const C_RECURSION_LIMIT: usize = 3000;
#[cfg(not(windows))]
pub(crate) const C_RECURSION_LIMIT: usize = 8000;

/// Run `f` on a stack segment with room for one more level of a recursive
/// native walk (the `_json` C recursion is bounded by [`C_RECURSION_LIMIT`],
/// far deeper than the interpreter thread's native stack allows in debug
/// builds).
#[inline]
pub(crate) fn with_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, f)
}
