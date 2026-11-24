#[test]
fn set_probe_journal_replays_mutations_and_constructor_topology() {
    let mut deferred = PySet::default();
    let mut eager = PySetProbeSnapshot::with_table_size(8);
    let mut replay_ids = HashMap::new();
    let mut next_replay_id = 0u64;

    for value in 0..24 {
        let key = PyKey::Int(value);
        deferred.insert(key.clone());
        replay_ids.insert(value, next_replay_id);
        eager.insert(SetProbeEntry {
            python_hash: py_hash_pykey(&PyKey::Int(value)) as u64,
            replay_id: next_replay_id,
        });
        next_replay_id += 1;
    }
    for value in [3, 17, 0, 11] {
        deferred.shift_remove(&PyKey::Int(value));
        eager.remove(py_hash_pykey(&PyKey::Int(value)) as u64, replay_ids[&value]);
    }
    for value in [30, 31, 3] {
        let key = PyKey::Int(value);
        deferred.insert(key.clone());
        replay_ids.insert(value, next_replay_id);
        eager.insert(SetProbeEntry {
            python_hash: py_hash_pykey(&PyKey::Int(value)) as u64,
            replay_id: next_replay_id,
        });
        next_replay_id += 1;
    }
    assert_eq!(deferred.python_hash_snapshot(), eager);

    let mut copied_source = PySet::default();
    copied_source.insert(PyKey::Int(0));
    copied_source.insert(PyKey::Int(1));
    copied_source.shift_remove(&PyKey::Int(0));
    copied_source.insert(PyKey::Int(0));
    let copied = PySet::cpython_merged_copy(&copied_source);
    assert_eq!(
        copied.python_hash_snapshot(),
        copied_source.python_hash_snapshot()
    );

    let mut dict_source = PySet::with_cpython_dict_capacity(5);
    for value in 0..5 {
        dict_source.insert(PyKey::Int(value));
    }
    assert_eq!(dict_source.python_hash_snapshot().table_size(), 16);

    let mut deferred_cleanup = PySet::default();
    let mut eager_cleanup = PySetProbeSnapshot::with_table_size(8);
    for value in 0..18 {
        let key = PyKey::Int(value);
        deferred_cleanup.insert(key.clone());
        eager_cleanup.insert(SetProbeEntry {
            python_hash: py_hash_pykey(&PyKey::Int(value)) as u64,
            replay_id: value as u64,
        });
    }
    for value in 0..8 {
        deferred_cleanup.shift_remove(&PyKey::Int(value));
        eager_cleanup.remove(py_hash_pykey(&PyKey::Int(value)) as u64, value as u64);
    }
    deferred_cleanup.finish_cpython_difference_update();
    eager_cleanup.finish_difference_update();
    assert_eq!(deferred_cleanup.python_hash_snapshot(), eager_cleanup);
    assert_eq!(eager_cleanup.table_size(), 64);
}

#[test]
fn set_sparse_tail_removal_copy_preserves_probe_topology() {
    let mut source = PySet::default();
    let mut eager = PySetProbeSnapshot::with_table_size(8);
    for value in 0..512 {
        let key = PyKey::Int(value);
        source.insert(key.clone());
        eager.insert(SetProbeEntry {
            python_hash: py_hash_pykey(&PyKey::Int(value)) as u64,
            replay_id: value as u64,
        });
    }
    for value in (170..512).rev() {
        source.shift_remove(&PyKey::Int(value));
        eager.remove(py_hash_pykey(&PyKey::Int(value)) as u64, value as u64);
    }

    let journal = source
        .python_hash_mutation_journal
        .as_deref()
        .expect("tail removals must retain probe history");
    assert!(journal.has_only_tail_removals(source.len()));
    assert_eq!(source.python_hash_snapshot(), eager);

    let copied = PySet::cpython_merged_copy(&source);
    let mut expected_copy = PySetProbeSnapshot::with_table_size(512);
    for (replay_id, python_hash) in eager.active_python_hashes().enumerate() {
        expected_copy.insert(SetProbeEntry {
            python_hash,
            replay_id: replay_id as u64,
        });
    }
    assert_eq!(copied.python_hash_snapshot(), expected_copy);
}

#[test]
fn set_bulk_retain_preserves_descending_removal_topology() {
    let mut bulk = PySet::default();
    let mut manual = PySet::default();
    for value in 0..128 {
        bulk.insert(PyKey::Int(value));
        manual.insert(PyKey::Int(value));
    }

    bulk.retain(|key| !matches!(key, PyKey::Int(value) if value % 3 != 0));
    for index in (0..128).filter(|value| value % 3 != 0).rev() {
        manual.shift_remove_index(index);
    }

    assert_eq!(bulk, manual);
    assert_eq!(bulk.python_hash_snapshot(), manual.python_hash_snapshot());
}

#[test]
fn dict_deferred_journal_rebases_when_insertion_resizes() {
    let mut deferred = PyDict::default();
    let mut eager = PyDict::default();
    for value in 0..5 {
        deferred.insert(PyKey::Int(value), Value::int(value));
        eager.insert(PyKey::Int(value), Value::int(value));
    }
    eager.ensure_python_hash_index();

    deferred.shift_remove(&PyKey::Int(0));
    eager.shift_remove(&PyKey::Int(0));
    assert_eq!(
        deferred
            .python_hash_mutation_journal
            .as_ref()
            .unwrap()
            .mutations
            .len(),
        1
    );

    // The size-8 table has no remaining dk_usable entries. CPython
    // compacts it before this insertion, so all earlier history can become
    // a fresh size-16 baseline without building a persistent shadow.
    deferred.insert(PyKey::Int(100), Value::int(100));
    eager.insert(PyKey::Int(100), Value::int(100));
    assert!(!deferred.has_python_hash_index());
    assert!(
        deferred
            .python_hash_mutation_journal
            .as_ref()
            .unwrap()
            .mutations
            .is_empty()
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
fn dict_deferred_journal_resize_rebases_match_eager_layouts() {
    for initial_len in 1usize..=48 {
        let mut deferred = PyDict::default();
        let mut eager = PyDict::default();
        for value in 0..initial_len {
            let key = PyKey::Int(value as i64);
            deferred.insert(key.clone(), Value::int(value as i64));
            eager.insert(key, Value::int(value as i64));
        }
        eager.ensure_python_hash_index();

        let mut next_key = 1_000i64;
        for step in 0..(initial_len * 4 + 32) {
            if step % 3 == 0 && !deferred.is_empty() {
                let index = (step * 7) % deferred.len();
                deferred.shift_remove_index(index);
                eager.shift_remove_index(index);
            } else {
                let key = PyKey::Int(next_key);
                deferred.insert(key.clone(), Value::int(next_key));
                eager.insert(key, Value::int(next_key));
                next_key += 1;
            }
        }

        assert!(!deferred.has_python_hash_index());
        deferred.ensure_python_hash_index();
        let deferred = deferred.python_hash_index.as_ref().unwrap();
        let eager = eager.python_hash_index.as_ref().unwrap();
        assert_eq!(deferred.indices, eager.indices, "initial_len={initial_len}");
        assert_eq!(
            deferred.entry_keys, eager.entry_keys,
            "initial_len={initial_len}"
        );
        assert_eq!(deferred.usable, eager.usable, "initial_len={initial_len}");
        assert_eq!(
            deferred.live_len, eager.live_len,
            "initial_len={initial_len}"
        );
        assert_eq!(
            deferred.unicode_only, eager.unicode_only,
            "initial_len={initial_len}"
        );
    }
}

#[test]
fn dict_global_probe_shadow_preserves_cross_hash_dummy_reuse() {
    let class = Rc::new(RefCell::new(PyClass::new(
        "Key",
        "Key",
        None,
        IndexMap::new(),
    )));
    let stored_hash_nine = PyKey::Object {
        hash: 9,
        value: plain_instance(class),
    };

    // Hashes 1 and 9 share their initial size-8 slot. Removing hash 1
    // leaves a DUMMY which the new primitive hash-9 key reuses before the
    // older Object encountered later in hash 9's perturb chain.
    let mut dict = PyDict::default();
    dict.insert(PyKey::Int(1), Value::int(1));
    dict.insert(stored_hash_nine.clone(), Value::int(2));
    dict.shift_remove(&PyKey::Int(1));
    dict.insert(PyKey::Int(9), Value::int(3));
    assert_eq!(
        dict.python_hash_candidates(9).unwrap(),
        [PyKey::Int(9), stored_hash_nine]
    );
}

#[test]
fn dict_nested_object_keys_activate_global_probe_shadow_both_directions() {
    let class = Rc::new(RefCell::new(PyClass::new(
        "Key",
        "Key",
        None,
        IndexMap::new(),
    )));
    let nested = || {
        PyKey::Tuple(
            vec![PyKey::Object {
                hash: 1,
                value: plain_instance(Rc::clone(&class)),
            }]
            .into_boxed_slice(),
        )
    };
    let primitive = PyKey::Tuple(vec![PyKey::Int(1)].into_boxed_slice());
    let python_hash = py_hash_pykey(&primitive) as u64;

    let dynamic_first = nested();
    let mut dict = PyDict::default();
    dict.insert(dynamic_first.clone(), Value::int(1));
    assert!(dict.has_python_hash_index());
    dict.insert(primitive.clone(), Value::int(2));
    assert_eq!(
        dict.python_hash_candidates(python_hash).unwrap(),
        [dynamic_first.clone(), primitive.clone(), dynamic_first],
        "CPython's perturb sequence may revisit a slot before EMPTY"
    );

    let dynamic_second = nested();
    let mut reverse = PyDict::default();
    reverse.insert(primitive.clone(), Value::int(1));
    assert!(!reverse.has_python_hash_index());
    reverse.insert(dynamic_second.clone(), Value::int(2));
    assert_eq!(
        reverse.python_hash_candidates(python_hash).unwrap(),
        [primitive.clone(), dynamic_second, primitive]
    );
}

#[test]
fn dict_probe_history_compacts_on_resize_and_copy_like_cpython() {
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

    let deleted = object_key();
    let survivor = object_key();
    let replacement = object_key();
    let mut resized = PyDict::default();
    resized.insert(deleted.clone(), Value::int(1));
    resized.insert(PyKey::Int(1), Value::int(2));
    resized.insert(survivor.clone(), Value::int(3));
    resized.shift_remove(&deleted);
    resized.insert(PyKey::Int(100), Value::int(4));
    resized.insert(PyKey::Int(101), Value::int(5));
    resized.insert(replacement.clone(), Value::int(6));
    assert_eq!(
        resized.python_hash_candidates(1).unwrap(),
        [PyKey::Int(1), survivor.clone(), replacement],
        "usable exhaustion must compact dummies before the new insertion"
    );

    // used == 2/3 * nentries takes CPython's dense fast-copy path and
    // preserves the leading dummy.
    let dense_a = object_key();
    let dense_b = object_key();
    let mut dense = PyDict::default();
    dense.insert(dense_a.clone(), Value::int(1));
    dense.insert(PyKey::Int(1), Value::int(2));
    dense.insert(dense_b.clone(), Value::int(3));
    dense.shift_remove(&dense_a);
    let mut dense_copy = dense.clone();
    let dense_new = object_key();
    dense_copy.insert(dense_new.clone(), Value::int(4));
    assert_eq!(
        dense_copy.python_hash_candidates(1).unwrap(),
        [dense_new, PyKey::Int(1), dense_b]
    );

    // Below 2/3 density CPython copies through reinsertion, compacting all
    // dummies. The next Object therefore follows the surviving primitive.
    let sparse_a = object_key();
    let sparse_b = object_key();
    let mut sparse = PyDict::default();
    sparse.insert(sparse_a.clone(), Value::int(1));
    sparse.insert(PyKey::Int(1), Value::int(2));
    sparse.insert(sparse_b.clone(), Value::int(3));
    sparse.shift_remove(&sparse_a);
    sparse.shift_remove(&sparse_b);
    let mut sparse_copy = sparse.clone();
    let sparse_new = object_key();
    sparse_copy.insert(sparse_new.clone(), Value::int(4));
    assert_eq!(
        sparse_copy.python_hash_candidates(1).unwrap(),
        [PyKey::Int(1), sparse_new]
    );
}

#[test]
fn dict_unicode_conversion_and_sparse_copy_preserve_cpython_layout() {
    let mut dict = PyDict::default();
    let first = PyKey::str_from("first");
    let last = PyKey::str_from("last");
    dict.insert(first.clone(), Value::int(1));
    dict.insert(PyKey::Int(1), Value::int(2));
    dict.insert(last.clone(), Value::int(3));

    let converted = dict.python_hash_index.as_ref().unwrap();
    assert_eq!(converted.indices.len(), 16);
    assert!(!converted.unicode_only);

    dict.shift_remove(&PyKey::Int(1));
    dict.shift_remove(&last);
    let sparse_copy = dict.clone();
    let compacted = sparse_copy.python_hash_index.as_ref().unwrap();
    assert_eq!(compacted.indices.len(), 16);
    assert_eq!(compacted.entry_keys, [Some(first)]);
    assert_eq!(compacted.usable, 9);
    assert!(!compacted.unicode_only);

    let mut presized = PyDict::with_capacity_and_hasher(6, Default::default());
    presized.insert(PyKey::Int(1), Value::int(1));
    presized.shift_remove(&PyKey::Int(1));
    assert!(presized.python_hash_index.is_none());
    assert_eq!(
        presized
            .python_hash_mutation_journal
            .as_ref()
            .unwrap()
            .mutations
            .len(),
        1
    );
    presized.ensure_python_hash_index();
    let presized_shadow = presized.python_hash_index.as_ref().unwrap();
    assert_eq!(presized_shadow.indices.len(), 16);
    assert!(!presized_shadow.unicode_only);

    // BUILD_MAP scans every evaluated key before allocating. A large
    // duplicate-heavy mixed literal therefore starts as a presized General
    // table; it must not infer Unicode from the first repeated string and
    // shrink when the later integer arrives.
    let mut known_general = PyDict::with_capacity_and_known_key_kind(40, Default::default(), false);
    for value in 0..28 {
        known_general.insert(PyKey::str_from("duplicate"), Value::int(value));
    }
    known_general.insert(PyKey::Int(1), Value::int(28));
    known_general.ensure_python_hash_index();
    let known_general_shadow = known_general.python_hash_index.as_ref().unwrap();
    assert_eq!(known_general_shadow.indices.len(), 64);
    assert!(!known_general_shadow.unicode_only);

    let mut small_known_general =
        PyDict::with_capacity_and_known_key_kind(2, Default::default(), false);
    small_known_general.insert(PyKey::str_from("first"), Value::int(1));
    small_known_general.insert(PyKey::Int(1), Value::int(2));
    let small_shadow = small_known_general.python_hash_index.as_ref().unwrap();
    assert_eq!(small_shadow.indices.len(), 16);
    assert!(!small_shadow.unicode_only);

    let exact_fromkeys = PyDict::with_fromkeys_fast_path(1, Default::default(), false);
    assert_eq!(exact_fromkeys.python_hash_initial_table_size, 16);
    assert_eq!(
        exact_fromkeys.python_key_table_kind,
        super::DictKeyTableKind::General
    );
}

#[test]
fn dict_pop_and_retain_keep_precise_probe_metadata() {
    let class = Rc::new(RefCell::new(PyClass::new(
        "Key",
        "Key",
        None,
        IndexMap::new(),
    )));
    let object_key = |hash| PyKey::Object {
        hash,
        value: plain_instance(Rc::clone(&class)),
    };

    let mut popped = PyDict::default();
    popped.insert(object_key(1), Value::int(1));
    popped.insert(object_key(2), Value::int(2));
    popped.insert(PyKey::Int(99), Value::int(3));
    assert_eq!(popped.pop(), Some((PyKey::Int(99), Value::int(3))));
    assert_eq!(popped.live_object_key_count, 2);
    assert!(popped.may_have_object_key());
    assert!(!popped.may_have_non_object_key());
    assert!(popped.has_python_hash_index());

    let first = object_key(1);
    let middle = object_key(1);
    let last = object_key(1);
    let mut retained = PyDict::default();
    retained.insert(first.clone(), Value::int(1));
    retained.insert(middle.clone(), Value::int(2));
    retained.insert(last.clone(), Value::int(3));
    retained.retain(|key, _| key == &middle);
    retained.insert(PyKey::Int(1), Value::int(4));
    let replacement = object_key(1);
    retained.insert(replacement.clone(), Value::int(5));
    assert_eq!(
        retained.python_hash_candidates(1).unwrap(),
        [PyKey::Int(1), middle, replacement],
        "multiple retain deletions must reconstruct both original dummy positions"
    );
}
