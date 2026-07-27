//! Exact integer coercion and arithmetic helpers for `math`.

use crate::error::{PyError, Result};
use crate::interpreter::value_type_name_str;
use crate::value::{PyBigInt, PyToPrimitive, Value, ValueKind};

/// Extract a `Value` as a `PyBigInt` integer for math's integer-taking
/// functions (`gcd` / `lcm` / `comb` / `perm` / `factorial` / `isqrt`).
///
/// Accepts `Int`, `BigInt`, and `Bool` (bool is a subclass of int).  For a
/// `PyInstance` it follows CPython's `__index__` protocol: an `int` subclass
/// yields its primitive backing, otherwise `type(x).__index__(x)` is
/// dispatched (which must return an `int`).  `__float__` is NOT consulted —
/// CPython's integer-argument functions reject float-only objects with
/// `'X' object cannot be interpreted as an integer`.
pub(super) fn value_to_bigint_int(
    interp: &mut crate::Interpreter,
    _fn_name: &str,
    val: &Value,
) -> Result<PyBigInt> {
    // Route the int / int-subclass / `__index__` resolution through the shared
    // index protocol (#2022), then widen to `PyBigInt` (math functions need
    // arbitrary precision).  `__float__` is NOT consulted — CPython's
    // integer-argument functions reject float-only objects with the canonical
    // `'X' object cannot be interpreted as an integer`.
    let resolved = interp.value_to_index(val, |v| {
        PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(v)
            ),
        )
    })?;
    match resolved.kind() {
        ValueKind::Int(n) => Ok(PyBigInt::from(n)),
        ValueKind::BigInt(b) => Ok(b.clone()),
        ValueKind::Bool(b) => Ok(PyBigInt::from(b as i64)),
        _ => unreachable!("value_to_index guarantees an integer"),
    }
}

/// Integer coercion for `factorial()`.  CPython's `factorial` accepts the same
/// arguments as the other integer functions (int / int-subclass / `__index__`)
/// and rejects floats — so this delegates to `value_to_bigint_int`.
pub(super) fn value_to_bigint_strict_int(
    interp: &mut crate::Interpreter,
    fn_name: &str,
    val: &Value,
) -> Result<PyBigInt> {
    value_to_bigint_int(interp, fn_name, val)
}

/// Euclidean GCD for `PyBigInt`.  Result is always non-negative.
pub(super) fn bigint_gcd(mut a: PyBigInt, mut b: PyBigInt) -> PyBigInt {
    use num_traits::Signed;
    a = a.abs();
    b = b.abs();
    while b != PyBigInt::from(0i64) {
        let t = b.clone();
        b = a % b;
        a = t;
    }
    a
}

/// LCM for `PyBigInt`.  `lcm(a, b) = |a * b| / gcd(a, b)`.
/// When either argument is zero, returns zero.
pub(super) fn bigint_lcm(a: PyBigInt, b: PyBigInt) -> PyBigInt {
    use num_traits::Signed;
    let ab = a.abs() * b.abs();
    if ab == PyBigInt::from(0i64) {
        return PyBigInt::from(0i64);
    }
    let g = bigint_gcd(a, b);
    ab / g
}

/// Convert a `PyBigInt` back to the smallest `Value` representation:
/// `Value::int` when it fits in `i64`, `Value::bigint` otherwise.
pub(super) fn bigint_to_int_value(n: PyBigInt) -> Result<Value> {
    if let Some(i) = n.to_i64() {
        Ok(Value::int(i))
    } else {
        Ok(Value::bigint(n))
    }
}
