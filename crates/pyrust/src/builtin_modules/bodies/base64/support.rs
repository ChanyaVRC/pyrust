// Interpreter-facing helpers shared by the `base64` submodules: argument
// conversion (`_bytes_from_decode_data`, `memoryview(s).tobytes()`), call
// helpers, and the `binascii.Error` / `AssertionError` constructors.

use std::rc::Rc;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBytesLike};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{Value, ValueKind};

pub(super) fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

pub(super) fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

pub(super) fn call_with(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    expanded.extend(kw.iter().map(|(name, value)| ExpandedCallArg {
        name: Some((*name).to_string()),
        value: value.clone(),
    }));
    interp.call_function_expanded(callee, &expanded)
}

pub(super) fn call_pos(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    call_with(interp, callee, pos, &[])
}

pub(super) fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, pos)
}

pub(super) fn builtin_call(
    interp: &mut Interpreter,
    name: &'static str,
    pos: &[Value],
) -> Result<Value> {
    call_pos(interp, Value::builtin_function(name), pos)
}

/// A module global of `base64` (`binascii`, `b64encode`, ...), resolved at
/// call time so reassigning it on the module is observed like in CPython.
pub(super) fn module_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("base64")?;
    interp.get_attr(&module, name)
}

/// `binascii.<name>(*pos, **kw)` through the module's `binascii` global.
pub(super) fn binascii_call(
    interp: &mut Interpreter,
    name: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let binascii = module_attr(interp, "binascii")?;
    let func = interp.get_attr(&binascii, name)?;
    call_with(interp, func, pos, kw)
}

/// `raise binascii.Error(msg)`.
pub(super) fn binascii_error(interp: &mut Interpreter, msg: &str) -> PyError {
    if let Ok(binascii) = module_attr(interp, "binascii")
        && let Ok(cls) = interp.get_attr(&binascii, "Error")
        && let ValueKind::PyClass(cls_rc) = cls.kind()
    {
        return PyError::raised(crate::interpreter::instantiate_exception(
            Rc::clone(cls_rc),
            vec![Value::string(msg)],
        ));
    }
    value_error(msg)
}

/// `assert <cond>, <msg>` failing.
pub(super) fn assertion_error(msg: String) -> PyError {
    PyError::named("AssertionError", msg)
}

/// `raise <class_name>(message) from None`.
pub(super) fn error_from_none(
    interp: &mut Interpreter,
    class_name: &'static str,
    message: &str,
) -> PyError {
    let error = PyError::named(class_name, message.to_string());
    let Ok(exception) = interp.materialize_pyerror(error) else {
        return PyError::named(class_name, message.to_string());
    };
    let _ = interp.assign_attr(exception.clone(), "__cause__", Value::none());
    let _ = interp.assign_attr(
        exception.clone(),
        "__suppress_context__",
        Value::bool_(true),
    );
    PyError::raised(exception)
}

/// `str` content of a `str` or `str` subclass instance.
fn str_value(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        return Some(s.to_string());
    }
    let backing = crate::interpreter::builtin_data_backing(value)?;
    backing.as_str().map(str::to_string)
}

/// `isinstance(value, (bytes, bytearray))`.
pub(super) fn is_bytes_type_instance(value: &Value) -> bool {
    if matches!(value.kind(), ValueKind::Bytes(_))
        || pyrust_builtins::bytearray::as_bytearray_rc(value).is_some()
    {
        return true;
    }
    crate::interpreter::builtin_data_backing(value).is_some_and(|backing| {
        matches!(backing.kind(), ValueKind::Bytes(_))
            || pyrust_builtins::bytearray::as_bytearray_rc(&backing).is_some()
    })
}

/// `repr(value)` as a Rust string.
pub(super) fn repr_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let repr = builtin_call(interp, "repr", std::slice::from_ref(value))?;
    interp.render_value_as_str(&repr)
}

/// `len(value)`.
pub(super) fn len_of(interp: &mut Interpreter, value: &Value) -> Result<usize> {
    if let ValueKind::Bytes(rc) = value.kind() {
        return Ok(rc.len());
    }
    let n = builtin_call(interp, "len", std::slice::from_ref(value))?;
    Ok(n.as_int().unwrap_or(0).max(0) as usize)
}

/// `_bytes_from_decode_data(s)`: ASCII `str` or any bytes-like object.
pub(super) fn bytes_from_decode_data(interp: &mut Interpreter, s: &Value) -> Result<Vec<u8>> {
    if let Some(text) = str_value(s) {
        if text.is_ascii() {
            return Ok(text.into_bytes());
        }
        return Err(value_error(
            "string argument should contain only ASCII characters",
        ));
    }
    if PyBytesLike::matches(s) {
        return Ok(PyBytesLike::try_from_value_with_interp(interp, s, "base64", "s")?.into_vec());
    }
    Err(error_from_none(
        interp,
        "TypeError",
        &format!(
            "argument should be a bytes-like object or ASCII string, not '{}'",
            value_type_name_str(s)
        ),
    ))
}

/// `memoryview(s).tobytes()` for an encoder argument.
pub(super) fn bytes_from_buffer(interp: &mut Interpreter, s: &Value) -> Result<Vec<u8>> {
    if PyBytesLike::matches(s) {
        return Ok(PyBytesLike::try_from_value_with_interp(interp, s, "base64", "s")?.into_vec());
    }
    Err(type_error(format!(
        "memoryview: a bytes-like object is required, not '{}'",
        value_type_name_str(s)
    )))
}

/// `a.__contains__`-style membership: `item in container`.
pub(super) fn contains(interp: &mut Interpreter, item: Value, container: &Value) -> Result<bool> {
    let r = interp.eval_binary(item, BinaryOp::In, container.clone())?;
    interp.truthy_value(&r)
}

/// `bytes.maketrans(from, to)` as a translation table (later pairs win).
pub(super) fn translation_table(from: &[u8], to: &[u8]) -> [u8; 256] {
    let mut table = [0u8; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        *slot = i as u8;
    }
    for (&f, &t) in from.iter().zip(to) {
        table[f as usize] = t;
    }
    table
}

/// `data.translate(table)`.
pub(super) fn translate(data: &[u8], table: &[u8; 256]) -> Vec<u8> {
    data.iter().map(|&b| table[b as usize]).collect()
}
