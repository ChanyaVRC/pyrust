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
