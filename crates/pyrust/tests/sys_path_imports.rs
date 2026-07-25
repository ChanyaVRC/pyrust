//! `sys.path` import-resolution regression test.

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
        path.push(format!("pyrust_sys_path_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&path).expect("create temporary test directory");
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn path(&self) -> &Path {
        &self.0
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

#[test]
fn imports_honor_directories_added_to_sys_path() {
    let dir = TempDir::new();
    let module_dir = dir.join("extra_modules");
    std::fs::create_dir(&module_dir).expect("create module directory");
    std::fs::write(
        module_dir.join("added_via_sys_path.py"),
        "VALUE = 'loaded'\n",
    )
    .expect("write importable module");

    let script = dir.join("main.py");
    std::fs::write(
        &script,
        format!(
            "import sys\nprint(sys.path[0] == '{}')\nsys.path.append('{}')\nimport added_via_sys_path\nprint(added_via_sys_path.VALUE)\n",
            python_string(dir.path()),
            python_string(&module_dir),
        ),
    )
    .expect("write test script");

    let output = Command::new(pyrust_bin())
        .arg(&script)
        .output()
        .expect("run pyrust");
    assert!(
        output.status.success(),
        "script failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "True\nloaded\n");
}
