pub(crate) fn py_mod_i64(a: i64, b: i64) -> i64 {
    // `i64::MIN % -1` overflows; the mathematical result is 0.
    let mut remainder = a.wrapping_rem(b);
    if (remainder > 0 && b < 0) || (remainder < 0 && b > 0) {
        remainder += b;
    }
    remainder
}

/// Port of CPython's `float_divmod` (Objects/floatobject.c).
///
/// Returns `(floordiv, mod)` for non-zero divisor `b`, using `fmod` so that
/// infinities and signed zeros propagate exactly as in CPython:
///   - `divmod(inf, 1)` → `(nan, nan)` (the `(a - mod)/b` quotient is nan)
///   - `divmod(5.0, inf)` → `(0.0, 5.0)`, `divmod(-5.0, inf)` → `(-1.0, inf)`
///
/// The remainder matches the `%` operator and the quotient matches `//`,
/// keeping `divmod(a, b) == (a // b, a % b)` for floats.  The caller is
/// responsible for raising `ZeroDivisionError` when `b == 0`.
pub(crate) fn float_divmod(a: f64, b: f64) -> (f64, f64) {
    let mut mod_ = a % b; // fmod(a, b)
    let mut div = (a - mod_) / b;
    if mod_ != 0.0 {
        // Snap the remainder's sign to the divisor's, adjusting the quotient.
        if (b < 0.0) != (mod_ < 0.0) {
            mod_ += b;
            div -= 1.0;
        }
    } else {
        // The remainder is zero; ensure it has the sign of the divisor.
        mod_ = 0.0_f64.copysign(b);
    }
    let floordiv = if div != 0.0 {
        let fl = div.floor();
        // Round-half-up on the quotient boundary, as CPython does.
        if div - fl > 0.5 { fl + 1.0 } else { fl }
    } else {
        // div is zero; ensure it has the sign of a/b.
        0.0_f64.copysign(a / b)
    };
    (floordiv, mod_)
}

/// CPython's Py_HASH_MODULUS = 2^61 - 1 (Mersenne prime).
///
/// Used by `py_hash_int` and `py_hash_bigint` to reduce hash values the
/// same way CPython's `long_hash` does.  Shared between `value_to_pykey`
/// (dict/set key storage) and the `hash()` builtin so both code paths stay
/// in sync (issue #503).
pub(crate) const PY_HASH_MODULUS: i64 = (1i64 << 61) - 1;

/// Hash an `i64` integer using CPython's Mersenne-prime scheme.
///
/// For values with `|v| < 2^61-1` the result equals `v`, subject to the
/// `-1 → -2` sentinel remap.  Larger values are reduced modulo `2^61-1`
/// first (matching CPython `long_hash`).
///
/// The `-1 → -2` remap is always applied: `-1` is the C-level `tp_hash`
/// error sentinel and must never be the hash of any Python object.
pub(crate) fn py_hash_int(v: i64) -> i64 {
    let raw = v % PY_HASH_MODULUS;
    if raw == -1 { -2 } else { raw }
}

/// Reduce a `BigInt` to an `i64` hash using CPython's Mersenne-prime scheme.
///
/// Algorithm mirrors CPython `long_hash`:
/// 1. `r = n % (2^61 - 1)` — sign-preserving remainder.
/// 2. If `r == -1`, remap to `-2` (sentinel exclusion).
///
/// The result is in `[-(2^61-2), 2^61-1]`, always fitting in `i64`.
pub(crate) fn py_hash_bigint(n: &PyBigInt) -> i64 {
    let modulus = PyBigInt::from(PY_HASH_MODULUS);
    let reduced = n.clone() % &modulus;
    let raw = reduced.to_i64().unwrap_or(0);
    if raw == -1 { -2 } else { raw }
}

fn normalize_index(index: &Value, len: usize, label: &str) -> Result<usize> {
    normalize_index_inner(index, len, label, &format!("{label} index out of range"))
}

fn normalize_index_write(index: &Value, len: usize, label: &str) -> Result<usize> {
    normalize_index_inner(
        index,
        len,
        label,
        &format!("{label} assignment index out of range"),
    )
}

fn normalize_index_inner(index: &Value, len: usize, label: &str, oor_msg: &str) -> Result<usize> {
    let mut value = match index.kind() {
        ValueKind::Int(v) => v,
        ValueKind::Bool(b) => b as i64,
        // A BigInt is only ever produced by i64 overflow, so a BigInt index can
        // never fit a valid sequence position.  CPython raises this specific
        // IndexError (not "index out of range") for such an index.
        ValueKind::BigInt(_) => {
            return Err(PyError::named(
                "IndexError",
                "cannot fit 'int' into an index-sized integer",
            ));
        }
        _ => {
            // CPython uses a different message format for string vs other sequences.
            let type_name = value_type_name_str(index);
            let msg = if label == "string" {
                format!("string indices must be integers, not '{type_name}'")
            } else {
                format!("{label} indices must be integers or slices, not {type_name}")
            };
            return Err(PyError::named("TypeError", msg));
        }
    };
    if value < 0 {
        value += len as i64;
    }
    if value < 0 || value >= len as i64 {
        return Err(PyError::named("IndexError", oor_msg));
    }
    Ok(value as usize)
}
