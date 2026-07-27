#[test]
fn annotation_global_in_class_body_bare_is_syntax_error() {
    // `class C: global x; x: int` must raise SyntaxError (issue #770).
    let err = run_program_expect_error("x = 0\nclass C:\n    global x\n    x: int\n");
    assert_eq!(
        err.to_string(),
        "SyntaxError: annotated name 'x' can't be global"
    );
}

#[test]
fn annotation_global_in_class_body_assign_is_syntax_error() {
    // `class C: global x; x: int = 99` must raise SyntaxError (issue #770).
    let err = run_program_expect_error("x = 0\nclass C:\n    global x\n    x: int = 99\n");
    assert_eq!(
        err.to_string(),
        "SyntaxError: annotated name 'x' can't be global"
    );
}

#[test]
fn annotation_nonlocal_in_class_body_is_syntax_error() {
    // `class C: nonlocal x; x: int = 5` inside a function must raise
    // SyntaxError (issue #770).
    let err = run_program_expect_error(
        "def outer():\n    x = 1\n    class C:\n        nonlocal x\n        x: int = 5\n    C()\nouter()\n",
    );
    assert_eq!(
        err.to_string(),
        "SyntaxError: annotated name 'x' can't be nonlocal"
    );
}

#[test]
fn annotation_global_in_class_body_order_independent() {
    // Annotation before `global` must also be a SyntaxError — CPython
    // checks the whole scope, not statement order (issue #770).
    let err = run_program_expect_error("x = 0\nclass E:\n    x: int\n    global x\n");
    assert_eq!(
        err.to_string(),
        "SyntaxError: annotated name 'x' can't be global"
    );
}

#[test]
fn collections_defaultdict_rejects_non_callable_factory() {
    // The eager check in `bodies/collections.rs` surfaces a clean
    // TypeError at construction time instead of letting the
    // interpreter blow up on the first missing-key access.
    let err = run_program_expect_error("from collections import defaultdict\ndefaultdict(42)\n");
    let msg = err.to_string();
    assert!(
        msg.contains("TypeError") && msg.contains("callable"),
        "expected `must be callable` TypeError; got: {msg}"
    );
}

// ── #748: annotated name can't be global / nonlocal ─────────────────────

fn expect_syntax_error(src: &str, expected_msg: &str) {
    let tokens = Lexer::new(src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut interpreter = Interpreter::default();
    let err = interpreter.exec_program(&program, false).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("SyntaxError") && msg.contains(expected_msg),
        "expected SyntaxError containing '{expected_msg}'; got: {msg}"
    );
}

#[test]
fn bare_annotation_after_global_is_syntax_error() {
    expect_syntax_error(
        "x = 0\ndef f():\n    global x\n    x: int\nf()\n",
        "annotated name 'x' can't be global",
    );
}

#[test]
fn bare_annotation_before_global_is_syntax_error() {
    // Order must not matter: annotation before global declaration.
    expect_syntax_error(
        "x = 0\ndef f():\n    x: int\n    global x\nf()\n",
        "annotated name 'x' can't be global",
    );
}

#[test]
fn bare_annotation_after_nonlocal_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    x = 1\n    def g():\n        nonlocal x\n        x: int\n    g()\nf()\n",
        "annotated name 'x' can't be nonlocal",
    );
}

#[test]
fn bare_annotation_before_nonlocal_is_syntax_error() {
    // Order must not matter: annotation before nonlocal declaration.
    expect_syntax_error(
        "def f():\n    x = 1\n    def g():\n        x: int\n        nonlocal x\n    g()\nf()\n",
        "annotated name 'x' can't be nonlocal",
    );
}

#[test]
fn global_without_annotation_is_ok() {
    let interp = run_program("g = 0\ndef h():\n    global g\n    g = 42\nh()\n");
    assert_eq!(interp.lookup_name("g").unwrap(), Some(Value::int(42)));
}

#[test]
fn bare_annotation_without_global_or_nonlocal_is_ok() {
    // A bare annotation inside a function with no global/nonlocal conflict
    // must not raise any error.
    let _ = run_program("def k():\n    z: int\nk()\n");
}

// ── #1281: global/nonlocal after use or assignment raises SyntaxError ────

#[test]
fn global_after_assignment_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    x = 1\n    global x\n",
        "name 'x' is assigned to before global declaration",
    );
}

#[test]
fn global_after_use_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    print(x)\n    global x\n",
        "name 'x' is used prior to global declaration",
    );
}

#[test]
fn global_after_lambda_default_use_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    (lambda value=x: value)\n    global x\n",
        "name 'x' is used prior to global declaration",
    );
}

#[test]
fn global_after_lambda_default_assignment_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    (lambda value=(x := 1): value)\n    global x\n",
        "name 'x' is assigned to before global declaration",
    );
}

#[test]
fn global_after_comprehension_outer_iter_use_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    [item for item in source]\n    global source\n",
        "name 'source' is used prior to global declaration",
    );
}

#[test]
fn global_after_comprehension_walrus_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    [(result := item) for item in ()]\n    global result\n",
        "name 'result' is assigned to before global declaration",
    );
}

#[test]
fn global_after_function_default_use_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    def inner(value=x):\n        pass\n    global x\n",
        "name 'x' is used prior to global declaration",
    );
}

#[test]
fn pep695_annotation_scope_reads_do_not_precede_global() {
    let _ = run_program(
        r#"x = int
def outer():
    type Alias = x
    def generic[T: x](value: x):
        return value
    global x
    return Alias, generic
alias, generic = outer()
"#,
    );
}

#[test]
fn generic_function_default_still_precedes_global() {
    expect_syntax_error(
        "def outer():\n    def generic[T](value=x):\n        pass\n    global x\n",
        "name 'x' is used prior to global declaration",
    );
}

#[test]
fn nested_lambda_and_comprehension_body_reads_do_not_precede_global() {
    let _ = run_program(
        "x = 1\ndef f():\n    (lambda: x)\n    [x for item in ()]\n    global x\n    return x\nf()\n",
    );
}

#[test]
fn nonlocal_after_assignment_is_syntax_error() {
    expect_syntax_error(
        "def outer():\n    y = 0\n    def f():\n        y = 10\n        nonlocal y\n",
        "name 'y' is assigned to before nonlocal declaration",
    );
}

#[test]
fn nonlocal_after_use_is_syntax_error() {
    expect_syntax_error(
        "def outer():\n    y = 0\n    def f():\n        print(y)\n        nonlocal y\n",
        "name 'y' is used prior to nonlocal declaration",
    );
}

#[test]
fn global_after_for_target_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    for x in [1]: pass\n    global x\n",
        "name 'x' is assigned to before global declaration",
    );
}

#[test]
fn global_after_nested_if_assignment_is_syntax_error() {
    expect_syntax_error(
        "def f():\n    if True:\n        x = 1\n    global x\n",
        "name 'x' is assigned to before global declaration",
    );
}

#[test]
fn global_after_assign_and_use_reports_used_prior() {
    // When x is both assigned AND used before `global x`, CPython 3.12
    // always reports "used prior to" (not "assigned to").
    expect_syntax_error(
        "def f():\n    x = 1\n    print(x)\n    global x\n",
        "name 'x' is used prior to global declaration",
    );
}

#[test]
fn global_before_assignment_is_valid() {
    // global declared BEFORE any assignment is fine.
    let interp = run_program("g = 0\ndef h():\n    global g\n    g = 7\nh()\n");
    assert_eq!(interp.lookup_name("g").unwrap(), Some(Value::int(7)));
}

#[test]
fn nonlocal_before_assignment_is_valid() {
    // nonlocal declared BEFORE any assignment is fine.
    let interp = run_program(
        "def outer():\n    x = 0\n    def inner():\n        nonlocal x\n        x = 5\n    inner()\n    return x\nresult = outer()\n",
    );
    assert_eq!(interp.lookup_name("result").unwrap(), Some(Value::int(5)));
}
