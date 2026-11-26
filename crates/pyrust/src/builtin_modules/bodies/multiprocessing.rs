// `multiprocessing` root package — 100% native-Rust Python 3.12 facade
// re-exporting the exact CPython 3.12 `__all__` API surface and binding all
// `multiprocessing.*` submodules (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::mp_rt::call_pos;
use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::{PyModule, Value};

const CONTEXT_REEXPORTS: &[&str] = &[
    "Array",
    "AuthenticationError",
    "Barrier",
    "BoundedSemaphore",
    "BufferTooShort",
    "Condition",
    "Event",
    "JoinableQueue",
    "Lock",
    "Manager",
    "Pipe",
    "Pool",
    "Process",
    "ProcessError",
    "Queue",
    "RLock",
    "RawArray",
    "RawValue",
    "Semaphore",
    "SimpleQueue",
    "TimeoutError",
    "Value",
    "active_children",
    "allow_connection_pickling",
    "cpu_count",
    "current_process",
    "freeze_support",
    "get_all_start_methods",
    "get_context",
    "get_logger",
    "get_start_method",
    "log_to_stderr",
    "parent_process",
    "set_executable",
    "set_forkserver_preload",
    "set_start_method",
];

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let ctx_mod = interp.load_module("multiprocessing.context")?;
    let util_mod = interp.load_module("multiprocessing.util")?;
    let proc_mod = interp.load_module("multiprocessing.process")?;
    let red_mod = interp.load_module("multiprocessing.reduction")?;

    let def_ctx_cls = interp.get_attr(&ctx_mod, "DefaultContext")?;
    let def_ctx = call_pos(interp, def_ctx_cls, &[])?;

    {
        let mut m = module.borrow_mut();
        m.insert_attr("context".to_string(), ctx_mod.clone());
        m.insert_attr("process".to_string(), proc_mod);
        m.insert_attr("util".to_string(), util_mod.clone());
        m.insert_attr("reduction".to_string(), red_mod.clone());
        m.insert_attr("reducer".to_string(), red_mod);
        if let Ok(subdebug) = interp.get_attr(&util_mod, "SUBDEBUG") {
            m.insert_attr("SUBDEBUG".to_string(), subdebug);
        }
        if let Ok(subwarn) = interp.get_attr(&util_mod, "SUBWARNING") {
            m.insert_attr("SUBWARNING".to_string(), subwarn);
        }
    }

    for &name in CONTEXT_REEXPORTS {
        let val = if matches!(
            name,
            "ProcessError" | "BufferTooShort" | "TimeoutError" | "AuthenticationError" | "Process"
        ) {
            interp.get_attr(&ctx_mod, name)?
        } else {
            interp.get_attr(&def_ctx, name)?
        };
        module.borrow_mut().insert_attr(name.to_string(), val);
    }

    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("Array"),
            Value::string("AuthenticationError"),
            Value::string("Barrier"),
            Value::string("BoundedSemaphore"),
            Value::string("BufferTooShort"),
            Value::string("Condition"),
            Value::string("Event"),
            Value::string("JoinableQueue"),
            Value::string("Lock"),
            Value::string("Manager"),
            Value::string("Pipe"),
            Value::string("Pool"),
            Value::string("Process"),
            Value::string("ProcessError"),
            Value::string("Queue"),
            Value::string("RLock"),
            Value::string("RawArray"),
            Value::string("RawValue"),
            Value::string("Semaphore"),
            Value::string("SimpleQueue"),
            Value::string("TimeoutError"),
            Value::string("Value"),
            Value::string("active_children"),
            Value::string("allow_connection_pickling"),
            Value::string("cpu_count"),
            Value::string("current_process"),
            Value::string("freeze_support"),
            Value::string("get_all_start_methods"),
            Value::string("get_context"),
            Value::string("get_logger"),
            Value::string("get_start_method"),
            Value::string("log_to_stderr"),
            Value::string("parent_process"),
            Value::string("reducer"),
            Value::string("set_executable"),
            Value::string("set_forkserver_preload"),
            Value::string("set_start_method"),
        ]),
    }
}
