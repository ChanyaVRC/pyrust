//! Tree removal and moving of CPython 3.12.3 `Lib/shutil.py`: `rmtree` (with
//! `_rmtree_islink` / `_rmtree_unsafe`, and the fd-based `_rmtree_safe_fd`
//! on Unix), `_basename`, `move`, `_destinsrc` and `_is_immutable`.
//!
//! On Unix `rmtree` is the source's symlink-attack resistant variant
//! (`rmtree.avoids_symlink_attacks` is true): directories are opened with
//! `openat` and their entries removed relative to the open descriptor, so a
//! directory swapped for a symlink mid-walk is never followed.  The runtime's
//! `os` has no `dir_fd` support, so these steps call the C library directly;
//! the `func` handed to `onexc` is still the `os` function the source names
//! (`os.lstat`, `os.open`, `os.scandir`, `os.unlink`, `os.rmdir`, `os.close`
//! or `os.path.islink`).

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

use super::copy;
use super::support::{
    OnExc, S_IFLNK, audit, binary, call, contains, dep, global, hasattr, method, method_kw,
    os_attr, os_call, path_attr, path_call, path_test, percent, raise_builtin, raise_global,
    s_isfmt, sig, symlink_rmtree_error, truthy, try_os,
};

const SIG_RMTREE: Sig = sig(
    "rmtree",
    &["path", "ignore_errors", "onerror"],
    2,
    &["onexc", "dir_fd"],
);
const SIG_RMTREE_ISLINK: Sig = sig("_rmtree_islink", &["path"], 0, &[]);
const SIG_RMTREE_UNSAFE: Sig = sig("_rmtree_unsafe", &["path", "onexc"], 0, &[]);
const SIG_BASENAME: Sig = sig("_basename", &["path"], 0, &[]);
const SIG_MOVE: Sig = sig("move", &["src", "dst", "copy_function"], 1, &[]);
const SIG_DESTINSRC: Sig = sig("_destinsrc", &["src", "dst"], 0, &[]);
const SIG_IS_IMMUTABLE: Sig = sig("_is_immutable", &["src"], 0, &[]);

// ---------------------------------------------------------------------------
// rmtree
// ---------------------------------------------------------------------------

/// `rmtree(path, ignore_errors=False, onerror=None, *, onexc=None,
/// dir_fd=None)`.
pub(super) fn rmtree_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_RMTREE, args)?;
    let path = b.take_or_none(0);
    let ignore_errors = b.take_or(1, || Value::bool_(false));
    let onerror = b.take_or_none(2);
    let onexc = b.take_or_none(3);
    let dir_fd = b.take_or_none(4);
    let hook = if truthy(interp, &ignore_errors)? {
        OnExc::Ignore
    } else if onerror.is_none() && onexc.is_none() {
        OnExc::Raise
    } else if onexc.is_none() {
        // delegate to onerror
        OnExc::OnError(onerror)
    } else {
        OnExc::Call(onexc)
    };
    rmtree(interp, path, dir_fd, &hook)?;
    Ok(Value::none())
}

/// `rmtree(src)` as `move` calls it.
pub(super) fn rmtree_default(interp: &mut Interpreter, path: &Value) -> Result<()> {
    rmtree(interp, path.clone(), Value::none(), &OnExc::Raise)
}

#[cfg(not(unix))]
fn rmtree(interp: &mut Interpreter, path: Value, dir_fd: Value, hook: &OnExc) -> Result<()> {
    audit(interp, "shutil.rmtree", &[path.clone(), dir_fd.clone()])?;
    if !dir_fd.is_none() {
        return Err(raise_builtin(
            interp,
            "NotImplementedError",
            &[Value::string("dir_fd unavailable on this platform")],
        ));
    }
    let checked = try_os(interp, |interp| {
        if rmtree_islink(interp, &path)? {
            // symlinks to directories are forbidden, see bug #1669
            return Err(symlink_rmtree_error(interp));
        }
        Ok(())
    })?;
    if let Err(caught) = checked {
        let func = path_attr(interp, "islink")?;
        // can't continue even if onexc hook returns
        return hook.handle(interp, func, path, caught);
    }
    rmtree_unsafe(interp, &path, hook)
}

#[cfg(unix)]
fn rmtree(interp: &mut Interpreter, path: Value, dir_fd: Value, hook: &OnExc) -> Result<()> {
    audit(interp, "shutil.rmtree", &[path.clone(), dir_fd.clone()])?;
    safe_fd::rmtree(interp, path, dir_fd, hook)
}

/// `_rmtree_islink(path)`: a symlink, or (on Windows) a directory junction.
pub(super) fn rmtree_islink(interp: &mut Interpreter, path: &Value) -> Result<bool> {
    let Ok(st) = try_os(interp, |interp| {
        os_call(interp, "lstat", std::slice::from_ref(path))
    })?
    else {
        return Ok(false);
    };
    let mode = interp.get_attr(&st, "st_mode")?;
    if s_isfmt(interp, &mode, S_IFLNK)? {
        return Ok(true);
    }
    is_junction(interp, path, &st)
}

/// `st.st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT and
/// st.st_reparse_tag == IO_REPARSE_TAG_MOUNT_POINT`.
#[cfg(windows)]
fn is_junction(interp: &mut Interpreter, path: &Value, st: &Value) -> Result<bool> {
    const FILE_ATTRIBUTE_REPARSE_POINT: i64 = 0x400;
    const IO_REPARSE_TAG_MOUNT_POINT: i64 = 0xA000_0003;
    if let (Some(attributes), Some(tag)) = (
        super::support::getattr_opt(interp, st, "st_file_attributes")?,
        super::support::getattr_opt(interp, st, "st_reparse_tag")?,
    ) {
        return Ok(attributes
            .as_int()
            .is_some_and(|a| a & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            && tag.as_int() == Some(IO_REPARSE_TAG_MOUNT_POINT));
    }
    // The runtime's `stat_result` has no reparse fields: ask the file system.
    let fspath = os_call(interp, "fspath", std::slice::from_ref(path))?;
    let fspath = os_call(interp, "fsdecode", &[fspath])?;
    let Some(text) = fspath.as_str() else {
        return Ok(false);
    };
    Ok(std::fs::symlink_metadata(text).is_ok_and(|meta| meta.file_type().is_symlink()))
}

#[cfg(not(windows))]
fn is_junction(_interp: &mut Interpreter, _path: &Value, _st: &Value) -> Result<bool> {
    Ok(false)
}

pub(super) fn rmtree_islink_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_RMTREE_ISLINK, args)?;
    let path = b.take_or_none(0);
    Ok(Value::bool_(rmtree_islink(interp, &path)?))
}

/// `_rmtree_unsafe(path, onexc)` (the version vulnerable to race conditions).
fn rmtree_unsafe(interp: &mut Interpreter, path: &Value, hook: &OnExc) -> Result<()> {
    let entries = match try_os(interp, |interp| copy::scandir_list(interp, path))? {
        Ok(entries) => entries,
        Err(caught) => {
            let func = os_attr(interp, "scandir")?;
            hook.handle(interp, func, path.clone(), caught)?;
            Vec::new()
        }
    };
    for entry in entries {
        let fullname = interp.get_attr(&entry, "path")?;
        let is_dir = try_os(interp, |interp| {
            let result = method_kw(
                interp,
                &entry,
                "is_dir",
                &[],
                &[("follow_symlinks", Value::bool_(false))],
            )?;
            truthy(interp, &result)
        })?
        .unwrap_or(false);

        let is_junction = if is_dir {
            let result = method(interp, &entry, "is_junction", &[])?;
            truthy(interp, &result)?
        } else {
            false
        };
        if is_dir && !is_junction {
            let checked = try_os(interp, |interp| {
                let result = method(interp, &entry, "is_symlink", &[])?;
                if truthy(interp, &result)? {
                    // This can only happen if someone replaces a directory
                    // with a symlink after the call to os.scandir or
                    // entry.is_dir above.
                    return Err(symlink_rmtree_error(interp));
                }
                Ok(())
            })?;
            if let Err(caught) = checked {
                let func = path_attr(interp, "islink")?;
                hook.handle(interp, func, fullname, caught)?;
                continue;
            }
            rmtree_unsafe(interp, &fullname, hook)?;
        } else if let Err(caught) = try_os(interp, |interp| {
            os_call(interp, "unlink", std::slice::from_ref(&fullname))
        })? {
            let func = os_attr(interp, "unlink")?;
            hook.handle(interp, func, fullname, caught)?;
        }
    }
    if let Err(caught) = try_os(interp, |interp| {
        os_call(interp, "rmdir", std::slice::from_ref(path))
    })? {
        let func = os_attr(interp, "rmdir")?;
        hook.handle(interp, func, path.clone(), caught)?;
    }
    Ok(())
}

pub(super) fn rmtree_unsafe_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_RMTREE_UNSAFE, args)?;
    let path = b.take_or_none(0);
    let onexc = b.take_or_none(1);
    rmtree_unsafe(interp, &path, &OnExc::Call(onexc))?;
    Ok(Value::none())
}

#[cfg(unix)]
mod safe_fd {
    //! `_rmtree_safe_fd` and the fd-based half of `rmtree`.

    use std::ffi::{CStr, CString};
    use std::io;

    use libc::c_int;

    use crate::error::{PyError, Result};
    use crate::interpreter::Interpreter;
    use crate::value::Value;

    use super::super::support::{
        OnExc, is_bytes, os_attr, os_call, path_attr, symlink_rmtree_error,
    };

    fn lstat_at(dirfd: c_int, name: &CStr) -> io::Result<libc::stat> {
        // SAFETY: `st` is plain old data that `fstatat` fills in, and `name`
        // is NUL-terminated.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::fstatat(dirfd, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) };
        if rc == 0 {
            Ok(st)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn fstat(fd: c_int) -> io::Result<libc::stat> {
        // SAFETY: `st` is plain old data that `fstat` fills in.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::fstat(fd, &mut st) };
        if rc == 0 {
            Ok(st)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// `os.open(name, os.O_RDONLY | os.O_NONBLOCK, dir_fd=dirfd)` (descriptors
    /// `os.open` returns are non-inheritable).
    fn open_at(dirfd: c_int, name: &CStr) -> io::Result<c_int> {
        // SAFETY: `name` is NUL-terminated.
        let fd = unsafe {
            libc::openat(
                dirfd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd >= 0 {
            Ok(fd)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn close(fd: c_int) -> io::Result<()> {
        // SAFETY: `fd` is a descriptor this module opened and closes once.
        if unsafe { libc::close(fd) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// `os.unlink(name, dir_fd=dirfd)` / `os.rmdir(name, dir_fd=dirfd)`.
    fn unlink_at(dirfd: c_int, name: &CStr, flags: c_int) -> io::Result<()> {
        // SAFETY: `name` is NUL-terminated.
        if unsafe { libc::unlinkat(dirfd, name.as_ptr(), flags) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// `list(os.scandir(fd))`: the names (without `.` and `..`) and
    /// `d_type`s of the directory open as `fd`, which stays open.
    fn list_dir(fd: c_int) -> io::Result<Vec<(CString, u8)>> {
        // SAFETY: `dup` of a descriptor we own; `fdopendir` takes ownership of
        // the duplicate and `closedir` releases it.
        let dup = unsafe { libc::dup(fd) };
        if dup < 0 {
            return Err(io::Error::last_os_error());
        }
        let dir = unsafe { libc::fdopendir(dup) };
        if dir.is_null() {
            let err = io::Error::last_os_error();
            let _ = close(dup);
            return Err(err);
        }
        let mut entries = Vec::new();
        // SAFETY: `dir` is a valid directory stream until `closedir`; every
        // `dirent` returned by `readdir` is valid until the next call, and
        // its `d_name` is NUL-terminated.
        unsafe {
            libc::rewinddir(dir);
            loop {
                let entry = libc::readdir(dir);
                if entry.is_null() {
                    break;
                }
                let name = CStr::from_ptr((*entry).d_name.as_ptr());
                let bytes = name.to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                entries.push((name.to_owned(), (*entry).d_type));
            }
            libc::closedir(dir);
        }
        Ok(entries)
    }

    fn same_stat(a: &libc::stat, b: &libc::stat) -> bool {
        a.st_ino == b.st_ino && a.st_dev == b.st_dev
    }

    fn is_dir_mode(st: &libc::stat) -> bool {
        (st.st_mode & libc::S_IFMT) == libc::S_IFDIR
    }

    fn os_error(err: &io::Error, filename: Option<&str>) -> PyError {
        PyError::from_io_error(err, filename)
    }

    /// The `str` form of a file name from the directory stream.
    fn decode_name(interp: &mut Interpreter, name: &CStr) -> Result<String> {
        let decoded = os_call(
            interp,
            "fsdecode",
            &[Value::bytes(name.to_bytes().to_vec())],
        )?;
        Ok(decoded
            .as_str()
            .map_or_else(|| name.to_string_lossy().into_owned(), str::to_string))
    }

    fn join(path: &str, name: &str) -> String {
        if path.is_empty() || path.ends_with('/') {
            format!("{path}{name}")
        } else {
            format!("{path}/{name}")
        }
    }

    /// `rmtree` when `_use_fd_functions` holds.
    pub(super) fn rmtree(
        interp: &mut Interpreter,
        path: Value,
        dir_fd: Value,
        hook: &OnExc,
    ) -> Result<()> {
        // While the unsafe rmtree works fine on bytes, the fd based does not.
        let path = if is_bytes(interp, &path)? {
            os_call(interp, "fsdecode", &[path])?
        } else {
            path
        };
        // Note: To guard against symlink races, we use the standard
        // lstat()/open()/fstat() trick.
        let (base, c_path, path_text) = match resolve(interp, &path, &dir_fd) {
            Ok(resolved) => resolved,
            Err(err) => {
                let func = os_attr(interp, "lstat")?;
                return hook.handle_error(interp, func, path, err);
            }
        };
        let orig_st = match lstat_at(base, &c_path) {
            Ok(st) => st,
            Err(err) => {
                let func = os_attr(interp, "lstat")?;
                return hook.handle_error(interp, func, path, os_error(&err, Some(&path_text)));
            }
        };
        let fd = match open_at(base, &c_path) {
            Ok(fd) => fd,
            Err(err) => {
                let func = os_attr(interp, "open")?;
                return hook.handle_error(interp, func, path, os_error(&err, Some(&path_text)));
            }
        };
        let mut fd_closed = false;
        let body = remove_open_dir(
            interp,
            DirStep {
                parent: base,
                name: &c_path,
                fd,
                orig_st: &orig_st,
                path: &path,
                path_text: &path_text,
            },
            &mut fd_closed,
            hook,
        );
        if !fd_closed && let Err(err) = close(fd) {
            let func = os_attr(interp, "close")?;
            hook.handle_error(interp, func, path, os_error(&err, None))?;
        }
        body
    }

    /// The path (as a C string relative to `dir_fd`, or the cwd) and its text.
    fn resolve(
        interp: &mut Interpreter,
        path: &Value,
        dir_fd: &Value,
    ) -> Result<(c_int, CString, String)> {
        let base = if dir_fd.is_none() {
            libc::AT_FDCWD
        } else {
            let fd = interp.try_value_to_index(dir_fd)?.ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    format!(
                        "argument should be integer or None, not {}",
                        crate::interpreter::value_type_name_str(dir_fd)
                    ),
                )
            })?;
            fd.as_int()
                .and_then(|fd| c_int::try_from(fd).ok())
                .ok_or_else(|| PyError::named("OverflowError", "fd is out of range".to_string()))?
        };
        let fspath = os_call(interp, "fspath", std::slice::from_ref(path)).map_err(|_| {
            PyError::named(
                "TypeError",
                format!(
                    "lstat: path should be string, bytes or os.PathLike, not {}",
                    crate::interpreter::value_type_name_str(path)
                ),
            )
        })?;
        let text = os_call(interp, "fsdecode", std::slice::from_ref(&fspath))?;
        let text = text.as_str().unwrap_or_default().to_string();
        let encoded = os_call(interp, "fsencode", &[fspath])?;
        let bytes = match encoded.kind() {
            crate::value::ValueKind::Bytes(bytes) => bytes.to_vec(),
            _ => text.clone().into_bytes(),
        };
        let c_path = CString::new(bytes).map_err(|_| {
            PyError::named(
                "ValueError",
                "lstat: embedded null character in path".to_string(),
            )
        })?;
        Ok((base, c_path, text))
    }

    /// One directory the walk has opened: `fd`, opened as `name` relative to
    /// `parent`, with the `lstat` taken before opening it.
    struct DirStep<'a> {
        parent: c_int,
        name: &'a CStr,
        fd: c_int,
        orig_st: &'a libc::stat,
        /// The path handed to `onexc` (the caller's object at the top level).
        path: &'a Value,
        path_text: &'a str,
    }

    /// The `try` body after `os.open`: check `samestat`, empty the directory,
    /// close it and remove it.  `fd_closed` tracks the source's flag so the
    /// caller's `finally` closes the descriptor exactly once.
    fn remove_open_dir(
        interp: &mut Interpreter,
        step: DirStep<'_>,
        fd_closed: &mut bool,
        hook: &OnExc,
    ) -> Result<()> {
        let current = fstat(step.fd).map_err(|err| os_error(&err, None))?;
        if !same_stat(step.orig_st, &current) {
            // symlinks to directories are forbidden, see bug #1669
            let func = path_attr(interp, "islink")?;
            let err = symlink_rmtree_error(interp);
            return hook.handle_error(interp, func, step.path.clone(), err);
        }
        rmtree_safe_fd(interp, step.fd, step.path, step.path_text, hook)?;
        if let Err(err) = close(step.fd) {
            // close() should not be retried after an error.
            *fd_closed = true;
            let func = os_attr(interp, "close")?;
            hook.handle_error(interp, func, step.path.clone(), os_error(&err, None))?;
        }
        *fd_closed = true;
        if let Err(err) = unlink_at(step.parent, step.name, libc::AT_REMOVEDIR) {
            let func = os_attr(interp, "rmdir")?;
            let filename = step.name.to_string_lossy();
            let filename = if step.parent == libc::AT_FDCWD {
                step.path_text.to_string()
            } else {
                filename.into_owned()
            };
            hook.handle_error(
                interp,
                func,
                step.path.clone(),
                os_error(&err, Some(&filename)),
            )?;
        }
        Ok(())
    }

    /// `_rmtree_safe_fd(topfd, path, onexc)`.
    fn rmtree_safe_fd(
        interp: &mut Interpreter,
        topfd: c_int,
        path: &Value,
        path_text: &str,
        hook: &OnExc,
    ) -> Result<()> {
        let entries = match list_dir(topfd) {
            Ok(entries) => entries,
            Err(err) => {
                let func = os_attr(interp, "scandir")?;
                return hook.handle_error(
                    interp,
                    func,
                    path.clone(),
                    os_error(&err, Some(path_text)),
                );
            }
        };
        for (name, d_type) in entries {
            let name_text = decode_name(interp, &name)?;
            let fullname_text = join(path_text, &name_text);
            let fullname = Value::string(fullname_text.as_str());

            // is_dir = entry.is_dir(follow_symlinks=False), then
            // orig_st = entry.stat(follow_symlinks=False)
            let mut cached = None;
            let is_dir = match d_type {
                libc::DT_DIR => true,
                libc::DT_UNKNOWN => match lstat_at(topfd, &name) {
                    Ok(st) => {
                        let dir = is_dir_mode(&st);
                        cached = Some(st);
                        dir
                    }
                    Err(_) => false,
                },
                _ => false,
            };
            let orig_st = if is_dir {
                match cached.map_or_else(|| lstat_at(topfd, &name), Ok) {
                    Ok(st) => Some(st),
                    Err(err) => {
                        let func = os_attr(interp, "lstat")?;
                        hook.handle_error(
                            interp,
                            func,
                            fullname,
                            os_error(&err, Some(&name_text)),
                        )?;
                        continue;
                    }
                }
            } else {
                None
            };

            let Some(orig_st) = orig_st.filter(is_dir_mode) else {
                if let Err(err) = unlink_at(topfd, &name, 0) {
                    let func = os_attr(interp, "unlink")?;
                    hook.handle_error(interp, func, fullname, os_error(&err, Some(&name_text)))?;
                }
                continue;
            };
            let dirfd = match open_at(topfd, &name) {
                Ok(dirfd) => dirfd,
                Err(err) => {
                    let func = os_attr(interp, "open")?;
                    hook.handle_error(interp, func, fullname, os_error(&err, Some(&name_text)))?;
                    continue;
                }
            };
            let mut dirfd_closed = false;
            let body = remove_open_dir(
                interp,
                DirStep {
                    parent: topfd,
                    name: &name,
                    fd: dirfd,
                    orig_st: &orig_st,
                    path: &fullname,
                    path_text: &fullname_text,
                },
                &mut dirfd_closed,
                hook,
            );
            if !dirfd_closed && let Err(err) = close(dirfd) {
                let func = os_attr(interp, "close")?;
                hook.handle_error(interp, func, fullname.clone(), os_error(&err, None))?;
            }
            body?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// move
// ---------------------------------------------------------------------------

/// `_basename(path)`: `os.path.basename` after stripping trailing separators.
fn basename(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let path = os_call(interp, "fspath", std::slice::from_ref(path))?;
    let sep = path_attr(interp, "sep")?;
    let altsep = path_attr(interp, "altsep")?;
    // sep = os.path.sep + (os.path.altsep or '')
    let altsep = if truthy(interp, &altsep)? {
        altsep
    } else {
        Value::string("")
    };
    let seps = binary(interp, &sep, BinaryOp::Add, &altsep)?;
    let stripped = method(interp, &path, "rstrip", &[seps])?;
    path_call(interp, "basename", &[stripped])
}

pub(super) fn basename_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_BASENAME, args)?;
    let path = b.take_or_none(0);
    basename(interp, &path)
}

/// `move(src, dst, copy_function=copy2)`.
pub(super) fn move_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_MOVE, args)?;
    let src = b.take_or_none(0);
    let dst = b.take_or_none(1);
    let copy_function = b.take_or(2, || {
        global("copy2").unwrap_or_else(|| Value::builtin_function("shutil.copy2"))
    });
    move_impl(interp, &src, &dst, copy_function)
}

fn move_impl(
    interp: &mut Interpreter,
    src: &Value,
    dst: &Value,
    copy_function: Value,
) -> Result<Value> {
    audit(interp, "shutil.move", &[src.clone(), dst.clone()])?;
    let mut real_dst = dst.clone();
    if path_test(interp, "isdir", std::slice::from_ref(dst))? {
        if copy::samefile(interp, src, dst)?
            && !path_test(interp, "islink", std::slice::from_ref(src))?
        {
            // We might be on a case insensitive filesystem, perform the
            // rename anyway.
            os_call(interp, "rename", &[src.clone(), dst.clone()])?;
            return Ok(Value::none());
        }
        // Using _basename instead of os.path.basename is important, as we
        // must ignore any trailing slash to avoid the basename returning ''
        let name = basename(interp, src)?;
        real_dst = path_call(interp, "join", &[dst.clone(), name])?;
        if path_test(interp, "exists", std::slice::from_ref(&real_dst))? {
            let msg = percent(
                interp,
                "Destination path '%s' already exists",
                vec![real_dst.clone()],
            )?;
            return Err(raise_global(interp, "Error", &[msg]));
        }
    }
    let renamed = try_os(interp, |interp| {
        os_call(interp, "rename", &[src.clone(), real_dst.clone()])
    })?;
    if renamed.is_ok() {
        return Ok(real_dst);
    }
    if path_test(interp, "islink", std::slice::from_ref(src))? {
        let linkto = os_call(interp, "readlink", std::slice::from_ref(src))?;
        os_call(interp, "symlink", &[linkto, real_dst.clone()])?;
        os_call(interp, "unlink", std::slice::from_ref(src))?;
    } else if path_test(interp, "isdir", std::slice::from_ref(src))? {
        if destinsrc(interp, src, dst)? {
            let msg = percent(
                interp,
                "Cannot move a directory '%s' into itself '%s'.",
                vec![src.clone(), dst.clone()],
            )?;
            return Err(raise_global(interp, "Error", &[msg]));
        }
        if is_immutable(interp, src)? || lacks_write_permission_on_darwin(interp, src)? {
            let msg = percent(
                interp,
                "Cannot move the non-empty directory '%s': Lacking write permission to '%s'.",
                vec![src.clone(), src.clone()],
            )?;
            return Err(raise_builtin(interp, "PermissionError", &[msg]));
        }
        copy::copytree_for_move(interp, src, &real_dst, copy_function)?;
        rmtree_default(interp, src)?;
    } else {
        call(interp, copy_function, &[src.clone(), real_dst.clone()])?;
        os_call(interp, "unlink", std::slice::from_ref(src))?;
    }
    Ok(real_dst)
}

/// `not os.access(src, os.W_OK) and os.listdir(src) and sys.platform ==
/// 'darwin'`.
fn lacks_write_permission_on_darwin(interp: &mut Interpreter, src: &Value) -> Result<bool> {
    let w_ok = os_attr(interp, "W_OK")?;
    let writable = os_call(interp, "access", &[src.clone(), w_ok])?;
    if truthy(interp, &writable)? {
        return Ok(false);
    }
    let listed = os_call(interp, "listdir", std::slice::from_ref(src))?;
    if !truthy(interp, &listed)? {
        return Ok(false);
    }
    let sys = dep(interp, "sys")?;
    let platform = interp.get_attr(&sys, "platform")?;
    Ok(platform.as_str() == Some("darwin"))
}

/// `_destinsrc(src, dst)`.
fn destinsrc(interp: &mut Interpreter, src: &Value, dst: &Value) -> Result<bool> {
    let sep = path_attr(interp, "sep")?;
    let mut paths = Vec::with_capacity(2);
    for path in [src, dst] {
        let mut absolute = path_call(interp, "abspath", std::slice::from_ref(path))?;
        let ends = method(interp, &absolute, "endswith", std::slice::from_ref(&sep))?;
        if !truthy(interp, &ends)? {
            absolute = binary(interp, &absolute, BinaryOp::Add, &sep)?;
        }
        paths.push(absolute);
    }
    let starts = method(interp, &paths[1], "startswith", &[paths[0].clone()])?;
    truthy(interp, &starts)
}

pub(super) fn destinsrc_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_DESTINSRC, args)?;
    let src = b.take_or_none(0);
    let dst = b.take_or_none(1);
    Ok(Value::bool_(destinsrc(interp, &src, &dst)?))
}

/// `_is_immutable(src)`.
fn is_immutable(interp: &mut Interpreter, src: &Value) -> Result<bool> {
    let st = copy::stat_of(interp, src)?;
    if !hasattr(interp, &st, "st_flags")? {
        return Ok(false);
    }
    let flags = interp.get_attr(&st, "st_flags")?;
    let stat = dep(interp, "stat")?;
    let states = vec![
        interp.get_attr(&stat, "UF_IMMUTABLE")?,
        interp.get_attr(&stat, "SF_IMMUTABLE")?,
    ];
    contains(interp, &Value::list(states), &flags)
}

pub(super) fn is_immutable_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_IS_IMMUTABLE, args)?;
    let src = b.take_or_none(0);
    Ok(Value::bool_(is_immutable(interp, &src)?))
}
