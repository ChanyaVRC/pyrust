// `unittest` module — body for the `unittest @inject` entry in
// `pyrust_builtin_modules!` (issue #3153).
//
// Python 3.12's `unittest` is a pure-Python package built on `difflib`,
// `traceback`, `warnings`, `pprint`, `re`, `io`, `time`, `functools`,
// `collections`, `contextlib`, `os`, `sys`, `types`, and (lazily) `asyncio` /
// `inspect`.  Shipping it via `unittest_py.py` keeps the full test framework
// (`TestCase`, `FunctionTestCase`, `IsolatedAsyncioTestCase`, `TestSuite`,
// `BaseTestSuite`, `TestLoader`, `defaultTestLoader`, `TestResult`,
// `TextTestResult`, `TextTestRunner`, `TestProgram`, `main`, skipping and
// expected-failure decorators, cleanups, subtests, and `unittest.*`
// submodules) aligned with CPython 3.12 with zero hot-VM impact.
//
// Reference: <https://docs.python.org/3/library/unittest.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `unittest` package and its submodules.
const UNITTEST_PY_SOURCE: &str = include_str!("unittest_py.py");

/// Public names, package metadata, and submodule bindings copied from
/// `UNITTEST_PY_SOURCE` onto the `unittest` module.
const UNITTEST_PY_EXPORTS: &[&str] = &[
    "__all__",
    "__package__",
    "__path__",
    "__unittest",
    // Public API (matches CPython 3.12 `unittest.__all__`)
    "TestResult",
    "TestCase",
    "IsolatedAsyncioTestCase",
    "TestSuite",
    "BaseTestSuite",
    "TextTestRunner",
    "TestLoader",
    "FunctionTestCase",
    "TestProgram",
    "main",
    "defaultTestLoader",
    "SkipTest",
    "skip",
    "skipIf",
    "skipUnless",
    "expectedFailure",
    "TextTestResult",
    "installHandler",
    "registerResult",
    "removeResult",
    "removeHandler",
    "addModuleCleanup",
    "doModuleCleanups",
    "enterModuleContext",
    "getTestCaseNames",
    "makeSuite",
    "findTestCases",
    // Submodules
    "util",
    "signals",
    "result",
    "case",
    "suite",
    "loader",
    "runner",
    "async_case",
    "_log",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(
        UNITTEST_PY_SOURCE,
        "<frozen unittest>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("unittest: exec namespace not a dict".into()))?;
    for name in UNITTEST_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
