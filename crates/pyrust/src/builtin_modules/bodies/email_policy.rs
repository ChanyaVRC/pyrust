// `email.policy` module — body for the `"email.policy" as email_policy @inject`
// entry in `pyrust_builtin_modules!` (issue #3211).
//
// Pure-Python implementation of CPython 3.12's `email.policy` module
// (`email_policy_py.py`): `Policy`, `Compat32`, `compat32`, `EmailPolicy`,
// `default`, `strict`, `SMTP`, `SMTPUTF8`, `HTTP`, and the default
// `ContentManager` / `raw_data_manager`.
//
// Reference: <https://docs.python.org/3/library/email.policy.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for `email.policy`.
const EMAIL_POLICY_PY_SOURCE: &str = include_str!("email_policy_py.py");

/// Public and internal names exported onto `email.policy`.
const EMAIL_POLICY_PY_EXPORTS: &[&str] = &[
    "__all__",
    "Policy",
    "Compat32",
    "compat32",
    "EmailPolicy",
    "default",
    "strict",
    "SMTP",
    "SMTPUTF8",
    "HTTP",
    "ContentManager",
    "raw_data_manager",
];

/// Execute `EMAIL_POLICY_PY_SOURCE` once and copy its exports onto `email.policy`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(EMAIL_POLICY_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("email.policy: exec namespace not a dict".into()))?;
    for name in EMAIL_POLICY_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if *name != "ContentManager"
                && let ValueKind::PyClass(cls_rc) = val.kind()
            {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("email.policy"));
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
