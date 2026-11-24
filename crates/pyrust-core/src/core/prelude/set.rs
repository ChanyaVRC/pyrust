/// Insertion-ordered backing store for a Python `set` / `frozenset`.
///
/// `entries` remains the compact PyRust representation used by ordinary set
/// operations. The two provenance fields retain enough construction/mutation
/// history to reconstruct CPython's slot table only when frozenset equality
/// can make its probe order observable through user `__eq__`.
#[derive(Clone, Debug)]
pub struct PySet {
    entries: IndexSet<PyKey, PyHasher>,
    python_hash_initial_table_size: usize,
    python_hash_mutation_journal: Option<Box<SetMutationJournal>>,
}

impl PartialEq for PySet {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl Eq for PySet {}

impl PySet {
    pub fn with_capacity_and_hasher(capacity: usize, hasher: PyHasher) -> Self {
        Self {
            entries: IndexSet::with_capacity_and_hasher(capacity, hasher),
            python_hash_initial_table_size: SET_PROBE_MINSIZE,
            python_hash_mutation_journal: None,
        }
    }

    fn with_python_table_size(capacity: usize, table_size: usize) -> Self {
        Self {
            entries: IndexSet::with_capacity_and_hasher(capacity, PyHasher::default()),
            python_hash_initial_table_size: table_size.max(SET_PROBE_MINSIZE),
            python_hash_mutation_journal: None,
        }
    }

    /// Destination layout used by CPython's exact-dict set constructor path.
    pub fn with_cpython_dict_capacity(capacity: usize) -> Self {
        let table_size = if capacity.saturating_mul(5) >= (SET_PROBE_MINSIZE - 1).saturating_mul(3)
        {
            PySetProbeSnapshot::table_size_for_minused(capacity.saturating_mul(2))
        } else {
            SET_PROBE_MINSIZE
        };
        Self::with_python_table_size(capacity, table_size)
    }

    /// Copy an exact set/frozenset through CPython 3.12's `set_merge` rules.
    /// A same-sized dummy-free source copies its slots exactly; otherwise the
    /// destination reinserts active source slots into one presized table.
    pub fn cpython_merged_copy(source: &Self) -> Self {
        if source.is_empty() {
            return Self::default();
        }
        let table_size =
            if source.len().saturating_mul(5) >= (SET_PROBE_MINSIZE - 1).saturating_mul(3) {
                PySetProbeSnapshot::table_size_for_minused(source.len().saturating_mul(2))
            } else {
                SET_PROBE_MINSIZE
            };
        if let Some(journal) = source.python_hash_mutation_journal.as_deref()
            && journal.has_only_tail_removals(source.len())
        {
            let source_table = journal
                .materialize_tail_removals(&source.entries, source.python_hash_initial_table_size);
            let mut copied = Self::with_python_table_size(source.len(), table_size);
            for key in source_table.active_keys(source) {
                copied.insert(key.clone());
            }
            return copied;
        }

        let source_table = source.python_hash_snapshot();
        if table_size == source_table.table_size() && !source_table.has_dummies() {
            return source.clone();
        }

        let mut copied = Self::with_python_table_size(source.len(), table_size);
        for key in source_table.active_keys(source) {
            copied.insert(key.clone());
        }
        copied
    }

    pub fn python_hash_snapshot(&self) -> PySetProbeSnapshot {
        if let Some(journal) = self.python_hash_mutation_journal.as_deref() {
            return journal.materialize(&self.entries, self.python_hash_initial_table_size);
        }
        let mut table = PySetProbeSnapshot::with_table_size(self.python_hash_initial_table_size);
        for (replay_id, key) in self.entries.iter().enumerate() {
            table.insert(SetProbeEntry {
                python_hash: py_hash_pykey(key) as u64,
                replay_id: replay_id as u64,
            });
        }
        table
    }

    #[inline]
    pub fn insert_full(&mut self, key: PyKey) -> (usize, bool) {
        let (index, inserted) = self.entries.insert_full(key);
        if inserted && let Some(journal) = self.python_hash_mutation_journal.as_mut() {
            journal.record_inserted();
        }
        (index, inserted)
    }

    #[inline]
    pub fn insert(&mut self, key: PyKey) -> bool {
        self.insert_full(key).1
    }

    pub fn extend(&mut self, keys: impl IntoIterator<Item = PyKey>) {
        let keys = keys.into_iter();
        self.entries.reserve(keys.size_hint().0);
        for key in keys {
            self.insert(key);
        }
    }

    #[inline]
    fn prepare_removal_history(&mut self) {
        if self.python_hash_mutation_journal.is_none() {
            self.python_hash_mutation_journal = Some(Box::default());
        }
    }

    /// Record CPython's one-shot pre-resize before merging an exact set or
    /// exact dict. The actual decision is replayed against the then-current
    /// fill/mask, keeping this hot mutation path O(1).
    pub fn prepare_cpython_merge(&mut self, additional_used: usize) {
        if additional_used == 0 {
            return;
        }
        if self.python_hash_mutation_journal.is_none() {
            self.python_hash_mutation_journal = Some(Box::default());
        }
        self.python_hash_mutation_journal
            .as_mut()
            .expect("set merge must retain probe history")
            .record_merge(additional_used);
    }

    /// Record CPython's post-`difference_update` dummy compaction decision.
    /// It runs once per operand, after all removals for that operand.
    pub fn finish_cpython_difference_update(&mut self) {
        if self.python_hash_mutation_journal.is_none() {
            self.python_hash_mutation_journal = Some(Box::default());
        }
        self.python_hash_mutation_journal
            .as_mut()
            .expect("set difference must retain probe history")
            .record_difference_cleanup();
    }

    pub fn shift_remove<Q>(&mut self, key: &Q) -> bool
    where
        Q: ?Sized + Hash + indexmap::Equivalent<PyKey>,
    {
        let Some(index) = self.entries.get_index_of(key) else {
            return false;
        };
        self.shift_remove_index(index).is_some()
    }

    pub fn shift_remove_index(&mut self, index: usize) -> Option<PyKey> {
        self.entries.get_index(index)?;
        self.prepare_removal_history();
        let removed = self.entries.shift_remove_index(index)?;
        self.python_hash_mutation_journal
            .as_mut()
            .expect("set removal must retain probe history")
            .record_removed(index, &removed);
        Some(removed)
    }

    pub fn pop(&mut self) -> Option<PyKey> {
        let index = self.entries.len().checked_sub(1)?;
        self.shift_remove_index(index)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&PyKey) -> bool) {
        let removals: Vec<(usize, u64)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, key)| {
                if keep(key) {
                    None
                } else {
                    Some((index, py_hash_pykey(key) as u64))
                }
            })
            .collect();
        if removals.is_empty() {
            return;
        }

        // Preserve the existing descending-removal replay order, but compact
        // the live IndexSet in one linear pass. Repeated shift_remove_index()
        // shifts the tail once per removed key and turns bulk set difference
        // into quadratic work.
        self.prepare_removal_history();
        let journal = self
            .python_hash_mutation_journal
            .as_mut()
            .expect("set retain must retain probe history");
        for &(position, python_hash) in removals.iter().rev() {
            journal.record_removed_hash(position, python_hash);
        }

        let mut removals = removals.into_iter().peekable();
        let mut index = 0usize;
        self.entries.retain(|_| {
            let remove = removals
                .peek()
                .is_some_and(|(position, _)| *position == index);
            if remove {
                removals.next();
            }
            index += 1;
            !remove
        });
        debug_assert!(removals.next().is_none());
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.python_hash_initial_table_size = SET_PROBE_MINSIZE;
        self.python_hash_mutation_journal = None;
    }

    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.entries.reserve(additional);
    }

    #[inline]
    pub fn shrink_to_fit(&mut self) {
        self.entries.shrink_to_fit();
    }
}

impl Default for PySet {
    fn default() -> Self {
        Self::with_capacity_and_hasher(0, PyHasher::default())
    }
}

impl std::ops::Deref for PySet {
    type Target = IndexSet<PyKey, PyHasher>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl Extend<PyKey> for PySet {
    fn extend<T: IntoIterator<Item = PyKey>>(&mut self, iter: T) {
        PySet::extend(self, iter);
    }
}

impl FromIterator<PyKey> for PySet {
    fn from_iter<T: IntoIterator<Item = PyKey>>(iter: T) -> Self {
        let mut set = Self::default();
        set.extend(iter);
        set
    }
}

impl IntoIterator for PySet {
    type Item = PyKey;
    type IntoIter = indexmap::set::IntoIter<PyKey>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a PySet {
    type Item = &'a PyKey;
    type IntoIter = indexmap::set::Iter<'a, PyKey>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
