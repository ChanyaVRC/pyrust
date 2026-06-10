use pyrust_core::{PyError, Result, Value};

use crate::sequence;

/// Canonical list of method names dispatched by `call`.
pub const METHODS: &[&str] = &["__iter__", "index", "count"];

/// Returns `true` if `method` is the name of a built-in `tuple` method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Returns `true` if `method` may need mutable interpreter access — i.e. it
/// can fire user-defined `__eq__` while scanning the tuple (`index`/`count`).
/// Mirrors `list::requires_interpreter` so the VM dispatcher routes both
/// sequence types through the same predicate rather than a hardcoded inline
/// `method == "index"` check.  Single source of truth for the carve-out (see
/// `crates/pyrust-builtins/README.md`).
pub fn requires_interpreter(method: &str) -> bool {
    matches!(method, "index" | "count")
}

pub fn call(method: &str, items: &[Value], args: Vec<Value>) -> Result<Value> {
    match method {
        "index" => sequence::seq_index(items, &args, "tuple"),
        "count" => sequence::seq_count(items, &args, "tuple"),
        // Intercepted upstream in vm.rs / calls.rs; sentinel for drift guard.
        "__iter__" => Err(PyError::named(
            "TypeError",
            "'tuple' __iter__ must be dispatched by the interpreter",
        )),
        _ => Err(PyError::Runtime(format!(
            "'tuple' object has no attribute '{method}'"
        ))),
    }
}
