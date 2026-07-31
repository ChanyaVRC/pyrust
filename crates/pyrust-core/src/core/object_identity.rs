use std::cell::Cell;

use num_bigint::BigInt;

// Monotonic counter for list/tuple object identity. Each new allocation gets a
// unique id stored at hdr+24; clones copy the same id so `id(x) == id(y)` when
// y is a copy of x, and `id([1]) != id([2])` because they are separate objects.
thread_local! {
    static OBJ_ID_COUNTER: Cell<u64> = const { Cell::new(1) };
}

pub(crate) fn next_obj_id() -> u64 {
    OBJ_ID_COUNTER.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}

/// Object id for a `float`, derived from its NaN-box bit pattern.
///
/// A float's box is the double itself, so its identity consumes all 64 bits;
/// no fixed-width permutation can separate that full domain from another
/// kind's ids.  Put it in the next 64-bit-wide Python-integer namespace
/// instead:
///
/// ```text
/// ordinary ids: [0,    2^64)
/// float ids:    [2^64, 2^65)
/// ```
///
/// Adding the namespace base is reversible, so two floats have the same id
/// exactly when their NaN-box bits — the bits `is` compares — are equal.
pub(crate) fn float_obj_id(bits: u64) -> BigInt {
    (BigInt::from(1_u8) << 64) + BigInt::from(bits)
}

/// Exact object id for a `complex`'s two component bit patterns.
///
/// `complex` is the one heap-allocated value whose identity is *not* its
/// allocation: `is` compares the (real, imaginary) bit patterns (#2949), so
/// `id()` has to be a function of exactly those bits (#2956).
///
/// Concatenating the two 64-bit components is injective.  Prefix that 128-bit
/// payload with a bit outside it, placing every complex id in `[2^128, 2^129)`
/// and therefore outside both the ordinary and float namespaces.
pub(crate) fn complex_obj_id(re: f64, im: f64) -> BigInt {
    (BigInt::from(1_u8) << 128) + (BigInt::from(re.to_bits()) << 64) + BigInt::from(im.to_bits())
}

#[cfg(test)]
mod tests {
    use super::{complex_obj_id, float_obj_id};
    use num_bigint::BigInt;

    #[test]
    fn exact_numeric_identity_namespaces_are_disjoint() {
        let ordinary_max = BigInt::from(u64::MAX);
        let float_min = float_obj_id(0);
        let float_max = float_obj_id(u64::MAX);
        let complex_min = complex_obj_id(0.0, 0.0);

        assert!(ordinary_max < float_min);
        assert!(float_min < float_max);
        assert!(float_max < complex_min);
    }
}
