#[derive(Clone, Debug)]
struct DictReplayKey {
    key: Option<PyKey>,
    python_hash: u64,
    is_str: bool,
    replay_id: u64,
}

/// Temporary order-statistic sequence used while replaying an arbitrarily
/// long deferred journal. Undoing an ordinary deletion inserts its placeholder
/// at the historical live position; a `Vec::insert`/`remove` pair would make a
/// long series of front or middle deletions quadratic. This implicit treap
/// keeps every positional operation expected O(log n) and is discarded as
/// soon as the Python-hash shadow has been reconstructed.
#[derive(Debug)]
struct DictReplaySequenceNode {
    key: Option<DictReplayKey>,
    priority: u64,
    left: Option<usize>,
    right: Option<usize>,
    subtree_len: usize,
}

#[derive(Debug, Default)]
struct DictReplaySequence {
    nodes: Vec<DictReplaySequenceNode>,
    root: Option<usize>,
}

impl DictReplaySequence {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
            root: None,
        }
    }

    #[inline]
    fn node_len(&self, node: Option<usize>) -> usize {
        node.map_or(0, |index| self.nodes[index].subtree_len)
    }

    #[inline]
    fn len(&self) -> usize {
        self.node_len(self.root)
    }

    #[inline]
    fn priority_for(index: usize) -> u64 {
        // SplitMix64 gives deterministic, well-distributed priorities without
        // pulling a random generator into this temporary internal structure.
        let mut value = (index as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn allocate_node(&mut self, key: DictReplayKey) -> usize {
        let index = self.nodes.len();
        self.nodes.push(DictReplaySequenceNode {
            key: Some(key),
            priority: Self::priority_for(index),
            left: None,
            right: None,
            subtree_len: 1,
        });
        index
    }

    fn refresh(&mut self, node: usize) {
        let (left, right) = {
            let node = &self.nodes[node];
            (node.left, node.right)
        };
        self.nodes[node].subtree_len = 1 + self.node_len(left) + self.node_len(right);
    }

    fn merge(&mut self, left: Option<usize>, right: Option<usize>) -> Option<usize> {
        match (left, right) {
            (None, other) | (other, None) => other,
            (Some(left), Some(right)) => {
                if self.nodes[left].priority >= self.nodes[right].priority {
                    let left_right = self.nodes[left].right.take();
                    let merged = self.merge(left_right, Some(right));
                    self.nodes[left].right = merged;
                    self.refresh(left);
                    Some(left)
                } else {
                    let right_left = self.nodes[right].left.take();
                    let merged = self.merge(Some(left), right_left);
                    self.nodes[right].left = merged;
                    self.refresh(right);
                    Some(right)
                }
            }
        }
    }

    /// Split into the first `left_len` keys and the remaining suffix.
    fn split(&mut self, root: Option<usize>, left_len: usize) -> (Option<usize>, Option<usize>) {
        let Some(root) = root else {
            return (None, None);
        };
        let root_left_len = self.node_len(self.nodes[root].left);
        if left_len <= root_left_len {
            let left = self.nodes[root].left.take();
            let (prefix, new_left) = self.split(left, left_len);
            self.nodes[root].left = new_left;
            self.refresh(root);
            (prefix, Some(root))
        } else {
            let right = self.nodes[root].right.take();
            let (new_right, suffix) = self.split(right, left_len - root_left_len - 1);
            self.nodes[root].right = new_right;
            self.refresh(root);
            (Some(root), suffix)
        }
    }

    fn insert(&mut self, index: usize, key: DictReplayKey) {
        debug_assert!(index <= self.len());
        let root = self.root.take();
        let (left, right) = self.split(root, index);
        let node = self.allocate_node(key);
        let with_node = self.merge(left, Some(node));
        self.root = self.merge(with_node, right);
    }

    #[inline]
    fn push(&mut self, key: DictReplayKey) {
        let len = self.len();
        self.insert(len, key);
    }

    fn remove(&mut self, index: usize) -> DictReplayKey {
        debug_assert!(index < self.len());
        let root = self.root.take();
        let (left, remainder) = self.split(root, index);
        let (removed, right) = self.split(remainder, 1);
        self.root = self.merge(left, right);
        let removed = removed.expect("dict replay sequence removal must find a node");
        debug_assert_eq!(self.nodes[removed].subtree_len, 1);
        self.nodes[removed]
            .key
            .take()
            .expect("dict replay sequence node must be removed only once")
    }

    #[inline]
    fn pop(&mut self) -> Option<DictReplayKey> {
        self.len().checked_sub(1).map(|index| self.remove(index))
    }

    fn visit_in_order(&self, root: Option<usize>, visit: &mut impl FnMut(&DictReplayKey)) {
        let Some(root) = root else {
            return;
        };
        self.visit_in_order(self.nodes[root].left, visit);
        visit(
            self.nodes[root]
                .key
                .as_ref()
                .expect("live dict replay sequence node must retain its key"),
        );
        self.visit_in_order(self.nodes[root].right, visit);
    }

    fn for_each(&self, mut visit: impl FnMut(&DictReplayKey)) {
        self.visit_in_order(self.root, &mut visit);
    }

    fn matches_entries(&self, entries: &IndexMap<PyKey, Value, PyHasher>) -> bool {
        let mut entries = entries.keys();
        let mut matches = true;
        self.for_each(|replay| {
            matches &= replay
                .key
                .as_ref()
                .zip(entries.next())
                .is_some_and(|(replayed, stored)| replayed == stored);
        });
        matches && entries.next().is_none()
    }
}

/// O(1)-per-mutation history retained until a Python-hash probe shadow is
/// actually needed. Encoding the position and flags in one word keeps an
/// ordinary homogeneous deletion from cloning or allocating for every live
/// dict key.
#[derive(Clone, Copy, Debug)]
struct DictDeferredMutation {
    python_hash: u64,
    position_and_flags: usize,
}

#[derive(Clone, Debug, Default)]
struct DictMutationJournal {
    mutations: smallvec::SmallVec<[DictDeferredMutation; 2]>,
    /// Upper-bound accounting for compact-entry holes. `popitem()` truncates
    /// its own entry (and any preceding trailing holes), while ordinary
    /// deletion leaves one possible hole until a resize compacts the table.
    non_pop_removals: usize,
    /// `popitem()` can truncate earlier trailing holes, so `live_len +
    /// non_pop_removals` is exact `dk_nentries` only while this stays false.
    has_pop: bool,
    /// CPython's remaining compact-entry capacity. It is derivable without
    /// probing slots and lets an insertion recognize the exact resize that
    /// discards all earlier dummy history.
    usable: usize,
}

const DICT_MUTATION_KIND_MASK: usize = 0b11;
const DICT_MUTATION_INSERT: usize = 0;
const DICT_MUTATION_REMOVE: usize = 1;
const DICT_MUTATION_POP: usize = 2;
const DICT_MUTATION_IS_STR: usize = 0b100;
const DICT_MUTATION_POSITION_SHIFT: u32 = 3;

impl DictDeferredMutation {
    #[inline]
    fn inserted() -> Self {
        Self {
            python_hash: 0,
            position_and_flags: DICT_MUTATION_INSERT,
        }
    }

    fn removed(index: usize, key: &PyKey, popped: bool) -> Self {
        let position = index
            .checked_shl(DICT_MUTATION_POSITION_SHIFT)
            .expect("dict mutation journal position overflow");
        let kind = if popped {
            DICT_MUTATION_POP
        } else {
            DICT_MUTATION_REMOVE
        };
        let is_str = usize::from(matches!(key, PyKey::Str(_))) * DICT_MUTATION_IS_STR;
        Self {
            python_hash: py_hash_pykey(key) as u64,
            position_and_flags: position | is_str | kind,
        }
    }

    #[inline]
    fn kind(self) -> usize {
        self.position_and_flags & DICT_MUTATION_KIND_MASK
    }

    #[inline]
    fn position(self) -> usize {
        self.position_and_flags >> DICT_MUTATION_POSITION_SHIFT
    }

    #[inline]
    fn is_str(self) -> bool {
        self.position_and_flags & DICT_MUTATION_IS_STR != 0
    }
}

impl DictMutationJournal {
    fn from_current_entries(initial_table_size: usize, live_len: usize) -> Self {
        // Before the first deletion every structural operation was a unique
        // insertion. CPython's dk_usable and resize decisions therefore depend
        // only on the live count, so recover them in O(log n) resize epochs
        // without scanning or cloning the dictionary.
        let mut current_table_size = initial_table_size;
        let mut usable = dict_probe_usable_fraction(current_table_size);
        let mut placed = 0usize;
        while placed < live_len {
            if usable == 0 {
                current_table_size =
                    DictPythonHashIndex::table_size_for_target(placed.saturating_mul(3));
                usable = dict_probe_usable_fraction(current_table_size)
                    .checked_sub(placed)
                    .expect("resized dict probe table must hold every live entry");
            }
            let batch = usable.min(live_len - placed);
            placed += batch;
            usable -= batch;
        }
        Self {
            mutations: smallvec::SmallVec::new(),
            non_pop_removals: 0,
            has_pop: false,
            usable,
        }
    }

    /// Record a unique insertion and return a fresh replay baseline when
    /// CPython resizes first. A resize reinserts every live compact entry and
    /// removes all dummies, so prior mutations are no longer observable.
    fn record_inserted(&mut self, live_len_after: usize) -> Option<usize> {
        let live_len_before = live_len_after
            .checked_sub(1)
            .expect("inserted dict entry must make the dictionary non-empty");
        let rebased_table_size = if self.usable == 0 {
            let table_size =
                DictPythonHashIndex::table_size_for_target(live_len_before.saturating_mul(3));
            self.usable = dict_probe_usable_fraction(table_size)
                .checked_sub(live_len_before)
                .expect("resized dict probe table must hold every live entry");
            self.mutations.clear();
            self.non_pop_removals = 0;
            self.has_pop = false;
            Some(table_size)
        } else {
            None
        };
        debug_assert_ne!(self.usable, 0);
        self.usable -= 1;
        if rebased_table_size.is_none() {
            self.mutations.push(DictDeferredMutation::inserted());
        }
        rebased_table_size
    }

    #[inline]
    fn record_removed(&mut self, index: usize, key: &PyKey, popped: bool) {
        self.mutations
            .push(DictDeferredMutation::removed(index, key, popped));
        if !popped {
            self.non_pop_removals = self.non_pop_removals.saturating_add(1);
        } else {
            self.has_pop = true;
        }
    }

    #[inline]
    fn copy_definitely_preserves_layout(&self, live_len: usize) -> bool {
        // Each ordinary deletion can leave at most one compact-entry hole.
        // Resizes and popitem truncation only reduce that count, so satisfying
        // CPython's dense-copy threshold against this upper bound proves that
        // cloning the deferred history is layout-preserving.
        let compact_entries_upper = live_len.saturating_add(self.non_pop_removals);
        live_len >= compact_entries_upper.saturating_mul(2) / 3
    }

    #[inline]
    fn copy_definitely_compacts_layout(&self, live_len: usize) -> bool {
        if self.has_pop {
            return false;
        }
        // Without popitem truncation every ordinary deletion leaves exactly
        // one compact-entry hole, so this is CPython's exact dk_nentries. A
        // sparse copy reinserts only live entries; no slot replay is needed.
        let compact_entries = live_len.saturating_add(self.non_pop_removals);
        live_len < compact_entries.saturating_mul(2) / 3
    }

    #[cold]
    #[inline(never)]
    fn materialize(
        &self,
        entries: &IndexMap<PyKey, Value, PyHasher>,
        initial_table_size: usize,
        initial_unicode_only: bool,
        current_unicode_only: bool,
    ) -> DictPythonHashIndex {
        let mut next_replay_id = 0u64;
        let replay_capacity = entries
            .len()
            .saturating_add(self.mutations.len().saturating_mul(2));
        let mut state = DictReplaySequence::with_capacity(replay_capacity);
        for key in entries.keys() {
            state.push(DictReplayKey {
                key: Some(key.clone()),
                python_hash: py_hash_pykey(key) as u64,
                is_str: matches!(key, PyKey::Str(_)),
                replay_id: next_replay_id,
            });
            next_replay_id = next_replay_id.wrapping_add(1);
        }
        let mut inserted_keys: Vec<Option<DictReplayKey>> =
            (0..self.mutations.len()).map(|_| None).collect();

        // Undo the compact-entry mutations against the current live order.
        // Removed keys need only their hash and string/general classification:
        // CPython has already released the actual key object at that point.
        for (mutation_index, mutation) in self.mutations.iter().copied().enumerate().rev() {
            match mutation.kind() {
                DICT_MUTATION_INSERT => {
                    inserted_keys[mutation_index] = Some(
                        state
                            .pop()
                            .expect("dict mutation journal lost an inserted entry"),
                    );
                }
                DICT_MUTATION_REMOVE => {
                    let replay = DictReplayKey {
                        key: None,
                        python_hash: mutation.python_hash,
                        is_str: mutation.is_str(),
                        replay_id: next_replay_id,
                    };
                    next_replay_id = next_replay_id.wrapping_add(1);
                    state.insert(mutation.position(), replay);
                }
                DICT_MUTATION_POP => {
                    let replay = DictReplayKey {
                        key: None,
                        python_hash: mutation.python_hash,
                        is_str: mutation.is_str(),
                        replay_id: next_replay_id,
                    };
                    next_replay_id = next_replay_id.wrapping_add(1);
                    state.push(replay);
                }
                _ => unreachable!("invalid dict mutation journal operation"),
            }
        }

        let mut index = DictPythonHashIndex::with_table_size(initial_table_size);
        index.unicode_only = initial_unicode_only;
        index.begin_replay();
        state.for_each(|key| index.record_inserted_replay(key));

        // Replay the mutations to recover the exact current EMPTY/DUMMY and
        // compact-entry layout. `state` supplies stable identities even for
        // deleted keys whose full `PyKey` was deliberately not retained.
        for (mutation_index, mutation) in self.mutations.iter().copied().enumerate() {
            match mutation.kind() {
                DICT_MUTATION_INSERT => {
                    let key = inserted_keys[mutation_index]
                        .take()
                        .expect("dict mutation journal lost a forward insertion");
                    index.record_inserted_replay(&key);
                    state.push(key);
                }
                DICT_MUTATION_REMOVE => {
                    let key = state.remove(mutation.position());
                    debug_assert_eq!(key.python_hash, mutation.python_hash);
                    index.record_removed_replay(&key, false);
                }
                DICT_MUTATION_POP => {
                    let key = state
                        .pop()
                        .expect("dict mutation journal lost a popped entry");
                    debug_assert_eq!(key.python_hash, mutation.python_hash);
                    index.record_removed_replay(&key, true);
                }
                _ => unreachable!("invalid dict mutation journal operation"),
            }
        }

        debug_assert_eq!(state.len(), entries.len());
        debug_assert!(state.matches_entries(entries));
        if !current_unicode_only && index.unicode_only {
            index.prepare_general_insert();
        }
        index.finish_replay();
        index
    }
}

#[inline]
fn pykey_contains_object(key: &PyKey) -> bool {
    match key {
        PyKey::Object { .. } => true,
        PyKey::Tuple(items) => items.iter().any(pykey_contains_object),
        PyKey::FrozenSet(key) => key.contains_object(),
        _ => false,
    }
}
