// `email.mime` package module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Reference: <https://docs.python.org/3/library/email.mime.html>

use pyrust_derive::pyrust_module;

use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("email.mime"),
    }
}
