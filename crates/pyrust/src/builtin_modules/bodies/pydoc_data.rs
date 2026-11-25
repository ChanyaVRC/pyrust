// `pydoc_data` package module — 100% native Rust implementation (issue #3255).
//
// Exposes `pydoc_data.topics` so `import pydoc_data`, `import pydoc_data.topics`,
// and `from pydoc_data import topics` all share exact module identity.

use pyrust_derive::pyrust_module;

use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("pydoc_data"),
        "topics" => super::pydoc_data_topics::module(),
    }
}
