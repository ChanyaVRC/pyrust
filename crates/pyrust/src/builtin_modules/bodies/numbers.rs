// `numbers` module — body for the `numbers @inject` entry in
// `pyrust_builtin_modules!` (issue #3147).
//
// CPython 3.12's `numbers` is a pure-Python module defining the PEP 3141
// numeric tower of abstract base classes (`Number`, `Complex`, `Real`,
// `Rational`, `Integral`) using `abc.ABCMeta` and `abc.abstractmethod`, with
// virtual subclass registrations for `complex`, `float`, and `int` (which also
// covers `bool`).
//
// Reference: <https://docs.python.org/3/library/numbers.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `numbers` module.
const NUMBERS_PY_SOURCE: &str = include_str!("numbers_py.py");

/// Public ABCs exported onto the `numbers` module.
const NUMBERS_PY_EXPORTS: [&str; 5] = ["Number", "Complex", "Real", "Rational", "Integral"];

/// Execute `NUMBERS_PY_SOURCE` once and copy its exported ABCs onto the
/// `numbers` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    // Seed `ABCMeta` and `abstractmethod` from `abc` so top-level class
    // definitions in the exec namespace can reference them directly.
    let abc_mod = interp.load_module("abc")?;
    let ValueKind::PyModule(abc_rc) = abc_mod.kind() else {
        return Err(PyError::runtime("numbers: abc is not a module".into()));
    };
    for dep_name in ["ABCMeta", "abstractmethod"] {
        let val = abc_rc
            .borrow()
            .get_attr_value(dep_name)
            .ok_or_else(|| PyError::runtime(format!("numbers: abc.{dep_name} missing")))?;
        ns.dict_insert(PyKey::str_from(dep_name), val)?;
    }

    interp.exec_source(NUMBERS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("numbers: exec namespace not a dict".into()))?;
    for name in NUMBERS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("numbers"));
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
