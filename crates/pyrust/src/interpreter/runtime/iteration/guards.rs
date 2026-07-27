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
    if let Some(keys) = container.dict_with(|dict| dict.keys().cloned().collect()) {
        return Some(keys);
    }
    if let Some(keys) = container.set_with(|set| set.iter().cloned().collect()) {
        return Some(keys);
    }
    if let Some(backing) = builtin_data_backing(container) {
        if let Some(keys) = backing.dict_with(|dict| dict.keys().cloned().collect()) {
            return Some(keys);
        }
        if let Some(keys) = backing.set_with(|set| set.iter().cloned().collect()) {
            return Some(keys);
        }
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return Some(dict.borrow().keys().cloned().collect());
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
        .map(|dict| dict.borrow().keys().cloned().collect())
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

#[inline(always)]
fn advance_snapshot_key_cursor(
    container: &Value,
    cursor: &mut LiveKeyCursor,
) -> Result<Option<LiveDictViewItem>> {
    let key = cursor
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.get(cursor.snapshot_pos))
        .cloned();
    let Some(key) = key else {
        cursor.release();
        return Ok(None);
    };
    cursor.snapshot_pos += 1;
    if cursor.remaining.is_some()
        && cursor.snapshot_pos == cursor.recorded_len
        && !cursor.watching_terminal_key
        && let Some(state) = &cursor.mutation_state
    {
        cursor.last_index = cursor.snapshot_pos - 1;
        cursor.last_key = Some(key.clone());
        cursor.terminal_entry_cursor = state.watch_key_reinsertion(&key);
        cursor.watching_terminal_key = true;
    }
    let item = match cursor.kind {
        0 | 3 => LiveDictViewItem::Item(key_to_value(key)),
        1 => {
            let value = live_dict_value(container, &key).ok_or_else(|| {
                PyError::Runtime("dictionary keys changed during iteration".to_string())
            })?;
            LiveDictViewItem::Item(value)
        }
        _ => {
            let value = live_dict_value(container, &key).ok_or_else(|| {
                PyError::Runtime("dictionary keys changed during iteration".to_string())
            })?;
            LiveDictViewItem::Pair(key_to_value(key), value)
        }
    };
    Ok(Some(item))
}

/// Advance a live dict/set cursor after its size guard has passed.
pub(crate) fn advance_live_key_cursor(
    container: &Value,
    cursor: &mut LiveKeyCursor,
) -> Result<Option<LiveDictViewItem>> {
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
        cursor.release();
        return Err(PyError::Runtime(message.to_string()));
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
            cursor.seen = Some(seen);
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
        && cursor.yielded.len() >= ADAPTIVE_KEY_SNAPSHOT_THRESHOLD
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
            cursor.seen = Some(seen);
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
