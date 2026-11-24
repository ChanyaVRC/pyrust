// ─────────────────────────────────────────────────────────────────────────────
// PyKey — hashable subset of Value used as dict/set keys
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
    contains_object: bool,
}

impl FrozenSetKey {
    pub fn new(items: Rc<PySet>) -> Self {
        let py_hash = py_hash_frozenset_items(&items);
        let contains_object = items.iter().any(pykey_contains_object);
        Self {
            items,
            py_hash,
            contains_object,
        }
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

    /// Whether any element (transitively) holds a `PyKey::Object` whose
    /// equality may dispatch Python `__eq__`. Computed once at construction.
    #[inline]
    pub fn contains_object(&self) -> bool {
        self.contains_object
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
    /// Hashable tuple key.  Stores each element as its own `PyKey` in an exact-
    /// sized boxed slice (`Box<[PyKey]>`, 16 bytes vs 24 for `Vec<PyKey>`) so
    /// the whole `PyKey` enum stays `<= 24` bytes (#454).  Hashing and equality
    /// propagate element-wise (matching CPython's `hash((1, 2))`).  Unlike
    /// `FrozenSet`, ordering is significant — `(1, 2) != (2, 1)` — so we
    /// preserve element order.
    Tuple(Box<[PyKey]>),
    /// Bytes key.  `bytes` objects are immutable and therefore hashable in
    /// CPython.  Holds the backing `Rc<Vec<u8>>` directly to avoid
    /// re-allocation on the hot dict/set lookup path.
    Bytes(Rc<Vec<u8>>),
    /// Complex key.  Used for *every* complex value, including one with a zero
    /// imaginary part: retaining `1+0j` as a complex key is what makes
    /// `list({1+0j: 'a'})` yield `[(1+0j)]` rather than `[1.0]` (#2900).
    ///
    /// A real-valued complex (`im == 0.0`) still unifies with the other numeric
    /// key types — `1+0j == 1 == 1.0 == True` occupy one dict/set slot — via
    /// the dedicated `Complex <-> Int/Bool/Float/BigInt` arms in `PartialEq`
    /// and the delegating `Hash` arm.  Whichever key was inserted *first* is
    /// the one the container keeps, matching CPython.
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

const _: () = assert!(std::mem::size_of::<PyKey>() <= 24);

impl PyKey {
    /// Construct a string key from a bare `&str` / `String` (or anything
    /// implementing `AsRef<str>`).  Allocates a fresh `Value::string` once;
    /// subsequent `.clone()` calls on the returned `PyKey` are O(1) RC bumps.
    #[inline]
    pub fn str_from(s: impl AsRef<str>) -> PyKey {
        PyKey::Str(Value::string(s.as_ref()))
    }
}

/// Equality between a complex key and a *non-complex* key.
///
/// A complex with a zero imaginary part (either sign of zero) is numerically
/// real, so CPython puts `1+0j`, `1`, `1.0` and `True` in one dict slot; a
/// non-zero imaginary part matches nothing else (#2900).
///
/// Deliberately no NaN bit-equality fallback: CPython reports
/// `complex(nan, 0) == nan` as False.  The identity short-circuit that keeps
/// `{z: 1}[z]` working lives in the `Complex`/`Complex` arm of [`PyKey::eq`].
///
/// Kept `#[cold]` and out-of-line: complex dict keys are rare, while
/// `PyKey::eq` is inlined into every dict/set probe, so this code earns its
/// place off the hot path.
#[cold]
#[inline(never)]
fn real_valued_complex_eq(re: f64, im: f64, other: &PyKey) -> bool {
    if im != 0.0 {
        return false;
    }
    match other {
        PyKey::Float(bits) => re == f64::from_bits(*bits),
        PyKey::Int(i) => float_bits_eq_i64(re.to_bits(), *i),
        PyKey::Bool(b) => float_bits_eq_i64(re.to_bits(), *b as i64),
        PyKey::BigInt(n) => bigint_float_eq(n, re),
        _ => false,
    }
}

impl PyKey {
    /// Out-of-line equality for non-`(Int, Int)` / non-`(Str, Str)` key pairs
    /// so the inlined `PartialEq::eq` stays tiny on homogeneous int/str probes
    /// (#2924).
    #[inline(never)]
    fn eq_slow(&self, other: &Self) -> bool {
        match (self, other) {
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
                float_bits_eq_i64(*bits, *i)
            }
            (PyKey::Float(bits), PyKey::Bool(b)) | (PyKey::Bool(b), PyKey::Float(bits)) => {
                float_bits_eq_i64(*bits, *b as i64)
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
            (PyKey::None, PyKey::None) => true,
            (PyKey::Ellipsis, PyKey::Ellipsis) => true,
            (PyKey::FrozenSet(a), PyKey::FrozenSet(b)) => {
                Rc::ptr_eq(a, b) || a.items() == b.items()
            }
            (PyKey::Tuple(a), PyKey::Tuple(b)) => a == b,
            (PyKey::Bytes(a), PyKey::Bytes(b)) => a.as_ref() == b.as_ref(),
            // Complex equality: two complex keys are equal iff both components match.
            (PyKey::Complex(ar, ai), PyKey::Complex(br, bi)) => {
                complex_key_components_eq(*ar, *ai, *br, *bi)
            }
            // Cross-type: a *real-valued* complex equals the int / bool / float
            // / BigInt with the same value (#2900).
            (PyKey::Complex(re, im), other) | (other, PyKey::Complex(re, im)) => {
                real_valued_complex_eq(*re, *im, other)
            }
            (PyKey::Object { value: a, .. }, PyKey::Object { value: b, .. }) => {
                (matches!(a.kind(), ValueKind::Generator(_)) && a.is_identical_to(b)) || a == b
            }
            _ => false,
        }
    }

    /// Out-of-line hashing for non-`Int` / non-`Str` keys so the inlined
    /// `Hash::hash` stays minimal on homogeneous int/str probes (#2924).
    #[inline(never)]
    fn hash_slow<H: Hasher>(&self, state: &mut H) {
        match self {
            PyKey::Bool(b) => hash_i64_key(*b as i64, state),
            PyKey::Float(bits) => hash_float_bits(*bits, state),
            PyKey::BigInt(n) => hash_bigint_key(n, state),
            PyKey::None => {
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
                for k in items.iter() {
                    k.hash(state);
                }
            }
            PyKey::Bytes(b) => {
                6u8.hash(state);
                b.as_ref().hash(state);
            }
            PyKey::Complex(re, im) => hash_complex_key(*re, *im, state),
            PyKey::Object { hash, .. } => hash.hash(state),
            PyKey::Int(_) | PyKey::Str(_) => unreachable!(),
        }
    }
}

impl PartialEq for PyKey {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // Fast path for the two dominant homogeneous key types (`Int` and
        // `Str`), keeping all cross-type numeric / Complex / Object arms out
        // of line so cross-crate inlining into `IndexMap`/`IndexSet` stays
        // compact (#2924).
        match (self, other) {
            (PyKey::Int(a), PyKey::Int(b)) => a == b,
            (PyKey::Str(a), PyKey::Str(b)) => a.as_str() == b.as_str(),
            _ => self.eq_slow(other),
        }
    }
}

impl Eq for PyKey {}

impl Hash for PyKey {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        // NOTE: `Bool(b)` and integer-valued `Float`/`BigInt`/`Complex` share
        // tag 0 + the `i64` payload with `Int` so equal numeric keys collide
        // in the same bucket (Hash + Eq contract).
        match self {
            PyKey::Int(v) => hash_i64_key(*v, state),
            PyKey::Str(s) => {
                2u8.hash(state);
                s.as_str().unwrap_or("").hash(state);
            }
            _ => self.hash_slow(state),
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
