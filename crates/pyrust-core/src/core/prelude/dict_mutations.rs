impl PyDict {
    /// Perform CPython's unicode-only-to-general conversion before lookup of
    /// a non-exact-string insertion key. This must run before user equality:
    /// the conversion also happens when lookup finds an equal existing string.
    #[inline]
    pub fn prepare_python_insert(&mut self, key: &PyKey) {
        self.prepare_python_insert_is_str(matches!(key, PyKey::Str(_)));
    }

    /// Borrowed counterpart of [`Self::prepare_python_insert`] for callers
    /// probing with [`ValueKeyRef`] / [`StrKey`] before allocating a `PyKey`.
    #[inline]
    pub fn prepare_python_insert_is_str(&mut self, is_str: bool) {
        if self.metadata_is_precise() {
            match self.python_key_table_kind {
                DictKeyTableKind::General => return,
                DictKeyTableKind::Unicode if is_str => return,
                _ => {}
            }
        }
        self.prepare_python_insert_slow(is_str);
    }

    #[cold]
    #[inline(never)]
    fn prepare_python_insert_slow(&mut self, is_str: bool) {
        self.repair_poisoned_metadata();
        match self.python_key_table_kind {
            DictKeyTableKind::General => return,
            DictKeyTableKind::Unknown => {
                debug_assert!(self.entries.is_empty());
                self.python_key_table_kind = if is_str {
                    DictKeyTableKind::Unicode
                } else {
                    DictKeyTableKind::General
                };
                self.python_hash_initial_table_kind = self.python_key_table_kind;
                return;
            }
            DictKeyTableKind::Unicode if is_str => return,
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

    /// Borrow an existing entry's value mutably without poisoning key metadata
    /// or bumping `structural_version` (keys are immutable through this borrow).
    #[inline]
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut Value>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<PyKey>,
    {
        self.entries.get_mut(key)
    }

    #[inline]
    fn record_inserted_index(&mut self, entry_index: usize, is_object: bool, is_dynamic: bool) {
        self.bump_structural_version();
        if !is_object
            && !is_dynamic
            && self.python_hash_index.is_none()
            && self.python_hash_mutation_journal.is_none()
            && self.live_object_key_count == 0
        {
            return;
        }
        self.record_inserted_index_slow(entry_index, is_object, is_dynamic);
    }

    #[cold]
    #[inline(never)]
    fn record_inserted_index_slow(
        &mut self,
        entry_index: usize,
        is_object: bool,
        is_dynamic: bool,
    ) {
        if is_object {
            self.live_object_key_count = self.live_object_key_count.saturating_add(1);
        }
        if is_dynamic {
            self.live_dynamic_key_count = self.live_dynamic_key_count.saturating_add(1);
        }
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

    /// Insert a primitive (non-`Object`, non-nested-object) key whose table
    /// kind has already been prepared by [`Self::prepare_python_insert`]
    /// (#2924). Avoids redundant `prepare_python_insert` and `pykey_contains_object`
    /// checks on the hot `d[k] = v` path.
    #[inline]
    pub fn insert_primitive_prepared(&mut self, key: PyKey, value: Value) -> Option<Value> {
        debug_assert!(!matches!(key, PyKey::Object { .. }) && !pykey_contains_object(&key));
        let (entry_index, replaced) = self.entries.insert_full(key, value);
        if replaced.is_none() {
            self.record_inserted_index(entry_index, false, false);
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
