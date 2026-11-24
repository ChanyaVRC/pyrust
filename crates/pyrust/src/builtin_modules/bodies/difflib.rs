// `difflib` module — body for the `difflib @inject` entry in
// `pyrust_builtin_modules!` (issue #3147).
//
// CPython 3.12's `difflib` is a pure-Python module providing sequence
// comparison and delta helpers (`SequenceMatcher`, `Differ`, `ndiff`,
// `restore`, `unified_diff`, `context_diff`, `diff_bytes`,
// `get_close_matches`, `IS_LINE_JUNK`, `IS_CHARACTER_JUNK`, `HtmlDiff`,
// `Match`).
//
// Reference: <https://docs.python.org/3/library/difflib.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `difflib` module.
const DIFFLIB_PY_SOURCE: &str = include_str!("difflib_py.py");

/// Public names exported onto the `difflib` module.
/// Mirrors CPython 3.12's `difflib.__all__`.
const DIFFLIB_PY_EXPORTS: [&str; 12] = [
    "get_close_matches",
    "ndiff",
    "restore",
    "SequenceMatcher",
    "Differ",
    "IS_CHARACTER_JUNK",
    "IS_LINE_JUNK",
    "context_diff",
    "unified_diff",
    "diff_bytes",
    "HtmlDiff",
    "Match",
];

/// Execute `DIFFLIB_PY_SOURCE` once and copy its exported attributes onto the
/// `difflib` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    // Pre-seed dependencies referenced at module top-level or inside methods.
    seed_attr(interp, &ns, "heapq", "nlargest", "_nlargest")?;
    seed_attr(interp, &ns, "collections", "namedtuple", "_namedtuple")?;
    seed_attr(interp, &ns, "types", "GenericAlias", "GenericAlias")?;
    let re_mod = interp.load_module("re")?;
    ns.dict_insert(PyKey::str_from("_re"), re_mod)?;

    interp.exec_source(DIFFLIB_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("difflib: exec namespace not a dict".into()))?;
    for name in DIFFLIB_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("difflib"));
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
            "difflib: {module_name} is not a module"
        )));
    };
    let val = m
        .borrow()
        .get_attr_value(attr_name)
        .ok_or_else(|| PyError::runtime(format!("difflib: {module_name}.{attr_name} missing")))?;
    ns.dict_insert(PyKey::str_from(bind_name), val)?;
    Ok(())
}

pyrust_module! {}
