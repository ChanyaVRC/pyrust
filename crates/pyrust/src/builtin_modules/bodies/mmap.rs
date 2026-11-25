// `mmap` module — 100% native Rust implementation with PEP 688 buffer protocol
// and zero-copy `memoryview` integration (issue #3253).
//
// Supports:
// - Anonymous mappings (`fileno == -1`) and file-backed mappings (backed by
//   `std::fs::File` handles cloned from `os`, `tempfile`, `open()`, or Unix raw
//   fds, synced via positional `pread`/`pwrite` / `seek_read`/`seek_write` so
//   the caller's file seek cursor is never disturbed).
// - `ACCESS_DEFAULT`, `ACCESS_READ`, `ACCESS_WRITE`, `ACCESS_COPY`,
//   `PAGESIZE`, `ALLOCATIONGRANULARITY`, `error = OSError`, and Unix `MAP_*` /
//   `PROT_*` / `MADV_*` constants.
// - Full `mmap` method surface: `read`, `read_byte`, `readline`, `write`,
//   `write_byte`, `seek`, `tell`, `size`, `find`, `rfind`, `move`, `flush`,
//   `resize`, `close`, `closed`, `madvise` (Unix), `len(mm)`, indexing & slice
//   get/set, iteration, context manager (`__enter__`/`__exit__`), and PEP 688
//   `__buffer__` so `memoryview(mm)` shares the live buffer zero-copy and
//   enforces CPython `BufferError` export-count guards on `close()` / `resize()`.
//
// Reference: <https://docs.python.org/3/library/mmap.html>

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
#[cfg(windows)]
use crate::interpreter::builtin_args::PyStr;
use crate::interpreter::builtin_args::{PyBytesLike, PyIndex, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, NativeIterFrame};
use crate::value::{Value, ValueKind};

const ACCESS_DEFAULT: i64 = 0;
const ACCESS_READ: i64 = 1;
const ACCESS_WRITE: i64 = 2;
const ACCESS_COPY: i64 = 3;

const BUF_WRITABLE: i64 = 0x1;

fn get_pagesize() -> i64 {
    #[cfg(unix)]
    {
        let sz = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if sz > 0 { sz as i64 } else { 4096 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
        let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
        unsafe { GetSystemInfo(&mut info) };
        if info.dwPageSize > 0 {
            info.dwPageSize as i64
        } else {
            4096
        }
    }
}

fn get_allocation_granularity() -> i64 {
    #[cfg(unix)]
    {
        get_pagesize()
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
        let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
        unsafe { GetSystemInfo(&mut info) };
        if info.dwAllocationGranularity > 0 {
            info.dwAllocationGranularity as i64
        } else {
            65536
        }
    }
}

fn clone_file_from_fd(fd: i64) -> std::io::Result<std::fs::File> {
    if fd < 0 {
        #[cfg(unix)]
        return Err(std::io::Error::from_raw_os_error(libc::EBADF));
        #[cfg(windows)]
        return Err(std::io::Error::from_raw_os_error(9));
    }
    if let Ok(file) = super::os::clone_fd(fd) {
        return Ok(file);
    }
    if let Some(res) = super::tempfile::try_clone_temp_fd(fd) {
        return res;
    }
    if let Some(res) = pyrust_builtins::file::try_clone_file_by_fd(fd) {
        return res;
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::{BorrowedFd, RawFd};
        let raw = fd as libc::c_int;
        if unsafe { libc::fcntl(raw, libc::F_GETFD) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fcntl(F_GETFD)` succeeded so `raw` is a valid open descriptor.
        let borrowed = unsafe { BorrowedFd::borrow_raw(raw as RawFd) };
        borrowed.try_clone_to_owned().map(std::fs::File::from)
    }
    #[cfg(windows)]
    {
        Err(std::io::Error::from_raw_os_error(9))
    }
}

fn read_at_exact_or_eof(
    file: &std::fs::File,
    mut offset: u64,
    buf: &mut [u8],
) -> std::io::Result<()> {
    let mut filled = 0usize;
    while filled < buf.len() {
        #[cfg(unix)]
        let n = {
            use std::os::unix::fs::FileExt;
            file.read_at(&mut buf[filled..], offset)?
        };
        #[cfg(windows)]
        let n = {
            use std::os::windows::fs::FileExt;
            file.seek_read(&mut buf[filled..], offset)?
        };
        if n == 0 {
            break;
        }
        filled += n;
        offset += n as u64;
    }
    Ok(())
}

fn write_at_all(file: &std::fs::File, mut offset: u64, mut bytes: &[u8]) -> std::io::Result<()> {
    while !bytes.is_empty() {
        #[cfg(unix)]
        let n = {
            use std::os::unix::fs::FileExt;
            file.write_at(bytes, offset)?
        };
        #[cfg(windows)]
        let n = {
            use std::os::windows::fs::FileExt;
            file.seek_write(bytes, offset)?
        };
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "failed to write whole buffer",
            ));
        }
        bytes = &bytes[n..];
        offset += n as u64;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MmapAccess {
    Read,
    Write,
    Copy,
}

impl MmapAccess {
    fn is_readonly(self) -> bool {
        matches!(self, Self::Read)
    }

    fn is_writeback(self) -> bool {
        matches!(self, Self::Write)
    }

    fn repr_name(self) -> &'static str {
        match self {
            Self::Read => "ACCESS_READ",
            Self::Write => "ACCESS_WRITE",
            Self::Copy => "ACCESS_COPY",
        }
    }
}

struct MmapState {
    data: Rc<RefCell<Vec<u8>>>,
    exports: Rc<Cell<usize>>,
    file: Option<std::fs::File>,
    offset: u64,
    pos: usize,
    access: MmapAccess,
    closed: bool,
}

impl MmapState {
    fn ensure_open(&self) -> Result<()> {
        if self.closed {
            return Err(PyError::named("ValueError", "mmap closed or invalid"));
        }
        Ok(())
    }

    fn ensure_writable(&self) -> Result<()> {
        self.ensure_open()?;
        if self.access.is_readonly() {
            return Err(PyError::named(
                "TypeError",
                "mmap can't modify a readonly memory map.",
            ));
        }
        Ok(())
    }

    fn sync_range(&self, start: usize, end: usize) -> Result<()> {
        if !self.access.is_writeback() {
            return Ok(());
        }
        let Some(ref file) = self.file else {
            return Ok(());
        };
        let buf = self.data.borrow();
        let clamped_end = end.min(buf.len());
        if start >= clamped_end {
            return Ok(());
        }
        write_at_all(file, self.offset + start as u64, &buf[start..clamped_end])
            .map_err(|e| PyError::from_io_error(&e, None))
    }

    fn close_internal(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        if self.exports.get() > 0 {
            return Err(PyError::named(
                "BufferError",
                "cannot close exported pointers exist",
            ));
        }
        let len = self.data.borrow().len();
        let sync_res = self.sync_range(0, len);
        self.file = None;
        self.data.borrow_mut().clear();
        self.closed = true;
        sync_res
    }
}

impl Drop for MmapState {
    fn drop(&mut self) {
        if !self.closed && self.access.is_writeback() {
            let len = self.data.borrow().len();
            let _ = self.sync_range(0, len);
        }
        self.file = None;
        self.closed = true;
    }
}

fn init_mmap_state(fileno: i64, length: i64, access: MmapAccess, offset: i64) -> Result<MmapState> {
    if length < 0 {
        return Err(PyError::named(
            "ValueError",
            "memory mapped length must be positive",
        ));
    }
    if offset < 0 {
        return Err(PyError::named(
            "ValueError",
            "memory mapped offset must be positive",
        ));
    }
    let gran = get_allocation_granularity();
    if offset % gran != 0 {
        #[cfg(unix)]
        let err = std::io::Error::from_raw_os_error(libc::EINVAL);
        #[cfg(windows)]
        let err = std::io::Error::from_raw_os_error(22);
        return Err(PyError::from_io_error(&err, None));
    }
    let offset_u64 = offset as u64;

    if fileno == -1 {
        if length == 0 {
            return Err(PyError::named("ValueError", "cannot mmap an empty file"));
        }
        let len_usize = usize::try_from(length)
            .map_err(|_| PyError::named("OverflowError", "memory mapped length is too large"))?;
        return Ok(MmapState {
            data: Rc::new(RefCell::new(vec![0u8; len_usize])),
            exports: Rc::new(Cell::new(0)),
            file: None,
            offset: offset_u64,
            pos: 0,
            access,
            closed: false,
        });
    }

    let file = clone_file_from_fd(fileno).map_err(|e| PyError::from_io_error(&e, None))?;
    let file_len = file
        .metadata()
        .map_err(|e| PyError::from_io_error(&e, None))?
        .len();

    if offset_u64 > file_len {
        return Err(PyError::named(
            "ValueError",
            "mmap offset is greater than file size",
        ));
    }

    let map_len: usize = if length == 0 {
        let avail = file_len.saturating_sub(offset_u64);
        if avail == 0 {
            return Err(PyError::named("ValueError", "cannot mmap an empty file"));
        }
        usize::try_from(avail)
            .map_err(|_| PyError::named("OverflowError", "memory mapped length is too large"))?
    } else {
        let req = length as u64;
        let end = offset_u64
            .checked_add(req)
            .ok_or_else(|| PyError::named("OverflowError", "memory mapped size is too large"))?;
        if end > file_len {
            if cfg!(windows) && access.is_writeback() {
                file.set_len(end)
                    .map_err(|e| PyError::from_io_error(&e, None))?;
            } else {
                return Err(PyError::named(
                    "ValueError",
                    "mmap length is greater than file size",
                ));
            }
        }
        usize::try_from(req)
            .map_err(|_| PyError::named("OverflowError", "memory mapped length is too large"))?
    };

    let mut buf = vec![0u8; map_len];
    read_at_exact_or_eof(&file, offset_u64, &mut buf)
        .map_err(|e| PyError::from_io_error(&e, None))?;

    Ok(MmapState {
        data: Rc::new(RefCell::new(buf)),
        exports: Rc::new(Cell::new(0)),
        file: Some(file),
        offset: offset_u64,
        pos: 0,
        access,
        closed: false,
    })
}

fn normalize_find_bounds(
    len: usize,
    start: Option<i64>,
    end: Option<i64>,
    default_start: usize,
) -> (usize, usize) {
    let len_i64 = len as i64;
    let s = match start {
        None => default_start.min(len),
        Some(v) if v < 0 => (v + len_i64).max(0) as usize,
        Some(v) => v.min(len_i64) as usize,
    };
    let e = match end {
        None => len,
        Some(v) if v < 0 => (v + len_i64).max(0) as usize,
        Some(v) => v.min(len_i64) as usize,
    };
    (s, e)
}

fn collect_slice_indices(start: i64, stop: i64, step: i64) -> Vec<usize> {
    let mut out = Vec::new();
    let mut cur = start;
    if step > 0 {
        while cur < stop {
            out.push(cur as usize);
            cur += step;
        }
    } else {
        while cur > stop {
            out.push(cur as usize);
            cur += step;
        }
    }
    out
}

pyrust_module! {
    constants {
        "error" => Value::py_class(
            crate::interpreter::lookup_exc_class("OSError").expect("OSError class")
        ),
        "ACCESS_DEFAULT" => Value::int(ACCESS_DEFAULT),
        "ACCESS_READ" => Value::int(ACCESS_READ),
        "ACCESS_WRITE" => Value::int(ACCESS_WRITE),
        "ACCESS_COPY" => Value::int(ACCESS_COPY),
        "PAGESIZE" => Value::int(get_pagesize()),
        "ALLOCATIONGRANULARITY" => Value::int(get_allocation_granularity()),

        #[cfg(unix)]
        "MAP_SHARED" => Value::int(libc::MAP_SHARED as i64),
        #[cfg(unix)]
        "MAP_PRIVATE" => Value::int(libc::MAP_PRIVATE as i64),
        #[cfg(unix)]
        "MAP_ANON" => Value::int(libc::MAP_ANON as i64),
        #[cfg(unix)]
        "MAP_ANONYMOUS" => Value::int(libc::MAP_ANONYMOUS as i64),
        #[cfg(unix)]
        "PROT_READ" => Value::int(libc::PROT_READ as i64),
        #[cfg(unix)]
        "PROT_WRITE" => Value::int(libc::PROT_WRITE as i64),
        #[cfg(unix)]
        "PROT_EXEC" => Value::int(libc::PROT_EXEC as i64),

        #[cfg(unix)]
        "MADV_NORMAL" => Value::int(libc::MADV_NORMAL as i64),
        #[cfg(unix)]
        "MADV_RANDOM" => Value::int(libc::MADV_RANDOM as i64),
        #[cfg(unix)]
        "MADV_SEQUENTIAL" => Value::int(libc::MADV_SEQUENTIAL as i64),
        #[cfg(unix)]
        "MADV_WILLNEED" => Value::int(libc::MADV_WILLNEED as i64),
        #[cfg(unix)]
        "MADV_DONTNEED" => Value::int(libc::MADV_DONTNEED as i64),

        #[cfg(target_os = "linux")]
        "MADV_REMOVE" => Value::int(libc::MADV_REMOVE as i64),
        #[cfg(target_os = "linux")]
        "MADV_DONTFORK" => Value::int(libc::MADV_DONTFORK as i64),
        #[cfg(target_os = "linux")]
        "MADV_DOFORK" => Value::int(libc::MADV_DOFORK as i64),
        #[cfg(target_os = "linux")]
        "MADV_MERGEABLE" => Value::int(libc::MADV_MERGEABLE as i64),
        #[cfg(target_os = "linux")]
        "MADV_UNMERGEABLE" => Value::int(libc::MADV_UNMERGEABLE as i64),
        #[cfg(target_os = "linux")]
        "MADV_HUGEPAGE" => Value::int(libc::MADV_HUGEPAGE as i64),
        #[cfg(target_os = "linux")]
        "MADV_NOHUGEPAGE" => Value::int(libc::MADV_NOHUGEPAGE as i64),
        #[cfg(target_os = "linux")]
        "MADV_DONTDUMP" => Value::int(libc::MADV_DONTDUMP as i64),
        #[cfg(target_os = "linux")]
        "MADV_DODUMP" => Value::int(libc::MADV_DODUMP as i64),
        #[cfg(target_os = "linux")]
        "MADV_FREE" => Value::int(libc::MADV_FREE as i64),
    }

    #[native_state(MmapState)]
    class mmap {
        #[cfg(unix)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            fileno: PyIndex,
            length: PyIndex,
            #[default(None)] flags: Option<PyIndex>,
            #[default(None)] prot: Option<PyIndex>,
            #[default(0_i64.into())] access: PyIndex,
            #[default(0_i64.into())] offset: PyIndex,
        ) -> Result<Value> {
            let fd_val = fileno.expect_i64(FN_NAME, "fileno")?;
            let len_val = length.expect_i64(FN_NAME, "length")?;
            let access_val = access.expect_i64(FN_NAME, "access")?;
            let offset_val = offset.expect_i64(FN_NAME, "offset")?;

            let has_flags_or_prot = flags.is_some() || prot.is_some();
            if access_val != ACCESS_DEFAULT && has_flags_or_prot {
                return Err(PyError::named(
                    "ValueError",
                    "mmap can't specify both access and flags, prot.",
                ));
            }
            let effective_access = match access_val {
                ACCESS_DEFAULT => {
                    let fl = match flags {
                        Some(f) => f.expect_i64(FN_NAME, "flags")?,
                        None => libc::MAP_SHARED as i64,
                    };
                    let pr = match prot {
                        Some(p) => p.expect_i64(FN_NAME, "prot")?,
                        None => (libc::PROT_WRITE | libc::PROT_READ) as i64,
                    };
                    if (pr & (libc::PROT_WRITE as i64)) == 0 {
                        MmapAccess::Read
                    } else if (fl & (libc::MAP_PRIVATE as i64)) != 0 {
                        MmapAccess::Copy
                    } else {
                        MmapAccess::Write
                    }
                }
                ACCESS_READ => MmapAccess::Read,
                ACCESS_WRITE => MmapAccess::Write,
                ACCESS_COPY => MmapAccess::Copy,
                _ => {
                    return Err(PyError::named(
                        "ValueError",
                        "mmap invalid access parameter.",
                    ));
                }
            };
            let state = init_mmap_state(fd_val, len_val, effective_access, offset_val)?;
            set_state(self_val.0, state)?;
            Ok(Value::none())
        }

        #[cfg(windows)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            fileno: PyIndex,
            length: PyIndex,
            #[default(None)] tagname: Option<PyStr<'_>>,
            #[default(0_i64.into())] access: PyIndex,
            #[default(0_i64.into())] offset: PyIndex,
        ) -> Result<Value> {
            let _ = tagname;
            let fd_val = fileno.expect_i64(FN_NAME, "fileno")?;
            let len_val = length.expect_i64(FN_NAME, "length")?;
            let access_val = access.expect_i64(FN_NAME, "access")?;
            let offset_val = offset.expect_i64(FN_NAME, "offset")?;

            let effective_access = match access_val {
                ACCESS_DEFAULT | ACCESS_WRITE => MmapAccess::Write,
                ACCESS_READ => MmapAccess::Read,
                ACCESS_COPY => MmapAccess::Copy,
                _ => {
                    return Err(PyError::named(
                        "ValueError",
                        "mmap invalid access parameter.",
                    ));
                }
            };
            let state = init_mmap_state(fd_val, len_val, effective_access, offset_val)?;
            set_state(self_val.0, state)?;
            Ok(Value::none())
        }

        #[getter]
        fn closed(#[state] st: &MmapState) -> Result<Value> {
            Ok(Value::bool_(st.closed))
        }

        fn close(#[state_mut] st: &mut MmapState) -> Result<Value> {
            st.close_internal()?;
            Ok(Value::none())
        }

        fn size(#[state] st: &MmapState) -> Result<Value> {
            st.ensure_open()?;
            if let Some(ref file) = st.file {
                let len = file
                    .metadata()
                    .map_err(|e| PyError::from_io_error(&e, None))?
                    .len();
                Ok(Value::int(len as i64))
            } else {
                Ok(Value::int(st.data.borrow().len() as i64))
            }
        }

        fn tell(#[state] st: &MmapState) -> Result<Value> {
            st.ensure_open()?;
            Ok(Value::int(st.pos as i64))
        }

        fn seek(
            #[state_mut] st: &mut MmapState,
            #[positional_only] pos: PyIndex,
            #[positional_only]
            #[default(0_i64.into())]
            whence: PyIndex,
        ) -> Result<Value> {
            st.ensure_open()?;
            let delta = pos.expect_i64(FN_NAME, "pos")?;
            let w = whence.expect_i64(FN_NAME, "whence")?;
            let len = st.data.borrow().len() as i64;
            let base = match w {
                0 => 0i64,
                1 => st.pos as i64,
                2 => len,
                _ => return Err(PyError::named("ValueError", "unknown seek type")),
            };
            let target = base.checked_add(delta).ok_or_else(|| {
                PyError::named("ValueError", "seek out of range")
            })?;
            if target < 0 || target > len {
                return Err(PyError::named("ValueError", "seek out of range"));
            }
            st.pos = target as usize;
            Ok(Value::none())
        }

        fn read(
            #[state_mut] st: &mut MmapState,
            #[positional_only]
            #[default(None)]
            n: Option<PyIndex>,
        ) -> Result<Value> {
            st.ensure_open()?;
            let buf = st.data.borrow();
            let len = buf.len();
            let start = st.pos.min(len);
            let end = match n {
                None => len,
                Some(idx) => {
                    let count = idx.expect_i64(FN_NAME, "n")?;
                    if count < 0 {
                        len
                    } else {
                        start.saturating_add(count as usize).min(len)
                    }
                }
            };
            let out = buf[start..end].to_vec();
            drop(buf);
            st.pos = end;
            Ok(Value::bytes(out))
        }

        fn read_byte(#[state_mut] st: &mut MmapState) -> Result<Value> {
            st.ensure_open()?;
            let buf = st.data.borrow();
            if st.pos >= buf.len() {
                return Err(PyError::named("ValueError", "read byte out of range"));
            }
            let b = buf[st.pos];
            drop(buf);
            st.pos += 1;
            Ok(Value::int(b as i64))
        }

        fn readline(#[state_mut] st: &mut MmapState) -> Result<Value> {
            st.ensure_open()?;
            let buf = st.data.borrow();
            let len = buf.len();
            let start = st.pos.min(len);
            let mut end = start;
            while end < len {
                let b = buf[end];
                end += 1;
                if b == b'\n' {
                    break;
                }
            }
            let out = buf[start..end].to_vec();
            drop(buf);
            st.pos = end;
            Ok(Value::bytes(out))
        }

        fn write(
            #[state_mut] st: &mut MmapState,
            #[positional_only] bytes: PyBytesLike<'_>,
        ) -> Result<Value> {
            st.ensure_writable()?;
            let slice = bytes.as_bytes();
            let n = slice.len();
            let start = st.pos;
            let end = start.checked_add(n).ok_or_else(|| {
                PyError::named("ValueError", "data out of range")
            })?;
            {
                let mut buf = st.data.borrow_mut();
                if end > buf.len() {
                    return Err(PyError::named("ValueError", "data out of range"));
                }
                buf[start..end].copy_from_slice(slice);
            }
            st.sync_range(start, end)?;
            st.pos = end;
            Ok(Value::int(n as i64))
        }

        fn write_byte(
            #[state_mut] st: &mut MmapState,
            #[positional_only] byte: PyIndex,
        ) -> Result<Value> {
            st.ensure_writable()?;
            let v = byte.expect_i64(FN_NAME, "byte")?;
            if !(0..=255).contains(&v) {
                return Err(PyError::named(
                    "ValueError",
                    "write_byte() argument must be in range(0, 256)",
                ));
            }
            let pos = st.pos;
            {
                let mut buf = st.data.borrow_mut();
                if pos >= buf.len() {
                    return Err(PyError::named("ValueError", "write byte out of range"));
                }
                buf[pos] = v as u8;
            }
            st.sync_range(pos, pos + 1)?;
            st.pos = pos + 1;
            Ok(Value::none())
        }

        fn find(
            #[state] st: &MmapState,
            #[positional_only] sub: PyBytesLike<'_>,
            #[positional_only]
            #[default(None)]
            start: Option<PyIndex>,
            #[positional_only]
            #[default(None)]
            end: Option<PyIndex>,
        ) -> Result<Value> {
            st.ensure_open()?;
            let start_opt = match start {
                Some(i) => Some(i.expect_i64(FN_NAME, "start")?),
                None => None,
            };
            let end_opt = match end {
                Some(i) => Some(i.expect_i64(FN_NAME, "end")?),
                None => None,
            };
            let buf = st.data.borrow();
            let (s, e) = normalize_find_bounds(buf.len(), start_opt, end_opt, st.pos);
            if s > e {
                return Ok(Value::int(-1));
            }
            let needle = sub.as_bytes();
            if needle.is_empty() {
                return Ok(Value::int(s as i64));
            }
            let hay = &buf[s..e];
            match hay.windows(needle.len()).position(|w| w == needle) {
                Some(rel) => Ok(Value::int((s + rel) as i64)),
                None => Ok(Value::int(-1)),
            }
        }

        fn rfind(
            #[state] st: &MmapState,
            #[positional_only] sub: PyBytesLike<'_>,
            #[positional_only]
            #[default(None)]
            start: Option<PyIndex>,
            #[positional_only]
            #[default(None)]
            end: Option<PyIndex>,
        ) -> Result<Value> {
            st.ensure_open()?;
            let start_opt = match start {
                Some(i) => Some(i.expect_i64(FN_NAME, "start")?),
                None => None,
            };
            let end_opt = match end {
                Some(i) => Some(i.expect_i64(FN_NAME, "end")?),
                None => None,
            };
            let buf = st.data.borrow();
            let (s, e) = normalize_find_bounds(buf.len(), start_opt, end_opt, st.pos);
            if s > e {
                return Ok(Value::int(-1));
            }
            let needle = sub.as_bytes();
            if needle.is_empty() {
                return Ok(Value::int(e as i64));
            }
            let hay = &buf[s..e];
            match hay.windows(needle.len()).rposition(|w| w == needle) {
                Some(rel) => Ok(Value::int((s + rel) as i64)),
                None => Ok(Value::int(-1)),
            }
        }

        #[py_name = "move"]
        fn mmap_move(
            #[state_mut] st: &mut MmapState,
            #[positional_only] dest: PyIndex,
            #[positional_only] src: PyIndex,
            #[positional_only] count: PyIndex,
        ) -> Result<Value> {
            st.ensure_writable()?;
            let d = dest.expect_i64(FN_NAME, "dest")?;
            let s = src.expect_i64(FN_NAME, "src")?;
            let c = count.expect_i64(FN_NAME, "count")?;
            if d < 0 || s < 0 || c < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "source, destination, or count out of range",
                ));
            }
            let (du, su, cu) = (d as usize, s as usize, c as usize);
            {
                let mut buf = st.data.borrow_mut();
                let len = buf.len();
                if du.checked_add(cu).is_none_or(|e| e > len)
                    || su.checked_add(cu).is_none_or(|e| e > len)
                {
                    return Err(PyError::named(
                        "ValueError",
                        "source, destination, or count out of range",
                    ));
                }
                buf.copy_within(su..su + cu, du);
            }
            st.sync_range(du, du + cu)?;
            Ok(Value::none())
        }

        fn flush(
            #[state] st: &MmapState,
            #[positional_only]
            #[default(0_i64.into())]
            offset: PyIndex,
            #[positional_only]
            #[default(None)]
            size: Option<PyIndex>,
        ) -> Result<Value> {
            st.ensure_open()?;
            let len = st.data.borrow().len();
            let off_i64 = offset.expect_i64(FN_NAME, "offset")?;
            if off_i64 < 0 || (off_i64 as usize) > len {
                return Err(PyError::named("ValueError", "flush values out of range"));
            }
            let off = off_i64 as usize;
            let sz = match size {
                None => len - off,
                Some(i) => {
                    let s = i.expect_i64(FN_NAME, "size")?;
                    if s < 0 || off.checked_add(s as usize).is_none_or(|e| e > len) {
                        return Err(PyError::named("ValueError", "flush values out of range"));
                    }
                    s as usize
                }
            };
            st.sync_range(off, off + sz)?;
            if st.access.is_writeback()
                && let Some(ref file) = st.file
            {
                file.sync_data()
                    .map_err(|e| PyError::from_io_error(&e, None))?;
            }
            Ok(Value::none())
        }

        fn resize(
            #[state_mut] st: &mut MmapState,
            #[positional_only] newsize: PyIndex,
        ) -> Result<Value> {
            st.ensure_writable()?;
            if st.exports.get() > 0 {
                return Err(PyError::named(
                    "BufferError",
                    "cannot resize memory view that has exported buffers",
                ));
            }
            let ns_i64 = newsize.expect_i64(FN_NAME, "newsize")?;
            if ns_i64 <= 0 {
                return Err(PyError::named("ValueError", "new size out of range"));
            }
            let ns = ns_i64 as usize;
            if st.access.is_writeback()
                && let Some(ref file) = st.file
            {
                let old_len = st.data.borrow().len();
                st.sync_range(0, old_len)?;
                file.set_len(st.offset + ns as u64)
                    .map_err(|e| PyError::from_io_error(&e, None))?;
            }
            st.data.borrow_mut().resize(ns, 0);
            Ok(Value::none())
        }

        #[cfg(unix)]
        fn madvise(
            #[state] st: &MmapState,
            #[positional_only] option: PyIndex,
            #[positional_only]
            #[default(0_i64.into())]
            start: PyIndex,
            #[positional_only]
            #[default(None)]
            length: Option<PyIndex>,
        ) -> Result<Value> {
            st.ensure_open()?;
            let _ = option.expect_i64(FN_NAME, "option")?;
            let len = st.data.borrow().len();
            let s = start.expect_i64(FN_NAME, "start")?;
            if s < 0 || (s as usize) > len {
                return Err(PyError::named("ValueError", "madvise start out of bounds"));
            }
            if let Some(l_idx) = length {
                let l = l_idx.expect_i64(FN_NAME, "length")?;
                if l < 0 {
                    return Err(PyError::named(
                        "ValueError",
                        "madvise length invalid",
                    ));
                }
            }
            Ok(Value::none())
        }

        fn __len__(#[state] st: &MmapState) -> Result<Value> {
            st.ensure_open()?;
            Ok(Value::int(st.data.borrow().len() as i64))
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            with_state(self_val.0, |st| {
                st.ensure_open()?;
                let buf = st.data.borrow();
                let len = buf.len();
                if let Some((lo, hi, stp)) = pyrust_builtins::slice::slice_fields(&key.0) {
                    let (start, stop, step) =
                        pyrust_builtins::slice::compute_indices(len as i64, &lo, &hi, &stp)?;
                    if step == 1 {
                        let s = start as usize;
                        let e = (stop as usize).max(s);
                        return Ok(Value::bytes(buf[s..e].to_vec()));
                    }
                    let indices = collect_slice_indices(start, stop, step);
                    let mut out = Vec::with_capacity(indices.len());
                    for idx in indices {
                        out.push(buf[idx]);
                    }
                    return Ok(Value::bytes(out));
                }
                let raw_idx = _interp.call_index_protocol(&key.0, "mmap indices")?;
                let idx_i64 = raw_idx.as_int().ok_or_else(|| {
                    PyError::named("IndexError", "mmap index out of range")
                })?;
                let norm = if idx_i64 < 0 {
                    idx_i64 + len as i64
                } else {
                    idx_i64
                };
                if norm < 0 || norm >= len as i64 {
                    return Err(PyError::named("IndexError", "mmap index out of range"));
                }
                Ok(Value::int(buf[norm as usize] as i64))
            })
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
            #[positional_only] value: PyValue,
        ) -> Result<Value> {
            let is_slice = pyrust_builtins::slice::slice_fields(&key.0);
            if let Some((lo, hi, stp)) = is_slice {
                let rhs = crate::interpreter::extract_buffer_bytes(_interp, &value.0)?;
                return with_state_mut(self_val.0, |st| {
                    st.ensure_writable()?;
                    let len = st.data.borrow().len();
                    let (start, stop, step) =
                        pyrust_builtins::slice::compute_indices(len as i64, &lo, &hi, &stp)?;
                    if step == 1 {
                        let s = start as usize;
                        let e = (stop as usize).max(s);
                        if e - s != rhs.len() {
                            return Err(PyError::named(
                                "IndexError",
                                "mmap slice assignment is wrong size",
                            ));
                        }
                        st.data.borrow_mut()[s..e].copy_from_slice(&rhs);
                        st.sync_range(s, e)?;
                    } else {
                        let indices = collect_slice_indices(start, stop, step);
                        if indices.len() != rhs.len() {
                            return Err(PyError::named(
                                "ValueError",
                                format!(
                                    "attempt to assign bytes of size {} to extended slice of size {}",
                                    rhs.len(),
                                    indices.len()
                                ),
                            ));
                        }
                        let mut min_i = usize::MAX;
                        let mut max_i = 0usize;
                        {
                            let mut buf = st.data.borrow_mut();
                            for (&idx, &b) in indices.iter().zip(rhs.iter()) {
                                buf[idx] = b;
                                min_i = min_i.min(idx);
                                max_i = max_i.max(idx + 1);
                            }
                        }
                        if min_i < max_i {
                            st.sync_range(min_i, max_i)?;
                        }
                    }
                    Ok(Value::none())
                });
            }

            let raw_idx = _interp.call_index_protocol(&key.0, "mmap indices")?;
            let idx_i64 = raw_idx.as_int().ok_or_else(|| {
                PyError::named("IndexError", "mmap index out of range")
            })?;
            let byte_val = match value.0.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "an integer is required",
                    ));
                }
            };
            if !(0..=255).contains(&byte_val) {
                return Err(PyError::named(
                    "ValueError",
                    "mmap byte value must be in range(0, 256)",
                ));
            }
            with_state_mut(self_val.0, |st| {
                st.ensure_writable()?;
                let len = st.data.borrow().len();
                let norm = if idx_i64 < 0 {
                    idx_i64 + len as i64
                } else {
                    idx_i64
                };
                if norm < 0 || norm >= len as i64 {
                    return Err(PyError::named("IndexError", "mmap index out of range"));
                }
                let u = norm as usize;
                st.data.borrow_mut()[u] = byte_val as u8;
                st.sync_range(u, u + 1)?;
                Ok(Value::none())
            })
        }

        fn __iter__(#[state] st: &MmapState) -> Result<Value> {
            st.ensure_open()?;
            let items: Vec<Value> = st
                .data
                .borrow()
                .iter()
                .map(|&b| Value::bytes(vec![b]))
                .collect();
            Ok(Value::generator(Box::new(NativeIterFrame::generator(items))))
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state(self_val.0, |st| st.ensure_open())?;
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            with_state_mut(self_val.0, |st| st.close_internal())?;
            Ok(Value::none())
        }

        /// PEP 688 buffer protocol exporter: exposes live zero-copy `memoryview`.
        fn __buffer__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] flags: PyIndex,
        ) -> Result<Value> {
            let fl = flags.expect_i64(FN_NAME, "flags")?;
            with_state(self_val.0, |st| {
                st.ensure_open()?;
                if (fl & BUF_WRITABLE) != 0 && st.access.is_readonly() {
                    return Err(PyError::named(
                        "BufferError",
                        "Object is not writable.",
                    ));
                }
                pyrust_builtins::memoryview::from_mutable_buffer(
                    Rc::clone(&st.data),
                    Some(Rc::clone(&st.exports)),
                    self_val.0.clone(),
                    "B",
                    st.access.is_readonly(),
                )
            })
        }

        fn __repr__(#[state] st: &MmapState) -> Result<Value> {
            if st.closed {
                Ok(Value::string("<mmap.mmap closed=True>"))
            } else {
                Ok(Value::string(format!(
                    "<mmap.mmap closed=False, access={}, length={}, pos={}, offset={}>",
                    st.access.repr_name(),
                    st.data.borrow().len(),
                    st.pos,
                    st.offset
                )))
            }
        }
    }
}
