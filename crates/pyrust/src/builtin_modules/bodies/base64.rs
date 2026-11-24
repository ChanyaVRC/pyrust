// `base64` module — body for the `base64 @inject` entry in
// `pyrust_builtin_modules!` (issue #3145).
//
// CPython 3.12's `base64` is a pure-Python module built on `binascii` and
// `struct`.  Shipping it as `base64_py.py` (a close port of CPython 3.12's
// `Lib/base64.py`) keeps all encoding variants (Base64, URL-safe, Base32,
// Base32hex, Base16, Base85, Ascii85, and legacy `encodebytes`/`decodebytes`
// and `encode`/`decode`) in lock-step with CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/base64.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `base64` module.
const BASE64_PY_SOURCE: &str = include_str!("base64_py.py");

/// Public and compatibility attributes exported onto the `base64` module.
const BASE64_PY_EXPORTS: [&str; 26] = [
    "encode",
    "decode",
    "encodebytes",
    "decodebytes",
    "b64encode",
    "b64decode",
    "b32encode",
    "b32decode",
    "b32hexencode",
    "b32hexdecode",
    "b16encode",
    "b16decode",
    "b85encode",
    "b85decode",
    "a85encode",
    "a85decode",
    "standard_b64encode",
    "standard_b64decode",
    "urlsafe_b64encode",
    "urlsafe_b64decode",
    "MAXLINESIZE",
    "MAXBINSIZE",
    "bytes_types",
    "binascii",
    "struct",
    "re",
];

/// Execute `BASE64_PY_SOURCE` once and copy its exported attributes onto the
/// `base64` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(BASE64_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("base64: exec namespace not a dict".into()))?;
    for name in BASE64_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
