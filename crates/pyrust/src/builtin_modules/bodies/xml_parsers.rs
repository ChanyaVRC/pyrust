// `xml.parsers` package module — exposes `expat` (issue #3260).
//
// Reference: <https://docs.python.org/3/library/pyexpat.html>

use pyrust_derive::pyrust_module;

use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.parsers"),
        "expat" => super::xml_parsers_expat::module(),
    }
}
