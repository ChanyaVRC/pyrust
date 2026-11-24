// `glob` module — body for the `glob @inject` entry in
// `pyrust_builtin_modules!` (issue #3148).
//
// CPython 3.12's `glob` is a pure-Python module built on `os`, `os.path`,
// `fnmatch`, `re`, and `itertools`.  Shipping it as `glob_py.py` (a close port
// of CPython 3.12's `Lib/glob.py`) provides `glob`, `iglob`, `escape`,
// `has_magic`, `glob0`, and `glob1` with Python 3.12 `root_dir`, `recursive`,
// and `include_hidden` semantics.
//
// Reference: <https://docs.python.org/3/library/glob.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `glob` module.
const GLOB_PY_SOURCE: &str = include_str!("glob_py.py");

/// Public and compatibility attributes exported onto the `glob` module.
const GLOB_PY_EXPORTS: [&str; 6] = ["glob", "iglob", "escape", "has_magic", "glob0", "glob1"];

/// Execute `GLOB_PY_SOURCE` once and copy its exported attributes onto the
/// `glob` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(GLOB_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("glob: exec namespace not a dict".into()))?;
    for name in GLOB_PY_EXPORTS {
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
