/// Round a float to the nearest integer using banker's rounding (round half to even),
/// matching CPython's `round(x)` with no ndigits argument.
pub(crate) fn py_round_half_even(v: f64) -> i64 {
    let floor = v.floor();
    let diff = v - floor;
    if diff < 0.5 {
        floor as i64
    } else if diff > 0.5 {
        (floor + 1.0) as i64
    } else {
        // Exactly 0.5: round to even
        let floor_i = floor as i64;
        if floor_i % 2 == 0 {
            floor_i
        } else {
            floor_i + 1
        }
    }
}

/// Round a float to the nearest integer (banker's rounding) for `round(x)`
/// with no `ndigits`, which returns an `int`.  Non-finite inputs cannot be
/// converted to an integer, so they raise the same errors as `int(float)` /
/// `math.floor` / `math.ceil`:
/// - `OverflowError("cannot convert float infinity to integer")` for ±inf
/// - `ValueError("cannot convert float NaN to integer")` for NaN
///
/// Finite inputs delegate to [`py_round_half_even`].
pub(crate) fn py_round_half_even_checked(v: f64) -> crate::error::Result<i64> {
    if v.is_nan() {
        return Err(crate::error::PyError::named(
            "ValueError",
            "cannot convert float NaN to integer".to_string(),
        ));
    }
    if v.is_infinite() {
        return Err(crate::error::PyError::named(
            "OverflowError",
            "cannot convert float infinity to integer".to_string(),
        ));
    }
    Ok(py_round_half_even(v))
}

/// Round an f64 to `ndigits` decimal places using CPython's half-even semantics.
///
/// CPython's `float.__round__(ndigits)` determines the rounding direction from the
/// float's **exact** rational value (via `_Py_dg_dtoa` internally), not from a
/// scaled intermediate float.  The naïve multiply-round-divide approach fails for
/// values like `round(2.675, 2)` because `2.675 * 100` may be exactly `267.5` in
/// f64, rounding the wrong way, even though the true exact value of the IEEE 754
/// float `2.675` is slightly *below* 2.675.
///
/// For `n >= 0` this function delegates to Rust's built-in `{:.prec$}` formatter,
/// which already uses the exact float value internally (Grisu3/Dragon4), then
/// parses the string back to f64.  NaN and infinities pass through as-is.
///
/// For `n < 0` (rounding to the nearest `10^(-n)`) the function uses big-integer
/// exact arithmetic: the float's mantissa and binary exponent are extracted from
/// the IEEE 754 bits, scaled to integers, and compared against the target factor
/// to determine the tie-breaking direction without any floating-point rounding.
/// NaN and infinities pass through as-is.
pub(crate) fn round_float_ndigits(v: f64, n: i32) -> crate::error::Result<Value> {
    if n >= 0 {
        // A f64 has at most 1074 significant decimal digits (the subnormal 5e-324
        // has exactly 324 significant decimal digits; normal floats have fewer).
        // Any ndigits > 1074 cannot change the float's value, so return v unchanged.
        // This cap also prevents Rust's formatter from panicking: format!("{:.prec$}")
        // panics when prec >= 65536, and ndigits_i32 can be as large as i32::MAX.
        if n > 1074 {
            return Ok(Value::float(v));
        }
        let prec = n as usize;
        // Rust's {:.prec$} formatter uses the exact float value (Grisu3/Dragon4),
        // so it correctly rounds 2.675 to 2.67 (the exact IEEE 754 value is slightly
        // below 2.675).  Parse back to f64 to recover the rounded float.
        // NaN and ±Inf format as "NaN" / "inf" / "-inf" and parse back unchanged.
        let s = format!("{:.prec$}", v, prec = prec);
        // parse() produces -0.0 for "-0.00" etc., matching CPython's sign semantics.
        let result: f64 = s.parse().unwrap_or(v);
        return Ok(Value::float(result));
    }

    // n < 0: round to nearest 10^(-n).  NaN and ±Inf pass through unchanged
    // (CPython: round(nan, -2) == nan, round(inf, -2) == inf).
    if !v.is_finite() {
        return Ok(Value::float(v));
    }

    // n < 0: round to nearest 10^(-n).  Use exact big-integer arithmetic.
    let neg_n = (-n) as u32;

    // 10^neg_n as a float: guard against factor overflow.
    let factor = 10f64.powf(neg_n as f64);
    if factor.is_infinite() {
        // 10^neg_n doesn't fit in f64 — any finite v rounds to signed zero.
        return Ok(Value::float(if v.is_sign_negative() {
            -0.0f64
        } else {
            0.0f64
        }));
    }

    // Decompose |v| = m * 2^e2  (exact IEEE 754 representation).
    let bits = v.to_bits();
    let sign_neg = (bits >> 63) != 0;
    let biased_exp = ((bits >> 52) & 0x7FF) as i32;
    let mantissa_bits = bits & ((1u64 << 52) - 1);

    // e2 = biased_exp - 1023 - 52  (normal); -1074 for subnormals.
    let (m_u64, e2): (u64, i32) = if biased_exp == 0 {
        (mantissa_bits, -1074)
    } else {
        (mantissa_bits | (1u64 << 52), biased_exp - 1075)
    };

    // v = sign * m_u64 * 2^e2.
    //
    // To avoid fractions: if e2 < 0, we scale both |v| and the factor by 2^(-e2).
    // Then:  |v| * 2^(-e2)  = m_u64           (exact integer)
    //        factor * 2^(-e2) = 10^neg_n * 2^(-e2)
    //
    // If e2 >= 0: |v| * 1 = m_u64 * 2^e2     (exact integer)
    //             factor * 1 = 10^neg_n
    //
    // In both cases the quotient |v| / factor equals v_num / factor_scaled exactly.
    let e2_neg = if e2 < 0 { (-e2) as u32 } else { 0u32 };

    let v_num: PyBigInt = if e2 >= 0 {
        let pow2 = PyPow::pow(PyBigInt::from(2u64), e2 as u32);
        PyBigInt::from(m_u64) * pow2
    } else {
        // e2 < 0: v_num = m (the 2^(-e2) scaling is absorbed into factor_scaled)
        PyBigInt::from(m_u64)
    };

    let factor_bigint = PyPow::pow(PyBigInt::from(10u64), neg_n);
    let factor_scaled: PyBigInt = if e2_neg > 0 {
        let pow2 = PyPow::pow(PyBigInt::from(2u64), e2_neg);
        factor_bigint * pow2
    } else {
        factor_bigint
    };

    // floor-divmod: v_num = q * factor_scaled + r,  0 <= r < factor_scaled.
    let (q, r) = bigint_divmod_floor(&v_num, &factor_scaled);

    // Compare 2*r to factor_scaled to determine half-even rounding direction.
    use num_traits::Zero;
    let two_r = &r + &r;
    use std::cmp::Ordering;
    let q_rounded: PyBigInt = match two_r.cmp(&factor_scaled) {
        Ordering::Less => q,
        Ordering::Greater => &q + &PyBigInt::from(1u64),
        Ordering::Equal => {
            // Exactly at the halfway point: round to even.
            if (&q % &PyBigInt::from(2u64)).is_zero() {
                q
            } else {
                &q + &PyBigInt::from(1u64)
            }
        }
    };

    // result = q_rounded * 10^neg_n as f64.
    // q_rounded is small (it is floor(|v| / 10^neg_n) ± 1), so converting to f64
    // via to_f64() is accurate for reasonable inputs.  The result is then multiplied
    // by the float factor (which is exact for powers of 10 that fit in f64).
    use num_traits::ToPrimitive;
    let q_f64 = q_rounded.to_f64().unwrap_or(f64::INFINITY);
    let result = q_f64 * factor;

    // Overflow check: if the rounded result doesn't fit in f64.
    if v.is_finite() && result.is_infinite() {
        return Err(PyError::named(
            "OverflowError",
            "rounded value too large to represent".to_string(),
        ));
    }

    // Sign: negative zero is preserved when |v| rounds to zero and v was negative.
    let result = if result == 0.0 {
        if sign_neg { -0.0f64 } else { 0.0f64 }
    } else if sign_neg {
        -result
    } else {
        result
    };

    Ok(Value::float(result))
}

/// Round a `PyBigInt` to the nearest `10^neg_n` using banker's rounding.
///
/// This implements CPython's `int.__round__(ndigits)` semantics for negative
/// `ndigits`: divide by `factor = 10^neg_n` using floor division, keep the
/// floor multiple, then apply half-even tie-breaking.  The result is returned
/// as `Value::int` if it fits in `i64`, otherwise `Value::bigint`.
///
/// Called by `round()` in builtins for `Int`, `Bool`, and `BigInt` inputs
/// when `ndigits` is negative.
pub(crate) fn round_bigint_neg_ndigits(x: PyBigInt, neg_n: u32) -> Value {
    use num_traits::ToPrimitive;

    // Early-exit: if 10^neg_n is so large that even the biggest possible
    // rounding (halfway up) can't reach the first non-zero multiple, the
    // result is always 0.  This prevents the hang that occurs when neg_n is
    // clamped from a large-negative BigInt ndigits to i32::MAX (~2 billion),
    // which would otherwise cause PyPow::pow(10, 2_147_483_647) to allocate
    // an ~850 MB intermediate value.  CPython returns 0 for this case too.
    //
    // A BigInt with D decimal digits satisfies |x| < 10^D, so:
    //   - At neg_n == D: rounding to 10^D is possible if |x| >= 5*10^(D-1).
    //   - At neg_n > D: |x| < 10^D < 10^neg_n / 10, which is always less
    //     than half = 10^neg_n / 2, so the rounded value is always 0.
    //
    // The exact decimal digit count via to_str_radix(10) is O(digits) but
    // this path is not hot (only reached for BigInt rounding).
    let decimal_digits = x.magnitude().to_str_radix(10).len() as u32;
    if neg_n > decimal_digits {
        return Value::int(0);
    }

    let factor = PyPow::pow(PyBigInt::from(10i64), neg_n);
    let half = &factor / PyBigInt::from(2i64);
    // floor-divmod: 0 ≤ r < factor, q = floor(x / factor)
    let (q, r) = bigint_divmod_floor(&x, &factor);
    let base = &q * &factor;
    let rounded = if r < half {
        base
    } else if r > half {
        base + &factor
    } else {
        // Tie: banker's rounding — round to even quotient.
        if (&q % PyBigInt::from(2i64)).is_zero() {
            base
        } else {
            base + &factor
        }
    };
    match rounded.to_i64() {
        Some(v) => Value::int(v),
        None => Value::bigint(rounded),
    }
}

/// Modular exponentiation: (base^exp) % modulus using BigInt arithmetic.
///
/// Callers MUST ensure exp >= 0 and modulus != 0 before calling; this
/// function panics if either precondition is violated (delegated to
/// BigInt::modpow).  The result is returned as Value::int when it fits in
/// i64, otherwise Value::bigint.
pub(crate) fn modpow_bigint(base: &PyBigInt, exp: &PyBigInt, modulus: &PyBigInt) -> Value {
    use num_traits::ToPrimitive;
    let result = base.modpow(exp, modulus);
    match result.to_i64() {
        Some(v) => Value::int(v),
        None => Value::bigint(result),
    }
}

/// Modular exponentiation: (base^exp) % modulus for i64.
///
/// Intermediate products are widened to i128 to prevent overflow when
/// `modulus` is large (up to ~2^62).  `(i64::MAX)^2 ≈ 2^126 < i128::MAX`,
/// so all intermediates fit exactly.  Issue #1697.
pub(crate) fn modpow_i64(base: i64, exp: u64, modulus: i64) -> i64 {
    if modulus == 1 {
        return 0;
    }
    let m = modulus as i128;
    let mut result: i128 = 1;
    let mut base = ((base as i128 % m) + m) % m;
    let mut exp = exp;
    while exp > 0 {
        if exp % 2 == 1 {
            result = (result * base) % m;
        }
        exp >>= 1;
        base = (base * base) % m;
    }
    result as i64
}

/// Modular inverse of `value` modulo `|modulus|` using the extended Euclidean
/// algorithm.  Returns `None` if the inverse does not exist (i.e.
/// `gcd(value, |modulus|) != 1`).
///
/// The result is in the range `[0, |modulus| - 1]` (always non-negative).
/// Callers that need the result adjusted for a negative `modulus` must handle
/// the sign themselves.
///
/// `modulus` must not be zero.
pub(crate) fn modinv_bigint(value: &PyBigInt, modulus: &PyBigInt) -> Option<PyBigInt> {
    use num_traits::One;

    // Absolute value of modulus so the algorithm works on positive numbers.
    let m: PyBigInt = if *modulus < PyBigInt::from(0i64) {
        -modulus
    } else {
        modulus.clone()
    };

    // Reduce value modulo m so old_r starts non-negative.
    let v = ((value % &m) + &m) % &m;

    // Extended Euclidean algorithm (Knuth Vol. 2, §4.5.2 Algorithm X).
    let mut old_r = v;
    let mut r = m.clone();
    let mut old_s = PyBigInt::one();
    let mut s = PyBigInt::from(0i64);

    while r != PyBigInt::from(0i64) {
        let quotient = &old_r / &r;
        let tmp_r = old_r - &quotient * &r;
        old_r = r;
        r = tmp_r;
        let tmp_s = old_s - &quotient * &s;
        old_s = s;
        s = tmp_s;
    }

    // old_r is gcd(value, m).  Inverse exists only when gcd == 1.
    if old_r != PyBigInt::one() {
        return None;
    }

    // Normalise the Bézout coefficient to [0, m).
    let result = ((old_s % &m) + &m) % &m;
    Some(result)
}

/// Modular inverse of `value` modulo `|modulus|` for i64.  Returns `None` if
/// the inverse does not exist.  The result is in `[0, |modulus| - 1]`.
///
/// Callers must ensure `modulus != 0`.
pub(crate) fn modinv_i64(value: i64, modulus: i64) -> Option<i64> {
    let m = modulus.unsigned_abs() as i128;
    if m == 0 {
        return None;
    }
    // Reduce value modulo m so old_r starts non-negative.
    let v = ((value as i128 % m) + m) % m;
    let mut old_r = v;
    let mut r = m;
    let mut old_s: i128 = 1;
    let mut s: i128 = 0;

    while r != 0 {
        let quotient = old_r / r;
        let tmp_r = old_r - quotient * r;
        old_r = r;
        r = tmp_r;
        let tmp_s = old_s - quotient * s;
        old_s = s;
        s = tmp_s;
    }

    if old_r != 1 {
        return None;
    }

    // Normalise to [0, m).
    let result = ((old_s % m) + m) % m;
    Some(result as i64)
}

/// CPython's `_Py_HashDouble` algorithm for float hashing.
///
/// Implements the Mersenne-prime hash (P = 2^61 - 1) that CPython uses for
/// floating-point values.  The float is represented as `m * 2^e` (with integer
/// `m` and `e`) and the hash is `m * 2^e mod P`, signed to match the float's
/// sign.  The `-1 → -2` sentinel remap is applied at the end.
///
/// Special cases:
/// - `+inf` → `314159`, `-inf` → `-314159`  (CPython: `sys.hash_info.inf`)
/// - `NaN`  → a stable per-object hash derived from its minted identity payload
/// - `0.0` / `-0.0` → `0`
/// - Integral floats (e.g. `1.0`, `2.0`) hash the same as the corresponding
///   integer: `hash(1.0) == hash(1)` (CPython invariant).
pub(crate) fn py_hash_float(v: f64) -> i64 {
    // CPython Mersenne prime: P = 2^61 - 1.
    const P: u64 = (1u64 << 61) - 1;

    if v.is_infinite() {
        return if v > 0.0 { 314159 } else { -314159 };
    }
    if v.is_nan() {
        return pyrust_core::py_hash_nan(v);
    }
    if v == 0.0 {
        return 0;
    }

    // Decompose v using IEEE 754 bits: [sign(1)][exponent(11)][mantissa(52)].
    let bits = v.to_bits();
    let sign: i64 = if v < 0.0 { -1 } else { 1 };
    let biased_exp = ((bits >> 52) & 0x7ff) as i64;
    let mantissa_bits = bits & 0x000f_ffff_ffff_ffff;

    // Build (m, e) such that |v| = m * 2^e with m a positive integer.
    // For normal numbers:  m = mantissa | (1 << 52),  e = biased_exp - 1023 - 52
    // For subnormal numbers: m = mantissa,             e = 1 - 1023 - 52 = -1074
    let (m, e): (u64, i64) = if biased_exp == 0 {
        (mantissa_bits, -1074)
    } else {
        (mantissa_bits | (1u64 << 52), biased_exp - 1023 - 52)
    };

    // Compute h = m * 2^e mod P.
    //
    // Key identity: 2^61 ≡ 1 (mod P), so only the residue of e mod 61 matters
    // for the shift direction.
    //
    // Positive e: h = m * 2^(e mod 61) mod P.
    //   m fits in 53 bits, 2^(e mod 61) fits in 61 bits; product ≤ 2^114 → u128.
    //
    // Negative e: h = m * inv(2^|e|) mod P.
    //   inv(2^k) ≡ 2^(61 - k mod 61) mod P  when k mod 61 ≠ 0,
    //            ≡ 1                          when k mod 61 == 0.
    //   (Proof: 2^(k mod 61) * 2^(61 - k mod 61) = 2^61 ≡ 1 mod P.)
    let h: u64 = if e >= 0 {
        let shift = (e as u64) % 61;
        ((m as u128 * (1u128 << shift)) % (P as u128)) as u64
    } else {
        let neg_e_mod = ((-e) as u64) % 61;
        let inv_shift = if neg_e_mod == 0 { 0u64 } else { 61 - neg_e_mod };
        ((m as u128 * (1u128 << inv_shift)) % (P as u128)) as u64
    };

    // Apply sign, then remap the C-level sentinel -1 to -2.
    let signed = h as i64 * sign;
    if signed == -1 { -2 } else { signed }
}
