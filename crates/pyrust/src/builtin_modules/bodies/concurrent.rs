// `concurrent` package module — 100% native-Rust Python 3.12 concurrency
// namespace package (issue #3263).
//
// Reference: <https://docs.python.org/3.12/library/concurrent.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "__package__" => Value::string("concurrent"),
    }
}
