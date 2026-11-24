// `importlib.util` module — body for the `"importlib.util" as importlib_util
// @inject` entry in `pyrust_builtin_modules!` (issue #3189).
//
// Exposes Python 3.12's `resolve_name`, `find_spec`, `spec_from_file_location`,
// `spec_from_loader`, `module_from_spec`, `MAGIC_NUMBER`, `cache_from_source`,
// `source_from_cache`, and `decode_source`, backed by private native predicates
// for PyRust's built-in module table.
//
// Reference: <https://docs.python.org/3/library/importlib.html#module-importlib.util>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for `importlib.util`.
const IMPORTLIB_UTIL_PY_SOURCE: &str = include_str!("importlib_util_py.py");

/// Public and internal names exported onto `importlib.util`.
const IMPORTLIB_UTIL_PY_EXPORTS: &[&str] = &[
    "MAGIC_NUMBER",
    "cache_from_source",
    "source_from_cache",
    "decode_source",
    "resolve_name",
    "spec_from_loader",
    "spec_from_file_location",
    "module_from_spec",
    "find_spec",
    "_find_spec",
    "_init_module_attrs",
];

pyrust_module! {
    /// Return True when `name` is a registered built-in module in PyRust.
    #[arity_style(takes_exactly_one)]
    fn _is_builtin_module(#[positional_only] name: PyValue) -> Result<Value> {
        let is_builtin = name
            .0
            .as_str()
            .is_some_and(crate::builtin_modules::is_builtin_module);
        Ok(Value::bool_(is_builtin))
    }

    /// Return True when `name` is a built-in package in PyRust.
    #[arity_style(takes_exactly_one)]
    fn _is_builtin_package(#[positional_only] name: PyValue) -> Result<Value> {
        let is_pkg = name
            .0
            .as_str()
            .is_some_and(crate::builtin_modules::is_builtin_package);
        Ok(Value::bool_(is_pkg))
    }
}

/// Execute `IMPORTLIB_UTIL_PY_SOURCE` once and copy its exports onto
/// `importlib.util`, removing the private native bridge helpers from the
/// public module surface.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    for name in ["_is_builtin_module", "_is_builtin_package"] {
        let value = module
            .borrow()
            .attrs
            .get(name)
            .cloned()
            .ok_or_else(|| PyError::runtime(format!("importlib.util: missing seed {name}")))?;
        ns.dict_insert(PyKey::str_from(name), value)?;
    }
    interp.exec_source(IMPORTLIB_UTIL_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("importlib.util: exec namespace not a dict".into()))?;
    for name in IMPORTLIB_UTIL_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    {
        let mut module = module.borrow_mut();
        module.attrs.shift_remove("_is_builtin_module");
        module.attrs.shift_remove("_is_builtin_package");
    }
    Ok(Some(ns.clone()))
}
