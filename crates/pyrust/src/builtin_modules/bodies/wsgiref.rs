// `wsgiref` package module — 100% native-Rust Python 3.12 WSGI reference
// implementation package (issue #3256).
//
// Submodules (`util`, `headers`, `handlers`, `simple_server`, `validate`,
// `types`) are loaded lazily on explicit import, matching CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/wsgiref.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "__package__" => Value::string("wsgiref"),
    }
}
