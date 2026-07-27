//! Pure floating-point algorithms and result classification for `math`.

use crate::error::{PyError, Result};

/// Post-call result checker matching CPython's `math_1` errno/fpclassify logic.
///
/// Mirrors CPython's two-condition check in `Modules/mathmodule.c`:
/// - `isinf(r) && isfinite(x)` and `r > 0` → `OverflowError: math range error`
/// - `isinf(r) && isfinite(x)` and `r < 0` → `ValueError: math domain error` (e.g. log(0) → -inf)
/// - `isnan(r) && !isnan(x)` → `ValueError: math domain error`
///
/// The last condition catches inputs that are ±∞ (not NaN) but produce NaN
/// in the underlying C math function, such as `sin(inf)` / `cos(inf)` / `tan(inf)`.
/// Those are domain errors even though the input itself is not finite.
#[inline]
pub(super) fn check_math_result(arg: f64, result: f64) -> Result<f64> {
    if result.is_infinite() && arg.is_finite() {
        if result > 0.0 {
            return Err(PyError::named(
                "OverflowError",
                "math range error".to_string(),
            ));
        }
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    if result.is_nan() && !arg.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    Ok(result)
}

/// Result-checker for functions whose only way to produce an infinite result
/// from a finite argument is *overflow* (the exponential / hyperbolic-growth
/// family: exp2, expm1, sinh, cosh).  Unlike `check_math_result`, a `-inf`
/// result here is a range error, not a domain error: `sinh(-1000)` overflows
/// to `-inf` and CPython reports `OverflowError` ("math range error"), whereas
/// `check_math_result` maps every `-inf` to `ValueError` (correct only for the
/// logarithmic pole at `log(0)` / `log1p(-1)`).
pub(super) fn check_math_overflow(arg: f64, result: f64) -> Result<f64> {
    if result.is_infinite() && arg.is_finite() {
        return Err(PyError::named(
            "OverflowError",
            "math range error".to_string(),
        ));
    }
    if result.is_nan() && !arg.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    Ok(result)
}

/// IEEE 754 remainder, matching CPython's `m_remainder` (Modules/mathmodule.c):
/// the nearest-integer-ties-to-even remainder of `x` by `y`.  Returns NaN for
/// the degenerate finite cases (`y == 0`) and the infinite-`x` case, which the
/// callers translate into a `ValueError`.
pub(super) fn ieee_remainder(x: f64, y: f64) -> f64 {
    if x.is_finite() && y.is_finite() {
        if y == 0.0 {
            return f64::NAN;
        }
        let absx = x.abs();
        let absy = y.abs();
        let m = absx % absy;
        let c = absy - m;
        let r = if m < c {
            m
        } else if m > c {
            -c
        } else {
            // Exact halfway: pick the value that makes the quotient even.
            m - 2.0 * ((0.5 * (absx - m)) % absy)
        };
        return 1.0_f64.copysign(x) * r;
    }
    if x.is_nan() {
        return x;
    }
    if y.is_nan() {
        return y;
    }
    if x.is_infinite() {
        return f64::NAN;
    }
    // x finite, y infinite: remainder is x unchanged.
    x
}

/// Decompose `x` into `(m, e)` with `x == m * 2**e` and `0.5 <= |m| < 1`,
/// matching C `frexp` / CPython `math.frexp`.  For `0`, `±inf`, and `NaN` the
/// exponent is `0` and `m == x`.
pub(super) fn frexp_f64(x: f64) -> (f64, i32) {
    if x == 0.0 || !x.is_finite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let raw_exp = ((bits >> 52) & 0x7ff) as i32;
    if raw_exp == 0 {
        // Subnormal: scale up by 2**64 to normalise, then adjust the exponent.
        let (m, e) = frexp_f64(x * (2f64).powi(64));
        return (m, e - 64);
    }
    // Normalised: exponent is biased by 1022 so the mantissa lands in [0.5, 1).
    let e = raw_exp - 1022;
    let m = f64::from_bits((bits & !(0x7ffu64 << 52)) | (1022u64 << 52));
    (m, e)
}

/// Compute `x * 2**exp`, matching C `ldexp` / CPython `math.ldexp`.
pub(super) fn ldexp_f64(x: f64, exp: i32) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    // Scale in bounded steps so we keep correct gradual underflow into the
    // subnormal range: a single `2f64.powi(exp)` underflows to 0 once
    // `exp < -1074` even when `x * 2**exp` is still a representable subnormal.
    // 2**±1000 is always finite-and-normal, so multiplying by it never loses a
    // bit; the residual `|exp| <= 1000` step lands the result exactly.
    let mut e = exp;
    let mut r = x;
    while e > 1000 {
        r *= 2f64.powi(1000);
        e -= 1000;
        if r.is_infinite() {
            return r;
        }
    }
    while e < -1000 {
        r *= 2f64.powi(-1000);
        e += 1000;
        if r == 0.0 {
            return r;
        }
    }
    r * 2f64.powi(e)
}

/// Overflow/underflow-safe Euclidean norm of `vec`, used by `math.hypot` and
/// `math.dist`.  A faithful port of CPython 3.12's `vector_norm`
/// (`Modules/mathmodule.c`): it scales by the largest magnitude so the squares
/// never overflow, then accumulates the sum of scaled squares with a
/// double-length (compensated) running total plus a final differential
/// correction step.  This reproduces CPython's result byte-for-byte.
///
/// `vec` is taken by `&mut` because the subnormal-rescaling branch rewrites it
/// in place, mirroring the C code.
pub(super) fn vector_norm(vec: &mut [f64]) -> f64 {
    // Algorithm 1.1: compensated sum of two doubles with |a| >= |b|.
    fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
        let x = a + b;
        let y = (a - x) + b;
        (x, y)
    }
    // Algorithm 3.5: error-free transformation of a product (uses fused mul-add).
    fn dl_mul(x: f64, y: f64) -> (f64, f64) {
        let z = x * y;
        let zz = x.mul_add(y, -z);
        (z, zz)
    }

    let n = vec.len();
    let mut max = 0.0f64;
    let mut found_nan = false;
    for v in vec.iter_mut() {
        *v = v.abs();
        if *v > max {
            max = *v;
        }
        if v.is_nan() {
            found_nan = true;
        }
    }
    if max.is_infinite() {
        return max;
    }
    if found_nan {
        return f64::NAN;
    }
    if max == 0.0 || n <= 1 {
        return max;
    }
    let (_, max_e) = frexp_f64(max);
    if max_e < -1023 {
        // ldexp(1.0, -max_e) would overflow; convert subnormals to normals
        // by dividing through by DBL_MIN, recurse, then scale the result back.
        for v in vec.iter_mut() {
            *v /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(vec);
    }
    let scale = ldexp_f64(1.0, -max_e);
    let mut csum = 1.0f64;
    let mut frac1 = 0.0f64;
    let mut frac2 = 0.0f64;
    for &v in vec.iter() {
        let x = v * scale; // lossless scaling
        let pr = dl_mul(x, x); // lossless squaring
        let sm = dl_fast_sum(csum, pr.0); // lossless addition
        csum = sm.0;
        frac1 += pr.1; // lossy addition
        frac2 += sm.1; // lossy addition
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let pr = dl_mul(-h, h);
    let sm = dl_fast_sum(csum, pr.0);
    csum = sm.0;
    frac1 += pr.1;
    frac2 += sm.1;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h); // differential correction
    h / scale
}

/// The `steps`-th representable double after `x` toward `y`, matching CPython's
/// `math.nextafter`.  `steps` is assumed non-negative (validated by the caller).
///
/// Done with direct bit arithmetic (not a step loop): consecutive doubles have
/// consecutive sign-magnitude bit patterns, so we map each value to a totally
/// ordered `i64` key, advance by `steps`, and map back.  This is O(1) and
/// matches CPython for arbitrarily large `steps`.
pub(super) fn nextafter_f64(x: f64, y: f64, steps: i64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == y {
        // CPython returns y here (preserves -0.0 vs 0.0 of the target).
        return y;
    }
    if steps == 0 {
        return x;
    }
    // Map each f64 to a monotonic i64 key so that incrementing the key by 1
    // advances to the next representable double.  Positive floats keep their
    // bit pattern (a non-negative key); negative floats map to the negation of
    // their magnitude (an ordered negative key).  ±0.0 both map to key 0, which
    // is correct: a downward step from +0.0 and from -0.0 both reach -smallest.
    let to_ordered = |b: u64| -> i64 {
        if b & 0x8000_0000_0000_0000 == 0 {
            b as i64
        } else {
            -((b & 0x7fff_ffff_ffff_ffff) as i64)
        }
    };
    let from_ordered = |k: i64| -> u64 {
        if k >= 0 {
            k as u64
        } else {
            k.unsigned_abs() | 0x8000_0000_0000_0000
        }
    };
    let kx = to_ordered(x.to_bits()) as i128;
    let ky = to_ordered(y.to_bits()) as i128;
    // Move toward y by at most |ky - kx| steps; saturate at y.
    let dir: i128 = if ky > kx { 1 } else { -1 };
    let dist = (ky - kx).unsigned_abs();
    let advance = (steps as u128).min(dist) as i128;
    let kr = kx + dir * advance;
    f64::from_bits(from_ordered(kr as i64))
}

/// `math.ulp(x)`: the value of the least-significant bit of `x`.
pub(super) fn ulp_f64(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let ax = x.abs();
    if ax.is_infinite() {
        return ax;
    }
    let up = ax.next_up();
    if up.is_infinite() {
        // At the largest finite magnitude, step downward instead.
        ax - ax.next_down()
    } else {
        up - ax
    }
}
