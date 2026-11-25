// `ntpath` module — pure native Rust implementation (issue #3245).
//
// Importable on ALL platforms (Unix and Windows), matching CPython 3.12!
// Reference: <https://docs.python.org/3.12/library/os.path.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyValueRef};
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "ntpath/ops.rs"]
pub(crate) mod ops;

use crate::builtin_modules::genericpath::ops as gp;

pyrust_module! {
    constants {
        "curdir" => Value::string("."),
        "pardir" => Value::string(".."),
        "extsep" => Value::string("."),
        "sep" => Value::string("\\"),
        "pathsep" => Value::string(";"),
        "altsep" => Value::string("/"),
        "defpath" => Value::string(".;C:\\bin"),
        "devnull" => Value::string("nul"),
        "supports_unicode_filenames" => Value::bool_(true),
        "__all__" => Value::list(
            [
                "normcase",
                "isabs",
                "join",
                "splitdrive",
                "splitroot",
                "split",
                "splitext",
                "basename",
                "dirname",
                "commonprefix",
                "getsize",
                "getmtime",
                "getatime",
                "getctime",
                "islink",
                "exists",
                "lexists",
                "isdir",
                "isfile",
                "ismount",
                "expanduser",
                "expandvars",
                "normpath",
                "abspath",
                "curdir",
                "pardir",
                "sep",
                "pathsep",
                "defpath",
                "altsep",
                "extsep",
                "devnull",
                "realpath",
                "supports_unicode_filenames",
                "relpath",
                "samefile",
                "sameopenfile",
                "samestat",
                "commonpath",
                "isjunction",
                "isdevdrive",
            ]
            .into_iter()
            .map(Value::string)
            .collect(),
        ),
    }

    fn normcase(s: PyValueRef<'_>) -> Result<Value> {
        ops::normcase(_interp, s.0)
    }

    fn isabs(s: PyValueRef<'_>) -> Result<Value> {
        ops::isabs(_interp, s.0)
    }

    fn join(path: PyValueRef<'_>, #[varargs] paths: &[Value]) -> Result<Value> {
        ops::join(_interp, path.0, paths)
    }

    fn split(p: PyValueRef<'_>) -> Result<Value> {
        ops::split(_interp, p.0)
    }

    fn splitroot(p: PyValueRef<'_>) -> Result<Value> {
        ops::splitroot(_interp, p.0)
    }

    fn splitext(p: PyValueRef<'_>) -> Result<Value> {
        ops::splitext(_interp, p.0)
    }

    fn splitdrive(p: PyValueRef<'_>) -> Result<Value> {
        ops::splitdrive(_interp, p.0)
    }

    fn basename(p: PyValueRef<'_>) -> Result<Value> {
        ops::basename(_interp, p.0)
    }

    fn dirname(p: PyValueRef<'_>) -> Result<Value> {
        ops::dirname(_interp, p.0)
    }

    fn islink(path: PyValueRef<'_>) -> Result<Value> {
        gp::islink(_interp, path.0)
    }

    fn isjunction(path: PyValueRef<'_>) -> Result<Value> {
        ops::isjunction(_interp, path.0)
    }

    fn isdevdrive(path: PyValueRef<'_>) -> Result<Value> {
        ops::isdevdrive(_interp, path.0)
    }

    fn lexists(path: PyValueRef<'_>) -> Result<Value> {
        gp::lexists(_interp, path.0)
    }

    fn ismount(path: PyValueRef<'_>) -> Result<Value> {
        ops::ismount(_interp, path.0)
    }

    fn expanduser(path: PyValueRef<'_>) -> Result<Value> {
        ops::expanduser(_interp, path.0)
    }

    fn expandvars(path: PyValueRef<'_>) -> Result<Value> {
        ops::expandvars(_interp, path.0)
    }

    fn normpath(path: PyValueRef<'_>) -> Result<Value> {
        ops::normpath(_interp, path.0)
    }

    fn abspath(path: PyValueRef<'_>) -> Result<Value> {
        ops::abspath(_interp, path.0)
    }

    fn realpath(
        path: PyValueRef<'_>,
        #[keyword_only]
        #[default(false.into())]
        strict: PyBool,
    ) -> Result<Value> {
        ops::realpath(_interp, path.0, *strict)
    }

    fn relpath(
        path: PyValueRef<'_>,
        #[default(None)] start: Option<PyValueRef<'_>>,
    ) -> Result<Value> {
        ops::relpath(_interp, path.0, start.map(|v| v.0))
    }

    fn commonpath(paths: PyValueRef<'_>) -> Result<Value> {
        ops::commonpath(_interp, paths.0)
    }

    fn commonprefix(m: PyValueRef<'_>) -> Result<Value> {
        gp::commonprefix(_interp, m.0)
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
}
