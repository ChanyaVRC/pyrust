// `pydoc_data` package module — 100% native Rust implementation (issue #3255).
//
// `pydoc_data.topics` is loaded lazily on explicit import, matching CPython 3.12.

use pyrust_derive::pyrust_module;

use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("pydoc_data"),
    }
}
