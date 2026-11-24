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
    if !state.has_dict_layout.get() {
        return None;
    }
    let dict = dict.borrow();
    let watched_presence = if state.has_watchers.get() {
        state
            .removal_watchers
            .borrow()
            .iter()
            .map(|(key, _)| (key.clone(), dict.contains_key(key)))
            .collect()
    } else {
        Vec::new()
    };
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

    // An entry appeared or disappeared: every node from there on is relinked.
    // A pure value rewrite leaves both counts at zero, and a reset is the arm
    // CPython diagnoses by size instead, so neither advances the generation.
    if !dict_mutation.reset && dict_mutation.removed_count + dict_mutation.inserted_count > 0 {
        state
            .entry_order
            .set(state.entry_order.get().wrapping_add(1));
    }

    if !dict_mutation.removed_watched.is_empty() {
        let mut pending = state.pending_removed_keys.borrow_mut();
        for key in dict_mutation.removed_watched {
            if !pending.contains(&key) {
                pending.push(key);
                state.has_mutation_activity.set(true);
            }
        }
    }

    if !state.has_dict_layout.get() {
        return;
    }
    let mut layout = DictIterationLayout {
        live_len: state.dict_layout_live_len.get(),
        entry_count: state.dict_layout_entry_count.get(),
        usable: state.dict_layout_usable.get(),
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
            let was_removed = {
                let mut pending = state.pending_removed_keys.borrow_mut();
                pending
                    .iter()
                    .position(|pending| *pending == key)
                    .map(|index| pending.swap_remove(index))
                    .is_some()
            };
            if was_removed {
                let mut reinserted = state.reinserted_keys.borrow_mut();
                match reinserted
                    .iter_mut()
                    .find(|(reinserted, _, _)| *reinserted == key)
                {
                    Some(entry) => *entry = (key, next_version, position),
                    None => reinserted.push((key, next_version, position)),
                }
                state.has_mutation_activity.set(true);
            }
        }
    }
    layout.live_len = dict_mutation.after_len;
    state.dict_layout_live_len.set(layout.live_len);
    state.dict_layout_entry_count.set(layout.entry_count);
    state.dict_layout_usable.set(layout.usable);
}

#[cold]
#[inline(never)]
fn bump_collection_mutation_state_slow(key: MutableContainerKey) {
    let state = COLLECTION_MUTATION_STATES
        .with(|states| states.borrow().get(&key).and_then(Weak::upgrade))
        .filter(|state| state.observers.get() != 0);
    bump_active_collection_mutation_state(state, None);
}

#[inline]
fn bump_collection_mutation_state(key: MutableContainerKey) {
    if ACTIVE_COLLECTION_MUTATION_STATES
        .try_with(|active| active.get() == 0)
        .unwrap_or(true)
    {
        return;
    }
    bump_collection_mutation_state_slow(key);
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
    dict_iteration_mutation_state_with_len(dict, dict.borrow().len())
}

/// Register an active iterator against a dict backing store whose length was
/// already read under the caller's borrow, avoiding a redundant `RefCell`
/// borrow and `Weak::upgrade` on retained-registration hits (#2924).
pub fn dict_iteration_mutation_state_with_len(
    dict: &Rc<RefCell<PyDict>>,
    len: usize,
) -> CollectionMutationState {
    let key = dict_container_key(dict);
    let registered = COLLECTION_MUTATION_STATES.with(|states| {
        states
            .borrow()
            .get(&key)
            .and_then(Weak::upgrade)
            .filter(|state| state.target.matches_dict(dict))
    });
    if let Some(state) = registered {
        if state.observers.get() == 0 {
            restart_idle_mutation_state_with_len(&state, Some(len));
        }
        return CollectionMutationState::attach(state);
    }
    collection_mutation_state(key, MutableContainerTarget::Dict(Rc::downgrade(dict)))
}
