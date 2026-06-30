// `re` module — body for the `re` entry in `pyrust_builtin_modules!`
// (issue #2625).
//
// CPython's `re` is a regular-expression engine; a faithful minimal subset is
// most naturally expressed in Python itself (a recursive-descent compiler plus a
// backtracking matcher).  Shipping it as a pure-Python module (`re_py.py`,
// targeting CPython 3.12 behaviour) keeps flag handling, group semantics, and
// error wording in lock-step with the reference.
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated and
// `import re` resolves to a real `PyModule`.  The public names are copied onto
// the module by `inject_python_members`, wired from `env.rs::load_module`'s
// post-import hook (mirrors `json` / `operator` / `string`).
//
// `re` is a Rust keyword, so the Rust module ident is `re_mod` while the
// Python-level name stays `re` (declared as `"re" as re_mod`).
//
// Reference: <https://docs.python.org/3/library/re.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `re` member.  Exec'd once at first
/// import (see `inject_python_members`).
const RE_PY_SOURCE: &str = include_str!("re_py.py");

/// Public names from `RE_PY_SOURCE` exported onto the `re` module.
const RE_PY_EXPORTS: [&str; 27] = [
    "error",
    "Pattern",
    "Match",
    "compile",
    "match",
    "fullmatch",
    "search",
    "findall",
    "finditer",
    "sub",
    "subn",
    "split",
    "escape",
    "purge",
    "A",
    "ASCII",
    "I",
    "IGNORECASE",
    "M",
    "MULTILINE",
    "S",
    "DOTALL",
    "X",
    "VERBOSE",
    "_Parser",
    "_Matcher",
    "_expand",
];

/// Execute `RE_PY_SOURCE` once and copy its public names onto the `re` module's
/// attribute map.  Called from the `re` post-load hook in
/// `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(RE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("re: exec namespace not a dict".into()))?;
    for name in RE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            // The exec namespace's `__name__` defaults to `__main__`, so the
            // classes defined there pick up that module name.  Override
            // `__module__` to `re` so reprs / qualnames match CPython
            // (`re.error`, `re.Pattern`, `re.Match`).
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("re"));
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
