use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{
    Environment, InstanceAttrs, ParamBind, PyClass, PyInstance, PyModule, UserFunction,
    UserFunctionKind, Value, next_fn_id,
};

use super::{AttrCacheEntry, GlobalCacheEntry, KwCallCacheEntry};

fn make_cache_user_function() -> Rc<UserFunction> {
    Rc::new(UserFunction {
        id: next_fn_id(),
        kind: UserFunctionKind::Regular,
        name: Rc::from("method"),
        qualname: Rc::from("C.method"),
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
        iterable_coroutine: std::cell::Cell::new(false),
        precompiled_code: None,
        wrapped_func: None,
    })
}

#[test]
fn global_cache_entries_guard_namespace_and_shared_module_mutation() {
    let first_namespace = Environment::new(None);
    let second_namespace = Environment::new(None);
    let first_identity = first_namespace.borrow().namespace_id();
    let second_identity = second_namespace.borrow().namespace_id();

    let global = GlobalCacheEntry::environment(first_identity, 7, &Value::string("first"));
    assert_eq!(
        global.lookup(first_identity, 7, 3),
        Some(Value::string("first"))
    );
    assert_eq!(global.lookup(second_identity, 7, 3), None);
    assert_eq!(global.lookup(first_identity, 8, 3), None);
    assert_eq!(
        global.lookup(first_identity, 7, 4),
        Some(Value::string("first")),
        "environment entries must ignore the unrelated structure generation"
    );
    let exhausted =
        GlobalCacheEntry::environment(first_identity, u64::MAX, &Value::string("stale"));
    assert_eq!(exhausted.namespace_id(), None);
    assert_eq!(
        exhausted.lookup(first_identity, u64::MAX, 3),
        None,
        "an exhausted generation must disable cache hits rather than alias an old one"
    );

    let provider = Rc::new(RefCell::new(PyModule::new(
        "builtins".to_string(),
        crate::value::ModuleAttrs::default(),
    )));
    let provider_state = provider.borrow().mutation_state();
    let builtin =
        GlobalCacheEntry::builtin_module(first_identity, 3, provider_state, &Value::int(11));
    assert_eq!(builtin.lookup(first_identity, 7, 3), Some(Value::int(11)));
    assert_eq!(builtin.lookup(second_identity, 7, 3), None);
    assert_eq!(builtin.lookup(first_identity, 7, 4), None);
    assert_eq!(
        builtin.lookup(first_identity, 8, 3),
        Some(Value::int(11)),
        "builtin entries must ignore the unrelated environment generation"
    );
    provider
        .borrow_mut()
        .insert_attr("len".to_string(), Value::int(12));
    assert_eq!(
        builtin.lookup(first_identity, 7, 3),
        None,
        "a mutation through another interpreter's shared module must invalidate"
    );
    let exhausted_builtin = GlobalCacheEntry::builtin_module(
        first_identity,
        u64::MAX,
        provider.borrow().mutation_state(),
        &Value::int(99),
    );
    assert_eq!(exhausted_builtin.namespace_id(), None);
    assert_eq!(
        exhausted_builtin.lookup(first_identity, 7, u64::MAX),
        None,
        "an exhausted structure generation must disable builtin cache hits"
    );
}

#[test]
fn script_fastlocal_cache_always_guards_mirror_epoch() {
    let local_index = Rc::new(HashMap::from([("value".to_string(), 4_u32)]));
    let inline = GlobalCacheEntry::script_fastlocal(
        17,
        23,
        5,
        Rc::downgrade(&local_index),
        4,
        &Value::int(9),
    );
    assert_eq!(
        inline.lookup_with_fastlocal(17, 23, 0, 5, |_, _, _| {
            panic!("inline values do not need a live-slot read")
        }),
        Some(Value::int(9))
    );
    assert_eq!(
        inline.lookup_with_fastlocal(17, 23, 0, 6, |_, _, _| {
            panic!("a stale mirror epoch must reject before slot lookup")
        }),
        None
    );

    let heap = Value::string("heap string fastlocal locator value");
    let locator =
        GlobalCacheEntry::script_fastlocal(17, 24, 7, Rc::downgrade(&local_index), 4, &heap);
    assert_eq!(
        locator.lookup_with_fastlocal(17, 24, 0, 7, |epoch, layout, register| {
            assert_eq!(epoch, 7);
            assert!(std::rc::Weak::ptr_eq(layout, &Rc::downgrade(&local_index)));
            assert_eq!(register, 4);
            Some(heap.clone())
        }),
        Some(heap)
    );
}

#[test]
fn global_cache_preserves_live_identity_without_owning_instances() {
    let class = Rc::new(RefCell::new(PyClass::new("C", "C", None, IndexMap::new())));
    let instance = Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    }));
    let liveness = Rc::downgrade(&instance);
    let original = Value::py_instance(Rc::clone(&instance));
    let original_id = original.value_id();
    let entry = GlobalCacheEntry::environment(17, 23, &original);

    let hit = entry.lookup(17, 23, 0).expect("live weak cache hit");
    assert_eq!(
        hit.value_id(),
        original_id,
        "upgrading must preserve Python object identity"
    );

    drop(hit);
    drop(original);
    drop(instance);
    assert!(
        liveness.upgrade().is_none(),
        "the global cache must not keep the instance alive"
    );
    assert_eq!(
        entry.lookup(17, 23, 0),
        None,
        "a dead weak value is an ordinary cache miss"
    );
}

#[test]
fn method_cache_preserves_function_identity_without_owning_it() {
    let class = Rc::new(RefCell::new(PyClass::new("C", "C", None, IndexMap::new())));
    let function = make_cache_user_function();
    let liveness = Rc::downgrade(&function);
    let original = Value::user_function(Rc::clone(&function));
    let original_id = original.value_id();
    let entry = AttrCacheEntry::class_attr(&class, 0, 0, &original);
    let AttrCacheEntry::ClassAttr { value, .. } = &entry else {
        panic!("regular methods must be weak-cacheable");
    };

    let hit = value.upgrade().expect("live method cache hit");
    assert_eq!(
        hit.value_id(),
        original_id,
        "method-cache upgrade must preserve function identity"
    );
    drop(hit);
    drop(original);
    drop(function);
    assert!(
        liveness.upgrade().is_none(),
        "the method cache must not close a function -> code -> cache cycle"
    );
    assert!(
        value.upgrade().is_none(),
        "a dropped method backing must turn into a safe cache miss"
    );

    let unsupported = Value::string("longer than the inline string capacity");
    assert!(matches!(
        AttrCacheEntry::class_attr(&class, 0, 0, &unsupported),
        AttrCacheEntry::Uncacheable
    ));
}

#[test]
fn cache_weak_identities_keep_dropped_allocations_reserved_against_aba() {
    let class = Rc::new(RefCell::new(PyClass::new(
        "Old",
        "Old",
        None,
        IndexMap::new(),
    )));
    let class_entry = AttrCacheEntry::InstanceAttr {
        class_ptr: Rc::downgrade(&class),
        class_version: 0,
        epoch: 0,
    };
    let old_class_ptr = Rc::as_ptr(&class);
    drop(class);
    let replacement_class = Rc::new(RefCell::new(PyClass::new(
        "New",
        "New",
        None,
        IndexMap::new(),
    )));
    let AttrCacheEntry::InstanceAttr { class_ptr, .. } = class_entry else {
        unreachable!()
    };
    assert!(class_ptr.upgrade().is_none());
    assert_eq!(class_ptr.as_ptr(), old_class_ptr);
    assert_ne!(class_ptr.as_ptr(), Rc::as_ptr(&replacement_class));

    let binds = Rc::new(vec![ParamBind::Reg(0)]);
    let kw_entry = KwCallCacheEntry::Simple {
        param_binds_ptr: Rc::downgrade(&binds),
        npos: 0,
        slots: smallvec::SmallVec::new(),
    };
    let old_binds_ptr = Rc::as_ptr(&binds);
    drop(binds);
    let replacement_binds = Rc::new(vec![ParamBind::Reg(1)]);
    let KwCallCacheEntry::Simple {
        param_binds_ptr, ..
    } = kw_entry
    else {
        unreachable!()
    };
    assert!(param_binds_ptr.upgrade().is_none());
    assert_eq!(param_binds_ptr.as_ptr(), old_binds_ptr);
    assert_ne!(param_binds_ptr.as_ptr(), Rc::as_ptr(&replacement_binds));
}
