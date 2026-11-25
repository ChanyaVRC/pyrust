// `unittest.mock` module — body for the `"unittest.mock" as unittest_mock @inject`
// entry in `pyrust_builtin_modules!` (issue #3222).
//
// Python 3.12's `unittest.mock` is a pure-Python mocking and patching library
// (`NonCallableMock`, `Mock`, `NonCallableMagicMock`, `MagicMock`, `AsyncMock`,
// `PropertyMock`, `patch`, `patch.object`, `patch.dict`, `patch.multiple`,
// `patch.stopall`, `call`, `ANY`, `DEFAULT`, `sentinel`, `InvalidSpecError`,
// `FILTER_DIR`, `create_autospec`, `mock_open`, and `seal`).
//
// Reference: <https://docs.python.org/3/library/unittest.mock.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `unittest.mock` module.
const UNITTEST_MOCK_PY_SOURCE: &str = include_str!("unittest_mock_py.py");

/// Public and compatibility names exported from `UNITTEST_MOCK_PY_SOURCE`.
const UNITTEST_MOCK_PY_EXPORTS: &[&str] = &[
    "__all__",
    // Public API (matches CPython 3.12 `unittest.mock.__all__`)
    "Mock",
    "MagicMock",
    "patch",
    "sentinel",
    "DEFAULT",
    "ANY",
    "call",
    "create_autospec",
    "AsyncMock",
    "FILTER_DIR",
    "NonCallableMock",
    "NonCallableMagicMock",
    "mock_open",
    "PropertyMock",
    "seal",
    // Public / compatibility classes, helpers, and constants
    "InvalidSpecError",
    "Base",
    "CallableMixin",
    "MagicMixin",
    "AsyncMockMixin",
    "AsyncMagicMixin",
    "MagicProxy",
    "file_spec",
    "open_spec",
    "iscoroutinefunction",
    "FunctionTypes",
    "numerics",
    "right",
    "inplace",
    "magic_methods",
    "_Call",
    "_CallList",
    "_patch",
    "_patch_dict",
    "_Sentinel",
    "_SentinelObject",
    "_ANY",
    "_AsyncIterator",
    "_SpecState",
    "_deleted",
    "_missing",
    "_all_magics",
    "_magics",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(
        UNITTEST_MOCK_PY_SOURCE,
        "<frozen unittest.mock>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("unittest.mock: exec namespace not a dict".into()))?;
    for &name in UNITTEST_MOCK_PY_EXPORTS {
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
