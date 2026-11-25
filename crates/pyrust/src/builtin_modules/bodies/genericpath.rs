// `genericpath` module — pure native Rust implementation (issue #3245).
//
// Reference: <https://docs.python.org/3.12/library/os.path.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyIndex, PyStr, PyValueRef};
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "genericpath/ops.rs"]
pub(crate) mod ops;

pyrust_module! {
    constants {
        "__all__" => Value::list(
            [
                "commonprefix",
                "exists",
                "getatime",
                "getctime",
                "getmtime",
                "getsize",
                "isdir",
                "isfile",
                "islink",
                "isjunction",
                "isdevdrive",
                "samefile",
                "sameopenfile",
                "samestat",
            ]
            .into_iter()
            .map(Value::string)
            .collect(),
        ),
    }

    fn commonprefix(m: PyValueRef<'_>) -> Result<Value> {
        ops::commonprefix(_interp, m.0)
    }

    fn exists(path: PyValueRef<'_>) -> Result<Value> {
        ops::exists(_interp, path.0)
    }

    fn lexists(path: PyValueRef<'_>) -> Result<Value> {
        ops::lexists(_interp, path.0)
    }

    fn isfile(path: PyValueRef<'_>) -> Result<Value> {
        ops::isfile(_interp, path.0)
    }

    fn isdir(s: PyValueRef<'_>) -> Result<Value> {
        ops::isdir(_interp, s.0)
    }

    fn islink(path: PyValueRef<'_>) -> Result<Value> {
        ops::islink(_interp, path.0)
    }

    fn isjunction(path: PyValueRef<'_>) -> Result<Value> {
        ops::generic_isjunction(_interp, path.0)
    }

    fn isdevdrive(path: PyValueRef<'_>) -> Result<Value> {
        ops::generic_isdevdrive(_interp, path.0)
    }

    fn getsize(filename: PyValueRef<'_>) -> Result<Value> {
        ops::getsize(_interp, filename.0)
    }

    fn getmtime(filename: PyValueRef<'_>) -> Result<Value> {
        ops::getmtime(_interp, filename.0)
    }

    fn getatime(filename: PyValueRef<'_>) -> Result<Value> {
        ops::getatime(_interp, filename.0)
    }

    fn getctime(filename: PyValueRef<'_>) -> Result<Value> {
        ops::getctime(_interp, filename.0)
    }

    fn samestat(s1: PyValueRef<'_>, s2: PyValueRef<'_>) -> Result<Value> {
        ops::samestat(_interp, s1.0, s2.0)
    }

    fn samefile(f1: PyValueRef<'_>, f2: PyValueRef<'_>) -> Result<Value> {
        ops::samefile(_interp, f1.0, f2.0)
    }

    fn sameopenfile(fp1: PyIndex, fp2: PyIndex) -> Result<Value> {
        let fd1 = fp1.expect_i64(FN_NAME, "fp1")?;
        let fd2 = fp2.expect_i64(FN_NAME, "fp2")?;
        ops::sameopenfile(_interp, fd1, fd2)
    }

    fn _splitext(
        p: PyValueRef<'_>,
        sep: PyValueRef<'_>,
        altsep: PyValueRef<'_>,
        extsep: PyValueRef<'_>,
    ) -> Result<Value> {
        ops::generic_splitext_py(p.0, sep.0, altsep.0, extsep.0)
    }

    fn _check_arg_types(funcname: PyStr<'_>, #[varargs] args: &[Value]) -> Result<Value> {
        ops::check_arg_types(&funcname, args)
    }
}
