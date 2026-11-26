//! End-to-end tests that buffered writes on files still open at interpreter
//! shutdown reach the disk, as CPython's finalization guarantees (#3318).
//! Module globals live in reference cycles that are never dropped, so these
//! files are only flushed by the explicit shutdown pass.

use std::env;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh directory that is removed when the test finishes.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!(
            "pyrust_flush_at_exit_{tag}_{}_{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `source` as a script file whose `sys.argv[1]` is `dir/out.txt`.
fn run_script(dir: &TempDir, source: &str) -> Output {
    let script = dir.path("main.py");
    std::fs::write(&script, source).expect("write script");
    Command::new(env!("CARGO_BIN_EXE_pyrust"))
        .arg(&script)
        .arg(dir.path("out.txt"))
        .output()
        .expect("run pyrust")
}

fn stderr_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

#[test]
fn leaked_global_and_attribute_files_are_flushed_on_normal_exit() {
    let dir = TempDir::new("normal");
    let output = run_script(
        &dir,
        "import sys\n\
         f = open(sys.argv[1], 'w')\n\
         f.write('global')\n\
         def keep():\n\
         \x20   keep.g = open(sys.argv[1] + '.bin', 'wb')\n\
         \x20   keep.g.write(b'attribute')\n\
         keep()\n",
    );
    assert!(output.status.success(), "{}", stderr_str(&output));
    assert_eq!(dir.read("out.txt"), "global");
    assert_eq!(dir.read("out.txt.bin"), "attribute");
}

#[test]
fn sys_exit_flushes_leaked_files_and_keeps_exit_code() {
    let dir = TempDir::new("sys_exit");
    let output = run_script(
        &dir,
        "import sys\n\
         f = open(sys.argv[1], 'w')\n\
         f.write('sys-exit')\n\
         sys.exit(3)\n",
    );
    assert_eq!(output.status.code(), Some(3), "{}", stderr_str(&output));
    assert_eq!(dir.read("out.txt"), "sys-exit");
}

#[test]
fn uncaught_exception_flushes_leaked_files() {
    let dir = TempDir::new("raise");
    let output = run_script(
        &dir,
        "import sys\n\
         f = open(sys.argv[1], 'w')\n\
         f.write('raised')\n\
         raise ValueError('boom')\n",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr_str(&output).contains("ValueError: boom"));
    assert_eq!(dir.read("out.txt"), "raised");
}

#[test]
fn uncaught_exception_with_atexit_flushes_leaked_files() {
    let dir = TempDir::new("raise_atexit");
    let output = run_script(
        &dir,
        "import atexit, sys\n\
         atexit.register(lambda: None)\n\
         f = open(sys.argv[1], 'w')\n\
         f.write('raised-atexit')\n\
         raise ValueError('boom')\n",
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(dir.read("out.txt"), "raised-atexit");
}

#[test]
fn writes_from_atexit_callbacks_are_flushed() {
    // CPython runs atexit callbacks before finalizing open files.
    let dir = TempDir::new("atexit_write");
    let output = run_script(
        &dir,
        "import atexit, sys\n\
         f = open(sys.argv[1], 'w')\n\
         f.write('body,')\n\
         atexit.register(lambda: f.write('atexit'))\n",
    );
    assert!(output.status.success(), "{}", stderr_str(&output));
    assert_eq!(dir.read("out.txt"), "body,atexit");
}

#[test]
fn files_leaked_by_worker_threads_are_flushed() {
    let dir = TempDir::new("thread");
    let output = run_script(
        &dir,
        "import sys, threading\n\
         def work():\n\
         \x20   work.f = open(sys.argv[1], 'w')\n\
         \x20   work.f.write('thread')\n\
         t = threading.Thread(target=work)\n\
         t.start()\n\
         t.join()\n",
    );
    assert!(output.status.success(), "{}", stderr_str(&output));
    assert_eq!(dir.read("out.txt"), "thread");
}

#[test]
fn dash_c_flushes_leaked_files() {
    let dir = TempDir::new("dash_c");
    let output = Command::new(env!("CARGO_BIN_EXE_pyrust"))
        .args([
            "-c",
            "import sys\nf = open(sys.argv[1], 'w')\nf.write('dash-c')\n",
        ])
        .arg(dir.path("out.txt"))
        .output()
        .expect("run pyrust");
    assert!(output.status.success(), "{}", stderr_str(&output));
    assert_eq!(dir.read("out.txt"), "dash-c");
}
