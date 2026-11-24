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
