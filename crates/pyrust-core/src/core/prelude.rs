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

/// Insertion-ordered backing store for a module's direct attributes
/// (`PyModule::attrs`).
///
/// `vars(module)` / `module.__dict__` are Python dicts, and CPython guarantees
/// dict order, so the storage backing them must be insertion ordered too
/// (issue #2918 — a `HashMap` here made `list(vars(math))` differ run to run).
/// For a built-in module the insertion order is the `pyrust_module!` body's
/// declaration order, which is fixed at compile time; for `builtins` it is the
/// declared order of the composed sub-modules. `PyClass::attrs` uses the same
/// shape for the same reason. Removal must use `shift_remove`, never
/// `swap_remove`, to keep the surviving entries in order — exactly like
/// `dict.__delitem__`.
///
/// Hashed with `PyHasher` for the same reason `PyDict` is: a module namespace is
/// interpreter-internal, so SipHash buys nothing here. The hasher only decides
/// bucket placement — the insertion-ordered entry vector, and therefore
/// iteration order, is unaffected.
pub type ModuleAttrs = IndexMap<String, Value, PyHasher>;

pub use num_bigint::BigInt as PyBigInt;
pub use num_bigint::Sign as PyBigIntSign;
pub use num_traits::Pow as PyPow;
pub use num_traits::ToPrimitive as PyToPrimitive;
pub use num_traits::Zero as PyZero;

static FN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_fn_id() -> u64 {
    FN_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}
