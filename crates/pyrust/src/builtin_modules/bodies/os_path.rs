// `os.path` module — delegates directly to `posixpath` on Unix and `ntpath`
// on Windows, sharing implementation with `genericpath` (issue #3245).
//
// Reference: <https://docs.python.org/3.12/library/os.path.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyValueRef};
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[cfg(test)]
#[path = "os_path/tests.rs"]
mod nt_tests;

use crate::builtin_modules::genericpath::ops as gp;
#[cfg(windows)]
use crate::builtin_modules::ntpath::ops as host;
#[cfg(not(windows))]
use crate::builtin_modules::posixpath::ops as host;

pyrust_module! {
    constants {
        "sep" => Value::string(std::path::MAIN_SEPARATOR.to_string()),
        "altsep" => {
            if cfg!(windows) {
                Value::string("/")
            } else {
                Value::none()
            }
        },
        "curdir" => Value::string("."),
        "pardir" => Value::string(".."),
        "extsep" => Value::string("."),
        "pathsep" => Value::string(if cfg!(windows) { ";" } else { ":" }),
        "defpath" => Value::string(if cfg!(windows) { ".;C:\\bin" } else { "/bin:/usr/bin" }),
        "devnull" => Value::string(if cfg!(windows) { "nul" } else { "/dev/null" }),
        "supports_unicode_filenames" => Value::bool_(cfg!(any(windows, target_os = "macos"))),
    }

    fn join(path: PyValueRef<'_>, #[varargs] paths: &[Value]) -> Result<Value> {
        host::join(_interp, path.0, paths)
    }

    fn exists(path: PyValueRef<'_>) -> Result<Value> {
        gp::exists(_interp, path.0)
    }

    fn isfile(path: PyValueRef<'_>) -> Result<Value> {
        gp::isfile(_interp, path.0)
    }

    fn isdir(s: PyValueRef<'_>) -> Result<Value> {
        gp::isdir(_interp, s.0)
    }

    fn dirname(p: PyValueRef<'_>) -> Result<Value> {
        host::dirname(_interp, p.0)
    }

    fn basename(p: PyValueRef<'_>) -> Result<Value> {
        host::basename(_interp, p.0)
    }

    fn abspath(path: PyValueRef<'_>) -> Result<Value> {
        host::abspath(_interp, path.0)
    }

    fn splitext(p: PyValueRef<'_>) -> Result<Value> {
        host::splitext(_interp, p.0)
    }

    fn split(p: PyValueRef<'_>) -> Result<Value> {
        host::split(_interp, p.0)
    }

    fn splitroot(p: PyValueRef<'_>) -> Result<Value> {
        host::splitroot(_interp, p.0)
    }

    fn isabs(s: PyValueRef<'_>) -> Result<Value> {
        host::isabs(_interp, s.0)
    }

    fn normpath(path: PyValueRef<'_>) -> Result<Value> {
        host::normpath(_interp, path.0)
    }

    fn splitdrive(p: PyValueRef<'_>) -> Result<Value> {
        host::splitdrive(_interp, p.0)
    }

    fn commonpath(paths: PyValueRef<'_>) -> Result<Value> {
        host::commonpath(_interp, paths.0)
    }

    fn commonprefix(m: PyValueRef<'_>) -> Result<Value> {
        gp::commonprefix(_interp, m.0)
    }

    fn relpath(
        path: PyValueRef<'_>,
        #[default(None)] start: Option<PyValueRef<'_>>,
    ) -> Result<Value> {
        host::relpath(_interp, path.0, start.map(|v| v.0))
    }

    fn expanduser(path: PyValueRef<'_>) -> Result<Value> {
        host::expanduser(_interp, path.0)
    }

    fn expandvars(path: PyValueRef<'_>) -> Result<Value> {
        host::expandvars(_interp, path.0)
    }

    fn realpath(
        filename: PyValueRef<'_>,
        #[keyword_only]
        #[default(false.into())]
        strict: PyBool,
    ) -> Result<Value> {
        host::realpath(_interp, filename.0, *strict)
    }

    fn normcase(s: PyValueRef<'_>) -> Result<Value> {
        host::normcase(_interp, s.0)
    }

    fn islink(path: PyValueRef<'_>) -> Result<Value> {
        gp::islink(_interp, path.0)
    }

    fn isjunction(path: PyValueRef<'_>) -> Result<Value> {
        #[cfg(windows)]
        {
            host::isjunction(_interp, path.0)
        }
        #[cfg(not(windows))]
        {
            gp::generic_isjunction(_interp, path.0)
        }
    }

    fn isdevdrive(path: PyValueRef<'_>) -> Result<Value> {
        #[cfg(windows)]
        {
            host::isdevdrive(_interp, path.0)
        }
        #[cfg(not(windows))]
        {
            gp::generic_isdevdrive(_interp, path.0)
        }
    }

    fn ismount(path: PyValueRef<'_>) -> Result<Value> {
        host::ismount(_interp, path.0)
    }

    fn lexists(path: PyValueRef<'_>) -> Result<Value> {
        gp::lexists(_interp, path.0)
    }

    fn samefile(f1: PyValueRef<'_>, f2: PyValueRef<'_>) -> Result<Value> {
        gp::samefile(_interp, f1.0, f2.0)
    }

    fn sameopenfile(fp1: PyIndex, fp2: PyIndex) -> Result<Value> {
        let fd1 = fp1.expect_i64(FN_NAME, "fp1")?;
        let fd2 = fp2.expect_i64(FN_NAME, "fp2")?;
        gp::sameopenfile(_interp, fd1, fd2)
    }

    fn samestat(s1: PyValueRef<'_>, s2: PyValueRef<'_>) -> Result<Value> {
        gp::samestat(_interp, s1.0, s2.0)
    }

    fn getsize(filename: PyValueRef<'_>) -> Result<Value> {
        gp::getsize(_interp, filename.0)
    }

    fn getmtime(filename: PyValueRef<'_>) -> Result<Value> {
        gp::getmtime(_interp, filename.0)
    }

    fn getatime(filename: PyValueRef<'_>) -> Result<Value> {
        gp::getatime(_interp, filename.0)
    }

    fn getctime(filename: PyValueRef<'_>) -> Result<Value> {
        gp::getctime(_interp, filename.0)
    }
}
