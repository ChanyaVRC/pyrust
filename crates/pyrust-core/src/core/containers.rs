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

/// Shared backing for a Python `tuple` (heap variant, ≥4 elements).  Tuples are
/// immutable, so the backing is a plain `Vec<Value>` behind an `Rc` with no
/// `RefCell` — `Value::clone` / `Insn::Move` is an O(1) strong-count bump that
/// shares the backing instead of deep-copying it (#2268).  `obj_id` is captured
/// at construction and inherited by every aliased clone so `id(x) == id(y)` for
/// `y = x`.  The 2-/3-element shapes stay inline in `Opaque::SmallTuple2/3`.
pub struct TupleInner {
    pub items: Vec<Value>,
    pub obj_id: u64,
}

/// Shared backing for a Python `set`.  Same shape and rationale as
/// [`ListInner`]; `items` is an [`IndexSet`] (insertion-ordered) so iteration
/// order matches the rest of the interpreter's set surface.
pub struct SetInner {
    pub items: RefCell<PySet>,
    pub obj_id: u64,
}

/// Shared version observed only while one or more live dict/set iterators
/// exist for a backing store.
///
/// Mutable containers do not pay for a permanent version field. Iterator
/// creation registers one weak entry keyed by backing identity; mutation
/// helpers bump it only while a matching iterator retains this handle.
/// Iterator generations keep wrapping so a cursor spanning `u64::MAX` still
/// observes the next mutation; long-lived equality caches are disabled by a
/// sticky exhaustion flag after the first wrap.
#[derive(Clone)]
pub struct CollectionMutationState(Rc<CollectionMutationStateInner>);

impl CollectionMutationState {
    #[inline]
    pub fn version(&self) -> u64 {
        self.0.version.get()
    }

    /// Return the version only while equality-based caches may safely use it.
    #[inline]
    pub fn cache_version(&self) -> Option<u64> {
        let version = self.version();
        (!self.0.cache_exhausted.get() && version != u64::MAX).then_some(version)
    }

    /// Validate a cached version without accepting the saturated sentinel.
    #[inline]
    pub fn matches_cache_version(&self, cached: u64) -> bool {
        !self.0.cache_exhausted.get() && cached != u64::MAX && self.version() == cached
    }

    #[inline]
    pub fn same_backing(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// Register the final key of an iterator that has yielded its original
    /// dictionary-entry quota. Returns the iterator's virtual entry cursor.
    ///
    /// CPython distinguishes a temporary insert/remove (normal exhaustion)
    /// from removing and reinserting the iterator's final entry ("dictionary
    /// keys changed"). Only terminal iterators install these watches, so the
    /// ordinary per-item path does not maintain an entry-generation table.
    pub fn watch_key_reinsertion(&self, key: &PyKey) -> usize {
        let mut watchers = self.0.removal_watchers.borrow_mut();
        *watchers.entry(key.clone()).or_insert(0) += 1;
        self.0
            .dict_layout
            .borrow()
            .as_ref()
            .map_or(0, |layout| layout.entry_count)
    }

    /// Release a matching terminal-key removal watch.
    pub fn unwatch_key_reinsertion(&self, key: &PyKey) {
        let mut watchers = self.0.removal_watchers.borrow_mut();
        let remove = match watchers.get_mut(key) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => true,
            None => false,
        };
        if remove {
            watchers.remove(key);
            self.0.pending_removed_keys.borrow_mut().remove(key);
            self.0.reinserted_keys.borrow_mut().remove(key);
        }
    }

    /// Whether a watched key was reinserted at or beyond the iterator's
    /// physical entry cursor after `version`.
    pub fn key_reinserted_at_or_after_since(
        &self,
        key: &PyKey,
        version: u64,
        entry_cursor: usize,
    ) -> bool {
        self.0
            .reinserted_keys
            .borrow()
            .get(key)
            .is_some_and(|(inserted_at, position)| {
                *inserted_at > version && *position >= entry_cursor
            })
    }
}

struct DictIterationLayout {
    live_len: usize,
    entry_count: usize,
    usable: usize,
}

impl DictIterationLayout {
    fn for_len(len: usize) -> Self {
        if len == 0 {
            return Self {
                live_len: 0,
                entry_count: 0,
                usable: 0,
            };
        }
        let mut table_size = 8usize;
        while dict_usable_fraction(table_size) < len {
            table_size = table_size.saturating_mul(2);
        }
        Self {
            live_len: len,
            entry_count: len,
            usable: dict_usable_fraction(table_size).saturating_sub(len),
        }
    }

    fn reset(&mut self) {
        self.live_len = 0;
        self.entry_count = 0;
        self.usable = 0;
    }

    fn remove(&mut self, count: usize) {
        self.live_len = self.live_len.saturating_sub(count);
    }

    /// Record one new key and return its virtual compact-dict entry index.
    fn insert(&mut self) -> usize {
        let position = if self.usable == 0 {
            // CPython grows from `used * 3`, then compacts live entries into
            // the new table. This is why sizes 5, 10, 21, ... do not expose a
            // delete/reinserted final key past an old iterator cursor.
            let target = self.live_len.saturating_mul(3);
            let mut table_size = 8usize;
            while table_size < target {
                table_size = table_size.saturating_mul(2);
            }
            let position = self.live_len;
            self.entry_count = self.live_len.saturating_add(1);
            self.usable = dict_usable_fraction(table_size).saturating_sub(self.entry_count);
            position
        } else {
            let position = self.entry_count;
            self.entry_count = self.entry_count.saturating_add(1);
            self.usable -= 1;
            position
        };
        self.live_len = self.live_len.saturating_add(1);
        position
    }
}

fn dict_usable_fraction(table_size: usize) -> usize {
    table_size.saturating_mul(2) / 3
}

struct CollectionMutationStateInner {
    version: Cell<u64>,
    /// Sticky once the wrapping iterator generation exhausts its first cycle.
    ///
    /// Iterator cursors still need wrapping arithmetic so an iterator created
    /// near `u64::MAX` observes the next mutation. Equality-based long-lived
    /// caches use this flag and remain disabled permanently after wraparound.
    cache_exhausted: Cell<bool>,
    /// Ref-counted because multiple exhausted-at-the-boundary iterators may
    /// watch the same final key.
    removal_watchers: RefCell<HashMap<PyKey, usize>>,
    /// Watched keys currently absent after an entry deletion/table reset.
    pending_removed_keys: RefCell<HashSet<PyKey>>,
    /// Latest reinsertion generation and virtual compact-dict entry position.
    /// Its size is bounded by the number of terminal iterators.
    reinserted_keys: RefCell<HashMap<PyKey, (u64, usize)>>,
    /// CPython-compatible compact-entry accounting, active only while a dict
    /// iterator retains this state.
    dict_layout: RefCell<Option<DictIterationLayout>>,
    key: MutableContainerKey,
    /// Keeps the backing allocation reserved (without keeping its contents
    /// alive), so an address cannot be reused while an iterator still carries
    /// this generation.
    target: MutableContainerTarget,
}

impl Drop for CollectionMutationStateInner {
    fn drop(&mut self) {
        let _ = COLLECTION_MUTATION_STATES.try_with(|states| {
            let mut states = states.borrow_mut();
            let points_to_self = states
                .get(&self.key)
                .is_some_and(|state| std::ptr::eq(state.as_ptr(), self));
            if points_to_self {
                states.remove(&self.key);
            }
        });
        let _ = ACTIVE_COLLECTION_MUTATION_STATES.try_with(|active| {
            active.set(active.get().saturating_sub(1));
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MutableContainerKind {
    Dict,
    Set,
}

enum MutableContainerTarget {
    Dict(Weak<RefCell<PyDict>>),
    Set(Weak<SetInner>),
}

impl MutableContainerTarget {
    fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Dict(left), Self::Dict(right)) => Weak::ptr_eq(left, right),
            (Self::Set(left), Self::Set(right)) => Weak::ptr_eq(left, right),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MutableContainerKey {
    kind: MutableContainerKind,
    ptr: usize,
}

thread_local! {
    static COLLECTION_MUTATION_STATES:
        RefCell<HashMap<MutableContainerKey, Weak<CollectionMutationStateInner>>> =
        RefCell::new(HashMap::new());
    /// Cheap per-interpreter-thread gate for the mutation-state registry.
    ///
    /// Values and their backing stores are thread-confined, so a process-wide
    /// counter made an iterator on one thread force unrelated dict/set writes
    /// on every other thread through a TLS HashMap probe.
    static ACTIVE_COLLECTION_MUTATION_STATES: Cell<usize> = const { Cell::new(0) };
}

fn collection_mutation_state(
    key: MutableContainerKey,
    target: MutableContainerTarget,
) -> CollectionMutationState {
    COLLECTION_MUTATION_STATES.with(|states| {
        let mut states = states.borrow_mut();
        if let Some(state) = states
            .get(&key)
            .and_then(Weak::upgrade)
            .filter(|state| state.target.matches(&target))
        {
            return CollectionMutationState(state);
        }
        if states.len() >= 1024 {
            states.retain(|_, state| state.strong_count() != 0);
        }
        let dict_len = match &target {
            MutableContainerTarget::Dict(dict) => dict.upgrade().map(|dict| dict.borrow().len()),
            MutableContainerTarget::Set(_) => None,
        };
        let state = Rc::new(CollectionMutationStateInner {
            version: Cell::new(0),
            cache_exhausted: Cell::new(false),
            removal_watchers: RefCell::new(HashMap::new()),
            pending_removed_keys: RefCell::new(HashSet::new()),
            reinserted_keys: RefCell::new(HashMap::new()),
            dict_layout: RefCell::new(dict_len.map(DictIterationLayout::for_len)),
            key,
            target,
        });
        states.insert(key, Rc::downgrade(&state));
        ACTIVE_COLLECTION_MUTATION_STATES.with(|active| active.set(active.get().saturating_add(1)));
        CollectionMutationState(state)
    })
}

#[inline(always)]
fn active_collection_mutation_state(
    key: MutableContainerKey,
) -> Option<Rc<CollectionMutationStateInner>> {
    if ACTIVE_COLLECTION_MUTATION_STATES
        .try_with(|active| active.get() == 0)
        .unwrap_or(true)
    {
        return None;
    }
    COLLECTION_MUTATION_STATES.with(|states| states.borrow().get(&key).and_then(Weak::upgrade))
}

struct TrackedDictMutation {
    after_len: usize,
    removed_count: usize,
    inserted_count: usize,
    removed_watched: Vec<PyKey>,
    inserted_watched: Vec<(usize, PyKey)>,
    reset: bool,
}

struct TrackedDictMutationProbe {
    before_len: usize,
    watched_presence: Vec<(PyKey, bool)>,
}

fn begin_tracked_dict_mutation(
    state: &Option<Rc<CollectionMutationStateInner>>,
    dict: &Rc<RefCell<PyDict>>,
) -> Option<TrackedDictMutationProbe> {
    let state = state.as_ref()?;
    state.dict_layout.borrow().as_ref()?;
    let dict = dict.borrow();
    let watched_presence = state
        .removal_watchers
        .borrow()
        .keys()
        .map(|key| (key.clone(), dict.contains_key(key)))
        .collect();
    Some(TrackedDictMutationProbe {
        before_len: dict.len(),
        watched_presence,
    })
}

fn finish_tracked_dict_mutation(
    state: Option<Rc<CollectionMutationStateInner>>,
    before: Option<TrackedDictMutationProbe>,
    dict: &Rc<RefCell<PyDict>>,
    reset: bool,
) {
    let mutation = before.map(|before| {
        let dict = dict.borrow();
        let after_len = dict.len();
        let (removed_count, inserted_count) = if reset {
            (before.before_len, after_len)
        } else if before.before_len <= after_len {
            (0, after_len - before.before_len)
        } else {
            (before.before_len - after_len, 0)
        };
        let base_after_removals = before.before_len.saturating_sub(removed_count);
        let mut removed_watched = Vec::new();
        let mut inserted_watched = Vec::new();
        for (key, was_present) in before.watched_presence {
            let index = dict.get_index_of(&key);
            match (was_present, index) {
                (true, None) => removed_watched.push(key),
                (false, Some(index)) if index >= base_after_removals => {
                    inserted_watched.push((index - base_after_removals, key));
                }
                _ => {}
            }
        }
        inserted_watched.sort_unstable_by_key(|(ordinal, _)| *ordinal);
        TrackedDictMutation {
            after_len,
            removed_count,
            inserted_count,
            removed_watched,
            inserted_watched,
            reset,
        }
    });
    bump_active_collection_mutation_state(state, mutation);
}

fn bump_active_collection_mutation_state(
    state: Option<Rc<CollectionMutationStateInner>>,
    dict_mutation: Option<TrackedDictMutation>,
) {
    let Some(state) = state else {
        return;
    };
    let current_version = state.version.get();
    if current_version == u64::MAX {
        state.cache_exhausted.set(true);
    }
    let next_version = current_version.wrapping_add(1);
    state.version.set(next_version);

    let Some(dict_mutation) = dict_mutation else {
        return;
    };

    {
        let mut pending = state.pending_removed_keys.borrow_mut();
        for key in dict_mutation.removed_watched {
            pending.insert(key);
        }
    }

    let mut layout_ref = state.dict_layout.borrow_mut();
    let Some(layout) = layout_ref.as_mut() else {
        return;
    };
    if dict_mutation.reset {
        layout.reset();
    } else {
        layout.remove(dict_mutation.removed_count);
    }
    let mut inserted_watched = dict_mutation.inserted_watched.into_iter().peekable();
    for ordinal in 0..dict_mutation.inserted_count {
        let position = layout.insert();
        while inserted_watched
            .peek()
            .is_some_and(|(watched_ordinal, _)| *watched_ordinal == ordinal)
        {
            let (_, key) = inserted_watched.next().expect("peeked watched insertion");
            if state.pending_removed_keys.borrow_mut().remove(&key) {
                state
                    .reinserted_keys
                    .borrow_mut()
                    .insert(key, (next_version, position));
            }
        }
    }
    layout.live_len = dict_mutation.after_len;
}

fn bump_collection_mutation_state(key: MutableContainerKey) {
    bump_active_collection_mutation_state(active_collection_mutation_state(key), None);
}

#[inline]
fn dict_container_key(dict: &Rc<RefCell<PyDict>>) -> MutableContainerKey {
    MutableContainerKey {
        kind: MutableContainerKind::Dict,
        ptr: Rc::as_ptr(dict) as usize,
    }
}

#[inline]
fn set_container_key(set: &SetInner) -> MutableContainerKey {
    MutableContainerKey {
        kind: MutableContainerKind::Set,
        ptr: std::ptr::from_ref(set) as usize,
    }
}

/// Register an active iterator against a dict backing store.
pub fn dict_iteration_mutation_state(dict: &Rc<RefCell<PyDict>>) -> CollectionMutationState {
    collection_mutation_state(
        dict_container_key(dict),
        MutableContainerTarget::Dict(Rc::downgrade(dict)),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Opaque — heap-allocated types that don't fit in 48 bits
// ─────────────────────────────────────────────────────────────────────────────

pub enum Opaque {
    PyBigInt(Rc<BigInt>),
    Dict(Rc<RefCell<PyDict>>),
    /// Mutable `set` storage.  Shared via `Rc` so `Value::clone` produces an
    /// alias rather than a deep copy, matching Python's reference semantics.
    /// See [`SetInner`] and issue #305.
    Set(Rc<SetInner>),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    /// Arbitrary-precision `range` whose start/stop/step do not all fit in `i64`
    /// (#2118).  The common i64 case stays on [`Opaque::Range`]; this variant is
    /// only produced when `Value::range_big` sees a bound outside i64.
    BigRange(Rc<BigRangeData>),
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
    /// Proxy returned by the one-argument form `super(cls)` (#2704). It is an
    /// *unbound* super object: `__self__` / `__self_class__` are `None`. It acts
    /// as a descriptor — `super(cls).__get__(obj, owner)` returns a bound
    /// `super(cls, obj)` (or `super(cls, owner)` when `obj` is a class).
    SuperProxyUnbound {
        class: Rc<RefCell<PyClass>>,
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

/// Backing data for an arbitrary-precision `range` (#2118).  `start`/`stop`/`step`
/// are full Python ints; at least one of them is outside the `i64` range (otherwise
/// the value would be stored as the cheaper i64-backed [`Opaque::Range`]).
#[derive(Debug, PartialEq)]
pub struct BigRangeData {
    pub start: BigInt,
    pub stop: BigInt,
    pub step: BigInt,
}

/// Exact element count of an i64-backed Python range.
///
/// The result intentionally uses `i128`: although each bound fits in `i64`,
/// the distance from `i64::MIN` to `i64::MAX` does not. Keeping the
/// subtraction and `-step` out of signed i64 arithmetic also covers
/// `step == i64::MIN` without debug-build panics or release-build wrapping.
///
/// The calculation itself stays in `u64`, so ordinary range length queries do
/// not pay for a software i128 division on platforms without native support.
#[inline]
pub fn range_len(start: i64, stop: i64, step: i64) -> i128 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            0
        } else {
            let distance = stop.abs_diff(start);
            i128::from(((distance - 1) / (step as u64)) + 1)
        }
    } else if start <= stop {
        0
    } else {
        let distance = start.abs_diff(stop);
        i128::from(((distance - 1) / step.unsigned_abs()) + 1)
    }
}

/// Element count of a big range, as CPython computes `len(range(...))`: the count
/// of `start, start+step, …` strictly before `stop`.  Mirrors [`range_len`] but in
/// `BigInt` arithmetic; returns a non-negative `BigInt` (0 when empty).
pub fn bigrange_len(start: &BigInt, stop: &BigInt, step: &BigInt) -> BigInt {
    let zero = BigInt::from(0);
    let one = BigInt::from(1);
    if *step > zero {
        if stop > start {
            (stop - start - &one) / step + &one
        } else {
            zero
        }
    } else if *step < zero {
        if start > stop {
            (start - stop - &one) / (-step) + &one
        } else {
            zero
        }
    } else {
        zero
    }
}

/// CPython `range_equals` (Objects/rangeobject.c) over arbitrary-precision bounds:
/// two ranges are equal iff they yield the same sequence — same length, and (when
/// non-empty) same first element, and (when length ≥ 2) same step.  Mirrors the
/// content-based range hash so equal ranges hash equal.
pub fn bigrange_eq(
    as_: &BigInt,
    ao: &BigInt,
    at: &BigInt,
    bs: &BigInt,
    bo: &BigInt,
    bt: &BigInt,
) -> bool {
    let la = bigrange_len(as_, ao, at);
    let lb = bigrange_len(bs, bo, bt);
    let one = BigInt::from(1);
    let two = BigInt::from(2);
    la == lb && (la < one || as_ == bs) && (la < two || at == bt)
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
            Opaque::BigRange(rc) => Opaque::BigRange(Rc::clone(rc)),
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
            Opaque::SuperProxyUnbound { class, obj_id } => Opaque::SuperProxyUnbound {
                class: Rc::clone(class),
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

#[cfg(test)]
mod mutation_version_tests {
    use super::{
        CollectionMutationState, CollectionMutationStateInner, DictIterationLayout, HashMap,
        HashSet, MutableContainerKey, MutableContainerKind, MutableContainerTarget, PyDict, Rc,
        RefCell, Weak, bump_active_collection_mutation_state,
    };
    use std::cell::Cell;

    #[test]
    fn active_collection_version_wraps_but_permanently_disables_cache_matching() {
        let dict = Rc::new(RefCell::new(PyDict::default()));
        let key = MutableContainerKey {
            kind: MutableContainerKind::Dict,
            ptr: Rc::as_ptr(&dict) as usize,
        };
        let inner = Rc::new(CollectionMutationStateInner {
            version: Cell::new(u64::MAX - 1),
            cache_exhausted: Cell::new(false),
            removal_watchers: RefCell::new(HashMap::new()),
            pending_removed_keys: RefCell::new(HashSet::new()),
            reinserted_keys: RefCell::new(HashMap::new()),
            dict_layout: RefCell::new(Some(DictIterationLayout::for_len(0))),
            key,
            target: MutableContainerTarget::Dict(Weak::clone(&Rc::downgrade(&dict))),
        });
        let state = CollectionMutationState(Rc::clone(&inner));
        state.0.version.set(u64::MAX - 1);
        bump_active_collection_mutation_state(Some(Rc::clone(&inner)), None);
        assert_eq!(state.version(), u64::MAX);
        bump_active_collection_mutation_state(Some(inner), None);
        assert_eq!(state.version(), 0);
        assert_eq!(state.cache_version(), None);
        assert!(!state.matches_cache_version(0));
    }
}
