// `html.parser` module — body for the `"html.parser" as html_parser @inject`
// entry in `pyrust_builtin_modules!` (issue #3149).
//
// CPython 3.12's `html.parser` is a pure-Python streaming HTML/XHTML parser
// built on `_markupbase.ParserBase` and `html.unescape`. Shipping it as
// `html_parser_py.py` preserves `HTMLParser` and all callback/position/rawtext
// semantics aligned with CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/html.parser.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `html.parser` module.
const HTML_PARSER_PY_SOURCE: &str = include_str!("html_parser_py.py");

/// Public and parser-regex names exported from `HTML_PARSER_PY_SOURCE`.
const HTML_PARSER_PY_EXPORTS: [&str; 17] = [
    "__all__",
    "HTMLParser",
    "interesting_normal",
    "incomplete",
    "entityref",
    "charref",
    "starttagopen",
    "endtagopen",
    "piclose",
    "commentclose",
    "commentabruptclose",
    "tagfind_tolerant",
    "attrfind_tolerant",
    "locatetagend",
    "locatestarttagend_tolerant",
    "endendtag",
    "endtagfind",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(HTML_PARSER_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("html.parser: exec namespace not a dict".into()))?;
    for name in HTML_PARSER_PY_EXPORTS {
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
