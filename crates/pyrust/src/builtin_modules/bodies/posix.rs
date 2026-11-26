// `posix` module — 100% native Rust implementation of Python 3.12's POSIX OS
// bindings and Unix-specific `os` syscalls (issue #3261).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs`. Its POSIX-specific
// functions, struct-sequences, constants, and `_EnvironB` (`os.environb`) are
// re-exported onto `os` during `os::prepare_module` on Unix.
// Reference: <https://docs.python.org/3.12/library/os.html>

use std::cell::RefCell;
use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, PyBool, PyBytes, PyIndex, PyPath, PyValue, PyValueRef,
};
use crate::interpreter::{Interpreter, NativeIterFrame, lookup_exc_class, value_type_name_str};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, PyModule, Value, ValueKind};

pyrust_struct_seq! {
    pub(crate) uname_result in "posix" {
        fields: [sysname, nodename, release, version, machine],
    }
}

pyrust_struct_seq! {
    pub(crate) statvfs_result in "os" {
        fields: [
            f_bsize,
            f_frsize,
            f_blocks,
            f_bfree,
            f_bavail,
            f_files,
            f_ffree,
            f_favail,
            f_flag,
            f_namemax,
        ],
        extra_fields: [f_fsid],
    }
}

fn clear_errno() {
    #[cfg(target_os = "linux")]
    // SAFETY: `__errno_location()` returns a valid thread-local `c_int` pointer.
    unsafe {
        *libc::__errno_location() = 0;
    }
    #[cfg(target_os = "macos")]
    // SAFETY: `__error()` returns a valid thread-local `c_int` pointer.
    unsafe {
        *libc::__error() = 0;
    }
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn path_to_cstring(path: &PyPath) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| PyError::named("ValueError", "embedded null byte".to_string()))
}

#[allow(clippy::unnecessary_cast)]
fn c_array_to_string(arr: &[libc::c_char]) -> String {
    // SAFETY: `arr` is NUL-terminated by the kernel (or bounded by `arr.len()`).
    let len = arr.iter().position(|&c| c == 0).unwrap_or(arr.len());
    let bytes: Vec<u8> = arr[..len].iter().map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn statvfs_to_value(sv: &libc::statvfs) -> Value {
    let seq = vec![
        Value::int(sv.f_bsize as i64),
        Value::int(sv.f_frsize as i64),
        Value::int(sv.f_blocks as i64),
        Value::int(sv.f_bfree as i64),
        Value::int(sv.f_bavail as i64),
        Value::int(sv.f_files as i64),
        Value::int(sv.f_ffree as i64),
        Value::int(sv.f_favail as i64),
        Value::int(sv.f_flag as i64),
        Value::int(sv.f_namemax as i64),
    ];
    let extra = vec![Value::int(sv.f_fsid as i64)];
    statvfs_result::make_with_extra(seq, extra)
}

fn c_int_from_i64(val: i64) -> Result<libc::c_int> {
    libc::c_int::try_from(val).map_err(|_| {
        PyError::named(
            "OverflowError",
            "Python int too large to convert to C int".to_string(),
        )
    })
}

const PATHCONF_ENTRIES: &[(&str, libc::c_int)] = &[
    ("PC_ALLOC_SIZE_MIN", libc::_PC_ALLOC_SIZE_MIN),
    ("PC_ASYNC_IO", libc::_PC_ASYNC_IO),
    ("PC_CHOWN_RESTRICTED", libc::_PC_CHOWN_RESTRICTED),
    ("PC_FILESIZEBITS", libc::_PC_FILESIZEBITS),
    ("PC_LINK_MAX", libc::_PC_LINK_MAX),
    ("PC_MAX_CANON", libc::_PC_MAX_CANON),
    ("PC_MAX_INPUT", libc::_PC_MAX_INPUT),
    ("PC_NAME_MAX", libc::_PC_NAME_MAX),
    ("PC_NO_TRUNC", libc::_PC_NO_TRUNC),
    ("PC_PATH_MAX", libc::_PC_PATH_MAX),
    ("PC_PIPE_BUF", libc::_PC_PIPE_BUF),
    ("PC_PRIO_IO", libc::_PC_PRIO_IO),
    ("PC_REC_INCR_XFER_SIZE", libc::_PC_REC_INCR_XFER_SIZE),
    ("PC_REC_MAX_XFER_SIZE", libc::_PC_REC_MAX_XFER_SIZE),
    ("PC_REC_MIN_XFER_SIZE", libc::_PC_REC_MIN_XFER_SIZE),
    ("PC_REC_XFER_ALIGN", libc::_PC_REC_XFER_ALIGN),
    ("PC_SYMLINK_MAX", libc::_PC_SYMLINK_MAX),
    ("PC_SYNC_IO", libc::_PC_SYNC_IO),
    ("PC_VDISABLE", libc::_PC_VDISABLE),
    ("PC_2_SYMLINKS", libc::_PC_2_SYMLINKS),
];

const SYSCONF_ENTRIES: &[(&str, libc::c_int)] = &[
    ("SC_ARG_MAX", libc::_SC_ARG_MAX),
    ("SC_ATEXIT_MAX", libc::_SC_ATEXIT_MAX),
    ("SC_CHILD_MAX", libc::_SC_CHILD_MAX),
    ("SC_CLK_TCK", libc::_SC_CLK_TCK),
    ("SC_GETGR_R_SIZE_MAX", libc::_SC_GETGR_R_SIZE_MAX),
    ("SC_GETPW_R_SIZE_MAX", libc::_SC_GETPW_R_SIZE_MAX),
    ("SC_HOST_NAME_MAX", libc::_SC_HOST_NAME_MAX),
    ("SC_IOV_MAX", libc::_SC_IOV_MAX),
    ("SC_JOB_CONTROL", libc::_SC_JOB_CONTROL),
    ("SC_LINE_MAX", libc::_SC_LINE_MAX),
    ("SC_LOGIN_NAME_MAX", libc::_SC_LOGIN_NAME_MAX),
    ("SC_NGROUPS_MAX", libc::_SC_NGROUPS_MAX),
    ("SC_NPROCESSORS_CONF", libc::_SC_NPROCESSORS_CONF),
    ("SC_NPROCESSORS_ONLN", libc::_SC_NPROCESSORS_ONLN),
    ("SC_OPEN_MAX", libc::_SC_OPEN_MAX),
    ("SC_PAGESIZE", libc::_SC_PAGESIZE),
    ("SC_PAGE_SIZE", libc::_SC_PAGE_SIZE),
    ("SC_RE_DUP_MAX", libc::_SC_RE_DUP_MAX),
    ("SC_SAVED_IDS", libc::_SC_SAVED_IDS),
    ("SC_STREAM_MAX", libc::_SC_STREAM_MAX),
    ("SC_SYMLOOP_MAX", libc::_SC_SYMLOOP_MAX),
    ("SC_TTY_NAME_MAX", libc::_SC_TTY_NAME_MAX),
    ("SC_TZNAME_MAX", libc::_SC_TZNAME_MAX),
    ("SC_VERSION", libc::_SC_VERSION),
];

const CONFSTR_ENTRIES: &[(&str, libc::c_int)] = &[
    ("CS_PATH", libc::_CS_PATH),
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    ("CS_GNU_LIBC_VERSION", libc::_CS_GNU_LIBC_VERSION),
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    (
        "CS_GNU_LIBPTHREAD_VERSION",
        libc::_CS_GNU_LIBPTHREAD_VERSION,
    ),
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    ("CS_V6_ENV", libc::_CS_V6_ENV),
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    ("CS_V7_ENV", libc::_CS_V7_ENV),
    #[cfg(target_os = "macos")]
    ("CS_DARWIN_USER_DIR", libc::_CS_DARWIN_USER_DIR),
    #[cfg(target_os = "macos")]
    ("CS_DARWIN_USER_TEMP_DIR", libc::_CS_DARWIN_USER_TEMP_DIR),
    #[cfg(target_os = "macos")]
    ("CS_DARWIN_USER_CACHE_DIR", libc::_CS_DARWIN_USER_CACHE_DIR),
];

fn make_conf_dict(entries: &[(&str, libc::c_int)]) -> Value {
    let mut dict = PyDict::default();
    for &(name, val) in entries {
        dict.insert(PyKey::str_from(name), Value::int(val as i64));
    }
    Value::dict(dict)
}

fn resolve_conf_name(
    val: &Value,
    entries: &[(&str, libc::c_int)],
    fn_name: &str,
) -> Result<libc::c_int> {
    match val.kind() {
        ValueKind::Int(n) => c_int_from_i64(n),
        ValueKind::Bool(b) => Ok(libc::c_int::from(b)),
        ValueKind::Str(s) => entries
            .iter()
            .find(|&&(k, _)| k == s.as_ref())
            .map(|&(_, v)| v)
            .ok_or_else(|| {
                PyError::named("ValueError", "unrecognized configuration name".to_string())
            }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}(): configuration names must be strings or integers, not {}",
                value_type_name_str(val)
            ),
        )),
    }
}

fn require_env_bytes<'a>(val: &'a Value) -> Result<&'a [u8]> {
    match val.kind() {
        ValueKind::Bytes(b) => Ok(b.as_slice()),
        _ => Err(PyError::named(
            "TypeError",
            format!("bytes expected, not {}", value_type_name_str(val)),
        )),
    }
}

fn validate_env_key_bytes(key: &[u8]) -> Result<()> {
    if key.is_empty() || key.contains(&0) || key.contains(&b'=') {
        return Err(PyError::named(
            "ValueError",
            "illegal environment variable name".to_string(),
        ));
    }
    Ok(())
}

fn validate_env_val_bytes(val: &[u8]) -> Result<()> {
    if val.contains(&0) {
        return Err(PyError::named(
            "ValueError",
            "embedded null byte".to_string(),
        ));
    }
    Ok(())
}

/// Names exported by `posix` that are mirrored onto `os` on Unix during
/// `os::prepare_module`.
const POSIX_TO_OS_EXPORTS: &[&str] = &[
    // Structseq types
    "uname_result",
    "statvfs_result",
    // Constants
    "WNOHANG",
    "WUNTRACED",
    "WCONTINUED",
    "EX_OK",
    "EX_USAGE",
    "EX_DATAERR",
    "EX_NOINPUT",
    "EX_NOUSER",
    "EX_NOHOST",
    "EX_UNAVAILABLE",
    "EX_SOFTWARE",
    "EX_OSERR",
    "EX_OSFILE",
    "EX_CANTCREAT",
    "EX_IOERR",
    "EX_TEMPFAIL",
    "EX_PROTOCOL",
    "EX_NOPERM",
    "EX_CONFIG",
    "pathconf_names",
    "sysconf_names",
    "confstr_names",
    // Functions
    "uname",
    "fork",
    "_exit",
    "openpty",
    "getuid",
    "geteuid",
    "getgid",
    "getegid",
    "setuid",
    "seteuid",
    "setgid",
    "setegid",
    "setreuid",
    "setregid",
    "getgroups",
    "getpgrp",
    "setpgrp",
    "getpgid",
    "setpgid",
    "getsid",
    "setsid",
    "killpg",
    "chown",
    "lchown",
    "fchown",
    "statvfs",
    "fstatvfs",
    "mkfifo",
    "mknod",
    "wait",
    "WIFEXITED",
    "WEXITSTATUS",
    "WIFSIGNALED",
    "WTERMSIG",
    "WIFSTOPPED",
    "WSTOPSIG",
    "WIFCONTINUED",
    "WCOREDUMP",
    "pathconf",
    "fpathconf",
    "sysconf",
    "confstr",
    "getloadavg",
    "sync",
    "nice",
    "ctermid",
    "ttyname",
    "tcgetpgrp",
    "tcsetpgrp",
    "getenvb",
];

/// Cross-platform low-level `os` attributes mirrored onto `posix` so
/// `import posix` exposes the same core POSIX syscall surface as CPython.
const OS_TO_POSIX_EXPORTS: &[&str] = &[
    "F_OK",
    "R_OK",
    "W_OK",
    "X_OK",
    "O_RDONLY",
    "O_WRONLY",
    "O_RDWR",
    "O_CREAT",
    "O_EXCL",
    "O_TRUNC",
    "O_APPEND",
    "O_NONBLOCK",
    "O_CLOEXEC",
    "O_NOFOLLOW",
    "stat_result",
    "times_result",
    "terminal_size",
    "DirEntry",
    "ScandirIterator",
    "getcwd",
    "getcwdb",
    "chdir",
    "listdir",
    "mkdir",
    "remove",
    "unlink",
    "rmdir",
    "rename",
    "replace",
    "link",
    "symlink",
    "readlink",
    "stat",
    "lstat",
    "access",
    "chmod",
    "utime",
    "scandir",
    "open",
    "close",
    "read",
    "write",
    "dup",
    "dup2",
    "pipe",
    "lseek",
    "fstat",
    "fsync",
    "ftruncate",
    "truncate",
    "isatty",
    "closerange",
    "get_inheritable",
    "set_inheritable",
    "get_blocking",
    "set_blocking",
    "putenv",
    "unsetenv",
    "getpid",
    "getppid",
    "cpu_count",
    "urandom",
    "strerror",
    "get_terminal_size",
    "system",
    "kill",
    "waitpid",
    "waitstatus_to_exitcode",
    "abort",
    "umask",
    "getlogin",
    "times",
    "fspath",
];

/// Build a live snapshot dict `{bytes: bytes}` for `posix.environ`.
fn build_posix_environ_dict() -> Value {
    let _guard = super::os::env_lock();
    let mut dict = PyDict::default();
    for (k, v) in std::env::vars_os() {
        dict.insert(
            PyKey::Bytes(k.as_bytes().to_vec().into()),
            Value::bytes(v.as_bytes().to_vec()),
        );
    }
    Value::dict(dict)
}

/// Post-load wiring for `posix` invoked from `builtin_modules/finalization.rs`.
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(posix_rc) = module.kind() else {
        return;
    };
    let os_raw = super::os::module();
    let ValueKind::PyModule(os_rc) = os_raw.kind() else {
        return;
    };
    let os_borrowed = os_rc.borrow();
    let mut posix_mut = posix_rc.borrow_mut();
    for &name in OS_TO_POSIX_EXPORTS {
        if let Some(val) = os_borrowed.attrs.get(name) {
            posix_mut.insert_attr(name.to_string(), val.clone());
        }
    }
    posix_mut.insert_attr("environ".to_string(), build_posix_environ_dict());
}

/// Install all POSIX-only functions, structseq types, constants, and `environb`
/// onto `os` during `os::prepare_module` on Unix.
pub(crate) fn install_posix_on_os(interp: &mut Interpreter, os_rc: &Rc<RefCell<PyModule>>) {
    let Ok(posix_val) = interp.load_module("posix") else {
        return;
    };
    let ValueKind::PyModule(posix_rc) = posix_val.kind() else {
        return;
    };
    let (exports, environb_inst) = {
        let posix_borrowed = posix_rc.borrow();
        let mut pairs = Vec::with_capacity(POSIX_TO_OS_EXPORTS.len());
        for &name in POSIX_TO_OS_EXPORTS {
            if let Some(val) = posix_borrowed.attrs.get(name) {
                pairs.push((name, val.clone()));
            }
        }
        let environb = posix_borrowed.attrs.get("_EnvironB").and_then(|v| {
            if let ValueKind::PyClass(cls) = v.kind() {
                Some(Value::py_instance(Rc::new(RefCell::new(PyInstance {
                    class: Rc::clone(cls),
                    attrs: InstanceAttrs::new(),
                }))))
            } else {
                None
            }
        });
        (pairs, environb)
    };

    let mut os_mut = os_rc.borrow_mut();
    for (name, val) in exports {
        os_mut.insert_attr(name.to_string(), val);
    }
    if let Some(envb) = environb_inst {
        os_mut.insert_attr("environb".to_string(), envb);
    }
}

pyrust_module! {
    constants {
        "error" => Value::py_class(
            lookup_exc_class("OSError").expect("canonical OSError class must exist"),
        ),
        "uname_result" => uname_result::class(),
        "statvfs_result" => statvfs_result::class(),

        // Wait options
        "WNOHANG" => Value::int(libc::WNOHANG as i64),
        "WUNTRACED" => Value::int(libc::WUNTRACED as i64),
        "WCONTINUED" => Value::int(libc::WCONTINUED as i64),

        // Standard sysexits.h exit codes
        "EX_OK" => Value::int(0),
        "EX_USAGE" => Value::int(64),
        "EX_DATAERR" => Value::int(65),
        "EX_NOINPUT" => Value::int(66),
        "EX_NOUSER" => Value::int(67),
        "EX_NOHOST" => Value::int(68),
        "EX_UNAVAILABLE" => Value::int(69),
        "EX_SOFTWARE" => Value::int(70),
        "EX_OSERR" => Value::int(71),
        "EX_OSFILE" => Value::int(72),
        "EX_CANTCREAT" => Value::int(73),
        "EX_IOERR" => Value::int(74),
        "EX_TEMPFAIL" => Value::int(75),
        "EX_PROTOCOL" => Value::int(76),
        "EX_NOPERM" => Value::int(77),
        "EX_CONFIG" => Value::int(78),

        // Configuration name tables
        "pathconf_names" => make_conf_dict(PATHCONF_ENTRIES),
        "sysconf_names" => make_conf_dict(SYSCONF_ENTRIES),
        "confstr_names" => make_conf_dict(CONFSTR_ENTRIES),
    }

    /// Return information identifying the current operating system.
    fn uname() -> Result<Value> {
        // SAFETY: zeroed `libc::utsname` is valid to pass as out-pointer.
        let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
        if unsafe { libc::uname(&mut uts) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(uname_result::make([
            Value::string(c_array_to_string(&uts.sysname)),
            Value::string(c_array_to_string(&uts.nodename)),
            Value::string(c_array_to_string(&uts.release)),
            Value::string(c_array_to_string(&uts.version)),
            Value::string(c_array_to_string(&uts.machine)),
        ]))
    }

    /// Fork a child process. Returns 0 in child and child's PID in parent.
    fn fork() -> Result<Value> {
        // SAFETY: standard POSIX `fork`.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::int(pid as i64))
    }

    /// Exit the process with status `status`, without calling cleanup handlers.
    fn _exit(status: PyIndex) -> Result<Value> {
        let code = status.expect_i64(FN_NAME, "status")? as libc::c_int;
        // SAFETY: `_exit` terminates the current process immediately.
        unsafe { libc::_exit(code) }
    }

    /// Open a new pseudo-terminal pair `(master_fd, slave_fd)`.
    fn openpty() -> Result<Value> {
        let (master, slave) = super::pty::openpty_raw()?;
        Ok(Value::tuple(vec![
            Value::int(master as i64),
            Value::int(slave as i64),
        ]))
    }

    /// Return the current process's real user id.
    fn getuid() -> Result<Value> {
        // SAFETY: `getuid` has no preconditions.
        Ok(Value::int(unsafe { libc::getuid() } as i64))
    }

    /// Return the current process's effective user id.
    fn geteuid() -> Result<Value> {
        // SAFETY: `geteuid` has no preconditions.
        Ok(Value::int(unsafe { libc::geteuid() } as i64))
    }

    /// Return the current process's real group id.
    fn getgid() -> Result<Value> {
        // SAFETY: `getgid` has no preconditions.
        Ok(Value::int(unsafe { libc::getgid() } as i64))
    }

    /// Return the current process's effective group id.
    fn getegid() -> Result<Value> {
        // SAFETY: `getegid` has no preconditions.
        Ok(Value::int(unsafe { libc::getegid() } as i64))
    }

    /// Set the current process's user id.
    fn setuid(uid: PyIndex) -> Result<Value> {
        let u = uid.expect_i64(FN_NAME, "uid")? as libc::uid_t;
        if unsafe { libc::setuid(u) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Set the current process's effective user id.
    fn seteuid(euid: PyIndex) -> Result<Value> {
        let u = euid.expect_i64(FN_NAME, "euid")? as libc::uid_t;
        if unsafe { libc::seteuid(u) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Set the current process's group id.
    fn setgid(gid: PyIndex) -> Result<Value> {
        let g = gid.expect_i64(FN_NAME, "gid")? as libc::gid_t;
        if unsafe { libc::setgid(g) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Set the current process's effective group id.
    fn setegid(egid: PyIndex) -> Result<Value> {
        let g = egid.expect_i64(FN_NAME, "egid")? as libc::gid_t;
        if unsafe { libc::setegid(g) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Set the current process's real and effective user ids.
    fn setreuid(ruid: PyIndex, euid: PyIndex) -> Result<Value> {
        let r = ruid.expect_i64(FN_NAME, "ruid")? as libc::uid_t;
        let e = euid.expect_i64(FN_NAME, "euid")? as libc::uid_t;
        if unsafe { libc::setreuid(r, e) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Set the current process's real and effective group ids.
    fn setregid(rgid: PyIndex, egid: PyIndex) -> Result<Value> {
        let r = rgid.expect_i64(FN_NAME, "rgid")? as libc::gid_t;
        let e = egid.expect_i64(FN_NAME, "egid")? as libc::gid_t;
        if unsafe { libc::setregid(r, e) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Return list of supplemental group IDs associated with the current process.
    fn getgroups() -> Result<Value> {
        // SAFETY: `getgroups(0, NULL)` queries the count of supplemental groups.
        let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
        if count < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        if count == 0 {
            return Ok(Value::list(Vec::new()));
        }
        let mut buf: Vec<libc::gid_t> = vec![0; count as usize];
        // SAFETY: `buf` has capacity `count`.
        let n = unsafe { libc::getgroups(count, buf.as_mut_ptr()) };
        if n < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        let list = buf[..n as usize]
            .iter()
            .map(|&g| Value::int(g as i64))
            .collect();
        Ok(Value::list(list))
    }

    /// Return the id of the current process group.
    fn getpgrp() -> Result<Value> {
        // SAFETY: `getpgrp` takes no arguments.
        Ok(Value::int(unsafe { libc::getpgrp() } as i64))
    }

    /// Call the system call `setpgrp()` (equivalent to `setpgid(0, 0)`).
    fn setpgrp() -> Result<Value> {
        // SAFETY: POSIX `setpgid(0, 0)` is portable across Linux and macOS.
        if unsafe { libc::setpgid(0, 0) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Return the process group id of the process with process id `pid`.
    fn getpgid(pid: PyIndex) -> Result<Value> {
        let p = pid.expect_i64(FN_NAME, "pid")? as libc::pid_t;
        // SAFETY: standard POSIX `getpgid`.
        let res = unsafe { libc::getpgid(p) };
        if res < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::int(res as i64))
    }

    /// Call the system call `setpgid(pid, pgrp)`.
    fn setpgid(pid: PyIndex, pgrp: PyIndex) -> Result<Value> {
        let p = pid.expect_i64(FN_NAME, "pid")? as libc::pid_t;
        let g = pgrp.expect_i64(FN_NAME, "pgrp")? as libc::pid_t;
        // SAFETY: standard POSIX `setpgid`.
        if unsafe { libc::setpgid(p, g) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Return the session id of the process with process id `pid`.
    fn getsid(pid: PyIndex) -> Result<Value> {
        let p = pid.expect_i64(FN_NAME, "pid")? as libc::pid_t;
        // SAFETY: standard POSIX `getsid`.
        let res = unsafe { libc::getsid(p) };
        if res < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::int(res as i64))
    }

    /// Create a new session and return its session id.
    fn setsid() -> Result<Value> {
        // SAFETY: standard POSIX `setsid`.
        let res = unsafe { libc::setsid() };
        if res < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::int(res as i64))
    }

    /// Send signal `sig` to the process group `pgid`.
    fn killpg(pgid: PyIndex, sig: PyIndex) -> Result<Value> {
        let pg = pgid.expect_i64(FN_NAME, "pgid")? as libc::pid_t;
        let s = sig.expect_i64(FN_NAME, "sig")? as libc::c_int;
        // SAFETY: standard POSIX `killpg`.
        if unsafe { libc::killpg(pg, s) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Change the owner and group id of `path` to numeric `uid` and `gid`.
    fn chown(
        path: PyValue,
        uid: PyIndex,
        gid: PyIndex,
        #[keyword_only]
        #[default(None)]
        dir_fd: Option<PyIndex>,
        #[keyword_only]
        #[default(true.into())]
        follow_symlinks: PyBool,
    ) -> Result<Value> {
        let u = uid.expect_i64(FN_NAME, "uid")? as libc::uid_t;
        let g = gid.expect_i64(FN_NAME, "gid")? as libc::gid_t;
        match path.0.kind() {
            ValueKind::Int(fd) => {
                if dir_fd.is_some() {
                    return Err(PyError::named(
                        "ValueError",
                        "chown: can't specify both dir_fd and fd".to_string(),
                    ));
                }
                let raw_fd = c_int_from_i64(fd)?;
                if unsafe { libc::fchown(raw_fd, u, g) } < 0 {
                    return Err(PyError::from_io_error(
                        &std::io::Error::last_os_error(),
                        None,
                    ));
                }
                return Ok(Value::none());
            }
            ValueKind::Bool(b) => {
                let raw_fd = libc::c_int::from(b);
                if unsafe { libc::fchown(raw_fd, u, g) } < 0 {
                    return Err(PyError::from_io_error(
                        &std::io::Error::last_os_error(),
                        None,
                    ));
                }
                return Ok(Value::none());
            }
            _ => {}
        }
        let py_path = PyPath::try_from_value_with_interp(_interp, &path.0, FN_NAME, "path")?;
        let cpath = path_to_cstring(&py_path)?;
        let disp = py_path.as_path().to_string_lossy();
        let rc = if let Some(dfd) = dir_fd {
            let raw_dfd = c_int_from_i64(dfd.expect_i64(FN_NAME, "dir_fd")?)?;
            let flags = if *follow_symlinks {
                0
            } else {
                libc::AT_SYMLINK_NOFOLLOW
            };
            // SAFETY: `cpath` is NUL-terminated.
            unsafe { libc::fchownat(raw_dfd, cpath.as_ptr(), u, g, flags) }
        } else if *follow_symlinks {
            // SAFETY: `cpath` is NUL-terminated.
            unsafe { libc::chown(cpath.as_ptr(), u, g) }
        } else {
            // SAFETY: `cpath` is NUL-terminated.
            unsafe { libc::lchown(cpath.as_ptr(), u, g) }
        };
        if rc < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                Some(&disp),
            ));
        }
        Ok(Value::none())
    }

    /// Change the owner and group id of `path` without following symlinks.
    fn lchown(path: PyPath, uid: PyIndex, gid: PyIndex) -> Result<Value> {
        let u = uid.expect_i64(FN_NAME, "uid")? as libc::uid_t;
        let g = gid.expect_i64(FN_NAME, "gid")? as libc::gid_t;
        let cpath = path_to_cstring(&path)?;
        let disp = path.as_path().to_string_lossy();
        // SAFETY: `cpath` is NUL-terminated.
        if unsafe { libc::lchown(cpath.as_ptr(), u, g) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                Some(&disp),
            ));
        }
        Ok(Value::none())
    }

    /// Change the owner and group id of the file given by file descriptor `fd`.
    fn fchown(fd: PyIndex, uid: PyIndex, gid: PyIndex) -> Result<Value> {
        let raw_fd = c_int_from_i64(fd.expect_i64(FN_NAME, "fd")?)?;
        let u = uid.expect_i64(FN_NAME, "uid")? as libc::uid_t;
        let g = gid.expect_i64(FN_NAME, "gid")? as libc::gid_t;
        // SAFETY: standard POSIX `fchown`.
        if unsafe { libc::fchown(raw_fd, u, g) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Perform a `statvfs()` system call on the given path or file descriptor.
    fn statvfs(path: PyValue) -> Result<Value> {
        match path.0.kind() {
            ValueKind::Int(fd) => {
                let raw_fd = c_int_from_i64(fd)?;
                // SAFETY: zero-initialized `libc::statvfs` out-pointer.
                let mut sv: libc::statvfs = unsafe { std::mem::zeroed() };
                if unsafe { libc::fstatvfs(raw_fd, &mut sv) } < 0 {
                    return Err(PyError::from_io_error(
                        &std::io::Error::last_os_error(),
                        None,
                    ));
                }
                return Ok(statvfs_to_value(&sv));
            }
            ValueKind::Bool(b) => {
                let raw_fd = libc::c_int::from(b);
                let mut sv: libc::statvfs = unsafe { std::mem::zeroed() };
                if unsafe { libc::fstatvfs(raw_fd, &mut sv) } < 0 {
                    return Err(PyError::from_io_error(
                        &std::io::Error::last_os_error(),
                        None,
                    ));
                }
                return Ok(statvfs_to_value(&sv));
            }
            _ => {}
        }
        let py_path = PyPath::try_from_value_with_interp(_interp, &path.0, FN_NAME, "path")?;
        let cpath = path_to_cstring(&py_path)?;
        let disp = py_path.as_path().to_string_lossy();
        // SAFETY: `cpath` is NUL-terminated and `sv` is a valid out-pointer.
        let mut sv: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(cpath.as_ptr(), &mut sv) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                Some(&disp),
            ));
        }
        Ok(statvfs_to_value(&sv))
    }

    /// Return filesystem information for the file referenced by descriptor `fd`.
    fn fstatvfs(fd: PyIndex) -> Result<Value> {
        let raw_fd = c_int_from_i64(fd.expect_i64(FN_NAME, "fd")?)?;
        // SAFETY: zero-initialized `libc::statvfs` out-pointer.
        let mut sv: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatvfs(raw_fd, &mut sv) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(statvfs_to_value(&sv))
    }

    /// Create a FIFO (a POSIX named pipe).
    fn mkfifo(
        path: PyPath,
        #[default(0o666_i64.into())] mode: PyIndex,
    ) -> Result<Value> {
        let m = mode.expect_i64(FN_NAME, "mode")? as libc::mode_t;
        let cpath = path_to_cstring(&path)?;
        let disp = path.as_path().to_string_lossy();
        // SAFETY: `cpath` is NUL-terminated.
        if unsafe { libc::mkfifo(cpath.as_ptr(), m) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                Some(&disp),
            ));
        }
        Ok(Value::none())
    }

    /// Create a filesystem node (file, device special file or named pipe).
    fn mknod(
        path: PyPath,
        #[default(0o600_i64.into())] mode: PyIndex,
        #[default(0_i64.into())] device: PyIndex,
    ) -> Result<Value> {
        let m = mode.expect_i64(FN_NAME, "mode")? as libc::mode_t;
        let dev = device.expect_i64(FN_NAME, "device")? as libc::dev_t;
        let cpath = path_to_cstring(&path)?;
        let disp = path.as_path().to_string_lossy();
        // SAFETY: `cpath` is NUL-terminated.
        if unsafe { libc::mknod(cpath.as_ptr(), m, dev) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                Some(&disp),
            ));
        }
        Ok(Value::none())
    }

    /// Wait for completion of a child process; returns `(pid, status)`.
    fn wait() -> Result<Value> {
        let mut status: libc::c_int = 0;
        // SAFETY: `status` is a valid out-pointer.
        let pid = unsafe { libc::wait(&mut status) };
        if pid < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::tuple(vec![
            Value::int(pid as i64),
            Value::int(status as i64),
        ]))
    }

    /// Return True if the process exited normally (`exit()`).
    #[py_name = "WIFEXITED"]
    fn wifexited(status: PyIndex) -> Result<Value> {
        let st = status.expect_i64(FN_NAME, "status")? as libc::c_int;
        Ok(Value::bool_(libc::WIFEXITED(st)))
    }

    /// Return the process exit status code if `WIFEXITED(status)` is True.
    #[py_name = "WEXITSTATUS"]
    fn wexitstatus(status: PyIndex) -> Result<Value> {
        let st = status.expect_i64(FN_NAME, "status")? as libc::c_int;
        Ok(Value::int(libc::WEXITSTATUS(st) as i64))
    }

    /// Return True if the process exited due to a signal.
    #[py_name = "WIFSIGNALED"]
    fn wifsignaled(status: PyIndex) -> Result<Value> {
        let st = status.expect_i64(FN_NAME, "status")? as libc::c_int;
        Ok(Value::bool_(libc::WIFSIGNALED(st)))
    }

    /// Return the signal that caused the process to exit.
    #[py_name = "WTERMSIG"]
    fn wtermsig(status: PyIndex) -> Result<Value> {
        let st = status.expect_i64(FN_NAME, "status")? as libc::c_int;
        Ok(Value::int(libc::WTERMSIG(st) as i64))
    }

    /// Return True if the process was stopped.
    #[py_name = "WIFSTOPPED"]
    fn wifstopped(status: PyIndex) -> Result<Value> {
        let st = status.expect_i64(FN_NAME, "status")? as libc::c_int;
        Ok(Value::bool_(libc::WIFSTOPPED(st)))
    }

    /// Return the signal that caused the process to stop.
    #[py_name = "WSTOPSIG"]
    fn wstopsig(status: PyIndex) -> Result<Value> {
        let st = status.expect_i64(FN_NAME, "status")? as libc::c_int;
        Ok(Value::int(libc::WSTOPSIG(st) as i64))
    }

    /// Return True if the process was resumed from a job-control stop.
    #[py_name = "WIFCONTINUED"]
    fn wifcontinued(status: PyIndex) -> Result<Value> {
        let st = status.expect_i64(FN_NAME, "status")? as libc::c_int;
        Ok(Value::bool_(libc::WIFCONTINUED(st)))
    }

    /// Return True if a core dump was generated for the process.
    #[py_name = "WCOREDUMP"]
    fn wcoredump(status: PyIndex) -> Result<Value> {
        let st = status.expect_i64(FN_NAME, "status")? as libc::c_int;
        Ok(Value::bool_(libc::WCOREDUMP(st)))
    }

    /// Return the configuration limit `name` for the file or directory `path`.
    fn pathconf(path: PyValue, name: PyValue) -> Result<Value> {
        let conf_id = resolve_conf_name(&name.0, PATHCONF_ENTRIES, FN_NAME)?;
        match path.0.kind() {
            ValueKind::Int(fd) => {
                let raw_fd = c_int_from_i64(fd)?;
                clear_errno();
                // SAFETY: standard POSIX `fpathconf`.
                let val = unsafe { libc::fpathconf(raw_fd, conf_id) };
                if val < 0 {
                    let e = last_errno();
                    if e != 0 {
                        return Err(PyError::from_io_error(
                            &std::io::Error::from_raw_os_error(e),
                            None,
                        ));
                    }
                }
                return Ok(Value::int(i64::from(val)));
            }
            ValueKind::Bool(b) => {
                let raw_fd = libc::c_int::from(b);
                clear_errno();
                let val = unsafe { libc::fpathconf(raw_fd, conf_id) };
                if val < 0 {
                    let e = last_errno();
                    if e != 0 {
                        return Err(PyError::from_io_error(
                            &std::io::Error::from_raw_os_error(e),
                            None,
                        ));
                    }
                }
                return Ok(Value::int(i64::from(val)));
            }
            _ => {}
        }
        let py_path = PyPath::try_from_value_with_interp(_interp, &path.0, FN_NAME, "path")?;
        let cpath = path_to_cstring(&py_path)?;
        let disp = py_path.as_path().to_string_lossy();
        clear_errno();
        // SAFETY: `cpath` is NUL-terminated.
        let val = unsafe { libc::pathconf(cpath.as_ptr(), conf_id) };
        if val < 0 {
            let e = last_errno();
            if e != 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::from_raw_os_error(e),
                    Some(&disp),
                ));
            }
        }
        Ok(Value::int(i64::from(val)))
    }

    /// Return the configuration limit `name` for the file descriptor `fd`.
    fn fpathconf(fd: PyIndex, name: PyValue) -> Result<Value> {
        let raw_fd = c_int_from_i64(fd.expect_i64(FN_NAME, "fd")?)?;
        let conf_id = resolve_conf_name(&name.0, PATHCONF_ENTRIES, FN_NAME)?;
        clear_errno();
        // SAFETY: standard POSIX `fpathconf`.
        let val = unsafe { libc::fpathconf(raw_fd, conf_id) };
        if val < 0 {
            let e = last_errno();
            if e != 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::from_raw_os_error(e),
                    None,
                ));
            }
        }
        Ok(Value::int(i64::from(val)))
    }

    /// Return an integer-valued system configuration value.
    fn sysconf(name: PyValue) -> Result<Value> {
        let conf_id = resolve_conf_name(&name.0, SYSCONF_ENTRIES, FN_NAME)?;
        clear_errno();
        // SAFETY: standard POSIX `sysconf`.
        let val = unsafe { libc::sysconf(conf_id) };
        if val < 0 {
            let e = last_errno();
            if e != 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::from_raw_os_error(e),
                    None,
                ));
            }
        }
        Ok(Value::int(i64::from(val)))
    }

    /// Return a string-valued system configuration value.
    fn confstr(name: PyValue) -> Result<Value> {
        let conf_id = resolve_conf_name(&name.0, CONFSTR_ENTRIES, FN_NAME)?;
        clear_errno();
        // SAFETY: `confstr(id, NULL, 0)` queries required buffer length.
        let len = unsafe { libc::confstr(conf_id, std::ptr::null_mut(), 0) };
        if len == 0 {
            let e = last_errno();
            if e != 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::from_raw_os_error(e),
                    None,
                ));
            }
            return Ok(Value::none());
        }
        let mut buf: Vec<libc::c_char> = vec![0; len];
        clear_errno();
        // SAFETY: `buf` has length `len`.
        let written = unsafe { libc::confstr(conf_id, buf.as_mut_ptr(), buf.len()) };
        if written == 0 {
            let e = last_errno();
            if e != 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::from_raw_os_error(e),
                    None,
                ));
            }
            return Ok(Value::none());
        }
        Ok(Value::string(c_array_to_string(&buf)))
    }

    /// Return the number of processes in the system run queue averaged over
    /// the last 1, 5, and 15 minutes.
    fn getloadavg() -> Result<Value> {
        let mut avg = [0.0_f64; 3];
        // SAFETY: `avg` holds 3 `c_double` elements.
        let n = unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) };
        if n != 3 {
            return Err(PyError::named(
                "OSError",
                "Load averages are unobtainable".to_string(),
            ));
        }
        Ok(Value::tuple(vec![
            Value::float(avg[0]),
            Value::float(avg[1]),
            Value::float(avg[2]),
        ]))
    }

    /// Force write of everything to disk.
    fn sync() -> Result<Value> {
        // SAFETY: POSIX `sync()` takes no arguments.
        unsafe { libc::sync() };
        Ok(Value::none())
    }

    /// Add `increment` to the process's niceness and return the new niceness.
    fn nice(increment: PyIndex) -> Result<Value> {
        let inc = c_int_from_i64(increment.expect_i64(FN_NAME, "increment")?)?;
        clear_errno();
        // SAFETY: standard POSIX `nice`.
        let res = unsafe { libc::nice(inc) };
        if res == -1 {
            let e = last_errno();
            if e != 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::from_raw_os_error(e),
                    None,
                ));
            }
        }
        Ok(Value::int(res as i64))
    }

    /// Return the filename corresponding to the controlling terminal of the process.
    fn ctermid() -> Result<Value> {
        let mut buf: [libc::c_char; 256] = [0; 256];
        #[cfg(target_os = "macos")]
        let ptr = {
            unsafe extern "C" {
                fn ctermid(s: *mut libc::c_char) -> *mut libc::c_char;
            }
            // SAFETY: `buf` is at least `L_ctermid` bytes.
            unsafe { ctermid(buf.as_mut_ptr()) }
        };
        #[cfg(not(target_os = "macos"))]
        let ptr = {
            // SAFETY: `buf` is at least `L_ctermid` bytes.
            unsafe { libc::ctermid(buf.as_mut_ptr()) }
        };
        if ptr.is_null() {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        // SAFETY: `ptr` points to a NUL-terminated string.
        let s = unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        Ok(Value::string(s))
    }

    /// Return a string specifying the terminal device associated with `fd`.
    fn ttyname(fd: PyIndex) -> Result<Value> {
        let raw_fd = c_int_from_i64(fd.expect_i64(FN_NAME, "fd")?)?;
        // SAFETY: `ttyname` returns a pointer to a static NUL-terminated string or NULL.
        let ptr = unsafe { libc::ttyname(raw_fd) };
        if ptr.is_null() {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        // SAFETY: `ptr` is non-null and NUL-terminated.
        let name = unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        Ok(Value::string(name))
    }

    /// Return the process group associated with the terminal given by `fd`.
    fn tcgetpgrp(fd: PyIndex) -> Result<Value> {
        let raw_fd = c_int_from_i64(fd.expect_i64(FN_NAME, "fd")?)?;
        // SAFETY: standard POSIX `tcgetpgrp`.
        let pg = unsafe { libc::tcgetpgrp(raw_fd) };
        if pg < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::int(pg as i64))
    }

    /// Set the process group associated with the terminal given by `fd` to `pg`.
    fn tcsetpgrp(fd: PyIndex, pg: PyIndex) -> Result<Value> {
        let raw_fd = c_int_from_i64(fd.expect_i64(FN_NAME, "fd")?)?;
        let pgid = pg.expect_i64(FN_NAME, "pg")? as libc::pid_t;
        // SAFETY: standard POSIX `tcsetpgrp`.
        if unsafe { libc::tcsetpgrp(raw_fd, pgid) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Return the value of the environment variable `key` as `bytes`, or `default`.
    fn getenvb(
        key: PyBytes,
        #[default(None)] default: Option<PyValue>,
    ) -> Result<Value> {
        let os_key = OsStr::from_bytes(key.0.as_slice());
        let _guard = super::os::env_lock();
        match std::env::var_os(os_key) {
            Some(val) => Ok(Value::bytes(val.as_bytes().to_vec())),
            None => Ok(default.map_or_else(Value::none, |v| v.0)),
        }
    }

    /// Live bytes-keyed environment mapping backing `os.environb`.
    class _EnvironB {
        fn __getitem__(
            #[positional_only] _self: PyValueRef<'_>,
            key: PyValue,
        ) -> Result<Value> {
            let k_bytes = require_env_bytes(&key.0)?;
            let os_key = OsStr::from_bytes(k_bytes);
            let _guard = super::os::env_lock();
            match std::env::var_os(os_key) {
                Some(v) => Ok(Value::bytes(v.as_bytes().to_vec())),
                None => Err(PyError::key_error(key.0)),
            }
        }

        fn __setitem__(
            #[positional_only] _self: PyValueRef<'_>,
            key: PyValue,
            value: PyValue,
        ) -> Result<Value> {
            let k_bytes = require_env_bytes(&key.0)?;
            let v_bytes = require_env_bytes(&value.0)?;
            validate_env_key_bytes(k_bytes)?;
            validate_env_val_bytes(v_bytes)?;
            let os_key = OsStr::from_bytes(k_bytes);
            let os_val = OsStr::from_bytes(v_bytes);
            let _guard = super::os::env_lock();
            // SAFETY: serialized by `os::env_lock()`.
            unsafe { std::env::set_var(os_key, os_val) };
            Ok(Value::none())
        }

        fn __delitem__(
            #[positional_only] _self: PyValueRef<'_>,
            key: PyValue,
        ) -> Result<Value> {
            let k_bytes = require_env_bytes(&key.0)?;
            let os_key = OsStr::from_bytes(k_bytes);
            let _guard = super::os::env_lock();
            if std::env::var_os(os_key).is_none() {
                return Err(PyError::key_error(key.0));
            }
            // SAFETY: serialized by `os::env_lock()`.
            unsafe { std::env::remove_var(os_key) };
            Ok(Value::none())
        }

        fn __contains__(
            #[positional_only] _self: PyValueRef<'_>,
            key: PyValue,
        ) -> Result<Value> {
            let Ok(k_bytes) = require_env_bytes(&key.0) else {
                return Ok(Value::bool_(false));
            };
            let os_key = OsStr::from_bytes(k_bytes);
            let _guard = super::os::env_lock();
            Ok(Value::bool_(std::env::var_os(os_key).is_some()))
        }

        fn __iter__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _guard = super::os::env_lock();
            let keys: Vec<Value> = std::env::vars_os()
                .map(|(k, _)| Value::bytes(k.as_bytes().to_vec()))
                .collect();
            Ok(Value::generator(Box::new(NativeIterFrame::generator(keys))))
        }

        fn __len__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _guard = super::os::env_lock();
            Ok(Value::int(std::env::vars_os().count() as i64))
        }

        fn __repr__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _guard = super::os::env_lock();
            let count = std::env::vars_os().count();
            Ok(Value::string(format!("environ({{...{count} entries...}})")))
        }

        fn get(
            #[positional_only] _self: PyValueRef<'_>,
            key: PyValue,
            #[default(None)] default: Option<PyValue>,
        ) -> Result<Value> {
            let Ok(k_bytes) = require_env_bytes(&key.0) else {
                return Ok(default.map_or_else(Value::none, |v| v.0));
            };
            let os_key = OsStr::from_bytes(k_bytes);
            let _guard = super::os::env_lock();
            match std::env::var_os(os_key) {
                Some(v) => Ok(Value::bytes(v.as_bytes().to_vec())),
                None => Ok(default.map_or_else(Value::none, |v| v.0)),
            }
        }

        fn keys(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _guard = super::os::env_lock();
            let keys = std::env::vars_os()
                .map(|(k, _)| Value::bytes(k.as_bytes().to_vec()))
                .collect();
            Ok(Value::list(keys))
        }

        fn values(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _guard = super::os::env_lock();
            let values = std::env::vars_os()
                .map(|(_, v)| Value::bytes(v.as_bytes().to_vec()))
                .collect();
            Ok(Value::list(values))
        }

        fn items(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _guard = super::os::env_lock();
            let items = std::env::vars_os()
                .map(|(k, v)| {
                    Value::tuple(vec![
                        Value::bytes(k.as_bytes().to_vec()),
                        Value::bytes(v.as_bytes().to_vec()),
                    ])
                })
                .collect();
            Ok(Value::list(items))
        }

        fn pop(
            #[positional_only] _self: PyValueRef<'_>,
            key: PyValue,
            #[varargs] default: &[Value],
        ) -> Result<Value> {
            if default.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "pop expected at most 2 arguments, got {}",
                        default.len() + 1
                    ),
                ));
            }
            let k_bytes = require_env_bytes(&key.0)?;
            let os_key = OsStr::from_bytes(k_bytes);
            let _guard = super::os::env_lock();
            match std::env::var_os(os_key) {
                Some(v) => {
                    // SAFETY: serialized by `os::env_lock()`.
                    unsafe { std::env::remove_var(os_key) };
                    Ok(Value::bytes(v.as_bytes().to_vec()))
                }
                None => match default.first() {
                    Some(d) => Ok(d.clone()),
                    None => Err(PyError::key_error(key.0)),
                },
            }
        }

        fn copy(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(build_posix_environ_dict())
        }
    }
}
