//! End-to-end tests for `__del__` finalizers on the interpreter's exit paths
//! (#3341). Module globals live in reference cycles that are never dropped,
//! so instances still alive at shutdown are finalized by an explicit pass
//! that tears down `__main__`'s globals the way CPython does.
//!
//! The normal-exit order is covered by the parity fixture
//! `tests/cases/dunder/test_del_finalizer.py`; these tests cover the exits
//! the parity harness cannot express: `sys.exit`, an uncaught exception, and
//! the default `sys.unraisablehook` output.

use std::env;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const TRACKED: &str = "class Tracked:\n\
    \x20   def __init__(self, name):\n\
    \x20       self.name = name\n\
    \x20   def __del__(self):\n\
    \x20       print('del', self.name)\n";

/// Run `source` as a script file in a fresh directory that is removed again.
fn run_script(tag: &str, source: &str) -> Output {
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir: PathBuf = env::temp_dir().join(format!(
        "pyrust_del_at_exit_{tag}_{}_{serial}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let script = dir.join("main.py");
    std::fs::write(&script, source).expect("write script");
    let output = Command::new(env!("CARGO_BIN_EXE_pyrust"))
        .arg(&script)
        .output()
        .expect("run pyrust");
    let _ = std::fs::remove_dir_all(&dir);
    output
}

fn stdout_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

#[test]
fn globals_are_finalized_in_definition_order_on_normal_exit() {
    let output = run_script(
        "normal",
        &format!(
            "{TRACKED}\
             first = Tracked('first')\n\
             _private = Tracked('_private')\n\
             second = Tracked('second')\n\
             print('end')\n"
        ),
    );
    assert!(output.status.success(), "{}", stderr_str(&output));
    assert_eq!(
        stdout_str(&output),
        "end\ndel _private\ndel first\ndel second\n"
    );
}

#[test]
fn sys_exit_runs_finalizers_and_keeps_exit_code() {
    let output = run_script(
        "sys_exit",
        &format!(
            "import sys\n\
             {TRACKED}\
             leaked = Tracked('leaked')\n\
             def main():\n\
             \x20   local = Tracked('local')\n\
             \x20   sys.exit(3)\n\
             main()\n"
        ),
    );
    assert_eq!(output.status.code(), Some(3), "{}", stderr_str(&output));
    assert_eq!(stdout_str(&output), "del local\ndel leaked\n");
}

#[test]
fn uncaught_exception_still_runs_finalizers() {
    let output = run_script(
        "raise",
        &format!(
            "{TRACKED}\
             leaked = Tracked('leaked')\n\
             raise ValueError('boom')\n"
        ),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr_str(&output).contains("ValueError: boom"));
    assert_eq!(stdout_str(&output), "del leaked\n");
}

#[test]
fn atexit_callbacks_run_before_exit_finalizers() {
    let output = run_script(
        "atexit",
        &format!(
            "import atexit\n\
             {TRACKED}\
             leaked = Tracked('leaked')\n\
             atexit.register(lambda: print('atexit sees', leaked.name))\n"
        ),
    );
    assert!(output.status.success(), "{}", stderr_str(&output));
    assert_eq!(stdout_str(&output), "atexit sees leaked\ndel leaked\n");
}

#[test]
fn raising_finalizer_is_reported_and_does_not_fail_the_process() {
    let output = run_script(
        "unraisable",
        "class Raises:\n\
         \x20   def __del__(self):\n\
         \x20       raise ValueError('boom')\n\
         first = Raises()\n\
         del first\n\
         print('still running')\n\
         at_exit = Raises()\n",
    );
    assert!(output.status.success(), "{}", stderr_str(&output));
    assert_eq!(stdout_str(&output), "still running\n");
    let stderr = stderr_str(&output);
    assert_eq!(
        stderr
            .matches("Exception ignored in: <function Raises.__del__ at 0x")
            .count(),
        2,
        "{stderr}"
    );
    assert_eq!(stderr.matches("ValueError: boom").count(), 2, "{stderr}");
    assert!(
        stderr.contains("Traceback (most recent call last):"),
        "{stderr}"
    );
    assert!(stderr.contains("in __del__"), "{stderr}");
}

#[test]
fn finalizer_resurrecting_into_a_global_runs_once() {
    let output = run_script(
        "resurrect",
        "keep = []\n\
         class Resurrects:\n\
         \x20   def __del__(self):\n\
         \x20       print('del')\n\
         \x20       keep.append(self)\n\
         Resurrects()\n\
         print('kept', len(keep))\n",
    );
    assert!(output.status.success(), "{}", stderr_str(&output));
    assert_eq!(stdout_str(&output), "del\nkept 1\n");
}

#[test]
fn scripts_without_del_keep_their_globals_untouched_at_exit() {
    // No class defines `__del__`, so the shutdown pass must not rebind
    // globals: the atexit callback registered last still sees them.
    let output = run_script(
        "no_del",
        "import atexit\n\
         value = 'intact'\n\
         atexit.register(lambda: print(value))\n",
    );
    assert!(output.status.success(), "{}", stderr_str(&output));
    assert_eq!(stdout_str(&output), "intact\n");
}
