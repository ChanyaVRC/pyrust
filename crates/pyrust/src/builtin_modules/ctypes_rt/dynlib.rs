//! Cross-platform dynamic library loading, errno/LastError tracking, and
//! `ctypes.util.find_library` implementation (ZERO extra C dependencies).

use std::cell::Cell;

use crate::error::{PyError, Result};

// ── Thread-local ctypes errno / LastError copies ─────────────────────────────

thread_local! {
    static CTYPES_ERRNO: Cell<i32> = const { Cell::new(0) };
    #[cfg(windows)]
    static CTYPES_LAST_ERROR: Cell<u32> = const { Cell::new(0) };
}

pub(crate) fn get_errno() -> i32 {
    CTYPES_ERRNO.with(Cell::get)
}

pub(crate) fn set_errno(val: i32) -> i32 {
    CTYPES_ERRNO.with(|c| c.replace(val))
}

#[cfg(windows)]
pub(crate) fn get_last_error() -> u32 {
    CTYPES_LAST_ERROR.with(Cell::get)
}

#[cfg(windows)]
pub(crate) fn set_last_error(val: u32) -> u32 {
    CTYPES_LAST_ERROR.with(|c| c.replace(val))
}

#[cfg(windows)]
pub(crate) fn get_os_last_error() -> u32 {
    // SAFETY: `GetLastError` reads the calling thread's last-error code.
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

#[cfg(windows)]
pub(crate) fn set_os_last_error(code: u32) {
    // SAFETY: `SetLastError` writes the calling thread's last-error code.
    unsafe { windows_sys::Win32::Foundation::SetLastError(code) }
}

#[cfg(target_os = "linux")]
#[inline]
unsafe fn os_errno_ptr() -> *mut i32 {
    unsafe { libc::__errno_location() }
}

#[cfg(target_os = "macos")]
#[inline]
unsafe fn os_errno_ptr() -> *mut i32 {
    unsafe { libc::__error() }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
#[inline]
unsafe fn os_errno_ptr() -> *mut i32 {
    unsafe extern "C" {
        fn __errno() -> *mut i32;
    }
    unsafe { __errno() }
}

#[cfg(windows)]
#[inline]
unsafe fn os_errno_ptr() -> *mut i32 {
    unsafe extern "C" {
        fn _errno() -> *mut i32;
    }
    unsafe { _errno() }
}

pub(crate) fn sync_os_errno_before_call() {
    let val = get_errno();
    // SAFETY: `os_errno_ptr()` returns the calling thread's valid `errno` slot.
    unsafe {
        let ptr = os_errno_ptr();
        if !ptr.is_null() {
            *ptr = val;
        }
    }
}

pub(crate) fn sync_os_errno_after_call() {
    // SAFETY: `os_errno_ptr()` returns the calling thread's valid `errno` slot.
    let val = unsafe {
        let ptr = os_errno_ptr();
        if !ptr.is_null() { *ptr } else { 0 }
    };
    CTYPES_ERRNO.with(|c| c.set(val));
}

#[cfg(windows)]
pub(crate) fn sync_os_last_error_before_call() {
    set_os_last_error(get_last_error());
}

#[cfg(windows)]
pub(crate) fn sync_os_last_error_after_call() {
    let val = get_os_last_error();
    CTYPES_LAST_ERROR.with(|c| c.set(val));
}

// ── RTLD / DEFAULT_MODE constants ────────────────────────────────────────────

#[cfg(unix)]
pub(crate) fn rtld_global() -> i64 {
    libc::RTLD_GLOBAL as i64
}

#[cfg(unix)]
pub(crate) fn rtld_local() -> i64 {
    libc::RTLD_LOCAL as i64
}

#[cfg(unix)]
pub(crate) fn default_mode() -> i64 {
    libc::RTLD_LOCAL as i64
}

#[cfg(windows)]
pub(crate) fn rtld_global() -> i64 {
    0
}

#[cfg(windows)]
pub(crate) fn rtld_local() -> i64 {
    0
}

#[cfg(windows)]
pub(crate) fn default_mode() -> i64 {
    0
}

// ── Dynamic Library Loading: Unix ────────────────────────────────────────────

#[cfg(unix)]
pub(crate) fn dlopen(name: Option<&str>, mode: i32) -> Result<usize> {
    use std::ffi::{CStr, CString};

    let mut flags = mode;
    if (flags & (libc::RTLD_LAZY | libc::RTLD_NOW)) == 0 {
        flags |= libc::RTLD_NOW;
    }

    let handle = match name {
        None => {
            // SAFETY: passing NULL opens the global symbol table of the current process.
            unsafe { libc::dlopen(std::ptr::null(), flags) }
        }
        Some(path) => {
            let cstr = CString::new(path)
                .map_err(|_| PyError::named("ValueError", "embedded null character"))?;
            // SAFETY: `cstr` is a valid null-terminated string.
            unsafe { libc::dlopen(cstr.as_ptr(), flags) }
        }
    };

    if handle.is_null() {
        // SAFETY: `dlerror()` returns a static/thread-local C string or NULL.
        let err_ptr = unsafe { libc::dlerror() };
        let msg = if err_ptr.is_null() {
            format!("cannot open shared object '{}'", name.unwrap_or("<None>"))
        } else {
            unsafe { CStr::from_ptr(err_ptr) }
                .to_string_lossy()
                .into_owned()
        };
        return Err(PyError::named("OSError", msg));
    }
    Ok(handle as usize)
}

#[cfg(unix)]
pub(crate) fn dlsym(handle: usize, symbol: &str) -> Result<usize> {
    use std::ffi::{CStr, CString};

    let cstr = CString::new(symbol)
        .map_err(|_| PyError::named("ValueError", "embedded null character"))?;
    // SAFETY: Clear any prior dlerror, then call dlsym with valid handle and symbol name.
    unsafe {
        let _ = libc::dlerror();
        let sym = libc::dlsym(handle as *mut libc::c_void, cstr.as_ptr());
        if sym.is_null() {
            let err_ptr = libc::dlerror();
            let msg = if err_ptr.is_null() {
                format!("symbol '{symbol}' not found")
            } else {
                CStr::from_ptr(err_ptr).to_string_lossy().into_owned()
            };
            return Err(PyError::named("AttributeError", msg));
        }
        Ok(sym as usize)
    }
}

#[cfg(unix)]
pub(crate) fn dlclose(handle: usize) -> Result<()> {
    if handle == 0 {
        return Ok(());
    }
    // SAFETY: caller passes a valid handle returned by `dlopen`.
    let rc = unsafe { libc::dlclose(handle as *mut libc::c_void) };
    if rc != 0 {
        // SAFETY: `dlerror()` returns the error description for the failed `dlclose`.
        let err_ptr = unsafe { libc::dlerror() };
        let msg = if err_ptr.is_null() {
            "dlclose failed".to_string()
        } else {
            unsafe { std::ffi::CStr::from_ptr(err_ptr) }
                .to_string_lossy()
                .into_owned()
        };
        return Err(PyError::named("OSError", msg));
    }
    Ok(())
}

// ── Dynamic Library Loading: Windows ─────────────────────────────────────────

#[cfg(windows)]
pub(crate) fn format_error(code: Option<i32>) -> String {
    let err_code = match code {
        Some(c) if c != 0 => c,
        _ => get_os_last_error() as i32,
    };
    let raw = std::io::Error::from_raw_os_error(err_code).to_string();
    // Rust's `std::io::Error` formats as "<msg> (os error N)". Strip the trailing
    // " (os error N)" suffix so the output matches CPython's `FormatError`.
    if let Some(idx) = raw.rfind(" (os error ")
        && raw.ends_with(')')
    {
        return raw[..idx].trim_end().to_string();
    }
    raw.trim_end().to_string()
}

#[cfg(windows)]
pub(crate) fn load_library(name: &str) -> Result<usize> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;

    if name.contains('\0') {
        return Err(PyError::named("ValueError", "embedded null character"));
    }
    let wide: Vec<u16> = std::ffi::OsStr::new(name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a valid null-terminated UTF-16 string.
    let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
    if handle.is_null() {
        let code = get_os_last_error() as i32;
        let io_err = std::io::Error::from_raw_os_error(code);
        return Err(PyError::from_io_error(&io_err, Some(name)));
    }
    Ok(handle as usize)
}

#[cfg(windows)]
pub(crate) fn load_current_process_module() -> Result<usize> {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    // SAFETY: passing NULL retrieves the handle of the current executable module.
    let handle = unsafe { GetModuleHandleW(std::ptr::null()) };
    if handle.is_null() {
        let code = get_os_last_error() as i32;
        let io_err = std::io::Error::from_raw_os_error(code);
        return Err(PyError::from_io_error(&io_err, None));
    }
    Ok(handle as usize)
}

#[cfg(windows)]
pub(crate) fn get_proc_address(handle: usize, symbol: &str) -> Result<usize> {
    use std::ffi::CString;
    use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

    let cstr = CString::new(symbol)
        .map_err(|_| PyError::named("ValueError", "embedded null character"))?;
    // SAFETY: `handle` is an HMODULE and `cstr` is a valid null-terminated ANSI string.
    let proc = unsafe {
        GetProcAddress(
            handle as windows_sys::Win32::Foundation::HMODULE,
            cstr.as_ptr().cast::<u8>(),
        )
    };
    match proc {
        Some(f) => Ok(f as usize),
        None => Err(PyError::named(
            "AttributeError",
            format!("function '{symbol}' not found"),
        )),
    }
}

#[cfg(windows)]
pub(crate) fn get_proc_address_ordinal(handle: usize, ordinal: u16) -> Result<usize> {
    use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

    // SAFETY: Windows `MAKEINTRESOURCEA(ordinal)` passes an integer ordinal in the low 16 bits.
    let proc = unsafe {
        GetProcAddress(
            handle as windows_sys::Win32::Foundation::HMODULE,
            ordinal as usize as *const u8,
        )
    };
    match proc {
        Some(f) => Ok(f as usize),
        None => Err(PyError::named(
            "AttributeError",
            format!("function ordinal {ordinal} not found"),
        )),
    }
}

#[cfg(windows)]
pub(crate) fn free_library(handle: usize) -> Result<()> {
    use windows_sys::Win32::Foundation::FreeLibrary;

    if handle == 0 {
        return Ok(());
    }
    // SAFETY: `handle` is an HMODULE returned by `LoadLibraryW`.
    let ok = unsafe { FreeLibrary(handle as windows_sys::Win32::Foundation::HMODULE) };
    if ok == 0 {
        let code = get_os_last_error() as i32;
        let io_err = std::io::Error::from_raw_os_error(code);
        return Err(PyError::from_io_error(&io_err, None));
    }
    Ok(())
}

// ── Unified Cross-Platform Library Entry Points ──────────────────────────────

pub(crate) fn open_library(name: Option<&str>, mode: i32) -> Result<usize> {
    #[cfg(unix)]
    {
        dlopen(name, mode)
    }
    #[cfg(windows)]
    {
        let _ = mode;
        match name {
            None => load_current_process_module(),
            Some(path) => load_library(path),
        }
    }
}

pub(crate) fn lookup_symbol(handle: usize, symbol: &str) -> Result<usize> {
    #[cfg(unix)]
    {
        dlsym(handle, symbol)
    }
    #[cfg(windows)]
    {
        get_proc_address(handle, symbol)
    }
}

// ── `ctypes.util.find_library` ───────────────────────────────────────────────

#[cfg(windows)]
pub(crate) fn find_library(name: &str) -> Option<String> {
    // Match CPython 3.12 on Windows: `find_msvcrt()` returns `None` on Python 3.12+
    // (UCRT), so `find_library('c')` and `find_library('m')` return `None`.
    if name == "c" || name == "m" {
        return None;
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
        if name.to_ascii_lowercase().ends_with(".dll") {
            continue;
        }
        let with_dll = dir.join(format!("{name}.dll"));
        if with_dll.is_file() {
            return Some(with_dll.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(target_os = "macos")]
pub(crate) fn find_library(name: &str) -> Option<String> {
    use std::path::PathBuf;

    if name == "c" || name == "m" || name == "System" {
        // On macOS 11+, system dylibs live in the dyld shared cache; CPython's
        // `dyld_find` returns `/usr/lib/libSystem.dylib` (or `.B.dylib`).
        if dlopen(Some("/usr/lib/libSystem.dylib"), libc::RTLD_LAZY).is_ok() {
            return Some("/usr/lib/libSystem.dylib".to_string());
        }
        return Some("/usr/lib/libSystem.B.dylib".to_string());
    }

    let mut search_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(dyld) = std::env::var("DYLD_LIBRARY_PATH") {
        for p in dyld.split(':').filter(|s| !s.is_empty()) {
            search_dirs.push(PathBuf::from(p));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        search_dirs.push(PathBuf::from(home).join("lib"));
    }
    search_dirs.push(PathBuf::from("/usr/local/lib"));
    search_dirs.push(PathBuf::from("/opt/homebrew/lib"));
    search_dirs.push(PathBuf::from("/usr/lib"));
    search_dirs.push(PathBuf::from("/lib"));

    let candidates = [format!("lib{name}.dylib"), format!("{name}.dylib")];
    for dir in &search_dirs {
        for cand in &candidates {
            let full = dir.join(cand);
            if full.is_file() {
                return Some(full.to_string_lossy().into_owned());
            }
        }
    }

    // Check macOS Frameworks
    let fw_dirs = [
        PathBuf::from("/System/Library/Frameworks"),
        PathBuf::from("/Library/Frameworks"),
    ];
    for dir in &fw_dirs {
        let full = dir.join(format!("{name}.framework")).join(name);
        if full.is_file() {
            return Some(full.to_string_lossy().into_owned());
        }
    }

    // Fallback: dyld shared cache probe via dlopen
    for cand in &candidates {
        let usr_path = format!("/usr/lib/{cand}");
        if let Ok(h) = dlopen(Some(&usr_path), libc::RTLD_LAZY) {
            let _ = dlclose(h);
            return Some(usr_path);
        }
    }
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn find_library(name: &str) -> Option<String> {
    use std::path::Path;
    use std::process::{Command, Stdio};

    // 1. Try `/sbin/ldconfig -p` (exact CPython 3.12 strategy on Linux/glibc)
    let abi_hint: &str = if cfg!(target_arch = "x86_64") {
        "libc6,x86-64"
    } else if cfg!(target_arch = "aarch64") {
        "libc6,AArch64"
    } else {
        "libc6"
    };

    let prefix = format!("lib{name}.so");
    if let Ok(out) = Command::new("/sbin/ldconfig")
        .arg("-p")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let mut fallback_match: Option<String> = None;
        for line in text.lines() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with(&prefix) {
                continue;
            }
            let Some((soname, rest)) = trimmed.split_once(' ') else {
                continue;
            };
            // Ensure exact library name match: `lib<name>.so` or `lib<name>.so.<ver>`
            let suffix = &soname[prefix.len()..];
            if !suffix.is_empty() && !suffix.starts_with('.') {
                continue;
            }
            if rest.contains(abi_hint) {
                return Some(soname.to_string());
            }
            if fallback_match.is_none() && rest.contains("libc6") {
                fallback_match = Some(soname.to_string());
            }
        }
        if fallback_match.is_some() {
            return fallback_match;
        }
    }

    // 2. Fallback: scan standard library directories & LD_LIBRARY_PATH
    let mut dirs: Vec<String> = Vec::new();
    if let Ok(ld_path) = std::env::var("LD_LIBRARY_PATH") {
        for d in ld_path.split(':').filter(|s| !s.is_empty()) {
            dirs.push(d.to_string());
        }
    }
    #[cfg(target_arch = "x86_64")]
    dirs.extend([
        "/lib/x86_64-linux-gnu".to_string(),
        "/usr/lib/x86_64-linux-gnu".to_string(),
        "/lib64".to_string(),
        "/usr/lib64".to_string(),
    ]);
    #[cfg(target_arch = "aarch64")]
    dirs.extend([
        "/lib/aarch64-linux-gnu".to_string(),
        "/usr/lib/aarch64-linux-gnu".to_string(),
        "/lib64".to_string(),
        "/usr/lib64".to_string(),
    ]);
    dirs.extend(["/lib".to_string(), "/usr/lib".to_string()]);

    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(Path::new(dir)) else {
            continue;
        };
        let mut matches: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().into_owned();
            if let Some(rest) = fname.strip_prefix(&prefix)
                && (rest.is_empty() || rest.starts_with('.'))
            {
                matches.push(fname);
            }
        }
        matches.sort();
        if let Some(first) = matches.into_iter().next() {
            return Some(first);
        }
    }

    // 3. Final fallback for standard libc/libm
    if name == "c"
        && let Ok(h) = dlopen(Some("libc.so.6"), libc::RTLD_LAZY)
    {
        let _ = dlclose(h);
        return Some("libc.so.6".to_string());
    }
    if name == "m"
        && let Ok(h) = dlopen(Some("libm.so.6"), libc::RTLD_LAZY)
    {
        let _ = dlclose(h);
        return Some("libm.so.6".to_string());
    }
    None
}
