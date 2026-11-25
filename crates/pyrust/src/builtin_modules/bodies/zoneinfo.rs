// `zoneinfo` module — body for the `zoneinfo @inject` entry in
// `pyrust_builtin_modules!` (issue #3226, PEP 615).
//
// Pure-Python implementation of Python 3.12's `zoneinfo` package
// (`zoneinfo_py.py`): full RFC 8536 TZif v1/v2/v3/v4 binary parser,
// POSIX TZ string evaluator (including TZif v3 extended hours and
// southern hemisphere rules), PEP 495 fold/gap disambiguation,
// `ZoneInfo` weak+strong caching (`no_cache`, `clear_cache`, `from_file`),
// `TZPATH`/`reset_tzpath`/`available_timezones`, `ZoneInfoNotFoundError`,
// `InvalidTZPathWarning`, pickle support, and built-in IANA fallback rules
// for cross-platform support on Windows when system zoneinfo is absent.
//
// Reference: <https://docs.python.org/3.12/library/zoneinfo.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `zoneinfo` package.
const ZONEINFO_PY_SOURCE: &str = include_str!("zoneinfo_py.py");

/// Public names exported from `ZONEINFO_PY_SOURCE` onto `zoneinfo`.
const ZONEINFO_PY_EXPORTS: &[&str] = &[
    "__all__",
    "__package__",
    "__path__",
    "TZPATH",
    "ZoneInfo",
    "ZoneInfoNotFoundError",
    "InvalidTZPathWarning",
    "reset_tzpath",
    "available_timezones",
    "_unpickle_zoneinfo",
];

/// Execute `ZONEINFO_PY_SOURCE` once and publish its exports onto `zoneinfo`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(
        ZONEINFO_PY_SOURCE,
        "<frozen zoneinfo>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("zoneinfo: exec namespace not a dict".into()))?;
    for &name in ZONEINFO_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("zoneinfo"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    drop(dict);
    Ok(Some(ns))
}

pyrust_module! {}
