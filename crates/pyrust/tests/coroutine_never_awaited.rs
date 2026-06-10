//! `RuntimeWarning: coroutine '<name>' was never awaited` (issue #2306).
//!
//! When a coroutine object is dropped without ever having been awaited/driven,
//! pyrust emits CPython's never-awaited RuntimeWarning to stderr from
//! `GeneratorFrame::drop`. This is NOT a parity-fixture test: CPython's exact
//! shape depends on GC timing (`sys:1:` at interpreter shutdown vs a
//! file:line + source-line + tracemalloc form on immediate `del`), which pyrust
//! cannot reproduce deterministically because it lacks a mid-program object
//! finaliser. We instead assert the warning is emitted with the correct text
//! when the backing object actually drops (here, an explicit `del`).

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `src` to a temp .py file, run pyrust on it, and return (stdout, stderr).
fn run_pyrust(src: &str) -> (String, String) {
    let mut path = env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    path.push(format!(
        "pyrust_never_awaited_{}_{}.py",
        std::process::id(),
        n
    ));
    {
        let mut f = std::fs::File::create(&path).expect("create temp script");
        f.write_all(src.as_bytes()).expect("write temp script");
    }
    let output = Command::new(pyrust_bin())
        .arg(&path)
        .output()
        .expect("run pyrust");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn never_awaited_coroutine_warns_on_drop() {
    let (stdout, stderr) =
        run_pyrust("async def f():\n    pass\n\nc = f()\ndel c\nprint('after del')\n");
    assert!(
        stdout.contains("after del"),
        "program should run to completion; stdout was: {stdout:?}"
    );
    assert!(
        stderr.contains("RuntimeWarning: coroutine 'f' was never awaited"),
        "expected never-awaited warning on stderr; stderr was: {stderr:?}"
    );
}

#[test]
fn awaited_coroutine_does_not_warn() {
    // A coroutine driven to completion by asyncio.run must NOT warn.
    let (stdout, stderr) =
        run_pyrust("import asyncio\n\nasync def f():\n    return 42\n\nprint(asyncio.run(f()))\n");
    assert!(stdout.contains("42"), "stdout was: {stdout:?}");
    assert!(
        !stderr.contains("was never awaited"),
        "a fully-awaited coroutine must not warn; stderr was: {stderr:?}"
    );
}

#[test]
fn plain_generator_does_not_warn() {
    // A never-iterated plain generator is fine (the warning is coroutine-only).
    let (_stdout, stderr) = run_pyrust("def g():\n    yield 1\n\nx = g()\ndel x\n");
    assert!(
        !stderr.contains("was never awaited"),
        "a generator must not emit the coroutine warning; stderr was: {stderr:?}"
    );
}

#[test]
fn async_generator_does_not_warn() {
    // A never-iterated async generator is not a coroutine; no warning.
    let (_stdout, stderr) =
        run_pyrust("async def ag():\n    yield 1\n\na = ag()\ndel a\nprint('ok')\n");
    assert!(
        !stderr.contains("was never awaited"),
        "an async generator must not emit the coroutine warning; stderr was: {stderr:?}"
    );
}
