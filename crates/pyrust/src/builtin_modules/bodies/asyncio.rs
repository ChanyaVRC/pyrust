// `asyncio` module — 100% native-Rust Python 3.12 async I/O, streams,
// transports/protocols, `Runner`, subprocesses, TLS, and task/sync/queue
// primitives (issues #2281, #3154, #3254).
//
// ZERO `@inject`, ZERO `*_py.py` files.
//
// Reference: <https://docs.python.org/3.12/library/asyncio.html>

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::asyncio_rt::{
    self, ALL_COMPLETED, BarrierState, BarrierStatus, ConditionState, DEFAULT_STREAM_LIMIT,
    DEVNULL_SENTINEL, EventState, FIRST_COMPLETED, FIRST_EXCEPTION, FutureState, FutureStatus,
    LockState, PIPE_SENTINEL, ProcessState, QueueKind, QueueState, RunnerLifecycle, RunnerState,
    STDOUT_SENTINEL, SemaphoreState, StreamReaderProtocolState, StreamReaderState,
    StreamWriterState, SubprocessStreamProtocolState, TaskGroupState, TimeoutState, TimeoutStatus,
    awaitable::{
        self, AsCompletedState, BarrierWaitPhase, CondWaitPhase, ConstFactoryState, CoroBody,
        CreateConnPhase, EagerTaskFactoryState, ExecutorThunkState, GatherDoneCbState,
        NativeCoroState, OpenConnPhase, ProcCommPhase, ReleaseWaiterCbState, ShieldInnerCbState,
        ShieldOuterCbState, StaggeredPhase, StreamServerProtoFactoryState, WaitCallbackState,
        WaitForPhase, WrapFutureCbState,
    },
    io_engine::{
        self, DatagramTransportState, HandleState, LoopState, PipeReadTransportState,
        PipeWriteTransportState, ServerState, SocketTransportState, SubprocessTransportState,
    },
};
use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state, with_native_state_mut,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyNumber, PyPath, PyStr,
    PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, lookup_exc_class, render_value_repr, value_type_name_str,
};
use crate::value::{PyKey, PyModule, Value, ValueKind};

// ── Helper state for EventLoopPolicy & TransportSocket ───────────────────────

struct DefaultPolicyState {
    local_loop: Option<Value>,
    set_called: bool,
}

struct TransportSocketState {
    sock: Value,
}

struct AsCompletedIterState {
    queue: Value,
    remaining: usize,
}

struct BaseTransportExtraState {
    extra: IndexMap<String, Value>,
    protocol: Option<Value>,
    closing: bool,
}

fn opt_none_or_str(val: OptValueRef<'_>) -> Result<Option<String>> {
    match val.as_value() {
        None => Ok(None),
        Some(v) if v.is_none() => Ok(None),
        Some(v) => Ok(Some(
            v.as_str()
                .ok_or_else(|| {
                    PyError::named(
                        "TypeError",
                        format!("expected str or None, got {}", value_type_name_str(v)),
                    )
                })?
                .to_owned(),
        )),
    }
}

fn opt_none_or_i64(
    interp: &mut Interpreter,
    val: OptValueRef<'_>,
    fn_name: &str,
    arg: &str,
) -> Result<Option<i64>> {
    match val.as_value() {
        None => Ok(None),
        Some(v) if v.is_none() => Ok(None),
        Some(v) => {
            let idx = PyIndex::try_from_value_with_interp(interp, v, fn_name, arg)?;
            Ok(Some(idx.expect_i64(fn_name, arg)?))
        }
    }
}

fn opt_none_or_f64(val: OptValueRef<'_>, fn_name: &str, arg: &str) -> Result<Option<f64>> {
    match val.as_value() {
        None => Ok(None),
        Some(v) if v.is_none() => Ok(None),
        Some(v) => Ok(Some(asyncio_rt::value_to_f64(v, fn_name, arg)?)),
    }
}

fn opt_none_or_bool(interp: &mut Interpreter, val: OptValueRef<'_>) -> Result<Option<bool>> {
    match val.as_value() {
        None => Ok(None),
        Some(v) if v.is_none() => Ok(None),
        Some(v) => Ok(Some(interp.truthy_value(v)?)),
    }
}

fn opt_value_cloned(val: OptValueRef<'_>) -> Option<Value> {
    match val.as_value() {
        None => None,
        Some(v) if v.is_none() => None,
        Some(v) => Some(v.clone()),
    }
}

fn value_to_str_arg(interp: &mut Interpreter, val: &Value) -> Result<String> {
    if let Some(s) = val.as_str() {
        return Ok(s.to_owned());
    }
    if let ValueKind::Bytes(b) = val.kind() {
        return Ok(String::from_utf8_lossy(b).into_owned());
    }
    let p = PyPath::try_from_value_with_interp(interp, val, "subprocess", "arg")?;
    Ok(p.as_path().to_string_lossy().into_owned())
}

fn collect_iterable_values(interp: &mut Interpreter, iterable: &Value) -> Result<Vec<Value>> {
    let list_val = asyncio_rt::call_pos(
        interp,
        Value::builtin_function("list"),
        std::slice::from_ref(iterable),
    )?;
    match list_val.kind() {
        ValueKind::List(items) => Ok(items.clone()),
        _ => Err(PyError::named("TypeError", "object is not iterable")),
    }
}

fn cancel_all_tasks_on_loop(interp: &mut Interpreter, loop_val: &Value) -> Result<()> {
    let to_cancel = asyncio_rt::all_tasks_for_loop(loop_val);
    if to_cancel.is_empty() {
        return Ok(());
    }
    for t in &to_cancel {
        let _ = asyncio_rt::future_cancel(interp, t, None);
    }
    let gather_fn = asyncio_rt::asyncio_attr(interp, "gather")?;
    let gather_fut = asyncio_rt::call_kw(
        interp,
        gather_fn,
        &to_cancel,
        &[("return_exceptions", Value::bool_(true))],
    )?;
    let _ = asyncio_rt::call_method(interp, loop_val, "run_until_complete", &[gather_fut]);
    Ok(())
}

// ── Public `__all__` and post-load class `__module__` / submodule wiring ─────

pub(crate) const ASYNCIO_ALL: &[&str] = &[
    // base_events
    "BaseEventLoop",
    "Server",
    // coroutines
    "iscoroutinefunction",
    "iscoroutine",
    // events
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
    "DefaultEventLoopPolicy",
    "SelectorEventLoop",
    // exceptions
    "BrokenBarrierError",
    "CancelledError",
    "InvalidStateError",
    "TimeoutError",
    "IncompleteReadError",
    "LimitOverrunError",
    "SendfileNotAvailableError",
    // futures
    "Future",
    "wrap_future",
    "isfuture",
    // locks
    "Lock",
    "Event",
    "Condition",
    "Semaphore",
    "BoundedSemaphore",
    "Barrier",
    // protocols
    "BaseProtocol",
    "Protocol",
    "DatagramProtocol",
    "SubprocessProtocol",
    "BufferedProtocol",
    // runners
    "Runner",
    "run",
    // queues
    "Queue",
    "PriorityQueue",
    "LifoQueue",
    "QueueFull",
    "QueueEmpty",
    // streams
    "StreamReader",
    "StreamWriter",
    "StreamReaderProtocol",
    "open_connection",
    "start_server",
    // subprocess
    "create_subprocess_exec",
    "create_subprocess_shell",
    // tasks
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
    // taskgroups
    "TaskGroup",
    // threads
    "to_thread",
    // timeouts
    "Timeout",
    "timeout",
    "timeout_at",
    // transports
    "BaseTransport",
    "ReadTransport",
    "WriteTransport",
    "Transport",
    "DatagramTransport",
    "SubprocessTransport",
];

const CLASS_MODULES: &[(&str, &str)] = &[
    ("CancelledError", "asyncio.exceptions"),
    ("InvalidStateError", "asyncio.exceptions"),
    ("BrokenBarrierError", "asyncio.exceptions"),
    ("IncompleteReadError", "asyncio.exceptions"),
    ("LimitOverrunError", "asyncio.exceptions"),
    ("SendfileNotAvailableError", "asyncio.exceptions"),
    ("Future", "_asyncio"),
    ("Task", "_asyncio"),
    ("Timeout", "asyncio.timeouts"),
    ("TaskGroup", "asyncio.taskgroups"),
    ("Lock", "asyncio.locks"),
    ("Event", "asyncio.locks"),
    ("Condition", "asyncio.locks"),
    ("Semaphore", "asyncio.locks"),
    ("BoundedSemaphore", "asyncio.locks"),
    ("Barrier", "asyncio.locks"),
    ("Queue", "asyncio.queues"),
    ("LifoQueue", "asyncio.queues"),
    ("PriorityQueue", "asyncio.queues"),
    ("QueueEmpty", "asyncio.queues"),
    ("QueueFull", "asyncio.queues"),
    ("Handle", "asyncio.events"),
    ("TimerHandle", "asyncio.events"),
    ("AbstractEventLoop", "asyncio.events"),
    ("AbstractEventLoopPolicy", "asyncio.events"),
    ("AbstractServer", "asyncio.events"),
    ("BaseDefaultEventLoopPolicy", "asyncio.events"),
    ("DefaultEventLoopPolicy", "asyncio.events"),
    ("BaseEventLoop", "asyncio.base_events"),
    ("Server", "asyncio.base_events"),
    ("BaseSelectorEventLoop", "asyncio.selector_events"),
    ("SelectorEventLoop", "asyncio.selector_events"),
    ("BaseTransport", "asyncio.transports"),
    ("ReadTransport", "asyncio.transports"),
    ("WriteTransport", "asyncio.transports"),
    ("Transport", "asyncio.transports"),
    ("DatagramTransport", "asyncio.transports"),
    ("SubprocessTransport", "asyncio.transports"),
    ("BaseProtocol", "asyncio.protocols"),
    ("Protocol", "asyncio.protocols"),
    ("BufferedProtocol", "asyncio.protocols"),
    ("DatagramProtocol", "asyncio.protocols"),
    ("SubprocessProtocol", "asyncio.protocols"),
    ("StreamReader", "asyncio.streams"),
    ("StreamWriter", "asyncio.streams"),
    ("StreamReaderProtocol", "asyncio.streams"),
    ("Runner", "asyncio.runners"),
    ("Process", "asyncio.subprocess"),
    ("SubprocessStreamProtocol", "asyncio.subprocess"),
    ("TransportSocket", "asyncio.trsock"),
    ("_LoopBoundMixin", "asyncio.mixins"),
];

const EAGER_SUBMODULES: &[(&str, &str)] = &[
    ("constants", "asyncio.constants"),
    ("coroutines", "asyncio.coroutines"),
    ("exceptions", "asyncio.exceptions"),
    ("format_helpers", "asyncio.format_helpers"),
    ("log", "asyncio.log"),
    ("mixins", "asyncio.mixins"),
    ("base_futures", "asyncio.base_futures"),
    ("futures", "asyncio.futures"),
    ("protocols", "asyncio.protocols"),
    ("transports", "asyncio.transports"),
    ("trsock", "asyncio.trsock"),
    ("events", "asyncio.events"),
    ("base_tasks", "asyncio.base_tasks"),
    ("tasks", "asyncio.tasks"),
    ("locks", "asyncio.locks"),
    ("queues", "asyncio.queues"),
    ("timeouts", "asyncio.timeouts"),
    ("taskgroups", "asyncio.taskgroups"),
    ("streams", "asyncio.streams"),
    ("base_subprocess", "asyncio.base_subprocess"),
    ("subprocess", "asyncio.subprocess"),
    ("threads", "asyncio.threads"),
    ("base_events", "asyncio.base_events"),
    ("selector_events", "asyncio.selector_events"),
    ("runners", "asyncio.runners"),
];

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // 1. Tag every public class with its CPython 3.12 `__module__`.
    {
        let m = module.borrow();
        for &(cls_name, mod_name) in CLASS_MODULES {
            if let Some(val) = m.get_attr_value(cls_name)
                && let ValueKind::PyClass(cls_rc) = val.kind()
            {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string(mod_name));
            }
        }
    }
    // 2. Publish `__all__` as a Python tuple matching CPython 3.12.
    let all_tuple = Value::tuple(
        ASYNCIO_ALL
            .iter()
            .map(|&name| Value::string(name))
            .collect(),
    );
    module
        .borrow_mut()
        .insert_attr("__all__".to_string(), all_tuple);

    // 3. Load and attach standard `asyncio.*` submodules as attributes on `asyncio`.
    for &(attr_name, full_name) in EAGER_SUBMODULES {
        if let Ok(submod) = interp.load_module(full_name) {
            module
                .borrow_mut()
                .insert_attr(attr_name.to_string(), submod);
        }
    }
    Ok(())
}

// ── Native `asyncio` module definition ───────────────────────────────────────

pyrust_module! {
    constants {
        "__package__" => Value::string("asyncio"),
        "FIRST_COMPLETED" => Value::string(FIRST_COMPLETED),
        "FIRST_EXCEPTION" => Value::string(FIRST_EXCEPTION),
        "ALL_COMPLETED" => Value::string(ALL_COMPLETED),
        "TimeoutError" => Value::py_class(
            lookup_exc_class("TimeoutError").expect("TimeoutError class")
        ),
        "PIPE" => Value::int(PIPE_SENTINEL),
        "STDOUT" => Value::int(STDOUT_SENTINEL),
        "DEVNULL" => Value::int(DEVNULL_SENTINEL),
    }

    // ── Exceptions ───────────────────────────────────────────────────────────

    exception CancelledError(BaseException);
    exception InvalidStateError(Exception);
    exception BrokenBarrierError(RuntimeError);
    exception SendfileNotAvailableError(RuntimeError);
    exception QueueEmpty(Exception);
    exception QueueFull(Exception);

    class IncompleteReadError("EOFError") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            partial: PyValue,
            expected: PyValue,
        ) -> Result<Value> {
            let partial_len = match partial.0.kind() {
                ValueKind::Bytes(b) => b.len(),
                _ => {
                    let lv = asyncio_rt::call_pos(
                        _interp,
                        Value::builtin_function("len"),
                        std::slice::from_ref(&partial.0),
                    )?;
                    lv.as_int().unwrap_or(0) as usize
                }
            };
            let exp_str = if expected.0.is_none() {
                "undefined".to_string()
            } else {
                format!("{}", expected.0.as_int().unwrap_or(0))
            };
            let msg = format!(
                "{partial_len} bytes read on a total of {exp_str} expected bytes"
            );
            _interp.assign_attr(self_val.0.clone(), "partial", partial.0)?;
            _interp.assign_attr(self_val.0.clone(), "expected", expected.0)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "args",
                Value::tuple(vec![Value::string(msg)]),
            )?;
            Ok(Value::none())
        }
    }

    class LimitOverrunError("Exception") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            message: PyValue,
            consumed: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "consumed", consumed.0)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "args",
                Value::tuple(vec![message.0]),
            )?;
            Ok(Value::none())
        }
    }

    // ── Internal native coroutine / callback classes ─────────────────────────

    #[native_state(NativeCoroState)]
    class _NativeCoro {
        fn __await__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(self_val.0.clone())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let step = awaitable::step_native_coro(_interp, self_val.0, Value::none(), None);
            awaitable::coro_step_to_iter_result(step)
        }

        fn send(#[positional_only] self_val: PyValueRef<'_>, value: PyValue) -> Result<Value> {
            let step = awaitable::step_native_coro(_interp, self_val.0, value.0, None);
            awaitable::coro_step_to_iter_result(step)
        }

        fn throw(
            #[positional_only] self_val: PyValueRef<'_>,
            exc: PyValue,
            #[default(OptionalArg::Omitted)] _val: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _tb: OptValueRef<'_>,
        ) -> Result<Value> {
            let exc_inst = if matches!(exc.0.kind(), ValueKind::PyClass(_)) {
                asyncio_rt::call_pos(_interp, exc.0, &[])?
            } else {
                exc.0
            };
            let step = awaitable::step_native_coro(
                _interp,
                self_val.0,
                Value::none(),
                Some(PyError::raised(exc_inst)),
            );
            awaitable::coro_step_to_iter_result(step)
        }

        fn close(#[state_mut] st: &mut NativeCoroState) -> Result<Value> {
            let _ = _interp;
            st.done = true;
            st.body = None;
            Ok(Value::none())
        }
    }

    #[native_state(WaitCallbackState)]
    class _WaitCallback {
        fn __call__(#[positional_only] self_val: PyValueRef<'_>, f: PyValue) -> Result<Value> {
            let (should_fire, th, waiter) = with_state_mut(self_val.0, |st| {
                st.counter -= 1;
                let cancelled = with_native_state::<FutureState, _>(&f.0, "Future", |fs| {
                    Ok(fs.is_cancelled())
                })
                .unwrap_or(false);
                let has_exc = if !cancelled {
                    with_native_state::<FutureState, _>(&f.0, "Future", |fs| {
                        Ok(fs.exception.is_some())
                    })
                    .unwrap_or(false)
                } else {
                    false
                };
                let fire = st.counter <= 0
                    || st.return_when == FIRST_COMPLETED
                    || (st.return_when == FIRST_EXCEPTION && has_exc);
                Ok((fire, st.timeout_handle.clone(), st.waiter.clone()))
            })?;
            if should_fire {
                if let Some(h) = th {
                    let _ = asyncio_rt::call_method(_interp, &h, "cancel", &[]);
                }
                asyncio_rt::future_set_result_if_pending(_interp, &waiter, Value::none())?;
            }
            Ok(Value::none())
        }
    }

    #[native_state(ReleaseWaiterCbState)]
    class _ReleaseWaiterCb {
        fn __call__(
            #[state] st: &ReleaseWaiterCbState,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            asyncio_rt::future_set_result_if_pending(_interp, &st.waiter, Value::none())?;
            Ok(Value::none())
        }
    }

    #[native_state(ConstFactoryState)]
    class _ConstFactory {
        fn __call__(#[state] st: &ConstFactoryState) -> Result<Value> {
            let _ = _interp;
            Ok(st.val.clone())
        }
    }

    #[native_state(StreamServerProtoFactoryState)]
    class _StreamServerProtoFactory {
        fn __call__(#[state] st: &StreamServerProtoFactoryState) -> Result<Value> {
            let reader =
                awaitable::create_stream_reader(_interp, st.limit, Some(st.loop_val.clone()))?;
            awaitable::create_stream_reader_protocol(
                _interp,
                reader,
                Some(st.client_connected_cb.clone()),
                st.loop_val.clone(),
            )
        }
    }

    #[native_state(ExecutorThunkState)]
    class _ExecutorThunk {
        fn __call__(#[state] st: &ExecutorThunkState) -> Result<Value> {
            if st.kwargs.is_empty() {
                asyncio_rt::run_in_context(_interp, &st.context, &st.func, &st.args)
            } else {
                // Run func(*args, **kwargs) inside `st.context` via functools.partial.
                let functools = _interp.load_module("functools")?;
                let partial_cls = _interp.get_attr(&functools, "partial")?;
                let mut pos = Vec::with_capacity(1 + st.args.len());
                pos.push(st.func.clone());
                pos.extend(st.args.iter().cloned());
                let kw: Vec<(&str, Value)> = st
                    .kwargs
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.clone()))
                    .collect();
                let bound = asyncio_rt::call_kw(_interp, partial_cls, &pos, &kw)?;
                asyncio_rt::run_in_context(_interp, &st.context, &bound, &[])
            }
        }
    }

    #[native_state(GatherDoneCbState)]
    class _GatherDoneCb {
        fn __call__(#[state] st: &GatherDoneCbState, fut: PyValue) -> Result<Value> {
            let finished_now = {
                let mut n = st.nfinished.borrow_mut();
                *n += 1;
                *n
            };
            let outer_done = with_native_state::<FutureState, _>(&st.outer, "Future", |fs| {
                Ok(fs.is_done())
            })?;
            if outer_done {
                return Ok(Value::none());
            }
            if !st.return_exceptions {
                let is_canc = with_native_state::<FutureState, _>(&fut.0, "Future", |fs| {
                    Ok(fs.is_cancelled())
                })?;
                if is_canc {
                    let ce = asyncio_rt::future_make_cancelled_error(_interp, &fut.0)?;
                    asyncio_rt::future_set_exception(_interp, &st.outer, ce)?;
                    return Ok(Value::none());
                }
                let exc = asyncio_rt::future_exception(_interp, &fut.0)?;
                if !exc.is_none() {
                    asyncio_rt::future_set_exception(_interp, &st.outer, exc)?;
                    return Ok(Value::none());
                }
            }
            if finished_now == st.nfuts {
                let mut results = Vec::with_capacity(st.children.len());
                for child in &st.children {
                    let is_canc = with_native_state::<FutureState, _>(child, "Future", |fs| {
                        Ok(fs.is_cancelled())
                    })?;
                    if is_canc {
                        let msg = with_native_state::<FutureState, _>(child, "Future", |fs| {
                            Ok(fs.cancel_message.clone())
                        })?;
                        let ce = match msg {
                            Some(m) if !m.is_none() => {
                                asyncio_rt::make_asyncio_exc(_interp, "CancelledError", &[m])?
                            }
                            _ => asyncio_rt::make_asyncio_exc(
                                _interp,
                                "CancelledError",
                                &[Value::string("")],
                            )?,
                        };
                        results.push(ce);
                    } else {
                        let exc = asyncio_rt::future_exception(_interp, child)?;
                        if !exc.is_none() {
                            results.push(exc);
                        } else {
                            results.push(asyncio_rt::future_result(_interp, child)?);
                        }
                    }
                }
                let cancel_req =
                    with_native_state::<FutureState, _>(&st.outer, "Future", |fs| {
                        Ok(fs.gathering_cancel_requested)
                    })?;
                if cancel_req {
                    let ce = asyncio_rt::future_make_cancelled_error(_interp, &fut.0)?;
                    asyncio_rt::future_set_exception(_interp, &st.outer, ce)?;
                } else {
                    asyncio_rt::future_set_result(_interp, &st.outer, Value::list(results))?;
                }
            }
            Ok(Value::none())
        }
    }

    #[native_state(AsCompletedState)]
    class _AsCompletedDoneCb {
        fn __call__(#[state] st: &AsCompletedState, fut: PyValue) -> Result<Value> {
            let empty_now = {
                let mut todo = st.todo.borrow_mut();
                if todo.is_empty() {
                    return Ok(Value::none());
                }
                todo.retain(|x| !asyncio_rt::same_object(x, &fut.0));
                todo.is_empty()
            };
            let _ = awaitable::queue_put_nowait(_interp, &st.done_queue, fut.0);
            if empty_now
                && let Some(h) = st.timeout_handle.borrow_mut().take()
            {
                let _ = asyncio_rt::call_method(_interp, &h, "cancel", &[]);
            }
            Ok(Value::none())
        }
    }

    #[native_state(AsCompletedState)]
    class _AsCompletedTimeoutCb {
        fn __call__(#[state] st: &AsCompletedState) -> Result<Value> {
            let remaining = {
                let mut todo = st.todo.borrow_mut();
                std::mem::take(&mut *todo)
            };
            for _ in remaining {
                let _ = awaitable::queue_put_nowait(_interp, &st.done_queue, Value::none());
            }
            Ok(Value::none())
        }
    }

    #[native_state(AsCompletedIterState)]
    class _AsCompletedIter {
        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(self_val.0.clone())
        }

        fn __next__(#[state_mut] st: &mut AsCompletedIterState) -> Result<Value> {
            if st.remaining == 0 {
                return Err(PyError::named("StopIteration", String::new()));
            }
            st.remaining -= 1;
            awaitable::make_native_coro(
                _interp,
                CoroBody::AsCompletedOne {
                    queue: st.queue.clone(),
                    get_coro: None,
                },
            )
        }
    }

    #[native_state(ShieldInnerCbState)]
    class _ShieldInnerCb {
        fn __call__(#[state] st: &ShieldInnerCbState, inner: PyValue) -> Result<Value> {
            let outer_canc = with_native_state::<FutureState, _>(&st.outer, "Future", |fs| {
                Ok(fs.is_cancelled())
            })?;
            if outer_canc {
                return Ok(Value::none());
            }
            let inner_canc = with_native_state::<FutureState, _>(&inner.0, "Future", |fs| {
                Ok(fs.is_cancelled())
            })?;
            if inner_canc {
                let _ = asyncio_rt::future_cancel(_interp, &st.outer, None)?;
            } else {
                let exc = asyncio_rt::future_exception(_interp, &inner.0)?;
                if !exc.is_none() {
                    asyncio_rt::future_set_exception(_interp, &st.outer, exc)?;
                } else {
                    let res = asyncio_rt::future_result(_interp, &inner.0)?;
                    asyncio_rt::future_set_result(_interp, &st.outer, res)?;
                }
            }
            Ok(Value::none())
        }
    }

    #[native_state(ShieldOuterCbState)]
    class _ShieldOuterCb {
        fn __call__(#[state] st: &ShieldOuterCbState, _outer: PyValue) -> Result<Value> {
            let inner_done = with_native_state::<FutureState, _>(&st.inner, "Future", |fs| {
                Ok(fs.is_done())
            })?;
            if !inner_done {
                let _ = asyncio_rt::future_remove_done_callback(_interp, &st.inner, &st.inner_cb);
            }
            Ok(Value::none())
        }
    }

    #[native_state(WrapFutureCbState)]
    class _WrapFutureCb {
        fn __call__(#[state] st: &WrapFutureCbState, _f: PyValue) -> Result<Value> {
            let dst_done = with_native_state::<FutureState, _>(&st.dst, "Future", |fs| {
                Ok(fs.is_done())
            })?;
            if dst_done {
                return Ok(Value::none());
            }
            let src_canc = asyncio_rt::call_method(_interp, &st.src, "cancelled", &[])
                .and_then(|v| _interp.truthy_value(&v))
                .unwrap_or(false);
            if src_canc {
                let _ = asyncio_rt::future_cancel(_interp, &st.dst, None);
                return Ok(Value::none());
            }
            match asyncio_rt::call_method(_interp, &st.src, "exception", &[]) {
                Ok(exc) if !exc.is_none() => {
                    let _ = asyncio_rt::future_set_exception(_interp, &st.dst, exc);
                }
                _ => match asyncio_rt::call_method(_interp, &st.src, "result", &[]) {
                    Ok(res) => {
                        let _ = asyncio_rt::future_set_result(_interp, &st.dst, res);
                    }
                    Err(err) => {
                        let ev = asyncio_rt::pyerror_to_exc_value(_interp, err)?;
                        let _ = asyncio_rt::future_set_exception(_interp, &st.dst, ev);
                    }
                },
            }
            Ok(Value::none())
        }
    }

    #[native_state(EagerTaskFactoryState)]
    class _EagerTaskFactory {
        fn __call__(
            #[state] st: &EagerTaskFactoryState,
            loop_val: PyValue,
            coro: PyValue,
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut kw: Vec<(&str, Value)> = Vec::with_capacity(kwargs.len() + 2);
            kw.push(("loop", loop_val.0));
            kw.push(("eager_start", Value::bool_(true)));
            for (k, v) in &kwargs {
                if k != "loop" && k != "eager_start" {
                    kw.push((k.as_str(), v.clone()));
                }
            }
            asyncio_rt::call_kw(_interp, st.custom_constructor.clone(), &[coro.0], &kw)
        }
    }

    // ── Future & Task ────────────────────────────────────────────────────────

    #[native_state(FutureState)]
    class Future {
        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            item: PyValue,
        ) -> Result<Value> {
            let types_mod = _interp.load_module("types")?;
            let ga = _interp.get_attr(&types_mod, "GenericAlias")?;
            asyncio_rt::call_pos(_interp, ga, &[cls.0.clone(), item.0])
        }

        #[getter]
        fn _asyncio_future_blocking(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.future_blocking))
        }

        #[setter("_asyncio_future_blocking")]
        fn set_asyncio_future_blocking(
            #[state_mut] st: &mut FutureState,
            val: PyBool,
        ) -> Result<Value> {
            let _ = _interp;
            st.future_blocking = val.0;
            Ok(Value::none())
        }

        #[getter]
        fn _loop(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            Ok(st.loop_val.clone())
        }

        #[getter]
        fn _state(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::string(st.status.as_str()))
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>,
        ) -> Result<Value> {
            let lv = match opt_value_cloned(r#loop) {
                Some(l) => l,
                None => asyncio_rt::get_running_loop_or_err()?,
            };
            set_state(self_val.0, FutureState::new_future(lv))?;
            Ok(Value::none())
        }

        fn get_loop(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            Ok(st.loop_val.clone())
        }

        fn done(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.is_done()))
        }

        fn cancelled(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.is_cancelled()))
        }

        fn cancel(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] msg: OptValueRef<'_>,
        ) -> Result<Value> {
            let m = opt_value_cloned(msg);
            let ok = asyncio_rt::future_cancel(_interp, self_val.0, m)?;
            Ok(Value::bool_(ok))
        }

        fn _make_cancelled_error(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            asyncio_rt::future_make_cancelled_error(_interp, self_val.0)
        }

        fn result(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            asyncio_rt::future_result(_interp, self_val.0)
        }

        fn exception(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            asyncio_rt::future_exception(_interp, self_val.0)
        }

        fn set_result(#[positional_only] self_val: PyValueRef<'_>, result: PyValue) -> Result<Value> {
            asyncio_rt::future_set_result(_interp, self_val.0, result.0)?;
            Ok(Value::none())
        }

        fn set_exception(
            #[positional_only] self_val: PyValueRef<'_>,
            exception: PyValue,
        ) -> Result<Value> {
            asyncio_rt::future_set_exception(_interp, self_val.0, exception.0)?;
            Ok(Value::none())
        }

        fn add_done_callback(
            #[positional_only] self_val: PyValueRef<'_>,
            fn_cb: PyValue,
            #[keyword_only] #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            asyncio_rt::future_add_done_callback(
                _interp,
                self_val.0,
                fn_cb.0,
                opt_value_cloned(context),
            )?;
            Ok(Value::none())
        }

        fn remove_done_callback(
            #[positional_only] self_val: PyValueRef<'_>,
            fn_cb: PyValue,
        ) -> Result<Value> {
            let n = asyncio_rt::future_remove_done_callback(_interp, self_val.0, &fn_cb.0)?;
            Ok(Value::int(n as i64))
        }

        fn __await__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::AwaitFuture {
                    fut: self_val.0.clone(),
                    yielded: false,
                },
            )
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::AwaitFuture {
                    fut: self_val.0.clone(),
                    yielded: false,
                },
            )
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (status, res_opt, exc_opt) = with_state(self_val.0, |st| {
                Ok((st.status, st.result.clone(), st.exception.clone()))
            })?;
            let cls_name = value_type_name_str(self_val.0);
            let mut info = status.as_str().to_lowercase();
            if status == FutureStatus::Finished {
                if let Some(e) = exc_opt {
                    let er = render_value_repr(_interp, &e)?;
                    info.push_str(&format!(" exception={er}"));
                } else if let Some(r) = res_opt {
                    let rr = render_value_repr(_interp, &r)?;
                    info.push_str(&format!(" result={rr}"));
                }
            }
            Ok(Value::string(format!("<{cls_name} {info}>")))
        }
    }

    #[native_state(FutureState)]
    class Task("Future") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            coro: PyValue,
            #[keyword_only] #[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
            #[keyword_only] #[default(false.into())] eager_start: PyBool,
        ) -> Result<Value> {
            let lv = match opt_value_cloned(r#loop) {
                Some(l) => l,
                None => asyncio_rt::get_running_loop_or_err()?,
            };
            let task_name = match opt_value_cloned(name) {
                Some(v) => {
                    let s = asyncio_rt::call_pos(
                        _interp,
                        Value::builtin_function("str"),
                        &[v],
                    )?;
                    Some(s.as_str().unwrap_or("").to_owned())
                }
                None => None,
            };
            asyncio_rt::init_task_instance(
                _interp,
                self_val.0,
                coro.0,
                lv,
                task_name,
                opt_value_cloned(context),
                eager_start.0,
            )?;
            Ok(Value::none())
        }

        fn get_coro(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            Ok(st
                .task
                .as_ref()
                .map(|t| t.coro.clone())
                .unwrap_or_else(Value::none))
        }

        fn get_context(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            Ok(st
                .task
                .as_ref()
                .map(|t| t.context.clone())
                .unwrap_or_else(Value::none))
        }

        fn get_name(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::string(
                st.task.as_ref().map(|t| t.name.as_str()).unwrap_or(""),
            ))
        }

        fn set_name(#[positional_only] self_val: PyValueRef<'_>, value: PyValue) -> Result<Value> {
            let s_val =
                asyncio_rt::call_pos(_interp, Value::builtin_function("str"), &[value.0])?;
            let s = s_val.as_str().unwrap_or("").to_owned();
            with_state_mut(self_val.0, |st| {
                if let Some(ref mut t) = st.task {
                    t.name = s;
                }
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn set_result(#[positional_only] _self: PyValueRef<'_>, _result: PyValue) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named(
                "RuntimeError",
                "Task does not support set_result operation",
            ))
        }

        fn set_exception(
            #[positional_only] _self: PyValueRef<'_>,
            _exception: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named(
                "RuntimeError",
                "Task does not support set_exception operation",
            ))
        }

        fn cancelling(#[state] st: &FutureState) -> Result<Value> {
            let _ = _interp;
            let n = st
                .task
                .as_ref()
                .map(|t| t.num_cancels_requested)
                .unwrap_or(0);
            Ok(Value::int(n as i64))
        }

        fn uncancel(#[state_mut] st: &mut FutureState) -> Result<Value> {
            let _ = _interp;
            let n = if let Some(ref mut t) = st.task {
                if t.num_cancels_requested > 0 {
                    t.num_cancels_requested -= 1;
                }
                t.num_cancels_requested
            } else {
                0
            };
            Ok(Value::int(n as i64))
        }

        fn get_stack(
            #[positional_only] _self: PyValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _limit: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::list(Vec::new()))
        }

        fn print_stack(
            #[positional_only] _self: PyValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _limit: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _file: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn _step_run(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] exc: OptValueRef<'_>,
        ) -> Result<Value> {
            asyncio_rt::task_step_run(_interp, self_val.0, opt_value_cloned(exc))?;
            Ok(Value::none())
        }

        fn _step_run_eager(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            asyncio_rt::task_step_run_inner(_interp, self_val.0, None)?;
            Ok(Value::none())
        }

        fn _wakeup(#[positional_only] self_val: PyValueRef<'_>, future: PyValue) -> Result<Value> {
            asyncio_rt::task_wakeup(_interp, self_val.0, &future.0)?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (status, name, coro_opt) = with_state(self_val.0, |st| {
                Ok((
                    st.status,
                    st.task.as_ref().map(|t| t.name.clone()).unwrap_or_default(),
                    st.task.as_ref().map(|t| t.coro.clone()),
                ))
            })?;
            let cls_name = value_type_name_str(self_val.0);
            let coro_s = match coro_opt {
                Some(c) => render_value_repr(_interp, &c)?,
                None => "None".to_string(),
            };
            Ok(Value::string(format!(
                "<{cls_name} {} name={name:?} coro={coro_s}>",
                status.as_str().to_lowercase()
            )))
        }
    }

    // ── Timeout & TaskGroup ──────────────────────────────────────────────────

    #[native_state(TimeoutState)]
    class Timeout {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            when: OptValueRef<'_>,
        ) -> Result<Value> {
            let w = opt_none_or_f64(when, "Timeout", "when")?;
            set_state(
                self_val.0,
                TimeoutState {
                    status: TimeoutStatus::Created,
                    timeout_handler: None,
                    task: None,
                    when: w,
                    cancelling_at_enter: 0,
                },
            )?;
            Ok(Value::none())
        }

        fn when(#[state] st: &TimeoutState) -> Result<Value> {
            let _ = _interp;
            Ok(st.when.map(Value::float).unwrap_or_else(Value::none))
        }

        fn reschedule(
            #[positional_only] self_val: PyValueRef<'_>,
            when: OptValueRef<'_>,
        ) -> Result<Value> {
            let new_when = opt_none_or_f64(when, "reschedule", "when")?;
            let old_handler = with_state_mut(self_val.0, |st| {
                if st.status != TimeoutStatus::Entered {
                    if st.status == TimeoutStatus::Created {
                        return Err(PyError::named(
                            "RuntimeError",
                            "Timeout has not been entered",
                        ));
                    }
                    return Err(PyError::named(
                        "RuntimeError",
                        format!("Cannot change state of {} Timeout", st.status.as_str()),
                    ));
                }
                st.when = new_when;
                Ok(st.timeout_handler.take())
            })?;
            if let Some(h) = old_handler {
                let _ = asyncio_rt::call_method(_interp, &h, "cancel", &[]);
            }
            if let Some(w) = new_when {
                let loop_val = asyncio_rt::get_running_loop_or_err()?;
                let now = asyncio_rt::loop_time(_interp, &loop_val)?;
                let on_to = _interp.get_attr(self_val.0, "_on_timeout")?;
                let h = if w <= now {
                    asyncio_rt::loop_call_soon(_interp, &loop_val, on_to, Vec::new(), None)?
                } else {
                    asyncio_rt::loop_call_at(_interp, &loop_val, w, on_to, Vec::new(), None)?
                };
                with_state_mut(self_val.0, |st| {
                    st.timeout_handler = Some(h);
                    Ok(())
                })?;
            }
            Ok(Value::none())
        }

        fn expired(#[state] st: &TimeoutState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(matches!(
                st.status,
                TimeoutStatus::Expiring | TimeoutStatus::Expired
            )))
        }

        fn __repr__(#[state] st: &TimeoutState) -> Result<Value> {
            let _ = _interp;
            if st.status == TimeoutStatus::Entered {
                let w_str = match st.when {
                    Some(w) => format!("{}", (w * 1000.0).round() / 1000.0),
                    None => "None".to_string(),
                };
                Ok(Value::string(format!(
                    "<Timeout [{}] when={w_str}>",
                    st.status.as_str()
                )))
            } else {
                Ok(Value::string(format!("<Timeout [{}]>", st.status.as_str())))
            }
        }

        fn __aenter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::TimeoutAenter {
                    timeout_obj: self_val.0.clone(),
                },
            )
        }

        fn __aexit__(
            #[positional_only] self_val: PyValueRef<'_>,
            exc_type: PyValue,
            exc_val: PyValue,
            _exc_tb: PyValue,
        ) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::TimeoutAexit {
                    timeout_obj: self_val.0.clone(),
                    exc_type: exc_type.0,
                    exc_val: exc_val.0,
                },
            )
        }

        fn _on_timeout(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let task = with_state_mut(self_val.0, |st| {
                st.status = TimeoutStatus::Expiring;
                st.timeout_handler = None;
                Ok(st.task.clone())
            })?;
            if let Some(t) = task {
                let _ = asyncio_rt::future_cancel(_interp, &t, None);
            }
            Ok(Value::none())
        }
    }

    #[native_state(TaskGroupState)]
    class TaskGroup {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            set_state(
                self_val.0,
                TaskGroupState {
                    entered: false,
                    exiting: false,
                    aborting: false,
                    loop_val: None,
                    parent_task: None,
                    parent_cancel_requested: false,
                    tasks: Vec::new(),
                    errors: Some(Vec::new()),
                    base_error: None,
                    on_completed_fut: None,
                },
            )?;
            Ok(Value::none())
        }

        fn __repr__(#[state] st: &TaskGroupState) -> Result<Value> {
            let _ = _interp;
            let mut parts = Vec::new();
            if !st.tasks.is_empty() {
                parts.push(format!("tasks={}", st.tasks.len()));
            }
            if let Some(ref errs) = st.errors
                && !errs.is_empty()
            {
                parts.push(format!("errors={}", errs.len()));
            }
            if st.aborting {
                parts.push("cancelling".to_string());
            } else if st.entered {
                parts.push("entered".to_string());
            }
            if parts.is_empty() {
                Ok(Value::string("<TaskGroup>"))
            } else {
                Ok(Value::string(format!("<TaskGroup {}>", parts.join(" "))))
            }
        }

        fn __aenter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::TaskGroupAenter {
                    tg_obj: self_val.0.clone(),
                },
            )
        }

        fn __aexit__(
            #[positional_only] self_val: PyValueRef<'_>,
            et: PyValue,
            exc: PyValue,
            _tb: PyValue,
        ) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::TaskGroupAexit {
                    tg_obj: self_val.0.clone(),
                    et: et.0,
                    exc: exc.0,
                    started: false,
                    propagate_cancel: None,
                    waiting_fut: None,
                },
            )
        }

        fn create_task(
            #[positional_only] self_val: PyValueRef<'_>,
            coro: PyValue,
            #[keyword_only] #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let (entered, exiting, tasks_empty, aborting, loop_val_opt) =
                with_state(self_val.0, |st| {
                    Ok((
                        st.entered,
                        st.exiting,
                        st.tasks.is_empty(),
                        st.aborting,
                        st.loop_val.clone(),
                    ))
                })?;
            if !entered {
                let r = render_value_repr(_interp, self_val.0)?;
                return Err(PyError::named(
                    "RuntimeError",
                    format!("TaskGroup {r} has not been entered"),
                ));
            }
            if exiting && tasks_empty {
                let r = render_value_repr(_interp, self_val.0)?;
                return Err(PyError::named(
                    "RuntimeError",
                    format!("TaskGroup {r} is finished"),
                ));
            }
            if aborting {
                let r = render_value_repr(_interp, self_val.0)?;
                return Err(PyError::named(
                    "RuntimeError",
                    format!("TaskGroup {r} is shutting down"),
                ));
            }
            let lv = loop_val_opt
                .ok_or_else(|| PyError::named("RuntimeError", "TaskGroup has no loop"))?;
            let task_name = match opt_value_cloned(name) {
                Some(v) => {
                    let s = asyncio_rt::call_pos(_interp, Value::builtin_function("str"), &[v])?;
                    Some(s.as_str().unwrap_or("").to_owned())
                }
                None => None,
            };
            let task = asyncio_rt::loop_create_task(
                _interp,
                &lv,
                coro.0,
                task_name,
                opt_value_cloned(context),
            )?;
            let is_done = with_native_state::<FutureState, _>(&task, "Task", |fs| Ok(fs.is_done()))
                .unwrap_or(false);
            if is_done {
                awaitable::task_group_on_task_done(_interp, self_val.0, &task)?;
            } else {
                with_state_mut(self_val.0, |st| {
                    st.tasks.push(task.clone());
                    Ok(())
                })?;
                let cb = _interp.get_attr(self_val.0, "_on_task_done")?;
                asyncio_rt::future_add_done_callback(_interp, &task, cb, None)?;
            }
            Ok(task)
        }

        fn _on_task_done(
            #[positional_only] self_val: PyValueRef<'_>,
            task: PyValue,
        ) -> Result<Value> {
            awaitable::task_group_on_task_done(_interp, self_val.0, &task.0)?;
            Ok(Value::none())
        }
    }

    // ── Mixins & Synchronization Primitives ──────────────────────────────────

    class _LoopBoundMixin {
        fn _get_loop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let loop_val = asyncio_rt::get_running_loop_or_err()?;
            let existing = _interp.get_attr(self_val.0, "_loop").ok();
            match existing {
                Some(ref b) if !b.is_none() => {
                    if !asyncio_rt::same_object(b, &loop_val) {
                        let sr = render_value_repr(_interp, self_val.0)?;
                        return Err(PyError::named(
                            "RuntimeError",
                            format!("{sr} is bound to a different event loop"),
                        ));
                    }
                }
                _ => {
                    let _ = _interp.assign_attr(self_val.0.clone(), "_loop", loop_val.clone());
                }
            }
            Ok(loop_val)
        }
    }

    #[native_state(LockState)]
    class Lock("_LoopBoundMixin") {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            set_state(
                self_val.0,
                LockState {
                    loop_val: None,
                    locked: false,
                    waiters: VecDeque::new(),
                },
            )?;
            Ok(Value::none())
        }

        fn locked(#[state] st: &LockState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.locked))
        }

        fn acquire(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::LockAcquire {
                    lock_obj: self_val.0.clone(),
                    waiter: None,
                },
            )
        }

        fn release(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let was_locked = with_state_mut(self_val.0, |st| {
                if st.locked {
                    st.locked = false;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })?;
            if !was_locked {
                return Err(PyError::named("RuntimeError", "Lock is not acquired."));
            }
            awaitable::lock_wake_up_first(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn __aenter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ContextManagerAenter {
                    obj: self_val.0.clone(),
                    inner: None,
                },
            )
        }

        fn __aexit__(
            #[positional_only] self_val: PyValueRef<'_>,
            _et: PyValue,
            _exc: PyValue,
            _tb: PyValue,
        ) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ContextManagerAexit {
                    obj: self_val.0.clone(),
                },
            )
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            let (locked, nwait) = with_state(self_val.0, |st| Ok((st.locked, st.waiters.len())))?;
            let status = if locked { "locked" } else { "unlocked" };
            let extra = if nwait > 0 {
                format!("{status}, waiters:{nwait}")
            } else {
                status.to_string()
            };
            Ok(Value::string(format!(
                "<asyncio.locks.Lock object at {:#x} [{extra}]>",
                asyncio_rt::value_id(self_val.0)
            )))
        }
    }

    #[native_state(EventState)]
    class Event("_LoopBoundMixin") {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            set_state(
                self_val.0,
                EventState {
                    loop_val: None,
                    value: false,
                    waiters: VecDeque::new(),
                },
            )?;
            Ok(Value::none())
        }

        fn is_set(#[state] st: &EventState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.value))
        }

        fn set(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let waiters = with_state_mut(self_val.0, |st| {
                if !st.value {
                    st.value = true;
                    Ok(st.waiters.iter().cloned().collect::<Vec<_>>())
                } else {
                    Ok(Vec::new())
                }
            })?;
            for w in waiters {
                asyncio_rt::future_set_result_if_pending(_interp, &w, Value::bool_(true))?;
            }
            Ok(Value::none())
        }

        fn clear(#[state_mut] st: &mut EventState) -> Result<Value> {
            let _ = _interp;
            st.value = false;
            Ok(Value::none())
        }

        fn wait(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::EventWait {
                    event_obj: self_val.0.clone(),
                    waiter: None,
                },
            )
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            let (is_set, nwait) = with_state(self_val.0, |st| Ok((st.value, st.waiters.len())))?;
            let status = if is_set { "set" } else { "unset" };
            let extra = if nwait > 0 {
                format!("{status}, waiters:{nwait}")
            } else {
                status.to_string()
            };
            Ok(Value::string(format!(
                "<asyncio.locks.Event object at {:#x} [{extra}]>",
                asyncio_rt::value_id(self_val.0)
            )))
        }
    }

    #[native_state(ConditionState)]
    class Condition("_LoopBoundMixin") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] lock: OptValueRef<'_>,
        ) -> Result<Value> {
            let lock_obj = match opt_value_cloned(lock) {
                Some(l) => l,
                None => {
                    let cls = asyncio_rt::asyncio_attr(_interp, "Lock")?;
                    asyncio_rt::call_pos(_interp, cls, &[])?
                }
            };
            set_state(
                self_val.0,
                ConditionState {
                    loop_val: None,
                    lock: lock_obj,
                    waiters: VecDeque::new(),
                },
            )?;
            Ok(Value::none())
        }

        fn locked(#[state] st: &ConditionState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.lock, "locked", &[])
        }

        fn acquire(#[state] st: &ConditionState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.lock, "acquire", &[])
        }

        fn release(#[state] st: &ConditionState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.lock, "release", &[])
        }

        fn wait(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ConditionWait {
                    cond_obj: self_val.0.clone(),
                    phase: CondWaitPhase::Init,
                },
            )
        }

        fn wait_for(
            #[positional_only] self_val: PyValueRef<'_>,
            predicate: PyValue,
        ) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ConditionWaitFor {
                    cond_obj: self_val.0.clone(),
                    predicate: predicate.0,
                    wait_coro: None,
                },
            )
        }

        fn notify(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(1_i64.into())] n: PyIndex,
        ) -> Result<Value> {
            let n = n.expect_i64(FN_NAME, "n")?;
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let is_locked = asyncio_rt::call_method(_interp, &lock, "locked", &[])?;
            if !_interp.truthy_value(&is_locked)? {
                return Err(PyError::named(
                    "RuntimeError",
                    "cannot notify on un-acquired lock",
                ));
            }
            let waiters = with_state(self_val.0, |st| {
                Ok(st.waiters.iter().cloned().collect::<Vec<_>>())
            })?;
            let mut idx = 0i64;
            for fut in waiters {
                if idx >= n {
                    break;
                }
                let done = with_native_state::<FutureState, _>(&fut, "Future", |fs| Ok(fs.is_done()))
                    .unwrap_or(true);
                if !done {
                    idx += 1;
                    asyncio_rt::future_set_result_if_pending(_interp, &fut, Value::bool_(false))?;
                }
            }
            Ok(Value::none())
        }

        fn notify_all(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let len = with_state(self_val.0, |st| Ok(st.waiters.len() as i64))?;
            asyncio_rt::call_method(_interp, self_val.0, "notify", &[Value::int(len)])
        }

        fn __aenter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ContextManagerAenter {
                    obj: self_val.0.clone(),
                    inner: None,
                },
            )
        }

        fn __aexit__(
            #[positional_only] self_val: PyValueRef<'_>,
            _et: PyValue,
            _exc: PyValue,
            _tb: PyValue,
        ) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ContextManagerAexit {
                    obj: self_val.0.clone(),
                },
            )
        }
    }

    #[native_state(SemaphoreState)]
    class Semaphore("_LoopBoundMixin") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(1_i64.into())] value: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            let v = value.expect_i64(FN_NAME, "value")?;
            if v < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "Semaphore initial value must be >= 0",
                ));
            }
            set_state(
                self_val.0,
                SemaphoreState {
                    loop_val: None,
                    value: v,
                    bound_value: None,
                    waiters: VecDeque::new(),
                },
            )?;
            Ok(Value::none())
        }

        fn locked(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(awaitable::semaphore_is_locked(self_val.0)?))
        }

        fn acquire(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::SemaphoreAcquire {
                    sem_obj: self_val.0.clone(),
                    waiter: None,
                },
            )
        }

        fn release(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                if let Some(bound) = st.bound_value
                    && st.value >= bound
                {
                    return Err(PyError::named(
                        "ValueError",
                        "BoundedSemaphore released too many times",
                    ));
                }
                st.value += 1;
                Ok(())
            })?;
            awaitable::semaphore_wake_next(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn __aenter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ContextManagerAenter {
                    obj: self_val.0.clone(),
                    inner: None,
                },
            )
        }

        fn __aexit__(
            #[positional_only] self_val: PyValueRef<'_>,
            _et: PyValue,
            _exc: PyValue,
            _tb: PyValue,
        ) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ContextManagerAexit {
                    obj: self_val.0.clone(),
                },
            )
        }
    }

    #[native_state(SemaphoreState)]
    class BoundedSemaphore("Semaphore") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(1_i64.into())] value: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            let v = value.expect_i64(FN_NAME, "value")?;
            if v < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "Semaphore initial value must be >= 0",
                ));
            }
            set_state(
                self_val.0,
                SemaphoreState {
                    loop_val: None,
                    value: v,
                    bound_value: Some(v),
                    waiters: VecDeque::new(),
                },
            )?;
            Ok(Value::none())
        }
    }

    #[native_state(BarrierState)]
    class Barrier("_LoopBoundMixin") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            parties: PyIndex,
        ) -> Result<Value> {
            let p = parties.expect_i64(FN_NAME, "parties")?;
            if p < 1 {
                return Err(PyError::named("ValueError", "parties must be > 0"));
            }
            let cond_cls = asyncio_rt::asyncio_attr(_interp, "Condition")?;
            let cond = asyncio_rt::call_pos(_interp, cond_cls, &[])?;
            set_state(
                self_val.0,
                BarrierState {
                    cond,
                    parties: p,
                    status: BarrierStatus::Filling,
                    count: 0,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn parties(#[state] st: &BarrierState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(st.parties))
        }

        #[getter]
        fn n_waiting(#[state] st: &BarrierState) -> Result<Value> {
            let _ = _interp;
            if st.status == BarrierStatus::Filling {
                Ok(Value::int(st.count))
            } else {
                Ok(Value::int(0))
            }
        }

        #[getter]
        fn broken(#[state] st: &BarrierState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.status == BarrierStatus::Broken))
        }

        fn wait(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::BarrierWait {
                    barrier_obj: self_val.0.clone(),
                    phase: BarrierWaitPhase::Init,
                },
            )
        }

        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::BarrierReset {
                    barrier_obj: self_val.0.clone(),
                    acquire_coro: None,
                },
            )
        }

        fn abort(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::BarrierAbort {
                    barrier_obj: self_val.0.clone(),
                    acquire_coro: None,
                },
            )
        }

        fn __aenter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::BarrierWait {
                    barrier_obj: self_val.0.clone(),
                    phase: BarrierWaitPhase::Init,
                },
            )
        }

        fn __aexit__(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ReadyValue {
                    result: Value::none(),
                },
            )
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            let (status, count, parties) =
                with_state(self_val.0, |st| Ok((st.status, st.count, st.parties)))?;
            let mut extra = status.as_str().to_string();
            if status != BarrierStatus::Broken {
                let nw = if status == BarrierStatus::Filling {
                    count
                } else {
                    0
                };
                extra.push_str(&format!(", waiters:{nw}/{parties}"));
            }
            Ok(Value::string(format!(
                "<asyncio.locks.Barrier object at {:#x} [{extra}]>",
                asyncio_rt::value_id(self_val.0)
            )))
        }
    }

    // ── Queues ───────────────────────────────────────────────────────────────

    #[native_state(QueueState)]
    class Queue("_LoopBoundMixin") {
        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            item: PyValue,
        ) -> Result<Value> {
            let types_mod = _interp.load_module("types")?;
            let ga = _interp.get_attr(&types_mod, "GenericAlias")?;
            asyncio_rt::call_pos(_interp, ga, &[cls.0.clone(), item.0])
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(0_i64.into())] maxsize: PyIndex,
        ) -> Result<Value> {
            let ms = maxsize.expect_i64(FN_NAME, "maxsize")?;
            let ev_cls = asyncio_rt::asyncio_attr(_interp, "Event")?;
            let finished = asyncio_rt::call_pos(_interp, ev_cls, &[])?;
            let _ = asyncio_rt::call_method(_interp, &finished, "set", &[])?;
            set_state(
                self_val.0,
                QueueState {
                    kind: QueueKind::Fifo,
                    loop_val: None,
                    maxsize: ms,
                    items: VecDeque::new(),
                    getters: VecDeque::new(),
                    putters: VecDeque::new(),
                    unfinished_tasks: 0,
                    finished,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn maxsize(#[state] st: &QueueState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(st.maxsize))
        }

        fn qsize(#[state] st: &QueueState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(st.items.len() as i64))
        }

        fn empty(#[state] st: &QueueState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.items.is_empty()))
        }

        fn full(#[state] st: &QueueState) -> Result<Value> {
            let _ = _interp;
            if st.maxsize <= 0 {
                Ok(Value::bool_(false))
            } else {
                Ok(Value::bool_((st.items.len() as i64) >= st.maxsize))
            }
        }

        fn put(#[positional_only] self_val: PyValueRef<'_>, item: PyValue) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::QueuePut {
                    queue_obj: self_val.0.clone(),
                    item: item.0,
                    waiter: None,
                },
            )
        }

        fn put_nowait(#[positional_only] self_val: PyValueRef<'_>, item: PyValue) -> Result<Value> {
            awaitable::queue_put_nowait(_interp, self_val.0, item.0)?;
            Ok(Value::none())
        }

        fn get(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::QueueGet {
                    queue_obj: self_val.0.clone(),
                    waiter: None,
                },
            )
        }

        fn get_nowait(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::queue_get_nowait(_interp, self_val.0)
        }

        fn task_done(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let finished_opt = with_state_mut(self_val.0, |st| {
                if st.unfinished_tasks <= 0 {
                    return Err(PyError::named(
                        "ValueError",
                        "task_done() called too many times",
                    ));
                }
                st.unfinished_tasks -= 1;
                if st.unfinished_tasks == 0 {
                    Ok(Some(st.finished.clone()))
                } else {
                    Ok(None)
                }
            })?;
            if let Some(ev) = finished_opt {
                let _ = asyncio_rt::call_method(_interp, &ev, "set", &[])?;
            }
            Ok(Value::none())
        }

        fn join(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::QueueJoin {
                    queue_obj: self_val.0.clone(),
                    wait_coro: None,
                },
            )
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            let (ms, len, tasks) = with_state(self_val.0, |st| {
                Ok((st.maxsize, st.items.len(), st.unfinished_tasks))
            })?;
            let cls = value_type_name_str(self_val.0);
            let mut s = format!("<{cls} at {:#x} maxsize={ms}", asyncio_rt::value_id(self_val.0));
            if len > 0 {
                s.push_str(&format!(" _queue=[{len} items]"));
            }
            if tasks > 0 {
                s.push_str(&format!(" tasks={tasks}"));
            }
            s.push('>');
            Ok(Value::string(s))
        }
    }

    #[native_state(QueueState)]
    class LifoQueue("Queue") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(0_i64.into())] maxsize: PyIndex,
        ) -> Result<Value> {
            let ms = maxsize.expect_i64(FN_NAME, "maxsize")?;
            let ev_cls = asyncio_rt::asyncio_attr(_interp, "Event")?;
            let finished = asyncio_rt::call_pos(_interp, ev_cls, &[])?;
            let _ = asyncio_rt::call_method(_interp, &finished, "set", &[])?;
            set_state(
                self_val.0,
                QueueState {
                    kind: QueueKind::Lifo,
                    loop_val: None,
                    maxsize: ms,
                    items: VecDeque::new(),
                    getters: VecDeque::new(),
                    putters: VecDeque::new(),
                    unfinished_tasks: 0,
                    finished,
                },
            )?;
            Ok(Value::none())
        }
    }

    #[native_state(QueueState)]
    class PriorityQueue("Queue") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(0_i64.into())] maxsize: PyIndex,
        ) -> Result<Value> {
            let ms = maxsize.expect_i64(FN_NAME, "maxsize")?;
            let ev_cls = asyncio_rt::asyncio_attr(_interp, "Event")?;
            let finished = asyncio_rt::call_pos(_interp, ev_cls, &[])?;
            let _ = asyncio_rt::call_method(_interp, &finished, "set", &[])?;
            set_state(
                self_val.0,
                QueueState {
                    kind: QueueKind::Priority,
                    loop_val: None,
                    maxsize: ms,
                    items: VecDeque::new(),
                    getters: VecDeque::new(),
                    putters: VecDeque::new(),
                    unfinished_tasks: 0,
                    finished,
                },
            )?;
            Ok(Value::none())
        }
    }

    // ── Handle & TimerHandle ─────────────────────────────────────────────────

    #[native_state(HandleState)]
    class Handle {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            callback: PyValue,
            args: PyValue,
            loop_val: PyValue,
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let arg_vec = collect_iterable_values(_interp, &args.0)?;
            let ctx = match opt_value_cloned(context) {
                Some(c) => c,
                None => asyncio_rt::copy_context(_interp)?,
            };
            set_state(
                self_val.0,
                HandleState {
                    callback: callback.0,
                    args: arg_vec,
                    loop_val: loop_val.0,
                    context: ctx,
                    cancelled: false,
                    when: None,
                },
            )?;
            Ok(Value::none())
        }

        fn cancel(#[state_mut] st: &mut HandleState) -> Result<Value> {
            let _ = _interp;
            st.cancelled = true;
            Ok(Value::none())
        }

        fn cancelled(#[state] st: &HandleState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.cancelled))
        }

        fn get_context(#[state] st: &HandleState) -> Result<Value> {
            let _ = _interp;
            Ok(st.context.clone())
        }

        fn _run(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::handle_run(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (cancelled, cb) =
                with_state(self_val.0, |st| Ok((st.cancelled, st.callback.clone())))?;
            let cbr = render_value_repr(_interp, &cb)?;
            let extra = if cancelled { " cancelled" } else { "" };
            Ok(Value::string(format!("<Handle {cbr}{extra}>")))
        }
    }

    #[native_state(HandleState)]
    class TimerHandle("Handle") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            when: PyNumber,
            callback: PyValue,
            args: PyValue,
            loop_val: PyValue,
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let arg_vec = collect_iterable_values(_interp, &args.0)?;
            let ctx = match opt_value_cloned(context) {
                Some(c) => c,
                None => asyncio_rt::copy_context(_interp)?,
            };
            set_state(
                self_val.0,
                HandleState {
                    callback: callback.0,
                    args: arg_vec,
                    loop_val: loop_val.0,
                    context: ctx,
                    cancelled: false,
                    when: Some(when.as_f64()),
                },
            )?;
            Ok(Value::none())
        }

        fn when(#[state] st: &HandleState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::float(st.when.unwrap_or(0.0)))
        }

        fn __lt__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let _ = _interp;
            if !has_native_state::<HandleState>(&other.0) {
                return Ok(Value::not_implemented());
            }
            let a = with_state(self_val.0, |st| Ok(st.when.unwrap_or(0.0)))?;
            let b = with_native_state::<HandleState, _>(&other.0, "TimerHandle", |st| {
                Ok(st.when.unwrap_or(0.0))
            })?;
            Ok(Value::bool_(a < b))
        }

        fn __le__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let _ = _interp;
            if !has_native_state::<HandleState>(&other.0) {
                return Ok(Value::not_implemented());
            }
            let a = with_state(self_val.0, |st| Ok(st.when.unwrap_or(0.0)))?;
            let b = with_native_state::<HandleState, _>(&other.0, "TimerHandle", |st| {
                Ok(st.when.unwrap_or(0.0))
            })?;
            Ok(Value::bool_(a <= b))
        }

        fn __gt__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let _ = _interp;
            if !has_native_state::<HandleState>(&other.0) {
                return Ok(Value::not_implemented());
            }
            let a = with_state(self_val.0, |st| Ok(st.when.unwrap_or(0.0)))?;
            let b = with_native_state::<HandleState, _>(&other.0, "TimerHandle", |st| {
                Ok(st.when.unwrap_or(0.0))
            })?;
            Ok(Value::bool_(a > b))
        }

        fn __ge__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let _ = _interp;
            if !has_native_state::<HandleState>(&other.0) {
                return Ok(Value::not_implemented());
            }
            let a = with_state(self_val.0, |st| Ok(st.when.unwrap_or(0.0)))?;
            let b = with_native_state::<HandleState, _>(&other.0, "TimerHandle", |st| {
                Ok(st.when.unwrap_or(0.0))
            })?;
            Ok(Value::bool_(a >= b))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (when, cancelled, cb) = with_state(self_val.0, |st| {
                Ok((st.when.unwrap_or(0.0), st.cancelled, st.callback.clone()))
            })?;
            let cbr = render_value_repr(_interp, &cb)?;
            let extra = if cancelled { " cancelled" } else { "" };
            Ok(Value::string(format!(
                "<TimerHandle when={when:.3} {cbr}{extra}>"
            )))
        }
    }

    // ── Transports & Protocols ───────────────────────────────────────────────

    #[native_state(BaseTransportExtraState)]
    class BaseTransport {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] extra: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            let mut map = IndexMap::new();
            if let Some(v) = opt_value_cloned(extra)
                && let Some(d) = v.as_dict()
            {
                for (k, val) in d.iter() {
                    if let PyKey::Str(s) = k {
                        map.insert(s.to_string(), val.clone());
                    }
                }
            }
            set_state(
                self_val.0,
                BaseTransportExtraState {
                    extra: map,
                    protocol: None,
                    closing: false,
                },
            )?;
            Ok(Value::none())
        }

        fn get_extra_info(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            let def = opt_value_cloned(default).unwrap_or_else(Value::none);
            if has_state(self_val.0) {
                return with_state(self_val.0, |st| {
                    Ok(st.extra.get(name.0.as_ref()).cloned().unwrap_or(def))
                });
            }
            Ok(def)
        }

        fn is_closing(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            if has_state(self_val.0) {
                return with_state(self_val.0, |st| Ok(Value::bool_(st.closing)));
            }
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            if has_state(self_val.0) {
                return with_state_mut(self_val.0, |st| {
                    st.closing = true;
                    Ok(Value::none())
                });
            }
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn set_protocol(
            #[positional_only] self_val: PyValueRef<'_>,
            protocol: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            if has_state(self_val.0) {
                return with_state_mut(self_val.0, |st| {
                    st.protocol = Some(protocol.0);
                    Ok(Value::none())
                });
            }
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn get_protocol(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            if has_state(self_val.0) {
                return with_state(self_val.0, |st| {
                    Ok(st.protocol.clone().unwrap_or_else(Value::none))
                });
            }
            Err(PyError::named("NotImplementedError", String::new()))
        }
    }

    class ReadTransport("BaseTransport") {
        fn is_reading(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn pause_reading(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn resume_reading(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }
    }

    class WriteTransport("BaseTransport") {
        fn set_write_buffer_limits(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _high: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _low: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn get_write_buffer_size(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn get_write_buffer_limits(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn write(#[positional_only] _self: PyValueRef<'_>, _data: PyValue) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn writelines(
            #[positional_only] self_val: PyValueRef<'_>,
            list_of_data: PyValue,
        ) -> Result<Value> {
            let items = collect_iterable_values(_interp, &list_of_data.0)?;
            let mut combined = Vec::new();
            for item in items {
                let bl = PyBytesLike::try_from_value_with_interp(
                    _interp,
                    &item,
                    "writelines",
                    "data",
                )?;
                combined.extend_from_slice(bl.as_bytes());
            }
            asyncio_rt::call_method(_interp, self_val.0, "write", &[Value::bytes(combined)])
        }

        fn write_eof(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn can_write_eof(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn abort(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }
    }

    class Transport("ReadTransport", "WriteTransport") {}

    class DatagramTransport("BaseTransport") {
        fn sendto(
            #[positional_only] _self: PyValueRef<'_>,
            _data: PyValue,
            #[default(OptionalArg::Omitted)] _addr: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn abort(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }
    }

    class SubprocessTransport("BaseTransport") {
        fn get_pid(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn get_returncode(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn get_pipe_transport(
            #[positional_only] _self: PyValueRef<'_>,
            _fd: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn send_signal(
            #[positional_only] _self: PyValueRef<'_>,
            _signal: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn terminate(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn kill(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }
    }

    class BaseProtocol {
        fn connection_made(
            #[positional_only] _self: PyValueRef<'_>,
            _transport: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn connection_lost(
            #[positional_only] _self: PyValueRef<'_>,
            _exc: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn pause_writing(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn resume_writing(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }
    }

    class Protocol("BaseProtocol") {
        fn data_received(
            #[positional_only] _self: PyValueRef<'_>,
            _data: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn eof_received(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }
    }

    class BufferedProtocol("BaseProtocol") {
        fn get_buffer(
            #[positional_only] _self: PyValueRef<'_>,
            _sizehint: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn buffer_updated(
            #[positional_only] _self: PyValueRef<'_>,
            _nbytes: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn eof_received(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }
    }

    class DatagramProtocol("BaseProtocol") {
        fn datagram_received(
            #[positional_only] _self: PyValueRef<'_>,
            _data: PyValue,
            _addr: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn error_received(
            #[positional_only] _self: PyValueRef<'_>,
            _exc: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }
    }

    class SubprocessProtocol("BaseProtocol") {
        fn pipe_data_received(
            #[positional_only] _self: PyValueRef<'_>,
            _fd: PyIndex,
            _data: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn pipe_connection_lost(
            #[positional_only] _self: PyValueRef<'_>,
            _fd: PyIndex,
            _exc: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn process_exited(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }
    }

    #[native_state(TransportSocketState)]
    class TransportSocket {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] sock: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            set_state(
                self_val.0,
                TransportSocketState {
                    sock: opt_value_cloned(sock).unwrap_or_else(Value::none),
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn family(#[state] st: &TransportSocketState) -> Result<Value> {
            _interp.get_attr(&st.sock, "family")
        }

        #[getter]
        fn r#type(#[state] st: &TransportSocketState) -> Result<Value> {
            _interp.get_attr(&st.sock, "type")
        }

        #[getter]
        fn proto(#[state] st: &TransportSocketState) -> Result<Value> {
            _interp.get_attr(&st.sock, "proto")
        }

        fn getsockname(#[state] st: &TransportSocketState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.sock, "getsockname", &[])
        }

        fn getpeername(#[state] st: &TransportSocketState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.sock, "getpeername", &[])
        }

        fn getsockopt(
            #[state] st: &TransportSocketState,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.sock, "getsockopt", args)
        }

        fn fileno(#[state] st: &TransportSocketState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.sock, "fileno", &[])
        }

        fn gettimeout(#[state] st: &TransportSocketState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.sock, "gettimeout", &[])
        }

        fn getblocking(#[state] st: &TransportSocketState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.sock, "getblocking", &[])
        }

        fn __repr__(#[state] st: &TransportSocketState) -> Result<Value> {
            let sr = render_value_repr(_interp, &st.sock)?;
            Ok(Value::string(format!("<asyncio.TransportSocket {sr}>")))
        }
    }

    // ── Concrete Internal Transports ─────────────────────────────────────────

    #[native_state(SocketTransportState)]
    class _SelectorSocketTransport("Transport") {
        fn get_extra_info(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let def = opt_value_cloned(default).unwrap_or_else(Value::none);
            io_engine::transport_get_extra_info(_interp, self_val.0, &name.0, def)
        }

        fn is_closing(#[state] st: &SocketTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.closing))
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::transport_close(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn abort(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::transport_force_close(_interp, self_val.0, None)?;
            Ok(Value::none())
        }

        fn set_protocol(
            #[state_mut] st: &mut SocketTransportState,
            protocol: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            st.protocol = protocol.0;
            Ok(Value::none())
        }

        fn get_protocol(#[state] st: &SocketTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(st.protocol.clone())
        }

        fn is_reading(#[state] st: &SocketTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(!st.reading_paused && !st.closing))
        }

        fn pause_reading(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            let (loop_val, fd) = with_state_mut(self_val.0, |st| {
                st.reading_paused = true;
                Ok((st.loop_val.clone(), st.fd))
            })?;
            let _ = io_engine::loop_remove_reader(&loop_val, fd);
            Ok(Value::none())
        }

        fn resume_reading(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (was_paused, closing, loop_val, fd) = with_state_mut(self_val.0, |st| {
                let wp = st.reading_paused;
                st.reading_paused = false;
                Ok((wp, st.closing, st.loop_val.clone(), st.fd))
            })?;
            if was_paused && !closing {
                let on_read = _interp.get_attr(self_val.0, "_read_ready")?;
                io_engine::loop_add_reader(_interp, &loop_val, fd, on_read, Vec::new())?;
            }
            Ok(Value::none())
        }

        fn set_write_buffer_limits(
            #[state_mut] st: &mut SocketTransportState,
            #[default(OptionalArg::Omitted)] high: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] low: OptValueRef<'_>,
        ) -> Result<Value> {
            let h = opt_none_or_i64(_interp, high, FN_NAME, "high")?.map(|x| x.max(0) as usize);
            let l = opt_none_or_i64(_interp, low, FN_NAME, "low")?.map(|x| x.max(0) as usize);
            st.set_write_buffer_limits(h, l)?;
            Ok(Value::none())
        }

        fn get_write_buffer_size(#[state] st: &SocketTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(st.write_buf.len() as i64))
        }

        fn get_write_buffer_limits(#[state] st: &SocketTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::tuple(vec![
                Value::int(st.low_water as i64),
                Value::int(st.high_water as i64),
            ]))
        }

        fn write(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
        ) -> Result<Value> {
            io_engine::transport_write(_interp, self_val.0, data.as_bytes())?;
            Ok(Value::none())
        }

        fn write_eof(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::transport_write_eof(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn can_write_eof(#[state] st: &SocketTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.tls.is_none()))
        }

        fn _read_ready(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::transport_read_ready(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn _write_ready(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::transport_write_ready(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn _call_connection_lost(
            #[positional_only] self_val: PyValueRef<'_>,
            exc: PyValue,
        ) -> Result<Value> {
            let e = if exc.0.is_none() { None } else { Some(exc.0) };
            io_engine::transport_finish_close(_interp, self_val.0, e)?;
            Ok(Value::none())
        }

        fn _resolve_waiter(
            #[positional_only] _self: PyValueRef<'_>,
            waiter: PyValue,
        ) -> Result<Value> {
            asyncio_rt::future_set_result_if_pending(_interp, &waiter.0, Value::none())?;
            Ok(Value::none())
        }
    }

    #[native_state(DatagramTransportState)]
    class _SelectorDatagramTransport("DatagramTransport") {
        fn get_extra_info(
            #[state] st: &DatagramTransportState,
            name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            let def = opt_value_cloned(default).unwrap_or_else(Value::none);
            Ok(st
                .extra
                .iter()
                .find(|(k, _)| k.as_str() == name.0.as_ref())
                .map(|(_, v)| v.clone())
                .unwrap_or(def))
        }

        fn is_closing(#[state] st: &DatagramTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.closing))
        }

        fn get_protocol(#[state] st: &DatagramTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(st.protocol.clone())
        }

        fn set_protocol(
            #[state_mut] st: &mut DatagramTransportState,
            protocol: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            st.protocol = protocol.0;
            Ok(Value::none())
        }

        fn sendto(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
            #[default(OptionalArg::Omitted)] addr: OptValueRef<'_>,
        ) -> Result<Value> {
            let (sock, remote) =
                with_state(self_val.0, |st| Ok((st.sock.clone(), st.remote_addr.clone())))?;
            let bytes = Value::bytes(data.as_bytes().to_vec());
            match opt_value_cloned(addr).or(remote) {
                Some(a) => {
                    let _ = asyncio_rt::call_method(_interp, &sock, "sendto", &[bytes, a])?;
                }
                None => {
                    let _ = asyncio_rt::call_method(_interp, &sock, "send", &[bytes])?;
                }
            }
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (already, loop_val, fd, sock, protocol) = with_state_mut(self_val.0, |st| {
                let was = st.closed;
                st.closing = true;
                st.closed = true;
                Ok((
                    was,
                    st.loop_val.clone(),
                    st.fd,
                    st.sock.clone(),
                    st.protocol.clone(),
                ))
            })?;
            if !already {
                let _ = io_engine::loop_remove_reader(&loop_val, fd);
                let _ = asyncio_rt::call_method(_interp, &sock, "close", &[]);
                let lost = _interp.get_attr(&protocol, "connection_lost")?;
                asyncio_rt::loop_call_soon(_interp, &loop_val, lost, vec![Value::none()], None)?;
            }
            Ok(Value::none())
        }

        fn abort(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            asyncio_rt::call_method(_interp, self_val.0, "close", &[])
        }

        fn _read_ready(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (closed, sock, protocol) =
                with_state(self_val.0, |st| Ok((st.closed, st.sock.clone(), st.protocol.clone())))?;
            if closed {
                return Ok(Value::none());
            }
            match asyncio_rt::call_method(_interp, &sock, "recvfrom", &[Value::int(262_144)]) {
                Ok(pair) => {
                    if let ValueKind::Tuple(t) = pair.kind()
                        && t.len() >= 2
                    {
                        let _ = asyncio_rt::call_method(
                            _interp,
                            &protocol,
                            "datagram_received",
                            &[t[0].clone(), t[1].clone()],
                        );
                    }
                }
                Err(err) => {
                    let ev = asyncio_rt::pyerror_to_exc_value(_interp, err)?;
                    let is_wouldblock = lookup_exc_class("BlockingIOError")
                        .map(|c| asyncio_rt::is_isinstance(_interp, &ev, &Value::py_class(c)))
                        .unwrap_or(false);
                    if !is_wouldblock {
                        let _ = asyncio_rt::call_method(_interp, &protocol, "error_received", &[ev]);
                    }
                }
            }
            Ok(Value::none())
        }
    }

    #[native_state(PipeWriteTransportState)]
    class _SubprocessWritePipeTransport("WriteTransport") {
        fn write(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            io_engine::pipe_write_bytes(self_val.0, data.as_bytes())?;
            Ok(Value::none())
        }

        fn write_eof(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::pipe_write_close(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn can_write_eof(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(true))
        }

        fn is_closing(#[state] st: &PipeWriteTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.closing))
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::pipe_write_close(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn abort(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::pipe_write_close(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn get_write_buffer_size(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(0))
        }

        fn get_extra_info(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(opt_value_cloned(default).unwrap_or_else(Value::none))
        }
    }

    #[native_state(PipeReadTransportState)]
    class _SubprocessReadPipeTransport("ReadTransport") {
        fn is_reading(#[state] st: &PipeReadTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(!st.closed))
        }

        fn pause_reading(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn resume_reading(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn is_closing(#[state] st: &PipeReadTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.closing))
        }

        fn close(#[state_mut] st: &mut PipeReadTransportState) -> Result<Value> {
            let _ = _interp;
            st.closing = true;
            st.closed = true;
            Ok(Value::none())
        }

        fn get_extra_info(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(opt_value_cloned(default).unwrap_or_else(Value::none))
        }
    }

    #[native_state(SubprocessTransportState)]
    class _SubprocessTransport("SubprocessTransport") {
        fn get_pid(#[state] st: &SubprocessTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(st.pid))
        }

        fn get_returncode(#[state] st: &SubprocessTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(st.returncode.map(Value::int).unwrap_or_else(Value::none))
        }

        fn get_pipe_transport(
            #[state] st: &SubprocessTransportState,
            fd: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            let fd_n = fd.expect_i64(FN_NAME, "fd")?;
            let t = match fd_n {
                0 => st.stdin_transport.clone(),
                1 => st.stdout_transport.clone(),
                2 => st.stderr_transport.clone(),
                _ => None,
            };
            Ok(t.unwrap_or_else(Value::none))
        }

        fn get_protocol(#[state] st: &SubprocessTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(st.protocol.clone())
        }

        fn is_closing(#[state] st: &SubprocessTransportState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.closed))
        }

        fn get_extra_info(
            #[state] st: &SubprocessTransportState,
            name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            let _ = (st, name);
            Ok(opt_value_cloned(default).unwrap_or_else(Value::none))
        }

        fn send_signal(
            #[state_mut] st: &mut SubprocessTransportState,
            sig: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            let sig_n = sig.expect_i64(FN_NAME, "signal")?;
            if st.returncode.is_some() {
                return Err(PyError::named("ProcessLookupError", "process already exited"));
            }
            #[cfg(unix)]
            {
                let rc = unsafe { libc::kill(st.pid as libc::pid_t, sig_n as libc::c_int) };
                if rc != 0 {
                    return Err(PyError::from_io_error(
                        &std::io::Error::last_os_error(),
                        None,
                    ));
                }
            }
            #[cfg(windows)]
            {
                let _ = sig_n;
                let _ = st.child.kill();
            }
            Ok(Value::none())
        }

        fn terminate(#[state_mut] st: &mut SubprocessTransportState) -> Result<Value> {
            let _ = _interp;
            if st.returncode.is_some() {
                return Err(PyError::named("ProcessLookupError", "process already exited"));
            }
            #[cfg(unix)]
            {
                let rc = unsafe { libc::kill(st.pid as libc::pid_t, libc::SIGTERM) };
                if rc != 0 {
                    return Err(PyError::from_io_error(
                        &std::io::Error::last_os_error(),
                        None,
                    ));
                }
            }
            #[cfg(windows)]
            {
                let _ = st.child.kill();
            }
            Ok(Value::none())
        }

        fn kill(#[state_mut] st: &mut SubprocessTransportState) -> Result<Value> {
            let _ = _interp;
            if st.returncode.is_some() {
                return Err(PyError::named("ProcessLookupError", "process already exited"));
            }
            st.child
                .kill()
                .map_err(|e| PyError::from_io_error(&e, None))?;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stdin_t = with_state_mut(self_val.0, |st| {
                st.closed = true;
                Ok(st.stdin_transport.clone())
            })?;
            if let Some(t) = stdin_t {
                let _ = io_engine::pipe_write_close(_interp, &t);
            }
            Ok(Value::none())
        }
    }

    // ── Servers, Event Loops, Policies, Runner ───────────────────────────────

    class AbstractServer {
        fn close(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn get_loop(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn is_serving(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn start_serving(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn serve_forever(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn wait_closed(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn __aenter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ServerAenter {
                    server: self_val.0.clone(),
                },
            )
        }

        fn __aexit__(
            #[positional_only] self_val: PyValueRef<'_>,
            _et: PyValue,
            _exc: PyValue,
            _tb: PyValue,
        ) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ServerAexit {
                    server: self_val.0.clone(),
                    wait_coro: None,
                },
            )
        }
    }

    #[native_state(ServerState)]
    class Server("AbstractServer") {
        #[getter]
        fn sockets(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let socks = with_state(self_val.0, |st| Ok(st.sockets.clone()))?;
            let mut wrapped = Vec::with_capacity(socks.len());
            for s in socks {
                let ts = asyncio_rt::alloc_asyncio_instance(_interp, "TransportSocket")?;
                set_native_state(&ts, TransportSocketState { sock: s })?;
                wrapped.push(ts);
            }
            Ok(Value::tuple(wrapped))
        }

        fn get_loop(#[state] st: &ServerState) -> Result<Value> {
            let _ = _interp;
            Ok(st.loop_val.clone())
        }

        fn is_serving(#[state] st: &ServerState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.serving && !st.closed))
        }

        fn start_serving(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::server_start_serving(_interp, self_val.0)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::ReadyValue {
                    result: Value::none(),
                },
            )
        }

        fn serve_forever(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ServerServeForever {
                    server: self_val.0.clone(),
                    started: false,
                    waiter: None,
                },
            )
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::server_close(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn wait_closed(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ServerWaitClosed {
                    server: self_val.0.clone(),
                    waiter: None,
                },
            )
        }

        fn _accept_connection(
            #[positional_only] self_val: PyValueRef<'_>,
            sock: PyValue,
        ) -> Result<Value> {
            io_engine::server_accept_connection(_interp, self_val.0, &sock.0)?;
            Ok(Value::none())
        }

        fn __repr__(#[state] st: &ServerState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::string(format!(
                "<Server sockets=({} sockets)>",
                st.sockets.len()
            )))
        }
    }

    class AbstractEventLoop {
        fn run_forever(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn run_until_complete(
            #[positional_only] _self: PyValueRef<'_>,
            _future: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn stop(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn is_running(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn is_closed(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn close(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn time(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn call_soon(
            #[positional_only] _self: PyValueRef<'_>,
            _callback: PyValue,
            #[varargs] _args: &[Value],
            #[default(OptionalArg::Omitted)] _context: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn call_soon_threadsafe(
            #[positional_only] _self: PyValueRef<'_>,
            _callback: PyValue,
            #[varargs] _args: &[Value],
            #[default(OptionalArg::Omitted)] _context: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn call_later(
            #[positional_only] _self: PyValueRef<'_>,
            _delay: PyNumber,
            _callback: PyValue,
            #[varargs] _args: &[Value],
            #[default(OptionalArg::Omitted)] _context: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn call_at(
            #[positional_only] _self: PyValueRef<'_>,
            _when: PyNumber,
            _callback: PyValue,
            #[varargs] _args: &[Value],
            #[default(OptionalArg::Omitted)] _context: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn create_future(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn create_task(
            #[positional_only] _self: PyValueRef<'_>,
            _coro: PyValue,
            #[keyword_only] #[default(OptionalArg::Omitted)] _name: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _context: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }
    }

    #[native_state(LoopState)]
    class BaseEventLoop("AbstractEventLoop") {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            set_state(self_val.0, LoopState::new())?;
            Ok(Value::none())
        }

        #[getter]
        fn slow_callback_duration(#[state] st: &LoopState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::float(st.slow_callback_duration))
        }

        #[setter("slow_callback_duration")]
        fn set_slow_callback_duration(
            #[state_mut] st: &mut LoopState,
            val: PyNumber,
        ) -> Result<Value> {
            let _ = _interp;
            st.slow_callback_duration = val.as_f64();
            Ok(Value::none())
        }

        fn time(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::float(asyncio_rt::monotonic_secs()))
        }

        fn is_running(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            let running = asyncio_rt::get_running_loop_opt()
                .is_some_and(|rl| asyncio_rt::same_object(&rl, self_val.0));
            Ok(Value::bool_(running))
        }

        fn is_closed(#[state] st: &LoopState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.closed))
        }

        fn stop(#[state_mut] st: &mut LoopState) -> Result<Value> {
            let _ = _interp;
            st.stopping = true;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            let is_running = asyncio_rt::get_running_loop_opt()
                .is_some_and(|rl| asyncio_rt::same_object(&rl, self_val.0));
            if is_running {
                return Err(PyError::named(
                    "RuntimeError",
                    "Cannot close a running event loop",
                ));
            }
            with_state_mut(self_val.0, |st| {
                st.closed = true;
                st.ready.clear();
                st.scheduled.clear();
                st.readers.clear();
                st.writers.clear();
                st.active_subprocesses.clear();
                Ok(())
            })?;
            asyncio_rt::clear_loop_tasks(self_val.0);
            Ok(Value::none())
        }

        fn get_debug(#[state] st: &LoopState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.debug))
        }

        fn set_debug(#[state_mut] st: &mut LoopState, enabled: PyBool) -> Result<Value> {
            let _ = _interp;
            st.debug = enabled.0;
            Ok(Value::none())
        }

        fn set_task_factory(
            #[state_mut] st: &mut LoopState,
            factory: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            let f = opt_value_cloned(factory);
            if let Some(ref fv) = f {
                let is_call = asyncio_rt::call_pos(
                    _interp,
                    Value::builtin_function("callable"),
                    std::slice::from_ref(fv),
                )?;
                if !_interp.truthy_value(&is_call)? {
                    return Err(PyError::named(
                        "TypeError",
                        "task factory must be a callable or None",
                    ));
                }
            }
            st.task_factory = f;
            Ok(Value::none())
        }

        fn get_task_factory(#[state] st: &LoopState) -> Result<Value> {
            let _ = _interp;
            Ok(st.task_factory.clone().unwrap_or_else(Value::none))
        }

        fn set_exception_handler(
            #[state_mut] st: &mut LoopState,
            handler: OptValueRef<'_>,
        ) -> Result<Value> {
            let h = opt_value_cloned(handler);
            if let Some(ref hv) = h {
                let is_call = asyncio_rt::call_pos(
                    _interp,
                    Value::builtin_function("callable"),
                    std::slice::from_ref(hv),
                )?;
                if !_interp.truthy_value(&is_call)? {
                    return Err(PyError::named(
                        "TypeError",
                        "A callable object or None is expected",
                    ));
                }
            }
            st.exception_handler = h;
            Ok(Value::none())
        }

        fn get_exception_handler(#[state] st: &LoopState) -> Result<Value> {
            let _ = _interp;
            Ok(st.exception_handler.clone().unwrap_or_else(Value::none))
        }

        fn default_exception_handler(
            #[positional_only] _self: PyValueRef<'_>,
            _context: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn call_exception_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            context: PyValue,
        ) -> Result<Value> {
            let handler = with_state(self_val.0, |st| Ok(st.exception_handler.clone()))?;
            if let Some(h) = handler {
                let _ = asyncio_rt::call_pos(_interp, h, &[self_val.0.clone(), context.0]);
            } else {
                let _ = asyncio_rt::call_method(
                    _interp,
                    self_val.0,
                    "default_exception_handler",
                    &[context.0],
                );
            }
            Ok(Value::none())
        }

        fn create_future(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            asyncio_rt::create_future_on_loop(_interp, self_val.0.clone())
        }

        fn create_task(
            #[positional_only] self_val: PyValueRef<'_>,
            coro: PyValue,
            #[keyword_only] #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let task_name = match opt_value_cloned(name) {
                Some(v) => {
                    let s = asyncio_rt::call_pos(_interp, Value::builtin_function("str"), &[v])?;
                    Some(s.as_str().unwrap_or("").to_owned())
                }
                None => None,
            };
            io_engine::loop_create_task_native(
                _interp,
                self_val.0,
                coro.0,
                task_name,
                opt_value_cloned(context),
            )
        }

        fn call_soon(
            #[positional_only] self_val: PyValueRef<'_>,
            callback: PyValue,
            #[varargs] args: &[Value],
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            io_engine::loop_call_soon_native(
                _interp,
                self_val.0,
                callback.0,
                args.to_vec(),
                opt_value_cloned(context),
            )
        }

        fn call_soon_threadsafe(
            #[positional_only] self_val: PyValueRef<'_>,
            callback: PyValue,
            #[varargs] args: &[Value],
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            io_engine::loop_call_soon_native(
                _interp,
                self_val.0,
                callback.0,
                args.to_vec(),
                opt_value_cloned(context),
            )
        }

        fn call_at(
            #[positional_only] self_val: PyValueRef<'_>,
            when: PyNumber,
            callback: PyValue,
            #[varargs] args: &[Value],
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            io_engine::loop_call_at_native(
                _interp,
                self_val.0,
                when.as_f64(),
                callback.0,
                args.to_vec(),
                opt_value_cloned(context),
            )
        }

        fn call_later(
            #[positional_only] self_val: PyValueRef<'_>,
            delay: PyNumber,
            callback: PyValue,
            #[varargs] args: &[Value],
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let when = asyncio_rt::monotonic_secs() + delay.as_f64();
            io_engine::loop_call_at_native(
                _interp,
                self_val.0,
                when,
                callback.0,
                args.to_vec(),
                opt_value_cloned(context),
            )
        }

        fn run_forever(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::loop_run_forever(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn run_until_complete(
            #[positional_only] self_val: PyValueRef<'_>,
            future: PyValue,
        ) -> Result<Value> {
            io_engine::loop_run_until_complete(_interp, self_val.0, future.0)
        }

        fn _stop_on_done(
            #[state_mut] st: &mut LoopState,
            _future: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            st.stopping = true;
            Ok(Value::none())
        }

        fn shutdown_asyncgens(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::ReadyValue {
                    result: Value::none(),
                },
            )
        }

        fn shutdown_default_executor(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let exec = with_state_mut(self_val.0, |st| Ok(st.default_executor.take()))?;
            if let Some(ex) = exec
                && let Ok(shutdown_m) = _interp.get_attr(&ex, "shutdown")
            {
                let _ = asyncio_rt::call_kw(
                    _interp,
                    shutdown_m,
                    &[],
                    &[("wait", Value::bool_(true))],
                );
            }
            awaitable::make_native_coro(
                _interp,
                CoroBody::ReadyValue {
                    result: Value::none(),
                },
            )
        }

        fn set_default_executor(
            #[state_mut] st: &mut LoopState,
            executor: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            st.default_executor = if executor.0.is_none() {
                None
            } else {
                Some(executor.0)
            };
            Ok(Value::none())
        }

        fn run_in_executor(
            #[positional_only] self_val: PyValueRef<'_>,
            executor: PyValue,
            func: PyValue,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let chosen_exec = if executor.0.is_none() {
                with_state(self_val.0, |st| Ok(st.default_executor.clone()))?
            } else {
                Some(executor.0)
            };
            if let Some(ex) = chosen_exec {
                let mut pos = Vec::with_capacity(1 + args.len());
                pos.push(func.0);
                pos.extend(args.iter().cloned());
                let cf = asyncio_rt::call_method(_interp, &ex, "submit", &pos)?;
                let wf = asyncio_rt::asyncio_attr(_interp, "wrap_future")?;
                return asyncio_rt::call_kw(
                    _interp,
                    wf,
                    &[cf],
                    &[("loop", self_val.0.clone())],
                );
            }
            // Single-threaded default executor: schedule on next loop turn and resolve Future.
            let fut = asyncio_rt::create_future_on_loop(_interp, self_val.0.clone())?;
            let runner = asyncio_rt::asyncio_attr(_interp, "_run_executor_cb")?;
            let mut cb_args = Vec::with_capacity(2 + args.len());
            cb_args.push(fut.clone());
            cb_args.push(func.0);
            cb_args.extend(args.iter().cloned());
            io_engine::loop_call_soon_native(_interp, self_val.0, runner, cb_args, None)?;
            Ok(fut)
        }

        fn add_reader(
            #[positional_only] self_val: PyValueRef<'_>,
            fd: PyValue,
            callback: PyValue,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let fd_n = asyncio_rt::extract_fd(_interp, &fd.0)?;
            io_engine::loop_add_reader(_interp, self_val.0, fd_n, callback.0, args.to_vec())?;
            Ok(Value::none())
        }

        fn remove_reader(
            #[positional_only] self_val: PyValueRef<'_>,
            fd: PyValue,
        ) -> Result<Value> {
            let fd_n = asyncio_rt::extract_fd(_interp, &fd.0)?;
            let removed = io_engine::loop_remove_reader(self_val.0, fd_n)?;
            Ok(Value::bool_(removed))
        }

        fn add_writer(
            #[positional_only] self_val: PyValueRef<'_>,
            fd: PyValue,
            callback: PyValue,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let fd_n = asyncio_rt::extract_fd(_interp, &fd.0)?;
            io_engine::loop_add_writer(_interp, self_val.0, fd_n, callback.0, args.to_vec())?;
            Ok(Value::none())
        }

        fn remove_writer(
            #[positional_only] self_val: PyValueRef<'_>,
            fd: PyValue,
        ) -> Result<Value> {
            let fd_n = asyncio_rt::extract_fd(_interp, &fd.0)?;
            let removed = io_engine::loop_remove_writer(self_val.0, fd_n)?;
            Ok(Value::bool_(removed))
        }

        fn sock_recv(
            #[positional_only] self_val: PyValueRef<'_>,
            sock: PyValue,
            nbytes: PyIndex,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let nb = nbytes.expect_i64(FN_NAME, "nbytes")?;
            let fd = asyncio_rt::extract_fd(_interp, &sock.0)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopSockRecv {
                    loop_val: self_val.0.clone(),
                    sock: sock.0,
                    nbytes: nb,
                    fd,
                    waiter: None,
                },
            )
        }

        fn sock_recv_into(
            #[positional_only] self_val: PyValueRef<'_>,
            sock: PyValue,
            buf: PyValue,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let fd = asyncio_rt::extract_fd(_interp, &sock.0)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopSockRecvInto {
                    loop_val: self_val.0.clone(),
                    sock: sock.0,
                    buf: buf.0,
                    fd,
                    waiter: None,
                },
            )
        }

        fn sock_sendall(
            #[positional_only] self_val: PyValueRef<'_>,
            sock: PyValue,
            data: PyBytesLike<'_>,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let fd = asyncio_rt::extract_fd(_interp, &sock.0)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopSockSendall {
                    loop_val: self_val.0.clone(),
                    sock: sock.0,
                    data: data.as_bytes().to_vec(),
                    offset: 0,
                    fd,
                    waiter: None,
                },
            )
        }

        fn sock_connect(
            #[positional_only] self_val: PyValueRef<'_>,
            sock: PyValue,
            address: PyValue,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let fd = asyncio_rt::extract_fd(_interp, &sock.0)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopSockConnect {
                    loop_val: self_val.0.clone(),
                    sock: sock.0,
                    address: address.0,
                    fd,
                    started: false,
                    waiter: None,
                },
            )
        }

        fn sock_accept(
            #[positional_only] self_val: PyValueRef<'_>,
            sock: PyValue,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let fd = asyncio_rt::extract_fd(_interp, &sock.0)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopSockAccept {
                    loop_val: self_val.0.clone(),
                    sock: sock.0,
                    fd,
                    waiter: None,
                },
            )
        }

        fn getaddrinfo(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyValue,
            port: PyValue,
            #[keyword_only] #[default(0_i64.into())] family: PyIndex,
            #[keyword_only] #[default(0_i64.into())] r#type: PyIndex,
            #[keyword_only] #[default(0_i64.into())] proto: PyIndex,
            #[keyword_only] #[default(0_i64.into())] flags: PyIndex,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let sock_mod = _interp.load_module("socket")?;
            let gai = _interp.get_attr(&sock_mod, "getaddrinfo")?;
            let res = asyncio_rt::call_pos(
                _interp,
                gai,
                &[
                    host.0,
                    port.0,
                    Value::int(family.expect_i64(FN_NAME, "family")?),
                    Value::int(r#type.expect_i64(FN_NAME, "type")?),
                    Value::int(proto.expect_i64(FN_NAME, "proto")?),
                    Value::int(flags.expect_i64(FN_NAME, "flags")?),
                ],
            )?;
            awaitable::make_native_coro(_interp, CoroBody::ReadyValue { result: res })
        }

        fn getnameinfo(
            #[positional_only] self_val: PyValueRef<'_>,
            sockaddr: PyValue,
            #[default(0_i64.into())] flags: PyIndex,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let sock_mod = _interp.load_module("socket")?;
            let gni = _interp.get_attr(&sock_mod, "getnameinfo")?;
            let res = asyncio_rt::call_pos(
                _interp,
                gni,
                &[sockaddr.0, Value::int(flags.expect_i64(FN_NAME, "flags")?)],
            )?;
            awaitable::make_native_coro(_interp, CoroBody::ReadyValue { result: res })
        }

        fn create_connection(
            #[positional_only] self_val: PyValueRef<'_>,
            protocol_factory: PyValue,
            #[default(OptionalArg::Omitted)] host: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] port: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] ssl: OptValueRef<'_>,
            #[keyword_only] #[default(0_i64.into())] family: PyIndex,
            #[keyword_only] #[default(0_i64.into())] _proto: PyIndex,
            #[keyword_only] #[default(0_i64.into())] _flags: PyIndex,
            #[keyword_only] #[default(OptionalArg::Omitted)] sock: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _local_addr: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] server_hostname: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_handshake_timeout: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_shutdown_timeout: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _happy_eyeballs_delay: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _interleave: OptValueRef<'_>,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let host_opt = opt_none_or_str(host)?;
            let port_opt = opt_none_or_i64(_interp, port, FN_NAME, "port")?;
            let fam = family.expect_i64(FN_NAME, "family")?;
            let sh = opt_none_or_str(server_hostname)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopCreateConnection {
                    loop_val: self_val.0.clone(),
                    protocol_factory: protocol_factory.0,
                    host: host_opt,
                    port: port_opt,
                    ssl_arg: opt_value_cloned(ssl),
                    family: fam,
                    sock: opt_value_cloned(sock),
                    server_hostname: sh,
                    phase: CreateConnPhase::Init,
                },
            )
        }

        fn create_server(
            #[positional_only] self_val: PyValueRef<'_>,
            protocol_factory: PyValue,
            #[default(OptionalArg::Omitted)] host: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] port: OptValueRef<'_>,
            #[keyword_only] #[default(0_i64.into())] family: PyIndex,
            #[keyword_only] #[default(OptionalArg::Omitted)] _flags: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] sock: OptValueRef<'_>,
            #[keyword_only] #[default(100_i64.into())] backlog: PyIndex,
            #[keyword_only] #[default(OptionalArg::Omitted)] ssl: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] reuse_address: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _reuse_port: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_handshake_timeout: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_shutdown_timeout: OptValueRef<'_>,
            #[keyword_only] #[default(true.into())] start_serving: PyBool,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let port_opt = opt_none_or_i64(_interp, port, FN_NAME, "port")?;
            let fam = family.expect_i64(FN_NAME, "family")?;
            let bl = backlog.expect_i64(FN_NAME, "backlog")?;
            let ra = opt_none_or_bool(_interp, reuse_address)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopCreateServer {
                    loop_val: self_val.0.clone(),
                    protocol_factory: protocol_factory.0,
                    host: opt_value_cloned(host),
                    port: port_opt,
                    family: fam,
                    backlog: bl,
                    ssl_arg: opt_value_cloned(ssl),
                    reuse_address: ra,
                    sock: opt_value_cloned(sock),
                    start_serving: start_serving.0,
                },
            )
        }

        fn create_datagram_endpoint(
            #[positional_only] self_val: PyValueRef<'_>,
            protocol_factory: PyValue,
            #[default(OptionalArg::Omitted)] local_addr: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] remote_addr: OptValueRef<'_>,
            #[keyword_only] #[default(0_i64.into())] family: PyIndex,
            #[keyword_only] #[default(0_i64.into())] _proto: PyIndex,
            #[keyword_only] #[default(0_i64.into())] _flags: PyIndex,
            #[keyword_only] #[default(OptionalArg::Omitted)] _reuse_port: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _allow_broadcast: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] sock: OptValueRef<'_>,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let fam = family.expect_i64(FN_NAME, "family")?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopCreateDatagramEndpoint {
                    loop_val: self_val.0.clone(),
                    protocol_factory: protocol_factory.0,
                    local_addr: opt_value_cloned(local_addr),
                    remote_addr: opt_value_cloned(remote_addr),
                    family: fam,
                    sock: opt_value_cloned(sock),
                },
            )
        }

        fn start_tls(
            #[positional_only] self_val: PyValueRef<'_>,
            transport: PyValue,
            protocol: PyValue,
            sslcontext: PyValue,
            #[keyword_only] #[default(false.into())] server_side: PyBool,
            #[keyword_only] #[default(OptionalArg::Omitted)] server_hostname: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_handshake_timeout: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_shutdown_timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let sh = opt_none_or_str(server_hostname)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopStartTls {
                    loop_val: self_val.0.clone(),
                    transport: transport.0,
                    protocol: protocol.0,
                    sslcontext: sslcontext.0,
                    server_side: server_side.0,
                    server_hostname: sh,
                    waiter: None,
                },
            )
        }

        fn subprocess_exec(
            #[positional_only] self_val: PyValueRef<'_>,
            protocol_factory: PyValue,
            program: PyValue,
            #[varargs] args: &[Value],
            #[default(OptionalArg::Omitted)] stdin: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] stdout: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] stderr: OptValueRef<'_>,
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let prog_str = value_to_str_arg(_interp, &program.0)?;
            let mut str_args = Vec::with_capacity(args.len());
            for a in args {
                str_args.push(value_to_str_arg(_interp, a)?);
            }
            let cwd = match kwargs.get("cwd") {
                Some(v) if !v.is_none() => Some(value_to_str_arg(_interp, v)?),
                _ => None,
            };
            let env = match kwargs.get("env") {
                Some(v) if !v.is_none() => Some(io_engine::extract_env_pairs(v)?),
                _ => None,
            };
            let stdin_v = match stdin.as_value() {
                None => Some(Value::int(PIPE_SENTINEL)),
                Some(v) if v.is_none() => None,
                Some(v) => Some(v.clone()),
            };
            let stdout_v = match stdout.as_value() {
                None => Some(Value::int(PIPE_SENTINEL)),
                Some(v) if v.is_none() => None,
                Some(v) => Some(v.clone()),
            };
            let stderr_v = match stderr.as_value() {
                None => Some(Value::int(PIPE_SENTINEL)),
                Some(v) if v.is_none() => None,
                Some(v) => Some(v.clone()),
            };
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopSubprocess {
                    loop_val: self_val.0.clone(),
                    protocol_factory: protocol_factory.0,
                    program: prog_str,
                    args: str_args,
                    shell: false,
                    stdin: stdin_v,
                    stdout: stdout_v,
                    stderr: stderr_v,
                    cwd,
                    env,
                },
            )
        }

        fn subprocess_shell(
            #[positional_only] self_val: PyValueRef<'_>,
            protocol_factory: PyValue,
            cmd: PyValue,
            #[keyword_only] #[default(OptionalArg::Omitted)] stdin: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] stdout: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] stderr: OptValueRef<'_>,
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            io_engine::check_loop_open(self_val.0)?;
            let cmd_str = value_to_str_arg(_interp, &cmd.0)?;
            let cwd = match kwargs.get("cwd") {
                Some(v) if !v.is_none() => Some(value_to_str_arg(_interp, v)?),
                _ => None,
            };
            let env = match kwargs.get("env") {
                Some(v) if !v.is_none() => Some(io_engine::extract_env_pairs(v)?),
                _ => None,
            };
            let stdin_v = match stdin.as_value() {
                None => Some(Value::int(PIPE_SENTINEL)),
                Some(v) if v.is_none() => None,
                Some(v) => Some(v.clone()),
            };
            let stdout_v = match stdout.as_value() {
                None => Some(Value::int(PIPE_SENTINEL)),
                Some(v) if v.is_none() => None,
                Some(v) => Some(v.clone()),
            };
            let stderr_v = match stderr.as_value() {
                None => Some(Value::int(PIPE_SENTINEL)),
                Some(v) if v.is_none() => None,
                Some(v) => Some(v.clone()),
            };
            awaitable::make_native_coro(
                _interp,
                CoroBody::LoopSubprocess {
                    loop_val: self_val.0.clone(),
                    protocol_factory: protocol_factory.0,
                    program: cmd_str,
                    args: Vec::new(),
                    shell: true,
                    stdin: stdin_v,
                    stdout: stdout_v,
                    stderr: stderr_v,
                    cwd,
                    env,
                },
            )
        }
    }

    #[native_state(LoopState)]
    class BaseSelectorEventLoop("BaseEventLoop") {}

    #[native_state(LoopState)]
    class SelectorEventLoop("BaseSelectorEventLoop") {}

    class AbstractEventLoopPolicy {
        fn get_event_loop(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn set_event_loop(
            #[positional_only] _self: PyValueRef<'_>,
            _loop_val: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn new_event_loop(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn get_child_watcher(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }

        fn set_child_watcher(
            #[positional_only] _self: PyValueRef<'_>,
            _watcher: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("NotImplementedError", String::new()))
        }
    }

    #[native_state(DefaultPolicyState)]
    class BaseDefaultEventLoopPolicy("AbstractEventLoopPolicy") {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            set_state(
                self_val.0,
                DefaultPolicyState {
                    local_loop: None,
                    set_called: false,
                },
            )?;
            Ok(Value::none())
        }

        fn new_event_loop(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let cls = asyncio_rt::asyncio_attr(_interp, "SelectorEventLoop")?;
            asyncio_rt::call_pos(_interp, cls, &[])
        }

        fn get_event_loop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (existing, set_called) =
                with_state(self_val.0, |st| Ok((st.local_loop.clone(), st.set_called)))?;
            if let Some(lp) = existing {
                return Ok(lp);
            }
            if !set_called {
                let new_lp = asyncio_rt::call_method(_interp, self_val.0, "new_event_loop", &[])?;
                with_state_mut(self_val.0, |st| {
                    st.local_loop = Some(new_lp.clone());
                    st.set_called = true;
                    Ok(())
                })?;
                return Ok(new_lp);
            }
            Err(PyError::named(
                "RuntimeError",
                "There is no current event loop in thread 'MainThread'.",
            ))
        }

        fn set_event_loop(
            #[state_mut] st: &mut DefaultPolicyState,
            loop_val: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            st.set_called = true;
            st.local_loop = if loop_val.0.is_none() {
                None
            } else {
                Some(loop_val.0)
            };
            Ok(Value::none())
        }
    }

    #[native_state(DefaultPolicyState)]
    class DefaultEventLoopPolicy("BaseDefaultEventLoopPolicy") {}

    // ── Runner (Python 3.11+ context manager) ────────────────────────────────

    #[native_state(RunnerState)]
    class Runner {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] debug: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] loop_factory: OptValueRef<'_>,
        ) -> Result<Value> {
            let dbg = opt_none_or_bool(_interp, debug)?;
            set_state(
                self_val.0,
                RunnerState {
                    debug: dbg,
                    loop_factory: opt_value_cloned(loop_factory),
                    loop_val: None,
                    context: None,
                    state: RunnerLifecycle::Created,
                },
            )?;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = asyncio_rt::call_method(_interp, self_val.0, "_lazy_init", &[])?;
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            _exc_type: PyValue,
            _exc_val: PyValue,
            _exc_tb: PyValue,
        ) -> Result<Value> {
            let _ = asyncio_rt::call_method(_interp, self_val.0, "close", &[])?;
            Ok(Value::none())
        }

        fn _lazy_init(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (state, loop_factory, debug) = with_state(self_val.0, |st| {
                Ok((st.state, st.loop_factory.clone(), st.debug))
            })?;
            if state == RunnerLifecycle::Closed {
                return Err(PyError::named("RuntimeError", "Runner is closed"));
            }
            if state == RunnerLifecycle::Initialized {
                return Ok(Value::none());
            }
            let loop_val = match loop_factory {
                Some(f) => asyncio_rt::call_pos(_interp, f, &[])?,
                None => {
                    let cls = asyncio_rt::asyncio_attr(_interp, "SelectorEventLoop")?;
                    asyncio_rt::call_pos(_interp, cls, &[])?
                }
            };
            if let Some(d) = debug {
                let _ = asyncio_rt::call_method(_interp, &loop_val, "set_debug", &[Value::bool_(d)]);
            }
            let ctx = asyncio_rt::copy_context(_interp)?;
            with_state_mut(self_val.0, |st| {
                st.loop_val = Some(loop_val);
                st.context = Some(ctx);
                st.state = RunnerLifecycle::Initialized;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn get_loop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = asyncio_rt::call_method(_interp, self_val.0, "_lazy_init", &[])?;
            with_state(self_val.0, |st| {
                st.loop_val
                    .clone()
                    .ok_or_else(|| PyError::named("RuntimeError", "Runner is closed"))
            })
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (state, loop_opt) = with_state_mut(self_val.0, |st| {
                let prev = st.state;
                st.state = RunnerLifecycle::Closed;
                Ok((prev, st.loop_val.take()))
            })?;
            if state != RunnerLifecycle::Initialized {
                return Ok(Value::none());
            }
            if let Some(loop_val) = loop_opt {
                let _ = cancel_all_tasks_on_loop(_interp, &loop_val);
                if let Ok(ag_coro) = asyncio_rt::call_method(_interp, &loop_val, "shutdown_asyncgens", &[]) {
                    let _ = asyncio_rt::call_method(_interp, &loop_val, "run_until_complete", &[ag_coro]);
                }
                if let Ok(ex_coro) =
                    asyncio_rt::call_method(_interp, &loop_val, "shutdown_default_executor", &[])
                {
                    let _ = asyncio_rt::call_method(_interp, &loop_val, "run_until_complete", &[ex_coro]);
                }
                let _ = asyncio_rt::call_method(_interp, &loop_val, "close", &[]);
            }
            Ok(Value::none())
        }

        fn run(
            #[positional_only] self_val: PyValueRef<'_>,
            coro: PyValue,
            #[keyword_only] #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            if !asyncio_rt::is_coroutine_obj(&coro.0) {
                let r = render_value_repr(_interp, &coro.0)?;
                return Err(PyError::named(
                    "ValueError",
                    format!("a coroutine was expected, got {r}"),
                ));
            }
            if asyncio_rt::is_coro_done(&coro.0) {
                return Err(PyError::named(
                    "RuntimeError",
                    "cannot reuse already awaited coroutine",
                ));
            }
            if asyncio_rt::get_running_loop_opt().is_some() {
                return Err(PyError::named(
                    "RuntimeError",
                    "Runner.run() cannot be called from a running event loop",
                ));
            }
            let _ = asyncio_rt::call_method(_interp, self_val.0, "_lazy_init", &[])?;
            let (loop_val, default_ctx) = with_state(self_val.0, |st| {
                Ok((
                    st.loop_val
                        .clone()
                        .ok_or_else(|| PyError::named("RuntimeError", "Runner is closed"))?,
                    st.context.clone(),
                ))
            })?;
            let ctx = opt_value_cloned(context).or(default_ctx);
            let task = asyncio_rt::loop_create_task(_interp, &loop_val, coro.0, None, ctx)?;
            asyncio_rt::call_method(_interp, &loop_val, "run_until_complete", &[task])
        }
    }

    // ── Streams: StreamReader, StreamReaderProtocol, StreamWriter ────────────

    #[native_state(StreamReaderState)]
    class StreamReader {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default((DEFAULT_STREAM_LIMIT as i64).into())] limit: PyIndex,
            #[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            let lim = limit.expect_i64(FN_NAME, "limit")?;
            if lim <= 0 {
                return Err(PyError::named("ValueError", "Limit cannot be <= 0"));
            }
            set_state(
                self_val.0,
                StreamReaderState {
                    limit: lim as usize,
                    loop_val: opt_value_cloned(r#loop),
                    buffer: Vec::new(),
                    eof: false,
                    waiter: None,
                    exception: None,
                    transport: None,
                    paused: false,
                },
            )?;
            Ok(Value::none())
        }

        fn exception(#[state] st: &StreamReaderState) -> Result<Value> {
            let _ = _interp;
            Ok(st.exception.clone().unwrap_or_else(Value::none))
        }

        fn set_exception(
            #[positional_only] self_val: PyValueRef<'_>,
            exc: PyValue,
        ) -> Result<Value> {
            let waiter = with_state_mut(self_val.0, |st| {
                st.exception = Some(exc.0.clone());
                Ok(st.waiter.take())
            })?;
            if let Some(w) = waiter {
                asyncio_rt::future_set_exception_if_pending(_interp, &w, exc.0)?;
            }
            Ok(Value::none())
        }

        fn _set_transport(
            #[state_mut] st: &mut StreamReaderState,
            transport: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            st.transport = Some(transport.0);
            Ok(Value::none())
        }

        fn feed_eof(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                st.eof = true;
                Ok(())
            })?;
            awaitable::stream_reader_wakeup_waiter(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn at_eof(#[state] st: &StreamReaderState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.eof && st.buffer.is_empty()))
        }

        fn feed_data(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
        ) -> Result<Value> {
            let bytes = data.as_bytes();
            if bytes.is_empty() {
                return Ok(Value::none());
            }
            let to_pause = with_state_mut(self_val.0, |st| {
                st.buffer.extend_from_slice(bytes);
                if !st.paused
                    && st.buffer.len() > 2 * st.limit
                    && let Some(ref t) = st.transport
                {
                    st.paused = true;
                    return Ok(Some(t.clone()));
                }
                Ok(None)
            })?;
            awaitable::stream_reader_wakeup_waiter(_interp, self_val.0)?;
            if let Some(t) = to_pause {
                let _ = asyncio_rt::call_method(_interp, &t, "pause_reading", &[]);
            }
            Ok(Value::none())
        }

        fn read(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default((-1_i64).into())] n: PyIndex,
        ) -> Result<Value> {
            let n_val = n.expect_i64(FN_NAME, "n")?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::StreamRead {
                    reader: self_val.0.clone(),
                    n: n_val,
                    waiter: None,
                },
            )
        }

        fn readline(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::StreamReadline {
                    reader: self_val.0.clone(),
                    waiter: None,
                },
            )
        }

        fn readexactly(
            #[positional_only] self_val: PyValueRef<'_>,
            n: PyIndex,
        ) -> Result<Value> {
            let n_val = n.expect_i64(FN_NAME, "n")?;
            if n_val < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "readexactly size can not be less than zero",
                ));
            }
            awaitable::make_native_coro(
                _interp,
                CoroBody::StreamReadexactly {
                    reader: self_val.0.clone(),
                    n: n_val as usize,
                    waiter: None,
                },
            )
        }

        fn readuntil(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] separator: OptValueRef<'_>,
        ) -> Result<Value> {
            let sep = match separator.as_value() {
                None => b"\n".to_vec(),
                Some(v) => {
                    let bl = PyBytesLike::try_from_value_with_interp(
                        _interp,
                        v,
                        FN_NAME,
                        "separator",
                    )?;
                    bl.as_bytes().to_vec()
                }
            };
            awaitable::make_native_coro(
                _interp,
                CoroBody::StreamReaduntil {
                    reader: self_val.0.clone(),
                    sep,
                    waiter: None,
                },
            )
        }

        fn __aiter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(self_val.0.clone())
        }

        fn __anext__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::StreamAnext {
                    reader: self_val.0.clone(),
                    readline_coro: None,
                },
            )
        }

        fn __repr__(#[state] st: &StreamReaderState) -> Result<Value> {
            let _ = _interp;
            let mut info = vec!["StreamReader".to_string()];
            if !st.buffer.is_empty() {
                info.push(format!("{} bytes", st.buffer.len()));
            }
            if st.eof {
                info.push("eof".to_string());
            }
            if st.limit != DEFAULT_STREAM_LIMIT {
                info.push(format!("limit={}", st.limit));
            }
            Ok(Value::string(format!("<{}>", info.join(" "))))
        }
    }

    #[native_state(StreamReaderProtocolState)]
    class StreamReaderProtocol("Protocol") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            stream_reader: PyValue,
            #[default(OptionalArg::Omitted)] client_connected_cb: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>,
        ) -> Result<Value> {
            let lv = match opt_value_cloned(r#loop) {
                Some(l) => l,
                None => asyncio_rt::get_event_loop_compat(_interp)?,
            };
            set_state(
                self_val.0,
                StreamReaderProtocolState {
                    stream_reader: if stream_reader.0.is_none() {
                        None
                    } else {
                        Some(stream_reader.0)
                    },
                    client_connected_cb: opt_value_cloned(client_connected_cb),
                    loop_val: lv,
                    over_ssl: false,
                    transport: None,
                    paused: false,
                    drain_waiter: None,
                    connection_lost: false,
                    closed_fut: None,
                    client_handler_task: None,
                    stream_writer: None,
                },
            )?;
            Ok(Value::none())
        }

        fn connection_made(
            #[positional_only] self_val: PyValueRef<'_>,
            transport: PyValue,
        ) -> Result<Value> {
            let (reader_opt, cb_opt, loop_val) = with_state_mut(self_val.0, |st| {
                st.transport = Some(transport.0.clone());
                Ok((
                    st.stream_reader.clone(),
                    st.client_connected_cb.clone(),
                    st.loop_val.clone(),
                ))
            })?;
            let ssl_ctx = asyncio_rt::call_method(
                _interp,
                &transport.0,
                "get_extra_info",
                &[Value::string("sslcontext")],
            )
            .ok();
            let over_ssl = ssl_ctx.is_some_and(|v| !v.is_none());
            with_state_mut(self_val.0, |st| {
                st.over_ssl = over_ssl;
                Ok(())
            })?;

            if let Some(ref reader) = reader_opt {
                let _ = asyncio_rt::call_method(
                    _interp,
                    reader,
                    "_set_transport",
                    std::slice::from_ref(&transport.0),
                );
            }
            if let Some(cb) = cb_opt
                && let Some(ref reader) = reader_opt
            {
                let writer = awaitable::create_stream_writer(
                    _interp,
                    transport.0,
                    self_val.0.clone(),
                    Some(reader.clone()),
                    loop_val.clone(),
                )?;
                with_state_mut(self_val.0, |st| {
                    st.stream_writer = Some(writer.clone());
                    Ok(())
                })?;
                let res = asyncio_rt::call_pos(_interp, cb, &[reader.clone(), writer])?;
                if asyncio_rt::is_coroutine_obj(&res) {
                    let task =
                        asyncio_rt::loop_create_task(_interp, &loop_val, res, None, None)?;
                    with_state_mut(self_val.0, |st| {
                        st.client_handler_task = Some(task);
                        Ok(())
                    })?;
                }
            }
            Ok(Value::none())
        }

        fn data_received(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyValue,
        ) -> Result<Value> {
            let reader_opt = with_state(self_val.0, |st| Ok(st.stream_reader.clone()))?;
            if let Some(reader) = reader_opt {
                let _ = asyncio_rt::call_method(_interp, &reader, "feed_data", &[data.0])?;
            }
            Ok(Value::none())
        }

        fn eof_received(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (reader_opt, over_ssl) =
                with_state(self_val.0, |st| Ok((st.stream_reader.clone(), st.over_ssl)))?;
            if let Some(reader) = reader_opt {
                let _ = asyncio_rt::call_method(_interp, &reader, "feed_eof", &[])?;
            }
            // Return True to keep TCP transport open for half-closed writes, False for SSL.
            Ok(Value::bool_(!over_ssl))
        }

        fn pause_writing(#[state_mut] st: &mut StreamReaderProtocolState) -> Result<Value> {
            let _ = _interp;
            st.paused = true;
            Ok(Value::none())
        }

        fn resume_writing(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let waiter = with_state_mut(self_val.0, |st| {
                st.paused = false;
                Ok(st.drain_waiter.take())
            })?;
            if let Some(w) = waiter {
                asyncio_rt::future_set_result_if_pending(_interp, &w, Value::none())?;
            }
            Ok(Value::none())
        }

        fn connection_lost(
            #[positional_only] self_val: PyValueRef<'_>,
            exc: PyValue,
        ) -> Result<Value> {
            let (reader_opt, drain_w, closed_f) = with_state_mut(self_val.0, |st| {
                st.connection_lost = true;
                st.paused = false;
                Ok((
                    st.stream_reader.clone(),
                    st.drain_waiter.take(),
                    st.closed_fut.take(),
                ))
            })?;
            if let Some(reader) = reader_opt {
                if exc.0.is_none() {
                    let _ = asyncio_rt::call_method(_interp, &reader, "feed_eof", &[]);
                } else {
                    let _ = asyncio_rt::call_method(
                        _interp,
                        &reader,
                        "set_exception",
                        std::slice::from_ref(&exc.0),
                    );
                }
            }
            if let Some(w) = drain_w {
                if exc.0.is_none() {
                    let _ = asyncio_rt::future_set_result_if_pending(_interp, &w, Value::none());
                } else {
                    let _ = asyncio_rt::future_set_exception_if_pending(_interp, &w, exc.0.clone());
                }
            }
            if let Some(cf) = closed_f {
                if exc.0.is_none() {
                    let _ = asyncio_rt::future_set_result_if_pending(_interp, &cf, Value::none());
                } else {
                    let _ = asyncio_rt::future_set_exception_if_pending(_interp, &cf, exc.0);
                }
            }
            Ok(Value::none())
        }
    }

    #[native_state(StreamWriterState)]
    class StreamWriter {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            transport: PyValue,
            protocol: PyValue,
            reader: PyValue,
            loop_val: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            set_state(
                self_val.0,
                StreamWriterState {
                    transport: transport.0,
                    protocol: protocol.0,
                    reader: if reader.0.is_none() {
                        None
                    } else {
                        Some(reader.0)
                    },
                    loop_val: loop_val.0,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn transport(#[state] st: &StreamWriterState) -> Result<Value> {
            let _ = _interp;
            Ok(st.transport.clone())
        }

        fn write(#[state] st: &StreamWriterState, data: PyValue) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "write", &[data.0])
        }

        fn writelines(#[state] st: &StreamWriterState, data: PyValue) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "writelines", &[data.0])
        }

        fn write_eof(#[state] st: &StreamWriterState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "write_eof", &[])
        }

        fn can_write_eof(#[state] st: &StreamWriterState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "can_write_eof", &[])
        }

        fn close(#[state] st: &StreamWriterState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "close", &[])
        }

        fn is_closing(#[state] st: &StreamWriterState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "is_closing", &[])
        }

        fn wait_closed(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::StreamWaitClosed {
                    writer: self_val.0.clone(),
                    waiter: None,
                },
            )
        }

        fn get_extra_info(
            #[state] st: &StreamWriterState,
            name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let def = opt_value_cloned(default).unwrap_or_else(Value::none);
            asyncio_rt::call_method(
                _interp,
                &st.transport,
                "get_extra_info",
                &[Value::string(name.0), def],
            )
        }

        fn drain(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::StreamDrain {
                    writer: self_val.0.clone(),
                    phase: 0,
                    waiter: None,
                },
            )
        }

        fn start_tls(
            #[positional_only] self_val: PyValueRef<'_>,
            sslcontext: PyValue,
            #[keyword_only] #[default(OptionalArg::Omitted)] server_hostname: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_handshake_timeout: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_shutdown_timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let sh = opt_none_or_str(server_hostname)?;
            awaitable::make_native_coro(
                _interp,
                CoroBody::StreamStartTls {
                    writer: self_val.0.clone(),
                    sslcontext: sslcontext.0,
                    server_hostname: sh,
                    waiter: None,
                },
            )
        }

        fn __repr__(#[state] st: &StreamWriterState) -> Result<Value> {
            let tr = render_value_repr(_interp, &st.transport)?;
            Ok(Value::string(format!("<StreamWriter transport={tr}>")))
        }
    }

    // ── Subprocess Stream Protocol & Process ─────────────────────────────────

    #[native_state(SubprocessStreamProtocolState)]
    class SubprocessStreamProtocol("SubprocessProtocol") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            limit: PyIndex,
            loop_val: PyValue,
        ) -> Result<Value> {
            let lim = limit.expect_i64(FN_NAME, "limit")?.max(1) as usize;
            let exited_fut = asyncio_rt::loop_create_future(_interp, &loop_val.0)?;
            set_state(
                self_val.0,
                SubprocessStreamProtocolState {
                    limit: lim,
                    loop_val: loop_val.0,
                    stdin: None,
                    stdout: None,
                    stderr: None,
                    transport: None,
                    exited_fut,
                    pipe_closed_count: 0,
                    expected_pipes: 0,
                    process_exited: false,
                    connection_lost: false,
                },
            )?;
            Ok(Value::none())
        }

        fn connection_made(
            #[positional_only] self_val: PyValueRef<'_>,
            transport: PyValue,
        ) -> Result<Value> {
            let (limit, loop_val) =
                with_state(self_val.0, |st| Ok((st.limit, st.loop_val.clone())))?;

            let stdin_t =
                asyncio_rt::call_method(_interp, &transport.0, "get_pipe_transport", &[Value::int(0)])?;
            let stdout_t =
                asyncio_rt::call_method(_interp, &transport.0, "get_pipe_transport", &[Value::int(1)])?;
            let stderr_t =
                asyncio_rt::call_method(_interp, &transport.0, "get_pipe_transport", &[Value::int(2)])?;

            let stdin_writer = if !stdin_t.is_none() {
                Some(awaitable::create_stream_writer(
                    _interp,
                    stdin_t,
                    self_val.0.clone(),
                    None,
                    loop_val.clone(),
                )?)
            } else {
                None
            };
            let mut expected = 0usize;
            let stdout_reader = if !stdout_t.is_none() {
                expected += 1;
                Some(awaitable::create_stream_reader(
                    _interp,
                    limit,
                    Some(loop_val.clone()),
                )?)
            } else {
                None
            };
            let stderr_reader = if !stderr_t.is_none() {
                expected += 1;
                Some(awaitable::create_stream_reader(
                    _interp,
                    limit,
                    Some(loop_val),
                )?)
            } else {
                None
            };

            with_state_mut(self_val.0, |st| {
                st.transport = Some(transport.0);
                st.stdin = stdin_writer;
                st.stdout = stdout_reader;
                st.stderr = stderr_reader;
                st.expected_pipes = expected;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn pipe_data_received(
            #[positional_only] self_val: PyValueRef<'_>,
            fd: PyIndex,
            data: PyValue,
        ) -> Result<Value> {
            let fd_n = fd.expect_i64(FN_NAME, "fd")?;
            let reader_opt = with_state(self_val.0, |st| {
                Ok(match fd_n {
                    1 => st.stdout.clone(),
                    2 => st.stderr.clone(),
                    _ => None,
                })
            })?;
            if let Some(reader) = reader_opt {
                let _ = asyncio_rt::call_method(_interp, &reader, "feed_data", &[data.0])?;
            }
            Ok(Value::none())
        }

        fn pipe_connection_lost(
            #[positional_only] self_val: PyValueRef<'_>,
            fd: PyIndex,
            exc: PyValue,
        ) -> Result<Value> {
            let fd_n = fd.expect_i64(FN_NAME, "fd")?;
            let reader_opt = with_state_mut(self_val.0, |st| {
                let r = match fd_n {
                    1 => {
                        st.pipe_closed_count += 1;
                        st.stdout.clone()
                    }
                    2 => {
                        st.pipe_closed_count += 1;
                        st.stderr.clone()
                    }
                    _ => None,
                };
                Ok(r)
            })?;
            if let Some(reader) = reader_opt {
                if exc.0.is_none() {
                    let _ = asyncio_rt::call_method(_interp, &reader, "feed_eof", &[]);
                } else {
                    let _ = asyncio_rt::call_method(_interp, &reader, "set_exception", &[exc.0]);
                }
            }
            Ok(Value::none())
        }

        fn process_exited(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let fut = with_state_mut(self_val.0, |st| {
                st.process_exited = true;
                Ok(st.exited_fut.clone())
            })?;
            asyncio_rt::future_set_result_if_pending(_interp, &fut, Value::none())?;
            Ok(Value::none())
        }

        fn connection_lost(
            #[positional_only] self_val: PyValueRef<'_>,
            _exc: PyValue,
        ) -> Result<Value> {
            let (fut, out_r, err_r) = with_state_mut(self_val.0, |st| {
                st.connection_lost = true;
                Ok((st.exited_fut.clone(), st.stdout.clone(), st.stderr.clone()))
            })?;
            if let Some(r) = out_r {
                let _ = asyncio_rt::call_method(_interp, &r, "feed_eof", &[]);
            }
            if let Some(r) = err_r {
                let _ = asyncio_rt::call_method(_interp, &r, "feed_eof", &[]);
            }
            asyncio_rt::future_set_result_if_pending(_interp, &fut, Value::none())?;
            Ok(Value::none())
        }
    }

    #[native_state(ProcessState)]
    class Process {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            transport: PyValue,
            protocol: PyValue,
            loop_val: PyValue,
        ) -> Result<Value> {
            let pid_v = asyncio_rt::call_method(_interp, &transport.0, "get_pid", &[])?;
            let pid = pid_v.as_int().unwrap_or(0);
            let (stdin, stdout, stderr) =
                if has_native_state::<SubprocessStreamProtocolState>(&protocol.0) {
                    with_native_state::<SubprocessStreamProtocolState, _>(
                        &protocol.0,
                        "SubprocessStreamProtocol",
                        |st| Ok((st.stdin.clone(), st.stdout.clone(), st.stderr.clone())),
                    )?
                } else {
                    (None, None, None)
                };
            set_state(
                self_val.0,
                ProcessState {
                    transport: transport.0,
                    protocol: protocol.0,
                    loop_val: loop_val.0,
                    stdin,
                    stdout,
                    stderr,
                    pid,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn stdin(#[state] st: &ProcessState) -> Result<Value> {
            let _ = _interp;
            Ok(st.stdin.clone().unwrap_or_else(Value::none))
        }

        #[getter]
        fn stdout(#[state] st: &ProcessState) -> Result<Value> {
            let _ = _interp;
            Ok(st.stdout.clone().unwrap_or_else(Value::none))
        }

        #[getter]
        fn stderr(#[state] st: &ProcessState) -> Result<Value> {
            let _ = _interp;
            Ok(st.stderr.clone().unwrap_or_else(Value::none))
        }

        #[getter]
        fn pid(#[state] st: &ProcessState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(st.pid))
        }

        #[getter]
        fn returncode(#[state] st: &ProcessState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "get_returncode", &[])
        }

        fn _get_returncode(#[state] st: &ProcessState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "get_returncode", &[])
        }

        fn wait(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            awaitable::make_native_coro(
                _interp,
                CoroBody::ProcessWait {
                    proc: self_val.0.clone(),
                    waiter: None,
                },
            )
        }

        fn communicate(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] input: OptValueRef<'_>,
        ) -> Result<Value> {
            let input_bytes = match input.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => {
                    let bl = PyBytesLike::try_from_value_with_interp(_interp, v, FN_NAME, "input")?;
                    Some(bl.as_bytes().to_vec())
                }
            };
            awaitable::make_native_coro(
                _interp,
                CoroBody::ProcessCommunicate {
                    proc: self_val.0.clone(),
                    input: input_bytes,
                    phase: ProcCommPhase::Init,
                },
            )
        }

        fn send_signal(#[state] st: &ProcessState, signal: PyIndex) -> Result<Value> {
            let s = signal.expect_i64(FN_NAME, "signal")?;
            asyncio_rt::call_method(_interp, &st.transport, "send_signal", &[Value::int(s)])
        }

        fn terminate(#[state] st: &ProcessState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "terminate", &[])
        }

        fn kill(#[state] st: &ProcessState) -> Result<Value> {
            asyncio_rt::call_method(_interp, &st.transport, "kill", &[])
        }

        fn __repr__(#[state] st: &ProcessState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::string(format!("<Process {}>", st.pid)))
        }
    }

    // ── Module-Level Functions ───────────────────────────────────────────────

    fn run(
        main: PyValue,
        #[keyword_only] #[default(OptionalArg::Omitted)] debug: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] loop_factory: OptValueRef<'_>,
    ) -> Result<Value> {
        if asyncio_rt::get_running_loop_opt().is_some() {
            return Err(PyError::named(
                "RuntimeError",
                "asyncio.run() cannot be called from a running event loop",
            ));
        }
        if !asyncio_rt::is_coroutine_obj(&main.0) {
            let r = render_value_repr(_interp, &main.0)?;
            return Err(PyError::named(
                "ValueError",
                format!("a coroutine was expected, got {r}"),
            ));
        }
        if asyncio_rt::is_coro_done(&main.0) {
            return Err(PyError::named(
                "RuntimeError",
                "cannot reuse already awaited coroutine",
            ));
        }
        let runner_cls = asyncio_rt::asyncio_attr(_interp, "Runner")?;
        let mut kw: Vec<(&str, Value)> = Vec::new();
        if let Some(d) = opt_value_cloned(debug) {
            kw.push(("debug", d));
        }
        if let Some(lf) = opt_value_cloned(loop_factory) {
            kw.push(("loop_factory", lf));
        }
        let runner = asyncio_rt::call_kw(_interp, runner_cls, &[], &kw)?;
        let res = asyncio_rt::call_method(_interp, &runner, "run", &[main.0]);
        let _ = asyncio_rt::call_method(_interp, &runner, "close", &[]);
        res
    }

    fn sleep(
        delay: PyNumber,
        #[default(OptionalArg::Omitted)] result: OptValueRef<'_>,
    ) -> Result<Value> {
        let d = delay.as_f64();
        let res = opt_value_cloned(result).unwrap_or_else(Value::none);
        if d <= 0.0 {
            return awaitable::make_native_coro(
                _interp,
                CoroBody::YieldOnce {
                    yielded: false,
                    result: res,
                },
            );
        }
        awaitable::make_native_coro(
            _interp,
            CoroBody::Sleep {
                delay: d,
                result: res,
                waiter: None,
                timer: None,
            },
        )
    }

    fn gather(
        #[varargs] coros_or_futures: &[Value],
        #[default(false.into())] return_exceptions: PyBool,
    ) -> Result<Value> {
        if coros_or_futures.is_empty() {
            let loop_val = asyncio_rt::get_event_loop_compat(_interp)?;
            let outer = asyncio_rt::loop_create_future(_interp, &loop_val)?;
            asyncio_rt::future_set_result(_interp, &outer, Value::list(Vec::new()))?;
            return Ok(outer);
        }

        let mut unique_args: Vec<(usize, Value)> = Vec::new();
        let mut children: Vec<Value> = Vec::with_capacity(coros_or_futures.len());
        let mut loop_val: Option<Value> = None;
        let mut done_futs: Vec<Value> = Vec::new();
        let mut pending_futs: Vec<Value> = Vec::new();

        for arg in coros_or_futures {
            let aid = asyncio_rt::value_id(arg);
            if let Some((_, existing_fut)) = unique_args.iter().find(|(id, _)| *id == aid) {
                children.push(existing_fut.clone());
            } else {
                let fut = asyncio_rt::ensure_future_impl(_interp, arg.clone(), loop_val.clone())?;
                if loop_val.is_none() {
                    let fl = with_native_state::<FutureState, _>(&fut, "Future", |fs| {
                        Ok(fs.loop_val.clone())
                    })?;
                    loop_val = Some(fl);
                }
                let is_done = with_native_state::<FutureState, _>(&fut, "Future", |fs| {
                    Ok(fs.is_done())
                })?;
                if is_done {
                    done_futs.push(fut.clone());
                } else {
                    pending_futs.push(fut.clone());
                }
                unique_args.push((aid, fut.clone()));
                children.push(fut);
            }
        }

        let lv = loop_val.unwrap();
        let outer = asyncio_rt::create_future_on_loop(_interp, lv)?;
        with_native_state_mut::<FutureState, _>(&outer, "Future", |fs| {
            fs.gathering_children = Some(children.clone());
            Ok(())
        })?;

        let cb = asyncio_rt::alloc_asyncio_instance(_interp, "_GatherDoneCb")?;
        set_native_state(
            &cb,
            GatherDoneCbState {
                outer: outer.clone(),
                children,
                nfuts: unique_args.len(),
                nfinished: Rc::new(RefCell::new(0)),
                return_exceptions: return_exceptions.0,
            },
        )?;

        for fut in pending_futs {
            asyncio_rt::future_add_done_callback(_interp, &fut, cb.clone(), None)?;
        }
        for fut in done_futs {
            let _ = asyncio_rt::call_pos(_interp, cb.clone(), &[fut])?;
        }
        Ok(outer)
    }

    fn wait(
        fs: PyValue,
        #[keyword_only] #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        #[keyword_only] #[default(ALL_COMPLETED.into())] return_when: PyStr<'_>,
    ) -> Result<Value> {
        if asyncio_rt::is_future_obj(_interp, &fs.0) || asyncio_rt::is_coroutine_obj(&fs.0) {
            return Err(PyError::named(
                "TypeError",
                format!("expect a list of futures, not {}", value_type_name_str(&fs.0)),
            ));
        }
        if return_when.0 != FIRST_COMPLETED
            && return_when.0 != FIRST_EXCEPTION
            && return_when.0 != ALL_COMPLETED
        {
            return Err(PyError::named(
                "ValueError",
                format!("Invalid return_when value: {}", return_when.0),
            ));
        }
        let items = collect_iterable_values(_interp, &fs.0)?;
        if items.is_empty() {
            return Err(PyError::named("ValueError", "Set of Tasks/Futures is empty."));
        }
        let mut unique = Vec::new();
        for item in items {
            if asyncio_rt::is_coroutine_obj(&item) {
                return Err(PyError::named(
                    "TypeError",
                    "Passing coroutines is forbidden, use tasks explicitly.",
                ));
            }
            if !unique.iter().any(|u| asyncio_rt::same_object(u, &item)) {
                unique.push(item);
            }
        }
        let to = opt_none_or_f64(timeout, FN_NAME, "timeout")?;
        awaitable::make_native_coro(
            _interp,
            CoroBody::Wait {
                fs: unique,
                timeout: to,
                return_when: return_when.0.into_owned(),
                waiter: None,
                timer: None,
                cb: None,
            },
        )
    }

    fn wait_for(fut: PyValue, timeout: OptValueRef<'_>) -> Result<Value> {
        let to = opt_none_or_f64(timeout, FN_NAME, "timeout")?;
        awaitable::make_native_coro(
            _interp,
            CoroBody::WaitFor {
                fut_arg: fut.0,
                timeout: to,
                phase: WaitForPhase::Init,
            },
        )
    }

    fn as_completed(
        fs: PyValue,
        #[keyword_only] #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
    ) -> Result<Value> {
        if asyncio_rt::is_future_obj(_interp, &fs.0) || asyncio_rt::is_coroutine_obj(&fs.0) {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "expect an iterable of futures, not {}",
                    value_type_name_str(&fs.0)
                ),
            ));
        }
        let items = collect_iterable_values(_interp, &fs.0)?;
        let loop_val = asyncio_rt::get_event_loop_compat(_interp)?;
        let queue_cls = asyncio_rt::asyncio_attr(_interp, "Queue")?;
        let done_queue = asyncio_rt::call_pos(_interp, queue_cls, &[])?;

        let mut todo_vec = Vec::new();
        for item in items {
            let fut = asyncio_rt::ensure_future_impl(_interp, item, Some(loop_val.clone()))?;
            if !todo_vec.iter().any(|x| asyncio_rt::same_object(x, &fut)) {
                todo_vec.push(fut);
            }
        }
        let count = todo_vec.len();
        let todo_rc = Rc::new(RefCell::new(todo_vec.clone()));
        let th_rc = Rc::new(RefCell::new(None));

        let done_cb = asyncio_rt::alloc_asyncio_instance(_interp, "_AsCompletedDoneCb")?;
        set_native_state(
            &done_cb,
            AsCompletedState {
                todo: Rc::clone(&todo_rc),
                done_queue: done_queue.clone(),
                timeout_handle: Rc::clone(&th_rc),
            },
        )?;
        for f in &todo_vec {
            asyncio_rt::future_add_done_callback(_interp, f, done_cb.clone(), None)?;
        }
        if count > 0
            && let Some(t) = opt_none_or_f64(timeout, FN_NAME, "timeout")?
        {
            let to_cb = asyncio_rt::alloc_asyncio_instance(_interp, "_AsCompletedTimeoutCb")?;
            set_native_state(
                &to_cb,
                AsCompletedState {
                    todo: Rc::clone(&todo_rc),
                    done_queue: done_queue.clone(),
                    timeout_handle: Rc::clone(&th_rc),
                },
            )?;
            let h = asyncio_rt::loop_call_later(_interp, &loop_val, t, to_cb, Vec::new(), None)?;
            *th_rc.borrow_mut() = Some(h);
        }

        let iter_obj = asyncio_rt::alloc_asyncio_instance(_interp, "_AsCompletedIter")?;
        set_native_state(
            &iter_obj,
            AsCompletedIterState {
                queue: done_queue,
                remaining: count,
            },
        )?;
        Ok(iter_obj)
    }

    fn shield(arg: PyValue) -> Result<Value> {
        let inner = asyncio_rt::ensure_future_impl(_interp, arg.0, None)?;
        let done = with_native_state::<FutureState, _>(&inner, "Future", |fs| Ok(fs.is_done()))?;
        if done {
            return Ok(inner);
        }
        let loop_val = with_native_state::<FutureState, _>(&inner, "Future", |fs| {
            Ok(fs.loop_val.clone())
        })?;
        let outer = asyncio_rt::loop_create_future(_interp, &loop_val)?;

        let inner_cb = asyncio_rt::alloc_asyncio_instance(_interp, "_ShieldInnerCb")?;
        set_native_state(
            &inner_cb,
            ShieldInnerCbState {
                outer: outer.clone(),
            },
        )?;
        let outer_cb = asyncio_rt::alloc_asyncio_instance(_interp, "_ShieldOuterCb")?;
        set_native_state(
            &outer_cb,
            ShieldOuterCbState {
                inner: inner.clone(),
                inner_cb: inner_cb.clone(),
            },
        )?;
        asyncio_rt::future_add_done_callback(_interp, &inner, inner_cb, None)?;
        asyncio_rt::future_add_done_callback(_interp, &outer, outer_cb, None)?;
        Ok(outer)
    }

    fn create_task(
        coro: PyValue,
        #[keyword_only] #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
    ) -> Result<Value> {
        let loop_val = asyncio_rt::get_running_loop_or_err()?;
        let task_name = match opt_value_cloned(name) {
            Some(v) => {
                let s = asyncio_rt::call_pos(_interp, Value::builtin_function("str"), &[v])?;
                Some(s.as_str().unwrap_or("").to_owned())
            }
            None => None,
        };
        asyncio_rt::loop_create_task(_interp, &loop_val, coro.0, task_name, opt_value_cloned(context))
    }

    fn ensure_future(
        coro_or_future: PyValue,
        #[keyword_only] #[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>,
    ) -> Result<Value> {
        asyncio_rt::ensure_future_impl(_interp, coro_or_future.0, opt_value_cloned(r#loop))
    }

    fn run_coroutine_threadsafe(coro: PyValue, r#loop: PyValue) -> Result<Value> {
        if !asyncio_rt::is_coroutine_obj(&coro.0) {
            return Err(PyError::named("TypeError", "A coroutine object is required"));
        }
        let fut = asyncio_rt::loop_create_future(_interp, &r#loop.0)?;
        let cb = asyncio_rt::asyncio_attr(_interp, "_schedule_coro_into_future")?;
        asyncio_rt::loop_call_soon(
            _interp,
            &r#loop.0,
            cb,
            vec![r#loop.0.clone(), coro.0, fut.clone()],
            None,
        )?;
        Ok(fut)
    }

    fn _schedule_coro_into_future(
        loop_val: PyValue,
        coro: PyValue,
        target: PyValue,
    ) -> Result<Value> {
        let task = asyncio_rt::ensure_future_impl(_interp, coro.0, Some(loop_val.0))?;
        let cb = asyncio_rt::alloc_asyncio_instance(_interp, "_WrapFutureCb")?;
        set_native_state(
            &cb,
            WrapFutureCbState {
                src: task.clone(),
                dst: target.0,
            },
        )?;
        asyncio_rt::future_add_done_callback(_interp, &task, cb, None)?;
        Ok(Value::none())
    }

    fn current_task(#[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let lv = match opt_value_cloned(r#loop) {
            Some(l) => l,
            None => asyncio_rt::get_running_loop_or_err()?,
        };
        Ok(asyncio_rt::current_task_for_loop(&lv).unwrap_or_else(Value::none))
    }

    fn all_tasks(#[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let lv = match opt_value_cloned(r#loop) {
            Some(l) => l,
            None => asyncio_rt::get_running_loop_or_err()?,
        };
        let tasks = asyncio_rt::all_tasks_for_loop(&lv);
        asyncio_rt::make_py_set(_interp, tasks)
    }

    fn isfuture(obj: PyValue) -> Result<Value> {
        Ok(Value::bool_(asyncio_rt::is_future_obj(_interp, &obj.0)))
    }

    fn iscoroutine(obj: PyValue) -> Result<Value> {
        let _ = _interp;
        Ok(Value::bool_(asyncio_rt::is_coroutine_obj(&obj.0)))
    }

    fn iscoroutinefunction(func: PyValue) -> Result<Value> {
        Ok(Value::bool_(asyncio_rt::is_coroutine_function_obj(
            _interp, &func.0,
        )))
    }

    fn wrap_future(
        future: PyValue,
        #[keyword_only] #[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>,
    ) -> Result<Value> {
        if asyncio_rt::is_future_obj(_interp, &future.0) {
            return Ok(future.0);
        }
        // Duck-type check for concurrent.futures.Future-like object.
        if _interp.get_attr(&future.0, "add_done_callback").is_err()
            || _interp.get_attr(&future.0, "result").is_err()
        {
            return Err(PyError::named(
                "TypeError",
                "A Future or concurrent.futures.Future is required",
            ));
        }
        let lv = match opt_value_cloned(r#loop) {
            Some(l) => l,
            None => asyncio_rt::get_event_loop_compat(_interp)?,
        };
        let new_fut = asyncio_rt::loop_create_future(_interp, &lv)?;
        let cb = asyncio_rt::alloc_asyncio_instance(_interp, "_WrapFutureCb")?;
        set_native_state(
            &cb,
            WrapFutureCbState {
                src: future.0.clone(),
                dst: new_fut.clone(),
            },
        )?;
        let _ = asyncio_rt::call_method(_interp, &future.0, "add_done_callback", &[cb])?;
        Ok(new_fut)
    }

    fn to_thread(
        func: PyValue,
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: IndexMap<String, Value>,
    ) -> Result<Value> {
        let kw: Vec<(String, Value)> = kwargs.into_iter().collect();
        awaitable::make_native_coro(
            _interp,
            CoroBody::ToThread {
                func: func.0,
                args: args.to_vec(),
                kwargs: kw,
                waiter: None,
            },
        )
    }

    fn eager_task_factory(
        loop_val: PyValue,
        coro: PyValue,
        #[keyword_only] #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
    ) -> Result<Value> {
        let task_cls = asyncio_rt::asyncio_attr(_interp, "Task")?;
        let mut kw: Vec<(&str, Value)> = vec![
            ("loop", loop_val.0),
            ("eager_start", Value::bool_(true)),
        ];
        if let Some(n) = opt_value_cloned(name) {
            kw.push(("name", n));
        }
        if let Some(c) = opt_value_cloned(context) {
            kw.push(("context", c));
        }
        asyncio_rt::call_kw(_interp, task_cls, &[coro.0], &kw)
    }

    fn create_eager_task_factory(custom_task_constructor: PyValue) -> Result<Value> {
        let inst = asyncio_rt::alloc_asyncio_instance(_interp, "_EagerTaskFactory")?;
        set_native_state(
            &inst,
            EagerTaskFactoryState {
                custom_constructor: custom_task_constructor.0,
            },
        )?;
        Ok(inst)
    }

    fn timeout(delay: OptValueRef<'_>) -> Result<Value> {
        let loop_val = asyncio_rt::get_running_loop_or_err()?;
        let when = match opt_none_or_f64(delay, FN_NAME, "delay")? {
            Some(d) => Value::float(asyncio_rt::loop_time(_interp, &loop_val)? + d),
            None => Value::none(),
        };
        let cls = asyncio_rt::asyncio_attr(_interp, "Timeout")?;
        asyncio_rt::call_pos(_interp, cls, &[when])
    }

    fn timeout_at(when: OptValueRef<'_>) -> Result<Value> {
        let cls = asyncio_rt::asyncio_attr(_interp, "Timeout")?;
        let w = opt_value_cloned(when).unwrap_or_else(Value::none);
        asyncio_rt::call_pos(_interp, cls, &[w])
    }

    fn open_connection(
        #[default(OptionalArg::Omitted)] host: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] port: OptValueRef<'_>,
        #[keyword_only] #[default((DEFAULT_STREAM_LIMIT as i64).into())] limit: PyIndex,
        #[keyword_only] #[default(OptionalArg::Omitted)] ssl: OptValueRef<'_>,
        #[keyword_only] #[default(0_i64.into())] family: PyIndex,
        #[keyword_only] #[default(0_i64.into())] _proto: PyIndex,
        #[keyword_only] #[default(0_i64.into())] _flags: PyIndex,
        #[keyword_only] #[default(OptionalArg::Omitted)] sock: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] _local_addr: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] server_hostname: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_handshake_timeout: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_shutdown_timeout: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] _happy_eyeballs_delay: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] _interleave: OptValueRef<'_>,
    ) -> Result<Value> {
        let lim = limit.expect_i64(FN_NAME, "limit")?;
        if lim <= 0 {
            return Err(PyError::named("ValueError", "Limit cannot be <= 0"));
        }
        let host_opt = opt_none_or_str(host)?;
        let port_opt = opt_none_or_i64(_interp, port, FN_NAME, "port")?;
        let fam = family.expect_i64(FN_NAME, "family")?;
        let sh = opt_none_or_str(server_hostname)?;
        awaitable::make_native_coro(
            _interp,
            CoroBody::OpenConnection {
                host: host_opt,
                port: port_opt,
                limit: lim as usize,
                ssl_arg: opt_value_cloned(ssl),
                family: fam,
                sock: opt_value_cloned(sock),
                server_hostname: sh,
                phase: OpenConnPhase::Init,
            },
        )
    }

    fn start_server(
        client_connected_cb: PyValue,
        #[default(OptionalArg::Omitted)] host: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] port: OptValueRef<'_>,
        #[keyword_only] #[default((DEFAULT_STREAM_LIMIT as i64).into())] limit: PyIndex,
        #[keyword_only] #[default(0_i64.into())] family: PyIndex,
        #[keyword_only] #[default(OptionalArg::Omitted)] _flags: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] sock: OptValueRef<'_>,
        #[keyword_only] #[default(100_i64.into())] backlog: PyIndex,
        #[keyword_only] #[default(OptionalArg::Omitted)] ssl: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] reuse_address: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] _reuse_port: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_handshake_timeout: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] _ssl_shutdown_timeout: OptValueRef<'_>,
        #[keyword_only] #[default(true.into())] start_serving: PyBool,
    ) -> Result<Value> {
        let lim = limit.expect_i64(FN_NAME, "limit")?;
        if lim <= 0 {
            return Err(PyError::named("ValueError", "Limit cannot be <= 0"));
        }
        let port_opt = opt_none_or_i64(_interp, port, FN_NAME, "port")?;
        let fam = family.expect_i64(FN_NAME, "family")?;
        let bl = backlog.expect_i64(FN_NAME, "backlog")?;
        let ra = opt_none_or_bool(_interp, reuse_address)?;
        awaitable::make_native_coro(
            _interp,
            CoroBody::StartServer {
                client_connected_cb: client_connected_cb.0,
                host: opt_value_cloned(host),
                port: port_opt,
                limit: lim as usize,
                family: fam,
                backlog: bl,
                ssl_arg: opt_value_cloned(ssl),
                reuse_address: ra,
                sock: opt_value_cloned(sock),
                start_serving: start_serving.0,
                inner_coro: None,
            },
        )
    }

    fn create_subprocess_exec(
        program: PyValue,
        #[varargs] args: &[Value],
        #[default(OptionalArg::Omitted)] stdin: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] stdout: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] stderr: OptValueRef<'_>,
        #[default((DEFAULT_STREAM_LIMIT as i64).into())] limit: PyIndex,
        #[varkwargs] kwargs: IndexMap<String, Value>,
    ) -> Result<Value> {
        let lim = limit.expect_i64(FN_NAME, "limit")?.max(1) as usize;
        let prog_str = value_to_str_arg(_interp, &program.0)?;
        let mut str_args = Vec::with_capacity(args.len());
        for a in args {
            str_args.push(value_to_str_arg(_interp, a)?);
        }
        let cwd = match kwargs.get("cwd") {
            Some(v) if !v.is_none() => Some(value_to_str_arg(_interp, v)?),
            _ => None,
        };
        let env = match kwargs.get("env") {
            Some(v) if !v.is_none() => Some(io_engine::extract_env_pairs(v)?),
            _ => None,
        };
        awaitable::make_native_coro(
            _interp,
            CoroBody::CreateSubprocess {
                program: prog_str,
                args: str_args,
                shell: false,
                stdin: opt_value_cloned(stdin),
                stdout: opt_value_cloned(stdout),
                stderr: opt_value_cloned(stderr),
                limit: lim,
                cwd,
                env,
            },
        )
    }

    fn create_subprocess_shell(
        cmd: PyValue,
        #[default(OptionalArg::Omitted)] stdin: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] stdout: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] stderr: OptValueRef<'_>,
        #[default((DEFAULT_STREAM_LIMIT as i64).into())] limit: PyIndex,
        #[varkwargs] kwargs: IndexMap<String, Value>,
    ) -> Result<Value> {
        let lim = limit.expect_i64(FN_NAME, "limit")?.max(1) as usize;
        let cmd_str = value_to_str_arg(_interp, &cmd.0)?;
        let cwd = match kwargs.get("cwd") {
            Some(v) if !v.is_none() => Some(value_to_str_arg(_interp, v)?),
            _ => None,
        };
        let env = match kwargs.get("env") {
            Some(v) if !v.is_none() => Some(io_engine::extract_env_pairs(v)?),
            _ => None,
        };
        awaitable::make_native_coro(
            _interp,
            CoroBody::CreateSubprocess {
                program: cmd_str,
                args: Vec::new(),
                shell: true,
                stdin: opt_value_cloned(stdin),
                stdout: opt_value_cloned(stdout),
                stderr: opt_value_cloned(stderr),
                limit: lim,
                cwd,
                env,
            },
        )
    }

    // ── Event Loop Policy & Global Accessors ─────────────────────────────────

    fn get_event_loop_policy() -> Result<Value> {
        asyncio_rt::get_event_loop_policy(_interp)
    }

    fn set_event_loop_policy(policy: PyValue) -> Result<Value> {
        let _ = _interp;
        if policy.0.is_none() {
            asyncio_rt::set_event_loop_policy(None);
        } else {
            asyncio_rt::set_event_loop_policy(Some(policy.0));
        }
        Ok(Value::none())
    }

    fn get_running_loop() -> Result<Value> {
        let _ = _interp;
        asyncio_rt::get_running_loop_or_err()
    }

    fn _get_running_loop() -> Result<Value> {
        let _ = _interp;
        Ok(asyncio_rt::get_running_loop_opt().unwrap_or_else(Value::none))
    }

    fn _set_running_loop(loop_val: PyValue) -> Result<Value> {
        let _ = _interp;
        if loop_val.0.is_none() {
            asyncio_rt::set_running_loop_opt(None);
        } else {
            asyncio_rt::set_running_loop_opt(Some(loop_val.0));
        }
        Ok(Value::none())
    }

    fn get_event_loop() -> Result<Value> {
        asyncio_rt::get_event_loop_compat(_interp)
    }

    fn set_event_loop(loop_val: PyValue) -> Result<Value> {
        let policy = asyncio_rt::get_event_loop_policy(_interp)?;
        asyncio_rt::call_method(_interp, &policy, "set_event_loop", &[loop_val.0])
    }

    fn new_event_loop() -> Result<Value> {
        let policy = asyncio_rt::get_event_loop_policy(_interp)?;
        asyncio_rt::call_method(_interp, &policy, "new_event_loop", &[])
    }

    fn get_child_watcher() -> Result<Value> {
        let _ = _interp;
        Ok(Value::none())
    }

    fn set_child_watcher(_watcher: PyValue) -> Result<Value> {
        let _ = _interp;
        Ok(Value::none())
    }

    // ── Internal Task / Timer / Executor Helpers ─────────────────────────────

    fn _register_task(task: PyValue) -> Result<Value> {
        let _ = _interp;
        asyncio_rt::register_task(task.0);
        Ok(Value::none())
    }

    fn _unregister_task(task: PyValue) -> Result<Value> {
        let _ = _interp;
        asyncio_rt::unregister_task(&task.0);
        Ok(Value::none())
    }

    fn _enter_task(loop_val: PyValue, task: PyValue) -> Result<Value> {
        asyncio_rt::enter_task(_interp, &loop_val.0, &task.0)?;
        Ok(Value::none())
    }

    fn _leave_task(loop_val: PyValue, task: PyValue) -> Result<Value> {
        asyncio_rt::leave_task(_interp, &loop_val.0, &task.0)?;
        Ok(Value::none())
    }

    fn _set_result_unless_done(fut: PyValue, value: PyValue) -> Result<Value> {
        asyncio_rt::future_set_result_if_pending(_interp, &fut.0, value.0)?;
        Ok(Value::none())
    }

    fn _release_waiter(waiter: PyValue, #[varargs] _args: &[Value]) -> Result<Value> {
        asyncio_rt::future_set_result_if_pending(_interp, &waiter.0, Value::none())?;
        Ok(Value::none())
    }

    fn _run_executor_cb(
        fut: PyValue,
        func: PyValue,
        #[varargs] args: &[Value],
    ) -> Result<Value> {
        let done = with_native_state::<FutureState, _>(&fut.0, "Future", |fs| Ok(fs.is_done()))
            .unwrap_or(false);
        if done {
            return Ok(Value::none());
        }
        match asyncio_rt::call_pos(_interp, func.0, args) {
            Ok(res) => asyncio_rt::future_set_result_if_pending(_interp, &fut.0, res)?,
            Err(err) => {
                let ev = asyncio_rt::pyerror_to_exc_value(_interp, err)?;
                asyncio_rt::future_set_exception_if_pending(_interp, &fut.0, ev)?;
            }
        }
        Ok(Value::none())
    }

    fn _staggered_race(
        coro_fns: PyValue,
        delay: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] r#loop: OptValueRef<'_>,
    ) -> Result<Value> {
        let fns = collect_iterable_values(_interp, &coro_fns.0)?;
        let d = opt_none_or_f64(delay, "staggered_race", "delay")?;
        awaitable::make_native_coro(
            _interp,
            CoroBody::StaggeredRace {
                coro_fns: fns,
                delay: d,
                loop_opt: opt_value_cloned(r#loop),
                phase: StaggeredPhase::Init,
            },
        )
    }
}
