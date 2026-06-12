//! Issues #2418 / #2411: the uncaught (top-level) stderr traceback formatter
//! source-line + caret rendering must match CPython 3.12.
//!
//! #2418: the displayed source line is dedented to a fixed 4-space indent —
//! CPython strips the line's own leading whitespace, so an indented statement
//! is not over-indented.
//!
//! #2411 (partial): CPython 3.12's PEP 657 underlines are fine-grained — they
//! underline the precise sub-expression that raised, and they OMIT the caret
//! row entirely when the anchor covers the whole stripped line (a bare `name`,
//! `f()`, `raise X(...)`, etc.).  pyrust's line tables carry no column info, so
//! it cannot reproduce the narrow span; the achievable, strictly-correct
//! behaviour is to emit NO caret row at all.  These tests assert that:
//!   * whole-line-anchor frames (the common uncaught case) are byte-exact with
//!     CPython — including the absence of any `^^^` row, and
//!   * no traceback frame ever prints a `^` underline.
//!
//! Unlike `uncaught_reraise_traceback.rs` (which normalises file paths and
//! asserts only on the frame list), these assert on the FULL, un-normalised
//! stderr for scripts whose every displayed frame has a whole-line anchor — so
//! the byte-exact comparison is achievable today.

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

/// Write `src` to a temp .py file with a fixed basename, run pyrust on it, and
/// return stderr with the temp directory path normalised back to the basename
/// (so the `File "..."` header is stable across machines).
fn run_pyrust_stderr(basename: &str, src: &str) -> String {
    let mut dir = env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.push(format!("pyrust_src_render_{}_{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join(basename);
    {
        let mut f = std::fs::File::create(&path).expect("create temp script");
        f.write_all(src.as_bytes()).expect("write temp script");
    }
    let output = Command::new(pyrust_bin())
        .arg(&path)
        .output()
        .expect("run pyrust");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // Normalise the absolute temp path to just the basename.
    stderr.replace(&path.to_string_lossy().into_owned(), basename)
}

#[test]
fn indented_raise_source_line_is_dedented() {
    // #2418: the source line under an indented `raise` must be dedented to the
    // fixed 4-space traceback indent, NOT carry its own leading indentation on
    // top.  Whole-line anchor => no caret row (byte-exact with CPython 3.12).
    let stderr = run_pyrust_stderr(
        "indented.py",
        "if True:\n    raise ValueError(\"indented\")\n",
    );
    let expected = "\
Traceback (most recent call last):
  File \"indented.py\", line 2, in <module>
    raise ValueError(\"indented\")
ValueError: indented
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn deeply_indented_raise_source_line_is_dedented() {
    // Nested indentation (12 spaces) collapses to the fixed 4-space indent.
    let stderr = run_pyrust_stderr(
        "nested.py",
        "if True:\n    if True:\n        if True:\n            raise RuntimeError(\"deep\")\n",
    );
    let expected = "\
Traceback (most recent call last):
  File \"nested.py\", line 4, in <module>
    raise RuntimeError(\"deep\")
RuntimeError: deep
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn module_scope_raise_has_no_caret_row() {
    // #2411: a top-level `raise X(...)` has a whole-line anchor — CPython 3.12
    // emits NO caret row.  pyrust must match (byte-exact, no `^^^`).
    let stderr = run_pyrust_stderr("simple.py", "raise ValueError(\"boom\")\n");
    let expected = "\
Traceback (most recent call last):
  File \"simple.py\", line 1, in <module>
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn no_frame_ever_prints_a_caret_underline() {
    // #2411: across a multi-frame traceback with various statement shapes
    // (call, indented raise, assignment), no frame may emit a `^` underline,
    // since pyrust has no column data to place one correctly.
    let scripts = [
        "raise ValueError(\"x\")\n",
        "if True:\n    raise ValueError(\"y\")\n",
        "def f():\n    raise ValueError(\"z\")\nf()\n",
        "x = some_undefined_name\n",
        "d = {}\nd[\"k\"]\n",
    ];
    for (i, src) in scripts.iter().enumerate() {
        let stderr = run_pyrust_stderr("probe.py", src);
        assert!(
            !stderr.contains('^'),
            "script {i} produced a caret underline, which pyrust cannot place \
             correctly without column data:\n{stderr}",
        );
    }
}

#[test]
fn module_frame_caret_omitted_matches_cpython_byte_for_byte() {
    // The simplest deep-call case where every DISPLAYED frame's source line is
    // present and has a whole-line anchor: the module call site `f()`.  CPython
    // omits its caret; pyrust must too.
    let stderr = run_pyrust_stderr("call.py", "def f():\n    raise ValueError(\"v\")\nf()\n");
    // The module frame's `f()` line must appear with no caret beneath it.
    assert!(
        stderr.contains("  File \"call.py\", line 3, in <module>\n    f()\n"),
        "module call-site source line must be present and caret-free:\n{stderr}",
    );
    assert!(!stderr.contains('^'), "no caret anywhere:\n{stderr}");
}
