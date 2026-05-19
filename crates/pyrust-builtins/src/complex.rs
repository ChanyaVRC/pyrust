//! Complex number methods.
//!
//! Currently only `conjugate` is implemented, matching CPython 3.12's `complex`
//! method set for the non-mutating, no-interpreter-needed operations.

use pyrust_core::{PyError, Result, Value, ValueKind};

/// Canonical list of method names exposed by `complex`.  Keep in sync with
/// the `match method` arm in `call` below and with `COMPLEX_METHODS` in
/// `crates/pyrust/src/interpreter/helpers.rs`.
pub const METHODS: &[&str] = &["conjugate"];

/// Dispatch a `complex` method call.  `receiver` must be a `ValueKind::Complex`
/// value; `args` are the remaining positional arguments (the receiver itself is
/// NOT in `args`).
pub fn call(method: &str, receiver: &Value, args: Vec<Value>) -> Result<Value> {
    match method {
        "conjugate" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "complex.conjugate() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            match receiver.kind() {
                ValueKind::Complex(re, im) => Ok(Value::complex(re, -im)),
                _ => Err(PyError::named(
                    "TypeError",
                    "descriptor 'conjugate' for 'complex' objects doesn't apply to this object"
                        .to_string(),
                )),
            }
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'complex' object has no attribute '{method}'"),
        )),
    }
}
