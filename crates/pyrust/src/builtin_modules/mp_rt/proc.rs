//! OS process launching (`spawn` cross-platform + `fork` on `#[cfg(unix)]`) and
//! lifecycle management (`join`, `is_alive`, `terminate`, `kill`, `exitcode`,
//! `pid`, `sentinel`).

use std::io::Write;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use super::call_pos;
use super::{SHUTDOWN_POLL_SLICE, get_executable_path, pickle_dumps};
use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, current_thread_is_daemon_worker};
use crate::value::{PyKey, Value};

/// Bootstrap Python snippet executed inside every `'spawn'` child process.
///
/// Reads one framed pickle payload `(sys_path, main_file, cwd, parent_pid,
/// proc_bytes)` from raw stdin, sets up `__mp_main__` (so `if __name__ ==
/// '__main__':` guards in the parent script are NEVER re-entered), installs
/// `current_process()` and `parent_process()`, unpickles the `Process` object,
/// and invokes `proc._bootstrap()`.
pub(crate) const SPAWN_PROCESS_BOOTSTRAP: &str = r#"
import os, sys, types
import multiprocessing.spawn as _mp_spawn
_mp_spawn._run_spawn_child()
"#;

/// Underlying OS child process handle.
pub(crate) enum ChildBackend {
    Spawned {
        child: Child,
        exitcode: Option<i64>,
    },
    #[cfg(unix)]
    Forked {
        pid: libc::pid_t,
        exitcode: Option<i64>,
    },
}

/// Shared handle to a started OS child process.
pub(crate) struct ProcessHandle {
    pub pid: i64,
    backend: Mutex<ChildBackend>,
}

impl ProcessHandle {
    /// Launch a `'spawn'` child process executing `process_obj`.
    pub(crate) fn spawn(interp: &mut Interpreter, process_obj: &Value) -> Result<Arc<Self>> {
        let exe_path = get_executable_path()?;

        // Gather preparation data: (sys_path, main_file, cwd, parent_pid)
        let sys_mod = interp.load_module("sys")?;
        let sys_path = interp
            .get_attr(&sys_mod, "path")
            .unwrap_or_else(|_| Value::list(Vec::new()));
        let path_items: Vec<Value> = interp
            .collect_iterable(&sys_path)
            .unwrap_or_default()
            .into_iter()
            .filter(|v| v.as_str().is_some())
            .collect();

        let main_file = if let Ok(modules) = interp.get_attr(&sys_mod, "modules")
            && let Some(dict) = modules.as_dict()
            && let Some(main_mod) = dict.get(&PyKey::str_from("__main__"))
            && let Ok(f) = interp.get_attr(main_mod, "__file__")
            && f.as_str().is_some()
        {
            f
        } else {
            Value::none()
        };

        let cwd = std::env::current_dir()
            .ok()
            .map(|p| Value::string(p.to_string_lossy().as_ref()))
            .unwrap_or_else(Value::none);

        let parent_pid = Value::int(std::process::id() as i64);

        // Pickle the process object itself (triggers `__reduce__` on args such
        // as Queues, Pipes, Locks, Events, Value/Array, Manager proxies, etc.).
        let proc_bytes = pickle_dumps(interp, process_obj.clone())?;
        let envelope = Value::tuple(vec![
            Value::list(path_items),
            main_file,
            cwd,
            parent_pid,
            Value::bytes(proc_bytes),
        ]);
        let frame = pickle_dumps(interp, envelope)?;

        let mut child = Command::new(&exe_path)
            .args(["-c", SPAWN_PROCESS_BOOTSTRAP])
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| PyError::from_io_error(&e, None))?;

        let pid = child.id() as i64;
        if let Some(mut stdin) = child.stdin.take() {
            let hdr = (frame.len() as u32).to_be_bytes();
            let write_res = interp.release_gil_during(|| -> std::io::Result<()> {
                stdin.write_all(&hdr)?;
                stdin.write_all(&frame)?;
                stdin.flush()
            });
            if let Err(e) = write_res {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PyError::from_io_error(&e, None));
            }
        }

        Ok(Arc::new(Self {
            pid,
            backend: Mutex::new(ChildBackend::Spawned {
                child,
                exitcode: None,
            }),
        }))
    }

    /// Launch a `'fork'` child process on Unix via `libc::fork()`.
    #[cfg(unix)]
    pub(crate) fn fork(interp: &mut Interpreter, process_obj: &Value) -> Result<Arc<Self>> {
        // Flush buffers and ensure all live Queues/Sync primitives have broker
        // sockets active in the parent BEFORE forking.
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        super::broker::pre_fork_prepare();

        let parent_pid = std::process::id() as i64;
        let parent_proc = {
            let proc_mod = interp.load_module("multiprocessing.process")?;
            let cur_fn = interp.get_attr(&proc_mod, "current_process")?;
            call_pos(interp, cur_fn, &[])?
        };

        // SAFETY: `libc::fork()` duplicates the process. In the child (`pid == 0`),
        // we immediately run the target process and terminate with `libc::_exit`.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            let err = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&err, None));
        }

        if pid == 0 {
            // Child process!
            let _ = crate::builtin_modules::multiprocessing_process::install_child_process_context(
                interp,
                process_obj,
                parent_pid,
            );
            let _ = parent_proc;
            let code = match interp.get_attr(process_obj, "_bootstrap") {
                Ok(boot_fn) => match call_pos(interp, boot_fn, &[]) {
                    Ok(v) => v.as_int().unwrap_or(0) as i32,
                    Err(_) => 1,
                },
                Err(_) => 1,
            };
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            // SAFETY: Terminate forked child without running parent atexit/Drop.
            unsafe {
                libc::_exit(code);
            }
        }

        Ok(Arc::new(Self {
            pid: pid as i64,
            backend: Mutex::new(ChildBackend::Forked {
                pid,
                exitcode: None,
            }),
        }))
    }

    /// Poll whether the child has exited; returns `Some(exitcode)` if exited.
    pub(crate) fn poll_exitcode(&self) -> Option<i64> {
        let mut g = self
            .backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *g {
            ChildBackend::Spawned { child, exitcode } => {
                if let Some(c) = *exitcode {
                    return Some(c);
                }
                if let Ok(Some(status)) = child.try_wait() {
                    let c = status_to_exitcode(status);
                    *exitcode = Some(c);
                    return Some(c);
                }
                None
            }
            #[cfg(unix)]
            ChildBackend::Forked { pid, exitcode } => {
                if let Some(c) = *exitcode {
                    return Some(c);
                }
                let mut status: libc::c_int = 0;
                // SAFETY: `waitpid` with `WNOHANG` polls child status non-blockingly.
                let r = unsafe { libc::waitpid(*pid, &mut status, libc::WNOHANG) };
                if r == *pid {
                    let c = raw_unix_status_to_exitcode(status);
                    *exitcode = Some(c);
                    return Some(c);
                }
                None
            }
        }
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.poll_exitcode().is_none()
    }

    /// Wait for child termination (up to `timeout`), releasing the GIL while
    /// blocked so threads (including Manager server threads!) run freely.
    pub(crate) fn join(&self, interp: &mut Interpreter, timeout: Option<Duration>) -> Option<i64> {
        if let Some(c) = self.poll_exitcode() {
            return Some(c);
        }
        let gil = Arc::clone(&interp.cold.gil_state);
        let deadline = timeout.map(|d| Instant::now() + d);
        interp.release_gil_during(|| {
            loop {
                if let Some(c) = self.poll_exitcode() {
                    return Some(c);
                }
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    return None;
                }
                if let Some(dl) = deadline {
                    let now = Instant::now();
                    if now >= dl {
                        return self.poll_exitcode();
                    }
                    let sleep_for = (dl - now).min(Duration::from_millis(5));
                    thread::sleep(sleep_for);
                } else {
                    thread::sleep(Duration::from_millis(5).min(SHUTDOWN_POLL_SLICE));
                }
            }
        })
    }

    pub(crate) fn terminate(&self) -> Result<()> {
        let mut g = self
            .backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *g {
            ChildBackend::Spawned { child, exitcode } => {
                if exitcode.is_some() {
                    return Ok(());
                }
                #[cfg(unix)]
                {
                    let pid = child.id() as libc::pid_t;
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                    Ok(())
                }
                #[cfg(windows)]
                {
                    let _ = child.kill();
                    Ok(())
                }
            }
            #[cfg(unix)]
            ChildBackend::Forked { pid, exitcode } => {
                if exitcode.is_some() {
                    return Ok(());
                }
                unsafe {
                    libc::kill(*pid, libc::SIGTERM);
                }
                Ok(())
            }
        }
    }

    pub(crate) fn kill(&self) -> Result<()> {
        let mut g = self
            .backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *g {
            ChildBackend::Spawned { child, exitcode } => {
                if exitcode.is_some() {
                    return Ok(());
                }
                #[cfg(unix)]
                {
                    let pid = child.id() as libc::pid_t;
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                    Ok(())
                }
                #[cfg(windows)]
                {
                    let _ = child.kill();
                    Ok(())
                }
            }
            #[cfg(unix)]
            ChildBackend::Forked { pid, exitcode } => {
                if exitcode.is_some() {
                    return Ok(());
                }
                unsafe {
                    libc::kill(*pid, libc::SIGKILL);
                }
                Ok(())
            }
        }
    }
}

fn status_to_exitcode(status: ExitStatus) -> i64 {
    if let Some(code) = status.code() {
        return code as i64;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return -(sig as i64);
        }
    }
    -1
}

#[cfg(unix)]
fn raw_unix_status_to_exitcode(status: libc::c_int) -> i64 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status) as i64
    } else if libc::WIFSIGNALED(status) {
        -(libc::WTERMSIG(status) as i64)
    } else {
        -1
    }
}
