// `abc` module — body for the `abc` entry in `pyrust_builtin_modules!`
// (issue #2612).
//
// CPython's `abc` exports the `ABCMeta` metaclass, the `ABC` helper base
// class, the `@abstractmethod` decorator (plus the deprecated
// `abstractclassmethod` / `abstractstaticmethod` / `abstractproperty`), and
// the `get_cache_token` / `update_abstractmethods` helpers.  The whole surface
// is most naturally expressed in Python itself — `ABCMeta` is a metaclass that
// walks the class namespace — so we ship it as a pure-Python module
// (`abc_py.py`) exec'd once into a throwaway namespace at first import; the
// public names are copied onto the module by `inject_python_members`, wired
// from `env.rs::load_module`'s post-import hook (mirrors `operator` /
// `string`, issues #2514 / #2515).
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import abc` resolves to a real `PyModule`.
//
// Reference: <https://docs.python.org/3/library/abc.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `abc` member.  Exec'd once at
/// first import (see `inject_python_members`).
const ABC_PY_SOURCE: &str = include_str!("abc_py.py");

/// Public names from `ABC_PY_SOURCE` exported onto the `abc` module.  Matches
/// CPython 3.12's `abc.__all__` plus the deprecated abstract-decorator
/// classes (which `abc.__all__` also lists).
const ABC_PY_EXPORTS: [&str; 8] = [
    "ABC",
    "ABCMeta",
    "abstractclassmethod",
    "abstractmethod",
    "abstractproperty",
    "abstractstaticmethod",
    "get_cache_token",
    "update_abstractmethods",
];

/// Execute `ABC_PY_SOURCE` once and copy its public names onto the `abc`
/// module's attribute map.  Called from the `abc` post-load hook in
/// `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(ABC_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("abc: exec namespace not a dict".into()))?;
    for name in ABC_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            // The exec namespace's `__name__` defaults to `__main__`; override
            // `__module__` to `abc` so `ABCMeta.__module__ == "abc"` and class
            // reprs read `<class 'abc.ABC'>`, matching CPython.
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("abc"));
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
