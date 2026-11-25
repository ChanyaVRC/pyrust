// `wsgiref` package module — 100% native-Rust Python 3.12 WSGI reference
// implementation package (issue #3256).
//
// Exposes submodules `util`, `headers`, `handlers`, `simple_server`,
// `validate`, and `types`.
//
// Reference: <https://docs.python.org/3/library/wsgiref.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "__package__" => Value::string("wsgiref"),
        "util" => super::wsgiref_util::module(),
        "headers" => super::wsgiref_headers::module(),
        "handlers" => super::wsgiref_handlers::module(),
        "simple_server" => super::wsgiref_simple_server::module(),
        "validate" => super::wsgiref_validate::module(),
        "types" => super::wsgiref_types::module(),
    }
}
