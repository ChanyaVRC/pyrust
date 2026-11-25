// `xml.etree.ElementPath` module — body for the
// `"xml.etree.ElementPath" as xml_etree_elementpath @inject` entry in
// `pyrust_builtin_modules!` (issue #3210).
//
// Implements CPython 3.12's XPath subset evaluator for `xml.etree.ElementTree`
// (`find`, `findall`, `findtext`, `iterfind`, `xpath_tokenizer`,
// `get_parent_map`, `prepare_*`, and `ops`).
//
// Reference: <https://docs.python.org/3/library/xml.etree.elementtree.html#xpath-support>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `xml.etree.ElementPath` module.
const XML_ETREE_ELEMENTPATH_PY_SOURCE: &str = include_str!("xml_etree_elementpath_py.py");

/// Public and compatibility names exported from `XML_ETREE_ELEMENTPATH_PY_SOURCE`.
const XML_ETREE_ELEMENTPATH_PY_EXPORTS: &[&str] = &[
    "__package__",
    "xpath_tokenizer",
    "get_parent_map",
    "prepare_child",
    "prepare_star",
    "prepare_self",
    "prepare_descendant",
    "prepare_parent",
    "prepare_predicate",
    "ops",
    "_cache",
    "iterfind",
    "find",
    "findall",
    "findtext",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(XML_ETREE_ELEMENTPATH_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns.as_dict().ok_or_else(|| {
        PyError::runtime("xml.etree.ElementPath: exec namespace not a dict".into())
    })?;
    for name in XML_ETREE_ELEMENTPATH_PY_EXPORTS {
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
