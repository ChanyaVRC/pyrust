#[cfg(test)]
mod vm_tests {
    use super::{BuiltinCallProbe, PositionalCallCacheProbe, RegSlice, Value};
    use crate::bytecode::{FnCode, Insn};
    use crate::interpreter::Interpreter;
    use crate::interpreter::fast_path::BuiltinCallCacheMiss;

    fn empty_code(insns: Vec<Insn>) -> FnCode {
        let n = insns.len();
        FnCode {
            insns,
            filename: std::sync::Arc::from("<unknown>"),
            qualname: std::sync::Arc::from("<module>"),
            lineno_table: vec![0u32; n],
            col_table: vec![(0, 0, 0, 0); n],
            first_lineno: 0,
            consts: vec![],
            names: vec![],
            num_regs: 0,
            num_iters: 0,
            num_locals: 0,
            fn_protos: vec![],
            cell_slots: smallvec::smallvec![],
            own_cell_count: 0,
            free_var_candidates: std::cell::OnceCell::new(),
            is_generator: false,
            is_coroutine: false,
            is_class_method: false,
            is_function_scope: false,
            is_inlined_comp: false,
            comp_enclosing_locals: None,
            attr_cache: std::cell::RefCell::new(crate::bytecode::SiteCache::new()),
            global_cache: std::cell::RefCell::new(Vec::new()),
            global_cache_interest_masks: Vec::new(),
            binop_cache: std::cell::RefCell::new(Vec::new()),
            kwcall_cache: std::cell::RefCell::new(crate::bytecode::SparsePcCache::new()),
            fmt_spec_cache: std::cell::RefCell::new(crate::bytecode::SparsePcCache::new()),
            call_builtin_cache: std::cell::RefCell::new(crate::bytecode::SiteCache::new()),
            cached_code_object: std::cell::RefCell::new(None),
            // Empty: these hand-built test fixtures run unoptimized, so the VM
            // uses the dynamic SetupExcept/PopExcept handler stack.
            exc_table: Vec::new(),
            has_exc_handlers: false,
        }
    }

    fn call_code(argc: u8) -> FnCode {
        let mut code = empty_code(vec![Insn::Call(0, argc), Insn::Return(0)]);
        code.num_regs = crate::bytecode::Reg::from(argc) + 1;
        code.num_locals = code.num_regs;
        code
    }

    fn execute_call(
        code: &FnCode,
        function: Value,
        arguments: Vec<Value>,
    ) -> crate::error::Result<Value> {
        let mut registers = Vec::with_capacity(arguments.len() + 1);
        registers.push(function);
        registers.extend(arguments);
        let registers = unsafe { RegSlice::from_raw(registers.as_mut_ptr(), registers.len()) };
        Interpreter::default().run_bytecode(code, registers)
    }

    fn cache_test_user_function() -> Value {
        use std::cell::RefCell;
        use std::collections::{HashMap, HashSet};
        use std::rc::Rc;

        use pyrust_core::{Environment, UserFunction, UserFunctionKind, next_fn_id};

        Value::user_function(Rc::new(UserFunction {
            id: next_fn_id(),
            kind: UserFunctionKind::Regular,
            name: Rc::from("cache_test"),
            qualname: Rc::from("cache_test"),
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
            closure_cells: smallvec::SmallVec::new(),
            is_memo_pure: false,
            iterable_coroutine: std::cell::Cell::new(false),
            precompiled_code: None,
            wrapped_func: None,
        }))
    }

    fn execute_call_with_probe(
        code: &FnCode,
        function: Value,
        arguments: Vec<Value>,
        probe: BuiltinCallProbe,
    ) -> crate::error::Result<Value> {
        let argument_count = arguments.len() as u8;
        let mut registers = Vec::with_capacity(arguments.len() + 1);
        registers.push(function.clone());
        registers.extend(arguments);
        let mut registers = unsafe { RegSlice::from_raw(registers.as_mut_ptr(), registers.len()) };
        Interpreter::default().call_positional_cached(
            &mut registers,
            crate::bytecode::Reg::from(argument_count) + 1,
            0,
            argument_count,
            function,
            code,
            0,
            0,
            PositionalCallCacheProbe::Probed(probe),
        )
    }

    #[test]
    fn matchexcept_with_no_active_exception_returns_error() {
        // MatchExcept must error when no exception is active (compiler bug scenario).
        let mut code = empty_code(vec![]);
        code.num_regs = 1;
        code.insns.push(Insn::LoadNone(0)); // type_reg = None (placeholder)
        code.insns.push(Insn::MatchExcept(0, 1)); // no active_exception → error
        code.insns.push(Insn::ReturnNone);
        code.lineno_table.extend([0u32, 0, 0]);
        code.col_table.extend([(0u32, 0u32, 0u32, 0u32); 3]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![Value::unset(); 1];
        // SAFETY (test): regs is alive for the duration of run_bytecode;
        // no VmFrameView is active, so there is no concurrent access.
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(result.is_err(), "expected Err, got {:?}", result);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no active exception"),
            "error should mention no active exception"
        );
    }

    #[test]
    fn oob_pc_returns_error_not_none() {
        // Jump(100): new_pc = 1 + 100 = 101 > insns.len() (1) → error
        let code = empty_code(vec![Insn::Jump(100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(
            result.is_err(),
            "expected Err for OOB jump, got {:?}",
            result
        );
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn negative_jump_returns_error() {
        // Jump(-100): new_pc = 1 + (-100) = -99 → underflow error
        let code = empty_code(vec![Insn::Jump(-100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(
            result.is_err(),
            "expected Err for negative jump, got {:?}",
            result
        );
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn normal_fallthrough_returns_none() {
        let code = empty_code(vec![Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        assert_eq!(
            interp.run_bytecode(&code, regs_slice).unwrap(),
            Value::none()
        );
    }

    #[test]
    fn builtin_call_probe_rejects_user_function_before_cache_borrow() {
        let code = call_code(0);
        let function = cache_test_user_function();
        let _exclusive_cache_borrow = code.call_builtin_cache.borrow_mut();

        assert!(matches!(
            Interpreter::probe_builtin_vectorcall(&code, 0, &function, &[]),
            BuiltinCallProbe::Uncacheable
        ));
    }

    #[test]
    fn builtin_call_probe_rejects_other_value_kind_before_cache_borrow() {
        let code = call_code(0);
        let function = Value::int(1);
        let _exclusive_cache_borrow = code.call_builtin_cache.borrow_mut();

        assert!(matches!(
            Interpreter::probe_builtin_vectorcall(&code, 0, &function, &[]),
            BuiltinCallProbe::Uncacheable
        ));
    }

    #[test]
    fn ordinary_class_call_records_negative_primitive_identity() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let class = Rc::new(RefCell::new(pyrust_core::PyClass::new(
            "Ordinary",
            "Ordinary",
            Some(crate::interpreter::object_class_singleton()),
            indexmap::IndexMap::new(),
        )));
        let code = call_code(0);
        let function = Value::py_class(Rc::clone(&class));

        execute_call(&code, function.clone(), Vec::new())
            .expect("ordinary class construction must execute");
        assert!(matches!(
            code.call_builtin_cache.borrow().get(0),
            Some(crate::interpreter::CallBuiltinCacheEntry::ClassAfterPrimitiveMiss(cached))
                if cached.as_ptr() == Rc::as_ptr(&class)
        ));
        let warm_probe = Interpreter::probe_builtin_vectorcall(&code, 0, &function, &[]);
        assert!(matches!(
            warm_probe,
            BuiltinCallProbe::ClassAfterPrimitiveMiss
        ));
        let exclusive_cache_borrow = code.call_builtin_cache.borrow_mut();
        execute_call_with_probe(&code, function.clone(), Vec::new(), warm_probe)
            .expect("a copied negative token must not read the cache again");
        drop(exclusive_cache_borrow);
        execute_call(&code, function, Vec::new())
            .expect("the cached negative must still execute ordinary construction");
    }

    #[test]
    fn ordinary_class_negative_weak_identity_rejects_stale_and_cross_category_keys() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let template = || {
            Rc::new(RefCell::new(pyrust_core::PyClass::new(
                "len",
                "len",
                Some(crate::interpreter::object_class_singleton()),
                indexmap::IndexMap::new(),
            )))
        };
        let code = call_code(1);
        let old_class = template();
        code.call_builtin_cache.borrow_mut().set_or_init(
            &code.insns,
            0,
            crate::bytecode::is_call_builtin_cache_site,
            || crate::interpreter::CallBuiltinCacheEntry::Empty,
            crate::interpreter::CallBuiltinCacheEntry::ClassAfterPrimitiveMiss(Rc::downgrade(
                &old_class,
            )),
        );
        drop(old_class);

        let replacement = template();
        let replacement_value = Value::py_class(Rc::clone(&replacement));
        assert!(matches!(
            Interpreter::probe_builtin_vectorcall(&code, 0, &replacement_value, &[Value::int(1)]),
            BuiltinCallProbe::EligibleMiss(BuiltinCallCacheMiss::Class)
        ));

        assert!(matches!(
            Interpreter::probe_builtin_vectorcall(
                &code,
                0,
                &Value::builtin_function("len"),
                &[Value::tuple(Vec::new())]
            ),
            BuiltinCallProbe::EligibleMiss(BuiltinCallCacheMiss::Registry)
        ));
        execute_call(
            &code,
            Value::builtin_function("len"),
            vec![Value::tuple(Vec::new())],
        )
        .expect("a registry callable must replace a negative class key");
        assert!(matches!(
            code.call_builtin_cache.borrow().get(0),
            Some(crate::interpreter::CallBuiltinCacheEntry::Cached(entry))
                if matches!(
                    entry.key,
                    crate::interpreter::formatting::CallBuiltinCacheKey::RegistryName("len")
                )
        ));
    }

    #[test]
    fn call_memo_bypass_performs_one_builtin_cache_probe() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let mut code = empty_code(vec![Insn::CallMemo(0, 0), Insn::Return(0)]);
        code.num_regs = 1;
        code.num_locals = 1;
        let class = Rc::new(RefCell::new(pyrust_core::PyClass::new(
            "MemoBypass",
            "MemoBypass",
            Some(crate::interpreter::object_class_singleton()),
            indexmap::IndexMap::new(),
        )));

        execute_call(&code, Value::py_class(Rc::clone(&class)), Vec::new())
            .expect("CallMemo bypass must execute ordinary construction");
        assert!(matches!(
            code.call_builtin_cache.borrow().get(0),
            Some(crate::interpreter::CallBuiltinCacheEntry::ClassAfterPrimitiveMiss(cached))
                if cached.as_ptr() == Rc::as_ptr(&class)
        ));
        execute_call(&code, Value::py_class(class), Vec::new())
            .expect("CallMemo warm negative must keep executing the generic tail");
    }

    #[test]
    fn exact_builtin_class_call_populates_typed_cache_metadata() {
        use crate::interpreter::formatting::CallBuiltinCacheKey;
        use pyrust_core::BuiltinTypeClassTag;

        let cases = vec![
            (
                "zip",
                BuiltinTypeClassTag::Zip,
                vec![Value::tuple(Vec::new()), Value::tuple(Vec::new())],
            ),
            (
                "map",
                BuiltinTypeClassTag::Map,
                vec![Value::builtin_function("len"), Value::tuple(Vec::new())],
            ),
            (
                "filter",
                BuiltinTypeClassTag::Filter,
                vec![Value::none(), Value::tuple(Vec::new())],
            ),
            (
                "enumerate",
                BuiltinTypeClassTag::Enumerate,
                vec![Value::tuple(Vec::new())],
            ),
            ("slice", BuiltinTypeClassTag::Slice, vec![Value::int(1)]),
            (
                "reversed",
                BuiltinTypeClassTag::Reversed,
                vec![Value::tuple(Vec::new())],
            ),
        ];

        for (name, expected_tag, arguments) in cases {
            let code = call_code(arguments.len() as u8);
            let class = crate::interpreter::builtin_type_class_by_name(name)
                .expect("the canonical class must be initialized");
            assert_eq!(class.borrow().builtin_type_tag, Some(expected_tag));
            assert_eq!(
                crate::interpreter::BuiltinTypeClass::from_tag(expected_tag).class_name(),
                name
            );
            execute_call(
                &code,
                Value::py_class(std::rc::Rc::clone(&class)),
                arguments,
            )
            .expect("the canonical class call must execute");

            let registration = crate::builtin_registry::lookup_registration(name)
                .expect("the canonical class constructor must be registered");
            match code.call_builtin_cache.borrow().get(0) {
                Some(crate::interpreter::CallBuiltinCacheEntry::Cached(entry))
                    if matches!(entry.key, CallBuiltinCacheKey::PrimitiveClass(_)) =>
                {
                    let CallBuiltinCacheKey::PrimitiveClass(actual_class) = &entry.key else {
                        unreachable!()
                    };
                    assert_eq!(actual_class.as_ptr(), std::rc::Rc::as_ptr(&class));
                    match (entry.fast, registration.fast) {
                        (Some((_, minimum, maximum)), Some(_)) => {
                            assert_eq!(minimum, registration.min_arity);
                            assert_eq!(maximum, registration.max_arity);
                        }
                        (None, None) => {}
                        _ => panic!("cached fast metadata disagrees for {name}"),
                    }
                }
                other => panic!("missing typed cache entry for {name}: {other:?}"),
            }
        }
    }

    #[test]
    fn builtin_class_cache_cross_key_and_vectorcall_guards() {
        use crate::interpreter::formatting::CallBuiltinCacheKey;

        let code = call_code(1);
        let empty_tuple = || Value::tuple(Vec::new());
        let enumerate_class = crate::interpreter::builtin_type_class_by_name("enumerate")
            .expect("the canonical enumerate class must be initialized");
        execute_call(
            &code,
            Value::py_class(std::rc::Rc::clone(&enumerate_class)),
            vec![empty_tuple()],
        )
        .expect("enumerate(()) must execute");
        assert!(matches!(
            code.call_builtin_cache.borrow().get(0),
            Some(crate::interpreter::CallBuiltinCacheEntry::Cached(entry))
                if matches!(
                    &entry.key,
                    CallBuiltinCacheKey::PrimitiveClass(class)
                        if class.as_ptr() == std::rc::Rc::as_ptr(&enumerate_class)
                )
        ));

        execute_call(
            &code,
            Value::builtin_function("enumerate"),
            vec![empty_tuple()],
        )
        .expect("the registry-function form must overwrite the class key");
        assert!(matches!(
            code.call_builtin_cache.borrow().get(0),
            Some(crate::interpreter::CallBuiltinCacheEntry::Cached(entry))
                if matches!(entry.key, CallBuiltinCacheKey::RegistryName("enumerate"))
        ));

        assert!(
            execute_call(&code, Value::int(0), vec![empty_tuple()]).is_err(),
            "an uncacheable non-callable must fall through"
        );
        assert!(matches!(
            code.call_builtin_cache.borrow().get(0),
            Some(crate::interpreter::CallBuiltinCacheEntry::Cached(entry))
                if matches!(entry.key, CallBuiltinCacheKey::RegistryName("enumerate"))
        ));

        let reversed_class = crate::interpreter::builtin_type_class_by_name("reversed")
            .expect("the canonical reversed class must be initialized");
        execute_call(
            &code,
            Value::py_class(std::rc::Rc::clone(&reversed_class)),
            vec![empty_tuple()],
        )
        .expect("a class-tag mismatch must overwrite the registry key");
        assert!(matches!(
            code.call_builtin_cache.borrow().get(0),
            Some(crate::interpreter::CallBuiltinCacheEntry::Cached(entry))
                if matches!(
                    &entry.key,
                    CallBuiltinCacheKey::PrimitiveClass(class)
                        if class.as_ptr() == std::rc::Rc::as_ptr(&reversed_class)
                )
        ));

        let function = Value::py_class(reversed_class);
        assert!(
            matches!(
                Interpreter::probe_builtin_vectorcall(&code, 0, &function, &[]),
                BuiltinCallProbe::Expanded(_)
            ),
            "wrong positional arity must retain the cached expanded dispatch"
        );
        assert!(
            matches!(
                Interpreter::probe_builtin_vectorcall(&code, 0, &function, &[Value::unset()]),
                BuiltinCallProbe::Expanded(_)
            ),
            "an unset register must retain the cached expanded dispatch"
        );
    }

    #[test]
    fn builtin_class_cache_weak_identity_prevents_aba_alias() {
        use crate::interpreter::formatting::CallBuiltinCacheKey;

        let template = crate::interpreter::builtin_type_class_by_name("zip")
            .expect("the canonical zip class must be initialized");
        let old_class = std::rc::Rc::new(std::cell::RefCell::new(template.borrow().clone()));
        let old_ptr = std::rc::Rc::as_ptr(&old_class);
        let key = CallBuiltinCacheKey::PrimitiveClass(std::rc::Rc::downgrade(&old_class));
        drop(old_class);

        let replacement = std::rc::Rc::new(std::cell::RefCell::new(template.borrow().clone()));
        match key {
            CallBuiltinCacheKey::PrimitiveClass(class) => {
                assert!(class.upgrade().is_none());
                assert_eq!(class.as_ptr(), old_ptr);
                assert_ne!(
                    class.as_ptr(),
                    std::rc::Rc::as_ptr(&replacement),
                    "a live weak control block must prevent allocator-address reuse"
                );
            }
            CallBuiltinCacheKey::RegistryName(_) => panic!("expected a class identity key"),
        }
    }

    #[test]
    fn every_primitive_dispatch_identity_populates_the_call_site_cache() {
        use crate::interpreter::{DictViewClass, NativeIteratorClass};

        let mut classes = Vec::new();
        for name in [
            "bool",
            "bytearray",
            "bytes",
            "complex",
            "dict",
            "float",
            "frozenset",
            "int",
            "list",
            "set",
            "str",
            "tuple",
            "NoneType",
            "NotImplementedType",
            "ellipsis",
        ] {
            classes.push(
                crate::interpreter::primitive_class_by_name(name)
                    .unwrap_or_else(|| panic!("missing primitive class {name}")),
            );
        }
        classes.push(crate::interpreter::type_class_singleton());
        classes.push(crate::interpreter::range_class_singleton());
        for name in ["zip", "map", "filter", "enumerate", "slice", "reversed"] {
            classes.push(
                crate::interpreter::builtin_type_class_by_name(name)
                    .unwrap_or_else(|| panic!("missing built-in class {name}")),
            );
        }
        classes.extend(DictViewClass::ALL.into_iter().map(DictViewClass::singleton));
        classes.extend(
            NativeIteratorClass::ALL
                .into_iter()
                .map(NativeIteratorClass::singleton),
        );

        assert_eq!(classes.len(), 32);
        let identities: std::collections::HashSet<_> =
            classes.iter().map(std::rc::Rc::as_ptr).collect();
        assert_eq!(
            identities.len(),
            classes.len(),
            "the primitive dispatch inventory must contain 32 unique classes"
        );

        for class in classes {
            let name = class.borrow().name.clone();
            let expected_tag = class.borrow().builtin_type_tag;
            let expected_fast = expected_tag.and_then(|tag| {
                let registration_name =
                    crate::interpreter::BuiltinTypeClass::from_tag(tag).class_name();
                crate::builtin_registry::lookup_registration(registration_name).and_then(
                    |registration| {
                        registration
                            .fast
                            .map(|_| (registration.min_arity, registration.max_arity))
                    },
                )
            });
            assert!(
                crate::interpreter::primitive_class_dispatch(&class).is_some(),
                "{name} is missing from the primitive dispatch table"
            );
            let code = call_code(0);
            let _ = execute_call(
                &code,
                Value::py_class(std::rc::Rc::clone(&class)),
                Vec::new(),
            );
            match code.call_builtin_cache.borrow().get(0) {
                Some(crate::interpreter::CallBuiltinCacheEntry::Cached(entry))
                    if matches!(
                        entry.key,
                        crate::interpreter::formatting::CallBuiltinCacheKey::PrimitiveClass(_)
                    ) =>
                {
                    let crate::interpreter::formatting::CallBuiltinCacheKey::PrimitiveClass(
                        cached_class,
                    ) = &entry.key
                    else {
                        unreachable!()
                    };
                    assert_eq!(cached_class.as_ptr(), std::rc::Rc::as_ptr(&class));
                    match (entry.fast, expected_fast) {
                        (Some((_, minimum, maximum)), Some((expected_min, expected_max))) => {
                            assert_eq!((minimum, maximum), (expected_min, expected_max));
                        }
                        (None, None) => {}
                        _ => panic!("{name} has incorrect vectorcall metadata"),
                    }
                }
                other => panic!("{name} did not populate a primitive-class entry: {other:?}"),
            }
        }
    }

    #[test]
    fn builtin_class_cache_borrow_conflicts_are_adaptive() {
        let class = crate::interpreter::builtin_type_class_by_name("reversed")
            .expect("the canonical reversed class must be initialized");
        let function = Value::py_class(std::rc::Rc::clone(&class));
        let warm_code = call_code(1);
        execute_call(&warm_code, function.clone(), vec![Value::tuple(Vec::new())])
            .expect("the first reversed call must populate the cache");

        let mut interpreter = Interpreter::default();
        let class_borrow = class.borrow_mut();
        let arguments = [Value::tuple(Vec::new())];
        let BuiltinCallProbe::Vector(warm_dispatch) =
            Interpreter::probe_builtin_vectorcall(&warm_code, 0, &function, &arguments)
        else {
            panic!("a warm identity hit must return its copied vector dispatch");
        };
        assert!(
            warm_dispatch(&mut interpreter, &arguments).is_ok(),
            "the warm fast dispatch must execute while the class is borrowed"
        );

        let cold_code = call_code(1);
        execute_call(&cold_code, function.clone(), vec![Value::tuple(Vec::new())])
            .expect("a cold borrow conflict must fall back to generic class dispatch");
        assert!(
            matches!(
                cold_code.call_builtin_cache.borrow().get(0),
                None | Some(crate::interpreter::CallBuiltinCacheEntry::Empty)
            ),
            "a cold borrow conflict must decline to fill the class cache"
        );
        drop(class_borrow);

        execute_call(&cold_code, function, vec![Value::tuple(Vec::new())])
            .expect("the next unborrowed call must populate the cache");
        assert!(
            !matches!(
                cold_code.call_builtin_cache.borrow().get(0),
                None | Some(crate::interpreter::CallBuiltinCacheEntry::Empty)
            ),
            "the cold miss must remain adaptive after the conflict ends"
        );
    }

    #[test]
    fn setup_except_negative_offset_returns_error() {
        // SetupExcept(-100): handler_pc = 1 + (-100) < 0 → error at push time
        let code = empty_code(vec![Insn::SetupExcept(-100), Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(
            result.is_err(),
            "expected Err for SetupExcept with OOB offset, got {:?}",
            result
        );
    }

    #[test]
    fn generator_drive_hot_struct_sizes_stay_compact() {
        use super::{
            GenDriveFrame, GeneratorFrame, IterCacheBuf, IterState, ItersBuf, LoopFastOutcome,
            LoopIteratorAdvance,
        };
        use std::mem::size_of;

        assert!(
            size_of::<IterState>() <= 40,
            "IterState grew to {} B (guard <= 40 B; keep guarded iterator state boxed)",
            size_of::<IterState>()
        );
        assert!(
            size_of::<Option<IterState>>() <= 40,
            "Option<IterState> grew to {} B (guard <= 40 B)",
            size_of::<Option<IterState>>()
        );
        assert!(
            size_of::<ItersBuf>() <= 56,
            "ItersBuf grew to {} B (guard <= 56 B; copied 4x per generator drive cycle)",
            size_of::<ItersBuf>()
        );
        assert!(
            size_of::<IterCacheBuf>() <= 56,
            "IterCacheBuf grew to {} B (guard <= 56 B; keep IterNextCacheEntry boxed)",
            size_of::<IterCacheBuf>()
        );
        assert_eq!(
            size_of::<LoopIteratorAdvance>(),
            16,
            "LoopIteratorAdvance grew to {} B (expected 16 B with boxed PyError)",
            size_of::<LoopIteratorAdvance>()
        );
        assert_eq!(
            size_of::<LoopFastOutcome>(),
            16,
            "LoopFastOutcome grew to {} B (expected 16 B with boxed PyError)",
            size_of::<LoopFastOutcome>()
        );
        assert!(
            size_of::<GeneratorFrame>() <= 304,
            "GeneratorFrame grew to {} B (guard <= 304 B)",
            size_of::<GeneratorFrame>()
        );
        assert!(
            size_of::<GenDriveFrame>() <= 256,
            "GenDriveFrame grew to {} B (guard <= 256 B)",
            size_of::<GenDriveFrame>()
        );
    }

    #[test]
    fn fncode_heap_bytes_per_insn_budget_and_report() {
        use std::collections::HashSet;
        use std::mem::size_of;
        use std::rc::Rc;

        use crate::bytecode::{
            AttrCacheEntry, BinOpCacheEntry, GlobalCacheEntry, KwCallCacheEntry,
        };

        fn sum_fncode_stats(code: &FnCode) -> (usize, usize, usize, usize) {
            let mut insns = code.insns.len();
            let mut base_heap = code.insns.capacity() * size_of::<Insn>()
                + code.lineno_table.capacity() * size_of::<u32>()
                + code.col_table.capacity() * size_of::<crate::ast::CaretSpan>()
                + code.consts.capacity() * size_of::<Value>()
                + code.names.capacity() * size_of::<pyrust_core::Symbol>()
                + code.global_cache_interest_masks.capacity() * size_of::<u64>();
            let mut cache_heap = code.attr_cache.borrow().heap_bytes()
                + code.global_cache.borrow().capacity() * size_of::<GlobalCacheEntry>()
                + code.binop_cache.borrow().capacity() * size_of::<BinOpCacheEntry>()
                + code.kwcall_cache.borrow().heap_bytes()
                + code.fmt_spec_cache.borrow().heap_bytes()
                + code.call_builtin_cache.borrow().heap_bytes();
            let mut exc_heap = code.exc_table.capacity() * size_of::<u32>();

            for proto in &code.fn_protos {
                let (i, b, c, e) = sum_fncode_stats(&proto.code);
                insns += i;
                base_heap += b;
                cache_heap += c;
                exc_heap += e;
            }
            (insns, base_heap, cache_heap, exc_heap)
        }

        fn sum_names(code: &FnCode) -> usize {
            code.names.len()
                + code
                    .fn_protos
                    .iter()
                    .map(|p| sum_names(&p.code))
                    .sum::<usize>()
        }

        let src = r#"
class Point:
    __slots__ = ("x", "y")
    def __init__(self, x, y):
        self.x = x
        self.y = y
    def norm2(self):
        return self.x * self.x + self.y * self.y

class Accumulator:
    def __init__(self, init=0.0):
        self.total = init
    def add(self, value, scale=1.0):
        self.total = self.total + value * scale
        return self.total
    def report(self):
        return f"total={self.total:.2f}"

def helper_unused_a(items):
    out = []
    for item in items:
        if item > 0:
            out.append(item * 2)
    return len(out)

def helper_unused_b(a, b, c=1):
    return (a + b) * c

def driver():
    acc = Accumulator(init=1.0)
    pts = [Point(i, i + 1) for i in range(10)]
    for p in pts:
        acc.add(float(p.norm2()), scale=0.5)
    return len(acc.report())

out = driver()
assert out > 0
"#;

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
        let raw =
            crate::compiler::compile_script_with_linenos(&stmts, local_index, false, &[], "<test>")
                .unwrap();
        let optimized = crate::optimizer::optimize(raw);

        let (insns, base_heap, cold_cache_heap, cold_exc_heap) = sum_fncode_stats(&optimized);
        assert!(insns > 0);
        assert_eq!(
            cold_cache_heap, 0,
            "cold compiled FnCode must allocate 0 bytes of inline caches"
        );
        assert_eq!(
            cold_exc_heap, 0,
            "handler-free compiled FnCode must allocate 0 bytes for exc_table"
        );
        let cold_total_per_insn =
            (base_heap + cold_cache_heap + cold_exc_heap) as f64 / insns as f64;
        assert!(
            cold_total_per_insn < 80.0,
            "cold FnCode heap bytes/insn regressed to {cold_total_per_insn:.1} B/insn"
        );

        let mut interp = Interpreter::default();
        let mut regs = vec![Value::unset(); optimized.num_regs as usize];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        interp
            .run_bytecode(&optimized, regs_slice)
            .expect("driver workload must execute");

        let (_, _, warm_cache_heap, warm_exc_heap) = sum_fncode_stats(&optimized);
        assert!(
            warm_cache_heap > 0,
            "executing the workload must populate at least one inline cache"
        );
        assert_eq!(warm_exc_heap, 0);
        let warm_cache_per_insn = warm_cache_heap as f64 / insns as f64;
        let warm_total_per_insn =
            (base_heap + warm_cache_heap + warm_exc_heap) as f64 / insns as f64;
        assert!(
            warm_total_per_insn < 85.0,
            "warm FnCode heap bytes/insn regressed to {warm_total_per_insn:.1} B/insn"
        );

        for proto in &optimized.fn_protos {
            if proto.name.starts_with("helper_unused") {
                let (_, _, helper_cache, helper_exc) = sum_fncode_stats(&proto.code);
                assert_eq!(
                    helper_cache, 0,
                    "uncalled function {} must allocate 0 cache bytes",
                    proto.name
                );
                assert_eq!(helper_exc, 0);
            }
        }

        let total_names = sum_names(&optimized);
        let legacy_cache_and_exc = insns * (120 + 64 + 48 + 24 + 2 + 4) + total_names * 72;
        let legacy_total_per_insn = (base_heap + legacy_cache_and_exc) as f64 / insns as f64;

        eprintln!(
            "ENTRY SIZES: Insn={} KwCall={} Attr={} CallBuiltin={} FmtSpec={} BinOp={} Global={}",
            size_of::<Insn>(),
            size_of::<KwCallCacheEntry>(),
            size_of::<AttrCacheEntry>(),
            size_of::<crate::interpreter::CallBuiltinCacheEntry>(),
            size_of::<crate::interpreter::FmtSpecCacheEntry>(),
            size_of::<BinOpCacheEntry>(),
            size_of::<GlobalCacheEntry>(),
        );
        eprintln!(
            "FNCODE MEM (insns={insns}, names={total_names}): legacy={legacy_total_per_insn:.1} B/insn | cold={cold_total_per_insn:.1} B/insn (cache=0.0 B/insn) | warm={warm_total_per_insn:.1} B/insn (cache={warm_cache_per_insn:.1} B/insn)",
        );
    }
}
