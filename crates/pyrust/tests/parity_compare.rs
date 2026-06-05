use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_and_capture(program: &Path, args: &[&Path]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .map_err(|e| format!("failed to run {}: {e}", program.display()))?;

    let mut merged = String::new();
    merged.push_str(&String::from_utf8_lossy(&output.stdout));
    merged.push_str(&String::from_utf8_lossy(&output.stderr));

    if output.status.success() {
        Ok(merged)
    } else {
        Err(format!(
            "{} exited with status {}\n{}",
            program.display(),
            output
                .status
                .code()
                .map_or_else(|| "<signal>".to_string(), |c| c.to_string()),
            merged
        ))
    }
}

fn find_python_executable(root: &Path) -> PathBuf {
    if let Some(python) = env::var_os("PYRUST_PYTHON") {
        return PathBuf::from(python);
    }

    // Search `root` and its ancestors for a .venv directory.
    let mut dir = root;
    loop {
        let venv_unix = dir.join(".venv").join("bin").join("python");
        if venv_unix.exists() {
            return venv_unix;
        }
        let venv_win = dir.join(".venv").join("Scripts").join("python.exe");
        if venv_win.exists() {
            return venv_win;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => break,
        }
    }

    // Fallback when .venv is not present.  Prefer a versioned `python3.12`
    // executable so that platforms with both 3.11 and 3.12 installed pick
    // the one pyrust's parity tests are written against — some CPython
    // APIs changed slot semantics between versions (`sorted(reverse=…)`,
    // for example, switched from `__index__` in 3.11 to `bool()` in 3.12),
    // and the test fixtures lock in 3.12 behaviour.
    // Windows: prefer `python` over `py` — GitHub Actions and most local
    // setups alias `python` to the `setup-python`-installed interpreter
    // (or the user's PATH-default), while `py` is the Python launcher
    // which picks the highest installed version per `py.ini`, often
    // newer (and behaviour-divergent) than the project's pinned 3.12.
    let candidates: &[&str] = if cfg!(windows) {
        &["python3.12", "python", "py"]
    } else {
        &["python3.12", "python3"]
    };
    for candidate in candidates {
        if Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return PathBuf::from(candidate);
        }
    }
    PathBuf::from(candidates.last().copied().unwrap_or("python3"))
}

/// Confirm `python` is CPython 3.12+ (the slot semantics target).  Emits a
/// warning if not — doesn't fail the test, because the developer may have
/// deliberately pointed `PYRUST_PYTHON` at a different version.
fn warn_if_python_version_off_target(python: &Path) {
    let out = match Command::new(python)
        .args([
            "-c",
            "import sys; print(sys.version_info[0], sys.version_info[1])",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return,
    };
    let raw = String::from_utf8_lossy(&out);
    let mut parts = raw.split_whitespace();
    let major: u32 = match parts.next().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None => return,
    };
    let minor: u32 = match parts.next().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None => return,
    };
    if (major, minor) < (3, 12) {
        eprintln!(
            "WARNING: parity test is running CPython {major}.{minor}, but pyrust \
             pins behaviour to 3.12+ (see `.python-version`).  Some test \
             fixtures may report spurious mismatches.  Override with \
             PYRUST_PYTHON=/path/to/python3.12 or add 3.12 to a local .venv."
        );
    }
}

/// Return true if `line` looks like a CPython warning header of the form:
///   <file>:<lineno>: SomeWarning: <message>
/// Detected by requiring "Warning:" in the line and a ":<digits>" in the prefix.
fn is_cpython_warning_header(line: &str) -> bool {
    let Some(warn_pos) = line.find("Warning:") else {
        return false;
    };
    let prefix = &line[..warn_pos];
    let mut chars = prefix.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek().is_some_and(|d| d.is_ascii_digit()) {
            return true;
        }
    }
    false
}

fn normalize_pythonish_output(raw: &str) -> String {
    // CPython emits SyntaxWarnings to stderr at compile time for unrecognized
    // escape sequences and similar issues.  The harness merges stdout+stderr, so
    // these lines appear in CPython output but not pyrust output.  Strip:
    //   <file>:<lineno>: SyntaxWarning: <message>
    //     <source context line>   (indented, follows the header)
    let mut skip_context = false;
    let mut out: Vec<&str> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if skip_context {
            // The source-context line that follows a warning header is indented.
            if line.starts_with(' ') || line.starts_with('\t') {
                continue;
            }
            skip_context = false;
        }
        if trimmed.starts_with("Traceback (most recent call last):")
            || trimmed.starts_with("File \"")
            || is_cpython_warning_header(trimmed)
        {
            if is_cpython_warning_header(trimmed) {
                skip_context = true;
            }
            continue;
        }
        // Strip PEP 657 underline markers (`^` and `~` lines).  CPython emits
        // precise column markers (e.g. `    ~~^^~~~`) while pyrust emits a
        // simpler full-width `^` underline.  Strip both so the parity diff
        // focuses on the exception message and echoed source line only.
        if !trimmed.is_empty() && trimmed.chars().all(|c| c == '^' || c == '~') {
            continue;
        }
        out.push(line.trim_end());
    }
    out.join("\n")
}

fn collect_test_scripts(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_test_scripts(&path, out);
            continue;
        }

        if let Some(name) = path.file_name().and_then(|s| s.to_str())
            && name.starts_with("test_")
            && name.ends_with(".py")
        {
            out.push(path);
        }
    }
}

#[test]
fn compare_against_python_reference_for_all_py_tests() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tests_dir = root.join("tests");
    let cases_dir = tests_dir.join("cases");

    let pyrust_bin = PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    );
    let python = find_python_executable(&root);
    warn_if_python_version_off_target(&python);

    let mut scripts: Vec<PathBuf> = Vec::new();
    collect_test_scripts(&cases_dir, &mut scripts);

    scripts.sort();
    assert!(!scripts.is_empty(), "no test_*.py found under tests/cases/");

    let mut failures: Vec<String> = Vec::new();

    for script in scripts {
        let name = script
            .strip_prefix(&root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("<unknown>");

        let py_out = match run_and_capture(&python, &[script.as_path()]) {
            Ok(out) => out,
            Err(err) => {
                failures.push(format!("[{name}] Python run failed:\n{err}"));
                continue;
            }
        };

        let rust_out = match run_and_capture(&pyrust_bin, &[script.as_path()]) {
            Ok(out) => out,
            Err(err) => {
                failures.push(format!("[{name}] PyRust run failed:\n{err}"));
                continue;
            }
        };

        let py_norm = normalize_pythonish_output(&py_out);
        let rust_norm = normalize_pythonish_output(&rust_out);

        if py_norm != rust_norm {
            failures.push(format!(
                "[{name}] output mismatch\n--- Python (normalized) ---\n{}\n--- PyRust (normalized) ---\n{}\n\n--- Python (raw) ---\n{}\n--- PyRust (raw) ---\n{}",
                py_norm, rust_norm, py_out, rust_out
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "Python parity failed for {} file(s):\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }
}
