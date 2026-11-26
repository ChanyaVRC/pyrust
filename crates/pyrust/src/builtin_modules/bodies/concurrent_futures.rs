// `concurrent.futures` package module — 100% native-Rust Python 3.12 high-level
// asynchronous execution facade (issue #3263).
//
// Re-exports the exact classes and functions from `concurrent.futures._base`,
// `concurrent.futures.thread`, and `concurrent.futures.process` so object
// identity matches CPython 3.12.
//
// Reference: <https://docs.python.org/3.12/library/concurrent.futures.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::{PyModule, Value};

const BASE_EXPORTS: &[&str] = &[
    "FIRST_COMPLETED",
    "FIRST_EXCEPTION",
    "ALL_COMPLETED",
    "CancelledError",
    "TimeoutError",
    "InvalidStateError",
    "BrokenExecutor",
    "Future",
    "Executor",
    "wait",
    "as_completed",
];

const THREAD_EXPORTS: &[&str] = &["ThreadPoolExecutor", "BrokenThreadPool"];
const PROCESS_EXPORTS: &[&str] = &["ProcessPoolExecutor", "BrokenProcessPool"];

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let base_mod = interp.load_module("concurrent.futures._base")?;
    let thread_mod = interp.load_module("concurrent.futures.thread")?;
    let process_mod = interp.load_module("concurrent.futures.process")?;

    {
        let mut m = module.borrow_mut();
        m.insert_attr("_base".to_string(), base_mod.clone());
        m.insert_attr("thread".to_string(), thread_mod.clone());
        m.insert_attr("process".to_string(), process_mod.clone());
    }

    for &name in BASE_EXPORTS {
        let val = interp.get_attr(&base_mod, name)?;
        module.borrow_mut().insert_attr(name.to_string(), val);
    }
    for &name in THREAD_EXPORTS {
        let val = interp.get_attr(&thread_mod, name)?;
        module.borrow_mut().insert_attr(name.to_string(), val);
    }
    for &name in PROCESS_EXPORTS {
        let val = interp.get_attr(&process_mod, name)?;
        module.borrow_mut().insert_attr(name.to_string(), val);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("concurrent.futures"),
        "__all__" => Value::tuple(vec![
            Value::string("FIRST_COMPLETED"),
            Value::string("FIRST_EXCEPTION"),
            Value::string("ALL_COMPLETED"),
            Value::string("CancelledError"),
            Value::string("TimeoutError"),
            Value::string("InvalidStateError"),
            Value::string("BrokenExecutor"),
            Value::string("Future"),
            Value::string("Executor"),
            Value::string("wait"),
            Value::string("as_completed"),
            Value::string("ProcessPoolExecutor"),
            Value::string("ThreadPoolExecutor"),
        ]),
    }
}
