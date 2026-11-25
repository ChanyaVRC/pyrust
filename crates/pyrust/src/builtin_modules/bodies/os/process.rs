//! Process & system control operations exposed by `os`:
//! `system`, `popen` (`_wrap_close`), `kill`, `waitpid`,
//! `waitstatus_to_exitcode`, `abort`, `umask`, `getlogin`, and `times`.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::rc::Rc;

use crate::builtin_modules::macros::native_state::set_native_state;
use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

use super::result_types::make_times_result;

fn shell_command(cmd: &str) -> Command {
    #[cfg(windows)]
    {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut c = Command::new(comspec);
        c.args(["/c", cmd]);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", cmd]);
        c
    }
}

fn exit_status_to_wait_code(status: ExitStatus) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.into_raw() as i64
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(1) as i64
    }
}

/// CPython: `os.system(command)` -> int exit/wait status.
pub(super) fn system(command: &str) -> Result<Value> {
    let status = shell_command(command)
        .status()
        .map_err(|e| PyError::from_io_error(&e, None))?;
    Ok(Value::int(exit_status_to_wait_code(status)))
}

// ── os.popen (`_wrap_close`) ─────────────────────────────────────────────────

enum PopenStream {
    Reader(BufReader<ChildStdout>),
    Writer(ChildStdin),
}

pub(super) struct PopenStreamState {
    child: Option<Child>,
    stream: Option<PopenStream>,
    mode: String,
    closed: bool,
}

fn closed_pipe_error() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file.".to_string())
}

pub(super) fn popen_open(
    interp: &mut Interpreter,
    cmd: &str,
    mode: &str,
    buffering: i64,
) -> Result<Value> {
    if mode != "r" && mode != "w" {
        return Err(PyError::named(
            "ValueError",
            format!("invalid mode {mode:?}"),
        ));
    }
    if buffering == 0 || buffering < -1 {
        return Err(PyError::named(
            "ValueError",
            format!("invalid buffering size {buffering}"),
        ));
    }
    let mut command = shell_command(cmd);
    let mut child = if mode == "r" {
        command
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| PyError::from_io_error(&e, None))?
    } else {
        command
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| PyError::from_io_error(&e, None))?
    };

    let stream = if mode == "r" {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PyError::runtime("os.popen: failed to capture stdout".to_string()))?;
        PopenStream::Reader(BufReader::new(stdout))
    } else {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PyError::runtime("os.popen: failed to capture stdin".to_string()))?;
        PopenStream::Writer(stdin)
    };

    let os_mod = interp.load_module("os")?;
    let cls_val = interp.get_attr(&os_mod, "_wrap_close")?;
    let cls = match cls_val.kind() {
        ValueKind::PyClass(rc) => Rc::clone(rc),
        _ => {
            return Err(PyError::runtime(
                "internal: os._wrap_close is not a class".to_string(),
            ));
        }
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: cls,
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        PopenStreamState {
            child: Some(child),
            stream: Some(stream),
            mode: mode.to_string(),
            closed: false,
        },
    )?;
    Ok(inst)
}

fn normalize_newlines(s: String) -> String {
    if s.contains('\r') {
        s.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        s
    }
}

pub(super) fn popen_read(st: &mut PopenStreamState, size: Option<i64>) -> Result<Value> {
    if st.closed {
        return Err(closed_pipe_error());
    }
    let Some(PopenStream::Reader(reader)) = st.stream.as_mut() else {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not readable".to_string(),
        ));
    };
    match size {
        None | Some(..0) => {
            let mut buf = String::new();
            reader
                .read_to_string(&mut buf)
                .map_err(|e| PyError::from_io_error(&e, None))?;
            Ok(Value::string(normalize_newlines(buf)))
        }
        Some(n) => {
            let mut raw = vec![0u8; n as usize];
            let count = reader
                .read(&mut raw)
                .map_err(|e| PyError::from_io_error(&e, None))?;
            raw.truncate(count);
            let s = String::from_utf8_lossy(&raw).into_owned();
            Ok(Value::string(normalize_newlines(s)))
        }
    }
}

pub(super) fn popen_readline(st: &mut PopenStreamState, size: Option<i64>) -> Result<Value> {
    if st.closed {
        return Err(closed_pipe_error());
    }
    let Some(PopenStream::Reader(reader)) = st.stream.as_mut() else {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not readable".to_string(),
        ));
    };
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| PyError::from_io_error(&e, None))?;
    let mut norm = normalize_newlines(line);
    if let Some(lim) = size
        && lim >= 0
    {
        norm = norm.chars().take(lim as usize).collect();
    }
    Ok(Value::string(norm))
}

pub(super) fn popen_readlines(st: &mut PopenStreamState) -> Result<Value> {
    if st.closed {
        return Err(closed_pipe_error());
    }
    let mut out = Vec::new();
    loop {
        let line = popen_readline(st, None)?;
        if line.as_str().is_some_and(str::is_empty) {
            break;
        }
        out.push(line);
    }
    Ok(Value::list(out))
}

pub(super) fn popen_next(st: &mut PopenStreamState) -> Result<Value> {
    let line = popen_readline(st, None)?;
    if line.as_str().is_some_and(str::is_empty) {
        Err(PyError::named("StopIteration", String::new()))
    } else {
        Ok(line)
    }
}

pub(super) fn popen_write(st: &mut PopenStreamState, data: &str) -> Result<Value> {
    if st.closed {
        return Err(closed_pipe_error());
    }
    let Some(PopenStream::Writer(writer)) = st.stream.as_mut() else {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not writable".to_string(),
        ));
    };
    writer
        .write_all(data.as_bytes())
        .map_err(|e| PyError::from_io_error(&e, None))?;
    Ok(Value::int(data.chars().count() as i64))
}

pub(super) fn popen_flush(st: &mut PopenStreamState) -> Result<Value> {
    if st.closed {
        return Err(closed_pipe_error());
    }
    if let Some(PopenStream::Writer(writer)) = st.stream.as_mut() {
        writer
            .flush()
            .map_err(|e| PyError::from_io_error(&e, None))?;
    }
    Ok(Value::none())
}

pub(super) fn popen_close(st: &mut PopenStreamState) -> Result<Value> {
    if st.closed {
        return Ok(Value::none());
    }
    // Close pipe before waiting on child so writer EOF reaches child cleanly.
    if let Some(PopenStream::Writer(mut writer)) = st.stream.take() {
        let _ = writer.flush();
    } else {
        st.stream = None;
    }
    st.closed = true;
    let Some(mut child) = st.child.take() else {
        return Ok(Value::none());
    };
    let status = child.wait().map_err(|e| PyError::from_io_error(&e, None))?;
    let code = exit_status_to_wait_code(status);
    if code == 0 {
        Ok(Value::none())
    } else {
        Ok(Value::int(code))
    }
}

pub(super) fn popen_is_closed(st: &PopenStreamState) -> Value {
    Value::bool_(st.closed)
}

pub(super) fn popen_mode(st: &PopenStreamState) -> Value {
    Value::string(&st.mode)
}

pub(super) fn popen_readable(st: &PopenStreamState) -> Result<Value> {
    if st.closed {
        return Err(closed_pipe_error());
    }
    Ok(Value::bool_(st.mode == "r"))
}

pub(super) fn popen_writable(st: &PopenStreamState) -> Result<Value> {
    if st.closed {
        return Err(closed_pipe_error());
    }
    Ok(Value::bool_(st.mode == "w"))
}

// ── kill / waitpid / waitstatus_to_exitcode / abort / umask / getlogin / times ─

pub(super) fn kill(pid: i64, sig: i64) -> Result<Value> {
    #[cfg(unix)]
    {
        // SAFETY: standard POSIX `kill`.
        if unsafe { libc::kill(pid as libc::pid_t, sig as libc::c_int) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
            TerminateProcess,
        };
        const STILL_ACTIVE: u32 = 259;

        if pid <= 0 {
            return Err(PyError::named("OSError", "Invalid process id".to_string()));
        }
        if sig == 0 {
            // SAFETY: checks whether process exists and is still active.
            unsafe {
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
                if handle.is_null() {
                    return Err(PyError::from_io_error(
                        &std::io::Error::last_os_error(),
                        None,
                    ));
                }
                let mut code: u32 = 0;
                let ok = GetExitCodeProcess(handle, &mut code);
                CloseHandle(handle);
                if ok == 0 || code != STILL_ACTIVE {
                    return Err(PyError::named(
                        "ProcessLookupError",
                        format!("process {pid} does not exist"),
                    ));
                }
            }
            return Ok(Value::none());
        }
        // SAFETY: opens process with terminate rights and terminates it.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid as u32);
            if handle.is_null() {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    None,
                ));
            }
            let ok = TerminateProcess(handle, sig as u32);
            let err = std::io::Error::last_os_error();
            CloseHandle(handle);
            if ok == 0 {
                return Err(PyError::from_io_error(&err, None));
            }
        }
        Ok(Value::none())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, sig);
        Err(PyError::named(
            "NotImplementedError",
            "os.kill is not supported on this platform".to_string(),
        ))
    }
}

pub(super) fn waitpid(pid: i64, options: i64) -> Result<Value> {
    #[cfg(unix)]
    {
        let mut status: libc::c_int = 0;
        // SAFETY: `status` is a valid out pointer for `waitpid`.
        let ret = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, options as libc::c_int) };
        if ret < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::tuple(vec![
            Value::int(ret as i64),
            Value::int(status as i64),
        ]))
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        if options != 0 {
            return Err(PyError::named(
                "OSError",
                "options must be 0 on Windows".to_string(),
            ));
        }
        if pid <= 0 {
            return Err(PyError::named(
                "ChildProcessError",
                "No child processes".to_string(),
            ));
        }
        // SAFETY: waits on process handle and reads its exit code.
        unsafe {
            let handle = OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid as u32,
            );
            if handle.is_null() {
                return Err(PyError::named(
                    "ChildProcessError",
                    "No child processes".to_string(),
                ));
            }
            let wait_res = WaitForSingleObject(handle, INFINITE);
            if wait_res == WAIT_FAILED {
                let err = std::io::Error::last_os_error();
                CloseHandle(handle);
                return Err(PyError::from_io_error(&err, None));
            }
            let mut exit_code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut exit_code);
            let err = std::io::Error::last_os_error();
            CloseHandle(handle);
            if ok == 0 {
                return Err(PyError::from_io_error(&err, None));
            }
            Ok(Value::tuple(vec![
                Value::int(pid),
                Value::int((exit_code as i64) << 8),
            ]))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, options);
        Err(PyError::named(
            "NotImplementedError",
            "os.waitpid is not supported on this platform".to_string(),
        ))
    }
}

pub(super) fn waitstatus_to_exitcode(status: i64) -> Result<Value> {
    #[cfg(unix)]
    {
        if status < i32::MIN as i64 || status > i32::MAX as i64 {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            ));
        }
        let st = status as libc::c_int;
        if libc::WIFEXITED(st) {
            Ok(Value::int(libc::WEXITSTATUS(st) as i64))
        } else if libc::WIFSIGNALED(st) {
            let sig = libc::WTERMSIG(st);
            if sig <= 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid wait status: {status}"),
                ));
            }
            Ok(Value::int(-(sig as i64)))
        } else if libc::WIFSTOPPED(st) {
            Err(PyError::named(
                "ValueError",
                format!(
                    "process stopped by delivery of signal {}",
                    libc::WSTOPSIG(st)
                ),
            ))
        } else {
            Err(PyError::named(
                "ValueError",
                format!("invalid wait status: {status}"),
            ))
        }
    }
    #[cfg(windows)]
    {
        if status < 0 {
            return Err(PyError::named(
                "OverflowError",
                "can't convert negative int to unsigned".to_string(),
            ));
        }
        if status > u32::MAX as i64 {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C unsigned long".to_string(),
            ));
        }
        let exitcode = (status as u32) >> 8;
        Ok(Value::int(exitcode as i64))
    }
    #[cfg(not(any(unix, windows)))]
    {
        if status < 0 {
            return Err(PyError::named(
                "ValueError",
                format!("invalid wait status: {status}"),
            ));
        }
        Ok(Value::int(status >> 8))
    }
}

pub(super) fn abort() -> Result<Value> {
    std::process::abort()
}

pub(super) fn umask(mask: i64) -> Result<Value> {
    #[cfg(unix)]
    {
        // SAFETY: `umask` is always safe and returns the previous mask.
        let prev = unsafe { libc::umask(mask as libc::mode_t) };
        Ok(Value::int(prev as i64))
    }
    #[cfg(windows)]
    {
        unsafe extern "C" {
            fn _umask(pmode: i32) -> i32;
        }
        // SAFETY: UCRT `_umask` validates mask bits and returns the previous mask.
        let prev = unsafe { _umask((mask & 0o600) as i32) };
        if prev < 0 {
            return Err(PyError::named("OSError", "Invalid argument".to_string()));
        }
        Ok(Value::int(prev as i64))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = mask;
        Ok(Value::int(0))
    }
}

pub(super) fn getlogin() -> Result<Value> {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        // SAFETY: `getlogin` returns NULL or a NUL-terminated static string.
        let ptr = unsafe { libc::getlogin() };
        if !ptr.is_null() {
            let name = unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned();
            return Ok(Value::string(name));
        }
        Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            None,
        ))
    }
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;

        let mut buf = vec![0u16; 257];
        let mut len: u32 = buf.len() as u32;
        // SAFETY: `buf` has length `len`; on success `len` includes the NUL terminator.
        let ok = unsafe { GetUserNameW(buf.as_mut_ptr(), &mut len) };
        if ok != 0 && len > 0 {
            let name = OsString::from_wide(&buf[..(len as usize - 1)])
                .to_string_lossy()
                .into_owned();
            return Ok(Value::string(name));
        }
        if let Ok(user) = std::env::var("USERNAME") {
            return Ok(Value::string(user));
        }
        Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            None,
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(PyError::named(
            "OSError",
            "getlogin is not supported on this platform".to_string(),
        ))
    }
}

pub(super) fn times() -> Result<Value> {
    #[cfg(unix)]
    {
        // SAFETY: `times` fills a zero-initialized `tms` struct and returns clock ticks.
        let mut tms: libc::tms = unsafe { std::mem::zeroed() };
        let ticks = unsafe { libc::times(&mut tms) };
        if ticks == (-1_i64 as libc::clock_t) {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let divisor = if clk_tck > 0 { clk_tck as f64 } else { 100.0 };
        Ok(make_times_result(
            tms.tms_utime as f64 / divisor,
            tms.tms_stime as f64 / divisor,
            tms.tms_cutime as f64 / divisor,
            tms.tms_cstime as f64 / divisor,
            ticks as f64 / divisor,
        ))
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

        fn ft_secs(ft: FILETIME) -> f64 {
            let ticks = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
            (ticks as f64) / 10_000_000.0
        }

        // SAFETY: `GetCurrentProcess()` returns a pseudo-handle; all FILETIME
        // out-pointers are valid stack structs.
        unsafe {
            let mut creation: FILETIME = std::mem::zeroed();
            let mut exit: FILETIME = std::mem::zeroed();
            let mut kernel: FILETIME = std::mem::zeroed();
            let mut user: FILETIME = std::mem::zeroed();
            if GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    None,
                ));
            }
            Ok(make_times_result(
                ft_secs(user),
                ft_secs(kernel),
                0.0,
                0.0,
                0.0,
            ))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(make_times_result(0.0, 0.0, 0.0, 0.0, 0.0))
    }
}
