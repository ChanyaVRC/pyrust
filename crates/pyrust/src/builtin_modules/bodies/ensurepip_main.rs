// `ensurepip.__main__` submodule — 100% native Rust (issue #3267).
//
// Reference: <https://docs.python.org/3.12/library/ensurepip.html>

use pyrust_derive::pyrust_module;

use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("ensurepip"),
    }
}
