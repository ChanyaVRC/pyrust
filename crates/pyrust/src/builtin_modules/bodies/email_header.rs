// `email.header` module — body for the `"email.header" as email_header @inject`
// entry in `pyrust_builtin_modules!` (issue #3211).
//
// Pure-Python implementation of CPython 3.12's `email.header` module
// (`email_header_py.py`): `Header`, `decode_header`, and `make_header` with
// RFC 2047 B/Q encoded-word encoding/decoding and RFC 2822 header folding.
//
// Reference: <https://docs.python.org/3/library/email.header.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for `email.header`.
const EMAIL_HEADER_PY_SOURCE: &str = include_str!("email_header_py.py");

/// Public names exported onto `email.header`.
const EMAIL_HEADER_PY_EXPORTS: &[&str] = &[
    "__all__",
    "Header",
    "decode_header",
    "make_header",
    "HeaderParseError",
    "MAXLINELEN",
    "USASCII",
    "UTF8",
    "ecre",
    "fcre",
];

/// Execute `EMAIL_HEADER_PY_SOURCE` once and copy its exports onto `email.header`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(EMAIL_HEADER_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("email.header: exec namespace not a dict".into()))?;
    for name in EMAIL_HEADER_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("email.header"));
            }
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
