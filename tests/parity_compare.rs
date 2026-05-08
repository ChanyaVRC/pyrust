use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_and_capture(program: &Path, args: &[&Path]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
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

    let venv_python = root.join(".venv").join("bin").join("python");
    if venv_python.exists() {
        return venv_python;
    }

    let venv_python_windows = root.join(".venv").join("Scripts").join("python.exe");
    if venv_python_windows.exists() {
        return venv_python_windows;
    }

    // Fallback when .venv is not present.
    if cfg!(windows) {
        PathBuf::from("python")
    } else {
        PathBuf::from("python3")
    }
}

fn normalize_pythonish_output(raw: &str) -> String {
    raw.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("Traceback (most recent call last):")
                && !trimmed.starts_with("File \"")
        })
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
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

        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if name.starts_with("test_") && name.ends_with(".py") {
                out.push(path);
            }
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
