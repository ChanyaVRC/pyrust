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

#[test]
fn length_result_normalization_uses_the_index_protocol() {
    let mut interp = run_program(
        r#"events = []
class IndexResult:
    def __init__(self, value, label):
        self.value = value
        self.label = label
    def __index__(self):
        events.append(self.label)
        return self.value
class BadIndexResult:
    def __index__(self):
        events.append('bad')
        return 1.5
class IntSubclass(int):
    def __index__(self):
        events.append('int-subclass-override')
        return 99
valid = IndexResult(4, 'valid')
negative = IndexResult(-(2**80), 'negative')
huge = IndexResult(2**63, 'huge')
bad = BadIndexResult()
int_subclass = IntSubclass(5)
nested_int_subclass = IndexResult(IntSubclass(6), 'nested-int-subclass')
"#,
    );

    let value = interp.lookup_name("valid").unwrap().unwrap();
    assert_eq!(interp.normalize_len_result(&value).unwrap(), 4);

    let value = interp.lookup_name("int_subclass").unwrap().unwrap();
    assert_eq!(interp.normalize_len_result(&value).unwrap(), 5);

    let value = interp.lookup_name("nested_int_subclass").unwrap().unwrap();
    assert_eq!(interp.normalize_len_result(&value).unwrap(), 6);

    let value = interp.lookup_name("negative").unwrap().unwrap();
    let error = interp.normalize_len_result(&value).unwrap_err();
    assert!(
        error.to_string().contains("__len__() should return >= 0"),
        "unexpected negative-length error: {error}"
    );

    let value = interp.lookup_name("huge").unwrap().unwrap();
    let error = interp.normalize_len_result(&value).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot fit 'int' into an index-sized integer"),
        "unexpected oversized-length error: {error}"
    );

    let value = interp.lookup_name("bad").unwrap().unwrap();
    let error = interp.normalize_len_result(&value).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("__index__ returned non-int (type float)"),
        "unexpected bad-index error: {error}"
    );

    assert_eq!(
        interp.lookup_name("events").unwrap(),
        Some(Value::list(vec![
            Value::string("valid"),
            Value::string("nested-int-subclass"),
            Value::string("negative"),
            Value::string("huge"),
            Value::string("bad"),
        ])),
        "an int subclass must use its integer backing without invoking an override",
    );
}

#[test]
fn optional_index_resolution_distinguishes_missing_from_protocol_errors() {
    let mut interp = run_program(
        r#"class MissingIndex:
    pass
class GoodIndex:
    def __index__(self):
        return 7
class BadIndex:
    def __index__(self):
        return 1.5
class RaisingIndex:
    def __index__(self):
        raise ValueError("index boom")
class IndexMeta(type):
    def __index__(cls):
        return 11
class IndexedClass(metaclass=IndexMeta):
    pass
class MissingIndexMeta(type):
    pass
class MissingIndexClass(metaclass=MissingIndexMeta):
    pass
class BadIndexMeta(type):
    def __index__(cls):
        return 1.5
class BadIndexClass(metaclass=BadIndexMeta):
    pass
class RaisingIndexMeta(type):
    def __index__(cls):
        raise ValueError("metaclass index boom")
class RaisingIndexClass(metaclass=RaisingIndexMeta):
    pass
missing_index = MissingIndex()
good_index = GoodIndex()
bad_index = BadIndex()
raising_index = RaisingIndex()
"#,
    );

    let value = interp.lookup_name("missing_index").unwrap().unwrap();
    assert_eq!(interp.try_value_to_index(&value).unwrap(), None);
    assert_eq!(
        interp
            .try_value_to_index(&Value::string("not-index"))
            .unwrap(),
        None
    );

    let value = interp.lookup_name("good_index").unwrap().unwrap();
    assert_eq!(
        interp.try_value_to_index(&value).unwrap(),
        Some(Value::int(7))
    );

    let value = interp.lookup_name("IndexedClass").unwrap().unwrap();
    assert_eq!(
        interp.try_value_to_index(&value).unwrap(),
        Some(Value::int(11))
    );

    let value = interp.lookup_name("MissingIndexClass").unwrap().unwrap();
    assert_eq!(interp.try_value_to_index(&value).unwrap(), None);

    let value = interp.lookup_name("BadIndexClass").unwrap().unwrap();
    let error = interp.try_value_to_index(&value).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("__index__ returned non-int (type float)"),
        "an invalid metaclass slot result must remain an error: {error}"
    );

    let value = interp.lookup_name("RaisingIndexClass").unwrap().unwrap();
    let error = interp.try_value_to_index(&value).unwrap_err();
    assert!(
        error.to_string().contains("metaclass index boom"),
        "a metaclass __index__ exception must propagate: {error}"
    );

    let value = interp.lookup_name("bad_index").unwrap().unwrap();
    let error = interp.try_value_to_index(&value).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("__index__ returned non-int (type float)"),
        "invalid slot result must remain an error: {error}"
    );

    let value = interp.lookup_name("raising_index").unwrap().unwrap();
    let error = interp.try_value_to_index(&value).unwrap_err();
    assert!(
        error.to_string().contains("index boom"),
        "an exception raised by __index__ must propagate: {error}"
    );
}

// Issue #272: `dir()` on a built-in type instance must surface every
// name in the corresponding `pyrust_builtins::*::METHODS` slice, since
// that slice is the single source of truth.  This locks in that the
// `builtin_method_names` table is derived from `METHODS` rather than
// duplicated.
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
    // CPython 3.12: chr() with a non-int / non-__index__ argument raises
    // `TypeError: 'X' object cannot be interpreted as an integer` (#1908).
    let err = run_program_expect_error("_x = chr('not an int')\n");
    let msg = err.to_string();
    assert!(
        msg.contains("'str' object cannot be interpreted as an integer"),
        "expected substituted type name, got: {msg}"
    );
    assert!(
        !msg.contains("'{}' object"),
        "literal `{{}}` placeholder still present in: {msg}"
    );
}

#[test]
fn bin_oct_hex_type_error_messages_substitute_type_name() {
    // Dead-result builtin calls remain runtime operations and must propagate
    // their type errors.
    for src in ["bin('s')\n", "oct(3.14)\n", "hex([])\n"] {
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
