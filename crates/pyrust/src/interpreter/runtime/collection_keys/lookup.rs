impl Interpreter {
    /// Look up a key in a dict where the key may be a `PyKey::Object`.
    ///
    /// IndexMap's `get` will find entries whose `PyKey` matches by
    /// pointer-identity (because `PyKey::Object`'s `PartialEq` defers to
    /// `Value::eq`, which uses `Rc::ptr_eq` for `PyInstance`).  When the
    /// fast path misses and the key is an `Object`, we linearly scan
    /// entries with the same precomputed hash and dispatch user `__eq__`
    /// for full Python semantics.  Returns `Ok(Some((index, value)))` on
    /// a hit (index returned so callers can implement `pop`/`del`).
    ///
    /// Takes the receiver `&Value` (rather than `&IndexMap`) so the dict
    /// borrow can be scoped tightly: the fast path borrows for `get_full`
    /// only, and the `__eq__`-dispatching slow path borrows only long
    /// enough to extract the same-hash candidate list before dropping the
    /// borrow and running user code.  This avoids the O(N) whole-dict
    /// snapshot that callers used to have to make for soundness.
    pub(crate) fn dict_lookup(
        &mut self,
        receiver: &Value,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        // Fast path — dict borrow scoped to this block.
        {
            let dict = receiver
                .as_dict()
                .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
            if let Some((idx, _, v)) = dict.get_full(key) {
                return Ok(Some((idx, v.clone())));
            }
        }
        // Slow path — `Object` keys (and cross-variant None/Object matching,
        // issue #906).  Probe only the lookup key's hash bucket (issue #2060),
        // collecting candidate keys under a narrow borrow, then drop the borrow
        // before user `__eq__` runs.
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                collect_object_bucket_keys_map(&dict, key, |k| match k {
                    PyKey::Object { hash, .. } => hash == target_hash,
                    // PyKey::None has Python-level hash py_hash_none().  When
                    // the Object key hashes to the same value, check whether
                    // __eq__ considers them equal (issue #906).
                    PyKey::None => *target_hash == none_hash,
                    _ => false,
                })
            };
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&cand_val, target)? {
                    return self.dict_entry_by_key(receiver, &cand);
                }
            }
        }
        // Cross-variant slow path: lookup key is PyKey::None but a stored
        // PyKey::Object with hash py_hash_none() may __eq__-match None (issue #906).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                collect_object_bucket_keys_map(
                    &dict,
                    key,
                    |k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash),
                )
            };
            let none_val = Value::none();
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&none_val, &cand_val)? {
                    return self.dict_entry_by_key(receiver, &cand);
                }
            }
        }
        // Nested-object slow path (issue #2059): a Tuple/FrozenSet key that
        // nests a user object compares its nested element by `__eq__`, not the
        // raw `PyKey` identity used by `get_full`.  Probe the lookup key's hash
        // bucket for same-shape candidates and dispatch element-wise `__eq__`.
        if nested_object_tuple_key(key) {
            let candidate_keys = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                collect_object_bucket_keys_map(&dict, key, nested_object_tuple_key)
            };
            for cand in candidate_keys {
                if self.nested_object_keys_eq(&cand, key)? {
                    return self.dict_entry_by_key(receiver, &cand);
                }
            }
        }
        Ok(None)
    }

    /// Recover the `(index, value)` of an entry by its exact stored key.
    /// `key` must be a key cloned from the dict's own bucket (so `Object`
    /// matches by `Rc::ptr_eq` and `None` matches `None`), making this a
    /// single O(bucket) probe rather than a full scan.
    fn dict_entry_by_key(&self, receiver: &Value, key: &PyKey) -> Result<Option<(usize, Value)>> {
        let dict = receiver
            .as_dict()
            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        Ok(dict.get_full(key).map(|(idx, _, v)| (idx, v.clone())))
    }

    /// `dict_lookup` variant that takes the `IndexMap` directly.  Used by
    /// callers that already hold a `&IndexMap` (typically because they
    /// own/snapshotted the dict, so aliasing with mutable access is
    /// impossible).  Prefer [`Self::dict_lookup`] for new call sites — it
    /// scopes the dict borrow tightly without a whole-dict clone.
    pub(crate) fn dict_lookup_in(
        &mut self,
        dict: &PyDict,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        if let Some((idx, _, v)) = dict.get_full(key) {
            return Ok(Some((idx, v.clone())));
        }
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            // Probe only the lookup key's hash bucket (issue #2060).
            let candidate_keys = collect_object_bucket_keys_map(dict, key, |k| match k {
                PyKey::Object { hash, .. } => hash == target_hash,
                // Cross-variant: PyKey::None has Python-level hash py_hash_none();
                // include it as a candidate when the Object also hashes to that
                // value so that __eq__ can confirm the match (issue #906).
                PyKey::None => *target_hash == none_hash,
                _ => false,
            });
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&cand_val, target)? {
                    return Ok(dict.get_full(&cand).map(|(idx, _, v)| (idx, v.clone())));
                }
            }
        }
        // Cross-variant slow path: None key vs Object entries with hash py_hash_none()
        // (issue #906).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = collect_object_bucket_keys_map(
                dict,
                key,
                |k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash),
            );
            let none_val = Value::none();
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&none_val, &cand_val)? {
                    return Ok(dict.get_full(&cand).map(|(idx, _, v)| (idx, v.clone())));
                }
            }
        }
        // Nested-object slow path (issue #2059).
        if nested_object_tuple_key(key) {
            let candidate_keys = collect_object_bucket_keys_map(dict, key, nested_object_tuple_key);
            for cand in candidate_keys {
                if self.nested_object_keys_eq(&cand, key)? {
                    return Ok(dict.get_full(&cand).map(|(idx, _, v)| (idx, v.clone())));
                }
            }
        }
        Ok(None)
    }

    /// Zero-allocation string key lookup in a dict receiver (issue #506).
    ///
    /// Probes the `PyDict` using `StrKey`, which hashes
    /// identically to `PyKey::Str` without constructing a `PyKey` (zero RC
    /// bump, zero allocation).  Use this in place of
    /// `dict_lookup(&PyKey::str_from(s))` whenever the lookup key is already
    /// a `&str`.  The `PyKey::Object` slow path is omitted: a `&str` can
    /// never match an `Object` key.
    pub(crate) fn dict_str_lookup(
        &mut self,
        receiver: &Value,
        key: &str,
    ) -> Result<Option<(usize, Value)>> {
        let dict = receiver
            .as_dict()
            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        Ok(dict
            .get_full(&StrKey(key))
            .map(|(idx, _, v)| (idx, v.clone())))
    }

    /// Check whether a set contains `key`, dispatching user `__eq__` for
    /// `PyKey::Object` keys (issue #368).  Returns the entry index so
    /// callers can implement `discard`/`remove`.
    ///
    /// Takes the receiver `&Value` so the set borrow is scoped tightly —
    /// see [`Self::dict_lookup`] for the rationale.
    pub(crate) fn set_lookup(&mut self, receiver: &Value, key: &PyKey) -> Result<Option<usize>> {
        {
            let set = receiver
                .as_set()
                .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
            if let Some(idx) = set.get_index_of(key) {
                return Ok(Some(idx));
            }
        }
        // Slow path: probe only the lookup key's hash bucket (issue #2060),
        // dispatching user __eq__ to the few candidates that share its hash.
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = {
                let set = receiver
                    .as_set()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                collect_object_bucket_keys_set(&set, key, |k| match k {
                    PyKey::Object { hash, .. } => hash == target_hash,
                    // PyKey::None has Python-level hash py_hash_none(); include it
                    // as a candidate when the Object key hashes to the same value
                    // so that __eq__ can confirm the match (issue #906).
                    PyKey::None => *target_hash == none_hash,
                    _ => false,
                })
            };
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&cand_val, target)? {
                    return self.set_index_by_key(receiver, &cand);
                }
            }
        }
        // Cross-variant slow path: None key vs Object entries with hash py_hash_none()
        // (issue #906).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = {
                let set = receiver
                    .as_set()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                collect_object_bucket_keys_set(
                    &set,
                    key,
                    |k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash),
                )
            };
            let none_val = Value::none();
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&none_val, &cand_val)? {
                    return self.set_index_by_key(receiver, &cand);
                }
            }
        }
        // Nested-object slow path (issue #2059): a Tuple/FrozenSet element key
        // nesting a user object compares that element by `__eq__`.
        if nested_object_tuple_key(key) {
            let candidate_keys = {
                let set = receiver
                    .as_set()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                collect_object_bucket_keys_set(&set, key, nested_object_tuple_key)
            };
            for cand in candidate_keys {
                if self.nested_object_keys_eq(&cand, key)? {
                    return self.set_index_by_key(receiver, &cand);
                }
            }
        }
        Ok(None)
    }

    /// Recover the index of a set entry by its exact stored key (a key cloned
    /// from the set's own bucket).  A single O(bucket) probe, not a full scan.
    fn set_index_by_key(&self, receiver: &Value, key: &PyKey) -> Result<Option<usize>> {
        let set = receiver
            .as_set()
            .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
        Ok(set.get_index_of(key))
    }

    /// `set_lookup` variant that takes the `IndexSet` directly — for
    /// callers that already hold a `&IndexSet`.  Prefer
    /// [`Self::set_lookup`] for new call sites.
    pub(crate) fn set_lookup_in(&mut self, set: &PySet, key: &PyKey) -> Result<Option<usize>> {
        if let Some(idx) = set.get_index_of(key) {
            return Ok(Some(idx));
        }
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            // Probe only the lookup key's hash bucket (issue #2060).
            let candidate_keys = collect_object_bucket_keys_set(set, key, |k| match k {
                PyKey::Object { hash, .. } => hash == target_hash,
                // Cross-variant: PyKey::None has Python-level hash py_hash_none();
                // include it as a candidate when the Object hashes to the same
                // value (issue #906).
                PyKey::None => *target_hash == none_hash,
                _ => false,
            });
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&cand_val, target)? {
                    return Ok(set.get_index_of(&cand));
                }
            }
        }
        // Cross-variant slow path: None key vs Object entries with hash py_hash_none()
        // (issue #906).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = collect_object_bucket_keys_set(
                set,
                key,
                |k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash),
            );
            let none_val = Value::none();
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&none_val, &cand_val)? {
                    return Ok(set.get_index_of(&cand));
                }
            }
        }
        // Nested-object slow path (issue #2059).
        if nested_object_tuple_key(key) {
            let candidate_keys = collect_object_bucket_keys_set(set, key, nested_object_tuple_key);
            for cand in candidate_keys {
                if self.nested_object_keys_eq(&cand, key)? {
                    return Ok(set.get_index_of(&cand));
                }
            }
        }
        Ok(None)
    }
}
