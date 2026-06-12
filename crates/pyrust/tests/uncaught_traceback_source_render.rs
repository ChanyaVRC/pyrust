//! Issues #2418 / #2411 / #2426: the uncaught (top-level) stderr traceback
//! formatter source-line + PEP 657 caret rendering must match CPython 3.12.
//!
//! #2418: the displayed source line is dedented to a fixed 4-space indent.
//!
//! #2411/#2426: CPython 3.12's PEP 657 underlines are fine-grained — they
//! underline the precise sub-expression that raised, and OMIT the caret row
//! when the anchor covers the whole stripped line (a bare `name`, `f()`,
//! `raise X(...)`, etc.).  Stage 1 of #2426 plumbs the column anchor for the
//! highest-value form: a bare-name `Var` load (the instruction that raises an
//! uncaught `NameError`).  For that form pyrust now renders the exact narrow
//! `^` caret, byte-for-byte with CPython 3.12 — whether the name appears as an
//! assignment RHS, a call argument, a binary-op operand, or a subscript
//! base/index.
//!
//! Forms NOT yet plumbed (binary-op `~^` context marks, subscript spans,
//! attribute access, function-frame anchors) carry NO column span and stay
//! caret-free — strictly safer than a wrong caret ("a wrong caret is worse
//! than no caret").  These tests assert:
//!   * plumbed `Var`-anchor frames are byte-exact with CPython, and
//!   * unplumbed forms never print a `^` underline they cannot place correctly.

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
    stderr.replace(&path.to_string_lossy().into_owned(), basename)
}

// ── #2418: source-line dedent (whole-line anchor → no caret) ────────────────

#[test]
fn indented_raise_source_line_is_dedented() {
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
fn module_scope_raise_has_no_caret_row() {
    let stderr = run_pyrust_stderr("simple.py", "raise ValueError(\"boom\")\n");
    let expected = "\
Traceback (most recent call last):
  File \"simple.py\", line 1, in <module>
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

// ── #2426 stage 1: bare-name Var anchor → exact narrow caret ────────────────

#[test]
fn nameerror_assignment_rhs_caret_is_byte_exact() {
    // `x = undefined`: CPython underlines only the RHS name.
    let stderr = run_pyrust_stderr("rhs.py", "x = some_undefined_name\n");
    let expected = "\
Traceback (most recent call last):
  File \"rhs.py\", line 1, in <module>
    x = some_undefined_name
        ^^^^^^^^^^^^^^^^^^^
NameError: name 'some_undefined_name' is not defined
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn nameerror_bare_name_whole_line_anchor_omits_caret() {
    // A bare `name` is a whole-line anchor → CPython omits the caret row.
    let stderr = run_pyrust_stderr("bare.py", "undefined_bare\n");
    let expected = "\
Traceback (most recent call last):
  File \"bare.py\", line 1, in <module>
    undefined_bare
NameError: name 'undefined_bare' is not defined
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn nameerror_call_argument_caret_is_byte_exact() {
    // `f(undef)`: CPython underlines the failing argument name.
    let stderr = run_pyrust_stderr("arg.py", "def f(a): pass\nf(undef_arg)\n");
    let expected = "\
Traceback (most recent call last):
  File \"arg.py\", line 2, in <module>
    f(undef_arg)
      ^^^^^^^^^
NameError: name 'undef_arg' is not defined
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn nameerror_first_operand_in_binop_caret_is_byte_exact() {
    // `a + b + undef`: the FIRST undefined name evaluated raises; CPython
    // underlines exactly it (a single-char `^` here).
    let stderr = run_pyrust_stderr("binop.py", "x = a_undef + b + c\n");
    let expected = "\
Traceback (most recent call last):
  File \"binop.py\", line 1, in <module>
    x = a_undef + b + c
        ^^^^^^^
NameError: name 'a_undef' is not defined
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn nameerror_indented_var_caret_rebases_to_dedented_line() {
    // The anchor's column is measured against the original line; the formatter
    // rebases it onto the dedented display line (the leading 4 spaces collapse).
    let stderr = run_pyrust_stderr("indent_var.py", "if True:\n    print(undefined_indented)\n");
    let expected = "\
Traceback (most recent call last):
  File \"indent_var.py\", line 2, in <module>
    print(undefined_indented)
          ^^^^^^^^^^^^^^^^^^
NameError: name 'undefined_indented' is not defined
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn nameerror_non_ascii_name_caret_uses_char_columns() {
    // The caret must align by char columns, not bytes, for a non-ASCII name.
    let stderr = run_pyrust_stderr("unicode.py", "y = café_undef\n");
    let expected = "\
Traceback (most recent call last):
  File \"unicode.py\", line 1, in <module>
    y = café_undef
        ^^^^^^^^^^
NameError: name 'café_undef' is not defined
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

// ── Unplumbed forms: caret-free (never a wrong caret) ───────────────────────

#[test]
fn unplumbed_forms_never_print_a_caret_underline() {
    // Binary-op `~^` context marks, subscript spans, and attribute access are
    // not yet plumbed; they must NOT print a caret pyrust cannot place exactly.
    let scripts = [
        "1 + \"s\"\n",            // binop TypeError
        "d = {}\nd[\"k\"]\n",     // subscript KeyError
        "(1).nonexistent_attr\n", // attribute AttributeError
        "[1, 2, 3][10]\n",        // subscript IndexError
    ];
    for (i, src) in scripts.iter().enumerate() {
        let stderr = run_pyrust_stderr("unplumbed.py", src);
        assert!(
            !stderr.contains('^'),
            "script {i} printed a caret pyrust cannot place exactly:\n{stderr}",
        );
    }
}

#[test]
fn nameerror_inside_function_frame_is_caret_free_for_now() {
    // Function-frame anchors are a stage-2 follow-up.  A NameError raised inside
    // a function must not print any (necessarily wrong) caret on either the
    // function frame or the module call-site frame in stage 1.
    let stderr = run_pyrust_stderr("func.py", "def g():\n    return undef_in_func\ng()\n");
    assert!(
        stderr.contains("  File \"func.py\", line 2, in g\n"),
        "function frame must appear in the traceback:\n{stderr}",
    );
    assert!(
        !stderr.contains('^'),
        "function-frame NameError must be caret-free in stage 1:\n{stderr}",
    );
}
