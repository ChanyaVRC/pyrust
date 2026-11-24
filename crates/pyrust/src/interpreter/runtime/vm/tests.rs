#[cfg(test)]
mod vm_tests {
    use super::{BuiltinCallProbe, PositionalCallCacheProbe, RegSlice, Value};
    use crate::bytecode::{FnCode, Insn};
    use crate::interpreter::Interpreter;
    use crate::interpreter::fast_path::BuiltinCallCacheMiss;

    fn empty_code(insns: Vec<Insn>) -> FnCode {
        FnCode {
            insns,
            filename: std::sync::Arc::from("<unknown>"),
            qualname: std::sync::Arc::from("<module>"),
            lineno_table: crate::bytecode::LinenoTable::empty(),
            col_table: crate::bytecode::ColTable::empty(),
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
            GenColdState, GenDriveFrame, GeneratorFrame, IterCacheBuf, IterState, ItersBuf,
            LoopFastOutcome, LoopIteratorAdvance,
        };
        use crate::interpreter::InterpreterColdState;
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
        eprintln!(
            "STRUCT SIZES (#3162): GeneratorFrame={} B, GenColdState={} B, Interpreter={} B, InterpreterColdState={} B, EnvironmentStorage={} B, Environment={} B",
            size_of::<GeneratorFrame>(),
            size_of::<GenColdState>(),
            size_of::<Interpreter>(),
            size_of::<InterpreterColdState>(),
            size_of::<pyrust_core::EnvironmentStorage>(),
            size_of::<pyrust_core::Environment>(),
        );
        assert!(
            size_of::<GeneratorFrame>() <= 224,
            "GeneratorFrame grew to {} B (guard <= 224 B; keep cold exc/async/frame state in GenColdState)",
            size_of::<GeneratorFrame>()
        );
        assert!(
            size_of::<GenDriveFrame>() <= 256,
            "GenDriveFrame grew to {} B (guard <= 256 B)",
            size_of::<GenDriveFrame>()
        );
        assert!(
            size_of::<Interpreter>() <= 320,
            "Interpreter grew to {} B (guard <= 320 B; keep cold tables/buffers in InterpreterColdState)",
            size_of::<Interpreter>()
        );
        assert_eq!(
            size_of::<pyrust_core::EnvironmentStorage>(),
            size_of::<pyrust_core::EnvValues>(),
            "EnvironmentStorage must carry zero discriminant overhead after removing Cells"
        );
    }

    #[test]
    fn generator_cold_state_remains_unallocated_on_happy_path_and_allocates_lazily() {
        use super::GeneratorFrame;
        use pyrust_core::ValueKind;

        let mut interp = Interpreter::default();
        interp
            .exec_source(
                r#"
def plain_gen():
    yield 1
    yield 2
    return 3

def exc_gen():
    try:
        raise ValueError("boom")
    except ValueError:
        yield 42

g = plain_gen()
first = next(g)
assert first == 1

eg = exc_gen()
exc_first = next(eg)
assert exc_first == 42
"#,
                None,
                None,
            )
            .expect("generator setup must execute");

        let g_val = interp.lookup_name("g").unwrap().unwrap();
        let ValueKind::Generator(g_cell) = g_val.kind() else {
            panic!("g must be a generator");
        };
        {
            let borrow = g_cell.borrow();
            let frame = borrow
                .downcast_ref::<GeneratorFrame>()
                .expect("g must hold a GeneratorFrame");
            assert!(
                frame.cold.borrow().is_none(),
                "plain yield/resume generator must not allocate GenColdState"
            );
        }

        // Introspecting gi_frame lazily allocates GenColdState with frame_cache.
        interp
            .exec_source("f = g.gi_frame\nassert f is not None\n", None, None)
            .expect("gi_frame introspection must succeed");
        {
            let borrow = g_cell.borrow();
            let frame = borrow
                .downcast_ref::<GeneratorFrame>()
                .expect("g must hold a GeneratorFrame");
            assert!(
                frame.has_frame_cache(),
                "gi_frame introspection must lazily populate frame_cache"
            );
        }

        // Yielding inside an except block lazily allocates GenColdState with exc state.
        let eg_val = interp.lookup_name("eg").unwrap().unwrap();
        let ValueKind::Generator(eg_cell) = eg_val.kind() else {
            panic!("eg must be a generator");
        };
        {
            let borrow = eg_cell.borrow();
            let frame = borrow
                .downcast_ref::<GeneratorFrame>()
                .expect("eg must hold a GeneratorFrame");
            assert!(
                frame.has_exc_state(),
                "generator yielding inside except must persist exc state in GenColdState"
            );
        }
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
                + code.lineno_table.heap_bytes()
                + code.col_table.heap_bytes()
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

        let (tokens, line_nos, cols, cols_end) = crate::lexer::Lexer::new(src)
            .unwrap()
            .into_tokens_with_pos();
        let mut parser = crate::parser::Parser::new_with_pos(tokens, line_nos, cols, cols_end);
        let (stmts, stmt_linenos) = parser.parse_program_with_linenos().unwrap();
        let empty = HashSet::new();
        let names = crate::interpreter::collect_local_names(&[], &stmts, &empty, &empty);
        let local_index = Rc::new(
            (0u32..)
                .zip(names)
                .map(|(slot, name)| (name, slot))
                .collect(),
        );
        let raw = crate::compiler::compile_script_with_linenos(
            &stmts,
            local_index,
            false,
            &stmt_linenos,
            "<test>",
        )
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

        fn sum_debug_bytes(code: &FnCode) -> (usize, usize) {
            let mut lineno_bytes = code.lineno_table.heap_bytes();
            let mut col_bytes = code.col_table.heap_bytes();
            for proto in &code.fn_protos {
                let (l, c) = sum_debug_bytes(&proto.code);
                lineno_bytes += l;
                col_bytes += c;
            }
            (lineno_bytes, col_bytes)
        }

        let (lineno_bytes, col_bytes) = sum_debug_bytes(&optimized);
        let debug_bytes_per_insn = (lineno_bytes + col_bytes) as f64 / insns as f64;
        assert!(
            debug_bytes_per_insn < 10.0,
            "compressed debug tables regressed to {debug_bytes_per_insn:.2} B/insn (dense baseline 20.0 B/insn)"
        );

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
            "DEBUG TABLES (insns={insns}): lineno={:.2} B/insn | col={:.2} B/insn | total={debug_bytes_per_insn:.2} B/insn (dense baseline=20.00 B/insn)",
            lineno_bytes as f64 / insns as f64,
            col_bytes as f64 / insns as f64,
        );
        eprintln!(
            "FNCODE MEM (insns={insns}, names={total_names}): legacy={legacy_total_per_insn:.1} B/insn | cold={cold_total_per_insn:.1} B/insn (cache=0.0 B/insn) | warm={warm_total_per_insn:.1} B/insn (cache={warm_cache_per_insn:.1} B/insn)",
        );
    }

    #[test]
    fn vm_frame_view_and_frame_outcome_sizes_stay_compact() {
        use super::{FrameOutcome, TRAMP_ARENA_CAPACITY, TrampFrame, VmFrameView};
        use std::mem::size_of;

        assert!(
            size_of::<VmFrameView>() <= 56,
            "VmFrameView grew to {} B (guard <= 56 B; keep FrameOwner compact)",
            size_of::<VmFrameView>()
        );
        assert!(
            size_of::<FrameOutcome>() <= 24,
            "FrameOutcome grew to {} B (guard <= 24 B; keep GenSaveState boxed)",
            size_of::<FrameOutcome>()
        );
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(size_of::<VmFrameView>(), 48);
            assert_eq!(size_of::<FrameOutcome>(), 16);
        }
        assert!(
            size_of::<TrampFrame>() <= 88,
            "TrampFrame grew to {} B (guard <= 88 B)",
            size_of::<TrampFrame>()
        );
        assert_eq!(TRAMP_ARENA_CAPACITY, 16 * 1024);
    }

    #[test]
    fn precomputed_trampoline_arity_matches_eligibility_rules() {
        use pyrust_core::{UserFunction, ValueKind};

        let mut interp = Interpreter::default();
        interp
            .exec_source(
                r#"
g = 0
def eligible0():
    return 1
def eligible2(a, b):
    return a + b
def eligible_default(a, b=10):
    return a + b
def has_args(*args):
    return args
def has_kwargs(**kwargs):
    return kwargs
def has_kwonly(a, *, b):
    return a + b
def uses_global(x):
    global g
    g = x
    return x
def is_gen(x):
    yield x
async def is_coro(x):
    return x
def has_loop(xs):
    s = 0
    for x in xs:
        s += x
    return s
def has_try(x):
    try:
        return 1 // x
    except ZeroDivisionError:
        return 0
def has_own_cell(x):
    def inner(y):
        return x + y
    return inner
closure_no_own_cells = has_own_cell(5)
class C:
    @staticmethod
    def sm(x):
        return x
    @classmethod
    def cm(cls, x):
        return x
sm_fn = C.__dict__['sm']
cm_fn = C.__dict__['cm']
"#,
                None,
                None,
            )
            .expect("definitions must execute");

        let arity_of = |name: &str| -> u16 {
            let val = interp
                .lookup_name(name)
                .expect("lookup succeeds")
                .unwrap_or_else(|| panic!("missing binding {name}"));
            let ValueKind::UserFunction(f) = val.kind() else {
                panic!("{name} must be a UserFunction");
            };
            f.trampoline_arity
        };

        assert_eq!(arity_of("eligible0"), 0);
        assert_eq!(arity_of("eligible2"), 2);
        assert_eq!(arity_of("eligible_default"), 2);
        assert_eq!(arity_of("closure_no_own_cells"), 1);
        assert_eq!(arity_of("has_args"), UserFunction::TRAMPOLINE_INELIGIBLE);
        assert_eq!(arity_of("has_kwargs"), UserFunction::TRAMPOLINE_INELIGIBLE);
        assert_eq!(arity_of("has_kwonly"), UserFunction::TRAMPOLINE_INELIGIBLE);
        assert_eq!(arity_of("uses_global"), UserFunction::TRAMPOLINE_INELIGIBLE);
        assert_eq!(arity_of("is_gen"), UserFunction::TRAMPOLINE_INELIGIBLE);
        assert_eq!(arity_of("is_coro"), UserFunction::TRAMPOLINE_INELIGIBLE);
        assert_eq!(arity_of("has_loop"), UserFunction::TRAMPOLINE_INELIGIBLE);
        assert_eq!(arity_of("has_try"), UserFunction::TRAMPOLINE_INELIGIBLE);
        assert_eq!(
            arity_of("has_own_cell"),
            UserFunction::TRAMPOLINE_INELIGIBLE
        );
        assert_eq!(arity_of("sm_fn"), UserFunction::TRAMPOLINE_INELIGIBLE);
        assert_eq!(arity_of("cm_fn"), UserFunction::TRAMPOLINE_INELIGIBLE);
    }

    #[test]
    fn shared_tramp_arena_preserves_pointer_stability_across_reentrancy_and_unwind() {
        use super::TRAMP_ARENA_CAPACITY;

        let mut interp = Interpreter::default();
        assert_eq!(interp.tramp_arena.capacity(), 0);

        // 1. Re-entrant native <-> Python callbacks (map / sorted / __getattr__)
        //    with live outer trampolined frames sharing the single tramp_arena.
        interp
            .exec_source(
                r#"
def leaf(x):
    return x * 2 + 1

def deep_tramp(n):
    if n <= 0:
        return 1
    return 1 + deep_tramp(n - 1)

class AttrProxy:
    def __init__(self, bias):
        self.bias = bias
    def __getattr__(self, name):
        return deep_tramp(len(name)) + self.bias

def reentrant_outer(xs, proxy):
    # Outer frame is trampolined; calls into native map/sorted and __getattr__
    # which each open nested VM entries that trampoline onto the same arena.
    mapped = list(map(deep_tramp, xs))
    ordered = sorted(xs, key=leaf, reverse=True)
    attr_val = proxy.abcdef
    return (mapped, ordered, attr_val, deep_tramp(5))

res = reentrant_outer([3, 1, 4, 2], AttrProxy(10))
assert res == ([4, 2, 5, 3], [4, 3, 2, 1], 17, 6), res
"#,
                None,
                None,
            )
            .expect("re-entrant native callbacks must preserve live trampolined frames");

        assert_eq!(interp.tramp_arena.capacity(), TRAMP_ARENA_CAPACITY);
        assert!(interp.tramp_arena.is_empty());
        assert!(interp.tramp_stack.is_empty());
        assert!(interp.gen_drive_stack.is_empty());
        let initial_arena_ptr = interp.tramp_arena.as_ptr();

        // 2. Exception unwinding across nested trampoline entries (caught and uncaught).
        interp
            .exec_source(
                r#"
def boom(n):
    if n <= 0:
        raise ValueError("boom")
    return boom(n - 1)

def key_fn(x):
    if x == 2:
        return boom(4)
    return leaf(x)

def outer_catch(xs):
    try:
        return sorted(xs, key=key_fn)
    except ValueError as e:
        return ("caught", str(e), deep_tramp(6))

caught_res = outer_catch([1, 2, 3])
assert caught_res == ("caught", "boom", 7), caught_res

def outer_tramp_with_inner_catch(x):
    return (x, outer_catch([1, 2, 3]), leaf(x))

assert outer_tramp_with_inner_catch(9) == (9, ("caught", "boom", 7), 19)
"#,
                None,
                None,
            )
            .expect("caught exception unwinding across nested trampoline entries must succeed");

        assert_eq!(interp.tramp_arena.as_ptr(), initial_arena_ptr);
        assert_eq!(interp.tramp_arena.capacity(), TRAMP_ARENA_CAPACITY);
        assert!(interp.tramp_arena.is_empty());
        assert!(interp.tramp_stack.is_empty());
        assert!(interp.gen_drive_stack.is_empty());

        // Uncaught exception escaping nested trampoline entries must still unwind cleanly.
        let uncaught = interp.exec_source(
            r#"
def wrapper_uncaught(xs):
    return sorted(xs, key=key_fn)
wrapper_uncaught([1, 2, 3])
"#,
            None,
            None,
        );
        assert!(uncaught.is_err());
        assert_eq!(interp.tramp_arena.as_ptr(), initial_arena_ptr);
        assert!(interp.tramp_arena.is_empty());
        assert!(interp.tramp_stack.is_empty());
        assert!(interp.gen_drive_stack.is_empty());

        // 3. Interleaved driven generator calling next() on another generator +
        //    trampolined calls + exception unwind (guards `gen_base` / `tramp_base`).
        interp
            .exec_source(
                r#"
def inner_gen(n):
    i = 0
    while i < n:
        yield leaf(i)
        i += 1

def outer_gen(n):
    g = inner_gen(n)
    i = 0
    while i < n:
        v = next(g)
        yield (v, deep_tramp(i))
        i += 1

collected = []
for pair in outer_gen(5):
    collected.append(pair)
assert collected == [(1, 1), (3, 2), (5, 3), (7, 4), (9, 5)], collected

def raising_inner_gen():
    yield leaf(1)
    boom(3)

def outer_gen_err():
    g = raising_inner_gen()
    yield next(g)
    yield next(g)

got = []
try:
    for item in outer_gen_err():
        got.append(item)
except ValueError as e:
    got.append(str(e))
assert got == [3, "boom"], got
"#,
                None,
                None,
            )
            .expect("interleaved driven generators and nested next() must respect gen_base");

        assert_eq!(interp.tramp_arena.as_ptr(), initial_arena_ptr);
        assert!(interp.tramp_arena.is_empty());
        assert!(interp.tramp_stack.is_empty());
        assert!(interp.gen_drive_stack.is_empty());

        // 4. Deep recursion that exhausts TRAMP_ARENA_CAPACITY and falls back to
        //    native frames without reallocating tramp_arena, plus RecursionError.
        //    Keep ~40 live registers across the recursive call so the arena
        //    fills well before the debug-build native stack limit.
        interp
            .exec_source(
                r#"
def wide_rec(n):
    a0 = n + 1
    a1 = n + 2
    a2 = n + 3
    a3 = n + 4
    a4 = n + 5
    a5 = n + 6
    a6 = n + 7
    a7 = n + 8
    a8 = n + 9
    a9 = n + 10
    b0 = a0 + a1
    b1 = a2 + a3
    b2 = a4 + a5
    b3 = a6 + a7
    b4 = a8 + a9
    c0 = b0 + b1
    c1 = b2 + b3
    c2 = c0 + c1 + b4
    d0 = c2 + a0
    d1 = d0 + a1
    d2 = d1 + a2
    d3 = d2 + a3
    d4 = d3 + a4
    d5 = d4 + a5
    d6 = d5 + a6
    d7 = d6 + a7
    d8 = d7 + a8
    d9 = d8 + a9
    if n <= 0:
        return 0
    sub = wide_rec(n - 1)
    check = (
        a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9
        + b0 + b1 + b2 + b3 + b4 + c0 + c1 + c2
        + d0 + d1 + d2 + d3 + d4 + d5 + d6 + d7 + d8 + d9
    )
    return sub + (1 if check >= 0 else 0)
"#,
                None,
                None,
            )
            .expect("wide_rec definition must execute");

        let wide_rec_num_regs = {
            let val = interp
                .lookup_name("wide_rec")
                .expect("lookup succeeds")
                .expect("wide_rec defined");
            let pyrust_core::ValueKind::UserFunction(f) = val.kind() else {
                panic!("wide_rec must be a UserFunction");
            };
            assert_eq!(
                f.trampoline_arity, 1,
                "wide_rec must be trampoline-eligible"
            );
            f.precompiled_code
                .as_deref()
                .and_then(|any| any.downcast_ref::<FnCode>())
                .expect("wide_rec must have precompiled FnCode")
                .num_regs as usize
        };
        let arena_frames = TRAMP_ARENA_CAPACITY / wide_rec_num_regs;
        let overflow_depth = arena_frames + 5;
        let limit = overflow_depth + 5;
        let too_deep = limit + 20;
        assert!(
            overflow_depth * wide_rec_num_regs > TRAMP_ARENA_CAPACITY,
            "test must actually overflow TRAMP_ARENA_CAPACITY"
        );

        interp
            .exec_source(
                &format!(
                    r#"
import sys
old_limit = sys.getrecursionlimit()
sys.setrecursionlimit({limit})
try:
    assert wide_rec({overflow_depth}) == {overflow_depth}

    # Exceed recursion limit across the trampoline + native fallback boundary.
    hit_rec_err = False
    try:
        wide_rec({too_deep})
    except RecursionError:
        hit_rec_err = True
    assert hit_rec_err
finally:
    sys.setrecursionlimit(old_limit)
"#
                ),
                None,
                None,
            )
            .expect("deep recursion overflowing tramp_arena must fall back without reallocating");

        assert_eq!(
            interp.tramp_arena.as_ptr(),
            initial_arena_ptr,
            "tramp_arena must never reallocate once reserved"
        );
        assert_eq!(interp.tramp_arena.capacity(), TRAMP_ARENA_CAPACITY);
        assert!(interp.tramp_arena.is_empty());
        assert!(interp.tramp_stack.is_empty());
        assert!(interp.gen_drive_stack.is_empty());
    }

    #[test]
    fn load_const_interns_short_strings_at_fncode_build() {
        let src = r#"
def first():
    return "shared_short_key_3157"

def second():
    return "shared_short_key_3157"

assert first() is second()
assert first() == "shared_short_key_3157"
"#;
        let tokens = crate::lexer::Lexer::new(src).unwrap().into_tokens();
        let mut parser = crate::parser::Parser::new(tokens);
        let stmts = parser.parse_program().unwrap();
        let code = crate::compiler::compile_script_with_linenos(
            &stmts,
            std::rc::Rc::new(std::collections::HashMap::new()),
            false,
            &[],
            "<intern-test>",
        )
        .unwrap();
        let optimized = crate::optimizer::optimize(code);
        let first_const = optimized
            .fn_protos
            .iter()
            .find(|p| p.name.as_ref() == "first")
            .and_then(|p| {
                p.code
                    .consts
                    .iter()
                    .find(|v| v.as_str() == Some("shared_short_key_3157"))
            })
            .expect("first() must keep string constant");
        let second_const = optimized
            .fn_protos
            .iter()
            .find(|p| p.name.as_ref() == "second")
            .and_then(|p| {
                p.code
                    .consts
                    .iter()
                    .find(|v| v.as_str() == Some("shared_short_key_3157"))
            })
            .expect("second() must keep string constant");
        assert_eq!(
            first_const.value_id(),
            second_const.value_id(),
            "short string constants across FnCode pools must be pre-interned at build time"
        );

        let mut interp = Interpreter::default();
        interp
            .exec_source(src, None, None)
            .expect("pre-interned LoadConst identity check must succeed");
    }

    fn run_script_with_tracebacks(src: &str, filename: &str) -> crate::error::Result<()> {
        let (tokens, line_nos, cols, cols_end) = crate::lexer::Lexer::new(src)
            .unwrap()
            .into_tokens_with_pos();
        let mut parser = crate::parser::Parser::new_with_pos(tokens, line_nos, cols, cols_end);
        let (stmts, linenos) = parser.parse_program_with_linenos().unwrap();
        let mut interp = Interpreter::default();
        interp.cold.script_filename = Some(std::sync::Arc::from(filename));
        interp.exec_program_with_linenos(&stmts, &linenos, src, false)
    }

    #[test]
    fn lazy_line_resolution_preserves_f_lineno_and_generator_suspended_line() {
        let src = "\
import sys

def tramp_callee(x):
    a = sys._getframe().f_lineno
    gf = sys._getframe
    b = gf().f_lineno
    return (a, b)

def gen_throw_caught():
    try:
        yield 1
    except ValueError as e:
        yield e.__traceback__.tb_lineno

def gen_throw_uncaught():
    yield 10

def sub_gen():
    yield 100

def delegator():
    yield from sub_gen()

# 1. sys._getframe().f_lineno via both CallMethod and Call inside a trampolined callee.
a, b = tramp_callee(1)
assert a == 4, f'expected line 4, got {a}'
assert b == 6, f'expected line 6, got {b}'

# 2. generator.gi_frame.f_lineno before first next() and while suspended.
g_unstarted = gen_throw_uncaught()
assert g_unstarted.gi_frame.f_lineno == 15, g_unstarted.gi_frame.f_lineno
assert next(g_unstarted) == 10
assert g_unstarted.gi_frame.f_lineno == 16, g_unstarted.gi_frame.f_lineno

# 3. generator.throw() caught inside generator reports the yield line (#2445).
g = gen_throw_caught()
assert next(g) == 1
caught_line = g.throw(ValueError('boom'))
assert caught_line == 11, f'expected yield line 11, got {caught_line}'

# 4. generator.throw() escaping generator reports the yield line (#2445).
g2 = gen_throw_uncaught()
assert next(g2) == 10
try:
    g2.throw(RuntimeError('escape'))
except RuntimeError as e:
    tb = e.__traceback__
    lines = []
    while tb is not None:
        lines.append(tb.tb_lineno)
        tb = tb.tb_next
    assert lines[-1] == 16, f'expected yield line 16 at inner frame, got {lines}'

# 5. yield from suspended line parity.
d = delegator()
assert next(d) == 100
assert d.gi_frame.f_lineno == 22, d.gi_frame.f_lineno
";
        run_script_with_tracebacks(src, "<lazy-line-test>")
            .expect("lazy f_lineno and generator suspended_line parity must hold");
    }

    #[test]
    fn lazy_line_and_sparse_col_preserve_pep657_carets_across_trampoline_and_generators() {
        // 1. Trampolined call chain preserves both caller call-site and inner BinOp carets (#2411/#2443).
        let tramp_src = "\
def boom(a, b):
    return a / b

def caller(x):
    return boom(x, 0)

caller(10)
";
        let err = run_script_with_tracebacks(tramp_src, "<pep657-tramp>")
            .expect_err("must raise ZeroDivisionError");
        let rendered = err.to_string();
        assert!(
            rendered.contains("line 7")
                && rendered.contains("line 5")
                && rendered.contains("line 2"),
            "all traceback lines must be preserved:\n{rendered}"
        );
        assert!(
            rendered.contains("~~~~^~~~~") || rendered.contains("~~^~~") || rendered.contains("^"),
            "PEP 657 carets must be rendered across trampolined frames:\n{rendered}"
        );
        assert!(
            rendered.contains("caller(10)") && rendered.contains("boom(x, 0)"),
            "caller source lines must appear in traceback:\n{rendered}"
        );

        // 2. Generator driven via `for` (gen-drive trampoline) preserves yield-body PEP 657 caret (#2904).
        let gen_src = "\
def gen():
    yield 1 / 0

def drive():
    for v in gen():
        pass

drive()
";
        let err2 = run_script_with_tracebacks(gen_src, "<pep657-gen>")
            .expect_err("must raise ZeroDivisionError");
        let rendered2 = err2.to_string();
        assert!(
            rendered2.contains("line 2")
                && rendered2.contains("line 5")
                && rendered2.contains("line 8"),
            "generator-drive traceback lines must be preserved:\n{rendered2}"
        );
        assert!(
            rendered2.contains("~~^~~"),
            "generator yield 1 / 0 must preserve PEP 657 caret:\n{rendered2}"
        );

        // 3. Inlined list comprehension preserves PEP 657 caret without synthetic <listcomp> frame (#2993).
        let comp_src = "\
def f():
    return [1 / x for x in [0]]

f()
";
        let err3 = run_script_with_tracebacks(comp_src, "<pep657-comp>")
            .expect_err("must raise ZeroDivisionError");
        let rendered3 = err3.to_string();
        assert!(
            !rendered3.contains("<listcomp>"),
            "inlined comprehension must omit synthetic <listcomp> frame:\n{rendered3}"
        );
        assert!(
            rendered3.contains("line 2") && rendered3.contains("~~^~~"),
            "inlined comprehension must preserve line and PEP 657 caret:\n{rendered3}"
        );
    }
}
