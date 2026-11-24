// `configparser` module — body for the `configparser @inject` entry in
// `pyrust_builtin_modules!` (issue #3185).
//
// CPython 3.12's `configparser` is a pure-Python INI configuration file parser
// built on `collections.abc`, `collections.ChainMap`, `functools`, `io`,
// `itertools`, `os`, `re`, `sys`, and `warnings`.
//
// Reference: <https://docs.python.org/3/library/configparser.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `configparser` module.
const CONFIGPARSER_PY_SOURCE: &str = include_str!("configparser_py.py");

/// Public names exported from `CONFIGPARSER_PY_SOURCE` onto the
/// `configparser` module.
const CONFIGPARSER_PY_EXPORTS: &[&str] = &[
    "__all__",
    "DEFAULTSECT",
    "MAX_INTERPOLATION_DEPTH",
    "Error",
    "NoSectionError",
    "DuplicateSectionError",
    "DuplicateOptionError",
    "NoOptionError",
    "InterpolationError",
    "InterpolationMissingOptionError",
    "InterpolationSyntaxError",
    "InterpolationDepthError",
    "ParsingError",
    "MissingSectionHeaderError",
    "Interpolation",
    "BasicInterpolation",
    "ExtendedInterpolation",
    "LegacyInterpolation",
    "RawConfigParser",
    "ConfigParser",
    "SectionProxy",
    "ConverterMapping",
];

/// Execute `CONFIGPARSER_PY_SOURCE` once and copy its exported attributes onto
/// the `configparser` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(CONFIGPARSER_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("configparser: exec namespace not a dict".into()))?;
    for name in CONFIGPARSER_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("configparser"));
            }
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
