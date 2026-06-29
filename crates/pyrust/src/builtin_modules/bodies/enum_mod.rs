// `enum` module — `Enum`, `IntEnum`, `EnumMeta`/`EnumType`, and `auto`.
//
// Included into `pub mod enum_mod { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs`
// (Python-level name `enum`; the Rust ident can't be `enum`, a keyword).
//
// The whole module is expressed in ordinary Python (`enum_py.py`): the
// member machinery is metaclass-driven (`EnumMeta.__new__` collects the
// members, `__iter__` / `__getitem__` / `__call__` implement iteration and
// value/name lookup), which the interpreter's metaclass support now drives
// for an inherited metaclass (`class Color(Enum)` runs `EnumMeta.__new__`).
// Doing it in Python mirrors CPython's own `Lib/enum.py` shape and avoids
// re-deriving the member-singleton protocol in Rust.
//
// The native `pyrust_module!` block is intentionally empty — it exists only
// so the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is
// generated and `import enum` resolves to a real `PyModule`; the public
// names are copied onto the module by `inject_python_members`, wired from
// `env.rs::load_module`'s post-import hook (mirrors `operator` / `string`).
//
// Reference: <https://docs.python.org/3/library/enum.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::PyKey;
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `enum` member.  Exec'd once at
/// first import (see `inject_python_members`).
const ENUM_PY_SOURCE: &str = include_str!("enum_py.py");

/// Public names from `ENUM_PY_SOURCE` exported onto the `enum` module.
const ENUM_PY_EXPORTS: [&str; 8] = [
    "EnumMeta", "EnumType", "Enum", "IntEnum", "auto", "Flag", "IntFlag", "StrEnum",
];

/// Execute `ENUM_PY_SOURCE` once and copy its public names onto the `enum`
/// module's attribute map.  Wired from `env.rs::load_module`'s post-import
/// hook (mirrors the `operator` / `string` injection).
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(ENUM_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("enum: exec namespace not a dict".into()))?;
    for name in ENUM_PY_EXPORTS {
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
