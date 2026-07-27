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
