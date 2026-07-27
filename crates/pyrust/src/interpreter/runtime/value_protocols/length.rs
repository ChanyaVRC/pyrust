// Canonical validation for values returned by Python's `__len__` slot.

impl Interpreter {
    /// Normalize a user `__len__` result to the interpreter's Py_ssize_t-sized
    /// representation.
    ///
    /// Both `len(value)` and truth-value testing route through this boundary so
    /// they accept and reject the same protocol results:
    ///
    /// - integers, integer subclasses, and values implementing `__index__`
    ///   are accepted;
    /// - every negative integer raises `ValueError`, including a negative
    ///   bigint too small to fit in `i64`;
    /// - a non-negative integer larger than `i64::MAX` raises `OverflowError`;
    /// - non-index values retain the canonical index-protocol `TypeError`.
    pub(crate) fn normalize_len_result(&mut self, result: &Value) -> Result<i64> {
        let resolved = self.value_to_index(result, |value| {
            pyrust_core::type_err!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(value),
            )
        })?;

        match resolved.kind() {
            ValueKind::Int(value) if value < 0 => {
                Err(pyrust_core::value_err!("__len__() should return >= 0"))
            }
            ValueKind::Int(value) => Ok(value),
            ValueKind::Bool(value) => Ok(if value { 1 } else { 0 }),
            ValueKind::BigInt(value) if value.sign() == num_bigint::Sign::Minus => {
                Err(pyrust_core::value_err!("__len__() should return >= 0"))
            }
            ValueKind::BigInt(value) => {
                use crate::value::PyToPrimitive;

                value.to_i64().ok_or_else(|| {
                    pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer")
                })
            }
            _ => unreachable!("value_to_index guarantees an integer"),
        }
    }
}
