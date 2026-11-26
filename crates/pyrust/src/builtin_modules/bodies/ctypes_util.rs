// `ctypes.util` module — 100% native-Rust dynamic library discovery (issue #3265).

use pyrust_derive::pyrust_module;

use crate::builtin_modules::ctypes_rt::dynlib;
use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::PyStr;
use crate::value::Value;

pyrust_module! {
    fn find_library(name: PyStr<'_>) -> Result<Value> {
        Ok(match dynlib::find_library(&name.0) {
            Some(found) => Value::string(found),
            None => Value::none(),
        })
    }

    #[cfg(windows)]
    fn find_msvcrt() -> Result<Value> {
        // CPython 3.12 on Windows uses UCRT and returns `None` from `find_msvcrt()`.
        Ok(Value::none())
    }
}
