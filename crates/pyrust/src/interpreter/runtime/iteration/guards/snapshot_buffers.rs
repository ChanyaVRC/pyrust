// Snapshot buffer pools and frozen-key steady-state steppers.

const ADAPTIVE_KEY_SNAPSHOT_THRESHOLD: usize = 64;

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

// Thread-local justification:
// (a) Why `thread_local!`: Pools reusable `Vec<PyKey>`, `Vec<Value>`, and
//     `Box<LiveKeysGuardedState>` scratch buffers containing `!Send` `Value` /
//     `PyKey` elements to eliminate per-loop heap allocations without locking.
// (b) Multi-interpreter / multi-threaded impact: Buffers are cleared before
//     returning to the pool, so sharing across interpreters on one thread
//     recycles capacity only (no semantic state leaks); separate threads own
//     independent bounded pools.
thread_local! {
    static KEY_SNAPSHOT_BUFFERS: SnapshotBufferPool<PyKey> =
        const { SnapshotBufferPool::new() };
    static FROZEN_KEY_BUFFERS: SnapshotBufferPool<Value> =
        const { SnapshotBufferPool::new() };
    static LIVE_KEYS_GUARDED_BOXES: RefCell<[Option<Box<LiveKeysGuardedState>>; SNAPSHOT_BUFFER_SLOTS]> =
        const { RefCell::new([const { None }; SNAPSHOT_BUFFER_SLOTS]) };
}

pub(crate) fn take_live_keys_guarded_box(
    container: Value,
    cursor: LiveKeyCursor,
) -> Box<LiveKeysGuardedState> {
    let recycled = LIVE_KEYS_GUARDED_BOXES.with(|pool| {
        let mut slots = pool.borrow_mut();
        slots.iter_mut().find_map(Option::take)
    });
    if let Some(mut boxed) = recycled {
        boxed.container = container;
        boxed.cursor = cursor;
        boxed
    } else {
        Box::new(LiveKeysGuardedState { cursor, container })
    }
}

pub(crate) fn release_live_keys_guarded_box(mut boxed: Box<LiveKeysGuardedState>) {
    boxed.cursor.release();
    boxed.container = Value::none();
    LIVE_KEYS_GUARDED_BOXES.with(|pool| {
        let mut slots = pool.borrow_mut();
        if let Some(empty) = slots.iter_mut().find(|slot| slot.is_none()) {
            *empty = Some(boxed);
        }
    });
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

/// Build the initial frozen key order or key snapshot from an already-borrowed
/// plain `PyDict` in a single `RefCell` borrow (#2924).
pub(crate) fn initial_dict_order_from_ref(
    dict: &pyrust_core::PyDict,
    kind: u8,
) -> (Option<Vec<Value>>, Option<Vec<PyKey>>) {
    let len = dict.len();
    if len > ADAPTIVE_KEY_SNAPSHOT_THRESHOLD {
        return (None, None);
    }
    if kind == 0 {
        let mut order = FROZEN_KEY_BUFFERS.with(SnapshotBufferPool::take);
        if push_frozen_key_order(dict.keys(), &mut order) && order.len() == len {
            return (Some(order), None);
        }
        release_frozen_key_buffer(order);
    }
    let mut keys = take_key_snapshot_buffer();
    keys.extend(dict.keys().cloned());
    if keys.len() == len {
        (None, Some(keys))
    } else {
        release_key_snapshot_buffer(keys);
        (None, None)
    }
}

/// Outcome of the inline steady-state stepper for frozen key/set walks (#2924).
pub(crate) enum FrozenKeyStep {
    Item(Value),
    Exhausted,
    Slow,
}

/// Yield the next key — or fast-exhaust — a walk whose order is still frozen.
///
/// This is the whole steady state of `for k in dict` and `for k in set`: the
/// order is already held in yielded form, so each step is a generation compare
/// and one unchecked indexed read, and clean exhaustion releases the cursor
/// without entering `advance_live_key_cursor`. The final entry (which installs
/// the terminal-key reinsertion watch) and any mutated order decline here with
/// `Slow` and run the general state machine.
#[inline(always)]
pub(crate) fn next_frozen_key_step(cursor: &mut LiveKeyCursor) -> FrozenKeyStep {
    let Some(frozen) = cursor.frozen_keys.as_ref() else {
        return FrozenKeyStep::Slow;
    };
    let Some(state) = cursor.mutation_state.as_ref() else {
        return FrozenKeyStep::Slow;
    };
    if state.version() != cursor.observed_mutation {
        return FrozenKeyStep::Slow;
    }
    let pos = cursor.snapshot_pos;
    let len = frozen.len();
    if pos + 1 < len {
        // SAFETY: `pos + 1 < len` guarantees `pos < frozen.len()`.
        let item = unsafe { frozen.get_unchecked(pos).clone() };
        cursor.snapshot_pos = pos + 1;
        return FrozenKeyStep::Item(item);
    }
    if pos == len {
        cursor.release();
        return FrozenKeyStep::Exhausted;
    }
    // `pos + 1 == len`: terminal entry; delegate to the general path so it
    // installs the terminal-key reinsertion watch.
    FrozenKeyStep::Slow
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
