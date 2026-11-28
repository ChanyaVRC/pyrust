//! Windows-specific `os` and `nt` helpers (issue #3262).
//!
//! Compiled exclusively on `#[cfg(windows)]`. Provides the native Win32
//! implementations for:
//! - `startfile` (`ShellExecuteW`)
//! - `_add_dll_directory` / `_remove_dll_directory` / `add_dll_directory` /
//!   `_AddedDllDirectory` (`AddDllDirectory` / `RemoveDllDirectory`)
//! - `listdrives` (`GetLogicalDrives`)
//! - `listvolumes` (`FindFirstVolumeW` / `FindNextVolumeW` / `FindVolumeClose`)
//! - `listmounts` (`GetVolumePathNamesForVolumeNameW`)
//! - `_getfinalpathname` (`GetFinalPathNameByHandleW`)
//! - `_getfullpathname` (`GetFullPathNameW`)
//! - `_getdiskusage` (`GetDiskFreeSpaceExW`)
//! - `_getvolumepathname` (`GetVolumePathNameW`)
//! - `device_encoding` (`GetConsoleCP` / `GetConsoleOutputCP`)

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA,
    ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND, GetLastError, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDiskFreeSpaceExW,
    GetFinalPathNameByHandleW, GetFullPathNameW, GetLogicalDrives, GetVolumePathNameW,
    GetVolumePathNamesForVolumeNameW, OPEN_EXISTING, VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::Console::{GetConsoleCP, GetConsoleOutputCP};
use windows_sys::Win32::System::LibraryLoader::{AddDllDirectory, RemoveDllDirectory};
use windows_sys::Win32::UI::Shell::ShellExecuteW;

use crate::builtin_modules::macros::native_state::set_native_state;
use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::interpreter::builtin_args::PyPath;
use crate::value::{InstanceAttrs, PyBigInt, PyInstance, Value, ValueKind};

use super::fd_ops;

/// Active cookies returned by `AddDllDirectory`, tracked so invalid or
/// double-closed cookies raise `TypeError` matching CPython 3.12.
static ACTIVE_DLL_DIR_COOKIES: LazyLock<Mutex<HashSet<usize>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub(crate) struct AddedDllDirectoryState {
    pub(crate) path: Option<String>,
    pub(crate) cookie: usize,
}

fn encode_wide_nul(s: &OsStr) -> Result<Vec<u16>> {
    let mut wide: Vec<u16> = s.encode_wide().collect();
    if wide.contains(&0) {
        return Err(PyError::named(
            "ValueError",
            "embedded null character".to_string(),
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn encode_str_wide_nul(s: &str) -> Result<Vec<u16>> {
    encode_wide_nul(OsStr::new(s))
}

fn win32_error_to_py(code: u32, filename: Option<&str>) -> PyError {
    let io_err = std::io::Error::from_raw_os_error(code as i32);
    PyError::from_io_error(&io_err, filename)
}

fn u64_to_py_int(v: u64) -> Value {
    if v <= i64::MAX as u64 {
        Value::int(v as i64)
    } else {
        Value::bigint(PyBigInt::from(v))
    }
}

fn string_or_bytes_like_input(path: &PyPath, out: String) -> Result<Value> {
    if path.is_bytes() {
        Ok(Value::bytes(out.into_bytes()))
    } else {
        Ok(Value::string(out))
    }
}

// ── startfile ────────────────────────────────────────────────────────────────

pub(crate) fn startfile(
    filepath: PyPath,
    operation: Option<&str>,
    arguments: Option<&str>,
    cwd: Option<PyPath>,
    show_cmd: i64,
) -> Result<Value> {
    let file_disp = filepath.as_path().to_string_lossy().into_owned();
    let file_wide = encode_wide_nul(filepath.as_os_str())?;
    let op_wide = match operation {
        Some(op) => Some(encode_str_wide_nul(op)?),
        None => None,
    };
    let args_wide = match arguments {
        Some(a) => Some(encode_str_wide_nul(a)?),
        None => None,
    };
    let cwd_wide = match &cwd {
        Some(dir) => Some(encode_wide_nul(dir.as_os_str())?),
        None => None,
    };

    // SAFETY: All non-null wide buffers are NUL-terminated for the duration of
    // the synchronous `ShellExecuteW` call.
    let res = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op_wide.as_ref().map_or(std::ptr::null(), Vec::as_ptr),
            file_wide.as_ptr(),
            args_wide.as_ref().map_or(std::ptr::null(), Vec::as_ptr),
            cwd_wide.as_ref().map_or(std::ptr::null(), Vec::as_ptr),
            show_cmd as i32,
        )
    };
    let code = res as isize;
    if code <= 32 {
        // ShellExecuteW returns error codes <= 32 where 2=SE_ERR_FNF, 3=SE_ERR_PNF,
        // 5=SE_ERR_ACCESSDENIED match Win32 error codes directly.
        let win_err = if code > 0 {
            code as u32
        } else {
            // SAFETY: `GetLastError` has no preconditions.
            unsafe { GetLastError() }
        };
        return Err(win32_error_to_py(win_err, Some(&file_disp)));
    }
    Ok(Value::none())
}

// ── DLL directory helpers ────────────────────────────────────────────────────

pub(crate) fn add_dll_directory_raw(path: &PyPath) -> Result<usize> {
    let p = path.as_path();
    let p_disp = p.to_string_lossy().into_owned();
    let wide = encode_wide_nul(path.as_os_str())?;

    // Match CPython: reject non-absolute paths with ERROR_INVALID_PARAMETER (87)
    // and non-existent paths with FileNotFoundError before registering.
    if !p.is_absolute() {
        return Err(win32_error_to_py(ERROR_INVALID_PARAMETER, Some(&p_disp)));
    }
    match std::fs::metadata(p) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(win32_error_to_py(ERROR_PATH_NOT_FOUND, Some(&p_disp)));
            }
        }
        Err(e) => {
            return Err(PyError::from_io_error(&e, Some(&p_disp)));
        }
    }

    // SAFETY: `wide` is a valid NUL-terminated UTF-16 path string.
    let cookie = unsafe { AddDllDirectory(wide.as_ptr()) };
    if cookie.is_null() {
        // SAFETY: `GetLastError` reads the error set by the failed call above.
        let err = unsafe { GetLastError() };
        let code = if err == 0 { ERROR_FILE_NOT_FOUND } else { err };
        return Err(win32_error_to_py(code, Some(&p_disp)));
    }
    let cookie_int = cookie as usize;
    ACTIVE_DLL_DIR_COOKIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cookie_int);
    Ok(cookie_int)
}

pub(crate) fn nt_add_dll_directory(path: PyPath) -> Result<Value> {
    let cookie = add_dll_directory_raw(&path)?;
    Ok(u64_to_py_int(cookie as u64))
}

pub(crate) fn nt_remove_dll_directory(cookie: usize) -> Result<Value> {
    let removed = ACTIVE_DLL_DIR_COOKIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&cookie);
    if !removed {
        return Err(PyError::named(
            "TypeError",
            "Provided cookie was not returned from os.add_dll_directory".to_string(),
        ));
    }
    // SAFETY: `cookie` was returned by `AddDllDirectory` and removed from the
    // active set so it can never be passed to `RemoveDllDirectory` twice.
    let ok = unsafe { RemoveDllDirectory(cookie as *const core::ffi::c_void) };
    if ok == 0 {
        return Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            None,
        ));
    }
    Ok(Value::none())
}

pub(crate) fn add_dll_directory(interp: &mut Interpreter, path: PyPath) -> Result<Value> {
    let path_str = path.as_path().to_string_lossy().into_owned();
    let cookie = add_dll_directory_raw(&path)?;
    let os_mod = interp.load_module("os")?;
    let cls_val = interp.get_attr(&os_mod, "_AddedDllDirectory")?;
    let cls = match cls_val.kind() {
        ValueKind::PyClass(rc) => Rc::clone(rc),
        _ => {
            let _ = nt_remove_dll_directory(cookie);
            return Err(PyError::runtime(
                "internal: os._AddedDllDirectory is not a class".to_string(),
            ));
        }
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: cls,
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        AddedDllDirectoryState {
            path: Some(path_str),
            cookie,
        },
    )?;
    Ok(inst)
}

pub(crate) fn added_dll_dir_path(st: &AddedDllDirectoryState) -> Value {
    match &st.path {
        Some(p) => Value::string(p),
        None => Value::none(),
    }
}

pub(crate) fn added_dll_dir_close(st: &mut AddedDllDirectoryState) -> Result<Value> {
    nt_remove_dll_directory(st.cookie)?;
    st.path = None;
    Ok(Value::none())
}

pub(crate) fn added_dll_dir_repr(st: &AddedDllDirectoryState) -> Result<Value> {
    match &st.path {
        Some(p) => {
            let repr_path = Value::string(p).repr_raw();
            Ok(Value::string(format!("<AddedDllDirectory({repr_path})>")))
        }
        None => Ok(Value::string("<AddedDllDirectory()>")),
    }
}

// ── Drives, volumes, mounts ──────────────────────────────────────────────────

pub(crate) fn listdrives() -> Result<Value> {
    // SAFETY: `GetLogicalDrives` takes no arguments and returns a bitmask.
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            None,
        ));
    }
    let mut drives = Vec::new();
    for i in 0..26u32 {
        if (mask & (1u32 << i)) != 0 {
            let letter = (b'A' + i as u8) as char;
            drives.push(Value::string(format!("{letter}:\\")));
        }
    }
    Ok(Value::list(drives))
}

pub(crate) fn listvolumes() -> Result<Value> {
    let mut buf = vec![0u16; 512];
    // SAFETY: `buf` is writable for `buf.len()` UTF-16 code units.
    let handle = unsafe { FindFirstVolumeW(buf.as_mut_ptr(), buf.len() as u32) };
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: `GetLastError` inspects the failed Win32 call.
        let err = unsafe { GetLastError() };
        if err == ERROR_NO_MORE_FILES {
            return Ok(Value::list(Vec::new()));
        }
        return Err(win32_error_to_py(err, None));
    }

    let mut volumes = Vec::new();
    loop {
        let nul_pos = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = std::ffi::OsString::from_wide(&buf[..nul_pos])
            .to_string_lossy()
            .into_owned();
        volumes.push(Value::string(s));

        // SAFETY: `handle` is an open volume find handle and `buf` is valid.
        let ok = unsafe { FindNextVolumeW(handle, buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            // SAFETY: `GetLastError` inspects `FindNextVolumeW` failure.
            let err = unsafe { GetLastError() };
            // SAFETY: `handle` is closed before returning.
            unsafe { FindVolumeClose(handle) };
            if err == ERROR_NO_MORE_FILES {
                break;
            }
            return Err(win32_error_to_py(err, None));
        }
    }
    Ok(Value::list(volumes))
}

pub(crate) fn listmounts(volume: PyPath) -> Result<Value> {
    let vol_disp = volume.as_path().to_string_lossy().into_owned();
    let vol_wide = encode_wide_nul(volume.as_os_str())?;

    let mut buf = vec![0u16; 512];
    loop {
        let mut ret_len: u32 = 0;
        // SAFETY: `vol_wide` is NUL-terminated and `buf` is writable for `buf.len()`.
        let ok = unsafe {
            GetVolumePathNamesForVolumeNameW(
                vol_wide.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut ret_len,
            )
        };
        if ok != 0 {
            let mut mounts = Vec::new();
            let mut start = 0usize;
            let limit = (ret_len as usize).min(buf.len());
            while start < limit {
                if buf[start] == 0 {
                    break;
                }
                let end = buf[start..limit]
                    .iter()
                    .position(|&c| c == 0)
                    .map(|off| start + off)
                    .unwrap_or(limit);
                let s = std::ffi::OsString::from_wide(&buf[start..end])
                    .to_string_lossy()
                    .into_owned();
                mounts.push(Value::string(s));
                start = end + 1;
            }
            return Ok(Value::list(mounts));
        }
        // SAFETY: `GetLastError` inspects the failed call above.
        let err = unsafe { GetLastError() };
        if err == ERROR_MORE_DATA && ret_len as usize > buf.len() {
            buf.resize(ret_len as usize, 0);
            continue;
        }
        return Err(win32_error_to_py(err, Some(&vol_disp)));
    }
}

// ── nt path / disk / console helpers ─────────────────────────────────────────

pub(crate) fn nt_getfullpathname(path: PyPath) -> Result<Value> {
    let p_disp = path.as_path().to_string_lossy().into_owned();
    let wide = encode_wide_nul(path.as_os_str())?;
    let mut buf = vec![0u16; 260];
    loop {
        // SAFETY: `wide` is NUL-terminated and `buf` is valid for `buf.len()`.
        let len = unsafe {
            GetFullPathNameW(
                wide.as_ptr(),
                buf.len() as u32,
                buf.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if len == 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                Some(&p_disp),
            ));
        }
        if (len as usize) < buf.len() {
            let out = std::ffi::OsString::from_wide(&buf[..len as usize])
                .to_string_lossy()
                .into_owned();
            return string_or_bytes_like_input(&path, out);
        }
        buf.resize(len as usize + 1, 0);
    }
}

pub(crate) fn nt_getfinalpathname(path: PyPath) -> Result<Value> {
    let p_disp = path.as_path().to_string_lossy().into_owned();
    let wide = encode_wide_nul(path.as_os_str())?;
    // SAFETY: `wide` is NUL-terminated; valid handles are closed before returning.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            Some(&p_disp),
        ));
    }

    let mut buf = vec![0u16; 260];
    let result = loop {
        // SAFETY: `handle` is open and `buf` is valid for `buf.len()`.
        let len = unsafe {
            GetFinalPathNameByHandleW(handle, buf.as_mut_ptr(), buf.len() as u32, VOLUME_NAME_DOS)
        };
        if len == 0 {
            break Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                Some(&p_disp),
            ));
        }
        if (len as usize) < buf.len() {
            let out = std::ffi::OsString::from_wide(&buf[..len as usize])
                .to_string_lossy()
                .into_owned();
            break Ok(out);
        }
        buf.resize(len as usize + 1, 0);
    };
    // SAFETY: `handle` is valid and owned by this function.
    unsafe { CloseHandle(handle) };
    let out = result?;
    string_or_bytes_like_input(&path, out)
}

pub(crate) fn nt_getdiskusage(path: PyPath) -> Result<Value> {
    let p_disp = path.as_path().to_string_lossy().into_owned();
    let wide = encode_wide_nul(path.as_os_str())?;
    let mut free_avail: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;
    // SAFETY: `wide` is NUL-terminated and all output pointers are valid.
    let mut ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free_avail, &mut total, &mut total_free) };
    let mut err = std::io::Error::last_os_error();
    // A file, not a directory (`ERROR_DIRECTORY`): like CPython, retry with
    // the path cut at its last separator (`_dirnameW`).
    const ERROR_DIRECTORY: i32 = 267;
    if ok == 0 && err.raw_os_error() == Some(ERROR_DIRECTORY) {
        let len = wide.len() - 1;
        let cut = (1..len)
            .rev()
            .find(|&i| wide[i] == u16::from(b'\\') || wide[i] == u16::from(b'/'))
            .unwrap_or(0);
        let mut dir = wide[..cut].to_vec();
        dir.push(0);
        // SAFETY: `dir` is NUL-terminated and all output pointers are valid.
        ok = unsafe {
            GetDiskFreeSpaceExW(dir.as_ptr(), &mut free_avail, &mut total, &mut total_free)
        };
        err = std::io::Error::last_os_error();
    }
    if ok == 0 {
        return Err(PyError::from_io_error(&err, Some(&p_disp)));
    }
    Ok(Value::tuple(vec![
        u64_to_py_int(total),
        u64_to_py_int(free_avail),
    ]))
}

pub(crate) fn nt_getvolumepathname(path: PyPath) -> Result<Value> {
    let p_disp = path.as_path().to_string_lossy().into_owned();
    let wide = encode_wide_nul(path.as_os_str())?;
    let cap = wide.len().max(260) + 1;
    let mut buf = vec![0u16; cap];
    // SAFETY: `wide` is NUL-terminated and `buf` is valid for `buf.len()`.
    let ok = unsafe { GetVolumePathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if ok == 0 {
        return Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            Some(&p_disp),
        ));
    }
    let nul_pos = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let out = std::ffi::OsString::from_wide(&buf[..nul_pos])
        .to_string_lossy()
        .into_owned();
    string_or_bytes_like_input(&path, out)
}

pub(crate) fn device_encoding(fd: i64) -> Result<Value> {
    let is_tty = fd_ops::isatty(fd)?.as_bool();
    if !is_tty {
        return Ok(Value::none());
    }
    let cp = match fd {
        // SAFETY: `GetConsoleCP` / `GetConsoleOutputCP` take no arguments.
        0 => unsafe { GetConsoleCP() },
        1 | 2 => unsafe { GetConsoleOutputCP() },
        _ => return Ok(Value::none()),
    };
    if cp == 0 {
        Ok(Value::none())
    } else {
        Ok(Value::string(format!("cp{cp}")))
    }
}
