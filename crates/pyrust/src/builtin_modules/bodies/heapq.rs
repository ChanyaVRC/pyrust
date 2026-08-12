// `heapq` module — body for the `heapq` entry in `pyrust_builtin_modules!`
// (issue #2784).
//
// CPython's `heapq` is most naturally expressed in Python itself: it is a set
// of binary-heap operations built on `__lt__` comparisons and list mutation.
// Shipping it as a pure-Python module (`heapq_py.py`, a close port of CPython
// 3.12's pure-Python `Lib/heapq.py` — the `_siftup` / `_siftdown` path, not the
// `_heapq` C accelerator) keeps the comparison semantics and tie-breaking in
// lock-step with the reference.
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import heapq` resolves to a real `PyModule`.  The public names are
// copied onto the module by `inject_python_members`, wired from
// `env.rs::load_module`'s post-import hook (mirrors `operator` / `json`).
//
// Reference: <https://docs.python.org/3/library/heapq.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `heapq` member.  Exec'd once at
/// first import (see `inject_python_members`).
const HEAPQ_PY_SOURCE: &str = include_str!("heapq_py.py");

/// Public names from `HEAPQ_PY_SOURCE` exported onto the `heapq` module.
/// Matches CPython 3.12's `heapq.__all__` exactly.
const HEAPQ_PY_EXPORTS: [&str; 8] = [
    "heappush",
    "heappop",
    "heapify",
    "heapreplace",
    "merge",
    "nlargest",
    "nsmallest",
    "heappushpop",
];

/// Execute `HEAPQ_PY_SOURCE` once and copy its public names onto the `heapq`
/// module's attribute map.  Called from the `heapq` post-load hook in
/// `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(HEAPQ_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("heapq: exec namespace not a dict".into()))?;
    for name in HEAPQ_PY_EXPORTS {
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
