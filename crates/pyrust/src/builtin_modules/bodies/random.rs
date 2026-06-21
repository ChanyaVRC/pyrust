// `random` module — body for the `random` entry in `pyrust_builtin_modules!`
// (issue #2785).
//
// CPython's `random` is a thin Python layer over the `_random.Random` C
// extension (MT19937).  pyrust has no C-extension layer, so the whole module
// is shipped as pure Python (`random_py.py`): a reference MT19937 core plus the
// documented public API (`random`, `randint`, `choice`, `shuffle`, `sample`,
// `gauss`, …).  Default (None) seeding draws OS entropy via `os.urandom`.
//
// The numeric draws do NOT match CPython for the same seed (CPython seeds via
// `init_by_array`; pyrust uses scalar `init_genrand`), but the API contract —
// types, ranges, reproducibility with a fixed seed, uniqueness of `sample` — is
// preserved.  Parity fixtures assert the contract, not the exact bytes.
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import random` resolves to a real `PyModule`.  The public names are
// copied onto the module by `inject_python_members`, wired from
// `env.rs::load_module`'s post-import hook (mirrors `operator` / `json`).
//
// Reference: <https://docs.python.org/3/library/random.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `random` member.  Exec'd once at
/// first import (see `inject_python_members`).
const RANDOM_PY_SOURCE: &str = include_str!("random_py.py");

/// Public names from `RANDOM_PY_SOURCE` exported onto the `random` module.
const RANDOM_PY_EXPORTS: [&str; 24] = [
    "__all__",
    "Random",
    "seed",
    "random",
    "uniform",
    "randint",
    "randrange",
    "choice",
    "choices",
    "shuffle",
    "sample",
    "getstate",
    "setstate",
    "getrandbits",
    "gauss",
    "normalvariate",
    "expovariate",
    "triangular",
    "betavariate",
    "gammavariate",
    "lognormvariate",
    "paretovariate",
    "weibullvariate",
    "vonmisesvariate",
];

/// Execute `RANDOM_PY_SOURCE` once and copy its public names onto the `random`
/// module's attribute map.  Called from the `random` post-load hook in
/// `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(RANDOM_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("random: exec namespace not a dict".into()))?;
    for name in RANDOM_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            // `Random` renders its repr/qualname via `__module__`; the exec
            // namespace's `__name__` defaults to `__main__`, so override it to
            // `random` so `random.Random` reports the right module.
            if name == "Random"
                && let ValueKind::PyClass(cls_rc) = val.kind()
            {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("random"));
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
