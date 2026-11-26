// `multiprocessing.context` — 100% native-Rust Python 3.12 start-method
// contexts (`BaseContext`, `SpawnContext`, `ForkContext`, `ForkServerContext`,
// `DefaultContext`, `Process`, `ProcessError`, `BufferTooShort`,
// `TimeoutError`, `AuthenticationError`) (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::builtin_modules::mp_rt::{
    MP_STATE, call_pos, call_pos_kw, cpu_count_value, get_start_method_impl,
    platform_all_start_methods, set_start_method_impl,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyBool, PyStr, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let proc_mod = interp.load_module("multiprocessing.process")?;
    let base_proc = interp.get_attr(&proc_mod, "BaseProcess")?;
    let ValueKind::PyClass(base_rc) = base_proc.kind() else {
        return Ok(());
    };
    for cname in ["Process", "SpawnProcess"] {
        if let Some(cls_val) = module.borrow().get_attr_value(cname)
            && let ValueKind::PyClass(cls_rc) = cls_val.kind()
        {
            link_class_bases(cls_rc, vec![Rc::clone(base_rc)]);
        }
    }
    #[cfg(unix)]
    for cname in ["ForkProcess", "ForkServerProcess"] {
        if let Some(cls_val) = module.borrow().get_attr_value(cname)
            && let ValueKind::PyClass(cls_rc) = cls_val.kind()
        {
            link_class_bases(cls_rc, vec![Rc::clone(base_rc)]);
        }
    }
    if let Some(base_ctx) = module.borrow().get_attr_value("BaseContext")
        && let ValueKind::PyClass(ctx_rc) = base_ctx.kind()
    {
        for exc_name in [
            "ProcessError",
            "BufferTooShort",
            "TimeoutError",
            "AuthenticationError",
        ] {
            if let Some(exc_cls) = module.borrow().get_attr_value(exc_name) {
                ctx_rc
                    .borrow_mut()
                    .attrs
                    .insert(exc_name.to_string(), exc_cls);
            }
        }
    }
    Ok(())
}

fn call_mod_ctor(
    interp: &mut Interpreter,
    mod_name: &str,
    attr_name: &str,
    args: &[Value],
    kwargs: IndexMap<String, Value>,
) -> Result<Value> {
    let m = interp.load_module(mod_name)?;
    let ctor = interp.get_attr(&m, attr_name)?;
    let kw: Vec<(String, Value)> = kwargs.into_iter().collect();
    call_pos_kw(interp, ctor, args, &kw)
}

fn get_context_instance(interp: &mut Interpreter, method: Option<&str>) -> Result<Value> {
    let m_name = match method {
        Some(s) => s.to_string(),
        None => get_start_method_impl(false).unwrap_or_else(|| {
            crate::builtin_modules::mp_rt::platform_default_start_method().to_string()
        }),
    };
    let ctx_mod = interp.load_module("multiprocessing.context")?;
    let cls_name = match m_name.as_str() {
        "spawn" => "SpawnContext",
        #[cfg(unix)]
        "fork" => "ForkContext",
        #[cfg(unix)]
        "forkserver" => "ForkServerContext",
        other => {
            return Err(PyError::named(
                "ValueError",
                format!("cannot find context for '{other}'"),
            ));
        }
    };
    let cls = interp.get_attr(&ctx_mod, cls_name)?;
    call_pos(interp, cls, &[])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::tuple(Vec::new()),
    }

    exception ProcessError;
    exception BufferTooShort(ProcessError);
    exception TimeoutError(ProcessError);
    exception AuthenticationError(ProcessError);

    fn assert_spawning(#[positional_only] _obj: PyValueRef<'_>) -> Result<Value> {
        Ok(Value::none())
    }

    fn get_spawning_popen() -> Result<Value> {
        Ok(Value::none())
    }

    fn set_spawning_popen(#[positional_only] _popen: PyValueRef<'_>) -> Result<Value> {
        Ok(Value::none())
    }

    class Process {
        constants {
            "_start_method" => Value::none(),
        }
    }

    class SpawnProcess {
        constants {
            "_start_method" => Value::string("spawn"),
        }
    }

    #[cfg(unix)]
    class ForkProcess {
        constants {
            "_start_method" => Value::string("fork"),
        }
    }

    #[cfg(unix)]
    class ForkServerProcess {
        constants {
            "_start_method" => Value::string("forkserver"),
        }
    }

    class BaseContext {
        constants {
            "ProcessError" => Value::none(),
            "BufferTooShort" => Value::none(),
            "TimeoutError" => Value::none(),
            "AuthenticationError" => Value::none(),
        }

        fn cpu_count(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::int(cpu_count_value()?))
        }

        fn current_process(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.process")?;
            let f = _interp.get_attr(&m, "current_process")?;
            call_pos(_interp, f, &[])
        }

        fn parent_process(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.process")?;
            let f = _interp.get_attr(&m, "parent_process")?;
            call_pos(_interp, f, &[])
        }

        fn active_children(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.process")?;
            let f = _interp.get_attr(&m, "active_children")?;
            call_pos(_interp, f, &[])
        }

        fn freeze_support(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn set_executable(
            #[positional_only] _self: PyValueRef<'_>,
            #[positional_only] exe: PyValueRef<'_>,
        ) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.spawn")?;
            let f = _interp.get_attr(&m, "set_executable")?;
            call_pos(_interp, f, std::slice::from_ref(exe.0))
        }

        fn get_logger(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.util")?;
            let f = _interp.get_attr(&m, "get_logger")?;
            call_pos(_interp, f, &[])
        }

        fn log_to_stderr(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] level: OptValueRef<'_>,
        ) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.util")?;
            let f = _interp.get_attr(&m, "log_to_stderr")?;
            match level.as_value() {
                Some(v) => call_pos(_interp, f, std::slice::from_ref(v)),
                None => call_pos(_interp, f, &[]),
            }
        }

        fn allow_connection_pickling(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn set_forkserver_preload(
            #[positional_only] _self: PyValueRef<'_>,
            #[positional_only] module_names: PyValueRef<'_>,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(module_names.0)?;
            let mut names = Vec::with_capacity(items.len());
            for it in items {
                let s = it
                    .as_str()
                    .ok_or_else(|| PyError::named("TypeError", "module names must be strings"))?;
                names.push(s.to_string());
            }
            MP_STATE.with(|s| {
                s.borrow_mut().forkserver_preload = names;
            });
            Ok(Value::none())
        }

        fn get_all_start_methods(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::list(
                platform_all_start_methods()
                    .iter()
                    .map(Value::string)
                    .collect(),
            ))
        }

        fn get_start_method(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(false.into())] allow_none: PyBool,
        ) -> Result<Value> {
            if let Ok(sm) = _interp.get_attr(self_val.0, "_name")
                && !sm.is_none()
            {
                return Ok(sm);
            }
            Ok(get_start_method_impl(allow_none.0)
                .map(|s| Value::string(&s))
                .unwrap_or_else(Value::none))
        }

        fn set_start_method(
            #[positional_only] _self: PyValueRef<'_>,
            #[positional_only] method: PyStr<'_>,
            #[default(false.into())] force: PyBool,
        ) -> Result<Value> {
            set_start_method_impl(&method.0, force.0)?;
            Ok(Value::none())
        }

        fn get_context(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] method: OptValueRef<'_>,
        ) -> Result<Value> {
            match method.as_value() {
                None => Ok(self_val.0.clone()),
                Some(v) if v.is_none() => get_context_instance(_interp, None),
                Some(v) => {
                    let s = v
                        .as_str()
                        .ok_or_else(|| PyError::named("TypeError", "method must be str or None"))?;
                    get_context_instance(_interp, Some(s))
                }
            }
        }

        #[py_name = "Pipe"]
        fn pipe(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(true.into())] duplex: PyBool,
        ) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.connection")?;
            let f = _interp.get_attr(&m, "Pipe")?;
            call_pos(_interp, f, &[Value::bool_(duplex.0)])
        }

        #[py_name = "Queue"]
        fn queue(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.queues", "Queue", args, kwargs)
        }

        #[py_name = "SimpleQueue"]
        fn simple_queue(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.queues", "SimpleQueue", args, kwargs)
        }

        #[py_name = "JoinableQueue"]
        fn joinable_queue(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.queues", "JoinableQueue", args, kwargs)
        }

        #[py_name = "Lock"]
        fn lock(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.synchronize", "Lock", &[], IndexMap::new())
        }

        #[py_name = "RLock"]
        fn rlock(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.synchronize", "RLock", &[], IndexMap::new())
        }

        #[py_name = "Semaphore"]
        fn semaphore(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.synchronize", "Semaphore", args, kwargs)
        }

        #[py_name = "BoundedSemaphore"]
        fn bounded_semaphore(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(
                _interp,
                "multiprocessing.synchronize",
                "BoundedSemaphore",
                args,
                kwargs,
            )
        }

        #[py_name = "Condition"]
        fn condition(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.synchronize", "Condition", args, kwargs)
        }

        #[py_name = "Event"]
        fn event(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.synchronize", "Event", &[], IndexMap::new())
        }

        #[py_name = "Barrier"]
        fn barrier(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.synchronize", "Barrier", args, kwargs)
        }

        #[py_name = "RawValue"]
        fn raw_value(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.sharedctypes", "RawValue", args, kwargs)
        }

        #[py_name = "RawArray"]
        fn raw_array(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.sharedctypes", "RawArray", args, kwargs)
        }

        #[py_name = "Value"]
        fn value(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.sharedctypes", "Value", args, kwargs)
        }

        #[py_name = "Array"]
        fn array(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_mod_ctor(_interp, "multiprocessing.sharedctypes", "Array", args, kwargs)
        }

        #[py_name = "Manager"]
        fn manager(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.managers")?;
            let cls = _interp.get_attr(&m, "SyncManager")?;
            let mgr = call_pos(_interp, cls, &[])?;
            let start_fn = _interp.get_attr(&mgr, "start")?;
            call_pos(_interp, start_fn, &[])?;
            Ok(mgr)
        }

        #[py_name = "Pool"]
        fn pool(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut kwargs = kwargs;
            kwargs
                .entry("context".to_string())
                .or_insert_with(|| self_val.0.clone());
            call_mod_ctor(_interp, "multiprocessing.pool", "Pool", args, kwargs)
        }
    }

    class SpawnContext : "BaseContext" {
        constants {
            "_name" => Value::string("spawn"),
        }

        #[getter]
        #[py_name = "Process"]
        fn process_cls(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.context")?;
            _interp.get_attr(&m, "SpawnProcess")
        }
    }

    #[cfg(unix)]
    class ForkContext : "BaseContext" {
        constants {
            "_name" => Value::string("fork"),
        }

        #[getter]
        #[py_name = "Process"]
        fn process_cls(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.context")?;
            _interp.get_attr(&m, "ForkProcess")
        }
    }

    #[cfg(unix)]
    class ForkServerContext : "BaseContext" {
        constants {
            "_name" => Value::string("forkserver"),
        }

        #[getter]
        #[py_name = "Process"]
        fn process_cls(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.context")?;
            _interp.get_attr(&m, "ForkServerProcess")
        }
    }

    class DefaultContext : "BaseContext" {
        constants {
            "_name" => Value::none(),
        }

        #[getter]
        #[py_name = "Process"]
        fn process_cls(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let m = _interp.load_module("multiprocessing.context")?;
            _interp.get_attr(&m, "Process")
        }
    }
}
