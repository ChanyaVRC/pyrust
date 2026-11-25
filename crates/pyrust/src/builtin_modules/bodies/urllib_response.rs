// `urllib.response` module — body for the `"urllib.response" as urllib_response @inject`
// entry in `pyrust_builtin_modules!` (issue #3223).
//
// Pure-Python response wrapper classes (`addbase`, `addclosehook`, `addinfo`,
// `addinfourl`) aligned with CPython 3.12's `Lib/urllib/response.py`.
//
// Reference: <https://docs.python.org/3/library/urllib.request.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `urllib.response` module.
const URLLIB_RESPONSE_PY_SOURCE: &str = include_str!("urllib_response_py.py");

/// Public names exported from `URLLIB_RESPONSE_PY_SOURCE`.
const URLLIB_RESPONSE_PY_EXPORTS: &[&str] = &[
    "__all__",
    "addbase",
    "addclosehook",
    "addinfo",
    "addinfourl",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(URLLIB_RESPONSE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("urllib.response: exec namespace not a dict".into()))?;
    for &name in URLLIB_RESPONSE_PY_EXPORTS {
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
