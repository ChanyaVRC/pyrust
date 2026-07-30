use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

/// Address-space cap for each interpreter child (bytes).  An adversarial
/// fixture or repro must never be able to out-allocate the host: a runaway
/// interpreter (unbounded native recursion, allocation storms) has taken down
/// the whole WSL2 VM.  Override with PYRUST_PARITY_MEM_MB when a fixture
/// legitimately needs more.
#[cfg(unix)]
fn child_address_space_limit() -> u64 {
    let mb = env::var("PYRUST_PARITY_MEM_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(4096);
    mb * 1024 * 1024
}

/// Wall-clock cap for each interpreter child (seconds).  Override with
/// PYRUST_PARITY_TIMEOUT_S.
fn child_timeout_secs() -> u64 {
    env::var("PYRUST_PARITY_TIMEOUT_S")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(120)
}

/// Cap on how much of one child pipe the harness keeps in memory.  The rlimits
/// above bound each *child*; this bounds the harness process, which runs
/// uncapped — a fixture stuck in a `print` loop would otherwise stream
/// gigabytes into this process before the wall-clock kill fires, which is the
/// same host-exhaustion failure the caps exist to prevent.
const MAX_CAPTURED_BYTES: u64 = 64 * 1024 * 1024;

/// How long to wait for a drain thread to hand over its buffer once the child
/// is gone.  Bounded rather than a plain `join` so that a grandchild holding
/// the pipe open cannot hang the harness: a truncated capture surfaces as a
/// loud parity mismatch, a hang surfaces as nothing at all.
const DRAIN_HANDOVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Drain one child pipe on a helper thread, so the wait loop below never
/// blocks on a child that has filled its pipe buffer.
fn drain_pipe<R: std::io::Read + Send + 'static>(mut pipe: R) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = (&mut pipe).take(MAX_CAPTURED_BYTES).read_to_end(&mut buf);
        if buf.len() as u64 >= MAX_CAPTURED_BYTES {
            buf.extend_from_slice(
                format!(
                    "\n<parity harness: output truncated at {} MiB>\n",
                    MAX_CAPTURED_BYTES / (1024 * 1024)
                )
                .as_bytes(),
            );
            // Keep draining into the void so the child never blocks on a full
            // pipe; the wall-clock kill is what stops it.
            let _ = std::io::copy(&mut pipe, &mut std::io::sink());
        }
        let _ = tx.send(buf);
    });
    rx
}

fn collect_drained(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    rx.recv_timeout(DRAIN_HANDOVER_TIMEOUT).unwrap_or_default()
}

/// The tail of a killed child's output — the only clue about where a hanging
/// fixture got stuck.  Capped so a chatty child cannot bury the report.
fn output_tail(stdout: &[u8], stderr: &[u8]) -> String {
    const TAIL: usize = 20;
    let mut merged = String::from_utf8_lossy(stdout).into_owned();
    merged.push_str(&String::from_utf8_lossy(stderr));
    let lines: Vec<&str> = merged.lines().collect();
    if lines.is_empty() {
        return "(no output before the kill)".to_string();
    }
    let start = lines.len().saturating_sub(TAIL);
    let mut out = format!(
        "--- last {} of {} captured output line(s) before the kill ---\n",
        lines.len() - start,
        lines.len()
    );
    out.push_str(&lines[start..].join("\n"));
    out
}

fn run_and_capture(program: &Path, args: &[&Path]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args).env("PYTHONIOENCODING", "utf-8");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let limit = child_address_space_limit();
        // SAFETY: the closure runs between fork and exec, where only
        // async-signal-safe work is allowed.  `setrlimit` is not on the POSIX
        // async-signal-safe list, but the libc entry point is a bare syscall
        // wrapper on both glibc and musl: it allocates nothing, takes no lock,
        // and touches no state the parent could have been holding at fork.
        unsafe {
            command.pre_exec(move || {
                let rlim = libc::rlimit {
                    rlim_cur: limit as libc::rlim_t,
                    rlim_max: limit as libc::rlim_t,
                };
                libc::setrlimit(libc::RLIMIT_AS, &rlim);
                libc::setrlimit(libc::RLIMIT_DATA, &rlim);
                Ok(())
            });
        }
    }
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run {}: {e}", program.display()))?;

    // Drain the pipes on threads so a chatty child cannot dead-lock against
    // the wait loop below.
    let stdout_rx = drain_pipe(child.stdout.take().expect("stdout piped"));
    let stderr_rx = drain_pipe(child.stderr.take().expect("stderr piped"));

    // Poll with a short backoff that grows to 10 ms: a typical fixture exits in
    // single-digit milliseconds, and a fixed 10 ms tick would spend most of that
    // asleep across ~3k child runs.
    let timeout_secs = child_timeout_secs();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut poll = std::time::Duration::from_micros(200);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{} timed out after {timeout_secs}s (killed; raise PYRUST_PARITY_TIMEOUT_S if legitimate)\n{}",
                        program.display(),
                        output_tail(&collect_drained(&stdout_rx), &collect_drained(&stderr_rx))
                    ));
                }
                thread::sleep(poll);
                poll = (poll * 2).min(std::time::Duration::from_millis(10));
            }
            Err(e) => return Err(format!("failed to wait on {}: {e}", program.display())),
        }
    };
    let stdout = collect_drained(&stdout_rx);
    let stderr = collect_drained(&stderr_rx);
    let output = std::process::Output {
        status,
        stdout,
        stderr,
    };

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

/// Focused mismatch report: the first divergent (normalized) line with ±3
/// lines of context from both interpreters, so the signal is not buried in two
/// full transcripts.  Capped raw transcripts follow for orientation.
fn mismatch_report(name: &str, py_norm: &str, rust_norm: &str) -> String {
    const CONTEXT: usize = 3;
    const RAW_CAP: usize = 40;

    let py_lines: Vec<&str> = py_norm.lines().collect();
    let rust_lines: Vec<&str> = rust_norm.lines().collect();
    let common = py_lines.len().min(rust_lines.len());
    let divergence = (0..common)
        .find(|&i| py_lines[i] != rust_lines[i])
        .unwrap_or(common);
    let start = divergence.saturating_sub(CONTEXT);

    let mut out = format!(
        "[{name}] output mismatch (first divergence at normalized line {}; \
         Python {} lines, PyRust {} lines)\n",
        divergence + 1,
        py_lines.len(),
        rust_lines.len()
    );
    let side = |label: &str, lines: &[&str]| {
        let mut s = format!("--- {label} around divergence ---\n");
        for (offset, line) in lines
            .iter()
            .enumerate()
            .skip(start)
            .take(CONTEXT * 2 + 1)
            .map(|(i, l)| (i, *l))
        {
            let marker = if offset == divergence { ">>" } else { "  " };
            s.push_str(&format!("{marker} {:>4} | {line}\n", offset + 1));
        }
        if divergence >= lines.len() {
            s.push_str(&format!(">> {:>4} | <end of output>\n", lines.len() + 1));
        }
        s
    };
    out.push_str(&side("Python", &py_lines));
    out.push_str(&side("PyRust", &rust_lines));

    let capped = |label: &str, text: &str| {
        let lines: Vec<&str> = text.lines().collect();
        let shown = lines.len().min(RAW_CAP);
        let mut s = format!("--- {label} (normalized, first {shown} lines) ---\n");
        s.push_str(&lines[..shown].join("\n"));
        if lines.len() > RAW_CAP {
            s.push_str(&format!("\n… {} more lines", lines.len() - RAW_CAP));
        }
        s.push('\n');
        s
    };
    out.push_str(&capped("Python", py_norm));
    out.push_str(&capped("PyRust", rust_norm));
    out.push_str(&format!(
        "re-run just this fixture: PYRUST_PARITY_FILTER={} cargo test --release --test parity_compare\n",
        Path::new(name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(name)
    ));
    out
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

    // PYRUST_PARITY_FILTER narrows the run to fixtures whose repo-relative
    // path contains the given substring — the per-fixture debugging loop.
    if let Ok(filter) = env::var("PYRUST_PARITY_FILTER") {
        scripts.retain(|p| p.to_str().is_some_and(|s| s.contains(&filter)));
        assert!(
            !scripts.is_empty(),
            "PYRUST_PARITY_FILTER={filter} matched no fixture under tests/cases/"
        );
    }

    // Fixtures are independent processes: run them across a worker pool.
    // The CPython and pyrust runs of one fixture stay sequential on one
    // worker, so fixtures that create fixture-named scratch files keep their
    // existing single-run file discipline.  PYRUST_PARITY_JOBS=1 restores the
    // serial order.
    let jobs = env::var("PYRUST_PARITY_JOBS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or_else(|| thread::available_parallelism().map_or(1, |n| n.get()))
        .clamp(1, 32)
        .min(scripts.len());

    let next = AtomicUsize::new(0);
    let failures: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(script) = scripts.get(index) else {
                        break;
                    };
                    let name = script
                        .strip_prefix(&root)
                        .ok()
                        .and_then(|p| p.to_str())
                        .unwrap_or("<unknown>")
                        .to_string();

                    let py_out = match run_and_capture(&python, &[script.as_path()]) {
                        Ok(out) => out,
                        Err(err) => {
                            failures.lock().unwrap().push((
                                name.clone(),
                                format!("[{name}] Python run failed:\n{err}"),
                            ));
                            continue;
                        }
                    };
                    let rust_out = match run_and_capture(&pyrust_bin, &[script.as_path()]) {
                        Ok(out) => out,
                        Err(err) => {
                            failures.lock().unwrap().push((
                                name.clone(),
                                format!("[{name}] PyRust run failed:\n{err}"),
                            ));
                            continue;
                        }
                    };

                    let py_norm = normalize_pythonish_output(&py_out);
                    let rust_norm = normalize_pythonish_output(&rust_out);
                    if py_norm != rust_norm {
                        let report = mismatch_report(&name, &py_norm, &rust_norm);
                        failures.lock().unwrap().push((name, report));
                    }
                }
            });
        }
    });

    let mut failures = failures.into_inner().unwrap();
    failures.sort();
    if !failures.is_empty() {
        panic!(
            "Python parity failed for {} file(s):\n\n{}",
            failures.len(),
            failures
                .iter()
                .map(|(_, report)| report.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        );
    }
}
