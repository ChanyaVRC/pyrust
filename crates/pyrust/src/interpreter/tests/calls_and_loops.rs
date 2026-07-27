#[test]
fn deeply_recursive_function_raises_recursion_error() {
    let src = "
def inf():
    return inf()
caught = False
try:
    inf()
except RecursionError:
    caught = True
";
    // A modest native stack is intentional: recursive Python frames must live
    // in the VM trampoline rather than consuming this host thread's stack.
    let caught: bool = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let tokens = Lexer::new(src).unwrap().into_tokens();
            let program = Parser::new(tokens).parse_program().unwrap();
            let mut interp = Interpreter::default();
            interp.exec_program(&program, false).unwrap();
            let caught = interp.lookup_name("caught").unwrap() == Some(Value::bool_(true));
            assert!(
                interp.memo_in_flight.is_empty(),
                "an errored memo miss must remove its in-flight key"
            );
            caught
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(caught);
}

#[test]
fn distinctly_keyed_recursive_function_raises_recursion_error() {
    let src = "
def runaway(n):
    return runaway(n + 1)
caught = False
try:
    runaway(0)
except RecursionError:
    caught = True
";
    let caught: bool = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let tokens = Lexer::new(src).unwrap().into_tokens();
            let program = Parser::new(tokens).parse_program().unwrap();
            let mut interp = Interpreter::default();
            interp.exec_program(&program, false).unwrap();
            let caught = interp.lookup_name("caught").unwrap() == Some(Value::bool_(true));
            assert!(
                interp.memo_in_flight.is_empty(),
                "unwinding trampoline frames must cancel every distinct memo key"
            );
            caught
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(caught);
}

#[test]
fn recursion_depth_survives_trampoline_arena_segmentation() {
    const LIMIT: usize = 80;
    let mut src = format!(
        "import sys\nsys.setrecursionlimit({LIMIT})\n\
         def wide_runaway(n):\n"
    );
    // Make each frame wider than 1/80 of the fixed 16K-value arena so the
    // recursion must cross at least one trampoline -> native VM boundary.
    for index in 0..320 {
        src.push_str(&format!("    local_{index} = n\n"));
    }
    src.push_str(
        r#"    return wide_runaway(n + 1)
caught = False
traceback_depth = 0
try:
    wide_runaway(0)
except RecursionError as error:
    caught = True
    tb = error.__traceback__
    while tb is not None:
        traceback_depth += 1
        tb = tb.tb_next
"#,
    );

    let ok = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let tokens = Lexer::new(&src).unwrap().into_tokens();
            let program = Parser::new(tokens).parse_program().unwrap();
            let mut interp = Interpreter::default();
            interp.exec_program(&program, false).unwrap();

            let function = interp.lookup_name("wide_runaway").unwrap().unwrap();
            let ValueKind::UserFunction(function) = function.kind() else {
                panic!("wide_runaway must be a user function");
            };
            let code = function
                .precompiled_code
                .as_ref()
                .unwrap()
                .clone()
                .downcast::<FnCode>()
                .unwrap();
            assert!(
                code.num_regs as usize * LIMIT > 16 * 1024,
                "test must exhaust one trampoline arena before the recursion limit"
            );

            interp.lookup_name("caught").unwrap() == Some(Value::bool_(true))
                && interp
                    .lookup_name("traceback_depth")
                    .unwrap()
                    .is_some_and(|depth| {
                        depth
                            .as_int()
                            .is_some_and(|depth| depth <= LIMIT as i64 + 2)
                    })
                && interp.memo_in_flight.is_empty()
                && get_call_depth() == 0
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(ok);
}

#[test]
fn recursion_within_limit_succeeds() {
    let src = "
def count(n):
    if n == 0:
        return 0
    return 1 + count(n - 1)
result = count(200)
";
    // The recursive frames are VM-trampolined; keep enough native stack only
    // for the debug interpreter entry machinery.
    let ok: bool = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let tokens = Lexer::new(src).unwrap().into_tokens();
            let program = Parser::new(tokens).parse_program().unwrap();
            let mut interp = Interpreter::default();
            interp.exec_program(&program, false).unwrap();
            interp.lookup_name("result").unwrap() == Some(Value::int(200))
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(ok);
}

#[test]
fn pure_recursive_function_runs_correctly() {
    // Pure self-recursive function correctness via the full Interpreter
    // path.  (The fn_cache memoization that once collapsed this to 35
    // unique calls was removed in #1987; keep n modest so the full call
    // tree stays fast under the debug test build.)
    let src = "
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)
result = fib(30)
";
    let ok: bool = std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let tokens = Lexer::new(src).unwrap().into_tokens();
            let program = Parser::new(tokens).parse_program().unwrap();
            let mut interp = Interpreter::default();
            interp.exec_program(&program, false).unwrap();
            interp.lookup_name("result").unwrap() == Some(Value::int(832040))
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(ok);
}

#[test]
fn impure_function_is_not_cached() {
    // A function that calls `print` is detected as impure and must execute
    // its body on every call (no stale cache returns).
    let interpreter = run_program(
        "log = []\ndef track(n):\n    print(n)\n    return n * 2\na = track(3)\nb = track(3)\n",
    );
    assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::int(6)));
    assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::int(6)));
}

#[test]
fn function_with_global_write_is_not_cached() {
    let interpreter = run_program(
        "count = 0\ndef inc(n):\n    global count\n    count += n\n    return count\na = inc(1)\nb = inc(1)\n",
    );
    assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::int(1)));
    assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::int(2)));
}

#[test]
fn fastpath_semantics_memo_call_observes_defaults_reassignment() {
    let interpreter = run_program(
        "def plus_one(x=1):\n    return x + 1\nbefore = plus_one()\nplus_one.__defaults__ = (5,)\nafter = plus_one()\n",
    );
    assert_eq!(
        interpreter.lookup_name("before").unwrap(),
        Some(Value::int(2))
    );
    assert_eq!(
        interpreter.lookup_name("after").unwrap(),
        Some(Value::int(6))
    );
}

#[test]
fn recursive_memo_call_observes_transitive_defaults_reassignment() {
    let interpreter = run_program(
        r#"def resolve(n, explicit, fallback=1):
    if n == 0:
        return fallback
    return resolve(n - 1, explicit)
before = resolve(1, 99, 88)
resolve.__defaults__ = (2,)
after = resolve(1, 99, 88)
"#,
    );
    assert_eq!(
        interpreter.lookup_name("before").unwrap(),
        Some(Value::int(1))
    );
    assert_eq!(
        interpreter.lookup_name("after").unwrap(),
        Some(Value::int(2))
    );
    let function = interpreter.lookup_name("resolve").unwrap().unwrap();
    let ValueKind::UserFunction(function) = function.kind() else {
        panic!("resolve must be a user function");
    };
    assert!(
        !function.is_memo_pure,
        "a direct self-call that can consume mutable defaults must not use CallMemo"
    );
}

#[test]
fn fastpath_semantics_builtin_spelling_does_not_prove_memo_purity() {
    let interpreter = run_program(
        r#"calls = 0
def fake(value):
    global calls
    calls += 1
    return calls

abs = fake
def wrapped(value):
    return abs(value)

first = wrapped(5)
second = wrapped(5)
"#,
    );
    assert_eq!(
        interpreter.lookup_name("first").unwrap(),
        Some(Value::int(1))
    );
    assert_eq!(
        interpreter.lookup_name("second").unwrap(),
        Some(Value::int(2))
    );
    assert_eq!(
        interpreter.lookup_name("calls").unwrap(),
        Some(Value::int(2))
    );
}

#[test]
fn fastpath_semantics_sibling_reassignment_does_not_poison_memoization() {
    let interpreter = run_program(
        r#"calls = 0
def original(value):
    return value + 10

def wrapped(value):
    return original(value)

def replacement(value):
    global calls
    calls += 1
    return calls

original = replacement
first = wrapped(5)
second = wrapped(5)
"#,
    );
    assert_eq!(
        interpreter.lookup_name("first").unwrap(),
        Some(Value::int(1))
    );
    assert_eq!(
        interpreter.lookup_name("second").unwrap(),
        Some(Value::int(2))
    );
    assert_eq!(
        interpreter.lookup_name("calls").unwrap(),
        Some(Value::int(2))
    );
}

#[test]
fn shared_builtins_module_mutation_invalidates_another_interpreters_fncode_cache() {
    let builtins = cached_builtins_module();
    let ValueKind::PyModule(module) = builtins.kind() else {
        panic!("canonical builtins provider must be a module");
    };
    let original_len = module
        .borrow()
        .attrs
        .get("len")
        .cloned()
        .expect("builtins.len");

    let mut first = run_program("def read_len():\n    return len\nbefore = read_len()\n");
    let second_result = {
        let mut second = Interpreter::default();
        second.assign_attr(builtins.clone(), "len", Value::int(71))
    };
    let after_result = first.exec_source("after = read_len()\n", None, None);

    // Restore the thread-local provider before asserting so a failure cannot
    // contaminate another test later on this worker thread.
    module
        .borrow_mut()
        .insert_attr("len".to_string(), original_len);

    second_result.unwrap();
    after_result.unwrap();
    assert_eq!(first.lookup_name("after").unwrap(), Some(Value::int(71)));
}

#[test]
fn while_true_loop_runs_until_break() {
    let interpreter =
        run_program("n = 0\nwhile True:\n    n += 1\n    if n == 5:\n        break\n");
    assert_eq!(interpreter.lookup_name("n").unwrap(), Some(Value::int(5)));
}

#[test]
fn while_false_body_never_runs() {
    let interpreter = run_program("x = 0\nwhile False:\n    x = 99\n");
    assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::int(0)));
}

#[test]
fn while_false_else_branch_runs() {
    let interpreter = run_program("x = 0\nwhile False:\n    x = 99\nelse:\n    x = 42\n");
    assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::int(42)));
}

#[test]
fn enumerate_builtin_yields_index_value_pairs() {
    let interpreter = run_program("result = list(enumerate(['a', 'b', 'c']))\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![
            Value::tuple(vec![Value::int(0), Value::string("a")]),
            Value::tuple(vec![Value::int(1), Value::string("b")]),
            Value::tuple(vec![Value::int(2), Value::string("c")]),
        ]))
    );
}

#[test]
fn enumerate_with_start_offset() {
    let interpreter = run_program("result = list(enumerate(['x', 'y'], 10))\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![
            Value::tuple(vec![Value::int(10), Value::string("x")]),
            Value::tuple(vec![Value::int(11), Value::string("y")]),
        ]))
    );
}

#[test]
fn fastpath_semantics_loop_retains_list_when_source_local_is_rebound() {
    let interpreter = run_program(
        "items = [1, 2, 3]\nseen = []\nfor item in items:\n    seen.append(item)\n    items = [9]\n",
    );
    assert_eq!(
        interpreter.lookup_name("seen").unwrap(),
        Some(Value::list(vec![
            Value::int(1),
            Value::int(2),
            Value::int(3),
        ]))
    );
}

#[test]
fn fastpath_semantics_enumerate_preserves_shared_iterator_identity() {
    let interpreter = run_program(
        r#"iterator = enumerate([10, 20, 30])
loop_indices = []
alias_indices = []
stopped = 0
for pair in iterator:
    loop_indices.append(pair[0])
    try:
        alias_indices.append(next(iterator)[0])
    except StopIteration:
        stopped += 1
"#,
    );
    assert_eq!(
        interpreter.lookup_name("loop_indices").unwrap(),
        Some(Value::list(vec![Value::int(0), Value::int(2)]))
    );
    assert_eq!(
        interpreter.lookup_name("alias_indices").unwrap(),
        Some(Value::list(vec![Value::int(1)]))
    );
    assert_eq!(
        interpreter.lookup_name("stopped").unwrap(),
        Some(Value::int(1))
    );
}

#[test]
fn fastpath_semantics_next_cache_observes_class_mutation() {
    let interpreter = run_program(
        r#"class Iterator:
    def __init__(self):
        self.index = 0
    def __iter__(self):
        return self
    def __next__(self):
        self.index += 1
        if self.index == 1:
            Iterator.__next__ = replacement
            return "original"
        raise StopIteration

def replacement(self):
    self.index += 1
    if self.index == 2:
        return "replacement"
    raise StopIteration

seen = []
for item in Iterator():
    seen.append(item)
"#,
    );
    assert_eq!(
        interpreter.lookup_name("seen").unwrap(),
        Some(Value::list(vec![
            Value::string("original"),
            Value::string("replacement"),
        ]))
    );
}

#[test]
fn zip_builtin_pairs_two_iterables() {
    let interpreter = run_program("result = list(zip([1, 2, 3], ['a', 'b', 'c']))\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![
            Value::tuple(vec![Value::int(1), Value::string("a")]),
            Value::tuple(vec![Value::int(2), Value::string("b")]),
            Value::tuple(vec![Value::int(3), Value::string("c")]),
        ]))
    );
}

#[test]
fn zip_truncates_to_shortest() {
    let interpreter = run_program("result = list(zip([1, 2, 3], [10, 20]))\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![
            Value::tuple(vec![Value::int(1), Value::int(10)]),
            Value::tuple(vec![Value::int(2), Value::int(20)]),
        ]))
    );
}

#[test]
fn sorted_builtin_returns_sorted_list() {
    let interpreter = run_program("result = sorted([3, 1, 4, 1, 5, 9, 2, 6])\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![
            Value::int(1),
            Value::int(1),
            Value::int(2),
            Value::int(3),
            Value::int(4),
            Value::int(5),
            Value::int(6),
            Value::int(9),
        ]))
    );
}

#[test]
fn sorted_with_reverse_flag() {
    let interpreter = run_program("result = sorted([3, 1, 2], reverse=True)\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![
            Value::int(3),
            Value::int(2),
            Value::int(1)
        ]))
    );
}

#[test]
fn reversed_builtin_reverses_list() {
    let interpreter = run_program("result = list(reversed([1, 2, 3]))\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::list(vec![
            Value::int(3),
            Value::int(2),
            Value::int(1)
        ]))
    );
}

#[test]
fn abs_min_max_sum_work() {
    let interpreter =
        run_program("a = abs(-7)\nb = min(3, 1, 4)\nc = max([5, 2, 8])\nd = sum([1, 2, 3, 4])\n");
    assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::int(7)));
    assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::int(1)));
    assert_eq!(interpreter.lookup_name("c").unwrap(), Some(Value::int(8)));
    assert_eq!(interpreter.lookup_name("d").unwrap(), Some(Value::int(10)));
}

#[test]
fn int_float_str_bool_conversions() {
    let interpreter =
        run_program("a = int('42')\nb = float('2.5')\nc = str(100)\nd = bool(0)\ne = bool(1)\n");
    assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::int(42)));
    assert_eq!(
        interpreter.lookup_name("b").unwrap(),
        Some(Value::float(2.5))
    );
    assert_eq!(
        interpreter.lookup_name("c").unwrap(),
        Some(Value::string("100"))
    );
    assert_eq!(
        interpreter.lookup_name("d").unwrap(),
        Some(Value::bool_(false))
    );
    assert_eq!(
        interpreter.lookup_name("e").unwrap(),
        Some(Value::bool_(true))
    );
}

#[test]
fn int_add_specializes_after_repeated_calls() {
    let interpreter = run_program("total = 0\nfor i in range(20):\n    total = total + i\n");
    assert_eq!(
        interpreter.lookup_name("total").unwrap(),
        Some(Value::int(190))
    );
}

#[test]
fn specialize_degrades_gracefully_on_type_change() {
    let interpreter = run_program(
        "def add(a, b):\n    return a + b\nx = add(1, 2)\ny = add('hello', ' world')\n",
    );
    assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::int(3)));
    assert_eq!(
        interpreter.lookup_name("y").unwrap(),
        Some(Value::string("hello world"))
    );
}

#[test]
fn def_bound_params_read_without_unbound_error() {
    let interpreter = run_program("def add(a, b):\n    return a + b\nresult = add(3, 4)\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(7))
    );
}

#[test]
fn unconditional_top_level_assign_is_def_bound() {
    let interpreter = run_program("def f():\n    x = 10\n    return x + 1\nresult = f()\n");
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(11))
    );
}

#[test]
fn hot_frame_reuse_produces_correct_results() {
    // Call a simple function more than HOT_THRESHOLD times to trigger
    // hot-frame promotion; results must remain correct.
    let src = "
def square(n):
    return n * n
total = 0
for i in range(60):
    total = total + square(i)
";
    let ok: bool = std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let tokens = Lexer::new(src).unwrap().into_tokens();
            let program = Parser::new(tokens).parse_program().unwrap();
            let mut interp = Interpreter::default();
            interp.exec_program(&program, false).unwrap();
            // sum of squares 0..59 = (59*60*119)/6 = 70,210
            interp.lookup_name("total").unwrap() == Some(Value::int(70210))
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(ok);
}

#[test]
fn hot_frame_falls_back_for_recursive_calls() {
    // A hot function that calls itself must use the normal path (recursion guard),
    // not the hot frame, for the recursive invocation.
    let src = "
def fact(n):
    if n <= 1:
        return 1
    return n * fact(n - 1)
result = fact(10)
";
    let ok: bool = std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let tokens = Lexer::new(src).unwrap().into_tokens();
            let program = Parser::new(tokens).parse_program().unwrap();
            let mut interp = Interpreter::default();
            interp.exec_program(&program, false).unwrap();
            interp.lookup_name("result").unwrap() == Some(Value::int(3628800))
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(ok);
}

#[test]
fn aug_assign_fused_rmw_sum_loop() {
    let interpreter = run_program("total = 0\nfor i in range(100):\n    total += i\n");
    assert_eq!(
        interpreter.lookup_name("total").unwrap(),
        Some(Value::int(4950))
    );
}

#[test]
fn range_step1_sum_matches_formula() {
    // sum(range(1000)) = 999*1000/2 = 499500
    let interpreter = run_program("s = 0\nfor i in range(1000):\n    s += i\n");
    assert_eq!(
        interpreter.lookup_name("s").unwrap(),
        Some(Value::int(499500))
    );
}

#[test]
fn range_step1_break_preserves_value() {
    let interpreter =
        run_program("s = 0\nfor i in range(100):\n    if i == 10:\n        break\n    s += i\n");
    assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::int(45)));
}

#[test]
fn boxed_return_signal_propagates_correctly() {
    let interpreter = run_program(
        "def f(n):\n    for i in range(n):\n        if i == 5:\n            return i * 2\n    return -1\nresult = f(10)\n",
    );
    assert_eq!(
        interpreter.lookup_name("result").unwrap(),
        Some(Value::int(10))
    );
}

#[test]
fn nested_return_propagates_through_loops() {
    let interpreter = run_program(
        "def search(lst, target):\n    for i, x in enumerate(lst):\n        if x == target:\n            return i\n    return -1\nidx = search([10, 20, 30, 40], 30)\n",
    );
    assert_eq!(interpreter.lookup_name("idx").unwrap(), Some(Value::int(2)));
}

#[test]
fn while_compare_increment_detected_as_range() {
    // while i < n: ...; i += 1 should behave identically to for i in range(n)
    let interpreter = run_program("i = 0\ns = 0\nwhile i < 10:\n    s += i\n    i += 1\n");
    assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::int(45)));
    assert_eq!(interpreter.lookup_name("i").unwrap(), Some(Value::int(10)));
}

#[test]
fn while_le_increment_detected() {
    let interpreter =
        run_program("i = 1\nproduct = 1\nwhile i <= 5:\n    product *= i\n    i += 1\n");
    assert_eq!(
        interpreter.lookup_name("product").unwrap(),
        Some(Value::int(120))
    );
    assert_eq!(interpreter.lookup_name("i").unwrap(), Some(Value::int(6)));
}

#[test]
fn small_range_unroll_correct() {
    let interpreter = run_program("s = 0\nfor i in range(4):\n    s += i\n");
    assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::int(6)));
}

#[test]
fn while_with_continue_not_converted() {
    // continue in body means increment might be skipped — don't convert
    let interpreter = run_program(
        "i = 0\ns = 0\nwhile i < 10:\n    i += 1\n    if i % 2 == 0:\n        continue\n    s += i\n",
    );
    // Sum of odd numbers 1..9: 1+3+5+7+9 = 25
    assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::int(25)));
}

#[test]
fn for_loop_list_lazy_iter_sum() {
    let interpreter =
        run_program("lst = [1, 2, 3, 4, 5]\ntotal = 0\nfor x in lst:\n    total += x\n");
    assert_eq!(
        interpreter.lookup_name("total").unwrap(),
        Some(Value::int(15))
    );
}

#[test]
fn for_loop_tuple_unpack_sum() {
    let interpreter = run_program(
        "pairs = [(1, 10), (2, 20), (3, 30)]\ntotal = 0\nfor a, b in pairs:\n    total += a + b\n",
    );
    assert_eq!(
        interpreter.lookup_name("total").unwrap(),
        Some(Value::int(66))
    );
}

#[test]
fn for_loop_tuple_unpack_names() {
    let interpreter = run_program(
        "pairs = [('hello', 1), ('world', 2)]\nlast_k = ''\nfor k, v in pairs:\n    last_k = k\n",
    );
    // Just verify it runs without panic and assigned the last key
    assert_eq!(
        interpreter.lookup_name("last_k").unwrap(),
        Some(Value::string("world"))
    );
}
#[test]
fn licm_invariant_flag_loop() {
    // 'done' is never assigned in the body → condition is invariant
    // loop exits via break
    let interpreter = run_program(
        "done = False\ncount = 0\nwhile not done:\n    count += 1\n    if count >= 10:\n        break\n",
    );
    assert_eq!(
        interpreter.lookup_name("count").unwrap(),
        Some(Value::int(10))
    );
}

#[test]
fn licm_false_condition_skips_body() {
    let interpreter = run_program("x = False\nran = False\nwhile x:\n    ran = True\n");
    assert_eq!(
        interpreter.lookup_name("ran").unwrap(),
        Some(Value::bool_(false))
    );
}

#[test]
fn licm_not_applied_when_condition_name_modified() {
    // 'i' IS modified in body → NOT invariant → normal while loop behavior
    let interpreter = run_program("i = 0\ns = 0\nwhile i < 10:\n    s += i\n    i += 1\n");
    assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::int(45)));
    assert_eq!(interpreter.lookup_name("i").unwrap(), Some(Value::int(10)));
}

#[test]
fn licm_not_applied_when_body_mutates_condition_container() {
    // Issue #2034: `while x: x.pop()` mutates the container in place without
    // reassigning the register `x`.  LICM must NOT hoist the truthiness
    // check — the loop must drain to empty and stop, not over-iterate.
    let interpreter = run_program("x = [1, 2, 3]\nn = 0\nwhile x:\n    x.pop()\n    n += 1\n");
    assert_eq!(interpreter.lookup_name("n").unwrap(), Some(Value::int(3)));
    let x = interpreter.lookup_name("x").unwrap().unwrap();
    assert!(!x.truthy_raw(), "x should be drained to empty");
}

#[test]
fn licm_not_applied_when_body_mutates_via_alias() {
    // Issue #2034 (aliasing): the mutation flows through a different name
    // that aliases the condition's object.  Still must drain.
    let interpreter =
        run_program("a = [1, 2, 3]\nb = a\nn = 0\nwhile a:\n    b.pop()\n    n += 1\n");
    assert_eq!(interpreter.lookup_name("n").unwrap(), Some(Value::int(3)));
}

// ── Register-VM specific tests ──────────────────────────────────────────
