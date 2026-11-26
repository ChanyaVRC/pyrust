// `syslog` module — 100% native Rust implementation of Python 3.12's Unix
// syslog interface (issue #3261).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs`.
// Reference: <https://docs.python.org/3.12/library/syslog.html>

use std::ffi::CString;
use std::sync::{LazyLock, Mutex};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyStr, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{Value, ValueKind};

/// POSIX `openlog` retains the `ident` pointer without copying it, so we keep
/// the active `CString` alive until the next `openlog` or `closelog` call.
static SYSLOG_IDENT: LazyLock<Mutex<Option<CString>>> = LazyLock::new(|| Mutex::new(None));

pyrust_module! {
    constants {
        // Priorities
        "LOG_EMERG" => Value::int(libc::LOG_EMERG as i64),
        "LOG_ALERT" => Value::int(libc::LOG_ALERT as i64),
        "LOG_CRIT" => Value::int(libc::LOG_CRIT as i64),
        "LOG_ERR" => Value::int(libc::LOG_ERR as i64),
        "LOG_WARNING" => Value::int(libc::LOG_WARNING as i64),
        "LOG_NOTICE" => Value::int(libc::LOG_NOTICE as i64),
        "LOG_INFO" => Value::int(libc::LOG_INFO as i64),
        "LOG_DEBUG" => Value::int(libc::LOG_DEBUG as i64),

        // Options
        "LOG_PID" => Value::int(libc::LOG_PID as i64),
        "LOG_CONS" => Value::int(libc::LOG_CONS as i64),
        "LOG_NDELAY" => Value::int(libc::LOG_NDELAY as i64),
        "LOG_ODELAY" => Value::int(libc::LOG_ODELAY as i64),
        "LOG_NOWAIT" => Value::int(libc::LOG_NOWAIT as i64),
        "LOG_PERROR" => Value::int(libc::LOG_PERROR as i64),

        // Facilities
        "LOG_KERN" => Value::int(libc::LOG_KERN as i64),
        "LOG_USER" => Value::int(libc::LOG_USER as i64),
        "LOG_MAIL" => Value::int(libc::LOG_MAIL as i64),
        "LOG_DAEMON" => Value::int(libc::LOG_DAEMON as i64),
        "LOG_AUTH" => Value::int(libc::LOG_AUTH as i64),
        "LOG_LPR" => Value::int(libc::LOG_LPR as i64),
        "LOG_NEWS" => Value::int(libc::LOG_NEWS as i64),
        "LOG_UUCP" => Value::int(libc::LOG_UUCP as i64),
        "LOG_CRON" => Value::int(libc::LOG_CRON as i64),
        "LOG_SYSLOG" => Value::int(libc::LOG_SYSLOG as i64),
        "LOG_AUTHPRIV" => Value::int(libc::LOG_AUTHPRIV as i64),
        "LOG_LOCAL0" => Value::int(libc::LOG_LOCAL0 as i64),
        "LOG_LOCAL1" => Value::int(libc::LOG_LOCAL1 as i64),
        "LOG_LOCAL2" => Value::int(libc::LOG_LOCAL2 as i64),
        "LOG_LOCAL3" => Value::int(libc::LOG_LOCAL3 as i64),
        "LOG_LOCAL4" => Value::int(libc::LOG_LOCAL4 as i64),
        "LOG_LOCAL5" => Value::int(libc::LOG_LOCAL5 as i64),
        "LOG_LOCAL6" => Value::int(libc::LOG_LOCAL6 as i64),
        "LOG_LOCAL7" => Value::int(libc::LOG_LOCAL7 as i64),
    }

    /// Set logging options for subsequent `syslog()` calls.
    fn openlog(
        #[default(None)] ident: Option<PyStr<'_>>,
        #[default(0_i64.into())] logoption: PyIndex,
        #[default((libc::LOG_USER as i64).into())] facility: PyIndex,
    ) -> Result<Value> {
        let logopt_i = logoption.expect_i64(FN_NAME, "logoption")? as libc::c_int;
        let fac_i = facility.expect_i64(FN_NAME, "facility")? as libc::c_int;
        let new_ident = match ident {
            Some(s) => Some(CString::new(s.as_bytes()).map_err(|_| {
                PyError::named("ValueError", "embedded null character".to_string())
            })?),
            None => None,
        };
        let mut slot = SYSLOG_IDENT.lock().unwrap_or_else(|e| e.into_inner());
        let ptr = new_ident
            .as_ref()
            .map_or(std::ptr::null(), |cs| cs.as_ptr());
        // SAFETY: `ptr` is either NULL or backed by `new_ident`, which is moved into
        // `SYSLOG_IDENT` immediately before `slot` is dropped.
        unsafe { libc::openlog(ptr, logopt_i, fac_i) };
        *slot = new_ident;
        Ok(Value::none())
    }

    /// Send a message to the system logger (`syslog(message)` or `syslog(priority, message)`).
    fn syslog(
        #[positional_only] first: PyValue,
        #[positional_only]
        #[default(None)]
        second: Option<PyStr<'_>>,
    ) -> Result<Value> {
        let (priority, message): (libc::c_int, String) = match second {
            Some(msg) => {
                let pri = match first.0.kind() {
                    ValueKind::Int(n) => n as libc::c_int,
                    ValueKind::Bool(b) => libc::c_int::from(b),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "syslog(): priority must be int, not {}",
                                value_type_name_str(&first.0)
                            ),
                        ));
                    }
                };
                (pri, msg.as_ref().to_string())
            }
            None => match first.0.kind() {
                ValueKind::Str(s) => (libc::LOG_INFO, s.to_string()),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "syslog(): message must be str, not {}",
                            value_type_name_str(&first.0)
                        ),
                    ));
                }
            },
        };
        let cmsg = CString::new(message.as_bytes()).map_err(|_| {
            PyError::named("ValueError", "embedded null character".to_string())
        })?;
        let _guard = SYSLOG_IDENT.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: format string is constant `c"%s"`, preventing format-string injection.
        unsafe { libc::syslog(priority, c"%s".as_ptr(), cmsg.as_ptr()) };
        Ok(Value::none())
    }

    /// Reset the syslog module values and call the system library `closelog()`.
    fn closelog() -> Result<Value> {
        let mut slot = SYSLOG_IDENT.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `closelog` takes no arguments and is serialized with `openlog`.
        unsafe { libc::closelog() };
        *slot = None;
        Ok(Value::none())
    }

    /// Set the priority mask to `maskpri` and return the previous mask value.
    fn setlogmask(#[positional_only] maskpri: PyIndex) -> Result<Value> {
        let mask = maskpri.expect_i64(FN_NAME, "maskpri")? as libc::c_int;
        // SAFETY: `setlogmask` is always safe and returns the previous mask.
        let prev = unsafe { libc::setlogmask(mask) };
        Ok(Value::int(prev as i64))
    }

    /// Calculate the bit mask for the individual priority `pri`.
    #[py_name = "LOG_MASK"]
    fn log_mask(#[positional_only] pri: PyIndex) -> Result<Value> {
        let p = pri.expect_i64(FN_NAME, "pri")?;
        let shift = (p as u32) & 31;
        Ok(Value::int(i64::from(1_i32.wrapping_shl(shift))))
    }

    /// Calculate the bit mask for all priorities up to and including `pri`.
    #[py_name = "LOG_UPTO"]
    fn log_upto(#[positional_only] pri: PyIndex) -> Result<Value> {
        let p = pri.expect_i64(FN_NAME, "pri")?;
        let shift = ((p.wrapping_add(1)) as u32) & 31;
        Ok(Value::int(i64::from(
            1_i32.wrapping_shl(shift).wrapping_sub(1),
        )))
    }
}
