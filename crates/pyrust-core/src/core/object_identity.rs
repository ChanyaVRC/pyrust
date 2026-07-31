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

/// Golden-ratio constant, so no bit pattern folds to a trivial id: the
/// finaliser below maps 0 to 0, and an unseeded fold would hand `0j` the id
/// `0` — which `0.0`, whose box bits really are zero, already owns.
const ID_SEED: u64 = 0x9e37_79b9_7f4a_7c15;

/// murmur3's 64-bit finaliser.  A bijection on `u64`, so it re-labels an id
/// space without ever merging two distinct inputs.
fn fmix64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^= x >> 33;
    x
}

/// Object id for a `float`, derived from its NaN-box bit pattern.
///
/// The bits cannot be used raw.  Every other kind carries a tag in the top 16
/// bits, but a float's box *is* the double, so the float id space is the
/// entire 64-bit word and overlaps every other kind's — and it overlaps them
/// in a structured, constructible way.  Heap ids are small: the monotonic
/// list/tuple/set counter starts at 1, and an `Rc` address fits in 48 bits.
/// Every such pattern is a subnormal double and `5e-324 * n` is exactly the
/// subnormal whose bits are `n`, so raw bits made
///
/// ```text
/// id(5e-324) == id((1, 2, 3))          # the first tuple allocated
/// d = {}; id(5e-324 * id(d)) == id(d)  # any heap object at all
/// ```
///
/// both `True` while `is` said `False` (#2956 review).
///
/// The finaliser is a bijection, so distinct floats keep distinct ids — `is`
/// compares exactly these bits — while the id lands anywhere in the word, so
/// colliding with another kind now means inverting murmur3 rather than
/// multiplying out a subnormal.
pub(crate) fn float_obj_id(bits: u64) -> u64 {
    fmix64(bits ^ ID_SEED)
}

/// Fold a `complex`'s two component bit patterns into a single object id.
///
/// `complex` is the one heap-allocated value whose identity is *not* its
/// allocation: `is` compares the (real, imaginary) bit patterns (#2949), so
/// `id()` has to be a function of exactly those bits (#2956).
///
/// Both components run through the murmur3 finaliser, so the fold is
/// injective in each component taken separately; only the unavoidable
/// 128 -> 64 narrowing can collide.
pub(crate) fn complex_obj_id(re: f64, im: f64) -> u64 {
    fmix64(fmix64(re.to_bits() ^ ID_SEED) ^ im.to_bits())
}
