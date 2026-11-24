// `argparse` module — body for the `argparse @inject` entry in
// `pyrust_builtin_modules!` (issue #3150).
//
// CPython 3.12's `argparse` is a pure-Python command-line argument parsing
// library built on `os`, `re`, `sys`, `textwrap`, `copy`, and `warnings`.
// We ship a direct port (`argparse_py.py`) exec'd once at first import and
// injected onto the module by `inject_python_members`.
//
// Reference: <https://docs.python.org/3/library/argparse.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `argparse` module.
const ARGPARSE_PY_SOURCE: &str = include_str!("argparse_py.py");

/// Public and internal names exported onto the `argparse` module.
const ARGPARSE_PY_EXPORTS: &[&str] = &[
    "__version__",
    "ArgumentParser",
    "ArgumentError",
    "ArgumentTypeError",
    "BooleanOptionalAction",
    "FileType",
    "HelpFormatter",
    "ArgumentDefaultsHelpFormatter",
    "RawDescriptionHelpFormatter",
    "RawTextHelpFormatter",
    "MetavarTypeHelpFormatter",
    "Namespace",
    "Action",
    "ONE_OR_MORE",
    "OPTIONAL",
    "PARSER",
    "REMAINDER",
    "SUPPRESS",
    "ZERO_OR_MORE",
    "_AttributeHolder",
    "_ActionsContainer",
    "_ArgumentGroup",
    "_MutuallyExclusiveGroup",
    "_StoreAction",
    "_StoreConstAction",
    "_StoreTrueAction",
    "_StoreFalseAction",
    "_AppendAction",
    "_AppendConstAction",
    "_CountAction",
    "_HelpAction",
    "_VersionAction",
    "_SubParsersAction",
    "_ExtendAction",
];

/// Execute `ARGPARSE_PY_SOURCE` once and copy its exported attributes onto the
/// `argparse` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(ARGPARSE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("argparse: exec namespace not a dict".into()))?;
    for name in ARGPARSE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("argparse"));
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
