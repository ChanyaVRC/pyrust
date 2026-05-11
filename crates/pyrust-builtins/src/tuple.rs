use pyrust_core::{PyError, Result, Value};

use crate::sequence;

pub fn call(method: &str, items: &[Value], args: Vec<Value>) -> Result<Value> {
    match method {
        "index" => sequence::seq_index(items, &args, "tuple"),
        "count" => sequence::seq_count(items, &args, "tuple"),
        _ => Err(PyError::Runtime(format!(
            "'tuple' object has no attribute '{method}'"
        ))),
    }
}
