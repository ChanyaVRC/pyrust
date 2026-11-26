// `multiprocessing.resource_tracker` — 100% native-Rust Python 3.12 resource
// tracking registry (`ResourceTracker`, `ensure_running`, `register`,
// `unregister`, `getfd`, `main`, `ReentrantCallError`) (issue #3264).

use std::cell::RefCell;
use std::collections::HashSet;

use pyrust_core::gil_local;
use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyIndex, PyStr, PyValueRef};
use crate::value::Value;

gil_local! {
    static TRACKED_RESOURCES: RefCell<HashSet<(String, String)>> = RefCell::new(HashSet::new());
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("ensure_running"),
            Value::string("register"),
            Value::string("unregister"),
        ]),
    }

    exception ReentrantCallError(RuntimeError);

    class ResourceTracker {
        fn getfd(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::int(-1))
        }

        fn ensure_running(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn register(
            #[positional_only] _self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
            #[positional_only] rtype: PyStr<'_>,
        ) -> Result<Value> {
            TRACKED_RESOURCES.with(|r| {
                r.borrow_mut()
                    .insert((name.0.to_string(), rtype.0.to_string()));
            });
            Ok(Value::none())
        }

        fn unregister(
            #[positional_only] _self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
            #[positional_only] rtype: PyStr<'_>,
        ) -> Result<Value> {
            TRACKED_RESOURCES.with(|r| {
                r.borrow_mut()
                    .remove(&(name.0.to_string(), rtype.0.to_string()));
            });
            Ok(Value::none())
        }
    }

    fn ensure_running() -> Result<Value> {
        Ok(Value::none())
    }

    fn getfd() -> Result<Value> {
        Ok(Value::int(-1))
    }

    fn register(
        #[positional_only] name: PyStr<'_>,
        #[positional_only] rtype: PyStr<'_>,
    ) -> Result<Value> {
        TRACKED_RESOURCES.with(|r| {
            r.borrow_mut()
                .insert((name.0.to_string(), rtype.0.to_string()));
        });
        Ok(Value::none())
    }

    fn unregister(
        #[positional_only] name: PyStr<'_>,
        #[positional_only] rtype: PyStr<'_>,
    ) -> Result<Value> {
        TRACKED_RESOURCES.with(|r| {
            r.borrow_mut()
                .remove(&(name.0.to_string(), rtype.0.to_string()));
        });
        Ok(Value::none())
    }

    fn main(#[positional_only] _fd: PyIndex) -> Result<Value> {
        Ok(Value::none())
    }
}
