impl Value {
    /// Scoped read access to the dict.
    #[inline(always)]
    pub fn dict_with<R>(&self, f: impl FnOnce(&PyDict) -> R) -> Option<R> {
        let rc = self.dict_rc()?;
        Some(f(&rc.borrow()))
    }

    /// Install the dict's lazy Python-hash probe cache without reporting a
    /// Python-visible mutation. The cache changes neither keys nor values and
    /// therefore must not invalidate namespace or collection observers.
    pub fn dict_ensure_python_hash_index(&self) -> Option<()> {
        let rc = self.dict_rc()?;
        rc.borrow_mut().ensure_python_hash_index();
        Some(())
    }

    /// Apply insertion-only key-table preparation without reporting a logical
    /// dict mutation. The following insertion/replacement owns that observer
    /// notification.
    pub fn dict_prepare_python_insert(&self, key: &PyKey) -> Option<()> {
        let rc = self.dict_rc()?;
        rc.borrow_mut().prepare_python_insert(key);
        Some(())
    }

    /// Scoped mutable access to the dict.
    pub fn dict_with_mut<R>(&self, f: impl FnOnce(&mut PyDict) -> R) -> Option<R> {
        let rc = self.dict_rc()?;
        let Some(mutation_state) = active_collection_mutation_state(dict_container_key(rc)) else {
            let result = {
                let mut dict = rc.borrow_mut();
                f(&mut dict)
            };
            crate::environment::notify_namespace_dict_mutation(rc);
            return Some(result);
        };
        let mutation_state = Some(mutation_state);
        let before = begin_tracked_dict_mutation(&mutation_state, rc);
        let result = {
            let mut dict = rc.borrow_mut();
            f(&mut dict)
        };
        finish_tracked_dict_mutation(mutation_state, before, rc, false);
        crate::environment::notify_namespace_dict_mutation(rc);
        Some(result)
    }

    /// Move one exact dict entry to another insertion-order index.
    ///
    /// This is the storage primitive for operations such as
    /// `OrderedDict.move_to_end`: unlike remove + insert, it preserves the
    /// stored key and value objects.  A real move advances both the ordinary
    /// mutation version and the entry-order generation observed by ordered
    /// iterators.  It does not touch removal/reinsertion watches or compact
    /// dict layout bookkeeping because the entry set and length are unchanged.
    /// Moving an entry to its current index is a true no-op.
    pub fn dict_move_index(&self, from: usize, to: usize) -> Result<bool> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "dict_move_index receiver is not a dict".to_string(),
            )
        })?;
        let len = rc.borrow().len();
        if from >= len || to >= len {
            return Err(PyError::Runtime(format!(
                "internal: dict_move_index index out of bounds ({from} -> {to}, len {len})"
            )));
        }
        if from == to {
            return Ok(false);
        }

        let mutation_state = active_collection_mutation_state(dict_container_key(rc));
        rc.borrow_mut().move_index(from, to);
        if let Some(state) = mutation_state {
            state
                .entry_order
                .set(state.entry_order.get().wrapping_add(1));
            bump_active_collection_mutation_state(Some(state), None);
        }
        crate::environment::notify_namespace_dict_mutation(rc);
        Ok(true)
    }

    /// `dict[key] = value`.
    pub fn dict_insert(&self, key: PyKey, value: Value) -> Result<Option<Value>> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "dict_insert receiver is not a dict".to_string(),
            )
        })?;
        let namespace_key = if crate::environment::namespace_alias_tracking_active()
            && matches!(&key, PyKey::Str(_))
        {
            Some(key.clone())
        } else {
            None
        };
        let Some(mutation_state) = active_collection_mutation_state(dict_container_key(rc)) else {
            let replaced = rc.borrow_mut().insert(key, value);
            if let Some(PyKey::Str(key)) = namespace_key.as_ref()
                && let Some(name) = key.as_str()
            {
                crate::environment::notify_namespace_dict_key_mutation(rc, name);
            }
            return Ok(replaced);
        };
        let mutation_state = Some(mutation_state);
        let before = begin_tracked_dict_mutation(&mutation_state, rc);
        let replaced = rc.borrow_mut().insert(key, value);
        finish_tracked_dict_mutation(mutation_state, before, rc, false);
        if let Some(PyKey::Str(key)) = namespace_key.as_ref()
            && let Some(name) = key.as_str()
        {
            crate::environment::notify_namespace_dict_key_mutation(rc, name);
        }
        Ok(replaced)
    }

    /// Replace the value of an existing dict entry using a borrowed probe key
    /// (e.g. [`ValueKeyRef`] or [`StrKey`]) without allocating a `PyKey` (#454).
    ///
    /// Returns `Ok(Ok(old_value))` on a hit (having updated `PyDict` table
    /// kind, `CollectionMutationState` version, and namespace-alias mirrors
    /// identically to [`Self::dict_insert`]), or `Ok(Err(value))` on a miss so
    /// the caller can construct the owned `PyKey` and insert.
    pub fn dict_replace_existing<Q>(
        &self,
        key: &Q,
        key_is_str: bool,
        str_name: Option<&str>,
        value: Value,
    ) -> Result<std::result::Result<Value, Value>>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<PyKey>,
    {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "dict_replace_existing receiver is not a dict".to_string(),
            )
        })?;
        let replaced = {
            let mut dict = rc.borrow_mut();
            dict.prepare_python_insert_is_str(key_is_str);
            if let Some(slot) = dict.get_mut(key) {
                Some(std::mem::replace(slot, value))
            } else {
                return Ok(Err(value));
            }
        };
        let old = replaced.expect("just replaced existing dict value");
        if let Some(mutation_state) = active_collection_mutation_state(dict_container_key(rc)) {
            bump_active_collection_mutation_state(Some(mutation_state), None);
        }
        if crate::environment::namespace_alias_tracking_active()
            && let Some(name) = str_name
        {
            crate::environment::notify_namespace_dict_key_mutation(rc, name);
        }
        Ok(Ok(old))
    }

    /// `dict.shift_remove(key)`.
    pub fn dict_shift_remove(&self, key: &PyKey) -> Result<Option<Value>> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "dict_shift_remove receiver is not a dict".to_string(),
            )
        })?;
        let Some(mutation_state) = active_collection_mutation_state(dict_container_key(rc)) else {
            let removed = rc.borrow_mut().shift_remove(key);
            if removed.is_some()
                && let PyKey::Str(key) = key
                && let Some(name) = key.as_str()
            {
                crate::environment::notify_namespace_dict_key_mutation(rc, name);
            }
            return Ok(removed);
        };
        let mutation_state = Some(mutation_state);
        let before = begin_tracked_dict_mutation(&mutation_state, rc);
        let removed = rc.borrow_mut().shift_remove(key);
        finish_tracked_dict_mutation(mutation_state, before, rc, false);
        if removed.is_some()
            && let PyKey::Str(key) = key
            && let Some(name) = key.as_str()
        {
            crate::environment::notify_namespace_dict_key_mutation(rc, name);
        }
        Ok(removed)
    }

    /// Remove several exact stored keys in one stable, linear pass.
    ///
    /// The caller must supply keys obtained from this dictionary (or otherwise
    /// already resolve Python-level equality).  This core layer deliberately
    /// performs no user `__eq__` dispatch.  [`IndexMap::retain`] preserves the
    /// relative insertion order of every surviving entry.
    ///
    /// Active iterator bookkeeping observes this as one mutation transaction:
    /// the tracker receives the total removal count and every watched key that
    /// disappeared, so size-change and terminal-reinsertion diagnostics remain
    /// equivalent to repeated [`Self::dict_shift_remove`] calls without their
    /// repeated O(n) compaction.  An empty key list is a true no-op and does not
    /// advance the mutation version.
    pub fn dict_shift_remove_many(&self, keys: Vec<PyKey>) -> Result<usize> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "dict_shift_remove_many receiver is not a dict".to_string(),
            )
        })?;
        if keys.is_empty() {
            return Ok(0);
        }

        let keys: HashSet<PyKey, FxBuildHasher> = keys.into_iter().collect();
        let Some(mutation_state) = active_collection_mutation_state(dict_container_key(rc)) else {
            let removed = {
                let mut dict = rc.borrow_mut();
                let old_len = dict.len();
                dict.retain(|key, _| !keys.contains(key));
                old_len - dict.len()
            };
            if removed != 0 {
                crate::environment::notify_namespace_dict_mutation(rc);
            }
            return Ok(removed);
        };
        let mutation_state = Some(mutation_state);
        let before = begin_tracked_dict_mutation(&mutation_state, rc);
        let removed = {
            let mut dict = rc.borrow_mut();
            let old_len = dict.len();
            dict.retain(|key, _| !keys.contains(key));
            old_len - dict.len()
        };
        finish_tracked_dict_mutation(mutation_state, before, rc, false);
        if removed != 0 {
            crate::environment::notify_namespace_dict_mutation(rc);
        }
        Ok(removed)
    }

    /// `dict.clear()`.
    pub fn dict_clear(&self) -> Result<()> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named("TypeError", "dict_clear receiver is not a dict".to_string())
        })?;
        let Some(mutation_state) = active_collection_mutation_state(dict_container_key(rc)) else {
            let changed = {
                let mut dict = rc.borrow_mut();
                let changed = !dict.is_empty();
                dict.clear();
                changed
            };
            if changed {
                crate::environment::notify_namespace_dict_mutation(rc);
            }
            return Ok(());
        };
        let mutation_state = Some(mutation_state);
        let before = begin_tracked_dict_mutation(&mutation_state, rc);
        let changed = !rc.borrow().is_empty();
        rc.borrow_mut().clear();
        finish_tracked_dict_mutation(mutation_state, before, rc, true);
        if changed {
            crate::environment::notify_namespace_dict_mutation(rc);
        }
        Ok(())
    }

    /// `dict.update(snapshot)`.
    pub fn dict_extend(&self, snapshot: Vec<(PyKey, Value)>) -> Result<()> {
        let rc = self.dict_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "dict_extend receiver is not a dict".to_string(),
            )
        })?;
        let changed = !snapshot.is_empty();
        // A bulk update still has exact storage keys. Preserve that information
        // for namespace aliases instead of degrading to an opaque full-mirror
        // refresh. The thread-local gate keeps ordinary dictionaries on the
        // allocation-free path.
        let namespace_names = if crate::environment::namespace_alias_tracking_active() {
            snapshot
                .iter()
                .filter_map(|(key, _)| match key {
                    PyKey::Str(key) => key.as_str().map(str::to_owned),
                    _ => None,
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let Some(mutation_state) = active_collection_mutation_state(dict_container_key(rc)) else {
            rc.borrow_mut().extend(snapshot);
            if changed {
                for name in &namespace_names {
                    crate::environment::notify_namespace_dict_key_mutation(rc, name);
                }
            }
            return Ok(());
        };
        let mutation_state = Some(mutation_state);
        let before = begin_tracked_dict_mutation(&mutation_state, rc);
        rc.borrow_mut().extend(snapshot);
        finish_tracked_dict_mutation(mutation_state, before, rc, false);
        if changed {
            for name in &namespace_names {
                crate::environment::notify_namespace_dict_key_mutation(rc, name);
            }
        }
        Ok(())
    }
}
