#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    // `range_len` is no longer used in this module's non-test code (it
    // moved to `builtin_modules::builtins::len`); pull it in for the
    // legacy unit test below.
    use crate::value::range_len;

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
            Some(Value::int(2))
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
        let tokens = Lexer::new("def f():\n    break\n")
            .unwrap()
            .into_tokens();
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
        let src =
            "def outer():\n    x = 1\n    def inner():\n        nonlocal x\n        x: int = 5\n    inner()\nouter()\n";
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
        let tokens = Lexer::new("if False:\n    return\n")
            .unwrap()
            .into_tokens();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut interpreter = Interpreter::default();

        let error = interpreter.exec_program(&program, false).unwrap_err();

        assert_eq!(error.to_string(), "SyntaxError: 'return' outside function");
    }

    #[test]
    fn yield_in_dead_code_at_module_level_is_syntax_error() {
        let tokens = Lexer::new("if False:\n    yield\n")
            .unwrap()
            .into_tokens();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut interpreter = Interpreter::default();

        let error = interpreter.exec_program(&program, false).unwrap_err();

        assert_eq!(error.to_string(), "SyntaxError: 'yield' outside function");
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

    #[test]
    fn class_instances_bind_methods_and_init() {
        let interpreter = run_program(
            "class Counter:\n    def __init__(self, start):\n        self.value = start\n    def inc(self, step=1):\n        self.value = self.value + step\n        return self.value\nc = Counter(10)\nresult = [c.value, c.inc(), c.inc(4), c.value]\n",
        );

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::list(vec![
                Value::int(10),
                Value::int(11),
                Value::int(15),
                Value::int(15),
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
            Some(Value::list(vec![
                Value::string("base"),
                Value::int(10),
                Value::int(11),
                Value::string("base"),
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
            Some(Value::list(vec![
                Value::string("bad"),
                Value::string("finally"),
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
            Some(Value::string("inner"))
        );
    }

    #[test]
    fn zero_division_inside_try_is_catchable_as_zerodivisionerror() {
        // `1/0` raises `ZeroDivisionError` (CPython parity, #336).
        // Previously this raised the generic `RuntimeError`.
        let interpreter = run_program(
            "result = ''\ntry:\n    x = 1 / 0\nexcept ZeroDivisionError as err:\n    result = err.args[0]\n",
        );

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::string("division by zero"))
        );
    }

    #[test]
    fn import_math_module_provides_constants_and_functions() {
        let interpreter = run_program(
            "import math\npi_val = math.pi\nfloor_val = math.floor(2.9)\nsqrt_val = math.sqrt(16.0)\n",
        );

        assert_eq!(
            interpreter.lookup_name("pi_val").unwrap(),
            Some(Value::float(std::f64::consts::PI))
        );
        assert_eq!(
            interpreter.lookup_name("floor_val").unwrap(),
            Some(Value::int(2))
        );
        assert_eq!(
            interpreter.lookup_name("sqrt_val").unwrap(),
            Some(Value::float(4.0))
        );
    }

    #[test]
    fn from_math_import_binds_names_directly() {
        let interpreter =
            run_program("from math import pi, floor\npi_val = pi\nfloor_val = floor(3.7)\n");

        assert_eq!(
            interpreter.lookup_name("pi_val").unwrap(),
            Some(Value::float(std::f64::consts::PI))
        );
        assert_eq!(
            interpreter.lookup_name("floor_val").unwrap(),
            Some(Value::int(3))
        );
    }

    #[test]
    fn import_alias_binds_module_under_alias_name() {
        let interpreter = run_program("import math as m\nresult = m.floor(5.8)\n");

        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::int(5))
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
                interp.lookup_name("caught").unwrap() == Some(Value::bool_(true))
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
                interp.lookup_name("result").unwrap() == Some(Value::int(200))
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
                interp.lookup_name("result").unwrap() == Some(Value::int(9227465))
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
    fn while_true_loop_runs_until_break() {
        let interpreter = run_program(
            "n = 0\nwhile True:\n    n += 1\n    if n == 5:\n        break\n",
        );
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
        let interpreter =
            run_program("result = list(enumerate(['a', 'b', 'c']))\n");
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
        let interpreter = run_program(
            "result = list(enumerate(['x', 'y'], 10))\n",
        );
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::list(vec![
                Value::tuple(vec![Value::int(10), Value::string("x")]),
                Value::tuple(vec![Value::int(11), Value::string("y")]),
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
                Value::int(1), Value::int(1), Value::int(2), Value::int(3),
                Value::int(4), Value::int(5), Value::int(6), Value::int(9),
            ]))
        );
    }

    #[test]
    fn sorted_with_reverse_flag() {
        let interpreter = run_program("result = sorted([3, 1, 2], reverse=True)\n");
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::list(vec![Value::int(3), Value::int(2), Value::int(1)]))
        );
    }

    #[test]
    fn reversed_builtin_reverses_list() {
        let interpreter = run_program("result = list(reversed([1, 2, 3]))\n");
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::list(vec![Value::int(3), Value::int(2), Value::int(1)]))
        );
    }

    #[test]
    fn abs_min_max_sum_work() {
        let interpreter = run_program(
            "a = abs(-7)\nb = min(3, 1, 4)\nc = max([5, 2, 8])\nd = sum([1, 2, 3, 4])\n",
        );
        assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::int(7)));
        assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::int(1)));
        assert_eq!(interpreter.lookup_name("c").unwrap(), Some(Value::int(8)));
        assert_eq!(interpreter.lookup_name("d").unwrap(), Some(Value::int(10)));
    }

    #[test]
    fn int_float_str_bool_conversions() {
        let interpreter = run_program(
            "a = int('42')\nb = float('2.5')\nc = str(100)\nd = bool(0)\ne = bool(1)\n",
        );
        assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::int(42)));
        assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::float(2.5)));
        assert_eq!(interpreter.lookup_name("c").unwrap(), Some(Value::string("100")));
        assert_eq!(interpreter.lookup_name("d").unwrap(), Some(Value::bool_(false)));
        assert_eq!(interpreter.lookup_name("e").unwrap(), Some(Value::bool_(true)));
    }

    #[test]
    fn int_add_specializes_after_repeated_calls() {
        let interpreter = run_program(
            "total = 0\nfor i in range(20):\n    total = total + i\n",
        );
        assert_eq!(interpreter.lookup_name("total").unwrap(), Some(Value::int(190)));
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
        let interpreter = run_program(
            "def add(a, b):\n    return a + b\nresult = add(3, 4)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(7)));
    }

    #[test]
    fn unconditional_top_level_assign_is_def_bound() {
        let interpreter = run_program(
            "def f():\n    x = 10\n    return x + 1\nresult = f()\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(11)));
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
        let interpreter = run_program(
            "total = 0\nfor i in range(100):\n    total += i\n",
        );
        assert_eq!(interpreter.lookup_name("total").unwrap(), Some(Value::int(4950)));
    }

    #[test]
    fn range_step1_sum_matches_formula() {
        // sum(range(1000)) = 999*1000/2 = 499500
        let interpreter = run_program(
            "s = 0\nfor i in range(1000):\n    s += i\n",
        );
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::int(499500)));
    }

    #[test]
    fn range_step1_break_preserves_value() {
        let interpreter = run_program(
            "s = 0\nfor i in range(100):\n    if i == 10:\n        break\n    s += i\n",
        );
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::int(45)));
    }

    #[test]
    fn boxed_return_signal_propagates_correctly() {
        let interpreter = run_program(
            "def f(n):\n    for i in range(n):\n        if i == 5:\n            return i * 2\n    return -1\nresult = f(10)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(10)));
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
        let interpreter = run_program(
            "i = 0\ns = 0\nwhile i < 10:\n    s += i\n    i += 1\n",
        );
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::int(45)));
        assert_eq!(interpreter.lookup_name("i").unwrap(), Some(Value::int(10)));
    }

    #[test]
    fn while_le_increment_detected() {
        let interpreter = run_program(
            "i = 1\nproduct = 1\nwhile i <= 5:\n    product *= i\n    i += 1\n",
        );
        assert_eq!(interpreter.lookup_name("product").unwrap(), Some(Value::int(120)));
        assert_eq!(interpreter.lookup_name("i").unwrap(), Some(Value::int(6)));
    }

    #[test]
    fn small_range_unroll_correct() {
        let interpreter = run_program(
            "s = 0\nfor i in range(4):\n    s += i\n",
        );
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
        let interpreter = run_program(
            "lst = [1, 2, 3, 4, 5]\ntotal = 0\nfor x in lst:\n    total += x\n",
        );
        assert_eq!(interpreter.lookup_name("total").unwrap(), Some(Value::int(15)));
    }

    #[test]
    fn for_loop_tuple_unpack_sum() {
        let interpreter = run_program(
            "pairs = [(1, 10), (2, 20), (3, 30)]\ntotal = 0\nfor a, b in pairs:\n    total += a + b\n",
        );
        assert_eq!(interpreter.lookup_name("total").unwrap(), Some(Value::int(66)));
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
        assert_eq!(interpreter.lookup_name("count").unwrap(), Some(Value::int(10)));
    }

    #[test]
    fn licm_false_condition_skips_body() {
        let interpreter = run_program(
            "x = False\nran = False\nwhile x:\n    ran = True\n",
        );
        assert_eq!(interpreter.lookup_name("ran").unwrap(), Some(Value::bool_(false)));
    }

    #[test]
    fn licm_not_applied_when_condition_name_modified() {
        // 'i' IS modified in body → NOT invariant → normal while loop behavior
        let interpreter = run_program(
            "i = 0\ns = 0\nwhile i < 10:\n    s += i\n    i += 1\n",
        );
        assert_eq!(interpreter.lookup_name("s").unwrap(), Some(Value::int(45)));
        assert_eq!(interpreter.lookup_name("i").unwrap(), Some(Value::int(10)));
    }

    // ── Register-VM specific tests ──────────────────────────────────────────

    #[test]
    fn vm_basic_arithmetic() {
        let interpreter = run_program(
            "def f(a, b): return a * b + 1\nresult = f(6, 7)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(43)));
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
    fn vm_pure_fn_memoized_fib() {
        // fib(35) without memoization requires ~29M recursive calls and takes seconds.
        // With CallMemo, each unique n is computed once and cached — must finish fast.
        let ok: bool = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let interpreter = run_program(
                    "def fib(n):\n    if n <= 1: return n\n    return fib(n-1) + fib(n-2)\nresult = fib(35)\n",
                );
                interpreter.lookup_name("result").unwrap() == Some(Value::int(9227465))
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn self_recursive_pure_fn_uses_call_memo() {
        // Regression test for issue #52.  A pure function that calls itself
        // recursively must:
        //  (a) still be marked is_pure = true (fixpoint assumption holds), and
        //  (b) have its inner recursive calls compiled as CallMemo so that
        //      repeated calls with the same argument hit the fn_cache without
        //      re-entering call_function_expanded.
        //
        // Verify correctness for several known Fibonacci values to confirm
        // memoization is not returning stale/wrong cached results.
        let ok: bool = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
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
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(4950)));
    }

    #[test]
    fn vm_early_return_from_for_loop() {
        let interpreter = run_program(
            "def f(n, limit):\n    s = 0\n    for i in range(n):\n        s += i\n        if s > limit:\n            return s\n    return s\nresult = f(100, 50)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(55)));
    }

    #[test]
    fn vm_list_and_index() {
        let interpreter = run_program(
            "def f(lst): return lst[0] + lst[2]\nresult = f([10, 20, 30])\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(40)));
    }

    #[test]
    fn vm_tuple_unpack_in_for() {
        let interpreter = run_program(
            "def f(pairs):\n    t = 0\n    for a, b in pairs:\n        t += a + b\n    return t\nresult = f([(1, 2), (3, 4), (5, 6)])\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(21)));
    }

    #[test]
    fn vm_while_loop() {
        let interpreter = run_program(
            "def f(n):\n    i = 0\n    s = 0\n    while i < n:\n        s += i\n        i += 1\n    return s\nresult = f(10)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(45)));
    }

    #[test]
    fn vm_and_returns_operand_not_bool() {
        // Python `and` returns the actual operand, not a coerced bool.
        let interpreter = run_program(
            "def f(a, b): return a and b\nresult = f(1, 42)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(42)));
    }

    #[test]
    fn vm_or_returns_operand_not_bool() {
        let interpreter = run_program(
            "def f(a, b): return a or b\nresult = f(0, 99)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(99)));
    }

    #[test]
    fn vm_and_short_circuits_on_falsy_lhs() {
        let interpreter = run_program(
            "def f(a, b): return a and b\nresult = f(0, 42)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(0)));
    }

    #[test]
    fn vm_for_else_runs_when_not_broken() {
        let interpreter = run_program(
            "def f(lst):\n    for x in lst:\n        if x > 10: return x\n    else:\n        return -1\n    return 0\nresult = f([1, 2, 3])\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(-1)));
    }

    #[test]
    fn vm_for_else_skipped_on_break() {
        let interpreter = run_program(
            "def f(lst):\n    for x in lst:\n        if x > 1: break\n    else:\n        return -1\n    return x\nresult = f([1, 5, 3])\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(5)));
    }

    #[test]
    fn vm_while_else_runs_when_condition_false() {
        let interpreter = run_program(
            "def f(n):\n    i = 0\n    while i < n:\n        i += 1\n    else:\n        return i\n    return -1\nresult = f(3)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(3)));
    }

    #[test]
    fn vm_assert_falls_back_correctly() {
        // Functions containing `assert` must fall back to the tree-walker, which handles
        // AssertionError.  Verify the successful-assert path still returns the right value.
        let interpreter = run_program(
            "def f(x):\n    assert x > 0\n    return x * 2\nresult = f(5)\n",
        );
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(10)));
    }

    #[test]
    fn variadic_args_packed_into_tuple() {
        let interpreter = run_program(
            "def f(*args): return args\nresult = f(1, 2, 3)\n",
        );
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::tuple(vec![Value::int(1), Value::int(2), Value::int(3)]))
        );
    }

    #[test]
    fn variadic_kwargs_packed_into_dict() {
        let interpreter = run_program(
            "def f(**kw): return kw['x']\nresult = f(x=42)\n",
        );
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
        assert_eq!(interpreter.lookup_name("result").unwrap(), Some(Value::int(10)));
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

    #[test]
    fn call_depth_restored_after_runtime_error() {
        let tokens = Lexer::new("def f():\n    return undefined_var\ntry:\n    f()\nexcept:\n    pass\n")
            .unwrap()
            .into_tokens();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut interpreter = Interpreter::default();
        let depth_before = call_depth();
        interpreter.exec_program(&program, false).unwrap();
        assert_eq!(
            call_depth(), depth_before,
            "call_depth must be restored after a runtime error inside a function"
        );
    }

    #[test]
    fn del_list_front_middle_end_produces_correct_result() {
        let interpreter = run_program(
            "lst = [1, 2, 3, 4, 5]\ndel lst[2]\ndel lst[0]\ndel lst[-1]\nresult = lst\n",
        );
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::list(vec![Value::int(2), Value::int(4)]))
        );
    }

    #[test]
    fn dict_update_via_double_splat_call() {
        // DictUpdate instruction is emitted when calling f(**a, **b).
        // Verifies that the merged kwargs dict has the correct values.
        let interpreter = run_program(
            "def merge(**kw): return kw\na = {'x': 1, 'y': 2}\nb = {'y': 99, 'z': 3}\nresult = merge(**a, **b)\n",
        );
        use crate::value::PyKey;
        let mut expected = indexmap::IndexMap::new();
        expected.insert(PyKey::Str("x".to_string()), Value::int(1));
        expected.insert(PyKey::Str("y".to_string()), Value::int(99));
        expected.insert(PyKey::Str("z".to_string()), Value::int(3));
        assert_eq!(
            interpreter.lookup_name("result").unwrap(),
            Some(Value::dict(expected))
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
        assert_eq!(interpreter.lookup_name("a").unwrap(), Some(Value::float(4.0)));
        assert_eq!(interpreter.lookup_name("b").unwrap(), Some(Value::float(7.0)));
        assert_eq!(interpreter.lookup_name("c").unwrap(), Some(Value::float(8.0)));
        assert_eq!(interpreter.lookup_name("d").unwrap(), Some(Value::float(4.5)));
        assert_eq!(interpreter.lookup_name("e").unwrap(), Some(Value::string("hello")));
        assert_eq!(interpreter.lookup_name("f").unwrap(), Some(Value::int(256)));
        assert_eq!(interpreter.lookup_name("g").unwrap(), Some(Value::int(1)));
    }

    #[allow(dead_code)]
    fn run_program_result(src: &str) -> crate::error::Result<()> {
        let tokens = Lexer::new(src).unwrap().into_tokens();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut interpreter = Interpreter::default();
        interpreter.exec_program(&program, false)
    }

    #[test]
    fn math_floor_large_float_returns_bignum() {
        let interp = run_program("import math\nresult = math.floor(1e100) > 2**62\n");
        assert_eq!(
            interp.lookup_name("result").unwrap(),
            Some(Value::bool_(true))
        );
    }

    #[test]
    fn math_ceil_large_float_returns_bignum() {
        let interp = run_program("import math\nresult = math.ceil(1e100) > 2**62\n");
        assert_eq!(
            interp.lookup_name("result").unwrap(),
            Some(Value::bool_(true))
        );
    }

    #[test]
    fn value_is_eight_bytes() {
        // Issue #64: Value must be a NaN-boxed u64 (8 bytes), not a tagged enum.
        assert_eq!(std::mem::size_of::<Value>(), 8);
    }

    /// Issue #272: `dir()` on a built-in type instance must surface every
    /// name in the corresponding `pyrust_builtins::*::METHODS` slice, since
    /// that slice is the single source of truth.  This locks in that the
    /// `builtin_method_names` table is derived from `METHODS` rather than
    /// duplicated.
    // ── Copilot-review regression tests (PR #326) ────────────────────────────
    //
    // The migrated `bodies/builtins.rs` originally carried two latent bugs
    // forward from the legacy match arms in `calls.rs`:
    //
    //   - The TypeError message for `chr`/`bin`/`oct`/`hex` was built with
    //     `"…'{}' object…".to_string()` instead of `format!()`, so users saw
    //     the literal `{}` instead of the offending type name.
    //   - `bin(i64::MIN)` (and oct/hex) computed `-v` directly, which panics
    //     in debug and wraps in release for the boundary case.
    //
    // Both are now fixed; these tests pin the corrected behaviour so a
    // future refactor can't silently regress them.

    fn run_program_expect_error(src: &str) -> crate::error::PyError {
        let tokens = Lexer::new(src).unwrap().into_tokens();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut interpreter = Interpreter::default();
        interpreter.exec_program(&program, false).unwrap_err()
    }

    #[test]
    fn chr_type_error_message_substitutes_type_name() {
        let err = run_program_expect_error("chr('not an int')\n");
        let msg = err.to_string();
        assert!(
            msg.contains("got type str"),
            "expected substituted type name, got: {msg}"
        );
        assert!(
            !msg.contains("got type {}"),
            "literal `{{}}` placeholder still present in: {msg}"
        );
    }

    #[test]
    fn bin_oct_hex_type_error_messages_substitute_type_name() {
        for src in [
            "bin('s')\n",
            "oct(3.14)\n",
            "hex([])\n",
        ] {
            let err = run_program_expect_error(src);
            let msg = err.to_string();
            assert!(
                !msg.contains("'{}' object"),
                "literal `{{}}` placeholder still present in: {msg} (src: {src:?})"
            );
            assert!(
                msg.contains("object cannot be interpreted"),
                "expected canonical TypeError shape, got: {msg}"
            );
        }
    }

    #[test]
    fn bin_oct_hex_at_i64_min_do_not_overflow() {
        // `-9223372036854775807 - 1` is the only way to *write* i64::MIN
        // since the lexer parses leading-minus literals by negating an
        // unsigned magnitude — and 9223372036854775808 doesn't fit in i64.
        //
        // Before the fix, the `-v` inside the bin/oct/hex bodies would
        // panic in debug (`attempt to negate with overflow`) and wrap to
        // 0 in release (producing the wrong string).
        let interp = run_program(
            "n = -9223372036854775807 - 1\n\
             b = bin(n)\n\
             o = oct(n)\n\
             h = hex(n)\n",
        );
        // 0x8000_0000_0000_0000 as the unsigned magnitude:
        //   binary  = 1 followed by 63 zeros
        //   octal   = 1 followed by 21 zeros
        //   hex     = 8000000000000000
        assert_eq!(
            interp.lookup_name("b").unwrap(),
            Some(Value::string(format!("-0b1{}", "0".repeat(63))))
        );
        assert_eq!(
            interp.lookup_name("o").unwrap(),
            Some(Value::string(format!("-0o1{}", "0".repeat(21))))
        );
        assert_eq!(
            interp.lookup_name("h").unwrap(),
            Some(Value::string("-0x8000000000000000"))
        );
    }

    #[test]
    fn super_two_arg_form_works_after_py_name_migration() {
        // `super` migrated from `calls.rs` into `bodies/builtins.rs` via
        // `#[py_name = "super"]` because `super` is a strict Rust keyword
        // that can't be a raw ident.  This pins both classic two-arg uses.
        let interp = run_program(
            "class A:\n    def f(self): return 'A'\n\
             class B(A):\n    def f(self): return 'B+' + super(B, self).f()\n\
             instance_chain = B().f()\n\
             class C:\n    @classmethod\n    def cm(cls): return 'C'\n\
             class D(C):\n    @classmethod\n    def cm(cls): return 'D+' + super(D, cls).cm()\n\
             class_chain = D.cm()\n",
        );
        assert_eq!(
            interp.lookup_name("instance_chain").unwrap(),
            Some(Value::string("B+A"))
        );
        assert_eq!(
            interp.lookup_name("class_chain").unwrap(),
            Some(Value::string("D+C"))
        );
    }

    #[test]
    fn print_and_str_use_shared_dunder_render() {
        // `print(x)` and `str(x)` route through the same `render_instance_str`
        // helper, so they must produce identical text for instances that
        // define `__str__`, `__repr__`, both, or neither.  Capturing print's
        // output is awkward here, so we verify the `str()` path with each
        // priority tier and trust the shared call site.
        let interp = run_program(
            "class Neither: pass\n\
             class StrOnly:\n    def __str__(self): return 'S'\n\
             class ReprOnly:\n    def __repr__(self): return 'R'\n\
             class Both:\n    def __str__(self): return 'BS'\n    def __repr__(self): return 'BR'\n\
             a = str(Neither())\nb = str(StrOnly())\nc = str(ReprOnly())\nd = str(Both())\n",
        );
        // __str__ wins over __repr__; falls through to __repr__ when only it
        // exists; falls all the way to `<ClassName object>` when neither does.
        assert!(
            matches!(
                interp.lookup_name("a").unwrap(),
                Some(v) if matches!(v.kind(), ValueKind::Str(s) if s.contains("Neither object"))
            ),
            "Neither instance should render as `<Neither object>`"
        );
        assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::string("S")));
        assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::string("R")));
        assert_eq!(interp.lookup_name("d").unwrap(), Some(Value::string("BS")));
    }

    #[test]
    fn dir_covers_every_pyrust_builtins_methods_entry() {
        fn dir_names(interp: &Interpreter, name: &str) -> Vec<String> {
            let v = interp.lookup_name(name).unwrap().unwrap();
            match v.kind() {
                ValueKind::List(items) => items
                    .iter()
                    .map(|s| match s.kind() {
                        ValueKind::Str(rc) => rc.to_string(),
                        _ => panic!("dir() must return list of str"),
                    })
                    .collect(),
                _ => panic!("dir() must return a list"),
            }
        }

        let interp = run_program(
            "ds = dir(\"\")\n\
             dl = dir([])\n\
             dt = dir(())\n\
             dd = dir({})\n\
             dset = dir(set())\n",
        );

        let cases: &[(&str, &[&str])] = &[
            ("ds", pyrust_builtins::string::METHODS),
            ("dl", pyrust_builtins::list::METHODS),
            ("dt", pyrust_builtins::tuple::METHODS),
            ("dd", pyrust_builtins::dict::METHODS),
            ("dset", pyrust_builtins::set::METHODS),
        ];

        for (var, expected) in cases {
            let got = dir_names(&interp, var);
            for name in *expected {
                assert!(
                    got.iter().any(|g| g == name),
                    "dir({var}) missing {name:?}; got {got:?}"
                );
            }
        }
    }

    // ── stdlib phase-2 modules (issue #250) ──────────────────────────────────
    //
    // os.path / functools / itertools / collections live in
    // `crates/pyrust/src/builtin_modules/bodies/`.  The tests below pin one
    // representative behaviour per public surface — full method coverage
    // belongs in CPython-parity test suites once we wire one up.

    #[test]
    fn os_path_join_handles_absolute_components_like_cpython() {
        // CPython quirk: any absolute component resets the running path.
        // Expected output is platform-specific because `Path::is_absolute`
        // disagrees across OSes — on Unix `/abs` is absolute (so `b` and
        // `c` reset to `/abs/...`); on Windows `/abs` isn't absolute (no
        // drive prefix), so it's just another non-resetting component and
        // separators get mixed.  Mirror CPython by computing the expected
        // strings with the same `PathBuf` ops the impl uses — that keeps
        // the test honest on both platforms without skipping coverage.
        let interp = run_program(
            "import os.path as op\n\
             a = op.join('a', 'b', 'c')\n\
             b = op.join('/abs', 'rel')\n\
             c = op.join('rel', '/abs', 'tail')\n",
        );
        let expect = |parts: &[&str]| {
            let mut p = std::path::PathBuf::new();
            for part in parts {
                let q = std::path::Path::new(part);
                if q.is_absolute() {
                    p = q.to_path_buf();
                } else {
                    p.push(q);
                }
            }
            Value::string(p.to_string_lossy().into_owned())
        };
        assert_eq!(interp.lookup_name("a").unwrap(), Some(expect(&["a", "b", "c"])));
        assert_eq!(interp.lookup_name("b").unwrap(), Some(expect(&["/abs", "rel"])));
        assert_eq!(
            interp.lookup_name("c").unwrap(),
            Some(expect(&["rel", "/abs", "tail"]))
        );
    }

    #[test]
    fn os_path_splitext_treats_leading_dots_as_basename() {
        // `.bashrc` → ('.bashrc', '') — a leading dot is *not* an
        // extension separator (CPython rule).  Pinning this because it's
        // the easy-to-regress branch in `splitext`.
        let interp = run_program(
            "from os.path import splitext\n\
             a = splitext('foo.tar.gz')\n\
             b = splitext('.bashrc')\n\
             c = splitext('no_ext')\n",
        );
        assert_eq!(
            interp.lookup_name("a").unwrap(),
            Some(Value::tuple(vec![
                Value::string("foo.tar"),
                Value::string(".gz"),
            ]))
        );
        assert_eq!(
            interp.lookup_name("b").unwrap(),
            Some(Value::tuple(vec![
                Value::string(".bashrc"),
                Value::string(""),
            ]))
        );
        assert_eq!(
            interp.lookup_name("c").unwrap(),
            Some(Value::tuple(vec![Value::string("no_ext"), Value::string("")]))
        );
    }

    #[test]
    fn functools_reduce_with_and_without_initializer() {
        let interp = run_program(
            "from functools import reduce\n\
             a = reduce(lambda x, y: x + y, [1, 2, 3, 4])\n\
             b = reduce(lambda x, y: x + y, [1, 2, 3, 4], 100)\n\
             c = reduce(lambda x, y: x * y, [1, 2, 3, 4])\n\
             d = reduce(lambda x, y: x + y, [], 'seed')\n",
        );
        assert_eq!(interp.lookup_name("a").unwrap(), Some(Value::int(10)));
        assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::int(110)));
        assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::int(24)));
        assert_eq!(interp.lookup_name("d").unwrap(), Some(Value::string("seed")));
    }

    #[test]
    fn functools_reduce_empty_without_initializer_is_type_error() {
        let err = run_program_expect_error(
            "from functools import reduce\nreduce(lambda x, y: x + y, [])\n",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("of empty iterable with no initial value"),
            "expected canonical CPython error wording, got: {msg}"
        );
    }

    #[test]
    fn itertools_chain_concatenates_iterables() {
        let interp = run_program(
            "from itertools import chain\n\
             a = list(chain([1, 2], [3, 4], [5]))\n\
             b = list(chain([]))\n\
             c = list(chain())\n",
        );
        assert_eq!(
            interp.lookup_name("a").unwrap(),
            Some(Value::list((1..=5).map(Value::int).collect()))
        );
        assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::list(vec![])));
        assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::list(vec![])));
    }

    #[test]
    fn itertools_islice_covers_all_arities() {
        // `islice(seq, stop)`, `islice(seq, start, stop)`, and
        // `islice(seq, start, stop, step)` — plus `None` in each slot
        // (which means "default": 0 / drain / 1).
        let interp = run_program(
            "from itertools import islice\n\
             a = list(islice([0,1,2,3,4,5,6,7,8,9], 5))\n\
             b = list(islice([0,1,2,3,4,5,6,7,8,9], 2, 7))\n\
             c = list(islice([0,1,2,3,4,5,6,7,8,9], 0, 10, 2))\n\
             d = list(islice(range(5), None, 10))\n\
             e = list(islice(range(5), 1, None))\n",
        );
        assert_eq!(
            interp.lookup_name("a").unwrap(),
            Some(Value::list((0..5).map(Value::int).collect()))
        );
        assert_eq!(
            interp.lookup_name("b").unwrap(),
            Some(Value::list((2..7).map(Value::int).collect()))
        );
        assert_eq!(
            interp.lookup_name("c").unwrap(),
            Some(Value::list((0..10).step_by(2).map(Value::int).collect()))
        );
        assert_eq!(
            interp.lookup_name("d").unwrap(),
            Some(Value::list((0..5).map(Value::int).collect()))
        );
        assert_eq!(
            interp.lookup_name("e").unwrap(),
            Some(Value::list((1..5).map(Value::int).collect()))
        );
    }

    #[test]
    fn collections_counter_tallies_iterables() {
        // Counter is now a real Python class (defined via `pyrust_module!`'s
        // `class { … }` block).  Pin the counts via `c[key]` (which routes
        // through `__getitem__`) and `len(c)` rather than comparing to a
        // plain dict — Counter instances are PyInstances, not dicts.
        let interp = run_program(
            "from collections import Counter\n\
             a = Counter([1, 2, 1, 3, 2, 1])\n\
             a_one = a[1]\n\
             a_two = a[2]\n\
             a_three = a[3]\n\
             a_missing = a[99]\n\
             a_len = len(a)\n\
             b = Counter('aabcccd')\n\
             b_a = b['a']\n\
             b_c = b['c']\n\
             c = Counter()\n\
             c_len = len(c)\n\
             c_missing = c['anything']\n\
             d = Counter({'x': 5, 'y': 3})\n\
             d_x = d['x']\n\
             d_y = d['y']\n",
        );
        // Counter([1, 2, 1, 3, 2, 1])
        assert_eq!(interp.lookup_name("a_one").unwrap(), Some(Value::int(3)));
        assert_eq!(interp.lookup_name("a_two").unwrap(), Some(Value::int(2)));
        assert_eq!(interp.lookup_name("a_three").unwrap(), Some(Value::int(1)));
        // Missing-key returns 0 (the dict-subclass quirk).
        assert_eq!(interp.lookup_name("a_missing").unwrap(), Some(Value::int(0)));
        assert_eq!(interp.lookup_name("a_len").unwrap(), Some(Value::int(3)));
        // Counter('aabcccd')
        assert_eq!(interp.lookup_name("b_a").unwrap(), Some(Value::int(2)));
        assert_eq!(interp.lookup_name("b_c").unwrap(), Some(Value::int(3)));
        // Counter() — empty.
        assert_eq!(interp.lookup_name("c_len").unwrap(), Some(Value::int(0)));
        assert_eq!(interp.lookup_name("c_missing").unwrap(), Some(Value::int(0)));
        // Counter({'x': 5, 'y': 3}) — mapping form preserves the values.
        assert_eq!(interp.lookup_name("d_x").unwrap(), Some(Value::int(5)));
        assert_eq!(interp.lookup_name("d_y").unwrap(), Some(Value::int(3)));
    }

    #[test]
    fn os_path_dotted_import_works_via_parent_package() {
        // The `os` parent module is synthesised in `bodies/os.rs` and
        // its `path` constant points at the `os.path` module, so the
        // bare `import os.path; os.path.join(...)` pattern (which the
        // compiler binds under the topmost component `os`) resolves.
        let interp = run_program(
            "import os.path\nresult = os.path.join('a', 'b')\n",
        );
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            interp.lookup_name("result").unwrap(),
            Some(Value::string(format!("a{sep}b")))
        );
    }

    #[test]
    fn itertools_islice_is_lazy_over_huge_source() {
        // The point of moving islice off the eager `Vec<Value>` path is
        // that it must *not* drain a huge source when the consumer only
        // asks for a handful.  Pull three elements from a 100k-range and
        // confirm the rest never materialises by relying on the test
        // wall-clock; with the eager implementation, the same test ran
        // visibly slower because it had to walk the full range.
        let interp = run_program(
            "from itertools import islice\n\
             it = islice(range(100000), 3)\n\
             a = next(it); b = next(it); c = next(it)\n",
        );
        assert_eq!(interp.lookup_name("a").unwrap(), Some(Value::int(0)));
        assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::int(1)));
        assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::int(2)));
    }

    #[test]
    fn collections_counter_exposes_full_method_surface() {
        // Counter is now a real BuiltinObject — pin each of the methods
        // that lights up only with the BuiltinTypeOps implementation
        // (missing-key returns 0, most_common, elements, update,
        // subtract, copy independence).
        let interp = run_program(
            "from collections import Counter\n\
             c = Counter('aabbc')\n\
             missing = c['z']\n\
             top2 = c.most_common(2)\n\
             elts = c.elements()\n\
             c.update('aa')\n\
             after_update = c['a']\n\
             c.subtract('aaaaa')\n\
             after_subtract = c['a']\n\
             c2 = c.copy()\n\
             c2['a'] = 999\n\
             original_a = c['a']\n\
             copy_a = c2['a']\n",
        );
        assert_eq!(interp.lookup_name("missing").unwrap(), Some(Value::int(0)));
        assert_eq!(
            interp.lookup_name("top2").unwrap(),
            Some(Value::list(vec![
                Value::tuple(vec![Value::string("a"), Value::int(2)]),
                Value::tuple(vec![Value::string("b"), Value::int(2)]),
            ]))
        );
        // elements() lists 'a' twice, 'b' twice, 'c' once — insertion
        // order preserved.
        assert_eq!(
            interp.lookup_name("elts").unwrap(),
            Some(Value::list(vec![
                Value::string("a"), Value::string("a"),
                Value::string("b"), Value::string("b"),
                Value::string("c"),
            ]))
        );
        assert_eq!(interp.lookup_name("after_update").unwrap(), Some(Value::int(4)));
        assert_eq!(interp.lookup_name("after_subtract").unwrap(), Some(Value::int(-1)));
        assert_eq!(interp.lookup_name("original_a").unwrap(), Some(Value::int(-1)));
        assert_eq!(interp.lookup_name("copy_a").unwrap(), Some(Value::int(999)));
    }

    #[test]
    fn collections_defaultdict_runs_factory_on_missing_key() {
        // Two complementary uses pin the missing-key dispatch:
        //
        //   - `defaultdict(int)` for the canonical `counts[c] += 1` idiom
        //     — `+=` re-binds via `set_item` so the increment persists
        //     across iterations, matching CPython.
        //   - `defaultdict(None)` falls through to KeyError, matching a
        //     plain dict — there's no factory to call.
        let interp = run_program(
            "from collections import defaultdict\n\
             counts = defaultdict(int)\n\
             for c in 'aabbbc':\n    \
             counts[c] += 1\n\
             a = counts['a']\n\
             b = counts['b']\n\
             c = counts['c']\n",
        );
        assert_eq!(interp.lookup_name("a").unwrap(), Some(Value::int(2)));
        assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::int(3)));
        assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::int(1)));
    }

    #[test]
    fn collections_defaultdict_none_factory_raises_key_error() {
        // `defaultdict(None)` matches plain dict semantics: missing key
        // raises KeyError instead of running a factory.  The behaviour
        // is driven by `defaultdict.__missing__` checking
        // `self.default_factory is None` and short-circuiting to
        // KeyError when so — pin both halves of that branch.
        let err = run_program_expect_error(
            "from collections import defaultdict\nd = defaultdict(None)\nd['missing']\n",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("KeyError"),
            "expected KeyError, got: {msg}"
        );
    }

    #[test]
    fn collections_counter_iterates_keys_in_insertion_order() {
        // This pins the original bug that motivated migrating Counter to a
        // class-based implementation: the previous `BuiltinTypeOps` Counter
        // returned `None` from `iter_next`, so `for k in c` and `list(c)`
        // both silently yielded nothing.  With `__iter__` defined as a
        // dunder, iteration goes through pyrust's normal class machinery.
        let interp = run_program(
            "from collections import Counter\n\
             c = Counter('aab')\n\
             keys_list = list(c)\n\
             # Re-iteration must work too (each iter(c) takes a fresh snapshot).\n\
             keys_again = list(c)\n",
        );
        // Insertion order: 'a' (first seen), 'b' (second seen).
        assert_eq!(
            interp.lookup_name("keys_list").unwrap(),
            Some(Value::list(vec![Value::string("a"), Value::string("b")]))
        );
        assert_eq!(
            interp.lookup_name("keys_again").unwrap(),
            Some(Value::list(vec![Value::string("a"), Value::string("b")]))
        );
    }

    #[test]
    fn collections_counter_dunder_dispatch_exercises_each_site() {
        // The dispatch unification in `invoke_class_method` routes
        // `__contains__`, `__setitem__`, `__len__`, and `__getitem__`
        // through the same helper.  One end-to-end Python program
        // exercising each ensures the helper handles every dispatch
        // site (we'd otherwise only cover `__iter__` and
        // `__getitem__` via the existing tests).
        let interp = run_program(
            "from collections import Counter\n\
             c = Counter('aab')\n\
             a_present = 'a' in c\n\
             z_missing = 'z' in c\n\
             length = len(c)\n\
             before = c['a']\n\
             c['a'] = 99\n\
             after = c['a']\n\
             # __setitem__ propagation: a fresh `[]` lookup should see\n\
             # the new value (which proves set_item routed through the\n\
             # class dunder rather than landing on a clone).\n\
             after_again = c['a']\n",
        );
        assert_eq!(interp.lookup_name("a_present").unwrap(), Some(Value::bool_(true)));
        assert_eq!(interp.lookup_name("z_missing").unwrap(), Some(Value::bool_(false)));
        assert_eq!(interp.lookup_name("length").unwrap(), Some(Value::int(2)));
        assert_eq!(interp.lookup_name("before").unwrap(), Some(Value::int(2)));
        assert_eq!(interp.lookup_name("after").unwrap(), Some(Value::int(99)));
        assert_eq!(interp.lookup_name("after_again").unwrap(), Some(Value::int(99)));
    }

    #[test]
    fn collections_counter_corrupted_counts_surfaces_type_error() {
        // `c._counts = "lol"` overwrites the internal storage with a
        // non-dict.  The next `c[k]` access should surface a TypeError
        // pointing at the user's tampering — not a `Runtime("internal:
        // …")` that looks like an interpreter bug.
        let err = run_program_expect_error(
            "from collections import Counter\nc = Counter('a')\nc._counts = 'lol'\nc['a']\n",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("TypeError"),
            "expected TypeError diagnostic, got: {msg}"
        );
        assert!(
            msg.contains("_counts"),
            "error should name the offending attribute, got: {msg}"
        );
    }

    #[test]
    fn collections_counter_is_a_class_instance() {
        // After the migration to `pyrust_module!`'s `class { … }` block,
        // `Counter(...)` returns a real PyInstance whose class name is
        // exactly `"Counter"` (not `"collections.Counter"`).  This pins the
        // `class_name_lit` codepath in the macro's class emission.
        let interp = run_program(
            "from collections import Counter\n\
             c = Counter([1, 2])\n\
             tname = type(c).__name__\n",
        );
        assert_eq!(
            interp.lookup_name("tname").unwrap(),
            Some(Value::string("Counter"))
        );
    }

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
        let err = run_program_expect_error(
            "from collections import defaultdict\ndefaultdict(42)\n",
        );
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
}
