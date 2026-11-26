// `multiprocessing.popen_spawn_posix` — Unix `'spawn'` Popen module (issue #3264).

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::PyValueRef;
use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![Value::string("Popen")]),
    }

    class Popen {
        constants {
            "method" => Value::string("spawn"),
        }

        fn __init__(
            #[positional_only] _self_val: PyValueRef<'_>,
            #[positional_only] _process_obj: PyValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::none())
        }
    }
}
