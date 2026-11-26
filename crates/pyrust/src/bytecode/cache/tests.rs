use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{
    Environment, InstanceAttrs, ParamBind, PyClass, PyInstance, PyModule, UserFunction,
    UserFunctionKind, Value, next_fn_id,
};

use super::{
    AttrCacheEntry, BinOpCacheEntry, GlobalCacheEntry, KwCallCacheEntry, KwCallSimpleCache,
    SiteCache, SparsePcCache, is_attr_cache_site, is_call_builtin_cache_site,
};

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
        inline.lookup_with_fastlocal(
            17,
            23,
            0,
            5,
            |_, _, _| panic!("inline values do not need a live-slot read"),
            |_, _| panic!("a register entry never reads a root slot"),
        ),
        Some(Value::int(9))
    );
    assert_eq!(
        inline.lookup_with_fastlocal(
            17,
            23,
            0,
            6,
            |_, _, _| panic!("a stale mirror epoch must reject before slot lookup"),
            |_, _| panic!("a register entry never reads a root slot"),
        ),
        None
    );

    let heap = Value::string("heap string fastlocal locator value");
    let locator =
        GlobalCacheEntry::script_fastlocal(17, 24, 7, Rc::downgrade(&local_index), 4, &heap);
    assert_eq!(
        locator.lookup_with_fastlocal(
            17,
            24,
            0,
            7,
            |epoch, layout, register| {
                assert_eq!(epoch, 7);
                assert!(std::rc::Weak::ptr_eq(layout, &Rc::downgrade(&local_index)));
                assert_eq!(register, 4);
                Some(heap.clone())
            },
            |_, _| panic!("a register entry never reads a root slot"),
        ),
        Some(heap)
    );
}

/// Issue #3423: a root-slot entry caches *where* a global lives, never its
/// value, so rebinding the name is observed by the next hit without any
/// generation advancing, while removing or re-inserting it is a miss.
#[test]
fn environment_slot_entries_read_the_live_binding_and_reject_a_replaced_key() {
    let root = Environment::new(None);
    let namespace_id = root.borrow().namespace_id();
    for name in ["first", "second", "counter"] {
        root.borrow_mut()
            .namespace_values_mut()
            .insert(name, Value::int(1));
    }
    let (index, key) = root
        .borrow()
        .namespace_root_binding_slot("counter", &Value::int(1))
        .expect("a bound name has a slot locator");
    assert_eq!(
        root.borrow()
            .namespace_root_binding_slot("counter", &Value::int(2)),
        None,
        "a value some other provider resolved must not be attributed to the slot"
    );
    let entry = GlobalCacheEntry::environment_slot(namespace_id, 3, index, key);
    let read = |environment_version, structure_version| {
        entry.lookup_with_fastlocal(
            namespace_id,
            environment_version,
            structure_version,
            0,
            |_, _, _| panic!("a root-slot entry never reads a register"),
            |index, key| root.borrow().namespace_root_slot_value(index, key),
        )
    };

    assert_eq!(read(7, 3), Some(Value::int(1)));
    // Rebinding keeps the slot and its key: the entry observes the new value
    // although no generation moved.
    root.borrow_mut()
        .namespace_values_mut()
        .insert("counter", Value::int(41));
    assert_eq!(read(7, 3), Some(Value::int(41)));
    assert_eq!(
        read(8, 3),
        Some(Value::int(41)),
        "root-slot entries must ignore the unrelated value generation"
    );
    assert_eq!(
        read(7, 4),
        None,
        "a structure change (a PEP 695 scope, a deletion) must reject the entry"
    );
    assert_eq!(
        entry.lookup_with_fastlocal(
            namespace_id + 1,
            7,
            3,
            0,
            |_, _, _| None,
            |_, _| panic!("another namespace must reject before the slot read"),
        ),
        None
    );

    // A lexical child reads the same root binding through its parent chain.
    let child = Environment::new(Some(Rc::clone(&root)));
    let (child_index, child_key) = child
        .borrow()
        .namespace_root_binding_slot("counter", &Value::int(41))
        .expect("a child resolves the root slot");
    assert_eq!(
        child
            .borrow()
            .namespace_root_slot_value(child_index, &child_key),
        Some(Value::int(41))
    );

    // `del counter; counter = 5` re-inserts under a fresh key, and deleting an
    // earlier name shifts every later slot: both are ordinary misses.
    root.borrow_mut().namespace_values_mut().remove("counter");
    assert_eq!(read(7, 3), None);
    root.borrow_mut()
        .namespace_values_mut()
        .insert("counter", Value::int(5));
    assert_eq!(read(7, 3), None, "a re-inserted binding has a new key");
    let (index, key) = root
        .borrow()
        .namespace_root_binding_slot("counter", &Value::int(5))
        .expect("the re-inserted name has a slot locator");
    let shifted = GlobalCacheEntry::environment_slot(namespace_id, 3, index, key);
    root.borrow_mut().namespace_values_mut().remove("first");
    assert_eq!(
        shifted.lookup_with_fastlocal(
            namespace_id,
            7,
            3,
            0,
            |_, _, _| None,
            |index, key| root.borrow().namespace_root_slot_value(index, key),
        ),
        None,
        "a shifted slot must never be read as the cached name"
    );

    assert!(matches!(
        GlobalCacheEntry::environment_slot(namespace_id, u64::MAX, 0, Rc::from("counter")),
        GlobalCacheEntry::Empty
    ));
    assert!(matches!(
        GlobalCacheEntry::environment_slot(u64::MAX, 3, 0, Rc::from("counter")),
        GlobalCacheEntry::Empty
    ));
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
    let AttrCacheEntry::ClassAttr(cached) = &entry else {
        panic!("regular methods must be weak-cacheable");
    };
    let value = &cached.value;

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
    let kw_entry = KwCallCacheEntry::Simple(Box::new(KwCallSimpleCache {
        param_binds_ptr: Rc::downgrade(&binds),
        npos: 0,
        slots: smallvec::SmallVec::new(),
    }));
    let old_binds_ptr = Rc::as_ptr(&binds);
    drop(binds);
    let replacement_binds = Rc::new(vec![ParamBind::Reg(1)]);
    let KwCallCacheEntry::Simple(cached_kw) = kw_entry else {
        unreachable!()
    };
    assert!(cached_kw.param_binds_ptr.upgrade().is_none());
    assert_eq!(cached_kw.param_binds_ptr.as_ptr(), old_binds_ptr);
    assert_ne!(
        cached_kw.param_binds_ptr.as_ptr(),
        Rc::as_ptr(&replacement_binds)
    );
}

#[test]
fn set_slot_attr_entry_keeps_weak_class_identity_and_stays_compact() {
    let class = Rc::new(RefCell::new(PyClass::new(
        "Slotted",
        "Slotted",
        None,
        IndexMap::new(),
    )));
    let slot_id = pyrust_core::MemberSlotId::fresh();
    let entry = AttrCacheEntry::SetSlotAttr(Box::new(super::SlotAttrCacheEntry {
        class_ptr: Rc::downgrade(&class),
        class_version: 3,
        epoch: 7,
        slot_id,
    }));
    assert!(format!("{entry:?}").starts_with("SetSlotAttr("));
    let AttrCacheEntry::SetSlotAttr(payload) = &entry else {
        unreachable!()
    };
    let old_ptr = Rc::as_ptr(&class);
    drop(class);
    // The weak allocation identity stays reserved after the class dies (ABA).
    assert!(payload.class_ptr.upgrade().is_none());
    assert_eq!(payload.class_ptr.as_ptr(), old_ptr);
    assert_eq!(payload.slot_id, slot_id);
    assert!(std::mem::size_of::<AttrCacheEntry>() <= 32);
}

#[test]
fn property_get_entry_holds_getter_weakly_and_stays_compact() {
    let class = Rc::new(RefCell::new(PyClass::new("P", "P", None, IndexMap::new())));
    let fget = Value::user_function(Rc::new(UserFunction {
        id: next_fn_id(),
        kind: UserFunctionKind::Regular,
        name: Rc::from("x"),
        qualname: Rc::from("P.x"),
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
    }));
    let entry = AttrCacheEntry::PropertyGet(Box::new(super::PropertyGetCacheEntry {
        class_ptr: Rc::downgrade(&class),
        class_version: 2,
        epoch: 5,
        fget: pyrust_core::WeakValueCache::new(&fget).expect("function getter is cacheable"),
    }));
    assert!(format!("{entry:?}").starts_with("PropertyGet("));
    let AttrCacheEntry::PropertyGet(payload) = &entry else {
        unreachable!()
    };
    assert!(payload.fget.upgrade_user_function().is_some());
    // The cache never owns the getter: once the property drops it, the entry
    // is an ordinary miss instead of keeping `function -> code -> cache` alive.
    drop(fget);
    assert!(payload.fget.upgrade_user_function().is_none());
    let old_ptr = Rc::as_ptr(&class);
    drop(class);
    assert!(payload.class_ptr.upgrade().is_none());
    assert_eq!(payload.class_ptr.as_ptr(), old_ptr);
    assert!(std::mem::size_of::<AttrCacheEntry>() <= 32);
}

#[test]
fn inline_cache_entry_sizes_stay_compact() {
    use std::mem::size_of;

    assert!(
        size_of::<KwCallCacheEntry>() <= 24,
        "KwCallCacheEntry grew to {} B (guard <= 24 B; keep heavy variants boxed)",
        size_of::<KwCallCacheEntry>()
    );
    assert!(
        size_of::<AttrCacheEntry>() <= 32,
        "AttrCacheEntry grew to {} B (guard <= 32 B; keep heavy variants boxed)",
        size_of::<AttrCacheEntry>()
    );
    assert!(
        size_of::<crate::interpreter::CallBuiltinCacheEntry>() <= 24,
        "CallBuiltinCacheEntry grew to {} B (guard <= 24 B; keep Cached payload boxed)",
        size_of::<crate::interpreter::CallBuiltinCacheEntry>()
    );
    assert!(
        size_of::<crate::bytecode::BuiltinMethodCacheEntry>() <= 16,
        "BuiltinMethodCacheEntry grew to {} B (guard <= 16 B; keep NativeMethod boxed)",
        size_of::<crate::bytecode::BuiltinMethodCacheEntry>()
    );
    assert!(
        size_of::<BinOpCacheEntry>() <= 4,
        "BinOpCacheEntry grew to {} B (guard <= 4 B)",
        size_of::<BinOpCacheEntry>()
    );
    assert!(
        size_of::<crate::bytecode::GlobalStoreCacheEntry>() <= 64,
        "GlobalStoreCacheEntry grew to {} B (guard <= 64 B; one entry per name slot)",
        size_of::<crate::bytecode::GlobalStoreCacheEntry>()
    );
}

#[test]
fn site_cache_and_sparse_pc_cache_allocate_zero_until_populated() {
    use crate::bytecode::Insn;

    let mut site_cache: SiteCache<AttrCacheEntry> = SiteCache::new();
    assert!(site_cache.is_empty());
    assert_eq!(site_cache.heap_bytes(), 0);
    assert!(site_cache.get(0).is_none());

    let insns = vec![
        Insn::LoadNone(0),
        Insn::GetAttr(1, 0, 0),
        Insn::LoadNone(2),
        Insn::SetAttr(0, 1, 2),
        Insn::Return(1),
    ];
    site_cache.set_or_init(
        &insns,
        1,
        is_attr_cache_site,
        || AttrCacheEntry::Empty,
        AttrCacheEntry::Megamorphic,
    );
    assert!(!site_cache.is_empty());
    assert!(site_cache.get(0).is_none());
    assert!(matches!(
        site_cache.get(1),
        Some(AttrCacheEntry::Megamorphic)
    ));
    assert!(site_cache.get(2).is_none());
    assert!(matches!(site_cache.get(3), Some(AttrCacheEntry::Empty)));
    assert!(site_cache.get(4).is_none());
    // 5 bytes pc_to_slot + 2 * size_of::<AttrCacheEntry>() (vs 5 * 64 = 320 B before)
    assert_eq!(
        site_cache.heap_bytes(),
        insns.len() + 2 * std::mem::size_of::<AttrCacheEntry>()
    );

    let mut builtin_cache: SiteCache<crate::interpreter::CallBuiltinCacheEntry> = SiteCache::new();
    let call_insns = vec![Insn::LoadNone(0), Insn::Call(0, 0), Insn::Return(0)];
    assert_eq!(builtin_cache.heap_bytes(), 0);
    builtin_cache.set_or_init(
        &call_insns,
        1,
        is_call_builtin_cache_site,
        || crate::interpreter::CallBuiltinCacheEntry::Empty,
        crate::interpreter::CallBuiltinCacheEntry::Empty,
    );
    assert_eq!(
        builtin_cache.heap_bytes(),
        call_insns.len() + std::mem::size_of::<crate::interpreter::CallBuiltinCacheEntry>()
    );

    let mut sparse: SparsePcCache<KwCallCacheEntry> = SparsePcCache::new();
    assert!(sparse.is_empty());
    assert_eq!(sparse.capacity(), 0);
    assert_eq!(sparse.heap_bytes(), 0);
    assert!(sparse.get(10).is_none());

    // Insert out of order across the linear-scan/binary-search boundary (> 8 entries).
    for pc in [50usize, 3, 25, 10, 99, 1, 40, 7, 18, 88] {
        sparse.insert(pc, KwCallCacheEntry::Fallback);
    }
    for pc in [1usize, 3, 7, 10, 18, 25, 40, 50, 88, 99] {
        assert!(matches!(sparse.get(pc), Some(KwCallCacheEntry::Fallback)));
    }
    assert!(sparse.get(2).is_none());
    assert!(sparse.get(100).is_none());
}

fn compile_test_script(src: &str) -> crate::bytecode::FnCode {
    let tokens = crate::lexer::Lexer::new(src).unwrap().into_tokens();
    let mut parser = crate::parser::Parser::new(tokens);
    let stmts = parser.parse_program().unwrap();
    let empty = HashSet::new();
    let names = crate::interpreter::collect_local_names(&[], &stmts, &empty, &empty);
    let local_index = Rc::new(
        (0u32..)
            .zip(names)
            .map(|(slot, name)| (name, slot))
            .collect(),
    );
    crate::compiler::compile_script_with_linenos(&stmts, local_index, false, &[], "<test>").unwrap()
}

#[test]
fn compiled_and_optimized_fncode_starts_with_zero_cache_and_handler_free_exc_table_ram() {
    let src = "\
def work(x, y):
    a = x + y
    return a * 2
work(1, 2)
";
    let raw = compile_test_script(src);
    let assert_zero_caches = |code: &crate::bytecode::FnCode, stage: &str| {
        assert!(
            code.attr_cache.borrow().is_empty(),
            "{stage}: attr_cache must start unallocated"
        );
        assert_eq!(
            code.global_cache.borrow().capacity(),
            0,
            "{stage}: global_cache must start unallocated"
        );
        assert_eq!(
            code.binop_cache.borrow().capacity(),
            0,
            "{stage}: binop_cache must start unallocated"
        );
        assert_eq!(
            code.kwcall_cache.borrow().capacity(),
            0,
            "{stage}: kwcall_cache must start unallocated"
        );
        assert_eq!(
            code.fmt_spec_cache.borrow().capacity(),
            0,
            "{stage}: fmt_spec_cache must start unallocated"
        );
        assert!(
            code.call_builtin_cache.borrow().is_empty(),
            "{stage}: call_builtin_cache must start unallocated"
        );
        assert!(
            code.builtin_method_cache.borrow().is_empty(),
            "{stage}: builtin_method_cache must start unallocated"
        );
        assert_eq!(
            code.exc_table.capacity(),
            0,
            "{stage}: handler-free exc_table must allocate 0 bytes"
        );
        for proto in &code.fn_protos {
            assert!(proto.code.attr_cache.borrow().is_empty());
            assert_eq!(proto.code.global_cache.borrow().capacity(), 0);
            assert_eq!(proto.code.binop_cache.borrow().capacity(), 0);
            assert_eq!(proto.code.kwcall_cache.borrow().capacity(), 0);
            assert_eq!(proto.code.fmt_spec_cache.borrow().capacity(), 0);
            assert!(proto.code.call_builtin_cache.borrow().is_empty());
            assert!(proto.code.builtin_method_cache.borrow().is_empty());
            assert_eq!(proto.code.exc_table.capacity(), 0);
        }
    };

    assert_zero_caches(&raw, "Compiler::finish");
    let mir_lowered = crate::mir::optimize_fncode_via_mir(raw.clone());
    assert_zero_caches(&mir_lowered, "lower_mir_to_fncode");
    let optimized = crate::optimizer::optimize(raw);
    assert_zero_caches(&optimized, "optimize_fn_code");
    assert!(!optimized.has_exc_handlers);
    assert!(!optimized.fn_protos[0].code.has_exc_handlers);
}

#[test]
fn sparse_inline_caches_preserve_hits_and_reentrancy_safety() {
    let src = r#"
class Node:
    def __init__(self, v):
        self.v = v

    def walk(self, depth, step=1):
        if depth <= 0:
            return f"{self.v:.1f}"
        # Re-entrantly exercises CallMethod, CallKw, GetAttr, SetAttr,
        # BinOp (float), FormatValueSpec, and builtin len() on the same FnCode.
        self.v = self.v + float(step)
        sub = self.walk(depth - 1, step=step)
        return f"{len(sub)}:{sub}"

class Slotted:
    __slots__ = ("x",)
    def __init__(self, x):
        self.x = x

def kw_target(a, b=2, c=3):
    return a + b + c

def run():
    n = Node(1.5)
    r1 = n.walk(4, step=2)
    s = Slotted(41)
    r2 = s.x + s.x - 41
    d = {"b": 20, "c": 30}
    r3 = kw_target(10, **d)
    r4 = kw_target(*(10, 20), **{"c": 30})
    return (r1, r2, r3, r4)

result = run()
assert result == ('9:7:5:3:9.5', 41, 60, 60), result
"#;
    let mut interp = crate::interpreter::Interpreter::default();
    let tokens = crate::lexer::Lexer::new(src).unwrap().into_tokens();
    let mut parser = crate::parser::Parser::new(tokens);
    let stmts = parser.parse_program().unwrap();
    interp
        .exec_program(&stmts, false)
        .expect("re-entrant inline caches must preserve exact results");
}

/// Execute `src` and return the named module-level function's compiled body.
fn executed_function_code(
    interp: &mut crate::interpreter::Interpreter,
    name: &str,
) -> (Value, *const crate::bytecode::FnCode) {
    let function = interp
        .lookup_name(name)
        .expect("lookup succeeds")
        .unwrap_or_else(|| panic!("{name} must be defined"));
    let pyrust_core::ValueKind::UserFunction(f) = function.kind() else {
        panic!("{name} must be a UserFunction");
    };
    let code: *const crate::bytecode::FnCode = f
        .precompiled_code
        .as_deref()
        .and_then(|any| any.downcast_ref::<crate::bytecode::FnCode>())
        .unwrap_or_else(|| panic!("{name} must have precompiled FnCode"));
    (function, code)
}

fn call_method_pc(code: &crate::bytecode::FnCode) -> usize {
    code.insns
        .iter()
        .position(|insn| matches!(insn, crate::bytecode::Insn::CallMethod { .. }))
        .expect("function body must contain a fused CallMethod site")
}

/// Issue #3379: each `CallMethod` site records the typed target for its
/// built-in receiver, demotes on a second receiver shape, and allocates
/// nothing for a site that never ran.
#[test]
fn builtin_method_cache_records_typed_targets_per_site() {
    use crate::bytecode::BuiltinMethodCacheEntry;

    let mut interp = crate::interpreter::Interpreter::default();
    interp
        .exec_source(
            r#"
from collections import deque

def on_list(o):
    return o.append(1)

def on_deque(o):
    return o.append(1)

def on_both(o):
    return o.append(1)

def on_user(o):
    return o.append(1)

def never_called(o):
    return o.append(1)

class User:
    def append(self, x):
        return x

for _ in range(4):
    on_list([])
    on_deque(deque())
    on_both([])
    on_both(deque())
    on_user(User())
"#,
            None,
            None,
        )
        .expect("cache probe script must execute");

    let entry_at = |name: &str, interp: &mut crate::interpreter::Interpreter| {
        let (_keepalive, code) = executed_function_code(interp, name);
        // SAFETY: `_keepalive` holds the function (and so its `FnCode`) alive
        // for the duration of this closure.
        let code = unsafe { &*code };
        let pc = call_method_pc(code);
        code.builtin_method_cache.borrow().get(pc).cloned()
    };

    assert!(matches!(
        entry_at("on_list", &mut interp),
        Some(BuiltinMethodCacheEntry::Container(_))
    ));
    assert!(matches!(
        entry_at("on_deque", &mut interp),
        Some(BuiltinMethodCacheEntry::NativeMethod(_))
    ));
    assert!(matches!(
        entry_at("on_both", &mut interp),
        Some(BuiltinMethodCacheEntry::Megamorphic)
    ));
    assert!(
        !matches!(
            entry_at("on_user", &mut interp),
            Some(BuiltinMethodCacheEntry::Container(_) | BuiltinMethodCacheEntry::NativeMethod(_))
        ),
        "a Python-level method must never be cached as a built-in target"
    );
    let (_keepalive, cold) = executed_function_code(&mut interp, "never_called");
    // SAFETY: `_keepalive` keeps the function's `FnCode` alive.
    let cold = unsafe { &*cold };
    assert!(
        cold.builtin_method_cache.borrow().is_empty(),
        "an unexecuted function must pay 0 heap bytes for the cache"
    );
}
