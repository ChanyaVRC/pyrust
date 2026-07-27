/// Build-hasher used by the `dict` and `set` backing stores.
///
/// An interpreter's internal dicts carry no DoS-resistance requirement (CPython
/// itself uses a fast, non-cryptographic hash), so the stdlib's SipHash default
/// is pure overhead on every insert/lookup/set-op.  `FxBuildHasher`
/// (`rustc_hash`) replaces it with a fast multiply-xor hash.  This only changes
/// bucket placement — `IndexMap`/`IndexSet` keep their insertion-ordered Vec, so
/// dict/set iteration order is unaffected.  `PyKey`'s `Hash` impl is unchanged.
pub type PyHasher = FxBuildHasher;

/// Insertion-ordered backing store for a Python `dict`.
pub type PyDict = IndexMap<PyKey, Value, PyHasher>;

/// Insertion-ordered backing store for a Python `set` / `frozenset`.
pub type PySet = IndexSet<PyKey, PyHasher>;

pub use num_bigint::BigInt as PyBigInt;
pub use num_bigint::Sign as PyBigIntSign;
pub use num_traits::Pow as PyPow;
pub use num_traits::ToPrimitive as PyToPrimitive;
pub use num_traits::Zero as PyZero;

static FN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_fn_id() -> u64 {
    FN_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}
