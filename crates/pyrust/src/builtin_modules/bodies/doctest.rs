// `doctest` module — body for the `doctest @inject` entry in
// `pyrust_builtin_modules!` (issue #3222).
//
// Python 3.12's `doctest` is a pure-Python interactive-example testing library
// (`Example`, `DocTest`, `DocTestParser`, `DocTestFinder`, `DocTestRunner`,
// `OutputChecker`, `DebugRunner`, `TestResults`, `DocTestFailure`,
// `UnexpectedException`, `testmod`, `testfile`, `run_docstring_examples`,
// `script_from_examples`, `testsource`, `DocTestSuite`, `DocFileSuite`,
// `set_unittest_reportflags`, `register_optionflag`, and option flags).
//
// Reference: <https://docs.python.org/3/library/doctest.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `doctest` module.
const DOCTEST_PY_SOURCE: &str = include_str!("doctest_py.py");

/// Public and compatibility names exported from `DOCTEST_PY_SOURCE`.
const DOCTEST_PY_EXPORTS: &[&str] = &[
    "__all__",
    // Option flag registry and constants
    "OPTIONFLAGS_BY_NAME",
    "register_optionflag",
    "DONT_ACCEPT_TRUE_FOR_1",
    "DONT_ACCEPT_BLANKLINE",
    "NORMALIZE_WHITESPACE",
    "ELLIPSIS",
    "SKIP",
    "IGNORE_EXCEPTION_DETAIL",
    "COMPARISON_FLAGS",
    "REPORT_UDIFF",
    "REPORT_CDIFF",
    "REPORT_NDIFF",
    "REPORT_ONLY_FIRST_FAILURE",
    "REPORTING_FLAGS",
    "FAIL_FAST",
    "BLANKLINE_MARKER",
    "ELLIPSIS_MARKER",
    // Core classes and exceptions
    "TestResults",
    "Example",
    "DocTest",
    "DocTestParser",
    "DocTestFinder",
    "DocTestRunner",
    "OutputChecker",
    "DocTestFailure",
    "UnexpectedException",
    "DebugRunner",
    // Top-level test and script functions
    "testmod",
    "testfile",
    "run_docstring_examples",
    "script_from_examples",
    "testsource",
    "debug_src",
    "debug_script",
    "debug",
    // Unittest integration
    "DocTestSuite",
    "DocFileSuite",
    "DocTestCase",
    "DocFileCase",
    "SkipDocTestCase",
    "DocFileTest",
    "set_unittest_reportflags",
    // Compatibility helpers
    "master",
    "_SpoofOut",
    "_TestClass",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(
        DOCTEST_PY_SOURCE,
        "<frozen doctest>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("doctest: exec namespace not a dict".into()))?;
    for &name in DOCTEST_PY_EXPORTS {
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
