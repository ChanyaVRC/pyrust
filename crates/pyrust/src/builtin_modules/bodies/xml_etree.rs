// `xml.etree` package module — exposes `ElementPath` and `ElementTree`
// (issue #3210).
//
// Declaring `ElementPath` and `ElementTree` as `PyModule` constants lets
// `Interpreter::load_module` stitch them to the cached
// `sys.modules["xml.etree.ElementPath"]` and
// `sys.modules["xml.etree.ElementTree"]` objects so all import forms share
// exact module identity.
//
// Reference: <https://docs.python.org/3/library/xml.etree.elementtree.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.etree"),
        "ElementPath" => super::xml_etree_elementpath::module(),
        "ElementTree" => super::xml_etree_elementtree::module(),
    }
}
