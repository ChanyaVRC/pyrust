//! Declarative macro generating all 26 public `asyncio.*` submodules
//! 100% natively in Rust (issue #3254).
//!
//! Every submodule re-exports exact identical class/function objects from
//! `asyncio` (`asyncio.streams.StreamReader is asyncio.StreamReader`, etc.),
//! plus submodule-specific constants and helpers (`asyncio.constants.*`,
//! `asyncio.log.logger`, `asyncio.format_helpers.*`, `asyncio.base_futures.*`,
//! `asyncio.base_tasks.*`, `asyncio.base_subprocess.*`,
//! `asyncio.staggered.staggered_race`, `asyncio.trsock.TransportSocket`,
//! `asyncio.mixins._LoopBoundMixin`, etc.).

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::asyncio_rt;
use crate::builtin_registry::BuiltinReg;
use crate::error::Result;
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr};
use crate::value::{PyModule, Value};

fn copy_selected_from_asyncio(
    interp: &mut Interpreter,
    names: &[&str],
    dest: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let asyncio_mod = interp.load_module("asyncio")?;
    for &name in names {
        if let Ok(v) = interp.get_attr(&asyncio_mod, name) {
            let mut dst = dest.borrow_mut();
            if dst.get_attr_value(name).is_none() {
                dst.insert_attr(name.to_string(), v);
            }
        }
    }
    Ok(())
}

fn set_all_tuple(dest: &Rc<RefCell<PyModule>>, names: &[&str]) {
    let all = Value::tuple(names.iter().map(|&n| Value::string(n)).collect());
    dest.borrow_mut().insert_attr("__all__".to_string(), all);
}

// ── Specialized submodule helpers ────────────────────────────────────────────

crate::pyrust_int_enum! {
    pub(crate) SendfileMode in "asyncio.constants" {
        UNSUPPORTED = 1,
        TRY_NATIVE = 2,
        FALLBACK = 3,
    }
}

fn format_args_and_kwargs_impl(
    interp: &mut Interpreter,
    args: &Value,
    kwargs: &Value,
) -> Result<String> {
    let mut items = Vec::new();
    if let Ok(list_v) = asyncio_rt::call_pos(
        interp,
        Value::builtin_function("list"),
        std::slice::from_ref(args),
    ) && let crate::value::ValueKind::List(vals) = list_v.kind()
    {
        for v in vals.iter() {
            items.push(render_value_repr(interp, v)?);
        }
    }
    if let Some(d) = kwargs.as_dict() {
        for (k, v) in d.iter() {
            let ks = match k {
                crate::value::PyKey::Str(s) => s.to_string(),
                _ => format!("{k:?}"),
            };
            let vr = render_value_repr(interp, v)?;
            items.push(format!("{ks}={vr}"));
        }
    }
    Ok(format!("({})", items.join(", ")))
}

fn format_callback_impl(
    interp: &mut Interpreter,
    func: &Value,
    args: &Value,
    kwargs: Option<&Value>,
    suffix: &str,
) -> Result<String> {
    let func_name = interp
        .get_attr(func, "__qualname__")
        .or_else(|_| interp.get_attr(func, "__name__"))
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| render_value_repr(interp, func).unwrap_or_else(|_| "<cb>".into()));
    let kw_val = kwargs
        .cloned()
        .unwrap_or_else(|| Value::dict(crate::value::PyDict::default()));
    let formatted = format_args_and_kwargs_impl(interp, args, &kw_val)?;
    Ok(format!("{func_name}{formatted}{suffix}"))
}

// ── Macro defining all 26 `asyncio.*` submodules ─────────────────────────────

macro_rules! define_asyncio_reexport_submodules {
    (
        $(
            $mod_ident:ident => (
                $py_name:literal,
                all: [$($all_name:literal),* $(,)?],
                extra: [$($extra_name:literal),* $(,)?],
                init($interp:ident, $module:ident) $init_body:block
            )
        ),* $(,)?
    ) => {
        $(
            pub mod $mod_ident {
                use super::*;

                pub(crate) const MODULE_NAME: &str = $py_name;
                #[allow(dead_code)]
                pub(crate) const FN_PREFIX: &str = concat!($py_name, ".");

                pyrust_module! {
                    constants {
                        "__package__" => Value::string("asyncio"),
                    }
                }

                pub(crate) fn post_load_init(
                    $interp: &mut Interpreter,
                    $module: &Rc<RefCell<PyModule>>,
                ) -> Result<()> {
                    const ALL_NAMES: &[&str] = &[$($all_name),*];
                    const EXTRA_NAMES: &[&str] = &[$($extra_name),*];
                    copy_selected_from_asyncio($interp, ALL_NAMES, $module)?;
                    copy_selected_from_asyncio($interp, EXTRA_NAMES, $module)?;
                    if !ALL_NAMES.is_empty() {
                        set_all_tuple($module, ALL_NAMES);
                    }
                    $init_body
                }
            }
        )*

        pub(crate) fn all_regs() -> Vec<BuiltinReg> {
            let mut all = Vec::new();
            $(
                all.extend_from_slice($mod_ident::regs());
            )*
            all.extend_from_slice(asyncio_format_helpers::regs());
            all.extend_from_slice(asyncio_base_futures::regs());
            all.extend_from_slice(asyncio_base_tasks::regs());
            all.extend_from_slice(asyncio_staggered::regs());
            all
        }

        pub(crate) fn load_builtin_module(name: &str) -> Option<Value> {
            match name {
                $(
                    $py_name => Some($mod_ident::module()),
                )*
                "asyncio.format_helpers" => Some(asyncio_format_helpers::module()),
                "asyncio.base_futures" => Some(asyncio_base_futures::module()),
                "asyncio.base_tasks" => Some(asyncio_base_tasks::module()),
                "asyncio.staggered" => Some(asyncio_staggered::module()),
                _ => None,
            }
        }

        pub(crate) fn is_builtin_module(name: &str) -> bool {
            matches!(
                name,
                $($py_name)|*
                    | "asyncio.format_helpers"
                    | "asyncio.base_futures"
                    | "asyncio.base_tasks"
                    | "asyncio.staggered"
            )
        }

        pub(crate) fn post_load_init(
            name: &str,
            interp: &mut Interpreter,
            module: &Rc<RefCell<PyModule>>,
        ) -> Result<bool> {
            match name {
                $(
                    $py_name => {
                        $mod_ident::post_load_init(interp, module)?;
                        Ok(true)
                    }
                )*
                "asyncio.format_helpers" => {
                    asyncio_format_helpers::post_load_init(interp, module)?;
                    Ok(true)
                }
                "asyncio.base_futures" => {
                    asyncio_base_futures::post_load_init(interp, module)?;
                    Ok(true)
                }
                "asyncio.base_tasks" => {
                    asyncio_base_tasks::post_load_init(interp, module)?;
                    Ok(true)
                }
                "asyncio.staggered" => {
                    asyncio_staggered::post_load_init(interp, module)?;
                    Ok(true)
                }
                _ => Ok(false),
            }
        }
    };
}

// ── Submodules with custom native functions ──────────────────────────────────

pub mod asyncio_format_helpers {
    use super::*;

    pub(crate) const MODULE_NAME: &str = "asyncio.format_helpers";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "asyncio.format_helpers.";

    pyrust_module! {
        constants {
            "__package__" => Value::string("asyncio"),
        }

        fn _format_args_and_kwargs(args: PyValue, kwargs: PyValue) -> Result<Value> {
            Ok(Value::string(format_args_and_kwargs_impl(_interp, &args.0, &kwargs.0)?))
        }

        fn _format_callback(
            func: PyValue,
            args: PyValue,
            kwargs: OptValueRef<'_>,
            #[default("".into())] suffix: crate::interpreter::builtin_args::PyStr<'_>,
        ) -> Result<Value> {
            let kw = kwargs.as_value().filter(|v| !v.is_none());
            Ok(Value::string(format_callback_impl(
                _interp, &func.0, &args.0, kw, &suffix.0,
            )?))
        }

        fn _format_callback_source(func: PyValue, args: PyValue) -> Result<Value> {
            Ok(Value::string(format_callback_impl(
                _interp, &func.0, &args.0, None, "",
            )?))
        }

        fn _get_function_source(_func: PyValue) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn extract_stack(
            #[default(OptionalArg::Omitted)] f: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] limit: OptValueRef<'_>,
        ) -> Result<Value> {
            let tb_mod = _interp.load_module("traceback")?;
            let es = _interp.get_attr(&tb_mod, "extract_stack")?;
            let f_val = f.as_value().cloned().unwrap_or_else(Value::none);
            let lim_val = limit.as_value().cloned().unwrap_or_else(Value::none);
            asyncio_rt::call_pos(_interp, es, &[f_val, lim_val])
        }
    }

    pub(crate) fn post_load_init(
        _interp: &mut Interpreter,
        _module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        Ok(())
    }
}

pub mod asyncio_base_futures {
    use super::*;

    pub(crate) const MODULE_NAME: &str = "asyncio.base_futures";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "asyncio.base_futures.";

    pyrust_module! {
        constants {
            "__package__" => Value::string("asyncio"),
            "_PENDING" => Value::string("PENDING"),
            "_CANCELLED" => Value::string("CANCELLED"),
            "_FINISHED" => Value::string("FINISHED"),
        }

        fn _future_repr_info(future: PyValue) -> Result<Value> {
            let st = _interp
                .get_attr(&future.0, "_state")
                .ok()
                .and_then(|v| v.as_str().map(str::to_lowercase))
                .unwrap_or_else(|| "pending".to_string());
            Ok(Value::list(vec![Value::string(st)]))
        }

        fn _future_repr(future: PyValue) -> Result<Value> {
            let r = render_value_repr(_interp, &future.0)?;
            Ok(Value::string(r))
        }

        fn _format_callbacks(cb: PyValue) -> Result<Value> {
            let r = render_value_repr(_interp, &cb.0)?;
            Ok(Value::string(format!("cb=[{r}]")))
        }
    }

    pub(crate) fn post_load_init(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_selected_from_asyncio(interp, &["isfuture"], module)
    }
}

pub mod asyncio_base_tasks {
    use super::*;

    pub(crate) const MODULE_NAME: &str = "asyncio.base_tasks";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "asyncio.base_tasks.";

    pyrust_module! {
        constants {
            "__package__" => Value::string("asyncio"),
        }

        fn _task_repr_info(task: PyValue) -> Result<Value> {
            let r = render_value_repr(_interp, &task.0)?;
            Ok(Value::list(vec![Value::string(r)]))
        }

        fn _task_repr(task: PyValue) -> Result<Value> {
            let r = render_value_repr(_interp, &task.0)?;
            Ok(Value::string(r))
        }

        fn _task_get_stack(
            task: PyValue,
            #[default(OptionalArg::Omitted)] limit: OptValueRef<'_>,
        ) -> Result<Value> {
            let lim = limit.as_value().cloned().unwrap_or_else(Value::none);
            asyncio_rt::call_method_kw(_interp, &task.0, "get_stack", &[], &[("limit", lim)])
        }

        fn _task_print_stack(
            task: PyValue,
            #[default(OptionalArg::Omitted)] limit: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] file: OptValueRef<'_>,
        ) -> Result<Value> {
            let lim = limit.as_value().cloned().unwrap_or_else(Value::none);
            let f = file.as_value().cloned().unwrap_or_else(Value::none);
            asyncio_rt::call_method_kw(
                _interp,
                &task.0,
                "print_stack",
                &[],
                &[("limit", lim), ("file", f)],
            )
        }
    }

    pub(crate) fn post_load_init(
        _interp: &mut Interpreter,
        _module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        Ok(())
    }
}

pub mod asyncio_staggered {
    use super::*;

    pub(crate) const MODULE_NAME: &str = "asyncio.staggered";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "asyncio.staggered.";

    pyrust_module! {
        constants {
            "__package__" => Value::string("asyncio"),
        }

        fn staggered_race(
            coro_fns: PyValue,
            delay: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>,
        ) -> Result<Value> {
            let helper = asyncio_rt::asyncio_attr(_interp, "_staggered_race")?;
            let d = delay.as_value().cloned().unwrap_or_else(Value::none);
            let lp = r#loop.as_value().cloned().unwrap_or_else(Value::none);
            asyncio_rt::call_kw(_interp, helper, &[coro_fns.0, d], &[("loop", lp)])
        }
    }

    pub(crate) fn post_load_init(
        _interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        set_all_tuple(module, &["staggered_race"]);
        Ok(())
    }
}

define_asyncio_reexport_submodules! {
    asyncio_exceptions => (
        "asyncio.exceptions",
        all: [
            "BrokenBarrierError",
            "CancelledError",
            "InvalidStateError",
            "TimeoutError",
            "IncompleteReadError",
            "LimitOverrunError",
            "SendfileNotAvailableError",
        ],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_streams => (
        "asyncio.streams",
        all: [
            "StreamReader",
            "StreamWriter",
            "StreamReaderProtocol",
            "open_connection",
            "start_server",
        ],
        extra: [],
        init(_interp, module) {
            module.borrow_mut().insert_attr(
                "_DEFAULT_LIMIT".to_string(),
                Value::int(asyncio_rt::DEFAULT_STREAM_LIMIT as i64),
            );
            Ok(())
        }
    ),
    asyncio_tasks => (
        "asyncio.tasks",
        all: [
            "Task",
            "create_task",
            "FIRST_COMPLETED",
            "FIRST_EXCEPTION",
            "ALL_COMPLETED",
            "wait",
            "wait_for",
            "as_completed",
            "sleep",
            "gather",
            "shield",
            "ensure_future",
            "run_coroutine_threadsafe",
            "current_task",
            "all_tasks",
            "create_eager_task_factory",
            "eager_task_factory",
            "_register_task",
            "_unregister_task",
            "_enter_task",
            "_leave_task",
        ],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_futures => (
        "asyncio.futures",
        all: ["Future", "wrap_future", "isfuture"],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_locks => (
        "asyncio.locks",
        all: ["Lock", "Event", "Condition", "Semaphore", "BoundedSemaphore", "Barrier"],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_queues => (
        "asyncio.queues",
        all: ["Queue", "PriorityQueue", "LifoQueue", "QueueFull", "QueueEmpty"],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_runners => (
        "asyncio.runners",
        all: ["Runner", "run"],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_events => (
        "asyncio.events",
        all: [
            "AbstractEventLoopPolicy",
            "AbstractEventLoop",
            "AbstractServer",
            "Handle",
            "TimerHandle",
            "get_event_loop_policy",
            "set_event_loop_policy",
            "get_event_loop",
            "set_event_loop",
            "new_event_loop",
            "get_child_watcher",
            "set_child_watcher",
            "_set_running_loop",
            "get_running_loop",
            "_get_running_loop",
        ],
        extra: ["BaseDefaultEventLoopPolicy", "DefaultEventLoopPolicy"],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_protocols => (
        "asyncio.protocols",
        all: [
            "BaseProtocol",
            "Protocol",
            "DatagramProtocol",
            "SubprocessProtocol",
            "BufferedProtocol",
        ],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_transports => (
        "asyncio.transports",
        all: [
            "BaseTransport",
            "ReadTransport",
            "WriteTransport",
            "Transport",
            "DatagramTransport",
            "SubprocessTransport",
        ],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_subprocess => (
        "asyncio.subprocess",
        all: ["create_subprocess_exec", "create_subprocess_shell"],
        extra: ["PIPE", "STDOUT", "DEVNULL", "Process", "SubprocessStreamProtocol"],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_timeouts => (
        "asyncio.timeouts",
        all: ["Timeout", "timeout", "timeout_at"],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_taskgroups => (
        "asyncio.taskgroups",
        all: ["TaskGroup"],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_coroutines => (
        "asyncio.coroutines",
        all: ["iscoroutinefunction", "iscoroutine"],
        extra: [],
        init(interp, module) {
            let sentinel = asyncio_rt::is_coro_sentinel(interp)?;
            module
                .borrow_mut()
                .insert_attr("_is_coroutine".to_string(), sentinel);
            Ok(())
        }
    ),
    asyncio_base_events => (
        "asyncio.base_events",
        all: ["BaseEventLoop", "Server"],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_selector_events => (
        "asyncio.selector_events",
        all: ["BaseSelectorEventLoop"],
        extra: ["SelectorEventLoop"],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_constants => (
        "asyncio.constants",
        all: [],
        extra: [],
        init(interp, module) {
            let mut m = module.borrow_mut();
            m.insert_attr("LOG_THRESHOLD_FOR_CONNLOST_WRITES".to_string(), Value::int(5));
            m.insert_attr("ACCEPT_RETRY_DELAY".to_string(), Value::int(1));
            m.insert_attr("DEBUG_STACK_DEPTH".to_string(), Value::int(10));
            m.insert_attr("SSL_HANDSHAKE_TIMEOUT".to_string(), Value::float(60.0));
            m.insert_attr("SSL_SHUTDOWN_TIMEOUT".to_string(), Value::float(30.0));
            m.insert_attr("SENDFILE_FALLBACK_READBUFFER_SIZE".to_string(), Value::int(262_144));
            m.insert_attr("FLOW_CONTROL_HIGH_WATER_SSL_READ".to_string(), Value::int(256));
            m.insert_attr("FLOW_CONTROL_HIGH_WATER_SSL_WRITE".to_string(), Value::int(512));
            m.insert_attr("THREAD_JOIN_TIMEOUT".to_string(), Value::int(300));
            drop(m);
            let mod_val = Value::py_module(Rc::clone(module));
            SendfileMode::install(interp, &mod_val)?;
            if let Ok(sfm) = interp.get_attr(&mod_val, "SendfileMode") {
                module.borrow_mut().insert_attr("_SendfileMode".to_string(), sfm);
            }
            Ok(())
        }
    ),
    asyncio_log => (
        "asyncio.log",
        all: [],
        extra: [],
        init(interp, module) {
            let logging_mod = interp.load_module("logging")?;
            let get_logger = interp.get_attr(&logging_mod, "getLogger")?;
            let logger = asyncio_rt::call_pos(interp, get_logger, &[Value::string("asyncio")])?;
            module.borrow_mut().insert_attr("logger".to_string(), logger);
            Ok(())
        }
    ),
    asyncio_mixins => (
        "asyncio.mixins",
        all: [],
        extra: ["_LoopBoundMixin"],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_threads => (
        "asyncio.threads",
        all: ["to_thread"],
        extra: [],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_trsock => (
        "asyncio.trsock",
        all: [],
        extra: ["TransportSocket"],
        init(_interp, _module) { Ok(()) }
    ),
    asyncio_base_subprocess => (
        "asyncio.base_subprocess",
        all: [],
        extra: [],
        init(interp, module) {
            let asyncio_mod = interp.load_module("asyncio")?;
            if let Ok(cls) = interp.get_attr(&asyncio_mod, "_SubprocessTransport") {
                module
                    .borrow_mut()
                    .insert_attr("BaseSubprocessTransport".to_string(), cls);
            }
            if let Ok(cls) = interp.get_attr(&asyncio_mod, "_SubprocessWritePipeTransport") {
                module
                    .borrow_mut()
                    .insert_attr("WriteSubprocessPipeProto".to_string(), cls);
            }
            if let Ok(cls) = interp.get_attr(&asyncio_mod, "_SubprocessReadPipeTransport") {
                module
                    .borrow_mut()
                    .insert_attr("ReadSubprocessPipeProto".to_string(), cls);
            }
            Ok(())
        }
    ),
}
