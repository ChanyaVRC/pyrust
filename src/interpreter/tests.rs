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
}
