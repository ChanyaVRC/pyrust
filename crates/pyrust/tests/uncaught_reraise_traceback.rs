//! Issue #2404: the uncaught (top-level) stderr traceback formatter consumes
//! the exception's prepended `__traceback__` chain for a carried re-raise,
//! instead of rebuilding from the captured-frame snapshot (which #2367/#2403
//! reset at the re-raise site, so it diverged and dropped frames).
//!
//! This is NOT a parity fixture: the parity harness requires the reference
//! Python process to exit 0, but these scripts deliberately let an exception
//! escape to the top level (exit 1).  We instead run the pyrust binary on a
//! temp script and assert the emitted `File "...", line N, in NAME` frame list
//! matches the CPython 3.12 walk for each scenario (line + function name; the
//! file path is normalised away).

use std::env;
use std::io::Write;
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

/// Write `src` to a temp .py file, run pyrust on it, and return stderr.
fn run_pyrust_stderr(src: &str) -> String {
    let mut path = env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    path.push(format!("pyrust_unc_tb_{}_{}.py", std::process::id(), n));
    {
        let mut f = std::fs::File::create(&path).expect("create temp script");
        f.write_all(src.as_bytes()).expect("write temp script");
    }
    let output = Command::new(pyrust_bin())
        .arg(&path)
        .output()
        .expect("run pyrust");
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Extract the `(funcname, lineno)` of every `File "...", line N, in NAME` frame
/// line from a traceback, outermost-first (CPython's stderr order).
fn frame_list(stderr: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let t = line.trim_start();
        // `File "<path>", line N, in NAME`
        let Some(rest) = t.strip_prefix("File \"") else {
            continue;
        };
        let Some((_path, after)) = rest.split_once("\", line ") else {
            continue;
        };
        let Some((lineno, name)) = after.split_once(", in ") else {
            continue;
        };
        if let Ok(n) = lineno.trim().parse::<u32>() {
            out.push((name.trim().to_string(), n));
        }
    }
    out
}

#[test]
fn uncaught_carried_reraise_same_frame() {
    // `raise e` re-raising a caught exception, then uncaught.  CPython 3.12:
    // <module> 8 -> g 7 -> g 5 -> f 2.
    let src = "def f():\n    raise IndexError(\"idx\")\ndef g():\n    try:\n        f()\n    except IndexError as e:\n        raise e\ng()\n";
    let frames = frame_list(&run_pyrust_stderr(src));
    assert_eq!(
        frames,
        vec![
            ("<module>".to_string(), 8),
            ("g".to_string(), 7),
            ("g".to_string(), 5),
            ("f".to_string(), 2),
        ],
        "uncaught `raise e` must consume the prepended __traceback__ chain",
    );
}

#[test]
fn uncaught_bare_reraise() {
    // Bare `raise` re-raising, then uncaught.  CPython 3.12 adds NO node for the
    // bare-`raise` line: <module> 8 -> g 5 -> f 2.
    let src = "def f():\n    raise IndexError(\"idx\")\ndef g():\n    try:\n        f()\n    except IndexError:\n        raise\ng()\n";
    let frames = frame_list(&run_pyrust_stderr(src));
    assert_eq!(
        frames,
        vec![
            ("<module>".to_string(), 8),
            ("g".to_string(), 5),
            ("f".to_string(), 2),
        ],
        "uncaught bare `raise` must not add a node for the re-raising line",
    );
}

#[test]
fn uncaught_with_traceback_transplant() {
    // `raise e.with_traceback(e.__traceback__)`, then uncaught.  CPython 3.12:
    // <module> 8 -> g 7 -> g 5 -> f 2.
    let src = "def f():\n    raise IndexError(\"idx\")\ndef g():\n    try:\n        f()\n    except IndexError as e:\n        raise e.with_traceback(e.__traceback__)\ng()\n";
    let frames = frame_list(&run_pyrust_stderr(src));
    assert_eq!(
        frames,
        vec![
            ("<module>".to_string(), 8),
            ("g".to_string(), 7),
            ("g".to_string(), 5),
            ("f".to_string(), 2),
        ],
        "uncaught with_traceback transplant must consume the chain",
    );
}

#[test]
fn uncaught_carried_reraise_at_module_scope() {
    // A variable carried out of a frame and re-raised at module scope, then
    // uncaught.  CPython 3.12: <module> 9 -> g 5 -> f 2 (the carried chain with
    // the re-raise frame — module — prepended).
    let src = "def f():\n    raise IndexError(\"idx\")\ndef g():\n    try:\n        f()\n    except IndexError as e:\n        return e\ncarried = g()\nraise carried\n";
    let frames = frame_list(&run_pyrust_stderr(src));
    assert_eq!(
        frames,
        vec![
            ("<module>".to_string(), 9),
            ("g".to_string(), 5),
            ("f".to_string(), 2),
        ],
        "uncaught module-scope re-raise of a carried exception must keep the carried chain",
    );
}
