// `statistics` module — body for the `statistics` entry in
// `pyrust_builtin_modules!` (issue #2811).
//
// CPython's `statistics` module is pure Python.  It internally relies on
// `fractions.Fraction` for exact rational arithmetic, but pyrust ships
// neither `fractions` nor `decimal`, so `statistics_py.py` is a float-based
// adaptation that still returns an exact `int` from `mean` when the inputs
// are integral and divide evenly (the load-bearing parity case
// `statistics.mean([1, 2, 3, 4, 5]) == 3`).
//
// The native `pyrust_module!` block is intentionally empty — it exists only
// so the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is
// generated and `import statistics` resolves to a real `PyModule`.  The
// public names are copied onto the module by `inject_python_members`, wired
// from `env.rs::load_module`'s post-import hook (mirrors `json` / `string`).
//
// Reference: <https://docs.python.org/3/library/statistics.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `statistics` member.  Exec'd
/// once at first import (see `inject_python_members`).
const STATISTICS_PY_SOURCE: &str = include_str!("statistics_py.py");

/// Public names from `STATISTICS_PY_SOURCE` exported onto the `statistics`
/// module.
const STATISTICS_PY_EXPORTS: [&str; 16] = [
    "StatisticsError",
    "mean",
    "fmean",
    "geometric_mean",
    "harmonic_mean",
    "median",
    "median_low",
    "median_high",
    "median_grouped",
    "mode",
    "multimode",
    "pstdev",
    "pvariance",
    "stdev",
    "variance",
    "NormalDist",
];

/// Execute `STATISTICS_PY_SOURCE` once and copy its public names onto the
/// `statistics` module's attribute map.  Called from the `statistics`
/// post-load hook in `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(STATISTICS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("statistics: exec namespace not a dict".into()))?;
    for name in STATISTICS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            // Classes render their repr/qualname via `__module__`; the exec
            // namespace's `__name__` defaults to `__main__`, so override it
            // to `statistics` so `repr` and qualname match CPython.
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("statistics"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(())
}

pyrust_module! {}
