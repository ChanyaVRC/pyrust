// `xml` package module — subpackages (`dom`, `parsers`, `sax`, `etree`) are
// loaded lazily on explicit import, matching CPython 3.12 (issues #3210, #3260).
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
    }
}
