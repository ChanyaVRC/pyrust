use std::cell::Cell;

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

/// Fold a `complex`'s two component bit patterns into a single object id.
///
/// `complex` is the one heap-allocated value whose identity is *not* its
/// allocation: `is` compares the (real, imaginary) bit patterns (#2949), so
/// `id()` has to be a function of exactly those bits (#2956).
///
/// Both components run through the murmur3 finaliser, which is a bijection on
/// `u64`, so the fold is injective in each component taken separately; only
/// the unavoidable 128 -> 64 narrowing can collide.
///
/// The seed matters: the finaliser maps 0 to 0, so an unseeded fold would hand
/// `0j` the id `0` — which `0.0`, whose box bits really are zero, already
/// owns.
pub(crate) fn complex_obj_id(re: f64, im: f64) -> u64 {
    /// Golden-ratio constant, so no component pattern folds to a trivial id.
    const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

    fn fmix64(mut x: u64) -> u64 {
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        x ^= x >> 33;
        x
    }
    fmix64(fmix64(re.to_bits() ^ SEED) ^ im.to_bits())
}
