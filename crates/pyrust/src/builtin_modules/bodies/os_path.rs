// `os.path` module — included into `pub mod os_path { … }` declared by
// the `"os.path" as os_path` entry in `pyrust_builtin_modules!` in
// `builtin_modules/mod.rs`.  The macro injects `MODULE_NAME = "os.path"` and
// `FN_PREFIX = "os.path."`, so registered names look like `"os.path.join"`.
//
// This file is the thin Python-registration facade; path splitting/joining,
// normalization, and filesystem stat queries live in submodules under
// `bodies/os_path/`.
//
// Reference: <https://docs.python.org/3/library/os.path.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "os_path/normalize.rs"]
mod normalize;
#[cfg(test)]
#[path = "os_path/tests.rs"]
mod nt_tests;
#[path = "os_path/split_join.rs"]
mod split_join;
#[path = "os_path/stat_queries.rs"]
mod stat_queries;

use self::normalize::{
    host_abspath, host_expanduser, host_isabs, host_normpath, normcase_impl, relpath_impl,
};
use self::split_join::{
    commonprefix_impl, host_split, host_splitdrive, host_splitext, join_impl, single_path,
};
use self::stat_queries::{
    exists_impl, isdir_impl, isfile_impl, islink_impl, lexists_impl, realpath_impl, samefile_impl,
};

pyrust_module! {
    constants {
        // Expose the path separator here so callers can build platform-aware
        // paths without depending on `os` proper.
        "sep" => Value::string(std::path::MAIN_SEPARATOR.to_string()),
        "altsep" => {
            if cfg!(windows) {
                Value::string("/")
            } else {
                Value::none()
            }
        },
    }

    /// CPython: os.path.join(path, *paths) — join path components.
    /// <https://docs.python.org/3/library/os.path.html#os.path.join>
    fn join(args) -> Result<Value> {
        join_impl(FN_NAME, args)
    }

    /// CPython: os.path.exists(path) — true if `path` refers to an existing path.
    /// <https://docs.python.org/3/library/os.path.html#os.path.exists>
    fn exists(args) -> Result<Value> {
        exists_impl(FN_NAME, args)
    }

    /// CPython: os.path.isfile(path) — true if `path` is a regular file.
    /// <https://docs.python.org/3/library/os.path.html#os.path.isfile>
    fn isfile(args) -> Result<Value> {
        isfile_impl(FN_NAME, args)
    }

    /// CPython: os.path.isdir(path) — true if `path` is a directory.
    /// <https://docs.python.org/3/library/os.path.html#os.path.isdir>
    fn isdir(args) -> Result<Value> {
        isdir_impl(FN_NAME, args)
    }

    /// CPython: os.path.dirname(path) — directory name of `path`.
    /// <https://docs.python.org/3/library/os.path.html#os.path.dirname>
    fn dirname(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let (head, _) = host_split(&path);
        Ok(Value::string(head))
    }

    /// CPython: os.path.basename(path) — final path component.
    /// <https://docs.python.org/3/library/os.path.html#os.path.basename>
    fn basename(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let (_, tail) = host_split(&path);
        Ok(Value::string(tail))
    }

    /// CPython: os.path.abspath(path) — absolute normalised form of `path`.
    /// <https://docs.python.org/3/library/os.path.html#os.path.abspath>
    fn abspath(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::string(host_abspath(&path)?))
    }

    /// CPython: os.path.splitext(path) -> (root, ext).
    /// <https://docs.python.org/3/library/os.path.html#os.path.splitext>
    fn splitext(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let (root, ext) = host_splitext(&path);
        Ok(Value::tuple(vec![Value::string(root), Value::string(ext)]))
    }

    /// CPython: os.path.split(path) -> (head, tail).
    /// <https://docs.python.org/3/library/os.path.html#os.path.split>
    fn split(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let (head, tail) = host_split(&path);
        Ok(Value::tuple(vec![Value::string(head), Value::string(tail)]))
    }

    /// CPython: os.path.isabs(path) — true if `path` is absolute.
    /// <https://docs.python.org/3/library/os.path.html#os.path.isabs>
    fn isabs(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::bool_(host_isabs(&path)))
    }

    /// CPython: os.path.normpath(path) — collapse redundant separators and `.`/`..`.
    /// <https://docs.python.org/3/library/os.path.html#os.path.normpath>
    fn normpath(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::string(host_normpath(&path)))
    }

    /// CPython: os.path.splitdrive(path) -> (drive, tail).
    /// <https://docs.python.org/3/library/os.path.html#os.path.splitdrive>
    fn splitdrive(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        let (drive, tail) = host_splitdrive(&path);
        Ok(Value::tuple(vec![Value::string(drive), Value::string(tail)]))
    }

    /// CPython: os.path.commonprefix(list) — longest common leading substring.
    /// <https://docs.python.org/3/library/os.path.html#os.path.commonprefix>
    fn commonprefix(args) -> Result<Value> {
        commonprefix_impl(_interp, FN_NAME, args)
    }

    /// CPython: os.path.relpath(path, start=os.curdir) — relative path from `start` to `path`.
    /// <https://docs.python.org/3/library/os.path.html#os.path.relpath>
    fn relpath(args) -> Result<Value> {
        relpath_impl(FN_NAME, args)
    }

    /// CPython: os.path.expanduser(path) — expand a leading `~`.
    /// <https://docs.python.org/3/library/os.path.html#os.path.expanduser>
    fn expanduser(args) -> Result<Value> {
        let path = single_path(FN_NAME, args)?;
        Ok(Value::string(host_expanduser(&path)))
    }

    /// CPython: os.path.realpath(path) — canonical path, resolving symlinks where possible.
    /// <https://docs.python.org/3/library/os.path.html#os.path.realpath>
    fn realpath(args) -> Result<Value> {
        realpath_impl(FN_NAME, args)
    }

    /// CPython: os.path.normcase(path) — normalize case and separators for the host OS.
    /// <https://docs.python.org/3/library/os.path.html#os.path.normcase>
    fn normcase(args) -> Result<Value> {
        normcase_impl(FN_NAME, args)
    }

    /// CPython: os.path.islink(path) — true if `path` refers to a symbolic link.
    /// <https://docs.python.org/3/library/os.path.html#os.path.islink>
    fn islink(args) -> Result<Value> {
        islink_impl(FN_NAME, args)
    }

    /// CPython: os.path.lexists(path) — true if `path` exists (including broken symlinks).
    /// <https://docs.python.org/3/library/os.path.html#os.path.lexists>
    fn lexists(args) -> Result<Value> {
        lexists_impl(FN_NAME, args)
    }

    /// CPython: os.path.samefile(path1, path2) — true if both paths refer to the same file.
    /// <https://docs.python.org/3/library/os.path.html#os.path.samefile>
    fn samefile(args) -> Result<Value> {
        samefile_impl(FN_NAME, args)
    }
}
