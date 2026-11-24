// Live dict/set key-cursor state machine and snapshot stepping.

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
