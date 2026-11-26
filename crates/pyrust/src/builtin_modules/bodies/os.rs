// `os` registration facade (100% native Rust, issue #3245).
//
// Python-visible names stay in this owner while private Rust modules implement
// independent responsibilities:
//
// - `arguments`: call-shape and primitive argument validation;
// - `environment`: live process environment, `_Environ`, `putenv`/`unsetenv`,
//   `fsencode`/`fsdecode`, and `get_exec_path`;
// - `fd_ops`: low-level file-descriptor table & I/O (`open`, `fdopen`, `dup`,
//   `dup2`, `pipe`, `lseek`, `fstat`, `fsync`, `truncate`, `ftruncate`,
//   `isatty`, `closerange`, `get_inheritable`/`set_inheritable`, blocking);
// - `filesystem`: cwd, filesystem mutation, metadata, and traversal;
// - `path_protocol`: `os.fspath`, `os.PathLike` ABC wiring, `supports_*` sets;
// - `process`: `system`, `popen` (`_wrap_close`), `kill`, `waitpid`,
//   `waitstatus_to_exitcode`, `abort`, `umask`, `getlogin`, `times`;
// - `result_types`: `terminal_size`/`stat_result`/`times_result` types;
// - `scandir`: `scandir`, `ScandirIterator`, and `DirEntry`;
// - `system`: process ids, host capabilities, entropy, and terminal queries.
//
// Reference: <https://docs.python.org/3.12/library/os.html>

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyBytesLike, PyIndex, PyPath, PyStr, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PySet, Value};
use pyrust_derive::pyrust_module;

#[path = "os/arguments.rs"]
mod arguments;
#[path = "os/environment.rs"]
mod environment;
#[cfg(unix)]
pub(crate) use environment::env_lock;
#[path = "os/fd_ops.rs"]
pub(crate) mod fd_ops;
pub(crate) use fd_ops::{
    O_APPEND, O_CREAT, O_EXCL, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, clone_fd, close_fd_raw,
    fstat_fd_result, metadata_for_fd, register_fd, stat_path_result,
};
#[path = "os/filesystem.rs"]
mod filesystem;
#[path = "os/path_protocol.rs"]
mod path_protocol;
#[path = "os/process.rs"]
mod process;
#[path = "os/result_types.rs"]
mod result_types;
pub(crate) use result_types::make_stat_result;
#[path = "os/scandir.rs"]
mod scandir;
#[path = "os/system.rs"]
mod system;
#[cfg(windows)]
#[path = "os/windows.rs"]
pub(crate) mod windows;

use self::process::PopenStreamState;
use self::scandir::{DirEntryState, ScandirState};
#[cfg(windows)]
use self::windows::AddedDllDirectoryState;

/// Post-load wiring invoked from `builtin_modules/finalization.rs`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    path_protocol::prepare_os_module(interp, module);
}

/// Post-cache wiring invoked from `builtin_modules/mod.rs::post_load_inject`.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &std::rc::Rc<std::cell::RefCell<crate::value::PyModule>>,
) -> Result<()> {
    path_protocol::post_load_os_module(interp, module)
}

/// Registry names for the private `_Environ` class.
const ENVIRON_METHODS: &[(&str, &str)] = &[
    ("__getitem__", "os._Environ.__getitem__"),
    ("__setitem__", "os._Environ.__setitem__"),
    ("__delitem__", "os._Environ.__delitem__"),
    ("__contains__", "os._Environ.__contains__"),
    ("__iter__", "os._Environ.__iter__"),
    ("__len__", "os._Environ.__len__"),
    ("__repr__", "os._Environ.__repr__"),
    ("get", "os._Environ.get"),
    ("keys", "os._Environ.keys"),
    ("values", "os._Environ.values"),
    ("items", "os._Environ.items"),
];

pub(super) const TERMINAL_SIZE_NEW_REGISTRY: &str = "os.terminal_size.__new__";
pub(super) const TERMINAL_SIZE_REPR_REGISTRY: &str = "os.terminal_size.__repr__";
pub(super) const STAT_RESULT_NEW_REGISTRY: &str = "os.stat_result.__new__";
pub(super) const STAT_RESULT_REPR_REGISTRY: &str = "os.stat_result.__repr__";

pyrust_module! {
    constants {
        "sep" => Value::string(std::path::MAIN_SEPARATOR.to_string()),
        "environ" => environment::make_environ_instance(ENVIRON_METHODS),
        "name" => Value::string(if cfg!(windows) { "nt" } else { "posix" }),
        "linesep" => Value::string(if cfg!(windows) { "\r\n" } else { "\n" }),
        "curdir" => Value::string("."),
        "pardir" => Value::string(".."),
        "extsep" => Value::string("."),
        "pathsep" => Value::string(if cfg!(windows) { ";" } else { ":" }),
        "defpath" => Value::string(if cfg!(windows) { ".;C:\\bin" } else { "/bin:/usr/bin" }),
        "altsep" => {
            if cfg!(windows) {
                Value::string("/")
            } else {
                Value::none()
            }
        },
        "devnull" => Value::string(if cfg!(windows) { "nul" } else { "/dev/null" }),
        "F_OK" => Value::int(0),
        "R_OK" => Value::int(4),
        "W_OK" => Value::int(2),
        "X_OK" => Value::int(1),
        "SEEK_SET" => Value::int(fd_ops::SEEK_SET),
        "SEEK_CUR" => Value::int(fd_ops::SEEK_CUR),
        "SEEK_END" => Value::int(fd_ops::SEEK_END),
        "O_RDONLY" => Value::int(fd_ops::O_RDONLY),
        "O_WRONLY" => Value::int(fd_ops::O_WRONLY),
        "O_RDWR" => Value::int(fd_ops::O_RDWR),
        "O_CREAT" => Value::int(fd_ops::O_CREAT),
        "O_EXCL" => Value::int(fd_ops::O_EXCL),
        "O_TRUNC" => Value::int(fd_ops::O_TRUNC),
        "O_APPEND" => Value::int(fd_ops::O_APPEND),
        #[cfg(windows)]
        "O_BINARY" => Value::int(fd_ops::O_BINARY),
        #[cfg(windows)]
        "O_TEXT" => Value::int(fd_ops::O_TEXT),
        #[cfg(unix)]
        "O_NONBLOCK" => Value::int(libc::O_NONBLOCK as i64),
        #[cfg(unix)]
        "O_CLOEXEC" => Value::int(libc::O_CLOEXEC as i64),
        #[cfg(unix)]
        "O_NOFOLLOW" => Value::int(libc::O_NOFOLLOW as i64),
        #[cfg(unix)]
        "WNOHANG" => Value::int(libc::WNOHANG as i64),
        "EX_OK" => Value::int(0),
        "error" => Value::py_class(
            crate::interpreter::lookup_exc_class("OSError")
                .expect("canonical OSError class must exist"),
        ),
        "supports_bytes_environ" => Value::bool_(cfg!(unix)),
        "supports_dir_fd" => Value::set(PySet::default()),
        "supports_effective_ids" => Value::set(PySet::default()),
        "supports_fd" => Value::set(PySet::default()),
        "supports_follow_symlinks" => Value::set(PySet::default()),
        "terminal_size" => result_types::terminal_size_class_value(),
        "stat_result" => result_types::stat_result_class_value(),
        "times_result" => result_types::times_result_class_value(),
    }

    // ── Filesystem & path operations ─────────────────────────────────────────
    fn getcwd(args) -> Result<Value> { filesystem::getcwd(args, FN_NAME) }
    fn getcwdb() -> Result<Value> { filesystem::getcwdb() }
    fn chdir(args) -> Result<Value> { filesystem::chdir(_interp, args, FN_NAME) }
    fn listdir(args) -> Result<Value> { filesystem::listdir(_interp, args, FN_NAME) }
    fn mkdir(args) -> Result<Value> { filesystem::mkdir(_interp, args, FN_NAME) }
    fn makedirs(args) -> Result<Value> { filesystem::makedirs(_interp, args, FN_NAME) }
    fn remove(args) -> Result<Value> { filesystem::remove(_interp, args, FN_NAME) }
    fn unlink(args) -> Result<Value> { filesystem::unlink(_interp, args, FN_NAME) }
    fn rmdir(args) -> Result<Value> { filesystem::rmdir(_interp, args, FN_NAME) }
    fn removedirs(name: PyPath) -> Result<Value> { filesystem::removedirs(name) }
    fn rename(args) -> Result<Value> { filesystem::rename(_interp, args, FN_NAME) }
    fn renames(old: PyPath, new: PyPath) -> Result<Value> { filesystem::renames(old, new) }
    fn replace(src: PyPath, dst: PyPath) -> Result<Value> { filesystem::replace(src, dst) }
    fn link(src: PyPath, dst: PyPath) -> Result<Value> { filesystem::link(src, dst) }
    fn walk(args) -> Result<Value> { filesystem::walk(_interp, args, FN_NAME) }
    fn stat(args) -> Result<Value> { filesystem::stat(_interp, args, FN_NAME) }
    fn lstat(args) -> Result<Value> { filesystem::lstat(_interp, args, FN_NAME) }
    fn access(args) -> Result<Value> { filesystem::access(_interp, args, FN_NAME) }
    fn chmod(args) -> Result<Value> { filesystem::chmod(_interp, args, FN_NAME) }
    fn symlink(args) -> Result<Value> { filesystem::symlink(_interp, args, FN_NAME) }
    fn readlink(args) -> Result<Value> { filesystem::readlink(_interp, args, FN_NAME) }
    fn fspath(args) -> Result<Value> { path_protocol::fspath(_interp, args, FN_NAME) }

    fn utime(
        path: PyPath,
        #[default(None)] times: Option<PyValueRef<'_>>,
        #[keyword_only]
        #[default(None)]
        ns: Option<PyValueRef<'_>>,
        #[keyword_only]
        #[default(true.into())]
        follow_symlinks: PyBool,
    ) -> Result<Value> {
        filesystem::utime(_interp, path, times.map(|v| v.0), ns.map(|v| v.0), *follow_symlinks)
    }

    fn scandir(#[default(None)] path: Option<PyValueRef<'_>>) -> Result<Value> {
        scandir::scandir_open(_interp, path.map(|v| v.0))
    }

    // ── Low-level FD operations ──────────────────────────────────────────────
    fn open(
        path: PyPath,
        flags: PyIndex,
        #[default(0o777_i64.into())] mode: PyIndex,
    ) -> Result<Value> {
        fd_ops::open_path(path, flags.expect_i64(FN_NAME, "flags")?, mode.expect_i64(FN_NAME, "mode")?)
    }

    fn fdopen(
        fd: PyIndex,
        #[default("r".into())] mode: PyStr<'_>,
        #[default((-1_i64).into())] buffering: PyIndex,
        #[default(None)] encoding: Option<PyStr<'_>>,
        #[default(None)] errors: Option<PyStr<'_>>,
        #[default(None)] newline: Option<PyStr<'_>>,
        #[default(true.into())] closefd: PyBool,
    ) -> Result<Value> {
        fd_ops::fdopen(
            fd.expect_i64(FN_NAME, "fd")?,
            &mode,
            buffering.expect_i64(FN_NAME, "buffering")?,
            encoding.as_deref(),
            errors.as_deref(),
            newline.as_deref(),
            *closefd,
        )
    }

    fn close(fd: PyIndex) -> Result<Value> { fd_ops::close_fd(fd.expect_i64(FN_NAME, "fd")?) }
    fn read(fd: PyIndex, n: PyIndex) -> Result<Value> {
        fd_ops::read_fd(fd.expect_i64(FN_NAME, "fd")?, n.expect_i64(FN_NAME, "n")?)
    }
    fn write(fd: PyIndex, data: PyBytesLike<'_>) -> Result<Value> {
        fd_ops::write_fd(fd.expect_i64(FN_NAME, "fd")?, data)
    }
    fn dup(fd: PyIndex) -> Result<Value> { fd_ops::dup(fd.expect_i64(FN_NAME, "fd")?) }
    fn dup2(
        fd: PyIndex,
        fd2: PyIndex,
        #[default(true.into())] inheritable: PyBool,
    ) -> Result<Value> {
        fd_ops::dup2(fd.expect_i64(FN_NAME, "fd")?, fd2.expect_i64(FN_NAME, "fd2")?, *inheritable)
    }
    fn pipe() -> Result<Value> { fd_ops::pipe() }
    fn lseek(fd: PyIndex, pos: PyIndex, how: PyIndex) -> Result<Value> {
        fd_ops::lseek(
            fd.expect_i64(FN_NAME, "fd")?,
            pos.expect_i64(FN_NAME, "pos")?,
            how.expect_i64(FN_NAME, "how")?,
        )
    }
    fn fstat(fd: PyIndex) -> Result<Value> { fd_ops::fstat(fd.expect_i64(FN_NAME, "fd")?) }
    fn fsync(fd: PyIndex) -> Result<Value> { fd_ops::fsync(fd.expect_i64(FN_NAME, "fd")?) }
    fn ftruncate(fd: PyIndex, length: PyIndex) -> Result<Value> {
        fd_ops::ftruncate(fd.expect_i64(FN_NAME, "fd")?, length.expect_i64(FN_NAME, "length")?)
    }
    fn truncate(path: PyValueRef<'_>, length: PyIndex) -> Result<Value> {
        fd_ops::truncate(_interp, path.0, length.expect_i64(FN_NAME, "length")?)
    }
    fn isatty(fd: PyIndex) -> Result<Value> { fd_ops::isatty(fd.expect_i64(FN_NAME, "fd")?) }
    fn closerange(fd_low: PyIndex, fd_high: PyIndex) -> Result<Value> {
        fd_ops::closerange(fd_low.expect_i64(FN_NAME, "fd_low")?, fd_high.expect_i64(FN_NAME, "fd_high")?)
    }
    fn get_inheritable(fd: PyIndex) -> Result<Value> {
        fd_ops::get_inheritable(fd.expect_i64(FN_NAME, "fd")?)
    }
    fn set_inheritable(fd: PyIndex, inheritable: PyBool) -> Result<Value> {
        fd_ops::set_inheritable(fd.expect_i64(FN_NAME, "fd")?, *inheritable)
    }
    #[cfg(unix)]
    fn get_blocking(fd: PyIndex) -> Result<Value> {
        fd_ops::get_blocking(fd.expect_i64(FN_NAME, "fd")?)
    }
    #[cfg(unix)]
    fn set_blocking(fd: PyIndex, blocking: PyBool) -> Result<Value> {
        fd_ops::set_blocking(fd.expect_i64(FN_NAME, "fd")?, *blocking)
    }

    // ── Environment & encoding operations ────────────────────────────────────
    fn getenv(args) -> Result<Value> { environment::getenv(args, FN_NAME) }
    fn putenv(name: PyValueRef<'_>, value: PyValueRef<'_>) -> Result<Value> {
        environment::putenv(name.0, value.0, FN_NAME)
    }
    fn unsetenv(name: PyValueRef<'_>) -> Result<Value> {
        environment::unsetenv(name.0, FN_NAME)
    }
    fn fsencode(filename: PyValueRef<'_>) -> Result<Value> {
        environment::fsencode(_interp, filename.0)
    }
    fn fsdecode(filename: PyValueRef<'_>) -> Result<Value> {
        environment::fsdecode(_interp, filename.0)
    }
    fn get_exec_path(#[default(None)] env: Option<PyValueRef<'_>>) -> Result<Value> {
        environment::get_exec_path(_interp, env.map(|v| v.0))
    }

    // ── Process & system operations ──────────────────────────────────────────
    fn getpid(args) -> Result<Value> { system::getpid(args, FN_NAME) }
    fn getppid(args) -> Result<Value> { system::getppid(args, FN_NAME) }
    fn cpu_count(args) -> Result<Value> { system::cpu_count(args, FN_NAME) }
    fn urandom(args) -> Result<Value> { system::urandom(args, FN_NAME) }
    fn strerror(args) -> Result<Value> { system::strerror(args, FN_NAME) }
    fn get_terminal_size(args) -> Result<Value> { system::get_terminal_size(args, FN_NAME) }
    fn system(command: PyStr<'_>) -> Result<Value> { process::system(&command) }
    fn popen(
        cmd: PyStr<'_>,
        #[default("r".into())] mode: PyStr<'_>,
        #[default((-1_i64).into())] buffering: PyIndex,
    ) -> Result<Value> {
        process::popen_open(_interp, &cmd, &mode, buffering.expect_i64(FN_NAME, "buffering")?)
    }
    fn kill(pid: PyIndex, sig: PyIndex) -> Result<Value> {
        process::kill(pid.expect_i64(FN_NAME, "pid")?, sig.expect_i64(FN_NAME, "sig")?)
    }
    fn waitpid(pid: PyIndex, options: PyIndex) -> Result<Value> {
        process::waitpid(pid.expect_i64(FN_NAME, "pid")?, options.expect_i64(FN_NAME, "options")?)
    }
    fn waitstatus_to_exitcode(status: PyIndex) -> Result<Value> {
        process::waitstatus_to_exitcode(status.expect_i64(FN_NAME, "status")?)
    }
    fn abort() -> Result<Value> { process::abort() }
    fn umask(mask: PyIndex) -> Result<Value> { process::umask(mask.expect_i64(FN_NAME, "mask")?) }
    fn getlogin() -> Result<Value> { process::getlogin() }
    fn times() -> Result<Value> { process::times() }
    #[cfg(windows)]
    fn startfile(filepath: PyPath, #[default(None)] operation: Option<PyStr<'_>>, #[default(None)] arguments: Option<PyStr<'_>>, #[default(None)] cwd: Option<PyPath>, #[default(1_i64.into())] show_cmd: PyIndex) -> Result<Value> {
        windows::startfile(filepath, operation.as_deref(), arguments.as_deref(), cwd, show_cmd.expect_i64(FN_NAME, "show_cmd")?)
    }
    #[cfg(windows)]
    fn add_dll_directory(path: PyPath) -> Result<Value> { windows::add_dll_directory(_interp, path) }
    #[cfg(windows)]
    fn listdrives() -> Result<Value> { windows::listdrives() }
    #[cfg(windows)]
    fn listvolumes() -> Result<Value> { windows::listvolumes() }
    #[cfg(windows)]
    fn listmounts(volume: PyPath) -> Result<Value> { windows::listmounts(volume) }
    #[cfg(windows)]
    fn device_encoding(fd: PyIndex) -> Result<Value> { windows::device_encoding(fd.expect_i64(FN_NAME, "fd")?) }

    // ── Result-type & _Environ internal methods ──────────────────────────────
    #[py_name = "terminal_size.__repr__"]
    fn terminal_size_repr(args) -> Result<Value> { result_types::terminal_size_repr(args, FN_NAME) }
    #[py_name = "terminal_size.__new__"]
    fn terminal_size_new(args) -> Result<Value> { result_types::terminal_size_new(_interp, args, FN_NAME) }
    #[py_name = "stat_result.__new__"]
    fn stat_result_new(args) -> Result<Value> { result_types::stat_result_new(_interp, args, FN_NAME) }
    #[py_name = "stat_result.__repr__"]
    fn stat_result_repr(args) -> Result<Value> { result_types::stat_result_repr(args, FN_NAME) }
    #[py_name = "_Environ.__getitem__"]
    fn environ_getitem(args) -> Result<Value> { environment::environ_getitem(args, FN_NAME) }
    #[py_name = "_Environ.__setitem__"]
    fn environ_setitem(args) -> Result<Value> { environment::environ_setitem(args, FN_NAME) }
    #[py_name = "_Environ.__delitem__"]
    fn environ_delitem(args) -> Result<Value> { environment::environ_delitem(args, FN_NAME) }
    #[py_name = "_Environ.__contains__"]
    fn environ_contains(args) -> Result<Value> { environment::environ_contains(args, FN_NAME) }
    #[py_name = "_Environ.__iter__"]
    fn environ_iter(args) -> Result<Value> { environment::environ_iter(args, FN_NAME) }
    #[py_name = "_Environ.__len__"]
    fn environ_len(args) -> Result<Value> { environment::environ_len(args, FN_NAME) }
    #[py_name = "_Environ.__repr__"]
    fn environ_repr(args) -> Result<Value> { environment::environ_repr(args, FN_NAME) }
    #[py_name = "_Environ.get"]
    fn environ_get(args) -> Result<Value> { environment::environ_get(args, FN_NAME) }
    #[py_name = "_Environ.keys"]
    fn environ_keys(args) -> Result<Value> { environment::environ_keys(args, FN_NAME) }
    #[py_name = "_Environ.values"]
    fn environ_values(args) -> Result<Value> { environment::environ_values(args, FN_NAME) }
    #[py_name = "_Environ.items"]
    fn environ_items(args) -> Result<Value> { environment::environ_items(args, FN_NAME) }

    // ── Native classes (`DirEntry`, `ScandirIterator`, `_wrap_close`, `PathLike`) ──
    #[native_state(DirEntryState)]
    class DirEntry {
        #[getter]
        fn name(#[state] st: &DirEntryState) -> Result<Value> { scandir::entry_name(st) }
        #[getter]
        fn path(#[state] st: &DirEntryState) -> Result<Value> { scandir::entry_path(st) }
        fn inode(#[state_mut] st: &mut DirEntryState) -> Result<Value> { scandir::entry_inode(st) }
        fn is_dir(
            #[state_mut] st: &mut DirEntryState,
            #[keyword_only] #[default(true.into())] follow_symlinks: PyBool,
        ) -> Result<Value> { scandir::entry_is_dir(st, *follow_symlinks) }
        fn is_file(
            #[state_mut] st: &mut DirEntryState,
            #[keyword_only] #[default(true.into())] follow_symlinks: PyBool,
        ) -> Result<Value> { scandir::entry_is_file(st, *follow_symlinks) }
        fn is_symlink(#[state_mut] st: &mut DirEntryState) -> Result<Value> { scandir::entry_is_symlink(st) }
        fn is_junction(#[state_mut] st: &mut DirEntryState) -> Result<Value> { scandir::entry_is_junction(st) }
        fn stat(
            #[state_mut] st: &mut DirEntryState,
            #[keyword_only] #[default(true.into())] follow_symlinks: PyBool,
        ) -> Result<Value> { scandir::entry_stat(st, *follow_symlinks) }
        fn __fspath__(#[state] st: &DirEntryState) -> Result<Value> { scandir::entry_path(st) }
        fn __repr__(#[state] st: &DirEntryState) -> Result<Value> { scandir::entry_repr(st) }
    }

    #[native_state(ScandirState)]
    class ScandirIterator {
        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> { Ok(self_val.0.clone()) }
        fn __next__(#[state_mut] st: &mut ScandirState) -> Result<Value> { scandir::scandir_next(st) }
        fn close(#[state_mut] st: &mut ScandirState) -> Result<Value> { scandir::scandir_close(st) }
        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> { Ok(self_val.0.clone()) }
        fn __exit__(#[state_mut] st: &mut ScandirState, #[varargs] _args: &[Value]) -> Result<Value> {
            scandir::scandir_close(st)
        }
    }

    #[native_state(PopenStreamState)]
    class _wrap_close {
        #[getter]
        fn closed(#[state] st: &PopenStreamState) -> Result<Value> { Ok(process::popen_is_closed(st)) }
        #[getter]
        fn mode(#[state] st: &PopenStreamState) -> Result<Value> { Ok(process::popen_mode(st)) }
        fn readable(#[state] st: &PopenStreamState) -> Result<Value> { process::popen_readable(st) }
        fn writable(#[state] st: &PopenStreamState) -> Result<Value> { process::popen_writable(st) }
        fn seekable(#[state] _st: &PopenStreamState) -> Result<Value> { Ok(Value::bool_(false)) }
        fn read(#[state_mut] st: &mut PopenStreamState, #[default(None)] size: Option<PyIndex>) -> Result<Value> {
            let sz = match size {
                Some(i) => Some(i.expect_i64(FN_NAME, "size")?),
                None => None,
            };
            process::popen_read(st, sz)
        }
        fn readline(#[state_mut] st: &mut PopenStreamState, #[default(None)] size: Option<PyIndex>) -> Result<Value> {
            let sz = match size {
                Some(i) => Some(i.expect_i64(FN_NAME, "size")?),
                None => None,
            };
            process::popen_readline(st, sz)
        }
        fn readlines(#[state_mut] st: &mut PopenStreamState) -> Result<Value> { process::popen_readlines(st) }
        fn write(#[state_mut] st: &mut PopenStreamState, data: PyStr<'_>) -> Result<Value> {
            process::popen_write(st, &data)
        }
        fn flush(#[state_mut] st: &mut PopenStreamState) -> Result<Value> { process::popen_flush(st) }
        fn close(#[state_mut] st: &mut PopenStreamState) -> Result<Value> { process::popen_close(st) }
        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> { Ok(self_val.0.clone()) }
        fn __exit__(#[state_mut] st: &mut PopenStreamState, #[varargs] _args: &[Value]) -> Result<Value> {
            let _ = process::popen_close(st)?;
            Ok(Value::none())
        }
        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> { Ok(self_val.0.clone()) }
        fn __next__(#[state_mut] st: &mut PopenStreamState) -> Result<Value> { process::popen_next(st) }
    }

    class PathLike {
        fn __fspath__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", String::new()))
        }
        #[classmethod]
        fn __subclasshook__(#[positional_only] cls: PyValueRef<'_>, subclass: PyValueRef<'_>) -> Result<Value> {
            path_protocol::pathlike_subclasshook(cls.0, subclass.0)
        }
        #[classmethod]
        fn __class_getitem__(#[positional_only] cls: PyValueRef<'_>, item: PyValueRef<'_>) -> Result<Value> {
            path_protocol::pathlike_class_getitem(cls.0, item.0)
        }
    }

    #[cfg(windows)]
    #[native_state(AddedDllDirectoryState)]
    class _AddedDllDirectory {
        #[getter]
        fn path(#[state] st: &AddedDllDirectoryState) -> Result<Value> { Ok(windows::added_dll_dir_path(st)) }
        fn close(#[state_mut] st: &mut AddedDllDirectoryState) -> Result<Value> { windows::added_dll_dir_close(st) }
        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> { Ok(self_val.0.clone()) }
        fn __exit__(#[state_mut] st: &mut AddedDllDirectoryState, #[varargs] _args: &[Value]) -> Result<Value> {
            windows::added_dll_dir_close(st)
        }
        fn __repr__(#[state] st: &AddedDllDirectoryState) -> Result<Value> { windows::added_dll_dir_repr(st) }
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::ENVIRON_METHODS;

    const OWNER: &str = include_str!("os.rs");
    const ARGUMENTS: &str = include_str!("os/arguments.rs");
    const ENVIRONMENT: &str = include_str!("os/environment.rs");
    const FD_OPS: &str = include_str!("os/fd_ops.rs");
    const FILESYSTEM: &str = include_str!("os/filesystem.rs");
    const PATH_PROTOCOL: &str = include_str!("os/path_protocol.rs");
    const PROCESS: &str = include_str!("os/process.rs");
    const RESULT_TYPES: &str = include_str!("os/result_types.rs");
    const SCANDIR: &str = include_str!("os/scandir.rs");
    const SYSTEM: &str = include_str!("os/system.rs");
    const IMPLEMENTATIONS: &str = concat!(
        include_str!("os/arguments.rs"),
        include_str!("os/environment.rs"),
        include_str!("os/fd_ops.rs"),
        include_str!("os/filesystem.rs"),
        include_str!("os/path_protocol.rs"),
        include_str!("os/process.rs"),
        include_str!("os/result_types.rs"),
        include_str!("os/scandir.rs"),
        include_str!("os/system.rs"),
    );

    #[test]
    fn owner_keeps_the_only_registration_surface() {
        let registration_macro = concat!("pyrust_", "module!");
        assert_eq!(OWNER.matches(registration_macro).count(), 1);
        assert!(!IMPLEMENTATIONS.contains(registration_macro));
        assert!(
            !IMPLEMENTATIONS
                .lines()
                .any(|line| line.trim_start().starts_with(concat!("#[py_", "name")))
        );
    }

    #[test]
    fn implementation_dependencies_are_explicit_real_modules() {
        assert!(!IMPLEMENTATIONS.contains(concat!("use super::", "*")));
        assert!(!IMPLEMENTATIONS.contains(concat!("include", "!(")));
        assert!(!OWNER.contains(concat!("include", "!(")));
    }

    #[test]
    fn responsibilities_stay_on_their_side_of_the_boundary() {
        assert!(ARGUMENTS.contains("fn require_str"));
        assert!(ENVIRONMENT.contains("static ENV_LOCK"));
        assert!(FD_OPS.contains("static FD_TABLE"));
        assert!(FILESYSTEM.contains("fn walk_collect"));
        assert!(PATH_PROTOCOL.contains("\"__fspath__\""));
        assert!(PROCESS.contains("fn waitstatus_to_exitcode"));
        assert!(RESULT_TYPES.contains("STAT_RESULT_CLASS"));
        assert!(SCANDIR.contains("struct DirEntryState"));
        assert!(SYSTEM.contains("fn get_parent_pid"));

        assert!(!ARGUMENTS.contains("std::fs::"));
        assert!(!ARGUMENTS.contains("std::env::"));
        assert!(!ENVIRONMENT.contains("std::fs::"));
        assert!(!PATH_PROTOCOL.contains("std::fs::"));
        assert!(!PATH_PROTOCOL.contains("std::env::"));
        assert!(!SYSTEM.contains("walk_collect"));
    }

    #[test]
    fn registration_facade_stays_small_and_complete() {
        let environ_registration = concat!("#[py_", "name = \"_Environ.");
        assert!(
            OWNER.lines().count() <= 550,
            "os registration facade grew beyond its 550-line budget ({} lines)",
            OWNER.lines().count()
        );
        assert_eq!(OWNER.matches(environ_registration).count(), 11);
        assert_eq!(ENVIRON_METHODS.len(), 11);
    }
}
