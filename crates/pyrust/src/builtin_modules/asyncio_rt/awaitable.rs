//! Native-Rust coroutine / awaitable state machine (`_NativeCoro`) driving
//! every async operation in `asyncio` without any Python bytecode glue
//! (issue #3254).

use std::cell::RefCell;
use std::rc::Rc;

use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state, with_native_state_mut,
};
use crate::error::{PyError, Result};
use crate::interpreter::{
    CoroStep, Interpreter, instantiate_exception, lookup_exc_class, render_value_repr,
};
use crate::value::{Value, ValueKind};

use super::io_engine::{self, ServerState, SubprocessTransportState, TlsLayer};
use super::{
    BarrierState, BarrierStatus, ConditionState, EventState, FutureState, LockState, ProcessState,
    QueueKind, QueueState, SemaphoreState, StreamReaderProtocolState, StreamReaderState,
    StreamWriterState, SubprocessStreamProtocolState, TaskGroupState, TimeoutState, TimeoutStatus,
    alloc_asyncio_instance, asyncio_attr, bind_or_check_loop, call_method, call_pos, copy_context,
    current_task_for_loop, ensure_future_impl, extract_fd, future_add_done_callback, future_cancel,
    future_exception, future_remove_done_callback, future_result, future_set_result_if_pending,
    get_running_loop_or_err, is_base_fatal_error_val, is_cancelled_error_val, is_coroutine_obj,
    loop_call_later, loop_create_future, loop_create_task, make_asyncio_exc, make_py_set,
    make_stop_async_iteration, make_stop_iteration_with_value, pyerror_to_exc_value,
    raise_asyncio_exc, same_object, set_exc_cause, step_coro_once,
};

pub(crate) struct NativeCoroState {
    pub(crate) body: Option<CoroBody>,
    pub(crate) done: bool,
}

/// Coroutine body supplied by another native module (for example `contextlib`).
///
/// `step` is resumed exactly like the built-in bodies: with the value sent into
/// the coroutine, or with an exception to throw into it.
pub(crate) trait ExternalCoroBody {
    fn step(
        &mut self,
        interp: &mut Interpreter,
        sent_value: Value,
        inject_exc: Option<PyError>,
    ) -> Result<CoroStep>;
}

/// All native async operations in `asyncio`.
pub(crate) enum CoroBody {
    /// A body implemented by another native module.
    External(Box<dyn ExternalCoroBody>),
    /// `Future.__await__()`
    AwaitFuture {
        fut: Value,
        yielded: bool,
    },
    /// Single-turn fairness yield (`yield None; return result`)
    YieldOnce {
        yielded: bool,
        result: Value,
    },
    /// Immediate return without yielding
    ReadyValue {
        result: Value,
    },
    /// Wrap a custom `__await__` object for `ensure_future`
    WrapAwaitable {
        awaitable: Value,
        iter: Option<Value>,
    },
    /// `asyncio.sleep(delay, result=None)`
    Sleep {
        delay: f64,
        result: Value,
        waiter: Option<Value>,
        timer: Option<Value>,
    },
    /// `asyncio.wait(fs, *, timeout=None, return_when=ALL_COMPLETED)`
    Wait {
        fs: Vec<Value>,
        timeout: Option<f64>,
        return_when: String,
        waiter: Option<Value>,
        timer: Option<Value>,
        cb: Option<Value>,
    },
    /// `asyncio.wait_for(fut, timeout)`
    WaitFor {
        fut_arg: Value,
        timeout: Option<f64>,
        phase: WaitForPhase,
    },
    /// Single coroutine item produced by `asyncio.as_completed(...)`
    AsCompletedOne {
        queue: Value,
        get_coro: Option<Value>,
    },
    /// `Timeout.__aenter__` / `Timeout.__aexit__`
    TimeoutAenter {
        timeout_obj: Value,
    },
    TimeoutAexit {
        timeout_obj: Value,
        exc_type: Value,
        exc_val: Value,
    },
    /// `TaskGroup.__aenter__` / `TaskGroup.__aexit__`
    TaskGroupAenter {
        tg_obj: Value,
    },
    TaskGroupAexit {
        tg_obj: Value,
        et: Value,
        exc: Value,
        started: bool,
        propagate_cancel: Option<Value>,
        waiting_fut: Option<Value>,
    },
    /// `Lock.acquire` / `Lock.__aenter__` / `Lock.__aexit__`
    LockAcquire {
        lock_obj: Value,
        waiter: Option<Value>,
    },
    ContextManagerAenter {
        obj: Value,
        inner: Option<Value>,
    },
    ContextManagerAexit {
        obj: Value,
    },
    /// `Event.wait`
    EventWait {
        event_obj: Value,
        waiter: Option<Value>,
    },
    /// `Condition.wait` / `Condition.wait_for`
    ConditionWait {
        cond_obj: Value,
        phase: CondWaitPhase,
    },
    ConditionWaitFor {
        cond_obj: Value,
        predicate: Value,
        wait_coro: Option<Value>,
    },
    /// `Semaphore.acquire`
    SemaphoreAcquire {
        sem_obj: Value,
        waiter: Option<Value>,
    },
    /// `Barrier.wait` / `reset` / `abort` / `__aenter__` / `__aexit__`
    BarrierWait {
        barrier_obj: Value,
        phase: BarrierWaitPhase,
    },
    BarrierReset {
        barrier_obj: Value,
        acquire_coro: Option<Value>,
    },
    BarrierAbort {
        barrier_obj: Value,
        acquire_coro: Option<Value>,
    },
    /// `Queue.put` / `Queue.get` / `Queue.join`
    QueuePut {
        queue_obj: Value,
        item: Value,
        waiter: Option<Value>,
    },
    QueueGet {
        queue_obj: Value,
        waiter: Option<Value>,
    },
    QueueJoin {
        queue_obj: Value,
        wait_coro: Option<Value>,
    },
    /// `StreamReader` async methods
    StreamRead {
        reader: Value,
        n: i64,
        waiter: Option<Value>,
    },
    StreamReadline {
        reader: Value,
        waiter: Option<Value>,
    },
    StreamReadexactly {
        reader: Value,
        n: usize,
        waiter: Option<Value>,
    },
    StreamReaduntil {
        reader: Value,
        sep: Vec<u8>,
        waiter: Option<Value>,
    },
    StreamAnext {
        reader: Value,
        readline_coro: Option<Value>,
    },
    /// `StreamWriter` async methods
    StreamDrain {
        writer: Value,
        phase: u8,
        waiter: Option<Value>,
    },
    StreamWaitClosed {
        writer: Value,
        waiter: Option<Value>,
    },
    StreamStartTls {
        writer: Value,
        sslcontext: Value,
        server_hostname: Option<String>,
        waiter: Option<Value>,
    },
    /// `Server` async methods
    ServerWaitClosed {
        server: Value,
        waiter: Option<Value>,
    },
    ServerServeForever {
        server: Value,
        started: bool,
        waiter: Option<Value>,
    },
    ServerAenter {
        server: Value,
    },
    ServerAexit {
        server: Value,
        wait_coro: Option<Value>,
    },
    /// `SelectorEventLoop` async socket & connection methods
    LoopSockRecv {
        loop_val: Value,
        sock: Value,
        nbytes: i64,
        fd: i64,
        waiter: Option<Value>,
    },
    LoopSockRecvInto {
        loop_val: Value,
        sock: Value,
        buf: Value,
        fd: i64,
        waiter: Option<Value>,
    },
    LoopSockSendall {
        loop_val: Value,
        sock: Value,
        data: Vec<u8>,
        offset: usize,
        fd: i64,
        waiter: Option<Value>,
    },
    LoopSockConnect {
        loop_val: Value,
        sock: Value,
        address: Value,
        fd: i64,
        started: bool,
        waiter: Option<Value>,
    },
    LoopSockAccept {
        loop_val: Value,
        sock: Value,
        fd: i64,
        waiter: Option<Value>,
    },
    LoopCreateConnection {
        loop_val: Value,
        protocol_factory: Value,
        host: Option<String>,
        port: Option<i64>,
        ssl_arg: Option<Value>,
        family: i64,
        sock: Option<Value>,
        server_hostname: Option<String>,
        phase: CreateConnPhase,
    },
    LoopCreateServer {
        loop_val: Value,
        protocol_factory: Value,
        host: Option<Value>,
        port: Option<i64>,
        family: i64,
        backlog: i64,
        ssl_arg: Option<Value>,
        reuse_address: Option<bool>,
        sock: Option<Value>,
        start_serving: bool,
    },
    LoopCreateDatagramEndpoint {
        loop_val: Value,
        protocol_factory: Value,
        local_addr: Option<Value>,
        remote_addr: Option<Value>,
        family: i64,
        sock: Option<Value>,
    },
    LoopStartTls {
        loop_val: Value,
        transport: Value,
        protocol: Value,
        sslcontext: Value,
        server_side: bool,
        server_hostname: Option<String>,
        waiter: Option<Value>,
    },
    OpenConnection {
        host: Option<String>,
        port: Option<i64>,
        limit: usize,
        ssl_arg: Option<Value>,
        family: i64,
        sock: Option<Value>,
        server_hostname: Option<String>,
        phase: OpenConnPhase,
    },
    StartServer {
        client_connected_cb: Value,
        host: Option<Value>,
        port: Option<i64>,
        limit: usize,
        family: i64,
        backlog: i64,
        ssl_arg: Option<Value>,
        reuse_address: Option<bool>,
        sock: Option<Value>,
        start_serving: bool,
        inner_coro: Option<Value>,
    },
    /// Subprocesses
    LoopSubprocess {
        loop_val: Value,
        protocol_factory: Value,
        program: String,
        args: Vec<String>,
        shell: bool,
        stdin: Option<Value>,
        stdout: Option<Value>,
        stderr: Option<Value>,
        cwd: Option<String>,
        env: Option<Vec<(String, String)>>,
    },
    CreateSubprocess {
        program: String,
        args: Vec<String>,
        shell: bool,
        stdin: Option<Value>,
        stdout: Option<Value>,
        stderr: Option<Value>,
        limit: usize,
        cwd: Option<String>,
        env: Option<Vec<(String, String)>>,
    },
    ProcessWait {
        proc: Value,
        waiter: Option<Value>,
    },
    ProcessCommunicate {
        proc: Value,
        input: Option<Vec<u8>>,
        phase: ProcCommPhase,
    },
    /// `asyncio.to_thread(func, *args, **kwargs)`
    ToThread {
        func: Value,
        args: Vec<Value>,
        kwargs: Vec<(String, Value)>,
        waiter: Option<Value>,
    },
    /// `asyncio.staggered.staggered_race`
    StaggeredRace {
        coro_fns: Vec<Value>,
        delay: Option<f64>,
        loop_opt: Option<Value>,
        phase: StaggeredPhase,
    },
}

pub(crate) enum WaitForPhase {
    Init,
    ZeroCancelWait {
        fut: Value,
        waiter: Value,
        cb: Value,
    },
    EnteringTimeout {
        timeout_cm: Value,
        enter_coro: Value,
        fut: Value,
    },
    AwaitingInner {
        timeout_cm: Value,
        fut_iter: Value,
    },
    ExitingTimeout {
        exit_coro: Value,
        saved_result: Option<Value>,
        saved_exc: Option<Value>,
    },
}

pub(crate) enum CondWaitPhase {
    Init,
    WaitingFuture {
        waiter: Value,
    },
    Reacquiring {
        acquire_coro: Value,
        cancelled: bool,
        saved_exc: Option<Value>,
    },
}

pub(crate) enum BarrierWaitPhase {
    Init,
    AcquiringCond { acquire_coro: Value },
    WaitingUnblock { wait_coro: Value },
    WaitingFill { wait_coro: Value, index: i64 },
}

pub(crate) enum CreateConnPhase {
    Init,
    Connecting {
        sock: Value,
        connect_coro: Value,
        ssl_ctx: Option<Value>,
        server_hostname: Option<String>,
    },
    HandshakeWait {
        transport: Value,
        protocol: Value,
        waiter: Value,
    },
}

pub(crate) enum OpenConnPhase {
    Init,
    Connecting {
        reader: Value,
        protocol: Value,
        conn_coro: Value,
    },
}

pub(crate) enum ProcCommPhase {
    Init,
    Gathering { gather_fut: Value },
}

pub(crate) enum StaggeredPhase {
    Init,
    Running {
        tasks: Vec<Value>,
        exceptions: Vec<Value>,
        winner_idx: Option<usize>,
        winner_val: Option<Value>,
        next_idx: usize,
        waiter: Value,
        delay_timer: Option<Value>,
    },
}

pub(crate) fn make_native_coro(interp: &mut Interpreter, body: CoroBody) -> Result<Value> {
    let inst = alloc_asyncio_instance(interp, "_NativeCoro")?;
    set_native_state(
        &inst,
        NativeCoroState {
            body: Some(body),
            done: false,
        },
    )?;
    Ok(inst)
}

/// Yield a pending `Future` to the enclosing `Task` driver.
fn yield_future(fut: &Value) -> Result<CoroStep> {
    if has_native_state::<FutureState>(fut) {
        with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
            st.future_blocking = true;
            Ok(())
        })?;
    }
    Ok(CoroStep::Yielded(fut.clone()))
}

fn is_fut_done(interp: &mut Interpreter, fut: &Value) -> Result<bool> {
    if has_native_state::<FutureState>(fut) {
        return with_native_state::<FutureState, _>(fut, "Future", |st| Ok(st.is_done()));
    }
    let d = call_method(interp, fut, "done", &[])?;
    interp.truthy_value(&d)
}

fn get_fut_result(interp: &mut Interpreter, fut: &Value) -> Result<Value> {
    if has_native_state::<FutureState>(fut) {
        return future_result(interp, fut);
    }
    call_method(interp, fut, "result", &[])
}

/// Drive a `_NativeCoro` one step (used by BOTH `Task._step_run` and
/// `_NativeCoro.__next__`/`send`/`throw`).
pub(crate) fn step_native_coro(
    interp: &mut Interpreter,
    coro: &Value,
    sent_value: Value,
    inject_exc: Option<PyError>,
) -> Result<CoroStep> {
    // Take body out of `NativeCoroState` during execution to avoid RefCell borrow conflicts!
    let body_opt = with_native_state_mut::<NativeCoroState, _>(coro, "coroutine", |st| {
        if st.done {
            return Err(PyError::named(
                "RuntimeError",
                "cannot reuse already awaited coroutine",
            ));
        }
        Ok(st.body.take())
    })?;

    let Some(mut body) = body_opt else {
        return Err(PyError::named("ValueError", "coroutine already executing"));
    };

    let outcome = step_body(interp, &mut body, sent_value, inject_exc);

    match outcome {
        Ok(CoroStep::Yielded(v)) => {
            with_native_state_mut::<NativeCoroState, _>(coro, "coroutine", |st| {
                st.body = Some(body);
                Ok(())
            })?;
            Ok(CoroStep::Yielded(v))
        }
        Ok(CoroStep::Returned(v)) => {
            with_native_state_mut::<NativeCoroState, _>(coro, "coroutine", |st| {
                st.done = true;
                st.body = None;
                Ok(())
            })?;
            Ok(CoroStep::Returned(v))
        }
        Err(e) => {
            with_native_state_mut::<NativeCoroState, _>(coro, "coroutine", |st| {
                st.done = true;
                st.body = None;
                Ok(())
            })?;
            Err(e)
        }
    }
}

/// Convert `CoroStep` into the Python iterator protocol (`__next__` / `send` /
/// `throw`) expected by `YieldFrom`:
/// - `Yielded(v)` -> `Ok(v)`
/// - `Returned(v)` -> `Err(StopIteration(v))`
pub(crate) fn coro_step_to_iter_result(step: Result<CoroStep>) -> Result<Value> {
    match step {
        Ok(CoroStep::Yielded(v)) => Ok(v),
        Ok(CoroStep::Returned(v)) => Err(make_stop_iteration_with_value(v)),
        Err(e) => Err(e),
    }
}

fn cancel_handle_if_some(interp: &mut Interpreter, handle: &mut Option<Value>) {
    if let Some(h) = handle.take() {
        let _ = call_method(interp, &h, "cancel", &[]);
    }
}

fn step_sub_iter(
    interp: &mut Interpreter,
    iter: &Value,
    sent: Value,
    inject_exc: Option<PyError>,
) -> Result<CoroStep> {
    if is_coroutine_obj(iter) {
        return step_coro_once(interp, iter, sent, inject_exc);
    }
    if let Some(exc) = inject_exc {
        let exc_val = pyerror_to_exc_value(interp, exc)?;
        if let Ok(throw_m) = interp.get_attr(iter, "throw") {
            match call_pos(interp, throw_m, &[exc_val]) {
                Ok(v) => return Ok(CoroStep::Yielded(v)),
                Err(e) if e.class_name_is("StopIteration") => {
                    let val = extract_stop_iter_val(interp, e);
                    return Ok(CoroStep::Returned(val));
                }
                Err(e) => return Err(e),
            }
        }
        return Err(PyError::raised(exc_val));
    }
    let res = if !sent.is_none()
        && let Ok(send_m) = interp.get_attr(iter, "send")
    {
        call_pos(interp, send_m, &[sent])
    } else {
        call_method(interp, iter, "__next__", &[])
    };
    match res {
        Ok(v) => Ok(CoroStep::Yielded(v)),
        Err(e) if e.class_name_is("StopIteration") => {
            let val = extract_stop_iter_val(interp, e);
            Ok(CoroStep::Returned(val))
        }
        Err(e) => Err(e),
    }
}

fn extract_stop_iter_val(interp: &mut Interpreter, err: PyError) -> Value {
    if let pyrust_core::PyErrorKind::Raised(v) = err.into_kind()
        && let Ok(val) = interp.get_attr(&v, "value")
    {
        return val;
    }
    Value::none()
}

fn step_body(
    interp: &mut Interpreter,
    body: &mut CoroBody,
    sent_value: Value,
    inject_exc: Option<PyError>,
) -> Result<CoroStep> {
    match body {
        CoroBody::External(external) => external.step(interp, sent_value, inject_exc),

        // ── Future.__await__ ─────────────────────────────────────────────
        CoroBody::AwaitFuture { fut, yielded } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            if !is_fut_done(interp, fut)? {
                if !*yielded {
                    *yielded = true;
                    return yield_future(fut);
                }
                return Err(PyError::named(
                    "RuntimeError",
                    "await wasn't used with future",
                ));
            }
            let res = get_fut_result(interp, fut)?;
            Ok(CoroStep::Returned(res))
        }

        // ── YieldOnce ────────────────────────────────────────────────────
        CoroBody::YieldOnce { yielded, result } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            if !*yielded {
                *yielded = true;
                return Ok(CoroStep::Yielded(Value::none()));
            }
            Ok(CoroStep::Returned(result.clone()))
        }

        // ── ReadyValue ───────────────────────────────────────────────────
        CoroBody::ReadyValue { result } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            Ok(CoroStep::Returned(result.clone()))
        }

        // ── WrapAwaitable ────────────────────────────────────────────────
        CoroBody::WrapAwaitable { awaitable, iter } => {
            if iter.is_none() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let it = call_method(interp, awaitable, "__await__", &[])?;
                *iter = Some(it);
            }
            let it = iter.as_ref().unwrap().clone();
            step_sub_iter(interp, &it, sent_value, inject_exc)
        }

        // ── asyncio.sleep ────────────────────────────────────────────────
        CoroBody::Sleep {
            delay,
            result,
            waiter,
            timer,
        } => {
            if let Some(exc) = inject_exc {
                cancel_handle_if_some(interp, timer);
                return Err(exc);
            }
            if let Some(w) = waiter.as_ref() {
                cancel_handle_if_some(interp, timer);
                let res = get_fut_result(interp, w)?;
                return Ok(CoroStep::Returned(res));
            }
            if *delay <= 0.0 {
                // Handled via YieldOnce when created, but guard just in case.
                return Ok(CoroStep::Returned(result.clone()));
            }
            let loop_val = get_running_loop_or_err()?;
            let fut = loop_create_future(interp, &loop_val)?;
            let helper = asyncio_attr(interp, "_set_result_unless_done")?;
            let h = loop_call_later(
                interp,
                &loop_val,
                *delay,
                helper,
                vec![fut.clone(), result.clone()],
                None,
            )?;
            *waiter = Some(fut.clone());
            *timer = Some(h);
            yield_future(&fut)
        }

        // ── asyncio.wait ─────────────────────────────────────────────────
        CoroBody::Wait {
            fs,
            timeout,
            return_when,
            waiter,
            timer,
            cb,
        } => {
            if let Some(exc) = inject_exc {
                cancel_handle_if_some(interp, timer);
                if let Some(c) = cb.take() {
                    for f in fs.iter() {
                        let _ = future_remove_done_callback(interp, f, &c);
                    }
                }
                return Err(exc);
            }
            if waiter.is_some() {
                cancel_handle_if_some(interp, timer);
                if let Some(c) = cb.take() {
                    for f in fs.iter() {
                        let _ = future_remove_done_callback(interp, f, &c);
                    }
                }
                let mut done = Vec::new();
                let mut pending = Vec::new();
                for f in fs.iter() {
                    if is_fut_done(interp, f)? {
                        done.push(f.clone());
                    } else {
                        pending.push(f.clone());
                    }
                }
                let done_set = make_py_set(interp, done)?;
                let pending_set = make_py_set(interp, pending)?;
                return Ok(CoroStep::Returned(Value::tuple(vec![
                    done_set,
                    pending_set,
                ])));
            }

            let loop_val = get_running_loop_or_err()?;
            let w = loop_create_future(interp, &loop_val)?;
            let th = if let Some(t) = *timeout {
                let rel = asyncio_attr(interp, "_release_waiter")?;
                Some(loop_call_later(
                    interp,
                    &loop_val,
                    t,
                    rel,
                    vec![w.clone()],
                    None,
                )?)
            } else {
                None
            };
            let cb_obj = alloc_asyncio_instance(interp, "_WaitCallback")?;
            set_native_state(
                &cb_obj,
                WaitCallbackState {
                    counter: fs.len() as i64,
                    return_when: return_when.clone(),
                    waiter: w.clone(),
                    timeout_handle: th.clone(),
                },
            )?;
            for f in fs.iter() {
                if has_native_state::<FutureState>(f) {
                    future_add_done_callback(interp, f, cb_obj.clone(), None)?;
                } else {
                    call_method(
                        interp,
                        f,
                        "add_done_callback",
                        std::slice::from_ref(&cb_obj),
                    )?;
                }
            }
            *waiter = Some(w.clone());
            *timer = th;
            *cb = Some(cb_obj);
            if is_fut_done(interp, &w)? {
                // Re-enter to collect done/pending immediately.
                return step_body(interp, body, Value::none(), None);
            }
            yield_future(&w)
        }

        // ── asyncio.wait_for ─────────────────────────────────────────────
        CoroBody::WaitFor {
            fut_arg,
            timeout,
            phase,
        } => loop {
            match phase {
                WaitForPhase::Init => {
                    if let Some(exc) = inject_exc {
                        return Err(exc);
                    }
                    if let Some(t) = *timeout
                        && t <= 0.0
                    {
                        let fut = ensure_future_impl(interp, fut_arg.clone(), None)?;
                        if is_fut_done(interp, &fut)? {
                            let res = get_fut_result(interp, &fut)?;
                            return Ok(CoroStep::Returned(res));
                        }
                        let loop_val = get_running_loop_or_err()?;
                        let w = loop_create_future(interp, &loop_val)?;
                        let cb_obj = alloc_asyncio_instance(interp, "_ReleaseWaiterCb")?;
                        set_native_state(&cb_obj, ReleaseWaiterCbState { waiter: w.clone() })?;
                        future_add_done_callback(interp, &fut, cb_obj.clone(), None)?;
                        let _ = future_cancel(interp, &fut, None)?;
                        *phase = WaitForPhase::ZeroCancelWait {
                            fut,
                            waiter: w.clone(),
                            cb: cb_obj,
                        };
                        return yield_future(&w);
                    }
                    let timeout_fn = asyncio_attr(interp, "timeout")?;
                    let t_arg = timeout.map(Value::float).unwrap_or_else(Value::none);
                    let cm = call_pos(interp, timeout_fn, &[t_arg])?;
                    let enter_coro = call_method(interp, &cm, "__aenter__", &[])?;
                    *phase = WaitForPhase::EnteringTimeout {
                        timeout_cm: cm,
                        enter_coro,
                        fut: fut_arg.clone(),
                    };
                }
                WaitForPhase::ZeroCancelWait { fut, waiter, cb } => {
                    let _ = future_remove_done_callback(interp, fut, cb);
                    if let Some(exc) = inject_exc {
                        return Err(exc);
                    }
                    let _ = waiter;
                    match get_fut_result(interp, fut) {
                        Ok(res) => return Ok(CoroStep::Returned(res)),
                        Err(e) => {
                            let exc_val = pyerror_to_exc_value(interp, e)?;
                            if is_cancelled_error_val(interp, &exc_val) {
                                let te = instantiate_exception(
                                    lookup_exc_class("TimeoutError").expect("TimeoutError"),
                                    Vec::new(),
                                );
                                set_exc_cause(interp, &te, Some(exc_val));
                                return Err(PyError::raised(te));
                            }
                            return Err(PyError::raised(exc_val));
                        }
                    }
                }
                WaitForPhase::EnteringTimeout {
                    timeout_cm,
                    enter_coro,
                    fut,
                } => {
                    match step_sub_iter(interp, enter_coro, Value::none(), inject_exc) {
                        Ok(CoroStep::Yielded(v)) => return Ok(CoroStep::Yielded(v)),
                        Ok(CoroStep::Returned(_)) => {
                            let cm = timeout_cm.clone();
                            let fut_iter = if is_coroutine_obj(fut) {
                                fut.clone()
                            } else {
                                call_method(interp, fut, "__await__", &[])?
                            };
                            *phase = WaitForPhase::AwaitingInner {
                                timeout_cm: cm,
                                fut_iter,
                            };
                            // Step immediately once (no inject_exc now).
                            return step_body(interp, body, Value::none(), None);
                        }
                        Err(e) => return Err(e),
                    }
                }
                WaitForPhase::AwaitingInner {
                    timeout_cm,
                    fut_iter,
                } => match step_sub_iter(interp, fut_iter, sent_value, inject_exc) {
                    Ok(CoroStep::Yielded(v)) => return Ok(CoroStep::Yielded(v)),
                    Ok(CoroStep::Returned(res)) => {
                        let cm = timeout_cm.clone();
                        let exit_coro = call_method(
                            interp,
                            &cm,
                            "__aexit__",
                            &[Value::none(), Value::none(), Value::none()],
                        )?;
                        *phase = WaitForPhase::ExitingTimeout {
                            exit_coro,
                            saved_result: Some(res),
                            saved_exc: None,
                        };
                        return step_body(interp, body, Value::none(), None);
                    }
                    Err(e) => {
                        let exc_val = pyerror_to_exc_value(interp, e)?;
                        let exc_type = interp
                            .get_attr(&exc_val, "__class__")
                            .unwrap_or_else(|_| Value::none());
                        let cm = timeout_cm.clone();
                        let exit_coro = call_method(
                            interp,
                            &cm,
                            "__aexit__",
                            &[exc_type, exc_val.clone(), Value::none()],
                        )?;
                        *phase = WaitForPhase::ExitingTimeout {
                            exit_coro,
                            saved_result: None,
                            saved_exc: Some(exc_val),
                        };
                        return step_body(interp, body, Value::none(), None);
                    }
                },
                WaitForPhase::ExitingTimeout {
                    exit_coro,
                    saved_result,
                    saved_exc,
                } => match step_sub_iter(interp, exit_coro, Value::none(), inject_exc) {
                    Ok(CoroStep::Yielded(v)) => return Ok(CoroStep::Yielded(v)),
                    Ok(CoroStep::Returned(suppress)) => {
                        if let Some(exc) = saved_exc.take()
                            && !interp.truthy_value(&suppress)?
                        {
                            return Err(PyError::raised(exc));
                        }
                        return Ok(CoroStep::Returned(
                            saved_result.take().unwrap_or_else(Value::none),
                        ));
                    }
                    Err(e) => return Err(e),
                },
            }
        },

        // ── asyncio.as_completed item coroutine ──────────────────────────
        CoroBody::AsCompletedOne { queue, get_coro } => {
            if get_coro.is_none() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let gc = call_method(interp, queue, "get", &[])?;
                *get_coro = Some(gc);
            }
            let gc = get_coro.as_ref().unwrap().clone();
            match step_sub_iter(interp, &gc, sent_value, inject_exc) {
                Ok(CoroStep::Yielded(v)) => Ok(CoroStep::Yielded(v)),
                Ok(CoroStep::Returned(f)) => {
                    if f.is_none() {
                        return Err(PyError::named("TimeoutError", String::new()));
                    }
                    let res = get_fut_result(interp, &f)?;
                    Ok(CoroStep::Returned(res))
                }
                Err(e) => Err(e),
            }
        }

        // ── Timeout.__aenter__ / __aexit__ ───────────────────────────────
        CoroBody::TimeoutAenter { timeout_obj } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let loop_val = get_running_loop_or_err()?;
            let task = current_task_for_loop(&loop_val).ok_or_else(|| {
                PyError::named("RuntimeError", "Timeout should be used inside a task")
            })?;
            let cancelling = with_native_state::<FutureState, _>(&task, "Task", |st| {
                Ok(st
                    .task
                    .as_ref()
                    .map(|t| t.num_cancels_requested)
                    .unwrap_or(0))
            })?;
            let when = with_native_state_mut::<TimeoutState, _>(timeout_obj, "Timeout", |st| {
                if st.status != TimeoutStatus::Created {
                    return Err(PyError::named(
                        "RuntimeError",
                        "Timeout has already been entered",
                    ));
                }
                st.status = TimeoutStatus::Entered;
                st.task = Some(task);
                st.cancelling_at_enter = cancelling;
                Ok(st.when)
            })?;
            let when_val = when.map(Value::float).unwrap_or_else(Value::none);
            call_method(interp, timeout_obj, "reschedule", &[when_val])?;
            Ok(CoroStep::Returned(timeout_obj.clone()))
        }

        CoroBody::TimeoutAexit {
            timeout_obj,
            exc_type,
            exc_val,
        } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let (status, handler, task_opt, cancelling_at_enter) =
                with_native_state_mut::<TimeoutState, _>(timeout_obj, "Timeout", |st| {
                    let h = st.timeout_handler.take();
                    let status = st.status;
                    if status == TimeoutStatus::Expiring {
                        st.status = TimeoutStatus::Expired;
                    } else if status == TimeoutStatus::Entered {
                        st.status = TimeoutStatus::Exited;
                    }
                    Ok((status, h, st.task.clone(), st.cancelling_at_enter))
                })?;
            if let Some(h) = handler {
                let _ = call_method(interp, &h, "cancel", &[]);
            }
            if status == TimeoutStatus::Expiring
                && let Some(task) = task_opt
            {
                let remaining = with_native_state_mut::<FutureState, _>(&task, "Task", |st| {
                    if let Some(ref mut t) = st.task
                        && t.num_cancels_requested > 0
                    {
                        t.num_cancels_requested -= 1;
                    }
                    Ok(st
                        .task
                        .as_ref()
                        .map(|t| t.num_cancels_requested)
                        .unwrap_or(0))
                })?;
                let ce_cls = asyncio_attr(interp, "CancelledError")?;
                if remaining <= cancelling_at_enter && same_object(exc_type, &ce_cls) {
                    let te = instantiate_exception(
                        lookup_exc_class("TimeoutError").expect("TimeoutError"),
                        Vec::new(),
                    );
                    set_exc_cause(interp, &te, Some(exc_val.clone()));
                    return Err(PyError::raised(te));
                }
            }
            Ok(CoroStep::Returned(Value::none()))
        }

        // ── TaskGroup.__aenter__ / __aexit__ ─────────────────────────────
        CoroBody::TaskGroupAenter { tg_obj } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let already =
                with_native_state::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| Ok(st.entered))?;
            if already {
                let r = render_value_repr(interp, tg_obj)?;
                return Err(PyError::named(
                    "RuntimeError",
                    format!("TaskGroup {r} has already been entered"),
                ));
            }
            let loop_val = get_running_loop_or_err()?;
            let parent = current_task_for_loop(&loop_val);
            let Some(parent_task) = parent else {
                let r = render_value_repr(interp, tg_obj)?;
                return Err(PyError::named(
                    "RuntimeError",
                    format!("TaskGroup {r} cannot determine the parent task"),
                ));
            };
            with_native_state_mut::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
                if st.loop_val.is_none() {
                    st.loop_val = Some(loop_val);
                }
                st.parent_task = Some(parent_task);
                st.entered = true;
                Ok(())
            })?;
            Ok(CoroStep::Returned(tg_obj.clone()))
        }

        CoroBody::TaskGroupAexit {
            tg_obj,
            et,
            exc,
            started,
            propagate_cancel,
            waiting_fut,
        } => {
            let ce_cls = asyncio_attr(interp, "CancelledError")?;
            if !*started {
                *started = true;
                if let Some(e) = inject_exc {
                    return Err(e);
                }
                let is_base = !exc.is_none() && is_base_fatal_error_val(interp, exc);
                let is_ce = !et.is_none() && same_object(et, &ce_cls);
                if is_ce {
                    *propagate_cancel = Some(exc.clone());
                }
                let (parent_cancel_req, parent_task, should_abort) =
                    with_native_state_mut::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
                        st.exiting = true;
                        if is_base && st.base_error.is_none() {
                            st.base_error = Some(exc.clone());
                        }
                        let sa = !et.is_none() && !st.aborting;
                        Ok((st.parent_cancel_requested, st.parent_task.clone(), sa))
                    })?;
                if parent_cancel_req && let Some(pt) = parent_task {
                    let rem = call_method(interp, &pt, "uncancel", &[])?;
                    if rem.as_int() == Some(0) {
                        *propagate_cancel = None;
                    }
                }
                if should_abort {
                    task_group_abort(interp, tg_obj)?;
                }
            } else {
                // Resumed after awaiting `on_completed_fut`.
                *waiting_fut = None;
                let _ = with_native_state_mut::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
                    st.on_completed_fut = None;
                    Ok(())
                });
                if let Some(err) = inject_exc {
                    let exc_val = pyerror_to_exc_value(interp, err)?;
                    if is_cancelled_error_val(interp, &exc_val) {
                        let aborting =
                            with_native_state::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
                                Ok(st.aborting)
                            })?;
                        if !aborting {
                            *propagate_cancel = Some(exc_val);
                            task_group_abort(interp, tg_obj)?;
                        }
                    } else {
                        return Err(PyError::raised(exc_val));
                    }
                }
            }

            // Wait while tasks remain.
            let (has_tasks, loop_val_opt, existing_fut) =
                with_native_state::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
                    Ok((
                        !st.tasks.is_empty(),
                        st.loop_val.clone(),
                        st.on_completed_fut.clone(),
                    ))
                })?;
            if has_tasks {
                let fut = match existing_fut {
                    Some(f) => f,
                    None => {
                        let lv = loop_val_opt.ok_or_else(|| {
                            PyError::named("RuntimeError", "TaskGroup has no loop")
                        })?;
                        let f = loop_create_future(interp, &lv)?;
                        with_native_state_mut::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
                            st.on_completed_fut = Some(f.clone());
                            Ok(())
                        })?;
                        f
                    }
                };
                *waiting_fut = Some(fut.clone());
                return yield_future(&fut);
            }

            // All child tasks finished! Finalize errors.
            let (base_err, mut errors) =
                with_native_state_mut::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
                    let be = st.base_error.clone();
                    let errs = st.errors.take().unwrap_or_default();
                    Ok((be, errs))
                })?;

            if let Some(be) = base_err {
                return Err(PyError::raised(be));
            }
            if let Some(pce) = propagate_cancel.take()
                && errors.is_empty()
            {
                return Err(PyError::raised(pce));
            }
            if !et.is_none() && !same_object(et, &ce_cls) {
                errors.push(exc.clone());
            }
            if !errors.is_empty() {
                let beg_cls = Value::py_class(
                    lookup_exc_class("BaseExceptionGroup").expect("BaseExceptionGroup"),
                );
                let eg = call_pos(
                    interp,
                    beg_cls,
                    &[
                        Value::string("unhandled errors in a TaskGroup"),
                        Value::list(errors),
                    ],
                )?;
                set_exc_cause(interp, &eg, None);
                return Err(PyError::raised(eg));
            }
            Ok(CoroStep::Returned(Value::none()))
        }

        // ── Lock.acquire ─────────────────────────────────────────────────
        CoroBody::LockAcquire { lock_obj, waiter } => {
            if let Some(w) = waiter.take() {
                // Remove waiter from queue
                let should_wake = with_native_state_mut::<LockState, _>(lock_obj, "Lock", |st| {
                    st.waiters.retain(|x| !same_object(x, &w));
                    Ok(!st.locked)
                })?;
                if let Some(err) = inject_exc {
                    let exc_val = pyerror_to_exc_value(interp, err)?;
                    if is_cancelled_error_val(interp, &exc_val) && should_wake {
                        lock_wake_up_first(interp, lock_obj)?;
                    }
                    return Err(PyError::raised(exc_val));
                }
                // Check if the waiter itself failed/was cancelled
                if let Err(err) = get_fut_result(interp, &w) {
                    let exc_val = pyerror_to_exc_value(interp, err)?;
                    if is_cancelled_error_val(interp, &exc_val) && should_wake {
                        lock_wake_up_first(interp, lock_obj)?;
                    }
                    return Err(PyError::raised(exc_val));
                }
                with_native_state_mut::<LockState, _>(lock_obj, "Lock", |st| {
                    st.locked = true;
                    Ok(())
                })?;
                return Ok(CoroStep::Returned(Value::bool_(true)));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let can_acquire = with_native_state::<LockState, _>(lock_obj, "Lock", |st| {
                if st.locked {
                    return Ok(false);
                }
                let all_cancelled = st.waiters.iter().all(|w| {
                    with_native_state::<FutureState, _>(w, "Future", |fs| Ok(fs.is_cancelled()))
                        .unwrap_or(false)
                });
                Ok(all_cancelled)
            })?;
            if can_acquire {
                with_native_state_mut::<LockState, _>(lock_obj, "Lock", |st| {
                    st.locked = true;
                    Ok(())
                })?;
                return Ok(CoroStep::Returned(Value::bool_(true)));
            }
            let mut stored_loop =
                with_native_state::<LockState, _>(lock_obj, "Lock", |st| Ok(st.loop_val.clone()))?;
            let loop_val = bind_or_check_loop(interp, lock_obj, &mut stored_loop)?;
            let fut = loop_create_future(interp, &loop_val)?;
            with_native_state_mut::<LockState, _>(lock_obj, "Lock", |st| {
                st.loop_val = stored_loop;
                st.waiters.push_back(fut.clone());
                Ok(())
            })?;
            *waiter = Some(fut.clone());
            yield_future(&fut)
        }

        // ── ContextManagerMixin __aenter__ / __aexit__ ───────────────────
        CoroBody::ContextManagerAenter { obj, inner } => {
            if inner.is_none() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let acq = call_method(interp, obj, "acquire", &[])?;
                *inner = Some(acq);
            }
            let acq = inner.as_ref().unwrap().clone();
            match step_sub_iter(interp, &acq, sent_value, inject_exc) {
                Ok(CoroStep::Yielded(v)) => Ok(CoroStep::Yielded(v)),
                Ok(CoroStep::Returned(_)) => Ok(CoroStep::Returned(Value::none())),
                Err(e) => Err(e),
            }
        }

        CoroBody::ContextManagerAexit { obj } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let _ = call_method(interp, obj, "release", &[])?;
            Ok(CoroStep::Returned(Value::none()))
        }

        // ── Event.wait ───────────────────────────────────────────────────
        CoroBody::EventWait { event_obj, waiter } => {
            if let Some(w) = waiter.take() {
                let _ = with_native_state_mut::<EventState, _>(event_obj, "Event", |st| {
                    st.waiters.retain(|x| !same_object(x, &w));
                    Ok(())
                });
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
                return Ok(CoroStep::Returned(Value::bool_(true)));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let is_set = with_native_state::<EventState, _>(event_obj, "Event", |st| Ok(st.value))?;
            if is_set {
                return Ok(CoroStep::Returned(Value::bool_(true)));
            }
            let mut stored_loop = with_native_state::<EventState, _>(event_obj, "Event", |st| {
                Ok(st.loop_val.clone())
            })?;
            let loop_val = bind_or_check_loop(interp, event_obj, &mut stored_loop)?;
            let fut = loop_create_future(interp, &loop_val)?;
            with_native_state_mut::<EventState, _>(event_obj, "Event", |st| {
                st.loop_val = stored_loop;
                st.waiters.push_back(fut.clone());
                Ok(())
            })?;
            *waiter = Some(fut.clone());
            yield_future(&fut)
        }

        // ── Condition.wait ───────────────────────────────────────────────
        CoroBody::ConditionWait { cond_obj, phase } => match phase {
            CondWaitPhase::Init => {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let locked = call_method(interp, cond_obj, "locked", &[])?;
                if !interp.truthy_value(&locked)? {
                    return Err(PyError::named(
                        "RuntimeError",
                        "cannot wait on un-acquired lock",
                    ));
                }
                let _ = call_method(interp, cond_obj, "release", &[])?;
                let mut stored_loop =
                    with_native_state::<ConditionState, _>(cond_obj, "Condition", |st| {
                        Ok(st.loop_val.clone())
                    })?;
                let loop_val = bind_or_check_loop(interp, cond_obj, &mut stored_loop)?;
                let fut = loop_create_future(interp, &loop_val)?;
                with_native_state_mut::<ConditionState, _>(cond_obj, "Condition", |st| {
                    st.loop_val = stored_loop;
                    st.waiters.push_back(fut.clone());
                    Ok(())
                })?;
                *phase = CondWaitPhase::WaitingFuture {
                    waiter: fut.clone(),
                };
                yield_future(&fut)
            }
            CondWaitPhase::WaitingFuture { waiter } => {
                let w = waiter.clone();
                let _ = with_native_state_mut::<ConditionState, _>(cond_obj, "Condition", |st| {
                    st.waiters.retain(|x| !same_object(x, &w));
                    Ok(())
                });
                let mut cancelled = false;
                let mut saved_exc: Option<Value> = None;
                if let Some(err) = inject_exc {
                    let ev = pyerror_to_exc_value(interp, err)?;
                    if is_cancelled_error_val(interp, &ev) {
                        cancelled = true;
                    } else {
                        saved_exc = Some(ev);
                    }
                } else if let Err(err) = get_fut_result(interp, &w) {
                    let ev = pyerror_to_exc_value(interp, err)?;
                    if is_cancelled_error_val(interp, &ev) {
                        cancelled = true;
                    } else {
                        saved_exc = Some(ev);
                    }
                }
                let acq = call_method(interp, cond_obj, "acquire", &[])?;
                *phase = CondWaitPhase::Reacquiring {
                    acquire_coro: acq,
                    cancelled,
                    saved_exc,
                };
                step_body(interp, body, Value::none(), None)
            }
            CondWaitPhase::Reacquiring {
                acquire_coro,
                cancelled,
                saved_exc,
            } => match step_sub_iter(interp, acquire_coro, sent_value, inject_exc) {
                Ok(CoroStep::Yielded(v)) => Ok(CoroStep::Yielded(v)),
                Ok(CoroStep::Returned(_)) => {
                    if let Some(ev) = saved_exc.take() {
                        return Err(PyError::raised(ev));
                    }
                    if *cancelled {
                        return Err(raise_asyncio_exc(interp, "CancelledError", ""));
                    }
                    Ok(CoroStep::Returned(Value::bool_(true)))
                }
                Err(err) => {
                    let ev = pyerror_to_exc_value(interp, err)?;
                    if is_cancelled_error_val(interp, &ev) {
                        *cancelled = true;
                        let acq = call_method(interp, cond_obj, "acquire", &[])?;
                        *acquire_coro = acq;
                        return step_body(interp, body, Value::none(), None);
                    }
                    Err(PyError::raised(ev))
                }
            },
        },

        // ── Condition.wait_for ───────────────────────────────────────────
        CoroBody::ConditionWaitFor {
            cond_obj,
            predicate,
            wait_coro,
        } => {
            if let Some(wc) = wait_coro.clone() {
                match step_sub_iter(interp, &wc, sent_value, inject_exc) {
                    Ok(CoroStep::Yielded(v)) => return Ok(CoroStep::Yielded(v)),
                    Ok(CoroStep::Returned(_)) => {
                        *wait_coro = None;
                    }
                    Err(e) => return Err(e),
                }
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }
            loop {
                let res = call_pos(interp, predicate.clone(), &[])?;
                if interp.truthy_value(&res)? {
                    return Ok(CoroStep::Returned(res));
                }
                let wc = call_method(interp, cond_obj, "wait", &[])?;
                match step_sub_iter(interp, &wc, Value::none(), None) {
                    Ok(CoroStep::Yielded(v)) => {
                        *wait_coro = Some(wc);
                        return Ok(CoroStep::Yielded(v));
                    }
                    Ok(CoroStep::Returned(_)) => continue,
                    Err(e) => return Err(e),
                }
            }
        }

        // ── Semaphore.acquire ────────────────────────────────────────────
        CoroBody::SemaphoreAcquire { sem_obj, waiter } => {
            if let Some(w) = waiter.take() {
                let _ = with_native_state_mut::<SemaphoreState, _>(sem_obj, "Semaphore", |st| {
                    st.waiters.retain(|x| !same_object(x, &w));
                    Ok(())
                });
                let w_cancelled =
                    with_native_state::<FutureState, _>(&w, "Future", |fs| Ok(fs.is_cancelled()))
                        .unwrap_or(false);
                if let Some(err) = inject_exc {
                    let ev = pyerror_to_exc_value(interp, err)?;
                    if is_cancelled_error_val(interp, &ev) && !w_cancelled {
                        with_native_state_mut::<SemaphoreState, _>(sem_obj, "Semaphore", |st| {
                            st.value += 1;
                            Ok(())
                        })?;
                        semaphore_wake_next(interp, sem_obj)?;
                    }
                    return Err(PyError::raised(ev));
                }
                if let Err(err) = get_fut_result(interp, &w) {
                    let ev = pyerror_to_exc_value(interp, err)?;
                    if is_cancelled_error_val(interp, &ev) && !w_cancelled {
                        with_native_state_mut::<SemaphoreState, _>(sem_obj, "Semaphore", |st| {
                            st.value += 1;
                            Ok(())
                        })?;
                        semaphore_wake_next(interp, sem_obj)?;
                    }
                    return Err(PyError::raised(ev));
                }
                let val_pos = with_native_state::<SemaphoreState, _>(sem_obj, "Semaphore", |st| {
                    Ok(st.value > 0)
                })?;
                if val_pos {
                    semaphore_wake_next(interp, sem_obj)?;
                }
                return Ok(CoroStep::Returned(Value::bool_(true)));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let locked = semaphore_is_locked(sem_obj)?;
            if !locked {
                with_native_state_mut::<SemaphoreState, _>(sem_obj, "Semaphore", |st| {
                    st.value -= 1;
                    Ok(())
                })?;
                return Ok(CoroStep::Returned(Value::bool_(true)));
            }
            let mut stored_loop =
                with_native_state::<SemaphoreState, _>(sem_obj, "Semaphore", |st| {
                    Ok(st.loop_val.clone())
                })?;
            let loop_val = bind_or_check_loop(interp, sem_obj, &mut stored_loop)?;
            let fut = loop_create_future(interp, &loop_val)?;
            with_native_state_mut::<SemaphoreState, _>(sem_obj, "Semaphore", |st| {
                st.loop_val = stored_loop;
                st.waiters.push_back(fut.clone());
                Ok(())
            })?;
            *waiter = Some(fut.clone());
            yield_future(&fut)
        }

        // ── Barrier.wait ─────────────────────────────────────────────────
        CoroBody::BarrierWait { barrier_obj, phase } => loop {
            let cond = with_native_state::<BarrierState, _>(barrier_obj, "Barrier", |st| {
                Ok(st.cond.clone())
            })?;
            match phase {
                BarrierWaitPhase::Init => {
                    if let Some(exc) = inject_exc {
                        return Err(exc);
                    }
                    let acq = call_method(interp, &cond, "acquire", &[])?;
                    *phase = BarrierWaitPhase::AcquiringCond { acquire_coro: acq };
                }
                BarrierWaitPhase::AcquiringCond { acquire_coro } => {
                    match step_sub_iter(interp, acquire_coro, sent_value, inject_exc) {
                        Ok(CoroStep::Yielded(v)) => return Ok(CoroStep::Yielded(v)),
                        Ok(CoroStep::Returned(_)) => {
                            let status = with_native_state::<BarrierState, _>(
                                barrier_obj,
                                "Barrier",
                                |st| Ok(st.status),
                            )?;
                            if matches!(status, BarrierStatus::Draining | BarrierStatus::Resetting)
                            {
                                let wc = call_method(interp, &cond, "wait", &[])?;
                                *phase = BarrierWaitPhase::WaitingUnblock { wait_coro: wc };
                                return step_body(interp, body, Value::none(), None);
                            }
                            if status == BarrierStatus::Broken {
                                let _ = call_method(interp, &cond, "release", &[]);
                                return Err(raise_asyncio_exc(
                                    interp,
                                    "BrokenBarrierError",
                                    "Barrier aborted",
                                ));
                            }
                            let (index, reached) = with_native_state_mut::<BarrierState, _>(
                                barrier_obj,
                                "Barrier",
                                |st| {
                                    let idx = st.count;
                                    st.count += 1;
                                    let r = idx + 1 == st.parties;
                                    if r {
                                        st.status = BarrierStatus::Draining;
                                    }
                                    Ok((idx, r))
                                },
                            )?;
                            if reached {
                                let _ = call_method(interp, &cond, "notify_all", &[]);
                                barrier_exit_locked(interp, barrier_obj, &cond)?;
                                let _ = call_method(interp, &cond, "release", &[]);
                                return Ok(CoroStep::Returned(Value::int(index)));
                            }
                            let wc = call_method(interp, &cond, "wait", &[])?;
                            *phase = BarrierWaitPhase::WaitingFill {
                                wait_coro: wc,
                                index,
                            };
                            return step_body(interp, body, Value::none(), None);
                        }
                        Err(e) => return Err(e),
                    }
                }
                BarrierWaitPhase::WaitingUnblock { wait_coro } => {
                    match step_sub_iter(interp, wait_coro, sent_value, inject_exc) {
                        Ok(CoroStep::Yielded(v)) => return Ok(CoroStep::Yielded(v)),
                        Ok(CoroStep::Returned(_)) => {
                            let status = with_native_state::<BarrierState, _>(
                                barrier_obj,
                                "Barrier",
                                |st| Ok(st.status),
                            )?;
                            if matches!(status, BarrierStatus::Draining | BarrierStatus::Resetting)
                            {
                                let wc = call_method(interp, &cond, "wait", &[])?;
                                *wait_coro = wc;
                                return step_body(interp, body, Value::none(), None);
                            }
                            if status == BarrierStatus::Broken {
                                let _ = call_method(interp, &cond, "release", &[]);
                                return Err(raise_asyncio_exc(
                                    interp,
                                    "BrokenBarrierError",
                                    "Barrier aborted",
                                ));
                            }
                            let (index, reached) = with_native_state_mut::<BarrierState, _>(
                                barrier_obj,
                                "Barrier",
                                |st| {
                                    let idx = st.count;
                                    st.count += 1;
                                    let r = idx + 1 == st.parties;
                                    if r {
                                        st.status = BarrierStatus::Draining;
                                    }
                                    Ok((idx, r))
                                },
                            )?;
                            if reached {
                                let _ = call_method(interp, &cond, "notify_all", &[]);
                                barrier_exit_locked(interp, barrier_obj, &cond)?;
                                let _ = call_method(interp, &cond, "release", &[]);
                                return Ok(CoroStep::Returned(Value::int(index)));
                            }
                            let wc = call_method(interp, &cond, "wait", &[])?;
                            *phase = BarrierWaitPhase::WaitingFill {
                                wait_coro: wc,
                                index,
                            };
                            return step_body(interp, body, Value::none(), None);
                        }
                        Err(e) => {
                            let _ = call_method(interp, &cond, "release", &[]);
                            return Err(e);
                        }
                    }
                }
                BarrierWaitPhase::WaitingFill { wait_coro, index } => {
                    match step_sub_iter(interp, wait_coro, sent_value, inject_exc) {
                        Ok(CoroStep::Yielded(v)) => return Ok(CoroStep::Yielded(v)),
                        Ok(CoroStep::Returned(_)) => {
                            let status = with_native_state::<BarrierState, _>(
                                barrier_obj,
                                "Barrier",
                                |st| Ok(st.status),
                            )?;
                            if status == BarrierStatus::Filling {
                                let wc = call_method(interp, &cond, "wait", &[])?;
                                *wait_coro = wc;
                                return step_body(interp, body, Value::none(), None);
                            }
                            let idx_val = *index;
                            barrier_exit_locked(interp, barrier_obj, &cond)?;
                            let _ = call_method(interp, &cond, "release", &[]);
                            if matches!(status, BarrierStatus::Broken | BarrierStatus::Resetting) {
                                return Err(raise_asyncio_exc(
                                    interp,
                                    "BrokenBarrierError",
                                    "Abort or reset of barrier",
                                ));
                            }
                            return Ok(CoroStep::Returned(Value::int(idx_val)));
                        }
                        Err(e) => {
                            let _ = barrier_exit_locked(interp, barrier_obj, &cond);
                            let _ = call_method(interp, &cond, "release", &[]);
                            return Err(e);
                        }
                    }
                }
            }
        },

        CoroBody::BarrierReset {
            barrier_obj,
            acquire_coro,
        } => {
            let cond = with_native_state::<BarrierState, _>(barrier_obj, "Barrier", |st| {
                Ok(st.cond.clone())
            })?;
            if acquire_coro.is_none() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                *acquire_coro = Some(call_method(interp, &cond, "acquire", &[])?);
            }
            let acq = acquire_coro.as_ref().unwrap().clone();
            match step_sub_iter(interp, &acq, sent_value, inject_exc) {
                Ok(CoroStep::Yielded(v)) => Ok(CoroStep::Yielded(v)),
                Ok(CoroStep::Returned(_)) => {
                    with_native_state_mut::<BarrierState, _>(barrier_obj, "Barrier", |st| {
                        if st.count > 0 {
                            if st.status != BarrierStatus::Resetting {
                                st.status = BarrierStatus::Resetting;
                            }
                        } else {
                            st.status = BarrierStatus::Filling;
                        }
                        Ok(())
                    })?;
                    let _ = call_method(interp, &cond, "notify_all", &[]);
                    let _ = call_method(interp, &cond, "release", &[]);
                    Ok(CoroStep::Returned(Value::none()))
                }
                Err(e) => Err(e),
            }
        }

        CoroBody::BarrierAbort {
            barrier_obj,
            acquire_coro,
        } => {
            let cond = with_native_state::<BarrierState, _>(barrier_obj, "Barrier", |st| {
                Ok(st.cond.clone())
            })?;
            if acquire_coro.is_none() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                *acquire_coro = Some(call_method(interp, &cond, "acquire", &[])?);
            }
            let acq = acquire_coro.as_ref().unwrap().clone();
            match step_sub_iter(interp, &acq, sent_value, inject_exc) {
                Ok(CoroStep::Yielded(v)) => Ok(CoroStep::Yielded(v)),
                Ok(CoroStep::Returned(_)) => {
                    with_native_state_mut::<BarrierState, _>(barrier_obj, "Barrier", |st| {
                        st.status = BarrierStatus::Broken;
                        Ok(())
                    })?;
                    let _ = call_method(interp, &cond, "notify_all", &[]);
                    let _ = call_method(interp, &cond, "release", &[]);
                    Ok(CoroStep::Returned(Value::none()))
                }
                Err(e) => Err(e),
            }
        }

        // ── Queue.put / Queue.get / Queue.join ───────────────────────────
        CoroBody::QueuePut {
            queue_obj,
            item,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                let _ = with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
                    st.putters.retain(|x| !same_object(x, &w));
                    Ok(())
                });
                let err_opt = if let Some(e) = inject_exc {
                    Some(e)
                } else {
                    get_fut_result(interp, &w).err()
                };
                if let Some(e) = err_opt {
                    let _ = future_cancel(interp, &w, None);
                    let w_cancelled = with_native_state::<FutureState, _>(&w, "Future", |fs| {
                        Ok(fs.is_cancelled())
                    })
                    .unwrap_or(false);
                    let full = queue_is_full(queue_obj)?;
                    if !full && !w_cancelled {
                        queue_wake_next_putter(interp, queue_obj)?;
                    }
                    return Err(e);
                }
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            if queue_is_full(queue_obj)? {
                let mut stored_loop =
                    with_native_state::<QueueState, _>(queue_obj, "Queue", |st| {
                        Ok(st.loop_val.clone())
                    })?;
                let loop_val = bind_or_check_loop(interp, queue_obj, &mut stored_loop)?;
                let fut = loop_create_future(interp, &loop_val)?;
                with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
                    st.loop_val = stored_loop;
                    st.putters.push_back(fut.clone());
                    Ok(())
                })?;
                *waiter = Some(fut.clone());
                return yield_future(&fut);
            }
            queue_put_nowait(interp, queue_obj, item.clone())?;
            Ok(CoroStep::Returned(Value::none()))
        }

        CoroBody::QueueGet { queue_obj, waiter } => {
            if let Some(w) = waiter.take() {
                let _ = with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
                    st.getters.retain(|x| !same_object(x, &w));
                    Ok(())
                });
                let err_opt = if let Some(e) = inject_exc {
                    Some(e)
                } else {
                    get_fut_result(interp, &w).err()
                };
                if let Some(e) = err_opt {
                    let _ = future_cancel(interp, &w, None);
                    let w_cancelled = with_native_state::<FutureState, _>(&w, "Future", |fs| {
                        Ok(fs.is_cancelled())
                    })
                    .unwrap_or(false);
                    let empty = queue_is_empty(queue_obj)?;
                    if !empty && !w_cancelled {
                        queue_wake_next_getter(interp, queue_obj)?;
                    }
                    return Err(e);
                }
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            if queue_is_empty(queue_obj)? {
                let mut stored_loop =
                    with_native_state::<QueueState, _>(queue_obj, "Queue", |st| {
                        Ok(st.loop_val.clone())
                    })?;
                let loop_val = bind_or_check_loop(interp, queue_obj, &mut stored_loop)?;
                let fut = loop_create_future(interp, &loop_val)?;
                with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
                    st.loop_val = stored_loop;
                    st.getters.push_back(fut.clone());
                    Ok(())
                })?;
                *waiter = Some(fut.clone());
                return yield_future(&fut);
            }
            let item = queue_get_nowait(interp, queue_obj)?;
            Ok(CoroStep::Returned(item))
        }

        CoroBody::QueueJoin {
            queue_obj,
            wait_coro,
        } => {
            if wait_coro.is_none() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let (unfinished, finished_ev) =
                    with_native_state::<QueueState, _>(queue_obj, "Queue", |st| {
                        Ok((st.unfinished_tasks, st.finished.clone()))
                    })?;
                if unfinished <= 0 {
                    return Ok(CoroStep::Returned(Value::none()));
                }
                let wc = call_method(interp, &finished_ev, "wait", &[])?;
                *wait_coro = Some(wc);
            }
            let wc = wait_coro.as_ref().unwrap().clone();
            match step_sub_iter(interp, &wc, sent_value, inject_exc) {
                Ok(CoroStep::Yielded(v)) => Ok(CoroStep::Yielded(v)),
                Ok(CoroStep::Returned(_)) => Ok(CoroStep::Returned(Value::none())),
                Err(e) => Err(e),
            }
        }

        // ── StreamReader.read ────────────────────────────────────────────
        CoroBody::StreamRead { reader, n, waiter } => {
            if let Some(w) = waiter.take() {
                stream_reader_clear_waiter(reader, &w);
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            // Check stored exception first.
            if let Some(exc) = stream_reader_exception(reader)? {
                return Err(PyError::raised(exc));
            }
            if *n == 0 {
                return Ok(CoroStep::Returned(Value::bytes(Vec::new())));
            }
            if *n < 0 {
                // Read until EOF.
                let is_eof =
                    with_native_state::<StreamReaderState, _>(reader, "StreamReader", |st| {
                        Ok(st.eof)
                    })?;
                if is_eof {
                    let data = stream_reader_take_all(interp, reader)?;
                    return Ok(CoroStep::Returned(Value::bytes(data)));
                }
                let w = stream_reader_wait_for_data(interp, reader)?;
                *waiter = Some(w.clone());
                return yield_future(&w);
            }
            let want = *n as usize;
            let (has_data, is_eof) =
                with_native_state::<StreamReaderState, _>(reader, "StreamReader", |st| {
                    Ok((!st.buffer.is_empty(), st.eof))
                })?;
            if has_data || is_eof {
                let data = stream_reader_take_up_to(interp, reader, want)?;
                return Ok(CoroStep::Returned(Value::bytes(data)));
            }
            let w = stream_reader_wait_for_data(interp, reader)?;
            *waiter = Some(w.clone());
            yield_future(&w)
        }

        // ── StreamReader.readline ────────────────────────────────────────
        CoroBody::StreamReadline { reader, waiter } => {
            if let Some(w) = waiter.take() {
                stream_reader_clear_waiter(reader, &w);
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            if let Some(exc) = stream_reader_exception(reader)? {
                return Err(PyError::raised(exc));
            }
            enum ReadlineCheck {
                Found(usize),
                Eof(Vec<u8>),
                Overrun,
                NeedMore,
            }
            let check =
                with_native_state_mut::<StreamReaderState, _>(reader, "StreamReader", |st| {
                    if let Some(pos) = st.buffer.iter().position(|&b| b == b'\n') {
                        let end = pos + 1;
                        if pos > st.limit {
                            st.buffer.drain(..end);
                            return Ok(ReadlineCheck::Overrun);
                        }
                        return Ok(ReadlineCheck::Found(end));
                    }
                    if st.buffer.len() > st.limit {
                        st.buffer.clear();
                        return Ok(ReadlineCheck::Overrun);
                    }
                    if st.eof {
                        let data = std::mem::take(&mut st.buffer);
                        return Ok(ReadlineCheck::Eof(data));
                    }
                    Ok(ReadlineCheck::NeedMore)
                })?;
            match check {
                ReadlineCheck::Found(end) => {
                    let data = stream_reader_take_up_to(interp, reader, end)?;
                    Ok(CoroStep::Returned(Value::bytes(data)))
                }
                ReadlineCheck::Eof(data) => {
                    stream_reader_maybe_resume_transport(interp, reader)?;
                    Ok(CoroStep::Returned(Value::bytes(data)))
                }
                ReadlineCheck::Overrun => {
                    stream_reader_maybe_resume_transport(interp, reader)?;
                    Err(PyError::named(
                        "ValueError",
                        "Separator is not found, and chunk exceed the limit",
                    ))
                }
                ReadlineCheck::NeedMore => {
                    let w = stream_reader_wait_for_data(interp, reader)?;
                    *waiter = Some(w.clone());
                    yield_future(&w)
                }
            }
        }

        // ── StreamReader.readexactly ─────────────────────────────────────
        CoroBody::StreamReadexactly { reader, n, waiter } => {
            if let Some(w) = waiter.take() {
                stream_reader_clear_waiter(reader, &w);
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            if let Some(exc) = stream_reader_exception(reader)? {
                return Err(PyError::raised(exc));
            }
            if *n == 0 {
                return Ok(CoroStep::Returned(Value::bytes(Vec::new())));
            }
            let (buflen, is_eof) =
                with_native_state::<StreamReaderState, _>(reader, "StreamReader", |st| {
                    Ok((st.buffer.len(), st.eof))
                })?;
            if buflen >= *n {
                let data = stream_reader_take_up_to(interp, reader, *n)?;
                return Ok(CoroStep::Returned(Value::bytes(data)));
            }
            if is_eof {
                let partial = stream_reader_take_all(interp, reader)?;
                let ire = make_asyncio_exc(
                    interp,
                    "IncompleteReadError",
                    &[Value::bytes(partial), Value::int(*n as i64)],
                )?;
                return Err(PyError::raised(ire));
            }
            let w = stream_reader_wait_for_data(interp, reader)?;
            *waiter = Some(w.clone());
            yield_future(&w)
        }

        // ── StreamReader.readuntil ───────────────────────────────────────
        CoroBody::StreamReaduntil {
            reader,
            sep,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                stream_reader_clear_waiter(reader, &w);
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            if sep.is_empty() {
                return Err(PyError::named(
                    "ValueError",
                    "Separator should be at least one-byte string",
                ));
            }
            if let Some(exc) = stream_reader_exception(reader)? {
                return Err(PyError::raised(exc));
            }

            enum UntilCheck {
                Found(usize),
                OverrunFound(usize),
                OverrunNoSep(usize),
                Incomplete(Vec<u8>),
                NeedMore,
            }

            let seplen = sep.len();
            let check =
                with_native_state_mut::<StreamReaderState, _>(reader, "StreamReader", |st| {
                    if st.buffer.len() >= seplen
                        && let Some(idx) = find_subslice(&st.buffer, sep)
                    {
                        if idx > st.limit {
                            return Ok(UntilCheck::OverrunFound(idx));
                        }
                        return Ok(UntilCheck::Found(idx + seplen));
                    }
                    if st.buffer.len() > st.limit {
                        let consumed = st.buffer.len();
                        return Ok(UntilCheck::OverrunNoSep(consumed));
                    }
                    if st.eof {
                        let partial = std::mem::take(&mut st.buffer);
                        return Ok(UntilCheck::Incomplete(partial));
                    }
                    Ok(UntilCheck::NeedMore)
                })?;

            match check {
                UntilCheck::Found(end) => {
                    let data = stream_reader_take_up_to(interp, reader, end)?;
                    Ok(CoroStep::Returned(Value::bytes(data)))
                }
                UntilCheck::OverrunFound(consumed) => {
                    let loe = make_asyncio_exc(
                        interp,
                        "LimitOverrunError",
                        &[
                            Value::string("Separator is found, but chunk is longer than limit"),
                            Value::int(consumed as i64),
                        ],
                    )?;
                    Err(PyError::raised(loe))
                }
                UntilCheck::OverrunNoSep(consumed) => {
                    let loe = make_asyncio_exc(
                        interp,
                        "LimitOverrunError",
                        &[
                            Value::string("Separator is not found, and chunk exceed the limit"),
                            Value::int(consumed as i64),
                        ],
                    )?;
                    Err(PyError::raised(loe))
                }
                UntilCheck::Incomplete(partial) => {
                    stream_reader_maybe_resume_transport(interp, reader)?;
                    let ire = make_asyncio_exc(
                        interp,
                        "IncompleteReadError",
                        &[Value::bytes(partial), Value::none()],
                    )?;
                    Err(PyError::raised(ire))
                }
                UntilCheck::NeedMore => {
                    let w = stream_reader_wait_for_data(interp, reader)?;
                    *waiter = Some(w.clone());
                    yield_future(&w)
                }
            }
        }

        // ── StreamReader.__anext__ ───────────────────────────────────────
        CoroBody::StreamAnext {
            reader,
            readline_coro,
        } => {
            if readline_coro.is_none() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let rc = call_method(interp, reader, "readline", &[])?;
                *readline_coro = Some(rc);
            }
            let rc = readline_coro.as_ref().unwrap().clone();
            match step_sub_iter(interp, &rc, sent_value, inject_exc) {
                Ok(CoroStep::Yielded(v)) => Ok(CoroStep::Yielded(v)),
                Ok(CoroStep::Returned(line)) => {
                    let is_empty = matches!(line.kind(), ValueKind::Bytes(b) if b.is_empty());
                    if is_empty {
                        Err(make_stop_async_iteration())
                    } else {
                        Ok(CoroStep::Returned(line))
                    }
                }
                Err(e) => Err(e),
            }
        }

        // ── StreamWriter.drain ───────────────────────────────────────────
        CoroBody::StreamDrain {
            writer,
            phase,
            waiter,
        } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            if *phase == 0 {
                let (reader_opt, transport, protocol, loop_val) =
                    with_native_state::<StreamWriterState, _>(writer, "StreamWriter", |st| {
                        Ok((
                            st.reader.clone(),
                            st.transport.clone(),
                            st.protocol.clone(),
                            st.loop_val.clone(),
                        ))
                    })?;
                if let Some(ref r) = reader_opt
                    && let Some(exc) = stream_reader_exception(r)?
                {
                    return Err(PyError::raised(exc));
                }
                let is_closing = call_method(interp, &transport, "is_closing", &[])
                    .and_then(|v| interp.truthy_value(&v))
                    .unwrap_or(false);
                if is_closing {
                    *phase = 1;
                    return Ok(CoroStep::Yielded(Value::none()));
                }
                // If protocol is paused for backpressure, wait on drain_waiter.
                if has_native_state::<StreamReaderProtocolState>(&protocol) {
                    let paused = with_native_state::<StreamReaderProtocolState, _>(
                        &protocol,
                        "StreamReaderProtocol",
                        |st| Ok(st.paused && !st.connection_lost),
                    )?;
                    if paused {
                        let w = loop_create_future(interp, &loop_val)?;
                        with_native_state_mut::<StreamReaderProtocolState, _>(
                            &protocol,
                            "StreamReaderProtocol",
                            |st| {
                                st.drain_waiter = Some(w.clone());
                                Ok(())
                            },
                        )?;
                        *phase = 2;
                        *waiter = Some(w.clone());
                        return yield_future(&w);
                    }
                }
                // Yield one turn so queued socket writes flush.
                *phase = 1;
                return Ok(CoroStep::Yielded(Value::none()));
            }
            if *phase == 2
                && let Some(w) = waiter.take()
            {
                let _ = get_fut_result(interp, &w)?;
            }
            Ok(CoroStep::Returned(Value::none()))
        }

        // ── StreamWriter.wait_closed ─────────────────────────────────────
        CoroBody::StreamWaitClosed { writer, waiter } => {
            if let Some(w) = waiter.take() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
                return Ok(CoroStep::Returned(Value::none()));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let (protocol, loop_val) =
                with_native_state::<StreamWriterState, _>(writer, "StreamWriter", |st| {
                    Ok((st.protocol.clone(), st.loop_val.clone()))
                })?;
            if has_native_state::<StreamReaderProtocolState>(&protocol) {
                let (lost, existing) = with_native_state::<StreamReaderProtocolState, _>(
                    &protocol,
                    "StreamReaderProtocol",
                    |st| Ok((st.connection_lost, st.closed_fut.clone())),
                )?;
                if lost {
                    return Ok(CoroStep::Returned(Value::none()));
                }
                let fut = match existing {
                    Some(f) => f,
                    None => {
                        let f = loop_create_future(interp, &loop_val)?;
                        with_native_state_mut::<StreamReaderProtocolState, _>(
                            &protocol,
                            "StreamReaderProtocol",
                            |st| {
                                st.closed_fut = Some(f.clone());
                                Ok(())
                            },
                        )?;
                        f
                    }
                };
                *waiter = Some(fut.clone());
                return yield_future(&fut);
            }
            Ok(CoroStep::Returned(Value::none()))
        }

        // ── StreamWriter.start_tls ───────────────────────────────────────
        CoroBody::StreamStartTls {
            writer,
            sslcontext,
            server_hostname,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
                return Ok(CoroStep::Returned(Value::none()));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let (transport, protocol, loop_val) =
                with_native_state::<StreamWriterState, _>(writer, "StreamWriter", |st| {
                    Ok((
                        st.transport.clone(),
                        st.protocol.clone(),
                        st.loop_val.clone(),
                    ))
                })?;
            let w = loop_create_future(interp, &loop_val)?;
            io_engine::upgrade_transport_to_tls(
                interp,
                &transport,
                protocol,
                sslcontext.clone(),
                false,
                server_hostname.clone(),
                w.clone(),
            )?;
            if is_fut_done(interp, &w)? {
                let _ = get_fut_result(interp, &w)?;
                return Ok(CoroStep::Returned(Value::none()));
            }
            *waiter = Some(w.clone());
            yield_future(&w)
        }

        // ── Server.wait_closed / serve_forever / __aenter__ / __aexit__ ──
        CoroBody::ServerWaitClosed { server, waiter } => {
            if let Some(w) = waiter.take() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
                return Ok(CoroStep::Returned(Value::none()));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let (closed, loop_val) = with_native_state::<ServerState, _>(server, "Server", |st| {
                Ok((st.closed, st.loop_val.clone()))
            })?;
            if closed {
                return Ok(CoroStep::Returned(Value::none()));
            }
            let w = loop_create_future(interp, &loop_val)?;
            with_native_state_mut::<ServerState, _>(server, "Server", |st| {
                st.waiters.push(w.clone());
                Ok(())
            })?;
            *waiter = Some(w.clone());
            yield_future(&w)
        }

        CoroBody::ServerServeForever {
            server,
            started,
            waiter,
        } => {
            if let Some(exc) = inject_exc {
                let _ = io_engine::server_close(interp, server);
                return Err(exc);
            }
            if let Some(w) = waiter.take() {
                let _ = get_fut_result(interp, &w)?;
                return Ok(CoroStep::Returned(Value::none()));
            }
            if !*started {
                *started = true;
                io_engine::server_start_serving(interp, server)?;
            }
            let (closed, loop_val) = with_native_state::<ServerState, _>(server, "Server", |st| {
                Ok((st.closed, st.loop_val.clone()))
            })?;
            if closed {
                return Ok(CoroStep::Returned(Value::none()));
            }
            let w = loop_create_future(interp, &loop_val)?;
            with_native_state_mut::<ServerState, _>(server, "Server", |st| {
                st.waiters.push(w.clone());
                Ok(())
            })?;
            *waiter = Some(w.clone());
            yield_future(&w)
        }

        CoroBody::ServerAenter { server } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            Ok(CoroStep::Returned(server.clone()))
        }

        CoroBody::ServerAexit { server, wait_coro } => {
            if wait_coro.is_none() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                io_engine::server_close(interp, server)?;
                let wc = call_method(interp, server, "wait_closed", &[])?;
                *wait_coro = Some(wc);
            }
            let wc = wait_coro.as_ref().unwrap().clone();
            match step_sub_iter(interp, &wc, sent_value, inject_exc) {
                Ok(CoroStep::Yielded(v)) => Ok(CoroStep::Yielded(v)),
                Ok(CoroStep::Returned(_)) => Ok(CoroStep::Returned(Value::none())),
                Err(e) => Err(e),
            }
        }

        // ── Loop.sock_recv ───────────────────────────────────────────────
        CoroBody::LoopSockRecv {
            loop_val,
            sock,
            nbytes,
            fd,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                let _ = io_engine::loop_remove_reader(loop_val, *fd);
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            match call_method(interp, sock, "recv", &[Value::int(*nbytes)]) {
                Ok(data) => Ok(CoroStep::Returned(data)),
                Err(err) => {
                    let ev = pyerror_to_exc_value(interp, err)?;
                    if is_blocking_io_error(interp, &ev) {
                        let w = loop_create_future(interp, loop_val)?;
                        let cb = make_release_waiter_cb(interp, w.clone())?;
                        io_engine::loop_add_reader(interp, loop_val, *fd, cb, Vec::new())?;
                        *waiter = Some(w.clone());
                        return yield_future(&w);
                    }
                    Err(PyError::raised(ev))
                }
            }
        }

        // ── Loop.sock_recv_into ──────────────────────────────────────────
        CoroBody::LoopSockRecvInto {
            loop_val,
            sock,
            buf,
            fd,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                let _ = io_engine::loop_remove_reader(loop_val, *fd);
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            match call_method(interp, sock, "recv_into", std::slice::from_ref(buf)) {
                Ok(n) => Ok(CoroStep::Returned(n)),
                Err(err) => {
                    let ev = pyerror_to_exc_value(interp, err)?;
                    if is_blocking_io_error(interp, &ev) {
                        let w = loop_create_future(interp, loop_val)?;
                        let cb = make_release_waiter_cb(interp, w.clone())?;
                        io_engine::loop_add_reader(interp, loop_val, *fd, cb, Vec::new())?;
                        *waiter = Some(w.clone());
                        return yield_future(&w);
                    }
                    Err(PyError::raised(ev))
                }
            }
        }

        // ── Loop.sock_sendall ────────────────────────────────────────────
        CoroBody::LoopSockSendall {
            loop_val,
            sock,
            data,
            offset,
            fd,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                let _ = io_engine::loop_remove_writer(loop_val, *fd);
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            while *offset < data.len() {
                let slice = data[*offset..].to_vec();
                match call_method(interp, sock, "send", &[Value::bytes(slice)]) {
                    Ok(n_val) => {
                        let sent = n_val.as_int().unwrap_or(0) as usize;
                        if sent == 0 {
                            return Err(PyError::named(
                                "ConnectionResetError",
                                "socket connection broken",
                            ));
                        }
                        *offset += sent;
                    }
                    Err(err) => {
                        let ev = pyerror_to_exc_value(interp, err)?;
                        if is_blocking_io_error(interp, &ev) {
                            let w = loop_create_future(interp, loop_val)?;
                            let cb = make_release_waiter_cb(interp, w.clone())?;
                            io_engine::loop_add_writer(interp, loop_val, *fd, cb, Vec::new())?;
                            *waiter = Some(w.clone());
                            return yield_future(&w);
                        }
                        return Err(PyError::raised(ev));
                    }
                }
            }
            Ok(CoroStep::Returned(Value::none()))
        }

        // ── Loop.sock_connect ────────────────────────────────────────────
        CoroBody::LoopSockConnect {
            loop_val,
            sock,
            address,
            fd,
            started,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                let _ = io_engine::loop_remove_writer(loop_val, *fd);
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
                // Check SO_ERROR to verify connection succeeded.
                let sock_mod = interp.load_module("socket")?;
                let sol = interp.get_attr(&sock_mod, "SOL_SOCKET")?;
                let so_err = interp.get_attr(&sock_mod, "SO_ERROR")?;
                if let Ok(code_val) = call_method(interp, sock, "getsockopt", &[sol, so_err])
                    && let Some(code) = code_val.as_int()
                    && code != 0
                {
                    let err = std::io::Error::from_raw_os_error(code as i32);
                    return Err(PyError::from_io_error(&err, None));
                }
                return Ok(CoroStep::Returned(Value::none()));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            if !*started {
                *started = true;
                let code_val =
                    call_method(interp, sock, "connect_ex", std::slice::from_ref(address))?;
                let code = code_val.as_int().unwrap_or(0);
                if code == 0 {
                    return Ok(CoroStep::Returned(Value::none()));
                }
                if is_in_progress_errno(code) {
                    let w = loop_create_future(interp, loop_val)?;
                    let cb = make_release_waiter_cb(interp, w.clone())?;
                    io_engine::loop_add_writer(interp, loop_val, *fd, cb, Vec::new())?;
                    *waiter = Some(w.clone());
                    return yield_future(&w);
                }
                let err = std::io::Error::from_raw_os_error(code as i32);
                return Err(PyError::from_io_error(&err, None));
            }
            Ok(CoroStep::Returned(Value::none()))
        }

        // ── Loop.sock_accept ─────────────────────────────────────────────
        CoroBody::LoopSockAccept {
            loop_val,
            sock,
            fd,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                let _ = io_engine::loop_remove_reader(loop_val, *fd);
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
            } else if let Some(exc) = inject_exc {
                return Err(exc);
            }

            match call_method(interp, sock, "accept", &[]) {
                Ok(pair) => {
                    if let ValueKind::Tuple(t) = pair.kind()
                        && !t.is_empty()
                    {
                        let _ = call_method(interp, &t[0], "setblocking", &[Value::bool_(false)]);
                    }
                    Ok(CoroStep::Returned(pair))
                }
                Err(err) => {
                    let ev = pyerror_to_exc_value(interp, err)?;
                    if is_blocking_io_error(interp, &ev) {
                        let w = loop_create_future(interp, loop_val)?;
                        let cb = make_release_waiter_cb(interp, w.clone())?;
                        io_engine::loop_add_reader(interp, loop_val, *fd, cb, Vec::new())?;
                        *waiter = Some(w.clone());
                        return yield_future(&w);
                    }
                    Err(PyError::raised(ev))
                }
            }
        }

        // ── Loop.create_connection ───────────────────────────────────────
        CoroBody::LoopCreateConnection {
            loop_val,
            protocol_factory,
            host,
            port,
            ssl_arg,
            family,
            sock,
            server_hostname,
            phase,
        } => loop {
            match phase {
                CreateConnPhase::Init => {
                    if let Some(exc) = inject_exc {
                        return Err(exc);
                    }
                    let ssl_ctx = io_engine::resolve_ssl_context(interp, ssl_arg.as_ref(), false)?;
                    let effective_hostname = server_hostname.clone().or_else(|| {
                        if ssl_ctx.is_some() {
                            host.clone()
                        } else {
                            None
                        }
                    });
                    if let Some(existing_sock) = sock.take() {
                        let _ = call_method(
                            interp,
                            &existing_sock,
                            "setblocking",
                            &[Value::bool_(false)],
                        );
                        let protocol = call_pos(interp, protocol_factory.clone(), &[])?;
                        if let Some(ctx) = ssl_ctx {
                            let w = loop_create_future(interp, loop_val)?;
                            let tls = TlsLayer::new(
                                interp,
                                ctx,
                                false,
                                effective_hostname,
                                Some(w.clone()),
                            )?;
                            let transport = io_engine::create_socket_transport(
                                interp,
                                loop_val.clone(),
                                existing_sock,
                                protocol.clone(),
                                Some(tls),
                                None,
                            )?;
                            if is_fut_done(interp, &w)? {
                                let _ = get_fut_result(interp, &w)?;
                                return Ok(CoroStep::Returned(Value::tuple(vec![
                                    transport, protocol,
                                ])));
                            }
                            *phase = CreateConnPhase::HandshakeWait {
                                transport,
                                protocol,
                                waiter: w.clone(),
                            };
                            return yield_future(&w);
                        } else {
                            let transport = io_engine::create_socket_transport(
                                interp,
                                loop_val.clone(),
                                existing_sock,
                                protocol.clone(),
                                None,
                                None,
                            )?;
                            return Ok(CoroStep::Returned(Value::tuple(vec![transport, protocol])));
                        }
                    }

                    let host_str = host.clone().unwrap_or_else(|| "127.0.0.1".to_string());
                    let port_num = port.ok_or_else(|| {
                        PyError::named("ValueError", "host and port must be specified")
                    })?;

                    let sock_mod = interp.load_module("socket")?;
                    let getaddrinfo = interp.get_attr(&sock_mod, "getaddrinfo")?;
                    let sock_stream = interp.get_attr(&sock_mod, "SOCK_STREAM")?;
                    let infos = call_pos(
                        interp,
                        getaddrinfo,
                        &[
                            Value::string(&host_str),
                            Value::int(port_num),
                            Value::int(*family),
                            sock_stream,
                        ],
                    )?;
                    let info_list = match infos.kind() {
                        ValueKind::List(items) if !items.is_empty() => items.clone(),
                        _ => {
                            return Err(PyError::named(
                                "OSError",
                                format!(
                                    "getaddrinfo() returned empty list for {host_str}:{port_num}"
                                ),
                            ));
                        }
                    };
                    let first = &info_list[0];
                    let (af, stype, proto, addr) = match first.kind() {
                        ValueKind::Tuple(t) if t.len() >= 5 => {
                            (t[0].clone(), t[1].clone(), t[2].clone(), t[4].clone())
                        }
                        _ => {
                            return Err(PyError::named("OSError", "invalid getaddrinfo entry"));
                        }
                    };
                    let sock_cls = interp.get_attr(&sock_mod, "socket")?;
                    let s = call_pos(interp, sock_cls, &[af, stype, proto])?;
                    let _ = call_method(interp, &s, "setblocking", &[Value::bool_(false)])?;
                    let fd = extract_fd(interp, &s)?;
                    let connect_coro = make_native_coro(
                        interp,
                        CoroBody::LoopSockConnect {
                            loop_val: loop_val.clone(),
                            sock: s.clone(),
                            address: addr,
                            fd,
                            started: false,
                            waiter: None,
                        },
                    )?;
                    *phase = CreateConnPhase::Connecting {
                        sock: s,
                        connect_coro,
                        ssl_ctx,
                        server_hostname: effective_hostname,
                    };
                }
                CreateConnPhase::Connecting {
                    sock: s,
                    connect_coro,
                    ssl_ctx,
                    server_hostname: sh,
                } => match step_sub_iter(interp, connect_coro, sent_value, inject_exc) {
                    Ok(CoroStep::Yielded(v)) => return Ok(CoroStep::Yielded(v)),
                    Ok(CoroStep::Returned(_)) => {
                        let s_val = s.clone();
                        let ctx_opt = ssl_ctx.clone();
                        let sh_opt = sh.clone();
                        let protocol = call_pos(interp, protocol_factory.clone(), &[])?;
                        if let Some(ctx) = ctx_opt {
                            let w = loop_create_future(interp, loop_val)?;
                            let tls = TlsLayer::new(interp, ctx, false, sh_opt, Some(w.clone()))?;
                            let transport = io_engine::create_socket_transport(
                                interp,
                                loop_val.clone(),
                                s_val,
                                protocol.clone(),
                                Some(tls),
                                None,
                            )?;
                            if is_fut_done(interp, &w)? {
                                let _ = get_fut_result(interp, &w)?;
                                return Ok(CoroStep::Returned(Value::tuple(vec![
                                    transport, protocol,
                                ])));
                            }
                            *phase = CreateConnPhase::HandshakeWait {
                                transport,
                                protocol,
                                waiter: w.clone(),
                            };
                            return yield_future(&w);
                        } else {
                            let transport = io_engine::create_socket_transport(
                                interp,
                                loop_val.clone(),
                                s_val,
                                protocol.clone(),
                                None,
                                None,
                            )?;
                            return Ok(CoroStep::Returned(Value::tuple(vec![transport, protocol])));
                        }
                    }
                    Err(e) => {
                        let _ = call_method(interp, s, "close", &[]);
                        return Err(e);
                    }
                },
                CreateConnPhase::HandshakeWait {
                    transport,
                    protocol,
                    waiter,
                } => {
                    if let Some(exc) = inject_exc {
                        let _ = io_engine::transport_force_close(interp, transport, None);
                        return Err(exc);
                    }
                    let _ = get_fut_result(interp, waiter)?;
                    return Ok(CoroStep::Returned(Value::tuple(vec![
                        transport.clone(),
                        protocol.clone(),
                    ])));
                }
            }
        },

        // ── Loop.create_server ───────────────────────────────────────────
        CoroBody::LoopCreateServer {
            loop_val,
            protocol_factory,
            host,
            port,
            family,
            backlog,
            ssl_arg,
            reuse_address,
            sock,
            start_serving,
        } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let ssl_ctx = io_engine::resolve_ssl_context(interp, ssl_arg.as_ref(), true)?;
            let mut sockets = Vec::new();
            if let Some(existing_sock) = sock.take() {
                let _ = call_method(
                    interp,
                    &existing_sock,
                    "setblocking",
                    &[Value::bool_(false)],
                );
                sockets.push(existing_sock);
            } else {
                let sock_mod = interp.load_module("socket")?;
                let sock_cls = interp.get_attr(&sock_mod, "socket")?;
                let sol_socket = interp.get_attr(&sock_mod, "SOL_SOCKET")?;
                let so_reuseaddr = interp.get_attr(&sock_mod, "SO_REUSEADDR")?;
                let sock_stream = interp.get_attr(&sock_mod, "SOCK_STREAM")?;
                let getaddrinfo = interp.get_attr(&sock_mod, "getaddrinfo")?;

                let host_str = match host {
                    None => "0.0.0.0".to_string(),
                    Some(v) if v.is_none() => "0.0.0.0".to_string(),
                    Some(v) => v.as_str().unwrap_or("127.0.0.1").to_string(),
                };
                let port_num = port.unwrap_or(0);
                let infos = call_pos(
                    interp,
                    getaddrinfo,
                    &[
                        Value::string(&host_str),
                        Value::int(port_num),
                        Value::int(*family),
                        sock_stream,
                    ],
                )?;
                let info_list = match infos.kind() {
                    ValueKind::List(items) if !items.is_empty() => items.clone(),
                    _ => {
                        return Err(PyError::named(
                            "OSError",
                            format!("getaddrinfo() returned empty list for {host_str}:{port_num}"),
                        ));
                    }
                };
                let first = &info_list[0];
                let (af, stype, proto, addr) = match first.kind() {
                    ValueKind::Tuple(t) if t.len() >= 5 => {
                        (t[0].clone(), t[1].clone(), t[2].clone(), t[4].clone())
                    }
                    _ => {
                        return Err(PyError::named("OSError", "invalid getaddrinfo entry"));
                    }
                };
                let s = call_pos(interp, sock_cls, &[af, stype, proto])?;
                let do_reuse = reuse_address.unwrap_or(!cfg!(windows));
                if do_reuse {
                    let _ = call_method(
                        interp,
                        &s,
                        "setsockopt",
                        &[sol_socket, so_reuseaddr, Value::int(1)],
                    );
                }
                if let Err(e) = call_method(interp, &s, "bind", &[addr]) {
                    let _ = call_method(interp, &s, "close", &[]);
                    return Err(e);
                }
                let _ = call_method(interp, &s, "setblocking", &[Value::bool_(false)]);
                sockets.push(s);
            }

            let server = alloc_asyncio_instance(interp, "Server")?;
            set_native_state(
                &server,
                ServerState {
                    loop_val: loop_val.clone(),
                    sockets,
                    protocol_factory: protocol_factory.clone(),
                    ssl_context: ssl_ctx,
                    backlog: *backlog,
                    serving: false,
                    closed: false,
                    waiters: Vec::new(),
                },
            )?;
            if *start_serving {
                io_engine::server_start_serving(interp, &server)?;
            }
            Ok(CoroStep::Returned(server))
        }

        // ── Loop.create_datagram_endpoint ────────────────────────────────
        CoroBody::LoopCreateDatagramEndpoint {
            loop_val,
            protocol_factory,
            local_addr,
            remote_addr,
            family,
            sock,
        } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let s = if let Some(existing) = sock.take() {
                existing
            } else {
                let sock_mod = interp.load_module("socket")?;
                let sock_cls = interp.get_attr(&sock_mod, "socket")?;
                let af = if *family != 0 {
                    Value::int(*family)
                } else {
                    interp.get_attr(&sock_mod, "AF_INET")?
                };
                let dgram = interp.get_attr(&sock_mod, "SOCK_DGRAM")?;
                let s = call_pos(interp, sock_cls, &[af, dgram])?;
                if let Some(la) = local_addr {
                    call_method(interp, &s, "bind", std::slice::from_ref(la))?;
                }
                if let Some(ra) = remote_addr {
                    call_method(interp, &s, "connect", std::slice::from_ref(ra))?;
                }
                s
            };
            let protocol = call_pos(interp, protocol_factory.clone(), &[])?;
            let transport = io_engine::create_datagram_transport(
                interp,
                loop_val.clone(),
                s,
                protocol.clone(),
                remote_addr.clone(),
            )?;
            Ok(CoroStep::Returned(Value::tuple(vec![transport, protocol])))
        }

        // ── Loop.start_tls ───────────────────────────────────────────────
        CoroBody::LoopStartTls {
            loop_val,
            transport,
            protocol,
            sslcontext,
            server_side,
            server_hostname,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
                return Ok(CoroStep::Returned(transport.clone()));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let w = loop_create_future(interp, loop_val)?;
            io_engine::upgrade_transport_to_tls(
                interp,
                transport,
                protocol.clone(),
                sslcontext.clone(),
                *server_side,
                server_hostname.clone(),
                w.clone(),
            )?;
            if is_fut_done(interp, &w)? {
                let _ = get_fut_result(interp, &w)?;
                return Ok(CoroStep::Returned(transport.clone()));
            }
            *waiter = Some(w.clone());
            yield_future(&w)
        }

        // ── asyncio.open_connection ──────────────────────────────────────
        CoroBody::OpenConnection {
            host,
            port,
            limit,
            ssl_arg,
            family,
            sock,
            server_hostname,
            phase,
        } => loop {
            match phase {
                OpenConnPhase::Init => {
                    if let Some(exc) = inject_exc {
                        return Err(exc);
                    }
                    let loop_val = get_running_loop_or_err()?;
                    let reader = create_stream_reader(interp, *limit, Some(loop_val.clone()))?;
                    let protocol = create_stream_reader_protocol(
                        interp,
                        reader.clone(),
                        None,
                        loop_val.clone(),
                    )?;
                    let factory = make_const_factory(interp, protocol.clone())?;
                    let conn_coro = make_native_coro(
                        interp,
                        CoroBody::LoopCreateConnection {
                            loop_val,
                            protocol_factory: factory,
                            host: host.clone(),
                            port: *port,
                            ssl_arg: ssl_arg.clone(),
                            family: *family,
                            sock: sock.take(),
                            server_hostname: server_hostname.clone(),
                            phase: CreateConnPhase::Init,
                        },
                    )?;
                    *phase = OpenConnPhase::Connecting {
                        reader,
                        protocol,
                        conn_coro,
                    };
                }
                OpenConnPhase::Connecting {
                    reader,
                    protocol,
                    conn_coro,
                } => match step_sub_iter(interp, conn_coro, sent_value, inject_exc) {
                    Ok(CoroStep::Yielded(v)) => return Ok(CoroStep::Yielded(v)),
                    Ok(CoroStep::Returned(pair)) => {
                        let transport = match pair.kind() {
                            ValueKind::Tuple(t) if !t.is_empty() => t[0].clone(),
                            _ => {
                                return Err(PyError::runtime(
                                    "create_connection did not return (transport, protocol)".into(),
                                ));
                            }
                        };
                        let loop_val = get_running_loop_or_err()?;
                        let writer = create_stream_writer(
                            interp,
                            transport,
                            protocol.clone(),
                            Some(reader.clone()),
                            loop_val,
                        )?;
                        return Ok(CoroStep::Returned(Value::tuple(vec![
                            reader.clone(),
                            writer,
                        ])));
                    }
                    Err(e) => return Err(e),
                },
            }
        },

        // ── asyncio.start_server ─────────────────────────────────────────
        CoroBody::StartServer {
            client_connected_cb,
            host,
            port,
            limit,
            family,
            backlog,
            ssl_arg,
            reuse_address,
            sock,
            start_serving,
            inner_coro,
        } => {
            if inner_coro.is_none() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let loop_val = get_running_loop_or_err()?;
                let factory = alloc_asyncio_instance(interp, "_StreamServerProtoFactory")?;
                set_native_state(
                    &factory,
                    StreamServerProtoFactoryState {
                        client_connected_cb: client_connected_cb.clone(),
                        limit: *limit,
                        loop_val: loop_val.clone(),
                    },
                )?;
                let c = make_native_coro(
                    interp,
                    CoroBody::LoopCreateServer {
                        loop_val,
                        protocol_factory: factory,
                        host: host.clone(),
                        port: *port,
                        family: *family,
                        backlog: *backlog,
                        ssl_arg: ssl_arg.clone(),
                        reuse_address: *reuse_address,
                        sock: sock.take(),
                        start_serving: *start_serving,
                    },
                )?;
                *inner_coro = Some(c);
            }
            let ic = inner_coro.as_ref().unwrap().clone();
            step_sub_iter(interp, &ic, sent_value, inject_exc)
        }

        // ── Loop.subprocess_exec / subprocess_shell ──────────────────────
        CoroBody::LoopSubprocess {
            loop_val,
            protocol_factory,
            program,
            args,
            shell,
            stdin,
            stdout,
            stderr,
            cwd,
            env,
        } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let protocol = call_pos(interp, protocol_factory.clone(), &[])?;
            let transport = io_engine::spawn_subprocess_transport(
                interp,
                loop_val.clone(),
                protocol.clone(),
                program.clone(),
                args.clone(),
                *shell,
                stdin.as_ref(),
                stdout.as_ref(),
                stderr.as_ref(),
                cwd.clone(),
                env.clone(),
            )?;
            Ok(CoroStep::Returned(Value::tuple(vec![transport, protocol])))
        }

        // ── asyncio.create_subprocess_exec / shell ───────────────────────
        CoroBody::CreateSubprocess {
            program,
            args,
            shell,
            stdin,
            stdout,
            stderr,
            limit,
            cwd,
            env,
        } => {
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let loop_val = get_running_loop_or_err()?;
            let exited_fut = loop_create_future(interp, &loop_val)?;
            let proto = alloc_asyncio_instance(interp, "SubprocessStreamProtocol")?;
            set_native_state(
                &proto,
                SubprocessStreamProtocolState {
                    limit: *limit,
                    loop_val: loop_val.clone(),
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
            let transport = io_engine::spawn_subprocess_transport(
                interp,
                loop_val.clone(),
                proto.clone(),
                program.clone(),
                args.clone(),
                *shell,
                stdin.as_ref(),
                stdout.as_ref(),
                stderr.as_ref(),
                cwd.clone(),
                env.clone(),
            )?;
            let pid = with_native_state::<SubprocessTransportState, _>(
                &transport,
                "SubprocessTransport",
                |st| Ok(st.pid),
            )?;
            let (stdin_w, stdout_r, stderr_r) =
                with_native_state::<SubprocessStreamProtocolState, _>(
                    &proto,
                    "SubprocessStreamProtocol",
                    |st| Ok((st.stdin.clone(), st.stdout.clone(), st.stderr.clone())),
                )?;
            let proc = alloc_asyncio_instance(interp, "Process")?;
            set_native_state(
                &proc,
                ProcessState {
                    transport,
                    protocol: proto,
                    loop_val,
                    stdin: stdin_w,
                    stdout: stdout_r,
                    stderr: stderr_r,
                    pid,
                },
            )?;
            Ok(CoroStep::Returned(proc))
        }

        // ── Process.wait ─────────────────────────────────────────────────
        CoroBody::ProcessWait { proc, waiter } => {
            if let Some(w) = waiter.take() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let _ = get_fut_result(interp, &w)?;
                let rc = call_method(interp, proc, "_get_returncode", &[])?;
                return Ok(CoroStep::Returned(rc));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let (transport, protocol) =
                with_native_state::<ProcessState, _>(proc, "Process", |st| {
                    Ok((st.transport.clone(), st.protocol.clone()))
                })?;
            let rc = with_native_state::<SubprocessTransportState, _>(
                &transport,
                "SubprocessTransport",
                |st| Ok(st.returncode),
            )?;
            if let Some(code) = rc {
                return Ok(CoroStep::Returned(Value::int(code)));
            }
            let exited_fut = with_native_state::<SubprocessStreamProtocolState, _>(
                &protocol,
                "SubprocessStreamProtocol",
                |st| Ok(st.exited_fut.clone()),
            )?;
            if is_fut_done(interp, &exited_fut)? {
                let code = with_native_state::<SubprocessTransportState, _>(
                    &transport,
                    "SubprocessTransport",
                    |st| Ok(st.returncode.unwrap_or(0)),
                )?;
                return Ok(CoroStep::Returned(Value::int(code)));
            }
            *waiter = Some(exited_fut.clone());
            yield_future(&exited_fut)
        }

        // ── Process.communicate ──────────────────────────────────────────
        CoroBody::ProcessCommunicate { proc, input, phase } => loop {
            match phase {
                ProcCommPhase::Init => {
                    if let Some(exc) = inject_exc {
                        return Err(exc);
                    }
                    let (stdin_opt, stdout_opt, stderr_opt, loop_val) =
                        with_native_state::<ProcessState, _>(proc, "Process", |st| {
                            Ok((
                                st.stdin.clone(),
                                st.stdout.clone(),
                                st.stderr.clone(),
                                st.loop_val.clone(),
                            ))
                        })?;
                    if let Some(ref stdin_w) = stdin_opt {
                        if let Some(data) = input.take() {
                            let _ = call_method(interp, stdin_w, "write", &[Value::bytes(data)]);
                        }
                        let _ = call_method(interp, stdin_w, "close", &[]);
                    }

                    let mut tasks = Vec::new();
                    let stdout_coro = match stdout_opt {
                        Some(r) => call_method(interp, &r, "read", &[])?,
                        None => make_native_coro(
                            interp,
                            CoroBody::ReadyValue {
                                result: Value::none(),
                            },
                        )?,
                    };
                    let stderr_coro = match stderr_opt {
                        Some(r) => call_method(interp, &r, "read", &[])?,
                        None => make_native_coro(
                            interp,
                            CoroBody::ReadyValue {
                                result: Value::none(),
                            },
                        )?,
                    };
                    let wait_coro = call_method(interp, proc, "wait", &[])?;
                    tasks.push(stdout_coro);
                    tasks.push(stderr_coro);
                    tasks.push(wait_coro);
                    let gather_fn = asyncio_attr(interp, "gather")?;
                    let gather_fut = call_pos(interp, gather_fn, &tasks)?;
                    let _ = loop_val;
                    *phase = ProcCommPhase::Gathering {
                        gather_fut: gather_fut.clone(),
                    };
                    if is_fut_done(interp, &gather_fut)? {
                        continue;
                    }
                    return yield_future(&gather_fut);
                }
                ProcCommPhase::Gathering { gather_fut } => {
                    if let Some(exc) = inject_exc {
                        let _ = future_cancel(interp, gather_fut, None);
                        return Err(exc);
                    }
                    let res = get_fut_result(interp, gather_fut)?;
                    let (out_v, err_v) = match res.kind() {
                        ValueKind::List(items) if items.len() >= 2 => {
                            (items[0].clone(), items[1].clone())
                        }
                        _ => (Value::none(), Value::none()),
                    };
                    return Ok(CoroStep::Returned(Value::tuple(vec![out_v, err_v])));
                }
            }
        },

        // ── asyncio.to_thread ────────────────────────────────────────────
        CoroBody::ToThread {
            func,
            args,
            kwargs,
            waiter,
        } => {
            if let Some(w) = waiter.take() {
                if let Some(exc) = inject_exc {
                    return Err(exc);
                }
                let res = get_fut_result(interp, &w)?;
                return Ok(CoroStep::Returned(res));
            }
            if let Some(exc) = inject_exc {
                return Err(exc);
            }
            let loop_val = get_running_loop_or_err()?;
            let ctx = copy_context(interp)?;
            let thunk = alloc_asyncio_instance(interp, "_ExecutorThunk")?;
            set_native_state(
                &thunk,
                ExecutorThunkState {
                    func: func.clone(),
                    args: args.clone(),
                    kwargs: kwargs.clone(),
                    context: ctx,
                },
            )?;
            let fut = call_method(
                interp,
                &loop_val,
                "run_in_executor",
                &[Value::none(), thunk],
            )?;
            if is_fut_done(interp, &fut)? {
                let res = get_fut_result(interp, &fut)?;
                return Ok(CoroStep::Returned(res));
            }
            *waiter = Some(fut.clone());
            yield_future(&fut)
        }

        // ── asyncio.staggered.staggered_race ─────────────────────────────
        CoroBody::StaggeredRace {
            coro_fns,
            delay,
            loop_opt,
            phase,
        } => {
            if let Some(exc) = inject_exc {
                if let StaggeredPhase::Running {
                    tasks, delay_timer, ..
                } = phase
                {
                    cancel_handle_if_some(interp, delay_timer);
                    for t in tasks.iter() {
                        let _ = future_cancel(interp, t, None);
                    }
                }
                return Err(exc);
            }
            let loop_val = match loop_opt {
                Some(l) if !l.is_none() => l.clone(),
                _ => get_running_loop_or_err()?,
            };
            if let StaggeredPhase::Init = phase {
                if coro_fns.is_empty() {
                    return Ok(CoroStep::Returned(Value::tuple(vec![
                        Value::none(),
                        Value::none(),
                        Value::list(Vec::new()),
                    ])));
                }
                let waiter = loop_create_future(interp, &loop_val)?;
                let mut tasks = Vec::new();
                let mut exceptions = Vec::new();
                let coro0 = call_pos(interp, coro_fns[0].clone(), &[])?;
                let t0 = loop_create_task(interp, &loop_val, coro0, None, None)?;
                let cb = make_release_waiter_cb(interp, waiter.clone())?;
                future_add_done_callback(interp, &t0, cb, None)?;
                tasks.push(t0);
                exceptions.push(Value::none());
                let delay_timer = if coro_fns.len() > 1
                    && let Some(d) = *delay
                {
                    let rel = asyncio_attr(interp, "_release_waiter")?;
                    Some(loop_call_later(
                        interp,
                        &loop_val,
                        d,
                        rel,
                        vec![waiter.clone()],
                        None,
                    )?)
                } else {
                    None
                };
                *phase = StaggeredPhase::Running {
                    tasks,
                    exceptions,
                    winner_idx: None,
                    winner_val: None,
                    next_idx: 1,
                    waiter: waiter.clone(),
                    delay_timer,
                };
                return yield_future(&waiter);
            }

            if let StaggeredPhase::Running {
                tasks,
                exceptions,
                winner_idx,
                winner_val,
                next_idx,
                waiter,
                delay_timer,
            } = phase
            {
                cancel_handle_if_some(interp, delay_timer);
                // Inspect all started tasks.
                let mut all_done = true;
                for (idx, t) in tasks.iter().enumerate() {
                    if is_fut_done(interp, t)? {
                        if exceptions[idx].is_none() {
                            match get_fut_result(interp, t) {
                                Ok(val) => {
                                    *winner_idx = Some(idx);
                                    *winner_val = Some(val);
                                    break;
                                }
                                Err(err) => {
                                    let ev = pyerror_to_exc_value(interp, err)?;
                                    exceptions[idx] = ev;
                                }
                            }
                        }
                    } else {
                        all_done = false;
                    }
                }
                if let (Some(wi), Some(wv)) = (*winner_idx, winner_val.take()) {
                    for (idx, t) in tasks.iter().enumerate() {
                        if idx != wi && !is_fut_done(interp, t)? {
                            let _ = future_cancel(interp, t, None);
                        }
                    }
                    return Ok(CoroStep::Returned(Value::tuple(vec![
                        wv,
                        Value::int(wi as i64),
                        Value::list(exceptions.clone()),
                    ])));
                }
                if *next_idx < coro_fns.len() {
                    let idx = *next_idx;
                    *next_idx += 1;
                    let coro_i = call_pos(interp, coro_fns[idx].clone(), &[])?;
                    let ti = loop_create_task(interp, &loop_val, coro_i, None, None)?;
                    let new_w = loop_create_future(interp, &loop_val)?;
                    let cb = make_release_waiter_cb(interp, new_w.clone())?;
                    for t in tasks.iter() {
                        if !is_fut_done(interp, t)? {
                            let _ = future_add_done_callback(interp, t, cb.clone(), None);
                        }
                    }
                    let _ = future_add_done_callback(interp, &ti, cb, None);
                    tasks.push(ti);
                    exceptions.push(Value::none());
                    if *next_idx < coro_fns.len()
                        && let Some(d) = *delay
                    {
                        let rel = asyncio_attr(interp, "_release_waiter")?;
                        *delay_timer = Some(loop_call_later(
                            interp,
                            &loop_val,
                            d,
                            rel,
                            vec![new_w.clone()],
                            None,
                        )?);
                    }
                    *waiter = new_w.clone();
                    return yield_future(&new_w);
                }
                if all_done {
                    return Ok(CoroStep::Returned(Value::tuple(vec![
                        Value::none(),
                        Value::none(),
                        Value::list(exceptions.clone()),
                    ])));
                }
                let new_w = loop_create_future(interp, &loop_val)?;
                let cb = make_release_waiter_cb(interp, new_w.clone())?;
                for t in tasks.iter() {
                    if !is_fut_done(interp, t)? {
                        let _ = future_add_done_callback(interp, t, cb.clone(), None);
                    }
                }
                *waiter = new_w.clone();
                return yield_future(&new_w);
            }
            Ok(CoroStep::Returned(Value::none()))
        }
    }
}

// ── Native helper callable states (zero Python closures!) ────────────────────

pub(crate) struct WaitCallbackState {
    pub(crate) counter: i64,
    pub(crate) return_when: String,
    pub(crate) waiter: Value,
    pub(crate) timeout_handle: Option<Value>,
}

pub(crate) struct ReleaseWaiterCbState {
    pub(crate) waiter: Value,
}

pub(crate) struct ConstFactoryState {
    pub(crate) val: Value,
}

pub(crate) struct StreamServerProtoFactoryState {
    pub(crate) client_connected_cb: Value,
    pub(crate) limit: usize,
    pub(crate) loop_val: Value,
}

pub(crate) struct ExecutorThunkState {
    pub(crate) func: Value,
    pub(crate) args: Vec<Value>,
    pub(crate) kwargs: Vec<(String, Value)>,
    pub(crate) context: Value,
}

pub(crate) struct GatherDoneCbState {
    pub(crate) outer: Value,
    pub(crate) children: Vec<Value>,
    pub(crate) nfuts: usize,
    pub(crate) nfinished: Rc<RefCell<usize>>,
    pub(crate) return_exceptions: bool,
}

pub(crate) struct AsCompletedState {
    pub(crate) todo: Rc<RefCell<Vec<Value>>>,
    pub(crate) done_queue: Value,
    pub(crate) timeout_handle: Rc<RefCell<Option<Value>>>,
}

pub(crate) struct ShieldInnerCbState {
    pub(crate) outer: Value,
}

pub(crate) struct ShieldOuterCbState {
    pub(crate) inner: Value,
    pub(crate) inner_cb: Value,
}

pub(crate) struct WrapFutureCbState {
    pub(crate) src: Value,
    pub(crate) dst: Value,
}

pub(crate) struct EagerTaskFactoryState {
    pub(crate) custom_constructor: Value,
}

pub(crate) fn make_release_waiter_cb(interp: &mut Interpreter, waiter: Value) -> Result<Value> {
    let inst = alloc_asyncio_instance(interp, "_ReleaseWaiterCb")?;
    set_native_state(&inst, ReleaseWaiterCbState { waiter })?;
    Ok(inst)
}

pub(crate) fn make_const_factory(interp: &mut Interpreter, val: Value) -> Result<Value> {
    let inst = alloc_asyncio_instance(interp, "_ConstFactory")?;
    set_native_state(&inst, ConstFactoryState { val })?;
    Ok(inst)
}

// ── Shared Sync / Queue / Stream / TaskGroup helpers ─────────────────────────

pub(crate) fn task_group_abort(interp: &mut Interpreter, tg_obj: &Value) -> Result<()> {
    let tasks = with_native_state_mut::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
        st.aborting = true;
        Ok(st.tasks.clone())
    })?;
    for t in tasks {
        if !is_fut_done(interp, &t)? {
            let _ = future_cancel(interp, &t, None);
        }
    }
    Ok(())
}

pub(crate) fn task_group_on_task_done(
    interp: &mut Interpreter,
    tg_obj: &Value,
    task: &Value,
) -> Result<()> {
    let on_completed = with_native_state_mut::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
        st.tasks.retain(|t| !same_object(t, task));
        if st.tasks.is_empty() {
            Ok(st.on_completed_fut.clone())
        } else {
            Ok(None)
        }
    })?;
    if let Some(fut) = on_completed {
        future_set_result_if_pending(interp, &fut, Value::bool_(true))?;
    }

    let cancelled = with_native_state::<FutureState, _>(task, "Task", |st| Ok(st.is_cancelled()))
        .unwrap_or(false);
    if cancelled {
        return Ok(());
    }
    let exc = future_exception(interp, task)?;
    if exc.is_none() {
        return Ok(());
    }
    let is_base = is_base_fatal_error_val(interp, &exc);
    let (parent_opt, loop_opt, should_cancel_parent) =
        with_native_state_mut::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
            if let Some(ref mut errs) = st.errors {
                errs.push(exc.clone());
            }
            if is_base && st.base_error.is_none() {
                st.base_error = Some(exc.clone());
            }
            let scp = !st.aborting && !st.parent_cancel_requested;
            Ok((st.parent_task.clone(), st.loop_val.clone(), scp))
        })?;

    if let Some(ref parent) = parent_opt
        && is_fut_done(interp, parent)?
    {
        if let Some(lv) = loop_opt {
            let tr = render_value_repr(interp, task).unwrap_or_else(|_| "<Task>".into());
            let pr = render_value_repr(interp, parent).unwrap_or_else(|_| "<Task>".into());
            let ctx = super::make_py_dict(&[
                (
                    "message",
                    Value::string(format!(
                        "Task {tr} has errored out but its parent task {pr} is already completed"
                    )),
                ),
                ("exception", exc),
                ("task", task.clone()),
            ]);
            let _ = call_method(interp, &lv, "call_exception_handler", &[ctx]);
        }
        return Ok(());
    }

    if should_cancel_parent {
        task_group_abort(interp, tg_obj)?;
        with_native_state_mut::<TaskGroupState, _>(tg_obj, "TaskGroup", |st| {
            st.parent_cancel_requested = true;
            Ok(())
        })?;
        if let Some(parent) = parent_opt {
            let _ = future_cancel(interp, &parent, None);
        }
    }
    Ok(())
}

pub(crate) fn lock_wake_up_first(interp: &mut Interpreter, lock_obj: &Value) -> Result<()> {
    let first =
        with_native_state::<LockState, _>(lock_obj, "Lock", |st| Ok(st.waiters.front().cloned()))?;
    if let Some(fut) = first {
        future_set_result_if_pending(interp, &fut, Value::bool_(true))?;
    }
    Ok(())
}

pub(crate) fn semaphore_is_locked(sem_obj: &Value) -> Result<bool> {
    with_native_state::<SemaphoreState, _>(sem_obj, "Semaphore", |st| {
        if st.value == 0 {
            return Ok(true);
        }
        let any_active = st.waiters.iter().any(|w| {
            !with_native_state::<FutureState, _>(w, "Future", |fs| Ok(fs.is_cancelled()))
                .unwrap_or(false)
        });
        Ok(any_active)
    })
}

pub(crate) fn semaphore_wake_next(interp: &mut Interpreter, sem_obj: &Value) -> Result<()> {
    let next_fut = with_native_state_mut::<SemaphoreState, _>(sem_obj, "Semaphore", |st| {
        for fut in &st.waiters {
            let done = with_native_state::<FutureState, _>(fut, "Future", |fs| Ok(fs.is_done()))
                .unwrap_or(true);
            if !done {
                st.value -= 1;
                return Ok(Some(fut.clone()));
            }
        }
        Ok(None)
    })?;
    if let Some(fut) = next_fut {
        future_set_result_if_pending(interp, &fut, Value::bool_(true))?;
    }
    Ok(())
}

fn barrier_exit_locked(interp: &mut Interpreter, barrier_obj: &Value, cond: &Value) -> Result<()> {
    let should_notify = with_native_state_mut::<BarrierState, _>(barrier_obj, "Barrier", |st| {
        st.count -= 1;
        if st.count == 0 {
            if matches!(
                st.status,
                BarrierStatus::Resetting | BarrierStatus::Draining
            ) {
                st.status = BarrierStatus::Filling;
            }
            return Ok(true);
        }
        Ok(false)
    })?;
    if should_notify {
        let _ = call_method(interp, cond, "notify_all", &[]);
    }
    Ok(())
}

pub(crate) fn queue_is_empty(queue_obj: &Value) -> Result<bool> {
    with_native_state::<QueueState, _>(queue_obj, "Queue", |st| Ok(st.items.is_empty()))
}

pub(crate) fn queue_is_full(queue_obj: &Value) -> Result<bool> {
    with_native_state::<QueueState, _>(queue_obj, "Queue", |st| {
        if st.maxsize <= 0 {
            return Ok(false);
        }
        Ok((st.items.len() as i64) >= st.maxsize)
    })
}

pub(crate) fn queue_wake_next_getter(interp: &mut Interpreter, queue_obj: &Value) -> Result<()> {
    loop {
        let next = with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
            Ok(st.getters.pop_front())
        })?;
        let Some(w) = next else {
            break;
        };
        if !is_fut_done(interp, &w)? {
            future_set_result_if_pending(interp, &w, Value::none())?;
            break;
        }
    }
    Ok(())
}

pub(crate) fn queue_wake_next_putter(interp: &mut Interpreter, queue_obj: &Value) -> Result<()> {
    loop {
        let next = with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
            Ok(st.putters.pop_front())
        })?;
        let Some(w) = next else {
            break;
        };
        if !is_fut_done(interp, &w)? {
            future_set_result_if_pending(interp, &w, Value::none())?;
            break;
        }
    }
    Ok(())
}

pub(crate) fn queue_put_nowait(
    interp: &mut Interpreter,
    queue_obj: &Value,
    item: Value,
) -> Result<()> {
    if queue_is_full(queue_obj)? {
        return Err(raise_asyncio_exc(interp, "QueueFull", ""));
    }
    let kind = with_native_state::<QueueState, _>(queue_obj, "Queue", |st| Ok(st.kind))?;
    match kind {
        QueueKind::Fifo | QueueKind::Lifo => {
            with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
                st.items.push_back(item);
                Ok(())
            })?;
        }
        QueueKind::Priority => {
            // Insert maintaining min-heap / sorted order using Python `<` comparison.
            let items_snap = with_native_state::<QueueState, _>(queue_obj, "Queue", |st| {
                Ok(st.items.iter().cloned().collect::<Vec<_>>())
            })?;
            let op_mod = interp.load_module("operator")?;
            let lt_fn = interp.get_attr(&op_mod, "lt")?;
            let mut insert_pos = items_snap.len();
            for (i, existing) in items_snap.iter().enumerate() {
                let is_lt = call_pos(interp, lt_fn.clone(), &[item.clone(), existing.clone()])?;
                if interp.truthy_value(&is_lt)? {
                    insert_pos = i;
                    break;
                }
            }
            with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
                let idx = insert_pos.min(st.items.len());
                st.items.insert(idx, item);
                Ok(())
            })?;
        }
    }
    let finished_ev = with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
        st.unfinished_tasks += 1;
        Ok(st.finished.clone())
    })?;
    let _ = call_method(interp, &finished_ev, "clear", &[]);
    queue_wake_next_getter(interp, queue_obj)
}

pub(crate) fn queue_get_nowait(interp: &mut Interpreter, queue_obj: &Value) -> Result<Value> {
    if queue_is_empty(queue_obj)? {
        return Err(raise_asyncio_exc(interp, "QueueEmpty", ""));
    }
    let item = with_native_state_mut::<QueueState, _>(queue_obj, "Queue", |st| {
        let popped = match st.kind {
            QueueKind::Fifo | QueueKind::Priority => st.items.pop_front(),
            QueueKind::Lifo => st.items.pop_back(),
        };
        popped.ok_or_else(|| PyError::named("QueueEmpty", String::new()))
    })?;
    queue_wake_next_putter(interp, queue_obj)?;
    Ok(item)
}

// ── StreamReader / StreamWriter / StreamReaderProtocol helpers ───────────────

pub(crate) fn create_stream_reader(
    interp: &mut Interpreter,
    limit: usize,
    loop_val: Option<Value>,
) -> Result<Value> {
    let inst = alloc_asyncio_instance(interp, "StreamReader")?;
    set_native_state(
        &inst,
        StreamReaderState {
            limit,
            loop_val,
            buffer: Vec::new(),
            eof: false,
            waiter: None,
            exception: None,
            transport: None,
            paused: false,
        },
    )?;
    Ok(inst)
}

pub(crate) fn create_stream_reader_protocol(
    interp: &mut Interpreter,
    stream_reader: Value,
    client_connected_cb: Option<Value>,
    loop_val: Value,
) -> Result<Value> {
    let inst = alloc_asyncio_instance(interp, "StreamReaderProtocol")?;
    set_native_state(
        &inst,
        StreamReaderProtocolState {
            stream_reader: Some(stream_reader),
            client_connected_cb,
            loop_val,
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
    Ok(inst)
}

pub(crate) fn create_stream_writer(
    interp: &mut Interpreter,
    transport: Value,
    protocol: Value,
    reader: Option<Value>,
    loop_val: Value,
) -> Result<Value> {
    let inst = alloc_asyncio_instance(interp, "StreamWriter")?;
    set_native_state(
        &inst,
        StreamWriterState {
            transport,
            protocol,
            reader,
            loop_val,
        },
    )?;
    Ok(inst)
}

pub(crate) fn stream_reader_exception(reader: &Value) -> Result<Option<Value>> {
    with_native_state::<StreamReaderState, _>(reader, "StreamReader", |st| Ok(st.exception.clone()))
}

pub(crate) fn stream_reader_wakeup_waiter(interp: &mut Interpreter, reader: &Value) -> Result<()> {
    let w = with_native_state_mut::<StreamReaderState, _>(reader, "StreamReader", |st| {
        Ok(st.waiter.take())
    })?;
    if let Some(fut) = w {
        future_set_result_if_pending(interp, &fut, Value::none())?;
    }
    Ok(())
}

fn stream_reader_clear_waiter(reader: &Value, w: &Value) {
    let _ = with_native_state_mut::<StreamReaderState, _>(reader, "StreamReader", |st| {
        if let Some(ref cur) = st.waiter
            && same_object(cur, w)
        {
            st.waiter = None;
        }
        Ok(())
    });
}

fn stream_reader_wait_for_data(interp: &mut Interpreter, reader: &Value) -> Result<Value> {
    let (has_waiter, loop_opt, paused, transport) =
        with_native_state::<StreamReaderState, _>(reader, "StreamReader", |st| {
            Ok((
                st.waiter.is_some(),
                st.loop_val.clone(),
                st.paused,
                st.transport.clone(),
            ))
        })?;
    if has_waiter {
        return Err(PyError::named(
            "RuntimeError",
            "read() called while another coroutine is already waiting for incoming data",
        ));
    }
    if paused {
        with_native_state_mut::<StreamReaderState, _>(reader, "StreamReader", |st| {
            st.paused = false;
            Ok(())
        })?;
        if let Some(t) = transport {
            let _ = call_method(interp, &t, "resume_reading", &[]);
        }
    }
    let loop_val = match loop_opt {
        Some(l) => l,
        None => get_running_loop_or_err()?,
    };
    let w = loop_create_future(interp, &loop_val)?;
    with_native_state_mut::<StreamReaderState, _>(reader, "StreamReader", |st| {
        st.loop_val = Some(loop_val);
        st.waiter = Some(w.clone());
        Ok(())
    })?;
    Ok(w)
}

pub(crate) fn stream_reader_maybe_resume_transport(
    interp: &mut Interpreter,
    reader: &Value,
) -> Result<()> {
    let to_resume = with_native_state_mut::<StreamReaderState, _>(reader, "StreamReader", |st| {
        if st.paused && st.buffer.len() <= st.limit {
            st.paused = false;
            return Ok(st.transport.clone());
        }
        Ok(None)
    })?;
    if let Some(t) = to_resume {
        let _ = call_method(interp, &t, "resume_reading", &[]);
    }
    Ok(())
}

fn stream_reader_take_all(interp: &mut Interpreter, reader: &Value) -> Result<Vec<u8>> {
    let data = with_native_state_mut::<StreamReaderState, _>(reader, "StreamReader", |st| {
        Ok(std::mem::take(&mut st.buffer))
    })?;
    stream_reader_maybe_resume_transport(interp, reader)?;
    Ok(data)
}

fn stream_reader_take_up_to(interp: &mut Interpreter, reader: &Value, n: usize) -> Result<Vec<u8>> {
    let data = with_native_state_mut::<StreamReaderState, _>(reader, "StreamReader", |st| {
        let take = n.min(st.buffer.len());
        Ok(st.buffer.drain(..take).collect())
    })?;
    stream_reader_maybe_resume_transport(interp, reader)?;
    Ok(data)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn is_blocking_io_error(interp: &mut Interpreter, exc: &Value) -> bool {
    lookup_exc_class("BlockingIOError")
        .map(|c| super::is_isinstance(interp, exc, &Value::py_class(c)))
        .unwrap_or(false)
}

fn is_in_progress_errno(code: i64) -> bool {
    #[cfg(windows)]
    {
        // WSAEWOULDBLOCK (10035), WSAEINPROGRESS (10036), WSAEALREADY (10037)
        matches!(code, 10035..=10037)
    }
    #[cfg(not(windows))]
    {
        code == libc::EINPROGRESS as i64
            || code == libc::EWOULDBLOCK as i64
            || code == libc::EAGAIN as i64
            || code == libc::EALREADY as i64
    }
}
