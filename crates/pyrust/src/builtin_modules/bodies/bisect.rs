// `bisect` module — body for the `bisect` entry in `pyrust_builtin_modules!`
// (issue #2784).
//
// CPython's `bisect` is most naturally expressed in Python itself: it is a
// handful of array-bisection helpers built on `__lt__` comparisons and list
// `insert`.  Shipping it as a pure-Python module (`bisect_py.py`, a close port
// of CPython 3.12's `Lib/bisect.py`) keeps the comparison semantics, the
// `key=` handling (Python 3.10+), and error wording in lock-step with the
// reference.
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import bisect` resolves to a real `PyModule`.  The public names are
// copied onto the module by `inject_python_members`, wired from
// `env.rs::load_module`'s post-import hook (mirrors `operator` / `json`).
//
// Reference: <https://docs.python.org/3/library/bisect.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `bisect` member.  Exec'd once at
/// first import (see `inject_python_members`).
const BISECT_PY_SOURCE: &str = include_str!("bisect_py.py");

/// Public names from `BISECT_PY_SOURCE` exported onto the `bisect` module.
/// CPython 3.12's `bisect` module defines no `__all__`; this list is its set
/// of public (non-underscore) names.
const BISECT_PY_EXPORTS: [&str; 6] = [
    "bisect",
    "bisect_left",
    "bisect_right",
    "insort",
    "insort_left",
    "insort_right",
];

/// Execute `BISECT_PY_SOURCE` once and copy its public names onto the `bisect`
/// module's attribute map.  Called from the `bisect` post-load hook in
/// `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(BISECT_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("bisect: exec namespace not a dict".into()))?;
    for name in BISECT_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(())
}

pyrust_module! {}
