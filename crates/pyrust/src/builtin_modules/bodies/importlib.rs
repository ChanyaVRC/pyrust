// `importlib` package module — body for the `importlib @inject` entry in
// `pyrust_builtin_modules!` (issue #3189).
//
// Exposes `import_module`, `reload`, `invalidate_caches`, and `__import__` from
// `importlib_py.py` alongside the `machinery` and `util` submodules. Declaring
// `machinery` and `util` as `PyModule` constants lets `Interpreter::load_module`
// stitch them to the cached `sys.modules["importlib.machinery"]` and
// `sys.modules["importlib.util"]` objects so all import forms share exact
// module identity.
//
// Reference: <https://docs.python.org/3/library/importlib.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `importlib` module.
const IMPORTLIB_PY_SOURCE: &str = include_str!("importlib_py.py");

/// Public names exported from `IMPORTLIB_PY_SOURCE` onto `importlib`.
const IMPORTLIB_PY_EXPORTS: &[&str] = &[
    "__all__",
    "__import__",
    "import_module",
    "invalidate_caches",
    "reload",
];

pyrust_module! {
    constants {
        "machinery" => super::importlib_machinery::module(),
        "util" => super::importlib_util::module(),
    }

    /// Private bridge into `Interpreter::load_module` for `importlib.import_module`.
    #[arity_style(takes_exactly_one)]
    fn _load_module(#[positional_only] name: PyValue) -> Result<Value> {
        let Some(name_str) = name.0.as_str() else {
            return Err(PyError::named(
                "TypeError",
                "module name must be a string".to_string(),
            ));
        };
        _interp.load_module(name_str)
    }

    /// Private bridge into `is_builtin_module` for `importlib.reload`.
    #[arity_style(takes_exactly_one)]
    fn _is_builtin_module(#[positional_only] name: PyValue) -> Result<Value> {
        let is_builtin = name
            .0
            .as_str()
            .is_some_and(crate::builtin_modules::is_builtin_module);
        Ok(Value::bool_(is_builtin))
    }
}

/// Execute `IMPORTLIB_PY_SOURCE` once and copy its public names onto
/// `importlib`, removing the private native bridge helpers from the public
/// module surface.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    for name in ["_load_module", "_is_builtin_module"] {
        let value = module
            .borrow()
            .attrs
            .get(name)
            .cloned()
            .ok_or_else(|| PyError::runtime(format!("importlib: missing seed {name}")))?;
        ns.dict_insert(PyKey::str_from(name), value)?;
    }
    interp.exec_source(IMPORTLIB_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("importlib: exec namespace not a dict".into()))?;
    for name in IMPORTLIB_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    {
        let mut module = module.borrow_mut();
        module.attrs.shift_remove("_load_module");
        module.attrs.shift_remove("_is_builtin_module");
    }
    Ok(Some(ns.clone()))
}
