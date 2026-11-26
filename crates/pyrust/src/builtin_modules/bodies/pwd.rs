// `pwd` module — 100% native Rust implementation of Python 3.12's Unix password
// database module (`struct_passwd`, `getpwuid`, `getpwnam`, `getpwall`)
// (issue #3261).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs`.
// Reference: <https://docs.python.org/3.12/library/pwd.html>

use std::ffi::{CStr, CString};
use std::sync::{LazyLock, Mutex};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyIndex, PyStr};
use crate::value::Value;

/// Serializes calls into POSIX non-reentrant `passwd` database functions
/// (`getpwuid`, `getpwnam`, `setpwent`, `getpwent`, `endpwent`).
static PWD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pyrust_struct_seq! {
    pub(crate) struct_passwd in "pwd" {
        fields: [pw_name, pw_passwd, pw_uid, pw_gid, pw_gecos, pw_dir, pw_shell],
    }
}

/// Convert a possibly-null C string pointer to a Rust `String`.
///
/// # Safety
/// If `ptr` is non-null, it must point to a valid NUL-terminated C string.
unsafe fn c_ptr_to_string(ptr: *const libc::c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        // SAFETY: caller guarantees `ptr` is a valid NUL-terminated C string.
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Convert a non-null `libc::passwd` pointer into a `pwd.struct_passwd` Value.
///
/// # Safety
/// `pw` must be a valid, non-null pointer to a `libc::passwd` struct.
unsafe fn passwd_to_value(pw: *const libc::passwd) -> Value {
    // SAFETY: caller guarantees `pw` is valid and non-null.
    let entry = unsafe { &*pw };
    let pw_name = unsafe { c_ptr_to_string(entry.pw_name) };
    let pw_passwd = unsafe { c_ptr_to_string(entry.pw_passwd) };
    let pw_uid = entry.pw_uid as i64;
    let pw_gid = entry.pw_gid as i64;
    let pw_gecos = unsafe { c_ptr_to_string(entry.pw_gecos) };
    let pw_dir = unsafe { c_ptr_to_string(entry.pw_dir) };
    let pw_shell = unsafe { c_ptr_to_string(entry.pw_shell) };

    struct_passwd::make([
        Value::string(pw_name),
        Value::string(pw_passwd),
        Value::int(pw_uid),
        Value::int(pw_gid),
        Value::string(pw_gecos),
        Value::string(pw_dir),
        Value::string(pw_shell),
    ])
}

pyrust_module! {
    constants {
        "struct_passwd" => struct_passwd::class(),
    }

    /// Return the password database entry for the given numeric user ID.
    fn getpwuid(#[positional_only] uid: PyIndex) -> Result<Value> {
        let uid_i64 = uid.expect_i64(FN_NAME, "uid")?;
        let raw_uid = libc::uid_t::try_from(uid_i64).map_err(|_| {
            PyError::key_error(Value::string(format!(
                "getpwuid(): uid not found: {uid_i64}"
            )))
        })?;
        let _guard = PWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by `PWD_LOCK`; returned pointer is read before releasing lock.
        let ptr = unsafe { libc::getpwuid(raw_uid) };
        if ptr.is_null() {
            return Err(PyError::key_error(Value::string(format!(
                "getpwuid(): uid not found: {uid_i64}"
            ))));
        }
        // SAFETY: `ptr` is non-null and valid while `PWD_LOCK` is held.
        Ok(unsafe { passwd_to_value(ptr) })
    }

    /// Return the password database entry for the given user name.
    fn getpwnam(#[positional_only] name: PyStr<'_>) -> Result<Value> {
        let cname = CString::new(name.as_bytes()).map_err(|_| {
            PyError::named("ValueError", "embedded null character".to_string())
        })?;
        let _guard = PWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `cname` is NUL-terminated and access is serialized by `PWD_LOCK`.
        let ptr = unsafe { libc::getpwnam(cname.as_ptr()) };
        if ptr.is_null() {
            return Err(PyError::key_error(Value::string(format!(
                "getpwnam(): name not found: '{}'",
                name.as_ref()
            ))));
        }
        // SAFETY: `ptr` is non-null and valid while `PWD_LOCK` is held.
        Ok(unsafe { passwd_to_value(ptr) })
    }

    /// Return a list of all available password database entries.
    fn getpwall() -> Result<Value> {
        let _guard = PWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries = Vec::new();
        // SAFETY: `setpwent`/`getpwent`/`endpwent` sequence is serialized by `PWD_LOCK`.
        unsafe {
            libc::setpwent();
            loop {
                let ptr = libc::getpwent();
                if ptr.is_null() {
                    break;
                }
                entries.push(passwd_to_value(ptr));
            }
            libc::endpwent();
        }
        Ok(Value::list(entries))
    }
}
