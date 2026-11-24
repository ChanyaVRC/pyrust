// `secrets` module — body for the `secrets @inject` entry in
// `pyrust_builtin_modules!` (issue #3146).
//
// CPython 3.12's `secrets` is a pure-Python module built on `base64`,
// `hmac.compare_digest`, and `random.SystemRandom`.
//
// Reference: <https://docs.python.org/3/library/secrets.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `secrets` module.
const SECRETS_PY_SOURCE: &str = include_str!("secrets_py.py");

/// Public names exported from `SECRETS_PY_SOURCE` onto the `secrets` module.
const SECRETS_PY_EXPORTS: [&str; 10] = [
    "__all__",
    "DEFAULT_ENTROPY",
    "SystemRandom",
    "choice",
    "randbelow",
    "randbits",
    "token_bytes",
    "token_hex",
    "token_urlsafe",
    "compare_digest",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(SECRETS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("secrets: exec namespace not a dict".into()))?;
    for name in SECRETS_PY_EXPORTS {
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
