/// Hash-bucket probe for collecting `PyKey::Object` / `PyKey::None` lookup
/// candidates in O(bucket) instead of O(n) (issue #2060).
///
/// `PyKey::Object` keys can never resolve via `IndexMap::get` (their `PartialEq`
/// is `Rc::ptr_eq`), so a distinct-but-`__eq__`-equal key always misses the
/// fast path.  The old slow path then linear-scanned *every* entry to find
/// same-hash candidates — O(n) per access, O(n²) to build a dict/set keyed on
/// custom objects.
///
/// Instead, this probe drives `IndexMap`/`IndexSet::get_index_of`, which hashes
/// the probe (placing it in the matching entries' bucket) and calls
/// [`Equivalent::equivalent`] for each entry sharing that bucket's hash.  The
/// probe records every entry its `is_match` predicate accepts as a side effect
/// and always reports "not equivalent", so the walk covers the whole collision
/// chain and returns `None`; the collected Vec then holds the candidates, which
/// the caller confirms via user `__eq__`.  Only bucket entries are visited.
///
/// `probe_key` drives the hash: `PyKey::Object { hash, .. }` and (for the None
/// cross-variant case, issue #906) `PyKey::None` both hash on a Python-level
/// hash value, so matching entries collide into the same bucket.
struct ObjectBucketProbe<'a, F: Fn(&PyKey) -> bool> {
    probe_key: &'a PyKey,
    is_match: F,
    collected: std::cell::RefCell<Vec<PyKey>>,
}

impl<F: Fn(&PyKey) -> bool> std::hash::Hash for ObjectBucketProbe<'_, F> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.probe_key.hash(state);
    }
}

impl<F: Fn(&PyKey) -> bool> indexmap::Equivalent<PyKey> for ObjectBucketProbe<'_, F> {
    fn equivalent(&self, key: &PyKey) -> bool {
        if (self.is_match)(key) {
            self.collected.borrow_mut().push(key.clone());
        }
        // Never report equality: force the bucket walk to continue so we see
        // every collision, and dispatch the real user `__eq__` afterwards.
        false
    }
}

/// Collect the candidate keys in a dict's hash bucket that `is_match` accepts,
/// for later user-`__eq__` dispatch.  Returns each candidate's cloned key (an
/// O(1) RC bump for `Object`/`None`); the caller recovers the entry on a hit
/// via `get_full` (one O(bucket) probe).  See [`ObjectBucketProbe`].
fn collect_object_bucket_keys_map(
    dict: &PyDict,
    probe_key: &PyKey,
    is_match: impl Fn(&PyKey) -> bool,
) -> Vec<PyKey> {
    let probe = ObjectBucketProbe {
        probe_key,
        is_match,
        collected: std::cell::RefCell::new(Vec::new()),
    };
    let _ = dict.get_index_of(&probe);
    probe.collected.into_inner()
}

/// `IndexSet` counterpart of [`collect_object_bucket_keys_map`].
fn collect_object_bucket_keys_set(
    set: &PySet,
    probe_key: &PyKey,
    is_match: impl Fn(&PyKey) -> bool,
) -> Vec<PyKey> {
    let probe = ObjectBucketProbe {
        probe_key,
        is_match,
        collected: std::cell::RefCell::new(Vec::new()),
    };
    let _ = set.get_index_of(&probe);
    probe.collected.into_inner()
}

/// Extract the Python-level `Value` from an `Object`/`None` candidate key for
/// dispatching user `__eq__`.  Bucket candidates are always `Object` or `None`.
fn pykey_object_or_none_value(key: &PyKey) -> Value {
    match key {
        PyKey::Object { value, .. } => value.clone(),
        _ => Value::none(),
    }
}

/// True if `key` is — or transitively contains — a `PyKey::Object` (a user
/// instance whose equality requires `__eq__` dispatch).  Recurses into
/// `Tuple` / `FrozenSet` keys so a nested object inside a tuple key is found
/// (issue #2059).  Primitive keys (`Int`, `Str`, …) and tuples of primitives
/// return `false` and stay on the raw `IndexSet`/`IndexMap` fast path.
pub(super) fn key_contains_object(key: &PyKey) -> bool {
    match key {
        PyKey::Object { .. } => true,
        PyKey::Tuple(items) => items.iter().any(key_contains_object),
        PyKey::FrozenSet(key) => key.items().iter().any(key_contains_object),
        _ => false,
    }
}

/// Returns `true` when `key` is a `Tuple` / `FrozenSet` key that nests a
/// user-object element (so the raw identity-based `PyKey` equality misses an
/// `__eq__`-equal-but-distinct match).  A bare top-level `PyKey::Object` is
/// *not* included here — that case is already handled by the dedicated
/// `Object`-key slow paths.  Issue #2059.
pub(super) fn nested_object_tuple_key(key: &PyKey) -> bool {
    matches!(key, PyKey::Tuple(_) | PyKey::FrozenSet(_)) && key_contains_object(key)
}
