// `fractions` module — body for the `fractions` entry in
// `pyrust_builtin_modules!` (issue #2810).
//
// CPython's `fractions` exports the single public class `Fraction`, an
// infinite-precision rational number.  The whole surface is most naturally
// expressed in Python itself — `Fraction` is a class whose arithmetic routes
// through `math.gcd` and the interpreter's own operator machinery, which gives
// BigInt promotion and mixed-type (`int` / `float`) coercion for free.  We
// therefore ship `fractions` as a pure-Python module (`fractions_py.py`, a
// close port of CPython 3.12's `Lib/fractions.py`) exec'd once into a throwaway
// namespace at first import; the public names are copied onto the module by
// `inject_python_members`, wired from `env.rs::load_module`'s post-import hook
// (mirrors `operator` / `abc`, issues #2514 / #2612).
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import fractions` resolves to a real `PyModule`.
//
// Reference: <https://docs.python.org/3/library/fractions.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `fractions` member.  Exec'd once
/// at first import (see `inject_python_members`).
const FRACTIONS_PY_SOURCE: &str = include_str!("fractions_py.py");

/// Public names from `FRACTIONS_PY_SOURCE` exported onto the `fractions`
/// module.  Matches CPython 3.12's `fractions.__all__`.
const FRACTIONS_PY_EXPORTS: [&str; 1] = ["Fraction"];

/// Execute `FRACTIONS_PY_SOURCE` once and copy its public names onto the
/// `fractions` module's attribute map.  Called from the `fractions` post-load
/// hook in `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    // Pre-seed the exec namespace with the modules `fractions_py.py` depends on
    // (`math`, `functools`, `operator`, `re`).  We can't rely on module-level
    // `import` statements inside the exec'd source: a top-level `import X` in an
    // explicit-dict exec namespace binds the name where function-scope reads see
    // it but module-top-level reads don't, so `_RATIONAL_FORMAT = re.compile(…)`
    // would raise `NameError: name 're' is not defined`.  Seeding the modules
    // directly side-steps that pre-existing exec/import limitation.
    let deps: Vec<(&str, Value)> = ["math", "functools", "operator", "re"]
        .into_iter()
        .map(|name| interp.load_module(name).map(|m| (name, m)))
        .collect::<Result<_>>()?;
    ns.dict_with_mut(|d| {
        for (name, m) in &deps {
            d.insert(PyKey::str_from(name), m.clone());
        }
    });
    interp.exec_source(FRACTIONS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("fractions: exec namespace not a dict".into()))?;
    for name in FRACTIONS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            // The exec namespace's `__name__` defaults to `__main__`; override
            // `__module__` to `fractions` so `Fraction.__module__ == "fractions"`
            // and class reprs read `<class 'fractions.Fraction'>`, matching
            // CPython.
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("fractions"));
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
