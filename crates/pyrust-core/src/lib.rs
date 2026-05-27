use std::alloc::{Layout, alloc, dealloc};
use std::any::Any;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::{Rc, Weak};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use indexmap::{IndexMap, IndexSet};
use num_bigint::BigInt;
use num_traits::{FromPrimitive, ToPrimitive, Zero};
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

pub use num_bigint::BigInt as PyBigInt;
pub use num_bigint::Sign as PyBigIntSign;
pub use num_traits::Pow as PyPow;
pub use num_traits::ToPrimitive as PyToPrimitive;
pub use num_traits::Zero as PyZero;

static FN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_fn_id() -> u64 {
    FN_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

// Global class-mutation epoch counter.  Bumped on every PyClass attribute write
// or delete, regardless of which class was mutated.  Inline attribute caches
// store the epoch at fill time and re-validate it on each hit; a mismatch means
// some class (possibly an ancestor in the MRO) was mutated since the fill,
// which triggers a cache miss and a slow-path re-fill.
//
// This is the same approach CPython's specialising adaptive interpreter uses to
// invalidate inline caches after class mutations.  The counter wraps on u64
// overflow — a missed invalidation after 2^64 mutations is benign in practice.
thread_local! {
    static CLASS_MUTATION_EPOCH: Cell<u64> = const { Cell::new(0) };
}

/// Bump the global class-mutation epoch.  Call this whenever any `PyClass`
/// attribute is written or deleted so that all attribute caches are invalidated.
pub fn bump_class_epoch() {
    CLASS_MUTATION_EPOCH.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Return the current global class-mutation epoch.
pub fn class_epoch() -> u64 {
    CLASS_MUTATION_EPOCH.with(|c| c.get())
}

// Monotonic counter for list/tuple object identity. Each new allocation gets a
// unique id stored at hdr+24; clones copy the same id so `id(x) == id(y)` when
// y is a copy of x, and `id([1]) != id([2])` because they are separate objects.
thread_local! {
    static OBJ_ID_COUNTER: Cell<u64> = const { Cell::new(1) };
}

fn next_obj_id() -> u64 {
    OBJ_ID_COUNTER.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}

/// Maximum byte length of a string eligible for interning.
///
/// Covers all identifier strings, most dict keys, and all dunder names
/// while excluding long user-visible strings that are unlikely to repeat.
/// Mirrors Lua 5.4's short-string threshold.
const INTERN_MAX_BYTES: usize = 40;

/// Maximum number of entries in the per-thread intern table.
///
/// Caps memory usage for programs that use many unique short strings
/// (e.g. programs that generate lots of distinct short keys).
const INTERN_MAX_ENTRIES: usize = 1024;

thread_local! {
    /// Per-thread cache mapping short string byte slices to their `Value`.
    ///
    /// Holding a strong `Value` reference is safe: `Value` is NaN-boxed, and
    /// string Values carry an Rc-like refcount in their heap header.  The intern
    /// table keeps exactly one extra reference alive per cached string.  Each
    /// `intern_string` call returns a clone of the cached value (a cheap
    /// refcount bump).
    static INTERN: RefCell<HashMap<Box<str>, Value>> = RefCell::new(HashMap::new());
}

/// Return a `Value::string` for `s`, reusing a cached allocation when `s`
/// is short (≤ [`INTERN_MAX_BYTES`] bytes) and the table has not yet hit
/// [`INTERN_MAX_ENTRIES`].
///
/// **Only call this for immutable, constant-pool strings.**  Never intern
/// strings produced by concatenation, `input()`, or user code — those are
/// not reused and polluting the table wastes memory.
///
/// Strings longer than `INTERN_MAX_BYTES` are not interned (identity is not
/// preserved across loads of the same long constant).  Use
/// [`intern_string_value`] when a pre-built `Value::string` is already
/// available to avoid a redundant allocation on the long-string path.
pub fn intern_string(s: &str) -> Value {
    if s.len() > INTERN_MAX_BYTES {
        // Long string: interning would bloat the table without meaningful
        // reuse.  Caller must allocate a fresh Value here.
        return Value::string(s);
    }
    INTERN.with(|cache| {
        let mut map = cache.borrow_mut();
        if let Some(v) = map.get(s) {
            return v.clone();
        }
        let v = Value::string(s);
        if map.len() < INTERN_MAX_ENTRIES {
            map.insert(s.into(), v.clone());
        }
        v
    })
}

/// Like [`intern_string`] but takes a pre-built `Value::string` to avoid
/// a redundant allocation on the long-string fast-exit path.
///
/// - Short strings (≤ [`INTERN_MAX_BYTES`]): looked up / inserted in the
///   intern table; the pre-built value is used as the initial allocation
///   if the string is not yet cached.
/// - Long strings (> `INTERN_MAX_BYTES`): `val` is returned as-is — no
///   new `Value::string` allocation is needed (issue #845).
///
/// **Call site contract**: `val` must already be a `Value::string` whose
/// content equals `s`.  The const-pool `LoadConst` path satisfies this
/// by passing both the borrowed `&str` from `cv.kind()` and `cv` itself.
pub fn intern_string_value(s: &str, val: &Value) -> Value {
    if s.len() > INTERN_MAX_BYTES {
        // Long string: not interned; return a cheap clone of the existing
        // const-pool Value rather than allocating a second copy.
        return val.clone();
    }
    INTERN.with(|cache| {
        let mut map = cache.borrow_mut();
        if let Some(v) = map.get(s) {
            return v.clone();
        }
        // Use the caller's pre-built Value as the canonical copy so the
        // const-pool allocation doubles as the intern-table entry.
        let v = val.clone();
        if map.len() < INTERN_MAX_ENTRIES {
            map.insert(s.into(), v.clone());
        }
        v
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Cycle-detection guards for `repr` and `==`
//
// CPython uses `Py_ReprEnter` / `Py_ReprLeave` (a thread-local set of currently
// being-formatted ids) to recognise self-referential collections.  Without the
// guard a structure like `a = []; a.append(a); repr(a)` recurses until the
// thread blows its stack.
//
// We mirror the same trick for `Value::repr` (issue #364) and for `PartialEq`
// on collection variants (so `a == b` for two distinct cycles terminates and
// returns `True`, matching CPython's "we've already proven the prefix equal"
// semantics).
//
// The sets stay empty on the non-cyclic hot path until we actually recurse
// *into* a collection variant, so a flat `repr([1] * 1000)` pays only a
// single thread-local lookup per recursive level and never inserts.
// ─────────────────────────────────────────────────────────────────────────────

thread_local! {
    /// Stack of `Value::value_id()`s currently in the middle of being formatted
    /// by `Value::repr`.  A second `repr` call for an id already on this stack
    /// short-circuits to the CPython placeholder (`[...]` / `{...}` / `(...)`)
    /// instead of recursing.
    ///
    /// Stored as a `Vec` rather than a `HashSet`: in practice the depth is
    /// shallow (a handful of nested collections) so the linear scan is
    /// faster than a HashSet's hashing on the hot path.  Wrapped in
    /// `RefCell` rather than `Cell` so we can borrow the inner `Vec`
    /// without moving it in and out for every push/pop.
    static REPR_IN_PROGRESS: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };

    /// Stack of ordered pairs `(value_id(a), value_id(b))` currently being
    /// compared by `Value::eq`.  Encountering the same pair again means we've
    /// hit a cycle; we treat the cycle as equal (the recursion bottoms out as
    /// "we've already proven the prefix equal") so the comparison terminates
    /// instead of blowing the stack.
    static EQ_IN_PROGRESS: RefCell<Vec<(i64, i64)>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard for the `repr` cycle-detection stack.  Pushes `id` on
/// construction (caller must have checked it wasn't already present) and pops
/// on drop so an early-return or panic in the recursive body can't poison the
/// stack.
struct ReprGuard;

impl ReprGuard {
    /// Attempts to enter the recursion for `id`.  Returns `Some(guard)` when
    /// the caller may proceed to format the children, or `None` if `id` is
    /// already on the stack (the caller should emit the placeholder).
    fn enter(id: i64) -> Option<Self> {
        REPR_IN_PROGRESS.with(|cell| {
            let mut stack = cell.borrow_mut();
            if stack.contains(&id) {
                return None;
            }
            stack.push(id);
            Some(ReprGuard)
        })
    }
}

impl Drop for ReprGuard {
    fn drop(&mut self) {
        REPR_IN_PROGRESS.with(|cell| {
            cell.borrow_mut().pop();
        });
    }
}

/// RAII guard for the `eq` cycle-detection stack.  Identical shape to
/// [`ReprGuard`] but keyed on the ordered pair of value ids being compared.
struct EqGuard;

impl EqGuard {
    /// Attempts to enter the recursion for `(a_id, b_id)`.  Returns
    /// `Some(guard)` when the caller may proceed with element-wise comparison,
    /// or `None` if the pair is already on the stack (the caller treats the
    /// cycle as equal).
    fn enter(a_id: i64, b_id: i64) -> Option<Self> {
        EQ_IN_PROGRESS.with(|cell| {
            let mut stack = cell.borrow_mut();
            let pair = (a_id, b_id);
            if stack.contains(&pair) {
                return None;
            }
            stack.push(pair);
            Some(EqGuard)
        })
    }
}

impl Drop for EqGuard {
    fn drop(&mut self) {
        EQ_IN_PROGRESS.with(|cell| {
            cell.borrow_mut().pop();
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PyKey — hashable subset of Value used as dict/set keys (unchanged)
// ─────────────────────────────────────────────────────────────────────────────

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
    /// Hashable frozenset key.  Stores a sorted-canonical Vec of inner keys
    /// so equality and hashing are content-based (matching CPython).
    FrozenSet(Vec<PyKey>),
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
            (PyKey::Float(a), PyKey::Float(b)) => f64::from_bits(*a) == f64::from_bits(*b),
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
            (PyKey::FrozenSet(a), PyKey::FrozenSet(b)) => a == b,
            (PyKey::Tuple(a), PyKey::Tuple(b)) => a == b,
            (PyKey::Bytes(a), PyKey::Bytes(b)) => a.as_ref() == b.as_ref(),
            // Complex equality: two complex keys are equal iff both components match.
            // -0.0 == 0.0 in IEEE 754, which matches CPython's `==` for complex.
            (PyKey::Complex(ar, ai), PyKey::Complex(br, bi)) => ar == br && ai == bi,
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
                    if f.is_finite() && f.fract() == 0.0 {
                        if let Some(big) = BigInt::from_f64(f) {
                            pykey_hash_bigint(&big).hash(state);
                            return;
                        }
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
            PyKey::FrozenSet(items) => {
                4u8.hash(state);
                for k in items {
                    k.hash(state);
                }
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

    // CPython frozenset shuffle: ((h ^ 89869747) ^ (h << 16)) * 3644798167
    #[inline]
    fn cpython_shuffle(h: u64) -> u64 {
        let s = (h ^ 89869747u64) ^ (h << 16);
        s.wrapping_mul(3644798167u64)
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
                let acc = (acc << 31) | (acc >> 33); // rotl31
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
        PyKey::FrozenSet(items) => {
            // CPython Objects/setobject.c frozenset_hash algorithm.
            let mut h: u64 = 0;
            for item in items {
                h ^= cpython_shuffle(py_hash_pykey(item) as u64);
            }
            // Length mixing
            let n = items.len() as u64;
            h ^= (n + 1).wrapping_mul(1927868237u64);
            // Secondary mix
            h ^= (h >> 11) ^ (h >> 25);
            // Final scramble
            h = h.wrapping_mul(69069u64).wrapping_add(907133923u64);
            let result = h as i64;
            if result == -1 { 590923713 } else { result }
        }
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

// ─────────────────────────────────────────────────────────────────────────────
// Shared types
// ─────────────────────────────────────────────────────────────────────────────

pub type NameSet = Rc<HashSet<String>>;

#[derive(Debug, Clone)]
pub struct Environment {
    pub values: HashMap<String, Value>,
    pub local_names: NameSet,
    pub global_names: NameSet,
    pub nonlocal_names: NameSet,
    pub parent: Option<EnvRef>,
}

pub type EnvRef = Rc<RefCell<Environment>>;

impl Environment {
    pub fn new(parent: Option<EnvRef>) -> EnvRef {
        Rc::new(RefCell::new(Self {
            values: HashMap::new(),
            local_names: Rc::new(HashSet::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            parent,
        }))
    }
}

#[derive(Debug, Clone)]
pub struct UserFunctionParam {
    pub name: String,
    pub default: Option<Value>,
    pub is_args: bool,
    pub is_kwargs: bool,
    pub is_keyword_only: bool,
    pub is_positional_only: bool,
}

/// Discriminator for `UserFunction` semantics.  `@classmethod` and
/// `@staticmethod` decorators produce a UserFunction whose body Rc-shares
/// with the original, distinguished only by this tag — no wrapper variant.
/// `Builtin` is the relocated form of the former `Opaque::BuiltinFunction`
/// variant: a Rust built-in dispatched by name (`len`, `print`, …).  Same
/// representable state as the old variant, but unified into the function
/// value's kind tag so `Opaque` shrinks by one variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserFunctionKind {
    #[default]
    Regular,
    ClassMethod,
    StaticMethod,
    Builtin(&'static str),
}

#[derive(Debug, Clone)]
pub struct UserFunction {
    /// Globally unique identity for fn_cache keying — stable across Rc drops/reallocations.
    pub id: u64,
    pub kind: UserFunctionKind,
    /// The bare function name as declared.  Used for self-recursive slot lookup
    /// and error messages.  Do not mutate through this field; user code that
    /// assigns `f.__name__ = "x"` writes to `user_name` instead.
    pub name: String,
    /// Fully-qualified compile-time name (e.g. `"outer.<locals>.inner"`).
    /// Exposed as `f.__qualname__`.  User code that assigns `f.__qualname__ = "x"`
    /// writes to `user_qualname` instead.
    pub qualname: String,
    /// Mutable override for `f.__name__`.  `None` means fall back to `name`.
    pub user_name: RefCell<Option<String>>,
    /// Mutable override for `f.__qualname__`.  `None` means fall back to `qualname`.
    pub user_qualname: RefCell<Option<String>>,
    /// `f.__module__` — the name of the module in which the function was defined.
    /// Defaults to `"__main__"` (module-name tracking is not yet implemented).
    /// User code may assign any value; `del f.__module__` resets it to `None`.
    pub module: RefCell<Value>,
    /// `f.__doc__` — the function's docstring, or `None` if absent.
    /// pyrust does not yet extract docstrings at compile time, so this is always
    /// `None` at construction time.  User code may assign any value;
    /// `del f.__doc__` resets it to `None`.
    pub doc: RefCell<Value>,
    /// Arbitrary dynamic attributes set by user code (`f.x = v`).
    /// Exposed as `f.__dict__`.  Stored as a `Value::dict` wrapped in
    /// `Rc<RefCell<...>>` so that:
    ///   1. `get_attr("__dict__")` returns the **live** dict object (CPython
    ///      semantics: mutations through the returned dict propagate back to
    ///      the function).
    ///   2. `f.__dict__ = new_dict` replaces the inner Value via
    ///      `*attrs.borrow_mut() = new_dict_value`.
    ///   3. Bound-method copies and `@classmethod`/`@staticmethod` wrappers
    ///      share the same `Rc` (same as before) so they all see the same dict.
    ///
    /// Initialized lazily on first use (`None` means no attrs have been set
    /// yet).  The `RefCell` provides interior mutability so that
    /// `get_attr("__dict__")` can initialize the dict through a shared
    /// `Rc<UserFunction>` without requiring `&mut self`.
    pub attrs: RefCell<Option<Rc<RefCell<Value>>>>,
    /// `f.__annotations__` — dict mapping annotated parameter names (and
    /// `'return'` for the return annotation) to their evaluated annotation
    /// values.  Populated at function-definition time (matching CPython's
    /// runtime evaluation semantics).  User code may replace the entire dict
    /// via `f.__annotations__ = {...}`.
    ///
    /// Stored as a `Value` (always `Value::dict(...)`) so that repeated
    /// attribute reads return the *same* dict object (Rc identity), matching
    /// CPython: `f.__annotations__ is f.__annotations__` is `True`.
    pub annotations: RefCell<Value>,
    pub params: Vec<UserFunctionParam>,
    pub local_names: NameSet,
    pub local_index: Rc<HashMap<String, u32>>,
    pub global_names: NameSet,
    pub nonlocal_names: NameSet,
    pub env: EnvRef,
    pub is_pure: bool,
    pub precompiled_code: Option<Rc<dyn Any>>,
    /// When `kind` is `StaticMethod` or `ClassMethod`, holds the original
    /// wrapped function `Rc` so that `sm.__func__` can return the exact same
    /// object that was passed to `staticmethod(f)` / `classmethod(f)`, preserving
    /// `sm.__func__ is f` identity.  `None` for `Regular` and `Builtin` functions.
    pub wrapped_func: Option<Rc<UserFunction>>,
}

#[derive(Debug, Clone)]
pub struct PyClass {
    pub name: String,
    /// Qualified name (e.g. `Outer.Inner` for nested classes).  Exposed as
    /// `C.__qualname__` via the attribute lookup fast-path in `get_attr`; NOT
    /// stored in `attrs` — CPython keeps `__qualname__` as a type-level
    /// descriptor on `type`, not as an entry in the class's own `__dict__`.
    pub qualname: String,
    /// First (primary) base class, or `None` if there is no explicit base.
    /// Kept as a dedicated `Option` so that the many existing single-inheritance
    /// paths (exception walks, `super()`, primitive-class chains) remain fast
    /// without iterating a `Vec`.
    pub base: Option<Rc<RefCell<PyClass>>>,
    /// Second through Nth bases for multiple inheritance.  Empty for classes
    /// with zero or one explicit base.  Combined with `base`, the full list of
    /// direct bases is `[base] ++ extra_bases` (in declaration order).
    pub extra_bases: Vec<Rc<RefCell<PyClass>>>,
    pub attrs: IndexMap<String, Value>,
    /// Bumped on every `assign_attr` / `delete_attr` on this class (but NOT
    /// its bases — a base mutation is separately detectable via the base's own
    /// counter).  Inline attribute caches store the version at fill time and
    /// re-validate on each hit; a mismatch triggers a slow-path re-fill.
    ///
    /// Wrapped u64: overflow after 2^64 writes is benign (just a cold-path
    /// cache miss).  `Cell<u64>` avoids the `borrow_mut()` overhead on the
    /// hot re-validation path.
    pub mutation_version: Cell<u64>,
    pub subclasses: RefCell<Vec<Weak<RefCell<PyClass>>>>,
}

#[derive(Debug, Clone)]
pub struct PyInstance {
    pub class: Rc<RefCell<PyClass>>,
    pub attrs: IndexMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct PyModule {
    pub name: String,
    pub attrs: HashMap<String, Value>,
}

// ─────────────────────────────────────────────────────────────────────────────
// NaN-boxing constants
// ─────────────────────────────────────────────────────────────────────────────

const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const INT_SIGN_BIT: u64 = 1 << 47;
const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;

const TAG_NONE_BITS: u64 = 0xFFF9_0000_0000_0000;
/// Internal-only sentinel for "uninitialised register slot". Positive NaN
/// bit pattern outside the negative-NaN range used by the tag system; not
/// observable from Python code. See `Value::unset()`.
const UNSET_BITS: u64 = 0x7FF8_0000_0000_BAD0;
/// The `NotImplemented` singleton.  Stored as a reserved NaN-box pattern
/// so identity comparison is a cheap u64-eq, and the value doesn't take an
/// `Opaque` variant.  Pattern is a positive NaN in the same family as
/// `UNSET_BITS` — not classified as a float by `top16()`-based checks
/// because we test the exact bit pattern explicitly. See [`Value::not_implemented`].
const NOT_IMPLEMENTED_BITS: u64 = 0x7FF8_0000_0000_BAD2;
/// The `Ellipsis` singleton (`...`). Stored as a reserved NaN-box pattern
/// in the same positive-NaN family as `UNSET_BITS` and `NOT_IMPLEMENTED_BITS`.
/// Must be tested explicitly before the float arm in `kind()` / `is_float()`.
const ELLIPSIS_BITS: u64 = 0x7FF8_0000_0000_BAD4;
const TAG_BOOL_BITS: u64 = 0xFFFA_0000_0000_0000;
const TAG_INT_BITS: u64 = 0xFFFB_0000_0000_0000;
const TAG_STR_BITS: u64 = 0xFFFC_0000_0000_0000;
const TAG_TUPLE_BITS: u64 = 0xFFFD_0000_0000_0000;
const TAG_LIST_BITS: u64 = 0xFFFE_0000_0000_0000;
const TAG_OPAQUE_BITS: u64 = 0xFFFF_0000_0000_0000;

// top16() tag values used in match arms and comparisons
const TAG_FLOAT_MAX: u16 = 0xFFF8; // all top16 values ≤ this are floats
const TAG_NONE: u16 = 0xFFF9;
const TAG_BOOL: u16 = 0xFFFA;
const TAG_INT: u16 = 0xFFFB;
const TAG_STR: u16 = 0xFFFC;
const TAG_TUPLE: u16 = 0xFFFD;
const TAG_LIST: u16 = 0xFFFE;
const TAG_OPAQUE: u16 = 0xFFFF;

/// Convert a ryu decimal string like `"0.00009999"` (or `"-0.00001"`) to
/// CPython-style scientific notation like `"9.999e-05"`.  Only called when
/// the value's absolute magnitude is known to be in (0, 1e-4), so the string
/// always starts with `"0.000"` (possibly with a leading `"-"`).
fn decimal_to_python_sci(s: &str) -> String {
    // Handle sign.
    let (neg, digits_str) = if let Some(d) = s.strip_prefix('-') {
        (true, d)
    } else {
        (false, s)
    };
    let sign_prefix = if neg { "-" } else { "" };

    // digits_str is like "0.00009999".  Split on '.'.
    // We know it's "0.<zeros><sig_digits>".
    let after_dot = digits_str
        .split_once('.')
        .map(|(_, frac)| frac)
        .unwrap_or(digits_str);

    // Count leading zeros to determine the exponent.
    let leading_zeros = after_dot.chars().take_while(|&c| c == '0').count();
    let exp = -(leading_zeros as i32 + 1);
    let sig = &after_dot[leading_zeros..]; // significant digits, no leading zeros

    // Build mantissa string: first digit, then '.' + rest (if any).
    let mantissa = if sig.len() <= 1 {
        sig.to_string()
    } else {
        format!("{}.{}", &sig[..1], &sig[1..])
    };

    let exp_abs = exp.unsigned_abs();
    let exp_sign = if exp < 0 { "-" } else { "+" };
    if exp_abs < 10 {
        format!("{sign_prefix}{mantissa}e{exp_sign}0{exp_abs}")
    } else {
        format!("{sign_prefix}{mantissa}e{exp_sign}{exp_abs}")
    }
}

fn format_float(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    // Use ryu for the shortest round-trip decimal representation.
    let raw = ryu::Buffer::new().format(v).to_string();
    // ryu omits the '+' in positive exponents and may use a single digit for the
    // exponent, e.g. "1e20" or "1e-5".  CPython always writes "1e+20" and "1e-05"
    // (sign present, exponent at least two digits).  Normalise to match.
    if let Some(e_pos) = raw.find('e') {
        let mantissa = &raw[..e_pos];
        let exp_str = &raw[e_pos + 1..]; // everything after 'e'
        let (sign, digits) = if let Some(d) = exp_str.strip_prefix('-') {
            ("-", d)
        } else if let Some(d) = exp_str.strip_prefix('+') {
            ("+", d)
        } else {
            ("+", exp_str)
        };
        // Pad the exponent to at least two digits.
        return if digits.len() < 2 {
            format!("{mantissa}e{sign}0{digits}")
        } else {
            format!("{mantissa}e{sign}{digits}")
        };
    }
    // No scientific notation in ryu output (e.g. "0.0001", "1.1", "1.0").
    // CPython uses scientific notation when 0 < abs(v) < 1e-4, but ryu may
    // produce a plain decimal string for those values (e.g. "0.00001" for 1e-5).
    // Detect this case and reformat as scientific to match CPython.
    let abs_v = v.abs();
    if abs_v != 0.0 && abs_v < 1e-4 {
        // ryu produced a decimal string (e.g. "0.00009999") for a value that
        // CPython would render in scientific notation (e.g. "9.999e-05").
        // Parse ryu's decimal string to extract the significant digits and
        // exponent, then re-emit in CPython's exponent format.
        return decimal_to_python_sci(&raw);
    }
    // ryu guarantees a decimal point for non-integer floats, and always includes
    // ".0" for integer-valued floats, so no further fixup is needed.
    raw
}

/// Returns the top 16 bits of a Value's u64 encoding — the tag.
///
/// **Caveat:** the sentinels [`UNSET_BITS`] and [`NOT_IMPLEMENTED_BITS`] are
/// positive-NaN bit patterns whose `top16` is `0x7FF8` (≤ [`TAG_FLOAT_MAX`]).
/// They will be classified as `Float` by a raw `top16`-based check.  Always
/// route through [`Value::kind`] (which checks the exact bit pattern first),
/// [`Value::is_unset`], or [`Value::is_not_implemented`] when distinguishing
/// these from real floats.
#[inline(always)]
fn top16(bits: u64) -> u16 {
    (bits >> 48) as u16
}

// ─────────────────────────────────────────────────────────────────────────────
// BuiltinTypeOps — operations the VM performs on built-in objects whose
// concrete implementation lives in `pyrust-builtins`.  `pyrust-core` never
// names a concrete built-in type; the VM dispatches through this trait.
//
// `state` is `Rc<RefCell<Box<dyn Any>>>` so impls can downcast to their
// concrete state and `RefCell::borrow_mut` when they need to mutate.  Default
// methods return CPython-style "object is not X" errors; impls override only
// the operations their type actually supports.
// ─────────────────────────────────────────────────────────────────────────────

pub type BuiltinState = Rc<RefCell<Box<dyn Any>>>;

pub trait BuiltinTypeOps: 'static {
    fn type_name(&self) -> &'static str;

    fn repr(&self, state: &BuiltinState) -> String {
        let _ = state;
        format!("<{} object>", self.type_name())
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        let _ = state;
        true
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let _ = (state, other);
        false
    }

    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        let _ = state;
        None
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let _ = (state, name);
        None
    }

    fn setattr(&self, state: &BuiltinState, name: &str, value: Value) -> Result<()> {
        let _ = (state, value);
        Err(PyError::named(
            "AttributeError",
            format!("'{}' object has no attribute '{}'", self.type_name(), name),
        ))
    }

    fn call(
        &self,
        state: &BuiltinState,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let _ = (state, args, kwargs);
        Err(PyError::named(
            "TypeError",
            format!("'{}' object is not callable", self.type_name()),
        ))
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let _ = (state, args, kwargs);
        Err(PyError::named(
            "AttributeError",
            format!("'{}' object has no attribute '{}'", self.type_name(), name),
        ))
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        let _ = state;
        Err(PyError::named(
            "TypeError",
            format!("'{}' object is not iterable", self.type_name()),
        ))
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        let _ = state;
        None
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let _ = (state, key);
        Err(PyError::named(
            "TypeError",
            format!("'{}' object is not subscriptable", self.type_name()),
        ))
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let _ = (state, key, value);
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object does not support item assignment",
                self.type_name()
            ),
        ))
    }

    fn delete_item(&self, state: &BuiltinState, key: &Value) -> Result<()> {
        let _ = (state, key);
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object does not support item deletion",
                self.type_name()
            ),
        ))
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let _ = (state, item);
        Err(PyError::named(
            "TypeError",
            format!("argument of type '{}' is not iterable", self.type_name()),
        ))
    }

    /// Returns true if `name` is a method this type exposes.  Used by
    /// `hasattr(x, name)`.  Default returns `false`; impls with a method
    /// table should override.  (We don't probe `call_method` here because
    /// that would require running it with placeholder args, which has
    /// observable side effects.)
    fn has_method(&self, name: &str) -> bool {
        let _ = name;
        false
    }

    /// Returns true if this type is iterable.  Default returns `false`;
    /// impls that override `iter_next` must also override this — the VM
    /// uses `is_iterable()` to choose the dispatch path before ever
    /// calling `iter_next`, so an iterable type that forgets to override
    /// `is_iterable` will be treated as non-iterable.
    fn is_iterable(&self) -> bool {
        false
    }

    /// Convert this object to a `PyKey` for use as a dict/set key.  Returns
    /// `None` if this type is not hashable.  Frozensets etc. override this.
    fn to_key(&self, state: &BuiltinState) -> Option<PyKey> {
        let _ = state;
        None
    }
}

/// Registry function: maps a stable type-name string to its `BuiltinTypeOps`.
/// Installed once at interpreter startup by the consumer of pyrust-core
/// (typically `pyrust-builtins`).  `pyrust-core` never names a concrete
/// built-in type — it only looks up by string and calls through the trait.
pub type BuiltinRegistry = fn(&str) -> Option<&'static dyn BuiltinTypeOps>;

static BUILTIN_REGISTRY: std::sync::OnceLock<BuiltinRegistry> = std::sync::OnceLock::new();

/// Install the registry that maps built-in type names to their dispatch ops.
/// Safe to call multiple times — only the first call wins.
pub fn install_builtin_registry(registry: BuiltinRegistry) {
    let _ = BUILTIN_REGISTRY.set(registry);
}

/// Look up dispatch ops for a built-in type name.  Returns `None` if no
/// registry has been installed or the type is unknown.
pub fn lookup_builtin_ops(type_name: &str) -> Option<&'static dyn BuiltinTypeOps> {
    BUILTIN_REGISTRY.get().and_then(|reg| reg(type_name))
}

/// Callback for materialising an arbitrary `Value` into its iteration items.
/// Installed by `pyrust` (which owns the interpreter's `iter_values` impl)
/// so that `pyrust-builtins` iterator helpers can drain a source value
/// without depending on the interpreter crate.
///
/// The helpers (`enumerate`/`zip`/`reversed`) call this lazily — at first
/// `iter_next` invocation — to preserve side-effect timing: side effects of
/// the source (e.g. `open()` reading a file) happen at iteration start, not
/// at helper construction.
pub type IterValuesFn = fn(&Value) -> Result<Vec<Value>>;

static ITER_VALUES_FN: std::sync::OnceLock<IterValuesFn> = std::sync::OnceLock::new();

pub fn install_iter_values(f: IterValuesFn) {
    let _ = ITER_VALUES_FN.set(f);
}

pub fn iter_values_via_registry(value: &Value) -> Result<Vec<Value>> {
    match ITER_VALUES_FN.get() {
        Some(f) => f(value),
        None => Err(PyError::Runtime(
            "iter_values callback not installed".to_string(),
        )),
    }
}

/// Callback for the canonical Python `<` ordering between two `Value`s.
/// Installed by `pyrust` (which owns the interpreter's `compare_values`
/// in `interpreter/helpers.rs`) so that `pyrust-builtins` sort helpers
/// can route through the same predicate the `<` / `>` operators use —
/// covering BigInt, nested List, and any other types the interpreter
/// supports — without depending on the interpreter crate.
///
/// Mirrors the [`IterValuesFn`] / [`install_iter_values`] pattern (see
/// issue #428 — duplicate `compare_values` in `pyrust-builtins::list`
/// was missing BigInt and List support, so `list.sort()` / `sorted()`
/// raised `TypeError` on values the `<` operator accepted).
pub type CompareValuesFn = fn(&Value, &Value) -> Result<std::cmp::Ordering>;

static COMPARE_VALUES_FN: std::sync::OnceLock<CompareValuesFn> = std::sync::OnceLock::new();

pub fn install_compare_values(f: CompareValuesFn) {
    let _ = COMPARE_VALUES_FN.set(f);
}

pub fn compare_values_via_registry(a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
    match COMPARE_VALUES_FN.get() {
        Some(f) => f(a, b),
        None => Err(PyError::Runtime(
            "compare_values callback not installed".to_string(),
        )),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared backing storage for mutable Tier 1 containers
//
// Lists and sets share their items behind an `Rc<…RefCell…>` so that
// `Value::clone` preserves Python's reference semantics for mutable
// containers: a copy of the Value points at the same backing storage as the
// original, mutations through either alias propagate, and `id(a) == id(b)`
// after `b = a`.  Dict already used this shape (`Rc<RefCell<IndexMap<…>>>`
// inside `Opaque::Dict`); see issue #305.
// ─────────────────────────────────────────────────────────────────────────────

/// Shared backing for a Python `list`.  `items` holds the elements; `obj_id`
/// is a monotonic identity captured at construction and inherited by every
/// `Rc::clone` so `id(x) == id(y)` whenever `y` is an aliased clone of `x`.
pub struct ListInner {
    pub items: RefCell<Vec<Value>>,
    pub obj_id: u64,
}

/// Shared backing for a Python `set`.  Same shape and rationale as
/// [`ListInner`]; `items` is an [`IndexSet`] (insertion-ordered) so iteration
/// order matches the rest of the interpreter's set surface.
pub struct SetInner {
    pub items: RefCell<IndexSet<PyKey>>,
    pub obj_id: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Opaque — heap-allocated types that don't fit in 48 bits
// ─────────────────────────────────────────────────────────────────────────────

pub enum Opaque {
    PyBigInt(Rc<BigInt>),
    Dict(Rc<RefCell<IndexMap<PyKey, Value>>>),
    /// Mutable `set` storage.  Shared via `Rc` so `Value::clone` produces an
    /// alias rather than a deep copy, matching Python's reference semantics.
    /// See [`SetInner`] and issue #305.
    Set(Rc<SetInner>),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    UserFunction(Rc<UserFunction>),
    PyClass(Rc<RefCell<PyClass>>),
    PyInstance(Rc<RefCell<PyInstance>>),
    PyModule(Rc<RefCell<PyModule>>),
    BoundMethod {
        function: Rc<UserFunction>,
        receiver: Rc<RefCell<PyInstance>>,
        /// Monotonic allocation id so that `a = obj.method; a is a` is True
        /// while `obj.method is obj.method` is False (#722).  Preserved across
        /// clones (Rc-sharing semantics), matches the `SmallTuple2` pattern.
        obj_id: u64,
    },
    /// A classmethod bound to a specific class (the first argument will be `cls`).
    ClassBoundMethod {
        function: Rc<UserFunction>,
        class: Rc<RefCell<PyClass>>,
        /// Monotonic allocation id for identity semantics (#722).
        obj_id: u64,
    },
    /// Proxy returned by `super(cls, instance)`. Attribute lookup on this proxy
    /// starts from `cls`'s parent class and binds to `instance`.
    ///
    /// Note: zero-argument `super()` (CPython's implicit `__class__` cell) is not
    /// supported. Use the two-argument form `super(CurrentClass, self)` explicitly.
    SuperProxy {
        class: Rc<RefCell<PyClass>>,
        instance: Rc<RefCell<PyInstance>>,
        /// Monotonic allocation id for identity semantics (#722).
        obj_id: u64,
    },
    /// Proxy returned by `super(cls, cls_instance)` where the second argument is
    /// a class (used in classmethods). Attribute lookup starts from `cls`'s parent
    /// and binds as a `ClassBoundMethod` to `obj_class`.
    SuperProxyClass {
        class: Rc<RefCell<PyClass>>,
        obj_class: Rc<RefCell<PyClass>>,
        /// Monotonic allocation id for identity semantics (#722).
        obj_id: u64,
    },
    /// A live generator object.  The concrete execution state (registers, pc,
    /// iterator slots, etc.) is stored as a type-erased `Box<dyn Any>` so that
    /// `pyrust-core` does not need to depend on `pyrust`'s bytecode types.
    Generator(Rc<RefCell<Box<dyn std::any::Any>>>),
    /// An immutable byte string.  Constructed via the `b"..."` literal or
    /// the `bytes(...)` builtin.  Stored behind `Rc` for cheap clones.
    Bytes(Rc<Vec<u8>>),
    /// A Python `complex` number stored as (real, imag).
    Complex(f64, f64),
    /// Inline storage for a 2-element tuple.  Eliminates the secondary
    /// `Vec<Value>` heap allocation for the most common Python tuple shape
    /// (e.g. `dict.items()` entries, `enumerate()` yields, `divmod()`).
    /// `obj_id` preserves stable `id()` identity across clones, matching the
    /// behaviour of the pool-based tuple path.
    SmallTuple2 {
        items: [Value; 2],
        obj_id: u64,
    },
    /// Inline storage for a 3-element tuple.  Same rationale as
    /// `SmallTuple2`; covers `str.partition()`/`rpartition()` and similar
    /// fixed-arity returns.
    SmallTuple3 {
        items: [Value; 3],
        obj_id: u64,
    },
    /// A type-erased built-in object whose operations are dispatched through
    /// the installed [`BuiltinTypeOps`] table.  Used for built-in types whose
    /// payload doesn't justify a dedicated Tier 1 variant (file, property,
    /// frozenset value, dict views, iterator helpers, …).  `pyrust-core`
    /// never names the concrete type — it only calls through `ops`.
    BuiltinObject {
        ops: &'static dyn BuiltinTypeOps,
        state: BuiltinState,
    },
}

impl Clone for Opaque {
    fn clone(&self) -> Self {
        match self {
            Opaque::PyBigInt(rc) => Opaque::PyBigInt(Rc::clone(rc)),
            Opaque::Dict(rc) => Opaque::Dict(Rc::clone(rc)),
            // Sets share backing storage on clone; see `SetInner` and #305.
            Opaque::Set(rc) => Opaque::Set(Rc::clone(rc)),
            Opaque::Range { start, stop, step } => Opaque::Range {
                start: *start,
                stop: *stop,
                step: *step,
            },
            Opaque::UserFunction(f) => Opaque::UserFunction(Rc::clone(f)),
            Opaque::PyClass(c) => Opaque::PyClass(Rc::clone(c)),
            Opaque::PyInstance(i) => Opaque::PyInstance(Rc::clone(i)),
            Opaque::PyModule(m) => Opaque::PyModule(Rc::clone(m)),
            Opaque::BoundMethod {
                function,
                receiver,
                obj_id,
            } => Opaque::BoundMethod {
                function: Rc::clone(function),
                receiver: Rc::clone(receiver),
                obj_id: *obj_id,
            },
            Opaque::ClassBoundMethod {
                function,
                class,
                obj_id,
            } => Opaque::ClassBoundMethod {
                function: Rc::clone(function),
                class: Rc::clone(class),
                obj_id: *obj_id,
            },
            Opaque::SuperProxy {
                class,
                instance,
                obj_id,
            } => Opaque::SuperProxy {
                class: Rc::clone(class),
                instance: Rc::clone(instance),
                obj_id: *obj_id,
            },
            Opaque::SuperProxyClass {
                class,
                obj_class,
                obj_id,
            } => Opaque::SuperProxyClass {
                class: Rc::clone(class),
                obj_class: Rc::clone(obj_class),
                obj_id: *obj_id,
            },
            Opaque::Generator(state) => Opaque::Generator(Rc::clone(state)),
            Opaque::Bytes(rc) => Opaque::Bytes(Rc::clone(rc)),
            Opaque::Complex(re, im) => Opaque::Complex(*re, *im),
            Opaque::SmallTuple2 { items, obj_id } => Opaque::SmallTuple2 {
                items: [items[0].clone(), items[1].clone()],
                obj_id: *obj_id,
            },
            Opaque::SmallTuple3 { items, obj_id } => Opaque::SmallTuple3 {
                items: [items[0].clone(), items[1].clone(), items[2].clone()],
                obj_id: *obj_id,
            },
            Opaque::BuiltinObject { ops, state } => Opaque::BuiltinObject {
                ops: *ops,
                state: Rc::clone(state),
            },
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ValueKind — borrow-based view used for pattern matching
// ─────────────────────────────────────────────────────────────────────────────

pub enum ValueKind<'a> {
    None,
    Bool(bool),
    Int(i64),
    BigInt(&'a BigInt),
    Float(f64),
    Str(&'a str),
    /// Borrowed view of a list's elements.  Holds a `Ref<'_, Vec<Value>>`
    /// guard so the `RefCell`'s borrow counter is bumped for the duration
    /// of the match (#450).  A concurrent `borrow_mut()` (e.g. from a
    /// `list_push` triggered by user code during a match-arm body) now
    /// panics with the standard `RefCell` already-borrowed message
    /// instead of producing silent UB through `RefCell::as_ptr()`.
    List(std::cell::Ref<'a, Vec<Value>>),
    /// Borrowed view of a tuple's elements.  Backed either by the pool path
    /// (`TAG_TUPLE`) which stores `Vec<Value>`, or by the inline
    /// `Opaque::SmallTuple2/3` path which stores a fixed-size array.  Tuples
    /// are immutable so no `RefCell` wraps them — a raw slice is sound.
    Tuple(&'a [Value]),
    /// Borrowed view of a dict.  Like [`Self::List`], holds a
    /// `Ref<'_, IndexMap<...>>` guard so the `RefCell` borrow check
    /// catches concurrent mutation (#450).
    Dict(std::cell::Ref<'a, IndexMap<PyKey, Value>>),
    /// Borrowed view of a set.  Same `Ref` guard rationale as
    /// [`Self::List`] / [`Self::Dict`] (#450).
    Set(std::cell::Ref<'a, IndexSet<PyKey>>),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    UserFunction(&'a Rc<UserFunction>),
    BuiltinFunction(&'static str),
    PyClass(&'a Rc<RefCell<PyClass>>),
    PyInstance(&'a Rc<RefCell<PyInstance>>),
    PyModule(&'a Rc<RefCell<PyModule>>),
    BoundMethod {
        function: &'a Rc<UserFunction>,
        receiver: &'a Rc<RefCell<PyInstance>>,
    },
    ClassBoundMethod {
        function: &'a Rc<UserFunction>,
        class: &'a Rc<RefCell<PyClass>>,
    },
    SuperProxy {
        class: &'a Rc<RefCell<PyClass>>,
        instance: &'a Rc<RefCell<PyInstance>>,
    },
    SuperProxyClass {
        class: &'a Rc<RefCell<PyClass>>,
        obj_class: &'a Rc<RefCell<PyClass>>,
    },
    Generator(&'a Rc<RefCell<Box<dyn std::any::Any>>>),
    /// Synthesized view of the [`NotImplemented`] sentinel.  No backing
    /// `Opaque` variant — the value is encoded as a reserved NaN-box bit
    /// pattern, and `kind()` decodes it here so existing matchers keep
    /// working.  Identity-test via `Value::is_not_implemented()` is cheaper.
    NotImplemented,
    /// Synthesized view of the `Ellipsis` singleton (`...`).  Encoded as a
    /// reserved NaN-box bit pattern; no `Opaque` variant.
    Ellipsis,
    Bytes(&'a Rc<Vec<u8>>),
    Complex(f64, f64),
    BuiltinObject {
        ops: &'static dyn BuiltinTypeOps,
        state: &'a BuiltinState,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Thread-local free lists for fixed-size allocations
// ─────────────────────────────────────────────────────────────────────────────

// Each free slot stores a *mut u8 to the next free slot in its first 8 bytes.
thread_local! {
    // (head, len)
    static POOL_B: Cell<(*mut u8, usize)> = const { Cell::new((std::ptr::null_mut(), 0)) };
}

const POOL_B_CAP: usize = 64;

#[inline(always)]
unsafe fn pool_b_alloc() -> *mut u8 {
    POOL_B.with(|c| {
        let (head, len) = c.get();
        if len > 0 {
            let next = unsafe { *(head as *const *mut u8) };
            c.set((next, len - 1));
            head
        } else {
            unsafe { alloc(Layout::from_size_align(20, 8).unwrap()) }
        }
    })
}

#[inline(always)]
unsafe fn pool_b_dealloc(ptr: *mut u8) {
    POOL_B.with(|c| {
        let (head, len) = c.get();
        // In debug builds catch double-free: a block already in the pool has its first
        // 8 bytes overwritten with the next-pointer, so it cannot equal any live
        // allocation's first word.  Check ptr != head as a lightweight guard; a full
        // traversal is too expensive for a hot path.
        debug_assert!(
            ptr != head || head.is_null(),
            "pool_b_dealloc: double-free detected (ptr == head)"
        );
        if len < POOL_B_CAP {
            unsafe { *(ptr as *mut *mut u8) = head };
            c.set((ptr, len + 1));
        } else {
            unsafe { dealloc(ptr, Layout::from_size_align(20, 8).unwrap()) };
        }
    })
}

// Pool for `Box<Opaque>` allocations (#281).  Every `Value::opaque(...)` boxes
// an `Opaque` enum; for hot patterns like `Opaque::SmallTuple2` per-iteration
// these allocations dominate.  Recycling the fixed-size slabs (the enum
// reserves max-variant bytes regardless of which arm is alive) eliminates the
// general allocator round-trip.  Allocator round-trips for non-hot variants
// also benefit; the pool is per-thread and has bounded capacity so it can
// never leak memory unboundedly.
const POOL_OPAQUE_CAP: usize = 128;

thread_local! {
    static POOL_OPAQUE: Cell<(*mut u8, usize)> = const { Cell::new((std::ptr::null_mut(), 0)) };
}

#[inline(always)]
fn opaque_layout() -> Layout {
    Layout::new::<Opaque>()
}

#[inline(always)]
unsafe fn pool_opaque_alloc() -> *mut u8 {
    POOL_OPAQUE.with(|c| {
        let (head, len) = c.get();
        if len > 0 {
            let next = unsafe { *(head as *const *mut u8) };
            c.set((next, len - 1));
            head
        } else {
            unsafe { alloc(opaque_layout()) }
        }
    })
}

#[inline(always)]
unsafe fn pool_opaque_dealloc(ptr: *mut u8) {
    POOL_OPAQUE.with(|c| {
        let (head, len) = c.get();
        if len < POOL_OPAQUE_CAP {
            unsafe { *(ptr as *mut *mut u8) = head };
            c.set((ptr, len + 1));
        } else {
            unsafe { dealloc(ptr, opaque_layout()) };
        }
    })
}

// Pool for Vec<Value> struct headers (list / tuple).
// Layout: [ptr: *mut Value][len: usize][cap: usize][obj_id: u64] = 32 bytes / align 8.
// The extra 8 bytes at offset 24 hold a unique monotonic id for id() identity.

const VEC_HDR_SIZE: usize = 32; // Vec<Value>(24) + obj_id(8) — asserted in Value impl
const VEC_HDR_ALIGN: usize = 8;
const POOL_VEC_HDR_CAP: usize = 64;

thread_local! {
    static POOL_VEC_HDR: Cell<(*mut u8, usize)> = const { Cell::new((std::ptr::null_mut(), 0)) };
}

#[inline(always)]
unsafe fn pool_vec_hdr_alloc() -> *mut u8 {
    POOL_VEC_HDR.with(|c| {
        let (head, len) = c.get();
        if len > 0 {
            let next = unsafe { *(head as *const *mut u8) };
            c.set((next, len - 1));
            head
        } else {
            unsafe { alloc(Layout::from_size_align(VEC_HDR_SIZE, VEC_HDR_ALIGN).unwrap()) }
        }
    })
}

#[inline(always)]
unsafe fn pool_vec_hdr_dealloc(ptr: *mut u8) {
    POOL_VEC_HDR.with(|c| {
        let (head, len) = c.get();
        if len < POOL_VEC_HDR_CAP {
            unsafe { *(ptr as *mut *mut u8) = head };
            c.set((ptr, len + 1));
        } else {
            unsafe {
                dealloc(
                    ptr,
                    Layout::from_size_align(VEC_HDR_SIZE, VEC_HDR_ALIGN).unwrap(),
                )
            };
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Value — NaN-boxed u64
// ─────────────────────────────────────────────────────────────────────────────

#[repr(transparent)]
pub struct Value(u64);

impl Value {
    const _ASSERT_VEC_HDR: () = {
        assert!(std::mem::size_of::<Vec<Value>>() == 24); // Vec<Value> must be 24 bytes
        assert!(std::mem::align_of::<Vec<Value>>() == VEC_HDR_ALIGN);
        assert!(VEC_HDR_SIZE >= 32); // room for obj_id at offset 24
    };

    // ── Constructors ─────────────────────────────────────────────────────────

    pub fn none() -> Self {
        Value(TAG_NONE_BITS)
    }

    /// A distinct, internal-only sentinel representing "register slot not
    /// initialised". The bit pattern is a specific positive NaN that the VM
    /// never produces from real Python values — `0x7FF8_0000_0000_BAD0`.
    ///
    /// `is_unset()` returns true only for this exact pattern.
    ///
    /// Reading an unset slot through `kind()`, `truthy()`, or any accessor
    /// that routes through `kind()` will panic in debug builds (via
    /// `debug_assert!`).  In release builds the assert is elided; the runtime
    /// tripwire is the compiler's `Insn::CheckLocal` emission.  Do not pass an
    /// unset `Value` to any accessor other than `is_unset()` / `as_some()`.
    pub fn unset() -> Self {
        Value(UNSET_BITS)
    }

    pub fn is_unset(&self) -> bool {
        self.0 == UNSET_BITS
    }

    /// `Some(self)` if this slot has been written, else `None`.
    /// Useful for migrating call sites that previously held `Option<Value>`.
    #[inline]
    pub fn as_some(&self) -> Option<&Value> {
        if self.is_unset() { None } else { Some(self) }
    }

    #[inline]
    pub fn as_some_mut(&mut self) -> Option<&mut Value> {
        if self.is_unset() { None } else { Some(self) }
    }

    pub fn bool_(b: bool) -> Self {
        Value(TAG_BOOL_BITS | b as u64)
    }

    pub fn int(n: i64) -> Self {
        const MAX_I48: i64 = (1 << 47) - 1;
        const MIN_I48: i64 = -(1 << 47);
        if (MIN_I48..=MAX_I48).contains(&n) {
            Value(TAG_INT_BITS | (n as u64 & PAYLOAD_MASK))
        } else {
            Value::opaque(Opaque::PyBigInt(Rc::new(BigInt::from(n))))
        }
    }

    pub fn bigint(n: BigInt) -> Self {
        Value::opaque(Opaque::PyBigInt(Rc::new(n)))
    }

    pub fn float(f: f64) -> Self {
        if f.is_nan() {
            Value(CANONICAL_NAN)
        } else {
            Value(f.to_bits())
        }
    }

    pub fn string(s: impl AsRef<str>) -> Self {
        let s = s.as_ref();
        let len = s.len();
        // Layout A: [rc_type:u32][sub_len:u32][ref:*mut u8][bytes: u8 × len]
        //            offset 0     offset 4     offset 8     offset 16
        let layout = Layout::from_size_align(16 + len, 8).unwrap();
        let ptr = unsafe { alloc(layout) };
        unsafe {
            (ptr as *mut u32).write(2u32); // rc=1, type=0
            (ptr.add(4) as *mut u32).write(len as u32);
            // Store the self-referential pointer as *const u8 (immutable bytes).
            (ptr.add(8) as *mut *const u8).write(ptr.add(16)); // ref → own bytes
            if len > 0 {
                ptr.add(16)
                    .copy_from_nonoverlapping(s.as_bytes().as_ptr(), len);
            }
        }
        Value(TAG_STR_BITS | (ptr as u64 & PAYLOAD_MASK))
    }

    pub fn string_slice(&self, byte_start: usize, byte_end: usize) -> Self {
        // Guard against inverted indices: wrapping subtraction would produce a
        // colossal sub_len and the resulting slice descriptor would be invalid.
        assert!(
            byte_start <= byte_end,
            "string_slice: byte_start ({byte_start}) > byte_end ({byte_end})"
        );
        let sub_len = byte_end - byte_start;
        let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
        let rc_type = unsafe { *(hdr as *const u32) };
        // self.ref (offset 8) points to self's bytes[0]; add byte_start for new slice
        let self_ref = unsafe { *(hdr.add(8) as *const *const u8) };
        let new_ref = unsafe { self_ref.add(byte_start) };

        // Find A_ptr (Layout A root) to increment its rc, and compute new offset.
        // Layout A: A_ptr = hdr,   new_offset = byte_start
        // Layout B: A_ptr = ref - stored_offset - 16,  new_offset = stored_offset + byte_start
        //
        // For Layout B→B chains the stored_offset already encodes the distance from A's
        // bytes[0] to this slice's bytes[0], so subtracting it (plus the 16-byte header)
        // from self_ref always recovers A_ptr without underflow.
        let (a_ptr, new_offset): (*mut u8, usize) = if rc_type & 1 == 0 {
            (hdr as *mut u8, byte_start)
        } else {
            let base = unsafe { *(hdr.add(16) as *const u32) as usize };
            // SAFETY: `base` is the byte distance from Layout A's bytes[0] to this
            // slice's bytes[0], written by a prior `string_slice` call.  Therefore
            // `self_ref == a_ptr + 16 + base` by construction, and the subtraction
            // `self_ref - (base + 16)` cannot underflow.  The `byte_start <= byte_end`
            // assert at entry guarantees we never produce an invalid descriptor, so
            // this invariant is preserved through any chain of slices.
            let a_ptr = unsafe { (self_ref as *mut u8).sub(base.wrapping_add(16)) };
            debug_assert!(
                a_ptr as usize + 16 + base == self_ref as usize,
                "string_slice: Layout B offset mismatch — possible heap corruption"
            );
            (a_ptr, base + byte_start)
        };

        // Increment A.rc. Saturate instead of wrapping: a saturated rc leaks the
        // backing buffer, but u32::MAX/2 simultaneous slice references is unreachable.
        unsafe {
            let hdr_a = a_ptr as *mut u32;
            *hdr_a = (*hdr_a).saturating_add(2);
        }

        // Layout B: [rc_type:u32][sub_len:u32][ref:*mut u8][offset:u32]
        //            offset 0     offset 4     offset 8     offset 16
        // ref points directly to this slice's bytes[0]; ref - offset - 16 = A_ptr
        let ptr = unsafe { pool_b_alloc() };
        unsafe {
            (ptr as *mut u32).write(3u32); // rc=1, type=1
            (ptr.add(4) as *mut u32).write(sub_len as u32);
            *(ptr.add(8) as *mut *const u8) = new_ref;
            (ptr.add(16) as *mut u32).write(new_offset as u32);
        }
        Value(TAG_STR_BITS | (ptr as u64 & PAYLOAD_MASK))
    }

    // Shared allocator for the tuple pool header.  Writes Vec<Value> at offset 0
    // and the unique obj_id at offset 24, then tags with the supplied tag bits.
    //
    // Tuple is the only TAG_*_BITS payload that still uses this 32-byte slab
    // layout.  List moved to an `Rc<ListInner>` payload in #305 to make
    // `Value::clone` an alias rather than a deep copy.
    unsafe fn alloc_seq_hdr(tag_bits: u64, v: Vec<Value>, obj_id: u64) -> Self {
        let hdr = unsafe { pool_vec_hdr_alloc() };
        unsafe {
            std::ptr::write(hdr as *mut Vec<Value>, v);
            std::ptr::write(hdr.add(24) as *mut u64, obj_id);
        }
        Value(tag_bits | (hdr as u64 & PAYLOAD_MASK))
    }

    /// Construct a new `list` Value.  Storage is an `Rc<ListInner>` so that
    /// `Value::clone` shares the backing — matching Python's reference
    /// semantics for mutable containers (#305).
    pub fn list(v: Vec<Value>) -> Self {
        let inner = Rc::new(ListInner {
            items: RefCell::new(v),
            obj_id: next_obj_id(),
        });
        unsafe { Self::list_from_rc(inner) }
    }

    /// Construct a list Value from an existing `Rc<ListInner>` — used when
    /// multiple Values must share the same backing list (e.g. cloning).
    /// Caller is responsible for incrementing the strong count *before*
    /// calling this if they want a logical alias rather than a move.
    ///
    /// SAFETY: consumes one strong-count reference from `rc`.  The matching
    /// drop happens in `Drop for Value` when `TAG_LIST` is observed.
    unsafe fn list_from_rc(rc: Rc<ListInner>) -> Self {
        let raw = Rc::into_raw(rc);
        Value(TAG_LIST_BITS | (raw as u64 & PAYLOAD_MASK))
    }

    pub fn tuple(mut v: Vec<Value>) -> Self {
        // Small-tuple fast path (#281): route 2- and 3-element tuples through
        // `Opaque::SmallTuple2/3` so the backing `Vec<Value>` heap allocation
        // is avoided.  These shapes dominate hot sites (`dict.items()`,
        // `enumerate()`, `divmod()`, `str.partition()`, …).
        match v.len() {
            2 => {
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                Value::opaque(Opaque::SmallTuple2 {
                    items: [a, b],
                    obj_id: next_obj_id(),
                })
            }
            3 => {
                let c = v.pop().unwrap();
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                Value::opaque(Opaque::SmallTuple3 {
                    items: [a, b, c],
                    obj_id: next_obj_id(),
                })
            }
            _ => unsafe { Self::alloc_seq_hdr(TAG_TUPLE_BITS, v, next_obj_id()) },
        }
    }

    fn tuple_with_id(v: Vec<Value>, obj_id: u64) -> Self {
        unsafe { Self::alloc_seq_hdr(TAG_TUPLE_BITS, v, obj_id) }
    }

    pub fn dict(d: IndexMap<PyKey, Value>) -> Self {
        Value::opaque(Opaque::Dict(Rc::new(RefCell::new(d))))
    }

    pub fn set(s: IndexSet<PyKey>) -> Self {
        Value::opaque(Opaque::Set(Rc::new(SetInner {
            items: RefCell::new(s),
            obj_id: next_obj_id(),
        })))
    }

    pub fn bytes(b: Vec<u8>) -> Self {
        Value::opaque(Opaque::Bytes(Rc::new(b)))
    }

    pub fn complex(re: f64, im: f64) -> Self {
        Value::opaque(Opaque::Complex(re, im))
    }

    /// Construct a generic built-in object dispatched through the installed
    /// [`BuiltinTypeOps`] table.  `ops` must outlive the program (typically
    /// `&'static`); `state` is owned heap state of any concrete type.
    pub fn builtin_object(ops: &'static dyn BuiltinTypeOps, state: Box<dyn Any>) -> Self {
        Value::opaque(Opaque::BuiltinObject {
            ops,
            state: Rc::new(RefCell::new(state)),
        })
    }

    /// Construct a generic built-in object that shares state with an existing
    /// `BuiltinState` cell.  Used when multiple Values must reference the
    /// same underlying mutable state.
    pub fn builtin_object_shared(ops: &'static dyn BuiltinTypeOps, state: BuiltinState) -> Self {
        Value::opaque(Opaque::BuiltinObject { ops, state })
    }

    pub fn range(start: i64, stop: i64, step: i64) -> Self {
        Value::opaque(Opaque::Range { start, stop, step })
    }

    pub fn user_function(f: Rc<UserFunction>) -> Self {
        Value::opaque(Opaque::UserFunction(f))
    }

    /// Construct a built-in function value.  Stored as a `UserFunction` with
    /// `kind = Builtin(name)` so the function machinery is unified (one Opaque
    /// variant for both user and built-in functions).  The per-name
    /// `UserFunction` stub is interned in a thread-local cache so repeated
    /// calls don't reallocate it — equivalent in cost to the previous
    /// single-pointer payload.
    pub fn builtin_function(name: &'static str) -> Self {
        thread_local! {
            static CACHE: RefCell<HashMap<&'static str, Rc<UserFunction>>>
                = RefCell::new(HashMap::new());
        }
        let func = CACHE.with(|c| {
            if let Some(f) = c.borrow().get(name) {
                return Rc::clone(f);
            }
            let f = Rc::new(UserFunction {
                id: next_fn_id(),
                kind: UserFunctionKind::Builtin(name),
                name: name.to_string(),
                qualname: name.to_string(),
                user_name: RefCell::new(None),
                user_qualname: RefCell::new(None),
                module: RefCell::new(Value::none()),
                doc: RefCell::new(Value::none()),
                attrs: RefCell::new(None),
                annotations: RefCell::new(Value::dict(IndexMap::new())),
                params: Vec::new(),
                local_names: Rc::new(HashSet::new()),
                local_index: Rc::new(HashMap::new()),
                global_names: Rc::new(HashSet::new()),
                nonlocal_names: Rc::new(HashSet::new()),
                env: Environment::new(None),
                is_pure: false,
                precompiled_code: None,
                wrapped_func: None,
            });
            c.borrow_mut().insert(name, Rc::clone(&f));
            f
        });
        Value::opaque(Opaque::UserFunction(func))
    }

    /// The `NotImplemented` singleton.  Stored as a reserved NaN-box bit
    /// pattern so identity comparison is a single u64 equality check.
    pub fn not_implemented() -> Self {
        Value(NOT_IMPLEMENTED_BITS)
    }

    pub fn is_not_implemented(&self) -> bool {
        self.0 == NOT_IMPLEMENTED_BITS
    }

    pub fn ellipsis() -> Self {
        Value(ELLIPSIS_BITS)
    }

    pub fn is_ellipsis(&self) -> bool {
        self.0 == ELLIPSIS_BITS
    }

    pub fn py_class(c: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::PyClass(c))
    }

    pub fn py_instance(i: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::PyInstance(i))
    }

    pub fn py_module(m: Rc<RefCell<PyModule>>) -> Self {
        Value::opaque(Opaque::PyModule(m))
    }

    pub fn bound_method(function: Rc<UserFunction>, receiver: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::BoundMethod {
            function,
            receiver,
            obj_id: next_obj_id(),
        })
    }

    /// Wrap a function with a different `UserFunctionKind` tag.  Used by
    /// `@classmethod` / `@staticmethod`: produces a new UserFunction that
    /// shares everything but the kind tag.
    ///
    /// The wrapped function reuses the **original** `id` so the fn_cache and
    /// any other id-keyed caches share a single entry between the decorated
    /// and undecorated forms.  The function body and `is_pure` flag are
    /// identical (the kind tag only affects attribute-lookup-time binding,
    /// not execution), so cache hits across forms are correct.  See #303.
    pub fn with_function_kind(f: Rc<UserFunction>, kind: UserFunctionKind) -> Self {
        // Fast path: kind already matches and is not a wrapper kind — reuse the Rc
        // directly.  Wrapper kinds (StaticMethod/ClassMethod) must always produce a
        // new Rc so that `staticmethod(sm)` gives a fresh object distinct from `sm`
        // (matching CPython identity semantics where each `staticmethod(x)` call
        // returns a new object regardless of whether `x` is already a staticmethod).
        let is_wrapper_kind = matches!(
            kind,
            UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
        );
        if f.kind == kind && !is_wrapper_kind {
            return Value::opaque(Opaque::UserFunction(f));
        }
        // When wrapping as staticmethod/classmethod, record `f` directly so
        // `sm.__func__` returns the exact same Rc that was passed in, preserving
        // object identity (`sm.__func__ is f`).
        let wrapped_func = if is_wrapper_kind {
            Some(Rc::clone(&f))
        } else {
            None
        };
        let new_fn = UserFunction {
            id: f.id,
            kind,
            name: f.name.clone(),
            qualname: f.qualname.clone(),
            user_name: RefCell::new(f.user_name.borrow().clone()),
            user_qualname: RefCell::new(f.user_qualname.borrow().clone()),
            module: RefCell::new(f.module.borrow().clone()),
            doc: RefCell::new(f.doc.borrow().clone()),
            attrs: RefCell::new(f.attrs.borrow().as_ref().map(Rc::clone)),
            annotations: RefCell::new(f.annotations.borrow().clone()),
            params: f.params.clone(),
            local_names: Rc::clone(&f.local_names),
            local_index: Rc::clone(&f.local_index),
            global_names: Rc::clone(&f.global_names),
            nonlocal_names: Rc::clone(&f.nonlocal_names),
            env: Rc::clone(&f.env),
            is_pure: f.is_pure,
            precompiled_code: f.precompiled_code.clone(),
            wrapped_func,
        };
        Value::opaque(Opaque::UserFunction(Rc::new(new_fn)))
    }

    pub fn class_method(f: Rc<UserFunction>) -> Self {
        Value::with_function_kind(f, UserFunctionKind::ClassMethod)
    }

    pub fn static_method(f: Rc<UserFunction>) -> Self {
        Value::with_function_kind(f, UserFunctionKind::StaticMethod)
    }

    pub fn class_bound_method(function: Rc<UserFunction>, class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::ClassBoundMethod {
            function,
            class,
            obj_id: next_obj_id(),
        })
    }

    pub fn super_proxy(class: Rc<RefCell<PyClass>>, instance: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::SuperProxy {
            class,
            instance,
            obj_id: next_obj_id(),
        })
    }

    pub fn super_proxy_class(class: Rc<RefCell<PyClass>>, obj_class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::SuperProxyClass {
            class,
            obj_class,
            obj_id: next_obj_id(),
        })
    }

    /// Create a generator value.  `state` is the type-erased `GeneratorFrame`
    /// managed by the VM.
    pub fn generator(state: Box<dyn std::any::Any>) -> Self {
        Value::opaque(Opaque::Generator(Rc::new(RefCell::new(state))))
    }

    fn opaque(o: Opaque) -> Self {
        // SAFETY: `pool_opaque_alloc` returns a block sized/aligned for `Opaque`
        // (either a recycled one from this thread's free list or a fresh
        // `alloc(Layout::new::<Opaque>())`).  Writing through the cast pointer
        // initialises the slot; the matching `pool_opaque_dealloc` in `Drop` is
        // only invoked after `drop_in_place`, so no double-drop.  See #281.
        let ptr = unsafe { pool_opaque_alloc() as *mut Opaque };
        unsafe { std::ptr::write(ptr, o) };
        Value(TAG_OPAQUE_BITS | (ptr as u64 & PAYLOAD_MASK))
    }

    // ── Type checks ──────────────────────────────────────────────────────────

    pub fn is_none(&self) -> bool {
        self.0 == TAG_NONE_BITS
    }

    pub fn is_bool(&self) -> bool {
        top16(self.0) == TAG_BOOL
    }

    pub fn is_int(&self) -> bool {
        top16(self.0) == TAG_INT
            || (top16(self.0) == TAG_OPAQUE
                && matches!(unsafe { &*self.opaque_ptr() }, Opaque::PyBigInt(_)))
    }

    pub fn is_float(&self) -> bool {
        // Exclude the reserved positive-NaN sentinels (UNSET, NotImplemented,
        // Ellipsis) whose `top16` falls in the float range but which aren't floats.
        if self.0 == NOT_IMPLEMENTED_BITS || self.0 == UNSET_BITS || self.0 == ELLIPSIS_BITS {
            return false;
        }
        top16(self.0) <= TAG_FLOAT_MAX
    }

    pub fn is_str(&self) -> bool {
        top16(self.0) == TAG_STR
    }

    pub fn is_tuple(&self) -> bool {
        if top16(self.0) == TAG_TUPLE {
            return true;
        }
        // Small tuples (2/3 elements) live in `Opaque::SmallTuple2/3` to
        // avoid the backing `Vec<Value>` heap allocation.  See #281.
        if top16(self.0) == TAG_OPAQUE {
            return matches!(
                unsafe { &*self.opaque_ptr() },
                Opaque::SmallTuple2 { .. } | Opaque::SmallTuple3 { .. }
            );
        }
        false
    }

    pub fn is_list(&self) -> bool {
        top16(self.0) == TAG_LIST
    }

    /// Returns a stable identity value for pool-allocated and Rc-shared types:
    /// - tuple: reads the monotonic obj_id stored at hdr+24
    /// - list: reads `obj_id` from the shared [`ListInner`]; aliased clones
    ///   (Rc-shared) all surface the same id, matching Python's `id()`
    ///   semantics for `b = a` aliasing (#305).
    /// - set: reads `obj_id` from the shared [`SetInner`] (same rationale).
    /// - str: uses the pool pointer address directly.
    ///
    /// Returns `None` for primitive types (callers handle those directly).
    pub fn value_id(&self) -> Option<i64> {
        // `as i64` wraps past 2^63; tracked separately, not specific to this
        // PR (tuple has the same shape).
        match top16(self.0) {
            TAG_TUPLE => {
                let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
                Some(unsafe { *(hdr.add(24) as *const u64) } as i64)
            }
            TAG_LIST => Some(unsafe { self.list_inner() }.obj_id as i64),
            TAG_STR => Some((self.0 & PAYLOAD_MASK) as i64),
            TAG_OPAQUE => match unsafe { &*self.opaque_ptr() } {
                // Small-tuple variants stash a monotonic obj_id alongside their
                // inline payload so `id()` stays stable across clones; see #281.
                Opaque::SmallTuple2 { obj_id, .. } => Some(*obj_id as i64),
                Opaque::SmallTuple3 { obj_id, .. } => Some(*obj_id as i64),
                // Sets are Rc-shared with an obj_id captured at construction;
                // aliased clones surface the same id (#305).
                Opaque::Set(rc) => Some(rc.obj_id as i64),
                // Dicts already share an Rc backing.  Surface the Rc pointer
                // address so `b = a; id(a) == id(b)` for dicts too (#305).
                Opaque::Dict(rc) => Some(Rc::as_ptr(rc) as i64),
                // BigInt clones share the inner Rc even though the opaque
                // wrapper is reallocated.  Use that Rc address for Python
                // object identity (`b = a; a is b`) parity (#523).
                Opaque::PyBigInt(rc) => Some(Rc::as_ptr(rc) as i64),
                // Generator clones share the same Rc<RefCell<...>>; surface its
                // pointer address so `id(g)` is non-zero and stable, and so
                // `g is iter(g)` can be backed by ptr equality (#714).
                Opaque::Generator(rc) => Some(Rc::as_ptr(rc) as i64),
                // Bytes and PyModule share Rc backing across clones; use the
                // Rc pointer address so `b = a; id(a) == id(b)` holds (#722).
                Opaque::Bytes(rc) => Some(Rc::as_ptr(rc) as i64),
                Opaque::PyModule(rc) => Some(Rc::as_ptr(rc) as i64),
                // BoundMethod / ClassBoundMethod / SuperProxy / SuperProxyClass:
                // each allocation gets a unique monotonic obj_id so that
                // `a = obj.method; a is a` is True while
                // `obj.method is obj.method` is False (#722).
                Opaque::BoundMethod { obj_id, .. } => Some(*obj_id as i64),
                Opaque::ClassBoundMethod { obj_id, .. } => Some(*obj_id as i64),
                Opaque::SuperProxy { obj_id, .. } => Some(*obj_id as i64),
                Opaque::SuperProxyClass { obj_id, .. } => Some(*obj_id as i64),
                // BuiltinObject: the Rc<RefCell<...>> state is shared across
                // clones; its address is a stable, per-object id (#722).
                Opaque::BuiltinObject { state, .. } => Some(Rc::as_ptr(state) as i64),
                _ => None,
            },
            _ => None,
        }
    }

    // ── Private unsafe helpers ───────────────────────────────────────────────

    unsafe fn str_hdr(&self) -> *const u8 {
        (self.0 & PAYLOAD_MASK) as *const u8
    }

    unsafe fn str_as_str(&self) -> &str {
        unsafe {
            let hdr = self.str_hdr();
            let sub_len = *(hdr.add(4) as *const u32) as usize;
            let ref_ptr = *(hdr.add(8) as *const *const u8);
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(ref_ptr, sub_len))
        }
    }

    unsafe fn tuple_ptr(&self) -> *mut Vec<Value> {
        (self.0 & PAYLOAD_MASK) as *mut _
    }

    /// Raw pointer to the shared [`ListInner`] backing.  Caller must guarantee
    /// `self` is a TAG_LIST value.
    unsafe fn list_inner_ptr(&self) -> *const ListInner {
        (self.0 & PAYLOAD_MASK) as *const ListInner
    }

    /// Borrow the inner list header.  SAFETY: `self` must be a TAG_LIST value
    /// and the Rc must be live (which it is for any reachable `Value`).
    unsafe fn list_inner(&self) -> &ListInner {
        unsafe { &*self.list_inner_ptr() }
    }

    unsafe fn opaque_ptr(&self) -> *mut Opaque {
        (self.0 & PAYLOAD_MASK) as *mut _
    }

    // ── Public accessors ─────────────────────────────────────────────────────

    pub fn as_bool(&self) -> bool {
        debug_assert!(
            !self.is_unset(),
            "Value::as_bool() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        (self.0 & 1) != 0
    }

    pub fn as_int_raw(&self) -> i64 {
        debug_assert!(
            !self.is_unset(),
            "Value::as_int_raw() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        let raw = (self.0 & PAYLOAD_MASK) as i64;
        if self.0 & INT_SIGN_BIT != 0 {
            raw | !PAYLOAD_MASK as i64
        } else {
            raw
        }
    }

    pub fn as_float_raw(&self) -> f64 {
        debug_assert!(
            !self.is_unset(),
            "Value::as_float_raw() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        f64::from_bits(self.0)
    }

    pub fn as_str(&self) -> Option<&str> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_str() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if self.is_str() {
            Some(unsafe { self.str_as_str() })
        } else {
            None
        }
    }

    /// Borrow the list's elements as a shared slice.
    ///
    /// SAFETY CONTRACT: the returned `&[Value]` borrows the underlying Vec
    /// through a raw pointer obtained from `RefCell::as_ptr()`; **no
    /// `Ref<...>` guard is held**.  This means the Rust aliasing model treats
    /// the read borrow as live for as long as the caller holds the returned
    /// reference, even though the `RefCell`'s internal counter is not
    /// incremented.
    ///
    /// Callers MUST NOT, while the returned reference is live:
    ///   1. obtain another borrow (mutable OR shared) on the same `Value`,
    ///   2. obtain a borrow on **any other `Value` that aliases the same
    ///      `Rc<ListInner>`** — list backing storage is Rc-shared after
    ///      #305, so `Value::clone` produces a second `Value` whose
    ///      `as_list[_mut]` would point at the same Vec,
    ///   3. call into code that may transitively re-enter this list (e.g.
    ///      user `__iter__`/`__hash__`).
    ///
    /// Single-threaded execution alone is NOT sufficient — the threat model
    /// here is intra-thread aliasing via Rc-clone, not data races.  When in
    /// doubt, materialise the read side into an owned `Vec<Value>` via
    /// `as_list().map(<[_]>::to_vec)` before reaching for a `&mut` borrow on
    /// any potentially-aliased Value.
    ///
    /// See `unalias_args_for_mutation` for the helper used at builtin
    /// dispatch sites to make this safe automatically.
    pub fn as_list(&self) -> Option<&[Value]> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_list() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if self.is_list() {
            let inner = unsafe { self.list_inner() };
            Some(unsafe { &*inner.items.as_ptr() })
        } else {
            None
        }
    }

    // `as_list_mut` was removed in #448 — use the scoped operation
    // methods (`Value::list_with_mut`, `list_push`, `list_extend`, …)
    // instead.  The previous `unsafe { &mut *cell.as_ptr() }` pattern
    // exposed an unguarded `&mut Vec<Value>` across crate boundaries
    // and forced callers to manually re-derive the aliasing-safety
    // property that `RefCell` already enforces internally.

    /// Borrow the tuple's elements as a slice.  Backs both the pool-allocated
    /// path (`TAG_TUPLE`) and the inline small-tuple path
    /// (`Opaque::SmallTuple2/3`); see #281.
    pub fn as_tuple(&self) -> Option<&[Value]> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_tuple() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if top16(self.0) == TAG_TUPLE {
            return Some(unsafe { &*self.tuple_ptr() });
        }
        if top16(self.0) == TAG_OPAQUE {
            match unsafe { &*self.opaque_ptr() } {
                Opaque::SmallTuple2 { items, .. } => return Some(&items[..]),
                Opaque::SmallTuple3 { items, .. } => return Some(&items[..]),
                _ => {}
            }
        }
        None
    }

    pub fn as_opaque(&self) -> Option<&Opaque> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_opaque() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if top16(self.0) == TAG_OPAQUE {
            Some(unsafe { &*self.opaque_ptr() })
        } else {
            None
        }
    }

    /// Zero-cost borrow of the inner `Rc<RefCell<PyInstance>>` without
    /// an `Rc::clone`.  Returns `None` for any non-PyInstance value.
    ///
    /// Used by the GetAttr / CallMethod inline cache to read the class
    /// pointer and version without paying the clone cost on the fast path.
    pub fn as_py_instance_rc(&self) -> Option<&Rc<RefCell<PyInstance>>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::PyInstance(rc) = o {
                Some(rc)
            } else {
                None
            }
        })
    }

    // `as_opaque_mut` removed in #448 — the only callers were the
    // `as_dict_mut` / `as_set_mut` accessors that have themselves
    // been retired.

    /// Borrow the dict's IndexMap (read-only).
    ///
    /// SAFETY CONTRACT: see [`Value::as_list`].  Dict storage is
    /// `Rc<RefCell<IndexMap<...>>>` (Rc-shared since dict was the original
    /// reference type); the read borrow is unguarded via `RefCell::as_ptr`,
    /// so callers MUST NOT hold any other borrow (shared or mutable) on any
    /// Value that shares this `Rc` while the returned reference is live.
    /// **For new code prefer [`Self::dict_with`] / [`Self::dict_with_mut`]
    /// (#448)** — these scope the borrow to the operation and prevent the
    /// aliasing class of bugs by construction.
    pub fn as_dict(&self) -> Option<&IndexMap<PyKey, Value>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(unsafe { &*rc.as_ref().as_ptr() })
            } else {
                None
            }
        })
    }

    // `as_dict_mut` removed in #448 — use `Value::dict_with_mut`,
    // `dict_insert`, `dict_shift_remove`, `dict_clear`, `dict_extend`
    // instead.

    /// Borrow the set's IndexSet (read-only).
    ///
    /// SAFETY CONTRACT: see [`Value::as_list`].  Set storage is `Rc<SetInner>`
    /// after #305; same Rc-aliasing concerns apply.  Prefer
    /// [`Self::set_with`] / [`Self::set_with_mut`] (#448) for new code.
    pub fn as_set(&self) -> Option<&IndexSet<PyKey>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Set(rc) = o {
                Some(unsafe { &*rc.items.as_ptr() })
            } else {
                None
            }
        })
    }

    // `as_set_mut` removed in #448 — use `Value::set_with_mut`,
    // `set_add`, `set_discard`, `set_clear`, `set_extend` instead.

    pub fn get_dict_rc(&self) -> Option<&Rc<RefCell<IndexMap<PyKey, Value>>>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(rc)
            } else {
                None
            }
        })
    }

    // ── Scoped-borrow operation API (#448) ────────────────────────────
    //
    // The methods below are the safe replacements for `as_list_mut` /
    // `as_dict_mut` / `as_set_mut`.  They take `&self`, scope their
    // `RefCell::borrow_mut()` to the operation's lifetime, and never
    // hand a `&mut <storage>` out across the function boundary.
    //
    // What this DOES guarantee:
    // - No mutable reference to the underlying `Vec` / `IndexMap` /
    //   `IndexSet` crosses an API boundary.  Every mutating operation
    //   is bounded by a single `borrow_mut()` window.
    // - The dispatcher's previous `unalias_args_for_mutation` dance
    //   is no longer needed for self-aliased iterating calls
    //   (`a.extend(a)` etc.) because the iterable is snapshotted
    //   before the receiver's `borrow_mut()` opens.
    //
    // What this DOES NOT (yet) guarantee:
    // - Full exclusivity against the still-unguarded read accessors
    //   `as_list` / `as_dict` / `as_set` / `kind()`.  Those return
    //   shared references via `RefCell::as_ptr()` *without* bumping
    //   the cell's borrow counter, so a `borrow_mut()` taken here
    //   will succeed even if a `&[Value]` / `&IndexMap<...>` from
    //   one of those accessors is still live.  Rewriting the read
    //   accessors to use `borrow()` is a follow-up to this PR — see
    //   the trailing notes on #448.
    //
    // Each method panics with the standard `RefCell::borrow_mut`
    // already-borrowed message if a re-entrant call (e.g. user
    // `__hash__` that mutates the same container while another
    // borrow is live) violates the `RefCell` rules.  That panic
    // surfaces UB-adjacent behaviour at the earliest possible point.

    /// Borrow the list's `Rc<ListInner>`.  Returns `None` when `self`
    /// is not a list.  Internal helper for the operation methods
    /// below.
    fn list_inner_rc(&self) -> Option<&ListInner> {
        if self.is_list() {
            Some(unsafe { self.list_inner() })
        } else {
            None
        }
    }

    /// Borrow the dict's `Rc<RefCell<IndexMap<...>>>`.
    fn dict_rc(&self) -> Option<&Rc<RefCell<IndexMap<PyKey, Value>>>> {
        self.get_dict_rc()
    }

    /// Borrow the set's `Rc<SetInner>`.
    fn set_inner_rc(&self) -> Option<&SetInner> {
        self.as_opaque().and_then(|o| match o {
            Opaque::Set(rc) => Some(rc.as_ref()),
            _ => None,
        })
    }

    /// Scoped read access to the list's elements.  The closure runs
    /// while the immutable `RefCell` borrow is live; the borrow is
    /// dropped before this method returns.  Returns `None` when
    /// `self` is not a list.
    pub fn list_with<R>(&self, f: impl FnOnce(&Vec<Value>) -> R) -> Option<R> {
        let inner = self.list_inner_rc()?;
        Some(f(&inner.items.borrow()))
    }

    /// Scoped mutable access.  See [`Self::list_with`].  Inner
    /// closures MUST NOT call back into the same list (e.g. by
    /// recursing through user `__eq__`) — a re-entrant access will
    /// panic with `RefCell` already-borrowed.
    pub fn list_with_mut<R>(&self, f: impl FnOnce(&mut Vec<Value>) -> R) -> Option<R> {
        let inner = self.list_inner_rc()?;
        Some(f(&mut inner.items.borrow_mut()))
    }

    /// `list.append(item)`.  Returns `Err` (TypeError) when `self`
    /// is not a list.
    pub fn list_push(&self, item: Value) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "list_push receiver is not a list".to_string())
        })?;
        inner.items.borrow_mut().push(item);
        Ok(())
    }

    /// `list.extend(snapshot)` — caller passes an owned Vec already
    /// materialised from the iterable, so no aliasing window exists
    /// between the read and the write.
    pub fn list_extend(&self, snapshot: Vec<Value>) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "list_extend receiver is not a list".to_string(),
            )
        })?;
        inner.items.borrow_mut().extend(snapshot);
        Ok(())
    }

    /// `list.clear()`.
    pub fn list_clear(&self) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "list_clear receiver is not a list".to_string())
        })?;
        inner.items.borrow_mut().clear();
        Ok(())
    }

    /// `list.reverse()`.
    pub fn list_reverse(&self) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "list_reverse receiver is not a list".to_string(),
            )
        })?;
        inner.items.borrow_mut().reverse();
        Ok(())
    }

    /// `list.insert(idx, item)` — `idx` is the already-normalised
    /// position (caller does CPython-style negative-index folding).
    pub fn list_insert(&self, idx: usize, item: Value) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "list_insert receiver is not a list".to_string(),
            )
        })?;
        let mut items = inner.items.borrow_mut();
        let pos = idx.min(items.len());
        items.insert(pos, item);
        Ok(())
    }

    /// `list.pop(idx)` — removes and returns the element at `idx`
    /// (already normalised; caller raises IndexError on out-of-range).
    pub fn list_pop_at(&self, idx: usize) -> Result<Value> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "list_pop receiver is not a list".to_string())
        })?;
        let mut items = inner.items.borrow_mut();
        if idx >= items.len() {
            return Err(PyError::named(
                "IndexError",
                "pop index out of range".to_string(),
            ));
        }
        Ok(items.remove(idx))
    }

    /// Length of the list.
    pub fn list_len(&self) -> Option<usize> {
        self.list_inner_rc().map(|i| i.items.borrow().len())
    }

    /// Length of the set.
    pub fn set_len(&self) -> Option<usize> {
        self.set_inner_rc().map(|i| i.items.borrow().len())
    }

    /// Length of the dict.
    pub fn dict_len(&self) -> Option<usize> {
        self.dict_rc().map(|rc| rc.borrow().len())
    }

    /// Scoped read access to the set.
    pub fn set_with<R>(&self, f: impl FnOnce(&IndexSet<PyKey>) -> R) -> Option<R> {
        let inner = self.set_inner_rc()?;
        Some(f(&inner.items.borrow()))
    }

    /// Scoped mutable access to the set.
    pub fn set_with_mut<R>(&self, f: impl FnOnce(&mut IndexSet<PyKey>) -> R) -> Option<R> {
        let inner = self.set_inner_rc()?;
        Some(f(&mut inner.items.borrow_mut()))
    }

    /// `set.add(key)` — returns true if the key was newly inserted,
    /// false if it was already present.
    pub fn set_add(&self, key: PyKey) -> Result<bool> {
        let inner = self.set_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "set_add receiver is not a set".to_string())
        })?;
        Ok(inner.items.borrow_mut().insert(key))
    }

    /// `set.discard(key)` — removes if present, no error if missing.
    pub fn set_discard(&self, key: &PyKey) -> Result<bool> {
        let inner = self.set_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "set_discard receiver is not a set".to_string())
        })?;
        Ok(inner.items.borrow_mut().shift_remove(key))
    }

    /// `set.update(snapshot)`.
    pub fn set_extend(&self, snapshot: Vec<PyKey>) -> Result<()> {
        let inner = self.set_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "set_extend receiver is not a set".to_string())
        })?;
        inner.items.borrow_mut().extend(snapshot);
        Ok(())
    }

    /// `set.clear()`.
    pub fn set_clear(&self) -> Result<()> {
        let inner = self.set_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "set_clear receiver is not a set".to_string())
        })?;
        inner.items.borrow_mut().clear();
        Ok(())
    }

    /// Scoped read access to the dict.
    pub fn dict_with<R>(&self, f: impl FnOnce(&IndexMap<PyKey, Value>) -> R) -> Option<R> {
        let rc = self.dict_rc()?;
        Some(f(&rc.borrow()))
    }

    /// Scoped mutable access to the dict.
    pub fn dict_with_mut<R>(&self, f: impl FnOnce(&mut IndexMap<PyKey, Value>) -> R) -> Option<R> {
        let rc = self.dict_rc()?;
        Some(f(&mut rc.borrow_mut()))
    }

    /// `dict[key] = value`.
    pub fn dict_insert(&self, key: PyKey, value: Value) -> Result<Option<Value>> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "dict_insert receiver is not a dict".to_string(),
            )
        })?;
        Ok(rc.borrow_mut().insert(key, value))
    }

    /// `dict.shift_remove(key)`.
    pub fn dict_shift_remove(&self, key: &PyKey) -> Result<Option<Value>> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "dict_shift_remove receiver is not a dict".to_string(),
            )
        })?;
        Ok(rc.borrow_mut().shift_remove(key))
    }

    /// `dict.clear()`.
    pub fn dict_clear(&self) -> Result<()> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named("TypeError", "dict_clear receiver is not a dict".to_string())
        })?;
        rc.borrow_mut().clear();
        Ok(())
    }

    /// `dict.update(snapshot)`.
    pub fn dict_extend(&self, snapshot: Vec<(PyKey, Value)>) -> Result<()> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "dict_extend receiver is not a dict".to_string(),
            )
        })?;
        rc.borrow_mut().extend(snapshot);
        Ok(())
    }

    /// Unified int accessor (handles inline i48 and PyBigInt that fits in i64)
    pub fn as_int(&self) -> Option<i64> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_int() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        match top16(self.0) {
            TAG_INT => Some(self.as_int_raw()),
            TAG_OPAQUE => {
                if let Opaque::PyBigInt(rc) = unsafe { &*self.opaque_ptr() } {
                    rc.to_i64()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    // ── kind() — borrow-based view for pattern matching ──────────────────────

    pub fn kind(&self) -> ValueKind<'_> {
        // Catch reads of uninitialised register slots early.  In debug builds
        // this panics with a diagnostic message so the bug surfaces immediately
        // rather than silently propagating a NaN through the program.  Release
        // builds elide the assert (zero cost on the hot path).
        debug_assert!(
            !self.is_unset(),
            "Value::kind() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        // Reserved NaN-box sentinels: check before the float arm so they
        // don't get classified as float NaNs.
        if self.0 == NOT_IMPLEMENTED_BITS {
            return ValueKind::NotImplemented;
        }
        if self.0 == ELLIPSIS_BITS {
            return ValueKind::Ellipsis;
        }
        match top16(self.0) {
            t if t <= TAG_FLOAT_MAX => ValueKind::Float(self.as_float_raw()),
            TAG_NONE => ValueKind::None,
            TAG_BOOL => ValueKind::Bool(self.as_bool()),
            TAG_INT => ValueKind::Int(self.as_int_raw()),
            TAG_STR => ValueKind::Str(unsafe { self.str_as_str() }),
            TAG_TUPLE => ValueKind::Tuple(unsafe { &*self.tuple_ptr() }),
            // List/Dict/Set views: take a scoped `RefCell::borrow()` so
            // the cell's runtime borrow check is *honoured*.  A
            // concurrent `borrow_mut()` while the resulting ValueKind
            // is alive will panic with the standard already-borrowed
            // message — strictly safer than the previous
            // `unsafe { &*cell.as_ptr() }` bypass which produced silent
            // UB (#450).
            TAG_LIST => {
                let inner = unsafe { self.list_inner() };
                ValueKind::List(inner.items.borrow())
            }
            TAG_OPAQUE => match unsafe { &*self.opaque_ptr() } {
                Opaque::PyBigInt(rc) => {
                    if let Some(n) = rc.to_i64() {
                        ValueKind::Int(n)
                    } else {
                        ValueKind::BigInt(rc.as_ref())
                    }
                }
                Opaque::Dict(rc) => ValueKind::Dict(rc.as_ref().borrow()),
                Opaque::Set(rc) => ValueKind::Set(rc.items.borrow()),
                Opaque::Range { start, stop, step } => ValueKind::Range {
                    start: *start,
                    stop: *stop,
                    step: *step,
                },
                Opaque::UserFunction(f) => match f.kind {
                    UserFunctionKind::Builtin(name) => ValueKind::BuiltinFunction(name),
                    _ => ValueKind::UserFunction(f),
                },
                Opaque::PyClass(c) => ValueKind::PyClass(c),
                Opaque::PyInstance(i) => ValueKind::PyInstance(i),
                Opaque::PyModule(m) => ValueKind::PyModule(m),
                Opaque::BoundMethod {
                    function, receiver, ..
                } => ValueKind::BoundMethod { function, receiver },
                Opaque::ClassBoundMethod {
                    function, class, ..
                } => ValueKind::ClassBoundMethod { function, class },
                Opaque::SuperProxy {
                    class, instance, ..
                } => ValueKind::SuperProxy { class, instance },
                Opaque::SuperProxyClass {
                    class, obj_class, ..
                } => ValueKind::SuperProxyClass { class, obj_class },
                Opaque::Generator(state) => ValueKind::Generator(state),
                Opaque::Bytes(rc) => ValueKind::Bytes(rc),
                Opaque::Complex(re, im) => ValueKind::Complex(*re, *im),
                // Inline small tuples surface as `ValueKind::Tuple(&[Value])`
                // so all existing match arms keep working without learning
                // about the new variant.  See #281.
                Opaque::SmallTuple2 { items, .. } => ValueKind::Tuple(&items[..]),
                Opaque::SmallTuple3 { items, .. } => ValueKind::Tuple(&items[..]),
                Opaque::BuiltinObject { ops, state } => {
                    ValueKind::BuiltinObject { ops: *ops, state }
                }
            },
            _ => unreachable!(),
        }
    }

    // ── Existing Value methods rewritten with kind() ─────────────────────────

    pub fn truthy(&self) -> bool {
        match self.kind() {
            ValueKind::Bool(v) => v,
            ValueKind::Int(v) => v != 0,
            ValueKind::BigInt(v) => !v.is_zero(),
            ValueKind::Float(v) => v != 0.0,
            ValueKind::Str(v) => !v.is_empty(),
            ValueKind::None => false,
            ValueKind::List(v) => !v.is_empty(),
            ValueKind::Dict(v) => !v.is_empty(),
            ValueKind::Set(v) => !v.is_empty(),
            ValueKind::Range { start, stop, step } => range_len(start, stop, step) > 0,
            ValueKind::UserFunction(_) => true,
            ValueKind::BuiltinFunction(_) => true,
            ValueKind::PyClass(_) => true,
            ValueKind::PyInstance(_) => true,
            ValueKind::BoundMethod { .. } => true,
            ValueKind::PyModule(_) => true,
            ValueKind::Tuple(v) => !v.is_empty(),
            ValueKind::ClassBoundMethod { .. } => true,
            ValueKind::SuperProxy { .. } => true,
            ValueKind::SuperProxyClass { .. } => true,
            ValueKind::Generator(_) => true,
            ValueKind::NotImplemented => true,
            ValueKind::Ellipsis => true,
            // (NaN-box pattern handled by kind() dispatch above; included
            // in this match for completeness.)
            ValueKind::Bytes(b) => !b.is_empty(),
            ValueKind::Complex(re, im) => re != 0.0 || im != 0.0,
            ValueKind::BuiltinObject { ops, state } => ops.truthy(state),
        }
    }

    pub fn to_py_str(&self) -> String {
        match self.kind() {
            ValueKind::PyInstance(instance) if is_exception_instance(instance) => {
                exception_to_string(instance)
            }
            ValueKind::Str(s) => s.to_string(),
            _ => self.repr(),
        }
    }

    pub fn repr(&self) -> String {
        match self.kind() {
            ValueKind::Int(v) => v.to_string(),
            ValueKind::BigInt(v) => v.to_string(),
            ValueKind::Float(v) => format_float(v),
            ValueKind::Str(v) => {
                let q = repr_quote(v);
                format!("{}{}{}", q, escape_str(v, q), q)
            }
            ValueKind::Bool(v) => {
                if v {
                    "True".to_string()
                } else {
                    "False".to_string()
                }
            }
            ValueKind::None => "None".to_string(),
            ValueKind::Ellipsis => "Ellipsis".to_string(),
            ValueKind::List(items) => {
                // Cycle detection (#364): if the same list is already being
                // formatted further up the call stack, emit CPython's
                // placeholder `[...]` instead of recursing into ourselves.
                let id = self.value_id();
                let _guard = match id {
                    Some(id) => match ReprGuard::enter(id) {
                        Some(g) => Some(g),
                        None => return "[...]".to_string(),
                    },
                    None => None,
                };
                let inner = items
                    .iter()
                    .map(|v| v.repr())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{inner}]")
            }
            ValueKind::Dict(items) => {
                // Cycle detection (#364): self-referential dicts (via a value)
                // are reported as `{...}` by CPython.
                let id = self.value_id();
                let _guard = match id {
                    Some(id) => match ReprGuard::enter(id) {
                        Some(g) => Some(g),
                        None => return "{...}".to_string(),
                    },
                    None => None,
                };
                let mut out = String::new();
                out.push('{');
                for (i, (k, v)) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&key_repr(k));
                    out.push_str(": ");
                    out.push_str(&v.repr());
                }
                out.push('}');
                out
            }
            ValueKind::Set(items) => {
                if items.is_empty() {
                    return "set()".to_string();
                }
                // Cycle detection (#364): a set can only hold hashable values,
                // and the cycle-producing collections (list/dict/set) aren't
                // hashable, so a true set self-cycle is impossible.  Keep the
                // guard anyway for defence-in-depth — the cost is one
                // thread-local lookup per set repr.
                let id = self.value_id();
                let _guard = match id {
                    Some(id) => match ReprGuard::enter(id) {
                        Some(g) => Some(g),
                        None => return "{...}".to_string(),
                    },
                    None => None,
                };
                let inner = items.iter().map(key_repr).collect::<Vec<_>>().join(", ");
                format!("{{{inner}}}")
            }
            ValueKind::Range { start, stop, step } => {
                if step == 1 {
                    format!("range({start}, {stop})")
                } else {
                    format!("range({start}, {stop}, {step})")
                }
            }
            ValueKind::BuiltinFunction(name) => format!("<built-in function {name}>"),
            ValueKind::UserFunction(func) => match func.kind {
                UserFunctionKind::ClassMethod => format!("<classmethod '{}'>", func.name),
                UserFunctionKind::StaticMethod => format!("<staticmethod '{}'>", func.name),
                UserFunctionKind::Regular => format!("<function {}>", func.name),
                // Builtins are surfaced via `ValueKind::BuiltinFunction` by
                // `kind()`, so we never reach this arm — but the match is
                // total either way.
                UserFunctionKind::Builtin(name) => format!("<built-in function {name}>"),
            },
            ValueKind::PyClass(class) => {
                let (qualname, module) = {
                    let c = class.borrow();
                    let qualname = c.qualname.clone();
                    let module = c
                        .attrs
                        .get("__module__")
                        .and_then(|v| v.as_str().map(|s| s.to_string()));
                    (qualname, module)
                };
                match module.as_deref() {
                    Some("builtins") | None => format!("<class '{qualname}'>"),
                    Some(m) => format!("<class '{m}.{qualname}'>"),
                }
            }
            ValueKind::PyInstance(instance) => {
                if is_exception_instance(instance) {
                    return exception_repr(instance);
                }
                let (qualname, module) = {
                    let inst = instance.borrow();
                    let class = inst.class.borrow();
                    let qualname = class.qualname.clone();
                    let module = class
                        .attrs
                        .get("__module__")
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| "__main__".to_string());
                    (qualname, module)
                };
                let addr = Rc::as_ptr(instance) as usize;
                format!("<{module}.{qualname} object at 0x{addr:x}>")
            }
            ValueKind::BoundMethod { function, receiver } => {
                let class_name = receiver.borrow().class.borrow().name.clone();
                format!("<bound method {class_name}.{}>", function.name)
            }
            ValueKind::PyModule(m) => format!("<module '{}'>", m.borrow().name),
            ValueKind::Tuple(items) => {
                // Cycle detection (#364): tuples are immutable so a *direct*
                // self-cycle isn't constructible from Python, but a tuple can
                // hold a list that holds the tuple — and the recursion still
                // passes through here.  CPython emits `(...)` for a tuple
                // self-cycle; we match that.
                let id = self.value_id();
                let _guard = match id {
                    Some(id) => match ReprGuard::enter(id) {
                        Some(g) => Some(g),
                        None => return "(...)".to_string(),
                    },
                    None => None,
                };
                let inner = items
                    .iter()
                    .map(|v| v.repr())
                    .collect::<Vec<_>>()
                    .join(", ");
                if items.len() == 1 {
                    format!("({inner},)")
                } else {
                    format!("({inner})")
                }
            }
            ValueKind::ClassBoundMethod { function, class } => {
                format!("<bound method {}.{}>", class.borrow().name, function.name)
            }
            ValueKind::SuperProxy { class, .. } => {
                format!("<super: <class '{}'>>", class.borrow().name)
            }
            ValueKind::SuperProxyClass { class, .. } => {
                format!("<super: <class '{}'>>", class.borrow().name)
            }
            ValueKind::Generator(_) => "<generator object>".to_string(),
            ValueKind::NotImplemented => "NotImplemented".to_string(),
            ValueKind::Bytes(rc) => bytes_repr(rc),
            ValueKind::Complex(re, im) => complex_repr(re, im),
            ValueKind::BuiltinObject { ops, state } => ops.repr(state),
        }
    }

    pub fn to_key(&self) -> Option<PyKey> {
        match self.kind() {
            ValueKind::Int(v) => Some(PyKey::Int(v)),
            ValueKind::BigInt(v) => v
                .to_i64()
                .map(PyKey::Int)
                .or_else(|| Some(PyKey::BigInt(Box::new(v.clone())))),
            ValueKind::Float(v) => Some(PyKey::Float(v.to_bits())),
            ValueKind::Str(_) => Some(PyKey::Str(self.clone())),
            ValueKind::Bool(v) => Some(PyKey::Bool(v)),
            ValueKind::None => Some(PyKey::None),
            ValueKind::Ellipsis => Some(PyKey::Ellipsis),
            ValueKind::Bytes(rc) => Some(PyKey::Bytes(Rc::clone(rc))),
            // Complex with zero imaginary part maps to PyKey::Float(re.to_bits()) so
            // that cross-type equality (1+0j == 1 == 1.0) is handled by the existing
            // Float <-> Int arms in PyKey::PartialEq without extra special-casing.
            // -0.0 imaginary is treated as zero (IEEE 754: -0.0 == 0.0).
            ValueKind::Complex(re, im) => {
                if im == 0.0 {
                    Some(PyKey::Float(re.to_bits()))
                } else {
                    Some(PyKey::Complex(re, im))
                }
            }
            ValueKind::BuiltinObject { ops, state } => ops.to_key(state),
            ValueKind::Tuple(items) => {
                // Recursively hash each element.  If any element is itself
                // unhashable (e.g. a list inside the tuple), the whole tuple
                // is unhashable — matches CPython's `hash((1, [2]))` raising
                // TypeError.
                let mut keys = Vec::with_capacity(items.len());
                for item in items {
                    keys.push(item.to_key()?);
                }
                Some(PyKey::Tuple(keys))
            }
            _ => None,
        }
    }
}

// ── Clone ─────────────────────────────────────────────────────────────────────

impl Clone for Value {
    fn clone(&self) -> Self {
        match top16(self.0) {
            // Primitives: just copy bits
            t if t <= TAG_INT => Value(self.0),
            // Str
            TAG_STR => {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u32;
                unsafe {
                    // rc is stored in bits 31:1; increment by 2 (the type bit stays in bit 0).
                    // Saturate instead of wrapping: a saturated rc means we never free the
                    // backing buffer (acceptable memory leak for absurdly-shared strings).
                    let old = *hdr;
                    *hdr = old.saturating_add(2);
                } // rc++ (bits 31:1)
                Value(self.0) // same bits, 0 allocations
            }
            // Tuple — copy the stored obj_id so the clone shares the same identity
            TAG_TUPLE => {
                let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
                let obj_id = unsafe { *(hdr.add(24) as *const u64) };
                let v = unsafe { &*self.tuple_ptr() };
                Value::tuple_with_id(v.clone(), obj_id)
            }
            // List — share the backing Rc<ListInner> with the original so that
            // mutations through any alias propagate to all clones (#305).  The
            // NaN-box pattern is reused directly; we only bump the strong
            // count to keep the Rc alive.  `obj_id` is inherent to the shared
            // `ListInner`, so identity (`id()`) is automatically stable.
            TAG_LIST => {
                unsafe {
                    Rc::increment_strong_count(self.list_inner_ptr());
                }
                Value(self.0)
            }
            // Opaque
            TAG_OPAQUE => {
                let o = unsafe { &*self.opaque_ptr() };
                Value::opaque(o.clone())
            }
            _ => unreachable!(),
        }
    }
}

// ── Drop ──────────────────────────────────────────────────────────────────────

impl Drop for Value {
    fn drop(&mut self) {
        match top16(self.0) {
            t if t <= TAG_INT => {} // primitives: no heap
            TAG_STR => unsafe {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u8;
                let rc_type_ptr = hdr as *mut u32;
                *rc_type_ptr -= 2; // rc--
                if *rc_type_ptr >> 1 == 0 {
                    // rc reached 0
                    if *rc_type_ptr & 1 == 0 {
                        // Layout A: [rc_type:u32][sub_len:u32][ref:*mut u8][bytes...]
                        let len = *(hdr.add(4) as *const u32) as usize;
                        dealloc(hdr, Layout::from_size_align(16 + len, 8).unwrap());
                    } else {
                        // Layout B: [rc_type:u32][sub_len:u32][ref:*mut u8][offset:u32]
                        // A_ptr = ref - offset - 16
                        let ref_ptr = *(hdr.add(8) as *const *mut u8);
                        let offset = *(hdr.add(16) as *const u32) as usize;
                        let a_ptr = ref_ptr.sub(offset + 16);
                        *(a_ptr as *mut u32) -= 2; // A.rc--
                        if *(a_ptr as *const u32) >> 1 == 0 {
                            let root_len = *(a_ptr.add(4) as *const u32) as usize;
                            dealloc(a_ptr, Layout::from_size_align(16 + root_len, 8).unwrap());
                        }
                        pool_b_dealloc(hdr);
                    }
                }
            },
            TAG_TUPLE => unsafe {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u8;
                std::ptr::drop_in_place(hdr as *mut Vec<Value>);
                pool_vec_hdr_dealloc(hdr);
            },
            // List — decrement the Rc strong count; the Rc layer drops the
            // underlying `ListInner` (and its `Vec<Value>`) when the count
            // reaches zero.  Pool allocations from the pre-#305 layout are
            // gone — the Rc-allocated block is freed by the standard
            // allocator, not the pool.
            TAG_LIST => unsafe {
                Rc::decrement_strong_count(self.list_inner_ptr());
            },
            TAG_OPAQUE => unsafe {
                // Matched allocator: `Value::opaque` allocates through
                // `pool_opaque_alloc`; drop the contained value in place and
                // hand the slab back to the same pool.  See #281.
                let ptr = self.opaque_ptr();
                std::ptr::drop_in_place(ptr);
                pool_opaque_dealloc(ptr as *mut u8);
            },
            _ => unreachable!(),
        }
    }
}

// ── PartialEq helpers ─────────────────────────────────────────────────────────

/// Exact equality between an `i64` integer and an `f64` float.
///
/// CPython's int-float comparison converts the float to its exact integer
/// representation and compares, rather than converting the int to float (which
/// loses precision beyond 2^53).  Concretely: `2**53 + 1 == float(2**53 + 1)`
/// is `False` in CPython because `float(2**53 + 1)` rounds to `2**53`.
///
/// The algorithm:
/// 1. If `f` is not finite, return `false`.
/// 2. If `f` has a fractional part, return `false` (no integer can equal it).
/// 3. If `f` is outside the `i64` value range, return `false`.
/// 4. Otherwise convert `f` to `i64` exactly (safe: `f` is finite,
///    integer-valued, and in range) and compare.
///
/// Step 4 is the key insight: `f as i64` gives the *float's* exact integer
/// value, not a lossy round-trip through `i as f64`.
fn int_float_eq(i: i64, f: f64) -> bool {
    if !f.is_finite() || f != f.trunc() {
        return false;
    }
    // 9223372036854775808.0 is 2^63, the smallest f64 strictly greater than
    // i64::MAX.  Any finite integer-valued float in [i64::MIN, 2^63) is
    // safely representable as i64.
    const I64_MAX_PLUS_ONE: f64 = 9_223_372_036_854_775_808.0_f64;
    if f < (i64::MIN as f64) || f >= I64_MAX_PLUS_ONE {
        return false;
    }
    (f as i64) == i
}

/// Exact equality between a `BigInt` and an `f64` float.
///
/// Mirrors `int_float_eq` for arbitrarily large integers.  Only finite,
/// integer-valued floats can equal a `BigInt`; anything else (NaN, infinity,
/// or a fractional value like 1.2) returns `false` immediately.
///
/// We must guard with `f.is_finite() && f == f.trunc()` before calling
/// `BigInt::from_f64`, because `from_f64` **truncates** fractional floats
/// (e.g. 1.2 → BigInt(1)) rather than returning `None`, which would make
/// `bigint_float_eq(&BigInt::from(1), 1.2)` incorrectly return `true`.
fn bigint_float_eq(big: &BigInt, f: f64) -> bool {
    if !f.is_finite() || f != f.trunc() {
        return false;
    }
    match BigInt::from_f64(f) {
        Some(f_as_bigint) => f_as_bigint == *big,
        None => false,
    }
}

// ── PartialEq ─────────────────────────────────────────────────────────────────

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        // Cycle detection (#364): for collection kinds, check whether we're
        // already comparing this exact pair further up the call stack.  If we
        // are we've hit a cycle; we treat cyclic equality as true (the
        // recursion bottoms out as "we've already proven the prefix equal").
        //
        // We only consult the guard when *both* sides are cycle-capable
        // collection kinds — primitives can't form cycles and shouldn't pay
        // the thread-local lookup.
        let _eq_guard = match (self.kind(), other.kind()) {
            (ValueKind::List(_), ValueKind::List(_))
            | (ValueKind::Dict(_), ValueKind::Dict(_))
            | (ValueKind::Set(_), ValueKind::Set(_))
            | (ValueKind::Tuple(_), ValueKind::Tuple(_)) => {
                match (self.value_id(), other.value_id()) {
                    (Some(a_id), Some(b_id)) => match EqGuard::enter(a_id, b_id) {
                        Some(g) => Some(g),
                        None => return true,
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        match (self.kind(), other.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => a == b,
            // Python: 1 == 1.0 is True, but 2**53+1 == float(2**53+1) is False.
            // Convert the float to its exact integer value (not the int to float)
            // to avoid precision loss for values beyond the 53-bit mantissa.
            (ValueKind::Int(a), ValueKind::Float(b)) => int_float_eq(a, b),
            (ValueKind::Float(a), ValueKind::Int(b)) => int_float_eq(b, a),
            (ValueKind::Float(a), ValueKind::Float(b)) => a == b,
            (ValueKind::BigInt(a), ValueKind::BigInt(b)) => a == b,
            (ValueKind::BigInt(a), ValueKind::Int(b)) => *a == BigInt::from(b),
            (ValueKind::Int(a), ValueKind::BigInt(b)) => BigInt::from(a) == *b,
            (ValueKind::BigInt(a), ValueKind::Float(b)) => bigint_float_eq(a, b),
            (ValueKind::Float(a), ValueKind::BigInt(b)) => bigint_float_eq(b, a),
            (ValueKind::Str(a), ValueKind::Str(b)) => a == b,
            (ValueKind::Bool(a), ValueKind::Bool(b)) => a == b,
            // Python: True == 1 is True
            (ValueKind::Bool(a), ValueKind::Int(b)) => (a as i64) == b,
            (ValueKind::Int(a), ValueKind::Bool(b)) => a == (b as i64),
            // Python: True == 1.0 is True
            (ValueKind::Bool(a), ValueKind::Float(b)) => (a as u8 as f64) == b,
            (ValueKind::Float(a), ValueKind::Bool(b)) => a == (b as u8 as f64),
            (ValueKind::None, ValueKind::None) => true,
            (ValueKind::Ellipsis, ValueKind::Ellipsis) => true,
            // `Ref<T>` doesn't impl `==` directly — deref to compare the
            // underlying containers.  `*a == *b` calls Vec/IndexMap/IndexSet's
            // `PartialEq`, which is what we want.
            (ValueKind::List(a), ValueKind::List(b)) => *a == *b,
            (ValueKind::Tuple(a), ValueKind::Tuple(b)) => a == b,
            (ValueKind::Dict(a), ValueKind::Dict(b)) => *a == *b,
            (ValueKind::Set(a), ValueKind::Set(b)) => *a == *b,
            (ValueKind::Bytes(a), ValueKind::Bytes(b)) => a.as_ref() == b.as_ref(),
            (ValueKind::Complex(ar, ai), ValueKind::Complex(br, bi)) => ar == br && ai == bi,
            (ValueKind::Int(n), ValueKind::Complex(br, bi)) => (n as f64) == br && bi == 0.0,
            (ValueKind::Complex(ar, ai), ValueKind::Int(n)) => ar == (n as f64) && ai == 0.0,
            (ValueKind::Float(f), ValueKind::Complex(br, bi)) => f == br && bi == 0.0,
            (ValueKind::Complex(ar, ai), ValueKind::Float(f)) => ar == f && ai == 0.0,
            (
                ValueKind::Range {
                    start: as_,
                    stop: ao,
                    step: at,
                },
                ValueKind::Range {
                    start: bs,
                    stop: bo,
                    step: bt,
                },
            ) => as_ == bs && ao == bo && at == bt,
            (ValueKind::BuiltinFunction(a), ValueKind::BuiltinFunction(b)) => a == b,
            (ValueKind::UserFunction(a), ValueKind::UserFunction(b)) => Rc::ptr_eq(a, b),
            (ValueKind::PyClass(a), ValueKind::PyClass(b)) => Rc::ptr_eq(a, b),
            (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) => Rc::ptr_eq(a, b),
            (ValueKind::PyModule(a), ValueKind::PyModule(b)) => Rc::ptr_eq(a, b),
            (
                ValueKind::BoundMethod {
                    function: af,
                    receiver: ar,
                },
                ValueKind::BoundMethod {
                    function: bf,
                    receiver: br,
                },
            ) => Rc::ptr_eq(af, bf) && Rc::ptr_eq(ar, br),
            // Built-in objects dispatch equality through their ops trait so
            // pyrust-core never names a concrete built-in type.  Try both
            // directions so e.g. `frozenset == set` and `set == frozenset`
            // both reach the frozenset impl.
            (ValueKind::BuiltinObject { ops, state }, _) => ops.eq(state, other),
            (_, ValueKind::BuiltinObject { ops, state }) => ops.eq(state, self),
            _ => false,
        }
    }
}

// ── Display / Debug ───────────────────────────────────────────────────────────

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_py_str())
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.repr())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper free functions
// ─────────────────────────────────────────────────────────────────────────────

// `unalias_args_for_mutation` was removed in #448.  Its job was to
// satisfy the manual aliasing-safety contract that `as_list_mut` /
// `as_dict_mut` / `as_set_mut` documented.  With those accessors gone
// and the new scoped-borrow API in place (`list_with_mut`,
// `dict_with_mut`, `set_with_mut`, …), the dispatcher no longer holds
// a `&mut <storage>` across calls into the builtin, so no aliasing
// window exists to pre-empt.

/// Returns the Python built-in type name (e.g. `"list"`, `"str"`) for a
/// `Value`.  Used by error messages (`'X' object is not iterable`, attribute
/// errors), built-in method repr strings (`<built-in method append of list
/// object>`), and similar diagnostics.
///
/// This is the canonical implementation — every crate in the workspace
/// routes type-name lookup through this function so naming stays consistent.
/// The match is exhaustive over [`ValueKind`]; new variants must be added
/// here, not in per-crate copies.
/// Python-visible type name for a value, used in error messages and `type(x).__name__`.
///
/// Returns `Cow<'static, str>` so the common builtin arms stay zero-allocation
/// (`Cow::Borrowed`), while `PyInstance` can honestly report its runtime class
/// name (`Cow::Owned`) instead of the placeholder `"object"` (issue #437).
pub fn builtin_type_name(value: &Value) -> Cow<'static, str> {
    match value.kind() {
        ValueKind::None => Cow::Borrowed("NoneType"),
        ValueKind::Bool(_) => Cow::Borrowed("bool"),
        ValueKind::Int(_) | ValueKind::BigInt(_) => Cow::Borrowed("int"),
        ValueKind::Float(_) => Cow::Borrowed("float"),
        ValueKind::Str(_) => Cow::Borrowed("str"),
        ValueKind::List(_) => Cow::Borrowed("list"),
        ValueKind::Tuple(_) => Cow::Borrowed("tuple"),
        ValueKind::Dict(_) => Cow::Borrowed("dict"),
        ValueKind::Set(_) => Cow::Borrowed("set"),
        ValueKind::Range { .. } => Cow::Borrowed("range"),
        ValueKind::Bytes(_) => Cow::Borrowed("bytes"),
        ValueKind::Complex(_, _) => Cow::Borrowed("complex"),
        ValueKind::BuiltinFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. } => Cow::Borrowed("function"),
        ValueKind::UserFunction(f) => match f.kind {
            UserFunctionKind::StaticMethod => Cow::Borrowed("staticmethod"),
            UserFunctionKind::ClassMethod => Cow::Borrowed("classmethod"),
            _ => Cow::Borrowed("function"),
        },
        ValueKind::PyClass(_) => Cow::Borrowed("type"),
        ValueKind::PyInstance(inst) => Cow::Owned(inst.borrow().class.borrow().name.clone()),
        ValueKind::PyModule(_) => Cow::Borrowed("module"),
        ValueKind::SuperProxy { .. } | ValueKind::SuperProxyClass { .. } => Cow::Borrowed("super"),
        ValueKind::Generator(_) => Cow::Borrowed("generator"),
        ValueKind::NotImplemented => Cow::Borrowed("NotImplementedType"),
        ValueKind::Ellipsis => Cow::Borrowed("ellipsis"),
        ValueKind::BuiltinObject { ops, .. } => Cow::Borrowed(ops.type_name()),
    }
}

/// Render a `bytes` value the way Python does (b'...' with escapes).
fn bytes_repr(bytes: &[u8]) -> String {
    // Choose a quote: if any single quote and no double quote, use double; else single.
    let has_single = bytes.contains(&b'\'');
    let has_double = bytes.contains(&b'"');
    let q = if has_single && !has_double { '"' } else { '\'' };
    let mut out = String::with_capacity(bytes.len() + 3);
    out.push('b');
    out.push(q);
    for &b in bytes {
        match b {
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x5c => out.push_str("\\\\"),
            b'\'' if q == '\'' => out.push_str("\\'"),
            b'"' if q == '"' => out.push_str("\\\""),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(q);
    out
}

/// Format a single complex component the way CPython's repr does:
///   - integer-valued floats with |v| < 1e16 → `"3"` (no `.0`)
///   - |v| >= 1e16 → scientific notation `"1e+20"` (Python style)
///   - NaN / inf via `format_float`
///   - everything else → standard float repr
///
/// Python uses scientific notation for absolute values >= 1e16 (where i64
/// rounding would lose precision) and for very small non-zero values; we
/// mirror that boundary.
fn complex_component(v: f64) -> String {
    if !v.is_finite() {
        return format_float(v);
    }
    let abs = v.abs();
    if v == v.trunc() && abs < 1e16 {
        // -0.0 as i64 yields 0, losing the sign.  Preserve it explicitly.
        if v == 0.0 && v.is_sign_negative() {
            return "-0".to_string();
        }
        return format!("{}", v as i64);
    }
    if abs >= 1e16 || (abs != 0.0 && abs < 1e-4) {
        // Rust's `{:e}` produces "1e20"; CPython prints "1e+20". Patch the sign.
        let raw = format!("{v:e}");
        if let Some(idx) = raw.find('e') {
            let (mantissa, exp) = raw.split_at(idx);
            let exp = &exp[1..]; // skip 'e'
            if let Some(stripped) = exp.strip_prefix('-') {
                return format!("{mantissa}e-{stripped:0>2}");
            }
            return format!("{mantissa}e+{exp:0>2}");
        }
        return raw;
    }
    format_float(v)
}

/// Format a complex number the way Python does:
///   `1j`, `(2+3j)`, `(2-3j)`, `(-1+0j)`, etc.
fn complex_repr(re: f64, im: f64) -> String {
    let im_str = complex_component(im);
    if re == 0.0 && (1.0_f64).copysign(re) > 0.0 {
        return format!("{im_str}j");
    }
    let re_str = complex_component(re);
    let sep = if im < 0.0 || (im == 0.0 && im.is_sign_negative()) {
        ""
    } else {
        "+"
    };
    format!("({re_str}{sep}{im_str}j)")
}

/// Canonical CPython-parity `repr()` for a `PyKey`.
///
/// This is the single source of truth for hashable-key reprs across the
/// workspace.  Consumers in `pyrust-builtins` (frozenset, dict views) and
/// `pyrust` (collections.deque) all route through this function rather than
/// keeping local copies — see issue #422 for the divergence that motivated
/// consolidation (whole-number floats were losing their trailing `.0` in the
/// frozenset path because the copy there used `f64::to_string` instead of
/// `format_float`).
///
/// Umbrella tracking: #420.
pub fn key_repr(key: &PyKey) -> String {
    match key {
        PyKey::Int(v) => v.to_string(),
        PyKey::BigInt(v) => v.to_string(),
        PyKey::Float(v) => format_float(f64::from_bits(*v)),
        PyKey::Str(v) => {
            let s = v.as_str().unwrap_or("");
            let q = repr_quote(s);
            format!("{}{}{}", q, escape_str(s, q), q)
        }
        PyKey::Bool(v) => {
            if *v {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        PyKey::None => "None".to_string(),
        PyKey::Ellipsis => "Ellipsis".to_string(),
        PyKey::FrozenSet(items) => {
            if items.is_empty() {
                "frozenset()".to_string()
            } else {
                let inner = items.iter().map(key_repr).collect::<Vec<_>>().join(", ");
                format!("frozenset({{{inner}}})")
            }
        }
        PyKey::Tuple(items) => {
            if items.is_empty() {
                "()".to_string()
            } else if items.len() == 1 {
                format!("({},)", key_repr(&items[0]))
            } else {
                let inner = items.iter().map(key_repr).collect::<Vec<_>>().join(", ");
                format!("({inner})")
            }
        }
        PyKey::Bytes(b) => bytes_repr(b),
        PyKey::Complex(re, im) => complex_repr(*re, *im),
        PyKey::Object { value, .. } => value.repr(),
    }
}

pub fn is_exception_instance(instance: &Rc<RefCell<PyInstance>>) -> bool {
    let class = Rc::clone(&instance.borrow().class);
    class_chain_contains_exception(&class)
}

/// Canonical "is this class an exception?" predicate, shared by the runtime
/// (`raise`/`except` machinery) and `Value::repr`/`Value::str` for
/// `PyInstance`.  Both paths must agree, or `raise X(...)` succeeds while
/// `repr(X(...))` falls back to the default `<X object>` formatting (issue
/// #429).
///
/// `BaseException` is the root of the CPython exception hierarchy (#574).
/// Any class whose base chain reaches `"BaseException"` (or the legacy
/// sentinels `"Exception"` / `"GeneratorExit"` kept for backward compat) is
/// treated as an exception class.  See
/// [`crate::interpreter::helpers::install_exception_builtins`] in the
/// `pyrust` crate for where the classes are constructed.
pub fn class_chain_contains_exception(class: &Rc<RefCell<PyClass>>) -> bool {
    let (name, base, extra_bases) = {
        let borrowed = class.borrow();
        (
            borrowed.name.clone(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    if name == "BaseException" || name == "Exception" || name == "GeneratorExit" {
        return true;
    }
    if base.is_some_and(|base| class_chain_contains_exception(&base)) {
        return true;
    }
    extra_bases
        .iter()
        .any(|b| class_chain_contains_exception(b))
}

/// Walk the class base chain and return `true` if any class in the chain has
/// the given `name`.  Used by `PyError::class_name_is` to handle subclasses
/// of `StopIteration` / `GeneratorExit` carried as `PyError::Raised`.
fn class_chain_has_name(class: &Rc<RefCell<PyClass>>, name: &str) -> bool {
    let (class_name, base, extra_bases) = {
        let borrowed = class.borrow();
        (
            borrowed.name.clone(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    if class_name == name {
        return true;
    }
    if base.is_some_and(|base| class_chain_has_name(&base, name)) {
        return true;
    }
    extra_bases.iter().any(|b| class_chain_has_name(b, name))
}

fn exception_args(instance: &Rc<RefCell<PyInstance>>) -> Vec<Value> {
    match instance.borrow().attrs.get("args").map(|v| v.kind()) {
        Some(ValueKind::Tuple(args)) => args.to_vec(),
        _ => Vec::new(),
    }
}

fn format_exception_args(args: &[Value], repr_mode: bool) -> String {
    match args {
        [] => String::new(),
        [value] => {
            if repr_mode {
                value.repr()
            } else {
                value.to_py_str()
            }
        }
        _ => {
            let inner = args
                .iter()
                .map(|value| value.repr())
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
    }
}

fn exception_to_string(instance: &Rc<RefCell<PyInstance>>) -> String {
    let args = exception_args(instance);
    // CPython's `KeyError.__str__` always uses repr of the single arg, so
    // `str(KeyError('x'))` returns `"'x'"` (one level of quoting).  All other
    // exception classes use `str()` of the arg (no extra quoting).
    let is_key_error = class_chain_has_name(&instance.borrow().class, "KeyError");

    // CPython's `OSError.__str__` (and all subclasses) formats as
    // "[Errno N] strerror" or "[Errno N] strerror: repr(filename)" when
    // the instance was constructed with 2+ args (i.e. errno/strerror C slots
    // were initialised by `OSError.__init__`).  The format is used regardless
    // of whether those attributes were subsequently set to None from Python —
    // CPython's `OSError_str` (Objects/exceptions.c) checks the C member
    // pointers for NULL (never-initialised) rather than for Py_None
    // (explicitly-set-to-None), and `args.len() >= 2` is the pyrust proxy for
    // "the C slots were initialised".
    // With the 5-arg form, if filename2 is also non-None: "... -> repr(filename2)".
    if class_chain_has_name(&instance.borrow().class, "OSError") && args.len() >= 2 {
        let borrowed = instance.borrow();
        let errno_val = borrowed.attrs.get("errno");
        let strerror_val = borrowed.attrs.get("strerror");
        let filename_val = borrowed.attrs.get("filename");
        let filename2_val = borrowed.attrs.get("filename2");
        if let (Some(errno), Some(strerror)) = (errno_val, strerror_val) {
            let base = format!("[Errno {}] {}", errno.to_py_str(), strerror.to_py_str());
            match filename_val {
                Some(fname) if !fname.is_none() => {
                    let with_fname = format!("{}: {}", base, fname.repr());
                    match filename2_val {
                        Some(fname2) if !fname2.is_none() => {
                            return format!("{} -> {}", with_fname, fname2.repr());
                        }
                        _ => return with_fname,
                    }
                }
                _ => return base,
            }
        }
    }

    format_exception_args(&args, is_key_error)
}

fn exception_repr(instance: &Rc<RefCell<PyInstance>>) -> String {
    let class_name = instance.borrow().class.borrow().name.clone();
    let args = exception_args(instance);
    if args.is_empty() {
        format!("{class_name}()")
    } else {
        // CPython's BaseException.__repr__ renders all args comma-separated
        // inside the class-name parens: `ExcName(repr(a0), repr(a1), ...)`.
        // Do NOT use `format_exception_args` here — its multi-arg branch wraps
        // in an extra pair of parens (`"(a, b)"`), which produces the wrong
        // `ExcName((a, b))` instead of `ExcName(a, b)`.
        let inner = args.iter().map(|v| v.repr()).collect::<Vec<_>>().join(", ");
        format!("{class_name}({inner})")
    }
}

/// Choose the quote character for a string repr.  CPython prefers single
/// quotes; it switches to double quotes when the string contains a single
/// quote but no double quote (avoids backslash escapes in the common case).
fn repr_quote(s: &str) -> char {
    if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    }
}

fn escape_str(s: &str, quote: char) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if !py_is_printable(c) => {
                let n = c as u32;
                if n <= 0xFF {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n <= 0xFFFF {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Returns `true` when `c` is considered "printable" by Python's
/// `str.isprintable()` / CPython's `Py_UNICODE_ISPRINTABLE`.
///
/// CPython considers a character non-printable when its Unicode general
/// category is one of: Cc (control), Cf (format), Cs (surrogate),
/// Co (private-use), Cn (unassigned), Zl/Zp (line/paragraph separators),
/// or any Zs (space separator) except ASCII space (U+0020).
#[inline]
fn py_is_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    !matches!(
        c.general_category(),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::SpaceSeparator
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
    )
}

pub fn range_len(start: i64, stop: i64, step: i64) -> i64 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            0
        } else {
            ((stop - start - 1) / step) + 1
        }
    } else if start <= stop {
        0
    } else {
        ((start - stop - 1) / (-step)) + 1
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Traceback frame tracking
// ─────────────────────────────────────────────────────────────────────────────

/// A single frame in a Python-style traceback.
///
/// Populated by the interpreter when errors propagate out of
/// `run_bytecode` / `run_bytecode_for_fn`.  `lineno` is `None` until
/// per-instruction line-number tracking is implemented (Phase 2).
#[derive(Debug, Clone)]
pub struct FrameInfo {
    /// Path to the source file that contains this frame.  Stored as `Arc<str>`
    /// so that cloning a frame (e.g. when snapshotting `CAPTURED_ERROR_FRAMES`)
    /// is a cheap reference-count bump rather than a heap allocation.
    pub filename: Arc<str>,
    /// 1-based source line that raised the error, or `None` when no line
    /// table is available (Phase 1 limitation).
    pub lineno: Option<u32>,
    /// Function or method name.  `"<module>"` for module-scope code.
    /// `Arc<str>` so cloning a frame is a reference-count bump, not a heap alloc.
    pub funcname: Arc<str>,
}

thread_local! {
    /// Active frame stack for the current interpreter thread.
    ///
    /// Pushed by `call_user_function_expanded` before entering each user
    /// function body and popped when the body returns (or errors).
    static TRACEBACK_FRAME_STACK: RefCell<Vec<FrameInfo>> = RefCell::new(Vec::new());

    /// The frame snapshot captured at the first point an error exits a user
    /// function boundary during the current interpreter invocation.
    ///
    /// `calls.rs::call_user_function_expanded` checks this BEFORE popping its
    /// frame: if `None`, it snapshots the full frame stack (which still includes
    /// the current frame) and stores it here.  Subsequent outer callers see
    /// `Some(_)` and leave it intact, so the innermost error site wins.
    ///
    /// Reset to `None` at the top of each `try_exec_vm_script_with_index` run.
    static CAPTURED_ERROR_FRAMES: RefCell<Option<Vec<FrameInfo>>> = RefCell::new(None);
}

/// Push a frame onto the current thread's traceback stack.
///
/// Called by `calls.rs::call_user_function_expanded` immediately before
/// entering each user-function body.
#[inline]
pub fn push_traceback_frame(frame: FrameInfo) {
    TRACEBACK_FRAME_STACK.with(|s| s.borrow_mut().push(frame));
}

/// Pop the innermost frame from the current thread's traceback stack,
/// optionally capturing the frame chain into `CAPTURED_ERROR_FRAMES` when
/// the call returned an error, or clearing it when the call succeeded.
///
/// `is_error` — pass `true` when the `run_bytecode_for_fn` call returned
/// `Err(_)`.
///
/// - When `true` and no frame snapshot has been captured yet: snapshots the
///   current stack (including the frame being popped) into
///   `CAPTURED_ERROR_FRAMES` before popping.  Subsequent outer frames see
///   `Some(_)` and leave the snapshot intact so the innermost error site wins.
///
/// - When `false` (the call returned successfully): clears any stale
///   `CAPTURED_ERROR_FRAMES`.  This handles the case where an inner call
///   raised but the error was caught by a `try/except` inside that inner
///   call — the outer call returns successfully, so the stale inner-frame
///   snapshot must not pollute a subsequent error in the same outer call.
#[inline]
pub fn pop_traceback_frame(is_error: bool) {
    TRACEBACK_FRAME_STACK.with(|stack| {
        if is_error {
            CAPTURED_ERROR_FRAMES.with(|captured| {
                let mut cap = captured.borrow_mut();
                if cap.is_none() {
                    // First frame to see this error: snapshot the full stack
                    // while it still contains the current frame.
                    *cap = Some(stack.borrow().clone());
                }
            });
        } else {
            // Success path: clear any stale snapshot from a previously caught
            // error inside this call stack so it does not pollute future errors.
            CAPTURED_ERROR_FRAMES.with(|captured| {
                *captured.borrow_mut() = None;
            });
        }
        stack.borrow_mut().pop();
    });
}

/// Take the captured error frame snapshot, leaving the thread-local as `None`.
///
/// Called once by `try_exec_vm_script_with_index` after `run_bytecode`
/// returns an error, to build the traceback header.  Also called at the start
/// of each script run to reset any stale snapshot from a previous run.
#[inline]
pub fn take_captured_error_frames() -> Option<Vec<FrameInfo>> {
    CAPTURED_ERROR_FRAMES.with(|c| c.borrow_mut().take())
}

/// Clear the captured error frame snapshot (reset between script runs).
#[inline]
pub fn reset_captured_error_frames() {
    CAPTURED_ERROR_FRAMES.with(|c| *c.borrow_mut() = None);
}

/// Format a traceback chain as CPython does, returning it as a `String`.
///
/// `frames` is the list produced by `snapshot_traceback_frames()` (innermost
/// last) with the `<module>` frame prepended by the caller.
///
/// Output format (no column underlines — Phase 1):
/// ```text
/// Traceback (most recent call last):
///   File "test.py", in <module>
///   File "test.py", in foo
/// SomeError: message
/// ```
pub fn format_traceback(frames: &[FrameInfo], error_line: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("Traceback (most recent call last):\n");
    for frame in frames {
        match frame.lineno {
            Some(n) => {
                let _ = write!(
                    out,
                    "  File \"{}\", line {}, in {}\n",
                    frame.filename, n, frame.funcname
                );
            }
            None => {
                let _ = write!(
                    out,
                    "  File \"{}\", in {}\n",
                    frame.filename, frame.funcname
                );
            }
        }
    }
    out.push_str(error_line);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Error types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PyError {
    Lex(String),
    Parse(String),
    Runtime(String),
    /// A named Python exception identified by **class name string**.
    ///
    /// Used by builtin code in `pyrust-core` and `pyrust-builtins` that
    /// cannot hold a `Rc<RefCell<PyClass>>` because the class objects live
    /// in the interpreter crate.  The VM materialises this into a
    /// `PyInstance` via an env-hierarchy name lookup before propagating.
    ///
    /// `class_name` is a `Cow<'static, str>` so the overwhelmingly common
    /// case (a string literal like `"TypeError"`) is zero-allocation; rare
    /// dynamic class names can still be carried via `Cow::Owned`.
    Named(Cow<'static, str>, String), // (class_name, message)
    /// A named Python exception identified by **class identity** (`Rc`).
    ///
    /// Used by interpreter-internal raise sites that already hold the class
    /// object (e.g. the VM dispatch loop).  The VM can materialise this into
    /// a `PyInstance` directly, with no env-hierarchy name lookup — making
    /// the hot path (typed exception from a built-in opcode) zero-lookup.
    ///
    /// Construct via [`PyError::class`].  Call sites in `pyrust-core` and
    /// `pyrust-builtins` that cannot reference a `PyClass` Rc should
    /// continue to use [`PyError::named`] / `PyError::Named`.
    Class(Rc<RefCell<PyClass>>, String), // (class, message)
    /// A `KeyError` that carries the **raw key `Value`** as `args[0]`.
    ///
    /// CPython stores the key object itself in `args[0]`, not a stringified
    /// repr.  This variant lets all raise sites (builtin and VM) pass the key
    /// through without pre-rendering it as a string.  The VM materialises it
    /// as `instantiate_exception(KeyError_class, vec![key])`.
    KeyError(Value),
    /// An `ImportError` (or `ModuleNotFoundError`) that carries the module
    /// name so the VM can set `.name` and `.path` on the resulting instance.
    ///
    /// CPython 3.12: `ImportError.__init__` accepts `*args, name=None,
    /// path=None` keyword arguments and stores them as instance attributes.
    /// Raise sites that know the module name should use this variant instead
    /// of `PyError::Named("ImportError", …)` so that `e.name` works in
    /// `except ImportError as e:` blocks.
    ///
    /// `class_name` is `"ImportError"` or `"ModuleNotFoundError"`.
    /// `module_name` is `None` when the module name is not available.
    ImportError {
        class_name: &'static str,
        message: String,
        module_name: Option<String>,
    },
    /// An `OSError` (or one of its subclasses) raised from an OS-level
    /// operation.  Carries the structured fields that CPython 3.12 sets on
    /// every OS-sourced exception: `errno`, `strerror`, and optionally
    /// `filename` and `filename2`.
    ///
    /// The VM materialises this into a `PyInstance` via
    /// `instantiate_os_error`, which populates `errno`, `strerror`,
    /// `filename`, and `filename2` as instance attributes — matching
    /// CPython's `OSError.__init__(errno, strerror[, filename])` behaviour.
    /// Two-path operations (e.g. `os.rename`) set both `filename` (src) and
    /// `filename2` (dst).
    OsError {
        class_name: &'static str,
        errno: i64,
        strerror: String,
        filename: Option<String>,
        filename2: Option<String>,
    },
    Raised(Value),
}

impl PyError {
    /// Convenience constructor for a named Python exception with a static
    /// class-name literal.  Avoids the per-call `"TypeError".to_string()`
    /// allocation that every error site would otherwise perform.
    ///
    /// Prefer [`PyError::class`] when the class `Rc` is already in scope
    /// (avoids an env-hierarchy name lookup in the VM handler).
    #[inline]
    pub fn named(cls: &'static str, msg: impl Into<String>) -> Self {
        PyError::Named(Cow::Borrowed(cls), msg.into())
    }

    /// Constructor for a named Python exception with a **known class object**.
    ///
    /// The VM can materialise this directly (no env name lookup).  Use this
    /// from interpreter-internal raise sites that already hold the class `Rc`.
    #[inline]
    pub fn class(cls: Rc<RefCell<PyClass>>, msg: impl Into<String>) -> Self {
        PyError::Class(cls, msg.into())
    }

    /// Constructor for a `KeyError` that stores the raw key `Value`.
    ///
    /// CPython keeps the original key object as `args[0]` of the `KeyError`
    /// instance, so `e.args[0]` returns the key itself (not a repr string).
    /// Use this at every dict/set key-not-found raise site instead of
    /// `PyError::named("KeyError", key.repr())`.
    #[inline]
    pub fn key_error(key: Value) -> Self {
        PyError::KeyError(key)
    }

    /// Constructor for an `ImportError` or `ModuleNotFoundError` that carries
    /// the module name.
    ///
    /// `class_name` must be `"ImportError"` or `"ModuleNotFoundError"`.
    /// The VM materialises the exception and sets `.name` and `.path` on it.
    #[inline]
    pub fn import_error(
        class_name: &'static str,
        message: impl Into<String>,
        module_name: Option<String>,
    ) -> Self {
        PyError::ImportError {
            class_name,
            message: message.into(),
            module_name,
        }
    }

    /// Map a `std::io::ErrorKind` to the most-derived Python OSError subclass
    /// name following CPython 3.12's errno-to-subclass mapping.
    fn io_kind_to_class(kind: std::io::ErrorKind) -> &'static str {
        use std::io::ErrorKind::*;
        match kind {
            NotFound => "FileNotFoundError",
            PermissionDenied => "PermissionError",
            AlreadyExists => "FileExistsError",
            IsADirectory => "IsADirectoryError",
            NotADirectory => "NotADirectoryError",
            Interrupted => "InterruptedError",
            WouldBlock => "BlockingIOError",
            TimedOut => "TimeoutError",
            _ => "OSError",
        }
    }

    /// Convert a `std::io::Error` into a Python `OSError` (or subclass),
    /// attaching `filename` when provided.
    ///
    /// The `strerror` stored on the exception is the OS-provided message
    /// stripped of any file-path decoration, matching CPython's behaviour
    /// where `e.strerror` is `"No such file or directory"` rather than the
    /// decorated form.
    pub fn from_io_error(e: &std::io::Error, filename: Option<&str>) -> Self {
        Self::from_io_error2(e, filename, None)
    }

    /// Like [`from_io_error`] but also sets `filename2` for two-path operations
    /// (e.g. `os.rename(src, dst)` where `filename2` is the destination).
    /// CPython 3.12 sets `filename2` on the exception for rename/link/symlink.
    pub fn from_io_error2(
        e: &std::io::Error,
        filename: Option<&str>,
        filename2: Option<&str>,
    ) -> Self {
        let raw = e.raw_os_error().unwrap_or(0);
        let class_name = Self::io_kind_to_class(e.kind());
        // `std::io::Error::from_raw_os_error(N).to_string()` on Linux produces
        // "strerror(N) (os error N)" — strip the Rust-added trailer.
        // On Windows, FormatMessage strings end with a trailing period; strip
        // that too so the result matches CPython's `e.strerror` behaviour.
        let strerror = if raw != 0 {
            let full = std::io::Error::from_raw_os_error(raw).to_string();
            let base = if let Some(pos) = full.rfind(" (os error ") {
                &full[..pos]
            } else {
                &full[..]
            };
            base.trim_end_matches('.').to_owned()
        } else {
            e.to_string()
        };
        PyError::OsError {
            class_name,
            errno: raw as i64,
            strerror,
            filename: filename.map(|s| s.to_owned()),
            filename2: filename2.map(|s| s.to_owned()),
        }
    }

    /// Returns `true` when `self` is a `Named`, `Class`, `KeyError`, or `Raised` error
    /// whose exception class name equals `name`.
    ///
    /// Used by the generator/iterator machinery to cheaply detect
    /// `StopIteration` and `GeneratorExit` without materialising the error
    /// into a full `PyInstance`.  Works for the string-named (`Named`),
    /// class-identity (`Class`), and already-raised-instance (`Raised`)
    /// variants so that `PyError::Raised(StopIteration(...))` produced by
    /// `resume_generator` is treated the same as `PyError::Named`.
    #[inline]
    pub fn class_name_is(&self, name: &str) -> bool {
        match self {
            PyError::Named(cls, _) => cls.as_ref() == name,
            PyError::Class(cls, _) => cls.borrow().name == name,
            PyError::KeyError(_) => name == "KeyError",
            PyError::ImportError { class_name, .. } => {
                // ModuleNotFoundError is a subclass of ImportError; treat it
                // as both so that `class_name_is("ImportError")` returns true
                // for a ModuleNotFoundError variant too.
                *class_name == name
                    || (name == "ImportError" && *class_name == "ModuleNotFoundError")
            }
            PyError::OsError { class_name, .. } => {
                // All OSError subclasses also match "OSError" for the fast-path check.
                *class_name == name
                    || (name == "OSError"
                        && matches!(
                            *class_name,
                            "FileNotFoundError"
                                | "PermissionError"
                                | "FileExistsError"
                                | "IsADirectoryError"
                                | "NotADirectoryError"
                                | "InterruptedError"
                                | "BlockingIOError"
                                | "ChildProcessError"
                                | "ProcessLookupError"
                                | "TimeoutError"
                        ))
            }
            PyError::Raised(exc) => match exc.kind() {
                ValueKind::PyInstance(inst) => class_chain_has_name(&inst.borrow().class, name),
                ValueKind::PyClass(cls) => class_chain_has_name(cls, name),
                _ => false,
            },
            _ => false,
        }
    }
}

impl fmt::Display for PyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyError::Lex(s) => write!(f, "Lex error: {s}"),
            PyError::Parse(s) => write!(f, "Parse error: {s}"),
            PyError::Runtime(s) => write!(f, "Runtime error: {s}"),
            PyError::Named(cls, s) => write!(f, "{cls}: {s}"),
            PyError::Class(cls, s) => write!(f, "{}: {s}", cls.borrow().name),
            PyError::KeyError(key) => write!(f, "KeyError: {}", key.repr()),
            PyError::ImportError {
                class_name,
                message,
                ..
            } => {
                write!(f, "{class_name}: {message}")
            }
            PyError::OsError {
                class_name,
                errno,
                strerror,
                filename,
                ..
            } => {
                if let Some(fname) = filename {
                    write!(f, "{class_name}: [Errno {errno}] {strerror}: '{fname}'")
                } else {
                    write!(f, "{class_name}: [Errno {errno}] {strerror}")
                }
            }
            PyError::Raised(value) => write!(f, "Uncaught exception: {}", value.repr()),
        }
    }
}

pub type Result<T> = std::result::Result<T, PyError>;

// ─────────────────────────────────────────────────────────────────────────────
// Typed argument extractors
//
// These helpers are used by `pyrust-builtins` (which cannot depend on the
// interpreter crate) to extract typed values from `&[Value]` slices while
// producing CPython-3.12-compatible `TypeError` messages.
// ─────────────────────────────────────────────────────────────────────────────

/// Extract a `&str` from `v`.
///
/// On type mismatch raises `TypeError: <method>() argument '<param>' must be
/// str, not <typename>` — matching CPython 3.12's message format for
/// `str.removeprefix` and similar single-str-param methods.
pub fn extract_str<'a>(v: &'a Value, method: &str, param: &str) -> Result<&'a str> {
    match v.kind() {
        ValueKind::Str(s) => Ok(s),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{method}() argument '{param}' must be str, not {}",
                builtin_type_name(v),
            ),
        )),
    }
}

/// Extract an `i64` from `v`, accepting both `Int` and `Bool`.
///
/// On type mismatch raises `TypeError: '<typename>' object cannot be
/// interpreted as an integer` — matching CPython 3.12's message for integer
/// width/tabsize arguments.
pub fn extract_int(v: &Value, _method: &str, _param: &str) -> Result<i64> {
    match v.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(b as i64),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                builtin_type_name(v),
            ),
        )),
    }
}

/// Extract an optional `&str` from `args[idx]`.
///
/// Returns `None` when the slot is absent or `None`-typed.
/// Returns `Some(s)` when the slot holds a `Str`.
/// Returns an error with `TypeError` when the slot holds another type.
pub fn extract_optional_str<'a>(
    args: &'a [Value],
    idx: usize,
    method: &str,
    param: &str,
) -> Result<Option<&'a str>> {
    match args.get(idx).map(|v| v.kind()) {
        None | Some(ValueKind::None) => Ok(None),
        Some(ValueKind::Str(s)) => Ok(Some(s)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{method}() argument '{param}' must be str, not {}",
                builtin_type_name(&args[idx]),
            ),
        )),
    }
}

/// Extract an optional `i64` from `args[idx]`, accepting `Int` and `Bool`.
///
/// Returns `None` when the slot is absent.
/// Returns `Some(n)` when the slot holds `Int` or `Bool`.
/// Returns an error with `TypeError` when the slot holds another type.
pub fn extract_optional_int(args: &[Value], idx: usize) -> Result<Option<i64>> {
    match args.get(idx).map(|v| v.kind()) {
        None => Ok(None),
        Some(ValueKind::Int(n)) => Ok(Some(n)),
        Some(ValueKind::Bool(b)) => Ok(Some(b as i64)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                builtin_type_name(&args[idx]),
            ),
        )),
    }
}

/// Extract a fill `char` from `args[1]`, defaulting to `' '`.
///
/// CPython 3.12 error messages for the fill-character argument:
/// - Wrong type: `TypeError: The fill character must be a unicode character, not <typename>`
/// - Multiple chars: `TypeError: The fill character must be exactly one character long`
pub fn extract_fill_char(args: &[Value]) -> Result<char> {
    match args.get(1).map(|v| v.kind()) {
        None => Ok(' '),
        Some(ValueKind::Str(s)) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(c),
                _ => Err(PyError::named(
                    "TypeError",
                    "The fill character must be exactly one character long",
                )),
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "The fill character must be a unicode character, not {}",
                builtin_type_name(&args[1]),
            ),
        )),
    }
}

/// Validate argument count for a method that accepts `min..=max` positional args.
///
/// Uses the CPython 3.12 error message format:
/// - `min == max`: `"<method> takes exactly <n> argument(s) (<got> given)"`
/// - `min == 1, max == ∞`: `"<method>() takes at least 1 argument (<got> given)"`
/// - `min < max`: `"<method> expected at least <min> argument, got <got>"` or
///   `"<method> expected at most <max> arguments, got <got>"`
///
/// The messages are chosen to match CPython's output for the specific methods
/// migrated in this refactor.  Pass `max: usize::MAX` for "no upper bound".
pub fn expect_arg_count(args: &[Value], min: usize, max: usize, method: &str) -> Result<()> {
    let got = args.len();
    if got < min {
        if min == max {
            // "str.zfill() takes exactly one argument (0 given)"
            let noun = exactly_n_noun(min);
            return Err(PyError::named(
                "TypeError",
                format!("str.{method}() takes exactly {noun} ({got} given)"),
            ));
        }
        // "center expected at least 1 argument, got 0"
        return Err(PyError::named(
            "TypeError",
            format!("{method} expected at least {min} argument, got {got}"),
        ));
    }
    if max != usize::MAX && got > max {
        if min == max {
            let noun = exactly_n_noun(min);
            return Err(PyError::named(
                "TypeError",
                format!("str.{method}() takes exactly {noun} ({got} given)"),
            ));
        }
        // "center expected at most 2 arguments, got 3"
        return Err(PyError::named(
            "TypeError",
            format!("{method} expected at most {max} arguments, got {got}"),
        ));
    }
    Ok(())
}

fn exactly_n_noun(n: usize) -> String {
    if n == 1 {
        "one argument".to_string()
    } else {
        format!("{n} arguments")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_round_trips_through_kind() {
        let v = Value::not_implemented();
        assert!(v.is_not_implemented());
        assert!(matches!(v.kind(), ValueKind::NotImplemented));
        assert!(!v.is_unset());
    }

    #[test]
    fn not_implemented_is_not_classified_as_float() {
        // The NaN-box bit pattern shares the float top16 range; `kind()`
        // must intercept it before the float arm.  Regression guard for the
        // top16-vs-exact-bits caveat noted on `top16`.
        let v = Value::not_implemented();
        assert!(!matches!(v.kind(), ValueKind::Float(_)));
        assert!(!v.is_float());
    }

    #[test]
    fn not_implemented_repr_is_canonical() {
        assert_eq!(Value::not_implemented().repr(), "NotImplemented");
    }

    #[test]
    fn unset_and_not_implemented_are_distinct_patterns() {
        // Both use the positive-NaN sentinel family; they must not collide.
        let unset = Value::unset();
        let nimpl = Value::not_implemented();
        assert!(unset.is_unset());
        assert!(!unset.is_not_implemented());
        assert!(nimpl.is_not_implemented());
        assert!(!nimpl.is_unset());
    }

    #[test]
    fn unset_is_unset_returns_true() {
        // Basic sanity: the sentinel round-trips through is_unset().
        let v = Value::unset();
        assert!(v.is_unset());
        assert!(!v.is_none());
        assert!(!v.is_float());
        assert!(!v.is_not_implemented());
    }

    #[test]
    fn unset_as_some_returns_none() {
        // as_some() is the safe way to probe an unset slot.
        let v = Value::unset();
        assert!(v.as_some().is_none());
    }

    // In debug builds, calling kind() on an unset Value must panic with a
    // diagnostic message so missed CheckLocal emissions surface immediately
    // rather than silently propagating a NaN through the program.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_kind_panics_in_debug() {
        let v = Value::unset();
        let _ = v.kind();
    }

    // In debug builds, truthy() routes through kind() and must also panic.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_truthy_panics_in_debug() {
        let v = Value::unset();
        let _ = v.truthy();
    }

    // The direct NaN-box accessors bypass kind(), so they each need their own
    // tripwire.  The following tests confirm that each one panics (rather than
    // silently returning None / a garbage bit pattern) when called on an unset
    // Value in a debug build.

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_int_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_int();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_str_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_str();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_bool_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_bool();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_int_raw_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_int_raw();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_float_raw_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_float_raw();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_list_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_list();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_tuple_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_tuple();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_opaque_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_opaque();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_dict_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_dict();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_set_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_set();
    }

    // Regression: non-unset values must not be affected by the new guards.

    #[test]
    fn as_int_on_int_value_still_works() {
        assert_eq!(Value::int(42).as_int(), Some(42));
        assert_eq!(Value::none().as_int(), None);
    }

    #[test]
    fn as_str_on_str_value_still_works() {
        assert_eq!(Value::string("hello").as_str(), Some("hello"));
        assert_eq!(Value::none().as_str(), None);
    }

    /// Helper: build a minimal `UserFunction` for kind-wrapping tests.
    fn make_user_function() -> Rc<UserFunction> {
        Rc::new(UserFunction {
            id: next_fn_id(),
            kind: UserFunctionKind::Regular,
            name: "f".to_string(),
            qualname: "f".to_string(),
            user_name: RefCell::new(None),
            user_qualname: RefCell::new(None),
            module: RefCell::new(Value::string("__main__".to_string())),
            doc: RefCell::new(Value::none()),
            attrs: RefCell::new(None),
            annotations: RefCell::new(Value::dict(IndexMap::new())),
            params: Vec::new(),
            local_names: Rc::new(HashSet::new()),
            local_index: Rc::new(HashMap::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            env: Environment::new(None),
            is_pure: false,
            precompiled_code: None,
            wrapped_func: None,
        })
    }

    fn extract_user_function(v: &Value) -> Rc<UserFunction> {
        match v.kind() {
            ValueKind::UserFunction(f) => Rc::clone(f),
            _ => panic!("expected UserFunction value"),
        }
    }

    #[test]
    fn with_function_kind_reuses_original_id() {
        // Regression: #303 — `@classmethod` / `@staticmethod` must reuse the
        // original `id` so they share `fn_cache` entries with the undecorated
        // form (and with each other), instead of allocating a fresh `id`
        // every time and doubling cache footprint.
        let original = make_user_function();
        let original_id = original.id;

        let cm = Value::class_method(Rc::clone(&original));
        let sm = Value::static_method(Rc::clone(&original));

        let cm_fn = extract_user_function(&cm);
        let sm_fn = extract_user_function(&sm);

        assert_eq!(cm_fn.id, original_id, "classmethod must reuse id");
        assert_eq!(sm_fn.id, original_id, "staticmethod must reuse id");
        assert_eq!(cm_fn.kind, UserFunctionKind::ClassMethod);
        assert_eq!(sm_fn.kind, UserFunctionKind::StaticMethod);
    }

    #[test]
    fn with_function_kind_idempotent_reuses_rc() {
        // When the requested kind already matches, return the same Rc — no
        // reallocation at all.
        let original = make_user_function();
        let wrapped = Value::with_function_kind(Rc::clone(&original), UserFunctionKind::Regular);
        let wrapped_fn = extract_user_function(&wrapped);
        assert!(
            Rc::ptr_eq(&original, &wrapped_fn),
            "kind-preserving wrap must reuse the original Rc"
        );
    }

    #[test]
    fn list_clone_shares_storage_for_bound_method_mutation() {
        // Regression test for #305.  `Value::clone` on a list must produce an
        // alias of the same backing storage, so that captured bound methods
        // (`m = lst.append; m(4)`) and simple aliasing (`b = a; b.append(x)`)
        // mutate the original list — matching CPython's reference semantics.
        let a = Value::list(vec![Value::int(1)]);
        let b = a.clone();
        b.list_push(Value::int(2))
            .expect("clone must still be a list");
        let items = a.as_list().expect("original must still be a list");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_int(), Some(1));
        assert_eq!(items[1].as_int(), Some(2));
    }

    #[test]
    fn list_clone_preserves_identity() {
        // `id(b) == id(a)` after `b = a` for list, matching CPython.
        let a = Value::list(vec![Value::int(1), Value::int(2)]);
        let b = a.clone();
        assert_eq!(a.value_id(), b.value_id());

        // Distinct list literals must NOT share identity.
        let c = Value::list(vec![Value::int(1), Value::int(2)]);
        assert_ne!(a.value_id(), c.value_id());
    }

    #[test]
    fn set_clone_shares_storage() {
        // Same Rc-sharing invariant as list, exercised through set's mutating
        // accessor.
        let a = Value::set({
            let mut s = IndexSet::new();
            s.insert(PyKey::Int(1));
            s
        });
        let b = a.clone();
        b.set_add(PyKey::Int(2)).expect("clone must still be a set");
        let items = a.as_set().expect("original must still be a set");
        assert!(items.contains(&PyKey::Int(1)));
        assert!(items.contains(&PyKey::Int(2)));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn set_mutation_through_original_visible_in_clone() {
        // Symmetric counterpart to `set_clone_shares_storage`: mutate via
        // the original Value, the clone (alias) sees it.  This pins both
        // directions of the Rc-shared backing post-#305.
        let a = Value::set({
            let mut s = IndexSet::new();
            s.insert(PyKey::Int(1));
            s
        });
        let b = a.clone();
        a.set_add(PyKey::Int(2))
            .expect("original must still be a set");
        let items = b.as_set().expect("clone must still be a set");
        assert!(items.contains(&PyKey::Int(1)));
        assert!(items.contains(&PyKey::Int(2)));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn set_clone_preserves_identity() {
        let a = Value::set({
            let mut s = IndexSet::new();
            s.insert(PyKey::Int(1));
            s
        });
        let b = a.clone();
        assert_eq!(a.value_id(), b.value_id());

        let c = Value::set({
            let mut s = IndexSet::new();
            s.insert(PyKey::Int(1));
            s
        });
        assert_ne!(a.value_id(), c.value_id());
    }

    #[test]
    fn dict_clone_preserves_identity() {
        // Dict already used `Rc<RefCell<...>>` shared storage; #305 added an
        // `id()` surface for it via `value_id()`.  Pin the invariant.
        let a = Value::dict({
            let mut m = IndexMap::new();
            m.insert(PyKey::str_from("k"), Value::int(1));
            m
        });
        let b = a.clone();
        assert_eq!(a.value_id(), b.value_id());

        let c = Value::dict({
            let mut m = IndexMap::new();
            m.insert(PyKey::str_from("k"), Value::int(1));
            m
        });
        assert_ne!(a.value_id(), c.value_id());
    }

    // ── float_bits_as_exact_i64 boundary tests ───────────────────────────────

    #[test]
    fn float_bits_exact_i64_integer_values() {
        // Ordinary integer-valued floats within i64 range.
        assert_eq!(float_bits_as_exact_i64(1.0f64.to_bits()), Some(1));
        assert_eq!(float_bits_as_exact_i64((-1.0f64).to_bits()), Some(-1));
        assert_eq!(float_bits_as_exact_i64(0.0f64.to_bits()), Some(0));
        assert_eq!(float_bits_as_exact_i64(42.0f64.to_bits()), Some(42));
        assert_eq!(
            float_bits_as_exact_i64(1_000_000_000_000_000.0f64.to_bits()),
            Some(1_000_000_000_000_000)
        );
    }

    #[test]
    fn float_bits_exact_i64_fractional_returns_none() {
        assert_eq!(float_bits_as_exact_i64(0.5f64.to_bits()), None);
        assert_eq!(float_bits_as_exact_i64(1.5f64.to_bits()), None);
        assert_eq!(float_bits_as_exact_i64((-0.1f64).to_bits()), None);
    }

    #[test]
    fn float_bits_exact_i64_non_finite_returns_none() {
        assert_eq!(float_bits_as_exact_i64(f64::INFINITY.to_bits()), None);
        assert_eq!(float_bits_as_exact_i64(f64::NEG_INFINITY.to_bits()), None);
        assert_eq!(float_bits_as_exact_i64(f64::NAN.to_bits()), None);
    }

    #[test]
    fn float_bits_exact_i64_i64_min_is_exact() {
        // i64::MIN as f64 is exactly representable (-2^63); must return Some.
        let min_f = i64::MIN as f64;
        assert_eq!(float_bits_as_exact_i64(min_f.to_bits()), Some(i64::MIN));
    }

    #[test]
    fn float_bits_exact_i64_i64_max_rounds_up() {
        // i64::MAX = 2^63-1 is not exactly representable as f64; the nearest f64
        // is 2^63, which is out of range.  Must return None.
        let max_f = i64::MAX as f64; // rounds up to 2^63
        assert_eq!(float_bits_as_exact_i64(max_f.to_bits()), None);
    }

    #[test]
    fn float_bits_exact_i64_out_of_range_large() {
        // 2^63 is exactly representable but exceeds i64::MAX (= 2^63 - 1).
        let too_big = 9_223_372_036_854_775_808.0f64; // 2^63
        assert_eq!(float_bits_as_exact_i64(too_big.to_bits()), None);
        // 2^64 — clearly out of range.
        let much_bigger = 1.844_674_407_370_955_2e19_f64; // 2^64
        assert_eq!(float_bits_as_exact_i64(much_bigger.to_bits()), None);
        // Negative out-of-range: the f64 immediately below i64::MIN as f64.
        // i64::MIN as f64 is exactly -2^63 (in range); the next f64 towards
        // -inf has integer value -2^63 - 2^10, which is out of i64 range.
        let min_f = i64::MIN as f64;
        // Construct the next representable f64 below min_f by decrementing bits.
        let next_below_bits = min_f.to_bits() + 1; // negative floats: +1 bits → more negative
        let too_small = f64::from_bits(next_below_bits);
        assert!(
            too_small < min_f,
            "sanity: next_below must be more negative than i64::MIN as f64"
        );
        assert_eq!(float_bits_as_exact_i64(next_below_bits), None);
    }

    #[test]
    fn pykey_float_int_cross_type_eq() {
        // Core contract: Float(1.0) == Int(1) and they hash equal.
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        fn key_hash(k: &PyKey) -> u64 {
            let mut h = DefaultHasher::new();
            k.hash(&mut h);
            h.finish()
        }

        let f1 = PyKey::Float(1.0f64.to_bits());
        let i1 = PyKey::Int(1);
        assert_eq!(f1, i1, "Float(1.0) must equal Int(1)");
        assert_eq!(
            key_hash(&f1),
            key_hash(&i1),
            "hash(Float(1.0)) must equal hash(Int(1))"
        );

        // 0.5 must NOT equal 0
        let f05 = PyKey::Float(0.5f64.to_bits());
        let i0 = PyKey::Int(0);
        assert_ne!(f05, i0, "Float(0.5) must not equal Int(0)");

        // Float(-1.0) == Int(-1)
        let fn1 = PyKey::Float((-1.0f64).to_bits());
        let in1 = PyKey::Int(-1);
        assert_eq!(fn1, in1, "Float(-1.0) must equal Int(-1)");
        assert_eq!(key_hash(&fn1), key_hash(&in1), "hash contract for -1.0/-1");

        // Float(1.0) == Bool(true)
        let bt = PyKey::Bool(true);
        assert_eq!(f1, bt, "Float(1.0) must equal Bool(true)");
        assert_eq!(key_hash(&f1), key_hash(&bt), "hash contract for 1.0/true");
    }
}
