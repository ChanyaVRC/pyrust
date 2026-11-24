/// Shared version observed only while one or more live dict/set iterators
/// exist for a backing store.
///
/// Mutable containers do not pay for a permanent version field. Iterator
/// creation registers one weak entry keyed by backing identity; mutation
/// helpers bump it only while a matching iterator retains this handle.
/// Iterator generations keep wrapping so a cursor spanning `u64::MAX` still
/// observes the next mutation; long-lived equality caches are disabled by a
/// sticky exhaustion flag after the first wrap.
///
/// Existence of this handle — not existence of the registration — is what
/// makes a backing store observed. A released registration may be retained
/// for reuse (see [`retain_idle_mutation_state`]) without making unrelated
/// writes pay for generation tracking.
pub struct CollectionMutationState(Rc<CollectionMutationStateInner>);

impl Clone for CollectionMutationState {
    fn clone(&self) -> Self {
        Self::attach(Rc::clone(&self.0))
    }
}

impl Drop for CollectionMutationState {
    fn drop(&mut self) {
        let remaining = self.0.observers.get().saturating_sub(1);
        self.0.observers.set(remaining);
        if remaining == 0 {
            let _ = ACTIVE_COLLECTION_MUTATION_STATES.try_with(|active| {
                active.set(active.get().saturating_sub(1));
            });
        }
    }
}

impl CollectionMutationState {
    /// Take an observing handle, enabling generation tracking for the backing
    /// store while at least one handle is alive.
    fn attach(inner: Rc<CollectionMutationStateInner>) -> Self {
        let observers = inner.observers.get();
        inner.observers.set(observers + 1);
        if observers == 0 {
            ACTIVE_COLLECTION_MUTATION_STATES
                .with(|active| active.set(active.get().saturating_add(1)));
        }
        Self(inner)
    }

    #[inline]
    pub fn version(&self) -> u64 {
        self.0.version.get()
    }

    /// Entry-order generation — CPython's `od_state`. See the field's docs.
    #[inline]
    pub fn entry_order_version(&self) -> u64 {
        self.0.entry_order.get()
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
        match watchers.iter_mut().find(|(watched, _)| watched == key) {
            Some((_, count)) => *count += 1,
            None => watchers.push((key.clone(), 1)),
        }
        self.0.has_watchers.set(true);
        if self.0.has_dict_layout.get() {
            self.0.dict_layout_entry_count.get()
        } else {
            0
        }
    }

    /// Release a matching terminal-key removal watch.
    pub fn unwatch_key_reinsertion(&self, key: &PyKey) {
        let mut watchers = self.0.removal_watchers.borrow_mut();
        let Some(index) = watchers.iter().position(|(watched, _)| watched == key) else {
            return;
        };
        if watchers[index].1 > 1 {
            watchers[index].1 -= 1;
            return;
        }
        watchers.swap_remove(index);
        if watchers.is_empty() {
            self.0.has_watchers.set(false);
        }
        drop(watchers);
        // Both follow-up tables are populated only by a mutation observed while
        // a watch was installed; skip both RefCell borrows on the common
        // unmutated walk (#2924).
        if !self.0.has_mutation_activity.get() {
            return;
        }
        let mut pending = self.0.pending_removed_keys.borrow_mut();
        if let Some(index) = pending.iter().position(|pending| pending == key) {
            pending.swap_remove(index);
        }
        drop(pending);
        let mut reinserted = self.0.reinserted_keys.borrow_mut();
        if let Some(index) = reinserted
            .iter()
            .position(|(reinserted, _, _)| reinserted == key)
        {
            reinserted.swap_remove(index);
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
            .iter()
            .any(|(reinserted, inserted_at, position)| {
                reinserted == key && *inserted_at > version && *position >= entry_cursor
            })
    }
}

struct DictIterationLayout {
    live_len: usize,
    entry_count: usize,
    usable: usize,
}

impl DictIterationLayout {
    #[inline]
    fn for_len(len: usize) -> Self {
        if len == 0 {
            return Self {
                live_len: 0,
                entry_count: 0,
                usable: 0,
            };
        }
        // Smallest power of two >= 8 with `floor(2 * table_size / 3) >= len`,
        // i.e. `table_size >= ceil(3 * len / 2)`.
        let min_size = len.saturating_add(len / 2).saturating_add(len & 1);
        let table_size = min_size
            .checked_next_power_of_two()
            .unwrap_or(usize::MAX)
            .max(8);
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
    /// Generation of the backing store's *entry order* alone, advanced by a
    /// mutation that adds/removes an entry or explicitly relinks an existing
    /// entry — never by rewriting an existing key's value, and never by a
    /// table reset.
    ///
    /// This is CPython's `od_state`, which `OrderedDict` iterators compare
    /// per step: relinking a node (`move_to_end`, a delete, an insert of a new
    /// key) invalidates the walk even when the mapping's length is restored
    /// before the iterator looks again, while `od[k] = v` for a key already
    /// present leaves the walk valid. `clear()` is the documented exception —
    /// CPython reports it on the size arm instead, so a reset must leave this
    /// counter alone (issue #2931).
    entry_order: Cell<u64>,
    /// Sticky once the wrapping iterator generation exhausts its first cycle.
    ///
    /// Iterator cursors still need wrapping arithmetic so an iterator created
    /// near `u64::MAX` observes the next mutation. Equality-based long-lived
    /// caches use this flag and remain disabled permanently after wraparound.
    cache_exhausted: Cell<bool>,
    /// Live handles. Generation tracking costs unrelated writes nothing while
    /// this is zero, so a retained-but-unobserved registration is as cheap as
    /// no registration at all.
    observers: Cell<usize>,
    /// Ref-counted because multiple exhausted-at-the-boundary iterators may
    /// watch the same final key.
    ///
    /// All three watch tables are bounded by the number of iterators sitting
    /// exactly on their terminal entry — one in practice — so linear scans
    /// avoid hashing a `PyKey` on the loop-entry and loop-exit paths.
    removal_watchers: RefCell<Vec<(PyKey, usize)>>,
    /// True while `removal_watchers` is non-empty, avoiding a `RefCell` borrow
    /// on idle restart and mutation probes when no terminal watch is installed.
    has_watchers: Cell<bool>,
    /// True once a tracked mutation has populated `pending_removed_keys` or
    /// `reinserted_keys` since the last idle restart.
    has_mutation_activity: Cell<bool>,
    /// Watched keys currently absent after an entry deletion/table reset.
    pending_removed_keys: RefCell<Vec<PyKey>>,
    /// Latest reinsertion generation and virtual compact-dict entry position.
    /// Its size is bounded by the number of terminal iterators.
    reinserted_keys: RefCell<Vec<(PyKey, u64, usize)>>,
    /// CPython-compatible compact-entry accounting stored in plain `Cell`s so
    /// idle restarts and terminal-key watches avoid `RefCell` borrows (#2924).
    has_dict_layout: Cell<bool>,
    dict_layout_live_len: Cell<usize>,
    dict_layout_entry_count: Cell<usize>,
    dict_layout_usable: Cell<usize>,
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

    #[inline]
    fn matches_dict(&self, dict: &Rc<RefCell<PyDict>>) -> bool {
        matches!(self, Self::Dict(left) if std::ptr::eq(left.as_ptr(), Rc::as_ptr(dict)))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MutableContainerKey {
    kind: MutableContainerKind,
    ptr: usize,
}

// Thread-local justification:
// (a) Why `thread_local!`: Holds `!Send` `Rc`/`Weak<CollectionMutationStateInner>`
//     entries and a non-atomic `Cell<usize>` fast-path gate keyed by
//     thread-confined container addresses.
// (b) Multi-interpreter / multi-threaded impact: Multiple interpreters on one
//     thread share the address-keyed registry safely (distinct containers have
//     distinct addresses; idle cache holds weak targets only); separate threads
//     never perturb each other's dict/set mutation fast path.
thread_local! {
    /// Keyed by backing address, so the registry is a pointer-identity lookup
    /// on the loop-entry and tracked-write paths. The stdlib's DoS-resistant
    /// default hash costs more than the lookup it protects.
    static COLLECTION_MUTATION_STATES:
        RefCell<HashMap<MutableContainerKey, Weak<CollectionMutationStateInner>, FxBuildHasher>> =
        RefCell::new(HashMap::default());
    /// Cheap per-interpreter-thread gate for the mutation-state registry.
    ///
    /// Values and their backing stores are thread-confined, so a process-wide
    /// counter made an iterator on one thread force unrelated dict/set writes
    /// on every other thread through a TLS HashMap probe.
    static ACTIVE_COLLECTION_MUTATION_STATES: Cell<usize> = const { Cell::new(0) };
    /// Recently registered states kept alive after their last observer left.
    ///
    /// Registration is the dominant cost of entering a loop over a small
    /// dict/set: allocating the state, seeding its layout, publishing the weak
    /// entry, and tearing all of it down again one loop later. Retaining the
    /// last few registrations turns a repeated `for k in d` into a registry
    /// hit. Only weak container targets are held, so this never extends the
    /// lifetime of a container or its contents.
    static IDLE_COLLECTION_MUTATION_STATES:
        RefCell<Vec<Rc<CollectionMutationStateInner>>> = const { RefCell::new(Vec::new()) };
    static IDLE_MUTATION_STATE_CURSOR: Cell<usize> = const { Cell::new(0) };
}

/// How many released registrations stay resident for reuse.
///
/// Sized for the loop nests that actually recur (a handful of containers), not
/// for programs that stream through unrelated containers — those simply rotate
/// through the slots and drop the evicted registrations as before.
const IDLE_MUTATION_STATE_SLOTS: usize = 8;

/// Keep `state` registered after its observers leave.
fn retain_idle_mutation_state(state: &Rc<CollectionMutationStateInner>) {
    IDLE_COLLECTION_MUTATION_STATES.with(|idle| {
        let mut idle = idle.borrow_mut();
        if idle.len() < IDLE_MUTATION_STATE_SLOTS {
            idle.push(Rc::clone(state));
            return;
        }
        let slot = IDLE_MUTATION_STATE_CURSOR.with(|cursor| {
            let slot = cursor.get();
            cursor.set((slot + 1) % IDLE_MUTATION_STATE_SLOTS);
            slot
        });
        idle[slot] = Rc::clone(state);
    });
}

/// Restart a retained registration for its first new observer.
///
/// While `observers` is zero the state is not tracked by mutation helpers, so
/// its accounting may have gone stale exactly as it would have if the
/// registration had been dropped and rebuilt. Restoring the freshly-created
/// shape keeps reuse indistinguishable from re-registration.
#[inline]
fn set_dict_layout_cells(
    state: &CollectionMutationStateInner,
    layout: Option<DictIterationLayout>,
) {
    match layout {
        Some(layout) => {
            state.has_dict_layout.set(true);
            state.dict_layout_live_len.set(layout.live_len);
            state.dict_layout_entry_count.set(layout.entry_count);
            state.dict_layout_usable.set(layout.usable);
        }
        None => {
            state.has_dict_layout.set(false);
        }
    }
}

fn restart_idle_mutation_state_with_len(
    state: &CollectionMutationStateInner,
    dict_len: Option<usize>,
) {
    if state.has_watchers.get() {
        state.removal_watchers.borrow_mut().clear();
        state.has_watchers.set(false);
    }
    if state.has_mutation_activity.get() {
        state.pending_removed_keys.borrow_mut().clear();
        state.reinserted_keys.borrow_mut().clear();
        state.has_mutation_activity.set(false);
    }
    set_dict_layout_cells(state, dict_len.map(DictIterationLayout::for_len));
}

fn restart_idle_mutation_state(
    state: &CollectionMutationStateInner,
    target: &MutableContainerTarget,
) {
    restart_idle_mutation_state_with_len(state, dict_target_len(target));
}

fn dict_target_len(target: &MutableContainerTarget) -> Option<usize> {
    match target {
        MutableContainerTarget::Dict(dict) => dict.upgrade().map(|dict| dict.borrow().len()),
        MutableContainerTarget::Set(_) => None,
    }
}

fn collection_mutation_state(
    key: MutableContainerKey,
    target: MutableContainerTarget,
) -> CollectionMutationState {
    let registered = COLLECTION_MUTATION_STATES.with(|states| {
        states
            .borrow()
            .get(&key)
            .and_then(Weak::upgrade)
            .filter(|state| state.target.matches(&target))
    });
    if let Some(state) = registered {
        if state.observers.get() == 0 {
            restart_idle_mutation_state(&state, &target);
        }
        return CollectionMutationState::attach(state);
    }
    let dict_len = dict_target_len(&target);
    let dict_layout = dict_len.map(DictIterationLayout::for_len);
    let state = Rc::new(CollectionMutationStateInner {
        version: Cell::new(0),
        entry_order: Cell::new(0),
        cache_exhausted: Cell::new(false),
        observers: Cell::new(0),
        removal_watchers: RefCell::new(Vec::new()),
        has_watchers: Cell::new(false),
        has_mutation_activity: Cell::new(false),
        pending_removed_keys: RefCell::new(Vec::new()),
        reinserted_keys: RefCell::new(Vec::new()),
        has_dict_layout: Cell::new(dict_layout.is_some()),
        dict_layout_live_len: Cell::new(dict_layout.as_ref().map_or(0, |l| l.live_len)),
        dict_layout_entry_count: Cell::new(dict_layout.as_ref().map_or(0, |l| l.entry_count)),
        dict_layout_usable: Cell::new(dict_layout.as_ref().map_or(0, |l| l.usable)),
        key,
        target,
    });
    COLLECTION_MUTATION_STATES.with(|states| {
        let mut states = states.borrow_mut();
        if states.len() >= 1024 {
            states.retain(|_, state| state.strong_count() != 0);
        }
        states.insert(key, Rc::downgrade(&state));
    });
    // Evicting a retained registration drops it, which re-enters the registry;
    // publish first so no registry borrow is live at that point.
    retain_idle_mutation_state(&state);
    CollectionMutationState::attach(state)
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
    // A retained-but-unobserved registration must behave exactly like one that
    // was dropped: no generation bump, no compact-entry accounting.
    COLLECTION_MUTATION_STATES
        .with(|states| states.borrow().get(&key).and_then(Weak::upgrade))
        .filter(|state| state.observers.get() != 0)
}
