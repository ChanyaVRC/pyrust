// `fcntl` module — 100% native Rust implementation of Python 3.12's Unix
// file control and I/O control interface (`fcntl`, `ioctl`, `flock`, `lockf`)
// (issue #3261).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs`.
// Reference: <https://docs.python.org/3.12/library/fcntl.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyBytesLike, PyIndex, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{Value, ValueKind};

#[cfg(target_os = "linux")]
type IoctlReq = libc::Ioctl;
#[cfg(not(target_os = "linux"))]
type IoctlReq = libc::c_ulong;

const IOCTL_BUFSIZE: usize = 1024;

/// Extract a non-negative `c_int` file descriptor from an `int` or an object
/// implementing `.fileno()`.
pub(crate) fn extract_c_int_fd(interp: &mut Interpreter, obj: &Value) -> Result<libc::c_int> {
    let fd = match obj.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        ValueKind::BigInt(_) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            ));
        }
        _ => {
            let fileno_method = interp.get_attr(obj, "fileno").map_err(|_| {
                PyError::named(
                    "TypeError",
                    format!(
                        "argument must be an int, or have a fileno() method, not '{}'",
                        value_type_name_str(obj)
                    ),
                )
            })?;
            let fd_val = interp.call_function_expanded(fileno_method, &[])?;
            match fd_val.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                ValueKind::BigInt(_) => {
                    return Err(PyError::named(
                        "OverflowError",
                        "Python int too large to convert to C int".to_string(),
                    ));
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "fileno() returned a non-integer (type {})",
                            value_type_name_str(&fd_val)
                        ),
                    ));
                }
            }
        }
    };
    if fd < 0 {
        return Err(PyError::named(
            "ValueError",
            format!("file descriptor cannot be a negative integer ({fd})"),
        ));
    }
    libc::c_int::try_from(fd).map_err(|_| {
        PyError::named(
            "OverflowError",
            "Python int too large to convert to C int".to_string(),
        )
    })
}

fn try_extract_int(val: &Value) -> Option<i64> {
    match val.kind() {
        ValueKind::Int(n) => Some(n),
        ValueKind::Bool(b) => Some(i64::from(b)),
        _ => None,
    }
}

pyrust_module! {
    constants {
        "LOCK_SH" => Value::int(libc::LOCK_SH as i64),
        "LOCK_EX" => Value::int(libc::LOCK_EX as i64),
        "LOCK_NB" => Value::int(libc::LOCK_NB as i64),
        "LOCK_UN" => Value::int(libc::LOCK_UN as i64),

        "F_DUPFD" => Value::int(libc::F_DUPFD as i64),
        "F_DUPFD_CLOEXEC" => Value::int(libc::F_DUPFD_CLOEXEC as i64),
        "F_GETFD" => Value::int(libc::F_GETFD as i64),
        "F_SETFD" => Value::int(libc::F_SETFD as i64),
        "F_GETFL" => Value::int(libc::F_GETFL as i64),
        "F_SETFL" => Value::int(libc::F_SETFL as i64),
        "F_GETLK" => Value::int(libc::F_GETLK as i64),
        "F_SETLK" => Value::int(libc::F_SETLK as i64),
        "F_SETLKW" => Value::int(libc::F_SETLKW as i64),
        "F_GETOWN" => Value::int(libc::F_GETOWN as i64),
        "F_SETOWN" => Value::int(libc::F_SETOWN as i64),

        "F_RDLCK" => Value::int(libc::F_RDLCK as i64),
        "F_WRLCK" => Value::int(libc::F_WRLCK as i64),
        "F_UNLCK" => Value::int(libc::F_UNLCK as i64),
        "FD_CLOEXEC" => Value::int(libc::FD_CLOEXEC as i64),

        #[cfg(target_os = "linux")]
        "F_SETLEASE" => Value::int(libc::F_SETLEASE as i64),
        #[cfg(target_os = "linux")]
        "F_GETLEASE" => Value::int(libc::F_GETLEASE as i64),
        #[cfg(target_os = "linux")]
        "F_SETPIPE_SZ" => Value::int(libc::F_SETPIPE_SZ as i64),
        #[cfg(target_os = "linux")]
        "F_GETPIPE_SZ" => Value::int(libc::F_GETPIPE_SZ as i64),

        #[cfg(target_os = "macos")]
        "F_FULLFSYNC" => Value::int(libc::F_FULLFSYNC as i64),
        #[cfg(target_os = "macos")]
        "F_GETPATH" => Value::int(libc::F_GETPATH as i64),
        #[cfg(target_os = "macos")]
        "F_NOCACHE" => Value::int(libc::F_NOCACHE as i64),
    }

    /// Perform the operation `cmd` on file descriptor `fd`.
    fn fcntl(
        #[positional_only] fd: PyValue,
        #[positional_only] cmd: PyIndex,
        #[positional_only]
        #[default(None)]
        arg: Option<PyValue>,
    ) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        let raw_cmd = cmd.expect_i64(FN_NAME, "cmd")? as libc::c_int;

        let Some(arg_val) = arg else {
            // SAFETY: standard POSIX `fcntl` with integer argument 0.
            let rc = unsafe { libc::fcntl(raw_fd, raw_cmd, 0_i32) };
            if rc < 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    None,
                ));
            }
            return Ok(Value::int(rc as i64));
        };

        if let Some(int_arg) = try_extract_int(&arg_val.0) {
            // SAFETY: standard POSIX `fcntl` with integer argument.
            let rc = unsafe { libc::fcntl(raw_fd, raw_cmd, int_arg as libc::c_int) };
            if rc < 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    None,
                ));
            }
            return Ok(Value::int(rc as i64));
        }

        // Bytes-like buffer argument.
        let bytes_like = PyBytesLike::try_from_value_with_interp(_interp, &arg_val.0, FN_NAME, "arg")?;
        let slice = bytes_like.as_bytes();
        if slice.len() > IOCTL_BUFSIZE {
            return Err(PyError::named(
                "ValueError",
                "fcntl string arg too long".to_string(),
            ));
        }
        let mut buf = [0u8; IOCTL_BUFSIZE];
        buf[..slice.len()].copy_from_slice(slice);
        // SAFETY: `buf` is a 1024-byte stack buffer passed to POSIX `fcntl`.
        let rc = unsafe { libc::fcntl(raw_fd, raw_cmd, buf.as_mut_ptr()) };
        if rc < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::bytes(buf[..slice.len()].to_vec()))
    }

    /// Perform the operation `request` on file descriptor `fd`.
    fn ioctl(
        #[positional_only] fd: PyValue,
        #[positional_only] request: PyIndex,
        #[positional_only]
        #[default(None)]
        arg: Option<PyValue>,
        #[positional_only]
        #[default(true.into())]
        mutate_flag: PyBool,
    ) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        let req_i64 = request.expect_i64(FN_NAME, "request")?;
        #[allow(clippy::unnecessary_cast)]
        let raw_req = (req_i64 as u64) as IoctlReq;

        let Some(arg_val) = arg else {
            // SAFETY: POSIX `ioctl` with integer argument 0.
            let rc = unsafe { libc::ioctl(raw_fd, raw_req, 0_i32) };
            if rc < 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    None,
                ));
            }
            return Ok(Value::int(rc as i64));
        };

        if let Some(int_arg) = try_extract_int(&arg_val.0) {
            // SAFETY: POSIX `ioctl` with integer argument.
            let rc = unsafe { libc::ioctl(raw_fd, raw_req, int_arg as libc::c_int) };
            if rc < 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    None,
                ));
            }
            return Ok(Value::int(rc as i64));
        }

        // Mutable bytearray in-place path when `mutate_flag=True`.
        if *mutate_flag
            && let Some(ba_rc) = pyrust_builtins::bytearray::as_bytearray_rc(&arg_val.0)
        {
            let mut vec = ba_rc.borrow_mut();
            // SAFETY: `vec` is exclusively borrowed and passes a valid mutable byte buffer.
            let rc = unsafe { libc::ioctl(raw_fd, raw_req, vec.as_mut_ptr()) };
            if rc < 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    None,
                ));
            }
            return Ok(Value::int(rc as i64));
        }

        // Immutable or non-mutating bytes-like buffer path.
        let bytes_like = PyBytesLike::try_from_value_with_interp(_interp, &arg_val.0, FN_NAME, "arg")?;
        let slice = bytes_like.as_bytes();
        if slice.len() > IOCTL_BUFSIZE {
            return Err(PyError::named(
                "ValueError",
                "ioctl string arg too long".to_string(),
            ));
        }
        let mut buf = [0u8; IOCTL_BUFSIZE + 1];
        buf[..slice.len()].copy_from_slice(slice);
        // SAFETY: `buf` is a valid stack buffer passed to POSIX `ioctl`.
        let rc = unsafe { libc::ioctl(raw_fd, raw_req, buf.as_mut_ptr()) };
        if rc < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::bytes(buf[..slice.len()].to_vec()))
    }

    /// Perform the lock operation `operation` on file descriptor `fd`.
    fn flock(
        #[positional_only] fd: PyValue,
        #[positional_only] operation: PyIndex,
    ) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        let op = operation.expect_i64(FN_NAME, "operation")? as libc::c_int;
        // SAFETY: standard BSD/POSIX `flock`.
        let rc = unsafe { libc::flock(raw_fd, op) };
        if rc < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }

    /// Acquire, release, or test a POSIX record lock via `fcntl(F_SETLK/F_SETLKW)`.
    fn lockf(
        #[positional_only] fd: PyValue,
        #[positional_only] cmd: PyIndex,
        #[positional_only]
        #[default(0_i64.into())]
        len: PyIndex,
        #[positional_only]
        #[default(0_i64.into())]
        start: PyIndex,
        #[positional_only]
        #[default(0_i64.into())]
        whence: PyIndex,
    ) -> Result<Value> {
        let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
        let code = cmd.expect_i64(FN_NAME, "cmd")? as libc::c_int;
        #[allow(clippy::unnecessary_cast)]
        let l_len = len.expect_i64(FN_NAME, "len")? as libc::off_t;
        #[allow(clippy::unnecessary_cast)]
        let l_start = start.expect_i64(FN_NAME, "start")? as libc::off_t;
        let l_whence = whence.expect_i64(FN_NAME, "whence")? as libc::c_short;

        let l_type: libc::c_short = if code == libc::LOCK_UN {
            libc::F_UNLCK as libc::c_short
        } else if (code & libc::LOCK_SH) != 0 {
            libc::F_RDLCK as libc::c_short
        } else if (code & libc::LOCK_EX) != 0 {
            libc::F_WRLCK as libc::c_short
        } else {
            return Err(PyError::named(
                "ValueError",
                "unrecognized lockf argument".to_string(),
            ));
        };

        let fcntl_cmd = if (code & libc::LOCK_NB) != 0 {
            libc::F_SETLK
        } else {
            libc::F_SETLKW
        };

        // SAFETY: zero-initializing `libc::flock` then setting all standard fields is portable
        // across Linux and macOS (which differ in field order).
        let mut fl: libc::flock = unsafe { std::mem::zeroed() };
        fl.l_type = l_type;
        fl.l_whence = l_whence;
        fl.l_start = l_start;
        fl.l_len = l_len;
        fl.l_pid = 0;

        // SAFETY: `fl` is a valid `libc::flock` pointer.
        let rc = unsafe { libc::fcntl(raw_fd, fcntl_cmd, &fl) };
        if rc < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }
}
