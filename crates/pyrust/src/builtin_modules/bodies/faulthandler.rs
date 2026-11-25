// `faulthandler` module — 100% native Rust implementation of Python 3.12's
// `faulthandler` stdlib module (issue #3246).
//
// Provides `enable`, `disable`, `is_enabled`, `dump_traceback`,
// `dump_traceback_later`, `cancel_dump_traceback_later`, and (under
// `#[cfg(unix)]`) `register` / `unregister`.
//
// Reference: <https://docs.python.org/3.12/library/faulthandler.html>

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyBool, PyNumber};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{Value, ValueKind};

const MAX_TIMEOUT_SECS: f64 = 1e9;

fn current_thread_ident_u64() -> u64 {
    let mut hasher = DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    let raw = hasher.finish() & 0x7fff_ffff_ffff_ffff;
    if raw == 0 { 1 } else { raw }
}

/// Resolve and validate the `file` argument (`sys.stderr` if omitted/`None`,
/// non-negative integer fd, or file object supporting `.fileno()`).
enum ResolvedFile {
    /// Raw file descriptor (0/1/2 or platform fd).
    Fd(i64),
    /// Python file object whose `.fileno()` succeeded.
    PyFile(Value, i64),
}

fn resolve_file_arg(interp: &mut Interpreter, file: OptValueRef<'_>) -> Result<ResolvedFile> {
    let target = match file.as_value() {
        None => {
            let sys_mod = interp.load_module("sys")?;
            let stderr = interp.get_attr(&sys_mod, "stderr")?;
            if stderr.is_none() {
                return Err(PyError::named("RuntimeError", "sys.stderr is None"));
            }
            stderr
        }
        Some(v) if v.is_none() => {
            let sys_mod = interp.load_module("sys")?;
            let stderr = interp.get_attr(&sys_mod, "stderr")?;
            if stderr.is_none() {
                return Err(PyError::named("RuntimeError", "sys.stderr is None"));
            }
            stderr
        }
        Some(v) => v.clone(),
    };

    // Integer file descriptor (note: bool is NOT accepted as an fd in CPython).
    if let ValueKind::Int(fd) = target.kind() {
        if fd < 0 {
            return Err(PyError::named(
                "ValueError",
                "file is not a valid file descriptor",
            ));
        }
        return Ok(ResolvedFile::Fd(fd));
    }
    if matches!(target.kind(), ValueKind::Bool(_)) {
        return Err(PyError::named(
            "TypeError",
            "argument must be an int, or have a fileno() method.",
        ));
    }

    let fileno_fn = interp.get_attr(&target, "fileno").map_err(|_| {
        PyError::named(
            "TypeError",
            "argument must be an int, or have a fileno() method.",
        )
    })?;
    let fd_val = interp.call_function_expanded(fileno_fn, &[])?;
    let fd = match fd_val.kind() {
        ValueKind::Int(n) => n,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "fileno() returned a non-integer (type {})",
                    value_type_name_str(&fd_val)
                ),
            ));
        }
    };
    if fd < 0 {
        return Err(PyError::named(
            "ValueError",
            "file is not a valid file descriptor",
        ));
    }
    Ok(ResolvedFile::PyFile(target, fd))
}

/// Format the live PyRust call stack in CPython's `faulthandler` format:
///
/// ```text
/// Current thread 0x... (most recent call first):
///   File "...", line N in funcname
/// ```
fn format_live_traceback(
    interp: &mut Interpreter,
    header_prefix: &str,
    all_threads: bool,
) -> String {
    let tid = current_thread_ident_u64();
    let mut out = if all_threads {
        format!("{header_prefix}Current thread 0x{tid:016x} (most recent call first):\n")
    } else {
        format!("{header_prefix}Stack (most recent call first):\n")
    };

    let mut depth = 0usize;
    let cur_line = pyrust_core::get_current_vm_line() as i64;
    let mut frames_written = 0usize;
    while frames_written < 100 {
        let frame = interp.build_frame_object(depth, cur_line);
        if frame.is_none() {
            break;
        }
        let lineno = interp
            .get_attr(&frame, "f_lineno")
            .ok()
            .and_then(|v| v.as_int())
            .unwrap_or(0);
        let (filename, funcname) = match interp.get_attr(&frame, "f_code") {
            Ok(code) => {
                let fname = interp
                    .get_attr(&code, "co_filename")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "<unknown>".to_string());
                let cname = interp
                    .get_attr(&code, "co_name")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "<module>".to_string());
                (fname, cname)
            }
            Err(_) => ("<unknown>".to_string(), "<module>".to_string()),
        };
        out.push_str(&format!(
            "  File \"{filename}\", line {lineno} in {funcname}\n"
        ));
        depth += 1;
        frames_written += 1;
    }
    if frames_written == 0 {
        out.push_str("  <no Python frame>\n");
    }
    out
}

fn write_to_fd(fd: i64, text: &str) {
    match fd {
        1 => {
            let _ = std::io::stdout().write_all(text.as_bytes());
            let _ = std::io::stdout().flush();
        }
        2 => {
            let _ = std::io::stderr().write_all(text.as_bytes());
            let _ = std::io::stderr().flush();
        }
        #[cfg(unix)]
        other => {
            // SAFETY: caller validated `other >= 0`; `libc::write` handles bad
            // fds safely by returning -1 (EBADF).
            unsafe {
                let _ = libc::write(
                    other as libc::c_int,
                    text.as_ptr() as *const libc::c_void,
                    text.len(),
                );
            }
        }
        #[cfg(not(unix))]
        _ => {
            let _ = std::io::stderr().write_all(text.as_bytes());
            let _ = std::io::stderr().flush();
        }
    }
}

fn write_resolved_file(interp: &mut Interpreter, target: &ResolvedFile, text: &str) -> Result<()> {
    match target {
        ResolvedFile::Fd(fd) => {
            write_to_fd(*fd, text);
            Ok(())
        }
        ResolvedFile::PyFile(file_obj, fd) => {
            if let Ok(flush_fn) = interp.get_attr(file_obj, "flush") {
                let _ = interp.call_function_expanded(flush_fn, &[]);
            }
            if let Ok(write_fn) = interp.get_attr(file_obj, "write") {
                interp.call_function_expanded(
                    write_fn,
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(text),
                    }],
                )?;
                if let Ok(flush_fn) = interp.get_attr(file_obj, "flush") {
                    let _ = interp.call_function_expanded(flush_fn, &[]);
                }
                Ok(())
            } else {
                write_to_fd(*fd, text);
                Ok(())
            }
        }
    }
}

fn cancel_active_timer(interp: &mut Interpreter) {
    let prev = interp
        .cold
        .faulthandler_state
        .borrow_mut()
        .later_cancel
        .take();
    if let Some(pair) = prev {
        let (lock, cvar) = &*pair;
        if let Ok(mut cancelled) = lock.lock() {
            *cancelled = true;
            cvar.notify_all();
        }
    }
}

/// Optional background output target safe to move into the watchdog thread.
enum BackgroundTarget {
    StdErr,
    StdOut,
    FilePath(String),
    #[cfg(unix)]
    DupFd(i32),
}

impl BackgroundTarget {
    fn write_text(&self, text: &str) {
        match self {
            BackgroundTarget::StdErr => {
                let _ = std::io::stderr().write_all(text.as_bytes());
                let _ = std::io::stderr().flush();
            }
            BackgroundTarget::StdOut => {
                let _ = std::io::stdout().write_all(text.as_bytes());
                let _ = std::io::stdout().flush();
            }
            BackgroundTarget::FilePath(path) => {
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                    let _ = f.write_all(text.as_bytes());
                    let _ = f.flush();
                }
            }
            #[cfg(unix)]
            BackgroundTarget::DupFd(fd) => unsafe {
                let _ = libc::write(*fd, text.as_ptr() as *const libc::c_void, text.len());
            },
        }
    }
}

impl Drop for BackgroundTarget {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let BackgroundTarget::DupFd(fd) = *self {
            unsafe {
                let _ = libc::close(fd);
            }
        }
    }
}

fn make_background_target(interp: &mut Interpreter, resolved: &ResolvedFile) -> BackgroundTarget {
    match resolved {
        ResolvedFile::Fd(1) => BackgroundTarget::StdOut,
        ResolvedFile::Fd(2) => BackgroundTarget::StdErr,
        #[cfg(unix)]
        ResolvedFile::Fd(fd) => {
            let dup = unsafe { libc::dup(*fd as libc::c_int) };
            if dup >= 0 {
                BackgroundTarget::DupFd(dup)
            } else {
                BackgroundTarget::StdErr
            }
        }
        #[cfg(not(unix))]
        ResolvedFile::Fd(_) => BackgroundTarget::StdErr,
        ResolvedFile::PyFile(file_obj, fd) => {
            if *fd == 1 {
                return BackgroundTarget::StdOut;
            }
            if *fd == 2 {
                return BackgroundTarget::StdErr;
            }
            #[cfg(unix)]
            {
                let dup = unsafe { libc::dup(*fd as libc::c_int) };
                if dup >= 0 {
                    return BackgroundTarget::DupFd(dup);
                }
            }
            if let Ok(name_val) = interp.get_attr(file_obj, "name")
                && let Some(path) = name_val.as_str()
                && !path.starts_with('<')
            {
                return BackgroundTarget::FilePath(path.to_owned());
            }
            BackgroundTarget::StdErr
        }
    }
}

#[cfg(unix)]
fn validate_unix_signal(signum: i64) -> Result<i32> {
    if signum < 1 || signum >= 65 {
        return Err(PyError::named("ValueError", "signal number out of range"));
    }
    let sig = signum as i32;
    if sig == libc::SIGKILL || sig == libc::SIGSTOP {
        return Err(PyError::named(
            "RuntimeError",
            format!("Signal {sig} cannot be registered"),
        ));
    }
    Ok(sig)
}

pyrust_module! {
    fn enable(
        #[default(OptionalArg::Omitted)] file: OptValueRef<'_>,
        #[default(true.into())] all_threads: PyBool,
    ) -> Result<Value> {
        let resolved = resolve_file_arg(_interp, file)?;
        let stored_file = match resolved {
            ResolvedFile::PyFile(v, _) => Some(v),
            ResolvedFile::Fd(fd) => Some(Value::int(fd)),
        };
        let mut st = _interp.cold.faulthandler_state.borrow_mut();
        st.enabled = true;
        st.all_threads = all_threads.0;
        st.enabled_file = stored_file;
        Ok(Value::none())
    }

    fn disable() -> Result<Value> {
        let mut st = _interp.cold.faulthandler_state.borrow_mut();
        st.enabled = false;
        st.enabled_file = None;
        Ok(Value::none())
    }

    fn is_enabled() -> Result<Value> {
        Ok(Value::bool_(_interp.cold.faulthandler_state.borrow().enabled))
    }

    fn dump_traceback(
        #[default(OptionalArg::Omitted)] file: OptValueRef<'_>,
        #[default(true.into())] all_threads: PyBool,
    ) -> Result<Value> {
        let resolved = resolve_file_arg(_interp, file)?;
        let text = format_live_traceback(_interp, "", all_threads.0);
        write_resolved_file(_interp, &resolved, &text)?;
        Ok(Value::none())
    }

    fn dump_traceback_later(
        timeout: PyNumber,
        #[default(false.into())] repeat: PyBool,
        #[default(OptionalArg::Omitted)] file: OptValueRef<'_>,
        #[default(false.into())] exit: PyBool,
    ) -> Result<Value> {
        let secs = timeout.as_f64();
        if secs.is_nan() || secs <= 0.0 {
            return Err(PyError::named(
                "ValueError",
                "timeout value must be positive",
            ));
        }
        if secs.is_infinite() || secs > MAX_TIMEOUT_SECS {
            return Err(PyError::named(
                "OverflowError",
                "timeout value is too large",
            ));
        }

        let resolved = resolve_file_arg(_interp, file)?;
        let bg_target = make_background_target(_interp, &resolved);
        let header = format!(
            "Timeout ({:02}:{:02}:{:02})!\n",
            (secs as u64) / 3600,
            ((secs as u64) % 3600) / 60,
            (secs as u64) % 60,
        );
        let snapshot_text = format_live_traceback(_interp, &header, true);

        cancel_active_timer(_interp);

        let cancel_pair = Arc::new((Mutex::new(false), Condvar::new()));
        _interp.cold.faulthandler_state.borrow_mut().later_cancel = Some(Arc::clone(&cancel_pair));

        let wait_dur = Duration::from_secs_f64(secs);
        let do_repeat = repeat.0;
        let do_exit = exit.0;
        let _ = std::thread::Builder::new()
            .name("pyrust-faulthandler".to_string())
            .spawn(move || {
                let (lock, cvar) = &*cancel_pair;
                loop {
                    let guard = match lock.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    if *guard {
                        break;
                    }
                    let (guard, timeout_res) = match cvar.wait_timeout(guard, wait_dur) {
                        Ok(r) => r,
                        Err(_) => break,
                    };
                    if *guard {
                        break;
                    }
                    drop(guard);
                    if timeout_res.timed_out() {
                        bg_target.write_text(&snapshot_text);
                        if do_exit {
                            std::process::exit(1);
                        }
                        if !do_repeat {
                            break;
                        }
                    }
                }
            });

        Ok(Value::none())
    }

    fn cancel_dump_traceback_later() -> Result<Value> {
        cancel_active_timer(_interp);
        Ok(Value::none())
    }

    #[cfg(unix)]
    fn register(
        signum: crate::interpreter::builtin_args::PyIndex,
        #[default(OptionalArg::Omitted)] file: OptValueRef<'_>,
        #[default(true.into())] all_threads: PyBool,
        #[default(true.into())] chain: PyBool,
    ) -> Result<Value> {
        let raw_sig = signum.expect_i64("register", "signum")?;
        let sig = validate_unix_signal(raw_sig)?;
        let _resolved = resolve_file_arg(_interp, file)?;
        _interp
            .cold
            .faulthandler_state
            .borrow_mut()
            .registered_signals
            .insert(sig, (all_threads.0, chain.0));
        Ok(Value::none())
    }

    #[cfg(unix)]
    fn unregister(signum: crate::interpreter::builtin_args::PyIndex) -> Result<Value> {
        let raw_sig = signum.expect_i64("unregister", "signum")?;
        if !(1..65).contains(&raw_sig) {
            return Err(PyError::named("ValueError", "signal number out of range"));
        }
        let was_registered = _interp
            .cold
            .faulthandler_state
            .borrow_mut()
            .registered_signals
            .remove(&(raw_sig as i32))
            .is_some();
        Ok(Value::bool_(was_registered))
    }
}
