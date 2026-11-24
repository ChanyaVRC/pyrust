#[test]
fn collection_mutation_state_is_shared_and_unobserved_after_last_iterator() {
    let dict = Value::dict(PyDict::default());
    let first = dict.dict_iteration_mutation_state().unwrap();
    let second = dict.dict_iteration_mutation_state().unwrap();
    assert!(first.same_backing(&second));
    ACTIVE_COLLECTION_MUTATION_STATES.with(|active| assert_eq!(active.get(), 1));
    COLLECTION_MUTATION_STATES.with(|states| assert_eq!(states.borrow().len(), 1));

    dict.dict_insert(PyKey::str_from("key"), Value::int(1))
        .unwrap();
    assert_eq!(first.version(), 1);
    assert_eq!(second.version(), 1);

    drop(first);
    ACTIVE_COLLECTION_MUTATION_STATES.with(|active| assert_eq!(active.get(), 1));
    drop(second);
    // The registration is retained for the next iterator over the same
    // backing store, but it is no longer observed: writes must stop paying
    // for generation tracking exactly as they did when it was dropped.
    ACTIVE_COLLECTION_MUTATION_STATES.with(|active| assert_eq!(active.get(), 0));
    COLLECTION_MUTATION_STATES.with(|states| assert_eq!(states.borrow().len(), 1));

    dict.dict_insert(PyKey::str_from("unobserved"), Value::int(2))
        .unwrap();

    let reused = dict.dict_iteration_mutation_state().unwrap();
    assert_eq!(reused.version(), 1);
    ACTIVE_COLLECTION_MUTATION_STATES.with(|active| assert_eq!(active.get(), 1));
    COLLECTION_MUTATION_STATES.with(|states| assert_eq!(states.borrow().len(), 1));

    dict.dict_insert(PyKey::str_from("observed"), Value::int(3))
        .unwrap();
    assert_eq!(reused.version(), 2);
}

#[test]
fn retained_collection_mutation_state_restarts_with_fresh_dict_layout() {
    // Moving from five to six entries while idle distinguishes the retained
    // pre-resize layout from one seeded from the current dictionary.
    let dict_with_len = |len| {
        Value::dict(
            (0..len)
                .map(|value| (PyKey::Int(value), Value::int(value)))
                .collect(),
        )
    };
    let idle_len = 5;
    let watched = PyKey::Int(idle_len);
    let retained_dict = dict_with_len(idle_len);

    let initial = retained_dict.dict_iteration_mutation_state().unwrap();
    drop(initial);
    ACTIVE_COLLECTION_MUTATION_STATES.with(|active| assert_eq!(active.get(), 0));
    COLLECTION_MUTATION_STATES.with(|states| assert_eq!(states.borrow().len(), 1));

    retained_dict
        .dict_insert(watched.clone(), Value::int(idle_len))
        .unwrap();
    let restarted = retained_dict.dict_iteration_mutation_state().unwrap();
    let concurrent = retained_dict.dict_iteration_mutation_state().unwrap();
    assert!(restarted.same_backing(&concurrent));

    let fresh_dict = dict_with_len(idle_len + 1);
    let fresh = fresh_dict.dict_iteration_mutation_state().unwrap();

    let restarted_cursor = restarted.watch_key_reinsertion(&watched);
    let concurrent_cursor = concurrent.watch_key_reinsertion(&watched);
    let fresh_cursor = fresh.watch_key_reinsertion(&watched);
    assert_eq!(restarted_cursor, fresh_cursor);
    assert_eq!(concurrent_cursor, fresh_cursor);

    let restarted_before = restarted.version();
    retained_dict.dict_shift_remove(&watched).unwrap();
    retained_dict
        .dict_insert(watched.clone(), Value::none())
        .unwrap();
    let fresh_before = fresh.version();
    fresh_dict.dict_shift_remove(&watched).unwrap();
    fresh_dict
        .dict_insert(watched.clone(), Value::none())
        .unwrap();

    let fresh_reinserted =
        fresh.key_reinserted_at_or_after_since(&watched, fresh_before, fresh_cursor);
    assert!(fresh_reinserted);
    assert_eq!(
        restarted.key_reinserted_at_or_after_since(&watched, restarted_before, restarted_cursor,),
        fresh_reinserted
    );
    assert_eq!(
        concurrent.key_reinserted_at_or_after_since(&watched, restarted_before, concurrent_cursor,),
        fresh_reinserted
    );

    restarted.unwatch_key_reinsertion(&watched);
    concurrent.unwatch_key_reinsertion(&watched);
    fresh.unwatch_key_reinsertion(&watched);
    drop(restarted);
    drop(concurrent);
    drop(fresh);
    ACTIVE_COLLECTION_MUTATION_STATES.with(|active| assert_eq!(active.get(), 0));
}

#[test]
fn collection_mutation_state_tracks_dict_value_replacements() {
    let key = PyKey::str_from("key");
    let mut items = PyDict::default();
    items.insert(key.clone(), Value::int(1));
    let dict = Value::dict(items);
    let state = dict.dict_iteration_mutation_state().unwrap();
    let before = state.version();

    let replaced = dict.dict_insert(key.clone(), Value::int(2)).unwrap();

    assert_eq!(replaced.and_then(|value| value.as_int()), Some(1));
    assert_eq!(state.version(), before.wrapping_add(1));
    assert_eq!(
        dict.dict_with(|items| items.get(&key).and_then(Value::as_int)),
        Some(Some(2))
    );
}

#[test]
fn dict_move_index_relinks_exact_entry_and_tracks_only_real_moves() {
    let first = PyKey::str_from("first");
    let middle = PyKey::str_from("middle");
    let last = PyKey::str_from("last");
    let mut items = PyDict::default();
    items.insert(first.clone(), Value::int(1));
    items.insert(middle.clone(), Value::int(2));
    items.insert(last.clone(), Value::int(3));
    let dict = Value::dict(items);
    let state = dict.dict_iteration_mutation_state().unwrap();
    let terminal_cursor = state.watch_key_reinsertion(&first);

    let before_version = state.version();
    let before_order = state.entry_order_version();
    assert!(dict.dict_move_index(0, 2).unwrap());
    assert_eq!(state.version(), before_version.wrapping_add(1));
    assert_eq!(state.entry_order_version(), before_order.wrapping_add(1));
    assert_eq!(
        dict.dict_with(|items| items.keys().cloned().collect::<Vec<_>>()),
        Some(vec![middle.clone(), last.clone(), first.clone()])
    );
    assert_eq!(
        dict.dict_with(|items| items.get(&first).and_then(Value::as_int)),
        Some(Some(1))
    );
    assert!(!state.key_reinserted_at_or_after_since(&first, before_version, terminal_cursor));

    let before_noop_version = state.version();
    let before_noop_order = state.entry_order_version();
    assert!(!dict.dict_move_index(2, 2).unwrap());
    assert_eq!(state.version(), before_noop_version);
    assert_eq!(state.entry_order_version(), before_noop_order);

    assert!(dict.dict_move_index(2, 0).unwrap());
    assert_eq!(
        dict.dict_with(|items| items.keys().cloned().collect::<Vec<_>>()),
        Some(vec![first, middle, last])
    );
    assert_eq!(
        state.entry_order_version(),
        before_noop_order.wrapping_add(1)
    );
}

#[test]
fn set_mutation_state_advances_only_for_structural_changes() {
    let existing = PyKey::str_from("existing");
    let inserted = PyKey::str_from("inserted");
    let extended = PyKey::str_from("extended");
    let missing = PyKey::str_from("missing");
    let mut items = PySet::default();
    items.insert(existing.clone());
    let set = Value::set(items);
    let state = set.set_iteration_mutation_state().unwrap();
    let initial = state.version();

    assert!(!set.set_add(existing.clone()).unwrap());
    assert!(!set.set_discard(&missing).unwrap());
    set.set_extend(vec![existing.clone(), existing.clone()])
        .unwrap();
    assert_eq!(state.version(), initial);

    assert!(set.set_add(inserted.clone()).unwrap());
    assert_eq!(state.version(), initial.wrapping_add(1));

    set.set_extend(vec![inserted, extended]).unwrap();
    assert_eq!(state.version(), initial.wrapping_add(2));

    assert!(set.set_discard(&existing).unwrap());
    assert_eq!(state.version(), initial.wrapping_add(3));

    set.set_clear().unwrap();
    assert_eq!(state.version(), initial.wrapping_add(4));
    set.set_clear().unwrap();
    assert_eq!(state.version(), initial.wrapping_add(4));
}

#[test]
fn collection_mutation_state_tracks_only_watched_key_removals() {
    let watched = PyKey::str_from("watched");
    let temporary = PyKey::str_from("temporary");
    let mut items = PyDict::default();
    items.insert(watched.clone(), Value::int(1));
    let dict = Value::dict(items);
    let state = dict.dict_iteration_mutation_state().unwrap();

    let cursor = state.watch_key_reinsertion(&watched);
    let before_temporary = state.version();
    dict.dict_insert(temporary.clone(), Value::int(2)).unwrap();
    dict.dict_shift_remove(&temporary).unwrap();
    assert!(!state.key_reinserted_at_or_after_since(&watched, before_temporary, cursor));

    let before_watched = state.version();
    dict.dict_shift_remove(&watched).unwrap();
    dict.dict_insert(watched.clone(), Value::int(3)).unwrap();
    assert!(state.key_reinserted_at_or_after_since(&watched, before_watched, cursor));

    state.unwatch_key_reinsertion(&watched);
    assert!(!state.key_reinserted_at_or_after_since(&watched, before_watched, cursor));
}

#[test]
fn collection_mutation_state_models_compact_dict_resize_boundaries() {
    for (len, visible_after_cursor) in [
        (1, true),
        (5, false),
        (6, true),
        (10, false),
        (11, true),
        (21, false),
        (22, true),
    ] {
        let mut items = PyDict::default();
        for value in 0..len {
            items.insert(PyKey::Int(value), Value::int(value));
        }
        let final_key = PyKey::Int(len - 1);
        let dict = Value::dict(items);
        let state = dict.dict_iteration_mutation_state().unwrap();
        let cursor = state.watch_key_reinsertion(&final_key);
        let before = state.version();
        dict.dict_shift_remove(&final_key).unwrap();
        dict.dict_insert(final_key.clone(), Value::none()).unwrap();
        assert_eq!(
            state.key_reinserted_at_or_after_since(&final_key, before, cursor),
            visible_after_cursor,
            "len={len}"
        );
    }
}

#[test]
fn dict_shift_remove_many_preserves_order_and_tracks_one_bulk_mutation() {
    let keys = [
        PyKey::str_from("first"),
        PyKey::str_from("remove-a"),
        PyKey::str_from("middle"),
        PyKey::str_from("remove-b"),
        PyKey::str_from("last"),
        PyKey::str_from("tail"),
    ];
    let mut items = PyDict::default();
    for (index, key) in keys.iter().cloned().enumerate() {
        items.insert(key, Value::int(index as i64));
    }
    let dict = Value::dict(items);
    let state = dict.dict_iteration_mutation_state().unwrap();
    let watched = keys[3].clone();
    let terminal_cursor = state.watch_key_reinsertion(&watched);
    let before = state.version();

    let removed = dict
        .dict_shift_remove_many(vec![keys[1].clone(), watched.clone()])
        .unwrap();
    assert_eq!(removed, 2);
    assert_eq!(state.version(), before.wrapping_add(1));
    let survivors = dict
        .dict_with(|items| items.keys().cloned().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(
        survivors,
        vec![
            keys[0].clone(),
            keys[2].clone(),
            keys[4].clone(),
            keys[5].clone(),
        ]
    );

    dict.dict_insert(watched.clone(), Value::none()).unwrap();
    assert!(state.key_reinserted_at_or_after_since(&watched, before, terminal_cursor));

    let after_reinsert = state.version();
    assert_eq!(dict.dict_shift_remove_many(Vec::new()).unwrap(), 0);
    assert_eq!(state.version(), after_reinsert);
}

#[test]
fn list_clone_shares_storage_for_bound_method_mutation() {
    // Regression test for #305.  `Value::clone` on a list must produce an
    // alias of the same backing storage, so that captured bound methods
    // (`m = lst.append; m(4)`) and simple aliasing (`b = a; b.append(x)`)
    // mutate the original list — matching CPython's reference semantics.
    let a = Value::list(vec![Value::int(1)]);
    let b = a.clone();
    b.list_push(Value::int(2))
        .expect("clone must still be a list");
    let items = a.as_list().expect("original must still be a list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_int(), Some(1));
    assert_eq!(items[1].as_int(), Some(2));
}

#[test]
#[should_panic(expected = "already borrowed")]
fn as_list_holds_read_guard_across_aliases() {
    let list = Value::list(vec![Value::int(1)]);
    let alias = list.clone();
    let _read = list.as_list().expect("list");
    alias.list_push(Value::int(2)).expect("list");
}

#[test]
#[should_panic(expected = "already borrowed")]
fn as_dict_holds_read_guard_across_aliases() {
    let dict = Value::dict(PyDict::default());
    let alias = dict.clone();
    let _read = dict.as_dict().expect("dict");
    alias
        .dict_insert(PyKey::str_from("key"), Value::int(1))
        .expect("dict");
}

#[test]
#[should_panic(expected = "already borrowed")]
fn as_set_holds_read_guard_across_aliases() {
    let set = Value::set(PySet::default());
    let alias = set.clone();
    let _read = set.as_set().expect("set");
    alias.set_add(PyKey::Int(1)).expect("set");
}

#[test]
fn list_clone_preserves_identity() {
    // `id(b) == id(a)` after `b = a` for list, matching CPython.
    let a = Value::list(vec![Value::int(1), Value::int(2)]);
    let b = a.clone();
    assert_eq!(a.value_id(), b.value_id());

    // Distinct list literals must NOT share identity.
    let c = Value::list(vec![Value::int(1), Value::int(2)]);
    assert_ne!(a.value_id(), c.value_id());
}

#[test]
fn opaque_clone_reuses_refcounted_slot() {
    let original = Value::complex(1.25, -2.5);
    let slot = unsafe { &*original.opaque_slot_ptr() };
    assert_eq!(slot.strong.get(), 1);

    let alias = original.clone();
    assert_eq!(original.0, alias.0, "clone must reuse the opaque slab");
    assert_eq!(slot.strong.get(), 2);

    drop(original);
    assert_eq!(
        unsafe { &*alias.opaque_slot_ptr() }.strong.get(),
        1,
        "dropping one alias must keep the shared payload alive"
    );
    assert!(matches!(
        alias.kind(),
        ValueKind::Complex(re, im) if re == 1.25 && im == -2.5
    ));
}

#[test]
fn set_clone_shares_storage() {
    // Same Rc-sharing invariant as list, exercised through set's mutating
    // accessor.
    let a = Value::set({
        let mut s = PySet::default();
        s.insert(PyKey::Int(1));
        s
    });
    let b = a.clone();
    b.set_add(PyKey::Int(2)).expect("clone must still be a set");
    let items = a.as_set().expect("original must still be a set");
    assert!(items.contains(&PyKey::Int(1)));
    assert!(items.contains(&PyKey::Int(2)));
    assert_eq!(items.len(), 2);
}

#[test]
fn set_mutation_through_original_visible_in_clone() {
    // Symmetric counterpart to `set_clone_shares_storage`: mutate via
    // the original Value, the clone (alias) sees it.  This pins both
    // directions of the Rc-shared backing post-#305.
    let a = Value::set({
        let mut s = PySet::default();
        s.insert(PyKey::Int(1));
        s
    });
    let b = a.clone();
    a.set_add(PyKey::Int(2))
        .expect("original must still be a set");
    let items = b.as_set().expect("clone must still be a set");
    assert!(items.contains(&PyKey::Int(1)));
    assert!(items.contains(&PyKey::Int(2)));
    assert_eq!(items.len(), 2);
}

#[test]
fn set_clone_preserves_identity() {
    let a = Value::set({
        let mut s = PySet::default();
        s.insert(PyKey::Int(1));
        s
    });
    let b = a.clone();
    assert_eq!(a.value_id(), b.value_id());

    let c = Value::set({
        let mut s = PySet::default();
        s.insert(PyKey::Int(1));
        s
    });
    assert_ne!(a.value_id(), c.value_id());
}

#[test]
fn dict_clone_preserves_identity() {
    // Dict already used `Rc<RefCell<...>>` shared storage; #305 added an
    // `id()` surface for it via `value_id()`.  Pin the invariant.
    let a = Value::dict({
        let mut m = PyDict::default();
        m.insert(PyKey::str_from("k"), Value::int(1));
        m
    });
    let b = a.clone();
    assert_eq!(a.value_id(), b.value_id());

    let c = Value::dict({
        let mut m = PyDict::default();
        m.insert(PyKey::str_from("k"), Value::int(1));
        m
    });
    assert_ne!(a.value_id(), c.value_id());
}
