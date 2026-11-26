// `multiprocessing.resource_sharer` — 100% native-Rust Python 3.12 resource
// sharing helper (`stop`, `DupSocket`) (issue #3264).

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValueRef};
use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![Value::string("stop"), Value::string("DupSocket")]),
    }

    fn stop(#[default(OptionalArg::Omitted)] _timeout: OptValueRef<'_>) -> Result<Value> {
        Ok(Value::none())
    }

    class DupSocket {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] sock: PyValueRef<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_sock", sock.0.clone())?;
            Ok(Value::none())
        }

        fn detach(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_sock")
        }
    }
}
