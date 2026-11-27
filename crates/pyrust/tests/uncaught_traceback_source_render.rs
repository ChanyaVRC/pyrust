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

// ── #2904: generator frames carry the anchor of their raising instruction ───
//
// A generator frame's `FrameInfo` used to be recorded with `col_span: None` at
// both recording sites (the `for`-loop gen-drive trampoline in
// `vm::entry::vm_unwind_error` and the plain `next()` / `.send()` / `.throw()`
// resume in `vm::entry::resume_generator_with_exc`), so *every* generator frame
// rendered caret-free regardless of what the body raised.  Both sites now
// publish the same anchor a plain function frame does — the col span of the
// instruction that propagated the error inside the body.  Every expectation
// below was captured from `python3.12`.

#[test]
fn yield_binop_caret_is_byte_exact() {
    // `yield 1/0`: CPython underlines the `1/0` BinOp (`~^~`), NOT the whole
    // `yield ...` line.
    let src = "def g():\n    yield 1/0\nfor _ in g(): pass\n";
    let stderr = run_pyrust_stderr("yield_div.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"yield_div.py\", line 3, in <module>
    for _ in g(): pass
  File \"yield_div.py\", line 2, in g
    yield 1/0
          ~^~
ZeroDivisionError: division by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn yield_call_caret_is_byte_exact() {
    // `yield f()` with a raising `f`: the generator frame underlines `f()`.
    let src =
        "def f():\n    raise ValueError(\"boom\")\ndef g():\n    yield f()\nfor _ in g(): pass\n";
    let stderr = run_pyrust_stderr("yield_call.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"yield_call.py\", line 5, in <module>
    for _ in g(): pass
  File \"yield_call.py\", line 4, in g
    yield f()
          ^^^
  File \"yield_call.py\", line 2, in f
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn assigned_yield_call_caret_is_byte_exact() {
    // `x = yield f()`: the anchor is still just the `f()` call, offset by the
    // assignment prefix.
    let src = "def f():\n    raise ValueError(\"boom\")\ndef g():\n    x = yield f()\nfor _ in g(): pass\n";
    let stderr = run_pyrust_stderr("yield_assign_call.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"yield_assign_call.py\", line 5, in <module>
    for _ in g(): pass
  File \"yield_assign_call.py\", line 4, in g
    x = yield f()
              ^^^
  File \"yield_assign_call.py\", line 2, in f
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn yield_from_call_caret_is_byte_exact() {
    // `yield from f()` where `f` raises *at the call* (before any delegation):
    // the raising instruction is the `f()` Call, so CPython underlines `f()`.
    let src = "def f():\n    raise ValueError(\"boom\")\ndef g():\n    yield from f()\nfor _ in g(): pass\n";
    let stderr = run_pyrust_stderr("yield_from_call.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"yield_from_call.py\", line 5, in <module>
    for _ in g(): pass
  File \"yield_from_call.py\", line 4, in g
    yield from f()
               ^^^
  File \"yield_from_call.py\", line 2, in f
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn yield_binop_call_caret_is_byte_exact() {
    // `yield 1 + f()`: the Call raises before the BinOp runs, so the anchor is
    // the call's, not the operator's.
    let src = "def f():\n    raise ValueError(\"boom\")\ndef g():\n    yield 1 + f()\nfor _ in g(): pass\n";
    let stderr = run_pyrust_stderr("yield_binop_call.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"yield_binop_call.py\", line 5, in <module>
    for _ in g(): pass
  File \"yield_binop_call.py\", line 4, in g
    yield 1 + f()
              ^^^
  File \"yield_binop_call.py\", line 2, in f
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn generator_statement_before_yield_caret_is_byte_exact() {
    // The defect was never specific to the `yield` operand — a plain statement
    // in a generator body was caret-free too.
    let src = "def g():\n    x = 1/0\n    yield x\nfor _ in g(): pass\n";
    let stderr = run_pyrust_stderr("gen_stmt_before_yield.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"gen_stmt_before_yield.py\", line 4, in <module>
    for _ in g(): pass
  File \"gen_stmt_before_yield.py\", line 2, in g
    x = 1/0
        ~^~
ZeroDivisionError: division by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn next_resumed_generator_caret_is_byte_exact() {
    // `next(g())` resumes through `resume_generator_with_exc`, not the `for`
    // gen-drive trampoline — the second recording site.
    let src = "def g():\n    yield 1/0\nnext(g())\n";
    let stderr = run_pyrust_stderr("next_yield_div.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"next_yield_div.py\", line 3, in <module>
    next(g())
  File \"next_yield_div.py\", line 2, in g
    yield 1/0
          ~^~
ZeroDivisionError: division by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn send_resumed_generator_caret_is_byte_exact() {
    // `.send()` takes the same non-trampolined resume path as `next()`.
    let src = "def f():\n    raise ValueError(\"boom\")\ndef g():\n    yield f()\ng().send(None)\n";
    let stderr = run_pyrust_stderr("send_yield_call.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"send_yield_call.py\", line 5, in <module>
    g().send(None)
  File \"send_yield_call.py\", line 4, in g
    yield f()
          ^^^
  File \"send_yield_call.py\", line 2, in f
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn delegating_generator_frames_each_carry_their_own_caret() {
    // Two stacked generator frames: the inner one underlines its `f()` call; the
    // delegating `yield from inner()` is a whole-line anchor, which CPython
    // renders caret-free (pyrust must not invent one for it either).
    let src = "def f():\n    raise ValueError(\"boom\")\ndef inner():\n    yield f()\ndef outer():\n    yield from inner()\nfor _ in outer(): pass\n";
    let stderr = run_pyrust_stderr("nested_gen.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"nested_gen.py\", line 7, in <module>
    for _ in outer(): pass
  File \"nested_gen.py\", line 6, in outer
    yield from inner()
  File \"nested_gen.py\", line 4, in inner
    yield f()
          ^^^
  File \"nested_gen.py\", line 2, in f
    raise ValueError(\"boom\")
ValueError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn generator_over_deep_call_chain_carets_are_byte_exact() {
    // The generator frame's anchor is the call site carried forward by the
    // trampoline unwind loop, so it must not be confused with the trampolined
    // callee frames' own anchors below it.
    let src = "def a():\n    raise ValueError(\"deep\")\ndef b():\n    return 10 * a()\ndef g():\n    yield b()\nfor _ in g(): pass\n";
    let stderr = run_pyrust_stderr("gen_deep_call.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"gen_deep_call.py\", line 7, in <module>
    for _ in g(): pass
  File \"gen_deep_call.py\", line 6, in g
    yield b()
          ^^^
  File \"gen_deep_call.py\", line 4, in b
    return 10 * a()
                ^^^
  File \"gen_deep_call.py\", line 2, in a
    raise ValueError(\"deep\")
ValueError: deep
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn thrown_exception_at_yield_does_not_inherit_a_stale_caret() {
    // `gen.throw()` injects at the suspended `yield`, which carries no anchor.
    // The body already raised-and-caught a `1/0` earlier, so the published span
    // is stale; the injection escape path must clear it rather than let the
    // generator frame paint `~^~` onto `yield 1`.  CPython prints no caret row.
    let src = "def g():\n    try:\n        1/0\n    except ZeroDivisionError:\n        pass\n    yield 1\ngen = g()\nnext(gen)\ngen.throw(ValueError(\"thrown\"))\n";
    let stderr = run_pyrust_stderr("throw_after_caught.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"throw_after_caught.py\", line 9, in <module>
    gen.throw(ValueError(\"thrown\"))
  File \"throw_after_caught.py\", line 6, in g
    yield 1
ValueError: thrown
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn caught_generator_error_does_not_paint_the_consumer_frame() {
    // The mirror image of the `throw()` stale-span hazard above, on the *catch*
    // side: the consumer catches the generator's error at its `ForIter` and
    // re-raises it.  The frame the re-raise reports for the consumer is built
    // from the span published when the handler was entered, so the gen-drive
    // unwind must re-publish the `ForIter` anchor (it has none) before
    // dispatching — otherwise the generator body's `1/0` span is still standing
    // and gets painted onto the consumer's `for _ in g():` line.  CPython leaves
    // that frame caret-free and underlines only the generator's `1/0`.
    let src = "def g():\n    yield 1/0\ntry:\n    for _ in g():\n        pass\nexcept ZeroDivisionError:\n    raise\n";
    let stderr = run_pyrust_stderr("caught_gen_reraise.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"caught_gen_reraise.py\", line 4, in <module>
    for _ in g():
  File \"caught_gen_reraise.py\", line 2, in g
    yield 1/0
          ~^~
ZeroDivisionError: division by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn chained_generator_error_does_not_paint_the_consumer_frame() {
    // Same hazard reached through `raise ... from e`: the __cause__ chain's
    // consumer frame must stay caret-free too.
    let src = "def g():\n    yield 1/0\ntry:\n    for _ in g():\n        pass\nexcept ZeroDivisionError as e:\n    raise RuntimeError(\"wrapped\") from e\n";
    let stderr = run_pyrust_stderr("chained_gen_cause.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"chained_gen_cause.py\", line 4, in <module>
    for _ in g():
  File \"chained_gen_cause.py\", line 2, in g
    yield 1/0
          ~^~
ZeroDivisionError: division by zero

The above exception was the direct cause of the following exception:

Traceback (most recent call last):
  File \"chained_gen_cause.py\", line 7, in <module>
    raise RuntimeError(\"wrapped\") from e
RuntimeError: wrapped
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

#[test]
fn loop_constfold_sibling_with_fused_binop_caret_is_byte_exact() {
    // Issue #2905: a constant-folded sibling (`x = 1 + 2`) inside a loop drops
    // the global binop count below the pre-optimization count, disabling the
    // whole-function 1:1 binop recovery.  The interval-bounded `(dst, lhs, op,
    // const_idx)` pin must still recover both the line and caret of `b // 0`.
    let src = "\
def f(b):
    i = 0
    while i < 3:
        x = 1 + 2
        y = b // 0
        i += 1
f(10)
";
    let stderr = run_pyrust_stderr("loop_cf_fused.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"loop_cf_fused.py\", line 7, in <module>
    f(10)
  File \"loop_cf_fused.py\", line 5, in f
    y = b // 0
        ~~^^~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn loop_copyprop_retargeted_fused_binop_caret_is_byte_exact() {
    // Issue #2905: `pass_copy_prop` retargets the fused op's destination onto
    // `res` while a sibling constant fold disables the global 1:1 count guard.
    let src = "\
def f(b):
    i = 0
    while i < 3:
        t = (2 + 3) * 4
        res = (b + 1) // 0
        i += 1
f(10)
";
    let stderr = run_pyrust_stderr("loop_copyprop_fused.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"loop_copyprop_fused.py\", line 7, in <module>
    f(10)
  File \"loop_copyprop_fused.py\", line 5, in f
    res = (b + 1) // 0
          ~~~~~~~~^^~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn loop_earlier_same_op_before_folded_rhs_div_zero_keeps_line_and_caret() {
    // Issue #2905: `acc = b // 2` fuses into `BinOpConst(..., 2)` while the
    // later `b // (2 - 2)` survives as a plain `BinOp` on the same scratch
    // registers after `2 - 2` folds to `LoadConst(t, 0)`.  LCS must not
    // rank-match the surviving `BinOp` onto the earlier fused `b // 2`.
    let src = "\
def f(b):
    acc = 0
    for i in range(5):
        acc = b // 2
        if i == 2:
            b // (2 - 2)
        b * 2
f(10)
";
    let stderr = run_pyrust_stderr("loop_folded_rhs_div0.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"loop_folded_rhs_div0.py\", line 8, in <module>
    f(10)
  File \"loop_folded_rhs_div0.py\", line 6, in f
    b // (2 - 2)
    ~~^^~~~~~~~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn loop_later_constfold_same_op_does_not_steal_earlier_fused_caret() {
    // Issue #2905: when `res = s + 1` is the sole surviving binop and a later
    // statement `c3 = c1 + c2` folds away, suffix recovery must not hijack
    // `res = s + 1` onto line 7 (`c1 + c2`).
    let src = "\
def f(s):
    i = 0
    acc = 0
    while i < 4:
        res = s + 1
        t = 3 * 4
        c1 = 10; c2 = 20; c3 = c1 + c2
        acc += 2
        i += 1
f(\"hi\")
";
    let stderr = run_pyrust_stderr("loop_later_fold_no_steal.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"loop_later_fold_no_steal.py\", line 10, in <module>
    f(\"hi\")
  File \"loop_later_fold_no_steal.py\", line 5, in f
    res = s + 1
          ~~^~~
TypeError: can only concatenate str (not \"int\") to str
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn straightline_folded_rhs_and_repeated_augassign_keeps_line_and_caret() {
    // Issue #2905: `acc += 1` appears 3 times in the unoptimized stream and
    // folds away at line 4, leaving 2 surviving copies at lines 9 and 11.
    // Bounded Hunt-Szymanski slack in `lcs_align` keeps lines 9 and 11 aligned
    // to their true positions instead of collapsing the interval before line 6.
    let src = "\
def f(b):
    i = 0
    acc = 0
    acc += 1
    b / 2
    b // (2 - 2)
    acc = b // 2
    if b > 0:
        acc += 1
    if i % 2 == 0:
        acc += 1
f(10)
";
    let stderr = run_pyrust_stderr("straightline_slack.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"straightline_slack.py\", line 12, in <module>
    f(10)
  File \"straightline_slack.py\", line 6, in f
    b // (2 - 2)
    ~~^^~~~~~~~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn loop_folded_rhs_before_if_zero_comparison_keeps_line_and_caret() {
    // Issue #2905: `2 - 2` folds to `LoadConst(t, 0)` right before `b // t`,
    // matching the raw instruction of `if b > 0:`'s fused-away `LoadConst(t, 0)`.
    // LCS must not align the synthesized `LoadConst` onto `if b > 0:` and jump
    // `old_pos` past `b // (2 - 2)`.
    let src = "\
def f(b):
    i = 0
    acc = 0
    while i < 4:
        if i % 2 == 0:
            acc += 1
        acc = b // 2
        b // (2 - 2)
        if b > 0:
            acc += 1
        i += 1
f(10)
";
    let stderr = run_pyrust_stderr("loop_folded_rhs_if0.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"loop_folded_rhs_if0.py\", line 12, in <module>
    f(10)
  File \"loop_folded_rhs_if0.py\", line 8, in f
    b // (2 - 2)
    ~~^^~~~~~~~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn generator_constfold_with_yield_and_fused_raise_caret_is_byte_exact() {
    // Issue #2905: generator loop with a constant fold, `yield`, and a fused
    // raising division must keep exact line and caret.
    let src = "\
def gen(n):
    i = 0
    while i < n:
        c = 2 * 3
        yield i * 2
        if i == 2:
            i // 0
        i += 1
list(gen(5))
";
    let stderr = run_pyrust_stderr("gen_cf_fused.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"gen_cf_fused.py\", line 9, in <module>
    list(gen(5))
  File \"gen_cf_fused.py\", line 7, in gen
    i // 0
    ~~^^~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn nested_if_and_try_except_with_constfold_and_fused_raise_caret_is_byte_exact() {
    // Issue #2905: `try`/`except` inside a loop with a constant fold and a
    // non-caught fused raising `% 0` must keep exact line and caret.
    let src = "\
def f(b):
    i = 0
    while i < 3:
        try:
            a = 1 + 2
            b / 2
            b % 0
        except ValueError:
            pass
        i += 1
f(8)
";
    let stderr = run_pyrust_stderr("loop_try_cf_fused.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"loop_try_cf_fused.py\", line 11, in <module>
    f(8)
  File \"loop_try_cf_fused.py\", line 7, in f
    b % 0
    ~~^~~
ZeroDivisionError: integer modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

// ── #2992: await / yield from / yield anchors, multi-line calls, genexpr ────

#[test]
fn yield_from_assigned_sub_raise_caret_is_byte_exact() {
    let src = "\
def sub():
    raise RuntimeError(\"boom\")
    yield 1

def outer():
    x = yield from sub()
    return x

g = outer()
next(g)
";
    let stderr = run_pyrust_stderr("yf_assign.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"yf_assign.py\", line 10, in <module>
    next(g)
  File \"yf_assign.py\", line 6, in outer
    x = yield from sub()
        ^^^^^^^^^^^^^^^^
  File \"yf_assign.py\", line 2, in sub
    raise RuntimeError(\"boom\")
RuntimeError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn yield_from_multiline_whole_line_omits_caret() {
    let src = "\
def sub(a, b):
    raise RuntimeError(\"boom\")
    yield a

def outer():
    x = (
        yield from sub(
            1,
            2,
        )
    )
    return x

g = outer()
next(g)
";
    let stderr = run_pyrust_stderr("yf_ml.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"yf_ml.py\", line 15, in <module>
    next(g)
  File \"yf_ml.py\", line 7, in outer
    yield from sub(
  File \"yf_ml.py\", line 2, in sub
    raise RuntimeError(\"boom\")
RuntimeError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn yield_from_assigned_bad_iter_caret_is_byte_exact() {
    let src = "def outer():\n    x = yield from 123\nnext(outer())\n";
    let stderr = run_pyrust_stderr("yf_bad_iter.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"yf_bad_iter.py\", line 3, in <module>
    next(outer())
  File \"yf_bad_iter.py\", line 2, in outer
    x = yield from 123
        ^^^^^^^^^^^^^^
TypeError: 'int' object is not iterable
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn yield_from_assigned_throw_caret_is_byte_exact() {
    let src = "\
def sub():
    yield 1

def outer():
    x = yield from sub()

g = outer()
next(g)
g.throw(ValueError(\"t\"))
";
    let stderr = run_pyrust_stderr("yf_throw.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"yf_throw.py\", line 9, in <module>
    g.throw(ValueError(\"t\"))
  File \"yf_throw.py\", line 5, in outer
    x = yield from sub()
        ^^^^^^^^^^^^^^^^
  File \"yf_throw.py\", line 2, in sub
    yield 1
ValueError: t
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn yield_assigned_throw_caret_is_byte_exact() {
    let src = "def g():\n    x = yield 1\ngen = g()\nnext(gen)\ngen.throw(ValueError(\"t\"))\n";
    let stderr = run_pyrust_stderr("y_throw.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"y_throw.py\", line 5, in <module>
    gen.throw(ValueError(\"t\"))
  File \"y_throw.py\", line 2, in g
    x = yield 1
        ^^^^^^^
ValueError: t
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn await_return_caret_is_byte_exact() {
    let src = "\
async def bad():
    raise RuntimeError(\"boom\")

async def caller():
    return await bad()

caller().send(None)
";
    let stderr = run_pyrust_stderr("await_ret.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"await_ret.py\", line 7, in <module>
    caller().send(None)
  File \"await_ret.py\", line 5, in caller
    return await bad()
           ^^^^^^^^^^^
  File \"await_ret.py\", line 2, in bad
    raise RuntimeError(\"boom\")
RuntimeError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn await_assign_caret_is_byte_exact() {
    let src = "\
async def bad():
    raise RuntimeError(\"boom\")

async def caller():
    x = await bad()
    return x

caller().send(None)
";
    let stderr = run_pyrust_stderr("await_assign.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"await_assign.py\", line 8, in <module>
    caller().send(None)
  File \"await_assign.py\", line 5, in caller
    x = await bad()
        ^^^^^^^^^^^
  File \"await_assign.py\", line 2, in bad
    raise RuntimeError(\"boom\")
RuntimeError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn await_multiline_caret_is_byte_exact() {
    let src = "\
async def bad(a, b):
    raise RuntimeError(\"boom\")

async def caller():
    x = await bad(
        1,
        2,
    )
    return x

caller().send(None)
";
    let stderr = run_pyrust_stderr("await_ml.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"await_ml.py\", line 11, in <module>
    caller().send(None)
  File \"await_ml.py\", line 5, in caller
    x = await bad(
        ^^^^^^^^^^
  File \"await_ml.py\", line 2, in bad
    raise RuntimeError(\"boom\")
RuntimeError: boom
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn await_not_awaitable_caret_is_byte_exact() {
    let src = "\
async def caller():
    x = await 123
    return x

caller().send(None)
";
    let stderr = run_pyrust_stderr("await_int.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"await_int.py\", line 5, in <module>
    caller().send(None)
  File \"await_int.py\", line 2, in caller
    x = await 123
        ^^^^^^^^^
TypeError: object int can't be used in 'await' expression
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn multiline_call_in_function_caret_is_byte_exact() {
    let src = "\
def inner(a, b):
    raise ValueError(\"fail\")

def outer():
    return inner(
        1,
        2,
    )

outer()
";
    let stderr = run_pyrust_stderr("ml_call_fn.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ml_call_fn.py\", line 10, in <module>
    outer()
  File \"ml_call_fn.py\", line 5, in outer
    return inner(
           ^^^^^^
  File \"ml_call_fn.py\", line 2, in inner
    raise ValueError(\"fail\")
ValueError: fail
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn multiline_call_whole_line_omits_caret() {
    let src = "\
def f(a, b):
    raise ValueError(\"fail\")

f(
    1,
    2,
)
";
    let stderr = run_pyrust_stderr("ml_call_mod.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ml_call_mod.py\", line 4, in <module>
    f(
  File \"ml_call_mod.py\", line 2, in f
    raise ValueError(\"fail\")
ValueError: fail
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn multiline_method_call_caret_is_byte_exact() {
    let src = "\
class Box:
    def boom(self, a, b):
        raise ValueError(\"fail\")

def outer(b):
    return b.boom(
        1,
        2,
    )

outer(Box())
";
    let stderr = run_pyrust_stderr("ml_method.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ml_method.py\", line 11, in <module>
    outer(Box())
  File \"ml_method.py\", line 6, in outer
    return b.boom(
           ^^^^^^^
  File \"ml_method.py\", line 3, in boom
    raise ValueError(\"fail\")
ValueError: fail
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn multiline_chained_attr_call_caret_is_byte_exact() {
    let src = "\
class Box:
    def boom(self, a):
        raise ValueError(\"fail\")

def outer(b):
    return (
        b
        .boom(
            1,
        )
    )

outer(Box())
";
    let stderr = run_pyrust_stderr("ml_chain.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ml_chain.py\", line 13, in <module>
    outer(Box())
  File \"ml_chain.py\", line 8, in outer
    .boom(
     ^^^^^
  File \"ml_chain.py\", line 3, in boom
    raise ValueError(\"fail\")
ValueError: fail
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn multiline_nested_call_reports_inner_line() {
    let src = "\
def g(x):
    raise ValueError(\"inner\")

def f(x):
    return x

def outer():
    return f(
        g(
            1,
        )
    )

outer()
";
    let stderr = run_pyrust_stderr("ml_nested.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ml_nested.py\", line 14, in <module>
    outer()
  File \"ml_nested.py\", line 9, in outer
    g(
  File \"ml_nested.py\", line 2, in g
    raise ValueError(\"inner\")
ValueError: inner
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn multiline_call_in_binop_caret_is_byte_exact() {
    let src = "\
def inner(a, b):
    raise ValueError(\"fail\")

def outer():
    return 1 + inner(
        1,
        2,
    )

outer()
";
    let stderr = run_pyrust_stderr("ml_binop_call.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ml_binop_call.py\", line 10, in <module>
    outer()
  File \"ml_binop_call.py\", line 5, in outer
    return 1 + inner(
               ^^^^^^
  File \"ml_binop_call.py\", line 2, in inner
    raise ValueError(\"fail\")
ValueError: fail
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn multiline_keyword_call_caret_is_byte_exact() {
    let src = "\
def inner(a=0, b=0):
    raise ValueError(\"fail\")

def outer():
    return inner(
        a=1,
        b=2,
    )

outer()
";
    let stderr = run_pyrust_stderr("ml_kw_call.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ml_kw_call.py\", line 10, in <module>
    outer()
  File \"ml_kw_call.py\", line 5, in outer
    return inner(
           ^^^^^^
  File \"ml_kw_call.py\", line 2, in inner
    raise ValueError(\"fail\")
ValueError: fail
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn genexpr_single_line_element_caret_is_byte_exact() {
    let src = "\
def genexpr_test():
    g = (1 // x for x in [0])
    return list(g)

genexpr_test()
";
    let stderr = run_pyrust_stderr("ge_single.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ge_single.py\", line 5, in <module>
    genexpr_test()
  File \"ge_single.py\", line 3, in genexpr_test
    return list(g)
           ^^^^^^^
  File \"ge_single.py\", line 2, in <genexpr>
    g = (1 // x for x in [0])
         ~~^^~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn genexpr_multiline_element_caret_is_byte_exact() {
    let src = "\
def genexpr_test():
    g = (
        1 // x
        for x in [0]
    )
    return list(g)

genexpr_test()
";
    let stderr = run_pyrust_stderr("ge_ml.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ge_ml.py\", line 8, in <module>
    genexpr_test()
  File \"ge_ml.py\", line 6, in genexpr_test
    return list(g)
           ^^^^^^^
  File \"ge_ml.py\", line 3, in <genexpr>
    1 // x
    ~~^^~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn genexpr_filter_call_caret_is_byte_exact() {
    let src = "\
def check(x):
    raise ValueError(x)

def genexpr_test():
    g = (
        x
        for x in [0]
        if check(x)
    )
    return list(g)

genexpr_test()
";
    let stderr = run_pyrust_stderr("ge_filter_call.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ge_filter_call.py\", line 12, in <module>
    genexpr_test()
  File \"ge_filter_call.py\", line 10, in genexpr_test
    return list(g)
           ^^^^^^^
  File \"ge_filter_call.py\", line 8, in <genexpr>
    if check(x)
       ^^^^^^^^
  File \"ge_filter_call.py\", line 2, in check
    raise ValueError(x)
ValueError: 0
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn genexpr_filter_subscript_caret_is_byte_exact() {
    let src = "\
def genexpr_test():
    d = {}
    g = (
        x
        for x in [\"k\"]
        if d[x]
    )
    return list(g)

genexpr_test()
";
    let stderr = run_pyrust_stderr("ge_filter_sub.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ge_filter_sub.py\", line 10, in <module>
    genexpr_test()
  File \"ge_filter_sub.py\", line 8, in genexpr_test
    return list(g)
           ^^^^^^^
  File \"ge_filter_sub.py\", line 6, in <genexpr>
    if d[x]
       ~^^^
KeyError: 'k'
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn genexpr_nested_element_caret_is_byte_exact() {
    let src = "\
def genexpr_test():
    g = (
        (
            1 // y
            for y in [x]
        )
        for x in [0]
    )
    for inner in g:
        list(inner)

genexpr_test()
";
    let stderr = run_pyrust_stderr("ge_nested.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ge_nested.py\", line 12, in <module>
    genexpr_test()
  File \"ge_nested.py\", line 10, in genexpr_test
    list(inner)
  File \"ge_nested.py\", line 4, in <genexpr>
    1 // y
    ~~^^~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn genexpr_sum_multiline_caret_is_byte_exact() {
    let src = "\
def genexpr_test():
    return sum(
        1 // x
        for x in [0]
    )

genexpr_test()
";
    let stderr = run_pyrust_stderr("ge_sum.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ge_sum.py\", line 7, in <module>
    genexpr_test()
  File \"ge_sum.py\", line 2, in genexpr_test
    return sum(
           ^^^^
  File \"ge_sum.py\", line 3, in <genexpr>
    1 // x
    ~~^^~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn genexpr_multiline_call_element_is_byte_exact() {
    let src = "\
def boom(x):
    raise ValueError(x)

def genexpr_test():
    g = (
        boom(
            x,
        )
        for x in [1]
    )
    return list(g)

genexpr_test()
";
    let stderr = run_pyrust_stderr("ge_call_ml.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"ge_call_ml.py\", line 13, in <module>
    genexpr_test()
  File \"ge_call_ml.py\", line 11, in genexpr_test
    return list(g)
           ^^^^^^^
  File \"ge_call_ml.py\", line 6, in <genexpr>
    boom(
  File \"ge_call_ml.py\", line 2, in boom
    raise ValueError(x)
ValueError: 1
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

// ── #2993: PEP 709 comprehension frames, except* rewrap, asyncio filenames,
//           and PEP 479 chain ordering / traceback ownership ────────────────

#[test]
fn pep709_listcomp_over_generator_has_no_listcomp_frame() {
    let src = "def g():\n    yield 1/0\n[v for v in g()]\n";
    let stderr = run_pyrust_stderr("pep709_bare_listcomp.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"pep709_bare_listcomp.py\", line 3, in <module>
    [v for v in g()]
  File \"pep709_bare_listcomp.py\", line 2, in g
    yield 1/0
          ~^~
ZeroDivisionError: division by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn pep709_assigned_listcomp_over_generator_carries_comp_caret() {
    let src = "def g():\n    yield 1/0\nx = [v for v in g()]\n";
    let stderr = run_pyrust_stderr("pep709_assign_listcomp.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"pep709_assign_listcomp.py\", line 3, in <module>
    x = [v for v in g()]
        ^^^^^^^^^^^^^^^^
  File \"pep709_assign_listcomp.py\", line 2, in g
    yield 1/0
          ~^~
ZeroDivisionError: division by zero
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn pep709_listcomp_elt_and_filter_binops_carry_narrow_carets() {
    let src_elt = "def f():\n    return [1 // x for x in [0]]\nf()\n";
    let stderr_elt = run_pyrust_stderr("pep709_elt.py", src_elt);
    let expected_elt = "\
Traceback (most recent call last):
  File \"pep709_elt.py\", line 3, in <module>
    f()
  File \"pep709_elt.py\", line 2, in f
    return [1 // x for x in [0]]
            ~~^^~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr_elt, expected_elt, "got:\n{stderr_elt}");

    let src_cond = "def f():\n    return [x for x in [0] if 1 // x]\nf()\n";
    let stderr_cond = run_pyrust_stderr("pep709_cond.py", src_cond);
    let expected_cond = "\
Traceback (most recent call last):
  File \"pep709_cond.py\", line 3, in <module>
    f()
  File \"pep709_cond.py\", line 2, in f
    return [x for x in [0] if 1 // x]
                              ~~^^~~
ZeroDivisionError: integer division or modulo by zero
";
    assert_eq!(stderr_cond, expected_cond, "got:\n{stderr_cond}");
}

#[test]
fn pep709_nested_set_dict_and_non_iterable_comps_have_no_comp_frames() {
    let src_nested = "def g():\n    yield 1/0\nx = [[v for v in g()] for _ in [1]]\n";
    let stderr_nested = run_pyrust_stderr("pep709_nested.py", src_nested);
    let expected_nested = "\
Traceback (most recent call last):
  File \"pep709_nested.py\", line 3, in <module>
    x = [[v for v in g()] for _ in [1]]
         ^^^^^^^^^^^^^^^^
  File \"pep709_nested.py\", line 2, in g
    yield 1/0
          ~^~
ZeroDivisionError: division by zero
";
    assert_eq!(stderr_nested, expected_nested, "got:\n{stderr_nested}");

    let src_set = "def g():\n    yield 1/0\nx = {v for v in g()}\n";
    let stderr_set = run_pyrust_stderr("pep709_set.py", src_set);
    let expected_set = "\
Traceback (most recent call last):
  File \"pep709_set.py\", line 3, in <module>
    x = {v for v in g()}
        ^^^^^^^^^^^^^^^^
  File \"pep709_set.py\", line 2, in g
    yield 1/0
          ~^~
ZeroDivisionError: division by zero
";
    assert_eq!(stderr_set, expected_set, "got:\n{stderr_set}");

    let src_dict = "def g():\n    yield 1/0\nx = {v: v for v in g()}\n";
    let stderr_dict = run_pyrust_stderr("pep709_dict.py", src_dict);
    let expected_dict = "\
Traceback (most recent call last):
  File \"pep709_dict.py\", line 3, in <module>
    x = {v: v for v in g()}
        ^^^^^^^^^^^^^^^^^^^
  File \"pep709_dict.py\", line 2, in g
    yield 1/0
          ~^~
ZeroDivisionError: division by zero
";
    assert_eq!(stderr_dict, expected_dict, "got:\n{stderr_dict}");

    let src_noniter = "x = [v for v in 123]\n";
    let stderr_noniter = run_pyrust_stderr("pep709_noniter.py", src_noniter);
    let expected_noniter = "\
Traceback (most recent call last):
  File \"pep709_noniter.py\", line 1, in <module>
    x = [v for v in 123]
        ^^^^^^^^^^^^^^^^
TypeError: 'int' object is not iterable
";
    assert_eq!(stderr_noniter, expected_noniter, "got:\n{stderr_noniter}");
}

#[test]
fn except_star_bare_reraise_rewraps_plain_exception_into_exception_group() {
    let src = "def g():\n    yield 1/0\ntry:\n    for _ in g():\n        pass\nexcept* ZeroDivisionError:\n    raise\n";
    let stderr = run_pyrust_stderr("except_star_bare_reraise.py", src);
    let expected = concat!(
        "  | ExceptionGroup:  (1 sub-exception)\n",
        "  +-+---------------- 1 ----------------\n",
        "    | Traceback (most recent call last):\n",
        "    |   File \"except_star_bare_reraise.py\", line 4, in <module>\n",
        "    |     for _ in g():\n",
        "    |   File \"except_star_bare_reraise.py\", line 2, in g\n",
        "    |     yield 1/0\n",
        "    |           ~^~\n",
        "    | ZeroDivisionError: division by zero\n",
        "    +------------------------------------\n",
    );
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn except_star_bare_reraise_in_function_omits_reraise_frame() {
    let src = "def g():\n    yield 1/0\ndef outer():\n    try:\n        for _ in g():\n            pass\n    except* ZeroDivisionError:\n        raise\nouter()\n";
    let stderr = run_pyrust_stderr("except_star_fn_reraise.py", src);
    let expected = concat!(
        "  + Exception Group Traceback (most recent call last):\n",
        "  |   File \"except_star_fn_reraise.py\", line 9, in <module>\n",
        "  |     outer()\n",
        "  | ExceptionGroup:  (1 sub-exception)\n",
        "  +-+---------------- 1 ----------------\n",
        "    | Traceback (most recent call last):\n",
        "    |   File \"except_star_fn_reraise.py\", line 5, in outer\n",
        "    |     for _ in g():\n",
        "    |   File \"except_star_fn_reraise.py\", line 2, in g\n",
        "    |     yield 1/0\n",
        "    |           ~^~\n",
        "    | ZeroDivisionError: division by zero\n",
        "    +------------------------------------\n",
    );
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn except_star_partial_residual_and_combined_raise_render_byte_exact() {
    let src_res = "try:\n    raise ExceptionGroup(\"eg\", [ValueError(\"v\"), TypeError(\"t\")])\nexcept* ValueError:\n    pass\n";
    let stderr_res = run_pyrust_stderr("except_star_residual.py", src_res);
    let expected_res = concat!(
        "  + Exception Group Traceback (most recent call last):\n",
        "  |   File \"except_star_residual.py\", line 2, in <module>\n",
        "  |     raise ExceptionGroup(\"eg\", [ValueError(\"v\"), TypeError(\"t\")])\n",
        "  | ExceptionGroup: eg (1 sub-exception)\n",
        "  +-+---------------- 1 ----------------\n",
        "    | TypeError: t\n",
        "    +------------------------------------\n",
    );
    assert_eq!(stderr_res, expected_res, "got:\n{stderr_res}");

    let src_comb = "try:\n    raise ExceptionGroup(\"eg\", [ValueError(\"v\"), TypeError(\"t\")])\nexcept* ValueError:\n    raise KeyError(\"k\")\n";
    let stderr_comb = run_pyrust_stderr("except_star_combined.py", src_comb);
    let expected_comb = concat!(
        "  | ExceptionGroup:  (2 sub-exceptions)\n",
        "  +-+---------------- 1 ----------------\n",
        "    | Exception Group Traceback (most recent call last):\n",
        "    |   File \"except_star_combined.py\", line 2, in <module>\n",
        "    |     raise ExceptionGroup(\"eg\", [ValueError(\"v\"), TypeError(\"t\")])\n",
        "    | ExceptionGroup: eg (1 sub-exception)\n",
        "    +-+---------------- 1 ----------------\n",
        "      | ValueError: v\n",
        "      +------------------------------------\n",
        "    | \n",
        "    | During handling of the above exception, another exception occurred:\n",
        "    | \n",
        "    | Traceback (most recent call last):\n",
        "    |   File \"except_star_combined.py\", line 4, in <module>\n",
        "    |     raise KeyError(\"k\")\n",
        "    | KeyError: 'k'\n",
        "    +---------------- 2 ----------------\n",
        "    | Exception Group Traceback (most recent call last):\n",
        "    |   File \"except_star_combined.py\", line 2, in <module>\n",
        "    |     raise ExceptionGroup(\"eg\", [ValueError(\"v\"), TypeError(\"t\")])\n",
        "    | ExceptionGroup: eg (1 sub-exception)\n",
        "    +-+---------------- 1 ----------------\n",
        "      | TypeError: t\n",
        "      +------------------------------------\n",
    );
    assert_eq!(stderr_comb, expected_comb, "got:\n{stderr_comb}");
}

#[test]
fn asyncio_bundled_frames_report_frozen_filename_and_keep_run_anchor() {
    // Pad the script past line 100 so any regression that looks up asyncio's
    // internal line numbers against the script source would echo `# pad` lines.
    let mut src = String::new();
    for _ in 0..100 {
        src.push_str("# pad\n");
    }
    src.push_str("import asyncio\nasync def co():\n    1 / 0\nx = asyncio.run(co())\n");
    let stderr = run_pyrust_stderr("asyncio_frames.py", &src);
    assert!(
        stderr.contains(
            "  File \"asyncio_frames.py\", line 104, in <module>\n    x = asyncio.run(co())\n        ^^^^^^^^^^^^^^^^^\n"
        ),
        "missing module frame or asyncio.run caret:\n{stderr}"
    );
    // Native-Rust `asyncio` (issue #3254) has no bundled Python shim frames:
    // the traceback flows directly from the user's `asyncio.run(co())` anchor
    // into `co()` without emitting `<frozen asyncio>` frames or `# pad` lines.
    assert!(
        !stderr.contains("  File \"<frozen asyncio>\", line "),
        "native asyncio should not emit bundled Python frames:\n{stderr}"
    );
    assert!(
        !stderr.contains("# pad"),
        "bundled asyncio frame mistakenly echoed script source lines:\n{stderr}"
    );
    assert!(
        stderr.contains(
            "  File \"asyncio_frames.py\", line 103, in co\n    1 / 0\n    ~~^~~\nZeroDivisionError: division by zero\n"
        ),
        "missing co() frame or caret:\n{stderr}"
    );
    // Ensure no bundled frame reports the user's script filename.
    for line in stderr.lines() {
        if line.contains("File \"asyncio_frames.py\"") {
            assert!(
                line.contains("in <module>") || line.contains("in co"),
                "non-user frame reported script filename: {line}\nfull:\n{stderr}"
            );
        }
    }
}

#[test]
fn pep479_stopiteration_chain_order_and_frame_ownership_are_byte_exact() {
    let src = "def g():\n    raise StopIteration(\"s\")\n    yield 1\nnext(g())\n";
    let stderr = run_pyrust_stderr("pep479_simple.py", src);
    let expected = "\
Traceback (most recent call last):
  File \"pep479_simple.py\", line 2, in g
    raise StopIteration(\"s\")
StopIteration: s

The above exception was the direct cause of the following exception:

Traceback (most recent call last):
  File \"pep479_simple.py\", line 4, in <module>
    next(g())
RuntimeError: generator raised StopIteration
";
    assert_eq!(stderr, expected, "got:\n{stderr}");

    let src_deep = "def h():\n    raise StopIteration(\"deep\")\ndef g():\n    h()\n    yield 1\nfor _ in g():\n    pass\n";
    let stderr_deep = run_pyrust_stderr("pep479_deep.py", src_deep);
    let expected_deep = "\
Traceback (most recent call last):
  File \"pep479_deep.py\", line 4, in g
    h()
  File \"pep479_deep.py\", line 2, in h
    raise StopIteration(\"deep\")
StopIteration: deep

The above exception was the direct cause of the following exception:

Traceback (most recent call last):
  File \"pep479_deep.py\", line 6, in <module>
    for _ in g():
RuntimeError: generator raised StopIteration
";
    assert_eq!(stderr_deep, expected_deep, "got:\n{stderr_deep}");

    let src_ctx = "def g():\n    try:\n        raise KeyError(\"orig\")\n    except KeyError:\n        raise StopIteration(\"s\")\n    yield 1\nnext(g())\n";
    let stderr_ctx = run_pyrust_stderr("pep479_ctx.py", src_ctx);
    let expected_ctx = "\
Traceback (most recent call last):
  File \"pep479_ctx.py\", line 3, in g
    raise KeyError(\"orig\")
KeyError: 'orig'

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File \"pep479_ctx.py\", line 5, in g
    raise StopIteration(\"s\")
StopIteration: s

The above exception was the direct cause of the following exception:

Traceback (most recent call last):
  File \"pep479_ctx.py\", line 7, in <module>
    next(g())
RuntimeError: generator raised StopIteration
";
    assert_eq!(stderr_ctx, expected_ctx, "got:\n{stderr_ctx}");
}

// ── #3402: located SyntaxError printed like CPython's print_exception ──────
//
// A compile-time SyntaxError carries its location, so the uncaught printer
// shows CPython's `File "<name>", line N` block (source line + caret) and the
// bare `msg` instead of `str(exc)`'s `msg (<name>, line N)`.

#[test]
fn uncaught_exec_syntax_error_shows_location_block() {
    let stderr = run_pyrust_stderr("exec_syntax.py", "exec(\"x = 1\\ny = (2 +\\n\")\n");
    let expected = "\
Traceback (most recent call last):
  File \"exec_syntax.py\", line 1, in <module>
    exec(\"x = 1\\ny = (2 +\\n\")
  File \"<string>\", line 2
    y = (2 +
        ^
SyntaxError: '(' was never closed
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn uncaught_indentation_error_underlines_one_char() {
    let stderr = run_pyrust_stderr(
        "indent_syntax.py",
        "compile(\"if 1:\\n    x = 1\\n  y = 2\\n\", \"<cfg>\", \"exec\")\n",
    );
    let expected = "\
Traceback (most recent call last):
  File \"indent_syntax.py\", line 1, in <module>
    compile(\"if 1:\\n    x = 1\\n  y = 2\\n\", \"<cfg>\", \"exec\")
  File \"<cfg>\", line 3
    y = 2
         ^
IndentationError: unindent does not match any outer indentation level
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}

#[test]
fn uncaught_user_raised_syntax_error_spans_end_offset() {
    let stderr = run_pyrust_stderr(
        "raised_syntax.py",
        "raise SyntaxError(\"bad thing\", (\"<t>\", 1, 5, \"x = (1 +\\n\", 1, 9))\n",
    );
    let expected = "\
Traceback (most recent call last):
  File \"raised_syntax.py\", line 1, in <module>
    raise SyntaxError(\"bad thing\", (\"<t>\", 1, 5, \"x = (1 +\\n\", 1, 9))
  File \"<t>\", line 1
    x = (1 +
        ^^^^
SyntaxError: bad thing
";
    assert_eq!(stderr, expected, "got:\n{stderr}");
}
