/// Compact shadow of CPython's combined-dict key table.
///
/// `IndexMap` deliberately hashes different `PyKey` representations into
/// different native buckets, but Python compares all equal-hash keys in one
/// perturb probe sequence. Once a dict mixes Object/non-Object keys, this
/// shadow records that sequence exactly; homogeneous deletions first use a
/// bounded compact journal. Ordinary homogeneous insertion remains allocation-
/// and clone-free (#2060).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DictKeyTableKind {
    /// A presized empty table chooses its concrete layout from the first key.
    Unknown,
    Unicode,
    General,
}

impl DictKeyTableKind {
    #[inline]
    fn is_unicode(self) -> bool {
        self == Self::Unicode
    }
}

#[derive(Clone, Debug)]
struct DictPythonHashIndex {
    /// CPython-style indices: -1 empty, -2 dummy, otherwise `entry_keys` index.
    indices: Vec<isize>,
    /// Compact-dict entries in insertion order. Deleted entries are `None`.
    entry_keys: Vec<Option<PyKey>>,
    /// Temporary metadata used only while materializing a deferred mutation
    /// journal. Normal active shadows keep the original compact shape.
    replay_metadata: Option<Box<DictReplayMetadata>>,
    usable: usize,
    live_len: usize,
    unicode_only: bool,
}

#[derive(Clone, Debug, Default)]
struct DictReplayMetadata {
    entry_hashes: Vec<u64>,
    entry_replay_ids: Vec<u64>,
    entry_live: Vec<bool>,
}

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
const DICT_INDEX_EMPTY: isize = -1;
const DICT_INDEX_DUMMY: isize = -2;
const DICT_PERTURB_SHIFT: u32 = 5;

#[inline]
fn dict_probe_usable_fraction(table_size: usize) -> usize {
    table_size.saturating_mul(2) / 3
}

impl Default for DictPythonHashIndex {
    fn default() -> Self {
        Self {
            indices: Vec::new(),
            entry_keys: Vec::new(),
            replay_metadata: None,
            usable: 0,
            live_len: 0,
            unicode_only: true,
        }
    }
}

impl DictPythonHashIndex {
    /// CPython 3.12's `calculate_log2_keysize`, including its minimum-size
    /// bit trick. For small nonzero targets this intentionally selects 16
    /// rather than 8 (for example, unicode→general conversion at used == 1).
    fn table_size_for_target(target: usize) -> usize {
        let adjusted = (target | 8).saturating_sub(1);
        let log2_size = usize::BITS - adjusted.leading_zeros();
        1usize
            .checked_shl(log2_size)
            .expect("dict probe table size overflow")
    }

    #[inline]
    fn table_size_for_capacity(capacity: usize) -> usize {
        if capacity == 0 {
            return 0;
        }
        let mut table_size = 8usize;
        while dict_probe_usable_fraction(table_size) < capacity {
            table_size = table_size
                .checked_mul(2)
                .expect("dict probe table capacity overflow");
        }
        table_size
    }

    fn with_table_size(table_size: usize) -> Self {
        if table_size == 0 {
            return Self::default();
        }
        debug_assert!(table_size.is_power_of_two());
        let usable = dict_probe_usable_fraction(table_size);
        Self {
            indices: vec![DICT_INDEX_EMPTY; table_size],
            entry_keys: Vec::with_capacity(usable),
            replay_metadata: None,
            usable,
            live_len: 0,
            unicode_only: true,
        }
    }

    fn from_entries(
        entries: &IndexMap<PyKey, Value, PyHasher>,
        initial_table_size: usize,
        initial_unicode_only: bool,
        current_unicode_only: bool,
    ) -> Self {
        let mut index = Self::with_table_size(initial_table_size);
        index.unicode_only = initial_unicode_only;
        for key in entries.keys() {
            index.record_inserted(key);
        }
        if !current_unicode_only && index.unicode_only {
            index.prepare_general_insert();
        }
        index
    }

    #[inline]
    fn next_probe(slot: usize, perturb: &mut u64, mask: usize) -> usize {
        *perturb >>= DICT_PERTURB_SHIFT;
        slot.wrapping_mul(5)
            .wrapping_add(*perturb as usize)
            .wrapping_add(1)
            & mask
    }

    fn insertion_slot(&self, python_hash: u64) -> usize {
        debug_assert!(!self.indices.is_empty());
        let mask = self.indices.len() - 1;
        let mut perturb = python_hash;
        let mut slot = python_hash as usize & mask;
        loop {
            // CPython's find_empty_slot stops at the first negative index:
            // either an EMPTY or a DUMMY is immediately reusable.
            if self.indices[slot] < 0 {
                return slot;
            }
            slot = Self::next_probe(slot, &mut perturb, mask);
        }
    }

    fn place_without_resize(&mut self, key: PyKey) {
        debug_assert_ne!(self.usable, 0);
        debug_assert!(self.replay_metadata.is_none());
        let python_hash = py_hash_pykey(&key) as u64;
        let slot = self.insertion_slot(python_hash);
        let entry_index = self.entry_keys.len();
        self.entry_keys.push(Some(key));
        self.indices[slot] =
            isize::try_from(entry_index).expect("dict probe entry index must fit in isize");
        self.usable -= 1;
        self.live_len += 1;
    }

    fn place_without_resize_replay(
        &mut self,
        key: Option<PyKey>,
        python_hash: u64,
        replay_id: u64,
    ) {
        debug_assert_ne!(self.usable, 0);
        let slot = self.insertion_slot(python_hash);
        let entry_index = self.entry_keys.len();
        self.entry_keys.push(key);
        let replay = self
            .replay_metadata
            .as_mut()
            .expect("dict replay insertion requires temporary metadata");
        replay.entry_hashes.push(python_hash);
        replay.entry_replay_ids.push(replay_id);
        replay.entry_live.push(true);
        self.indices[slot] =
            isize::try_from(entry_index).expect("dict probe entry index must fit in isize");
        self.usable -= 1;
        self.live_len += 1;
    }

    /// Rebuild the table from live compact entries in insertion order.
    /// `target` is CPython's requested key-table size, not its usable count.
    fn resize(&mut self, target: usize) {
        let table_size = Self::table_size_for_target(target);
        let old_entries = std::mem::take(&mut self.entry_keys);
        self.indices = vec![DICT_INDEX_EMPTY; table_size];
        self.entry_keys = Vec::with_capacity(dict_probe_usable_fraction(table_size));
        self.usable = dict_probe_usable_fraction(table_size);
        self.live_len = 0;

        if let Some(mut replay) = self.replay_metadata.take() {
            let old_hashes = std::mem::take(&mut replay.entry_hashes);
            let old_replay_ids = std::mem::take(&mut replay.entry_replay_ids);
            let old_live = std::mem::take(&mut replay.entry_live);
            replay.entry_hashes = Vec::with_capacity(dict_probe_usable_fraction(table_size));
            replay.entry_replay_ids = Vec::with_capacity(dict_probe_usable_fraction(table_size));
            replay.entry_live = Vec::with_capacity(dict_probe_usable_fraction(table_size));
            self.replay_metadata = Some(replay);
            for (((key, python_hash), replay_id), live) in old_entries
                .into_iter()
                .zip(old_hashes)
                .zip(old_replay_ids)
                .zip(old_live)
            {
                if live {
                    self.place_without_resize_replay(key, python_hash, replay_id);
                }
            }
        } else {
            for key in old_entries.into_iter().flatten() {
                self.place_without_resize(key);
            }
        }
    }

    /// CPython converts a unicode-only table to the general key layout before
    /// looking up any non-exact-string insertion key. The conversion rebuilds
    /// all live entries and drops every dummy, even when lookup later finds an
    /// equal existing string and only replaces its value.
    fn prepare_general_insert(&mut self) {
        if self.unicode_only {
            self.resize(self.live_len.saturating_mul(3));
            self.unicode_only = false;
        }
    }

    fn record_inserted(&mut self, key: &PyKey) {
        if self.unicode_only && !matches!(key, PyKey::Str(_)) {
            self.prepare_general_insert();
        }
        if self.usable == 0 {
            self.resize(self.live_len.saturating_mul(3));
        }
        self.place_without_resize(key.clone());
    }

    fn record_inserted_replay(&mut self, key: &DictReplayKey) {
        if self.unicode_only && !key.is_str {
            self.prepare_general_insert();
        }
        if self.usable == 0 {
            self.resize(self.live_len.saturating_mul(3));
        }
        self.place_without_resize_replay(key.key.clone(), key.python_hash, key.replay_id);
    }

    #[inline]
    fn entry_is_live(&self, entry_index: usize) -> bool {
        self.replay_metadata.as_ref().map_or_else(
            || self.entry_keys[entry_index].is_some(),
            |replay| replay.entry_live[entry_index],
        )
    }

    #[inline]
    fn entry_python_hash(&self, entry_index: usize, key: &PyKey) -> u64 {
        self.replay_metadata.as_ref().map_or_else(
            || py_hash_pykey(key) as u64,
            |replay| replay.entry_hashes[entry_index],
        )
    }

    fn exact_slot(&self, key: &PyKey) -> Option<usize> {
        if self.indices.is_empty() {
            return None;
        }
        let python_hash = py_hash_pykey(key) as u64;
        let mask = self.indices.len() - 1;
        let mut perturb = python_hash;
        let mut slot = python_hash as usize & mask;
        loop {
            let entry_index = self.indices[slot];
            if entry_index == DICT_INDEX_EMPTY {
                return None;
            }
            if entry_index >= 0
                && self.entry_is_live(entry_index as usize)
                && self.entry_keys[entry_index as usize]
                    .as_ref()
                    .is_some_and(|stored| stored == key)
            {
                return Some(slot);
            }
            slot = Self::next_probe(slot, &mut perturb, mask);
        }
    }

    fn record_removed(&mut self, key: &PyKey) {
        let Some(slot) = self.exact_slot(key) else {
            debug_assert!(false, "dict probe shadow lost an exact stored key");
            return;
        };
        self.mark_slot_removed(slot);
        // Deletion does not restore dk_usable: new entries are still appended
        // to the compact entry array.
    }

    fn replay_slot(&self, key: &DictReplayKey) -> Option<usize> {
        if self.indices.is_empty() {
            return None;
        }
        let mask = self.indices.len() - 1;
        let replay = self
            .replay_metadata
            .as_ref()
            .expect("dict replay lookup requires temporary metadata");
        let mut perturb = key.python_hash;
        let mut slot = key.python_hash as usize & mask;
        loop {
            let entry_index = self.indices[slot];
            if entry_index == DICT_INDEX_EMPTY {
                return None;
            }
            if entry_index >= 0
                && replay.entry_live[entry_index as usize]
                && replay.entry_replay_ids[entry_index as usize] == key.replay_id
            {
                return Some(slot);
            }
            slot = Self::next_probe(slot, &mut perturb, mask);
        }
    }

    fn mark_slot_removed(&mut self, slot: usize) {
        let entry_index = self.indices[slot] as usize;
        self.indices[slot] = DICT_INDEX_DUMMY;
        self.entry_keys[entry_index] = None;
        if let Some(replay) = self.replay_metadata.as_mut() {
            replay.entry_live[entry_index] = false;
        }
        self.live_len -= 1;
    }

    fn record_removed_replay(&mut self, key: &DictReplayKey, popped: bool) {
        let Some(slot) = self.replay_slot(key) else {
            debug_assert!(false, "dict probe journal lost a replay entry");
            return;
        };
        self.mark_slot_removed(slot);
        if popped {
            self.truncate_popped_entries();
        }
    }

    fn record_popped(&mut self, key: &PyKey) {
        self.record_removed(key);
        self.truncate_popped_entries();
    }

    fn truncate_popped_entries(&mut self) {
        // PyDict_PopItem removes the last live compact entry and shortens
        // dk_nentries past any preceding holes, while leaving dk_usable alone.
        while self.entry_keys.last().is_some_and(|_| {
            let entry_index = self.entry_keys.len() - 1;
            !self.entry_is_live(entry_index)
        }) {
            self.entry_keys.pop();
            if let Some(replay) = self.replay_metadata.as_mut() {
                replay.entry_hashes.pop();
                replay.entry_replay_ids.pop();
                replay.entry_live.pop();
            }
        }
    }

    fn candidates(&self, python_hash: u64) -> Vec<PyKey> {
        if self.indices.is_empty() {
            return Vec::new();
        }
        let mask = self.indices.len() - 1;
        let mut perturb = python_hash;
        let mut slot = python_hash as usize & mask;
        let mut candidates = Vec::new();
        loop {
            let entry_index = self.indices[slot];
            if entry_index == DICT_INDEX_EMPTY {
                return candidates;
            }
            if entry_index >= 0
                && self.entry_is_live(entry_index as usize)
                && let Some(key) = self.entry_keys[entry_index as usize].as_ref()
                && self.entry_python_hash(entry_index as usize, key) == python_hash
            {
                candidates.push(key.clone());
            }
            slot = Self::next_probe(slot, &mut perturb, mask);
        }
    }

    fn has_candidate(&self, python_hash: u64, predicate: impl Fn(&PyKey) -> bool) -> bool {
        if self.indices.is_empty() {
            return false;
        }
        let mask = self.indices.len() - 1;
        let mut perturb = python_hash;
        let mut slot = python_hash as usize & mask;
        loop {
            let entry_index = self.indices[slot];
            if entry_index == DICT_INDEX_EMPTY {
                return false;
            }
            if entry_index >= 0
                && self.entry_is_live(entry_index as usize)
                && let Some(key) = self.entry_keys[entry_index as usize].as_ref()
                && self.entry_python_hash(entry_index as usize, key) == python_hash
                && predicate(key)
            {
                return true;
            }
            slot = Self::next_probe(slot, &mut perturb, mask);
        }
    }

    #[inline]
    fn has_object_candidate(&self, python_hash: u64) -> bool {
        self.has_candidate(python_hash, |key| matches!(key, PyKey::Object { .. }))
    }

    #[inline]
    fn has_dynamic_candidate(&self, python_hash: u64) -> bool {
        self.has_candidate(python_hash, pykey_contains_object)
    }

    fn has_non_object_key(&self, python_hash: u64) -> bool {
        self.candidates(python_hash)
            .iter()
            .any(|key| !matches!(key, PyKey::Object { .. }))
    }

    fn copy_preserves_layout(&self) -> bool {
        debug_assert!(self.replay_metadata.is_none());
        self.live_len >= self.entry_keys.len().saturating_mul(2) / 3
    }

    fn compacted_copy(&self) -> Self {
        debug_assert!(self.replay_metadata.is_none());
        // PyDict_Copy's sparse path merges into a fresh dict. `dict_merge`
        // presizes from the live count and explicitly preserves the source
        // unicode/general kind while reinserting live compact entries.
        let mut copy = Self::with_table_size(Self::sparse_copy_table_size(self.live_len));
        copy.unicode_only = self.unicode_only;
        for key in self.entry_keys.iter().filter_map(Option::as_ref) {
            copy.place_without_resize(key.clone());
        }
        copy
    }

    #[inline]
    fn sparse_copy_table_size(live_len: usize) -> usize {
        let estimate = live_len.saturating_mul(3).saturating_add(1) / 2;
        Self::table_size_for_target(estimate)
    }

    fn begin_replay(&mut self) {
        debug_assert!(self.entry_keys.is_empty());
        debug_assert!(self.replay_metadata.is_none());
        let capacity = self.entry_keys.capacity();
        self.replay_metadata = Some(Box::new(DictReplayMetadata {
            entry_hashes: Vec::with_capacity(capacity),
            entry_replay_ids: Vec::with_capacity(capacity),
            entry_live: Vec::with_capacity(capacity),
        }));
    }

    fn finish_replay(&mut self) {
        let replay = self
            .replay_metadata
            .as_ref()
            .expect("dict journal materialization must be in replay mode");
        debug_assert_eq!(replay.entry_live.len(), self.entry_keys.len());
        debug_assert!(
            replay
                .entry_live
                .iter()
                .zip(&self.entry_keys)
                .all(|(live, key)| !live || key.is_some())
        );
        self.replay_metadata = None;
    }
}

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
        PyKey::FrozenSet(key) => key.items().iter().any(pykey_contains_object),
        _ => false,
    }
}
