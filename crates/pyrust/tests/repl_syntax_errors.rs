//! REPL parser diagnostics must be normalized only after input is known to be
//! terminal. Regression coverage for issue #2855.

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

fn run_repl(input: &str) -> (String, String) {
    let mut child = Command::new(pyrust_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pyrust REPL");
    child
        .stdin
        .take()
        .expect("stdin piped")
        .write_all(input.as_bytes())
        .expect("write REPL input");
    let output = child.wait_with_output().expect("wait for pyrust REPL");
    assert!(output.status.success(), "REPL exited with {output:?}");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn assert_repeated_keyword_is_syntax_error(input: &str) {
    let (_, stderr) = run_repl(input);
    assert!(
        stderr.contains("SyntaxError: keyword argument repeated: a"),
        "got stderr:\n{stderr}"
    );
    assert!(!stderr.contains("Parse error:"), "got stderr:\n{stderr}");
}

#[test]
fn terminal_parse_error_is_normalized() {
    assert_repeated_keyword_is_syntax_error("f(a=1, a=2)\nexit\n");
}

#[test]
fn compound_statement_still_waits_for_blank_line() {
    let (stdout, stderr) = run_repl("if True:\n    print(\"continued\")\n\nexit\n");
    assert!(stdout.contains("continued"), "got stdout:\n{stdout}");
    assert!(stderr.is_empty(), "got stderr:\n{stderr}");
}

#[test]
fn runtime_execution_error_is_not_reclassified() {
    let (_, stderr) = run_repl("missing_name\nexit\n");
    assert!(
        stderr.contains("NameError: name 'missing_name' is not defined"),
        "got stderr:\n{stderr}"
    );
    assert!(!stderr.contains("SyntaxError:"), "got stderr:\n{stderr}");
}

#[test]
fn blank_line_flush_runtime_error_is_not_reclassified() {
    let (_, stderr) = run_repl("if True:\n    missing_name\n\nexit\n");
    assert!(
        stderr.contains("NameError: name 'missing_name' is not defined"),
        "got stderr:\n{stderr}"
    );
    assert!(!stderr.contains("SyntaxError:"), "got stderr:\n{stderr}");
}
