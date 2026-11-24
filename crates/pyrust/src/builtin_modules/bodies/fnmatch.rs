// `fnmatch` module — body for the `fnmatch @inject` entry in
// `pyrust_builtin_modules!` (issue #3148).
//
// CPython 3.12's `fnmatch` is a pure-Python module built on `os`, `os.path`,
// and `re`.  Shipping it as `fnmatch_py.py` (a close port of CPython 3.12's
// `Lib/fnmatch.py`) keeps `fnmatch`, `fnmatchcase`, `filter`, and `translate`
// in lock-step with CPython 3.12 on both POSIX and Windows.
//
// Reference: <https://docs.python.org/3/library/fnmatch.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `fnmatch` module.
const FNMATCH_PY_SOURCE: &str = include_str!("fnmatch_py.py");

/// Public attributes exported onto the `fnmatch` module.
const FNMATCH_PY_EXPORTS: [&str; 4] = ["filter", "fnmatch", "fnmatchcase", "translate"];

/// Execute `FNMATCH_PY_SOURCE` once and copy its exported attributes onto the
/// `fnmatch` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(FNMATCH_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("fnmatch: exec namespace not a dict".into()))?;
    for name in FNMATCH_PY_EXPORTS {
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
