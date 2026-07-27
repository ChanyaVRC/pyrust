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
