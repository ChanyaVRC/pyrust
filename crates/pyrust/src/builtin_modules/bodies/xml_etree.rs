// `xml.etree` package module — exposes `ElementPath`, `ElementTree`,
// `ElementInclude`, and `cElementTree` (issues #3210, #3260).
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
