impl Interpreter {
    /// Build a fresh dict from Python-level key/value pairs.
    pub(crate) fn dict_from_value_pairs<I>(
        &mut self,
        capacity: usize,
        unicode_only: bool,
        pairs: I,
    ) -> Result<Value>
    where
        I: IntoIterator<Item = Result<(Value, Value)>>,
    {
        // CPython's `_PyDict_FromItems` scans the already evaluated BUILD_MAP
        // keys before hashing/inserting any of them so it can choose a Unicode
        // or General presized table. Preserve that distinction: inferring from
        // only the first key shrinks duplicate-heavy mixed literals during a
        // spurious Unicode→General conversion and changes observable probe
        // order.
        let mut dict =
            PyDict::with_capacity_and_known_key_kind(capacity, Default::default(), unicode_only);
        for pair in pairs {
            let (key, value) = pair?;
            let key = self.value_to_pykey(&key)?;
            self.dict_insert(&mut dict, key, value)?;
        }
        Ok(Value::dict(dict))
    }

    /// Insert `(key, value)` into a dict that lives at register/local
    /// `dict_value`, dispatching user `__eq__` to deduplicate against an
    /// existing entry when its representation requires Python-level equality.
    /// This includes Object/Object, nested-object, and primitive/Object
    /// cross-representation matches (issues #906, #2059, and #2820).
    pub(crate) fn dict_insert(
        &mut self,
        dict: &mut PyDict,
        key: PyKey,
        value: Value,
    ) -> Result<()> {
        // CPython converts a unicode-only table before looking up a non-exact
        // string insertion key, even when that lookup only replaces a value.
        dict.prepare_python_insert(&key);
        // Object and nested-object keys always require Python equality.
        // Primitive keys need it only when the alternate Object-hash bucket
        // contains a same-Python-hash candidate (#2820); the precheck keeps
        // homogeneous primitive insertion on its raw IndexMap path.
        let needs_dedup = match &key {
            PyKey::Object { .. } => true,
            PyKey::Tuple(_) | PyKey::FrozenSet(_) if nested_object_tuple_key(&key) => true,
            _ => dict_has_object_hash_candidate(dict, &key),
        };
        if needs_dedup && !dict.has_python_hash_index() {
            let crosses_representation = match &key {
                PyKey::Object { .. } => dict.may_have_non_object_key(),
                _ if nested_object_tuple_key(&key) => !dict.is_empty(),
                _ => dict.may_have_dynamic_key(),
            };
            if crosses_representation {
                dict.ensure_python_hash_index();
            }
        }
        if needs_dedup && let Some((idx, _)) = self.dict_lookup_in(dict, &key)? {
            // Replace value in-place via index access to preserve order.
            let existing_key = dict.get_index(idx).map(|(k, _)| k.clone());
            if let Some(k) = existing_key {
                dict.insert(k, value);
                return Ok(());
            }
        }
        dict.insert(key, value);
        Ok(())
    }

    /// Insert into a live dict `Value`, preserving Python `__eq__`
    /// deduplication without holding its `RefCell` borrow across user code.
    ///
    /// `dict_lookup` scopes the backing-map borrow to raw probes and clones
    /// only same-Python-hash candidates before dispatching `__eq__`.
    /// Once it returns, no user code runs between recovering the matching
    /// index and the short mutable borrow used for the actual update.
    pub(crate) fn dict_insert_value(
        &mut self,
        receiver: &Value,
        key: PyKey,
        value: Value,
    ) -> Result<()> {
        receiver
            .dict_prepare_python_insert(&key)
            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        // Primitive keys are fully handled by IndexMap's Hash + PartialEq
        // implementation, including numeric cross-type equality.  Avoid the
        // redundant pre-lookup in that overwhelmingly common case: insert
        // itself already performs the required table probe.  Only keys whose
        // equality can dispatch Python code need the receiver-based lookup
        // before taking the short mutable borrow.
        let needs_dedup = match &key {
            PyKey::Object { .. } => true,
            PyKey::Tuple(_) | PyKey::FrozenSet(_) if nested_object_tuple_key(&key) => true,
            _ => receiver
                .dict_with(|dict| dict_has_object_hash_candidate(dict, &key))
                .unwrap_or(false),
        };
        if needs_dedup {
            let activate_hash_index = receiver
                .dict_with(|dict| {
                    if dict.has_python_hash_index() {
                        return false;
                    }
                    match &key {
                        PyKey::Object { .. } => dict.may_have_non_object_key(),
                        _ if nested_object_tuple_key(&key) => !dict.is_empty(),
                        _ => dict.may_have_dynamic_key(),
                    }
                })
                .unwrap_or(false);
            if activate_hash_index {
                receiver
                    .dict_ensure_python_hash_index()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
            }
        }
        let existing = if needs_dedup {
            self.dict_lookup(receiver, &key)?
        } else {
            None
        };
        receiver
            .dict_with_mut(|dict| {
                if let Some((idx, _)) = existing
                    && let Some(stored_key) = dict.get_index(idx).map(|(k, _)| k.clone())
                {
                    // Keep the original key object and insertion position when
                    // a distinct but Python-equal object is assigned.
                    dict.insert(stored_key, value);
                } else {
                    dict.insert(key, value);
                }
            })
            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        Ok(())
    }

    /// Bulk-insert `(key, value)` pairs into a dict with last-value-wins
    /// dedup, dispatching user `__eq__` for `PyKey::Object` keys (issues
    /// #1914 / #1919).  This is the shared mechanism behind `dict.update`,
    /// `|`/`|=`, `dict.fromkeys`, `dict(pairs)`, and the collections
    /// `Counter`/`defaultdict` bulk paths.
    ///
    /// Fast path: when neither the destination map nor any incoming key is a
    /// `PyKey::Object` (the overwhelmingly common primitive-key case), this is
    /// a plain `IndexMap::extend` — no `__eq__` dispatch, no per-key scan.  The
    /// slow path engages only when an `Object` key is present on either side,
    /// routing each insert through `dict_insert` (which dedups via
    /// `dict_lookup_in`'s `__hash__`-then-`__eq__` scan).
    pub(crate) fn dict_extend_dedup(
        &mut self,
        dict: &mut PyDict,
        pairs: Vec<(PyKey, Value)>,
    ) -> Result<()> {
        let dest_has_object = dict.keys().any(key_contains_object);
        let src_has_object = pairs.iter().any(|(k, _)| key_contains_object(k));
        if !dest_has_object && !src_has_object {
            // Primitive-key fast path: raw IndexMap::extend (last value wins).
            dict.extend(pairs);
            return Ok(());
        }
        for (key, value) in pairs {
            self.dict_insert(dict, key, value)?;
        }
        Ok(())
    }

    /// In-place bulk-update of a dict *receiver* `Value` with last-value-wins
    /// dedup, dispatching user `__eq__` for `PyKey::Object` keys (issue #1914).
    /// This is the receiver-based companion to [`Self::dict_extend_dedup`],
    /// used where the dict must be mutated in place (`dict.update`, `|=`) so
    /// aliasing references observe the change.
    ///
    /// Fast path: when neither the receiver nor any incoming key is a
    /// `PyKey::Object`, a single `dict_with_mut` raw `extend` (last value wins).
    /// Slow path: per-pair `dict_lookup` on the receiver (which drops the dict
    /// borrow before running user `__eq__`) followed by a scoped `dict_with_mut`
    /// insert that overwrites the `__eq__`-equal entry in place.
    pub(crate) fn dict_extend_value_dedup(
        &mut self,
        receiver: &Value,
        pairs: Vec<(PyKey, Value)>,
    ) -> Result<()> {
        let dest_has_object = receiver
            .dict_with(|d| d.keys().any(key_contains_object))
            .unwrap_or(false);
        let src_has_object = pairs.iter().any(|(k, _)| key_contains_object(k));
        if !dest_has_object && !src_has_object {
            // Primitive-key fast path: one storage-owned extend (last value
            // wins). The storage boundary can retain exact string-key
            // notifications for an aliased globals/locals dictionary.
            receiver.dict_extend(pairs)?;
            return Ok(());
        }
        for (key, value) in pairs {
            self.dict_insert_value(receiver, key, value)?;
        }
        Ok(())
    }

    /// Insert `key` into a set, dispatching user `__eq__` for dedup.
    /// Handles both `Object` keys and `None` keys for cross-variant dedup
    /// (issue #906): inserting None into a set that already holds an Object
    /// with hash py_hash_none() that __eq__-matches None must not create a
    /// duplicate.
    pub(crate) fn set_insert(&mut self, set: &mut PySet, key: PyKey) -> Result<()> {
        // Same fast pre-check as `dict_insert` (issue #934): for `PyKey::None`,
        // only call `set_lookup_in` when the set contains a `PyKey::Object` with
        // hash `py_hash_none()` (rare cross-variant case, issue #906).
        let needs_dedup = match &key {
            PyKey::Object { .. } => true,
            // Issue #2059: dedup a tuple/frozenset key nesting a user object
            // against an `__eq__`-equal-but-distinct existing element.
            PyKey::Tuple(_) | PyKey::FrozenSet(_) if nested_object_tuple_key(&key) => true,
            PyKey::None => {
                let none_hash = pyrust_core::py_hash_none() as u64;
                set.iter()
                    .any(|k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash))
            }
            _ => false,
        };
        if needs_dedup && self.set_lookup_in(set, &key)?.is_some() {
            return Ok(());
        }
        set.insert(key);
        Ok(())
    }

    /// Hash and insert one Python value into a live set accumulator.
    ///
    /// This receiver-based form is used by set displays/comprehensions. It
    /// keeps representation-aware equality deduplication out of the opcode
    /// loop and releases the set borrow before any user `__eq__` call.
    pub(crate) fn set_insert_value(&mut self, receiver: &Value, value: Value) -> Result<()> {
        let key = self.value_to_pykey(&value)?;
        let needs_dedup = match &key {
            PyKey::Object { .. } => true,
            PyKey::Tuple(_) | PyKey::FrozenSet(_) if nested_object_tuple_key(&key) => true,
            PyKey::None => {
                let none_hash = pyrust_core::py_hash_none() as u64;
                receiver
                    .as_set()
                    .map(|set| {
                        set.iter().any(|stored| {
                            matches!(stored,
                                PyKey::Object { hash, .. } if *hash == none_hash)
                        })
                    })
                    .unwrap_or(false)
            }
            _ => false,
        };
        if needs_dedup && self.set_lookup(receiver, &key)?.is_some() {
            return Ok(());
        }
        receiver.set_add(key).map(|_| ())
    }
}
