//! Shared child-process memory and wall-clock caps for parity tests.
//!
//! - On Unix (`#[cfg(unix)]`), installs `RLIMIT_AS` and `RLIMIT_DATA` via
//!   `CommandExt::pre_exec` and polls with a wall-clock kill.
//! - On Windows (`#[cfg(windows)]`), spawns the child via `CreateProcessW`
//!   with `CREATE_SUSPENDED`, assigns `pi.hProcess` to a dedicated Job Object
//!   configured with `JOB_OBJECT_LIMIT_PROCESS_MEMORY |
//!   JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and
//!   resumes `pi.hThread` directly in O(1) (avoiding the global kernel thread
//!   snapshot lock).  Dropping or terminating the Job Object kills the child
//!   and every descendant with zero orphaned processes.

// Shared across multiple integration test binaries (`parity_compare` and
// `windows_job_cap`); each binary uses a subset of the exported helpers.
#![allow(dead_code)]

use std::env;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::mpsc;
use std::thread;

pub const DEFAULT_MEM_MB: u64 = 4096;
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Maximum accepted MiB / seconds value (matches `tools/bench.sh`'s 7-digit
/// cap so `mb * 1024 * 1024` never overflows `u64` or 64-bit `usize`, and
/// rejects values like `18014398509481985` that previously wrapped around).
pub const MAX_OVERRIDE_VALUE: u64 = 9_999_999;

/// Cap on how much of one child pipe the harness keeps in memory.  The OS
/// limits bound each *child*; this bounds the harness process, which runs
/// uncapped — a fixture stuck in a `print` loop would otherwise stream
/// gigabytes into this process before the wall-clock kill fires.
const MAX_CAPTURED_BYTES: u64 = 64 * 1024 * 1024;

/// How long to wait for a drain thread to hand over its buffer once the child
/// is gone.  Bounded rather than a plain `join` so that a stray handle holder
/// cannot hang the harness.
const DRAIN_HANDOVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildLimits {
    pub mem_limit_bytes: u64,
    pub timeout_secs: u64,
}

impl ChildLimits {
    /// Load child limits from `PYRUST_PARITY_MEM_MB` and
    /// `PYRUST_PARITY_TIMEOUT_S`, panicking loudly on invalid/overflowing
    /// values so a typo never silently leaves children uncapped.
    pub fn from_env() -> Self {
        Self {
            mem_limit_bytes: child_mem_limit_bytes(),
            timeout_secs: child_timeout_secs(),
        }
    }
}

/// Parse a positive integer override in `1..=MAX_OVERRIDE_VALUE`.
///
/// Rejects empty strings, leading/trailing whitespace, signs (`+`/`-`),
/// non-ASCII digits, `0`, and anything above `MAX_OVERRIDE_VALUE`.
fn parse_positive_override(var_name: &str, raw: Option<&str>, default: u64) -> Result<u64, String> {
    let Some(text) = raw else {
        return Ok(default);
    };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "{var_name} must be an integer in 1..={MAX_OVERRIDE_VALUE} (got {text:?})"
        ));
    }
    let parsed = text.parse::<u64>().map_err(|_| {
        format!("{var_name} overflows valid range 1..={MAX_OVERRIDE_VALUE} (got {text:?})")
    })?;
    if parsed == 0 || parsed > MAX_OVERRIDE_VALUE {
        return Err(format!(
            "{var_name} must be in 1..={MAX_OVERRIDE_VALUE} (got {text:?})"
        ));
    }
    Ok(parsed)
}

/// Parse `PYRUST_PARITY_MEM_MB` into a byte limit, validating range and
/// checked multiplication (`mb * 1024 * 1024`).
pub fn parse_mem_limit_bytes(raw: Option<&str>) -> Result<u64, String> {
    let mb = parse_positive_override("PYRUST_PARITY_MEM_MB", raw, DEFAULT_MEM_MB)?;
    let bytes = mb
        .checked_mul(1024 * 1024)
        .ok_or_else(|| format!("PYRUST_PARITY_MEM_MB={mb} overflows byte limit"))?;
    #[cfg(windows)]
    if usize::try_from(bytes).is_err() {
        return Err(format!(
            "PYRUST_PARITY_MEM_MB={mb} exceeds platform address-space width"
        ));
    }
    Ok(bytes)
}

/// Parse `PYRUST_PARITY_TIMEOUT_S` into seconds, validating range.
pub fn parse_timeout_secs(raw: Option<&str>) -> Result<u64, String> {
    parse_positive_override("PYRUST_PARITY_TIMEOUT_S", raw, DEFAULT_TIMEOUT_SECS)
}

fn read_env_utf8(var_name: &str) -> Option<String> {
    let raw = env::var_os(var_name)?;
    match raw.into_string() {
        Ok(s) => Some(s),
        Err(bad) => panic!("{var_name} must be valid UTF-8 (got {bad:?})"),
    }
}

pub fn child_mem_limit_bytes() -> u64 {
    let raw = read_env_utf8("PYRUST_PARITY_MEM_MB");
    parse_mem_limit_bytes(raw.as_deref()).unwrap_or_else(|e| panic!("{e}"))
}

pub fn child_timeout_secs() -> u64 {
    let raw = read_env_utf8("PYRUST_PARITY_TIMEOUT_S");
    parse_timeout_secs(raw.as_deref()).unwrap_or_else(|e| panic!("{e}"))
}

/// Drain one child pipe on a helper thread, so the wait loop never blocks on
/// a child that has filled its pipe buffer.
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

#[cfg(windows)]
mod win_job {
    use std::collections::BTreeMap;
    use std::env;
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;
    use std::ptr;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, TRUE,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, GetExitCodeProcess,
        PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess,
        WaitForSingleObject,
    };

    use super::{ChildLimits, collect_drained, drain_pipe, output_tail};

    /// Serializes inheritable stdio handle creation + `CreateProcessW` + parent
    /// write-end close so concurrent parity worker threads never leak one
    /// child's pipe write-end into another child.
    static SPAWN_LOCK: Mutex<()> = Mutex::new(());

    /// RAII wrapper around a Win32 handle that closes it on drop.
    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        fn into_raw(mut self) -> HANDLE {
            let raw = self.0;
            self.0 = ptr::null_mut();
            raw
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                // SAFETY: `self.0` is an owned valid Win32 handle.
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// RAII wrapper around a Windows Job Object configured with per-process and
    /// per-job memory limits plus `KILL_ON_JOB_CLOSE`.
    struct WindowsJob {
        handle: OwnedHandle,
    }

    impl WindowsJob {
        fn create(mem_limit_bytes: u64) -> Result<Self, String> {
            let limit: usize = usize::try_from(mem_limit_bytes).map_err(|_| {
                format!("memory limit {mem_limit_bytes} exceeds platform usize width")
            })?;
            // SAFETY: null security attributes and unnamed job object are valid.
            let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if raw.is_null() {
                return Err(format!(
                    "CreateJobObjectW failed: {}",
                    io::Error::last_os_error()
                ));
            }
            let job = Self {
                handle: OwnedHandle(raw),
            };

            // SAFETY: `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` is a plain C struct.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_JOB_MEMORY
                | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            info.ProcessMemoryLimit = limit;
            info.JobMemoryLimit = limit;

            // SAFETY: `job.handle.0` is a valid job handle and `info` matches
            // `JobObjectExtendedLimitInformation`.
            let ok = unsafe {
                SetInformationJobObject(
                    job.handle.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(format!(
                    "SetInformationJobObject failed: {}",
                    io::Error::last_os_error()
                ));
            }
            Ok(job)
        }

        fn assign_process(&self, process_handle: HANDLE) -> Result<(), String> {
            // SAFETY: `self.handle.0` is a valid job handle and `process_handle`
            // is a live child process handle.
            let ok = unsafe { AssignProcessToJobObject(self.handle.0, process_handle) };
            if ok == 0 {
                return Err(format!(
                    "AssignProcessToJobObject failed: {}",
                    io::Error::last_os_error()
                ));
            }
            Ok(())
        }

        fn terminate(&self, exit_code: u32) {
            // SAFETY: `self.handle.0` is a valid job handle.
            unsafe {
                TerminateJobObject(self.handle.0, exit_code);
            }
        }
    }

    /// Encode one argument using Windows `CommandLineToArgvW` / CRT quoting rules.
    fn append_windows_arg(cmd: &mut Vec<u16>, arg: &OsStr, force_quote: bool) {
        let wide: Vec<u16> = arg.encode_wide().collect();
        let quote = force_quote
            || wide.is_empty()
            || wide
                .iter()
                .any(|&c| c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16);
        if quote {
            cmd.push(b'"' as u16);
        }
        let mut backslashes = 0usize;
        for &c in &wide {
            if c == b'\\' as u16 {
                backslashes += 1;
            } else if c == b'"' as u16 {
                for _ in 0..(backslashes * 2 + 1) {
                    cmd.push(b'\\' as u16);
                }
                cmd.push(b'"' as u16);
                backslashes = 0;
            } else {
                for _ in 0..backslashes {
                    cmd.push(b'\\' as u16);
                }
                backslashes = 0;
                cmd.push(c);
            }
        }
        if quote {
            for _ in 0..(backslashes * 2) {
                cmd.push(b'\\' as u16);
            }
            cmd.push(b'"' as u16);
        } else {
            for _ in 0..backslashes {
                cmd.push(b'\\' as u16);
            }
        }
    }

    pub(crate) fn build_command_line<S: AsRef<OsStr>>(program: &Path, args: &[S]) -> Vec<u16> {
        let mut cmd = Vec::new();
        // Always quote argv[0] so paths with spaces never split on `CreateProcessW`.
        append_windows_arg(&mut cmd, program.as_os_str(), true);
        for arg in args {
            cmd.push(b' ' as u16);
            append_windows_arg(&mut cmd, arg.as_ref(), false);
        }
        cmd.push(0);
        cmd
    }

    /// Build a UTF-16 `KEY=VALUE\0...\0\0` environment block for
    /// `CreateProcessW(CREATE_UNICODE_ENVIRONMENT)` merging the current
    /// environment with `PYTHONIOENCODING=utf-8` and `extra_env`.
    fn build_env_block(extra_env: &[(&str, &str)]) -> Vec<u16> {
        // Case-insensitive keys on Windows: fold key case for deduplication
        // while preserving original casing in the emitted block.
        let mut vars: BTreeMap<String, (OsString, OsString)> = BTreeMap::new();
        for (k, v) in env::vars_os() {
            let folded = k.to_string_lossy().to_ascii_uppercase();
            vars.insert(folded, (k, v));
        }
        vars.insert(
            "PYTHONIOENCODING".to_string(),
            (OsString::from("PYTHONIOENCODING"), OsString::from("utf-8")),
        );
        for &(k, v) in extra_env {
            vars.insert(
                k.to_ascii_uppercase(),
                (OsString::from(k), OsString::from(v)),
            );
        }

        let mut block = Vec::new();
        for (k, v) in vars.values() {
            block.extend(k.encode_wide());
            block.push(b'=' as u16);
            block.extend(v.encode_wide());
            block.push(0);
        }
        block.push(0);
        block
    }

    /// Create an anonymous pipe whose write end is inheritable by the child
    /// and whose read end is strictly non-inheritable.
    fn create_child_output_pipe() -> Result<(OwnedHandle, OwnedHandle), String> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: TRUE,
        };
        let mut read_raw: HANDLE = ptr::null_mut();
        let mut write_raw: HANDLE = ptr::null_mut();
        // SAFETY: valid out pointers and initialized `SECURITY_ATTRIBUTES`.
        if unsafe { CreatePipe(&raw mut read_raw, &raw mut write_raw, &raw const sa, 0) } == 0 {
            return Err(format!("CreatePipe failed: {}", io::Error::last_os_error()));
        }
        let read = OwnedHandle(read_raw);
        let write = OwnedHandle(write_raw);
        // SAFETY: clears `HANDLE_FLAG_INHERIT` on the parent-owned read end.
        if unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(format!(
                "SetHandleInformation failed: {}",
                io::Error::last_os_error()
            ));
        }
        Ok((read, write))
    }

    /// Open `NUL` for reading as an inheritable handle for child `stdin`.
    fn open_nul_stdin() -> Result<OwnedHandle, String> {
        let nul: Vec<u16> = "NUL\0".encode_utf16().collect();
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: TRUE,
        };
        // SAFETY: null-terminated `"NUL"` with valid `SECURITY_ATTRIBUTES`.
        let raw = unsafe {
            CreateFileW(
                nul.as_ptr(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &raw const sa,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE || raw.is_null() {
            return Err(format!(
                "CreateFileW(NUL) failed: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(OwnedHandle(raw))
    }

    pub(super) fn run_and_capture<S: AsRef<OsStr>>(
        program: &Path,
        args: &[S],
        extra_env: &[(&str, &str)],
        limits: ChildLimits,
    ) -> Result<String, String> {
        let job = WindowsJob::create(limits.mem_limit_bytes)?;
        let mut cmdline = build_command_line(program, args);
        let mut env_block = build_env_block(extra_env);

        let (process, stdout_file, stderr_file) = {
            // Keep inheritable child handles strictly scoped under `SPAWN_LOCK`
            // so concurrent worker threads never cross-inherit pipe write ends.
            let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let nul_in = open_nul_stdin()?;
            let (stdout_read, stdout_write) = create_child_output_pipe()?;
            let (stderr_read, stderr_write) = create_child_output_pipe()?;

            // SAFETY: plain C structs zero-initialized before setting `cb`.
            let mut si: STARTUPINFOW = unsafe { zeroed() };
            si.cb = size_of::<STARTUPINFOW>() as u32;
            si.dwFlags = STARTF_USESTDHANDLES;
            si.hStdInput = nul_in.0;
            si.hStdOutput = stdout_write.0;
            si.hStdError = stderr_write.0;

            let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };
            // SAFETY: `cmdline` and `env_block` are valid NUL-terminated UTF-16
            // buffers, `si.cb` is set, and `pi` is writable.
            let ok = unsafe {
                CreateProcessW(
                    ptr::null(),
                    cmdline.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    TRUE,
                    CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                    env_block.as_mut_ptr().cast(),
                    ptr::null(),
                    &raw const si,
                    &raw mut pi,
                )
            };
            if ok == 0 {
                return Err(format!(
                    "failed to run {}: {}",
                    program.display(),
                    io::Error::last_os_error()
                ));
            }
            let process = OwnedHandle(pi.hProcess);
            let thread = OwnedHandle(pi.hThread);
            // Drop parent copies of the inheritable handles immediately while
            // still holding `SPAWN_LOCK`.
            drop(nul_in);
            drop(stdout_write);
            drop(stderr_write);

            // Assign the suspended child to the Job Object BEFORE resuming its
            // primary thread, so 0 child instructions execute uncapped.
            if let Err(err) = job.assign_process(process.0) {
                // SAFETY: child is still suspended and owned by `process`.
                unsafe {
                    TerminateProcess(process.0, 1);
                    WaitForSingleObject(process.0, 5_000);
                }
                return Err(format!("failed to cap {}: {err}", program.display()));
            }

            // SAFETY: `thread.0` is the child's primary thread handle returned
            // directly by `CreateProcessW` (O(1), no global thread snapshot).
            let prev_suspend = unsafe { ResumeThread(thread.0) };
            if prev_suspend == u32::MAX {
                let err = io::Error::last_os_error();
                job.terminate(1);
                unsafe {
                    WaitForSingleObject(process.0, 5_000);
                }
                return Err(format!("failed to resume {}: {err}", program.display()));
            }
            drop(thread);

            // SAFETY: `stdout_read` / `stderr_read` are valid owned pipe read handles.
            let stdout_file = unsafe { File::from_raw_handle(stdout_read.into_raw().cast()) };
            let stderr_file = unsafe { File::from_raw_handle(stderr_read.into_raw().cast()) };
            (process, stdout_file, stderr_file)
        };

        let stdout_rx = drain_pipe(stdout_file);
        let stderr_rx = drain_pipe(stderr_file);

        let timeout_secs = limits.timeout_secs;
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let timed_out = loop {
            let now = Instant::now();
            if now >= deadline {
                break true;
            }
            let remaining_ms = (deadline - now).as_millis().clamp(1, u32::MAX as u128 - 1) as u32;
            // SAFETY: `process.0` is a valid process handle.
            let wait = unsafe { WaitForSingleObject(process.0, remaining_ms) };
            if wait == WAIT_OBJECT_0 {
                break false;
            }
            if wait == WAIT_TIMEOUT {
                if Instant::now() >= deadline {
                    break true;
                }
                continue;
            }
            job.terminate(1);
            drop(job);
            return Err(format!(
                "failed to wait on {}: {}",
                program.display(),
                io::Error::last_os_error()
            ));
        };

        if timed_out {
            job.terminate(1);
            // SAFETY: wait briefly for the terminated process object to signal.
            unsafe {
                WaitForSingleObject(process.0, 5_000);
            }
            drop(job);
            return Err(format!(
                "{} timed out after {timeout_secs}s (killed; raise PYRUST_PARITY_TIMEOUT_S if legitimate)\n{}",
                program.display(),
                output_tail(&collect_drained(&stdout_rx), &collect_drained(&stderr_rx))
            ));
        }

        let mut exit_code: u32 = 1;
        // SAFETY: `process.0` has exited and `exit_code` is writable.
        if unsafe { GetExitCodeProcess(process.0, &raw mut exit_code) } == 0 {
            drop(job);
            return Err(format!(
                "GetExitCodeProcess failed for {}: {}",
                program.display(),
                io::Error::last_os_error()
            ));
        }

        // Close the Job Object right after the child exits so any orphaned
        // grandchild holding stdout/stderr handles is immediately killed via
        // `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` before we collect the pipes.
        drop(job);
        drop(process);

        let stdout = collect_drained(&stdout_rx);
        let stderr = collect_drained(&stderr_rx);
        let mut merged = String::new();
        merged.push_str(&String::from_utf8_lossy(&stdout));
        merged.push_str(&String::from_utf8_lossy(&stderr));

        if exit_code == 0 {
            Ok(merged)
        } else {
            Err(format!(
                "{} exited with status {}\n{}",
                program.display(),
                exit_code as i32,
                merged
            ))
        }
    }
}

#[cfg(windows)]
pub fn build_windows_command_line<S: AsRef<OsStr>>(program: &Path, args: &[S]) -> Vec<u16> {
    win_job::build_command_line(program, args)
}

/// Run `program` with `args` and optional extra environment variables under
/// explicit `limits`, capturing merged stdout+stderr.
pub fn run_and_capture_with_limits<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    extra_env: &[(&str, &str)],
    limits: ChildLimits,
) -> Result<String, String> {
    #[cfg(windows)]
    {
        win_job::run_and_capture(program, args, extra_env, limits)
    }

    #[cfg(not(windows))]
    {
        use std::process::Command;

        let mut command = Command::new(program);
        command.args(args).env("PYTHONIOENCODING", "utf-8");
        for (k, v) in extra_env {
            command.env(k, v);
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let limit = limits.mem_limit_bytes;
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

        let stdout_rx = drain_pipe(child.stdout.take().expect("stdout piped"));
        let stderr_rx = drain_pipe(child.stderr.take().expect("stderr piped"));

        let timeout_secs = limits.timeout_secs;
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
        let mut merged = String::new();
        merged.push_str(&String::from_utf8_lossy(&stdout));
        merged.push_str(&String::from_utf8_lossy(&stderr));

        if status.success() {
            Ok(merged)
        } else {
            Err(format!(
                "{} exited with status {}\n{}",
                program.display(),
                status
                    .code()
                    .map_or_else(|| "<signal>".to_string(), |c| c.to_string()),
                merged
            ))
        }
    }
}

/// Convenience wrapper reading limits from `PYRUST_PARITY_MEM_MB` /
/// `PYRUST_PARITY_TIMEOUT_S`.
pub fn run_and_capture(program: &Path, args: &[&Path]) -> Result<String, String> {
    let limits = ChildLimits::from_env();
    run_and_capture_with_limits(program, args, &[], limits)
}
