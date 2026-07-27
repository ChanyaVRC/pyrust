impl Value {
    /// Borrow the list's elements while holding the `RefCell` read guard.
    ///
    /// The guard prevents an aliased `Value` from mutating the same backing
    /// storage while the returned view is live.  Callers that may invoke
    /// Python code should copy the required values and drop this guard first.
    #[inline(always)]
    pub fn as_list(&self) -> Option<std::cell::Ref<'_, Vec<Value>>> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_list() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if self.is_list() {
            let inner = unsafe { self.list_inner() };
            Some(inner.items.borrow())
        } else {
            None
        }
    }

    // `as_list_mut` was removed in #448 — use the scoped operation
    // methods (`Value::list_with_mut`, `list_push`, `list_extend`, …)
    // instead.  The previous `unsafe { &mut *cell.as_ptr() }` pattern
    // exposed an unguarded `&mut Vec<Value>` across crate boundaries
    // and forced callers to manually re-derive the aliasing-safety
    // property that `RefCell` already enforces internally.

    /// Borrow the tuple's elements as a slice.  Backs both the heap path
    /// (`TAG_TUPLE`, an `Rc<TupleInner>` since #2268) and the inline small-tuple
    /// path (`Opaque::SmallTuple2/3`); see #281.
    pub fn as_tuple(&self) -> Option<&[Value]> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_tuple() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if top16(self.0) == TAG_TUPLE {
            return Some(&unsafe { self.tuple_inner() }.items);
        }
        if top16(self.0) == TAG_OPAQUE {
            match unsafe { &*self.opaque_ptr() } {
                Opaque::SmallTuple2 { items, .. } => return Some(&items[..]),
                Opaque::SmallTuple3 { items, .. } => return Some(&items[..]),
                _ => {}
            }
        }
        None
    }

    pub fn as_opaque(&self) -> Option<&Opaque> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_opaque() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if top16(self.0) == TAG_OPAQUE {
            Some(unsafe { &*self.opaque_ptr() })
        } else {
            None
        }
    }

    /// Zero-cost borrow of the inner `Rc<RefCell<PyInstance>>` without
    /// an `Rc::clone`.  Returns `None` for any non-PyInstance value.
    ///
    /// Used by the GetAttr / CallMethod inline cache to read the class
    /// pointer and version without paying the clone cost on the fast path.
    pub fn as_py_instance_rc(&self) -> Option<&Rc<RefCell<PyInstance>>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::PyInstance(rc) = o {
                Some(rc)
            } else {
                None
            }
        })
    }

    // `as_opaque_mut` removed in #448 — the only callers were the
    // `as_dict_mut` / `as_set_mut` accessors that have themselves
    // been retired.

    /// Borrow the dict while holding the `RefCell` read guard.
    #[inline(always)]
    pub fn as_dict(&self) -> Option<std::cell::Ref<'_, PyDict>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(rc.borrow())
            } else {
                None
            }
        })
    }

    // `as_dict_mut` removed in #448 — use `Value::dict_with_mut`,
    // `dict_insert`, `dict_shift_remove`, `dict_clear`, `dict_extend`
    // instead.

    /// Borrow the set while holding the `RefCell` read guard.
    #[inline(always)]
    pub fn as_set(&self) -> Option<std::cell::Ref<'_, PySet>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Set(rc) = o {
                Some(rc.items.borrow())
            } else {
                None
            }
        })
    }

    // `as_set_mut` removed in #448 — use `Value::set_with_mut`,
    // `set_add`, `set_discard`, `set_clear`, `set_extend` instead.

    pub fn get_dict_rc(&self) -> Option<&Rc<RefCell<PyDict>>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(rc)
            } else {
                None
            }
        })
    }

    /// Register a live iterator against this dict's backing storage.
    pub fn dict_iteration_mutation_state(&self) -> Option<CollectionMutationState> {
        self.dict_rc().map(dict_iteration_mutation_state)
    }

    /// Register a live iterator against this set's backing storage.
    pub fn set_iteration_mutation_state(&self) -> Option<CollectionMutationState> {
        self.set_rc().map(|set| {
            collection_mutation_state(
                set_container_key(set),
                MutableContainerTarget::Set(Rc::downgrade(set)),
            )
        })
    }

    // ── Scoped-borrow operation API (#448) ────────────────────────────
    //
    // The methods below are the safe replacements for `as_list_mut` /
    // `as_dict_mut` / `as_set_mut`.  They take `&self`, scope their
    // `RefCell::borrow_mut()` to the operation's lifetime, and never
    // hand a `&mut <storage>` out across the function boundary.
    //
    // What this DOES guarantee:
    // - No mutable reference to the underlying `Vec` / `IndexMap` /
    //   `IndexSet` crosses an API boundary.  Every mutating operation
    //   is bounded by a single `borrow_mut()` window.
    // - The dispatcher's previous `unalias_args_for_mutation` dance
    //   is no longer needed for self-aliased iterating calls
    //   (`a.extend(a)` etc.) because the iterable is snapshotted
    //   before the receiver's `borrow_mut()` opens.
    //
    // Each method panics with the standard `RefCell::borrow_mut`
    // already-borrowed message if a re-entrant call (e.g. user
    // `__hash__` that mutates the same container while another
    // borrow is live) violates the `RefCell` rules.  That panic
    // surfaces UB-adjacent behaviour at the earliest possible point.

    /// Borrow the list's `Rc<ListInner>`.  Returns `None` when `self`
    /// is not a list.  Internal helper for the operation methods
    /// below.
    #[inline(always)]
    fn list_inner_rc(&self) -> Option<&ListInner> {
        if self.is_list() {
            Some(unsafe { self.list_inner() })
        } else {
            None
        }
    }

    /// Borrow the dict's `Rc<RefCell<IndexMap<...>>>`.
    #[inline(always)]
    fn dict_rc(&self) -> Option<&Rc<RefCell<PyDict>>> {
        self.get_dict_rc()
    }

    /// Borrow the set's shared backing.
    #[inline(always)]
    fn set_rc(&self) -> Option<&Rc<SetInner>> {
        self.as_opaque().and_then(|o| match o {
            Opaque::Set(rc) => Some(rc),
            _ => None,
        })
    }

    #[inline(always)]
    fn set_inner_rc(&self) -> Option<&SetInner> {
        self.set_rc().map(Rc::as_ref)
    }

    /// Scoped read access to the list's elements.  The closure runs
    /// while the immutable `RefCell` borrow is live; the borrow is
    /// dropped before this method returns.  Returns `None` when
    /// `self` is not a list.
    #[inline(always)]
    pub fn list_with<R>(&self, f: impl FnOnce(&Vec<Value>) -> R) -> Option<R> {
        let inner = self.list_inner_rc()?;
        Some(f(&inner.items.borrow()))
    }

    /// Scoped mutable access.  See [`Self::list_with`].  Inner
    /// closures MUST NOT call back into the same list (e.g. by
    /// recursing through user `__eq__`) — a re-entrant access will
    /// panic with `RefCell` already-borrowed.
    pub fn list_with_mut<R>(&self, f: impl FnOnce(&mut Vec<Value>) -> R) -> Option<R> {
        let inner = self.list_inner_rc()?;
        Some(f(&mut inner.items.borrow_mut()))
    }

    /// `list.append(item)`.  Returns `Err` (TypeError) when `self`
    /// is not a list.
    pub fn list_push(&self, item: Value) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "list_push receiver is not a list".to_string())
        })?;
        inner.items.borrow_mut().push(item);
        Ok(())
    }

    /// `list.extend(snapshot)` — caller passes an owned Vec already
    /// materialised from the iterable, so no aliasing window exists
    /// between the read and the write.
    pub fn list_extend(&self, snapshot: Vec<Value>) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "list_extend receiver is not a list".to_string(),
            )
        })?;
        inner.items.borrow_mut().extend(snapshot);
        Ok(())
    }

    /// `list.clear()`.
    pub fn list_clear(&self) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "list_clear receiver is not a list".to_string())
        })?;
        inner.items.borrow_mut().clear();
        Ok(())
    }

    /// `list.reverse()`.
    pub fn list_reverse(&self) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "list_reverse receiver is not a list".to_string(),
            )
        })?;
        inner.items.borrow_mut().reverse();
        Ok(())
    }

    /// `list.insert(idx, item)` — `idx` is the already-normalised
    /// position (caller does CPython-style negative-index folding).
    pub fn list_insert(&self, idx: usize, item: Value) -> Result<()> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named(
                "TypeError",
                "list_insert receiver is not a list".to_string(),
            )
        })?;
        let mut items = inner.items.borrow_mut();
        let pos = idx.min(items.len());
        items.insert(pos, item);
        Ok(())
    }

    /// `list.pop(idx)` — removes and returns the element at `idx`
    /// (already normalised; caller raises IndexError on out-of-range).
    pub fn list_pop_at(&self, idx: usize) -> Result<Value> {
        let inner = self.list_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "list_pop receiver is not a list".to_string())
        })?;
        let mut items = inner.items.borrow_mut();
        if idx >= items.len() {
            return Err(PyError::named(
                "IndexError",
                "pop index out of range".to_string(),
            ));
        }
        Ok(items.remove(idx))
    }

    /// Length of the list.
    #[inline(always)]
    pub fn list_len(&self) -> Option<usize> {
        self.list_inner_rc().map(|i| i.items.borrow().len())
    }

    /// Length of the set.
    #[inline(always)]
    pub fn set_len(&self) -> Option<usize> {
        self.set_inner_rc().map(|i| i.items.borrow().len())
    }

    /// Length of the dict.
    #[inline(always)]
    pub fn dict_len(&self) -> Option<usize> {
        self.dict_rc().map(|rc| rc.borrow().len())
    }

    /// Scoped read access to the set.
    #[inline(always)]
    pub fn set_with<R>(&self, f: impl FnOnce(&PySet) -> R) -> Option<R> {
        let inner = self.set_inner_rc()?;
        Some(f(&inner.items.borrow()))
    }

    /// Scoped mutable access to the set.
    pub fn set_with_mut<R>(&self, f: impl FnOnce(&mut PySet) -> R) -> Option<R> {
        let inner = self.set_inner_rc()?;
        let result = f(&mut inner.items.borrow_mut());
        bump_collection_mutation_state(set_container_key(inner));
        Some(result)
    }

    /// `set.add(key)` — returns true if the key was newly inserted,
    /// false if it was already present.
    pub fn set_add(&self, key: PyKey) -> Result<bool> {
        let inner = self.set_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "set_add receiver is not a set".to_string())
        })?;
        let inserted = inner.items.borrow_mut().insert(key);
        if inserted {
            bump_collection_mutation_state(set_container_key(inner));
        }
        Ok(inserted)
    }

    /// `set.discard(key)` — removes if present, no error if missing.
    pub fn set_discard(&self, key: &PyKey) -> Result<bool> {
        let inner = self.set_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "set_discard receiver is not a set".to_string())
        })?;
        let removed = inner.items.borrow_mut().shift_remove(key);
        if removed {
            bump_collection_mutation_state(set_container_key(inner));
        }
        Ok(removed)
    }

    /// `set.update(snapshot)`.
    pub fn set_extend(&self, snapshot: Vec<PyKey>) -> Result<()> {
        let inner = self.set_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "set_extend receiver is not a set".to_string())
        })?;
        let changed = {
            let mut items = inner.items.borrow_mut();
            let old_len = items.len();
            items.extend(snapshot);
            items.len() != old_len
        };
        if changed {
            bump_collection_mutation_state(set_container_key(inner));
        }
        Ok(())
    }

    /// `set.clear()`.
    pub fn set_clear(&self) -> Result<()> {
        let inner = self.set_inner_rc().ok_or_else(|| {
            PyError::named("TypeError", "set_clear receiver is not a set".to_string())
        })?;
        let changed = {
            let mut items = inner.items.borrow_mut();
            if items.is_empty() {
                false
            } else {
                items.clear();
                true
            }
        };
        if changed {
            bump_collection_mutation_state(set_container_key(inner));
        }
        Ok(())
    }

    /// Scoped read access to the dict.
    #[inline(always)]
    pub fn dict_with<R>(&self, f: impl FnOnce(&PyDict) -> R) -> Option<R> {
        let rc = self.dict_rc()?;
        Some(f(&rc.borrow()))
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
