//! Issues #2418 / #2411 / #2426: the uncaught (top-level) stderr traceback
//! formatter source-line + PEP 657 caret rendering must match CPython 3.12.
//!
//! #2418: the displayed source line is dedented to a fixed 4-space indent.
//!
//! #2411/#2426: CPython 3.12's PEP 657 underlines are fine-grained — they
//! underline the precise sub-expression that raised, and OMIT the caret row
//! when the anchor covers the whole stripped line (a bare `name`, `f()`,
//! `raise X(...)`, etc.).  pyrust plumbs the column anchor for the high-value
//! forms: bare-name `Var` loads (uncaught `NameError`), calls, binary ops
//! (`~` operands + `^` operator), and subscripts (`~` object + `^` `[...]`) —
//! on *every* frame, including re-raised and chained exceptions (#2411).  For
//! these forms pyrust renders the exact caret byte-for-byte with CPython 3.12.
//!
//! Forms still NOT plumbed (attribute access, comparison/short-circuit
//! operators, const-folded nested binary ops) carry NO column span and stay
//! caret-free — strictly safer than a wrong caret ("a wrong caret is worse
//! than no caret").  These tests assert:
//!   * plumbed-anchor frames are byte-exact with CPython, and
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

// ── #2411: binary-op `~^` context marks and subscript spans ─────────────────

#[test]
fn binop_typeerror_caret_is_byte_exact() {
    // `1 + "s"`: CPython underlines the operands with `~` and the operator `^`.
    let stderr = run_pyrust_stderr("binop_t.py", "1 + \"s\"\n");
    let expected = "\
Traceback (most recent call last):
  File \"binop_t.py\", line 1, in <module>
    1 + \"s\"
    ~~^~~~~
TypeError: unsupported operand type(s) for +: 'int' and 'str'
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn binop_parenthesized_right_operand_caret_is_byte_exact() {
    // Issue #2580: `"s" + (a + b)` with `a`/`b` const-known folds the inner
    // `a + b` into a constant, leaving only the outer `"s" + 3` as a fused
    // `BinOpConst`.  The fold collapses one of the two original `BinOp`s, which
    // used to disable the monotone caret recovery and drop the outer op's caret
    // entirely.  CPython 3.12 underlines the whole expression `~` with `^` on the
    // operator; the surviving fused op must recover that span by register match.
    let stderr = run_pyrust_stderr("paren_rhs.py", "a = 1\nb = 2\n\"s\" + (a + b)\n");
    let expected = "\
Traceback (most recent call last):
  File \"paren_rhs.py\", line 3, in <module>
    \"s\" + (a + b)
    ~~~~^~~~~~~~~
TypeError: can only concatenate str (not \"int\") to str
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn binop_parenthesized_left_operand_caret_is_byte_exact() {
    // Issue #2580, mirror case: the folded inner binop is the *left* operand.
    // `(a + b) + "s"` folds `a + b` away; the outer op reuses the same
    // destination temp as the inner, so a `(dst, lhs)`-register match (not `dst`
    // alone) is needed to pin the surviving fused op to the correct origin span.
    let stderr = run_pyrust_stderr("paren_lhs.py", "a = 1\nb = 2\n(a + b) + \"s\"\n");
    let expected = "\
Traceback (most recent call last):
  File \"paren_lhs.py\", line 3, in <module>
    (a + b) + \"s\"
    ~~~~~~~~^~~~~
TypeError: unsupported operand type(s) for +: 'int' and 'str'
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn subscript_keyerror_caret_is_byte_exact() {
    // `d["k"]`: the object gets `~`, the `[...]` subscript gets `^`.
    let stderr = run_pyrust_stderr("subkey.py", "d = {}\nd[\"k\"]\n");
    let expected = "\
Traceback (most recent call last):
  File \"subkey.py\", line 2, in <module>
    d[\"k\"]
    ~^^^^^
KeyError: 'k'
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn subscript_indexerror_caret_is_byte_exact() {
    // `[1, 2, 3][10]`: the list literal gets `~`, the `[10]` subscript gets `^`.
    let stderr = run_pyrust_stderr("subidx.py", "[1, 2, 3][10]\n");
    let expected = "\
Traceback (most recent call last):
  File \"subidx.py\", line 1, in <module>
    [1, 2, 3][10]
    ~~~~~~~~~^^^^
IndexError: list index out of range
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn chained_subscript_keyerror_caret_is_byte_exact() {
    // #2570: `d['a']['b']['c']` with a missing `'c'` underlines only the
    // failing third subscript (`^`), the rest of the chain with `~`.  The inner
    // `GetItem`s collapse to identical register operands after copy-prop, so the
    // optimizer's per-opcode "ambiguous" col guard used to drop every caret past
    // the first — leaving the chain caret-free.
    let stderr = run_pyrust_stderr("chainkey.py", "d = {'a': {'b': {}}}\nd['a']['b']['c']\n");
    let expected = "\
Traceback (most recent call last):
  File \"chainkey.py\", line 2, in <module>
    d['a']['b']['c']
    ~~~~~~~~~~~^^^^^
KeyError: 'c'
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn chained_subscript_indexerror_caret_is_byte_exact() {
    // #2570: `a[0][1][2]` — `a[0][1]` indexes the single-element `[0]` out of
    // range, so the *second* subscript fails; CPython underlines `a[0]` with `~`
    // and the failing `[1]` with `^`.
    let stderr = run_pyrust_stderr("chainidx.py", "a = [[0], [1]]\na[0][1][2]\n");
    let expected = "\
Traceback (most recent call last):
  File \"chainidx.py\", line 2, in <module>
    a[0][1][2]
    ~~~~^^^
IndexError: list index out of range
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn attribute_access_remains_caret_free() {
    // Attribute access carries no column span (CPython 3.12 also omits the caret
    // for a bare `obj.attr` whose anchor covers the whole stripped line).
    let stderr = run_pyrust_stderr("attr.py", "(1).nonexistent_attr\n");
    let expected = "\
Traceback (most recent call last):
  File \"attr.py\", line 1, in <module>
    (1).nonexistent_attr
AttributeError: 'int' object has no attribute 'nonexistent_attr'
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn nameerror_inside_function_frame_carries_its_caret() {
    // #2411: a NameError raised inside a function now underlines the offending
    // name on the *function* frame (byte-exact), while the module call-site
    // frame (`g()`, a whole-line anchor) stays caret-free.
    let stderr = run_pyrust_stderr("func.py", "def g():\n    return undef_in_func\ng()\n");
    let expected = "\
Traceback (most recent call last):
  File \"func.py\", line 3, in <module>
    g()
  File \"func.py\", line 2, in g
    return undef_in_func
           ^^^^^^^^^^^^^
NameError: name 'undef_in_func' is not defined
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

// ── #2443 stage 2: narrow caret on an *outer* function frame's call site ─────

#[test]
fn outer_function_frame_call_site_carries_its_caret() {
    // #2443: the call site that propagated the error on an outer trampolined
    // frame (`1 + inner()`, where CPython underlines just `inner()` with `^^^^^^^`)
    // now draws its caret, not just the innermost frame.  Stage 1 left every
    // non-innermost function frame caret-free.
    let src = "def inner():\n    raise ValueError(\"boom\")\n\ndef outer():\n    x = 1 + inner()\n    return x\n\nouter()\n";
    let stderr = run_pyrust_stderr("outer.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"outer.py\", line 8, in <module>
    outer()
  File \"outer.py\", line 5, in outer
    x = 1 + inner()
            ^^^^^^^
  File \"outer.py\", line 2, in inner
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn return_call_frame_carries_its_caret() {
    // #2443: an outer frame whose entire body is `return f()` must retain the
    // call instruction's PEP 657 anchor.  CPython underlines just `inner()`.
    let src = "def inner():\n    raise ValueError(\"boom\")\n\ndef outer():\n    return inner()\n\nouter()\n";
    let stderr = run_pyrust_stderr("tc.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"tc.py\", line 7, in <module>
    outer()
  File \"tc.py\", line 5, in outer
    return inner()
           ^^^^^^^
  File \"tc.py\", line 2, in inner
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn method_call_frame_carries_its_caret() {
    // #2443: a method call `obj.m(...)` compiles to `CallMethod`, not `Call`, so
    // the simple-positional caret arming missed it entirely (stage 1 left every
    // `CallMethod`/`CallKw` site caret-free).  CPython underlines `c.m()`.
    let src = "class C:\n    def m(self):\n        raise KeyError(\"k\")\n\nc = C()\n\ndef f():\n    x = c.m()\n    return x\n\nf()\n";
    let stderr = run_pyrust_stderr("method.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"method.py\", line 11, in <module>
    f()
  File \"method.py\", line 8, in f
    x = c.m()
        ^^^^^
  File \"method.py\", line 3, in m
    raise KeyError(\"k\")
KeyError: 'k'
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn keyword_call_frame_carries_its_caret() {
    // #2443: a keyword call `g(a=5)` compiles to `CallKw`; arm + remap-preserve
    // its caret too.  CPython underlines `g(a=5)`.
    let src = "def g(a=0):\n    raise TypeError(\"t\")\n\ndef f():\n    x = g(a=5)\n    return x\n\nf()\n";
    let stderr = run_pyrust_stderr("kw.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"kw.py\", line 8, in <module>
    f()
  File \"kw.py\", line 5, in f
    x = g(a=5)
        ^^^^^^
  File \"kw.py\", line 2, in g
    raise TypeError(\"t\")
TypeError: t
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn three_deep_frames_each_carry_their_own_caret() {
    // #2443: with three trampolined frames the middle and outer frames each draw
    // the narrow caret of their own call sub-expression (CPython underlines just
    // the `b()` / `a()` call, not the surrounding binop / subscript), while the
    // innermost `raise` is a whole-line anchor and stays caret-free.
    let src = "def a():\n    raise ValueError(\"deep\")\n\ndef b():\n    return [a()][0]\n\ndef c():\n    return 10 * b()\n\nc()\n";
    let stderr = run_pyrust_stderr("deep.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"deep.py\", line 10, in <module>
    c()
  File \"deep.py\", line 8, in c
    return 10 * b()
                ^^^
  File \"deep.py\", line 5, in b
    return [a()][0]
            ^^^
  File \"deep.py\", line 2, in a
    raise ValueError(\"deep\")
ValueError: deep
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn constfold_left_sibling_caret_is_byte_exact() {
    // #2577: `a + b` (both known ints) const-folds, leaving the raising `+ "s"`
    // as a fused `BinOpConst`.  Its caret must survive the fold and underline the
    // second `+`, byte-for-byte with CPython 3.12.
    let src = "a = 1\nb = 2\nr = a + b + \"s\"\n";
    let stderr = run_pyrust_stderr("cf_left.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"cf_left.py\", line 3, in <module>
    r = a + b + \"s\"
        ~~~~~~^~~~~
TypeError: unsupported operand type(s) for +: 'int' and 'str'
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn constfold_right_sibling_caret_is_byte_exact() {
    // #2578: `x + 2` (paren'd, x known) folds; the outer `"s" + (...)` survives
    // and keeps its caret under the outer `+`.
    let src = "x = 1\nz = \"s\" + (x + 2)\n";
    let stderr = run_pyrust_stderr("cf_right.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"cf_right.py\", line 2, in <module>
    z = \"s\" + (x + 2)
        ~~~~^~~~~~~~~
TypeError: can only concatenate str (not \"int\") to str
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn constfold_sibling_subtree_fold_underlines_raising_op() {
    // `(a+b) + "s" + (c+d)` with `a`/`b`/`c`/`d` bound to ints: the variable
    // sub-adds `a+b` and `c+d` do NOT const-fold, so all four `+` binops survive.
    // The raising op is the middle `(a+b) + "s"`, whose caret must underline its
    // own span (`(a+b)` left operand, `^` on its `+`), exactly as CPython 3.12
    // renders it (verified with python3.12).  The multi-fold left-spine recovery
    // (#2586) anchors this correctly rather than leaving it caret-free.
    let src = "a = 1\nb = 2\nc = 3\nd = 4\nr = (a+b) + \"s\" + (c+d)\n";
    let stderr = run_pyrust_stderr("cf_sibling.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"cf_sibling.py\", line 5, in <module>
    r = (a+b) + \"s\" + (c+d)
        ~~~~~~^~~~~
TypeError: unsupported operand type(s) for +: 'int' and 'str'
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}
