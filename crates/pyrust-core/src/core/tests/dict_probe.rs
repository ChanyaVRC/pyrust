#[test]
fn dict_key_kind_counts_are_live_precise_and_not_semantic() {
    let class = Rc::new(RefCell::new(PyClass::new(
        "Key",
        "Key",
        None,
        IndexMap::new(),
    )));
    let object_key = || PyKey::Object {
        hash: 7,
        value: plain_instance(Rc::clone(&class)),
    };
    let first_object = object_key();
    let second_object = object_key();

    let mut dict = PyDict::default();
    assert_eq!(dict.live_object_key_count, 0);
    assert!(!dict.may_have_non_object_key());
    assert!(!dict.may_have_object_key());
    assert!(dict.python_hash_index.is_none());

    dict.insert(first_object.clone(), Value::int(1));
    assert_eq!(dict.live_object_key_count, 1);
    assert!(!dict.may_have_non_object_key());
    assert!(dict.may_have_object_key());
    assert!(dict.python_hash_index.is_none());

    let object_only_clone = dict.clone();
    assert_eq!(object_only_clone.live_object_key_count, 1);
    assert!(object_only_clone.python_hash_index.is_none());
    assert_eq!(dict, object_only_clone);

    dict.extend([
        (second_object.clone(), Value::int(2)),
        (PyKey::Int(7), Value::int(3)),
    ]);
    assert_eq!(dict.live_object_key_count, 2);
    assert!(dict.may_have_non_object_key());
    assert!(dict.may_have_object_key());
    assert!(dict.python_hash_index.is_some());

    dict.shift_remove(&first_object);
    assert_eq!(dict.live_object_key_count, 1);
    assert!(dict.python_hash_index.is_some());
    dict.shift_remove(&PyKey::Int(7));
    assert_eq!(dict.live_object_key_count, 1);
    assert!(!dict.may_have_non_object_key());
    assert!(dict.has_python_hash_index());
    assert!(dict.python_hash_index.is_some());
    dict.shift_remove(&second_object);
    assert_eq!(dict.live_object_key_count, 0);
    assert!(!dict.may_have_object_key());

    let empty = PyDict::default();
    assert_eq!(dict, empty);
    assert_eq!(
        Value::dict(dict.clone()),
        Value::dict(empty),
        "optimization metadata must not affect dict equality"
    );

    let from_iter: PyDict = [
        (object_key(), Value::int(1)),
        (PyKey::Int(7), Value::int(2)),
    ]
    .into_iter()
    .collect();
    assert_eq!(from_iter.live_object_key_count, 1);
    assert!(from_iter.python_hash_index.is_some());
    assert_eq!(from_iter.python_hash_candidates(7).unwrap().len(), 2);

    let unrelated_object = PyKey::Object {
        hash: 8,
        value: plain_instance(Rc::clone(&class)),
    };
    let unrelated_hashes: PyDict = [
        (PyKey::Int(7), Value::int(1)),
        (unrelated_object.clone(), Value::int(2)),
    ]
    .into_iter()
    .collect();
    assert!(unrelated_hashes.has_python_hash_index());
    assert!(unrelated_hashes.has_non_object_python_hash(7));
    assert_eq!(
        unrelated_hashes.python_hash_candidates(7),
        Some(vec![PyKey::Int(7)])
    );
    assert_eq!(
        unrelated_hashes.python_hash_candidates(8),
        Some(vec![unrelated_object])
    );

    let target_object = object_key();
    let persistent_object = PyKey::Object {
        hash: 8,
        value: plain_instance(Rc::clone(&class)),
    };
    let mut recrossed_hash: PyDict = [
        (target_object.clone(), Value::int(1)),
        (PyKey::Int(7), Value::int(2)),
        (persistent_object, Value::int(3)),
        (PyKey::Int(9), Value::int(4)),
    ]
    .into_iter()
    .collect();
    recrossed_hash.shift_remove(&PyKey::Int(7));
    assert!(!recrossed_hash.has_non_object_python_hash(7));
    assert_eq!(
        recrossed_hash.python_hash_candidates(7),
        Some(vec![target_object.clone()])
    );
    recrossed_hash.shift_remove(&target_object);
    assert_eq!(
        recrossed_hash.python_hash_candidates(7),
        Some(Vec::new()),
        "an active global shadow distinguishes a precise empty probe"
    );
    recrossed_hash.insert(PyKey::Int(7), Value::int(5));
    let replacement_object = object_key();
    recrossed_hash.insert(replacement_object.clone(), Value::int(6));
    assert_eq!(
        recrossed_hash.python_hash_candidates(7).unwrap(),
        [PyKey::Int(7), replacement_object],
        "re-crossing a hash must reuse its retained deletion slots"
    );

    let mut swap_removed = from_iter.clone();
    let stored_object = swap_removed
        .keys()
        .find(|key| matches!(key, PyKey::Object { .. }))
        .unwrap()
        .clone();
    assert!(swap_removed.swap_remove(&stored_object).is_some());
    assert_eq!(swap_removed.live_object_key_count, 0);
    assert!(swap_removed.may_have_non_object_key());
    assert!(swap_removed.has_python_hash_index());
    assert!(swap_removed.python_hash_index.is_some());

    let mut retained = from_iter.clone();
    retained.retain(|key, _| !matches!(key, PyKey::Object { .. }));
    assert_eq!(retained.live_object_key_count, 0);
    assert!(retained.may_have_non_object_key());
    assert!(!retained.may_have_object_key());
    assert!(retained.has_python_hash_index());
    assert!(retained.python_hash_index.is_some());

    let mut cleared = from_iter.clone();
    cleared.clear();
    assert_eq!(cleared.live_object_key_count, 0);
    assert!(!cleared.may_have_object_key());
    assert!(!cleared.may_have_non_object_key());
    assert!(cleared.python_hash_index.is_none());

    let mut unknown_mutation = PyDict::default();
    let _: &mut IndexMap<PyKey, Value, super::PyHasher> = &mut unknown_mutation;
    assert_eq!(
        unknown_mutation.live_object_key_count,
        super::POISONED_OBJECT_KEY_COUNT
    );
    assert!(unknown_mutation.may_have_non_object_key());
    assert!(unknown_mutation.may_have_object_key());
    unknown_mutation.clear();
    assert_eq!(unknown_mutation.live_object_key_count, 0);
    assert!(!unknown_mutation.may_have_non_object_key());
    assert!(!unknown_mutation.may_have_object_key());
}

#[test]
fn dict_structural_version_and_mixed_probe_slots_track_exact_mutation() {
    let class = Rc::new(RefCell::new(PyClass::new(
        "Key",
        "Key",
        None,
        IndexMap::new(),
    )));
    let object_key = || PyKey::Object {
        hash: 1,
        value: plain_instance(Rc::clone(&class)),
    };
    let first = object_key();
    let second = object_key();
    let replacement = object_key();
    let primitive = PyKey::Int(1);

    let mut dict = PyDict::default();
    dict.insert(first.clone(), Value::int(1));
    dict.insert(primitive.clone(), Value::int(2));
    dict.insert(second.clone(), Value::int(3));
    let before_value_replacement = dict.structural_version();
    assert_eq!(
        dict.insert(primitive.clone(), Value::int(4)),
        Some(Value::int(2))
    );
    assert_eq!(dict.structural_version(), before_value_replacement);
    assert_eq!(dict.live_object_key_count, 2);

    for (_, value) in &mut dict {
        *value = Value::none();
    }
    assert_eq!(dict.structural_version(), before_value_replacement);

    let first_index = dict.get_index_of(&first).unwrap();
    dict.shift_remove_index(first_index);
    let after_removal = dict.structural_version();
    dict.insert(replacement.clone(), Value::int(5));
    assert_eq!(dict.structural_version(), after_removal.wrapping_add(1));
    assert_eq!(
        dict.python_hash_candidates(1).unwrap(),
        [replacement.clone(), primitive.clone(), second.clone()],
        "a same-hash insertion must reuse the deleted probe slot"
    );

    let cloned = dict.clone();
    assert_eq!(cloned.structural_version(), dict.structural_version());
    assert_eq!(
        cloned.python_hash_candidates(1),
        dict.python_hash_candidates(1)
    );

    let before_clear = dict.structural_version();
    dict.clear();
    assert_eq!(dict.structural_version(), before_clear.wrapping_add(1));
    assert_eq!(dict.live_object_key_count, 0);
    assert!(dict.python_hash_index.is_none());
    let after_clear = dict.structural_version();
    dict.clear();
    assert_eq!(dict.structural_version(), after_clear);
}

#[test]
fn dict_homogeneous_deletion_history_survives_cross_and_recross() {
    let class = Rc::new(RefCell::new(PyClass::new(
        "Key",
        "Key",
        None,
        IndexMap::new(),
    )));
    let object_key = || PyKey::Object {
        hash: 1,
        value: plain_instance(Rc::clone(&class)),
    };

    // The first deletion happens while the dict is Object-only. Its dummy
    // must precede the surviving same-hash Object after the first primitive
    // insertion crosses representations.
    let first = object_key();
    let survivor = object_key();
    let mut first_cross = PyDict::default();
    first_cross.insert(first.clone(), Value::int(1));
    first_cross.insert(survivor.clone(), Value::int(2));
    first_cross.shift_remove(&first);
    assert!(!first_cross.has_python_hash_index());
    assert_eq!(
        first_cross
            .python_hash_mutation_journal
            .as_ref()
            .unwrap()
            .mutations
            .len(),
        1,
        "a homogeneous deletion records only its compact mutation"
    );
    assert_eq!(first_cross.python_hash_candidates(1), None);
    first_cross.insert(PyKey::Int(1), Value::int(3));
    assert!(first_cross.python_hash_mutation_journal.is_none());
    assert_eq!(
        first_cross.python_hash_candidates(1).unwrap(),
        [PyKey::Int(1), survivor.clone()]
    );

    // Leaving mixed mode drops only kind counts. The crossed hash's dummy
    // order remains available when the primitive representation returns.
    let left = object_key();
    let right = object_key();
    let mut recross = PyDict::default();
    recross.insert(left.clone(), Value::int(1));
    recross.insert(PyKey::Int(1), Value::int(2));
    recross.insert(right.clone(), Value::int(3));
    recross.shift_remove(&PyKey::Int(1));
    assert!(recross.has_python_hash_index());
    recross.insert(PyKey::Int(1), Value::int(4));
    assert_eq!(
        recross.python_hash_candidates(1).unwrap(),
        [left, PyKey::Int(1), right]
    );
}

#[test]
fn dict_deferred_removal_journal_replays_the_eager_probe_layout() {
    let mut deferred = PyDict::with_capacity_and_hasher(20, Default::default());
    let mut eager = PyDict::with_capacity_and_hasher(20, Default::default());
    for value in 0..12 {
        deferred.insert(PyKey::Int(value), Value::int(value));
        eager.insert(PyKey::Int(value), Value::int(value));
    }
    eager.ensure_python_hash_index();

    for dict in [&mut deferred, &mut eager] {
        dict.shift_remove(&PyKey::Int(3));
        dict.insert(PyKey::Int(100), Value::int(100));
        dict.shift_remove_index(0);
        dict.pop();
        dict.insert(PyKey::Int(101), Value::int(101));
        dict.insert(PyKey::Int(102), Value::int(102));
        dict.shift_remove(&PyKey::Int(8));
    }

    assert!(!deferred.has_python_hash_index());
    assert_eq!(
        deferred
            .python_hash_mutation_journal
            .as_ref()
            .unwrap()
            .mutations
            .len(),
        7
    );

    let mut deferred_copy = deferred.clone();
    let eager_copy = eager.clone();
    assert!(deferred_copy.python_hash_index.is_none());
    assert!(deferred_copy.python_hash_mutation_journal.is_some());
    deferred_copy.ensure_python_hash_index();
    let deferred_copy_index = deferred_copy.python_hash_index.as_ref().unwrap();
    let eager_copy_index = eager_copy.python_hash_index.as_ref().unwrap();
    assert_eq!(deferred_copy_index.indices, eager_copy_index.indices);
    assert_eq!(deferred_copy_index.entry_keys, eager_copy_index.entry_keys);
    assert_eq!(deferred_copy_index.usable, eager_copy_index.usable);

    deferred.ensure_python_hash_index();
    assert!(deferred.python_hash_mutation_journal.is_none());

    let deferred = deferred.python_hash_index.as_ref().unwrap();
    let eager = eager.python_hash_index.as_ref().unwrap();
    assert_eq!(deferred.indices, eager.indices);
    assert_eq!(deferred.entry_keys, eager.entry_keys);
    assert_eq!(deferred.usable, eager.usable);
    assert_eq!(deferred.live_len, eager.live_len);
    assert_eq!(deferred.unicode_only, eager.unicode_only);

    let mut deferred_sparse = PyDict::default();
    let mut eager_sparse = PyDict::default();
    for value in 0..12 {
        deferred_sparse.insert(PyKey::Int(value), Value::int(value));
        eager_sparse.insert(PyKey::Int(value), Value::int(value));
    }
    eager_sparse.ensure_python_hash_index();
    for value in 0..9 {
        deferred_sparse.shift_remove(&PyKey::Int(value));
        eager_sparse.shift_remove(&PyKey::Int(value));
    }
    let mut deferred_sparse = deferred_sparse.clone();
    let mut eager_sparse = eager_sparse.clone();
    assert!(!deferred_sparse.has_python_hash_index());
    assert!(deferred_sparse.python_hash_mutation_journal.is_none());
    deferred_sparse.ensure_python_hash_index();
    eager_sparse.ensure_python_hash_index();
    let deferred_sparse = deferred_sparse.python_hash_index.as_ref().unwrap();
    let eager_sparse = eager_sparse.python_hash_index.as_ref().unwrap();
    assert_eq!(deferred_sparse.indices, eager_sparse.indices);
    assert_eq!(deferred_sparse.entry_keys, eager_sparse.entry_keys);
    assert_eq!(deferred_sparse.usable, eager_sparse.usable);
    assert_eq!(deferred_sparse.live_len, eager_sparse.live_len);

    let mut deferred_history = PyDict::default();
    let retained_history = 64usize;
    let live_len = retained_history * 4;
    for value in 0..live_len {
        deferred_history.insert(PyKey::Int(value as i64), Value::int(value as i64));
    }
    for value in 0..retained_history {
        deferred_history.shift_remove(&PyKey::Int(value as i64));
    }
    assert!(!deferred_history.has_python_hash_index());
    assert_eq!(
        deferred_history
            .python_hash_mutation_journal
            .as_ref()
            .unwrap()
            .mutations
            .len(),
        retained_history
    );
}

#[test]
fn dict_long_front_removal_journal_stays_deferred_and_matches_eager_probe_layout() {
    let mut deferred = PyDict::default();
    let mut eager = PyDict::default();
    for value in 0..256 {
        deferred.insert(PyKey::Int(value), Value::int(value));
        eager.insert(PyKey::Int(value), Value::int(value));
    }
    eager.ensure_python_hash_index();

    // Cross the former journal-versus-live-table threshold using the
    // positional pattern that made Vec-based reverse replay quadratic.
    // Homogeneous removals must not synchronously materialize the shadow.
    for value in 0..180 {
        deferred.shift_remove(&PyKey::Int(value));
        eager.shift_remove(&PyKey::Int(value));
    }
    assert!(!deferred.has_python_hash_index());
    assert_eq!(
        deferred
            .python_hash_mutation_journal
            .as_ref()
            .unwrap()
            .mutations
            .len(),
        180
    );

    deferred.ensure_python_hash_index();

    let deferred = deferred.python_hash_index.as_ref().unwrap();
    let eager = eager.python_hash_index.as_ref().unwrap();
    assert_eq!(deferred.indices, eager.indices);
    assert_eq!(deferred.entry_keys, eager.entry_keys);
    assert_eq!(deferred.usable, eager.usable);
    assert_eq!(deferred.live_len, eager.live_len);
    assert_eq!(deferred.unicode_only, eager.unicode_only);
}

#[test]
fn dict_sparse_copy_compacts_deferred_non_pop_history_without_replay() {
    let mut source = PyDict::default();
    for value in 0..512 {
        source.insert(PyKey::Int(value), Value::int(value));
    }
    for value in (170..512).rev() {
        source.shift_remove(&PyKey::Int(value));
    }
    assert!(!source.has_python_hash_index());
    assert!(source.python_hash_mutation_journal.is_some());

    let mut copied = source.clone();
    assert!(!copied.has_python_hash_index());
    assert!(copied.python_hash_mutation_journal.is_none());

    copied.ensure_python_hash_index();
    let copied_index = copied.python_hash_index.as_ref().unwrap();
    assert_eq!(copied_index.live_len, copied.len());
    assert_eq!(copied_index.entry_keys.len(), copied.len());
}
