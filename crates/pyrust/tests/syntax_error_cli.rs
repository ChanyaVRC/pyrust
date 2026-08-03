//! Direct-file parser failures must be rendered as Python `SyntaxError`s.
//! Regression coverage for issue #2855.

use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn assert_cli_syntax_error(source: &str, expected: &str) {
    let mut path = env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    path.push(format!(
        "pyrust_syntax_error_{}_{}.py",
        std::process::id(),
        n
    ));
    std::fs::write(&path, source).expect("write temporary script");

    let output = Command::new(pyrust_bin())
        .arg(&path)
        .output()
        .expect("run pyrust");
    let _ = std::fs::remove_file(&path);

    assert!(!output.status.success(), "invalid source unexpectedly ran");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("SyntaxError: {expected}\n")
    );
}

#[test]
fn repeated_keyword_is_a_syntax_error() {
    assert_cli_syntax_error("f(a=1, a=2)\n", "keyword argument repeated: a");
}

#[test]
fn duplicate_parameter_is_a_syntax_error() {
    assert_cli_syntax_error(
        "def g(x, x): pass\n",
        "duplicate argument 'x' in function definition",
    );
}

#[test]
fn unclosed_parenthesis_uses_cpython_message() {
    assert_cli_syntax_error("x = (1 +\n", "'(' was never closed");
}

#[test]
fn unclosed_parenthesis_after_complete_expression_uses_cpython_message() {
    assert_cli_syntax_error("x = (1\n", "'(' was never closed");
}

#[test]
fn unclosed_bracket_uses_cpython_message() {
    assert_cli_syntax_error("x = [1 +\n", "'[' was never closed");
}

#[test]
fn unclosed_brace_uses_cpython_message() {
    assert_cli_syntax_error("x = {1 +\n", "'{' was never closed");
}

#[test]
fn assignment_to_literal_uses_cpython_message() {
    assert_cli_syntax_error(
        "1 = 2\n",
        "cannot assign to literal here. Maybe you meant '==' instead of '='?",
    );
}

#[test]
fn invalid_starred_expression_uses_cpython_message() {
    assert_cli_syntax_error("x = *a\n", "can't use starred expression here");
}

#[test]
fn compiler_syntax_error_remains_a_syntax_error() {
    assert_cli_syntax_error("return 5\n", "'return' outside function");
}
