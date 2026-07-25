//! Script-runtime metadata regression tests.
//!
//! These exercise the public binary because `sys.argv` and `__file__` are
//! populated from command-line/script context rather than from the parser or
//! VM alone.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let mut path = env::temp_dir();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!(
            "pyrust_script_metadata_{}_{}",
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&path).expect("create temporary test directory");
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn python_string(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
}

fn run_pyrust(script: &Path, args: &[&str]) -> std::process::Output {
    Command::new(pyrust_bin())
        .arg(script)
        .args(args)
        .output()
        .expect("run pyrust")
}

#[test]
fn script_metadata_reflects_the_invocation() {
    let dir = TempDir::new();
    let script = dir.join("main.py");
    std::fs::write(
        &script,
        "import sys\n\
         print(sys.argv[0] == __file__)\n\
         print(sys.argv[1:] == ['first', 'second'])\n\
         print(__file__.endswith('main.py'))\n",
    )
    .expect("write test script");

    let output = run_pyrust(&script, &["first", "second"]);
    assert!(
        output.status.success(),
        "script failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "True\nTrue\nTrue\n"
    );
}

#[test]
fn imported_modules_inherit_the_main_script_argv() {
    let dir = TempDir::new();
    let script = dir.join("main.py");
    std::fs::write(
        dir.join("context_helper.py"),
        format!(
            "import sys\nprint(sys.argv == ['{}', 'from-main'])\n",
            python_string(&script)
        ),
    )
    .expect("write imported module");
    std::fs::write(&script, "import context_helper\n").expect("write test script");

    let output = run_pyrust(&script, &["from-main"]);
    assert!(
        output.status.success(),
        "script failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "True\n");
}
