const SET_PROBE_MINSIZE: usize = 8;
const SET_PROBE_LINEAR_PROBES: usize = 9;
const SET_PROBE_PERTURB_SHIFT: u32 = 5;

#[derive(Clone, Debug)]
struct SetProbeEntry {
    python_hash: u64,
    replay_id: u64,
}

const SET_REPLAY_EMPTY: usize = usize::MAX;
const SET_REPLAY_DUMMY: usize = usize::MAX - 1;

/// Immutable CPython 3.12 set-table snapshot used only when frozenset
/// equality can dispatch Python code. Keeping it separate from `IndexSet`
/// lets ordinary set operations retain their existing storage and hot path.
#[derive(Clone, Debug)]
pub struct PySetProbeSnapshot {
    /// EMPTY/DUMMY sentinels or a replay id indexing `hashes`.
    slots: Vec<usize>,
    /// Python hashes indexed by replay id. Removed keys need no retained
    /// `PyKey`; every live replay id indexes the source `PySet` directly.
    hashes: Vec<u64>,
    fill: usize,
    used: usize,
}

impl PartialEq for PySetProbeSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.fill == other.fill
            && self.used == other.used
            && self.slots.len() == other.slots.len()
            && self
                .slots
                .iter()
                .zip(&other.slots)
                .all(|(&left, &right)| match (left, right) {
                    (SET_REPLAY_EMPTY, SET_REPLAY_EMPTY) | (SET_REPLAY_DUMMY, SET_REPLAY_DUMMY) => {
                        true
                    }
                    (left, right) if left < SET_REPLAY_DUMMY && right < SET_REPLAY_DUMMY => {
                        self.hashes[left] == other.hashes[right]
                    }
                    _ => false,
                })
    }
}

impl PySetProbeSnapshot {
    fn with_table_size(table_size: usize) -> Self {
        let table_size = table_size.max(SET_PROBE_MINSIZE);
        debug_assert!(table_size.is_power_of_two());
        Self {
            slots: vec![SET_REPLAY_EMPTY; table_size],
            hashes: Vec::new(),
            fill: 0,
            used: 0,
        }
    }

    #[inline]
    fn table_size_for_minused(minused: usize) -> usize {
        let mut table_size = SET_PROBE_MINSIZE;
        while table_size <= minused {
            table_size = table_size
                .checked_mul(2)
                .expect("set probe table size overflow");
        }
        table_size
    }

    #[inline]
    fn table_size(&self) -> usize {
        self.slots.len()
    }

    #[inline]
    fn has_dummies(&self) -> bool {
        self.fill != self.used
    }

    fn insertion_slot(&self, python_hash: u64) -> (usize, bool) {
        let mask = self.slots.len() - 1;
        let mut perturb = python_hash;
        let mut slot = python_hash as usize & mask;
        let mut reusable_dummy = None;
        loop {
            let linear_end = if slot + SET_PROBE_LINEAR_PROBES <= mask {
                slot + SET_PROBE_LINEAR_PROBES
            } else {
                slot
            };
            for candidate_slot in slot..=linear_end {
                match self.slots[candidate_slot] {
                    SET_REPLAY_EMPTY => {
                        return reusable_dummy
                            .map_or((candidate_slot, true), |dummy| (dummy, false));
                    }
                    SET_REPLAY_DUMMY => reusable_dummy = Some(candidate_slot),
                    _ => {}
                }
            }
            perturb >>= SET_PROBE_PERTURB_SHIFT;
            slot = slot
                .wrapping_mul(5)
                .wrapping_add(1)
                .wrapping_add(perturb as usize)
                & mask;
        }
    }

    fn register_entry(&mut self, entry: SetProbeEntry) -> usize {
        let replay_id = usize::try_from(entry.replay_id).expect("set replay id must fit usize");
        assert!(replay_id < SET_REPLAY_DUMMY, "set replay id overflow");
        if replay_id >= self.hashes.len() {
            self.hashes.resize(replay_id + 1, 0);
        }
        self.hashes[replay_id] = entry.python_hash;
        replay_id
    }

    fn insert_clean_replay_id(&mut self, replay_id: usize) {
        let python_hash = self.hashes[replay_id];
        let mask = self.slots.len() - 1;
        let mut perturb = python_hash;
        let mut slot = python_hash as usize & mask;
        loop {
            let linear_end = if slot + SET_PROBE_LINEAR_PROBES <= mask {
                slot + SET_PROBE_LINEAR_PROBES
            } else {
                slot
            };
            for candidate_slot in slot..=linear_end {
                if self.slots[candidate_slot] == SET_REPLAY_EMPTY {
                    self.slots[candidate_slot] = replay_id;
                    self.fill += 1;
                    self.used += 1;
                    return;
                }
            }
            perturb >>= SET_PROBE_PERTURB_SHIFT;
            slot = slot
                .wrapping_mul(5)
                .wrapping_add(1)
                .wrapping_add(perturb as usize)
                & mask;
        }
    }

    fn insert(&mut self, entry: SetProbeEntry) {
        let python_hash = entry.python_hash;
        let replay_id = self.register_entry(entry);
        let (slot, was_empty) = self.insertion_slot(python_hash);
        self.slots[slot] = replay_id;
        self.used += 1;
        if !was_empty {
            return;
        }
        self.fill += 1;
        let mask = self.slots.len() - 1;
        if self.fill.saturating_mul(5) < mask.saturating_mul(3) {
            return;
        }
        let minused = if self.used > 50_000 {
            self.used.saturating_mul(2)
        } else {
            self.used.saturating_mul(4)
        };
        self.resize(minused);
    }

    fn resize(&mut self, minused: usize) {
        let new_size = Self::table_size_for_minused(minused);
        if new_size == SET_PROBE_MINSIZE
            && self.slots.len() == SET_PROBE_MINSIZE
            && !self.has_dummies()
        {
            return;
        }
        let old_slots = std::mem::replace(&mut self.slots, vec![SET_REPLAY_EMPTY; new_size]);
        self.fill = 0;
        self.used = 0;
        for replay_id in old_slots {
            if replay_id < SET_REPLAY_DUMMY {
                self.insert_clean_replay_id(replay_id);
            }
        }
    }

    fn prepare_merge(&mut self, additional_used: usize) {
        let mask = self.slots.len() - 1;
        if self.fill.saturating_add(additional_used).saturating_mul(5) >= mask.saturating_mul(3) {
            self.resize(self.used.saturating_add(additional_used).saturating_mul(2));
        }
    }

    fn finish_difference_update(&mut self) {
        let mask = self.slots.len() - 1;
        if self.fill.saturating_sub(self.used) <= mask / 4 {
            return;
        }
        let minused = if self.used > 50_000 {
            self.used.saturating_mul(2)
        } else {
            self.used.saturating_mul(4)
        };
        self.resize(minused);
    }

    fn remove(&mut self, python_hash: u64, replay_id: u64) {
        let replay_id = usize::try_from(replay_id).expect("set replay id must fit usize");
        let mask = self.slots.len() - 1;
        let mut perturb = python_hash;
        let mut slot = python_hash as usize & mask;
        loop {
            let linear_end = if slot + SET_PROBE_LINEAR_PROBES <= mask {
                slot + SET_PROBE_LINEAR_PROBES
            } else {
                slot
            };
            for candidate_slot in slot..=linear_end {
                match self.slots[candidate_slot] {
                    SET_REPLAY_EMPTY => panic!("set mutation journal lost a removed entry"),
                    candidate if candidate == replay_id => {
                        self.slots[candidate_slot] = SET_REPLAY_DUMMY;
                        self.used -= 1;
                        return;
                    }
                    _ => {}
                }
            }
            perturb >>= SET_PROBE_PERTURB_SHIFT;
            slot = slot
                .wrapping_mul(5)
                .wrapping_add(1)
                .wrapping_add(perturb as usize)
                & mask;
        }
    }

    pub fn active_keys<'a>(&'a self, source: &'a PySet) -> impl Iterator<Item = &'a PyKey> + 'a {
        self.slots
            .iter()
            .copied()
            .filter(|replay_id| *replay_id < SET_REPLAY_DUMMY)
            .map(move |replay_id| {
                source
                    .entries
                    .get_index(replay_id)
                    .expect("live set replay id must index a source key")
            })
    }

    #[cfg(test)]
    fn active_python_hashes(&self) -> impl Iterator<Item = u64> + '_ {
        self.slots
            .iter()
            .copied()
            .filter(|replay_id| *replay_id < SET_REPLAY_DUMMY)
            .map(|replay_id| self.hashes[replay_id])
    }

    pub fn collect_candidate_keys<'a>(
        &self,
        source: &'a PySet,
        python_hash: u64,
        candidates: &mut Vec<&'a PyKey>,
    ) {
        candidates.clear();
        let mask = self.slots.len() - 1;
        let mut perturb = python_hash;
        let mut slot = python_hash as usize & mask;
        loop {
            let linear_end = if slot + SET_PROBE_LINEAR_PROBES <= mask {
                slot + SET_PROBE_LINEAR_PROBES
            } else {
                slot
            };
            for candidate_slot in slot..=linear_end {
                match self.slots[candidate_slot] {
                    SET_REPLAY_EMPTY => return,
                    replay_id
                        if replay_id < SET_REPLAY_DUMMY
                            && self.hashes[replay_id] == python_hash =>
                    {
                        candidates.push(
                            source
                                .entries
                                .get_index(replay_id)
                                .expect("live set replay id must index a source key"),
                        );
                    }
                    _ => {}
                }
            }
            perturb >>= SET_PROBE_PERTURB_SHIFT;
            slot = slot
                .wrapping_mul(5)
                .wrapping_add(1)
                .wrapping_add(perturb as usize)
                & mask;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SetDeferredMutation {
    python_hash: u64,
    position_and_kind: usize,
}

const SET_MUTATION_INSERT: usize = 0;
const SET_MUTATION_REMOVE: usize = 1;
const SET_MUTATION_MERGE: usize = 2;
const SET_MUTATION_DIFFERENCE_CLEANUP: usize = 3;
const SET_MUTATION_KIND_MASK: usize = 0b11;
const SET_MUTATION_POSITION_SHIFT: u32 = 2;

impl SetDeferredMutation {
    #[inline]
    fn inserted() -> Self {
        Self {
            python_hash: 0,
            position_and_kind: SET_MUTATION_INSERT,
        }
    }

    fn removed(position: usize, key: &PyKey) -> Self {
        Self::removed_hash(position, py_hash_pykey(key) as u64)
    }

    fn removed_hash(position: usize, python_hash: u64) -> Self {
        Self {
            python_hash,
            position_and_kind: position
                .checked_shl(SET_MUTATION_POSITION_SHIFT)
                .expect("set mutation journal position overflow")
                | SET_MUTATION_REMOVE,
        }
    }

    fn merge(additional_used: usize) -> Self {
        Self {
            python_hash: u64::try_from(additional_used)
                .expect("set merge size must fit the probe journal"),
            position_and_kind: SET_MUTATION_MERGE,
        }
    }

    #[inline]
    fn difference_cleanup() -> Self {
        Self {
            python_hash: 0,
            position_and_kind: SET_MUTATION_DIFFERENCE_CLEANUP,
        }
    }

    #[inline]
    fn kind(self) -> usize {
        self.position_and_kind & SET_MUTATION_KIND_MASK
    }

    #[inline]
    fn position(self) -> usize {
        self.position_and_kind >> SET_MUTATION_POSITION_SHIFT
    }
}

#[derive(Clone, Debug, Default)]
struct SetMutationJournal {
    mutations: smallvec::SmallVec<[SetDeferredMutation; 2]>,
}

impl SetMutationJournal {
    #[inline]
    fn record_inserted(&mut self) {
        self.mutations.push(SetDeferredMutation::inserted());
    }

    #[inline]
    fn record_removed(&mut self, position: usize, key: &PyKey) {
        self.mutations
            .push(SetDeferredMutation::removed(position, key));
    }

    #[inline]
    fn record_removed_hash(&mut self, position: usize, python_hash: u64) {
        self.mutations
            .push(SetDeferredMutation::removed_hash(position, python_hash));
    }

    #[inline]
    fn record_merge(&mut self, additional_used: usize) {
        self.mutations
            .push(SetDeferredMutation::merge(additional_used));
    }

    #[inline]
    fn record_difference_cleanup(&mut self) {
        self.mutations
            .push(SetDeferredMutation::difference_cleanup());
    }

    fn has_only_tail_removals(&self, live_len: usize) -> bool {
        let original_len = live_len.saturating_add(self.mutations.len());
        !self.mutations.is_empty()
            && self
                .mutations
                .iter()
                .copied()
                .enumerate()
                .all(|(index, mutation)| {
                    mutation.kind() == SET_MUTATION_REMOVE
                        && mutation.position() == original_len - index - 1
                })
    }

    fn materialize_tail_removals(
        &self,
        entries: &IndexSet<PyKey, PyHasher>,
        initial_table_size: usize,
    ) -> PySetProbeSnapshot {
        let live_len = entries.len();
        let mut table = PySetProbeSnapshot::with_table_size(initial_table_size);
        for (replay_id, key) in entries.iter().enumerate() {
            table.insert(SetProbeEntry {
                python_hash: py_hash_pykey(key) as u64,
                replay_id: replay_id as u64,
            });
        }

        // Undoing tail removals only appends placeholders. Avoid building the
        // general order-statistic replay sequence for this common sparse-set
        // shape (for example, repeated remove(max_value)).
        for (reverse_index, mutation) in self.mutations.iter().copied().rev().enumerate() {
            table.insert(SetProbeEntry {
                python_hash: mutation.python_hash,
                replay_id: (live_len + reverse_index) as u64,
            });
        }
        for (mutation_index, mutation) in self.mutations.iter().copied().enumerate() {
            let reverse_index = self.mutations.len() - mutation_index - 1;
            table.remove(mutation.python_hash, (live_len + reverse_index) as u64);
        }
        debug_assert_eq!(table.used, live_len);
        table
    }

    fn materialize(
        &self,
        entries: &IndexSet<PyKey, PyHasher>,
        initial_table_size: usize,
    ) -> PySetProbeSnapshot {
        if self.has_only_tail_removals(entries.len()) {
            return self.materialize_tail_removals(entries, initial_table_size);
        }
        let mut next_replay_id = 0u64;
        let replay_capacity = entries
            .len()
            .saturating_add(self.mutations.len().saturating_mul(2));
        let mut state = DictReplaySequence::with_capacity(replay_capacity);
        for key in entries {
            state.push(DictReplayKey {
                key: Some(key.clone()),
                python_hash: py_hash_pykey(key) as u64,
                is_str: false,
                replay_id: next_replay_id,
            });
            next_replay_id = next_replay_id.wrapping_add(1);
        }

        let mut inserted_keys: Vec<Option<DictReplayKey>> =
            (0..self.mutations.len()).map(|_| None).collect();
        let mut removed_replay_ids = vec![None; self.mutations.len()];
        for (mutation_index, mutation) in self.mutations.iter().copied().enumerate().rev() {
            match mutation.kind() {
                SET_MUTATION_INSERT => {
                    inserted_keys[mutation_index] = Some(
                        state
                            .pop()
                            .expect("set mutation journal lost an inserted entry"),
                    );
                }
                SET_MUTATION_REMOVE => {
                    let replay_id = next_replay_id;
                    next_replay_id = next_replay_id.wrapping_add(1);
                    state.insert(
                        mutation.position(),
                        DictReplayKey {
                            key: None,
                            python_hash: mutation.python_hash,
                            is_str: false,
                            replay_id,
                        },
                    );
                    removed_replay_ids[mutation_index] = Some(replay_id);
                }
                SET_MUTATION_MERGE => {}
                SET_MUTATION_DIFFERENCE_CLEANUP => {}
                _ => unreachable!("invalid set mutation journal operation"),
            }
        }

        let mut table = PySetProbeSnapshot::with_table_size(initial_table_size);
        state.for_each(|key| {
            table.insert(SetProbeEntry {
                python_hash: key.python_hash,
                replay_id: key.replay_id,
            });
        });
        for (mutation_index, mutation) in self.mutations.iter().copied().enumerate() {
            match mutation.kind() {
                SET_MUTATION_INSERT => {
                    let key = inserted_keys[mutation_index]
                        .take()
                        .expect("set mutation replay lost its inserted key");
                    table.insert(SetProbeEntry {
                        python_hash: key.python_hash,
                        replay_id: key.replay_id,
                    });
                }
                SET_MUTATION_REMOVE => table.remove(
                    mutation.python_hash,
                    removed_replay_ids[mutation_index]
                        .expect("set mutation replay lost its removed key"),
                ),
                SET_MUTATION_MERGE => table.prepare_merge(
                    usize::try_from(mutation.python_hash).expect("set merge size must fit usize"),
                ),
                SET_MUTATION_DIFFERENCE_CLEANUP => table.finish_difference_update(),
                _ => unreachable!("invalid set mutation journal operation"),
            }
        }
        debug_assert_eq!(table.used, entries.len());
        table
    }
}
