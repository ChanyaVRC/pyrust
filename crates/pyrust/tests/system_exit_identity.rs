//! Top-level `SystemExit` handling must follow the canonical built-in class,
//! not mutable or reusable Python-visible class names.

use std::env;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

fn run_script(source: &str) -> Output {
    let mut path = env::temp_dir();
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    path.push(format!(
        "pyrust_system_exit_identity_{}_{}.py",
        std::process::id(),
        serial
    ));
    std::fs::write(&path, source).expect("write temporary Python script");
    let output = Command::new(pyrust_bin())
        .arg(&path)
        .output()
        .expect("run pyrust");
    let _ = std::fs::remove_file(path);
    output
}

#[test]
fn unrelated_class_named_system_exit_is_not_process_control() {
    let output = run_script(
        "\
BuiltinException = Exception
class SystemExit(BuiltinException):
    pass
raise SystemExit(4)
",
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("SystemExit: 4"),
        "fake SystemExit should render as an uncaught ordinary exception:\n{stderr}"
    );
}

#[test]
fn renamed_real_system_exit_subclass_still_controls_process_exit() {
    let output = run_script(
        "\
BuiltinSystemExit = SystemExit
class ExitSignal(BuiltinSystemExit):
    pass
ExitSignal.__name__ = 'NotSystemExitByName'
raise ExitSignal(7)
",
    );

    assert_eq!(output.status.code(), Some(7));
    assert!(
        output.stderr.is_empty(),
        "real SystemExit subclass should not print a traceback:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
