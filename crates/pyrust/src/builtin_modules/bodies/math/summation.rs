//! Compensated `fsum` and `sumprod` numerical kernels.

use crate::error::{PyError, Result};

// ── Shewchuk full-precision summation (math.fsum) ────────────────────────────

/// `math.fsum(iterable)` — exactly-rounded sum of an iterable of floats.
///
/// Faithful port of CPython 3.12's `math_fsum_impl` (`Modules/mathmodule.c`),
/// which implements Shewchuk's algorithm: maintain a growing set of
/// non-overlapping partial sums whose exact (infinite-precision) total equals
/// the running sum, then round that total to the nearest double once at the end.
/// This yields the correctly-rounded result even under catastrophic
/// cancellation (`fsum([1e16, 1, -1e16]) == 1.0`).
pub(super) fn fsum_impl(values: &[f64]) -> Result<f64> {
    // `partials` holds the non-overlapping partial sums, smallest-magnitude
    // first.  `special_sum` accumulates any non-finite term; `inf_sum` tracks
    // the running infinity so that `inf + -inf` is detected as NaN.
    let mut partials: Vec<f64> = Vec::new();
    let mut special_sum = 0.0f64;
    let mut inf_sum = 0.0f64;

    for &xsave in values {
        let mut x = xsave;
        let mut i = 0usize;
        // Knuth's two-sum: fold `x` into the running partials, capturing the
        // exact low-order rounding error of each step as a new partial.
        for j in 0..partials.len() {
            let mut y = partials[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let yr = hi - x;
            let lo = y - yr;
            if lo != 0.0 {
                partials[i] = lo;
                i += 1;
            }
            x = hi;
        }
        partials.truncate(i);
        if x != 0.0 {
            if !x.is_finite() {
                // A non-finite `x` arises either from intermediate overflow or
                // from a nan/inf in the summands.  If the original element was
                // finite, this is intermediate overflow — an OverflowError.
                if xsave.is_finite() {
                    return Err(PyError::named(
                        "OverflowError",
                        "intermediate overflow in fsum".to_string(),
                    ));
                }
                if xsave.is_infinite() {
                    inf_sum += xsave;
                }
                special_sum += xsave;
                partials.clear();
            } else {
                partials.push(x);
            }
        }
    }

    if special_sum != 0.0 {
        if inf_sum.is_nan() {
            return Err(PyError::named(
                "ValueError",
                "-inf + inf in fsum".to_string(),
            ));
        }
        return Ok(special_sum);
    }

    // Sum the partials from the top (largest magnitude first), stopping as soon
    // as the running sum becomes inexact, then apply CPython's half-even
    // rounding fix-up across the remaining partials.
    let mut n = partials.len();
    let mut hi = 0.0f64;
    let mut lo = 0.0f64;
    if n > 0 {
        n -= 1;
        hi = partials[n];
        while n > 0 {
            let x = hi;
            n -= 1;
            let y = partials[n];
            hi = x + y;
            let yr = hi - x;
            lo = y - yr;
            if lo != 0.0 {
                break;
            }
        }
        // Half-even rounding across multiple partials so that, e.g.,
        // fsum([1e-16, 1, 1e16]) rounds the last digit up to two, guaranteeing
        // commutativity.
        if n > 0 && ((lo < 0.0 && partials[n - 1] < 0.0) || (lo > 0.0 && partials[n - 1] > 0.0)) {
            let y = lo * 2.0;
            let x = hi + y;
            let yr = x - hi;
            if y == yr {
                hi = x;
            }
        }
    }
    Ok(hi)
}

// ── sumprod (math.sumprod) ───────────────────────────────────────────────────

/// A double-double `(hi, lo)` pair, matching CPython's `DoubleLength`.
#[derive(Clone, Copy)]
struct DoubleLength {
    hi: f64,
    lo: f64,
}

/// Error-free transformation of a sum (CPython `dl_sum`, Algorithm 3.1).
#[inline]
fn dl_sum(a: f64, b: f64) -> DoubleLength {
    let x = a + b;
    let z = x - a;
    let y = (a - (x - z)) + (b - z);
    DoubleLength { hi: x, lo: y }
}

/// Error-free transformation of a product via FMA (CPython `dl_mul`,
/// Algorithm 3.5 — the `UNRELIABLE_FMA`-off path used on modern hardware).
#[inline]
fn dl_mul(x: f64, y: f64) -> DoubleLength {
    let z = x * y;
    let zz = x.mul_add(y, -z);
    DoubleLength { hi: z, lo: zz }
}

/// A triple-double accumulator, matching CPython's `TripleLength`.
#[derive(Clone, Copy)]
struct TripleLength {
    hi: f64,
    lo: f64,
    tiny: f64,
}

const TL_ZERO: TripleLength = TripleLength {
    hi: 0.0,
    lo: 0.0,
    tiny: 0.0,
};

/// Fused multiply-add into the triple-double total (CPython `tl_fma`,
/// Algorithm 5.10 with SumKVert for K=3).
#[inline]
fn tl_fma(x: f64, y: f64, total: TripleLength) -> TripleLength {
    let pr = dl_mul(x, y);
    let sm = dl_sum(total.hi, pr.hi);
    let r1 = dl_sum(total.lo, pr.lo);
    let r2 = dl_sum(r1.hi, sm.lo);
    TripleLength {
        hi: sm.hi,
        lo: r2.hi,
        tiny: total.tiny + r1.lo + r2.lo,
    }
}

/// Collapse the triple-double total to a rounded double (CPython `tl_to_d`).
#[inline]
fn tl_to_d(total: TripleLength) -> f64 {
    let last = dl_sum(total.lo, total.hi);
    total.tiny + last.lo + last.hi
}

/// Float path of `math.sumprod`: triple-double compensated accumulation of the
/// products, matching CPython 3.12's `math_sumprod_impl` float path.  When a
/// running step overflows to a non-finite value, CPython finalises the
/// triple-double total and continues with a plain `total + p*q` accumulation;
/// we mirror that fallback so `sumprod([1e308], [2.0]) == inf`.
pub(super) fn sumprod_floats(p: &[f64], q: &[f64]) -> f64 {
    let mut flt_total = TL_ZERO;
    let mut fell_back = false;
    let mut plain_total = 0.0f64;
    for (&a, &b) in p.iter().zip(q.iter()) {
        if fell_back {
            plain_total += a * b;
            continue;
        }
        let new_total = tl_fma(a, b, flt_total);
        if new_total.hi.is_finite() {
            flt_total = new_total;
        } else {
            // Overflow: finalise the compensated total and switch to the plain
            // running-sum fallback for this and the remaining terms.
            plain_total = tl_to_d(flt_total) + a * b;
            fell_back = true;
        }
    }
    if fell_back {
        plain_total
    } else {
        tl_to_d(flt_total)
    }
}
