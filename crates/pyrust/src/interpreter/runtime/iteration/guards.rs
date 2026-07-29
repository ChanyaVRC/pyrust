// Mutation guards for collection-backed iterators.
///
/// Read the live element count of a `dict` / `set` / dict-view `container`,
/// used by the size-mutation guard (#1988). Returns `None` if `container` is
/// not one of those types (in which case the guard treats the snapshot as
/// non-mutating, since no other guarded source reaches this path).
pub(crate) fn live_collection_len(container: &Value) -> Option<usize> {
    // Each accessor holds a `RefCell` read guard only for this length read;
    // it is dropped before the iterator fetches or dispatches an element.
    if let Some(len) = container.dict_len() {
        return Some(len);
    }
    if let Some(len) = container.set_len() {
        return Some(len);
    }
    // dict/set-subclass instances (including provider-tagged ordered mappings
    // and primitive set subclasses): re-resolve `__builtin_data__` each step.
    // Re-reading the
    // instance attr each step (rather than capturing the backing `Rc` at
    // iterator creation) keeps the guard correct regardless of whether a
    // mutation rewrites the backing map in place (`store_backing`, #2447) or
    // replaces the whole `Value`: either way this sees the current backing and
    // detects the size change.  Only reached on the cold guarded path (these
    // three subclasses); the common dict/set/deque guard above is untouched.
    if let Some(backing) = builtin_data_backing(container) {
        if let Some(len) = backing.dict_len() {
            return Some(len);
        }
        if let Some(len) = backing.set_len() {
            return Some(len);
        }
    }
    // mappingproxy (issue #2728): both class-backed (`vars(C)`) and dict-backed
    // (`d.keys().mapping`) proxies re-read their live source size so iterators
    // over them detect a size change and raise the dict guard, like a plain dict.
    if let Some(n) = pyrust_builtins::mapping_proxy::live_len(container) {
        return Some(n);
    }
    pyrust_builtins::dict_views::as_dict_rc(container).map(|rc| rc.borrow().len())
}

/// Register a shared mutation generation for the live backing store reached by
/// `container`. Dict views and mapping proxies use the same generation as
/// their source dict, while builtin subclasses resolve `__builtin_data__`.
pub(crate) fn live_collection_mutation_state(
    container: &Value,
) -> Option<pyrust_core::CollectionMutationState> {
    if let Some(state) = container.dict_iteration_mutation_state() {
        return Some(state);
    }
    if let Some(state) = container.set_iteration_mutation_state() {
        return Some(state);
    }
    if let Some(backing) = builtin_data_backing(container) {
        if let Some(state) = backing.dict_iteration_mutation_state() {
            return Some(state);
        }
        if let Some(state) = backing.set_iteration_mutation_state() {
            return Some(state);
        }
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return Some(pyrust_core::dict_iteration_mutation_state(&dict));
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
        .map(|dict| pyrust_core::dict_iteration_mutation_state(&dict))
}

fn live_collection_key_at(container: &Value, index: usize) -> Option<PyKey> {
    if let Some(key) = container.dict_with(|dict| dict.get_index(index).map(|(key, _)| key.clone()))
    {
        return key;
    }
    if let Some(key) = container.set_with(|set| set.get_index(index).cloned()) {
        return key;
    }
    if let Some(backing) = builtin_data_backing(container) {
        if let Some(key) =
            backing.dict_with(|dict| dict.get_index(index).map(|(key, _)| key.clone()))
        {
            return key;
        }
        if let Some(key) = backing.set_with(|set| set.get_index(index).cloned()) {
            return key;
        }
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return dict.borrow().get_index(index).map(|(key, _)| key.clone());
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
        .and_then(|dict| dict.borrow().get_index(index).map(|(key, _)| key.clone()))
}

fn live_collection_keys(container: &Value) -> Option<Vec<PyKey>> {
    let mut keys = Vec::new();
    extend_live_collection_keys(container, &mut keys).then_some(keys)
}

/// Append the live key order of `container` to `keys`.
///
/// Written as an append so a cursor can refill a recycled buffer instead of
/// allocating one per loop entry.
fn extend_live_collection_keys(container: &Value, keys: &mut Vec<PyKey>) -> bool {
    if container
        .dict_with(|dict| keys.extend(dict.keys().cloned()))
        .is_some()
    {
        return true;
    }
    if container
        .set_with(|set| keys.extend(set.iter().cloned()))
        .is_some()
    {
        return true;
    }
    if let Some(backing) = builtin_data_backing(container) {
        if backing
            .dict_with(|dict| keys.extend(dict.keys().cloned()))
            .is_some()
        {
            return true;
        }
        if backing
            .set_with(|set| keys.extend(set.iter().cloned()))
            .is_some()
        {
            return true;
        }
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        keys.extend(dict.borrow().keys().cloned());
        return true;
    }
    if let Some(dict) = pyrust_builtins::mapping_proxy::as_dict_rc(container) {
        keys.extend(dict.borrow().keys().cloned());
        return true;
    }
    false
}

/// Per-thread free list of released snapshot buffers.
///
/// Entering a loop over a small container otherwise allocates and frees one
/// order buffer every time. The containers that recur in a hot loop are few, so
/// a handful of recycled buffers removes that traffic entirely.
struct SnapshotBufferPool<T> {
    buffers: RefCell<Vec<Vec<T>>>,
}

/// Recycled buffers held per thread, and the largest capacity worth keeping.
const SNAPSHOT_BUFFER_SLOTS: usize = 4;
const SNAPSHOT_BUFFER_CAPACITY: usize = 2 * ADAPTIVE_KEY_SNAPSHOT_THRESHOLD;

impl<T> SnapshotBufferPool<T> {
    const fn new() -> Self {
        Self {
            buffers: RefCell::new(Vec::new()),
        }
    }

    fn take(&self) -> Vec<T> {
        self.buffers.borrow_mut().pop().unwrap_or_default()
    }

    /// Large adaptive snapshots are dropped rather than retained: they belong
    /// to walks whose per-entry allocation is already amortised over the
    /// container they walk.
    fn release(&self, mut buffer: Vec<T>) {
        if buffer.capacity() == 0 || buffer.capacity() > SNAPSHOT_BUFFER_CAPACITY {
            return;
        }
        buffer.clear();
        let mut buffers = self.buffers.borrow_mut();
        if buffers.len() < SNAPSHOT_BUFFER_SLOTS {
            buffers.push(buffer);
        }
    }
}

thread_local! {
    static KEY_SNAPSHOT_BUFFERS: SnapshotBufferPool<PyKey> =
        const { SnapshotBufferPool::new() };
    static FROZEN_KEY_BUFFERS: SnapshotBufferPool<Value> =
        const { SnapshotBufferPool::new() };
}

fn take_key_snapshot_buffer() -> Vec<PyKey> {
    KEY_SNAPSHOT_BUFFERS.with(SnapshotBufferPool::take)
}

pub(crate) fn release_key_snapshot_buffer(buffer: Vec<PyKey>) {
    KEY_SNAPSHOT_BUFFERS.with(|pool| pool.release(buffer));
}

pub(crate) fn release_frozen_key_buffer(buffer: Vec<Value>) {
    FROZEN_KEY_BUFFERS.with(|pool| pool.release(buffer));
}

/// Whether `key` is reproduced exactly by converting it to a `Value` and back.
///
/// The frozen key walk stores its order in yielded form, so it can only be used
/// where mutation recovery can rebuild the original keys. Container keys and
/// instances that carry a precomputed hash are excluded rather than
/// round-tripped through a conversion that would have to re-derive it.
fn key_round_trips_through_value(key: &PyKey) -> bool {
    matches!(
        key,
        PyKey::Int(_)
            | PyKey::Str(_)
            | PyKey::Bool(_)
            | PyKey::None
            | PyKey::Ellipsis
            | PyKey::Float(_)
            | PyKey::BigInt(_)
            | PyKey::Bytes(_)
            | PyKey::Complex(_, _)
    )
}

/// Convert one key order into yielded form, refusing keys that cannot be
/// converted back.
fn push_frozen_key_order<'a>(
    keys: impl Iterator<Item = &'a PyKey>,
    order: &mut Vec<Value>,
) -> bool {
    for key in keys {
        if !key_round_trips_through_value(key) {
            return false;
        }
        order.push(key_ref_to_value(key));
    }
    true
}

/// `Some(false)` means the container was recognised but its keys are not
/// eligible; `None` means it is not one of the key-ordered shapes.
fn extend_frozen_key_order(container: &Value, order: &mut Vec<Value>) -> Option<bool> {
    if let Some(ok) = container.dict_with(|dict| push_frozen_key_order(dict.keys(), order)) {
        return Some(ok);
    }
    if let Some(ok) = container.set_with(|set| push_frozen_key_order(set.iter(), order)) {
        return Some(ok);
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return Some(push_frozen_key_order(dict.borrow().keys(), order));
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
        .map(|dict| push_frozen_key_order(dict.borrow().keys(), order))
}

/// Capture a small key/set walk's order already converted to yielded form.
///
/// A key walk yields exactly these values, so holding the order this way turns
/// the steady-state step into one indexed `Value` read — the per-item cost the
/// snapshot representation was paying twice, once to clone each `PyKey` into the
/// snapshot and again to convert it back on the way out.
///
/// Subclass carriers are excluded with the rest of the dynamic-backing cases,
/// so this only walks a container whose backing identity cannot move.
pub(crate) fn initial_frozen_key_order(
    container: &Value,
    dynamic_backing: bool,
    kind: u8,
    len: usize,
) -> Option<Vec<Value>> {
    if dynamic_backing || (kind != 0 && kind != 3) || len > ADAPTIVE_KEY_SNAPSHOT_THRESHOLD {
        return None;
    }
    let mut order = FROZEN_KEY_BUFFERS.with(SnapshotBufferPool::take);
    if extend_frozen_key_order(container, &mut order) == Some(true) && order.len() == len {
        return Some(order);
    }
    release_frozen_key_buffer(order);
    None
}

/// Restore the `PyKey` snapshot representation from a frozen key order.
///
/// Every path other than the steady-state stepper — a mutated order, the
/// terminal entry's reinsertion watch, exhaustion — runs on the general state
/// machine, so a walk that stops being ordinary converts back once and is
/// afterwards indistinguishable from one that never took the fast form.
fn deoptimize_frozen_key_order(cursor: &mut LiveKeyCursor) {
    let Some(frozen) = cursor.frozen_keys.take() else {
        return;
    };
    let mut snapshot = take_key_snapshot_buffer();
    snapshot.extend(frozen.iter().map(|value| {
        value
            .to_key()
            .expect("frozen key order admits only round-tripping keys")
    }));
    release_frozen_key_buffer(frozen);
    cursor.snapshot = Some(snapshot);
}

fn live_collection_key_index(container: &Value, key: &PyKey) -> Option<usize> {
    if let Some(index) = container.dict_with(|dict| dict.get_index_of(key)) {
        return index;
    }
    if let Some(index) = container.set_with(|set| set.get_index_of(key)) {
        return index;
    }
    if let Some(backing) = builtin_data_backing(container) {
        if let Some(index) = backing.dict_with(|dict| dict.get_index_of(key)) {
            return index;
        }
        if let Some(index) = backing.set_with(|set| set.get_index_of(key)) {
            return index;
        }
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return dict.borrow().get_index_of(key);
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
        .and_then(|dict| dict.borrow().get_index_of(key))
}

fn live_dict_value(container: &Value, key: &PyKey) -> Option<Value> {
    if let Some(value) = container.dict_with(|dict| dict.get(key).cloned()) {
        return value;
    }
    if let Some(backing) = builtin_data_backing(container)
        && let Some(value) = backing.dict_with(|dict| dict.get(key).cloned())
    {
        return value;
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return dict.borrow().get(key).cloned();
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
        .and_then(|dict| dict.borrow().get(key).cloned())
}

const ADAPTIVE_KEY_SNAPSHOT_THRESHOLD: usize = 64;

/// Resolve the backing mapping a live cursor may read positionally.
///
/// Only containers whose backing identity is fixed qualify: a builtin subclass
/// can replace `__builtin_data__` under an active iterator, so those cursors
/// keep re-probing the carrier on every step.
pub(crate) fn stable_cursor_backing(
    container: &Value,
    dynamic_backing: bool,
) -> Option<pyrust_builtins::dict_views::DictRc> {
    if dynamic_backing {
        return None;
    }
    if let Some(dict) = container.get_dict_rc() {
        return Some(Rc::clone(dict));
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return Some(dict);
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
}

/// Capture the key order of a container small enough that one snapshot costs
/// less than the general walk's per-item key history.
///
/// Larger containers keep O(1) iterator creation — a loop that breaks after a
/// few items must not pay for the whole key order — and adopt the snapshot
/// from [`advance_live_key_cursor`] once the walk proves it is amortised.
pub(crate) fn initial_key_snapshot(
    container: &Value,
    dynamic_backing: bool,
    len: usize,
) -> Option<Vec<PyKey>> {
    if dynamic_backing || len > ADAPTIVE_KEY_SNAPSHOT_THRESHOLD {
        return None;
    }
    let mut keys = take_key_snapshot_buffer();
    if !extend_live_collection_keys(container, &mut keys) || keys.len() != len {
        release_key_snapshot_buffer(keys);
        return None;
    }
    Some(keys)
}

/// How many items a larger container's walk must yield before one whole-order
/// snapshot beats continuing the general walk.
///
/// The snapshot costs `recorded_len` key clones, so the walk commits only once
/// it has consumed a comparable share of the container. The cap preserves the
/// existing behavior for very large containers.
fn adaptive_snapshot_trigger(recorded_len: usize) -> usize {
    (recorded_len / 8).clamp(1, ADAPTIVE_KEY_SNAPSHOT_THRESHOLD)
}

/// Result of the compact steady-state path used after a live key cursor has
/// captured its adaptive snapshot.
///
/// `Changed` hands control back to [`advance_live_key_cursor`], which owns the
/// same-size mutation recovery and size/key-change errors.  The ordinary
/// mutation-free path therefore consists only of one generation load, one
/// indexed snapshot read, and the key-to-value conversion.
pub(crate) enum StableSnapshotAdvance {
    Item(LiveDictViewItem),
    Exhausted,
    Changed,
}

/// Advance an adaptive snapshot while its backing mutation generation is
/// unchanged.
///
/// Callers probe this only when `cursor.snapshot.is_some()`.  Keeping the
/// generation comparison here avoids re-entering the general live-cursor
/// state machine for every item in long dict/set/Counter walks.  Dynamic
/// subclass backing still uses the general path because its backing identity
/// must be re-resolved before trusting the generation.
#[inline(always)]
pub(crate) fn advance_stable_snapshot_cursor(
    container: &Value,
    cursor: &mut LiveKeyCursor,
) -> Result<StableSnapshotAdvance> {
    if cursor.dynamic_backing
        || cursor
            .mutation_state
            .as_ref()
            .is_none_or(|state| state.version() != cursor.observed_mutation)
    {
        return Ok(StableSnapshotAdvance::Changed);
    }

    Ok(match advance_snapshot_key_cursor(container, cursor)? {
        Some(item) => StableSnapshotAdvance::Item(item),
        None => StableSnapshotAdvance::Exhausted,
    })
}

/// Yield the next key of a walk whose order is still frozen.
///
/// This is the whole steady state of `for k in dict` and `for k in set`: the
/// order is already held in yielded form, so the step is a generation compare
/// and one indexed read. Everything else — exhaustion, the terminal entry that
/// installs the reinsertion watch, mutated orders, value and item walks, and
/// subclass backing that can move — declines here and runs the general state
/// machine, which keeps this stepper free of the dispatch those cases need.
#[inline(always)]
pub(crate) fn next_frozen_key(cursor: &mut LiveKeyCursor) -> Option<Value> {
    let pos = cursor.snapshot_pos;
    // Leaving the final entry to the general path also covers exhaustion,
    // release, and the terminal-key reinsertion watch.
    if pos + 1 >= cursor.recorded_len {
        return None;
    }
    let state = cursor.mutation_state.as_ref()?;
    if state.version() != cursor.observed_mutation {
        return None;
    }
    let item = cursor.frozen_keys.as_ref()?.get(pos)?.clone();
    cursor.snapshot_pos = pos + 1;
    Some(item)
}

fn emit_live_key_item(
    container: &Value,
    cursor: &mut LiveKeyCursor,
    key: PyKey,
    index: usize,
) -> Result<LiveDictViewItem> {
    if cursor.remaining == Some(0) {
        cursor.keys_changed = true;
        cursor.release();
        return Err(PyError::Runtime(
            "dictionary keys changed during iteration".to_string(),
        ));
    }
    if let Some(seen) = &mut cursor.seen {
        seen.insert(key.clone());
    } else if cursor.snapshot.is_none() {
        cursor.yielded.push(key.clone());
    }
    cursor.last_key = Some(key.clone());
    cursor.last_index = index;
    if let Some(remaining) = &mut cursor.remaining {
        *remaining -= 1;
    }
    let item = match cursor.kind {
        0 | 3 => Ok(LiveDictViewItem::Item(key_to_value(key))),
        1 => {
            let value = live_dict_value(container, &key).ok_or_else(|| {
                PyError::Runtime("dictionary keys changed during iteration".to_string())
            })?;
            Ok(LiveDictViewItem::Item(value))
        }
        _ => {
            let value = live_dict_value(container, &key).ok_or_else(|| {
                PyError::Runtime("dictionary keys changed during iteration".to_string())
            })?;
            Ok(LiveDictViewItem::Pair(key_to_value(key), value))
        }
    }?;
    if cursor.remaining == Some(0)
        && !cursor.watching_terminal_key
        && let (Some(state), Some(last_key)) = (&cursor.mutation_state, &cursor.last_key)
    {
        cursor.terminal_entry_cursor = state.watch_key_reinsertion(last_key);
        cursor.watching_terminal_key = true;
    }
    Ok(item)
}

/// Finish a frozen key walk: its terminal entry, then exhaustion.
///
/// [`next_frozen_key`] hands both cases here so that installing the
/// reinsertion watch — the one step that needs the original key back — stays
/// out of the per-item path.
fn advance_frozen_key_cursor(cursor: &mut LiveKeyCursor) -> Option<LiveDictViewItem> {
    let pos = cursor.snapshot_pos;
    let Some(item) = cursor
        .frozen_keys
        .as_ref()
        .and_then(|frozen| frozen.get(pos))
        .cloned()
    else {
        cursor.release();
        return None;
    };
    cursor.snapshot_pos = pos + 1;
    if cursor.remaining.is_some()
        && !cursor.watching_terminal_key
        && cursor.snapshot_pos == cursor.recorded_len
    {
        watch_terminal_key(cursor, item.to_key(), pos);
    }
    Some(LiveDictViewItem::Item(item))
}

/// One entry read from a backing mapping whose order is currently frozen.
enum PositionalEntry {
    Value(Value),
    Pair(PyKey, Value),
}

#[inline(always)]
fn advance_snapshot_key_cursor(
    container: &Value,
    cursor: &mut LiveKeyCursor,
) -> Result<Option<LiveDictViewItem>> {
    let pos = cursor.snapshot_pos;
    // An unchanged mutation generation freezes insertion order, so the
    // snapshot position is also the live entry position. Value and item walks
    // therefore read the entry itself rather than hashing the snapshot key
    // back into the mapping on every step. Key and set walks already have the
    // key in the snapshot and need no borrow at all.
    let positional = cursor.backing.as_ref().and_then(|dict| match cursor.kind {
        1 => Some(pyrust_builtins::dict_views::value_at(dict, pos).map(PositionalEntry::Value)),
        2 => Some(
            pyrust_builtins::dict_views::entry_at(dict, pos)
                .map(|(key, value)| PositionalEntry::Pair(key, value)),
        ),
        _ => None,
    });
    if let Some(entry) = positional {
        let Some(entry) = entry else {
            cursor.release();
            return Ok(None);
        };
        cursor.snapshot_pos = pos + 1;
        if cursor.remaining.is_some()
            && cursor.snapshot_pos == cursor.recorded_len
            && !cursor.watching_terminal_key
        {
            let key = match &entry {
                PositionalEntry::Pair(key, _) => Some(key.clone()),
                PositionalEntry::Value(_) => cursor
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.get(pos))
                    .cloned(),
            };
            watch_terminal_key(cursor, key, pos);
        }
        return Ok(Some(match entry {
            PositionalEntry::Value(value) => LiveDictViewItem::Item(value),
            PositionalEntry::Pair(key, value) => LiveDictViewItem::Pair(key_to_value(key), value),
        }));
    }

    // The key order is already in hand: read it in place instead of copying
    // the whole `PyKey` out of the snapshot on every step.
    let Some(key) = cursor
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.get(pos))
    else {
        cursor.release();
        return Ok(None);
    };
    let item = match cursor.kind {
        0 | 3 => LiveDictViewItem::Item(key_ref_to_value(key)),
        1 => {
            let value = live_dict_value(container, key).ok_or_else(|| {
                PyError::Runtime("dictionary keys changed during iteration".to_string())
            })?;
            LiveDictViewItem::Item(value)
        }
        _ => {
            let value = live_dict_value(container, key).ok_or_else(|| {
                PyError::Runtime("dictionary keys changed during iteration".to_string())
            })?;
            LiveDictViewItem::Pair(key_ref_to_value(key), value)
        }
    };
    let terminal_key = (cursor.remaining.is_some()
        && pos + 1 == cursor.recorded_len
        && !cursor.watching_terminal_key)
        .then(|| key.clone());
    cursor.snapshot_pos = pos + 1;
    watch_terminal_key(cursor, terminal_key, pos);
    Ok(Some(item))
}

/// Install the one-key removal watch that separates a delete/reinsert of a
/// dict iterator's final entry from an unrelated temporary insert/remove.
fn watch_terminal_key(cursor: &mut LiveKeyCursor, key: Option<PyKey>, index: usize) {
    let Some(key) = key else {
        return;
    };
    let Some(entry_cursor) = cursor
        .mutation_state
        .as_ref()
        .map(|state| state.watch_key_reinsertion(&key))
    else {
        return;
    };
    cursor.last_index = index;
    cursor.last_key = Some(key);
    cursor.terminal_entry_cursor = entry_cursor;
    cursor.watching_terminal_key = true;
}

/// Advance a live dict/set cursor after its size guard has passed.
///
/// A cursor has three terminal states, and CPython distinguishes them: a size
/// change re-raises forever, the "keys changed" error fires once and then
/// reports exhaustion, and an ordinary walk reports exhaustion from the start.
pub(crate) fn advance_live_key_cursor(
    container: &Value,
    cursor: &mut LiveKeyCursor,
) -> Result<Option<LiveDictViewItem>> {
    if cursor.size_changed {
        return Err(PyError::Runtime(cursor.size_change_message.to_string()));
    }
    if cursor.exhausted || cursor.keys_changed {
        return Ok(None);
    }

    let mut mutated = false;
    if cursor.dynamic_backing {
        match live_collection_mutation_state(container) {
            Some(state)
                if cursor
                    .mutation_state
                    .as_ref()
                    .is_none_or(|current| !state.same_backing(current)) =>
            {
                cursor.mutation_state = Some(state);
                mutated = true;
            }
            None => mutated = true,
            _ => {}
        }
    }
    let mutation_state = cursor
        .mutation_state
        .as_ref()
        .expect("active live-key cursor requires mutation state");
    let version = mutation_state.version();
    if version != cursor.observed_mutation {
        if cursor.watching_terminal_key
            && cursor.last_key.as_ref().is_some_and(|key| {
                mutation_state.key_reinserted_at_or_after_since(
                    key,
                    cursor.observed_mutation,
                    cursor.terminal_entry_cursor,
                )
            })
        {
            cursor.structurally_changed = true;
        }
        cursor.observed_mutation = version;
        mutated = true;
    }

    // The active generation proves that the size cannot have changed while
    // its version is stable. Pay for a guarded length read only after an
    // actual mutation, preserving CPython's size-change error without adding
    // RefCell traffic to every ordinary iteration step.
    if mutated && live_collection_len(container) != Some(cursor.recorded_len) {
        let message = cursor.size_change_message;
        // Latch before releasing: `release` drops the walk's storage and its
        // mutation registration, but this iterator must keep reporting the
        // size change rather than falling back to plain exhaustion.
        cursor.size_changed = true;
        cursor.release();
        return Err(PyError::Runtime(message.to_string()));
    }

    if cursor.frozen_keys.is_some() {
        if !mutated {
            return Ok(advance_frozen_key_cursor(cursor));
        }
        deoptimize_frozen_key_order(cursor);
    }

    if !mutated && cursor.snapshot.is_some() {
        return advance_snapshot_key_cursor(container, cursor);
    }

    if mutated {
        if cursor.seen.is_none() {
            let mut seen = pyrust_core::PySet::default();
            if let Some(snapshot) = cursor.snapshot.take() {
                // The compact snapshot path deliberately advances only
                // `snapshot_pos`. Reconstruct the general live-walk counters
                // once, on the first actual mutation, instead of writing them
                // for every mutation-free item.
                cursor.next_index = cursor.snapshot_pos;
                if cursor.remaining.is_some() {
                    cursor.remaining =
                        Some(cursor.recorded_len.saturating_sub(cursor.snapshot_pos));
                }
                if cursor.snapshot_pos != 0 {
                    cursor.last_index = cursor.snapshot_pos - 1;
                    cursor.last_key = snapshot.get(cursor.last_index).cloned();
                }
                seen.extend(snapshot.into_iter().take(cursor.snapshot_pos));
                cursor.snapshot_pos = 0;
            } else {
                seen.extend(cursor.yielded.drain(..));
            }
            cursor.seen = Some(Box::new(seen));
        }
        if let Some(last_key) = &cursor.last_key
            && live_collection_key_index(container, last_key) != Some(cursor.last_index)
        {
            cursor.next_index = 0;
            cursor.structurally_changed = true;
        }
    }
    if cursor.remaining == Some(0) && cursor.structurally_changed {
        cursor.keys_changed = true;
        cursor.release();
        return Err(PyError::Runtime(
            "dictionary keys changed during iteration".to_string(),
        ));
    }

    // Preserve O(1) creation and early-break behavior, then switch long,
    // mutation-free walks to one compact key snapshot. The yielded prefix
    // lives in the first `snapshot_pos` entries, so mutation recovery still
    // has exact seen-key history without retaining a second O(n) buffer.
    if cursor.seen.is_none()
        && cursor.snapshot.is_none()
        && cursor.yielded.len() >= adaptive_snapshot_trigger(cursor.recorded_len)
        && let Some(snapshot) = live_collection_keys(container)
    {
        let yielded = cursor.yielded.len();
        if snapshot.len() >= yielded && snapshot[..yielded] == cursor.yielded {
            cursor.snapshot_pos = yielded;
            cursor.yielded = Vec::new();
            cursor.snapshot = Some(snapshot);
        } else {
            let mut seen = pyrust_core::PySet::default();
            seen.extend(cursor.yielded.drain(..));
            cursor.seen = Some(Box::new(seen));
            cursor.next_index = 0;
            cursor.structurally_changed = true;
        }
    }

    if cursor.snapshot.is_some() {
        return advance_snapshot_key_cursor(container, cursor);
    }

    while let Some(key) = live_collection_key_at(container, cursor.next_index) {
        let index = cursor.next_index;
        cursor.next_index += 1;
        if cursor.seen.as_ref().is_some_and(|seen| seen.contains(&key)) {
            continue;
        }
        return emit_live_key_item(container, cursor, key, index).map(Some);
    }
    cursor.release();
    Ok(None)
}

/// Resolve the backing identity required by a provider-supplied ordered
/// mapping guard. Generic iteration knows only the supported storage shapes.
fn ordered_mapping_backing_id(container: &Value) -> Option<i64> {
    if container.is_dict() {
        return container.value_id();
    }
    if let Some(backing) = builtin_data_backing(container) {
        return backing.value_id();
    }
    pyrust_builtins::dict_views::as_dict_rc(container).map(|rc| Rc::as_ptr(&rc) as i64)
}

pub(crate) fn ordered_mapping_guard_message(
    container: &Value,
    recorded_len: usize,
    iter_seq: u64,
) -> &'static str {
    pyrust_builtins::ordered_mapping::guard_message(
        ordered_mapping_backing_id(container),
        recorded_len,
        live_collection_len(container),
        iter_seq,
    )
}

#[cfg(test)]
mod live_key_cursor_tests {
    use pyrust_core::{PyDict, PyKey, Value};

    use super::{LiveKeyCursor, advance_live_key_cursor};

    #[test]
    fn exhausted_cursor_releases_history_and_mutation_registration() {
        let mut items = PyDict::default();
        items.insert(PyKey::Int(1), Value::int(10));
        items.insert(PyKey::Int(2), Value::int(20));
        let container = Value::dict(items);
        let mut cursor = LiveKeyCursor::dict(&container, 0, 2);

        assert!(
            advance_live_key_cursor(&container, &mut cursor)
                .unwrap()
                .is_some()
        );
        assert!(
            advance_live_key_cursor(&container, &mut cursor)
                .unwrap()
                .is_some()
        );
        assert!(
            advance_live_key_cursor(&container, &mut cursor)
                .unwrap()
                .is_none()
        );

        assert!(cursor.exhausted);
        assert!(cursor.yielded.is_empty());
        assert_eq!(cursor.yielded.capacity(), 0);
        assert!(cursor.snapshot.is_none());
        assert!(cursor.seen.is_none());
        assert!(cursor.mutation_state.is_none());
    }

    #[test]
    fn long_cursor_adapts_to_one_snapshot_and_releases_it() {
        let mut items = PyDict::default();
        for value in 0..100 {
            items.insert(PyKey::Int(value), Value::int(value));
        }
        let container = Value::dict(items);
        let mut cursor = LiveKeyCursor::dict(&container, 0, 100);

        for _ in 0..65 {
            assert!(
                advance_live_key_cursor(&container, &mut cursor)
                    .unwrap()
                    .is_some()
            );
        }
        assert!(cursor.yielded.is_empty());
        assert_eq!(cursor.snapshot.as_ref().map(Vec::len), Some(100));
        assert_eq!(cursor.snapshot_pos, 65);

        while advance_live_key_cursor(&container, &mut cursor)
            .unwrap()
            .is_some()
        {}
        assert!(cursor.snapshot.is_none());
        assert!(cursor.mutation_state.is_none());
    }
}
