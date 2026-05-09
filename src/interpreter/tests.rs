#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn run_program(src: &str) -> Interpreter {
        let tokens = Lexer::new(src).unwrap().into_tokens();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut interpreter = Interpreter::default();
        interpreter.exec_program(&program, false).unwrap();
        interpreter
    }

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
    fn closure_rebinding_uses_enclosing_frame() {
        let interpreter = run_program(
            "def outer():\n    x = 1\n    def inner():\n        return x\n    x = 2\n    return inner()\nresult = outer()\n",
        );

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::Int(2))
        );
    }

    #[test]
    fn assigned_names_are_local_for_the_whole_function() {
        let tokens =
            Lexer::new("x = 7\ndef f():\n    y = x\n    x = 9\n    return y\nresult = f()\n")
                .unwrap()
                .into_tokens();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut interpreter = Interpreter::default();

        let error = interpreter.exec_program(&program, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Runtime error: cannot access local variable 'x' where it is not associated with a value"
        );
    }

    #[test]
    fn global_assignment_updates_module_binding() {
        let interpreter = run_program("x = 1\ndef f():\n    global x\n    x = 3\nf()\n");

        assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::Int(3)));
    }

    #[test]
    fn global_declaration_reads_module_binding() {
        let interpreter = run_program(
            "x = 7\ndef f():\n    global x\n    y = x\n    x = 9\n    return y\nresult = f()\n",
        );

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::Int(7))
        );
        assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::Int(9)));
    }

    #[test]
    fn nonlocal_assignment_updates_enclosing_function_binding() {
        let interpreter = run_program(
            "def outer():\n    x = 1\n    def inner():\n        nonlocal x\n        x = x + 4\n        return x\n    return [inner(), x]\nresult = outer()\n",
        );

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![Value::Int(5), Value::Int(5)]))
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
            "Runtime error: no binding for nonlocal 'x' found"
        );
    }

    #[test]
    fn class_instances_bind_methods_and_init() {
        let interpreter = run_program(
            "class Counter:\n    def __init__(self, start):\n        self.value = start\n    def inc(self, step=1):\n        self.value = self.value + step\n        return self.value\nc = Counter(10)\nresult = [c.value, c.inc(), c.inc(4), c.value]\n",
        );

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![
                Value::Int(10),
                Value::Int(11),
                Value::Int(15),
                Value::Int(15),
            ]))
        );
    }

    #[test]
    fn derived_class_inherits_init_methods_and_class_attrs() {
        let interpreter = run_program(
            "class Base:\n    kind = 'base'\n    def __init__(self, value):\n        self.value = value\n    def total(self, extra=1):\n        return self.value + extra\nclass Derived(Base):\n    pass\nd = Derived(10)\nresult = [d.kind, d.value, d.total(), Derived.kind]\n",
        );

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![
                Value::Str("base".to_string()),
                Value::Int(10),
                Value::Int(11),
                Value::Str("base".to_string()),
            ]))
        );
    }

    #[test]
    fn raise_try_except_else_and_finally_work() {
        let interpreter = run_program(
            "events = []\ntry:\n    raise ValueError('bad')\nexcept ValueError as err:\n    events = [err.args[0]]\nelse:\n    events = ['else']\nfinally:\n    events = events + ['finally']\n",
        );

        assert_eq!(
            interpreter.lookup_name("events").unwrap(),
            Some(Value::List(vec![
                Value::Str("bad".to_string()),
                Value::Str("finally".to_string()),
            ]))
        );
    }

    #[test]
    fn bare_raise_reraises_active_exception() {
        let interpreter = run_program(
            "result = ''\ntry:\n    try:\n        raise RuntimeError('inner')\n    except RuntimeError:\n        raise\nexcept RuntimeError as err:\n    result = err.args[0]\n",
        );

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::Str("inner".to_string()))
        );
    }

    #[test]
    fn runtime_errors_inside_try_are_catchable_as_runtimeerror() {
        let interpreter = run_program(
            "result = ''\ntry:\n    x = 1 / 0\nexcept RuntimeError as err:\n    result = err.args[0]\n",
        );

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::Str("division by zero".to_string()))
        );
    }

    #[test]
    fn import_math_module_provides_constants_and_functions() {
        let interpreter = run_program(
            "import math\npi_val = math.pi\nfloor_val = math.floor(2.9)\nsqrt_val = math.sqrt(16.0)\n",
        );

        assert_eq!(
            interpreter.lookup_name("pi_val").unwrap(),
            Some(Value::Float(std::f64::consts::PI))
        );
        assert_eq!(
            interpreter.lookup_name("floor_val").unwrap(),
            Some(Value::Int(2))
        );
        assert_eq!(
            interpreter.lookup_name("sqrt_val").unwrap(),
            Some(Value::Float(4.0))
        );
    }

    #[test]
    fn from_math_import_binds_names_directly() {
        let interpreter =
            run_program("from math import pi, floor\npi_val = pi\nfloor_val = floor(3.7)\n");

        assert_eq!(
            interpreter.lookup_name("pi_val").unwrap(),
            Some(Value::Float(std::f64::consts::PI))
        );
        assert_eq!(
            interpreter.lookup_name("floor_val").unwrap(),
            Some(Value::Int(3))
        );
    }

    #[test]
    fn import_alias_binds_module_under_alias_name() {
        let interpreter = run_program("import math as m\nresult = m.floor(5.8)\n");

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::Int(5))
        );
    }

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
        // Must run on a large-stack thread; Interpreter (Rc-based) is created
        // inside so Send is not required for the interpreter itself.
        let caught: bool = std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(move || {
                let tokens = Lexer::new(src).unwrap().into_tokens();
                let program = Parser::new(tokens).parse_program().unwrap();
                let mut interp = Interpreter::default();
                interp.exec_program(&program, false).unwrap();
                matches!(interp.lookup_name("caught").unwrap(), Some(Value::Bool(true)))
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(caught);
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
        // Large-stack thread because 200 Python frames × ~80 KB/frame in debug
        // mode exceeds the test harness's default 8 MB stack.
        let ok: bool = std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(move || {
                let tokens = Lexer::new(src).unwrap().into_tokens();
                let program = Parser::new(tokens).parse_program().unwrap();
                let mut interp = Interpreter::default();
                interp.exec_program(&program, false).unwrap();
                matches!(interp.lookup_name("result").unwrap(), Some(Value::Int(200)))
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn pure_recursive_function_is_memoized() {
        // fib(35) makes ~39 million calls without memoization — far too slow
        // to finish in a test.  With memoization it needs only 35 unique calls.
        let src = "
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)
result = fib(35)
";
        let ok: bool = std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(move || {
                let tokens = Lexer::new(src).unwrap().into_tokens();
                let program = Parser::new(tokens).parse_program().unwrap();
                let mut interp = Interpreter::default();
                interp.exec_program(&program, false).unwrap();
                matches!(interp.lookup_name("result").unwrap(), Some(Value::Int(9227465)))
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
        assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::Int(6)));
        assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::Int(6)));
    }

    #[test]
    fn function_with_global_write_is_not_cached() {
        let interpreter = run_program(
            "count = 0\ndef inc(n):\n    global count\n    count += n\n    return count\na = inc(1)\nb = inc(1)\n",
        );
        assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::Int(1)));
        assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::Int(2)));
    }

    #[test]
    fn while_true_loop_runs_until_break() {
        let interpreter = run_program(
            "n = 0\nwhile True:\n    n += 1\n    if n == 5:\n        break\n",
        );
        assert_eq!(interpreter.lookup_name("n").unwrap(), Some(Value::Int(5)));
    }

    #[test]
    fn while_false_body_never_runs() {
        let interpreter = run_program("x = 0\nwhile False:\n    x = 99\n");
        assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::Int(0)));
    }

    #[test]
    fn while_false_else_branch_runs() {
        let interpreter = run_program("x = 0\nwhile False:\n    x = 99\nelse:\n    x = 42\n");
        assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::Int(42)));
    }

    #[test]
    fn enumerate_builtin_yields_index_value_pairs() {
        let interpreter =
            run_program("result = list(enumerate(['a', 'b', 'c']))\n");
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![
                Value::Tuple(vec![Value::Int(0), Value::Str("a".into())]),
                Value::Tuple(vec![Value::Int(1), Value::Str("b".into())]),
                Value::Tuple(vec![Value::Int(2), Value::Str("c".into())]),
            ]))
        );
    }

    #[test]
    fn enumerate_with_start_offset() {
        let interpreter = run_program(
            "result = list(enumerate(['x', 'y'], 10))\n",
        );
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![
                Value::Tuple(vec![Value::Int(10), Value::Str("x".into())]),
                Value::Tuple(vec![Value::Int(11), Value::Str("y".into())]),
            ]))
        );
    }

    #[test]
    fn zip_builtin_pairs_two_iterables() {
        let interpreter = run_program("result = list(zip([1, 2, 3], ['a', 'b', 'c']))\n");
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![
                Value::Tuple(vec![Value::Int(1), Value::Str("a".into())]),
                Value::Tuple(vec![Value::Int(2), Value::Str("b".into())]),
                Value::Tuple(vec![Value::Int(3), Value::Str("c".into())]),
            ]))
        );
    }

    #[test]
    fn zip_truncates_to_shortest() {
        let interpreter = run_program("result = list(zip([1, 2, 3], [10, 20]))\n");
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![
                Value::Tuple(vec![Value::Int(1), Value::Int(10)]),
                Value::Tuple(vec![Value::Int(2), Value::Int(20)]),
            ]))
        );
    }

    #[test]
    fn sorted_builtin_returns_sorted_list() {
        let interpreter = run_program("result = sorted([3, 1, 4, 1, 5, 9, 2, 6])\n");
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![
                Value::Int(1), Value::Int(1), Value::Int(2), Value::Int(3),
                Value::Int(4), Value::Int(5), Value::Int(6), Value::Int(9),
            ]))
        );
    }

    #[test]
    fn sorted_with_reverse_flag() {
        let interpreter = run_program("result = sorted([3, 1, 2], reverse=True)\n");
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![Value::Int(3), Value::Int(2), Value::Int(1)]))
        );
    }

    #[test]
    fn reversed_builtin_reverses_list() {
        let interpreter = run_program("result = list(reversed([1, 2, 3]))\n");
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::List(vec![Value::Int(3), Value::Int(2), Value::Int(1)]))
        );
    }

    #[test]
    fn abs_min_max_sum_work() {
        let interpreter = run_program(
            "a = abs(-7)\nb = min(3, 1, 4)\nc = max([5, 2, 8])\nd = sum([1, 2, 3, 4])\n",
        );
        assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::Int(7)));
        assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::Int(1)));
        assert_eq!(interpreter.lookup_name("c").unwrap(), Some(Value::Int(8)));
        assert_eq!(interpreter.lookup_name("d").unwrap(), Some(Value::Int(10)));
    }

    #[test]
    fn int_float_str_bool_conversions() {
        let interpreter = run_program(
            "a = int('42')\nb = float('3.14')\nc = str(100)\nd = bool(0)\ne = bool(1)\n",
        );
        assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::Int(42)));
        assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::Float(3.14)));
        assert_eq!(interpreter.lookup_name("c").unwrap(), Some(Value::Str("100".into())));
        assert_eq!(interpreter.lookup_name("d").unwrap(), Some(Value::Bool(false)));
        assert_eq!(interpreter.lookup_name("e").unwrap(), Some(Value::Bool(true)));
    }

    #[test]
    fn int_add_specializes_after_repeated_calls() {
        let interpreter = run_program(
            "total = 0\nfor i in range(20):\n    total = total + i\n",
        );
        assert_eq!(interpreter.lookup_name("total").unwrap(), Some(Value::Int(190)));
    }

    #[test]
    fn specialize_degrades_gracefully_on_type_change() {
        let interpreter = run_program(
            "def add(a, b):\n    return a + b\nx = add(1, 2)\ny = add('hello', ' world')\n",
        );
        assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::Int(3)));
        assert_eq!(
            interpreter.lookup_name("y").unwrap(),
            Some(Value::Str("hello world".to_string()))
        );
    }

    #[test]
    fn def_bound_params_read_without_unbound_error() {
        let interpreter = run_program(
            "def add(a, b):\n    return a + b\nresult = add(3, 4)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(7)));
    }

    #[test]
    fn unconditional_top_level_assign_is_def_bound() {
        let interpreter = run_program(
            "def f():\n    x = 10\n    return x + 1\nresult = f()\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(11)));
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
                matches!(interp.lookup_name("total").unwrap(), Some(Value::Int(70210)))
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
                matches!(interp.lookup_name("result").unwrap(), Some(Value::Int(3628800)))
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn aug_assign_fused_rmw_sum_loop() {
        let interpreter = run_program(
            "total = 0\nfor i in range(100):\n    total += i\n",
        );
        assert_eq!(interpreter.lookup_name("total").unwrap(), Some(Value::Int(4950)));
    }

    #[test]
    fn range_step1_sum_matches_formula() {
        // sum(range(1000)) = 999*1000/2 = 499500
        let interpreter = run_program(
            "s = 0\nfor i in range(1000):\n    s += i\n",
        );
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::Int(499500)));
    }

    #[test]
    fn range_step1_break_preserves_value() {
        let interpreter = run_program(
            "s = 0\nfor i in range(100):\n    if i == 10:\n        break\n    s += i\n",
        );
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::Int(45)));
    }

    #[test]
    fn boxed_return_signal_propagates_correctly() {
        let interpreter = run_program(
            "def f(n):\n    for i in range(n):\n        if i == 5:\n            return i * 2\n    return -1\nresult = f(10)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(10)));
    }

    #[test]
    fn nested_return_propagates_through_loops() {
        let interpreter = run_program(
            "def search(lst, target):\n    for i, x in enumerate(lst):\n        if x == target:\n            return i\n    return -1\nidx = search([10, 20, 30, 40], 30)\n",
        );
        assert_eq!(interpreter.lookup_name("idx").unwrap(), Some(Value::Int(2)));
    }

    #[test]
    fn while_compare_increment_detected_as_range() {
        // while i < n: ...; i += 1 should behave identically to for i in range(n)
        let interpreter = run_program(
            "i = 0\ns = 0\nwhile i < 10:\n    s += i\n    i += 1\n",
        );
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::Int(45)));
        assert_eq!(interpreter.lookup_name("i").unwrap(), Some(Value::Int(10)));
    }

    #[test]
    fn while_le_increment_detected() {
        let interpreter = run_program(
            "i = 1\nproduct = 1\nwhile i <= 5:\n    product *= i\n    i += 1\n",
        );
        assert_eq!(interpreter.lookup_name("product").unwrap(), Some(Value::Int(120)));
        assert_eq!(interpreter.lookup_name("i").unwrap(), Some(Value::Int(6)));
    }

    #[test]
    fn small_range_unroll_correct() {
        let interpreter = run_program(
            "s = 0\nfor i in range(4):\n    s += i\n",
        );
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::Int(6)));
    }

    #[test]
    fn while_with_continue_not_converted() {
        // continue in body means increment might be skipped — don't convert
        let interpreter = run_program(
            "i = 0\ns = 0\nwhile i < 10:\n    i += 1\n    if i % 2 == 0:\n        continue\n    s += i\n",
        );
        // Sum of odd numbers 1..9: 1+3+5+7+9 = 25
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::Int(25)));
    }

    #[test]
    fn for_loop_list_lazy_iter_sum() {
        let interpreter = run_program(
            "lst = [1, 2, 3, 4, 5]\ntotal = 0\nfor x in lst:\n    total += x\n",
        );
        assert_eq!(interpreter.lookup_name("total").unwrap(), Some(Value::Int(15)));
    }

    #[test]
    fn for_loop_tuple_unpack_sum() {
        let interpreter = run_program(
            "pairs = [(1, 10), (2, 20), (3, 30)]\ntotal = 0\nfor a, b in pairs:\n    total += a + b\n",
        );
        assert_eq!(interpreter.lookup_name("total").unwrap(), Some(Value::Int(66)));
    }

    #[test]
    fn for_loop_tuple_unpack_names() {
        let interpreter = run_program(
            "pairs = [('hello', 1), ('world', 2)]\nlast_k = ''\nfor k, v in pairs:\n    last_k = k\n",
        );
        // Just verify it runs without panic and assigned the last key
        assert_eq!(
            interpreter.lookup_name("last_k").unwrap(),
            Some(Value::Str("world".to_string()))
        );
    }
    #[test]
    fn licm_invariant_flag_loop() {
        // 'done' is never assigned in the body → condition is invariant
        // loop exits via break
        let interpreter = run_program(
            "done = False\ncount = 0\nwhile not done:\n    count += 1\n    if count >= 10:\n        break\n",
        );
        assert_eq!(interpreter.lookup_name("count").unwrap(), Some(Value::Int(10)));
    }

    #[test]
    fn licm_false_condition_skips_body() {
        let interpreter = run_program(
            "x = False\nran = False\nwhile x:\n    ran = True\n",
        );
        assert_eq!(interpreter.lookup_name("ran").unwrap(), Some(Value::Bool(false)));
    }

    #[test]
    fn licm_not_applied_when_condition_name_modified() {
        // 'i' IS modified in body → NOT invariant → normal while loop behavior
        let interpreter = run_program(
            "i = 0\ns = 0\nwhile i < 10:\n    s += i\n    i += 1\n",
        );
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::Int(45)));
        assert_eq!(interpreter.lookup_name("i").unwrap(), Some(Value::Int(10)));
    }

    // ── Register-VM specific tests ──────────────────────────────────────────

    #[test]
    fn vm_basic_arithmetic() {
        let interpreter = run_program(
            "def f(a, b): return a * b + 1\nresult = f(6, 7)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(43)));
    }

    #[test]
    fn vm_factorial_recursive() {
        let interpreter = run_program(
            "def fact(n):\n    if n <= 1: return 1\n    return n * fact(n - 1)\nresult = fact(10)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(3628800)));
    }

    #[test]
    fn vm_for_loop_sum() {
        let interpreter = run_program(
            "def s(n):\n    t = 0\n    for i in range(n):\n        t += i\n    return t\nresult = s(100)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(4950)));
    }

    #[test]
    fn vm_early_return_from_for_loop() {
        let interpreter = run_program(
            "def f(n, limit):\n    s = 0\n    for i in range(n):\n        s += i\n        if s > limit:\n            return s\n    return s\nresult = f(100, 50)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(55)));
    }

    #[test]
    fn vm_list_and_index() {
        let interpreter = run_program(
            "def f(lst): return lst[0] + lst[2]\nresult = f([10, 20, 30])\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(40)));
    }

    #[test]
    fn vm_tuple_unpack_in_for() {
        let interpreter = run_program(
            "def f(pairs):\n    t = 0\n    for a, b in pairs:\n        t += a + b\n    return t\nresult = f([(1, 2), (3, 4), (5, 6)])\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(21)));
    }

    #[test]
    fn vm_while_loop() {
        let interpreter = run_program(
            "def f(n):\n    i = 0\n    s = 0\n    while i < n:\n        s += i\n        i += 1\n    return s\nresult = f(10)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(45)));
    }

    #[test]
    fn vm_and_returns_operand_not_bool() {
        // Python `and` returns the actual operand, not a coerced bool.
        let interpreter = run_program(
            "def f(a, b): return a and b\nresult = f(1, 42)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(42)));
    }

    #[test]
    fn vm_or_returns_operand_not_bool() {
        let interpreter = run_program(
            "def f(a, b): return a or b\nresult = f(0, 99)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(99)));
    }

    #[test]
    fn vm_and_short_circuits_on_falsy_lhs() {
        let interpreter = run_program(
            "def f(a, b): return a and b\nresult = f(0, 42)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(0)));
    }

    #[test]
    fn vm_for_else_runs_when_not_broken() {
        let interpreter = run_program(
            "def f(lst):\n    for x in lst:\n        if x > 10: return x\n    else:\n        return -1\n    return 0\nresult = f([1, 2, 3])\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(-1)));
    }

    #[test]
    fn vm_for_else_skipped_on_break() {
        let interpreter = run_program(
            "def f(lst):\n    for x in lst:\n        if x > 1: break\n    else:\n        return -1\n    return x\nresult = f([1, 5, 3])\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(5)));
    }

    #[test]
    fn vm_while_else_runs_when_condition_false() {
        let interpreter = run_program(
            "def f(n):\n    i = 0\n    while i < n:\n        i += 1\n    else:\n        return i\n    return -1\nresult = f(3)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(3)));
    }

    #[test]
    fn vm_assert_falls_back_correctly() {
        // Functions containing `assert` must fall back to the tree-walker, which handles
        // AssertionError.  Verify the successful-assert path still returns the right value.
        let interpreter = run_program(
            "def f(x):\n    assert x > 0\n    return x * 2\nresult = f(5)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(10)));
    }

    #[test]
    fn variadic_args_packed_into_tuple() {
        let interpreter = run_program(
            "def f(*args): return args\nresult = f(1, 2, 3)\n",
        );
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::Tuple(vec![Value::Int(1), Value::Int(2), Value::Int(3)]))
        );
    }

    #[test]
    fn variadic_kwargs_packed_into_dict() {
        let interpreter = run_program(
            "def f(**kw): return kw['x']\nresult = f(x=42)\n",
        );
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::Int(42))
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
            Some(Value::Int(7))
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
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(10)));
    }
}
