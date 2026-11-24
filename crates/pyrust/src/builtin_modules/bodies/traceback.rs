// `traceback` module — body for the `traceback @inject` entry in
// `pyrust_builtin_modules!` (issue #3152).
//
// CPython 3.12's `traceback` is a pure-Python module built on runtime
// `traceback`, `frame`, and `code` objects plus `sys._getframe` and
// `sys.exception` / `sys.exc_info`.  Shipping it as `traceback_py.py`
// keeps `FrameSummary`, `StackSummary`, `TracebackException` (including
// chained exceptions, `__notes__`, `SyntaxError`, and `BaseExceptionGroup`),
// and the top-level extract/format/print helpers aligned with CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/traceback.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `traceback` module.
const TRACEBACK_PY_SOURCE: &str = include_str!("traceback_py.py");

/// Public names exported from `TRACEBACK_PY_SOURCE` onto the `traceback` module.
const TRACEBACK_PY_EXPORTS: [&str; 22] = [
    "__all__",
    "extract_stack",
    "extract_tb",
    "format_exception",
    "format_exception_only",
    "format_list",
    "format_stack",
    "format_tb",
    "print_exc",
    "format_exc",
    "print_exception",
    "print_last",
    "print_list",
    "print_stack",
    "print_tb",
    "clear_frames",
    "FrameSummary",
    "StackSummary",
    "TracebackException",
    "walk_stack",
    "walk_tb",
    "_format_final_exc_line",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(TRACEBACK_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("traceback: exec namespace not a dict".into()))?;
    for name in TRACEBACK_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
