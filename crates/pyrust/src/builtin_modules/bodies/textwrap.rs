// `textwrap` module — body for the `textwrap` entry in
// `pyrust_builtin_modules!` (issue #2786).
//
// CPython's `textwrap` is entirely pure Python (`Lib/textwrap.py`): the
// `TextWrapper` class plus the `wrap` / `fill` / `shorten` / `dedent` /
// `indent` convenience functions.  We ship it as a pure-Python module
// (`textwrap_py.py`, a close port of CPython 3.12) exec'd once into a
// throwaway namespace at first import; the public names are copied onto the
// module by `inject_python_members`, wired from `env.rs::load_module`'s
// post-import hook (mirrors `operator` / `string`, issues #2514 / #2515).
//
// Two adaptations versus upstream are required because pyrust's `re` engine
// lacks look-behind assertions and inline flags (see `textwrap_py.py`'s
// module docstring): the hyphen / em-dash chunking in `_split` is done in
// pure Python, and `dedent` passes `flags=re.MULTILINE` instead of `(?m)`.
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import textwrap` resolves to a real `PyModule`.
//
// Reference: <https://docs.python.org/3/library/textwrap.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `textwrap` member.  Exec'd once
/// at first import (see `inject_python_members`).
const TEXTWRAP_PY_SOURCE: &str = include_str!("textwrap_py.py");

/// Public names from `TEXTWRAP_PY_SOURCE` exported onto the `textwrap` module.
/// Matches CPython 3.12's `textwrap.__all__` exactly.
const TEXTWRAP_PY_EXPORTS: [&str; 6] =
    ["TextWrapper", "wrap", "fill", "dedent", "indent", "shorten"];

/// Execute `TEXTWRAP_PY_SOURCE` once and copy its public names onto the
/// `textwrap` module's attribute map.  Called from the `textwrap` post-load
/// hook in `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(TEXTWRAP_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("textwrap: exec namespace not a dict".into()))?;
    for name in TEXTWRAP_PY_EXPORTS {
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
