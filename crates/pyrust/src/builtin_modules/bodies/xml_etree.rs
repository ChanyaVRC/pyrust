// `xml.etree` package module — submodules (`ElementPath`, `ElementTree`,
// `ElementInclude`, `cElementTree`) are loaded lazily on explicit import,
// matching CPython 3.12 (issues #3210, #3260).
//
// Reference: <https://docs.python.org/3/library/xml.etree.elementtree.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.etree"),
    }
}
