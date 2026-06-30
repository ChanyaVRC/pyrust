// `csv` module — body for the `csv` entry in `pyrust_builtin_modules!`
// (issue #2808).
//
// CPython's `csv` package centres on a character-level reader/writer state
// machine and a couple of dialect/dict-helper classes.  None of that needs
// interpreter internals, so pyrust ships it as a pure-Python module
// (`csv_py.py`, targeting CPython 3.12 behaviour) which keeps the parsing
// rules, quoting modes, and the `\r\n` line terminator in lock-step with the
// reference.
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import csv` resolves to a real `PyModule`.  The public names are copied
// onto the module by `inject_python_members`, wired from `env.rs::load_module`'s
// post-import hook (mirrors `operator` / `string` / `json`).
//
// Reference: <https://docs.python.org/3/library/csv.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `csv` member.  Exec'd once at
/// first import (see `inject_python_members`).
const CSV_PY_SOURCE: &str = include_str!("csv_py.py");

/// Public names from `CSV_PY_SOURCE` exported onto the `csv` module.
const CSV_PY_EXPORTS: [&str; 17] = [
    "QUOTE_MINIMAL",
    "QUOTE_ALL",
    "QUOTE_NONNUMERIC",
    "QUOTE_NONE",
    "Error",
    "Dialect",
    "excel",
    "excel_tab",
    "unix_dialect",
    "reader",
    "writer",
    "DictReader",
    "DictWriter",
    "register_dialect",
    "unregister_dialect",
    "get_dialect",
    "list_dialects",
];

/// Execute `CSV_PY_SOURCE` once and copy its public names onto the `csv`
/// module's attribute map.  Called from the `csv` post-load hook in
/// `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(CSV_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("csv: exec namespace not a dict".into()))?;
    for name in CSV_PY_EXPORTS {
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
