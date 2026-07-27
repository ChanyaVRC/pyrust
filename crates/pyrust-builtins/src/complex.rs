//! Complex number methods.
//!
//! Currently only `conjugate` is implemented, matching CPython 3.12's `complex`
//! method set for the non-mutating, no-interpreter-needed operations.

use pyrust_core::{PyError, Result, Value, ValueKind};

pub const TYPE_NAME: &str = "complex";

/// Canonical list of method names exposed by `complex`.  Keep in sync with
/// the `match method` arm in `call` below.
pub const METHODS: &[&str] = &["conjugate", "__getnewargs__"];

pub const CLASS_ATTRS: crate::primitive_class_attrs::PrimitiveClassAttrs =
    crate::primitive_class_attrs::PrimitiveClassAttrs::new(TYPE_NAME, METHODS);

/// Returns `true` if `method` is the name of a built-in `complex` method.
/// Used by `hasattr` / `getattr` to validate attribute names without
/// invoking the method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

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
        "__getnewargs__" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "complex.__getnewargs__() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            // Unlike the other numeric types, complex.__getnewargs__ returns a
            // 2-tuple of (real, imag) as floats — these are the args complex()
            // would be reconstructed with, not the complex value wrapped.
            match receiver.kind() {
                ValueKind::Complex(re, im) => Ok(Value::tuple(vec![
                    Value::float(re),
                    Value::float(im),
                ])),
                _ => Err(PyError::named(
                    "TypeError",
                    "descriptor '__getnewargs__' for 'complex' objects doesn't apply to this object"
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
