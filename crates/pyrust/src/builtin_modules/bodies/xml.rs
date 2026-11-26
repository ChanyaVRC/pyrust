// `xml` package module — exposes `dom`, `parsers`, `sax`, and `etree`
// (issues #3210, #3260).
//
// Reference: <https://docs.python.org/3/library/xml.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "__package__" => Value::string("xml"),
        "__all__" => Value::list(vec![
            Value::string("dom"),
            Value::string("parsers"),
            Value::string("sax"),
            Value::string("etree"),
        ]),
        "etree" => super::xml_etree::module(),
        "parsers" => super::xml_parsers::module(),
        "sax" => super::xml_sax::module(),
        "dom" => super::xml_dom::module(),
    }
}
