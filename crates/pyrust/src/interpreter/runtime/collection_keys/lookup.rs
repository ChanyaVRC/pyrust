#[inline]
fn dict_lookup_needs_python_hash_index(dict: &PyDict, probe_key: &PyKey) -> bool {
    if dict.has_python_hash_index() {
        return false;
    }

    match probe_key {
        // A top-level Object can compare equal to any non-Object key, whose
        // type-aware backing hash cannot be probed through the Object bucket.
        PyKey::Object { .. } => dict.may_have_non_object_key(),
        // Nested dynamic aggregates already share a native backing bucket
        // with one another. They need the unified shadow only when a plain
        // representation may be present as well.
        PyKey::Tuple(_) | PyKey::FrozenSet(_) if nested_object_tuple_key(probe_key) => {
            dict.may_have_non_dynamic_key()
        }
        // The inverse nested crossing, e.g. `(1,)` probing a stored `(K(),)`,
        // also needs the unified Python-hash probe sequence.
        PyKey::Tuple(_) | PyKey::FrozenSet(_) => dict.may_have_dynamic_key(),
        // Scalar primitive probes can reach top-level Objects through their
        // alternate native bucket. A nested aggregate cannot equal a scalar.
        _ => false,
    }
}

#[inline]
fn dict_lookup_requires_python_probe(dict: &PyDict, probe_key: &PyKey) -> bool {
    match probe_key {
        // These probes can dispatch equality against every representation in
        // their CPython chain. An exact native-map hit is not sufficient: an
        // earlier colliding key may run user `__eq__` first.
        PyKey::Object { .. } => dict.may_have_object_key(),
        // A precise homogeneous dict cannot need a cross-representation
        // probe here. If raw `DerefMut` access poisoned the metadata, however,
        // both conservative presence predicates are true; force lazy repair
        // before allowing an exact native-map hit to bypass earlier user
        // equality candidates.
        _ if !dict.has_python_hash_index() => dict_lookup_needs_python_hash_index(dict, probe_key),
        PyKey::Tuple(_) | PyKey::FrozenSet(_) if nested_object_tuple_key(probe_key) => true,
        // Plain aggregate keys can compare through nested user objects as well
        // as top-level Objects.
        PyKey::Tuple(_) | PyKey::FrozenSet(_) => {
            dict.python_hash_has_dynamic_candidate(pyrust_core::py_hash_pykey(probe_key) as u64)
        }
        // Scalar primitives can only cross to a top-level Object.
        _ => dict.python_hash_has_object_candidate(pyrust_core::py_hash_pykey(probe_key) as u64),
    }
}

impl Interpreter {
    /// Look up a key in a dict using Python hash/equality semantics.
    ///
    /// IndexMap's `get` will find entries whose `PyKey` matches by
    /// pointer-identity (because `PyKey::Object`'s `PartialEq` defers to
    /// `Value::eq`, which uses `Rc::ptr_eq` for `PyInstance`).  When the
    /// fast path misses, same-Python-hash candidates whose backing-map hash
    /// differs are compared through user `__eq__`. Returns
    /// `Ok(Some((index, value)))` on a hit (index returned so callers can
    /// implement `pop`/`del`).
    ///
    /// Takes the receiver `&Value` (rather than `&IndexMap`) so the dict
    /// borrow can be scoped tightly: the fast path borrows for `get_full`
    /// only, and the `__eq__`-dispatching slow path borrows only long
    /// enough to extract the same-hash candidate list before dropping the
    /// borrow and running user code.  This avoids the O(N) whole-dict
    /// snapshot that callers used to have to make for soundness.
    #[inline]
    pub(crate) fn dict_lookup(
        &mut self,
        receiver: &Value,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        {
            let dict = receiver
                .as_dict()
                .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
            let exact = dict.get_full(key);
            if !dict.may_have_dynamic_key() && !key_contains_object(key) {
                return Ok(exact.map(|(idx, _, value)| (idx, value.clone())));
            }
        }
        self.dict_lookup_dynamic(receiver, key)
    }

    #[cold]
    #[inline(never)]
    fn dict_lookup_dynamic(
        &mut self,
        receiver: &Value,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        loop {
            // Fast path and slow-candidate snapshot share one narrow borrow.
            // The version is only observed once a user-equality candidate can
            // exist; ordinary homogeneous hits/misses do no restart work.
            let (candidate_keys, observed_version, activate_hash_index) = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                let exact = dict.get_full(key);
                if !dict.may_have_dynamic_key() && !key_contains_object(key) {
                    return Ok(exact.map(|(idx, _, value)| (idx, value.clone())));
                }
                if !dict_lookup_requires_python_probe(&dict, key)
                    && let Some((idx, _, value)) = exact
                {
                    return Ok(Some((idx, value.clone())));
                }
                if !dict_may_have_user_eq_candidate(&dict, key) {
                    return Ok(None);
                }
                if dict_lookup_needs_python_hash_index(&dict, key) {
                    (Vec::new(), 0, true)
                } else {
                    (
                        collect_dict_user_eq_candidates(&dict, key),
                        dict.structural_version(),
                        false,
                    )
                }
            };

            if activate_hash_index {
                receiver
                    .dict_ensure_python_hash_index()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                continue;
            }

            let mut restart = false;
            for cand in candidate_keys {
                let equal = self.pykeys_user_eq(&cand, key)?;
                let changed = receiver
                    .dict_with(|dict| dict.structural_version() != observed_version)
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                if changed {
                    // `__eq__` changed the key table. The candidate snapshot
                    // and its probe order are stale, so restart from the live
                    // dict (which may now have an exact fast-path match).
                    restart = true;
                    break;
                }
                if equal && let Some(entry) = self.dict_entry_by_key(receiver, &cand)? {
                    // A value-only rewrite leaves the version stable. Re-fetch
                    // the exact key so the returned value is the live one.
                    return Ok(Some(entry));
                }
            }
            if !restart {
                return Ok(None);
            }
        }
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

    /// `dict_lookup` variant that takes the `PyDict` directly.  Used by
    /// callers that already hold a `&PyDict` (typically because they
    /// own/snapshotted the dict, so aliasing with mutable access is
    /// impossible).  Prefer [`Self::dict_lookup`] for new call sites — it
    /// scopes the dict borrow tightly without a whole-dict clone.
    pub(crate) fn dict_lookup_in(
        &mut self,
        dict: &PyDict,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        let exact = dict.get_full(key);
        if !dict.may_have_dynamic_key() && !key_contains_object(key) {
            return Ok(exact.map(|(idx, _, value)| (idx, value.clone())));
        }
        if !dict_lookup_requires_python_probe(dict, key)
            && let Some((idx, _, v)) = exact
        {
            return Ok(Some((idx, v.clone())));
        }
        if !dict_may_have_user_eq_candidate(dict, key) {
            return Ok(None);
        }
        for cand in collect_dict_user_eq_candidates(dict, key) {
            if self.pykeys_user_eq(&cand, key)?
                && let Some((idx, _, value)) = dict.get_full(&cand)
            {
                return Ok(Some((idx, value.clone())));
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
    /// a `&str`.  The allocation-free hit remains unchanged; a miss enters the
    /// equality-aware fallback because a stored Object may compare equal to
    /// the string (#2820).
    #[inline]
    pub(crate) fn dict_str_lookup(
        &mut self,
        receiver: &Value,
        key: &str,
    ) -> Result<Option<(usize, Value)>> {
        {
            let dict = receiver
                .as_dict()
                .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
            let exact = dict
                .get_full(&StrKey(key))
                .map(|(idx, _, value)| (idx, value.clone()));
            if !dict.may_have_object_key() {
                return Ok(exact);
            }
        }
        // A dict containing Object keys must use the unified Python probe even
        // for an exact string hit: an earlier same-hash Object may dispatch
        // `__eq__` before CPython reaches that exact key. The rare mixed path
        // can allocate a canonical PyKey and reuse dict_lookup's structural-
        // mutation restart logic; homogeneous hits/misses remain allocation-
        // free above.
        self.dict_str_lookup_object_miss(receiver, key)
    }

    #[cold]
    #[inline(never)]
    fn dict_str_lookup_object_miss(
        &mut self,
        receiver: &Value,
        key: &str,
    ) -> Result<Option<(usize, Value)>> {
        self.dict_lookup(receiver, &PyKey::str_from(key))
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

    /// Equality-only lookup for immutable set snapshots.
    ///
    /// Frozenset equality is part of recursive dict-key equality. Its elements
    /// can therefore cross the same `PyKey::Object`/primitive backing-hash
    /// boundary as the outer dict key. Scan the immutable snapshot in its
    /// retained candidate order and dispatch stored-left equality only for
    /// same-Python-hash candidates. The exact check stays inside that walk so
    /// an earlier user key cannot be skipped by a later native-map hit.
    pub(crate) fn set_lookup_candidates_in_python_eq(
        &mut self,
        key: &PyKey,
        candidates: &[&PyKey],
    ) -> Result<bool> {
        let key_is_dynamic = key_contains_object(key);
        for &candidate in candidates {
            if candidate == key {
                return Ok(true);
            }
            if (key_is_dynamic || key_contains_object(candidate))
                && self.pykeys_user_eq(candidate, key)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
