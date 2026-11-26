// `multiprocessing.popen_spawn_win32` — Windows `'spawn'` Popen module
// (issue #3264).

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::PyValueRef;
use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![Value::string("Popen")]),
        "TERMINATE" => Value::int(0x10000),
        "WINEXE" => Value::bool_(false),
        "WINSERVICE" => Value::bool_(false),
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
