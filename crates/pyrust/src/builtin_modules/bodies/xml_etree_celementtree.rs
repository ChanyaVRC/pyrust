// `xml.etree.cElementTree` module — 100% native-Rust Python 3.12 compatibility
// alias re-exporting `xml.etree.ElementTree`'s public API with exact object
// identity (issue #3260).
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/xml/etree/cElementTree.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::{PyModule, Value};

const PUBLIC_NAMES: &[&str] = &[
    "Comment",
    "dump",
    "Element",
    "ElementTree",
    "fromstring",
    "fromstringlist",
    "indent",
    "iselement",
    "iterparse",
    "parse",
    "ParseError",
    "PI",
    "ProcessingInstruction",
    "QName",
    "SubElement",
    "tostring",
    "tostringlist",
    "TreeBuilder",
    "VERSION",
    "XML",
    "XMLID",
    "XMLParser",
    "XMLPullParser",
    "register_namespace",
    "canonicalize",
    "C14NWriterTarget",
];

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let et_mod = interp.load_module("xml.etree.ElementTree")?;
    let mut m = module.borrow_mut();
    for &name in PUBLIC_NAMES {
        if let Ok(val) = interp.get_attr(&et_mod, name) {
            m.insert_attr(name.to_string(), val);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.etree"),
    }
}
