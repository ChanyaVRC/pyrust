// `pprint` module — body for the `pprint` entry in `pyrust_builtin_modules!`
// (issue #2812).
//
// CPython's `pprint` is pure Python: it is repr-formatting and recursion
// bookkeeping, neither of which needs interpreter internals.  Shipping it as a
// pure-Python module (`pprint_py.py`, transcribed from CPython 3.12's
// `Lib/pprint.py`) keeps the wrapping heuristics, recursion markers, and the
// `_safe_repr` readable/recursive flags in lock-step with the reference.
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import pprint` resolves to a real `PyModule`.  The public names are
// copied onto the module by `inject_python_members`, wired from
// `env.rs::load_module`'s post-import hook (mirrors `json` / `string`).
//
// Reference: <https://docs.python.org/3/library/pprint.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `pprint` member.  Exec'd once at
/// first import (see `inject_python_members`).
const PPRINT_PY_SOURCE: &str = include_str!("pprint_py.py");

/// Public names from `PPRINT_PY_SOURCE` exported onto the `pprint` module
/// (CPython's `pprint.__all__`).
const PPRINT_PY_EXPORTS: [&str; 7] = [
    "pprint",
    "pformat",
    "isreadable",
    "isrecursive",
    "saferepr",
    "PrettyPrinter",
    "pp",
];

/// Execute `PPRINT_PY_SOURCE` once and copy its public names onto the `pprint`
/// module's attribute map.  Called from the `pprint` post-load hook in
/// `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(PPRINT_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("pprint: exec namespace not a dict".into()))?;
    for name in PPRINT_PY_EXPORTS {
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
