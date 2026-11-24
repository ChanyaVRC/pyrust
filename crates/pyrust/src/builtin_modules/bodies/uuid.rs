// `uuid` module — body for the `uuid @inject` entry in
// `pyrust_builtin_modules!` (issue #3185).
//
// CPython 3.12's `uuid` is a pure-Python module providing RFC 4122 UUID
// objects and version 1/3/4/5 generators backed by `hashlib`, `os.urandom`,
// `time`, and `enum`.
//
// Reference: <https://docs.python.org/3/library/uuid.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `uuid` module.
const UUID_PY_SOURCE: &str = include_str!("uuid_py.py");

/// Public names exported from `UUID_PY_SOURCE` onto the `uuid` module.
const UUID_PY_EXPORTS: &[&str] = &[
    "RESERVED_NCS",
    "RFC_4122",
    "RESERVED_MICROSOFT",
    "RESERVED_FUTURE",
    "int_",
    "bytes_",
    "SafeUUID",
    "UUID",
    "getnode",
    "uuid1",
    "uuid3",
    "uuid4",
    "uuid5",
    "NAMESPACE_DNS",
    "NAMESPACE_URL",
    "NAMESPACE_OID",
    "NAMESPACE_X500",
];

/// Execute `UUID_PY_SOURCE` once and copy its exported attributes onto the
/// `uuid` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(UUID_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("uuid: exec namespace not a dict".into()))?;
    for name in UUID_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("uuid"));
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
