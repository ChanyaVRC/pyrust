//! Real OS kernel shared memory (`OsSharedMem`) powering
//! `multiprocessing.shared_memory.SharedMemory`, `ShareableList`, `Value`, and
//! `Array`.
//!
//! - On `#[cfg(unix)]`: POSIX `shm_open` + `ftruncate` + `mmap(MAP_SHARED)` +
//!   `munmap` + `shm_unlink`.
//! - On `#[cfg(windows)]`: Win32 `CreateFileMappingW` / `OpenFileMappingW` +
//!   `MapViewOfFile` + `VirtualQuery` + `UnmapViewOfFile` + `CloseHandle`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::random_hex;
use crate::error::{PyError, Result};

struct MappingRaw {
    ptr: *mut u8,
    size: usize,
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: OS shared-memory mappings (`mmap` / `MapViewOfFile`) reside at a
// process-wide virtual address valid on every thread until unmapped.
unsafe impl Send for MappingRaw {}
unsafe impl Sync for MappingRaw {}

impl MappingRaw {
    fn unmap_and_close(&mut self) {
        if self.ptr.is_null() {
            return;
        }
        #[cfg(unix)]
        {
            // SAFETY: `self.ptr` and `self.size` came from a successful `libc::mmap`.
            unsafe {
                libc::munmap(self.ptr as *mut libc::c_void, self.size);
            }
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile};
            // SAFETY: `self.ptr` came from `MapViewOfFile` and `self.handle` from
            // `CreateFileMappingW`/`OpenFileMappingW`.
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.ptr as *mut core::ffi::c_void,
                });
                if !self.handle.is_null() {
                    CloseHandle(self.handle);
                }
            }
            self.handle = std::ptr::null_mut();
        }
        self.ptr = std::ptr::null_mut();
        self.size = 0;
    }
}

impl Drop for MappingRaw {
    fn drop(&mut self) {
        self.unmap_and_close();
    }
}

/// Live cross-process OS shared memory block.
pub(crate) struct OsSharedMem {
    pub name: String,
    pub size: usize,
    raw: Mutex<MappingRaw>,
    closed: AtomicBool,
    auto_unlink: bool,
    unlinked: AtomicBool,
}

impl OsSharedMem {
    /// Create or attach to an OS shared memory segment.
    ///
    /// When `auto_unlink` is true (used for anonymous `Value`/`Array` segments),
    /// Unix automatically calls `shm_unlink` on `Drop` if not already unlinked.
    pub(crate) fn open(
        name: Option<&str>,
        create: bool,
        size: usize,
        auto_unlink: bool,
    ) -> Result<Arc<Self>> {
        if create && size == 0 {
            return Err(PyError::named(
                "ValueError",
                "'size' must be a positive integer",
            ));
        }
        if !create && name.is_none() {
            return Err(PyError::named(
                "ValueError",
                "'name' can only be None if create=True",
            ));
        }

        let public_name = match name {
            Some(n) => n.strip_prefix('/').unwrap_or(n).to_string(),
            None => {
                #[cfg(windows)]
                {
                    format!("wnsm_{}", random_hex(4))
                }
                #[cfg(unix)]
                {
                    format!("psm_{}", random_hex(4))
                }
            }
        };

        #[cfg(unix)]
        let (raw, actual_size) = open_unix(&public_name, create, size)?;
        #[cfg(windows)]
        let (raw, actual_size) = open_windows(&public_name, create, size)?;

        Ok(Arc::new(Self {
            name: public_name,
            size: actual_size,
            raw: Mutex::new(raw),
            closed: AtomicBool::new(false),
            auto_unlink,
            unlinked: AtomicBool::new(false),
        }))
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let mut g = self
                .raw
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g.unmap_and_close();
        }
    }

    pub(crate) fn unlink(&self) -> Result<()> {
        self.unlinked.store(true, Ordering::Release);
        #[cfg(unix)]
        {
            unlink_unix(&self.name)
        }
        #[cfg(windows)]
        {
            // Windows releases file mappings automatically once all handles close.
            Ok(())
        }
    }

    /// Read bytes from `[offset..offset + out.len()]` directly from OS shared memory.
    pub(crate) fn read_bytes(&self, offset: usize, out: &mut [u8]) -> Result<()> {
        let g = self
            .raw
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.ptr.is_null() {
            return Err(PyError::named(
                "ValueError",
                "operation forbidden on closed SharedMemory",
            ));
        }
        let end = offset
            .checked_add(out.len())
            .ok_or_else(|| PyError::named("IndexError", "shared memory offset overflow"))?;
        if end > g.size {
            return Err(PyError::named(
                "IndexError",
                "shared memory read out of bounds",
            ));
        }
        // SAFETY: `g.ptr` is non-null and mapped for at least `g.size` bytes.
        let src = unsafe { std::slice::from_raw_parts(g.ptr.add(offset), out.len()) };
        out.copy_from_slice(src);
        Ok(())
    }

    /// Write bytes to `[offset..offset + src.len()]` directly into OS shared memory.
    pub(crate) fn write_bytes(&self, offset: usize, src: &[u8]) -> Result<()> {
        let g = self
            .raw
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.ptr.is_null() {
            return Err(PyError::named(
                "ValueError",
                "operation forbidden on closed SharedMemory",
            ));
        }
        let end = offset
            .checked_add(src.len())
            .ok_or_else(|| PyError::named("IndexError", "shared memory offset overflow"))?;
        if end > g.size {
            return Err(PyError::named(
                "IndexError",
                "shared memory write out of bounds",
            ));
        }
        // SAFETY: `g.ptr` is non-null, writable, and mapped for at least `g.size` bytes.
        let dst = unsafe { std::slice::from_raw_parts_mut(g.ptr.add(offset), src.len()) };
        dst.copy_from_slice(src);
        Ok(())
    }

    /// Read a single byte at `offset`.
    pub(crate) fn read_byte(&self, offset: usize) -> Result<u8> {
        let mut b = [0u8; 1];
        self.read_bytes(offset, &mut b)?;
        Ok(b[0])
    }

    /// Write a single byte at `offset`.
    pub(crate) fn write_byte(&self, offset: usize, val: u8) -> Result<()> {
        self.write_bytes(offset, &[val])
    }
}

impl Drop for OsSharedMem {
    fn drop(&mut self) {
        self.close();
        if self.auto_unlink && !self.unlinked.swap(true, Ordering::AcqRel) {
            #[cfg(unix)]
            {
                let _ = unlink_unix(&self.name);
            }
        }
    }
}

#[cfg(unix)]
fn open_unix(public_name: &str, create: bool, size: usize) -> Result<(MappingRaw, usize)> {
    use std::ffi::CString;

    let shm_name = format!("/{public_name}");
    let c_name = CString::new(shm_name).map_err(|_| {
        PyError::named(
            "ValueError",
            "embedded null character in shared memory name",
        )
    })?;

    let flags = if create {
        libc::O_CREAT | libc::O_EXCL | libc::O_RDWR
    } else {
        libc::O_RDWR
    };

    // SAFETY: `c_name` is a valid NUL-terminated C string.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let fd = unsafe { libc::shm_open(c_name.as_ptr(), flags, 0o600 as libc::c_uint) };
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let fd = unsafe { libc::shm_open(c_name.as_ptr(), flags, 0o600 as libc::mode_t) };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        return Err(PyError::from_io_error(&err, Some(public_name)));
    }

    let actual_size = if create {
        // SAFETY: `fd` is a valid open shared memory file descriptor.
        if unsafe { libc::ftruncate(fd, size as libc::off_t) } < 0 {
            let err = std::io::Error::last_os_error();
            unsafe {
                libc::close(fd);
                libc::shm_unlink(c_name.as_ptr());
            }
            return Err(PyError::from_io_error(&err, Some(public_name)));
        }
        size
    } else {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } < 0 {
            let err = std::io::Error::last_os_error();
            unsafe {
                libc::close(fd);
            }
            return Err(PyError::from_io_error(&err, Some(public_name)));
        }
        st.st_size as usize
    };

    if actual_size == 0 {
        unsafe {
            libc::close(fd);
        }
        return Err(PyError::named(
            "ValueError",
            "cannot map zero-sized shared memory",
        ));
    }

    // SAFETY: `fd` is open and sized to `actual_size`.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            actual_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    unsafe {
        libc::close(fd);
    }
    if ptr == libc::MAP_FAILED || ptr.is_null() {
        let err = std::io::Error::last_os_error();
        if create {
            unsafe {
                libc::shm_unlink(c_name.as_ptr());
            }
        }
        return Err(PyError::from_io_error(&err, Some(public_name)));
    }

    Ok((
        MappingRaw {
            ptr: ptr as *mut u8,
            size: actual_size,
        },
        actual_size,
    ))
}

#[cfg(unix)]
fn unlink_unix(public_name: &str) -> Result<()> {
    use std::ffi::CString;

    let shm_name = format!("/{public_name}");
    let c_name = CString::new(shm_name).map_err(|_| {
        PyError::named(
            "ValueError",
            "embedded null character in shared memory name",
        )
    })?;
    // SAFETY: `c_name` is a valid NUL-terminated C string.
    if unsafe { libc::shm_unlink(c_name.as_ptr()) } < 0 {
        let err = std::io::Error::last_os_error();
        return Err(PyError::from_io_error(&err, Some(public_name)));
    }
    Ok(())
}

#[cfg(windows)]
fn open_windows(public_name: &str, create: bool, size: usize) -> Result<(MappingRaw, usize)> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_BASIC_INFORMATION, MapViewOfFile,
        OpenFileMappingW, PAGE_READWRITE, VirtualQuery,
    };

    let wide: Vec<u16> = OsStr::new(public_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let handle = if create {
        let size_u64 = size as u64;
        let high = (size_u64 >> 32) as u32;
        let low = (size_u64 & 0xFFFF_FFFF) as u32;
        // SAFETY: `wide` is NUL-terminated UTF-16.
        let h = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                high,
                low,
                wide.as_ptr(),
            )
        };
        if h.is_null() {
            let err = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&err, Some(public_name)));
        }
        // SAFETY: `GetLastError` checks whether `CreateFileMappingW` opened an existing mapping.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(h);
            }
            let err = std::io::Error::from_raw_os_error(ERROR_ALREADY_EXISTS as i32);
            return Err(PyError::from_io_error(&err, Some(public_name)));
        }
        h
    } else {
        // SAFETY: `wide` is NUL-terminated UTF-16.
        let h = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wide.as_ptr()) };
        if h.is_null() {
            let err = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&err, Some(public_name)));
        }
        h
    };

    let map_len = if create { size } else { 0 };
    // SAFETY: `handle` is a valid file mapping handle.
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, map_len) };
    if view.Value.is_null() {
        let err = std::io::Error::last_os_error();
        unsafe {
            CloseHandle(handle);
        }
        return Err(PyError::from_io_error(&err, Some(public_name)));
    }

    let actual_size = if create {
        size
    } else {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let queried = unsafe {
            VirtualQuery(
                view.Value,
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried == 0 || mbi.RegionSize == 0 {
            size.max(4096)
        } else {
            mbi.RegionSize
        }
    };

    Ok((
        MappingRaw {
            ptr: view.Value as *mut u8,
            size: actual_size,
            handle,
        },
        actual_size,
    ))
}
