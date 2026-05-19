use pyrust_core::{PyError, Result, Value, ValueKind};

/// Canonical list of method names dispatched by `call`.
/// Single source of truth for `has_method` and the drift-guard test.
pub const METHODS: &[&str] = &["bit_length", "bit_count", "is_integer"];

/// Returns `true` if `method` is the name of a built-in `int` method.
/// Used by `hasattr` / `getattr` to validate attribute names without
/// invoking the method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

pub fn call(method: &str, receiver: &Value, args: &[Value]) -> Result<Value> {
    match method {
        "bit_length" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("int.bit_length() takes no arguments ({} given)", args.len()),
                ));
            }
            let result = match receiver.kind() {
                // CPython: bit_length uses abs(n), so -1 and 1 both give 1.
                ValueKind::Int(n) => {
                    let abs_n = n.unsigned_abs();
                    if abs_n == 0 {
                        0i64
                    } else {
                        (64 - abs_n.leading_zeros()) as i64
                    }
                }
                // bool is a subclass of int in CPython; True==1, False==0.
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(b) => {
                    // num_bigint::BigInt::bits() returns the number of bits in
                    // the magnitude (equivalent to CPython's abs(n).bit_length()).
                    b.bits() as i64
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "descriptor 'bit_length' for 'int' objects doesn't apply to a '{}' object",
                            pyrust_core::builtin_type_name(receiver)
                        ),
                    ));
                }
            };
            Ok(Value::int(result))
        }
        "bit_count" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("int.bit_count() takes no arguments ({} given)", args.len()),
                ));
            }
            let result = match receiver.kind() {
                // CPython: bit_count counts 1-bits in abs(n).
                // (-1).bit_count() == 1 because abs(-1) == 1 == 0b1.
                ValueKind::Int(n) => n.unsigned_abs().count_ones() as i64,
                // bool is a subclass of int; True.bit_count() == 1, False.bit_count() == 0.
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(b) => {
                    // magnitude() gives the absolute value as BigUint,
                    // which has count_ones() via num_bigint.
                    b.magnitude().count_ones() as i64
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "descriptor 'bit_count' for 'int' objects doesn't apply to a '{}' object",
                            pyrust_core::builtin_type_name(receiver)
                        ),
                    ));
                }
            };
            Ok(Value::int(result))
        }
        "is_integer" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("int.is_integer() takes no arguments ({} given)", args.len()),
                ));
            }
            // int (and bool, which subclasses int) is always an integer.
            // This method exists for duck-typing parity with float.is_integer().
            Ok(Value::bool_(true))
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'int' object has no attribute '{method}'"),
        )),
    }
}
