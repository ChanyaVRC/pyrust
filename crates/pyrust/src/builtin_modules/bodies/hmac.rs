// `hmac` module — body for the `hmac @inject` entry in
// `pyrust_builtin_modules!` (issue #3146).
//
// Provides constant-time `compare_digest` and native one-shot HMAC fast path
// in Rust; the public CPython 3.12 `HMAC`, `new`, `digest`, `trans_36`,
// `trans_5C`, and `digest_size` surface is injected from `hmac_py.py`.
//
// Reference: <https://docs.python.org/3/library/hmac.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, reject_keyword_args_expanded,
    value_type_name_str,
};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the public `hmac` surface.
const HMAC_PY_SOURCE: &str = include_str!("hmac_py.py");

/// Private native helpers seeded into the injected exec namespace and removed
/// from `hmac.__dict__` after injection.
const HMAC_NATIVE_HELPERS: [&str; 2] = ["_compare_digest", "_hmac_digest_fast"];

/// Public names exported from `HMAC_PY_SOURCE` onto the `hmac` module.
const HMAC_PY_EXPORTS: [&str; 7] = [
    "HMAC",
    "new",
    "digest",
    "compare_digest",
    "trans_36",
    "trans_5C",
    "digest_size",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in HMAC_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(HMAC_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("hmac: exec namespace not a dict".into()))?;
    for name in HMAC_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("hmac"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

fn try_extract_str(val: &Value) -> Option<String> {
    if let ValueKind::Str(s) = val.kind() {
        return Some(s.to_string());
    }
    if let Some(backing) = builtin_data_backing(val)
        && let ValueKind::Str(s) = backing.kind()
    {
        return Some(s.to_string());
    }
    None
}

/// Constant-time byte slice equality matching CPython's `_Py_compare_digest`.
/// When lengths differ, compares `b` against itself so runtime depends only on
/// `b.len()`, and `black_box` prevents short-circuit optimization.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff: usize = a.len() ^ b.len();
    let left = if a.len() == b.len() { a } else { b };
    for (&x, &y) in left.iter().zip(b.iter()) {
        diff |= usize::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
}

pyrust_module! {
    /// `_compare_digest(a, b)` -> `bool`.
    fn _compare_digest(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let a = &args[0].value;
        let b = &args[1].value;

        if let (Some(sa), Some(sb)) = (try_extract_str(a), try_extract_str(b)) {
            if !sa.is_ascii() || !sb.is_ascii() {
                return Err(PyError::named(
                    "TypeError",
                    "comparing strings with non-ASCII characters is not supported".to_string(),
                ));
            }
            return Ok(Value::bool_(constant_time_eq(sa.as_bytes(), sb.as_bytes())));
        }

        let ba = super::hashlib::try_extract_bytes_like(a);
        let bb = super::hashlib::try_extract_bytes_like(b);
        match (ba, bb) {
            (Some(bytes_a), Some(bytes_b)) => {
                Ok(Value::bool_(constant_time_eq(&bytes_a, &bytes_b)))
            }
            (Some(_), None) => Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    value_type_name_str(b)
                ),
            )),
            (None, Some(_)) => Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    value_type_name_str(a)
                ),
            )),
            (None, None) => Err(PyError::named(
                "TypeError",
                format!(
                    "unsupported operand types(s) or combination of types: '{}' and '{}'",
                    value_type_name_str(a),
                    value_type_name_str(b)
                ),
            )),
        }
    }

    /// `_hmac_digest_fast(key, msg, digest_name)` -> `bytes | None`.
    fn _hmac_digest_fast(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let key = super::hashlib::extract_bytes_like(&args[0].value)?;
        let msg = super::hashlib::extract_bytes_like(&args[1].value)?;
        let name = try_extract_str(&args[2].value).ok_or_else(|| {
            PyError::named("TypeError", "digest name must be a string".to_string())
        })?;
        match super::hashlib::hmac_oneshot_by_name(&name, &key, &msg)? {
            Some(mac) => Ok(Value::bytes(mac)),
            None => Ok(Value::none()),
        }
    }
}
