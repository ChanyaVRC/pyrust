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

// ─────────────────────────────────────────────────────────────────────────────
// Thread-local free lists for fixed-size allocations
// ─────────────────────────────────────────────────────────────────────────────

type FreeListState = (*mut u8, usize);

/// Test-only accounting keyed by the thread whose TLS pool was drained.
///
/// A process-wide before/after counter cannot attribute a drain to one test:
/// libtest may terminate another worker inside that measurement window.  Keep
/// the production pool path free of synchronization while making the teardown
/// assertion deterministic under parallel tests.
#[cfg(test)]
#[derive(Default)]
struct ThreadExitDrainCounts {
    pool_b: usize,
    pool_opaque: usize,
}

#[cfg(test)]
static THREAD_EXIT_DRAIN_COUNTS: std::sync::LazyLock<
    std::sync::Mutex<HashMap<std::thread::ThreadId, ThreadExitDrainCounts>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

#[cfg(test)]
fn record_thread_exit_drains(update: impl FnOnce(&mut ThreadExitDrainCounts)) {
    let mut drains = THREAD_EXIT_DRAIN_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    update(drains.entry(std::thread::current().id()).or_default());
}

#[cfg(test)]
fn take_thread_exit_drains(thread_id: std::thread::ThreadId) -> ThreadExitDrainCounts {
    THREAD_EXIT_DRAIN_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&thread_id)
        .unwrap_or_default()
}

/// Deallocate every block currently retained by one thread-local free list.
///
/// The state is cleared before deallocation so a later destructor cannot
/// observe pointers that have already been released.
unsafe fn drain_free_list(state: &Cell<FreeListState>, layout: Layout) -> usize {
    let (mut head, expected_len) = state.replace((std::ptr::null_mut(), 0));
    let mut drained = 0;
    while !head.is_null() {
        let next = unsafe { *(head as *const *mut u8) };
        unsafe { dealloc(head, layout) };
        head = next;
        drained += 1;
    }
    debug_assert_eq!(
        drained, expected_len,
        "free-list length disagrees with linked blocks"
    );
    drained
}

struct PoolB(Cell<FreeListState>);

impl PoolB {
    const fn new() -> Self {
        Self(Cell::new((std::ptr::null_mut(), 0)))
    }
}

impl Drop for PoolB {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(STR_SLICE_LAYOUT_SIZE, 8)
            .expect("valid string-slice pool layout");
        let _drained = unsafe { drain_free_list(&self.0, layout) };
        #[cfg(test)]
        record_thread_exit_drains(|counts| counts.pool_b += _drained);
    }
}

// Each free slot stores a *mut u8 to the next free slot in its first 8 bytes.
thread_local! {
    static POOL_B: PoolB = const { PoolB::new() };
}

const POOL_B_CAP: usize = 64;

#[inline(always)]
unsafe fn pool_b_alloc() -> *mut u8 {
    let layout =
        Layout::from_size_align(STR_SLICE_LAYOUT_SIZE, 8).expect("valid string-slice pool layout");
    POOL_B
        .try_with(|c| {
            let (head, len) = c.0.get();
            if len > 0 {
                let next = unsafe { *(head as *const *mut u8) };
                c.0.set((next, len - 1));
                head
            } else {
                unsafe { alloc_or_handle(layout) }
            }
        })
        // A destructor from another TLS key may allocate after this pool has
        // already been destroyed. It cannot re-enter the free list, but a
        // matched direct allocation remains valid for the rest of that drop.
        .unwrap_or_else(|_| unsafe { alloc_or_handle(layout) })
}

#[inline(always)]
unsafe fn pool_b_dealloc(ptr: *mut u8) {
    let layout =
        Layout::from_size_align(STR_SLICE_LAYOUT_SIZE, 8).expect("valid string-slice pool layout");
    if POOL_B
        .try_with(|c| {
            let (head, len) = c.0.get();
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
                c.0.set((ptr, len + 1));
            } else {
                unsafe { dealloc(ptr, layout) };
            }
        })
        .is_err()
    {
        // TLS keys are destroyed in reverse initialization order. A value kept
        // by an older key can therefore outlive this pool and must bypass it.
        unsafe { dealloc(ptr, layout) };
    }
}

/// Refcounted slab payload for [`TAG_OPAQUE`].
///
/// `Value` itself remains one NaN-boxed word.  Clones point at the same slot and
/// increment `strong`, avoiding both a slab allocation and a clone of the
/// `Opaque` enum (including its nested `Value`s).  Values are confined to the
/// interpreter thread (opaque variants contain `Rc` and the slab pool is
/// thread-local), so a non-atomic `Cell` matches the existing ownership model.
#[repr(C)]
struct OpaqueSlot {
    strong: Cell<usize>,
    value: Opaque,
}

// Pool for `OpaqueSlot` allocations (#281).  The fixed-size slabs are recycled
// after the final aliased `Value` is dropped.  The pool is per-thread and
// bounded, so unused capacity cannot grow without limit.
const POOL_OPAQUE_CAP: usize = 128;

struct PoolOpaque(Cell<FreeListState>);

impl PoolOpaque {
    const fn new() -> Self {
        Self(Cell::new((std::ptr::null_mut(), 0)))
    }
}

impl Drop for PoolOpaque {
    fn drop(&mut self) {
        let _drained = unsafe { drain_free_list(&self.0, opaque_layout()) };
        #[cfg(test)]
        record_thread_exit_drains(|counts| counts.pool_opaque += _drained);
    }
}

thread_local! {
    static POOL_OPAQUE: PoolOpaque = const { PoolOpaque::new() };
}

#[inline(always)]
fn opaque_layout() -> Layout {
    Layout::new::<OpaqueSlot>()
}

#[inline(always)]
unsafe fn pool_opaque_alloc() -> *mut u8 {
    let layout = opaque_layout();
    POOL_OPAQUE
        .try_with(|c| {
            let (head, len) = c.0.get();
            if len > 0 {
                let next = unsafe { *(head as *const *mut u8) };
                c.0.set((next, len - 1));
                head
            } else {
                unsafe { alloc_or_handle(layout) }
            }
        })
        .unwrap_or_else(|_| unsafe { alloc_or_handle(layout) })
}

#[inline(always)]
unsafe fn pool_opaque_dealloc(ptr: *mut u8) {
    let layout = opaque_layout();
    if POOL_OPAQUE
        .try_with(|c| {
            let (head, len) = c.0.get();
            if len < POOL_OPAQUE_CAP {
                unsafe { *(ptr as *mut *mut u8) = head };
                c.0.set((ptr, len + 1));
            } else {
                unsafe { dealloc(ptr, layout) };
            }
        })
        .is_err()
    {
        unsafe { dealloc(ptr, layout) };
    }
}

// The pre-#2268 `Vec<Value>` struct-header pool (32-byte slab, manual obj_id at
// offset 24) backed heap tuples.  Tuples moved to an `Rc<TupleInner>` payload in
// #2268 (matching the list `Rc<ListInner>` move from #305) so `Value::clone` is
// an O(1) refcount bump rather than a deep copy; the pool is gone with it.

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
