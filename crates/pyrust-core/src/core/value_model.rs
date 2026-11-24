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
    Dict(std::cell::Ref<'a, PyDict>),
    /// Borrowed view of a set.  Same `Ref` guard rationale as
    /// [`Self::List`] / [`Self::Dict`] (#450).
    Set(std::cell::Ref<'a, PySet>),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    /// Arbitrary-precision `range` view (#2118).
    BigRange {
        start: &'a BigInt,
        stop: &'a BigInt,
        step: &'a BigInt,
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
    SuperProxyUnbound {
        class: &'a Rc<RefCell<PyClass>>,
    },
    Generator(&'a Rc<GeneratorCell>),
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

/// Refcounted slab payload for [`TAG_OPAQUE`].
///
/// `Value` itself remains one NaN-boxed word.  Clones point at the same slot and
/// increment `strong`, avoiding both a slab allocation and a clone of the
/// `Opaque` enum (including its nested `Value`s).  Values are confined to the
/// interpreter thread (opaque variants contain `Rc` and the slab pool in
/// [`crate::pool`] is thread-local), so a non-atomic `Cell` matches the
/// existing ownership model.
#[repr(C)]
struct OpaqueSlot {
    strong: Cell<usize>,
    value: Opaque,
}

#[inline(always)]
pub(crate) fn opaque_layout() -> Layout {
    Layout::new::<OpaqueSlot>()
}

// The pre-#2268 `Vec<Value>` struct-header pool (32-byte slab, manual obj_id at
// offset 24) backed heap tuples.  Tuples moved to an `Rc<TupleInner>` payload in
// #2268 (matching the list `Rc<ListInner>` move from #305) so `Value::clone` is
// an O(1) refcount bump rather than a deep copy; the pool is gone with it.
// Remaining fixed-size slab pools (`POOL_B` and `POOL_OPAQUE`) live in
// `crate::pool`.

// ─────────────────────────────────────────────────────────────────────────────
// Value — NaN-boxed u64
// ─────────────────────────────────────────────────────────────────────────────

type ThreadBoundValueMarker = std::marker::PhantomData<Rc<()>>;

/// One-word Python value handle.
///
/// The bits may encode pointers to thread-confined `Rc`, `RefCell`, `Cell`, and
/// thread-local pool allocations.  The zero-sized marker therefore deliberately
/// keeps `Value` from implementing `Send` or `Sync`; moving an aliased value to
/// another thread would otherwise make its non-atomic reference counts racy.
///
/// ```compile_fail
/// use pyrust_core::Value;
///
/// fn require_send<T: Send>() {}
/// require_send::<Value>();
/// ```
///
/// ```compile_fail
/// use pyrust_core::Value;
///
/// fn require_sync<T: Sync>() {}
/// require_sync::<Value>();
/// ```
#[repr(transparent)]
pub struct Value(u64, ThreadBoundValueMarker);

impl Value {
    #[inline(always)]
    const fn from_bits(bits: u64) -> Self {
        Self(bits, std::marker::PhantomData)
    }
}
