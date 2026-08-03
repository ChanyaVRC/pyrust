/// Build-hasher used by the `dict` and `set` backing stores.
///
/// An interpreter's internal dicts carry no DoS-resistance requirement (CPython
/// itself uses a fast, non-cryptographic hash), so the stdlib's SipHash default
/// is pure overhead on every insert/lookup/set-op.  `FxBuildHasher`
/// (`rustc_hash`) replaces it with a fast multiply-xor hash.  This only changes
/// bucket placement — `IndexMap`/`IndexSet` keep their insertion-ordered Vec, so
/// dict/set iteration order is unaffected.  `PyKey`'s `Hash` impl is unchanged.
pub type PyHasher = FxBuildHasher;

const POISONED_OBJECT_KEY_COUNT: usize = usize::MAX;

/// Insertion-ordered backing store for a Python `dict`.
///
/// `PyKey::Object` hashes its precomputed Python hash directly, while primitive
/// variants use type-aware encodings optimized for homogeneous lookups. The
/// exact live Object count identifies homogeneous dictionaries in O(1), so
/// issue-#2060's object-only bucket lookup and ordinary primitive lookups never
/// touch the boxed mixed-key side index. The non-Object count is derived from
/// `entries.len() - live_object_key_count`, avoiding another word in every
/// dict. Unknown `DerefMut<IndexMap>` access poisons the count conservatively;
/// all wrapper-controlled mutation keeps it exact.
#[derive(Debug)]
pub struct PyDict {
    entries: IndexMap<PyKey, Value, PyHasher>,
    live_object_key_count: usize,
    /// Top-level Objects plus tuple/frozenset keys that recursively contain
    /// one. Those keys may require Python-level equality dispatch.
    live_dynamic_key_count: usize,
    structural_version: u64,
    python_hash_index: Option<Box<DictPythonHashIndex>>,
    /// Compact O(1)-per-mutation history for homogeneous dictionaries that
    /// have deleted keys but have not needed a Python-hash probe yet.
    python_hash_mutation_journal: Option<Box<DictMutationJournal>>,
    /// Initial CPython table size used to replay a still-homogeneous history
    /// lazily when the first mixed key or controlled deletion appears.
    python_hash_initial_table_size: usize,
    /// Kind of that initial table, before any unicode→general conversion.
    python_hash_initial_table_kind: DictKeyTableKind,
    /// Key-table kind survives deletion of the last non-string key. Presized
    /// empty dictionaries choose Unicode/General from their first key.
    python_key_table_kind: DictKeyTableKind,
}

impl PartialEq for PyDict {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl Clone for PyDict {
    fn clone(&self) -> Self {
        let entries = self.entries.clone();
        if entries.is_empty() {
            // PyDict_Copy returns a fresh shared-empty unicode table even when
            // the source became empty through deletions and retained dummies.
            return Self {
                entries,
                live_object_key_count: 0,
                live_dynamic_key_count: 0,
                structural_version: self.structural_version,
                python_hash_index: None,
                python_hash_mutation_journal: None,
                python_hash_initial_table_size: 0,
                python_hash_initial_table_kind: DictKeyTableKind::Unicode,
                python_key_table_kind: DictKeyTableKind::Unicode,
            };
        }
        let metadata_was_precise = self.live_object_key_count != POISONED_OBJECT_KEY_COUNT;
        let live_object_key_count = if metadata_was_precise {
            self.live_object_key_count
        } else {
            entries
                .keys()
                .filter(|key| matches!(key, PyKey::Object { .. }))
                .count()
        };
        let live_dynamic_key_count = if metadata_was_precise {
            self.live_dynamic_key_count
        } else {
            entries
                .keys()
                .filter(|key| pykey_contains_object(key))
                .count()
        };
        let shadow_required = live_dynamic_key_count > live_object_key_count
            || (live_object_key_count != 0 && live_object_key_count < entries.len());
        let mut copied_mutation_journal = None;
        let mut compacted_initial_table_size = None;
        let source_python_hash_index = if metadata_was_precise {
            if let Some(index) = self.python_hash_index.as_deref() {
                Some(index.clone())
            } else if let Some(journal) = self.python_hash_mutation_journal.as_ref() {
                if journal.copy_definitely_preserves_layout(entries.len()) {
                    copied_mutation_journal = Some((**journal).clone());
                    None
                } else if journal.copy_definitely_compacts_layout(entries.len()) {
                    compacted_initial_table_size =
                        Some(DictPythonHashIndex::sparse_copy_table_size(entries.len()));
                    None
                } else {
                    Some(journal.materialize(
                        &self.entries,
                        self.python_hash_initial_table_size,
                        self.python_hash_initial_table_kind.is_unicode(),
                        self.python_key_table_kind.is_unicode(),
                    ))
                }
            } else {
                shadow_required.then(|| {
                    DictPythonHashIndex::from_entries(
                        &self.entries,
                        self.python_hash_initial_table_size,
                        self.python_hash_initial_table_kind.is_unicode(),
                        self.python_key_table_kind.is_unicode(),
                    )
                })
            }
        } else if shadow_required {
            Some(DictPythonHashIndex::from_entries(&entries, 0, false, false))
        } else {
            None
        };
        let python_hash_index = source_python_hash_index.map(|index| {
            Box::new(if index.copy_preserves_layout() {
                index
            } else {
                // CPython's sparse-copy path reinserts live entries and
                // therefore discards all dummies/probe history.
                index.compacted_copy()
            })
        });
        let python_key_table_kind = if metadata_was_precise {
            python_hash_index
                .as_ref()
                .map_or(self.python_key_table_kind, |index| {
                    if index.unicode_only {
                        DictKeyTableKind::Unicode
                    } else {
                        DictKeyTableKind::General
                    }
                })
        } else {
            DictKeyTableKind::General
        };
        let python_hash_initial_table_size = compacted_initial_table_size.unwrap_or({
            if metadata_was_precise {
                self.python_hash_initial_table_size
            } else {
                0
            }
        });
        let python_hash_initial_table_kind = if compacted_initial_table_size.is_some() {
            python_key_table_kind
        } else if metadata_was_precise {
            self.python_hash_initial_table_kind
        } else {
            DictKeyTableKind::General
        };
        Self {
            entries,
            live_object_key_count,
            live_dynamic_key_count,
            structural_version: self.structural_version,
            python_hash_index,
            python_hash_mutation_journal: copied_mutation_journal.map(Box::new),
            python_hash_initial_table_size,
            python_hash_initial_table_kind,
            python_key_table_kind,
        }
    }
}

impl PyDict {
    pub fn with_capacity_and_hasher(capacity: usize, hasher: PyHasher) -> Self {
        let python_hash_initial_table_size = DictPythonHashIndex::table_size_for_capacity(capacity);
        let initial_kind = if capacity == 0 {
            DictKeyTableKind::Unicode
        } else {
            DictKeyTableKind::Unknown
        };
        Self {
            entries: IndexMap::with_capacity_and_hasher(capacity, hasher),
            live_object_key_count: 0,
            live_dynamic_key_count: 0,
            structural_version: 0,
            python_hash_index: None,
            python_hash_mutation_journal: None,
            python_hash_initial_table_size,
            python_hash_initial_table_kind: initial_kind,
            python_key_table_kind: initial_kind,
        }
    }

    /// Construct a presized dict when the caller has already inspected every
    /// initial key, as CPython's `_PyDict_FromItems` does for `BUILD_MAP`.
    /// Knowing the kind up front matters for duplicate-heavy literals: a mixed
    /// literal starts General at its presized size rather than transiently
    /// starting Unicode and shrinking during conversion.
    pub fn with_capacity_and_known_key_kind(
        capacity: usize,
        hasher: PyHasher,
        unicode_only: bool,
    ) -> Self {
        let mut dict = Self::with_capacity_and_hasher(capacity, hasher);
        // CPython's presizing helper returns the shared empty table through
        // five requested entries, ignoring the pre-scanned kind. In that
        // range the first insertion still selects Unicode/General and a later
        // non-string may perform the usual Unicode→General conversion.
        if capacity <= dict_probe_usable_fraction(8) {
            return dict;
        }
        let kind = if unicode_only {
            DictKeyTableKind::Unicode
        } else {
            DictKeyTableKind::General
        };
        dict.python_hash_initial_table_kind = kind;
        dict.python_key_table_kind = kind;
        dict
    }

    /// Construct the destination used by CPython's exact dict/set
    /// `_PyDict_FromKeys` shortcuts. Unlike general presizing, that path calls
    /// `dictresize(estimate_log2_keysize(n))` even for tiny inputs (so sizes
    /// 1..4 intentionally allocate a size-16 table while size 5 uses size 8).
    pub fn with_fromkeys_fast_path(capacity: usize, hasher: PyHasher, unicode_only: bool) -> Self {
        let mut dict = Self::with_capacity_and_hasher(capacity, hasher);
        let estimate = capacity.saturating_mul(3).saturating_add(1) / 2;
        dict.python_hash_initial_table_size = DictPythonHashIndex::table_size_for_target(estimate);
        let kind = if unicode_only {
            DictKeyTableKind::Unicode
        } else {
            DictKeyTableKind::General
        };
        dict.python_hash_initial_table_kind = kind;
        dict.python_key_table_kind = kind;
        dict
    }

    #[inline]
    pub fn may_have_non_object_key(&self) -> bool {
        self.live_object_key_count == POISONED_OBJECT_KEY_COUNT
            || self.entries.len() > self.live_object_key_count
    }

    #[inline]
    pub fn may_have_object_key(&self) -> bool {
        self.live_object_key_count != 0
    }

    #[inline]
    pub fn may_have_dynamic_key(&self) -> bool {
        self.live_dynamic_key_count != 0
    }

    #[inline]
    pub fn may_have_non_dynamic_key(&self) -> bool {
        self.live_dynamic_key_count == POISONED_OBJECT_KEY_COUNT
            || self.entries.len() > self.live_dynamic_key_count
    }

    /// Version of structural key-table changes only. Replacing an existing
    /// value leaves this unchanged so a reentrant slow lookup does not restart
    /// on value-only callbacks.
    #[inline]
    pub fn structural_version(&self) -> u64 {
        self.structural_version
    }

    /// Same-Python-hash candidates in CPython perturb-probe order. `None`
    /// means the shadow has not been activated yet; `Some([])` is a precise
    /// miss in an active table.
    #[inline]
    pub fn python_hash_candidates(&self, python_hash: u64) -> Option<Vec<PyKey>> {
        self.python_hash_index
            .as_ref()
            .map(|index| index.candidates(python_hash))
    }

    /// Whether the active CPython probe chain contains a top-level Object key
    /// with this Python hash. Scalar primitive probes only need ordered user
    /// equality when such a key can precede their exact native-map hit.
    #[inline]
    pub fn python_hash_has_object_candidate(&self, python_hash: u64) -> bool {
        self.python_hash_index
            .as_ref()
            .is_some_and(|index| index.has_object_candidate(python_hash))
    }

    /// Whether the active CPython probe chain contains any key whose equality
    /// can dispatch Python code, including nested tuple/frozenset Objects.
    #[inline]
    pub fn python_hash_has_dynamic_candidate(&self, python_hash: u64) -> bool {
        self.python_hash_index
            .as_ref()
            .is_some_and(|index| index.has_dynamic_candidate(python_hash))
    }

    /// Whether a complete CPython probe-table shadow is available.
    #[inline]
    pub fn has_python_hash_index(&self) -> bool {
        self.python_hash_index.is_some()
    }

    /// Key-table kind used by CPython's exact-dict `fromkeys` fast path.
    #[inline]
    pub fn python_key_table_is_unicode(&self) -> bool {
        self.python_key_table_kind.is_unicode()
    }

    #[inline]
    pub fn has_non_object_python_hash(&self, python_hash: u64) -> bool {
        self.python_hash_index
            .as_ref()
            .is_some_and(|index| index.has_non_object_key(python_hash))
    }

    #[inline]
    fn metadata_is_precise(&self) -> bool {
        self.live_object_key_count != POISONED_OBJECT_KEY_COUNT
            && self.live_dynamic_key_count != POISONED_OBJECT_KEY_COUNT
    }

    #[inline]
    fn requires_python_hash_index(&self) -> bool {
        self.metadata_is_precise()
            && (self.live_dynamic_key_count > self.live_object_key_count
                || (self.live_object_key_count != 0
                    && self.live_object_key_count < self.entries.len()))
    }

    fn repair_poisoned_metadata(&mut self) {
        if self.metadata_is_precise() {
            return;
        }
        self.live_object_key_count = self
            .entries
            .keys()
            .filter(|key| matches!(key, PyKey::Object { .. }))
            .count();
        self.live_dynamic_key_count = self
            .entries
            .keys()
            .filter(|key| pykey_contains_object(key))
            .count();
        // Unknown raw mutation loses probe-slot history. Rebuild only the
        // currently observable mixed state; homogeneous history cannot be
        // reconstructed after the fact.
        self.python_hash_mutation_journal = None;
        self.python_hash_index = self.requires_python_hash_index().then(|| {
            Box::new(DictPythonHashIndex::from_entries(
                &self.entries,
                0,
                self.python_hash_initial_table_kind.is_unicode(),
                self.python_key_table_kind.is_unicode(),
            ))
        });
    }

    #[inline]
    fn bump_structural_version(&mut self) {
        self.structural_version = self.structural_version.wrapping_add(1);
    }

    /// Lazily materialize the complete CPython key table before a probe that
    /// crosses PyKey representations. Subsequent misses are O(probe length)
    /// rather than repeated O(dict length) scans.
    pub fn ensure_python_hash_index(&mut self) {
        self.repair_poisoned_metadata();
        if self.python_hash_index.is_none() {
            let index = if let Some(journal) = self.python_hash_mutation_journal.take() {
                journal.materialize(
                    &self.entries,
                    self.python_hash_initial_table_size,
                    self.python_hash_initial_table_kind.is_unicode(),
                    self.python_key_table_kind.is_unicode(),
                )
            } else {
                DictPythonHashIndex::from_entries(
                    &self.entries,
                    self.python_hash_initial_table_size,
                    self.python_hash_initial_table_kind.is_unicode(),
                    self.python_key_table_kind.is_unicode(),
                )
            };
            self.python_hash_index = Some(Box::new(index));
        }
    }

    /// Perform CPython's unicode-only-to-general conversion before lookup of
    /// a non-exact-string insertion key. This must run before user equality:
    /// the conversion also happens when lookup finds an equal existing string.
    pub fn prepare_python_insert(&mut self, key: &PyKey) {
        self.repair_poisoned_metadata();
        match self.python_key_table_kind {
            DictKeyTableKind::General => return,
            DictKeyTableKind::Unknown => {
                debug_assert!(self.entries.is_empty());
                self.python_key_table_kind = if matches!(key, PyKey::Str(_)) {
                    DictKeyTableKind::Unicode
                } else {
                    DictKeyTableKind::General
                };
                self.python_hash_initial_table_kind = self.python_key_table_kind;
                return;
            }
            DictKeyTableKind::Unicode if matches!(key, PyKey::Str(_)) => return,
            DictKeyTableKind::Unicode => {}
        }
        self.python_key_table_kind = DictKeyTableKind::General;
        if self.entries.is_empty() {
            // The shared empty unicode table converts to the minimum general
            // table. A presized empty table begins Unknown instead.
            self.python_hash_initial_table_size = 8;
            return;
        }
        self.ensure_python_hash_index();
        self.python_hash_index
            .as_mut()
            .expect("just-installed dict probe shadow")
            .prepare_general_insert();
        // A reentrant outer lookup must discard any snapshot taken against the
        // old key table even though no logical key changed.
        self.bump_structural_version();
    }

    fn record_inserted_index(&mut self, entry_index: usize, is_object: bool, is_dynamic: bool) {
        if is_object {
            self.live_object_key_count = self.live_object_key_count.saturating_add(1);
        }
        if is_dynamic {
            self.live_dynamic_key_count = self.live_dynamic_key_count.saturating_add(1);
        }
        self.bump_structural_version();
        if let Some(index) = self.python_hash_index.as_mut() {
            let key = self
                .entries
                .get_index(entry_index)
                .expect("newly inserted dict entry must remain present")
                .0;
            index.record_inserted(key);
            return;
        }
        let rebased_table_size = self
            .python_hash_mutation_journal
            .as_mut()
            .and_then(|journal| journal.record_inserted(self.entries.len()));
        if let Some(table_size) = rebased_table_size {
            self.python_hash_initial_table_size = table_size;
            self.python_hash_initial_table_kind = self.python_key_table_kind;
        }
        if is_dynamic || self.live_object_key_count != 0 {
            // The overwhelmingly common primitive-only path cannot require a
            // shadow. Avoid the extra metadata predicates on every insertion.
            if !self.requires_python_hash_index() {
                return;
            }
            self.ensure_python_hash_index();
        }
    }

    fn prepare_removal_history(&mut self) {
        if self.python_hash_index.is_none() && self.python_hash_mutation_journal.is_none() {
            self.python_hash_mutation_journal =
                Some(Box::new(DictMutationJournal::from_current_entries(
                    self.python_hash_initial_table_size,
                    self.entries.len(),
                )));
        }
    }

    fn record_removed_key(&mut self, entry_index: usize, key: &PyKey, popped: bool) {
        if let Some(index) = self.python_hash_index.as_mut() {
            if popped {
                index.record_popped(key);
            } else {
                index.record_removed(key);
            }
        } else {
            let journal = self
                .python_hash_mutation_journal
                .as_mut()
                .expect("dict removal must retain probe history");
            journal.record_removed(entry_index, key, popped);
        }
        self.record_removed_metadata(key);
    }

    fn record_removed_metadata(&mut self, key: &PyKey) {
        if matches!(key, PyKey::Object { .. }) {
            self.live_object_key_count -= 1;
        }
        if pykey_contains_object(key) {
            self.live_dynamic_key_count -= 1;
        }
        self.bump_structural_version();
    }

    #[inline]
    pub fn insert(&mut self, key: PyKey, value: Value) -> Option<Value> {
        let is_object = matches!(key, PyKey::Object { .. });
        let is_dynamic = pykey_contains_object(&key);
        self.prepare_python_insert(&key);
        let (entry_index, replaced) = self.entries.insert_full(key, value);
        if replaced.is_none() {
            self.record_inserted_index(entry_index, is_object, is_dynamic);
        }
        replaced
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = (PyKey, Value)>) {
        let entries = entries.into_iter();
        self.entries.reserve(entries.size_hint().0);
        for (key, value) in entries {
            self.insert(key, value);
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        let changed = !self.entries.is_empty();
        self.entries.clear();
        self.live_object_key_count = 0;
        self.live_dynamic_key_count = 0;
        self.python_hash_index = None;
        self.python_hash_mutation_journal = None;
        self.python_hash_initial_table_size = 0;
        self.python_hash_initial_table_kind = DictKeyTableKind::Unicode;
        self.python_key_table_kind = DictKeyTableKind::Unicode;
        if changed {
            self.bump_structural_version();
        }
    }

    #[inline]
    pub fn shift_remove<Q>(&mut self, key: &Q) -> Option<Value>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<PyKey>,
    {
        self.repair_poisoned_metadata();
        let index = self.entries.get_index_of(key)?;
        self.prepare_removal_history();
        let (stored_key, value) = self.entries.shift_remove_index(index)?;
        self.record_removed_key(index, &stored_key, false);
        Some(value)
    }

    #[inline]
    pub fn shift_remove_index(&mut self, index: usize) -> Option<(PyKey, Value)> {
        self.repair_poisoned_metadata();
        self.entries.get_index(index)?;
        self.prepare_removal_history();
        let removed = self.entries.shift_remove_index(index)?;
        self.record_removed_key(index, &removed.0, false);
        Some(removed)
    }

    #[inline]
    pub fn swap_remove<Q>(&mut self, key: &Q) -> Option<Value>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<PyKey>,
    {
        self.repair_poisoned_metadata();
        let index = self.entries.get_index_of(key)?;
        // `swap_remove` also reorders the last IndexMap entry. Preserve the
        // independent CPython compact-entry order before that rare mutation.
        self.ensure_python_hash_index();
        let (stored_key, value) = self.entries.swap_remove_index(index)?;
        self.record_removed_key(index, &stored_key, false);
        Some(value)
    }

    /// Remove and return the last insertion-order entry with precise metadata.
    /// This shadows `IndexMap::pop` so `dict.popitem()` cannot poison the
    /// wrapper through `DerefMut`.
    #[inline]
    pub fn pop(&mut self) -> Option<(PyKey, Value)> {
        self.repair_poisoned_metadata();
        let index = self.entries.len().checked_sub(1)?;
        self.prepare_removal_history();
        let removed = self.entries.pop()?;
        self.record_removed_key(index, &removed.0, true);
        Some(removed)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&PyKey, &mut Value) -> bool) {
        self.repair_poisoned_metadata();
        let had_history =
            self.python_hash_index.is_some() || self.python_hash_mutation_journal.is_some();
        self.ensure_python_hash_index();
        let mut removed_keys = Vec::new();
        let mut removed_object_count = 0usize;
        let mut removed_dynamic_count = 0usize;
        self.entries.retain(|key, value| {
            let retain = keep(key, value);
            if !retain {
                removed_object_count += usize::from(matches!(key, PyKey::Object { .. }));
                removed_dynamic_count += usize::from(pykey_contains_object(key));
                removed_keys.push(key.clone());
            }
            retain
        });
        if removed_keys.is_empty() {
            if !had_history {
                self.python_hash_index = None;
            }
            return;
        }
        let index = self
            .python_hash_index
            .as_mut()
            .expect("retain installs a dict probe shadow before deleting");
        for key in &removed_keys {
            index.record_removed(key);
        }
        self.live_object_key_count -= removed_object_count;
        self.live_dynamic_key_count -= removed_dynamic_count;
        self.bump_structural_version();
    }

    #[inline]
    pub fn move_index(&mut self, from: usize, to: usize) {
        // OrderedDict keeps display order separately from the underlying dict
        // key table. Capture that table before IndexMap's presentation order
        // moves, otherwise a later lazy activation would replay the wrong
        // insertion history.
        self.ensure_python_hash_index();
        self.entries.move_index(from, to);
    }
}

impl Default for PyDict {
    fn default() -> Self {
        Self::with_capacity_and_hasher(0, PyHasher::default())
    }
}

impl std::ops::Deref for PyDict {
    type Target = IndexMap<PyKey, Value, PyHasher>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl std::ops::DerefMut for PyDict {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // The caller can perform an arbitrary IndexMap mutation which bypasses
        // all precise accounting. Invalidate before handing out the reference
        // so an in-progress equality callback observes a version change.
        self.live_object_key_count = POISONED_OBJECT_KEY_COUNT;
        self.live_dynamic_key_count = POISONED_OBJECT_KEY_COUNT;
        self.python_hash_index = None;
        self.python_hash_mutation_journal = None;
        self.python_hash_initial_table_size = 0;
        self.python_hash_initial_table_kind = DictKeyTableKind::General;
        self.python_key_table_kind = DictKeyTableKind::General;
        self.bump_structural_version();
        &mut self.entries
    }
}

impl Extend<(PyKey, Value)> for PyDict {
    fn extend<T: IntoIterator<Item = (PyKey, Value)>>(&mut self, iter: T) {
        PyDict::extend(self, iter);
    }
}

impl FromIterator<(PyKey, Value)> for PyDict {
    fn from_iter<T: IntoIterator<Item = (PyKey, Value)>>(iter: T) -> Self {
        let mut dict = Self::default();
        dict.extend(iter);
        dict
    }
}

impl IntoIterator for PyDict {
    type Item = (PyKey, Value);
    type IntoIter = indexmap::map::IntoIter<PyKey, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a PyDict {
    type Item = (&'a PyKey, &'a Value);
    type IntoIter = indexmap::map::Iter<'a, PyKey, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl<'a> IntoIterator for &'a mut PyDict {
    type Item = (&'a PyKey, &'a mut Value);
    type IntoIter = indexmap::map::IterMut<'a, PyKey, Value>;

    fn into_iter(self) -> Self::IntoIter {
        // Keys are immutable through this iterator; changing only values does
        // not invalidate key-kind metadata or the structural version.
        self.entries.iter_mut()
    }
}

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

/// Insertion-ordered backing store for a module's direct attributes
/// (`PyModule::attrs`).
///
/// `vars(module)` / `module.__dict__` are Python dicts, and CPython guarantees
/// dict order, so the storage backing them must be insertion ordered too
/// (issue #2918 — a `HashMap` here made `list(vars(math))` differ run to run).
/// For a built-in module the insertion order is the `pyrust_module!` body's
/// declaration order, which is fixed at compile time; for `builtins` it is the
/// declared order of the composed sub-modules. `PyClass::attrs` uses the same
/// shape for the same reason. Removal must use `shift_remove`, never
/// `swap_remove`, to keep the surviving entries in order — exactly like
/// `dict.__delitem__`.
///
/// Hashed with `PyHasher` for the same reason `PyDict` is: a module namespace is
/// interpreter-internal, so SipHash buys nothing here. The hasher only decides
/// bucket placement — the insertion-ordered entry vector, and therefore
/// iteration order, is unaffected.
pub type ModuleAttrs = IndexMap<String, Value, PyHasher>;

pub use num_bigint::BigInt as PyBigInt;
pub use num_bigint::Sign as PyBigIntSign;
pub use num_traits::Pow as PyPow;
pub use num_traits::ToPrimitive as PyToPrimitive;
pub use num_traits::Zero as PyZero;

static FN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_fn_id() -> u64 {
    FN_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}
