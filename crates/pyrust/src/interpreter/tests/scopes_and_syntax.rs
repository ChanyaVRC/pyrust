#[test]
fn python_modulo_sign_matches_divisor() {
    assert_eq!(py_mod_i64(-7, 3), 2);
    assert_eq!(py_mod_i64(7, -3), -2);
    assert_eq!(py_mod_i64(-7, -3), -1);
}

#[test]
fn range_len_works_for_both_directions() {
    assert_eq!(range_len(0, 5, 1), 5);
    assert_eq!(range_len(1, 8, 2), 4);
    assert_eq!(range_len(8, 1, -2), 4);
    assert_eq!(range_len(1, 1, 1), 0);
}

#[test]
fn wide_i64_range_indexes_without_narrowing_its_logical_length() {
    let interpreter = run_program(concat!(
        "r = range(-(2**63), 2**63 - 1)\n",
        "values = [r[-1], r[2**63], r[-(2**64 - 1)], r.index(2**63 - 2), bool(r)]\n",
        "try:\n",
        "    len(r)\n",
        "except OverflowError as exc:\n",
        "    len_error = str(exc)\n",
        "min_step = list(range(2**63 - 1, -(2**63), -(2**63)))\n",
    ));

    assert_eq!(
        interpreter.lookup_name("values").unwrap(),
        Some(Value::list(vec![
            Value::int(i64::MAX - 1),
            Value::int(0),
            Value::int(i64::MIN),
            Value::bigint(crate::value::PyBigInt::from(u64::MAX - 1)),
            Value::bool_(true),
        ]))
    );
    assert_eq!(
        interpreter.lookup_name("len_error").unwrap(),
        Some(Value::string(
            "Python int too large to convert to C ssize_t"
        ))
    );
    assert_eq!(
        interpreter.lookup_name("min_step").unwrap(),
        Some(Value::list(vec![Value::int(i64::MAX), Value::int(-1)]))
    );
}

#[test]
fn closure_rebinding_uses_enclosing_frame() {
    let interpreter = run_program(
        "def outer():\n    x = 1\n    def inner():\n        return x\n    x = 2\n    return inner()\nresult = outer()\n",
    );

    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(2))
    );
}

#[test]
fn class_type_alias_uses_its_live_annotation_namespace_without_changing_methods() {
    let interpreter = run_program(concat!(
        "x = 10\n",
        "class C:\n",
        "    x = 1\n",
        "    type A = x\n",
        "    x = 2\n",
        "    during = A.__value__\n",
        "class D:\n",
        "    x = 4\n",
        "    type A = x\n",
        "del D.x\n",
        "class E:\n",
        "    T = 5\n",
        "    type A[T] = T\n",
        "    def read(self):\n",
        "        return x\n",
        "C.x = 3\n",
        "result = [C.during, C.A.__value__, D.A.__value__, E.A.__value__, E().read()]\n",
    ));

    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![
            Value::int(2),
            Value::int(2),
            Value::int(10),
            Value::int(5),
            Value::int(10),
        ]))
    );
}

#[test]
fn assigned_names_are_local_for_the_whole_function() {
    let tokens = Lexer::new("x = 7\ndef f():\n    y = x\n    x = 9\n    return y\nresult = f()\n")
        .unwrap()
        .into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "UnboundLocalError: cannot access local variable 'x' where it is not associated with a value"
    );
}

#[test]
fn bare_augmented_assignment_declares_a_local() {
    let error = run_program_expect_error("x = 7\ndef f():\n    x += 1\nf()\n");
    assert_eq!(
        error.to_string(),
        "UnboundLocalError: cannot access local variable 'x' where it is not associated with a value"
    );
}

#[test]
fn global_assignment_updates_module_binding() {
    let interpreter = run_program("x = 1\ndef f():\n    global x\n    x = 3\nf()\n");

    assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::int(3)));
}

#[test]
fn global_declaration_reads_module_binding() {
    let interpreter = run_program(
        "x = 7\ndef f():\n    global x\n    y = x\n    x = 9\n    return y\nresult = f()\n",
    );

    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(7))
    );
    assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::int(9)));
}

#[test]
fn nonlocal_assignment_updates_enclosing_function_binding() {
    let interpreter = run_program(
        "def outer():\n    x = 1\n    def inner():\n        nonlocal x\n        x = x + 4\n        return x\n    return [inner(), x]\nresult = outer()\n",
    );

    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![Value::int(5), Value::int(5)]))
    );
}

#[test]
fn nonlocal_without_enclosing_binding_errors() {
    let tokens = Lexer::new("def bad():\n    nonlocal x\n")
        .unwrap()
        .into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "SyntaxError: no binding for nonlocal 'x' found"
    );
}

// Issue #639: the SyntaxError must be raised at compile time — before any
// statement in the module executes.  If the check were deferred to the
// MakeFunction VM instruction, the print() would run first.
#[test]
fn nonlocal_without_binding_errors_before_any_statement_executes() {
    let src = "print('before')\ndef f():\n    nonlocal missing\n    missing = 1\nprint('after')\n";
    let tokens = Lexer::new(src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "SyntaxError: no binding for nonlocal 'missing' found"
    );
}

// Issue #639: nonlocal where the enclosing scope only has `global x` (not a
// true local binding) must also raise SyntaxError.
#[test]
fn nonlocal_where_enclosing_scope_has_global_errors() {
    let src = "x = 1\ndef outer():\n    global x\n    def inner():\n        nonlocal x\n        x = 99\n    inner()\nouter()\n";
    let tokens = Lexer::new(src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "SyntaxError: no binding for nonlocal 'x' found"
    );
}

// Issue #639: nonlocal in a doubly-nested function with no binding at any
// enclosing level must raise SyntaxError at compile time.
#[test]
fn nonlocal_doubly_nested_without_any_binding_errors() {
    let src = "print('before')\ndef outer():\n    def middle():\n        def inner():\n            nonlocal x\n            x = 10\n        inner()\n    middle()\nprint('after')\n";
    let tokens = Lexer::new(src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "SyntaxError: no binding for nonlocal 'x' found"
    );
}

#[test]
fn break_outside_loop_is_syntax_error() {
    let tokens = Lexer::new("break\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(error.to_string(), "SyntaxError: 'break' outside loop");
}

#[test]
fn continue_outside_loop_is_syntax_error() {
    let tokens = Lexer::new("continue\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "SyntaxError: 'continue' not properly in loop"
    );
}

#[test]
fn break_in_function_outside_loop_is_syntax_error() {
    let tokens = Lexer::new("def f():\n    break\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(error.to_string(), "SyntaxError: 'break' outside loop");
}

#[test]
fn annotated_assign_with_global_is_syntax_error() {
    // `x: int = 99` combined with `global x` must raise SyntaxError.
    let src = "x = 0\ndef f():\n    global x\n    x: int = 99\nf()\n";
    let tokens = Lexer::new(src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "SyntaxError: annotated name 'x' can't be global"
    );
}

#[test]
fn annotated_assign_with_nonlocal_is_syntax_error() {
    // `x: int = 5` combined with `nonlocal x` must raise SyntaxError.
    let src = "def outer():\n    x = 1\n    def inner():\n        nonlocal x\n        x: int = 5\n    inner()\nouter()\n";
    let tokens = Lexer::new(src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "SyntaxError: annotated name 'x' can't be nonlocal"
    );
}

#[test]
fn bare_annotation_with_global_is_syntax_error() {
    // `x: int` (bare annotation, no value) combined with `global x` must raise SyntaxError.
    let src = "x = 0\ndef f():\n    global x\n    x: int\nf()\n";
    let tokens = Lexer::new(src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        "SyntaxError: annotated name 'x' can't be global"
    );
}

#[test]
fn annotated_assign_without_conflict_works() {
    // `x: int = 5` with no global/nonlocal conflict should work normally.
    let interpreter = run_program("def f():\n    x: int = 42\n    return x\nresult = f()\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(42))
    );
}

#[test]
fn return_at_module_level_is_syntax_error() {
    let tokens = Lexer::new("return\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(error.to_string(), "SyntaxError: 'return' outside function");
}

#[test]
fn return_with_value_at_module_level_is_syntax_error() {
    let tokens = Lexer::new("return 42\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(error.to_string(), "SyntaxError: 'return' outside function");
}

#[test]
fn yield_at_module_level_is_syntax_error() {
    let tokens = Lexer::new("yield\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(error.to_string(), "SyntaxError: 'yield' outside function");
}

#[test]
fn yield_from_at_module_level_is_syntax_error() {
    let tokens = Lexer::new("yield from []\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(error.to_string(), "SyntaxError: 'yield' outside function");
}

#[test]
fn return_in_dead_code_at_module_level_is_syntax_error() {
    // CPython validates return/yield even in statically-dead branches.
    let tokens = Lexer::new("if False:\n    return\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(error.to_string(), "SyntaxError: 'return' outside function");
}

#[test]
fn yield_in_dead_code_at_module_level_is_syntax_error() {
    let tokens = Lexer::new("if False:\n    yield\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(error.to_string(), "SyntaxError: 'yield' outside function");
}

#[test]
fn dead_assignment_targets_still_validate_await_and_yield_context() {
    for (source, expected) in [
        (
            "def f(items):\n    if False:\n        items[(await g())] += 1\n",
            "SyntaxError: 'await' outside async function",
        ),
        (
            "def f(items):\n    if False:\n        items[(await g())] = 1\n",
            "SyntaxError: 'await' outside async function",
        ),
        (
            "if False:\n    items[(yield 0)] += 1\n",
            "SyntaxError: 'yield' outside function",
        ),
        (
            "def f(items):\n    if False:\n        items[(await g())].value += 1\n",
            "SyntaxError: 'await' outside async function",
        ),
        (
            "def f(items):\n    if False:\n        items[(lambda value=(await g()): 0)()] += 1\n",
            "SyntaxError: 'await' outside async function",
        ),
    ] {
        let tokens = Lexer::new(source).unwrap().into_tokens();
        let program = Parser::new(tokens).parse_program().unwrap();
        let error = Interpreter::default()
            .exec_program(&program, false)
            .unwrap_err();
        assert_eq!(error.to_string(), expected, "source:\n{source}");
    }
}

#[test]
fn dead_statement_headers_validate_every_evaluated_expression_context() {
    for source in [
        "def f():\n    if False:\n        return await g()\n",
        "def f():\n    if False:\n        raise await g()\n",
        "def f():\n    if False:\n        assert await g()\n",
        "def f():\n    if False:\n        if await g():\n            pass\n",
        "def f():\n    if False:\n        while await g():\n            pass\n",
        "def f():\n    if False:\n        for item in await g():\n            pass\n",
        "def f():\n    if False:\n        with await g():\n            pass\n",
        "def f():\n    if False:\n        match await g():\n            case _:\n                pass\n",
        "def f():\n    if False:\n        match 1:\n            case _ if await g():\n                pass\n",
        "def f():\n    if False:\n        try:\n            pass\n        except await g():\n            pass\n",
        "def outer():\n    if False:\n        def inner(value=await g()):\n            pass\n",
        "def outer():\n    if False:\n        def inner(value: await g()):\n            pass\n",
        "def f():\n    if False:\n        class Inner(await g()):\n            pass\n",
        "def f():\n    if False:\n        del items[await g()]\n",
    ] {
        let tokens = Lexer::new(source).unwrap().into_tokens();
        let program = Parser::new(tokens).parse_program().unwrap();
        let error = Interpreter::default()
            .exec_program(&program, false)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "SyntaxError: 'await' outside async function",
            "source:\n{source}"
        );
    }
}

#[test]
fn dead_expression_validation_preserves_cpython_error_precedence() {
    for (source, expected) in [
        (
            "if False:\n    items[(yield 0)] = await g()\n",
            "SyntaxError: 'await' outside function",
        ),
        (
            "if False:\n    items[await g()] = (yield 0)\n",
            "SyntaxError: 'yield' outside function",
        ),
        (
            "if False:\n    @(yield 1)\n    def f(value=await g()):\n        pass\n",
            "SyntaxError: 'yield' outside function",
        ),
    ] {
        let tokens = Lexer::new(source).unwrap().into_tokens();
        let program = Parser::new(tokens).parse_program().unwrap();
        let error = Interpreter::default()
            .exec_program(&program, false)
            .unwrap_err();
        assert_eq!(error.to_string(), expected, "source:\n{source}");
    }
}

#[test]
fn dead_class_body_does_not_inherit_enclosing_async_context() {
    for source in [
        "async def outer():\n    if False:\n        class Inner:\n            async for item in items:\n                pass\n",
        "async def outer():\n    if False:\n        class Inner:\n            async with manager:\n                pass\n",
    ] {
        let tokens = Lexer::new(source).unwrap().into_tokens();
        let program = Parser::new(tokens).parse_program().unwrap();
        let error = Interpreter::default()
            .exec_program(&program, false)
            .unwrap_err();
        let expected = if source.contains("async for") {
            "SyntaxError: 'async for' outside async function"
        } else {
            "SyntaxError: 'async with' outside async function"
        };
        assert_eq!(error.to_string(), expected, "source:\n{source}");
    }
}

#[test]
fn return_at_module_level_in_for_loop_is_syntax_error() {
    let tokens = Lexer::new("for i in [1, 2]:\n    return i\n")
        .unwrap()
        .into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();

    let error = interpreter.exec_program(&program, false).unwrap_err();

    assert_eq!(error.to_string(), "SyntaxError: 'return' outside function");
}
