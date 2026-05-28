use pyrust_core::{PyBigInt, PyError, PyToPrimitive, PyZero, Result, Value};

/// Canonical list of *instance* method names dispatched by [`call`].
/// Single source of truth for `has_method` and the drift-guard test.
/// `fromhex` is a classmethod and is registered separately in `helpers.rs`.
/// Note: `real` and `imag` are read-only properties intercepted in `get_attr`;
/// `conjugate` is a zero-arg method and lives here.
pub const METHODS: &[&str] = &["conjugate", "is_integer", "as_integer_ratio", "hex"];

pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Dispatch a float instance method.
///
/// `receiver` is already extracted as `f64`; `args` are the remaining
/// positional arguments (after the receiver).
pub fn call(method: &str, receiver: f64, args: &[Value]) -> Result<Value> {
    match method {
        "conjugate" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "float.conjugate() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            // float.conjugate() returns self (float has no imaginary part).
            Ok(Value::float(receiver))
        }
        "is_integer" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "float.is_integer() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            // NaN and Inf are not integers; all finite floats with zero
            // fractional part are integers.
            Ok(Value::bool_(
                receiver.is_finite() && receiver.fract() == 0.0,
            ))
        }

        "as_integer_ratio" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "float.as_integer_ratio() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            if receiver.is_infinite() {
                return Err(PyError::named(
                    "OverflowError",
                    "cannot convert Infinity to integer ratio".to_string(),
                ));
            }
            if receiver.is_nan() {
                return Err(PyError::named(
                    "ValueError",
                    "cannot convert NaN to integer ratio".to_string(),
                ));
            }
            let (numerator, denominator) = float_as_integer_ratio(receiver);
            Ok(Value::tuple(vec![numerator, denominator]))
        }

        "hex" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("float.hex() takes no arguments ({} given)", args.len()),
                ));
            }
            Ok(Value::string(float_to_hex(receiver)))
        }

        _ => Err(PyError::Runtime(format!(
            "'float' object has no attribute '{method}'"
        ))),
    }
}

/// Parse a hex float string (CPython's `float.fromhex` semantics).
///
/// Accepts optional leading/trailing whitespace; handles `inf`, `-inf`,
/// `nan`, `0x`/`-0x` prefix, fractional hex mantissa, and `p`/`P` exponent.
/// Raises `ValueError` on malformed input, matching CPython 3.12.
pub fn fromhex(s: &str) -> Result<f64> {
    let t = s.trim();

    // Special cases: CPython accepts these case-insensitively (with optional sign).
    if t.eq_ignore_ascii_case("inf") || t.eq_ignore_ascii_case("+inf") {
        return Ok(f64::INFINITY);
    }
    if t.eq_ignore_ascii_case("-inf") {
        return Ok(f64::NEG_INFINITY);
    }
    if t.eq_ignore_ascii_case("nan")
        || t.eq_ignore_ascii_case("+nan")
        || t.eq_ignore_ascii_case("-nan")
    {
        return Ok(f64::NAN);
    }

    let bad = || PyError::named("ValueError", "invalid hexadecimal floating-point string");

    // Optional sign
    let (neg, rest) = if let Some(r) = t.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = t.strip_prefix('+') {
        (false, r)
    } else {
        (false, t)
    };

    // Optional 0x / 0X prefix
    let rest = rest
        .strip_prefix("0x")
        .or_else(|| rest.strip_prefix("0X"))
        .unwrap_or(rest);

    // Require 'p' or 'P' exponent marker
    let p_pos = rest.find(|c| c == 'p' || c == 'P').ok_or_else(bad)?;
    let mant_str = &rest[..p_pos];
    let exp_str = &rest[p_pos + 1..];

    // Parse binary exponent as a wide integer to detect absurd values
    let bin_exp: i64 = exp_str.parse::<i64>().map_err(|_| bad())?;

    // Split mantissa into integer and fractional hex parts
    let (int_hex, frac_hex) = if let Some(dot) = mant_str.find('.') {
        (&mant_str[..dot], &mant_str[dot + 1..])
    } else {
        (mant_str, "")
    };

    if int_hex.is_empty() && frac_hex.is_empty() {
        return Err(bad());
    }
    if !int_hex.chars().all(|c| c.is_ascii_hexdigit())
        || !frac_hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(bad());
    }

    // mantissa_value = (int_hex ++ frac_hex) as a big hex integer
    // The value represented is: mantissa_value * 2^eff_exp
    // where eff_exp = bin_exp - 4 * frac_hex.len()
    let combined = format!("{int_hex}{frac_hex}");
    if combined.is_empty() {
        return Err(bad());
    }

    let mantissa_big = PyBigInt::parse_bytes(combined.as_bytes(), 16).ok_or_else(bad)?;

    // Handle zero mantissa
    if PyZero::is_zero(&mantissa_big) {
        return Ok(if neg { -0.0f64 } else { 0.0f64 });
    }

    let frac_bits = frac_hex.len() as i64 * 4; // each hex digit = 4 bits
    let eff_exp: i64 = bin_exp - frac_bits;

    // We need to compute mantissa_big * 2^eff_exp as an f64.
    // Strategy: count the significant bits of mantissa_big, shift it to a
    // 53-bit window (the IEEE 754 double mantissa), then adjust the exponent.
    // This gives correct results for normals and subnormals without using
    // `powi` which can underflow to 0 for very negative exponents.
    let mantissa_bits = mantissa_big.bits() as i64; // number of bits in mantissa_big
    // After normalising to a 53-bit integer `m`, the f64 value is `m * 2^adj`.
    // IEEE 754 normalised form: (1.fraction) * 2^e  where the leading 1 is implicit.
    // m has 53 bits, so m = 1.xxx... * 2^52, giving e = adj + 52.
    // adj = eff_exp - shift = eff_exp - (53 - mantissa_bits)
    // true_exp (IEEE 754 unbiased) = adj + 52 = eff_exp - (53 - mantissa_bits) + 52
    //                               = eff_exp + mantissa_bits - 1
    let true_exp = eff_exp + mantissa_bits - 1;

    // Build the f64 from the 53-bit mantissa.
    // Shift mantissa_big to exactly 53 significant bits.
    let shift: i64 = 53 - mantissa_bits; // may be negative
    let mantissa_53_big = if shift >= 0 {
        &mantissa_big << shift as u64
    } else {
        // Round to nearest even when truncating bits
        let drop = (-shift) as u64;
        let half = PyBigInt::from(1u64) << (drop - 1);
        let mask = (PyBigInt::from(1u64) << drop) - PyBigInt::from(1u64);
        let lo = &mantissa_big & &mask;
        let shifted = &mantissa_big >> drop;
        // Round: add 1 if lo > half, or lo == half and shifted is odd
        if lo > half || (lo == half && (&shifted & PyBigInt::from(1u64)) != PyBigInt::from(0u64)) {
            shifted + PyBigInt::from(1u64)
        } else {
            shifted
        }
    };

    // Convert the 53-bit integer to u64
    let mantissa_53: u64 = mantissa_53_big.to_u64().unwrap_or(1u64 << 53);

    // If rounding caused mantissa to become 2^53, increment exponent
    let (mantissa_53, true_exp) = if mantissa_53 >= (1u64 << 53) {
        (mantissa_53 >> 1, true_exp + 1)
    } else {
        (mantissa_53, true_exp)
    };

    // IEEE 754 double: biased exponent range [1, 2046] for normals,
    // 0 for subnormals, 2047 for inf/nan.
    // Normal exponent range: [-1022, 1023]  (biased: 1..=2046)
    // Subnormal: true_exp = -1022 with no implicit leading 1 (biased: 0)
    let f = if true_exp > 1023 {
        return Err(PyError::named(
            "OverflowError",
            "hexadecimal value too large to represent as a float",
        ));
    } else if true_exp >= -1022 {
        // Normal number
        let biased = (true_exp + 1023) as u64;
        let frac_bits_u64 = mantissa_53 & 0x000f_ffff_ffff_ffff; // drop implicit leading 1
        f64::from_bits((biased << 52) | frac_bits_u64)
    } else {
        // Subnormal or underflow to zero
        // Subnormal: biased exponent = 0, value = mantissa * 2^(-1074)
        // We need to shift mantissa right by (-(true_exp + 1022)) more bits
        let extra_shift = (-(true_exp + 1022)) as u64; // how many more bits to drop
        if extra_shift >= 53 {
            // Underflow to 0 — CPython returns ±0.0 for values smaller than
            // the smallest subnormal
            0.0f64
        } else {
            let sub_mantissa = mantissa_53 >> extra_shift;
            f64::from_bits(sub_mantissa) // biased exponent 0
        }
    };

    Ok(if neg { -f } else { f })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Exact rational representation of a finite `f64`.
///
/// Returns `(numerator, denominator)` as [`Value`]s (either `Int` or `BigInt`)
/// such that `receiver == numerator / denominator` exactly and
/// `gcd(numerator, denominator) == 1`.  Panics on NaN/Inf — callers must
/// check first.
fn float_as_integer_ratio(f: f64) -> (Value, Value) {
    debug_assert!(f.is_finite(), "as_integer_ratio called on non-finite float");

    if f == 0.0 {
        return (Value::int(0), Value::int(1));
    }

    // IEEE 754 double decomposition:
    //   bits[62:52] — biased exponent (0 = denormal/zero)
    //   bits[51:0]  — mantissa fraction
    let bits = f.to_bits();
    let sign_neg = bits >> 63 != 0;
    let exp_raw = ((bits >> 52) & 0x7ff) as i32;
    let frac_bits = bits & 0x000f_ffff_ffff_ffff;

    let (mantissa_u64, bin_exp): (u64, i32) = if exp_raw == 0 {
        // Subnormal: no implicit leading bit.  Value = frac_bits * 2^(-1074).
        (frac_bits, -1074)
    } else {
        // Normal: implicit leading 1.  Value = (frac_bits | 2^52) * 2^(exp_raw - 1075).
        (frac_bits | 0x0010_0000_0000_0000, exp_raw - 1075)
    };

    // Remove trailing zero bits to reduce
    let trailing = mantissa_u64.trailing_zeros();
    let mantissa = mantissa_u64 >> trailing;
    let exp = bin_exp + trailing as i32;

    // mantissa is at most 53 bits — fits in i64
    let num_i64 = mantissa as i64;
    let num_i64 = if sign_neg { -num_i64 } else { num_i64 };

    if exp >= 0 {
        // denominator = 1, numerator = mantissa * 2^exp
        // exp can be at most ~971 (for the largest finite float),
        // so we need BigInt for the numerator.
        let num_big = PyBigInt::from(num_i64) << exp as usize;
        let num_val = bigint_to_value(num_big);
        (num_val, Value::int(1))
    } else {
        // denominator = 2^(-exp), numerator = mantissa (possibly negative)
        // (-exp) can be up to 1074 — must be BigInt.
        let denom_big = PyBigInt::from(1u64) << (-exp) as usize;
        let denom_val = bigint_to_value(denom_big);
        // numerator always fits in i64 (mantissa ≤ 53 bits)
        (Value::int(num_i64), denom_val)
    }
}

/// Produce a `Value::int` when the `BigInt` fits in i64, otherwise `Value::bigint`.
fn bigint_to_value(n: PyBigInt) -> Value {
    match n.to_i64() {
        Some(i) => Value::int(i),
        None => Value::bigint(n),
    }
}

/// Format a finite `f64` as CPython's `float.hex()` output.
///
/// Output format: `[-]0x<1>.<13-hex-digit-mantissa>p[+-]<decimal-exp>`
/// Special cases: `inf`, `-inf`, `nan` (CPython returns these without `0x`).
/// `-0.0` → `'-0x0.0p+0'` (CPython uses shortest form for zero).
fn float_to_hex(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    if f == 0.0 {
        let sign = if f.is_sign_negative() { "-" } else { "" };
        return format!("{sign}0x0.0p+0");
    }

    let bits = f.to_bits();
    let sign = if bits >> 63 != 0 { "-" } else { "" };
    let exp_raw = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = bits & 0x000f_ffff_ffff_ffff;

    if exp_raw == 0 {
        // Subnormal: value = stored_mantissa * 2^(-1074)
        // Format: 0x0.<13-hex-digits>p-1022
        // The stored 52-bit mantissa is displayed directly as 13 hex digits.
        let hex_mant = format!("{:013x}", mantissa);
        format!("{sign}0x0.{hex_mant}p-1022")
    } else {
        // Normal: implicit leading 1
        let exponent = exp_raw - 1023;
        let hex_mant = format!("{:013x}", mantissa);
        let exp_sign = if exponent >= 0 { "+" } else { "" };
        format!("{sign}0x1.{hex_mant}p{exp_sign}{exponent}")
    }
}
