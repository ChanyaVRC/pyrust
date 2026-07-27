#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;

    use indexmap::IndexMap;

    use crate::environment::Environment;

    use super::{
        ACTIVE_COLLECTION_MUTATION_STATES, COLLECTION_MUTATION_STATES, CanonicalClassTag,
        FrozenSetKey, InstanceAttrs, MemberSlotId, NANBOX_HEAP_POINTER_ALIGNMENT, PAYLOAD_MASK,
        POOL_B, POOL_OPAQUE, PyClass, PyDict, PyInstance, PyKey, PyModule, PySet,
        STR_CODEPOINT_LEN_SHIFT, STR_CODEPOINT_LEN_TAG, STR_MAX_BYTE_LEN, STR_OWNED_HEADER_SIZE,
        STR_RC_MAX, STR_RC_ONE, STR_SLICE_LAYOUT_SIZE, TAG_LIST_BITS, UserFunction,
        UserFunctionKind, Value, ValueKind, alloc, builtin_type_name, drain_free_list,
        encode_nanbox_heap_pointer, float_bits_as_exact_i64, instance_backing_for_repr, next_fn_id,
        opaque_layout, py_hash_pykey, range_len, str_is_inline_bits, take_thread_exit_drains,
        try_nanbox_heap_pointer_payload, try_nanbox_owned_string_layout, uncached_utf8_byte_offset,
        utf8_codepoint_count,
    };

    thread_local! {
        // Initialize this holder before either free-list key in the regression
        // test below. TLS destructors run in reverse initialization order, so
        // both pools are already gone when these retained Values are dropped.
        static THREAD_EXIT_HELD_POOL_VALUES: RefCell<Option<(Value, Value)>> =
            const { RefCell::new(None) };
    }

    #[test]
    fn value_remains_one_nanboxed_word() {
        assert_eq!(std::mem::size_of::<Value>(), 8);
        assert_eq!(std::mem::size_of::<Value>(), std::mem::size_of::<u64>());
        assert_eq!(std::mem::align_of::<Value>(), std::mem::align_of::<u64>());
    }

    #[test]
    fn range_len_is_exact_across_the_full_i64_domain() {
        assert_eq!(range_len(i64::MIN, i64::MAX, 1), i128::from(u64::MAX));
        assert_eq!(range_len(i64::MAX, i64::MIN, -1), i128::from(u64::MAX));
        assert_eq!(range_len(i64::MAX, i64::MIN, i64::MIN), 2);
        assert_eq!(range_len(i64::MIN, i64::MAX, i64::MAX), 3);
    }

    #[test]
    fn class_value_type_name_uses_its_custom_metaclass() {
        let plain = Rc::new(RefCell::new(PyClass::new(
            "Plain",
            "Plain",
            None,
            IndexMap::new(),
        )));
        assert_eq!(builtin_type_name(&Value::py_class(plain)), "type");

        let metaclass = Rc::new(RefCell::new(PyClass::new(
            "RuntimeMeta",
            "RuntimeMeta",
            None,
            IndexMap::new(),
        )));
        let mut custom = PyClass::new("Custom", "Custom", None, IndexMap::new());
        custom.metatype = Some(Rc::clone(&metaclass));
        let custom = Value::py_class(Rc::new(RefCell::new(custom)));
        assert_eq!(builtin_type_name(&custom), "RuntimeMeta");

        metaclass.borrow_mut().name = "RenamedMeta".to_string();
        assert_eq!(builtin_type_name(&custom), "RenamedMeta");
    }

    #[test]
    fn module_mutation_state_tracks_attribute_namespace_and_provider_identity() {
        let mut module = PyModule::new("provider".to_string(), HashMap::new());
        let state = module.mutation_state();
        assert_eq!(state.version(), 0);

        module.insert_attr("value".to_string(), Value::int(1));
        assert_eq!(state.version(), 1);
        module.insert_attr("value".to_string(), Value::int(2));
        assert_eq!(state.version(), 2);
        module.remove_attr("value");
        assert_eq!(state.version(), 3);
        module.remove_attr("missing");
        assert_eq!(state.version(), 3);

        let clone_state = module.clone().mutation_state();
        assert!(
            !state.same_provider(&clone_state),
            "a cloned PyModule struct is a distinct namespace provider"
        );
    }

    #[test]
    fn filesystem_module_shares_root_globals_without_owning_environment_strongly() {
        let root = Environment::new(None);
        let mut module = PyModule::new("source_module".to_string(), HashMap::new());
        let module_state = module.mutation_state();
        module.attach_filesystem_namespace(&root);

        module.insert_attr("x".to_string(), Value::int(1));
        assert_eq!(root.borrow().values.get("x"), Some(&Value::int(1)));
        let namespace = module.filesystem_namespace().unwrap();
        assert_eq!(
            namespace
                .globals()
                .dict_with(|dict| dict.get(&PyKey::str_from("x")).cloned())
                .flatten(),
            Some(Value::int(1))
        );
        assert!(root.borrow().namespace_globals_require_mirroring());

        root.borrow().expose_namespace_globals();
        assert_eq!(
            module_state.cache_version(),
            None,
            "a Python-visible globals alias must disable module provider caches"
        );

        drop(root);
        assert!(
            namespace.environment().is_none(),
            "PyModule must not retain its loader environment strongly"
        );
        assert_eq!(module.get_attr_value("x"), Some(Value::int(1)));
    }

    #[test]
    fn free_list_drain_releases_the_recorded_block_count() {
        for (layout, count) in [
            (
                std::alloc::Layout::from_size_align(STR_SLICE_LAYOUT_SIZE, 8).unwrap(),
                5,
            ),
            (opaque_layout(), 7),
        ] {
            let state = Cell::new((std::ptr::null_mut(), 0));
            for len in 0..count {
                let ptr = unsafe { alloc(layout) };
                assert!(!ptr.is_null());
                let (head, _) = state.get();
                unsafe { *(ptr as *mut *mut u8) = head };
                state.set((ptr, len + 1));
            }

            assert_eq!(unsafe { drain_free_list(&state, layout) }, count);
            assert_eq!(state.get(), (std::ptr::null_mut(), 0));
        }
    }

    #[test]
    fn thread_exit_drains_both_value_free_lists() {
        let worker = std::thread::spawn(|| {
            let root = Value::string("abcdefghijklmnopqrstuvwxyz");
            let slices = (0..5).map(|_| root.string_slice(2, 12)).collect::<Vec<_>>();
            drop(slices);

            let opaque_values = (0..7)
                .map(|_| Value::dict(PyDict::default()))
                .collect::<Vec<_>>();
            drop(opaque_values);

            assert_eq!(POOL_B.with(|pool| pool.0.get().1), 5);
            assert_eq!(POOL_OPAQUE.with(|pool| pool.0.get().1), 7);
        });
        let worker_id = worker.thread().id();
        worker.join().expect("pool exercise thread");

        let drains = take_thread_exit_drains(worker_id);
        assert_eq!(drains.pool_b, 5);
        assert_eq!(drains.pool_opaque, 7);
    }

    #[test]
    fn thread_exit_values_outliving_free_list_keys_deallocate_directly() {
        std::thread::spawn(|| {
            THREAD_EXIT_HELD_POOL_VALUES.with(|held| {
                let root = Value::string("abcdefghijklmnopqrstuvwxyz");
                let slice = root.string_slice(2, 12);
                let opaque = Value::dict(PyDict::default());
                *held.borrow_mut() = Some((slice, opaque));
            });
            // The values intentionally remain in the older TLS key. At thread
            // exit POOL_OPAQUE and POOL_B are destroyed first; dropping this
            // holder must use the direct-deallocation fallback rather than
            // panic while re-entering an already-destroyed LocalKey.
        })
        .join()
        .expect("values retained past pool destruction must not abort thread exit");
    }

    #[test]
    fn instance_attrs_slot_storage_preserves_plain_inline_footprint() {
        let expected = std::mem::size_of::<Vec<(Rc<str>, Value)>>()
            + std::mem::size_of::<Option<Box<()>>>()
            + std::mem::size_of::<Option<Value>>();
        assert_eq!(std::mem::size_of::<InstanceAttrs>(), expected);
    }

    #[test]
    fn instance_attrs_slot_and_visible_dict_keys_are_independent() {
        let mut attrs = InstanceAttrs::new();
        attrs.insert_slot("x", Value::int(1));
        attrs.insert_inline("x", Value::int(2));

        assert_eq!(attrs.get_slot("x").and_then(Value::as_int), Some(1));
        assert_eq!(attrs.inline_get("x").and_then(Value::as_int), Some(2));
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs.slot_len(), 1);

        assert_eq!(
            attrs.shift_remove_inline("x").and_then(|v| v.as_int()),
            Some(2)
        );
        assert_eq!(attrs.get_slot("x").and_then(Value::as_int), Some(1));
        assert_eq!(
            attrs.shift_remove_slot("x").and_then(|v| v.as_int()),
            Some(1)
        );
        assert_eq!(attrs.slot_len(), 0);
    }

    #[test]
    fn instance_attrs_member_native_and_visible_namespaces_are_independent() {
        let first = MemberSlotId::fresh();
        let second = MemberSlotId::fresh();
        let remapped = MemberSlotId::fresh();
        let mut attrs = InstanceAttrs::new();
        attrs.insert_member_slot(first, Value::int(1));
        attrs.insert_member_slot(second, Value::int(2));
        attrs.insert_slot("x", Value::int(3));
        attrs.insert_inline("x", Value::int(4));

        assert_eq!(
            attrs.get_member_slot(first).and_then(Value::as_int),
            Some(1)
        );
        assert_eq!(
            attrs.get_member_slot(second).and_then(Value::as_int),
            Some(2)
        );
        assert_eq!(attrs.get_slot("x").and_then(Value::as_int), Some(3));
        assert_eq!(attrs.inline_get("x").and_then(Value::as_int), Some(4));

        attrs.remap_member_slots(&[(first, remapped)]);
        assert_eq!(
            attrs.get_member_slot(remapped).and_then(Value::as_int),
            Some(1)
        );
        assert_eq!(
            attrs.get_member_slot(second).and_then(Value::as_int),
            Some(2)
        );
        assert_eq!(attrs.get_slot("x").and_then(Value::as_int), Some(3));
        assert_eq!(attrs.inline_get("x").and_then(Value::as_int), Some(4));
    }

    #[test]
    fn nanbox_heap_pointer_payload_checks_release_boundaries() {
        assert_eq!(try_nanbox_heap_pointer_payload(0), None);
        assert_eq!(try_nanbox_heap_pointer_payload(1), None);
        assert_eq!(
            try_nanbox_heap_pointer_payload(NANBOX_HEAP_POINTER_ALIGNMENT),
            Some(NANBOX_HEAP_POINTER_ALIGNMENT as u64)
        );

        let largest_aligned_payload =
            (PAYLOAD_MASK as usize) & !(NANBOX_HEAP_POINTER_ALIGNMENT - 1);
        assert_eq!(
            try_nanbox_heap_pointer_payload(largest_aligned_payload),
            Some(largest_aligned_payload as u64)
        );
        assert_eq!(
            try_nanbox_heap_pointer_payload((PAYLOAD_MASK as usize).saturating_add(1)),
            None
        );
        assert_eq!(try_nanbox_heap_pointer_payload(usize::MAX), None);
    }

    #[test]
    fn nanbox_heap_pointer_encoder_preserves_every_address_bit() {
        let address = 0x0000_1234_5678_9ab8usize;
        // The helper does not dereference its input; this synthetic pointer
        // tests the exact bit-level round trip without allocating at a fixed
        // virtual address.
        let pointer = std::ptr::without_provenance::<u8>(address);
        let bits = encode_nanbox_heap_pointer(TAG_LIST_BITS, pointer);
        assert_eq!(bits & PAYLOAD_MASK, address as u64);
        assert_eq!(bits & !PAYLOAD_MASK, TAG_LIST_BITS);
    }

    #[test]
    fn nanbox_owned_string_layout_checks_header_boundaries() {
        assert_eq!(
            try_nanbox_owned_string_layout(0).map(|layout| layout.size()),
            Some(STR_OWNED_HEADER_SIZE)
        );
        if usize::BITS > 32 {
            assert_eq!(
                try_nanbox_owned_string_layout(STR_MAX_BYTE_LEN).map(|layout| layout.size()),
                Some(STR_OWNED_HEADER_SIZE + STR_MAX_BYTE_LEN)
            );
        }
        assert!(try_nanbox_owned_string_layout(STR_MAX_BYTE_LEN.saturating_add(1)).is_none());
        assert!(try_nanbox_owned_string_layout(usize::MAX).is_none());
    }

    #[test]
    fn collection_mutation_state_is_shared_and_removed_after_last_iterator() {
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
        COLLECTION_MUTATION_STATES.with(|states| assert_eq!(states.borrow().len(), 1));
        drop(second);
        COLLECTION_MUTATION_STATES.with(|states| assert!(states.borrow().is_empty()));
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
    fn not_implemented_round_trips_through_kind() {
        let v = Value::not_implemented();
        assert!(v.is_not_implemented());
        assert!(matches!(v.kind(), ValueKind::NotImplemented));
        assert!(!v.is_unset());
    }

    #[test]
    fn not_implemented_is_not_classified_as_float() {
        // The NaN-box bit pattern shares the float top16 range; `kind()`
        // must intercept it before the float arm.  Regression guard for the
        // top16-vs-exact-bits caveat noted on `top16`.
        let v = Value::not_implemented();
        assert!(!matches!(v.kind(), ValueKind::Float(_)));
        assert!(!v.is_float());
    }

    #[test]
    fn not_implemented_repr_is_canonical() {
        assert_eq!(Value::not_implemented().repr_raw(), "NotImplemented");
    }

    #[test]
    fn unset_and_not_implemented_are_distinct_patterns() {
        // Both use the positive-NaN sentinel family; they must not collide.
        let unset = Value::unset();
        let nimpl = Value::not_implemented();
        assert!(unset.is_unset());
        assert!(!unset.is_not_implemented());
        assert!(nimpl.is_not_implemented());
        assert!(!nimpl.is_unset());
    }

    #[test]
    fn unset_is_unset_returns_true() {
        // Basic sanity: the sentinel round-trips through is_unset().
        let v = Value::unset();
        assert!(v.is_unset());
        assert!(!v.is_none());
        assert!(!v.is_float());
        assert!(!v.is_not_implemented());
    }

    #[test]
    fn unset_as_some_returns_none() {
        // as_some() is the safe way to probe an unset slot.
        let v = Value::unset();
        assert!(v.as_some().is_none());
    }

    // In debug builds, calling kind() on an unset Value must panic with a
    // diagnostic message so missed CheckLocal emissions surface immediately
    // rather than silently propagating a NaN through the program.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_kind_panics_in_debug() {
        let v = Value::unset();
        let _ = v.kind();
    }

    // In debug builds, truthy() routes through kind() and must also panic.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_truthy_panics_in_debug() {
        let v = Value::unset();
        let _ = v.truthy_raw();
    }

    // The direct NaN-box accessors bypass kind(), so they each need their own
    // tripwire.  The following tests confirm that each one panics (rather than
    // silently returning None / a garbage bit pattern) when called on an unset
    // Value in a debug build.

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_int_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_int();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_str_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_str();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_bool_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_bool();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_int_raw_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_int_raw();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_float_raw_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_float_raw();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_list_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_list();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_tuple_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_tuple();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_opaque_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_opaque();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_dict_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_dict();
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "uninitialised register slot")]
    fn unset_as_set_panics_in_debug() {
        let v = Value::unset();
        let _ = v.as_set();
    }

    // Regression: non-unset values must not be affected by the new guards.

    #[test]
    fn as_int_on_int_value_still_works() {
        assert_eq!(Value::int(42).as_int(), Some(42));
        assert_eq!(Value::none().as_int(), None);
    }

    #[test]
    fn as_str_on_str_value_still_works() {
        assert_eq!(Value::string("hello").as_str(), Some("hello"));
        assert_eq!(Value::none().as_str(), None);
    }

    /// Helper: build a minimal `UserFunction` for kind-wrapping tests.
    fn make_user_function() -> Rc<UserFunction> {
        Rc::new(UserFunction {
            id: next_fn_id(),
            kind: UserFunctionKind::Regular,
            name: Rc::from("f"),
            qualname: Rc::from("f"),
            name_overrides: RefCell::new(None),
            module: RefCell::new(Value::unset()),
            doc: RefCell::new(Value::none()),
            attrs: RefCell::new(None),
            annotations: RefCell::new(Value::unset()),
            defaults_override: RefCell::new(None),
            params: Vec::new(),
            param_binds: Rc::new(Vec::new()),
            memo_positional_parameter_count: 0,
            self_bind: None,
            local_names: Rc::new(HashSet::new()),
            local_index: Rc::new(HashMap::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            env: Environment::new(None),
            is_memo_pure: false,
            precompiled_code: None,
            wrapped_func: None,
        })
    }

    fn extract_user_function(v: &Value) -> Rc<UserFunction> {
        match v.kind() {
            ValueKind::UserFunction(f) => Rc::clone(f),
            _ => panic!("expected UserFunction value"),
        }
    }

    #[test]
    fn with_function_kind_reuses_original_id() {
        // Regression: #303 — `@classmethod` / `@staticmethod` must reuse the
        // original `id` so they share `fn_cache` entries with the undecorated
        // form (and with each other), instead of allocating a fresh `id`
        // every time and doubling cache footprint.
        let original = make_user_function();
        let original_id = original.id;

        let cm = Value::class_method(Rc::clone(&original));
        let sm = Value::static_method(Rc::clone(&original));

        let cm_fn = extract_user_function(&cm);
        let sm_fn = extract_user_function(&sm);

        assert_eq!(cm_fn.id, original_id, "classmethod must reuse id");
        assert_eq!(sm_fn.id, original_id, "staticmethod must reuse id");
        assert_eq!(cm_fn.kind, UserFunctionKind::ClassMethod);
        assert_eq!(sm_fn.kind, UserFunctionKind::StaticMethod);
    }

    #[test]
    fn with_function_kind_idempotent_reuses_rc() {
        // When the requested kind already matches, return the same Rc — no
        // reallocation at all.
        let original = make_user_function();
        let wrapped = Value::with_function_kind(Rc::clone(&original), UserFunctionKind::Regular);
        let wrapped_fn = extract_user_function(&wrapped);
        assert!(
            Rc::ptr_eq(&original, &wrapped_fn),
            "kind-preserving wrap must reuse the original Rc"
        );
    }

    #[test]
    fn builtin_function_cached_and_fresh_identity_are_distinct() {
        let cached_a = Value::builtin_function("__core_builtin_identity_test");
        let cached_b = Value::builtin_function("__core_builtin_identity_test");
        let fresh_a = Value::fresh_builtin_function("__core_builtin_identity_test");
        let fresh_b = Value::fresh_builtin_function("__core_builtin_identity_test");

        assert_eq!(cached_a, cached_b);
        assert_ne!(cached_a, fresh_a);
        assert_ne!(fresh_a, fresh_b);
        assert_eq!(cached_a.value_id(), cached_b.value_id());
        assert_ne!(cached_a.value_id(), fresh_a.value_id());
        assert_ne!(fresh_a.value_id(), fresh_b.value_id());

        let fresh_a_function = fresh_a.as_function_rc().expect("builtin function Rc");
        let fresh_b_function = fresh_b.as_function_rc().expect("builtin function Rc");
        *fresh_a_function.module.borrow_mut() = Value::string("changed");
        assert_eq!(
            fresh_a_function
                .module_value_with_default(Some("registry"))
                .as_str(),
            Some("changed")
        );
        assert_eq!(
            fresh_b_function
                .module_value_with_default(Some("registry"))
                .as_str(),
            Some("registry")
        );
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

    #[test]
    fn unicode_string_metadata_transitions_from_length_to_offsets() {
        let text = Value::string("日本語🙂");
        let initial = unsafe { *text.str_unicode_state_slot() };
        assert_eq!(initial, 0);

        assert_eq!(text.str_codepoint_len(), 4);
        let length_state = unsafe { *text.str_unicode_state_slot() };
        assert_eq!(
            length_state,
            (4 << STR_CODEPOINT_LEN_SHIFT) | STR_CODEPOINT_LEN_TAG
        );

        assert_eq!(text.str_codepoint_len_for_index(), 4);
        assert_eq!(text.str_codepoint_byte_range(2), (6, 9));
        assert_eq!(text.str_codepoint_len_for_index(), 4);
        let offset_state = unsafe { *text.str_unicode_state_slot() };
        assert_ne!(offset_state, 0);
        assert_eq!(offset_state & STR_CODEPOINT_LEN_TAG, 0);
        assert_eq!(text.str_codepoint_len(), 4);
    }

    #[test]
    fn inline_unicode_uses_payload_without_header_access() {
        for text in ["é", "界", "🙂"] {
            let value = Value::string(text);
            assert!(str_is_inline_bits(value.0));
            assert_eq!(value.str_codepoint_len(), 1);
            assert_eq!(value.str_codepoint_len_for_index(), 1);
            assert_eq!(value.str_codepoint_byte_range(0), (0, text.len()));
        }
    }

    #[test]
    fn unicode_codepoint_counter_handles_utf8_and_cesu8() {
        for text in ["", "ascii", "日本語🙂", "aé界🙂z", "αβγδεζηθ"] {
            assert_eq!(
                utf8_codepoint_count(text.as_bytes()),
                text.chars().count(),
                "{text:?}"
            );
        }

        // U+D800 encoded as pyrust's three-byte CESU-8 sequence, followed by
        // ASCII and U+DFFF. Rust rejects these as scalar values, but pyrust
        // intentionally stores them in its str backing.
        let cesu8 = [0xED, 0xA0, 0x80, b'a', 0xED, 0xBF, 0xBF];
        assert_eq!(utf8_codepoint_count(&cesu8), 3);
        for (index, expected) in [0, 3, 4, 7].into_iter().enumerate() {
            assert_eq!(uncached_utf8_byte_offset(&cesu8, index, 3), expected);
        }

        for text in [
            "日本語🙂abcdefαβγδεζηθ",
            "aaaaaaa🙂bbbbbbbb界ccccccccé",
            "🙂🙂🙂🙂🙂🙂🙂🙂🙂",
        ] {
            let offsets: Vec<usize> = text
                .char_indices()
                .map(|(offset, _)| offset)
                .chain(std::iter::once(text.len()))
                .collect();
            for (index, expected) in offsets.into_iter().enumerate() {
                assert_eq!(
                    uncached_utf8_byte_offset(text.as_bytes(), index, text.chars().count()),
                    expected,
                    "{text:?} at {index}"
                );
            }
        }
    }

    #[test]
    fn unicode_slice_cache_survives_root_drop() {
        let root = Value::string("αβγδεζηθ");
        assert_eq!(root.str_codepoint_len_for_index(), 8);
        assert_eq!(root.str_codepoint_len_for_index(), 8);
        assert_eq!(root.str_codepoint_byte_offset(7), 14);
        let view = root.string_slice(2, 14);
        assert_eq!(unsafe { *view.str_unicode_state_slot() }, 0);
        drop(root);

        assert_eq!(view.str_codepoint_len(), 6);
        assert_eq!(view.str_codepoint_byte_range(4), (8, 10));
    }

    #[test]
    fn unicode_append_invalidates_cached_metadata() {
        let mut text = Value::string("αβγδεζηθ");
        assert_eq!(text.str_codepoint_len(), 8);
        assert_ne!(unsafe { *text.str_unicode_state_slot() }, 0);

        assert!(text.str_append_in_place("界🙂"));
        assert_eq!(unsafe { *text.str_unicode_state_slot() }, 0);
        assert_eq!(text.str_codepoint_len(), 10);
        assert_eq!(text.str_codepoint_byte_range(9), (19, 23));
    }

    #[test]
    fn public_string_raw_storage_helpers_reject_invalid_safe_inputs() {
        let scalar = Value::int(1);
        assert!(std::panic::catch_unwind(|| scalar.string_slice(0, 0)).is_err());
        assert!(std::panic::catch_unwind(|| scalar.str_codepoint_byte_offset(0)).is_err());
        assert!(std::panic::catch_unwind(|| scalar.str_codepoint_byte_range(0)).is_err());

        let text = Value::string("éclair");
        assert!(
            std::panic::catch_unwind(|| text.string_slice(0, text.as_str().unwrap().len() + 1))
                .is_err()
        );
        assert!(std::panic::catch_unwind(|| text.string_slice(1, 2)).is_err());

        let mut scalar = Value::int(1);
        assert!(!scalar.str_append_in_place("unsafe"));
        assert_eq!(scalar.as_int(), Some(1));
    }

    #[test]
    fn saturated_string_refcount_preserves_layout_flags() {
        let text = Value::string("long enough for heap storage");
        let header = unsafe { text.str_hdr() as *mut u32 };
        let flags = unsafe { *header } & 0b111;
        unsafe { *header = (STR_RC_MAX << 3) | flags };

        let alias = text.clone();
        assert_eq!(unsafe { *header } & 0b111, flags);
        assert_eq!(unsafe { *header } >> 3, STR_RC_MAX);

        // Restore the exact number of live handles so the test does not leak
        // the allocation whose saturation behaviour it inspected.
        unsafe { *header = (2 * STR_RC_ONE) | flags };
        drop(alias);
        drop(text);
    }

    // ── float_bits_as_exact_i64 boundary tests ───────────────────────────────

    #[test]
    fn float_bits_exact_i64_integer_values() {
        // Ordinary integer-valued floats within i64 range.
        assert_eq!(float_bits_as_exact_i64(1.0f64.to_bits()), Some(1));
        assert_eq!(float_bits_as_exact_i64((-1.0f64).to_bits()), Some(-1));
        assert_eq!(float_bits_as_exact_i64(0.0f64.to_bits()), Some(0));
        assert_eq!(float_bits_as_exact_i64(42.0f64.to_bits()), Some(42));
        assert_eq!(
            float_bits_as_exact_i64(1_000_000_000_000_000.0f64.to_bits()),
            Some(1_000_000_000_000_000)
        );
    }

    #[test]
    fn float_bits_exact_i64_fractional_returns_none() {
        assert_eq!(float_bits_as_exact_i64(0.5f64.to_bits()), None);
        assert_eq!(float_bits_as_exact_i64(1.5f64.to_bits()), None);
        assert_eq!(float_bits_as_exact_i64((-0.1f64).to_bits()), None);
    }

    #[test]
    fn float_bits_exact_i64_non_finite_returns_none() {
        assert_eq!(float_bits_as_exact_i64(f64::INFINITY.to_bits()), None);
        assert_eq!(float_bits_as_exact_i64(f64::NEG_INFINITY.to_bits()), None);
        assert_eq!(float_bits_as_exact_i64(f64::NAN.to_bits()), None);
    }

    #[test]
    fn float_bits_exact_i64_i64_min_is_exact() {
        // i64::MIN as f64 is exactly representable (-2^63); must return Some.
        let min_f = i64::MIN as f64;
        assert_eq!(float_bits_as_exact_i64(min_f.to_bits()), Some(i64::MIN));
    }

    #[test]
    fn float_bits_exact_i64_i64_max_rounds_up() {
        // i64::MAX = 2^63-1 is not exactly representable as f64; the nearest f64
        // is 2^63, which is out of range.  Must return None.
        let max_f = i64::MAX as f64; // rounds up to 2^63
        assert_eq!(float_bits_as_exact_i64(max_f.to_bits()), None);
    }

    #[test]
    fn float_bits_exact_i64_out_of_range_large() {
        // 2^63 is exactly representable but exceeds i64::MAX (= 2^63 - 1).
        let too_big = 9_223_372_036_854_775_808.0f64; // 2^63
        assert_eq!(float_bits_as_exact_i64(too_big.to_bits()), None);
        // 2^64 — clearly out of range.
        let much_bigger = 1.844_674_407_370_955_2e19_f64; // 2^64
        assert_eq!(float_bits_as_exact_i64(much_bigger.to_bits()), None);
        // Negative out-of-range: the f64 immediately below i64::MIN as f64.
        // i64::MIN as f64 is exactly -2^63 (in range); the next f64 towards
        // -inf has integer value -2^63 - 2^10, which is out of i64 range.
        let min_f = i64::MIN as f64;
        // Construct the next representable f64 below min_f by decrementing bits.
        let next_below_bits = min_f.to_bits() + 1; // negative floats: +1 bits → more negative
        let too_small = f64::from_bits(next_below_bits);
        assert!(
            too_small < min_f,
            "sanity: next_below must be more negative than i64::MIN as f64"
        );
        assert_eq!(float_bits_as_exact_i64(next_below_bits), None);
    }

    #[test]
    fn pykey_float_int_cross_type_eq() {
        // Core contract: Float(1.0) == Int(1) and they hash equal.
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        fn key_hash(k: &PyKey) -> u64 {
            let mut h = DefaultHasher::new();
            k.hash(&mut h);
            h.finish()
        }

        let f1 = PyKey::Float(1.0f64.to_bits());
        let i1 = PyKey::Int(1);
        assert_eq!(f1, i1, "Float(1.0) must equal Int(1)");
        assert_eq!(
            key_hash(&f1),
            key_hash(&i1),
            "hash(Float(1.0)) must equal hash(Int(1))"
        );

        // 0.5 must NOT equal 0
        let f05 = PyKey::Float(0.5f64.to_bits());
        let i0 = PyKey::Int(0);
        assert_ne!(f05, i0, "Float(0.5) must not equal Int(0)");

        // Float(-1.0) == Int(-1)
        let fn1 = PyKey::Float((-1.0f64).to_bits());
        let in1 = PyKey::Int(-1);
        assert_eq!(fn1, in1, "Float(-1.0) must equal Int(-1)");
        assert_eq!(key_hash(&fn1), key_hash(&in1), "hash contract for -1.0/-1");

        // Float(1.0) == Bool(true)
        let bt = PyKey::Bool(true);
        assert_eq!(f1, bt, "Float(1.0) must equal Bool(true)");
        assert_eq!(key_hash(&f1), key_hash(&bt), "hash contract for 1.0/true");
    }

    #[test]
    fn pykey_frozenset_hash_is_order_independent_and_cached() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        fn key_hash(key: &PyKey) -> u64 {
            let mut hasher = DefaultHasher::new();
            key.hash(&mut hasher);
            hasher.finish()
        }

        let mut left_items = PySet::default();
        left_items.insert(PyKey::Int(1));
        left_items.insert(PyKey::Int(2));

        let mut right_items = PySet::default();
        right_items.insert(PyKey::Int(2));
        right_items.insert(PyKey::Int(1));

        let left_backing = Rc::new(FrozenSetKey::new(Rc::new(left_items)));
        let right_backing = Rc::new(FrozenSetKey::new(Rc::new(right_items)));
        let left = PyKey::FrozenSet(Rc::clone(&left_backing));
        let right = PyKey::FrozenSet(right_backing);

        assert_eq!(left, right);
        assert_eq!(key_hash(&left), key_hash(&right));
        assert_eq!(py_hash_pykey(&left), py_hash_pykey(&right));

        let PyKey::FrozenSet(shared) = left.clone() else {
            unreachable!();
        };
        assert!(Rc::ptr_eq(&left_backing, &shared));
    }

    fn repr_test_class(
        name: &str,
        tag: Option<CanonicalClassTag>,
        base: Option<Rc<RefCell<PyClass>>>,
        defines_repr: bool,
    ) -> Rc<RefCell<PyClass>> {
        let mut attrs = IndexMap::new();
        if defines_repr {
            attrs.insert(
                "__repr__".to_string(),
                Value::builtin_function("object.__repr__"),
            );
        }
        let mut class = PyClass::new(name, name, base, attrs);
        class.canonical_tag = tag;
        Rc::new(RefCell::new(class))
    }

    fn list_backed_instance(class: Rc<RefCell<PyClass>>) -> Rc<RefCell<PyInstance>> {
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            "__builtin_data__",
            Value::list(vec![Value::int(1), Value::int(2)]),
        );
        Rc::new(RefCell::new(PyInstance { class, attrs }))
    }

    #[test]
    fn backing_repr_reaches_renamed_canonical_base_by_tag() {
        let object = repr_test_class(
            "renamed-object",
            Some(CanonicalClassTag::Object),
            None,
            true,
        );
        let list = repr_test_class(
            "renamed-list",
            Some(CanonicalClassTag::List),
            Some(object),
            true,
        );
        let subclass = repr_test_class("ChangedList", None, Some(list), false);

        let backing = instance_backing_for_repr(&list_backed_instance(subclass));
        assert_eq!(backing.map(|value| value.repr_raw()), Some("[1, 2]".into()));
    }

    #[test]
    fn backing_repr_does_not_trust_spoofed_builtin_or_object_names() {
        let object = repr_test_class("object", Some(CanonicalClassTag::Object), None, true);
        let list = repr_test_class("list", Some(CanonicalClassTag::List), Some(object), true);
        for spoofed_name in ["list", "object"] {
            let spoof = repr_test_class(spoofed_name, None, Some(Rc::clone(&list)), true);
            assert!(
                instance_backing_for_repr(&list_backed_instance(spoof)).is_none(),
                "untagged class named {spoofed_name} must not hide its repr override"
            );
        }
    }
}
