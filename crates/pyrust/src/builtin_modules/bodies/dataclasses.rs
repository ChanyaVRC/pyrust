// `dataclasses` module — body for the `dataclasses` entry in
// `pyrust_builtin_modules!` (issue #2610).
//
// CPython's `dataclasses` exports the `@dataclass` decorator and the
// `field` / `fields` / `asdict` / `astuple` / `replace` / `is_dataclass`
// helpers.  The decorator inspects `__annotations__`, generates `__init__` /
// `__repr__` / `__eq__` (and frozen guards) via `exec` — all naturally
// expressed in Python — so we ship it as a pure-Python module
// (`dataclasses_py.py`) exec'd once into a throwaway namespace at first import;
// the public names are copied onto the module by `inject_python_members`,
// wired from `env.rs::load_module`'s post-import hook (mirrors `operator` /
// `string`, issues #2514 / #2515).
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import dataclasses` resolves to a real `PyModule`.
//
// Reference: <https://docs.python.org/3/library/dataclasses.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::PyKey;
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `dataclasses` member.  Exec'd
/// once at first import (see `inject_python_members`).
const DATACLASSES_PY_SOURCE: &str = include_str!("dataclasses_py.py");

/// Public names from `DATACLASSES_PY_SOURCE` exported onto the `dataclasses`
/// module.  A minimal subset of CPython 3.12's `dataclasses.__all__`.
const DATACLASSES_PY_EXPORTS: [&str; 13] = [
    "MISSING",
    "KW_ONLY",
    "Field",
    "FrozenInstanceError",
    "asdict",
    "astuple",
    "dataclass",
    "field",
    "fields",
    "is_dataclass",
    "replace",
    "_MISSING_TYPE",
    "_KW_ONLY_TYPE",
];

/// Execute `DATACLASSES_PY_SOURCE` once and copy its public names onto the
/// `dataclasses` module's attribute map.  Called from the `dataclasses`
/// post-load hook in `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(DATACLASSES_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("dataclasses: exec namespace not a dict".into()))?;
    for name in DATACLASSES_PY_EXPORTS {
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
