// `resource` module — 100% native Rust implementation of Python 3.12's Unix
// resource usage and limits module (issue #3261).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs`.
// Reference: <https://docs.python.org/3.12/library/resource.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter, lookup_exc_class};
use crate::value::Value;

pyrust_struct_seq! {
    pub(crate) struct_rusage in "resource" {
        fields: [
            ru_utime,
            ru_stime,
            ru_maxrss,
            ru_ixrss,
            ru_idrss,
            ru_isrss,
            ru_minflt,
            ru_majflt,
            ru_nswap,
            ru_inblock,
            ru_oublock,
            ru_msgsnd,
            ru_msgrcv,
            ru_nsignals,
            ru_nvcsw,
            ru_nivcsw,
        ],
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
type RlimitResource = libc::c_int;

#[allow(clippy::unnecessary_cast)]
fn c_long_to_i64(val: libc::c_long) -> i64 {
    val as i64
}

fn rlim_to_value(val: libc::rlim_t) -> Value {
    if val == libc::RLIM_INFINITY {
        Value::int(libc::RLIM_INFINITY as i64)
    } else if val <= i64::MAX as libc::rlim_t {
        Value::int(val as i64)
    } else {
        Value::bigint(crate::value::PyBigInt::from(val))
    }
}

fn value_to_rlim(val: &Value, fn_name: &str) -> Result<libc::rlim_t> {
    let n = match val.kind() {
        crate::value::ValueKind::Int(i) => i,
        crate::value::ValueKind::Bool(b) => i64::from(b),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}(): expected integer limit"),
            ));
        }
    };
    if n == libc::RLIM_INFINITY as i64 {
        Ok(libc::RLIM_INFINITY)
    } else if n < 0 {
        // CPython accepts negative values on Linux only when matching RLIM_INFINITY (-1);
        // other negatives raise ValueError or OverflowError.
        Err(PyError::named(
            "ValueError",
            "limit value cannot be negative".to_string(),
        ))
    } else {
        Ok(n as libc::rlim_t)
    }
}

fn parse_rlimit_pair(
    interp: &mut Interpreter,
    limits: &Value,
    fn_name: &str,
) -> Result<libc::rlimit> {
    let items = interp.collect_iterable(limits).map_err(|_| {
        PyError::named("TypeError", format!("{fn_name}() limits must be a 2-tuple"))
    })?;
    if items.len() != 2 {
        return Err(PyError::named(
            "ValueError",
            "expected a tuple of 2 integers".to_string(),
        ));
    }
    let rlim_cur = value_to_rlim(&items[0], fn_name)?;
    let rlim_max = value_to_rlim(&items[1], fn_name)?;
    if rlim_cur != libc::RLIM_INFINITY && rlim_max != libc::RLIM_INFINITY && rlim_cur > rlim_max {
        return Err(PyError::named(
            "ValueError",
            "current limit exceeds maximum limit".to_string(),
        ));
    }
    if rlim_cur == libc::RLIM_INFINITY && rlim_max != libc::RLIM_INFINITY {
        return Err(PyError::named(
            "ValueError",
            "current limit exceeds maximum limit".to_string(),
        ));
    }
    Ok(libc::rlimit { rlim_cur, rlim_max })
}

fn map_rlimit_errno(err: &std::io::Error) -> PyError {
    match err.raw_os_error() {
        Some(libc::EINVAL) => {
            PyError::named("ValueError", "invalid resource specified".to_string())
        }
        Some(libc::EPERM) => PyError::named(
            "ValueError",
            "not allowed to raise maximum limit".to_string(),
        ),
        _ => PyError::from_io_error(err, None),
    }
}

pyrust_module! {
    constants {
        "error" => Value::py_class(
            lookup_exc_class("OSError").expect("canonical OSError class must exist"),
        ),
        "struct_rusage" => struct_rusage::class(),

        "RLIM_INFINITY" => Value::int(libc::RLIM_INFINITY as i64),
        "RUSAGE_SELF" => Value::int(libc::RUSAGE_SELF as i64),
        "RUSAGE_CHILDREN" => Value::int(libc::RUSAGE_CHILDREN as i64),
        #[cfg(target_os = "linux")]
        "RUSAGE_THREAD" => Value::int(libc::RUSAGE_THREAD as i64),

        "RLIMIT_CORE" => Value::int(libc::RLIMIT_CORE as i64),
        "RLIMIT_CPU" => Value::int(libc::RLIMIT_CPU as i64),
        "RLIMIT_FSIZE" => Value::int(libc::RLIMIT_FSIZE as i64),
        "RLIMIT_DATA" => Value::int(libc::RLIMIT_DATA as i64),
        "RLIMIT_STACK" => Value::int(libc::RLIMIT_STACK as i64),
        "RLIMIT_RSS" => Value::int(libc::RLIMIT_RSS as i64),
        "RLIMIT_NPROC" => Value::int(libc::RLIMIT_NPROC as i64),
        "RLIMIT_NOFILE" => Value::int(libc::RLIMIT_NOFILE as i64),
        "RLIMIT_OFILE" => Value::int(libc::RLIMIT_NOFILE as i64),
        "RLIMIT_MEMLOCK" => Value::int(libc::RLIMIT_MEMLOCK as i64),
        "RLIMIT_AS" => Value::int(libc::RLIMIT_AS as i64),

        #[cfg(target_os = "linux")]
        "RLIMIT_LOCKS" => Value::int(libc::RLIMIT_LOCKS as i64),
        #[cfg(target_os = "linux")]
        "RLIMIT_SIGPENDING" => Value::int(libc::RLIMIT_SIGPENDING as i64),
        #[cfg(target_os = "linux")]
        "RLIMIT_MSGQUEUE" => Value::int(libc::RLIMIT_MSGQUEUE as i64),
        #[cfg(target_os = "linux")]
        "RLIMIT_NICE" => Value::int(libc::RLIMIT_NICE as i64),
        #[cfg(target_os = "linux")]
        "RLIMIT_RTPRIO" => Value::int(libc::RLIMIT_RTPRIO as i64),
        #[cfg(target_os = "linux")]
        "RLIMIT_RTTIME" => Value::int(libc::RLIMIT_RTTIME as i64),
    }

    /// Return a tuple `(soft, hard)` with the current limits for `resource`.
    fn getrlimit(#[positional_only] resource: PyIndex) -> Result<Value> {
        let res_i64 = resource.expect_i64(FN_NAME, "resource")?;
        if res_i64 < 0 || res_i64 > i32::MAX as i64 {
            return Err(PyError::named(
                "ValueError",
                "invalid resource specified".to_string(),
            ));
        }
        let mut rlim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `rlim` is a valid out-pointer.
        let rc = unsafe { libc::getrlimit(res_i64 as RlimitResource, &mut rlim) };
        if rc < 0 {
            return Err(map_rlimit_errno(&std::io::Error::last_os_error()));
        }
        Ok(Value::tuple(vec![
            rlim_to_value(rlim.rlim_cur),
            rlim_to_value(rlim.rlim_max),
        ]))
    }

    /// Set new consumption limits `(soft, hard)` for `resource`.
    fn setrlimit(
        #[positional_only] resource: PyIndex,
        #[positional_only] limits: PyValue,
    ) -> Result<Value> {
        let res_i64 = resource.expect_i64(FN_NAME, "resource")?;
        if res_i64 < 0 || res_i64 > i32::MAX as i64 {
            return Err(PyError::named(
                "ValueError",
                "invalid resource specified".to_string(),
            ));
        }
        let rlim = parse_rlimit_pair(_interp, &limits.0, FN_NAME)?;
        // SAFETY: `rlim` is a valid `libc::rlimit` pointer.
        let rc = unsafe { libc::setrlimit(res_i64 as RlimitResource, &rlim) };
        if rc < 0 {
            return Err(map_rlimit_errno(&std::io::Error::last_os_error()));
        }
        Ok(Value::none())
    }

    /// Get or set resource limits for an arbitrary process (Linux only).
    #[cfg(target_os = "linux")]
    fn prlimit(
        #[positional_only] pid: PyIndex,
        #[positional_only] resource: PyIndex,
        #[positional_only]
        #[default(None)]
        limits: Option<PyValue>,
    ) -> Result<Value> {
        let pid_i64 = pid.expect_i64(FN_NAME, "pid")?;
        let res_i64 = resource.expect_i64(FN_NAME, "resource")?;
        if res_i64 < 0 || res_i64 > i32::MAX as i64 {
            return Err(PyError::named(
                "ValueError",
                "invalid resource specified".to_string(),
            ));
        }
        let new_rlim = match limits {
            Some(ref lim) if !lim.0.is_none() => Some(parse_rlimit_pair(_interp, &lim.0, FN_NAME)?),
            _ => None,
        };
        let mut old_rlim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let new_ptr = match &new_rlim {
            Some(r) => r as *const libc::rlimit,
            None => std::ptr::null(),
        };
        // SAFETY: `new_ptr` is null or valid; `old_rlim` is a valid out-pointer.
        let rc = unsafe {
            libc::prlimit(
                pid_i64 as libc::pid_t,
                res_i64 as RlimitResource,
                new_ptr,
                &mut old_rlim,
            )
        };
        if rc < 0 {
            return Err(map_rlimit_errno(&std::io::Error::last_os_error()));
        }
        Ok(Value::tuple(vec![
            rlim_to_value(old_rlim.rlim_cur),
            rlim_to_value(old_rlim.rlim_max),
        ]))
    }

    /// Return a `struct_rusage` describing resources consumed by `who`.
    fn getrusage(#[positional_only] who: PyIndex) -> Result<Value> {
        let who_i64 = who.expect_i64(FN_NAME, "who")?;
        let who_c = libc::c_int::try_from(who_i64).map_err(|_| {
            PyError::named("ValueError", "invalid who parameter".to_string())
        })?;
        // SAFETY: zeroed `libc::rusage` is valid to pass as out-pointer to `getrusage`.
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::getrusage(who_c, &mut ru) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINVAL) {
                return Err(PyError::named(
                    "ValueError",
                    "invalid who parameter".to_string(),
                ));
            }
            return Err(PyError::from_io_error(&err, None));
        }
        let utime = ru.ru_utime.tv_sec as f64 + (ru.ru_utime.tv_usec as f64) / 1_000_000.0;
        let stime = ru.ru_stime.tv_sec as f64 + (ru.ru_stime.tv_usec as f64) / 1_000_000.0;
        Ok(struct_rusage::make([
            Value::float(utime),
            Value::float(stime),
            Value::int(c_long_to_i64(ru.ru_maxrss)),
            Value::int(c_long_to_i64(ru.ru_ixrss)),
            Value::int(c_long_to_i64(ru.ru_idrss)),
            Value::int(c_long_to_i64(ru.ru_isrss)),
            Value::int(c_long_to_i64(ru.ru_minflt)),
            Value::int(c_long_to_i64(ru.ru_majflt)),
            Value::int(c_long_to_i64(ru.ru_nswap)),
            Value::int(c_long_to_i64(ru.ru_inblock)),
            Value::int(c_long_to_i64(ru.ru_oublock)),
            Value::int(c_long_to_i64(ru.ru_msgsnd)),
            Value::int(c_long_to_i64(ru.ru_msgrcv)),
            Value::int(c_long_to_i64(ru.ru_nsignals)),
            Value::int(c_long_to_i64(ru.ru_nvcsw)),
            Value::int(c_long_to_i64(ru.ru_nivcsw)),
        ]))
    }

    /// Return the number of bytes in a system page.
    fn getpagesize() -> Result<Value> {
        // SAFETY: `sysconf(_SC_PAGESIZE)` has no preconditions.
        let pagesize = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if pagesize <= 0 {
            return Ok(Value::int(4096));
        }
        Ok(Value::int(c_long_to_i64(pagesize)))
    }
}
