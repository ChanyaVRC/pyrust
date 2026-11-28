//! System queries of CPython 3.12.3 `Lib/shutil.py`: `disk_usage`,
//! `_get_gid` / `_get_uid`, `chown`, `get_terminal_size`, `_access_check`,
//! `_win_path_needs_curdir` and `which`.

use std::collections::HashSet;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyKey, Value};

use super::support::{
    audit, binary, builtin, builtin_exc, call_kw, collect, dep, getitem, global, is_bytes,
    isinstance_builtin, method, os_attr, os_call, path_call, path_test, raise_builtin, repr_of,
    sig, truthy, try_except,
};

const SIG_DISK_USAGE: Sig = sig("disk_usage", &["path"], 0, &[]);
const SIG_GET_GID: Sig = sig("_get_gid", &["name"], 0, &[]);
const SIG_GET_UID: Sig = sig("_get_uid", &["name"], 0, &[]);
const SIG_CHOWN: Sig = sig("chown", &["path", "user", "group"], 2, &[]);
const SIG_TERMINAL: Sig = sig("get_terminal_size", &["fallback"], 1, &[]);
const SIG_ACCESS_CHECK: Sig = sig("_access_check", &["fn", "mode"], 0, &[]);
const SIG_NEEDS_CURDIR: Sig = sig("_win_path_needs_curdir", &["cmd", "mode"], 0, &[]);
const SIG_WHICH: Sig = sig("which", &["cmd", "mode", "path"], 2, &[]);

/// `_WIN_DEFAULT_PATHEXT`: CMD defaults in Windows 10.
pub(super) const WIN_DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.WS;.MSC";

// ---------------------------------------------------------------------------
// disk_usage
// ---------------------------------------------------------------------------

/// `_ntuple_diskusage = collections.namedtuple('usage', 'total used free')`,
/// built on first use.
fn ntuple_diskusage(interp: &mut Interpreter) -> Result<Value> {
    if let Some(class) = super::support::cached_ntuple_diskusage() {
        return Ok(class);
    }
    let collections = interp.load_module("collections")?;
    let namedtuple = interp.get_attr(&collections, "namedtuple")?;
    let class = call_kw(
        interp,
        namedtuple,
        &[Value::string("usage"), Value::string("total used free")],
        &[("module", Value::string("shutil"))],
    )?;
    // The source documents the fields only on its `os.statvfs` branch.
    if cfg!(unix) {
        for (field, doc) in [
            ("total", "Total space in bytes"),
            ("used", "Used space in bytes"),
            ("free", "Free space in bytes"),
        ] {
            if let Ok(descriptor) = interp.get_attr(&class, field) {
                let _ = interp.assign_attr(descriptor, "__doc__", Value::string(doc));
            }
        }
    }
    super::support::store_ntuple_diskusage(class.clone(), collections);
    Ok(class)
}

/// `disk_usage(path)`.
pub(super) fn disk_usage_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_DISK_USAGE, args)?;
    let path = b.take_or_none(0);
    let (total, used, free) = disk_usage_values(interp, &path)?;
    let class = ntuple_diskusage(interp)?;
    super::support::call(interp, class, &[total, used, free])
}

#[cfg(unix)]
fn disk_usage_values(interp: &mut Interpreter, path: &Value) -> Result<(Value, Value, Value)> {
    let st = os_call(interp, "statvfs", std::slice::from_ref(path))?;
    let bavail = interp.get_attr(&st, "f_bavail")?;
    let frsize = interp.get_attr(&st, "f_frsize")?;
    let blocks = interp.get_attr(&st, "f_blocks")?;
    let bfree = interp.get_attr(&st, "f_bfree")?;
    let free = binary(interp, &bavail, BinaryOp::Mul, &frsize)?;
    let total = binary(interp, &blocks, BinaryOp::Mul, &frsize)?;
    let used_blocks = binary(interp, &blocks, BinaryOp::Sub, &bfree)?;
    let used = binary(interp, &used_blocks, BinaryOp::Mul, &frsize)?;
    Ok((total, used, free))
}

#[cfg(not(unix))]
fn disk_usage_values(interp: &mut Interpreter, path: &Value) -> Result<(Value, Value, Value)> {
    let nt = dep(interp, "nt")?;
    let pair = method(interp, &nt, "_getdiskusage", std::slice::from_ref(path))?;
    let pair = collect(interp, &pair)?;
    let [total, free]: [Value; 2] = pair.try_into().map_err(|_| {
        crate::error::PyError::named("ValueError", "nt._getdiskusage() returned a bad pair")
    })?;
    let used = binary(interp, &total, BinaryOp::Sub, &free)?;
    Ok((total, used, free))
}

// ---------------------------------------------------------------------------
// _get_gid / _get_uid / chown
// ---------------------------------------------------------------------------

/// `_get_gid(name)` / `_get_uid(name)`: `module.function(name)[2]`, or `None`
/// when the module is missing or the name unknown.
fn lookup_id(
    interp: &mut Interpreter,
    name: &Value,
    module: &str,
    function: &str,
) -> Result<Value> {
    if name.is_none() {
        return Ok(Value::none());
    }
    let import_error = builtin_exc("ImportError")?;
    let Ok(lookup) = try_except(interp, &[import_error], |interp| {
        let module = interp.load_module(module)?;
        interp.get_attr(&module, function)
    })?
    else {
        return Ok(Value::none());
    };
    let key_error = builtin_exc("KeyError")?;
    match try_except(interp, &[key_error], |interp| {
        super::support::call(interp, lookup, std::slice::from_ref(name))
    })? {
        Ok(result) if !result.is_none() => getitem(interp, &result, Value::int(2)),
        _ => Ok(Value::none()),
    }
}

pub(super) fn get_gid_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_GET_GID, args)?;
    let name = b.take_or_none(0);
    lookup_id(interp, &name, "grp", "getgrnam")
}

pub(super) fn get_uid_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_GET_UID, args)?;
    let name = b.take_or_none(0);
    lookup_id(interp, &name, "pwd", "getpwnam")
}

pub(super) fn get_gid(interp: &mut Interpreter, name: &Value) -> Result<Value> {
    lookup_id(interp, name, "grp", "getgrnam")
}

pub(super) fn get_uid(interp: &mut Interpreter, name: &Value) -> Result<Value> {
    lookup_id(interp, name, "pwd", "getpwnam")
}

/// `chown(path, user=None, group=None)`.
pub(super) fn chown_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_CHOWN, args)?;
    let path = b.take_or_none(0);
    let user = b.take_or_none(1);
    let group = b.take_or_none(2);
    audit(
        interp,
        "shutil.chown",
        &[path.clone(), user.clone(), group.clone()],
    )?;

    if user.is_none() && group.is_none() {
        return Err(raise_builtin(
            interp,
            "ValueError",
            &[Value::string("user and/or group must be set")],
        ));
    }

    // -1 means don't change it
    let uid = if user.is_none() {
        Value::int(-1)
    } else if isinstance_builtin(interp, &user, "str")? {
        // user can either be an int (the uid) or a string (the system username)
        let uid = get_uid(interp, &user)?;
        if uid.is_none() {
            let msg = format!("no such user: {}", repr_of(interp, &user)?);
            return Err(raise_builtin(interp, "LookupError", &[Value::string(msg)]));
        }
        uid
    } else {
        user
    };
    let gid = if group.is_none() {
        Value::int(-1)
    } else if !isinstance_builtin(interp, &group, "int")? {
        let gid = get_gid(interp, &group)?;
        if gid.is_none() {
            let msg = format!("no such group: {}", repr_of(interp, &group)?);
            return Err(raise_builtin(interp, "LookupError", &[Value::string(msg)]));
        }
        gid
    } else {
        group
    };
    os_call(interp, "chown", &[path, uid, gid])?;
    Ok(Value::none())
}

// ---------------------------------------------------------------------------
// get_terminal_size
// ---------------------------------------------------------------------------

/// `int(os.environ[name])`, or 0 on `KeyError` / `ValueError`.
fn env_dimension(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let key_error = builtin_exc("KeyError")?;
    let value_error = builtin_exc("ValueError")?;
    let parsed = try_except(interp, &[key_error, value_error], |interp| {
        let environ = os_attr(interp, "environ")?;
        let raw = getitem(interp, &environ, Value::string(name))?;
        builtin(interp, "int", &[raw])
    })?;
    Ok(parsed.unwrap_or_else(|_| Value::int(0)))
}

fn non_positive(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    let result = binary(interp, value, BinaryOp::Le, &Value::int(0))?;
    truthy(interp, &result)
}

/// `os.get_terminal_size(fd)` for the descriptor of `sys.__stdout__`: the
/// window size of the terminal, or `None` when `fd` is not one.
#[cfg(unix)]
fn query_terminal(fd: i64) -> Option<(i64, i64)> {
    let fd = libc::c_int::try_from(fd).ok()?;
    // SAFETY: `winsize` is plain old data that `TIOCGWINSZ` fills in.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
    if rc == 0 {
        Some((i64::from(ws.ws_col), i64::from(ws.ws_row)))
    } else {
        None
    }
}

#[cfg(windows)]
fn query_terminal(fd: i64) -> Option<(i64, i64)> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    let which = match fd {
        0 => STD_INPUT_HANDLE,
        1 => STD_OUTPUT_HANDLE,
        2 => STD_ERROR_HANDLE,
        _ => return None,
    };
    // SAFETY: `GetStdHandle` has no preconditions; `info` is plain old data
    // that `GetConsoleScreenBufferInfo` fills in for a valid handle.
    unsafe {
        let handle = GetStdHandle(which);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(handle, &mut info) == 0 {
            return None;
        }
        let window = info.srWindow;
        Some((
            i64::from(window.Right) - i64::from(window.Left) + 1,
            i64::from(window.Bottom) - i64::from(window.Top) + 1,
        ))
    }
}

#[cfg(not(any(unix, windows)))]
fn query_terminal(_fd: i64) -> Option<(i64, i64)> {
    None
}

/// `get_terminal_size(fallback=(80, 24))`.
pub(super) fn get_terminal_size_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_TERMINAL, args)?;
    let fallback = b.take_or(0, || Value::tuple(vec![Value::int(80), Value::int(24)]));

    // columns, lines are the working values
    let mut columns = env_dimension(interp, "COLUMNS")?;
    let mut lines = env_dimension(interp, "LINES")?;

    // only query if necessary
    let columns_unset = non_positive(interp, &columns)?;
    let lines_unset = non_positive(interp, &lines)?;
    if columns_unset || lines_unset {
        let terminal_size = os_attr(interp, "terminal_size")?;
        let attribute_error = builtin_exc("AttributeError")?;
        let value_error = builtin_exc("ValueError")?;
        let os_error = builtin_exc("OSError")?;
        let queried = try_except(
            interp,
            &[attribute_error, value_error, os_error],
            |interp| {
                let sys = dep(interp, "sys")?;
                let stdout = interp.get_attr(&sys, "__stdout__")?;
                let fd = method(interp, &stdout, "fileno", &[])?;
                Ok(fd.as_int().and_then(query_terminal))
            },
        )?;
        let size = match queried {
            Ok(Some((cols, rows))) => super::support::call(
                interp,
                terminal_size.clone(),
                &[Value::tuple(vec![Value::int(cols), Value::int(rows)])],
            )?,
            // stdout is None, closed, detached, or not a terminal, or
            // os.get_terminal_size() is unsupported
            _ => super::support::call(
                interp,
                terminal_size.clone(),
                std::slice::from_ref(&fallback),
            )?,
        };
        if columns_unset {
            let value = interp.get_attr(&size, "columns")?;
            columns = if truthy(interp, &value)? {
                value
            } else {
                getitem(interp, &fallback, Value::int(0))?
            };
        }
        if lines_unset {
            let value = interp.get_attr(&size, "lines")?;
            lines = if truthy(interp, &value)? {
                value
            } else {
                getitem(interp, &fallback, Value::int(1))?
            };
        }
    }
    let terminal_size = os_attr(interp, "terminal_size")?;
    super::support::call(interp, terminal_size, &[Value::tuple(vec![columns, lines])])
}

// ---------------------------------------------------------------------------
// which
// ---------------------------------------------------------------------------

/// `_access_check(fn, mode)`: exists, passes `os.access` and is not a
/// directory (on Windows directories pass the `os.access` check).
fn access_check(interp: &mut Interpreter, path: &Value, mode: &Value) -> Result<bool> {
    if !path_test(interp, "exists", std::slice::from_ref(path))? {
        return Ok(false);
    }
    let accessible = os_call(interp, "access", &[path.clone(), mode.clone()])?;
    if !truthy(interp, &accessible)? {
        return Ok(false);
    }
    Ok(!path_test(interp, "isdir", std::slice::from_ref(path))?)
}

pub(super) fn access_check_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_ACCESS_CHECK, args)?;
    let path = b.take_or_none(0);
    let mode = b.take_or_none(1);
    Ok(Value::bool_(access_check(interp, &path, &mode)?))
}

/// `mode & os.X_OK`.
fn wants_exec(interp: &mut Interpreter, mode: &Value) -> Result<bool> {
    let x_ok = os_attr(interp, "X_OK")?;
    let masked = binary(interp, mode, BinaryOp::BitAnd, &x_ok)?;
    truthy(interp, &masked)
}

/// `_winapi.NeedCurrentDirectoryForExePath(name)`.
#[cfg(windows)]
fn need_current_directory_for_exe_path(name: &str) -> bool {
    use windows_sys::Win32::System::Environment::NeedCurrentDirectoryForExePathW;
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string.
    unsafe { NeedCurrentDirectoryForExePathW(wide.as_ptr()) != 0 }
}

#[cfg(not(windows))]
fn need_current_directory_for_exe_path(_name: &str) -> bool {
    true
}

/// `_win_path_needs_curdir(cmd, mode)`: on Windows, whether the current
/// directory is searched before `PATH`.
fn win_path_needs_curdir(interp: &mut Interpreter, cmd: &Value, mode: &Value) -> Result<bool> {
    if !wants_exec(interp, mode)? {
        return Ok(true);
    }
    let decoded = os_call(interp, "fsdecode", std::slice::from_ref(cmd))?;
    Ok(need_current_directory_for_exe_path(
        decoded.as_str().unwrap_or_default(),
    ))
}

pub(super) fn win_path_needs_curdir_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut b = args::bind(&SIG_NEEDS_CURDIR, args)?;
    let cmd = b.take_or_none(0);
    let mode = b.take_or_none(1);
    Ok(Value::bool_(win_path_needs_curdir(interp, &cmd, &mode)?))
}

/// `os.environ.get("PATH", None)`, then `os.confstr("CS_PATH")`, then
/// `os.defpath`.
fn default_search_path(interp: &mut Interpreter) -> Result<Value> {
    let environ = os_attr(interp, "environ")?;
    let path = method(
        interp,
        &environ,
        "get",
        &[Value::string("PATH"), Value::none()],
    )?;
    if !path.is_none() {
        return Ok(path);
    }
    let attribute_error = builtin_exc("AttributeError")?;
    let value_error = builtin_exc("ValueError")?;
    match try_except(interp, &[attribute_error, value_error], |interp| {
        os_call(interp, "confstr", &[Value::string("CS_PATH")])
    })? {
        Ok(path) => Ok(path),
        // os.confstr() or CS_PATH is not available
        Err(_) => os_attr(interp, "defpath"),
    }
}

/// `which(cmd, mode=os.F_OK | os.X_OK, path=None)`.
pub(super) fn which_entry(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_WHICH, args)?;
    let cmd = b.take_or_none(0);
    let mode = b.take_or(1, || Value::int(1));
    let path = b.take_or_none(2);
    which(interp, &cmd, &mode, path)
}

fn which(interp: &mut Interpreter, cmd: &Value, mode: &Value, path: Value) -> Result<Value> {
    let use_bytes = is_bytes(interp, cmd)?;

    // If we're given a path with a directory part, look it up directly
    // rather than referring to PATH directories. This includes checking
    // relative to the current directory, e.g. ./script
    let split = path_call(interp, "split", std::slice::from_ref(cmd))?;
    let [dirname, cmd]: [Value; 2] = collect(interp, &split)?
        .try_into()
        .map_err(|_| crate::error::PyError::named("ValueError", "os.path.split() result"))?;
    let dirs = if truthy(interp, &dirname)? {
        vec![dirname]
    } else {
        let path = if path.is_none() {
            // bpo-35755: Don't use os.defpath if the PATH environment
            // variable is set to an empty string
            default_search_path(interp)?
        } else {
            path
        };
        // PATH='' doesn't match, whereas PATH=':' looks in the current
        // directory
        if !truthy(interp, &path)? {
            return Ok(Value::none());
        }
        let pathsep = os_attr(interp, "pathsep")?;
        let parts = if use_bytes {
            let path = os_call(interp, "fsencode", &[path])?;
            let sep = os_call(interp, "fsencode", &[pathsep])?;
            method(interp, &path, "split", &[sep])?
        } else {
            let path = os_call(interp, "fsdecode", &[path])?;
            method(interp, &path, "split", &[pathsep])?
        };
        let mut dirs = collect(interp, &parts)?;
        if cfg!(windows) && win_path_needs_curdir(interp, &cmd, mode)? {
            let mut curdir = os_attr(interp, "curdir")?;
            if use_bytes {
                curdir = os_call(interp, "fsencode", &[curdir])?;
            }
            dirs.insert(0, curdir);
        }
        dirs
    };

    let files = if cfg!(windows) {
        // PATHEXT is necessary to check on Windows.
        let source = os_call(interp, "getenv", &[Value::string("PATHEXT")])?;
        let source = if truthy(interp, &source)? {
            source
        } else {
            global("_WIN_DEFAULT_PATHEXT").unwrap_or_else(|| Value::string(WIN_DEFAULT_PATHEXT))
        };
        let pathsep = os_attr(interp, "pathsep")?;
        let split = method(interp, &source, "split", &[pathsep])?;
        let mut pathext = Vec::new();
        for ext in collect(interp, &split)? {
            if truthy(interp, &ext)? {
                pathext.push(if use_bytes {
                    os_call(interp, "fsencode", &[ext])?
                } else {
                    ext
                });
            }
        }
        let mut files = Vec::with_capacity(pathext.len() + 1);
        files.push(cmd.clone());
        for ext in &pathext {
            files.push(binary(interp, &cmd, BinaryOp::Add, ext)?);
        }
        // gh-109590. If we are looking for an executable, we need to look
        // for a PATHEXT match. The first cmd is the direct match (e.g.
        // python.exe instead of python). Check that direct match first if
        // and only if the extension is in PATHEXT. Otherwise check it last.
        let parts = path_call(interp, "splitext", std::slice::from_ref(&files[0]))?;
        let ext = getitem(interp, &parts, Value::int(1))?;
        let suffix = method(interp, &ext, "upper", &[])?;
        if wants_exec(interp, mode)? {
            let mut listed = false;
            for ext in &pathext {
                let upper = method(interp, ext, "upper", &[])?;
                if super::support::equal(interp, &suffix, &upper)? {
                    listed = true;
                    break;
                }
            }
            if !listed {
                let first = files.remove(0);
                files.push(first);
            }
        }
        files
    } else {
        // On other platforms you don't have things like PATHEXT to tell you
        // what file suffixes are executable, so just pass on cmd as-is.
        vec![cmd]
    };

    let mut seen: HashSet<PyKey> = HashSet::new();
    for dir in dirs {
        let normdir = path_call(interp, "normcase", std::slice::from_ref(&dir))?;
        let key = interp.value_to_pykey(&normdir)?;
        if !seen.insert(key) {
            continue;
        }
        for file in &files {
            let name = path_call(interp, "join", &[dir.clone(), file.clone()])?;
            if access_check(interp, &name, mode)? {
                return Ok(name);
            }
        }
    }
    Ok(Value::none())
}
