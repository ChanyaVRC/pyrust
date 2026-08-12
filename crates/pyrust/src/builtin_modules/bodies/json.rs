// `json` module — body for the `json` entry in `pyrust_builtin_modules!`
// (issue #2620).
//
// CPython's `json` package is most naturally expressed in Python itself: the
// encoder is string formatting and the decoder is a recursive-descent parser,
// neither of which needs interpreter internals.  Shipping it as a pure-Python
// module (`json_py.py`, targeting CPython 3.12 behaviour) keeps type mapping,
// escape handling, and error wording in lock-step with the reference.
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import json` resolves to a real `PyModule`.  The public names are copied
// onto the module by `inject_python_members`, wired from `env.rs::load_module`'s
// post-import hook (mirrors `operator` / `string`).
//
// Reference: <https://docs.python.org/3/library/json.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `json` member.  Exec'd once at
/// first import (see `inject_python_members`).
const JSON_PY_SOURCE: &str = include_str!("json_py.py");

/// Public names from `JSON_PY_SOURCE` exported onto the `json` module.
const JSON_PY_EXPORTS: [&str; 5] = ["JSONDecodeError", "dump", "dumps", "load", "loads"];

/// Execute `JSON_PY_SOURCE` once and copy its public names onto the `json`
/// module's attribute map.  Called from the `json` post-load hook in
/// `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(JSON_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("json: exec namespace not a dict".into()))?;
    for name in JSON_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            // `JSONDecodeError` renders its repr via `__module__`; the exec
            // namespace's `__name__` defaults to `__main__`, so override it to
            // `json` so `repr(json.JSONDecodeError(...))` and qualname match.
            if name == "JSONDecodeError"
                && let ValueKind::PyClass(cls_rc) = val.kind()
            {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("json"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
