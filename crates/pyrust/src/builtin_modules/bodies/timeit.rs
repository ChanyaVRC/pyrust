// `timeit` module — body for the `timeit @inject` entry in
// `pyrust_builtin_modules!` (issue #3227).
//
// CPython 3.12's `timeit` is a pure-Python benchmarking module built on
// `time.perf_counter`, `time.process_time`, `itertools.repeat`, `compile`,
// `exec`, and `traceback`.  Shipping it as `timeit_py.py` keeps `Timer`,
// `timeit()`, `repeat()`, `autorange()`, `print_exc()`, and `main()` (for
// `pyrust -m timeit`) aligned with CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/timeit.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `timeit` module.
const TIMEIT_PY_SOURCE: &str = include_str!("timeit_py.py");

/// Public names exported from `TIMEIT_PY_SOURCE` onto the `timeit` module.
const TIMEIT_PY_EXPORTS: &[&str] = &[
    "__doc__",
    "__all__",
    "dummy_src_name",
    "default_number",
    "default_repeat",
    "default_timer",
    "template",
    "reindent",
    "Timer",
    "timeit",
    "repeat",
    "main",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(TIMEIT_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("timeit: exec namespace not a dict".into()))?;
    for name in TIMEIT_PY_EXPORTS {
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
