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
        // A tight loop hitting the same + site with Int operands should produce
        // correct results regardless of specialization state.
        let interpreter = run_program(
            "total = 0\nfor i in range(20):\n    total = total + i\n",
        );
        assert_eq!(interpreter.lookup_name("total").unwrap(), Some(Value::Int(190)));
    }

    #[test]
    fn specialize_degrades_gracefully_on_type_change() {
        // A site first observed as Int+Int, then called with Str+Str, must not
        // crash — it should fall back to the generic path and return the correct value.
        let interpreter = run_program(
            "def add(a, b):\n    return a + b\nx = add(1, 2)\ny = add('hello', ' world')\n",
        );
        assert_eq!(interpreter.lookup_name("x").unwrap(), Some(Value::Int(3)));
        assert_eq!(
            interpreter.lookup_name("y").unwrap(),
            Some(Value::Str("hello world".to_string()))
        );

    #[test]
    fn def_bound_params_read_without_unbound_error() {
        // Parameters are def-bound; reading them should never trigger the
        // "local variable not associated" error, even after deep inlining.
        let interpreter = run_program(
            "def add(a, b):\n    return a + b\nresult = add(3, 4)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(7)));
    }

    #[test]
    fn unconditional_top_level_assign_is_def_bound() {
        // A variable assigned at the very start of a function body (before any
        // branch) is def-bound; it must be readable on all paths.
        let interpreter = run_program(
            "def f():\n    x = 10\n    return x + 1\nresult = f()\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::Int(11)));
    }
}
