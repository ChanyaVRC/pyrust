// `xmlrpc` package module — 100% native-Rust Python 3.12 XML-RPC package
// (issue #3256).
//
// Submodules (`client`, `server`) are loaded lazily on explicit import,
// matching CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/xmlrpc.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "__package__" => Value::string("xmlrpc"),
    }
}
