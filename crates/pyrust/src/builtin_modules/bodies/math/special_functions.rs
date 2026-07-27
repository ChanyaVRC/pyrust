//! CPython-compatible gamma, lgamma, erf, and libm adapters.

use crate::error::{PyError, Result};

// CPython 3.12 implements gamma/lgamma itself (to dodge poor-quality libm
// tgamma) but evaluates the transcendental sub-terms (`exp`, `pow`, `log`,
// `sin`, `cos`) and the whole of `erf`/`erfc` through the platform libm.  To
// reproduce CPython's results to the last ULP we route the same sub-calls
// through libm rather than Rust's `std` math (which can round the final bit
// differently from glibc).
unsafe extern "C" {
    fn erf(x: f64) -> f64;
    fn erfc(x: f64) -> f64;
    fn log(x: f64) -> f64;
    fn exp(x: f64) -> f64;
    fn pow(x: f64, y: f64) -> f64;
    fn sin(x: f64) -> f64;
    fn cos(x: f64) -> f64;
}

#[inline]
fn libm_log(x: f64) -> f64 {
    // SAFETY: libm `log` has no preconditions.
    unsafe { log(x) }
}
#[inline]
fn libm_exp(x: f64) -> f64 {
    // SAFETY: libm `exp` has no preconditions.
    unsafe { exp(x) }
}
#[inline]
fn libm_pow(x: f64, y: f64) -> f64 {
    // SAFETY: libm `pow` has no preconditions.
    unsafe { pow(x, y) }
}

// ── Lanczos gamma / lgamma (math.gamma, math.lgamma) ─────────────────────────
//
// Constants and algorithm ported verbatim from CPython 3.12 Modules/mathmodule.c.
// The literal digits are kept verbatim from the C source (they round to the
// identical f64 either way); silence clippy's precision/constant lints so the
// port stays a 1:1 textual match with CPython.
#[allow(clippy::excessive_precision, clippy::approx_constant)]
const PI_M: f64 = 3.141592653589793238462643383279502884197;
#[allow(clippy::excessive_precision)]
const LANCZOS_G: f64 = 6.024680040776729583740234375;
#[allow(clippy::excessive_precision)]
const LANCZOS_G_MINUS_HALF: f64 = 5.524680040776729583740234375;
#[allow(clippy::excessive_precision)]
const LANCZOS_NUM_COEFFS: [f64; 13] = [
    23531376880.410759688572007674451636754734846804940,
    42919803642.649098768957899047001988850926355848959,
    35711959237.355668049440185451547166705960488635843,
    17921034426.037209699919755754458931112671403265390,
    6039542586.3520280050642916443072979210699388420708,
    1439720407.3117216736632230727949123939715485786772,
    248874557.86205415651146038641322942321632125127801,
    31426415.585400194380614231628318205362874684987640,
    2876370.6289353724412254090516208496135991145378768,
    186056.26539522349504029498971604569928220784236328,
    8071.6720023658162106380029022722506138218516325024,
    210.82427775157934587250973392071336271166969580291,
    2.5066282746310002701649081771338373386264310793408,
];
const LANCZOS_DEN_COEFFS: [f64; 13] = [
    0.0,
    39916800.0,
    120543840.0,
    150917976.0,
    105258076.0,
    45995730.0,
    13339535.0,
    2637558.0,
    357423.0,
    32670.0,
    1925.0,
    66.0,
    1.0,
];

const NGAMMA_INTEGRAL: usize = 23;
const GAMMA_INTEGRAL: [f64; NGAMMA_INTEGRAL] = [
    1.0,
    1.0,
    2.0,
    6.0,
    24.0,
    120.0,
    720.0,
    5040.0,
    40320.0,
    362880.0,
    3628800.0,
    39916800.0,
    479001600.0,
    6227020800.0,
    87178291200.0,
    1307674368000.0,
    20922789888000.0,
    355687428096000.0,
    6402373705728000.0,
    121645100408832000.0,
    2432902008176640000.0,
    51090942171709440000.0,
    1124000727777607680000.0,
];

/// Lanczos sum used by both gamma and lgamma.  Port of CPython `lanczos_sum`:
/// evaluate the rational approximation, choosing the Horner order that
/// minimises rounding error based on the magnitude of `x`.
fn lanczos_sum(x: f64) -> f64 {
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    if x < 5.0 {
        for i in (0..13).rev() {
            num = num * x + LANCZOS_NUM_COEFFS[i];
            den = den * x + LANCZOS_DEN_COEFFS[i];
        }
    } else {
        for i in 0..13 {
            num = num / x + LANCZOS_NUM_COEFFS[i];
            den = den / x + LANCZOS_DEN_COEFFS[i];
        }
    }
    num / den
}

/// `sin(pi*x)` computed accurately via argument reduction.  Port of CPython
/// `sinpi`.
fn sinpi(x: f64) -> f64 {
    let y = x.abs() % 2.0;
    let n = (2.0 * y).round() as i64;
    // SAFETY: libm `sin`/`cos` have no preconditions.
    let r = unsafe {
        match n {
            0 => sin(PI_M * y),
            1 => cos(PI_M * (y - 0.5)),
            2 => sin(PI_M * (1.0 - y)),
            3 => -cos(PI_M * (y - 1.5)),
            4 => sin(PI_M * (y - 2.0)),
            _ => unreachable!("sinpi: reduced argument out of range"),
        }
    };
    1.0_f64.copysign(x) * r
}

/// `math.gamma(x)` — Gamma function.  Faithful port of CPython 3.12 `m_tgamma`.
pub(super) fn m_tgamma(x: f64) -> Result<f64> {
    // NaN and +inf pass through unchanged.
    if x.is_nan() || (x.is_infinite() && x > 0.0) {
        return Ok(x);
    }
    // -inf: tgamma(-inf) is NaN — a domain error in CPython.
    if x.is_infinite() {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    // ±0.0: pole (tgamma → ±inf, divide-by-zero) → domain error.
    if x == 0.0 {
        return Err(PyError::named(
            "ValueError",
            "math domain error".to_string(),
        ));
    }
    // Integer arguments: small non-negative ints are exact; negative ints poles.
    if x == x.floor() {
        if x < 0.0 {
            return Err(PyError::named(
                "ValueError",
                "math domain error".to_string(),
            ));
        }
        if (x as usize) <= NGAMMA_INTEGRAL {
            return Ok(GAMMA_INTEGRAL[x as usize - 1]);
        }
    }
    let absx = x.abs();
    // Tiny arguments: tgamma(x) ~ 1/x near 0.
    if absx < 1e-20 {
        let r = 1.0 / x;
        if r.is_infinite() {
            return Err(PyError::named(
                "OverflowError",
                "math range error".to_string(),
            ));
        }
        return Ok(r);
    }
    // Large arguments: any |x| >= 200 overflows (or underflows for x < 0).
    if absx > 200.0 {
        if x < 0.0 {
            return Ok(0.0 / sinpi(x));
        }
        return Err(PyError::named(
            "OverflowError",
            "math range error".to_string(),
        ));
    }

    let y = absx + LANCZOS_G_MINUS_HALF;
    // Compute z = (absx - 0.5) / y accurately (CPython's careful subtraction).
    let z = if absx > LANCZOS_G_MINUS_HALF {
        let q = y - absx;
        q - LANCZOS_G_MINUS_HALF
    } else {
        let q = y - LANCZOS_G_MINUS_HALF;
        q - absx
    };
    let z = z * LANCZOS_G / y;

    let mut r;
    if x < 0.0 {
        r = -PI_M / sinpi(absx) / absx * libm_exp(y) / lanczos_sum(absx);
        r -= z * r;
        if absx < 140.0 {
            r /= libm_pow(y, absx - 0.5);
        } else {
            let sqrtpow = libm_pow(y, absx / 2.0 - 0.25);
            r /= sqrtpow;
            r /= sqrtpow;
        }
    } else {
        r = lanczos_sum(absx) / libm_exp(y);
        r += z * r;
        if absx < 140.0 {
            r *= libm_pow(y, absx - 0.5);
        } else {
            let sqrtpow = libm_pow(y, absx / 2.0 - 0.25);
            r *= sqrtpow;
            r *= sqrtpow;
        }
    }
    if r.is_infinite() {
        return Err(PyError::named(
            "OverflowError",
            "math range error".to_string(),
        ));
    }
    Ok(r)
}

/// `math.lgamma(x)` — natural log of |Gamma(x)|.  Faithful port of CPython 3.12
/// `m_lgamma`.  The statement order matches CPython exactly (compute `r` for the
/// positive branch, then apply the reflection formula on that same `r` when
/// `x < 0`) so the last-ULP rounding agrees; the `log` calls go through libm for
/// the same reason.
pub(super) fn m_lgamma(x: f64) -> Result<f64> {
    // log(pi) — CPython's `logpi` constant from mathmodule.c.
    const LOGPI: f64 = 1.144729885849400174143427351353058711647;
    if x.is_nan() {
        return Ok(x);
    }
    if x.is_infinite() {
        return Ok(f64::INFINITY);
    }
    // Integer arguments <= 2: lgamma(1) == lgamma(2) == 0; non-positive poles.
    if x == x.floor() && x <= 2.0 {
        if x <= 0.0 {
            return Err(PyError::named(
                "ValueError",
                "math domain error".to_string(),
            ));
        }
        return Ok(0.0);
    }
    let absx = x.abs();
    if absx < 1e-20 {
        return Ok(-libm_log(absx));
    }
    // Lanczos' formula, computed in CPython's statement order.
    let mut r = libm_log(lanczos_sum(absx)) - LANCZOS_G;
    r += (absx - 0.5) * (libm_log(absx + LANCZOS_G - 0.5) - 1.0);
    if x < 0.0 {
        // Reflection formula for negative x.
        r = LOGPI - libm_log(sinpi(absx).abs()) - libm_log(absx) - r;
    }
    if r.is_infinite() {
        return Err(PyError::named(
            "OverflowError",
            "math range error".to_string(),
        ));
    }
    Ok(r)
}

// ── erf / erfc (math.erf, math.erfc) ─────────────────────────────────────────
//
// CPython 3.12 registers erf/erfc via `FUNC1A(erf, erf, ...)`, i.e. it forwards
// directly to the C standard library's `erf` / `erfc` (unlike gamma/lgamma,
// which CPython implements itself to dodge poor-quality libm tgamma).  To match
// CPython to the last ULP we likewise call the platform libm functions (declared
// in the libm extern block above) rather than re-deriving a series /
// continued-fraction approximation (which diverged from libm by hundreds of ULP
// for large |x|).

/// `math.erf(x)` — forwards to libm `erf`, matching CPython 3.12.
pub(super) fn m_erf(x: f64) -> f64 {
    // SAFETY: `erf` is a pure libm function with no preconditions.
    unsafe { erf(x) }
}

/// `math.erfc(x)` — forwards to libm `erfc`, matching CPython 3.12.
pub(super) fn m_erfc(x: f64) -> f64 {
    // SAFETY: `erfc` is a pure libm function with no preconditions.
    unsafe { erfc(x) }
}
