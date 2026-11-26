// `grp` module — 100% native Rust implementation of Python 3.12's Unix group
// database module (`struct_group`, `getgrgid`, `getgrnam`, `getgrall`)
// (issue #3261).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs`.
// Reference: <https://docs.python.org/3.12/library/grp.html>

use std::ffi::{CStr, CString};
use std::sync::{LazyLock, Mutex};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyIndex, PyStr};
use crate::value::Value;

/// Serializes calls into POSIX non-reentrant `group` database functions
/// (`getgrgid`, `getgrnam`, `setgrent`, `getgrent`, `endgrent`).
static GRP_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pyrust_struct_seq! {
    pub(crate) struct_group in "grp" {
        fields: [gr_name, gr_passwd, gr_gid, gr_mem],
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

/// Convert a non-null `libc::group` pointer into a `grp.struct_group` Value.
///
/// # Safety
/// `gr` must be a valid, non-null pointer to a `libc::group` struct.
unsafe fn group_to_value(gr: *const libc::group) -> Value {
    // SAFETY: caller guarantees `gr` is valid and non-null.
    let entry = unsafe { &*gr };
    let gr_name = unsafe { c_ptr_to_string(entry.gr_name) };
    let gr_passwd = unsafe { c_ptr_to_string(entry.gr_passwd) };
    let gr_gid = entry.gr_gid as i64;

    let mut members = Vec::new();
    if !entry.gr_mem.is_null() {
        let mut cursor = entry.gr_mem;
        // SAFETY: `gr_mem` is a NULL-terminated array of NUL-terminated C strings.
        unsafe {
            while !(*cursor).is_null() {
                members.push(Value::string(c_ptr_to_string(*cursor)));
                cursor = cursor.add(1);
            }
        }
    }

    struct_group::make([
        Value::string(gr_name),
        Value::string(gr_passwd),
        Value::int(gr_gid),
        Value::list(members),
    ])
}

/// `grp.getgrgid(gid).gr_name`, used by `pathlib.Path.group()`.
pub(crate) fn group_name_for_gid(gid: libc::gid_t) -> Result<Value> {
    let _guard = GRP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: serialized by `GRP_LOCK`; the entry is copied before the lock is released.
    let ptr = unsafe { libc::getgrgid(gid) };
    if ptr.is_null() {
        return Err(PyError::key_error(Value::string(format!(
            "getgrgid(): gid not found: {gid}"
        ))));
    }
    // SAFETY: `ptr` is non-null and valid while `GRP_LOCK` is held.
    Ok(Value::string(unsafe { c_ptr_to_string((*ptr).gr_name) }))
}

pyrust_module! {
    constants {
        "struct_group" => struct_group::class(),
    }

    /// Return the group database entry for the given numeric group ID.
    fn getgrgid(id: PyIndex) -> Result<Value> {
        let gid_i64 = id.expect_i64(FN_NAME, "id")?;
        let raw_gid = libc::gid_t::try_from(gid_i64).map_err(|_| {
            PyError::key_error(Value::string(format!(
                "getgrgid(): gid not found: {gid_i64}"
            )))
        })?;
        let _guard = GRP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by `GRP_LOCK`; returned pointer is read before releasing lock.
        let ptr = unsafe { libc::getgrgid(raw_gid) };
        if ptr.is_null() {
            return Err(PyError::key_error(Value::string(format!(
                "getgrgid(): gid not found: {gid_i64}"
            ))));
        }
        // SAFETY: `ptr` is non-null and valid while `GRP_LOCK` is held.
        Ok(unsafe { group_to_value(ptr) })
    }

    /// Return the group database entry for the given group name.
    fn getgrnam(name: PyStr<'_>) -> Result<Value> {
        let cname = CString::new(name.as_bytes()).map_err(|_| {
            PyError::named("ValueError", "embedded null character".to_string())
        })?;
        let _guard = GRP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `cname` is NUL-terminated and access is serialized by `GRP_LOCK`.
        let ptr = unsafe { libc::getgrnam(cname.as_ptr()) };
        if ptr.is_null() {
            return Err(PyError::key_error(Value::string(format!(
                "getgrnam(): name not found: '{}'",
                name.as_ref()
            ))));
        }
        // SAFETY: `ptr` is non-null and valid while `GRP_LOCK` is held.
        Ok(unsafe { group_to_value(ptr) })
    }

    /// Return a list of all available group database entries.
    fn getgrall() -> Result<Value> {
        let _guard = GRP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries = Vec::new();
        // SAFETY: `setgrent`/`getgrent`/`endgrent` sequence is serialized by `GRP_LOCK`.
        unsafe {
            libc::setgrent();
            loop {
                let ptr = libc::getgrent();
                if ptr.is_null() {
                    break;
                }
                entries.push(group_to_value(ptr));
            }
            libc::endgrent();
        }
        Ok(Value::list(entries))
    }
}
