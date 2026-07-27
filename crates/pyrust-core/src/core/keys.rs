// ─────────────────────────────────────────────────────────────────────────────
// PyKey — hashable subset of Value used as dict/set keys (unchanged)
// ─────────────────────────────────────────────────────────────────────────────

/// Shared immutable backing for [`PyKey::FrozenSet`].
///
/// A frozenset value and every key derived from it share the same `IndexSet`;
/// the order-independent CPython hash is computed once at construction. This
/// makes repeated dict/set probes O(1) instead of rebuilding, sorting, and
/// hashing every element for each lookup.
#[derive(Debug)]
pub struct FrozenSetKey {
    items: Rc<PySet>,
    py_hash: i64,
}

impl FrozenSetKey {
    pub fn new(items: Rc<PySet>) -> Self {
        let py_hash = py_hash_frozenset_items(&items);
        Self { items, py_hash }
    }

    #[inline]
    pub fn items(&self) -> &PySet {
        &self.items
    }

    #[inline]
    pub fn items_rc(&self) -> Rc<PySet> {
        Rc::clone(&self.items)
    }

    #[inline]
    pub fn py_hash(&self) -> i64 {
        self.py_hash
    }
}

#[derive(Debug, Clone)]
pub enum PyKey {
    Int(i64),
    /// Integer-valued `BigInt` key for values beyond `i64` range.  Produced by
    /// `Value::to_key` when the BigInt does not fit in an `i64`.  Must hash and
    /// compare equal to the corresponding `Float` key when the float is
    /// integer-valued and exact (CPython: `hash(10**20) == hash(1e20)` and
    /// `{1e20: 'a', 10**20: 'b'}` has length 1).
    ///
    /// Uses `Box` rather than `Rc` so that `PyKey` stays `Send + Sync`
    /// and to make the recursive type representable with a fixed-size enum.
    BigInt(Box<BigInt>),
    Float(u64),
    /// A string key.  Holds a `Value` (O(1) RC bump clone) rather than a
    /// `String` (heap alloc + memcpy) so that `Value::to_key()` on the hot
    /// path incurs zero allocation.  Use `PyKey::str_from(s)` to construct
    /// from a bare `&str`/`String`.
    Str(Value),
    Bool(bool),
    None,
    Ellipsis,
    /// Hashable frozenset key. Shares the immutable backing set and its cached
    /// order-independent hash across value→key conversions and probes.
    FrozenSet(Rc<FrozenSetKey>),
    /// Hashable tuple key.  Stores each element as its own `PyKey` so
    /// hashing and equality propagate element-wise (matching CPython's
    /// `hash((1, 2))`).  Unlike `FrozenSet`, ordering is significant —
    /// `(1, 2) != (2, 1)` — so we preserve insertion order.
    Tuple(Vec<PyKey>),
    /// Bytes key.  `bytes` objects are immutable and therefore hashable in
    /// CPython.  Holds the backing `Rc<Vec<u8>>` directly to avoid
    /// re-allocation on the hot dict/set lookup path.
    Bytes(Rc<Vec<u8>>),
    /// Complex key with non-zero imaginary part.  Complex values with zero
    /// imaginary part are stored as `PyKey::Float(re.to_bits())` so that
    /// cross-type equality (`1+0j == 1 == 1.0`) works via the existing
    /// `Float <-> Int` arms without additional special-casing.
    Complex(f64, f64),
    /// Key constructed for a user-defined `PyInstance` (or any non-builtin
    /// Value that defines `__hash__`).  The `hash` is precomputed by the
    /// caller — pyrust-core has no interpreter reference, so dispatching
    /// `__hash__` happens in the runtime layer before constructing the key.
    ///
    /// `PartialEq` for `Object` uses `Value::eq`, which for a `PyInstance`
    /// performs `Rc::ptr_eq`.  Full `__eq__` semantics (so distinct instances
    /// that compare equal collapse to one entry) require runtime cooperation:
    /// the dict/set helpers in the runtime perform a linear scan dispatching
    /// `__eq__` via the interpreter when the lookup key is `PyKey::Object`.
    Object {
        hash: u64,
        value: Value,
    },
}

impl PyKey {
    /// Construct a string key from a bare `&str` / `String` (or anything
    /// implementing `AsRef<str>`).  Allocates a fresh `Value::string` once;
    /// subsequent `.clone()` calls on the returned `PyKey` are O(1) RC bumps.
    #[inline]
    pub fn str_from(s: impl AsRef<str>) -> PyKey {
        PyKey::Str(Value::string(s.as_ref()))
    }
}

// ── PyKey cross-type numeric helpers ─────────────────────────────────────────
//
// CPython guarantees: `hash(x) == hash(y)` whenever `x == y`, even across
// numeric types.  In particular `hash(1.0) == hash(1)` and `1.0 == 1` for
// dict/set purposes.  The helpers below implement the required logic without
// depending on the `pyrust` interpreter crate (which would create a cycle).

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

/// CPython's `Py_HASH_MODULUS = 2^61 - 1` (Mersenne prime).
///
/// Duplicated from `interpreter::helpers::PY_HASH_MODULUS` — pyrust-core
/// cannot depend on the interpreter crate (that would create a cycle), so we
/// keep a private copy here for `PyKey::BigInt` hashing.
const PY_HASH_MODULUS_BIGINT: i64 = (1i64 << 61) - 1;

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

    // Hash a single float component using CPython's Mersenne-prime scheme,
    // returning the result reinterpreted as u64.
    // Mirrors interpreter::helpers::py_hash_float and the Float arm of
    // py_hash_pykey so that hash(1.0) and hash(1+0j) agree.
    #[inline]
    fn hash_float_as_u64(v: f64) -> u64 {
        const P: u64 = (1u64 << 61) - 1;
        if v.is_nan() {
            return 0;
        }
        if v.is_infinite() {
            return if v > 0.0 {
                314159u64
            } else {
                (-314159i64) as u64
            };
        }
        if v == 0.0 {
            return 0;
        }
        let bits = v.to_bits();
        let sign: i64 = if v < 0.0 { -1 } else { 1 };
        let biased_exp = ((bits >> 52) & 0x7ff) as i64;
        let mantissa_bits = bits & 0x000f_ffff_ffff_ffff;
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
        if signed == -1 {
            (-2i64) as u64
        } else {
            signed as u64
        }
    }

    let hash_re = hash_float_as_u64(re);
    let hash_im = hash_float_as_u64(im);
    let combined = hash_re.wrapping_add(HASH_IMAG.wrapping_mul(hash_im));
    let result = combined as i64;
    if result == -1 { -2 } else { result }
}

impl PartialEq for PyKey {
    fn eq(&self, other: &Self) -> bool {
        // In CPython, `True == 1` and `False == 0`, and they hash equal, so
        // dict/set treat them as the same key.  We preserve the original key
        // (e.g. `{True: 1, 1: 2}` keeps `True` as the visible key) by storing
        // `PyKey::Bool` distinctly, but we make `Bool(b) == Int(b as i64)` so
        // lookups across the two types succeed.
        //
        // CPython also guarantees `1.0 == 1` for dict/set purposes, so
        // `Float` and `Int` (or `Bool`) must compare equal when the float is
        // integer-valued and representable as an `i64`.
        //
        // Two `Object` keys compare equal only when the underlying value
        // identity matches.  This is intentionally strict: the dict/set
        // runtime layer dispatches user-defined `__eq__` separately when the
        // precomputed hashes coincide so that distinct instances which the
        // user considers equal still collapse.
        match (self, other) {
            (PyKey::Int(a), PyKey::Int(b)) => a == b,
            (PyKey::Bool(a), PyKey::Bool(b)) => a == b,
            (PyKey::Bool(a), PyKey::Int(b)) | (PyKey::Int(b), PyKey::Bool(a)) => *b == *a as i64,
            // Bit-identical floats are equal first (`a == b`): this is the
            // dict/set counterpart of `Value::is_identical_nan` — CPython's
            // `PyObject_RichCompareBool` short-circuits on `a is b` before
            // `__eq__`, so a NaN key finds *itself* (`{n: 1}[n]`, `n in {n}`)
            // even though `nan == nan` is False.  Non-NaN floats fall through
            // to the usual value compare unchanged.
            (PyKey::Float(a), PyKey::Float(b)) => {
                a == b || f64::from_bits(*a) == f64::from_bits(*b)
            }
            // Cross-type: Float vs Int (and Bool, since Bool is a subtype of int).
            // A float equals an int key iff the float is finite, has no
            // fractional part, and its value equals the integer exactly.
            (PyKey::Float(bits), PyKey::Int(i)) | (PyKey::Int(i), PyKey::Float(bits)) => {
                float_bits_as_exact_i64(*bits).is_some_and(|fi| fi == *i)
            }
            (PyKey::Float(bits), PyKey::Bool(b)) | (PyKey::Bool(b), PyKey::Float(bits)) => {
                float_bits_as_exact_i64(*bits).is_some_and(|fi| fi == *b as i64)
            }
            // Cross-type: BigInt vs Int.  By construction `Value::to_key` only
            // produces `PyKey::BigInt` for values that don't fit in i64, so
            // this arm should never fire in practice; it is here for
            // completeness.
            (PyKey::BigInt(a), PyKey::Int(b)) | (PyKey::Int(b), PyKey::BigInt(a)) => {
                *a.as_ref() == BigInt::from(*b)
            }
            // Cross-type: BigInt vs BigInt.
            (PyKey::BigInt(a), PyKey::BigInt(b)) => a == b,
            // Cross-type: Float vs BigInt.  Uses `bigint_float_eq` which
            // guards that the float is finite and integer-valued before
            // converting to BigInt; non-finite and fractional floats always
            // return false, matching CPython.
            (PyKey::Float(bits), PyKey::BigInt(n)) | (PyKey::BigInt(n), PyKey::Float(bits)) => {
                bigint_float_eq(n, f64::from_bits(*bits))
            }
            (PyKey::Str(a), PyKey::Str(b)) => a.as_str() == b.as_str(),
            (PyKey::None, PyKey::None) => true,
            (PyKey::Ellipsis, PyKey::Ellipsis) => true,
            (PyKey::FrozenSet(a), PyKey::FrozenSet(b)) => {
                Rc::ptr_eq(a, b) || a.items() == b.items()
            }
            (PyKey::Tuple(a), PyKey::Tuple(b)) => a == b,
            (PyKey::Bytes(a), PyKey::Bytes(b)) => a.as_ref() == b.as_ref(),
            // Complex equality: two complex keys are equal iff both components match.
            // -0.0 == 0.0 in IEEE 754, which matches CPython's `==` for complex.
            // The bit-equality fallback (mirroring the `PyKey::Float` arm) lets a
            // NaN-bearing complex key find *itself* — CPython's
            // `PyObject_RichCompareBool` short-circuits on `a is b` before `__eq__`,
            // so `{z: 1}[z]` / `z in {z}` work even though `nan != nan`.  Components
            // are `f64`, so the fallback compares raw bit patterns (#2535).
            (PyKey::Complex(ar, ai), PyKey::Complex(br, bi)) => {
                (ar == br || ar.to_bits() == br.to_bits())
                    && (ai == bi || ai.to_bits() == bi.to_bits())
            }
            (PyKey::Object { value: a, .. }, PyKey::Object { value: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl Eq for PyKey {}

impl Hash for PyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // NOTE: `Bool(b)` must hash equal to `Int(b as i64)` so that the two
        // collide in dict/set buckets and PartialEq can deduplicate them.
        // We therefore deliberately omit `std::mem::discriminant` here and
        // hash a per-variant tag instead, treating Bool as Int.
        //
        // `Float` must also hash equal to `Int` when they are equal (CPython
        // invariant: `hash(1.0) == hash(1)`).  Integer-valued floats use tag 0
        // and the same `i64` value as the equivalent `Int`, satisfying the
        // `Hash + PartialEq` contract.  Fractional or non-finite floats use
        // tag 1, keeping them in a separate hash space.
        //
        // `BigInt` that fits in i64 uses tag 0 (same as Int/Bool) so that
        // `BigInt(n) == Int(n)` implies identical hashes — required by the
        // Hash+Eq contract.  BigInt values beyond i64 range use tag 1 +
        // Mersenne-prime reduction, matching the large-integer-valued Float
        // path.
        match self {
            PyKey::Int(v) => {
                0u8.hash(state);
                v.hash(state);
            }
            PyKey::Bool(b) => {
                0u8.hash(state);
                (*b as i64).hash(state);
            }
            PyKey::Float(bits) => {
                if let Some(i) = float_bits_as_exact_i64(*bits) {
                    // Integer-valued float in i64 range: hash identically to
                    // PyKey::Int(i) so that equal keys land in the same bucket.
                    0u8.hash(state);
                    i.hash(state);
                } else {
                    // Float beyond i64 range (or fractional, or non-finite).
                    // For integer-valued floats beyond i64 range, CPython's
                    // `hash(float)` uses the Mersenne-prime reduction, matching
                    // `hash(int)` for the same value.  We reproduce that here so
                    // `PyKey::Float(1e20)` and `PyKey::BigInt(10**20)` collide.
                    let f = f64::from_bits(*bits);
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
            }
            PyKey::BigInt(n) => {
                // BigInt that fits in i64 must hash identically to PyKey::Int
                // with the same value, because PartialEq makes them equal (the
                // `BigInt <-> Int` arm).  When it does not fit in i64 we use
                // tag 1 + Mersenne-prime reduction, which matches the Float
                // path for the same large integer-valued float.
                if let Some(i) = n.to_i64() {
                    0u8.hash(state);
                    i.hash(state);
                } else {
                    1u8.hash(state);
                    pykey_hash_bigint(n).hash(state);
                }
            }
            PyKey::Str(s) => {
                2u8.hash(state);
                s.as_str().unwrap_or("").hash(state);
            }
            PyKey::None => {
                // Hash the Python-level value so that a PyKey::Object whose
                // __hash__ returns hash(None) lands in the same IndexMap
                // bucket, enabling the slow-path __eq__ to deduplicate them
                // (issue #906).
                (py_hash_none() as u64).hash(state);
            }
            PyKey::Ellipsis => {
                (py_hash_ellipsis() as u64).hash(state);
            }
            PyKey::FrozenSet(key) => {
                4u8.hash(state);
                key.py_hash().hash(state);
            }
            PyKey::Tuple(items) => {
                5u8.hash(state);
                items.len().hash(state);
                for k in items {
                    k.hash(state);
                }
            }
            PyKey::Bytes(b) => {
                6u8.hash(state);
                b.as_ref().hash(state);
            }
            // Complex with non-zero imaginary: hash using the CPython formula
            // so that py_hash_pykey(Complex(re, im)) and this Rust Hash impl
            // agree (equal keys must hash equal per the Hash+Eq contract).
            PyKey::Complex(re, im) => {
                (py_hash_complex(*re, *im) as u64).hash(state);
            }
            PyKey::Object { hash, .. } => hash.hash(state),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// StrKey — zero-alloc probe type for `IndexMap<PyKey, …>` string lookups
// ─────────────────────────────────────────────────────────────────────────────

/// A borrowed `&str` that hashes identically to `PyKey::Str` and can be used
/// to probe an `IndexMap<PyKey, V>` without constructing a `PyKey::Str(Value)`.
///
/// `PyKey::Str` hashes as `2u8.hash(state); s.hash(state)`.  `StrKey` applies
/// the same sequence, satisfying the `Equivalent` contract (equal keys must
/// hash equal).
///
/// # Why not `impl Borrow<str> for PyKey`?
///
/// The `Borrow` contract requires `hash(owned) == hash(borrowed)` using the
/// same hasher.  `PyKey::Str` mixes a `2u8` discriminant tag before the string
/// content, so `PyKey::Str("x")` hashes differently than bare `"x"`.  A
/// blanket `Borrow<str>` impl would silently violate the contract.  `StrKey`
/// with an explicit `Hash` impl matching `PyKey::Str` avoids this.
pub struct StrKey<'a>(pub &'a str);

impl Hash for StrKey<'_> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        2u8.hash(state);
        self.0.hash(state);
    }
}

impl indexmap::Equivalent<PyKey> for StrKey<'_> {
    #[inline]
    fn equivalent(&self, key: &PyKey) -> bool {
        matches!(key, PyKey::Str(s) if s.as_str() == Some(self.0))
    }
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
    let addr = (&NONE_SENTINEL as *const u8 as usize) >> 4;
    let h = (addr as i64).wrapping_rem((1i64 << 61) - 1);
    if h == 0 || h == -1 { 2654435761 } else { h }
}

/// Compute a stable per-process hash for `Ellipsis`, matching CPython's
/// approach of using the object's identity address.
pub fn py_hash_ellipsis() -> i64 {
    static ELLIPSIS_SENTINEL: u8 = 0;
    let addr = (&ELLIPSIS_SENTINEL as *const u8 as usize) >> 4;
    let h = (addr as i64).wrapping_rem((1i64 << 61) - 1);
    if h == 0 || h == -1 { 2654435761 } else { h }
}

/// Compute a stable per-process hash for `NotImplemented`, matching CPython's
/// approach of using the object's identity address.
pub fn py_hash_not_implemented() -> i64 {
    static NOT_IMPLEMENTED_SENTINEL: u8 = 0;
    let addr = (&NOT_IMPLEMENTED_SENTINEL as *const u8 as usize) >> 4;
    let h = (addr as i64).wrapping_rem((1i64 << 61) - 1);
    if h == 0 || h == -1 { 2654435761 } else { h }
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
    const PY_HASH_MODULUS: i64 = (1i64 << 61) - 1;

    #[inline]
    fn hash_int(v: i64) -> i64 {
        let raw = v % PY_HASH_MODULUS;
        if raw == -1 { -2 } else { raw }
    }

    match key {
        PyKey::Int(v) => hash_int(*v),
        PyKey::Bool(b) => *b as i64,
        PyKey::BigInt(n) => pykey_hash_bigint(n),
        PyKey::Float(bits) => {
            let f = f64::from_bits(*bits);
            if f.is_nan() {
                0
            } else if f.is_infinite() {
                if f > 0.0 { 314159 } else { -314159 }
            } else if f == 0.0 {
                0
            } else if let Some(i) = float_bits_as_exact_i64(*bits) {
                // Whole-number float: hash like the integer it equals.
                hash_int(i)
            } else {
                // Fractional finite float: CPython Mersenne-prime algorithm.
                // Mirrors interpreter::helpers::py_hash_float exactly so that
                // hash(x) == hash(frozenset({x})) element contribution matches.
                const P: u64 = (1u64 << 61) - 1;
                let raw_bits = *bits;
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
        }
        PyKey::Str(s) => {
            // FNV-1a: matches pyrust's hash_value str arm.
            let mut h: u64 = 14695981039346656037u64;
            for b in s.as_str().unwrap_or("").bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(1099511628211u64);
            }
            h as i64
        }
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
            for item in items {
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
            let mut h: u64 = 14695981039346656037u64;
            for byte in b.as_ref().iter() {
                h ^= *byte as u64;
                h = h.wrapping_mul(1099511628211u64);
            }
            let result = h as i64;
            if result == -1 { -2 } else { result }
        }
        PyKey::Complex(re, im) => py_hash_complex(*re, *im),
        PyKey::Object { hash, .. } => *hash as i64,
    }
}
