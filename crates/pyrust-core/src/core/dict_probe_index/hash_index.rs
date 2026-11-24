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
