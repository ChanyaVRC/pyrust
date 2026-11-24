// `shlex` module — body for the `shlex @inject` entry in
// `pyrust_builtin_modules!` (issue #3147).
//
// CPython 3.12's `shlex` is a pure-Python module providing a lexical analyzer
// for simple shell-like syntaxes (`shlex` class) along with `split`, `join`,
// and `quote`.
//
// Reference: <https://docs.python.org/3/library/shlex.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `shlex` module.
const SHLEX_PY_SOURCE: &str = include_str!("shlex_py.py");

/// Public names exported onto the `shlex` module.
/// Mirrors CPython 3.12's `shlex.__all__`.
const SHLEX_PY_EXPORTS: [&str; 4] = ["shlex", "split", "quote", "join"];

/// Execute `SHLEX_PY_SOURCE` once and copy its exported attributes onto the
/// `shlex` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    // Pre-seed runtime dependencies used by `shlex_py.py`.
    let sys_mod = interp.load_module("sys")?;
    ns.dict_insert(PyKey::str_from("sys"), sys_mod)?;
    let os_path_mod = interp.load_module("os.path")?;
    ns.dict_insert(PyKey::str_from("_os_path"), os_path_mod)?;
    seed_attr(interp, &ns, "io", "StringIO", "StringIO")?;
    seed_attr(interp, &ns, "collections", "deque", "deque")?;

    interp.exec_source(SHLEX_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("shlex: exec namespace not a dict".into()))?;
    for name in SHLEX_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("shlex"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

fn seed_attr(
    interp: &mut Interpreter,
    ns: &Value,
    module_name: &str,
    attr_name: &str,
    bind_name: &str,
) -> Result<()> {
    let module = interp.load_module(module_name)?;
    let ValueKind::PyModule(m) = module.kind() else {
        return Err(PyError::runtime(format!(
            "shlex: {module_name} is not a module"
        )));
    };
    let val = m
        .borrow()
        .get_attr_value(attr_name)
        .ok_or_else(|| PyError::runtime(format!("shlex: {module_name}.{attr_name} missing")))?;
    ns.dict_insert(PyKey::str_from(bind_name), val)?;
    Ok(())
}

pyrust_module! {}
