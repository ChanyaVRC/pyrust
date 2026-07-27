#[test]
fn vm_basic_arithmetic() {
    let interpreter = run_program("def f(a, b): return a * b + 1\nresult = f(6, 7)\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(43))
    );
}

#[test]
fn vm_factorial_recursive() {
    // run_bytecode debug-mode frame is large (~150 KB); spawn with explicit
    // stack so this test stays stable as new VM arms are added.
    let ok: bool = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let interpreter = run_program(
                    "def fact(n):\n    if n <= 1: return 1\n    return n * fact(n - 1)\nresult = fact(10)\n",
                );
                interpreter.lookup_name("result").unwrap() == Some(Value::int(3628800))
            })
            .unwrap()
            .join()
            .unwrap();
    assert!(ok);
}

#[test]
fn vm_recursive_memo_pure_fn_fib() {
    // Recursive memo-pure function correctness through CallMemo's adaptive
    // scalar-result cache. Recursive misses use explicit VM frames; the modest
    // host stack covers only the debug interpreter entry machinery.
    let ok: bool = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let interpreter = run_program(
                    "def fib(n):\n    if n <= 1: return n\n    return fib(n-1) + fib(n-2)\nresult = fib(30)\n",
                );
                let result =
                    interpreter.lookup_name("result").unwrap() == Some(Value::int(832040));
                let function = interpreter.lookup_name("fib").unwrap().unwrap();
                let ValueKind::UserFunction(function) = function.kind() else {
                    panic!("fib must remain a user function");
                };
                let function_id = function.id;
                let top_key = (function_id, smallvec::smallvec![30]);
                let has_top_result =
                    interpreter.memo_cache.get(&top_key) == Some(&Value::int(832040));
                let has_cache_hit = interpreter
                    .memo_stats
                    .get(&function_id)
                    .is_some_and(|(_, hits, _)| *hits > 0);
                result
                    && has_top_result
                    && has_cache_hit
                    && interpreter.memo_in_flight.is_empty()
            })
            .unwrap()
            .join()
            .unwrap();
    assert!(ok);
}

#[test]
fn callmemo_uses_precomputed_high_arity_shape() {
    let interpreter = run_program(concat!(
        "def add8(a, b, c, d, e, f, g, h):\n",
        "    return a + b + c + d + e + f + g + h\n",
        "first = add8(1, 2, 3, 4, 5, 6, 7, 8)\n",
        "second = add8(1, 2, 3, 4, 5, 6, 7, 8)\n",
    ));
    assert_eq!(
        interpreter.lookup_name("second").unwrap(),
        Some(Value::int(36))
    );
    let value = interpreter.lookup_name("add8").unwrap().unwrap();
    let ValueKind::UserFunction(function) = value.kind() else {
        panic!("add8 must remain a user function");
    };
    assert_eq!(function.memo_positional_parameter_count, 8);
    assert!(
        interpreter
            .memo_stats
            .get(&function.id)
            .is_some_and(|(_, hits, _)| *hits > 0),
        "the second identical high-arity call must hit CallMemo"
    );
}

#[test]
fn self_recursive_memo_pure_fn_uses_call_memo() {
    // Regression test for issue #52.  A memo-pure function that calls itself
    // recursively must:
    //  (a) still have `UserFunction.is_memo_pure = true`, and
    //  (b) have its inner recursive calls compiled as CallMemo (the
    //      purity marker used by runtime result memoization).
    //
    // Verify correctness for several known Fibonacci values.
    let ok: bool = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let interp = run_program(
                "def fib(n):\n    if n <= 1: return n\n    return fib(n-1) + fib(n-2)\n\
                     r0 = fib(0)\nr1 = fib(1)\nr5 = fib(5)\nr10 = fib(10)\nr20 = fib(20)\n",
            );
            let get = |name| interp.lookup_name(name).unwrap();
            get("r0") == Some(Value::int(0))
                && get("r1") == Some(Value::int(1))
                && get("r5") == Some(Value::int(5))
                && get("r10") == Some(Value::int(55))
                && get("r20") == Some(Value::int(6765))
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(ok);
}

#[test]
fn vm_for_loop_sum() {
    let interpreter = run_program(
        "def s(n):\n    t = 0\n    for i in range(n):\n        t += i\n    return t\nresult = s(100)\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(4950))
    );
}

#[test]
fn vm_early_return_from_for_loop() {
    let interpreter = run_program(
        "def f(n, limit):\n    s = 0\n    for i in range(n):\n        s += i\n        if s > limit:\n            return s\n    return s\nresult = f(100, 50)\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(55))
    );
}

#[test]
fn vm_list_and_index() {
    let interpreter = run_program("def f(lst): return lst[0] + lst[2]\nresult = f([10, 20, 30])\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(40))
    );
}

#[test]
fn vm_tuple_unpack_in_for() {
    let interpreter = run_program(
        "def f(pairs):\n    t = 0\n    for a, b in pairs:\n        t += a + b\n    return t\nresult = f([(1, 2), (3, 4), (5, 6)])\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(21))
    );
}

#[test]
fn vm_while_loop() {
    let interpreter = run_program(
        "def f(n):\n    i = 0\n    s = 0\n    while i < n:\n        s += i\n        i += 1\n    return s\nresult = f(10)\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(45))
    );
}

#[test]
fn vm_and_returns_operand_not_bool() {
    // Python `and` returns the actual operand, not a coerced bool.
    let interpreter = run_program("def f(a, b): return a and b\nresult = f(1, 42)\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(42))
    );
}

#[test]
fn vm_or_returns_operand_not_bool() {
    let interpreter = run_program("def f(a, b): return a or b\nresult = f(0, 99)\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(99))
    );
}

#[test]
fn vm_and_short_circuits_on_falsy_lhs() {
    let interpreter = run_program("def f(a, b): return a and b\nresult = f(0, 42)\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(0))
    );
}

#[test]
fn vm_for_else_runs_when_not_broken() {
    let interpreter = run_program(
        "def f(lst):\n    for x in lst:\n        if x > 10: return x\n    else:\n        return -1\n    return 0\nresult = f([1, 2, 3])\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(-1))
    );
}

#[test]
fn vm_for_else_skipped_on_break() {
    let interpreter = run_program(
        "def f(lst):\n    for x in lst:\n        if x > 1: break\n    else:\n        return -1\n    return x\nresult = f([1, 5, 3])\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(5))
    );
}

#[test]
fn vm_while_else_runs_when_condition_false() {
    let interpreter = run_program(
        "def f(n):\n    i = 0\n    while i < n:\n        i += 1\n    else:\n        return i\n    return -1\nresult = f(3)\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(3))
    );
}

#[test]
fn vm_assert_falls_back_correctly() {
    // Functions containing `assert` must fall back to the tree-walker, which handles
    // AssertionError.  Verify the successful-assert path still returns the right value.
    let interpreter = run_program("def f(x):\n    assert x > 0\n    return x * 2\nresult = f(5)\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(10))
    );
}

#[test]
fn variadic_args_packed_into_tuple() {
    let interpreter = run_program("def f(*args): return args\nresult = f(1, 2, 3)\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::tuple(vec![
            Value::int(1),
            Value::int(2),
            Value::int(3)
        ]))
    );
}

#[test]
fn variadic_kwargs_packed_into_dict() {
    let interpreter = run_program("def f(**kw): return kw['x']\nresult = f(x=42)\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(42))
    );
}

#[test]
fn closure_over_two_cell_vars_correct_base_temp() {
    // Regression: when a function has N cell vars, base_temp was computed as
    // (total_locals - cell_var_count), causing temp regs to overlap with local
    // slots and clobbering captured variables.
    let interpreter = run_program(
        "def outer():\n    x = 10\n    y = 20\n    def inner(): return x + y\n    x = 3\n    y = 4\n    return inner()\nresult = outer()\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(7))
    );
}

#[test]
fn vm_unbound_local_after_lambda_drop() {
    // Regression: bytecode_cache stale-ptr bug — a lambda dropped after its
    // first call must not pollute the cache for a later function allocated at
    // the same address.
    let interpreter = run_program(
        "def find_first(lst, pred):\n    for i, x in enumerate(lst):\n        if pred(x): return i\n    return -1\nfind_first([1, 4, 7], lambda x: x > 5)\ndef g(n):\n    s = 0\n    for i in range(n):\n        s += i\n    return s\nresult = g(5)\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(10))
    );
}

// ── Edge-case tests (issue #57) ───────────────────────────────────────────

#[test]
fn int_add_at_i64_max_promotes_to_bigint() {
    // Issue #421: integer overflow must promote to BigInt rather than
    // wrap.  `i64::MAX + 1 == 9223372036854775808` in CPython; pyrust
    // must agree.
    let tokens = Lexer::new("x = 9223372036854775807\nx = x + 1\n")
        .unwrap()
        .into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();
    interpreter.exec_program(&program, false).unwrap();
    let expected = Value::bigint(crate::value::PyBigInt::from(i64::MAX) + 1);
    assert_eq!(
        interpreter.lookup_name("x").unwrap(),
        Some(expected),
        "i64::MAX + 1 must promote to BigInt"
    );
}
