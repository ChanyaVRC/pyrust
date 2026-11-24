// Descending cursor and mutation guard for `reversed(dict)` / `reversed(view)`.

impl ReverseDictCursor {
    /// Start a descending walk at the mapping's current final entry.
    ///
    /// Creation is O(1): nothing is materialised, so `reversed(d.values())`
    /// over a large mapping costs one generation read rather than a full value
    /// snapshot. Every container shape a `reversed()` walk reaches resolves to
    /// a backing mapping, so the length and the generation are taken from that
    /// one resolution instead of re-probing the carrier three times — a short
    /// walk pays for its own creation, and this is all of it.
    pub(crate) fn new(container: Value, kind: u8) -> Self {
        let dynamic_backing = container.as_py_instance_rc().is_some();
        let backing = stable_cursor_backing(&container, dynamic_backing);
        let (len, mutation_state) = match &backing {
            Some(dict) => (
                pyrust_builtins::dict_views::backing_len(dict),
                Some(pyrust_core::dict_iteration_mutation_state(dict)),
            ),
            None => (
                live_collection_len(&container).unwrap_or(0),
                live_collection_mutation_state(&container),
            ),
        };
        let observed_mutation = mutation_state.as_ref().map_or(0, |state| state.version());
        Self {
            backing,
            container,
            mutation_state,
            observed_mutation,
            next_index: len,
            last_key: None,
            relocate: false,
            kind,
            dynamic_backing,
            size_changed: false,
        }
    }

    /// The mapping's length when the walk began, which the frame's guard
    /// compares against.
    pub(crate) fn recorded_len(&self) -> usize {
        self.next_index
    }
}

/// Whether the frame's mutation guard must run before the next reverse step.
///
/// An unchanged mutation generation proves the mapping was not written at all,
/// so the ordinary step skips the guard's `RefCell` length read. A container
/// whose backing can move, or one with no registered generation, always pays
/// for the guard.
#[inline]
fn reverse_dict_needs_guard(cursor: &mut ReverseDictCursor) -> bool {
    if cursor.dynamic_backing || cursor.size_changed {
        return true;
    }
    let Some(state) = &cursor.mutation_state else {
        return true;
    };
    let version = state.version();
    if version == cursor.observed_mutation {
        return false;
    }
    cursor.observed_mutation = version;
    cursor.relocate = true;
    true
}

/// Re-anchor the descending cursor after the mapping was written.
///
/// A shift-removing map renumbers every entry after a deleted one, so a raw
/// position no longer names the entry it named before. Re-finding the last
/// yielded key restores the walk's place: the entries below it are exactly the
/// ones still owed, and an entry appended above it stays invisible — what
/// CPython's reverse iterator reports for a mapping it did not have to rehash.
/// A last key that has itself been removed leaves the raw position alone;
/// nothing below the cursor moved in that case.
fn relocate_reverse_dict_cursor(cursor: &mut ReverseDictCursor) {
    cursor.relocate = false;
    let Some(key) = &cursor.last_key else {
        return;
    };
    if let Some(index) = live_collection_key_index(&cursor.container, key) {
        cursor.next_index = index;
    }
}

/// Read the entry one position below the cursor and descend.
///
/// The guard has already accepted this step, so the entry at `next_index - 1`
/// is the one CPython's `dictreviter` would read: its value is fetched now,
/// not when the iterator was created (#2932).
fn advance_reverse_dict_cursor(cursor: &mut ReverseDictCursor) -> Result<Option<LiveDictViewItem>> {
    if cursor.relocate {
        relocate_reverse_dict_cursor(cursor);
    }
    let Some(index) = cursor.next_index.checked_sub(1) else {
        cursor.last_key = None;
        return Ok(None);
    };
    // A shortened mapping is reported by the frame's size guard, so reaching a
    // missing entry here only means the walk is over.
    let Some((key, value)) = reverse_dict_entry(cursor, index)? else {
        cursor.next_index = 0;
        cursor.last_key = None;
        return Ok(None);
    };
    cursor.next_index = index;
    let item = match value {
        None => LiveDictViewItem::Item(key_ref_to_value(&key)),
        Some(value) if cursor.kind == 1 => LiveDictViewItem::Item(value),
        Some(value) => LiveDictViewItem::Pair(key_ref_to_value(&key), value),
    };
    cursor.last_key = Some(key);
    Ok(Some(item))
}

/// Read the entry at `index`, fetching its value only for a walk that yields
/// one. A key walk still needs the key back to re-anchor after a mutation.
#[inline]
fn reverse_dict_entry(
    cursor: &ReverseDictCursor,
    index: usize,
) -> Result<Option<(PyKey, Option<Value>)>> {
    if let Some(dict) = &cursor.backing {
        return Ok(if cursor.kind == 0 {
            pyrust_builtins::dict_views::key_at(dict, index).map(|key| (key, None))
        } else {
            pyrust_builtins::dict_views::entry_at(dict, index)
                .map(|(key, value)| (key, Some(value)))
        });
    }
    // Carriers whose backing identity can move re-probe the container itself.
    let Some(key) = live_collection_key_at(&cursor.container, index) else {
        return Ok(None);
    };
    if cursor.kind == 0 {
        return Ok(Some((key, None)));
    }
    let value = live_dict_value(&cursor.container, &key)
        .ok_or_else(|| PyError::runtime("dictionary keys changed during iteration".to_string()))?;
    Ok(Some((key, Some(value))))
}
