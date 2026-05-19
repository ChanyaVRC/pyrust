use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};
use pyrust_core::{PyError, Result, Value};

/// Canonical list of instance method names dispatched by `call`.
/// Single source of truth for the drift-guard test.
///
/// Note: `fromhex` is a class method (invoked as `float.fromhex(s)`, not on a
/// float instance) and is installed separately in the float class attrs.
pub const METHODS: &[&str] = &["is_integer", "as_integer_ratio", "hex"];

/// Returns `true` if `method` is the name of a built-in `float` instance method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

pub fn call(method: &str, receiver: &Value, args: &[Value]) -> Result<Value> {
    let f = receiver.as_float_raw();
    match method {
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
            // CPython: inf.is_integer() == False, nan.is_integer() == False.
            Ok(Value::bool_(f.is_finite() && f.fract() == 0.0))
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
            if f.is_infinite() {
                return Err(PyError::named(
                    "OverflowError",
                    "cannot convert Infinity to integer ratio".to_string(),
                ));
            }
            if f.is_nan() {
                return Err(PyError::named(
                    "ValueError",
                    "cannot convert NaN to integer ratio".to_string(),
                ));
            }
            let (num, den) = float_as_integer_ratio(f);
            Ok(Value::tuple(vec![num, den]))
        }
        "hex" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("float.hex() takes no arguments ({} given)", args.len()),
                ));
            }
            Ok(Value::string(float_hex(f)))
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'float' object has no attribute '{method}'"),
        )),
    }
}

/// Implements `float.fromhex(s)` — a class method, dispatched separately.
pub fn from_hex(s: &str) -> Result<Value> {
    parse_float_hex(s.trim())
        .ok_or_else(|| {
            PyError::named(
                "ValueError",
                "invalid hexadecimal floating-point string".to_string(),
            )
        })
        .map(Value::float)
}

// ---------------------------------------------------------------------------
// as_integer_ratio
// ---------------------------------------------------------------------------

/// Decompose `f` into an exact integer ratio `(numerator, denominator)` such
/// that `f == numerator / denominator` exactly.  The denominator is always a
/// power of two.  The result is in lowest terms (trailing zeros removed from
/// the mantissa before constructing the ratio).
fn float_as_integer_ratio(f: f64) -> (Value, Value) {
    if f == 0.0 {
        // Both +0.0 and -0.0 map to (0, 1).
        return (Value::int(0), Value::int(1));
    }

    let bits = f.to_bits();
    let sign_neg = (bits >> 63) != 0;
    let biased_exp = ((bits >> 52) & 0x7ff) as i32;
    let mantissa_bits = bits & 0x000f_ffff_ffff_ffff;

    // Compute raw mantissa and binary exponent such that
    // |f| == mantissa * 2^exponent.
    let (mantissa, exponent): (u64, i32) = if biased_exp == 0 {
        // Subnormal: implicit leading 0, exponent = 1 - 1023 - 52 = -1074.
        (mantissa_bits, -1074)
    } else {
        // Normal: implicit leading 1.  exponent = biased_exp - 1023 - 52.
        (mantissa_bits | (1u64 << 52), biased_exp - 1023 - 52)
    };

    // Remove trailing zero bits to reduce the fraction (equivalent to GCD
    // with a power of 2, since denominator is always a power of 2).
    let trailing = mantissa.trailing_zeros() as i32;
    let mantissa_reduced = mantissa >> trailing;
    let exponent_adjusted = exponent + trailing;

    let abs_num = BigInt::from(mantissa_reduced);
    let num_bigint: BigInt = if sign_neg { -&abs_num } else { abs_num };

    if exponent_adjusted >= 0 {
        // f = mantissa_reduced * 2^exponent_adjusted, denominator = 1.
        let num_shifted = num_bigint << (exponent_adjusted as usize);
        (bigint_to_value(num_shifted), Value::int(1))
    } else {
        // f = mantissa_reduced / 2^(-exponent_adjusted).
        let den_exp = (-exponent_adjusted) as usize;
        let den_bigint = BigInt::from(1u64) << den_exp;
        (bigint_to_value(num_bigint), bigint_to_value(den_bigint))
    }
}

/// Convert a `BigInt` to the smallest `Value` variant that holds it exactly.
fn bigint_to_value(n: BigInt) -> Value {
    match n.to_i64() {
        Some(i) => Value::int(i),
        None => Value::bigint(n),
    }
}

// ---------------------------------------------------------------------------
// hex
// ---------------------------------------------------------------------------

/// Format an `f64` as a hex float string, matching CPython's `float.hex()`.
///
/// Format: `[-]0x{1|0}.{13-hex-digit-frac}p{+|-}{decimal-exp}`
/// Special values: `inf`, `-inf`, `nan`.
fn float_hex(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f.is_sign_negative() {
            "-inf".to_string()
        } else {
            "inf".to_string()
        };
    }
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0x0.0p+0".to_string()
        } else {
            "0x0.0p+0".to_string()
        };
    }

    let bits = f.to_bits();
    let sign_neg = (bits >> 63) != 0;
    let biased_exp = ((bits >> 52) & 0x7ff) as i32;
    let mantissa_bits = bits & 0x000f_ffff_ffff_ffff;

    let sign_str = if sign_neg { "-" } else { "" };
    let frac_hex = format!("{mantissa_bits:013x}");

    if biased_exp == 0 {
        // Subnormal: integer part 0, displayed exponent is always -1022.
        format!("{sign_str}0x0.{frac_hex}p-1022")
    } else {
        // Normal: integer part 1, exponent = biased_exp - 1023.
        let exp = biased_exp - 1023;
        let exp_sign = if exp >= 0 { "+" } else { "" };
        format!("{sign_str}0x1.{frac_hex}p{exp_sign}{exp}")
    }
}

// ---------------------------------------------------------------------------
// fromhex
// ---------------------------------------------------------------------------

/// Parse a hex float string in Python's `float.fromhex` format.
///
/// Grammar (whitespace already stripped by caller):
///   sign?  ( 'inf' | 'infinity' | 'nan' | hexfloat )
///   hexfloat = ('0x'|'0X')? hexdigits ('.' hexdigits?)? exponent?
///   exponent = ('p'|'P') sign? decimaldigits
///
/// Returns `None` if the string is not valid.
fn parse_float_hex(s: &str) -> Option<f64> {
    if s.is_empty() {
        return None;
    }

    // Parse optional sign.
    let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };

    let sign = if negative { -1.0f64 } else { 1.0f64 };

    // Special values (case-insensitive).
    let lower = s.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        return Some(sign * f64::INFINITY);
    }
    if lower == "nan" {
        // Sign is ignored for NaN in CPython.
        return Some(f64::NAN);
    }

    // Strip optional 0x / 0X prefix.
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);

    if s.is_empty() {
        return None;
    }

    // Split on 'p' or 'P' to separate mantissa from binary exponent.
    let (mantissa_str, bin_exp): (&str, i32) = if let Some(p_pos) = s.find(|c| c == 'p' || c == 'P')
    {
        let exp_part = &s[p_pos + 1..];
        let bin_exp: i32 = exp_part.parse().ok()?;
        (&s[..p_pos], bin_exp)
    } else {
        // No exponent means 2^0 = 1.
        (s, 0i32)
    };

    // Split mantissa on '.'.
    let (int_str, frac_str): (&str, &str) = if let Some(dot) = mantissa_str.find('.') {
        (&mantissa_str[..dot], &mantissa_str[dot + 1..])
    } else {
        (mantissa_str, "")
    };

    // Reject if both integer and fractional parts are empty.
    if int_str.is_empty() && frac_str.is_empty() {
        return None;
    }

    // Validate hex digits.
    if !int_str.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if !frac_str.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    // Concatenate int and frac digits to form a big integer.  This integer
    // represents the mantissa scaled by 2^(4 * frac_str.len()).
    let combined = format!("{int_str}{frac_str}");
    let frac_bits = (frac_str.len() as i32) * 4;

    // total_exp: value = combined_int * 2^total_exp.
    let total_exp = bin_exp.checked_sub(frac_bits)?;

    // Parse combined hex string as a BigInt (handles any length).
    if combined.is_empty() {
        return None;
    }
    let mantissa_bigint = BigInt::parse_bytes(combined.as_bytes(), 16)?;

    if mantissa_bigint.is_zero() {
        return Some(0.0);
    }

    // Convert to f64 via ldexp: value = sign * mantissa_bigint * 2^total_exp.
    let m_f64 = mantissa_bigint.to_f64()?;
    Some(sign * ldexp_f64(m_f64, total_exp))
}

/// Compute `x * 2^exp` for an f64, clamping to ±infinity on overflow
/// and flushing to zero on underflow (matching IEEE 754 semantics).
/// Equivalent to C's `ldexp(x, exp)`.
fn ldexp_f64(x: f64, exp: i32) -> f64 {
    // Split into steps of at most 1023 (max normal exponent range) to avoid
    // losing the mantissa for very large or very small total exponents.
    const MAX_STEP: i32 = 1023;

    if exp == 0 || x == 0.0 {
        return x;
    }

    if exp > 0 {
        let mut result = x;
        let mut remaining = exp;
        while remaining > 0 {
            let step = remaining.min(MAX_STEP);
            result *= f64::from_bits((1023u64 + step as u64) << 52);
            remaining -= step;
        }
        result
    } else {
        let mut result = x;
        let mut remaining = -exp;
        while remaining > 0 {
            // Use steps of at most 1022 to stay in the normal-exponent range.
            // The final step into subnormal territory is handled naturally by
            // IEEE 754 arithmetic.
            let step = remaining.min(1022);
            result *= f64::from_bits((1023u64 - step as u64) << 52);
            remaining -= step;
        }
        result
    }
}
