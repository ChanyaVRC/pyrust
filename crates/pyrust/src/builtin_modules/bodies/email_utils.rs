// `email.utils` module — body for the `"email.utils" as email_utils @inject`
// entry in `pyrust_builtin_modules!` (issue #3211).
//
// Pure-Python implementation of CPython 3.12's `email.utils` module
// (`email_utils_py.py`): RFC 2822 address parsing/formatting (`parseaddr`,
// `formataddr`, `getaddresses`), date parsing/formatting (`parsedate`,
// `parsedate_tz`, `parsedate_to_datetime`, `formatdate`, `format_datetime`,
// `mktime_tz`, `localtime`), `make_msgid`, `quote`/`unquote`, and RFC 2231
// parameter helpers (`encode_rfc2231`, `decode_rfc2231`, `decode_params`,
// `collapse_rfc2231_value`).
//
// Reference: <https://docs.python.org/3/library/email.utils.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for `email.utils`.
const EMAIL_UTILS_PY_SOURCE: &str = include_str!("email_utils_py.py");

/// Public and internal names exported onto `email.utils`.
const EMAIL_UTILS_PY_EXPORTS: &[&str] = &[
    "__all__",
    "COMMASPACE",
    "EMPTYSTRING",
    "UEMPTYSTRING",
    "CRLF",
    "TICK",
    "specialsre",
    "escapesre",
    "rfc2231_continuation",
    "AddrlistClass",
    "AddressList",
    "parseaddr",
    "formataddr",
    "getaddresses",
    "parsedate",
    "parsedate_tz",
    "_parsedate_tz",
    "parsedate_to_datetime",
    "formatdate",
    "format_datetime",
    "mktime_tz",
    "make_msgid",
    "quote",
    "unquote",
    "localtime",
    "encode_rfc2231",
    "decode_rfc2231",
    "decode_params",
    "collapse_rfc2231_value",
    "_has_surrogates",
    "_sanitize",
    "_Charset",
    "_QUOPRI_HEADER_MAP",
    "_QUOPRI_BODY_MAP",
    "_quopri_header_encode",
    "_quopri_header_decode",
    "_quopri_header_length",
    "_quopri_body_encode",
    "_quopri_body_decode",
    "_b64_header_encode",
    "_b64_header_length",
    "_b64_body_encode",
    "_b64_decode",
    "_decode_b_with_defects",
    "_CHARSET_ALIASES",
    "_UNKNOWN8BIT",
    "_QP",
    "_BASE64",
    "_SHORTEST",
    "_encode_with_codec",
    "_MessageError",
    "_MessageParseError",
    "_HeaderParseError",
    "_CharsetError",
];

/// Execute `EMAIL_UTILS_PY_SOURCE` once and copy its exports onto `email.utils`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(EMAIL_UTILS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("email.utils: exec namespace not a dict".into()))?;
    for name in EMAIL_UTILS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("email.utils"));
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
