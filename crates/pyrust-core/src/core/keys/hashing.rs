// ─────────────────────────────────────────────────────────────────────────────
// Cross-type numeric key helpers & CPython-compatible hashing
//
// CPython guarantees: `hash(x) == hash(y)` whenever `x == y`, even across
// numeric types.  In particular `hash(1.0) == hash(1)` and `1.0 == 1` for
// dict/set purposes.  The helpers below implement the required logic without
// depending on the `pyrust` interpreter crate (which would create a cycle).
// ─────────────────────────────────────────────────────────────────────────────

/// If `bits` (an `f64` stored as its IEEE-754 bit pattern) represents a
/// finite, integer-valued float whose magnitude fits in an `i64`, return
/// `Some(i as i64)`.  Otherwise return `None`.
///
/// Safe conversion range: `[i64::MIN, 2^63)`.  `i64::MIN as f64` is exactly
/// representable; `i64::MAX as f64` rounds up to `2^63`, so the upper bound
/// is strictly less than `-(i64::MIN as f64)`.
#[inline]
fn float_bits_as_exact_i64(bits: u64) -> Option<i64> {
    let f = f64::from_bits(bits);
    if !f.is_finite() || f.fract() != 0.0 {
        return None;
    }
    let min_f = i64::MIN as f64; // exact: -9223372036854775808.0
    let max_exclusive = -min_f; // 9223372036854775808.0 = 2^63
    if f >= min_f && f < max_exclusive {
        Some(f as i64)
    } else {
        None
    }
}

/// Exact equality between an IEEE-754 float bit pattern and an `i64`.
#[inline]
fn float_bits_eq_i64(bits: u64, i: i64) -> bool {
    float_bits_as_exact_i64(bits).is_some_and(|fi| fi == i)
}

/// Complex component equality with raw bit-equality fallback so NaN-bearing
/// complex keys find *themselves* in dicts/sets (`{z: 1}[z]` / `z in {z}`,
/// mirroring CPython's `a is b` short-circuit in `PyObject_RichCompareBool`,
/// #2535).
#[inline]
fn complex_key_components_eq(ar: f64, ai: f64, br: f64, bi: f64) -> bool {
    (ar == br || ar.to_bits() == br.to_bits()) && (ai == bi || ai.to_bits() == bi.to_bits())
}

/// CPython's `Py_HASH_MODULUS = 2^61 - 1` (Mersenne prime).
///
/// Duplicated from `interpreter::helpers::PY_HASH_MODULUS` — pyrust-core
/// cannot depend on the interpreter crate (that would create a cycle), so we
/// keep a private copy here for `PyKey` hashing.
const PY_HASH_MODULUS_BIGINT: i64 = (1i64 << 61) - 1;

#[inline]
fn py_hash_i64_raw(v: i64) -> i64 {
    let raw = v % PY_HASH_MODULUS_BIGINT;
    if raw == -1 { -2 } else { raw }
}

/// Hash a `BigInt` key using CPython's Mersenne-prime scheme so that
/// `PyKey::BigInt(n)` hashes identically to `hash(n)` and equal to the
/// corresponding `PyKey::Float` when `bigint_float_eq(n, f)` holds.
///
/// The remainder `n % modulus` is in `[-(modulus-1), modulus-1]`, which is
/// strictly within `i64` range (`modulus = 2^61 - 1 < i64::MAX`), so
/// `to_i64()` is always `Some`.  We use `expect` rather than `unwrap_or(0)`
/// so that any future logic error surfaces immediately instead of silently
/// producing a wrong hash.
#[inline]
fn pykey_hash_bigint(n: &BigInt) -> i64 {
    let modulus = BigInt::from(PY_HASH_MODULUS_BIGINT);
    let reduced = n % &modulus;
    let raw = reduced.to_i64().expect("n % (2^61-1) always fits in i64");
    if raw == -1 { -2 } else { raw }
}

/// Hash a NaN from the stable object identity carried in its IEEE-754 bits.
///
/// CPython hashes NaN with `_Py_HashPointer`: rotate the object's identity
/// word right by four bits, then remap the reserved `-1` result to `-2`.
/// pyrust's floats are immediate values rather than heap allocations, but each
/// boxed NaN carries a unique, stable identity in its payload (#2911).  Treating
/// the complete bit pattern as the identity word gives the same observable
/// contract: aliases hash stably while simultaneously live NaNs hash apart.
#[cold]
#[inline(never)]
pub fn py_hash_nan(value: f64) -> i64 {
    debug_assert!(value.is_nan());
    let hash = value.to_bits().rotate_right(4) as i64;
    if hash == -1 { -2 } else { hash }
}

/// Hash a single IEEE-754 `f64` using CPython's Mersenne-prime float algorithm.
///
/// Shared by `py_hash_complex` and `py_hash_pykey`'s `Float` arm so that
/// `hash(1.0)`, `hash(1+0j)`, and `hash(frozenset({x}))` cannot drift.
#[inline]
fn py_hash_float_bits(raw_bits: u64) -> i64 {
    const P: u64 = (1u64 << 61) - 1;
    let f = f64::from_bits(raw_bits);
    if f.is_nan() {
        return py_hash_nan(f);
    }
    if f.is_infinite() {
        return if f > 0.0 { 314159 } else { -314159 };
    }
    if f == 0.0 {
        return 0;
    }
    if let Some(i) = float_bits_as_exact_i64(raw_bits) {
        return py_hash_i64_raw(i);
    }
    let sign: i64 = if f < 0.0 { -1 } else { 1 };
    let biased_exp = ((raw_bits >> 52) & 0x7ff) as i64;
    let mantissa_bits = raw_bits & 0x000f_ffff_ffff_ffff;
    let (m, e): (u64, i64) = if biased_exp == 0 {
        (mantissa_bits, -1074)
    } else {
        (mantissa_bits | (1u64 << 52), biased_exp - 1023 - 52)
    };
    let h: u64 = if e >= 0 {
        let shift = (e as u64) % 61;
        ((m as u128 * (1u128 << shift)) % (P as u128)) as u64
    } else {
        let neg_e_mod = ((-e) as u64) % 61;
        let inv_shift = if neg_e_mod == 0 { 0u64 } else { 61 - neg_e_mod };
        ((m as u128 * (1u128 << inv_shift)) % (P as u128)) as u64
    };
    let signed = h as i64 * sign;
    if signed == -1 { -2 } else { signed }
}

/// Compute the CPython-compatible hash for a complex number.
///
/// Mirrors `complexobject.c` in CPython 3.12:
///   hash_real = _Py_HashDouble(re)  (as Py_uhash_t / u64)
///   hash_imag = _Py_HashDouble(im)  (as Py_uhash_t / u64)
///   combined  = hash_real + _Py_HASH_IMAG * hash_imag  (wrapping u64)
///   result    = combined as i64; if -1 return -2
///
/// The individual float hashes are reduced mod 2^61-1 by the CPython float
/// hash algorithm; wrapping u64 overflow on the combined sum matches C
/// behaviour (no additional modulo is applied to the sum).
fn py_hash_complex(re: f64, im: f64) -> i64 {
    const HASH_IMAG: u64 = 1000003;
    let hash_re = py_hash_float_bits(re.to_bits()) as u64;
    let hash_im = py_hash_float_bits(im.to_bits()) as u64;
    let combined = hash_re.wrapping_add(HASH_IMAG.wrapping_mul(hash_im));
    let result = combined as i64;
    if result == -1 { -2 } else { result }
}

/// Feed an `i64` (used by `Int`, `Bool`, and in-range `Float`/`BigInt` keys)
/// into the `IndexMap`/`IndexSet` bucket hasher.
#[inline]
fn hash_i64_key<H: Hasher>(value: i64, state: &mut H) {
    0u8.hash(state);
    value.hash(state);
}

/// Feed a `BigInt` into the `IndexMap`/`IndexSet` bucket hasher, unifying with
/// `Int` when in range and with out-of-range integer `Float` otherwise.
#[inline]
fn hash_bigint_key<H: Hasher>(n: &BigInt, state: &mut H) {
    if let Some(i) = n.to_i64() {
        hash_i64_key(i, state);
    } else {
        1u8.hash(state);
        pykey_hash_bigint(n).hash(state);
    }
}

/// Feed the IEEE-754 bit pattern `bits` into `state` using the bucket scheme
/// that `PyKey::Float` relies on.
///
/// Shared by `PyKey::Float`, `ValueKeyRef` float, and real-valued `Complex`
/// hashing: equal numeric keys must produce identical hasher input.
///
/// - Integer-valued and within `i64`: tag 0 + the `i64`, identical to
///   `PyKey::Int` / `PyKey::Bool`.
/// - Integer-valued beyond `i64`: tag 1 + Mersenne-prime reduction, identical
///   to the `PyKey::BigInt` out-of-range arm (so `1e20` and `10**20` collide).
/// - Fractional or non-finite: tag 1 + the raw bits.
#[inline]
fn hash_float_bits<H: Hasher>(bits: u64, state: &mut H) {
    if let Some(i) = float_bits_as_exact_i64(bits) {
        hash_i64_key(i, state);
        return;
    }
    let f = f64::from_bits(bits);
    1u8.hash(state);
    if f.is_finite()
        && f.fract() == 0.0
        && let Some(big) = BigInt::from_f64(f)
    {
        pykey_hash_bigint(&big).hash(state);
        return;
    }
    bits.hash(state);
}

/// Hash a complex key, mirroring [`real_valued_complex_eq`]'s notion of "real".
///
/// A real-valued complex delegates to the float hasher so `1+0j` and `1.0`
/// share a bucket; anything else uses the CPython complex formula so that this
/// impl and [`py_hash_pykey`] agree.  Cold and out-of-line because complex
/// dict keys are rare while `Hash::hash` is inlined into hot int/str probes.
#[cold]
#[inline(never)]
fn hash_complex_key<H: Hasher>(re: f64, im: f64, state: &mut H) {
    if im == 0.0 {
        hash_float_bits(re.to_bits(), state);
    } else {
        (py_hash_complex(re, im) as u64).hash(state);
    }
}

#[inline]
fn hash_sentinel_addr(sentinel: &'static u8) -> i64 {
    let addr = (sentinel as *const u8 as usize) >> 4;
    let h = (addr as i64).wrapping_rem(PY_HASH_MODULUS_BIGINT);
    if h == 0 || h == -1 { 2654435761 } else { h }
}

/// Returns the canonical Python-level hash of `None`.
///
/// CPython's `hash(None)` returns `_Py_HashPointer(Py_None)` — a stable
/// per-process value derived from `None`'s memory address.  pyrust's `None`
/// is NaN-boxed (no heap pointer), so we derive a stable per-process value
/// from a single `static` sentinel whose address is stable for the lifetime
/// of this process (fixed at process launch; may differ across runs with ASLR).
///
/// **All callers must use this function** — do NOT define a separate
/// `static NONE_SENTINEL` in another crate; two statics compile to two
/// distinct addresses, violating the invariant that `hash(None)` equals the
/// hash used for `None` in any container.
pub fn py_hash_none() -> i64 {
    static NONE_SENTINEL: u8 = 0;
    hash_sentinel_addr(&NONE_SENTINEL)
}

/// Compute a stable per-process hash for `Ellipsis`, matching CPython's
/// approach of using the object's identity address.
pub fn py_hash_ellipsis() -> i64 {
    static ELLIPSIS_SENTINEL: u8 = 0;
    hash_sentinel_addr(&ELLIPSIS_SENTINEL)
}

/// Compute a stable per-process hash for `NotImplemented`, matching CPython's
/// approach of using the object's identity address.
pub fn py_hash_not_implemented() -> i64 {
    static NOT_IMPLEMENTED_SENTINEL: u8 = 0;
    hash_sentinel_addr(&NOT_IMPLEMENTED_SENTINEL)
}

#[inline]
fn frozenset_shuffle_bits(h: u64) -> u64 {
    let shuffled = (h ^ 89869747u64) ^ (h << 16);
    shuffled.wrapping_mul(3644798167u64)
}

fn py_hash_frozenset_items(items: &PySet) -> i64 {
    let mut h: u64 = 0;
    for item in items {
        h ^= frozenset_shuffle_bits(py_hash_pykey(item) as u64);
    }
    let n = items.len() as u64;
    h ^= (n + 1).wrapping_mul(1927868237u64);
    h ^= (h >> 11) ^ (h >> 25);
    h = h.wrapping_mul(69069u64).wrapping_add(907133923u64);
    let result = h as i64;
    if result == -1 { 590923713 } else { result }
}

#[inline]
fn fnv1a_bytes(bytes: &[u8]) -> u64 {
    let mut h: u64 = 14695981039346656037u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211u64);
    }
    h
}

/// Compute the CPython-compatible hash for a `PyKey`.
///
/// This replicates the hash semantics that CPython applies at the C level for
/// each primitive type.  It is used by `FrozenSetOps::hash` so that
/// `hash(frozenset({1, 2}))` in pyrust produces the same value as CPython.
///
/// - `Int` / `Bool`: Mersenne-prime reduction (`v % (2^61-1)`), -1 mapped to -2.
/// - `Float`: integer-valued floats hash identically to the corresponding `Int`.
/// - `Str`: FNV-1a (matches pyrust's `hash_value` str arm; CPython uses SipHash
///   with a random seed so str-element frozenset hashes differ from CPython).
/// - `None`: stable per-process value via [`py_hash_none`].
/// - `Tuple`: CPython 3.12 xxHash-based tuplehash (Python 3.8+ algorithm):
///   `acc = PRIME5; for each item: acc = rotl31(acc + hash*PRIME2)*PRIME1; acc += n^(PRIME5^3527539)`.
/// - `FrozenSet`: CPython's XOR-shuffle accumulation with length mixing and
///   final scramble (Objects/setobject.c `frozenset_hash`).
/// - `Object { hash, .. }`: the precomputed hash cast to `i64`.
pub fn py_hash_pykey(key: &PyKey) -> i64 {
    match key {
        PyKey::Int(v) => py_hash_i64_raw(*v),
        PyKey::Bool(b) => *b as i64,
        PyKey::BigInt(n) => pykey_hash_bigint(n),
        PyKey::Float(bits) => py_hash_float_bits(*bits),
        PyKey::Str(s) => fnv1a_bytes(s.as_str().unwrap_or("").as_bytes()) as i64,
        PyKey::None => py_hash_none(),
        PyKey::Ellipsis => py_hash_ellipsis(),
        PyKey::Tuple(items) => {
            // CPython 3.12 xxHash-based tuplehash (Python 3.8+ algorithm).
            const PRIME1: u64 = 11400714785074694791;
            const PRIME2: u64 = 14029467366897019727;
            const PRIME5: u64 = 2870177450012600261;

            // Shared per-element accumulation step; mirrors the xxstep helper
            // in pyrust's tuple_hash_cpython / slice_hash_cpython.
            #[inline(always)]
            fn xxstep(acc: u64, lane: u64) -> u64 {
                let acc = acc.wrapping_add(lane.wrapping_mul(PRIME2));
                let acc = acc.rotate_left(31); // rotl31
                acc.wrapping_mul(PRIME1)
            }

            let mut acc: u64 = PRIME5;
            let mut n: u64 = 0;
            for item in items.iter() {
                acc = xxstep(acc, py_hash_pykey(item) as u64);
                n += 1;
            }
            acc = acc.wrapping_add(n ^ (PRIME5 ^ 3527539u64));
            if acc == u64::MAX {
                acc = 1546275796;
            }
            acc as i64
        }
        PyKey::FrozenSet(key) => key.py_hash(),
        PyKey::Bytes(b) => {
            // FNV-1a over the raw byte content — matches the hash_value Bytes
            // arm in builtins.rs so that py_hash_pykey(key) == hash(value).
            let result = fnv1a_bytes(b.as_ref()) as i64;
            if result == -1 { -2 } else { result }
        }
        // A real-valued complex must report the *float* hash: CPython's
        // `hash(1+0j) == hash(1.0) == hash(1)`.  Delegating (rather than
        // relying on `py_hash_complex(re, 0.0)` reducing to the same number)
        // makes the invariant structural (#2900).
        PyKey::Complex(re, im) if *im == 0.0 => py_hash_float_bits(re.to_bits()),
        PyKey::Complex(re, im) => py_hash_complex(*re, *im),
        PyKey::Object { hash, .. } => *hash as i64,
    }
}
