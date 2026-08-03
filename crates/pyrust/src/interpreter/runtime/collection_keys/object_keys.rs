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

/// Probe the bucket used by `PyKey::Object` entries with `python_hash`.
///
/// Primitive `PyKey` variants intentionally use type-aware Rust hashes for
/// the backing map, while `PyKey::Object` hashes its precomputed Python hash
/// directly.  A primitive lookup therefore needs this alternate probe to find
/// same-Python-hash Object entries after its ordinary `get_full` misses.
struct ObjectHashBucketProbe<F: Fn(&PyKey) -> bool> {
    python_hash: u64,
    is_match: F,
    collected: std::cell::RefCell<Vec<PyKey>>,
}

impl<F: Fn(&PyKey) -> bool> std::hash::Hash for ObjectHashBucketProbe<F> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.python_hash.hash(state);
    }
}

impl<F: Fn(&PyKey) -> bool> indexmap::Equivalent<PyKey> for ObjectHashBucketProbe<F> {
    fn equivalent(&self, key: &PyKey) -> bool {
        if (self.is_match)(key) {
            self.collected.borrow_mut().push(key.clone());
        }
        false
    }
}

fn collect_object_hash_bucket_keys_map(
    dict: &PyDict,
    python_hash: u64,
    is_match: impl Fn(&PyKey) -> bool,
) -> Vec<PyKey> {
    let probe = ObjectHashBucketProbe {
        python_hash,
        is_match,
        collected: std::cell::RefCell::new(Vec::new()),
    };
    let _ = dict.get_index_of(&probe);
    probe.collected.into_inner()
}

/// Collect dict keys that require Python-level equality after an exact
/// `get_full` miss.
///
/// A mixed Object/non-Object dict supplies hashes represented by both key kinds
/// through its unified side index, including deletion-slot reuse order. A hash
/// represented only by Objects stays on the native O(bucket) probe. An Object
/// probe against primitive-only candidates must scan for equal Python hashes
/// because their type-aware Rust hashes do not share the Object bucket;
/// primitive probes take the inverse cheap path through one alternate Object
/// bucket. A tuple/frozenset that nests an Object retains its existing fallback.
fn collect_dict_user_eq_candidates(dict: &PyDict, probe_key: &PyKey) -> Vec<PyKey> {
    let python_hash = pyrust_core::py_hash_pykey(probe_key) as u64;
    if let Some(candidates) = dict.python_hash_candidates(python_hash) {
        return candidates;
    }
    let must_scan_non_objects =
        !dict.has_python_hash_index() || dict.has_non_object_python_hash(python_hash);

    let mut candidates = match probe_key {
        PyKey::Object {
            hash: target_hash, ..
        } => {
            if dict.may_have_non_object_key() && must_scan_non_objects {
                // Homogeneous primitive dict (or conservatively poisoned
                // metadata): there is no alternate Rust bucket to probe.
                dict.keys()
                    .filter(|key| pyrust_core::py_hash_pykey(key) as u64 == *target_hash)
                    .cloned()
                    .collect()
            } else if dict.may_have_object_key() {
                collect_object_bucket_keys_map(
                    dict,
                    probe_key,
                    |key| matches!(key, PyKey::Object { hash, .. } if hash == target_hash),
                )
            } else {
                Vec::new()
            }
        }
        _ => {
            if dict.may_have_object_key() {
                collect_object_hash_bucket_keys_map(
                    dict,
                    python_hash,
                    |key| matches!(key, PyKey::Object { hash, .. } if *hash == python_hash),
                )
            } else {
                Vec::new()
            }
        }
    };

    if dict.may_have_non_object_key() && nested_object_tuple_key(probe_key) {
        candidates.extend(collect_object_bucket_keys_map(
            dict,
            probe_key,
            nested_object_tuple_key,
        ));
    }
    candidates
}

#[inline]
fn dict_may_have_user_eq_candidate(dict: &PyDict, probe_key: &PyKey) -> bool {
    match probe_key {
        PyKey::Object { .. } => dict.may_have_object_key() || dict.may_have_non_object_key(),
        _ if nested_object_tuple_key(probe_key) => {
            dict.may_have_object_key() || dict.may_have_non_object_key()
        }
        _ => dict.may_have_dynamic_key(),
    }
}

/// Whether a non-Object insertion has a same-Python-hash Object entry that
/// may compare equal.  This is a single alternate bucket probe, avoiding a
/// whole-map scan on the common primitive insertion path.
fn dict_has_object_hash_candidate(dict: &PyDict, probe_key: &PyKey) -> bool {
    let target_hash = pyrust_core::py_hash_pykey(probe_key) as u64;
    if let Some(candidates) = dict.python_hash_candidates(target_hash) {
        return candidates.iter().any(key_contains_object);
    }
    if !dict.may_have_object_key() {
        return false;
    }
    !collect_object_hash_bucket_keys_map(
        dict,
        target_hash,
        |key| matches!(key, PyKey::Object { hash, .. } if *hash == target_hash),
    )
    .is_empty()
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
