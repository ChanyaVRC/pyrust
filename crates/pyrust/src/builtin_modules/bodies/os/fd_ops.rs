//! Low-level file descriptor operations exposed by `os`:
//! `open`, `fdopen`, `close`, `read`, `write`, `dup`, `dup2`, `pipe`, `lseek`,
//! `fstat`, `fsync`, `truncate`, `ftruncate`, `isatty`, `closerange`,
//! `get_inheritable`, `set_inheritable`, and Unix `get_blocking`/`set_blocking`.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBytesLike, PyPath};
use crate::value::{Value, ValueKind};

#[cfg(not(windows))]
use super::result_types::make_stat_result;
use super::result_types::make_stat_result_for_path;

// ── Cross-platform O_* and SEEK_* flag constants ─────────────────────────────

#[cfg(unix)]
pub(crate) const O_RDONLY: i64 = libc::O_RDONLY as i64;
#[cfg(unix)]
pub(crate) const O_WRONLY: i64 = libc::O_WRONLY as i64;
#[cfg(unix)]
pub(crate) const O_RDWR: i64 = libc::O_RDWR as i64;
#[cfg(unix)]
pub(crate) const O_CREAT: i64 = libc::O_CREAT as i64;
#[cfg(unix)]
pub(crate) const O_EXCL: i64 = libc::O_EXCL as i64;
#[cfg(unix)]
pub(crate) const O_TRUNC: i64 = libc::O_TRUNC as i64;
#[cfg(unix)]
pub(crate) const O_APPEND: i64 = libc::O_APPEND as i64;

#[cfg(not(unix))]
pub(crate) const O_RDONLY: i64 = 0x0000;
#[cfg(not(unix))]
pub(crate) const O_WRONLY: i64 = 0x0001;
#[cfg(not(unix))]
pub(crate) const O_RDWR: i64 = 0x0002;
#[cfg(not(unix))]
pub(crate) const O_APPEND: i64 = 0x0008;
#[cfg(not(unix))]
pub(crate) const O_CREAT: i64 = 0x0100;
#[cfg(not(unix))]
pub(crate) const O_TRUNC: i64 = 0x0200;
#[cfg(not(unix))]
pub(crate) const O_EXCL: i64 = 0x0400;
#[cfg(windows)]
pub(super) const O_TEXT: i64 = 0x4000;
#[cfg(windows)]
pub(super) const O_BINARY: i64 = 0x8000;

pub(super) const SEEK_SET: i64 = 0;
pub(super) const SEEK_CUR: i64 = 1;
pub(super) const SEEK_END: i64 = 2;

// ── Process-local descriptor table ───────────────────────────────────────────

/// On Unix, entries are keyed by their real OS `RawFd` (`file.as_raw_fd()`) so
/// `FD_TABLE` never collides with fds created by `open()` or stdio (0/1/2).
/// On Windows, entries are allocated small integers starting at 3.
static FD_TABLE: LazyLock<Mutex<HashMap<i64, std::fs::File>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn ebadf_error() -> std::io::Error {
    #[cfg(unix)]
    {
        std::io::Error::from_raw_os_error(libc::EBADF)
    }
    #[cfg(not(unix))]
    {
        std::io::Error::from_raw_os_error(9)
    }
}

/// Register an open `std::fs::File` in the descriptor table and return its fd.
pub(crate) fn register_fd(file: std::fs::File) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd() as i64;
        let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        table.insert(fd, file);
        fd
    }
    #[cfg(not(unix))]
    {
        let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        let mut fd = 3_i64;
        while table.contains_key(&fd) {
            fd += 1;
        }
        table.insert(fd, file);
        fd
    }
}

/// Duplicate an open `std::fs::File` registered in the descriptor table.
pub(crate) fn clone_fd(fd: i64) -> std::io::Result<std::fs::File> {
    let table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = table.get(&fd) {
        return file.try_clone();
    }
    drop(table);
    #[cfg(unix)]
    {
        use std::os::unix::io::FromRawFd;
        if fd < 0 || fd > i32::MAX as i64 {
            return Err(ebadf_error());
        }
        // SAFETY: `fcntl(F_DUPFD_CLOEXEC)` safely duplicates an existing OS fd
        // or returns -1 with EBADF.
        let dup_raw = unsafe { libc::fcntl(fd as libc::c_int, libc::F_DUPFD_CLOEXEC, 0) };
        if dup_raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `dup_raw` is a fresh owned fd.
        Ok(unsafe { std::fs::File::from_raw_fd(dup_raw) })
    }
    #[cfg(not(unix))]
    {
        Err(ebadf_error())
    }
}

/// Remove and drop an fd from the descriptor table, returning `EBADF` if absent.
pub(crate) fn close_fd_raw(fd: i64) -> std::io::Result<()> {
    let file = {
        let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        table.remove(&fd)
    };
    if file.is_some() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        if fd < 0 || fd > i32::MAX as i64 {
            return Err(ebadf_error());
        }
        // SAFETY: closing a raw OS fd not in FD_TABLE; returns -1/EBADF if invalid.
        let rc = unsafe { libc::close(fd as libc::c_int) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        Err(ebadf_error())
    }
}

/// Query `std::fs::Metadata` for an integer fd (used by `genericpath` and `os.fstat`).
pub(crate) fn metadata_for_fd(fd: i64) -> std::io::Result<std::fs::Metadata> {
    {
        let table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = table.get(&fd) {
            return file.metadata();
        }
    }
    #[cfg(unix)]
    {
        use std::mem::ManuallyDrop;
        use std::os::unix::io::FromRawFd;
        if fd < 0 || fd > i32::MAX as i64 {
            return Err(ebadf_error());
        }
        // Verify fd is valid via fcntl before constructing a borrowed File.
        let valid = unsafe { libc::fcntl(fd as libc::c_int, libc::F_GETFD) };
        if valid < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `ManuallyDrop` ensures we never close the caller's fd.
        let borrowed = ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd as i32) });
        borrowed.metadata()
    }
    #[cfg(windows)]
    {
        if let Some(raw_h) = lookup_borrowed_win_handle(fd) {
            // Query via DuplicateHandle-free check or metadata on duplicated handle.
            use std::mem::ManuallyDrop;
            use std::os::windows::io::FromRawHandle;
            // SAFETY: `ManuallyDrop` ensures we never close the borrowed handle.
            let borrowed = ManuallyDrop::new(unsafe {
                std::fs::File::from_raw_handle(raw_h as *mut std::ffi::c_void)
            });
            return borrowed.metadata();
        }
        Err(ebadf_error())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fd;
        Err(ebadf_error())
    }
}

/// Return an `os.stat_result` for an open fd (with real Windows `st_ino`/`st_dev`).
pub(crate) fn fstat_fd_result(fd: i64) -> Result<Value> {
    {
        let table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = table.get(&fd) {
            let meta = file
                .metadata()
                .map_err(|e| PyError::from_io_error(&e, None))?;
            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawHandle;
                return Ok(super::result_types::make_stat_result_for_win_handle(
                    file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
                    &meta,
                ));
            }
            #[cfg(not(windows))]
            {
                return Ok(make_stat_result(&meta));
            }
        }
    }
    #[cfg(windows)]
    {
        if let Some(raw_h) = lookup_borrowed_win_handle(fd) {
            return super::result_types::stat_result_from_borrowed_win_handle(
                raw_h as windows_sys::Win32::Foundation::HANDLE,
            )
            .map_err(|e| PyError::from_io_error(&e, None));
        }
        Err(PyError::from_io_error(&ebadf_error(), None))
    }
    #[cfg(not(windows))]
    {
        let meta = metadata_for_fd(fd).map_err(|e| PyError::from_io_error(&e, None))?;
        Ok(make_stat_result(&meta))
    }
}

/// Return an `os.stat_result` for a filesystem path (with real Windows `st_ino`/`st_dev`).
pub(crate) fn stat_path_result(path: &Path) -> Result<Value> {
    let meta = std::fs::metadata(path)
        .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
    Ok(make_stat_result_for_path(path, &meta, true))
}

#[cfg(windows)]
fn lookup_borrowed_win_handle(fd: i64) -> Option<isize> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    if let Some(h) = pyrust_builtins::file::lookup_open_file_raw_handle(fd) {
        return Some(h);
    }
    let std_id = match fd {
        0 => STD_INPUT_HANDLE,
        1 => STD_OUTPUT_HANDLE,
        2 => STD_ERROR_HANDLE,
        _ => return None,
    };
    // SAFETY: `GetStdHandle` returns either a standard handle, NULL, or INVALID_HANDLE_VALUE.
    let h = unsafe { GetStdHandle(std_id) };
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        None
    } else {
        Some(h as isize)
    }
}

// ── Platform inheritable / blocking helpers ──────────────────────────────────

#[cfg(unix)]
fn set_fd_inheritable_raw(raw_fd: libc::c_int, inheritable: bool) -> std::io::Result<()> {
    // SAFETY: `fcntl(F_GETFD / F_SETFD)` on an OS fd.
    unsafe {
        let flags = libc::fcntl(raw_fd, libc::F_GETFD);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let new_flags = if inheritable {
            flags & !libc::FD_CLOEXEC
        } else {
            flags | libc::FD_CLOEXEC
        };
        if libc::fcntl(raw_fd, libc::F_SETFD, new_flags) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(unix)]
fn get_fd_inheritable_raw(raw_fd: libc::c_int) -> std::io::Result<bool> {
    // SAFETY: `fcntl(F_GETFD)` on an OS fd.
    let flags = unsafe { libc::fcntl(raw_fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((flags & libc::FD_CLOEXEC) == 0)
}

#[cfg(windows)]
fn set_handle_inheritable_raw(
    handle: windows_sys::Win32::Foundation::HANDLE,
    inheritable: bool,
) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    let flags = if inheritable { HANDLE_FLAG_INHERIT } else { 0 };
    // SAFETY: `handle` is an open Win32 handle.
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, flags) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn get_handle_inheritable_raw(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> std::io::Result<bool> {
    use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};
    let mut flags: u32 = 0;
    // SAFETY: `handle` is an open Win32 handle.
    if unsafe { GetHandleInformation(handle, &mut flags) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok((flags & HANDLE_FLAG_INHERIT) != 0)
    }
}

// ── Python-callable operations ───────────────────────────────────────────────

pub(super) fn open_path(path: PyPath, flags: i64, mode: i64) -> Result<Value> {
    let p = path.as_path();
    let p_str = p.to_string_lossy();
    if p_str.as_bytes().contains(&0) {
        return Err(PyError::named(
            "ValueError",
            "embedded null byte".to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::FromRawFd;
        let cpath = CString::new(p.as_os_str().as_bytes())
            .map_err(|_| PyError::named("ValueError", "embedded null byte".to_string()))?;
        let raw_flags = (flags as libc::c_int) | libc::O_CLOEXEC;
        let raw_mode = (mode & 0o7777) as libc::c_uint;
        // SAFETY: `cpath` is NUL-terminated; returned fd is checked for >= 0.
        let raw_fd = unsafe { libc::open(cpath.as_ptr(), raw_flags, raw_mode) };
        if raw_fd < 0 {
            let err = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&err, Some(&p_str)));
        }
        // SAFETY: `raw_fd` is a newly opened owned descriptor.
        let file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
        let fd = register_fd(file);
        Ok(Value::int(fd))
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        let mut opts = std::fs::OpenOptions::new();
        let access = flags & 0x3;
        match access {
            0 => {
                opts.read(true);
            }
            1 => {
                opts.write(true);
            }
            2 => {
                opts.read(true).write(true);
            }
            _ => {
                opts.read(true);
            }
        }
        if (flags & O_APPEND) != 0 {
            opts.append(true);
        }
        if (flags & O_CREAT) != 0 {
            if (flags & O_EXCL) != 0 {
                opts.create_new(true);
            } else {
                opts.create(true);
            }
            if access == 0 {
                opts.write(true);
            }
        }
        if (flags & O_TRUNC) != 0 {
            opts.truncate(true);
            if access == 0 {
                opts.write(true);
            }
        }
        let file = opts
            .open(p)
            .map_err(|e| PyError::from_io_error(&e, Some(&p_str)))?;
        let fd = register_fd(file);
        Ok(Value::int(fd))
    }
}

pub(super) fn fdopen(
    fd: i64,
    mode: &str,
    buffering: i64,
    encoding: Option<&str>,
    errors: Option<&str>,
    newline: Option<&str>,
    closefd: bool,
) -> Result<Value> {
    if fd < 0 {
        return Err(PyError::named(
            "ValueError",
            "negative file descriptor".to_string(),
        ));
    }
    let file = clone_fd(fd).map_err(|e| PyError::from_io_error(&e, None))?;
    let opts = pyrust_builtins::file::FileOpenOptions {
        mode,
        buffering,
        encoding,
        errors,
        newline,
        closefd,
    };
    pyrust_builtins::file::open_fd(fd, file, close_fd_raw, opts)
}

pub(super) fn close_fd(fd: i64) -> Result<Value> {
    close_fd_raw(fd).map_err(|e| PyError::from_io_error(&e, None))?;
    Ok(Value::none())
}

pub(super) fn read_fd(fd: i64, n: i64) -> Result<Value> {
    if n < 0 {
        return Err(PyError::named("OSError", "Invalid argument".to_string()));
    }
    let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = table.get_mut(&fd) {
        let mut buf = vec![0u8; n as usize];
        let count = file
            .read(&mut buf)
            .map_err(|e| PyError::from_io_error(&e, None))?;
        buf.truncate(count);
        return Ok(Value::bytes(buf));
    }
    drop(table);
    #[cfg(unix)]
    {
        if fd < 0 || fd > i32::MAX as i64 {
            return Err(PyError::from_io_error(&ebadf_error(), None));
        }
        let mut buf = vec![0u8; n as usize];
        // SAFETY: `buf` is valid for `n` bytes.
        let rc = unsafe { libc::read(fd as libc::c_int, buf.as_mut_ptr().cast(), buf.len()) };
        if rc < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        buf.truncate(rc as usize);
        Ok(Value::bytes(buf))
    }
    #[cfg(not(unix))]
    {
        Err(PyError::from_io_error(&ebadf_error(), None))
    }
}

pub(super) fn write_fd(fd: i64, data: PyBytesLike<'_>) -> Result<Value> {
    let bytes = data.as_bytes();
    let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = table.get_mut(&fd) {
        let written = file
            .write(bytes)
            .map_err(|e| PyError::from_io_error(&e, None))?;
        return Ok(Value::int(written as i64));
    }
    drop(table);
    #[cfg(unix)]
    {
        if fd < 0 || fd > i32::MAX as i64 {
            return Err(PyError::from_io_error(&ebadf_error(), None));
        }
        // SAFETY: `bytes` is valid for `bytes.len()` bytes.
        let rc = unsafe { libc::write(fd as libc::c_int, bytes.as_ptr().cast(), bytes.len()) };
        if rc < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::int(rc as i64))
    }
    #[cfg(not(unix))]
    {
        Err(PyError::from_io_error(&ebadf_error(), None))
    }
}

pub(super) fn dup(fd: i64) -> Result<Value> {
    let cloned = clone_fd(fd).map_err(|e| PyError::from_io_error(&e, None))?;
    // PEP 446: duplicated fds are non-inheritable by default.
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let _ = set_fd_inheritable_raw(cloned.as_raw_fd(), false);
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let _ = set_handle_inheritable_raw(
            cloned.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
            false,
        );
    }
    Ok(Value::int(register_fd(cloned)))
}

pub(super) fn dup2(fd: i64, fd2: i64, inheritable: bool) -> Result<Value> {
    if fd < 0 || fd2 < 0 {
        return Err(PyError::from_io_error(&ebadf_error(), None));
    }
    if fd == fd2 {
        // Verify fd is valid, then update its inheritable flag.
        let _ = get_inheritable(fd)?;
        set_inheritable(fd, inheritable)?;
        return Ok(Value::int(fd2));
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::FromRawFd;
        // Drop any tracked File at fd2 first so Rust doesn't double-close later.
        {
            let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
            table.remove(&fd2);
        }
        // SAFETY: `dup2` duplicates `fd` onto `fd2`, atomically closing old `fd2`.
        let res = unsafe { libc::dup2(fd as libc::c_int, fd2 as libc::c_int) };
        if res < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        set_fd_inheritable_raw(res, inheritable).map_err(|e| PyError::from_io_error(&e, None))?;
        if res >= 3 {
            // SAFETY: `res` is a newly created descriptor at `fd2`.
            let file = unsafe { std::fs::File::from_raw_fd(res) };
            let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
            table.insert(res as i64, file);
        }
        Ok(Value::int(res as i64))
    }
    #[cfg(not(unix))]
    {
        let cloned = clone_fd(fd).map_err(|e| PyError::from_io_error(&e, None))?;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let _ = set_handle_inheritable_raw(
                cloned.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
                inheritable,
            );
        }
        let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        table.insert(fd2, cloned);
        Ok(Value::int(fd2))
    }
}

pub(super) fn pipe() -> Result<Value> {
    let (reader, writer) = std::io::pipe().map_err(|e| PyError::from_io_error(&e, None))?;
    #[cfg(unix)]
    let (r_file, w_file) = {
        use std::os::unix::io::OwnedFd;
        (
            std::fs::File::from(OwnedFd::from(reader)),
            std::fs::File::from(OwnedFd::from(writer)),
        )
    };
    #[cfg(windows)]
    let (r_file, w_file) = {
        use std::os::windows::io::OwnedHandle;
        (
            std::fs::File::from(OwnedHandle::from(reader)),
            std::fs::File::from(OwnedHandle::from(writer)),
        )
    };
    let r_fd = register_fd(r_file);
    let w_fd = register_fd(w_file);
    Ok(Value::tuple(vec![Value::int(r_fd), Value::int(w_fd)]))
}

pub(super) fn lseek(fd: i64, pos: i64, whence: i64) -> Result<Value> {
    let seek_from = match whence {
        SEEK_SET => {
            if pos < 0 {
                return Err(PyError::named("OSError", "Invalid argument".to_string()));
            }
            SeekFrom::Start(pos as u64)
        }
        SEEK_CUR => SeekFrom::Current(pos),
        SEEK_END => SeekFrom::End(pos),
        _ => return Err(PyError::named("OSError", "Invalid argument".to_string())),
    };
    let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = table.get_mut(&fd) {
        let new_pos = file
            .seek(seek_from)
            .map_err(|e| PyError::from_io_error(&e, None))?;
        return Ok(Value::int(new_pos as i64));
    }
    drop(table);
    #[cfg(unix)]
    {
        if fd < 0 || fd > i32::MAX as i64 {
            return Err(PyError::from_io_error(&ebadf_error(), None));
        }
        // SAFETY: standard POSIX `lseek` call.
        let rc =
            unsafe { libc::lseek(fd as libc::c_int, pos as libc::off_t, whence as libc::c_int) };
        if rc < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::int(rc as i64))
    }
    #[cfg(not(unix))]
    {
        Err(PyError::from_io_error(&ebadf_error(), None))
    }
}

pub(super) fn fstat(fd: i64) -> Result<Value> {
    fstat_fd_result(fd)
}

pub(super) fn fsync(fd: i64) -> Result<Value> {
    {
        let table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = table.get(&fd) {
            file.sync_all()
                .map_err(|e| PyError::from_io_error(&e, None))?;
            return Ok(Value::none());
        }
    }
    #[cfg(unix)]
    {
        if fd < 0 || fd > i32::MAX as i64 {
            return Err(PyError::from_io_error(&ebadf_error(), None));
        }
        // SAFETY: POSIX `fsync` on raw fd.
        if unsafe { libc::fsync(fd as libc::c_int) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }
    #[cfg(not(unix))]
    {
        Err(PyError::from_io_error(&ebadf_error(), None))
    }
}

pub(super) fn ftruncate(fd: i64, length: i64) -> Result<Value> {
    if length < 0 {
        return Err(PyError::named("OSError", "Invalid argument".to_string()));
    }
    {
        let table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = table.get(&fd) {
            file.set_len(length as u64)
                .map_err(|e| PyError::from_io_error(&e, None))?;
            return Ok(Value::none());
        }
    }
    #[cfg(unix)]
    {
        if fd < 0 || fd > i32::MAX as i64 {
            return Err(PyError::from_io_error(&ebadf_error(), None));
        }
        // SAFETY: POSIX `ftruncate` on raw fd.
        if unsafe { libc::ftruncate(fd as libc::c_int, length as libc::off_t) } < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        Ok(Value::none())
    }
    #[cfg(not(unix))]
    {
        Err(PyError::from_io_error(&ebadf_error(), None))
    }
}

pub(super) fn truncate(
    interp: &mut crate::interpreter::Interpreter,
    path_or_fd: &Value,
    length: i64,
) -> Result<Value> {
    if length < 0 {
        return Err(PyError::named("OSError", "Invalid argument".to_string()));
    }
    match path_or_fd.kind() {
        ValueKind::Int(fd) => return ftruncate(fd, length),
        ValueKind::Bool(b) => return ftruncate(i64::from(b), length),
        _ => {}
    }
    let pv = crate::builtin_modules::genericpath::ops::fspath_resolve(interp, path_or_fd)?;
    let pb = pv.to_path_buf()?;
    let p_str = pb.to_string_lossy();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&pb)
        .map_err(|e| PyError::from_io_error(&e, Some(&p_str)))?;
    file.set_len(length as u64)
        .map_err(|e| PyError::from_io_error(&e, Some(&p_str)))?;
    Ok(Value::none())
}

pub(super) fn isatty(fd: i64) -> Result<Value> {
    if fd < 0 {
        return Ok(Value::bool_(false));
    }
    #[cfg(unix)]
    {
        if fd > i32::MAX as i64 {
            return Ok(Value::bool_(false));
        }
        // SAFETY: `isatty` safely returns 0 or 1 for any integer fd.
        Ok(Value::bool_(
            unsafe { libc::isatty(fd as libc::c_int) } != 0,
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_CHAR, GetFileType};
        use windows_sys::Win32::System::Console::GetConsoleMode;

        let handle: HANDLE = {
            let table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(file) = table.get(&fd) {
                file.as_raw_handle() as HANDLE
            } else {
                drop(table);
                match lookup_borrowed_win_handle(fd) {
                    Some(h) => h as HANDLE,
                    None => return Ok(Value::bool_(false)),
                }
            }
        };
        // SAFETY: `handle` is an open Win32 handle.
        let is_tty = unsafe {
            if GetFileType(handle) != FILE_TYPE_CHAR {
                false
            } else {
                let mut mode: u32 = 0;
                GetConsoleMode(handle, &mut mode) != 0
            }
        };
        Ok(Value::bool_(is_tty))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(Value::bool_(false))
    }
}

pub(super) fn closerange(fd_low: i64, fd_high: i64) -> Result<Value> {
    if fd_low >= fd_high {
        return Ok(Value::none());
    }
    let to_drop: Vec<std::fs::File> = {
        let mut table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
        let keys: Vec<i64> = table
            .keys()
            .copied()
            .filter(|&k| k >= fd_low && k < fd_high)
            .collect();
        keys.into_iter().filter_map(|k| table.remove(&k)).collect()
    };
    drop(to_drop);
    Ok(Value::none())
}

pub(super) fn get_inheritable(fd: i64) -> Result<Value> {
    if fd < 0 {
        return Err(PyError::from_io_error(&ebadf_error(), None));
    }
    #[cfg(unix)]
    {
        if fd > i32::MAX as i64 {
            return Err(PyError::from_io_error(&ebadf_error(), None));
        }
        let inheritable = get_fd_inheritable_raw(fd as libc::c_int)
            .map_err(|e| PyError::from_io_error(&e, None))?;
        Ok(Value::bool_(inheritable))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        let handle: HANDLE = {
            let table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(file) = table.get(&fd) {
                file.as_raw_handle() as HANDLE
            } else {
                drop(table);
                match lookup_borrowed_win_handle(fd) {
                    Some(h) => h as HANDLE,
                    None => return Err(PyError::from_io_error(&ebadf_error(), None)),
                }
            }
        };
        let inheritable =
            get_handle_inheritable_raw(handle).map_err(|e| PyError::from_io_error(&e, None))?;
        Ok(Value::bool_(inheritable))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(PyError::from_io_error(&ebadf_error(), None))
    }
}

pub(super) fn set_inheritable(fd: i64, inheritable: bool) -> Result<Value> {
    if fd < 0 {
        return Err(PyError::from_io_error(&ebadf_error(), None));
    }
    #[cfg(unix)]
    {
        if fd > i32::MAX as i64 {
            return Err(PyError::from_io_error(&ebadf_error(), None));
        }
        set_fd_inheritable_raw(fd as libc::c_int, inheritable)
            .map_err(|e| PyError::from_io_error(&e, None))?;
        Ok(Value::none())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        let handle: HANDLE = {
            let table = FD_TABLE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(file) = table.get(&fd) {
                file.as_raw_handle() as HANDLE
            } else {
                drop(table);
                match lookup_borrowed_win_handle(fd) {
                    Some(h) => h as HANDLE,
                    None => return Err(PyError::from_io_error(&ebadf_error(), None)),
                }
            }
        };
        set_handle_inheritable_raw(handle, inheritable)
            .map_err(|e| PyError::from_io_error(&e, None))?;
        Ok(Value::none())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = inheritable;
        Err(PyError::from_io_error(&ebadf_error(), None))
    }
}

#[cfg(unix)]
pub(super) fn get_blocking(fd: i64) -> Result<Value> {
    if fd < 0 || fd > i32::MAX as i64 {
        return Err(PyError::from_io_error(&ebadf_error(), None));
    }
    // SAFETY: `fcntl(F_GETFL)` safely checks flags or returns -1/EBADF.
    let flags = unsafe { libc::fcntl(fd as libc::c_int, libc::F_GETFL) };
    if flags < 0 {
        return Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            None,
        ));
    }
    Ok(Value::bool_((flags & libc::O_NONBLOCK) == 0))
}

#[cfg(unix)]
pub(super) fn set_blocking(fd: i64, blocking: bool) -> Result<Value> {
    if fd < 0 || fd > i32::MAX as i64 {
        return Err(PyError::from_io_error(&ebadf_error(), None));
    }
    // SAFETY: `fcntl(F_GETFL / F_SETFL)` toggles `O_NONBLOCK`.
    unsafe {
        let flags = libc::fcntl(fd as libc::c_int, libc::F_GETFL);
        if flags < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        let new_flags = if blocking {
            flags & !libc::O_NONBLOCK
        } else {
            flags | libc::O_NONBLOCK
        };
        if libc::fcntl(fd as libc::c_int, libc::F_SETFL, new_flags) < 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
    }
    Ok(Value::none())
}
