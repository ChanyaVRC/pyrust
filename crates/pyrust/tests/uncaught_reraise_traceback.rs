//! Issue #2404: the uncaught (top-level) stderr traceback formatter consumes
//! the exception's prepended `__traceback__` chain for a carried re-raise,
//! instead of rebuilding from the captured-frame snapshot (which #2367/#2403
//! reset at the re-raise site, so it diverged and dropped frames).
//!
//! Issues #2408/#2412: an uncaught exception that chains another (`__cause__`
//! via `raise X from Y`, or implicit `__context__` from raising during
//! handling) must print each chained exception's OWN full traceback block
//! (`Traceback (most recent call last):` + `File ...` frames + class line)
//! above the connecting banner, innermost-context first — not just the
//! one-line class summary.
//!
//! This is NOT a parity fixture: the parity harness requires the reference
//! Python process to exit 0, but these scripts deliberately let an exception
//! escape to the top level (exit 1).  We instead run the pyrust binary on a
//! temp script and assert the emitted `File "...", line N, in NAME` frame list
//! matches the CPython 3.12 walk for each scenario (line + function name; the
//! file path is normalised away).
//!
//! For the chained-block tests we assert on the chained exception's own
//! traceback frames (which are byte-exact vs CPython) and the banner text /
//! ordering, rather than diffing the whole stderr: the MAIN exception's frame
//! list carries a known spurious inner frame (#2407) and source-line/caret
//! rendering diverges (#2411), both out of scope here.

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

/// For each `File "...", line N, in NAME` frame header, return the *echoed
/// source line* that follows it (trimmed), or `None` when no source line was
/// emitted under that header.  Caret/tilde underline rows are skipped so the
/// pairing lands on the source text.  Used by the #2428 tests to assert that
/// every frame — not just the innermost — echoes its source line.
fn frame_source_lines(stderr: &str) -> Vec<Option<String>> {
    let lines: Vec<&str> = stderr.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if !t.starts_with("File \"") {
            continue;
        }
        // The source echo, when present, is the next line and is indented with
        // four spaces (CPython's fixed source-echo indent — deeper than the
        // two-space `File` header indent).  A frame with no echo is followed
        // directly by the next `File` header or the column-0 exception class
        // line, neither of which has that indent.
        match lines.get(i + 1) {
            Some(next) if next.starts_with("    ") => {
                let nt = next.trim();
                // Guard against an (unexpected) leading caret row.
                if !nt.is_empty() && !nt.chars().all(|c| c == '^' || c == '~') {
                    out.push(Some(nt.to_string()));
                } else {
                    out.push(None);
                }
            }
            _ => out.push(None),
        }
    }
    out
}

const CAUSE_BANNER: &str = "The above exception was the direct cause of the following exception:";
const CONTEXT_BANNER: &str = "During handling of the above exception, another exception occurred:";

/// Split a chained traceback's stderr into the blocks separated by the
/// connecting banners, in printed (oldest-first) order.  Each block is the text
/// between banners (a `Traceback ...` header + frames + class line, or — for a
/// never-raised chained exception — just the class line).  Returns
/// `(blocks, banners)` where `banners[i]` is the banner that *follows*
/// `blocks[i]`.
fn split_chain_blocks(stderr: &str) -> (Vec<String>, Vec<String>) {
    let mut blocks = Vec::new();
    let mut banners = Vec::new();
    let mut cur = String::new();
    for line in stderr.lines() {
        let t = line.trim();
        if t == CAUSE_BANNER || t == CONTEXT_BANNER {
            blocks.push(std::mem::take(&mut cur));
            banners.push(t.to_string());
        } else {
            cur.push_str(line);
            cur.push('\n');
        }
    }
    blocks.push(cur);
    (blocks, banners)
}

#[test]
fn uncaught_context_chain_prints_chained_traceback() {
    // Implicit __context__ chain: f() raises IndexError, g() handles it and
    // raises ValueError.  CPython 3.12 prints the IndexError's OWN traceback
    // block (g 5 -> f 2) above the "During handling..." banner, then the
    // ValueError block.
    let src = "def f():\n    raise IndexError(\"idx\")\ndef g():\n    try:\n        f()\n    except IndexError:\n        raise ValueError(\"v\")\ng()\n";
    let stderr = run_pyrust_stderr(src);
    let (blocks, banners) = split_chain_blocks(&stderr);
    assert_eq!(blocks.len(), 2, "expected one chained block + main block");
    assert_eq!(banners, vec![CONTEXT_BANNER.to_string()]);
    // Chained (IndexError) block: full traceback with g 5 -> f 2.
    assert!(
        blocks[0].contains("Traceback (most recent call last):"),
        "chained block must have a Traceback header, got:\n{}",
        blocks[0]
    );
    assert_eq!(
        frame_list(&blocks[0]),
        vec![("g".to_string(), 5), ("f".to_string(), 2)],
        "chained IndexError block frames",
    );
    assert!(
        blocks[0].trim_end().ends_with("IndexError: idx"),
        "chained block must end with its class line, got:\n{}",
        blocks[0]
    );
}

#[test]
fn uncaught_cause_chain_prints_chained_traceback() {
    // Explicit __cause__ chain (`raise X from e`): same frames as the context
    // case but the "...direct cause..." banner.
    let src = "def f():\n    raise IndexError(\"idx\")\ndef g():\n    try:\n        f()\n    except IndexError as e:\n        raise ValueError(\"v\") from e\ng()\n";
    let stderr = run_pyrust_stderr(src);
    let (blocks, banners) = split_chain_blocks(&stderr);
    assert_eq!(blocks.len(), 2);
    assert_eq!(banners, vec![CAUSE_BANNER.to_string()]);
    assert!(blocks[0].contains("Traceback (most recent call last):"));
    assert_eq!(
        frame_list(&blocks[0]),
        vec![("g".to_string(), 5), ("f".to_string(), 2)],
    );
    assert!(blocks[0].trim_end().ends_with("IndexError: idx"));
}

#[test]
fn uncaught_fresh_raise_in_handler_main_block_has_no_stale_frame() {
    // Issue #2407: a *fresh* `raise` inside an `except` must not inherit the
    // stale captured-frame snapshot of the exception being handled.  Here f()
    // raises IndexError, g() handles it and raises a brand-new ValueError.  The
    // MAIN (ValueError) block must list ONLY the ValueError's own unwind frame
    // (g 7) plus the module call site — never f (the IndexError's frame).
    let src = "def f():\n    raise IndexError(\"idx\")\ndef g():\n    try:\n        f()\n    except IndexError:\n        raise ValueError(\"v\")\ng()\n";
    let stderr = run_pyrust_stderr(src);
    let (blocks, banners) = split_chain_blocks(&stderr);
    assert_eq!(blocks.len(), 2, "one chained block + main block");
    assert_eq!(banners, vec![CONTEXT_BANNER.to_string()]);
    // Chained IndexError block keeps its own frames (g 5 -> f 2).
    assert_eq!(
        frame_list(&blocks[0]),
        vec![("g".to_string(), 5), ("f".to_string(), 2)],
        "chained IndexError block frames",
    );
    // Main ValueError block must NOT carry the spurious `f` frame.
    assert_eq!(
        frame_list(&blocks[1]),
        vec![("<module>".to_string(), 8), ("g".to_string(), 7)],
        "main ValueError block must not inherit the handled exception's frame, got:\n{}",
        blocks[1]
    );
}

#[test]
fn uncaught_fresh_raise_in_finally_main_block_has_no_stale_frame() {
    // Issue #2407: same guarantee for a fresh raise in a `finally` block during
    // unwind.  f() raises KeyError; g()'s finally raises a fresh ValueError.
    // The main block must list only g 7 + the module call site (8), not f.
    let src = "def f():\n    raise KeyError(\"k\")\ndef g():\n    try:\n        f()\n    finally:\n        raise ValueError(\"v\")\ng()\n";
    let stderr = run_pyrust_stderr(src);
    let main = split_chain_blocks(&stderr).0.pop().unwrap();
    assert_eq!(
        frame_list(&main),
        vec![("<module>".to_string(), 8), ("g".to_string(), 7)],
        "finally fresh-raise main block must not inherit the handled frame, got:\n{main}",
    );
}

#[test]
fn uncaught_implicit_second_exception_in_handler_chains_and_separates() {
    // Issue #2583: a second exception raised *implicitly* (here a TypeError
    // from `1 + x`, not an explicit `raise`) inside an `except` handler must
    //   (a) carry the original exception as its `__context__` so the
    //       "During handling of the above exception..." banner is printed, and
    //   (b) keep its own unwind frames separate — the original bug merged both
    //       exceptions' frames into a single traceback (a duplicated `f` frame)
    //       and dropped the banner entirely because the implicit exception
    //       escaped uncaught without `attach_implicit_context` ever running.
    let src =
        "def f(x):\n    return 1 + x\ntry:\n    f(\"z\")\nexcept TypeError:\n    f(\"oops\")\n";
    let stderr = run_pyrust_stderr(src);
    let (blocks, banners) = split_chain_blocks(&stderr);
    assert_eq!(
        blocks.len(),
        2,
        "expected one chained (context) block + main block, got:\n{stderr}"
    );
    assert_eq!(
        banners,
        vec![CONTEXT_BANNER.to_string()],
        "implicit second exception must print the 'During handling...' banner",
    );
    // Chained (first TypeError) block: <module> 4 -> f 2.
    assert_eq!(
        frame_list(&blocks[0]),
        vec![("<module>".to_string(), 4), ("f".to_string(), 2)],
        "context block frames, got:\n{}",
        blocks[0]
    );
    // Main (second TypeError) block: <module> 6 -> f 2.  Crucially it lists `f`
    // exactly ONCE — the merge bug repeated it.
    assert_eq!(
        frame_list(&blocks[1]),
        vec![("<module>".to_string(), 6), ("f".to_string(), 2)],
        "main block must not inherit the handled exception's frames, got:\n{}",
        blocks[1]
    );
}

#[test]
fn uncaught_from_none_suppresses_chain() {
    // `raise X from None` sets __suppress_context__: NO chained block, NO
    // banner — just the ValueError's own traceback.
    let src = "def g():\n    try:\n        raise IndexError(\"idx\")\n    except IndexError:\n        raise ValueError(\"v\") from None\ng()\n";
    let stderr = run_pyrust_stderr(src);
    let (blocks, banners) = split_chain_blocks(&stderr);
    assert!(banners.is_empty(), "from None must suppress the banner");
    assert_eq!(blocks.len(), 1);
    assert!(
        !stderr.contains("IndexError"),
        "the suppressed context must not appear at all, got:\n{stderr}"
    );
}

#[test]
fn uncaught_three_deep_chain_ordering() {
    // Three-deep implicit chain: IndexError -> KeyError -> ValueError.  CPython
    // prints oldest-first: IndexError block, banner, KeyError block, banner,
    // ValueError block.  Each chained block carries its own traceback frames.
    let src = "def a():\n    raise IndexError(\"first\")\ndef b():\n    try:\n        a()\n    except IndexError:\n        raise KeyError(\"second\")\ndef c():\n    try:\n        b()\n    except KeyError:\n        raise ValueError(\"third\")\nc()\n";
    let stderr = run_pyrust_stderr(src);
    let (blocks, banners) = split_chain_blocks(&stderr);
    assert_eq!(blocks.len(), 3, "two chained blocks + main block");
    assert_eq!(
        banners,
        vec![CONTEXT_BANNER.to_string(), CONTEXT_BANNER.to_string()],
    );
    // Block 0: IndexError (oldest), starts at b 5 -> a 2.
    assert!(blocks[0].trim_end().ends_with("IndexError: first"));
    assert_eq!(frame_list(&blocks[0])[0], ("b".to_string(), 5));
    // Block 1: KeyError, starts at c 10.
    assert!(blocks[1].trim_end().ends_with("KeyError: 'second'"));
    assert_eq!(frame_list(&blocks[1])[0], ("c".to_string(), 10));
    assert!(blocks[1].contains("Traceback (most recent call last):"));
}

#[test]
fn uncaught_chained_without_traceback_omits_block() {
    // A chained exception that was constructed but never raised has no
    // __traceback__; CPython omits its Traceback header and prints only the
    // class line above the banner.
    let src = "def g():\n    ctx = IndexError(\"never raised\")\n    try:\n        raise RuntimeError(\"real\")\n    except RuntimeError as e:\n        e.__context__ = ctx\n        raise\ng()\n";
    let stderr = run_pyrust_stderr(src);
    let (blocks, banners) = split_chain_blocks(&stderr);
    assert_eq!(banners, vec![CONTEXT_BANNER.to_string()]);
    assert_eq!(blocks.len(), 2);
    // Chained block is just the class line — no Traceback header, no frames.
    assert!(
        !blocks[0].contains("Traceback (most recent call last):"),
        "never-raised chained exception must NOT print a traceback header, got:\n{}",
        blocks[0]
    );
    assert!(frame_list(&blocks[0]).is_empty());
    assert!(blocks[0].trim_end().ends_with("IndexError: never raised"));
}

#[test]
fn uncaught_context_cycle_breaks() {
    // A __context__ cycle (e.__context__ is v, v.__context__ is e) must not
    // loop forever: CPython breaks the cycle and prints exactly one chained
    // block.
    let src = "def g():\n    try:\n        raise IndexError(\"idx\")\n    except IndexError as e:\n        v = ValueError(\"v\")\n        v.__context__ = e\n        e.__context__ = v\n        raise v\ng()\n";
    let stderr = run_pyrust_stderr(src);
    let (blocks, banners) = split_chain_blocks(&stderr);
    assert_eq!(
        banners,
        vec![CONTEXT_BANNER.to_string()],
        "exactly one banner"
    );
    assert_eq!(blocks.len(), 2, "cycle must not produce an unbounded chain");
    assert!(blocks[0].trim_end().ends_with("IndexError: idx"));
    assert_eq!(frame_list(&blocks[0])[0], ("g".to_string(), 3));
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

// ---------------------------------------------------------------------------
// Issue #2409: an uncaught `raise <param>` / failing `assert` inside a function
// called from module scope was silently swallowed (process exited 0, no
// traceback) because the body was mis-classified as *pure* and its dead-result
// `CallMemo` was dead-store-eliminated.  The exception never propagated.  These
// assert the exception now escapes with the correct frame list.
// ---------------------------------------------------------------------------

#[test]
fn uncaught_raise_param_from_function() {
    // `def h(e): raise e`, called from module scope with a fresh exception, then
    // uncaught.  CPython 3.12: <module> 3 -> h 2.  Pre-fix: exit 0, no frames.
    let src = "def h(e):\n    raise e\nh(IndexError(\"idx\"))\n";
    let frames = frame_list(&run_pyrust_stderr(src));
    assert_eq!(
        frames,
        vec![("<module>".to_string(), 3), ("h".to_string(), 2)],
        "uncaught `raise <param>` must propagate, not be swallowed (issue #2409)",
    );
}

#[test]
fn uncaught_raise_param_two_levels() {
    // Two function levels deep.  CPython 3.12: <module> 5 -> b 4 -> a 2.
    let src = "def a(e):\n    raise e\ndef b(e):\n    a(e)\nb(IndexError(\"deep\"))\n";
    let frames = frame_list(&run_pyrust_stderr(src));
    assert_eq!(
        frames,
        vec![
            ("<module>".to_string(), 5),
            ("b".to_string(), 4),
            ("a".to_string(), 2),
        ],
        "uncaught `raise <param>` two levels deep must propagate (issue #2409)",
    );
}

#[test]
fn uncaught_raise_param_in_method() {
    // Carried re-raise inside a method.  CPython 3.12: <module> 4 -> m 3.
    let src = "class C:\n    def m(self, e):\n        raise e\nC().m(ValueError(\"boom\"))\n";
    let frames = frame_list(&run_pyrust_stderr(src));
    assert_eq!(
        frames,
        vec![("<module>".to_string(), 4), ("m".to_string(), 3)],
        "uncaught `raise <param>` in a method must propagate (issue #2409)",
    );
}

#[test]
fn uncaught_raise_param_in_generator() {
    // Carried re-raise inside a generator body, surfaced via `next`.
    // CPython 3.12: <module> 6 -> gen 3.
    let src = "def gen(e):\n    yield 1\n    raise e\ng = gen(KeyError(\"k\"))\nnext(g)\nnext(g)\n";
    let frames = frame_list(&run_pyrust_stderr(src));
    assert_eq!(
        frames,
        vec![("<module>".to_string(), 6), ("gen".to_string(), 3)],
        "uncaught `raise <param>` in a generator must propagate (issue #2409)",
    );
}

#[test]
fn uncaught_raise_param_from_within_except() {
    // `raise e` of a carried exception from within an `except` block in a
    // function.  The final (escaping) exception's frames are <module> 6 -> h 5.
    // (CPython additionally prints the handled inner RuntimeError as a
    // `__context__` chain; pyrust's uncaught formatter does not yet render the
    // context block — tracked separately — so we assert only the escaping
    // exception's own frame list, which is what issue #2409 governs.)
    let src = "def h(e):\n    try:\n        raise RuntimeError(\"inner\")\n    except RuntimeError:\n        raise e\nh(ValueError(\"outer\"))\n";
    // Since #2416 the handled RuntimeError's own traceback block (its `h` 3
    // frame) prints above the context banner, exactly like CPython — scope the
    // assertion to the ESCAPING exception's block (after the last header).
    let stderr = run_pyrust_stderr(src);
    let last_block = stderr
        .rsplit("Traceback (most recent call last):")
        .next()
        .unwrap_or(&stderr);
    let frames = frame_list(last_block);
    assert_eq!(
        frames,
        vec![("<module>".to_string(), 6), ("h".to_string(), 5)],
        "uncaught `raise <param>` from within except must propagate (issue #2409)",
    );
}

#[test]
fn uncaught_failing_assert_from_function() {
    // A failing `assert` inside a function called from module scope.  Same
    // pure-misclassification root cause as the `raise` case.
    // CPython 3.12: <module> 3 -> check 2.
    let src = "def check(x):\n    assert x\ncheck(0)\n";
    let frames = frame_list(&run_pyrust_stderr(src));
    assert_eq!(
        frames,
        vec![("<module>".to_string(), 3), ("check".to_string(), 2)],
        "a failing `assert` in a dead-result call must propagate (issue #2409)",
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

// ---------------------------------------------------------------------------
// Issue #2428: every frame of an uncaught traceback echoes its (dedented)
// source line, not just the innermost raise frame.  Previously the captured
// frame snapshot recorded `source_line: None` for non-innermost frames, so the
// formatter printed a `File` header with no source echo under it.
// ---------------------------------------------------------------------------

#[test]
fn uncaught_every_frame_echoes_source_line() {
    // <module> 3 -> f 2.  Both frames must echo their source line.
    let src = "def f():\n    raise ValueError(\"x\")\nf()\n";
    let stderr = run_pyrust_stderr(src);
    assert_eq!(
        frame_source_lines(&stderr),
        vec![
            Some("f()".to_string()),
            Some("raise ValueError(\"x\")".to_string()),
        ],
        "every frame must echo its source line (issue #2428), got:\n{stderr}",
    );
}

#[test]
fn uncaught_deep_chain_each_frame_source() {
    // Four frames: <module> 5 -> f 4 -> g 2.  Each echoes its own line.
    let src = "def g():\n    raise RuntimeError(\"deep\")\ndef f():\n    g()\nf()\n";
    let stderr = run_pyrust_stderr(src);
    assert_eq!(
        frame_source_lines(&stderr),
        vec![
            Some("f()".to_string()),
            Some("g()".to_string()),
            Some("raise RuntimeError(\"deep\")".to_string()),
        ],
        "deep call chain frames must each echo their source line, got:\n{stderr}",
    );
}

#[test]
fn uncaught_recursion_frames_echo_source() {
    // Recursion: each repeated `rec` frame echoes `rec(n - 1)`, the deepest
    // echoes the raise.  Frames: <module> 5 -> rec 4 (x3) -> rec 3.
    let src = "def rec(n):\n    if n == 0:\n        raise ValueError(\"bottom\")\n    rec(n - 1)\nrec(2)\n";
    let stderr = run_pyrust_stderr(src);
    assert_eq!(
        frame_source_lines(&stderr),
        vec![
            Some("rec(2)".to_string()),
            Some("rec(n - 1)".to_string()),
            Some("rec(n - 1)".to_string()),
            Some("raise ValueError(\"bottom\")".to_string()),
        ],
        "recursive frames must each echo their source line, got:\n{stderr}",
    );
}

#[test]
fn uncaught_source_line_is_dedented() {
    // A deeply-indented raise line is echoed dedented (leading whitespace
    // stripped), matching CPython — the formatter re-indents to four spaces.
    let src = "def f():\n    if True:\n        raise ValueError(\"nested\")\nf()\n";
    let stderr = run_pyrust_stderr(src);
    let sources = frame_source_lines(&stderr);
    assert_eq!(
        sources.last().cloned().flatten(),
        Some("raise ValueError(\"nested\")".to_string()),
        "the innermost frame's source must be dedented, got:\n{stderr}",
    );
    // The called frame (`f`) is also present and dedented.
    assert!(
        sources.contains(&Some("f()".to_string())),
        "the module frame must echo `f()`, got:\n{stderr}",
    );
}

#[test]
fn uncaught_chained_blocks_each_frame_echoes_source() {
    // `raise ... from e`: frames in BOTH the cause block and the main block
    // echo their source lines (issue #2428 + #2416).
    let src = "def inner():\n    raise ValueError(\"orig\")\ndef outer():\n    try:\n        inner()\n    except ValueError as e:\n        raise RuntimeError(\"wrapped\") from e\nouter()\n";
    let stderr = run_pyrust_stderr(src);
    let (blocks, banners) = split_chain_blocks(&stderr);
    assert_eq!(banners, vec![CAUSE_BANNER.to_string()]);
    assert_eq!(blocks.len(), 2);
    // Cause block (ValueError): outer 5 -> inner 2.
    assert_eq!(
        frame_source_lines(&blocks[0]),
        vec![
            Some("inner()".to_string()),
            Some("raise ValueError(\"orig\")".to_string()),
        ],
        "cause block frames must echo their source lines, got:\n{}",
        blocks[0]
    );
    // Main block (RuntimeError): <module> 8 -> outer 7.
    assert_eq!(
        frame_source_lines(&blocks[1]),
        vec![
            Some("outer()".to_string()),
            Some("raise RuntimeError(\"wrapped\") from e".to_string()),
        ],
        "main block frames must echo their source lines, got:\n{}",
        blocks[1]
    );
}
