// `sysconfig` module — body for the `sysconfig @inject` entry in
// `pyrust_builtin_modules!` (issue #3229).
//
// Python 3.12's `sysconfig` exposes platform identification, install schemes,
// path template expansion, and configuration variables (`get_platform`,
// `get_python_version`, `get_scheme_names`, `get_default_scheme`,
// `get_preferred_scheme`, `get_path_names`, `get_paths`, `get_path`,
// `get_config_vars`, `get_config_var`, `is_python_build`,
// `get_config_h_filename`, `get_makefile_filename`, `parse_config_h`,
// `expand_makefile_vars`).
//
// Reference: <https://docs.python.org/3.12/library/sysconfig.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `sysconfig` module.
const SYSCONFIG_PY_SOURCE: &str = include_str!("sysconfig_py.py");

/// Private native helpers moved into the exec namespace and removed from
/// `dir(sysconfig)` after injection.
const SYSCONFIG_PRIVATE_HELPERS: &[&str] = &["_host_os_arch"];

/// Public names exported from `SYSCONFIG_PY_SOURCE` onto the `sysconfig` module.
const SYSCONFIG_PY_EXPORTS: &[&str] = &[
    "__all__",
    "get_config_h_filename",
    "get_config_var",
    "get_config_vars",
    "get_makefile_filename",
    "get_path",
    "get_path_names",
    "get_paths",
    "get_platform",
    "get_python_version",
    "get_scheme_names",
    "get_default_scheme",
    "get_preferred_scheme",
    "is_python_build",
    "parse_config_h",
    "expand_makefile_vars",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for &helper in SYSCONFIG_PRIVATE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(
        SYSCONFIG_PY_SOURCE,
        "<frozen sysconfig>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("sysconfig: exec namespace not a dict".into()))?;
    for &name in SYSCONFIG_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {
    /// Return `(std::env::consts::OS, std::env::consts::ARCH)` for platform
    /// tag construction in `sysconfig.get_platform()`.
    fn _host_os_arch(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        Ok(Value::tuple(vec![
            Value::string(std::env::consts::OS),
            Value::string(std::env::consts::ARCH),
        ]))
    }
}
