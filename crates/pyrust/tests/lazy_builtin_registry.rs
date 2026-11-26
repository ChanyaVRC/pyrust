//! Regression tests for lazy per-module builtin registration (issue #3291).
//!
//! A module's native callables live in a constant `REGS` slice that the
//! registry reads only when one of them is first looked up.  Each test spawns
//! a fresh `pyrust` process with `PYRUST_REGISTRY_STATS` set, which makes the
//! CLI report on exit which modules it registered, and checks that:
//!
//! 1. Start-up registers nothing beyond the always-available tier
//!    (`builtins` + the shared `_structseq` hooks).
//! 2. Using a module registers that module and no other.
//! 3. Importing a module without calling into it registers nothing.

use std::env;
use std::path::PathBuf;
use std::process::Command;

const STATS_ENV_VAR: &str = "PYRUST_REGISTRY_STATS";
const STATS_PREFIX: &str = "pyrust-registry: ";

/// Standard-library modules a start-up has no reason to register.
const UNUSED_STDLIB: [&str; 12] = [
    "asyncio",
    "collections",
    "decimal",
    "email",
    "http",
    "itertools",
    "json",
    "math",
    "sqlite3",
    "ssl",
    "xml",
    "zlib",
];

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

/// What one `pyrust` run reported about its registry use.
#[derive(Debug)]
struct RegistryReport {
    stdout: String,
    tier1_callables: usize,
    lazy_modules: usize,
    lazy_callables: usize,
    known_modules: usize,
    known_callables: usize,
    modules: Vec<String>,
}

impl RegistryReport {
    /// Whether `module`, or one of its submodules, was registered.
    fn registered(&self, module: &str) -> bool {
        self.modules.iter().any(|name| {
            name.strip_prefix(module)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
    }
}

fn run_with_stats(args: &[&str]) -> RegistryReport {
    let output = Command::new(pyrust_bin())
        .args(args)
        .env(STATS_ENV_VAR, "1")
        .output()
        .expect("run pyrust");
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    assert!(
        output.status.success(),
        "pyrust {args:?} failed (status {:?}):\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    let line = stderr
        .lines()
        .find_map(|line| line.strip_prefix(STATS_PREFIX))
        .unwrap_or_else(|| panic!("pyrust {args:?} printed no registry report:\n{stderr}"));

    let field = |key: &str| -> &str {
        line.split(' ')
            .find_map(|part| part.strip_prefix(key)?.strip_prefix('='))
            .unwrap_or_else(|| panic!("registry report lacks `{key}`: {line}"))
    };
    let count = |key: &str| -> usize {
        field(key)
            .parse()
            .unwrap_or_else(|_| panic!("`{key}` is not a count: {line}"))
    };
    let modules = field("modules")
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    RegistryReport {
        stdout,
        tier1_callables: count("tier1_callables"),
        lazy_modules: count("lazy_modules"),
        lazy_callables: count("lazy_callables"),
        known_modules: count("known_modules"),
        known_callables: count("known_callables"),
        modules,
    }
}

#[test]
fn bare_startup_registers_only_the_always_available_tier() {
    let report = run_with_stats(&["-S", "-c", "pass"]);
    assert_eq!(
        (report.lazy_modules, report.lazy_callables),
        (0, 0),
        "`pyrust -S -c pass` must not register any module: {report:?}"
    );
    assert!(report.modules.is_empty(), "{report:?}");
    // The always-available tier is `builtins` plus two `_structseq` hooks:
    // a few hundred callables, not the thousands the whole stdlib declares.
    assert!(
        (100..500).contains(&report.tier1_callables),
        "unexpected tier-1 size: {report:?}"
    );
    assert!(
        report.known_modules > 100 && report.known_callables > 2000,
        "the lazy tier should hold the rest of the stdlib: {report:?}"
    );
}

#[test]
fn site_startup_registers_no_unused_stdlib_module() {
    // With `site` enabled the interpreter does call into a few modules
    // (`os`, `os.path`, …); it must still leave the rest untouched.
    let report = run_with_stats(&["-c", "pass"]);
    for module in UNUSED_STDLIB {
        assert!(
            !report.registered(module),
            "start-up registered unused module `{module}`: {report:?}"
        );
    }
    assert!(
        report.lazy_modules < 20,
        "start-up registered too much of the stdlib: {report:?}"
    );
}

#[test]
fn importing_a_module_registers_nothing_until_it_is_called() {
    let report = run_with_stats(&[
        "-S",
        "-c",
        "import math, cmath, itertools\nprint(math.pi > 3, itertools.chain is not None)",
    ]);
    assert_eq!(report.stdout, "True True\n");
    assert!(
        report.modules.is_empty(),
        "importing a module must not read its registrations: {report:?}"
    );
}

#[test]
fn calling_a_module_function_registers_only_that_module() {
    let report = run_with_stats(&[
        "-S",
        "-c",
        "import math, cmath, itertools\nprint(math.sqrt(9.0), math.gcd(12, 18))",
    ]);
    assert_eq!(report.stdout, "3.0 6\n");
    assert_eq!(
        report.modules,
        ["math"],
        "only the module that was called may register"
    );
    assert_eq!(report.lazy_modules, 1);
    assert!(
        report.lazy_callables > 0 && report.lazy_callables * 10 < report.known_callables,
        "`math` is a small part of the stdlib: {report:?}"
    );
}

#[test]
fn native_class_methods_and_descriptors_register_their_module() {
    let report = run_with_stats(&[
        "-S",
        "-c",
        "import itertools\n\
         c = itertools.chain([1], [2])\n\
         print(next(c), list(c), type(c).__next__.__name__)",
    ]);
    assert_eq!(report.stdout, "1 [2] __next__\n");
    assert_eq!(report.modules, ["itertools"]);
}

#[test]
fn registered_callables_keep_their_identity_and_introspection() {
    let report = run_with_stats(&[
        "-S",
        "-c",
        "import math\n\
         from math import sqrt\n\
         print(sqrt is math.sqrt, sqrt == math.sqrt)\n\
         print(sqrt.__name__, sqrt.__qualname__, sqrt.__module__)\n\
         print(repr(sqrt), repr(len))",
    ]);
    assert_eq!(
        report.stdout,
        "True True\nsqrt sqrt math\n<built-in function sqrt> <built-in function len>\n"
    );
    assert_eq!(report.modules, ["math"]);
}

#[test]
fn the_registry_report_is_opt_in() {
    let output = Command::new(pyrust_bin())
        .args(["-S", "-c", "import math; print(math.sqrt(4.0))"])
        .env_remove(STATS_ENV_VAR)
        .output()
        .expect("run pyrust");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        "2.0\n"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains(STATS_PREFIX),
        "the report must not be printed unless {STATS_ENV_VAR} is set"
    );
}
