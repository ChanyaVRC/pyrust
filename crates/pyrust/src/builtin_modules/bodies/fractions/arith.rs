//! Exact integer helpers behind `Fraction`: `math.gcd`, floor division,
//! integer powers, digit-limited `str(int)`, the formatting rounders and
//! CPython's rational hash.  Every operation that CPython would perform on
//! Python `int`s keeps its error behaviour here (zero divisors raise
//! `ZeroDivisionError` instead of panicking, `str()` honours
//! `sys.set_int_max_str_digits`).

use num_bigint::BigInt;
use num_traits::{One, Signed, ToPrimitive, Zero};

use crate::error::{PyError, Result};

/// `_PyHASH_MODULUS` (`sys.hash_info.modulus` on 64-bit builds).
pub(super) const HASH_MODULUS: u64 = (1 << 61) - 1;
/// `_PyHASH_INF` (`sys.hash_info.inf`).
pub(super) const HASH_INF: i64 = 314_159;

pub(super) fn zero_division(msg: &str) -> PyError {
    PyError::named("ZeroDivisionError", msg.to_string())
}

/// `int // int` / `divmod(int, int)` by zero.
pub(super) fn int_div_by_zero() -> PyError {
    zero_division("integer division or modulo by zero")
}

/// `int % int` by zero.
pub(super) fn int_mod_by_zero() -> PyError {
    zero_division("integer modulo by zero")
}

/// `divmod(a, b)` with Python's floor semantics.
pub(super) fn divmod_floor(a: &BigInt, b: &BigInt) -> Result<(BigInt, BigInt)> {
    if b.is_zero() {
        return Err(int_div_by_zero());
    }
    Ok(pyrust_core::bigint_divmod_floor(a, b))
}

/// `a // b`.
pub(super) fn floor_div(a: &BigInt, b: &BigInt) -> Result<BigInt> {
    divmod_floor(a, b).map(|(q, _)| q)
}

/// `a % b`.
pub(super) fn floor_mod(a: &BigInt, b: &BigInt) -> Result<BigInt> {
    if b.is_zero() {
        return Err(int_mod_by_zero());
    }
    Ok(pyrust_core::bigint_divmod_floor(a, b).1)
}

fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

/// `math.gcd(a, b)` (always non-negative).
pub(super) fn gcd(a: &BigInt, b: &BigInt) -> BigInt {
    if let (Some(x), Some(y)) = (a.magnitude().to_u64(), b.magnitude().to_u64()) {
        return BigInt::from(gcd_u64(x, y));
    }
    let mut x = a.magnitude().clone();
    let mut y = b.magnitude().clone();
    while !y.is_zero() {
        if let (Some(xs), Some(ys)) = (x.to_u64(), y.to_u64()) {
            return BigInt::from(gcd_u64(xs, ys));
        }
        let r = &x % &y;
        x = y;
        y = r;
    }
    BigInt::from(x)
}

/// `base ** exp` for a non-negative exponent.
pub(super) fn pow(base: &BigInt, exp: &BigInt) -> Result<BigInt> {
    debug_assert!(!exp.is_negative());
    if exp.is_zero() || base.is_one() {
        return Ok(BigInt::one());
    }
    if base.is_zero() {
        return Ok(BigInt::zero());
    }
    if *base == BigInt::from(-1) {
        let odd = (exp % 2u32) != BigInt::zero();
        return Ok(if odd { base.clone() } else { BigInt::one() });
    }
    match exp.to_u32() {
        Some(e) => Ok(base.pow(e)),
        // |base| >= 2 with an exponent of 2**32 or more needs at least half a
        // gigabyte of digits; CPython runs out of memory as well.
        None => Err(PyError::named("MemoryError", String::new())),
    }
}

/// `10 ** exp` for a non-negative exponent.
pub(super) fn pow10(exp: &BigInt) -> Result<BigInt> {
    pow(&BigInt::from(10u32), exp)
}

/// `str(n)`, honouring `sys.set_int_max_str_digits`.
pub(super) fn int_str(n: &BigInt) -> Result<String> {
    if pyrust_core::bigint_str_digits_exceed_limit(n) {
        return Err(pyrust_core::int_max_str_digits_format_error());
    }
    Ok(n.to_string())
}

/// `int(text)` for a matched digit group (ASCII digits, optional sign, maybe
/// underscores), honouring `sys.set_int_max_str_digits`.
pub(super) fn parse_int(text: &str) -> Result<BigInt> {
    let digits = text.trim_start_matches(['-', '+']);
    pyrust_core::check_int_parse_digits(digits, 10)?;
    let cleaned: String = text.chars().filter(|&c| c != '_').collect();
    cleaned
        .parse::<BigInt>()
        .map_err(|_| PyError::named("ValueError", format!("invalid literal for int(): {text}")))
}

fn is_odd(n: &BigInt) -> bool {
    !(n % 2u32).is_zero()
}

/// `_round_to_exponent(n, d, exponent, no_neg_zero)`: round `n/d` to the
/// nearest multiple of `10**exponent` (ties to even).  Returns `(sign,
/// significand)`.
pub(super) fn round_to_exponent(
    n: &BigInt,
    d: &BigInt,
    exponent: &BigInt,
    no_neg_zero: bool,
) -> Result<(bool, BigInt)> {
    let (n, d) = if !exponent.is_negative() {
        (n.clone(), d * pow10(exponent)?)
    } else {
        (n * pow10(&-exponent)?, d.clone())
    };
    // The divmod quotient is correct for round-ties-towards-positive-infinity;
    // in the case of a tie, zero out the least significant bit of q.
    let half = floor_div(&d, &BigInt::from(2))?;
    let (mut q, r) = divmod_floor(&(&n + half), &d)?;
    if r.is_zero() && !is_odd(&d) && is_odd(&q) {
        q -= 1;
    }
    let sign = if no_neg_zero {
        q.is_negative()
    } else {
        n.is_negative()
    };
    Ok((sign, q.abs()))
}

/// `_round_to_figures(n, d, figures)`: round `n/d` to `figures` significant
/// digits.  Returns `(sign, significand, exponent)`.
pub(super) fn round_to_figures(
    n: &BigInt,
    d: &BigInt,
    figures: &BigInt,
) -> Result<(bool, BigInt, BigInt)> {
    if n.is_zero() {
        return Ok((false, BigInt::zero(), BigInt::one() - figures));
    }
    // Find an integer m with 10**(m - 1) <= abs(n)/d <= 10**m.
    let str_n = int_str(&n.abs())?;
    let str_d = int_str(d)?;
    let m = BigInt::from(str_n.len()) - BigInt::from(str_d.len())
        + BigInt::from(u8::from(str_d.as_str() <= str_n.as_str()));
    let mut exponent = m - figures;
    let (sign, mut significand) = round_to_exponent(n, d, &exponent, false)?;
    // Adjust in the case where significand == 10**figures.
    if BigInt::from(int_str(&significand)?.len()) == figures + 1 {
        significand = floor_div(&significand, &BigInt::from(10))?;
        exponent += 1;
    }
    Ok((sign, significand, exponent))
}

/// `a * b % P` for `a, b < P = 2**61 - 1`, using `2**61 ≡ 1 (mod P)`.
fn mulmod(a: u64, b: u64) -> u64 {
    let product = u128::from(a) * u128::from(b);
    let folded = (product & u128::from(HASH_MODULUS)) + (product >> 61);
    let folded = (folded & u128::from(HASH_MODULUS)) + (folded >> 61);
    let folded = folded as u64;
    if folded >= HASH_MODULUS {
        folded - HASH_MODULUS
    } else {
        folded
    }
}

fn reduce(n: &BigInt) -> u64 {
    pyrust_core::bigint_divmod_floor(n, &BigInt::from(HASH_MODULUS))
        .1
        .to_u64()
        .unwrap_or(0)
}

/// `_hash_algorithm(numerator, denominator)`: the hash shared by every
/// numerically equal `int`, `float`, `Decimal` and `Fraction`.
pub(super) fn hash_ratio(numerator: &BigInt, denominator: &BigInt) -> i64 {
    // dinv = pow(denominator, -1, _PyHASH_MODULUS); a ValueError (no inverse)
    // means the denominator is a multiple of the modulus.
    let d = reduce(denominator);
    let hash = if d == 0 {
        HASH_INF
    } else {
        // Fermat: d ** (P - 2) is the inverse of d modulo the prime P.
        let mut dinv = 1u64;
        let mut base = d;
        let mut e = HASH_MODULUS - 2;
        while e > 0 {
            if e & 1 == 1 {
                dinv = mulmod(dinv, base);
            }
            base = mulmod(base, base);
            e >>= 1;
        }
        // hash(hash(abs(numerator)) * dinv)
        mulmod(reduce(&numerator.abs()), dinv) as i64
    };
    let result = if numerator.is_negative() { -hash } else { hash };
    if result == -1 { -2 } else { result }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulmod_matches_naive_reduction() {
        let samples = [
            0,
            1,
            2,
            12_345,
            HASH_MODULUS - 1,
            HASH_MODULUS - 2,
            1 << 60,
            (1 << 61) - 3,
            0x0123_4567_89ab_cdef & HASH_MODULUS,
        ];
        for &a in &samples {
            for &b in &samples {
                let naive = (u128::from(a) * u128::from(b) % u128::from(HASH_MODULUS)) as u64;
                assert_eq!(mulmod(a, b), naive, "{a} * {b}");
            }
        }
    }

    #[test]
    fn round_helpers_follow_cpython() {
        let n = BigInt::from;
        // _round_to_exponent(5, 2, 0) == (False, 2): ties go to even.
        assert_eq!(
            round_to_exponent(&n(5), &n(2), &n(0), false).unwrap(),
            (false, n(2))
        );
        assert_eq!(
            round_to_exponent(&n(7), &n(2), &n(0), false).unwrap(),
            (false, n(4))
        );
        // A negative value rounding to zero keeps its sign unless no_neg_zero.
        assert_eq!(
            round_to_exponent(&n(-1), &n(3), &n(0), false).unwrap(),
            (true, n(0))
        );
        assert_eq!(
            round_to_exponent(&n(-1), &n(3), &n(0), true).unwrap(),
            (false, n(0))
        );
        // _round_to_figures(-999, 1, 2) == (True, 10, 2)
        assert_eq!(
            round_to_figures(&n(-999), &n(1), &n(2)).unwrap(),
            (true, n(10), n(2))
        );
        assert!(divmod_floor(&n(1), &n(0)).is_err());
        assert_eq!(pow(&n(-1), &(BigInt::one() << 40)).unwrap(), n(1));
    }
}
