thread_local! {
    // Initialize this holder before either free-list key in the regression
    // test below. TLS destructors run in reverse initialization order, so
    // both pools are already gone when these retained Values are dropped.
    static THREAD_EXIT_HELD_POOL_VALUES: RefCell<Option<(Value, Value)>> =
        const { RefCell::new(None) };
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

struct DiagnosticNameOps;

impl BuiltinTypeOps for DiagnosticNameOps {
    fn type_name(&self) -> &'static str {
        "native"
    }

    fn display_error_name(&self) -> &'static str {
        "module.native"
    }
}

static DIAGNOSTIC_NAME_OPS: DiagnosticNameOps = DiagnosticNameOps;

fn plain_instance(class: Rc<RefCell<PyClass>>) -> Value {
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })))
}

#[test]
fn error_type_name_is_exact_metadata_not_visible_or_inherited() {
    let mut native = PyClass::new("Native", "Native", None, IndexMap::new());
    native.error_name = Some("module.Native");
    let native = Rc::new(RefCell::new(native));
    let exact = plain_instance(Rc::clone(&native));
    assert_eq!(builtin_type_name(&exact), "Native");
    assert_eq!(error_type_name(&exact), "module.Native");

    let subclass = plain_instance(Rc::new(RefCell::new(PyClass::new(
        "Child",
        "Child",
        Some(native),
        IndexMap::new(),
    ))));
    assert_eq!(builtin_type_name(&subclass), "Child");
    assert_eq!(error_type_name(&subclass), "Child");

    let lookalike = plain_instance(Rc::new(RefCell::new(PyClass::new(
        "Native",
        "Native",
        None,
        IndexMap::new(),
    ))));
    assert_eq!(error_type_name(&lookalike), "Native");
}

#[test]
fn builtin_object_error_name_does_not_change_visible_type_name() {
    let value = Value::builtin_object(&DIAGNOSTIC_NAME_OPS, Box::new(()));
    assert_eq!(builtin_type_name(&value), "native");
    assert_eq!(error_type_name(&value), "module.native");
}

#[test]
fn module_mutation_state_tracks_attribute_namespace_and_provider_identity() {
    let mut module = PyModule::new("provider".to_string(), ModuleAttrs::default());
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

/// `vars(module)` is a Python dict, so the direct `attrs` storage backing a
/// built-in module must behave like one: insertion ordered, rebinding keeps
/// a name's position, deleting shifts rather than swaps, and re-inserting a
/// deleted name appends (issue #2918).
#[test]
fn module_attrs_preserve_insertion_order_like_a_dict() {
    let mut module = PyModule::new("ordered".to_string(), ModuleAttrs::default());
    for name in ["zebra", "alpha", "mike", "delta"] {
        module.insert_attr(name.to_string(), Value::int(1));
    }
    let names =
        |module: &PyModule| -> Vec<String> { module.attrs_snapshot().keys().cloned().collect() };
    assert_eq!(names(&module), ["zebra", "alpha", "mike", "delta"]);

    // Rebinding an existing name keeps its position.
    module.insert_attr("alpha".to_string(), Value::int(2));
    assert_eq!(names(&module), ["zebra", "alpha", "mike", "delta"]);

    // Deleting must not swap the last entry into the hole.
    module.remove_attr("alpha");
    assert_eq!(names(&module), ["zebra", "mike", "delta"]);

    // Re-inserting a removed name appends it, exactly like `dict`.
    module.insert_attr("alpha".to_string(), Value::int(3));
    assert_eq!(names(&module), ["zebra", "mike", "delta", "alpha"]);

    // Moving the namespace into a live dict preserves that order.
    module.attach_live_namespace();
    assert_eq!(names(&module), ["zebra", "mike", "delta", "alpha"]);
    module.remove_attr("mike");
    assert_eq!(names(&module), ["zebra", "delta", "alpha"]);
}

thread_local! {
    static LAZY_MODULE_RESOLUTIONS: Cell<usize> = const { Cell::new(0) };
}

fn resolve_lazy_module_fixture(name: &str) -> Option<Value> {
    let value = match name {
        "lazy_a" => Value::int(10),
        "lazy_b" => Value::int(20),
        _ => return None,
    };
    LAZY_MODULE_RESOLUTIONS.with(|count| count.set(count.get() + 1));
    Some(value)
}

/// Startup order of the fixture namespace: `first`, `lazy_a`, `second`, `lazy_b`.
fn visit_lazy_module_fixture(visit: LazyModuleSlotVisitor<'_>) {
    visit("first", None);
    visit("lazy_a", resolve_lazy_module_fixture("lazy_a"));
    visit("second", None);
    visit("lazy_b", resolve_lazy_module_fixture("lazy_b"));
}

fn lazy_module_fixture() -> PyModule {
    let mut attrs = ModuleAttrs::default();
    attrs.insert("first".to_string(), Value::int(1));
    attrs.insert("second".to_string(), Value::int(2));
    PyModule::with_lazy_attrs(
        "lazy".to_string(),
        attrs,
        LazyModuleAttrSource {
            resolve: resolve_lazy_module_fixture,
            visit_slots: visit_lazy_module_fixture,
        },
    )
}

fn module_attr_names(module: &PyModule) -> Vec<String> {
    module.attrs_snapshot().keys().cloned().collect()
}

/// Issue #3294: a deferred attribute is resolved per read and never stored,
/// so reading it neither allocates the other deferred slots nor invalidates
/// caches keyed on the provider version.
#[test]
fn lazy_module_attr_reads_resolve_on_demand_without_mutating_the_namespace() {
    let module = lazy_module_fixture();
    let state = module.mutation_state();
    let resolved_before = LAZY_MODULE_RESOLUTIONS.with(Cell::get);
    assert!(module.has_pending_lazy_attrs());

    assert_eq!(module.get_attr_value("first"), Some(Value::int(1)));
    assert_eq!(module.get_attr_value("missing"), None);
    assert_eq!(LAZY_MODULE_RESOLUTIONS.with(Cell::get), resolved_before);

    assert_eq!(module.get_attr_value("lazy_b"), Some(Value::int(20)));
    assert_eq!(LAZY_MODULE_RESOLUTIONS.with(Cell::get), resolved_before + 1);
    assert!(module.has_pending_lazy_attrs());
    assert!(!module.attrs.contains_key("lazy_b"));
    assert_eq!(state.version(), 0);
}

/// Deferred slots keep their startup position no matter which were read
/// first or what was rebound, deleted, or added beforehand (issue #2918).
#[test]
fn lazy_module_attrs_materialize_in_startup_order_after_earlier_mutations() {
    let mut module = lazy_module_fixture();
    assert_eq!(module.get_attr_value("lazy_b"), Some(Value::int(20)));
    assert_eq!(
        module_attr_names(&module),
        ["first", "lazy_a", "second", "lazy_b"]
    );

    // Writes that do not name a deferred slot leave it pending.
    module.insert_attr("added".to_string(), Value::int(3));
    module.insert_attr("second".to_string(), Value::int(22));
    assert_eq!(module.remove_attr("first"), Some(Value::int(1)));
    module.insert_attr("first".to_string(), Value::int(11));
    assert!(module.has_pending_lazy_attrs());

    // A rebound startup name keeps its slot, a deleted-then-rebound one is
    // appended, and later additions follow every startup slot.
    let expected = ["lazy_a", "second", "lazy_b", "added", "first"];
    assert_eq!(module_attr_names(&module), expected);
    assert!(module.has_pending_lazy_attrs());

    module.attach_live_namespace_with_dunders();
    assert!(!module.has_pending_lazy_attrs());
    let listed = module_attr_names(&module);
    assert_eq!(
        listed[..5],
        [
            "__name__",
            "__doc__",
            "__package__",
            "__loader__",
            "__spec__"
        ]
    );
    assert_eq!(listed[5..], expected);
    assert_eq!(module.get_attr_value("second"), Some(Value::int(22)));
    assert_eq!(module.get_attr_value("lazy_a"), Some(Value::int(10)));
}

#[test]
fn rebinding_a_lazy_module_attr_wins_over_the_deferred_value() {
    let mut module = lazy_module_fixture();
    let state = module.mutation_state();

    let previous = module.insert_attr("lazy_a".to_string(), Value::int(99));
    assert_eq!(previous, Some(Value::int(10)));
    assert!(
        state.version() > 0,
        "rebinding must invalidate provider caches"
    );
    assert!(!module.has_pending_lazy_attrs());
    assert_eq!(module.get_attr_value("lazy_a"), Some(Value::int(99)));
    assert_eq!(module.get_attr_value("lazy_b"), Some(Value::int(20)));
    assert_eq!(
        module_attr_names(&module),
        ["first", "lazy_a", "second", "lazy_b"]
    );
}

#[test]
fn deleting_a_lazy_module_attr_never_rematerializes_it() {
    let mut module = lazy_module_fixture();
    let state = module.mutation_state();

    assert_eq!(module.remove_attr("lazy_a"), Some(Value::int(10)));
    assert!(
        state.version() > 0,
        "deletion must invalidate provider caches"
    );
    assert_eq!(module.get_attr_value("lazy_a"), None);
    assert_eq!(module.remove_attr("lazy_a"), None);
    assert_eq!(module_attr_names(&module), ["first", "second", "lazy_b"]);
    let namespace = module.attach_live_namespace_with_dunders();
    assert_eq!(
        namespace.dict_with(|dict| dict.contains_key(&StrKey("lazy_a"))),
        Some(false)
    );

    // Re-binding after deletion is an ordinary append of the new value.
    module.insert_attr("lazy_a".to_string(), Value::int(7));
    assert_eq!(module.get_attr_value("lazy_a"), Some(Value::int(7)));
    assert_eq!(
        module_attr_names(&module).last().map(String::as_str),
        Some("lazy_a")
    );
}

/// Deferred state belongs to one module object: mutating one provider must
/// not leak into another built from the same source.
#[test]
fn lazy_module_attr_state_is_per_module_instance() {
    let mut deleted = lazy_module_fixture();
    let untouched = lazy_module_fixture();

    deleted.remove_attr("lazy_a");
    assert_eq!(deleted.get_attr_value("lazy_a"), None);
    assert!(untouched.has_pending_lazy_attrs());
    assert_eq!(untouched.get_attr_value("lazy_a"), Some(Value::int(10)));
    assert_eq!(
        module_attr_names(&untouched),
        ["first", "lazy_a", "second", "lazy_b"]
    );

    // A struct clone is a distinct, fully materialized namespace.
    let clone = untouched.clone();
    assert!(!clone.has_pending_lazy_attrs());
    assert_eq!(
        module_attr_names(&clone),
        ["first", "lazy_a", "second", "lazy_b"]
    );
}

/// Modules without deferred attributes pay one nullable pointer for the
/// backing, nothing more.
#[test]
fn lazy_module_attr_backing_costs_ordinary_modules_one_pointer() {
    assert_eq!(
        std::mem::size_of::<Option<Box<LazyModuleAttrs>>>(),
        std::mem::size_of::<usize>()
    );
    let module = PyModule::new("plain".to_string(), ModuleAttrs::default());
    assert!(!module.has_pending_lazy_attrs());
}

#[test]
fn live_module_namespace_shares_alias_mutations_and_collection_generation() {
    let mut attrs = ModuleAttrs::default();
    attrs.insert("initial".to_string(), Value::int(1));
    let mut module = PyModule::new("native".to_string(), attrs);
    module.attach_live_namespace();

    let namespace = module.live_namespace().expect("live namespace");
    let state = namespace.dict_iteration_mutation_state().unwrap();
    let before_alias_write = state.version();
    namespace
        .dict_insert(PyKey::str_from("from_alias"), Value::int(2))
        .unwrap();
    assert_eq!(
        state.version(),
        before_alias_write.wrapping_add(1),
        "direct __dict__ writes must invalidate namespace consumers"
    );
    assert_eq!(module.get_attr_value("from_alias"), Some(Value::int(2)));

    module.insert_attr("from_attribute".to_string(), Value::int(3));
    assert_eq!(
        namespace
            .dict_with(|dict| dict.get(&PyKey::str_from("from_attribute")).cloned())
            .flatten(),
        Some(Value::int(3))
    );
    assert!(module.attrs.is_empty(), "the live dict is authoritative");
}

#[test]
fn filesystem_module_shares_root_globals_without_owning_environment_strongly() {
    let root = Environment::new(None);
    let mut module = PyModule::new("source_module".to_string(), ModuleAttrs::default());
    let module_state = module.mutation_state();
    module.attach_filesystem_namespace(&root);

    module.insert_attr("x".to_string(), Value::int(1));
    assert_eq!(
        root.borrow().namespace_values().get("x"),
        Some(&Value::int(1))
    );
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

        assert_eq!(pool_b_len(), 5);
        assert_eq!(pool_opaque_len(), 7);
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
        trampoline_arity: UserFunction::TRAMPOLINE_INELIGIBLE,
        self_bind: None,
        local_names: Rc::new(HashSet::new()),
        local_index: Rc::new(HashMap::new()),
        global_names: Rc::new(HashSet::new()),
        nonlocal_names: Rc::new(HashSet::new()),
        env: Environment::new(None),
        closure_cells: smallvec::SmallVec::new(),
        is_memo_pure: false,
        iterable_coroutine: std::cell::Cell::new(false),
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
    assert_eq!(
        cm_fn.trampoline_arity,
        UserFunction::TRAMPOLINE_INELIGIBLE,
        "classmethod wrapper must never be trampoline-eligible"
    );
    assert_eq!(
        sm_fn.trampoline_arity,
        UserFunction::TRAMPOLINE_INELIGIBLE,
        "staticmethod wrapper must never be trampoline-eligible"
    );
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
    attrs.set_builtin_data(Value::list(vec![Value::int(1), Value::int(2)]));
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
