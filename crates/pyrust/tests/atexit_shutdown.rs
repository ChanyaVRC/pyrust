//! End-to-end integration tests verifying that `atexit` LIFO callbacks run on
//! interpreter shutdown across `-c`, script execution, `sys.exit()`, uncaught
//! exceptions, and imported modules (issue #3209).

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

fn run_cli(cwd: Option<&Path>, args: &[&str]) -> Output {
    let mut cmd = Command::new(pyrust_bin());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args).output().expect("run pyrust")
}

fn stdout_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

#[test]
fn dash_c_runs_atexit_callbacks_in_lifo_order() {
    let output = run_cli(
        None,
        &[
            "-c",
            "import atexit\n\
             atexit.register(print, 'first', end=',')\n\
             atexit.register(print, 'second', end=',')\n\
             atexit.register(print, 'third')\n\
             print('main')",
        ],
    );
    assert!(
        output.status.success(),
        "pyrust -c failed:\n{}",
        stderr_str(&output)
    );
    assert_eq!(stdout_str(&output), "main\nthird\nsecond,first,");
}

#[test]
fn sys_exit_runs_atexit_callbacks_and_preserves_exit_code() {
    let output = run_cli(
        None,
        &[
            "-c",
            "import atexit, sys\n\
             atexit.register(print, 'exit-1')\n\
             atexit.register(print, 'exit-2')\n\
             sys.exit(7)",
        ],
    );
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(stdout_str(&output), "exit-2\nexit-1\n");
    assert!(
        stderr_str(&output).is_empty(),
        "unexpected stderr:\n{}",
        stderr_str(&output)
    );
}

#[test]
fn uncaught_exception_still_runs_atexit_callbacks() {
    let output = run_cli(
        None,
        &[
            "-c",
            "import atexit\n\
             atexit.register(print, 'cleanup-ran')\n\
             raise ValueError('kaboom')",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_str(&output), "cleanup-ran\n");
    assert!(
        stderr_str(&output).contains("ValueError: kaboom"),
        "expected traceback in stderr:\n{}",
        stderr_str(&output)
    );
}

#[test]
fn imported_module_atexit_callback_runs_on_main_shutdown() {
    let mut dir = env::temp_dir();
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.push(format!(
        "pyrust_atexit_mod_{}_{}",
        std::process::id(),
        serial
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    std::fs::write(
        dir.join("helper_atexit.py"),
        "import atexit\natexit.register(print, 'from-helper')\n",
    )
    .expect("write helper module");

    let output = run_cli(
        Some(&dir),
        &[
            "-c",
            "import atexit, helper_atexit\n\
             atexit.register(print, 'from-main')\n\
             print('body')",
        ],
    );
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "pyrust -c failed:\n{}",
        stderr_str(&output)
    );
    assert_eq!(stdout_str(&output), "body\nfrom-main\nfrom-helper\n");
}
