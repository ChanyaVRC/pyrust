// `email.parser` module — body for the `"email.parser" as email_parser @inject`
// entry in `pyrust_builtin_modules!` (issue #3211).
//
// Pure-Python implementation of CPython 3.12's `email.parser` module
// (`email_parser_py.py`): `Parser`, `HeaderParser`, `BytesParser`,
// `BytesHeaderParser`, `FeedParser`, and `BytesFeedParser` supporting header
// unfolding, multipart boundaries (including nested multiparts),
// `message/rfc822`, and `headersonly=True`.
//
// Reference: <https://docs.python.org/3/library/email.parser.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for `email.parser`.
const EMAIL_PARSER_PY_SOURCE: &str = include_str!("email_parser_py.py");

/// Public names exported onto `email.parser`.
const EMAIL_PARSER_PY_EXPORTS: &[&str] = &[
    "__all__",
    "Parser",
    "HeaderParser",
    "BytesParser",
    "BytesHeaderParser",
    "FeedParser",
    "BytesFeedParser",
];

/// Execute `EMAIL_PARSER_PY_SOURCE` once and copy its exports onto `email.parser`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(EMAIL_PARSER_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("email.parser: exec namespace not a dict".into()))?;
    for name in EMAIL_PARSER_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("email.parser"));
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
