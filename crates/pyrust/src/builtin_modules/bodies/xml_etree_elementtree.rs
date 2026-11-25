// `xml.etree.ElementTree` module — body for the
// `"xml.etree.ElementTree" as xml_etree_elementtree @inject` entry in
// `pyrust_builtin_modules!` (issue #3210).
//
// Implements Python 3.12's `xml.etree.ElementTree` module: `Element`,
// `SubElement`, `Comment`, `ProcessingInstruction`/`PI`, `QName`,
// `ElementTree`, `TreeBuilder`, `XMLParser`, `XMLPullParser`,
// `C14NWriterTarget`, `ParseError`, XML 1.0 parser and serializer,
// `XML`/`XMLID`/`fromstring`/`fromstringlist`, `tostring`/`tostringlist`,
// `parse`/`iterparse`, `dump`, `indent`, `canonicalize`, `register_namespace`,
// and `iselement`.
//
// Reference: <https://docs.python.org/3/library/xml.etree.elementtree.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `xml.etree.ElementTree` module.
const XML_ETREE_ELEMENTTREE_PY_SOURCE: &str = include_str!("xml_etree_elementtree_py.py");

/// Public and compatibility names exported from `XML_ETREE_ELEMENTTREE_PY_SOURCE`.
const XML_ETREE_ELEMENTTREE_PY_EXPORTS: &[&str] = &[
    "__package__",
    "__all__",
    "VERSION",
    "HTML_EMPTY",
    "ElementPath",
    "ParseError",
    "iselement",
    "Element",
    "SubElement",
    "Comment",
    "ProcessingInstruction",
    "PI",
    "QName",
    "ElementTree",
    "register_namespace",
    "_namespace_map",
    "tostring",
    "tostringlist",
    "dump",
    "indent",
    "parse",
    "iterparse",
    "XML",
    "XMLID",
    "fromstring",
    "fromstringlist",
    "TreeBuilder",
    "XMLParser",
    "XMLPullParser",
    "C14NWriterTarget",
    "canonicalize",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(XML_ETREE_ELEMENTTREE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns.as_dict().ok_or_else(|| {
        PyError::runtime("xml.etree.ElementTree: exec namespace not a dict".into())
    })?;
    for name in XML_ETREE_ELEMENTTREE_PY_EXPORTS {
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
