use pyrust_core::{PyError, Result, Value};

use crate::sequence;

/// Canonical list of method names dispatched by `call`.
pub const METHODS: &[&str] = &["index", "count"];

/// Returns `true` if `method` is the name of a built-in `tuple` method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

pub fn call(method: &str, items: &[Value], args: Vec<Value>) -> Result<Value> {
    match method {
        "index" => sequence::seq_index(items, &args, "tuple"),
        "count" => sequence::seq_count(items, &args, "tuple"),
        _ => Err(PyError::Runtime(format!(
            "'tuple' object has no attribute '{method}'"
        ))),
    }
}
