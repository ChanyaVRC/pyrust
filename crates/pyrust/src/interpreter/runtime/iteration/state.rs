/// Small source list used by native `map` and related adapters.
pub(crate) type IterSrcBuf = smallvec::SmallVec<[Value; 2]>;

/// A `bytearray`'s shared storage cell, as handed out by
/// `pyrust_builtins::bytearray::as_bytearray_rc`.
pub(crate) type ByteArrayBuffer = Rc<RefCell<Vec<u8>>>;

/// Backing source for a built-in iterator object.
pub(crate) enum NativeIterSource {
    Materialized(Vec<Value>),
    /// Live dict/set key cursor. Dict cursors retain CPython's initial
    /// remaining-key count so replacing an already-yielded key reports
    /// "dictionary keys changed"; set cursors intentionally have no such
    /// limit and may observe an inserted replacement.
    LiveKeys {
        container: Value,
        cursor: Box<LiveKeyCursor>,
    },
    /// Independent live cursor over an instance's reusable `__dict__` proxy.
    ///
    /// The proxy itself is iterable but is not an iterator. Each `iter(proxy)`
    /// constructs a distinct source with its own position and permanent
    /// size-change latch, matching a native dict-key iterator.
    InstanceDict {
        proxy: Value,
        recorded_len: usize,
        size_changed: bool,
    },
    /// Live list/tuple source with CPython-compatible index-walk mutation
    /// behavior.
    Indexed(Value),
    /// Reverse sequence walk. The initial length is captured at iterator
    /// construction; each step reads the then-current value at the next
    /// descending index, so element replacement is visible while later
    /// appends are not.
    ReverseIndexed {
        value: Value,
        next_index: usize,
    },
    /// Reverse walk over a live mapping's entries (`reversed(dict)` and the
    /// three reversed dict views).
    ///
    /// Boxed so the reverse cursor's state does not widen every native
    /// iterator frame — a frame is written whole on each loop entry and copied
    /// with the frame of every suspended generator.
    ReverseDict(Box<ReverseDictCursor>),
    /// Dict-view keys are snapshotted for stable order while values are read
    /// from the live backing mapping on each step.
    DictView {
        dict: pyrust_builtins::dict_views::DictRc,
        keys: Vec<PyKey>,
        kind: pyrust_builtins::dict_views::DictViewKind,
    },
    /// Live `collections.deque` buffer. A separate mutation-state guard
    /// rejects structural changes before the next item is observed.
    Deque(pyrust_builtins::deque_storage::DequeData),
    /// Immutable bytes indexed lazily, one integer per step.
    Bytes(Value),
    /// Live `bytearray` buffer walked by index.
    ///
    /// CPython's `bytearray_iterator` holds the object plus a position and
    /// re-reads the buffer's size on every step, so bytes appended mid-walk are
    /// yielded and a shrink below the cursor ends the walk (#2921).
    ///
    /// The storage cell *is* the retained source: a `bytearray` allocates it
    /// once and only ever writes it in place, so it stays the object's live
    /// backing and outlives every binding to it — the walk keeps reading the
    /// same buffer after the source variable is rebound or deleted. Resolving
    /// it at construction also keeps each step a single indexed read.
    Bytearray(ByteArrayBuffer),
    /// Immutable UTF-8/CESU-8 string with an incremental byte cursor.
    String {
        value: Value,
        byte_pos: usize,
    },
    /// Permanently released after exhaustion or transfer to a loop fast path.
    Exhausted,
}

/// Descending entry cursor over a live mapping.
///
/// CPython's reverse dict iterators are a descending index into the mapping's
/// entry array: both the key and the value are read from the entry the cursor
/// reaches, so a value replaced mid-walk is observed (#2932). An unchanged
/// mutation generation proves insertion order is frozen, which makes the
/// descending position the entry's identity and reduces the ordinary step to
/// one generation compare plus one indexed read. A changed generation drops
/// back to the frame's size guard before the entry is read.
pub(crate) struct ReverseDictCursor {
    /// The `dict`, dict view, `mappingproxy`, or subclass carrier whose live
    /// entries are read.
    pub(crate) container: Value,
    /// Backing mapping resolved once, for containers whose backing identity
    /// cannot move. `None` re-probes `container` on every step.
    pub(crate) backing: Option<pyrust_builtins::dict_views::DictRc>,
    pub(crate) mutation_state: Option<pyrust_core::CollectionMutationState>,
    pub(crate) observed_mutation: u64,
    /// Entries still below the cursor. The next entry read is at index
    /// `next_index - 1`, so this is also the walk's remaining count.
    pub(crate) next_index: usize,
    /// Key of the entry yielded last, used to re-anchor `next_index` after the
    /// mapping is written. Kept only while the walk is live.
    pub(crate) last_key: Option<PyKey>,
    /// The mutation generation moved, so the raw position must be re-anchored
    /// before the next entry is read.
    pub(crate) relocate: bool,
    /// 0 = key, 1 = value, 2 = item pair.
    pub(crate) kind: u8,
    /// Builtin subclasses can replace `__builtin_data__`; primitive containers
    /// and views have a stable backing identity.
    pub(crate) dynamic_backing: bool,
    /// A size change has been reported. The generation shortcut must not
    /// silence the frame's guard again, because CPython keeps re-raising for
    /// the rest of the iterator's life (#2915).
    pub(crate) size_changed: bool,
}

#[derive(Clone)]
pub(crate) struct LiveKeyCursor {
    /// Keys yielded before the first mutation. Appending to a Vec keeps the
    /// ordinary non-mutating walk free of per-item hashing.
    pub(crate) yielded: Vec<PyKey>,
    /// Once a live walk reaches the adaptive threshold without mutation, the
    /// complete current key order is captured once. `snapshot_pos` divides
    /// the already-yielded prefix from the remaining fast indexed suffix.
    pub(crate) snapshot: Option<Vec<PyKey>>,
    /// A small key or set walk's frozen order, already in yielded form.
    ///
    /// Mutually exclusive with `snapshot`: the walk holds exactly one
    /// representation of its order, and converts to `snapshot` the moment it
    /// needs the original keys back.
    pub(crate) frozen_keys: Option<Vec<Value>>,
    pub(crate) snapshot_pos: usize,
    /// Backing mapping resolved once, for containers whose backing identity
    /// cannot move. While the mutation generation is unchanged the insertion
    /// order is frozen, so `snapshot_pos` is also the live entry position and
    /// values are read positionally instead of by key.
    pub(crate) backing: Option<pyrust_builtins::dict_views::DictRc>,
    /// Allocated only after a mutation generation changes, when restarting the
    /// index walk may otherwise yield an earlier key twice.
    ///
    /// Boxed so the recovery table's inline size does not sit in every loop
    /// slot: the cursor is written whole on each loop entry and copied with the
    /// frame of every suspended generator.
    pub(crate) seen: Option<Box<pyrust_core::PySet>>,
    pub(crate) next_index: usize,
    pub(crate) last_key: Option<PyKey>,
    pub(crate) last_index: usize,
    pub(crate) mutation_state: Option<pyrust_core::CollectionMutationState>,
    pub(crate) observed_mutation: u64,
    /// Initial size and error wording used only after the mutation generation
    /// changes. Non-mutating iteration therefore avoids a RefCell length
    /// borrow on every element.
    pub(crate) recorded_len: usize,
    pub(crate) size_change_message: &'static str,
    /// Builtin subclasses can replace `__builtin_data__`; primitive containers
    /// and views have a stable backing identity.
    pub(crate) dynamic_backing: bool,
    /// `Some(n)` for dict/dict-view iterators; `None` for set iterators.
    pub(crate) remaining: Option<usize>,
    /// 0 = key, 1 = value, 2 = item pair, 3 = set key.
    pub(crate) kind: u8,
    /// CPython permanently exhausts a dict iterator after the one-shot
    /// "dictionary keys changed" error.
    pub(crate) keys_changed: bool,
    /// A size change is the one terminal state that keeps raising. CPython
    /// stamps `di_used` / `si_used` with -1 and compares against it on every
    /// later step, so the same `RuntimeError` is re-raised for the rest of the
    /// iterator's life — including after the container is restored to its
    /// original size, and for every native consumer that drains it. This
    /// outlives [`LiveKeyCursor::release`], which only drops the walk's
    /// storage and its mutation registration.
    pub(crate) size_changed: bool,
    /// Once a dict iterator has yielded its original key quota, keep a
    /// one-key removal watch until the next advance. This distinguishes
    /// delete/reinsert of the final entry from an unrelated temporary
    /// insert/remove without adding bookkeeping to every yielded key.
    pub(crate) watching_terminal_key: bool,
    /// Virtual compact-dict entry position immediately after the final
    /// originally-yielded key.
    pub(crate) terminal_entry_cursor: usize,
    /// The last yielded key moved or disappeared. With the interpreter's
    /// shift-removing maps this is the typed structural-mutation marker used
    /// to restart the live index walk.
    pub(crate) structurally_changed: bool,
    /// Releases incremental key history and the active mutation registration
    /// as soon as the cursor reaches its terminal state.
    pub(crate) exhausted: bool,
}

impl LiveKeyCursor {
    pub(crate) fn dict(container: &Value, kind: u8, len: usize) -> Self {
        let mutation_state = live_collection_mutation_state(container)
            .expect("live dict cursor requires mutation state");
        let observed_mutation = mutation_state.version();
        let dynamic_backing = container.as_py_instance_rc().is_some();
        let frozen_keys = initial_frozen_key_order(container, dynamic_backing, kind, len);
        Self {
            yielded: Vec::new(),
            snapshot: (frozen_keys.is_none())
                .then(|| initial_key_snapshot(container, dynamic_backing, len))
                .flatten(),
            frozen_keys,
            snapshot_pos: 0,
            backing: stable_cursor_backing(container, dynamic_backing),
            seen: None,
            next_index: 0,
            last_key: None,
            last_index: 0,
            mutation_state: Some(mutation_state),
            observed_mutation,
            recorded_len: len,
            size_change_message: "dictionary changed size during iteration",
            dynamic_backing,
            remaining: Some(len),
            kind,
            keys_changed: false,
            size_changed: false,
            watching_terminal_key: false,
            terminal_entry_cursor: 0,
            structurally_changed: false,
            exhausted: false,
        }
    }

    pub(crate) fn set(container: &Value) -> Self {
        let len = live_collection_len(container).unwrap_or(0);
        let mutation_state = live_collection_mutation_state(container)
            .expect("live set cursor requires mutation state");
        let observed_mutation = mutation_state.version();
        let dynamic_backing = container.as_py_instance_rc().is_some();
        let frozen_keys = initial_frozen_key_order(container, dynamic_backing, 3, len);
        Self {
            yielded: Vec::new(),
            snapshot: (frozen_keys.is_none())
                .then(|| initial_key_snapshot(container, dynamic_backing, len))
                .flatten(),
            frozen_keys,
            snapshot_pos: 0,
            backing: None,
            seen: None,
            next_index: 0,
            last_key: None,
            last_index: 0,
            mutation_state: Some(mutation_state),
            observed_mutation,
            recorded_len: len,
            size_change_message: "Set changed size during iteration",
            dynamic_backing,
            remaining: None,
            kind: 3,
            keys_changed: false,
            size_changed: false,
            watching_terminal_key: false,
            terminal_entry_cursor: 0,
            structurally_changed: false,
            exhausted: false,
        }
    }

    pub(crate) fn with_size_change_message(mut self, message: &'static str) -> Self {
        self.size_change_message = message;
        self
    }

    pub(crate) fn yielded_len(&self) -> usize {
        if let Some(seen) = &self.seen {
            return seen.len();
        }
        if self.snapshot.is_some() || self.frozen_keys.is_some() {
            return self.snapshot_pos;
        }
        self.yielded.len()
    }

    pub(crate) fn release(&mut self) {
        if self.watching_terminal_key
            && let (Some(state), Some(key)) = (&self.mutation_state, &self.last_key)
        {
            state.unwatch_key_reinsertion(key);
        }
        self.watching_terminal_key = false;
        self.terminal_entry_cursor = 0;
        self.yielded = Vec::new();
        if let Some(snapshot) = self.snapshot.take() {
            release_key_snapshot_buffer(snapshot);
        }
        if let Some(frozen) = self.frozen_keys.take() {
            release_frozen_key_buffer(frozen);
        }
        self.snapshot_pos = 0;
        self.backing = None;
        self.seen = None;
        self.last_key = None;
        self.mutation_state = None;
        self.exhausted = true;
    }
}

/// Type-erased state stored by built-in iterator objects.
pub(crate) struct NativeIterFrame {
    pub(crate) source: NativeIterSource,
    pub(crate) pos: usize,
    pub(crate) type_name: &'static str,
    pub(crate) guard: Option<Box<NativeIterGuard>>,
    pub(crate) exhausted: bool,
}

/// Live-collection mutation guard attached to a native iterator.
pub(crate) struct NativeIterGuard {
    pub(crate) container: Value,
    pub(crate) version: i64,
    pub(crate) kind: GuardVersion,
    pub(crate) msg: &'static str,
    pub(crate) exhaust_first: bool,
    pub(crate) provider_sequence: u64,
}

#[derive(Clone)]
pub(crate) enum GuardVersion {
    Size,
    /// Safe shared structural-mutation state owned by opaque deque storage.
    DequeState {
        counter: pyrust_builtins::deque_storage::DequeMutationState,
    },
}

/// Lazy legacy sequence iterator over `obj.__getitem__(0..)`.
pub(crate) struct GetItemIter {
    pub(crate) obj: Value,
    pub(crate) method: Value,
    /// `reversed()` retains the sequence's `__len__` slot for its live
    /// length hint. Forward legacy sequence iterators do not have one.
    pub(crate) length_method: Option<Value>,
    pub(crate) index: i64,
    pub(crate) step: i64,
    pub(crate) remaining: Option<usize>,
    pub(crate) exhausted: bool,
}

/// Iterator created by two-argument `iter(callable, sentinel)`.
pub(crate) struct CallableIter {
    pub(crate) callable: Value,
    pub(crate) sentinel: Value,
    pub(crate) done: bool,
}

pub(crate) struct MapIter {
    pub(crate) func: Value,
    pub(crate) sources: IterSrcBuf,
    pub(crate) done: bool,
}

pub(crate) struct FilterIter {
    pub(crate) func: Option<Value>,
    pub(crate) source: Value,
    pub(crate) done: bool,
}

/// One-step callback for an iterator whose concrete cursor is owned by a
/// standard-library provider.
///
/// The generic iteration domain stores only this typed interface. The provider
/// owns the opaque state, algorithm, diagnostics, and Python presentation
/// names, while `call_next`, materialisation, and loop dispatch share this
/// single advancement hook.
pub(crate) type ProviderIteratorAdvance =
    fn(&mut Interpreter, &Rc<RefCell<Box<dyn std::any::Any>>>) -> Result<Option<Value>>;

/// Type-erased standard-library iterator adapter.
pub(crate) struct ProviderIterator {
    state: Rc<RefCell<Box<dyn std::any::Any>>>,
    advance: ProviderIteratorAdvance,
    full_type_name: &'static str,
    fallback_class_name: &'static str,
    class: Option<Rc<RefCell<crate::value::PyClass>>>,
}

impl ProviderIterator {
    pub(crate) fn new(
        state: Box<dyn std::any::Any>,
        advance: ProviderIteratorAdvance,
        full_type_name: &'static str,
        fallback_class_name: &'static str,
        class: Option<Rc<RefCell<crate::value::PyClass>>>,
    ) -> Self {
        Self {
            state: Rc::new(RefCell::new(state)),
            advance,
            full_type_name,
            fallback_class_name,
            class,
        }
    }

    #[inline]
    pub(crate) fn advance_parts(
        &self,
    ) -> (ProviderIteratorAdvance, Rc<RefCell<Box<dyn std::any::Any>>>) {
        (self.advance, Rc::clone(&self.state))
    }

    #[inline]
    pub(crate) fn full_type_name(&self) -> &'static str {
        self.full_type_name
    }

    #[inline]
    pub(crate) fn class(&self) -> Option<Rc<RefCell<crate::value::PyClass>>> {
        self.class.as_ref().map(Rc::clone)
    }

    #[inline]
    pub(crate) fn fallback_class_name(&self) -> &'static str {
        self.fallback_class_name
    }
}

pub(crate) struct EnumerateIter {
    pub(crate) source: Value,
    pub(crate) counter: Value,
    pub(crate) done: bool,
}

/// Loop cursor for `enumerate(...)` over a built-in element iterator.
///
/// Both handles are the cells the two Python objects already own, so the
/// counter and the element position stay shared: an aliased `next()` on the
/// enumerate — or on the inner iterator it was built from — interleaves with
/// the loop exactly as it does on the generic adapter path.
#[derive(Clone)]
pub(crate) struct EnumerateElementCursor {
    /// The `enumerate` object's own state cell, holding the running counter.
    pub(crate) enumerate: Rc<RefCell<Box<dyn std::any::Any>>>,
    /// The enumerate's inner iterator cell.
    pub(crate) inner: EnumerateInnerCursor,
}

/// The inner iterator cell an [`EnumerateElementCursor`] steps.
///
/// Each variant names the concrete state the cell was proven to hold when the
/// loop classified it. A cell's state type never changes, so one classification
/// covers the whole walk.
#[derive(Clone)]
pub(crate) enum EnumerateInnerCursor {
    /// A [`NativeIterFrame`] whose source is an unguarded element walk over a
    /// list, tuple, snapshot, or an immutable `bytes` / `str`.
    Frame(Rc<RefCell<Box<dyn std::any::Any>>>),
    /// A [`RangeIter`], the canonical i64-backed range cursor. Elements are
    /// generated from the cursor rather than read out of storage.
    Range(Rc<RefCell<Box<dyn std::any::Any>>>),
}

pub(crate) struct BigRangeIter {
    pub(crate) cur: PyBigInt,
    pub(crate) stop: PyBigInt,
    pub(crate) step: PyBigInt,
}

/// Lazy iterator for the common i64-backed `range`.
///
/// Direct `for x in range(...)` loops use [`IterState::Range`]. This state is
/// the iterator-object counterpart used by `iter(range)`, `map`, `zip`, and
/// every other consumer that must retain an independent cursor without
/// materialising the range.
pub(crate) struct RangeIter {
    pub(crate) cur: i64,
    pub(crate) stop: i64,
    pub(crate) step: i64,
}

pub(crate) struct ZipIter {
    pub(crate) sources: Vec<Value>,
    pub(crate) strict: bool,
    pub(crate) done: bool,
    pub(crate) count: usize,
}
