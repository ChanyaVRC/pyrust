// `pty` module — 100% native Rust implementation of Python 3.12's pseudo-
// terminal utilities (`openpty`, `master_open`, `slave_open`, `fork`, `spawn`,
// `STDIN_FILENO`, `STDOUT_FILENO`, `STDERR_FILENO`, `CHILD`) (issue #3261).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs`.
// Reference: <https://docs.python.org/3.12/library/pty.html>

use std::ffi::{CStr, CString};
use std::sync::{LazyLock, Mutex};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyCallable, PyPath, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

const STDIN_FILENO: i64 = 0;
const STDOUT_FILENO: i64 = 1;
const STDERR_FILENO: i64 = 2;
const CHILD: i64 = 0;

/// Serializes calls to POSIX `ptsname`, which returns a static buffer pointer.
static PTSNAME_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[cfg(target_os = "linux")]
type IoctlReq = libc::Ioctl;
#[cfg(not(target_os = "linux"))]
type IoctlReq = libc::c_ulong;

fn set_cloexec(fd: libc::c_int) {
    // SAFETY: `fcntl(F_GETFD / F_SETFD)` on a freshly opened fd.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// Open a new pseudo-terminal pair `(master_fd, slave_fd)` using POSIX
/// `posix_openpt` / `grantpt` / `unlockpt` / `ptsname` so no `-lutil` linkage
/// is required on older manylinux glibc targets.
pub(crate) fn openpty_raw() -> Result<(libc::c_int, libc::c_int)> {
    // SAFETY: standard POSIX pseudo-terminal allocation sequence.
    let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
    if master < 0 {
        return Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            None,
        ));
    }
    let slave_path = {
        let _guard = PTSNAME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if unsafe { libc::grantpt(master) } < 0 || unsafe { libc::unlockpt(master) } < 0 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(master) };
            return Err(PyError::from_io_error(&err, None));
        }
        let name_ptr = unsafe { libc::ptsname(master) };
        if name_ptr.is_null() {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(master) };
            return Err(PyError::from_io_error(&err, None));
        }
        // SAFETY: `name_ptr` is NUL-terminated and valid while `PTSNAME_LOCK` is held.
        unsafe { CStr::from_ptr(name_ptr) }.to_owned()
    };

    // SAFETY: `slave_path` is a valid NUL-terminated device path.
    let slave = unsafe { libc::open(slave_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };
    if slave < 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(master) };
        return Err(PyError::from_io_error(&err, None));
    }
    set_cloexec(master);
    set_cloexec(slave);
    Ok((master, slave))
}

/// Fork a child attached to a new controlling pseudo-terminal without needing
/// `-lutil`'s `forkpty` symbol.
fn forkpty_raw() -> Result<(libc::pid_t, libc::c_int)> {
    let (master, slave) = openpty_raw()?;
    // SAFETY: standard POSIX `fork`.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let err = std::io::Error::last_os_error();
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
        return Err(PyError::from_io_error(&err, None));
    }
    if pid == 0 {
        // Child process: detach from parent session, acquire controlling tty,
        // and bind stdin/stdout/stderr to `slave`.
        // SAFETY: standard POSIX child setup after `fork()`.
        unsafe {
            libc::close(master);
            libc::setsid();
            #[allow(clippy::unnecessary_cast)]
            let tiocsctty = libc::TIOCSCTTY as IoctlReq;
            let _ = libc::ioctl(slave, tiocsctty, 0_i32);
            let _ = libc::dup2(slave, STDIN_FILENO as libc::c_int);
            let _ = libc::dup2(slave, STDOUT_FILENO as libc::c_int);
            let _ = libc::dup2(slave, STDERR_FILENO as libc::c_int);
            if slave > STDERR_FILENO as libc::c_int {
                libc::close(slave);
            }
        }
        Ok((0, -1))
    } else {
        // SAFETY: parent closes its copy of `slave`.
        unsafe { libc::close(slave) };
        Ok((pid, master))
    }
}

fn default_read_fd(fd: libc::c_int) -> Result<Vec<u8>> {
    let mut buf = [0u8; 1024];
    // SAFETY: `buf` is valid for 1024 bytes.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        return Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            None,
        ));
    }
    Ok(buf[..n as usize].to_vec())
}

fn write_all_fd(fd: libc::c_int, mut data: &[u8]) -> Result<()> {
    while !data.is_empty() {
        // SAFETY: `data` is a valid byte slice.
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(PyError::from_io_error(&err, None));
        }
        data = &data[n as usize..];
    }
    Ok(())
}

fn read_via_callback(
    interp: &mut Interpreter,
    callback: Option<&Value>,
    fd: libc::c_int,
) -> Result<Vec<u8>> {
    match callback {
        Some(cb) => {
            let res = interp.call_function_expanded(
                cb.clone(),
                &[ExpandedCallArg {
                    name: None,
                    value: Value::int(fd as i64),
                }],
            )?;
            match res.kind() {
                ValueKind::Bytes(b) => Ok(b.to_vec()),
                _ => {
                    if let Some(ba) = pyrust_builtins::bytearray::as_bytearray_snapshot(&res) {
                        Ok(ba)
                    } else {
                        Err(PyError::named(
                            "TypeError",
                            "pty read callback must return bytes".to_string(),
                        ))
                    }
                }
            }
        }
        None => default_read_fd(fd),
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("openpty"),
            Value::string("fork"),
            Value::string("spawn"),
        ]),
        "STDIN_FILENO" => Value::int(STDIN_FILENO),
        "STDOUT_FILENO" => Value::int(STDOUT_FILENO),
        "STDERR_FILENO" => Value::int(STDERR_FILENO),
        "CHILD" => Value::int(CHILD),
    }

    /// Open a pseudo-terminal pair and return `(master_fd, slave_fd)`.
    fn openpty() -> Result<Value> {
        let (master, slave) = openpty_raw()?;
        Ok(Value::tuple(vec![
            Value::int(master as i64),
            Value::int(slave as i64),
        ]))
    }

    /// Open a pty master and return `(master_fd, slave_name)`.
    fn master_open() -> Result<Value> {
        let (master, slave) = openpty_raw()?;
        // SAFETY: `ttyname` returns a pointer to static NUL-terminated buffer or NULL.
        let name_ptr = unsafe { libc::ttyname(slave) };
        let err = std::io::Error::last_os_error();
        // SAFETY: `slave` is an owned fd returned by `openpty_raw`.
        unsafe { libc::close(slave) };
        if name_ptr.is_null() {
            unsafe { libc::close(master) };
            return Err(PyError::from_io_error(&err, None));
        }
        // SAFETY: `name_ptr` is a valid NUL-terminated string.
        let name = unsafe { CStr::from_ptr(name_ptr) }
            .to_string_lossy()
            .into_owned();
        Ok(Value::tuple(vec![
            Value::int(master as i64),
            Value::string(name),
        ]))
    }

    /// Open the pty slave device `tty_name` and return its file descriptor.
    fn slave_open(tty_name: PyPath) -> Result<Value> {
        use std::os::unix::ffi::OsStrExt;
        let cpath = CString::new(tty_name.as_os_str().as_bytes())
            .map_err(|_| PyError::named("ValueError", "embedded null byte".to_string()))?;
        // SAFETY: `cpath` is NUL-terminated.
        let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                Some(&tty_name.as_path().to_string_lossy()),
            ));
        }
        Ok(Value::int(fd as i64))
    }

    /// Fork a child process connected to a new controlling pseudo-terminal.
    /// Returns `(pid, master_fd)` (where `pid == 0` in the child).
    fn fork() -> Result<Value> {
        let (pid, master) = forkpty_raw()?;
        if pid > 0 {
            Ok(Value::tuple(vec![
                Value::int(pid as i64),
                Value::int(master as i64),
            ]))
        } else {
            Ok(Value::tuple(vec![Value::int(CHILD), Value::int(-1)]))
        }
    }

    /// Spawn a process and connect its controlling terminal with current stdio.
    fn spawn(
        argv: PyValue,
        #[default(None)] master_read: Option<PyCallable>,
        #[default(None)] stdin_read: Option<PyCallable>,
    ) -> Result<Value> {
        let arg_strings: Vec<String> = match argv.0.kind() {
            ValueKind::Str(s) => vec![s.to_string()],
            _ => {
                let items = _interp.collect_iterable(&argv.0)?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match item.kind() {
                        ValueKind::Str(s) => out.push(s.to_string()),
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                "pty.spawn(): argv elements must be strings".to_string(),
                            ));
                        }
                    }
                }
                out
            }
        };
        if arg_strings.is_empty() {
            return Err(PyError::named(
                "ValueError",
                "pty.spawn(): argv must not be empty".to_string(),
            ));
        }
        let c_args: Vec<CString> = arg_strings
            .iter()
            .map(|s| {
                CString::new(s.as_bytes())
                    .map_err(|_| PyError::named("ValueError", "embedded null byte".to_string()))
            })
            .collect::<Result<Vec<_>>>()?;

        let (pid, master) = forkpty_raw()?;
        if pid == 0 {
            let mut ptrs: Vec<*const libc::c_char> =
                c_args.iter().map(|s| s.as_ptr()).collect();
            ptrs.push(std::ptr::null());
            // SAFETY: child process calls `execvp` with NULL-terminated argv array.
            unsafe {
                libc::execvp(ptrs[0], ptrs.as_ptr());
                libc::_exit(127);
            }
        }

        // Save stdin terminal mode if stdin is a tty and switch to raw mode.
        let mut saved_termios: Option<libc::termios> = None;
        // SAFETY: `tcgetattr` fails cleanly with ENOTTY if stdin is not a tty.
        unsafe {
            let mut orig: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(STDIN_FILENO as libc::c_int, &mut orig) == 0 {
                let mut raw = orig;
                raw.c_iflag &= !(libc::IGNBRK
                    | libc::BRKINT
                    | libc::IGNPAR
                    | libc::PARMRK
                    | libc::INPCK
                    | libc::ISTRIP
                    | libc::INLCR
                    | libc::IGNCR
                    | libc::ICRNL
                    | libc::IXON);
                raw.c_oflag &= !libc::OPOST;
                raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
                raw.c_cflag |= libc::CS8;
                raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
                raw.c_cc[libc::VMIN] = 1;
                raw.c_cc[libc::VTIME] = 0;
                if libc::tcsetattr(STDIN_FILENO as libc::c_int, libc::TCSAFLUSH, &raw) == 0 {
                    saved_termios = Some(orig);
                }
            }
        }

        let master_cb = master_read.as_ref().map(|c| c.as_value());
        let stdin_cb = stdin_read.as_ref().map(|c| c.as_value());
        let mut stdin_open = true;

        let copy_res: Result<()> = (|| {
            loop {
                let mut pfds = Vec::with_capacity(2);
                pfds.push(libc::pollfd {
                    fd: master,
                    events: libc::POLLIN,
                    revents: 0,
                });
                if stdin_open {
                    pfds.push(libc::pollfd {
                        fd: STDIN_FILENO as libc::c_int,
                        events: libc::POLLIN,
                        revents: 0,
                    });
                }
                // SAFETY: `pfds` is a valid slice of `libc::pollfd`.
                let rc = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, -1) };
                if rc < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if (pfds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR)) != 0 {
                    match read_via_callback(_interp, master_cb, master) {
                        Ok(data) if !data.is_empty() => {
                            write_all_fd(STDOUT_FILENO as libc::c_int, &data)?;
                        }
                        Ok(_) => break,
                        Err(_) => break,
                    }
                }
                if stdin_open
                    && pfds.len() > 1
                    && (pfds[1].revents & (libc::POLLIN | libc::POLLHUP)) != 0
                {
                    match read_via_callback(_interp, stdin_cb, STDIN_FILENO as libc::c_int) {
                        Ok(data) if !data.is_empty() => {
                            write_all_fd(master, &data)?;
                        }
                        _ => {
                            stdin_open = false;
                        }
                    }
                }
            }
            Ok(())
        })();

        if let Some(orig) = saved_termios {
            // SAFETY: restoring previously saved `termios`.
            unsafe {
                let _ = libc::tcsetattr(STDIN_FILENO as libc::c_int, libc::TCSAFLUSH, &orig);
            }
        }
        // SAFETY: `master` is owned by the parent.
        unsafe { libc::close(master) };

        let mut status: libc::c_int = 0;
        // SAFETY: `waitpid` reaps the forked child.
        let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
        copy_res?;
        if waited < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::int(status as i64))
    }
}
