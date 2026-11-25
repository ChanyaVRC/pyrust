// `email.message` module — body for the `"email.message" as email_message
// @inject` entry in `pyrust_builtin_modules!` (issue #3211).
//
// Pure-Python implementation of CPython 3.12's `email.message` module
// (`email_message_py.py`): `Message`, `MIMEPart`, and `EmailMessage` with
// case-insensitive header mapping, MIME parameter handling, base64 /
// quoted-printable / uuencode payload decoding, multipart construction,
// `walk()`, `as_string()` / `as_bytes()`, and high-level `EmailMessage`
// helpers (`get_content`, `set_content`, `get_body`, `iter_parts`,
// `iter_attachments`, `add_alternative`, `add_attachment`).
//
// Reference: <https://docs.python.org/3/library/email.message.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for `email.message`.
const EMAIL_MESSAGE_PY_SOURCE: &str = include_str!("email_message_py.py");

/// Public and compatibility names exported onto `email.message`.
const EMAIL_MESSAGE_PY_EXPORTS: &[&str] = &[
    "__all__",
    "Message",
    "MIMEPart",
    "EmailMessage",
    "SEMISPACE",
    "tspecials",
    "_splitparam",
    "_formatparam",
    "_parseparam",
    "_unquotevalue",
];

/// Execute `EMAIL_MESSAGE_PY_SOURCE` once and copy its exports onto `email.message`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(EMAIL_MESSAGE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("email.message: exec namespace not a dict".into()))?;
    for name in EMAIL_MESSAGE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("email.message"));
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
