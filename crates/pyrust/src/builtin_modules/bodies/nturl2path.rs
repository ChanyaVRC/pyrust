// `nturl2path` module — pure native Rust implementation (issue #3245).
//
// Reference: CPython 3.12 `Lib/nturl2path.py`.

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::PyStr;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "nturl2path/convert.rs"]
mod convert;

pyrust_module! {
    /// Convert a file URL path to a Windows pathname.
    fn url2pathname(url: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(convert::url2pathname(&url)?))
    }

    /// Convert a Windows pathname to a file URL path.
    fn pathname2url(p: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(convert::pathname2url(&p)?))
    }
}
