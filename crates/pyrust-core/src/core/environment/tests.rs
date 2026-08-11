#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ptr::NonNull;
    use std::rc::Rc;

    use super::{Environment, namespace_alias_tracking_active};
    use crate::ModuleAttrs;
    use crate::object_model::{PyDict, PyKey, PyModule, Value, ValueKind};

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn environment_inline_size_stays_compact() {
        assert_eq!(std::mem::size_of::<Environment>(), 112);
    }

    #[test]
    fn generator_frame_environment_owner_is_weak_and_resettable() {
        let env = Environment::new(None);
        let value = Value::generator(Box::new(()));
        let owner = match value.kind() {
            ValueKind::Generator(owner) => Rc::clone(owner),
            _ => unreachable!("generator constructor returned another value kind"),
        };
        let strong_before = Rc::strong_count(&owner);

        env.borrow_mut().bind_generator_frame_owner(&owner);
        assert_eq!(Rc::strong_count(&owner), strong_before);
        assert!(Rc::ptr_eq(
            &env.borrow().generator_frame_owner().unwrap(),
            &owner
        ));
        assert!(env.borrow().clone().generator_frame_owner().is_none());

        env.borrow_mut().reset_for_reuse(None);
        assert!(env.borrow().generator_frame_owner().is_none());

        env.borrow_mut().bind_generator_frame_owner(&owner);
        drop(value);
        drop(owner);
        assert!(env.borrow().generator_frame_owner().is_none());
    }

    #[test]
    fn root_namespace_ids_are_distinct_and_children_inherit() {
        let first_root = Environment::new(None);
        let second_root = Environment::new(None);
        let child = Environment::new(Some(first_root.clone()));

        let first_id = first_root.borrow().namespace_id();
        assert_ne!(first_id, second_root.borrow().namespace_id());
        assert_eq!(child.borrow().namespace_id(), first_id);
    }

    #[test]
    fn root_namespace_generations_and_exposure_are_shared_by_children() {
        let root = Environment::new(None);
        let child = Environment::new(Some(root.clone()));

        child.borrow().bump_namespace_environment_version();
        assert_eq!(root.borrow().namespace_cache_snapshot().1, 1);

        root.borrow().bump_namespace_structure_version();
        assert_eq!(child.borrow().namespace_cache_snapshot().1, 2);
        assert_eq!(child.borrow().namespace_cache_snapshot().2, 1);

        let globals = child.borrow().expose_namespace_globals();
        assert!(root.borrow().namespace_cache_disabled());
        assert!(root.borrow().namespace_globals_exposed());
        assert!(globals.is_dict());
    }

    #[test]
    fn namespace_module_write_does_not_materialize_unexposed_globals() {
        let root = Environment::new(None);

        assert!(
            root.borrow()
                .root_namespace
                .globals_provider
                .borrow()
                .is_none()
        );
        assert!(
            root.borrow().prepare_namespace_module_write(0b11).is_none(),
            "ordinary script writes have no dictionary mirror before exposure"
        );
        assert!(
            root.borrow()
                .root_namespace
                .globals_provider
                .borrow()
                .is_none(),
            "the hot write path must preserve lazy globals allocation"
        );
        assert_eq!(
            root.borrow().namespace_cache_snapshot().1,
            0,
            "a write with no published cache interest needs no generation bump"
        );

        root.borrow()
            .register_namespace_fallback_cache_interest(0b11);
        assert!(
            root.borrow().prepare_namespace_module_write(0b11).is_none(),
            "cache invalidation still must not materialize a globals provider"
        );
        let snapshot = root.borrow().namespace_cache_snapshot();
        assert_eq!(
            snapshot.1, 1,
            "the structure bump advances the environment generation once"
        );
        assert_eq!(snapshot.2, 1, "an interested fallback cache is invalidated");
    }

    #[test]
    fn namespace_module_write_advances_environment_only_for_interested_names() {
        let root = Environment::new(None);

        root.borrow()
            .register_namespace_value_cache_interest(0b0011);
        assert!(
            root.borrow()
                .prepare_namespace_module_write(0b1100)
                .is_none()
        );
        assert_eq!(
            root.borrow().namespace_cache_snapshot().1,
            0,
            "an unrelated module write must preserve value-cache generations"
        );

        assert!(
            root.borrow()
                .prepare_namespace_module_write(0b0011)
                .is_none()
        );
        let snapshot = root.borrow().namespace_cache_snapshot();
        assert_eq!(snapshot.1, 1);
        assert_eq!(
            snapshot.2, 0,
            "environment-backed entries do not depend on structure generation"
        );
    }

    #[test]
    fn namespace_module_write_advances_fallback_generations_once() {
        let root = Environment::new(None);
        root.borrow()
            .register_namespace_value_cache_interest(0b0011);
        root.borrow()
            .register_namespace_fallback_cache_interest(0b0011);

        for expected in 1..=2 {
            assert!(
                root.borrow()
                    .prepare_namespace_module_write(0b0011)
                    .is_none()
            );
            let snapshot = root.borrow().namespace_cache_snapshot();
            assert_eq!(
                snapshot.1, expected,
                "the structure bump must not be preceded by a second environment bump"
            );
            assert_eq!(snapshot.2, expected);
        }
    }

    #[test]
    fn exposed_namespace_storage_updates_and_releases_fastlocal_mirror() {
        let root = Environment::new(None);
        let globals = root.borrow().expose_namespace_globals();
        let local_index = Rc::new(HashMap::from([("value".to_string(), 0_u32)]));
        let mut regs = vec![Value::int(1)];
        let regs_ptr = NonNull::new(regs.as_mut_ptr()).expect("Vec storage is non-null");
        let guard = unsafe {
            root.borrow()
                .register_namespace_fastlocals(regs_ptr, regs.len(), &local_index)
        };

        // First-exposure population is intentionally silent until the caller
        // has snapshotted every register into the dictionary.
        globals
            .dict_insert(PyKey::str_from("value"), Value::int(2))
            .unwrap();
        assert_eq!(regs[0].as_int(), Some(1));

        root.borrow().activate_namespace_globals_alias(&globals);
        globals
            .dict_insert(PyKey::str_from("value"), Value::int(3))
            .unwrap();
        assert_eq!(regs[0].as_int(), Some(3));
        globals
            .dict_shift_remove(&PyKey::str_from("value"))
            .unwrap();
        assert!(regs[0].is_unset());

        drop(guard);
        globals
            .dict_insert(PyKey::str_from("value"), Value::int(4))
            .unwrap();
        assert!(regs[0].is_unset(), "a dead mirror must not be written");
    }

    #[test]
    fn nested_fastlocal_snapshot_prefers_inner_and_controlled_writes_reach_siblings() {
        let root = Environment::new(None);
        let local_index = Rc::new(HashMap::from([("value".to_string(), 0_u32)]));
        let mut outer = vec![Value::int(1)];
        let outer_ptr = NonNull::new(outer.as_mut_ptr()).expect("Vec storage is non-null");
        let _outer_guard = unsafe {
            root.borrow()
                .register_namespace_fastlocals(outer_ptr, outer.len(), &local_index)
        };
        let outer_source = root
            .borrow()
            .namespace_fastlocal_cache_source("value")
            .expect("outer locator");
        assert_eq!(outer_source.value.as_int(), Some(1));
        assert_eq!(
            root.borrow()
                .namespace_fastlocal_cached_value(
                    outer_source.mirror_epoch,
                    &outer_source.local_index,
                    outer_source.register,
                )
                .and_then(|value| value.as_int()),
            Some(1)
        );

        let mut inner = vec![Value::unset()];
        let inner_ptr = NonNull::new(inner.as_mut_ptr()).expect("Vec storage is non-null");
        let inner_guard = unsafe {
            root.borrow()
                .register_namespace_fastlocals(inner_ptr, inner.len(), &local_index)
        };
        assert!(
            root.borrow()
                .namespace_fastlocal_cached_value(
                    outer_source.mirror_epoch,
                    &outer_source.local_index,
                    outer_source.register,
                )
                .is_none(),
            "pushing a mirror invalidates the old locator epoch"
        );
        let nested_source = root
            .borrow()
            .namespace_fastlocal_cache_source("value")
            .expect("an unset inner mirror falls through to the outer binding");
        assert_eq!(
            root.borrow()
                .namespace_fastlocal_cached_value(
                    nested_source.mirror_epoch,
                    &nested_source.local_index,
                    nested_source.register,
                )
                .and_then(|value| value.as_int()),
            Some(1),
            "a same-layout unset inner slot must not hide the bound outer slot"
        );
        inner[0] = Value::int(2);

        let snapshot: HashMap<_, _> = root
            .borrow()
            .namespace_fastlocals_snapshot()
            .into_iter()
            .collect();
        assert_eq!(snapshot["value"].as_int(), Some(2));
        assert_eq!(
            root.borrow()
                .namespace_fastlocal_value("value")
                .and_then(|value| value.as_int()),
            Some(2)
        );

        root.borrow().synchronize_namespace_fastlocal_binding(
            "value",
            &Value::int(3),
            Some(inner_ptr),
        );
        assert_eq!(outer[0].as_int(), Some(3));
        assert_eq!(
            inner[0].as_int(),
            Some(2),
            "the originating register file is not redundantly overwritten"
        );
        drop(inner_guard);
        assert!(
            root.borrow()
                .namespace_fastlocal_cached_value(
                    nested_source.mirror_epoch,
                    &nested_source.local_index,
                    nested_source.register,
                )
                .is_none(),
            "dropping a mirror invalidates every locator from that stack epoch"
        );
    }

    #[test]
    fn keyed_alias_mutation_updates_only_the_named_fastlocal() {
        let root = Environment::new(None);
        let globals = root.borrow().expose_namespace_globals();
        let local_index = Rc::new(HashMap::from([
            ("changed".to_string(), 0_u32),
            ("untouched".to_string(), 1_u32),
        ]));
        let mut regs = vec![Value::int(1), Value::int(9)];
        let regs_ptr = NonNull::new(regs.as_mut_ptr()).expect("Vec storage is non-null");
        let _guard = unsafe {
            root.borrow()
                .register_namespace_fastlocals(regs_ptr, regs.len(), &local_index)
        };
        root.borrow().activate_namespace_globals_alias(&globals);

        globals
            .dict_insert(PyKey::str_from("changed"), Value::int(2))
            .unwrap();
        assert_eq!(regs[0].as_int(), Some(2));
        assert_eq!(
            regs[1].as_int(),
            Some(9),
            "a single-key write must not scan or clear unrelated locals"
        );
    }

    #[test]
    fn filesystem_module_writes_advance_the_provider_generation() {
        let root = Environment::new(None);
        let module = PyModule::new("source_module".to_string(), ModuleAttrs::default());
        let mutation = module.mutation_state();
        let _globals = root.borrow().namespace_globals();
        root.borrow()
            .configure_filesystem_module_namespace(mutation.clone());

        assert_eq!(mutation.version(), 0);
        assert!(
            root.borrow().prepare_namespace_module_write(0b11).is_some(),
            "source-backed module writes mirror into the module dictionary"
        );
        assert_eq!(mutation.version(), 1);
        root.borrow().bump_filesystem_module_mutation();
        assert_eq!(mutation.version(), 2);
    }

    #[test]
    fn dropping_an_alias_owner_removes_the_thread_local_fast_gate() {
        assert!(!namespace_alias_tracking_active());
        let globals = Value::dict(PyDict::default());
        {
            let root = Environment::new(None);
            root.borrow()
                .configure_explicit_namespace(globals.clone(), None);
            assert!(namespace_alias_tracking_active());
        }
        assert!(
            !namespace_alias_tracking_active(),
            "dead explicit roots must not tax unrelated future dict mutations"
        );
    }

    #[test]
    fn explicit_separate_locals_are_the_authoritative_fastlocal_provider() {
        let root = Environment::new(None);
        let globals = Value::dict(PyDict::default());
        let locals = Value::dict(PyDict::default());
        root.borrow()
            .configure_explicit_namespace(globals.clone(), Some(locals.clone()));

        let local_index = Rc::new(HashMap::from([("value".to_string(), 0_u32)]));
        let mut regs = vec![Value::int(1)];
        let regs_ptr = NonNull::new(regs.as_mut_ptr()).expect("Vec storage is non-null");
        let _guard = unsafe {
            root.borrow()
                .register_namespace_fastlocals(regs_ptr, regs.len(), &local_index)
        };

        globals
            .dict_insert(PyKey::str_from("value"), Value::int(2))
            .unwrap();
        assert_eq!(
            regs[0].as_int(),
            Some(1),
            "separate globals do not replace ordinary module-code locals"
        );
        locals
            .dict_insert(PyKey::str_from("value"), Value::int(3))
            .unwrap();
        assert_eq!(regs[0].as_int(), Some(3));
    }

    #[test]
    fn pooled_environment_rebinds_to_the_new_parent_namespace() {
        let first_root = Environment::new(None);
        let second_root = Environment::new(None);
        let pooled = Environment::new(Some(first_root.clone()));
        let first_id = pooled.borrow().namespace_id();

        pooled
            .borrow_mut()
            .reset_for_reuse(Some(second_root.clone()));

        assert_ne!(pooled.borrow().namespace_id(), first_id);
        assert_eq!(
            pooled.borrow().namespace_id(),
            second_root.borrow().namespace_id()
        );
    }

    #[test]
    fn materialization_snapshot_follows_binding_order() {
        // Issue #2903: the namespace dictionary this snapshot fills is a
        // Python dict, so its order is the module's binding order.
        let root = Environment::new(None);
        {
            let mut root = root.borrow_mut();
            root.values.insert("__name__", Value::string("__main__"));
            root.values.insert("captured", Value::int(1));
            root.values.insert("from_function", Value::int(2));
        }
        // Registers are allocated in source binding order; `captured` is a
        // module-level cell variable whose value lives in the env instead.
        let local_index = Rc::new(HashMap::from([
            ("imported".to_string(), 0_u32),
            ("captured".to_string(), 1_u32),
            ("later".to_string(), 2_u32),
            ("never_bound".to_string(), 3_u32),
        ]));
        let mut regs = vec![
            Value::int(10),
            Value::unset(),
            Value::int(30),
            Value::unset(),
        ];
        let regs_ptr = NonNull::new(regs.as_mut_ptr()).expect("Vec storage is non-null");
        let _guard = unsafe {
            root.borrow()
                .register_namespace_fastlocals(regs_ptr, regs.len(), &local_index)
        };

        let names: Vec<String> = root
            .borrow()
            .namespace_materialization_snapshot()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            names,
            vec![
                // env-only bindings keep their own insertion order,
                "__name__",
                "from_function",
                // then every declared name in register order; an unset
                // register with an env binding keeps its declared position,
                // and a never-bound register contributes no key.
                "imported",
                "captured",
                "later",
            ]
        );
    }

    #[test]
    fn env_values_keep_insertion_order_across_promotion() {
        let mut values = super::EnvValues::new();
        values.insert("first", Value::int(1));
        values.insert("second", Value::int(2));
        // Past ENV_INLINE_CAP the store promotes to its hashed form, which
        // must stay insertion ordered (issue #2903).
        values.insert("third", Value::int(3));
        values.insert("fourth", Value::int(4));
        // Rebinding does not move a key; removing and re-inserting appends.
        values.insert("first", Value::int(11));
        values.remove("second");
        values.insert("second", Value::int(22));

        let names: Vec<&str> = values.keys().collect();
        assert_eq!(names, vec!["first", "third", "fourth", "second"]);
        assert_eq!(values.get("first").and_then(Value::as_int), Some(11));
        assert_eq!(values.get("second").and_then(Value::as_int), Some(22));
    }

    #[test]
    fn root_namespace_generations_saturate() {
        let root = Environment::new(None);
        {
            let root = root.borrow();
            root.root_namespace.environment_version.set(u64::MAX - 1);
            root.root_namespace.structure_version.set(u64::MAX - 1);
            root.bump_namespace_structure_version();
            root.bump_namespace_structure_version();
        }
        let (_, environment, structure, _, _) = root.borrow().namespace_cache_snapshot();
        assert_eq!(environment, u64::MAX);
        assert_eq!(structure, u64::MAX);
    }
}
