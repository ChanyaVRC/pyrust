#[test]
fn call_depth_restored_after_runtime_error() {
    let tokens =
        Lexer::new("def f():\n    return undefined_var\ntry:\n    f()\nexcept:\n    pass\n")
            .unwrap()
            .into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();
    let depth_before = call_depth();
    interpreter.exec_program(&program, false).unwrap();
    assert_eq!(
        call_depth(),
        depth_before,
        "call_depth must be restored after a runtime error inside a function"
    );
}

#[test]
fn recursion_limit_is_owned_by_each_root_interpreter() {
    let mut first = Interpreter::default();
    let mut second = Interpreter::default();

    first
        .exec_source(
            "import sys\nsys.setrecursionlimit(321)\nfirst_limit = sys.getrecursionlimit()\n",
            None,
            None,
        )
        .unwrap();
    second
        .exec_source(
            "import sys\nsecond_limit = sys.getrecursionlimit()\n",
            None,
            None,
        )
        .unwrap();

    assert_eq!(get_recursion_limit(&first), 321);
    assert_eq!(get_recursion_limit(&second), DEFAULT_RECURSION_LIMIT);
    assert_eq!(
        first.lookup_name("first_limit").unwrap(),
        Some(Value::int(321))
    );
    assert_eq!(
        second.lookup_name("second_limit").unwrap(),
        Some(Value::int(DEFAULT_RECURSION_LIMIT as i64))
    );
}

#[test]
fn generic_alias_type_keeps_its_explicit_metadata() {
    let interpreter = run_program(
        "alias_type = type(list[int])\n\
         module = alias_type.__module__\n\
         doc = alias_type.__doc__\n",
    );

    assert_eq!(
        interpreter.lookup_name("module").unwrap(),
        Some(Value::string("types"))
    );
    assert_eq!(
        interpreter.lookup_name("doc").unwrap(),
        Some(Value::string(
            "Represent a PEP 585 generic type\n\nE.g. for t = list[int], \
             t.__origin__ is list and t.__args__ is (int,)."
        ))
    );
}

#[test]
fn del_list_front_middle_end_produces_correct_result() {
    let interpreter =
        run_program("lst = [1, 2, 3, 4, 5]\ndel lst[2]\ndel lst[0]\ndel lst[-1]\nresult = lst\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![Value::int(2), Value::int(4)]))
    );
}

#[test]
fn dict_update_via_double_splat_call() {
    // DictMergeKwCall instruction is emitted when calling f(**a, **b).
    // Verifies that non-overlapping splats merge correctly.
    let interpreter = run_program(
        "def merge(**kw): return kw\na = {'x': 1, 'y': 2}\nb = {'z': 3}\nresult = merge(**a, **b)\n",
    );
    use crate::value::PyKey;
    let mut expected = pyrust_core::PyDict::default();
    expected.insert(PyKey::str_from("x"), Value::int(1));
    expected.insert(PyKey::str_from("y"), Value::int(2));
    expected.insert(PyKey::str_from("z"), Value::int(3));
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::dict(expected))
    );
}

#[test]
fn double_splat_call_duplicate_key_raises() {
    // A key present in two `**` splats of a call is a TypeError (CPython
    // DICT_MERGE), not a silent overwrite (issue #2413).
    let tokens = Lexer::new(
            "def merge(**kw): return kw\na = {'x': 1, 'y': 2}\nb = {'y': 99, 'z': 3}\nresult = merge(**a, **b)\n",
        )
        .unwrap()
        .into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();
    let err = interpreter.exec_program(&program, false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "TypeError: __main__.merge() got multiple values for keyword argument 'y'"
    );
}

#[test]
fn function_with_many_locals_executes_correctly() {
    // Generates a function with 60 locals to exercise the register allocator
    // without exceeding MAX_SCRIPT_LOCALS (issue #53 / #55).
    let assignments: String = (0..60).map(|i| format!("    v{i} = {i}\n")).collect();
    let src = format!("def f():\n{assignments}    return v59\nresult = f()\n");
    let tokens = Lexer::new(&src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();
    interpreter.exec_program(&program, false).unwrap();
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(59))
    );
}

#[test]
fn function_with_256_locals_executes_correctly() {
    // Pre-#53 fix: Reg was u8 so 256 locals caused silent wrap-around or panic.
    // Post-fix: Reg is u32; the 256th slot (index 255) must be readable.
    let assignments: String = (0..256).map(|i| format!("    v{i} = {i}\n")).collect();
    let src = format!("def f():\n{assignments}    return v255\nresult = f()\n");
    let tokens = Lexer::new(&src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();
    interpreter.exec_program(&program, false).unwrap();
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(255))
    );
}

#[test]
fn direct_calls_with_more_than_u8_max_positionals_use_expanded_lowering() {
    let arguments = (0..=u8::MAX)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let src = format!(
        "def collect(*args):\n    return (len(args), args[0], args[-1])\n\
         class Collector:\n    def collect(self, *args):\n        return (len(args), args[0], args[-1])\n\
         direct = collect({arguments})\n\
         method = Collector().collect({arguments})\n"
    );
    let interpreter = run_program(&src);
    let expected = Value::tuple(vec![Value::int(256), Value::int(0), Value::int(255)]);
    assert_eq!(
        interpreter.lookup_name("direct").unwrap(),
        Some(expected.clone())
    );
    assert_eq!(interpreter.lookup_name("method").unwrap(), Some(expected));
}

#[test]
fn bytecode_counts_do_not_wrap_at_256() {
    let defaults = (0..256)
        .map(|value| format!("a{value}={value}"))
        .collect::<Vec<_>>()
        .join(", ");
    let annotations = (0..256)
        .map(|value| format!("p{value}: int"))
        .collect::<Vec<_>>()
        .join(", ");
    let base_defs = (0..256)
        .map(|value| format!("class B{value}: pass\n"))
        .collect::<String>();
    let base_names = (0..256)
        .map(|value| format!("B{value}"))
        .collect::<Vec<_>>()
        .join(", ");
    let class_keywords = (0..256)
        .map(|value| format!("k{value}={value}"))
        .collect::<Vec<_>>()
        .join(", ");
    let unpack_after = (0..256)
        .map(|value| format!("u{value}"))
        .collect::<Vec<_>>()
        .join(", ");
    let match_args = (0..256)
        .map(|value| format!("'x{value}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let match_patterns = (0..256)
        .map(|value| format!("m{value}"))
        .collect::<Vec<_>>()
        .join(", ");
    let src = format!(
        "def with_defaults({defaults}):\n    return (a0, a255)\n\
         defaults_result = with_defaults()\n\
         def with_annotations({annotations}) -> int:\n    return 0\n\
         annotations_result = len(with_annotations.__annotations__)\n\
         {base_defs}\
         class WideBases({base_names}):\n    pass\n\
         bases_result = len(WideBases.__bases__)\n\
         class KeywordBase:\n    def __init_subclass__(cls, **kwargs):\n        cls.keyword_count = len(kwargs)\n\
         class WideKeywords(KeywordBase, {class_keywords}):\n    pass\n\
         keywords_result = WideKeywords.keyword_count\n\
         *unpack_rest, {unpack_after} = range(260)\n\
         unpack_result = (unpack_rest, u0, u255)\n\
         class PatternSubject:\n    __match_args__ = ({match_args},)\n\
         pattern_subject = PatternSubject()\n\
         for pattern_index in range(256):\n    setattr(pattern_subject, 'x' + str(pattern_index), pattern_index)\n\
         match pattern_subject:\n    case PatternSubject({match_patterns}):\n        pattern_result = (m0, m255)\n"
    );
    let interpreter = run_program(&src);
    assert_eq!(
        interpreter.lookup_name("defaults_result").unwrap(),
        Some(Value::tuple(vec![Value::int(0), Value::int(255)]))
    );
    assert_eq!(
        interpreter.lookup_name("annotations_result").unwrap(),
        Some(Value::int(257))
    );
    assert_eq!(
        interpreter.lookup_name("bases_result").unwrap(),
        Some(Value::int(256))
    );
    assert_eq!(
        interpreter.lookup_name("keywords_result").unwrap(),
        Some(Value::int(256))
    );
    assert_eq!(
        interpreter.lookup_name("unpack_result").unwrap(),
        Some(Value::tuple(vec![
            Value::list(vec![
                Value::int(0),
                Value::int(1),
                Value::int(2),
                Value::int(3)
            ]),
            Value::int(4),
            Value::int(259),
        ]))
    );
    assert_eq!(
        interpreter.lookup_name("pattern_result").unwrap(),
        Some(Value::tuple(vec![Value::int(0), Value::int(255)]))
    );
}

#[test]
fn extended_unpack_rejects_more_than_255_targets_before_star() {
    let targets = (0..256)
        .map(|value| format!("u{value}"))
        .collect::<Vec<_>>()
        .join(", ");
    let src = format!("{targets}, *rest = ()\n");
    let tokens = Lexer::new(&src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let err = Interpreter::default()
        .exec_program(&program, false)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "SyntaxError: too many expressions in star-unpacking assignment"
    );
}

#[test]
fn function_with_300_locals_executes_correctly() {
    // Confirms that Reg=u32 handles well beyond the old u8 limit.
    // 300 locals: assign each v_i = i, sum them, check sum(range(300)) = 44850.
    let assignments: String = (0..300).map(|i| format!("    v{i} = {i}\n")).collect();
    let mut sum_terms: String = "    s = 0\n".to_string();
    for i in 0..300 {
        sum_terms.push_str(&format!("    s = s + v{i}\n"));
    }
    let src = format!("def f():\n{assignments}{sum_terms}    return s\nresult = f()\n");
    let tokens = Lexer::new(&src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();
    interpreter.exec_program(&program, false).unwrap();
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(300 * 299 / 2))
    );
}

#[test]
fn binop_slow_path_float_and_mixed_types_correct() {
    // Exercises the BinOpConst slow path for non-Int types.
    // Issue #71 removed dead Int+Int arms from the slow path; the remaining
    // arms (Float, Str, etc.) must still produce correct results.
    let interpreter = run_program(
        "a = 1.5 + 2.5\nb = 10.0 - 3.0\nc = 2.0 * 4.0\nd = 9.0 / 2.0\ne = 'he' + 'llo'\nf = 2 ** 8\ng = 7 % 3\n",
    );
    assert_eq!(
        interpreter.lookup_name("a").unwrap(),
        Some(Value::float(4.0))
    );
    assert_eq!(
        interpreter.lookup_name("b").unwrap(),
        Some(Value::float(7.0))
    );
    assert_eq!(
        interpreter.lookup_name("c").unwrap(),
        Some(Value::float(8.0))
    );
    assert_eq!(
        interpreter.lookup_name("d").unwrap(),
        Some(Value::float(4.5))
    );
    assert_eq!(
        interpreter.lookup_name("e").unwrap(),
        Some(Value::string("hello"))
    );
    assert_eq!(interpreter.lookup_name("f").unwrap(), Some(Value::int(256)));
    assert_eq!(interpreter.lookup_name("g").unwrap(), Some(Value::int(1)));
}

#[test]
fn native_classmethod_cache_invalidates_on_target_and_mro_mutation() {
    // Each helper is one fused CallMethod site. Prime its native descriptor
    // plan, mutate the target and then an ancestor, and verify the class epoch
    // makes each lookup observe the new descriptor before the native plan is
    // eligible again.
    let interpreter = run_program(
        "class HotInt(int):\n\
         \x20   pass\n\
         def invoke_target():\n\
         \x20   return HotInt.from_bytes(b'\\x01', 'big')\n\
         before = invoke_target()\n\
         for _ in range(8):\n\
         \x20   invoke_target()\n\
         HotInt.from_bytes = classmethod(lambda cls, value, order: 41)\n\
         target_patched = invoke_target()\n\
         del HotInt.from_bytes\n\
         target_restored = invoke_target()\n\
         class MiddleInt(int):\n\
         \x20   pass\n\
         class LeafInt(MiddleInt):\n\
         \x20   pass\n\
         def invoke_leaf():\n\
         \x20   return LeafInt.from_bytes(b'\\x01', 'big')\n\
         mro_before = invoke_leaf()\n\
         for _ in range(8):\n\
         \x20   invoke_leaf()\n\
         MiddleInt.from_bytes = classmethod(lambda cls, value, order: 73)\n\
         mro_patched = invoke_leaf()\n\
         del MiddleInt.from_bytes\n\
         mro_restored = invoke_leaf()\n",
    );
    for (name, expected) in [
        ("before", 1),
        ("target_patched", 41),
        ("target_restored", 1),
        ("mro_before", 1),
        ("mro_patched", 73),
        ("mro_restored", 1),
    ] {
        assert_eq!(
            interpreter.lookup_name(name).unwrap(),
            Some(Value::int(expected)),
            "{name}"
        );
    }
}

#[test]
fn backingless_builtin_subclass_instances_raise_runtime_error_without_stack_overflow() {
    // Issue #2953: a builtin-backed subclass `PyInstance` lacking its internal
    // `__builtin_data__` storage (or pointing `__builtin_data__` back at a
    // `PyInstance`) must never recurse until the native stack overflows on
    // protocol dunders or builtin methods; it must fail with a catchable
    // Python `RuntimeError`.
    let mut interpreter = run_program(
        "from collections import Counter, OrderedDict, defaultdict, deque\n\
         class MyDict(dict): pass\n\
         class MyList(list): pass\n\
         class MySet(set): pass\n\
         class MyFrozenset(frozenset): pass\n\
         class MyTuple(tuple): pass\n\
         class MyBytearray(bytearray): pass\n\
         class MyStr(str): pass\n\
         class MyInt(int): pass\n\
         class MyFloat(float): pass\n\
         class MyBytes(bytes): pass\n\
         class MyOrderedDict(OrderedDict): pass\n\
         class MyCounter(Counter): pass\n\
         class MyDefaultDict(defaultdict): pass\n\
         class MyDeque(deque): pass\n",
    );

    let make_backingless = |interp: &mut Interpreter, class_name: &str, var_name: &str| {
        let cls_val = interp
            .lookup_name(class_name)
            .unwrap()
            .unwrap_or_else(|| panic!("missing class {class_name}"));
        let ValueKind::PyClass(class_rc) = cls_val.kind() else {
            panic!("{class_name} must be a PyClass");
        };
        let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class_rc),
            attrs: InstanceAttrs::new(),
        })));
        interp
            .env
            .borrow_mut()
            .namespace_values_mut()
            .insert(var_name, inst);
    };

    for (cls, var) in [
        ("MyDict", "d"),
        ("MyList", "l"),
        ("MySet", "s"),
        ("MyFrozenset", "fs"),
        ("MyTuple", "t"),
        ("MyBytearray", "ba"),
        ("MyStr", "st"),
        ("MyInt", "i"),
        ("MyFloat", "fl"),
        ("MyBytes", "bs"),
        ("MyOrderedDict", "od"),
        ("MyCounter", "ct"),
        ("MyDefaultDict", "dd"),
        ("MyDeque", "dq"),
    ] {
        make_backingless(&mut interpreter, cls, var);
    }

    // Also test a self-referential `builtin_data` cycle.
    make_backingless(&mut interpreter, "MyDict", "d_cyc");
    let d_cyc = interpreter.lookup_name("d_cyc").unwrap().unwrap();
    if let ValueKind::PyInstance(rc) = d_cyc.kind() {
        rc.borrow_mut().attrs.set_builtin_data(d_cyc.clone());
    }

    let must_fail_with_runtime_error = [
        // dict subclass
        "len(d)",
        "d.items()",
        "d.keys()",
        "d.values()",
        "iter(d)",
        "bool(d)",
        "0 in d",
        "d.__len__()",
        "d.__getitem__(0)",
        "d.__setitem__(0, 1)",
        "d.__delitem__(0)",
        // cyclic __builtin_data__
        "len(d_cyc)",
        "d_cyc.items()",
        "iter(d_cyc)",
        // list subclass
        "len(l)",
        "l.append(1)",
        "iter(l)",
        "bool(l)",
        "0 in l",
        // set / frozenset / tuple / bytearray / str / bytes / int / float subclasses
        "len(s)",
        "s.add(1)",
        "iter(s)",
        "len(fs)",
        "iter(fs)",
        "len(t)",
        "t.count(1)",
        "iter(t)",
        "len(ba)",
        "ba.append(1)",
        "iter(ba)",
        "len(st)",
        "st.upper()",
        "iter(st)",
        "len(bs)",
        "bs.upper()",
        "iter(bs)",
        "i.bit_length()",
        "fl.hex()",
        // OrderedDict (inherits dict.__len__ / dict.items)
        "len(od)",
        "od.items()",
        "repr(od)",
    ];

    for expr in must_fail_with_runtime_error {
        let wrapped = format!(
            "caught = False\n\
             try:\n\
             \x20   {expr}\n\
             except RuntimeError:\n\
             \x20   caught = True\n"
        );
        interpreter
            .exec_source(&wrapped, None, None)
            .unwrap_or_else(|e| panic!("unexpected uncaught error for `{expr}`: {e:?}"));
        assert_eq!(
            interpreter.lookup_name("caught").unwrap(),
            Some(Value::bool_(true)),
            "expected catchable RuntimeError for `{expr}`"
        );
    }

    // Operator fallbacks (`[]`, `+`) raise a catchable TypeError or
    // RuntimeError cleanly without overflowing the native stack.
    for expr in ["d[0]", "l[0]", "l + [1]", "i + 1"] {
        let wrapped = format!(
            "caught = False\n\
             try:\n\
             \x20   {expr}\n\
             except (RuntimeError, TypeError):\n\
             \x20   caught = True\n"
        );
        interpreter
            .exec_source(&wrapped, None, None)
            .unwrap_or_else(|e| panic!("unexpected uncaught error for `{expr}`: {e:?}"));
        assert_eq!(
            interpreter.lookup_name("caught").unwrap(),
            Some(Value::bool_(true)),
            "expected catchable error for `{expr}`"
        );
    }

    // Operations that fall back to `object.__repr__`/`__str__`/`__eq__` or
    // lazy storage init (`deque`, `Counter`, `defaultdict`) must also
    // terminate cleanly without overflowing the native stack.
    let must_not_overflow = [
        "repr(d)",
        "str(d)",
        "d == {}",
        "repr(d_cyc)",
        "repr(l)",
        "str(l)",
        "repr(s)",
        "repr(fs)",
        "repr(t)",
        "repr(ba)",
        "repr(st)",
        "repr(bs)",
        "repr(i)",
        "repr(fl)",
        "len(ct)",
        "ct.items()",
        "repr(ct)",
        "len(dd)",
        "dd.items()",
        "repr(dd)",
        "assert len(dq) == 0",
        "assert list(dq) == []",
        "assert isinstance(repr(dq), str)",
    ];
    for stmt in must_not_overflow {
        let wrapped = format!(
            "try:\n\
             \x20   {stmt}\n\
             except (RuntimeError, TypeError):\n\
             \x20   pass\n"
        );
        interpreter
            .exec_source(&wrapped, None, None)
            .unwrap_or_else(|e| panic!("unexpected uncaught error for `{stmt}`: {e:?}"));
    }
}

#[test]
fn setattr_and_delattr_allocate_zero_pep695_typing_singletons() {
    // Run on a dedicated thread so TLS OnceCells start pristine even under
    // `--test-threads=1`.
    std::thread::spawn(|| {
        assert_eq!(
            typing_class_singletons_initialized(),
            [
                ("TypeAliasType", false),
                ("TypeVar", false),
                ("ParamSpec", false),
                ("ParamSpecArgs", false),
                ("ParamSpecKwargs", false),
                ("TypeVarTuple", false),
            ],
            "fresh thread must start with zero typing class singletons initialized"
        );

        let interpreter = run_program(
            "class Box:\n\
             \x20   def __getitem__(self, item):\n\
             \x20       return item\n\
             b = Box()\n\
             b.x = 1\n\
             setattr(b, 'y', 2)\n\
             b.__bound__ = 'ok'\n\
             b.__constraints__ = ()\n\
             b.__value__ = 42\n\
             read_vals = (b.x, getattr(b, 'y'), b.__bound__, b.__constraints__, b.__value__, b[int])\n\
             del b.x\n\
             delattr(b, 'y')\n\
             del b.__bound__\n\
             del b.__value__\n",
        );
        assert!(interpreter.lookup_name("read_vals").unwrap().is_some());

        let user_class = Rc::new(RefCell::new(PyClass::new(
            "UserClass",
            "UserClass",
            None,
            IndexMap::new(),
        )));
        assert!(!is_typevar_class(&user_class));
        assert!(!is_type_alias_class(&user_class));
        assert!(!is_paramspec_class(&user_class));
        assert!(!is_typevartuple_class(&user_class));
        assert!(!is_paramspec_args_class(&user_class));
        assert!(!is_paramspec_kwargs_class(&user_class));

        assert_eq!(
            typing_class_singletons_initialized(),
            [
                ("TypeAliasType", false),
                ("TypeVar", false),
                ("ParamSpec", false),
                ("ParamSpecArgs", false),
                ("ParamSpecKwargs", false),
                ("TypeVarTuple", false),
            ],
            "setattr/delattr/getattr/indexing on user instances and predicate calls must allocate 0 PEP 695 typing singletons"
        );
    })
    .join()
    .expect("test thread panicked");
}

#[test]
fn pep695_typing_objects_enforce_readonly_attributes_after_materialization() {
    std::thread::spawn(|| {
        let interpreter = run_program(
            "def f[T, **P, *Ts](x: T) -> T:\n\
             \x20   return x\n\
             type Alias[U] = list[U]\n\
             T, P, Ts = f.__type_params__\n\
             p_args = P.args\n\
             p_kwargs = P.kwargs\n\
             def capture(fn):\n\
             \x20   try:\n\
             \x20       fn()\n\
             \x20   except AttributeError as exc:\n\
             \x20       return str(exc)\n\
             \x20   raise AssertionError('expected AttributeError')\n\
             results = {\n\
             \x20   'tv_set_bound': capture(lambda: setattr(T, '__bound__', int)),\n\
             \x20   'tv_del_bound': capture(lambda: delattr(T, '__bound__')),\n\
             \x20   'tv_set_constraints': capture(lambda: setattr(T, '__constraints__', (int,))),\n\
             \x20   'tv_set_name': capture(lambda: setattr(T, '__name__', 'X')),\n\
             \x20   'tv_del_name': capture(lambda: delattr(T, '__name__')),\n\
             \x20   'alias_set_name': capture(lambda: setattr(Alias, '__name__', 'Y')),\n\
             \x20   'alias_set_value': capture(lambda: setattr(Alias, '__value__', int)),\n\
             \x20   'alias_del_type_params': capture(lambda: delattr(Alias, '__type_params__')),\n\
             \x20   'alias_set_module': capture(lambda: setattr(Alias, '__module__', 'm')),\n\
             \x20   'ps_set_args': capture(lambda: setattr(P, 'args', 1)),\n\
             \x20   'ps_del_kwargs': capture(lambda: delattr(P, 'kwargs')),\n\
             \x20   'ps_set_name': capture(lambda: setattr(P, '__name__', 'Q')),\n\
             \x20   'tvt_set_name': capture(lambda: setattr(Ts, '__name__', 'Us')),\n\
             \x20   'tvt_del_name': capture(lambda: delattr(Ts, '__name__')),\n\
             \x20   'pa_set_origin': capture(lambda: setattr(p_args, '__origin__', P)),\n\
             \x20   'pk_del_origin': capture(lambda: delattr(p_kwargs, '__origin__')),\n\
             }\n\
             # Arbitrary attributes remain writable/deletable on PEP 695 objects\n\
             T.custom = 123\n\
             custom_ok = T.custom == 123\n\
             del T.custom\n\
             Alias.note = 'ok'\n\
             alias_note_ok = Alias.note == 'ok'\n\
             del Alias.note\n",
        );

        let results_val = interpreter.lookup_name("results").unwrap().unwrap();
        let ValueKind::Dict(results) = results_val.kind() else {
            panic!("expected results dict");
        };
        let expected = [
            (
                "tv_set_bound",
                "attribute '__bound__' of 'typing.TypeVar' objects is not writable",
            ),
            (
                "tv_del_bound",
                "attribute '__bound__' of 'typing.TypeVar' objects is not writable",
            ),
            (
                "tv_set_constraints",
                "attribute '__constraints__' of 'typing.TypeVar' objects is not writable",
            ),
            ("tv_set_name", "readonly attribute"),
            ("tv_del_name", "readonly attribute"),
            ("alias_set_name", "readonly attribute"),
            (
                "alias_set_value",
                "attribute '__value__' of 'typing.TypeAliasType' objects is not writable",
            ),
            (
                "alias_del_type_params",
                "attribute '__type_params__' of 'typing.TypeAliasType' objects is not writable",
            ),
            (
                "alias_set_module",
                "attribute '__module__' of 'typing.TypeAliasType' objects is not writable",
            ),
            (
                "ps_set_args",
                "attribute 'args' of 'typing.ParamSpec' objects is not writable",
            ),
            (
                "ps_del_kwargs",
                "attribute 'kwargs' of 'typing.ParamSpec' objects is not writable",
            ),
            ("ps_set_name", "readonly attribute"),
            ("tvt_set_name", "readonly attribute"),
            ("tvt_del_name", "readonly attribute"),
            ("pa_set_origin", "readonly attribute"),
            ("pk_del_origin", "readonly attribute"),
        ];
        for (key, msg) in expected {
            assert_eq!(
                results.get(&PyKey::str_from(key)).and_then(|v| v.as_str()),
                Some(msg),
                "mismatch for {key}"
            );
        }
        assert_eq!(
            interpreter.lookup_name("custom_ok").unwrap(),
            Some(Value::bool_(true))
        );
        assert_eq!(
            interpreter.lookup_name("alias_note_ok").unwrap(),
            Some(Value::bool_(true))
        );

        let singletons = [
            type_alias_class_singleton(),
            typevar_class_singleton(),
            paramspec_class_singleton(),
            paramspec_args_class_singleton(),
            paramspec_kwargs_class_singleton(),
            typevartuple_class_singleton(),
        ];
        type ClassPredicate = fn(&Rc<RefCell<PyClass>>) -> bool;
        let predicates: [ClassPredicate; 6] = [
            is_type_alias_class,
            is_typevar_class,
            is_paramspec_class,
            is_paramspec_args_class,
            is_paramspec_kwargs_class,
            is_typevartuple_class,
        ];
        for (i, cls) in singletons.iter().enumerate() {
            for (j, pred) in predicates.iter().enumerate() {
                assert_eq!(pred(cls), i == j, "predicate {j} on class {i}");
            }
        }
        assert_eq!(
            typing_class_singletons_initialized(),
            [
                ("TypeAliasType", true),
                ("TypeVar", true),
                ("ParamSpec", true),
                ("ParamSpecArgs", true),
                ("ParamSpecKwargs", true),
                ("TypeVarTuple", true),
            ]
        );
    })
    .join()
    .expect("test thread panicked");
}

#[test]
fn primitive_calls_and_object_subclasses_keep_collections_and_module_lazy() {
    std::thread::spawn(|| {
        assert_eq!(
            lazy_class_singletons_initialized(),
            (false, false, false),
            "fresh thread must start with odict/deque/module singletons uninitialized"
        );

        let mut interpreter = run_program(
            "a = int()\n\
             b = set()\n\
             c = dict()\n\
             d = list()\n\
             e = str()\n\
             r = range(3)\n\
             z = zip()\n\
             class User:\n\
             \x20   pass\n\
             u = User()\n\
             u_doc = User.__doc__\n\
             subs_before = [cls.__name__ for cls in object.__subclasses__()]\n\
             keys_subs_before = [cls.__name__ for cls in type({}.keys()).__subclasses__()]\n",
        );

        assert_eq!(
            lazy_class_singletons_initialized(),
            (false, false, false),
            "int()/set()/dict()/object.__subclasses__() must not initialize odict_*, _deque_*iterator, or MODULE_TYPE"
        );

        let subs_before = interpreter.lookup_name("subs_before").unwrap().unwrap();
        let ValueKind::List(subs_list) = subs_before.kind() else {
            panic!("expected subs_before list");
        };
        let names_before: Vec<String> = subs_list
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(!names_before.iter().any(|n| n == "_deque_iterator"));
        assert!(!names_before.iter().any(|n| n == "_deque_reverse_iterator"));
        assert!(!names_before.iter().any(|n| n == "module"));

        let keys_subs_before = interpreter.lookup_name("keys_subs_before").unwrap().unwrap();
        let ValueKind::List(keys_list) = keys_subs_before.kind() else {
            panic!("expected keys_subs_before list");
        };
        assert!(keys_list.is_empty());

        // Now materialize collections.OrderedDict views, deque iterators, and
        // types.ModuleType and verify dispatch + subclass registration work.
        interpreter
            .exec_source(
                "import collections, types\n\
                 dq = collections.deque([10, 20])\n\
                 dq_it = iter(dq)\n\
                 dq_rev = reversed(dq)\n\
                 dq_it2 = collections._deque_iterator(dq)\n\
                 dq_rev2 = type(dq_rev)(dq)\n\
                 dq_vals = (list(dq_it), list(dq_rev), list(dq_it2), list(dq_rev2))\n\
                 od = collections.OrderedDict([('a', 1), ('b', 2)])\n\
                 ok, oi, ov = od.keys(), od.items(), od.values()\n\
                 od_type_names = (type(ok).__name__, type(oi).__name__, type(ov).__name__)\n\
                 def ctor_err(cls):\n\
                 \x20   try:\n\
                 \x20       cls()\n\
                 \x20   except TypeError as exc:\n\
                 \x20       return str(exc)\n\
                 \x20   raise AssertionError('expected TypeError')\n\
                 od_ctor_errs = (ctor_err(type(ok)), ctor_err(type(oi)), ctor_err(type(ov)))\n\
                 m = types.ModuleType('demo', 'hello')\n\
                 m_info = (type(m) is types.ModuleType, m.__name__, m.__doc__)\n\
                 subs_after = [cls.__name__ for cls in object.__subclasses__()]\n\
                 keys_subs_after = [cls.__name__ for cls in type({}.keys()).__subclasses__()]\n",
                None,
                None,
            )
            .unwrap();

        assert_eq!(
            lazy_class_singletons_initialized(),
            (true, true, true),
            "odict, deque iterator, and module singletons must now be initialized"
        );
        assert_eq!(
            interpreter.lookup_name("od_type_names").unwrap(),
            Some(Value::tuple(vec![
                Value::string("odict_keys"),
                Value::string("odict_items"),
                Value::string("odict_values"),
            ]))
        );
        assert_eq!(
            interpreter.lookup_name("od_ctor_errs").unwrap(),
            Some(Value::tuple(vec![
                Value::string("cannot create 'odict_keys' instances"),
                Value::string("cannot create 'odict_items' instances"),
                Value::string("cannot create 'odict_values' instances"),
            ]))
        );
        assert_eq!(
            interpreter.lookup_name("m_info").unwrap(),
            Some(Value::tuple(vec![
                Value::bool_(true),
                Value::string("demo"),
                Value::string("hello"),
            ]))
        );

        let subs_after = interpreter.lookup_name("subs_after").unwrap().unwrap();
        let ValueKind::List(subs_after_list) = subs_after.kind() else {
            panic!("expected subs_after list");
        };
        let names_after: Vec<String> = subs_after_list
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(names_after.iter().any(|n| n == "_deque_iterator"));
        assert!(names_after.iter().any(|n| n == "_deque_reverse_iterator"));
        assert!(names_after.iter().any(|n| n == "module"));

        let keys_subs_after = interpreter.lookup_name("keys_subs_after").unwrap().unwrap();
        let ValueKind::List(keys_after_list) = keys_subs_after.kind() else {
            panic!("expected keys_subs_after list");
        };
        assert_eq!(keys_after_list.len(), 1);
        assert_eq!(keys_after_list[0].as_str(), Some("odict_keys"));
    })
    .join()
    .expect("test thread panicked");
}

#[test]
fn lazy_classes_materialized_before_primitive_dispatch_still_dispatch() {
    // Verify the reverse initialization order: materialize MODULE_TYPE,
    // ODICT_VIEW_CLASSES, and DEQUE_ITERATOR_CLASSES *before*
    // PRIMITIVE_CLASS_DISPATCH is initialized, then confirm all of them are
    // registered and dispatchable when PRIMITIVE_CLASS_DISPATCH is built.
    std::thread::spawn(|| {
        let module_cls = module_type_singleton();
        let odict_keys_cls = DictViewClass::OrderedKeys.singleton();
        let odict_items_cls = DictViewClass::OrderedItems.singleton();
        let odict_values_cls = DictViewClass::OrderedValues.singleton();
        let deque_it_cls = NativeIteratorClass::Deque.singleton();
        let deque_rev_cls = NativeIteratorClass::DequeReverse.singleton();

        assert_eq!(lazy_class_singletons_initialized(), (true, true, true));

        for cls in [
            &module_cls,
            &odict_keys_cls,
            &odict_items_cls,
            &odict_values_cls,
            &deque_it_cls,
            &deque_rev_cls,
        ] {
            assert!(
                primitive_class_dispatch(cls).is_some(),
                "expected dispatch entry for {}",
                cls.borrow().name
            );
        }
    })
    .join()
    .expect("test thread panicked");
}

/// Structure generation of the interpreter's root namespace, which every
/// full (uncached) global assignment advances.
fn root_structure_generation(interpreter: &Interpreter) -> u64 {
    interpreter.env.borrow().namespace_cache_snapshot().2
}

/// The compiled body of a module-level function bound to `name`.
fn function_code(interpreter: &Interpreter, name: &str) -> Rc<dyn std::any::Any> {
    let value = interpreter
        .lookup_name(name)
        .unwrap()
        .unwrap_or_else(|| panic!("missing binding {name}"));
    let ValueKind::UserFunction(function) = value.kind() else {
        panic!("{name} must be a user function");
    };
    function
        .precompiled_code
        .clone()
        .expect("a compiled function keeps its code")
}

/// Issue #3423: a repeated `global G; G = i` overwrites the cached root
/// binding in place. Only the first store of the loop runs the full
/// assignment, so the namespace generations stay flat however long it runs.
#[test]
fn hot_global_store_overwrites_the_cached_root_slot() {
    let interpreter = run_program(
        "G = 0\n\
         def loop(n):\n    global G\n    for i in range(n):\n        G = i\n\
         loop(5000)\n\
         seen = G\n",
    );
    assert_eq!(
        interpreter.lookup_name("seen").unwrap(),
        Some(Value::int(4999))
    );
    assert_eq!(
        interpreter.lookup_name("G").unwrap(),
        Some(Value::int(4999))
    );
    let generation = root_structure_generation(&interpreter);
    assert!(
        generation < 100,
        "5000 cached stores must not each advance the structure generation: {generation}"
    );
    let code = function_code(&interpreter, "loop");
    let code = code
        .downcast_ref::<crate::bytecode::FnCode>()
        .expect("function code");
    assert!(
        code.global_store_cache
            .borrow()
            .iter()
            .any(|entry| matches!(entry, GlobalStoreCacheEntry::RootSlot { .. })),
        "the store site must have cached its root slot"
    );
}

/// Issue #3423: `global G; G += 1` reads the binding through a slot locator
/// and writes it through one, so the read stays cached across every write —
/// and an unrelated global callee in the same loop is not evicted either.
#[test]
fn hot_global_read_modify_write_keeps_load_caches_valid() {
    let interpreter = run_program(
        "G = 0\n\
         def step(x):\n    return x + 1\n\
         def loop(n):\n    global G\n    for i in range(n):\n        G += 1\n        G = step(G)\n\
         loop(3000)\n\
         seen = G\n",
    );
    assert_eq!(
        interpreter.lookup_name("seen").unwrap(),
        Some(Value::int(6000))
    );
    let generation = root_structure_generation(&interpreter);
    assert!(
        generation < 100,
        "6000 cached stores must not each advance the structure generation: {generation}"
    );
    let code = function_code(&interpreter, "loop");
    let code = code
        .downcast_ref::<crate::bytecode::FnCode>()
        .expect("function code");
    let cache = code.global_cache.borrow();
    let entry_for = |name: &str| {
        let slot = code
            .names
            .iter()
            .position(|candidate| candidate.as_str() == name)
            .unwrap_or_else(|| panic!("{name} must be in the name pool"));
        &cache[slot]
    };
    assert!(
        matches!(entry_for("G"), GlobalCacheEntry::EnvironmentSlot { .. }),
        "a global rebound through a cached store must be read by slot locator: {:?}",
        entry_for("G")
    );
    assert!(
        matches!(entry_for("step"), GlobalCacheEntry::Environment { .. }),
        "a global no cached store rebinds keeps the cheaper value form: {:?}",
        entry_for("step")
    );
}

/// Issue #3423: a `global`-declaring callee runs on the call trampoline and
/// still writes its own module's binding; the caller's cached load of the
/// callee survives every one of those writes.
#[test]
fn trampolined_global_writer_updates_the_module_binding() {
    let interpreter = run_program(
        "TOTAL = 0\nLAST = None\n\
         def tick(x):\n    global TOTAL, LAST\n    TOTAL += x\n    LAST = x\n\
         def fact(n):\n    global TOTAL\n    TOTAL += 1\n    return 1 if n <= 1 else n * fact(n - 1)\n\
         def run(n):\n    for i in range(n):\n        tick(i)\n\
         run(1000)\n\
         after_run = (TOTAL, LAST)\n\
         TOTAL = 0\n\
         result = (fact(10), TOTAL)\n",
    );
    assert_eq!(
        interpreter.lookup_name("after_run").unwrap(),
        Some(Value::tuple(vec![Value::int(499_500), Value::int(999)]))
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::tuple(vec![Value::int(3_628_800), Value::int(10)]))
    );
    let generation = root_structure_generation(&interpreter);
    assert!(
        generation < 100,
        "trampolined global stores must stay on the cached slot: {generation}"
    );
}

/// Environment and structure generations of the interpreter's root namespace.
fn root_generations(interpreter: &Interpreter) -> (u64, u64) {
    let snapshot = interpreter.env.borrow().namespace_cache_snapshot();
    (snapshot.1, snapshot.2)
}

/// The `LoadGlobal` cache entry the module-level function `function` holds
/// for `name`.
fn load_global_entry(interpreter: &Interpreter, function: &str, name: &str) -> GlobalCacheEntry {
    let code = function_code(interpreter, function);
    let code = code
        .downcast_ref::<crate::bytecode::FnCode>()
        .expect("function code");
    let slot = code
        .names
        .iter()
        .position(|candidate| candidate.as_str() == name)
        .unwrap_or_else(|| panic!("{name} must be in the name pool of {function}"));
    code.global_cache
        .borrow()
        .get(slot)
        .cloned()
        .unwrap_or(GlobalCacheEntry::Empty)
}

fn list_items(interpreter: &Interpreter, name: &str) -> Vec<Value> {
    let value = interpreter
        .lookup_name(name)
        .unwrap()
        .unwrap_or_else(|| panic!("missing binding {name}"));
    value
        .as_list()
        .unwrap_or_else(|| panic!("{name} must be a list"))
        .to_vec()
}

/// Issue #3425: materializing `globals()` without mutating it keeps the root
/// global inline caches enabled. Functions first called after the exposure
/// still fill their `LoadGlobal` entries, and further read-only use of the
/// dictionary advances no cache generation, so those entries keep hitting.
#[test]
fn read_only_globals_exposure_keeps_load_global_caches_hot() {
    let program = "K = 3\nout = []\n\
         def helper(a):\n    return a\n\
         def g2(a, b):\n    return helper(a) + K\n\
         def run(n):\n    total = 0\n    for i in range(n):\n        total += g2(i, i)\n    return total\n\
         def peek(n):\n    seen = 0\n    for i in range(n):\n        seen += len(globals()) + ('K' in globals()) + len(list(globals().items()))\n    return seen > 0\n\
         out.append(len(list(globals().items())))\n\
         out.append(run(2000))\n";
    let interpreter = run_program(program);
    assert_eq!(list_items(&interpreter, "out")[1], Value::int(2_005_000));
    assert!(
        interpreter.env.borrow().namespace_globals_exposed(),
        "the program must have exposed its globals dictionary"
    );
    assert!(
        !interpreter.env.borrow().namespace_cache_snapshot().4,
        "exposure must leave the root global caches enabled"
    );
    for (function, name) in [("g2", "helper"), ("g2", "K"), ("run", "g2")] {
        let entry = load_global_entry(&interpreter, function, name);
        assert!(
            matches!(entry, GlobalCacheEntry::Environment { .. }),
            "{function} must cache its load of {name} after globals(): {entry:?}"
        );
    }
    let entry = load_global_entry(&interpreter, "run", "range");
    assert!(
        matches!(entry, GlobalCacheEntry::BuiltinModule { .. }),
        "run must cache its builtin load after globals(): {entry:?}"
    );

    // The same program followed by many more read-only uses of the
    // dictionary and more cached reads ends at the same generations.
    let longer = run_program(&format!(
        "{program}out.append(peek(200))\nout.append(run(10))\nout.append(peek(200))\n"
    ));
    assert_eq!(
        list_items(&longer, "out")[2..],
        [Value::bool_(true), Value::int(75), Value::bool_(true)]
    );
    assert_eq!(
        root_generations(&longer),
        root_generations(&interpreter),
        "read-only globals() must not invalidate any cached global"
    );
}

/// Issue #3425: a write through the exposed dictionary invalidates the cached
/// resolutions at once, the next read observes the new binding, and that read
/// fills the cache again.
#[test]
fn alias_write_invalidates_and_refills_load_global_caches() {
    let prefix = "K = 3\nout = []\n\
         def g2(a):\n    return a + K\n\
         def run(n):\n    total = 0\n    for i in range(n):\n        total += g2(i)\n    return total\n\
         d = globals()\n\
         out.append(run(100))\n";
    // A stale cached resolution would keep returning the value bound at the
    // first run (`K == 3`) instead of the outcome expected here.
    for (mutation, expected) in [
        ("d['K'] = 10\n", Some(5_950)),
        ("d.update(K=10)\n", Some(5_950)),
        ("d |= {'K': 10}\n", Some(5_950)),
        (
            "d.setdefault('K', 10)\nd.__setitem__('K', 10)\n",
            Some(5_950),
        ),
        ("exec('K = 10', d)\n", Some(5_950)),
        ("d.pop('K')\n", None),
        ("del d['K']\n", None),
    ] {
        let label = mutation.trim_end();
        let Some(expected) = expected else {
            let interpreter = run_program(&format!(
                "{prefix}{mutation}try:\n    run(1)\nexcept NameError as error:\n    out.append(str(error))\n"
            ));
            assert_eq!(
                list_items(&interpreter, "out")[1].as_str(),
                Some("name 'K' is not defined"),
                "`{label}` must be observed by the next read"
            );
            continue;
        };
        let interpreter = run_program(&format!("{prefix}{mutation}out.append(run(100))\n"));
        assert_eq!(
            list_items(&interpreter, "out"),
            [Value::int(5_250), Value::int(expected)],
            "`{label}` must be observed by the next read"
        );
        let entry = load_global_entry(&interpreter, "g2", "K");
        assert!(
            matches!(entry, GlobalCacheEntry::Environment { .. }),
            "the read after `{label}` must fill the cache again: {entry:?}"
        );
    }
}

/// Issue #3425: an exposed namespace keeps its in-place `StoreGlobal`. The
/// store mirrors into the dictionary on every iteration without advancing a
/// cache generation, so the loop's other cached loads stay valid.
#[test]
fn hot_global_store_stays_cached_after_globals_exposure() {
    let interpreter = run_program(
        "G = 0\nd = globals()\nseen = []\nout = []\n\
         def step(x):\n    return x + 1\n\
         def loop(n):\n    global G\n    for i in range(n):\n        G = step(G)\n        if d['G'] != G:\n            seen.append(i)\n\
         loop(5000)\n\
         out.append((G, d['G']))\n",
    );
    assert_eq!(
        list_items(&interpreter, "out"),
        [Value::tuple(vec![Value::int(5000), Value::int(5000)])]
    );
    assert!(
        list_items(&interpreter, "seen").is_empty(),
        "the dictionary must observe every in-place store"
    );
    let generation = root_structure_generation(&interpreter);
    assert!(
        generation < 100,
        "5000 mirrored stores must not each advance the structure generation: {generation}"
    );
    let code = function_code(&interpreter, "loop");
    let code = code
        .downcast_ref::<crate::bytecode::FnCode>()
        .expect("function code");
    assert!(
        code.global_store_cache
            .borrow()
            .iter()
            .any(|entry| matches!(entry, GlobalStoreCacheEntry::RootSlot { .. })),
        "the store site must have cached its root slot"
    );
    let entry = load_global_entry(&interpreter, "loop", "G");
    assert!(
        matches!(entry, GlobalCacheEntry::EnvironmentSlot { .. }),
        "the rebound global must be read by slot locator: {entry:?}"
    );
    let entry = load_global_entry(&interpreter, "loop", "step");
    assert!(
        matches!(entry, GlobalCacheEntry::Environment { .. }),
        "the callee load must stay on the value lane: {entry:?}"
    );
}

/// Issue #3425: after exposure a module-level assignment still mirrors into
/// the dictionary, but as an owner write: it invalidates only caches that
/// registered interest in the assigned name.
#[test]
fn module_writes_after_globals_exposure_keep_unrelated_caches_valid() {
    let mask_of = pyrust_core::namespace_name_interest_mask;
    let cached = ["globals", "range", "K", "g2", "run"]
        .iter()
        .fold(0_u64, |mask, name| mask | mask_of(name));
    for written in ["step", "acc"] {
        assert_ne!(
            mask_of(written) & !cached,
            0,
            "{written} must not collide with the cached names' interest masks"
        );
    }
    let interpreter = run_program(
        "K = 3\nout = []\n\
         def g2(a):\n    return a + K\n\
         def run(n):\n    total = 0\n    for i in range(n):\n        total += g2(i)\n    return total\n\
         d = globals()\n\
         acc = 0\n\
         for step in range(3000):\n    acc = run(2)\n\
         out.append((acc, step, d['acc'], d['step']))\n",
    );
    assert_eq!(
        list_items(&interpreter, "out"),
        [Value::tuple(vec![
            Value::int(7),
            Value::int(2999),
            Value::int(7),
            Value::int(2999),
        ])]
    );
    let generation = root_structure_generation(&interpreter);
    assert!(
        generation < 100,
        "6000 mirrored module writes must not each advance the structure generation: {generation}"
    );
}
