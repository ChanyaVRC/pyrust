impl Value {
    // ── Scoped-borrow list and set operation API (#448) ──────────────────────
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
}
