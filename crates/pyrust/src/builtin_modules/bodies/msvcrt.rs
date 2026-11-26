// Native Rust `msvcrt` module (Windows-only, issue #3262).
//
// Reference: <https://docs.python.org/3.12/library/msvcrt.html>

use std::collections::HashMap;
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use num_traits::ToPrimitive;
use pyrust_derive::pyrust_module;
use windows_sys::Win32::Foundation::{
    ERROR_LOCK_VIOLATION, GetHandleInformation, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_CURRENT, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, SetFilePointerEx,
    UnlockFileEx,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    GetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOALIGNMENTFAULTEXCEPT, SEM_NOGPFAULTERRORBOX,
    SEM_NOOPENFILEERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapCompact};

use crate::builtin_modules::os::fd_ops;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyBytesLike, PyIndex, PyStr};
use crate::value::Value;

// ── Constants ────────────────────────────────────────────────────────────────

const LK_UNLCK: i64 = 0;
const LK_LOCK: i64 = 1;
const LK_NBLCK: i64 = 2;
const LK_RLCK: i64 = 3;
const LK_NBRLCK: i64 = 4;

const OUT_TO_DEFAULT: i64 = 0;
const OUT_TO_STDERR: i64 = 1;
const OUT_TO_MSGBOX: i64 = 2;
const REPORT_ERRMODE: i64 = 3;

const O_TEXT: i64 = 0x4000;
const O_BINARY: i64 = 0x8000;
const O_WTEXT: i64 = 0x10000;
const O_U16TEXT: i64 = 0x20000;
const O_U8TEXT: i64 = 0x40000;

/// Per-fd text/binary translation mode tracked for `msvcrt.setmode`.
static FD_MODES: LazyLock<Mutex<HashMap<i64, i64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Process CRT error mode tracked for `set_error_mode` / `get_error_mode`.
static CRT_ERROR_MODE: AtomicI64 = AtomicI64::new(OUT_TO_DEFAULT);

// Standard UCRT console I/O functions linked on MSVC Windows targets.
unsafe extern "C" {
    fn _kbhit() -> i32;
    fn _getch() -> i32;
    fn _getche() -> i32;
    fn _getwch() -> u16;
    fn _getwche() -> u16;
    fn _putch(c: i32) -> i32;
    fn _putwch(c: u16) -> u16;
    fn _ungetch(c: i32) -> i32;
    fn _ungetwch(c: u16) -> u16;
}

fn bad_fd_err() -> PyError {
    PyError::from_io_error(&fd_ops::ebadf_error(), None)
}

fn einval_err() -> PyError {
    PyError::from_io_error(&std::io::Error::from_raw_os_error(22), None)
}

fn resolve_fd_handle(fd: i64) -> Result<HANDLE> {
    fd_ops::lookup_win_handle(fd)
        .map(|h| h as HANDLE)
        .ok_or_else(bad_fd_err)
}

fn index_to_handle_isize(handle: PyIndex, fn_name: &str) -> Result<isize> {
    if let Some(i) = handle.as_i64() {
        return Ok(i as isize);
    }
    let bi = handle.to_bigint();
    if let Some(u) = bi.to_u64() {
        return Ok(u as usize as isize);
    }
    if let Some(i) = bi.to_i64() {
        return Ok(i as isize);
    }
    Err(PyError::named(
        "OverflowError",
        format!("{fn_name}(): handle out of range"),
    ))
}

fn single_byte_arg(data: PyBytesLike<'_>, fn_name: &str) -> Result<u8> {
    let bytes = data.as_bytes();
    if bytes.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() argument must be a byte string of length 1"),
        ));
    }
    Ok(bytes[0])
}

fn single_wide_char_arg(s: &str, fn_name: &str) -> Result<u16> {
    let units: Vec<u16> = s.encode_utf16().collect();
    if units.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() argument must be a unicode character"),
        ));
    }
    Ok(units[0])
}

fn lock_range(handle: HANDLE, mode: i64, nbytes: i64) -> Result<()> {
    if nbytes < 0 {
        return Err(einval_err());
    }
    if !matches!(mode, LK_UNLCK | LK_LOCK | LK_NBLCK | LK_RLCK | LK_NBRLCK) {
        return Err(einval_err());
    }
    let mut pos: i64 = 0;
    // SAFETY: `handle` is an open Win32 handle and `pos` is a valid out-pointer.
    if unsafe { SetFilePointerEx(handle, 0, &mut pos, FILE_CURRENT) } == 0 {
        return Err(PyError::from_io_error(
            &std::io::Error::last_os_error(),
            None,
        ));
    }

    let len_u64 = nbytes as u64;
    let len_lo = len_u64 as u32;
    let len_hi = (len_u64 >> 32) as u32;

    // SAFETY: `OVERLAPPED` is a plain-old-data Win32 struct valid when zeroed.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.Anonymous.Anonymous.Offset = (pos as u64) as u32;
    overlapped.Anonymous.Anonymous.OffsetHigh = ((pos as u64) >> 32) as u32;

    if mode == LK_UNLCK {
        // SAFETY: `handle` and `overlapped` are valid for the synchronous call.
        if unsafe { UnlockFileEx(handle, 0, len_lo, len_hi, &mut overlapped) } == 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        return Ok(());
    }

    let non_blocking = matches!(mode, LK_NBLCK | LK_NBRLCK);
    let flags = LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY;
    let max_attempts = if non_blocking { 1 } else { 10 };

    for attempt in 0..max_attempts {
        // SAFETY: `handle` and `overlapped` are valid for the call.
        if unsafe { LockFileEx(handle, flags, 0, len_lo, len_hi, &mut overlapped) } != 0 {
            return Ok(());
        }
        // SAFETY: `GetLastError` retrieves the error from `LockFileEx`.
        let err = unsafe { GetLastError() };
        if err == ERROR_LOCK_VIOLATION && attempt + 1 < max_attempts {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        return Err(PyError::from_io_error(
            &std::io::Error::from_raw_os_error(err as i32),
            None,
        ));
    }
    Err(PyError::from_io_error(
        &std::io::Error::from_raw_os_error(ERROR_LOCK_VIOLATION as i32),
        None,
    ))
}

pyrust_module! {
    constants {
        "LK_UNLCK" => Value::int(LK_UNLCK),
        "LK_LOCK" => Value::int(LK_LOCK),
        "LK_NBLCK" => Value::int(LK_NBLCK),
        "LK_RLCK" => Value::int(LK_RLCK),
        "LK_NBRLCK" => Value::int(LK_NBRLCK),
        "SEM_FAILCRITICALERRORS" => Value::int(SEM_FAILCRITICALERRORS as i64),
        "SEM_NOALIGNMENTFAULTEXCEPT" => Value::int(SEM_NOALIGNMENTFAULTEXCEPT as i64),
        "SEM_NOGPFAULTERRORBOX" => Value::int(SEM_NOGPFAULTERRORBOX as i64),
        "SEM_NOOPENFILEERRORBOX" => Value::int(SEM_NOOPENFILEERRORBOX as i64),
        "OUT_TO_DEFAULT" => Value::int(OUT_TO_DEFAULT),
        "OUT_TO_STDERR" => Value::int(OUT_TO_STDERR),
        "OUT_TO_MSGBOX" => Value::int(OUT_TO_MSGBOX),
        "REPORT_ERRMODE" => Value::int(REPORT_ERRMODE),
        "CRT_ASSEMBLY_VERSION" => Value::string("14.40.33810.0"),
    }

    // ── File locking & CRT file descriptors ──────────────────────────────────

    fn locking(
        #[positional_only] fd: PyIndex,
        #[positional_only] mode: PyIndex,
        #[positional_only] nbytes: PyIndex,
    ) -> Result<Value> {
        let fd_i = fd.expect_i64(FN_NAME, "fd")?;
        let mode_i = mode.expect_i64(FN_NAME, "mode")?;
        let nbytes_i = nbytes.expect_i64(FN_NAME, "nbytes")?;
        let handle = resolve_fd_handle(fd_i)?;
        lock_range(handle, mode_i, nbytes_i)?;
        Ok(Value::none())
    }

    fn setmode(
        #[positional_only] fd: PyIndex,
        #[positional_only] mode: PyIndex,
    ) -> Result<Value> {
        let fd_i = fd.expect_i64(FN_NAME, "fd")?;
        let mode_i = mode.expect_i64(FN_NAME, "mode")?;
        let _handle = resolve_fd_handle(fd_i)?;
        if !matches!(mode_i, O_TEXT | O_BINARY | O_WTEXT | O_U16TEXT | O_U8TEXT) {
            return Err(einval_err());
        }
        let mut modes = FD_MODES.lock().unwrap_or_else(|e| e.into_inner());
        let prev = modes.insert(fd_i, mode_i).unwrap_or(O_BINARY);
        Ok(Value::int(prev))
    }

    fn get_osfhandle(#[positional_only] fd: PyIndex) -> Result<Value> {
        let fd_i = fd.expect_i64(FN_NAME, "fd")?;
        let handle = resolve_fd_handle(fd_i)?;
        Ok(Value::int(handle as isize as i64))
    }

    fn open_osfhandle(
        #[positional_only] handle: PyIndex,
        #[positional_only] flags: PyIndex,
    ) -> Result<Value> {
        let raw_h = index_to_handle_isize(handle, FN_NAME)?;
        let flags_i = flags.expect_i64(FN_NAME, "flags")?;
        let h = raw_h as HANDLE;
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(bad_fd_err());
        }
        let mut info_flags: u32 = 0;
        // SAFETY: `GetHandleInformation` validates that `h` is a live kernel handle.
        if unsafe { GetHandleInformation(h, &mut info_flags) } == 0 {
            return Err(PyError::from_io_error(
                &std::io::Error::last_os_error(),
                None,
            ));
        }
        // SAFETY: `open_osfhandle` transfers ownership of the validated OS handle
        // into the new file descriptor managed by `os::fd_ops`.
        let file = unsafe { std::fs::File::from_raw_handle(h) };
        let fd = fd_ops::register_fd(file);
        let init_mode = if (flags_i & O_TEXT) != 0 {
            O_TEXT
        } else {
            O_BINARY
        };
        FD_MODES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fd, init_mode);
        Ok(Value::int(fd))
    }

    // ── Console I/O ──────────────────────────────────────────────────────────

    fn kbhit() -> Result<Value> {
        // SAFETY: `_kbhit` takes no arguments and inspects the console input queue.
        let hit = unsafe { _kbhit() };
        Ok(Value::bool_(hit != 0))
    }

    fn getch() -> Result<Value> {
        // SAFETY: `_getch` reads a single byte from the console without echo.
        let ch = unsafe { _getch() };
        Ok(Value::bytes(vec![(ch & 0xFF) as u8]))
    }

    fn getche() -> Result<Value> {
        // SAFETY: `_getche` reads a single byte from the console with echo.
        let ch = unsafe { _getche() };
        Ok(Value::bytes(vec![(ch & 0xFF) as u8]))
    }

    fn getwch() -> Result<Value> {
        // SAFETY: `_getwch` reads a single UTF-16 code unit from the console.
        let ch = unsafe { _getwch() };
        let s = String::from_utf16_lossy(&[ch]);
        Ok(Value::string(s))
    }

    fn getwche() -> Result<Value> {
        // SAFETY: `_getwche` reads a single UTF-16 code unit with echo.
        let ch = unsafe { _getwche() };
        let s = String::from_utf16_lossy(&[ch]);
        Ok(Value::string(s))
    }

    fn putch(#[positional_only] char: PyBytesLike<'_>) -> Result<Value> {
        let b = single_byte_arg(char, FN_NAME)?;
        // SAFETY: `_putch` writes one byte to the console.
        unsafe { _putch(i32::from(b)) };
        Ok(Value::none())
    }

    fn putwch(#[positional_only] unicode_char: PyStr<'_>) -> Result<Value> {
        let unit = single_wide_char_arg(&unicode_char, FN_NAME)?;
        // SAFETY: `_putwch` writes one UTF-16 code unit to the console.
        unsafe { _putwch(unit) };
        Ok(Value::none())
    }

    fn ungetch(#[positional_only] char: PyBytesLike<'_>) -> Result<Value> {
        let b = single_byte_arg(char, FN_NAME)?;
        // SAFETY: `_ungetch` pushes one byte back to the console buffer.
        let rc = unsafe { _ungetch(i32::from(b)) };
        if rc == -1 {
            return Err(PyError::from_io_error(
                &std::io::Error::from_raw_os_error(13),
                None,
            ));
        }
        Ok(Value::none())
    }

    fn ungetwch(#[positional_only] unicode_char: PyStr<'_>) -> Result<Value> {
        let unit = single_wide_char_arg(&unicode_char, FN_NAME)?;
        // SAFETY: `_ungetwch` pushes one UTF-16 code unit back to the console buffer.
        let rc = unsafe { _ungetwch(unit) };
        if rc == 0xFFFF {
            return Err(PyError::from_io_error(
                &std::io::Error::from_raw_os_error(13),
                None,
            ));
        }
        Ok(Value::none())
    }

    // ── Heap & Error Mode ────────────────────────────────────────────────────

    fn heapmin() -> Result<Value> {
        // SAFETY: `GetProcessHeap` returns the default process heap; `HeapCompact`
        // coalesces adjacent free blocks without failing on valid heap handles.
        unsafe {
            let heap = GetProcessHeap();
            if !heap.is_null() {
                let _ = HeapCompact(heap, 0);
            }
        }
        Ok(Value::none())
    }

    #[py_name = "GetErrorMode"]
    fn get_win_error_mode() -> Result<Value> {
        // SAFETY: `GetErrorMode` takes no arguments.
        let mode = unsafe { GetErrorMode() };
        Ok(Value::int(mode as i64))
    }

    #[py_name = "SetErrorMode"]
    fn set_win_error_mode(#[positional_only] mode: PyIndex) -> Result<Value> {
        let m = mode.expect_i64(FN_NAME, "mode")?;
        // SAFETY: `SetErrorMode` updates the process error mode flags.
        let prev = unsafe { SetErrorMode(m as u32) };
        Ok(Value::int(prev as i64))
    }

    fn set_error_mode(#[positional_only] mode: PyIndex) -> Result<Value> {
        let m = mode.expect_i64(FN_NAME, "mode")?;
        if !matches!(
            m,
            OUT_TO_DEFAULT | OUT_TO_STDERR | OUT_TO_MSGBOX | REPORT_ERRMODE
        ) {
            return Err(einval_err());
        }
        if m == REPORT_ERRMODE {
            return Ok(Value::int(CRT_ERROR_MODE.load(Ordering::Relaxed)));
        }
        let prev = CRT_ERROR_MODE.swap(m, Ordering::Relaxed);
        Ok(Value::int(prev))
    }

    fn get_error_mode() -> Result<Value> {
        Ok(Value::int(CRT_ERROR_MODE.load(Ordering::Relaxed)))
    }
}
