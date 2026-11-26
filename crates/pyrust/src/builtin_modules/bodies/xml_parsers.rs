// `xml.parsers` package module — `expat` is loaded lazily on explicit import,
// matching CPython 3.12 (issue #3260).
//
// Reference: <https://docs.python.org/3/library/pyexpat.html>

use pyrust_derive::pyrust_module;

use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.parsers"),
    }
}
