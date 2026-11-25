// `colorsys` module — body for the `colorsys @inject` entry in
// `pyrust_builtin_modules!` (issue #3230).
//
// CPython 3.12's `colorsys` is a pure-Python module providing conversions
// between RGB and YIQ, HLS, and HSV color systems.
//
// Reference: <https://docs.python.org/3/library/colorsys.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `colorsys` module.
const COLORSYS_PY_SOURCE: &str = include_str!("colorsys_py.py");

/// Public names exported from `COLORSYS_PY_SOURCE` onto the `colorsys` module.
const COLORSYS_PY_EXPORTS: &[&str] = &[
    "__doc__",
    "__all__",
    "ONE_THIRD",
    "ONE_SIXTH",
    "TWO_THIRD",
    "rgb_to_yiq",
    "yiq_to_rgb",
    "rgb_to_hls",
    "hls_to_rgb",
    "_v",
    "rgb_to_hsv",
    "hsv_to_rgb",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(COLORSYS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("colorsys: exec namespace not a dict".into()))?;
    for name in COLORSYS_PY_EXPORTS {
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
