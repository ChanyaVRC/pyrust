//! Compensated `fsum` and `sumprod` numerical kernels.

use crate::error::{PyError, Result};

// ── Shewchuk full-precision summation (math.fsum) ────────────────────────────

/// Incremental state for `math.fsum`.
///
/// This is a faithful port of CPython 3.12's `math_fsum_impl`
/// (`Modules/mathmodule.c`), split into one-element [`Self::add`] steps so the
/// Python-facing function can consume its iterator lazily. `partials` contains
/// only the non-overlapping components needed by Shewchuk's algorithm; input
/// elements are never retained.
pub(super) struct FsumState {
    // `partials` holds the non-overlapping partial sums, smallest-magnitude
    // first.  `special_sum` accumulates any non-finite term; `inf_sum` tracks
    // the running infinity so that `inf + -inf` is detected as NaN.
    partials: Vec<f64>,
    special_sum: f64,
    inf_sum: f64,
}

impl FsumState {
    pub(super) fn new() -> Self {
        Self {
            partials: Vec::new(),
            special_sum: 0.0,
            inf_sum: 0.0,
        }
    }

    /// Fold one already-coerced float into the exact running expansion.
    pub(super) fn add(&mut self, xsave: f64) -> Result<()> {
        let mut x = xsave;
        let mut i = 0usize;
        // Knuth's two-sum: fold `x` into the running partials, capturing the
        // exact low-order rounding error of each step as a new partial.
        for j in 0..self.partials.len() {
            let mut y = self.partials[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let yr = hi - x;
            let lo = y - yr;
            if lo != 0.0 {
                self.partials[i] = lo;
                i += 1;
            }
            x = hi;
        }
        self.partials.truncate(i);
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
                    self.inf_sum += xsave;
                }
                self.special_sum += xsave;
                self.partials.clear();
            } else {
                self.partials.push(x);
            }
        }
        Ok(())
    }

    /// Round the accumulated expansion exactly once, matching CPython's
    /// half-even fix-up across the remaining partials.
    pub(super) fn finish(self) -> Result<f64> {
        if self.special_sum != 0.0 {
            if self.inf_sum.is_nan() {
                return Err(PyError::named(
                    "ValueError",
                    "-inf + inf in fsum".to_string(),
                ));
            }
            return Ok(self.special_sum);
        }

        // Sum the partials from the top (largest magnitude first), stopping as soon
        // as the running sum becomes inexact, then apply CPython's half-even
        // rounding fix-up across the remaining partials.
        let mut n = self.partials.len();
        let mut hi = 0.0f64;
        let mut lo = 0.0f64;
        if n > 0 {
            n -= 1;
            hi = self.partials[n];
            while n > 0 {
                let x = hi;
                n -= 1;
                let y = self.partials[n];
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
            if n > 0
                && ((lo < 0.0 && self.partials[n - 1] < 0.0)
                    || (lo > 0.0 && self.partials[n - 1] > 0.0))
            {
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
}

impl Default for FsumState {
    fn default() -> Self {
        Self::new()
    }
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

/// Incremental compensated-float accumulator used by `math.sumprod`.
///
/// The caller owns CPython's exact-int/float/generic mode transitions. This
/// numerical kernel only receives primitive pairs and therefore remains
/// independent of iteration and Python operator dispatch.
pub(super) struct SumProdFloatState {
    total: TripleLength,
}

impl SumProdFloatState {
    pub(super) fn new() -> Self {
        Self { total: TL_ZERO }
    }

    /// Try to add one already-coerced product to this contiguous float run.
    ///
    /// A non-finite high component ends CPython's speculative float path. The
    /// caller then flushes the accepted prefix and replays this pair through
    /// ordinary Python arithmetic.
    pub(super) fn try_add(&mut self, a: f64, b: f64) -> bool {
        let new_total = tl_fma(a, b, self.total);
        if new_total.hi.is_finite() {
            self.total = new_total;
            true
        } else {
            false
        }
    }

    pub(super) fn finish(&self) -> f64 {
        tl_to_d(self.total)
    }
}

impl Default for SumProdFloatState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{FsumState, SumProdFloatState};

    #[test]
    fn incremental_fsum_keeps_cancellation_precision() {
        let mut state = FsumState::new();
        for value in [1.0e16, 1.0, -1.0e16] {
            state.add(value).expect("finite term");
        }
        assert_eq!(state.finish().expect("finite total"), 1.0);
    }

    #[test]
    fn incremental_fsum_rejects_opposite_infinities() {
        let mut state = FsumState::new();
        state.add(f64::INFINITY).expect("infinity is deferred");
        state
            .add(f64::NEG_INFINITY)
            .expect("opposite infinity is deferred");
        assert!(state.finish().is_err());
    }

    #[test]
    fn incremental_sumprod_keeps_product_roundoff() {
        let mut state = SumProdFloatState::new();
        assert!(state.try_add(1.0e16, 1.0));
        assert!(state.try_add(1.0, 1.0));
        assert!(state.try_add(-1.0e16, 1.0));
        assert_eq!(state.finish(), 1.0);
    }

    #[test]
    fn incremental_sumprod_rejects_overflowing_term() {
        let mut state = SumProdFloatState::new();
        assert!(!state.try_add(1.0e308, 2.0));
        assert_eq!(state.finish(), 0.0);
    }
}
