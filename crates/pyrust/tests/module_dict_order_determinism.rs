//! Issue #2918: a built-in module's `__dict__` must iterate in the same order
//! in every process.
//!
//! `PyModule::attrs` used to be a `HashMap`, whose iteration order depends on
//! the per-process `RandomState` seed, so `list(vars(math))` differed run to
//! run. Insertion-ordered storage makes it the module's declaration order.
//!
//! This has to spawn the real binary repeatedly: the defect was *cross-process*
//! nondeterminism, which is invisible to any in-process assertion (a `HashMap`
//! iterates consistently within one process) and to the parity harness, which
//! runs each fixture once. The companion parity fixture
//! `tests/cases/stdlib/test_module_dict_order.py` pins the CPython-matching
//! shape of that order.

use std::env;
use std::path::PathBuf;
use std::process::Command;

/// Every module declared by `pyrust_builtin_modules!`, so that reintroducing
/// hash-ordered storage anywhere is caught. Keep this list in step with that
/// macro — a module missing here is a module whose namespace order is unpinned.
///
/// The list deliberately spans all the distinct namespace kinds: `math` keeps
/// the compact direct `attrs` storage and synthesises `__dict__`; `sys` moves
/// its namespace into a live dict (`attach_live_namespace`); `builtins` is
/// composed from several sub-module attr maps *plus* the exception-class table,
/// which was a second, separate hash-ordered source. The dotted names
/// (`os.path`, `collections.abc`) are bound through their parent package rather
/// than `__import__`, which returns the top-level module for a dotted name.
const PROBE: &str = r#"
import math
import os.path
import collections.abc
import __future__
names = ["abc", "asyncio", "bisect", "builtins", "collections", "contextlib",
         "copy", "csv", "dataclasses", "decimal", "enum", "errno",
         "fractions", "functools", "heapq", "io", "itertools", "json", "math",
         "operator", "os", "pathlib", "pprint", "random", "re", "statistics",
         "string", "sys", "textwrap", "time", "types", "typing", "warnings"]
for name in names:
    print(name, list(vars(__import__(name))))
for name, module in [("os.path", os.path),
                     ("collections.abc", collections.abc),
                     ("__future__", __future__)]:
    print(name, list(vars(module)))
print(list(math.__dict__))
"#;

const STAR_IMPORT_PROBE: &str = r#"
from math import *
print([name for name in globals() if not name.startswith('__')])
"#;

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

fn write_probe(name: &str, source: &str) -> PathBuf {
    let mut path = env::temp_dir();
    path.push(format!("pyrust_{}_{}.py", name, std::process::id()));
    std::fs::write(&path, source).expect("write probe script");
    path
}

fn run_probe(path: &PathBuf) -> String {
    let output = Command::new(pyrust_bin())
        .arg(path)
        .output()
        .expect("run pyrust");
    assert!(
        output.status.success(),
        "probe script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("probe output is utf-8")
}

/// Five separate processes, each with its own hash seed, must agree exactly.
fn assert_stable_across_runs(name: &str, source: &str) {
    let path = write_probe(name, source);
    let first = run_probe(&path);
    assert!(
        first.lines().count() >= 1 && !first.trim().is_empty(),
        "{name}: probe produced no listing"
    );
    for run in 2..=5 {
        let again = run_probe(&path);
        assert_eq!(
            first, again,
            "{name}: module namespace iteration order changed between run 1 and run {run}; \
             the backing storage must be insertion ordered, not hash ordered"
        );
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn builtin_module_dict_order_is_identical_across_processes() {
    assert_stable_across_runs("module_dict_order", PROBE);
}

#[test]
fn star_import_binding_order_is_identical_across_processes() {
    assert_stable_across_runs("star_import_order", STAR_IMPORT_PROBE);
}
