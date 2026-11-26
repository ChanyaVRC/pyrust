//! Event loop, I/O transports, TLS wrapper, TCP/UDP servers, and subprocess
//! engines for PyRust's native `asyncio` (issue #3254).

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use indexmap::IndexMap;

use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state, with_native_state_mut,
};
use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, lookup_exc_class, render_value_repr};
use crate::value::{PyKey, Value, ValueKind};

use super::{
    DEVNULL_SENTINEL, FutureState, PIPE_SENTINEL, STDOUT_SENTINEL, alloc_asyncio_instance, call_kw,
    call_method, call_method_kw, call_pos, copy_context, future_add_done_callback, future_cancel,
    future_result, future_set_exception_if_pending, future_set_result_if_pending,
    get_running_loop_opt, init_task_instance, loop_call_soon, make_py_dict, monotonic_secs,
    pyerror_to_exc_value, run_in_context, set_running_loop_opt, value_id,
};

// ── Handle & TimerHandle state ───────────────────────────────────────────────

pub(crate) struct HandleState {
    pub(crate) callback: Value,
    pub(crate) args: Vec<Value>,
    pub(crate) loop_val: Value,
    pub(crate) context: Value,
    pub(crate) cancelled: bool,
    pub(crate) when: Option<f64>,
}

pub(crate) fn create_handle(
    interp: &mut Interpreter,
    loop_val: Value,
    callback: Value,
    args: Vec<Value>,
    context: Option<Value>,
    when: Option<f64>,
) -> Result<Value> {
    let ctx = match context {
        Some(c) if !c.is_none() => c,
        _ => copy_context(interp)?,
    };
    let cls_name = if when.is_some() {
        "TimerHandle"
    } else {
        "Handle"
    };
    let inst = alloc_asyncio_instance(interp, cls_name)?;
    set_native_state(
        &inst,
        HandleState {
            callback,
            args,
            loop_val,
            context: ctx,
            cancelled: false,
            when,
        },
    )?;
    Ok(inst)
}

pub(crate) fn handle_run(interp: &mut Interpreter, handle: &Value) -> Result<()> {
    let (cancelled, cb, args, ctx, loop_val) =
        with_native_state::<HandleState, _>(handle, "Handle", |st| {
            Ok((
                st.cancelled,
                st.callback.clone(),
                st.args.clone(),
                st.context.clone(),
                st.loop_val.clone(),
            ))
        })?;
    if cancelled {
        return Ok(());
    }
    if let Err(err) = run_in_context(interp, &ctx, &cb, &args) {
        let exc_val = pyerror_to_exc_value(interp, err)?;
        if super::is_base_fatal_error_val(interp, &exc_val) {
            return Err(PyError::raised(exc_val));
        }
        let handle_repr = render_value_repr(interp, handle).unwrap_or_else(|_| "<Handle>".into());
        let context_dict = make_py_dict(&[
            (
                "message",
                Value::string(format!("Exception in callback {handle_repr}")),
            ),
            ("handle", handle.clone()),
            ("exception", exc_val),
        ]);
        let _ = call_method(interp, &loop_val, "call_exception_handler", &[context_dict]);
    }
    Ok(())
}

// ── Fd watcher & Event Loop state ────────────────────────────────────────────

pub(crate) struct FdWatcher {
    pub(crate) callback: Value,
    pub(crate) args: Vec<Value>,
    pub(crate) context: Value,
}

pub(crate) struct LoopState {
    pub(crate) ready: VecDeque<Value>,
    pub(crate) scheduled: Vec<(f64, u64, Value)>, // sorted by (when, seq)
    pub(crate) timer_seq: u64,
    pub(crate) readers: IndexMap<i64, FdWatcher>,
    pub(crate) writers: IndexMap<i64, FdWatcher>,
    pub(crate) active_subprocesses: Vec<Value>,
    pub(crate) stopping: bool,
    pub(crate) closed: bool,
    pub(crate) debug: bool,
    pub(crate) task_factory: Option<Value>,
    pub(crate) exception_handler: Option<Value>,
    pub(crate) default_executor: Option<Value>,
    pub(crate) slow_callback_duration: f64,
}

impl LoopState {
    pub(crate) fn new() -> Self {
        Self {
            ready: VecDeque::new(),
            scheduled: Vec::new(),
            timer_seq: 0,
            readers: IndexMap::new(),
            writers: IndexMap::new(),
            active_subprocesses: Vec::new(),
            stopping: false,
            closed: false,
            debug: false,
            task_factory: None,
            exception_handler: None,
            default_executor: None,
            slow_callback_duration: 0.1,
        }
    }
}

pub(crate) fn check_loop_open(loop_val: &Value) -> Result<()> {
    let closed = with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| Ok(st.closed))?;
    if closed {
        return Err(PyError::named("RuntimeError", "Event loop is closed"));
    }
    Ok(())
}

pub(crate) fn loop_call_soon_native(
    interp: &mut Interpreter,
    loop_val: &Value,
    callback: Value,
    args: Vec<Value>,
    context: Option<Value>,
) -> Result<Value> {
    check_loop_open(loop_val)?;
    let handle = create_handle(interp, loop_val.clone(), callback, args, context, None)?;
    with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
        st.ready.push_back(handle.clone());
        Ok(())
    })?;
    Ok(handle)
}

pub(crate) fn loop_call_at_native(
    interp: &mut Interpreter,
    loop_val: &Value,
    when: f64,
    callback: Value,
    args: Vec<Value>,
    context: Option<Value>,
) -> Result<Value> {
    check_loop_open(loop_val)?;
    let handle = create_handle(
        interp,
        loop_val.clone(),
        callback,
        args,
        context,
        Some(when),
    )?;
    with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
        st.timer_seq += 1;
        let seq = st.timer_seq;
        // Stable insertion keeping `scheduled` sorted by `(when, seq)`.
        let pos = st
            .scheduled
            .iter()
            .position(|(w, _, _)| *w > when)
            .unwrap_or(st.scheduled.len());
        st.scheduled.insert(pos, (when, seq, handle.clone()));
        Ok(())
    })?;
    Ok(handle)
}

pub(crate) fn loop_create_task_native(
    interp: &mut Interpreter,
    loop_val: &Value,
    coro: Value,
    name: Option<String>,
    context: Option<Value>,
) -> Result<Value> {
    check_loop_open(loop_val)?;
    let factory =
        with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| Ok(st.task_factory.clone()))?;
    if let Some(tf) = factory {
        let mut kw: Vec<(&str, Value)> = Vec::new();
        if let Some(ref c) = context
            && !c.is_none()
        {
            kw.push(("context", c.clone()));
        }
        let task = if kw.is_empty() {
            call_pos(interp, tf, &[loop_val.clone(), coro])?
        } else {
            call_kw(interp, tf, &[loop_val.clone(), coro], &kw)?
        };
        if let Some(n) = name {
            let _ = call_method(interp, &task, "set_name", &[Value::string(n)]);
        }
        return Ok(task);
    }
    let task = alloc_asyncio_instance(interp, "Task")?;
    init_task_instance(interp, &task, coro, loop_val.clone(), name, context, false)?;
    Ok(task)
}

pub(crate) fn loop_add_reader(
    interp: &mut Interpreter,
    loop_val: &Value,
    fd: i64,
    callback: Value,
    args: Vec<Value>,
) -> Result<()> {
    check_loop_open(loop_val)?;
    let ctx = copy_context(interp)?;
    with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
        st.readers.insert(
            fd,
            FdWatcher {
                callback,
                args,
                context: ctx,
            },
        );
        Ok(())
    })
}

pub(crate) fn loop_remove_reader(loop_val: &Value, fd: i64) -> Result<bool> {
    with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
        if st.closed {
            return Ok(false);
        }
        Ok(st.readers.shift_remove(&fd).is_some())
    })
}

pub(crate) fn loop_add_writer(
    interp: &mut Interpreter,
    loop_val: &Value,
    fd: i64,
    callback: Value,
    args: Vec<Value>,
) -> Result<()> {
    check_loop_open(loop_val)?;
    let ctx = copy_context(interp)?;
    with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
        st.writers.insert(
            fd,
            FdWatcher {
                callback,
                args,
                context: ctx,
            },
        );
        Ok(())
    })
}

pub(crate) fn loop_remove_writer(loop_val: &Value, fd: i64) -> Result<bool> {
    with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
        if st.closed {
            return Ok(false);
        }
        Ok(st.writers.shift_remove(&fd).is_some())
    })
}

/// Discard cancelled timers from the front of the timer list.
fn discard_cancelled_timers(loop_val: &Value) -> Result<()> {
    loop {
        let front = with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| {
            Ok(st.scheduled.first().map(|(_, _, h)| h.clone()))
        })?;
        let Some(h) = front else {
            break;
        };
        let is_cancelled = with_native_state::<HandleState, _>(&h, "Handle", |hs| Ok(hs.cancelled))
            .unwrap_or(false);
        if is_cancelled {
            let _ = with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
                if !st.scheduled.is_empty() {
                    st.scheduled.remove(0);
                }
                Ok(())
            });
        } else {
            break;
        }
    }
    Ok(())
}

/// Execute one iteration of the `SelectorEventLoop` (timers + socket select +
/// subprocess pipe/exit polling + ready callbacks).
pub(crate) fn loop_run_once(interp: &mut Interpreter, loop_val: &Value) -> Result<()> {
    discard_cancelled_timers(loop_val)?;

    // Poll active subprocesses first (non-blocking) so ready data immediately
    // schedules callbacks without waiting.
    poll_all_subprocesses(interp, loop_val)?;

    let (has_ready, next_when, read_fds, write_fds, has_subprocs, stopping) =
        with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| {
            let rfds: Vec<i64> = st.readers.keys().copied().collect();
            let wfds: Vec<i64> = st.writers.keys().copied().collect();
            Ok((
                !st.ready.is_empty(),
                st.scheduled.first().map(|(w, _, _)| *w),
                rfds,
                wfds,
                !st.active_subprocesses.is_empty(),
                st.stopping,
            ))
        })?;

    let mut timeout = if has_ready || stopping {
        Some(Duration::ZERO)
    } else if let Some(when) = next_when {
        let now = monotonic_secs();
        if when > now {
            Some(Duration::from_secs_f64(when - now))
        } else {
            Some(Duration::ZERO)
        }
    } else if !read_fds.is_empty() || !write_fds.is_empty() {
        None
    } else if has_subprocs {
        Some(Duration::from_millis(2))
    } else {
        Some(Duration::ZERO)
    };

    // If subprocesses are active alongside sockets or timers, bound wait to 2ms
    // so pipe/exit events from background threads are dispatched promptly.
    if has_subprocs {
        let cap = Duration::from_millis(2);
        timeout = Some(match timeout {
            Some(d) => d.min(cap),
            None => cap,
        });
    }

    // Perform OS readiness poll (or sleep if no sockets are registered).
    if !read_fds.is_empty() || !write_fds.is_empty() {
        match crate::builtin_modules::socket::select_raw_handles(&read_fds, &write_fds, timeout) {
            Ok((ready_r, ready_w)) => {
                // Collect watchers for ready FDs and enqueue them into `ready`.
                let mut to_schedule: Vec<(Value, Vec<Value>, Value)> = Vec::new();
                with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| {
                    for fd in &ready_r {
                        if let Some(w) = st.readers.get(fd) {
                            to_schedule.push((
                                w.callback.clone(),
                                w.args.clone(),
                                w.context.clone(),
                            ));
                        }
                    }
                    for fd in &ready_w {
                        if let Some(w) = st.writers.get(fd) {
                            to_schedule.push((
                                w.callback.clone(),
                                w.args.clone(),
                                w.context.clone(),
                            ));
                        }
                    }
                    Ok(())
                })?;
                for (cb, args, ctx) in to_schedule {
                    let h = create_handle(interp, loop_val.clone(), cb, args, Some(ctx), None)?;
                    with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
                        st.ready.push_back(h);
                        Ok(())
                    })?;
                }
            }
            Err(_) => {
                // If an FD became invalid/closed concurrently, prune closed FDs
                // or yield briefly without crashing the loop.
                if let Some(d) = timeout
                    && !d.is_zero()
                {
                    thread::sleep(d.min(Duration::from_millis(5)));
                }
            }
        }
    } else if let Some(d) = timeout
        && !d.is_zero()
    {
        thread::sleep(d);
    }

    // Re-poll subprocesses after waiting.
    if has_subprocs {
        poll_all_subprocesses(interp, loop_val)?;
    }

    // Move all due live timers into the ready queue.
    let now = monotonic_secs();
    loop {
        let due = with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
            if let Some((when, _, _)) = st.scheduled.first()
                && *when <= now
            {
                let (_, _, h) = st.scheduled.remove(0);
                return Ok(Some(h));
            }
            Ok(None)
        })?;
        let Some(h) = due else {
            break;
        };
        let is_cancelled = with_native_state::<HandleState, _>(&h, "Handle", |hs| Ok(hs.cancelled))
            .unwrap_or(false);
        if !is_cancelled {
            with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
                st.ready.push_back(h);
                Ok(())
            })?;
        }
    }

    // Drain exactly the handles ready at the start of this turn (handles
    // appended during this turn execute on the next turn — CPython semantics).
    let ntodo = with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| Ok(st.ready.len()))?;
    for _ in 0..ntodo {
        let next = with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
            Ok(st.ready.pop_front())
        })?;
        let Some(handle) = next else {
            break;
        };
        handle_run(interp, &handle)?;
    }
    Ok(())
}

pub(crate) fn loop_run_forever(interp: &mut Interpreter, loop_val: &Value) -> Result<()> {
    check_loop_open(loop_val)?;
    if get_running_loop_opt().is_some() {
        return Err(PyError::named(
            "RuntimeError",
            "This event loop is already running",
        ));
    }
    set_running_loop_opt(Some(loop_val.clone()));
    let result = (|| -> Result<()> {
        loop {
            loop_run_once(interp, loop_val)?;
            let stop =
                with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| Ok(st.stopping))?;
            if stop {
                break;
            }
        }
        Ok(())
    })();
    let _ = with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
        st.stopping = false;
        Ok(())
    });
    set_running_loop_opt(None);
    result
}

pub(crate) fn loop_run_until_complete(
    interp: &mut Interpreter,
    loop_val: &Value,
    future_or_coro: Value,
) -> Result<Value> {
    check_loop_open(loop_val)?;
    if get_running_loop_opt().is_some() {
        return Err(PyError::named(
            "RuntimeError",
            "This event loop is already running",
        ));
    }
    let new_task = !super::is_future_obj(interp, &future_or_coro);
    let fut = super::ensure_future_impl(interp, future_or_coro, Some(loop_val.clone()))?;
    let stop_cb = interp.get_attr(loop_val, "_stop_on_done")?;
    if has_native_state::<FutureState>(&fut) {
        future_add_done_callback(interp, &fut, stop_cb, None)?;
    } else {
        call_method(interp, &fut, "add_done_callback", &[stop_cb])?;
    }

    set_running_loop_opt(Some(loop_val.clone()));
    let run_res = (|| -> Result<()> {
        loop {
            discard_cancelled_timers(loop_val)?;
            let idle = with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| {
                Ok(st.ready.is_empty()
                    && st.scheduled.is_empty()
                    && st.readers.is_empty()
                    && st.writers.is_empty()
                    && st.active_subprocesses.is_empty())
            })?;
            if idle {
                break;
            }
            loop_run_once(interp, loop_val)?;
            let stop =
                with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| Ok(st.stopping))?;
            if stop {
                break;
            }
        }
        Ok(())
    })();
    let _ = with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
        st.stopping = false;
        Ok(())
    });
    set_running_loop_opt(None);

    if let Err(e) = run_res {
        if new_task {
            let _ = future_cancel(interp, &fut, None);
        }
        return Err(e);
    }

    let done = if has_native_state::<FutureState>(&fut) {
        with_native_state::<FutureState, _>(&fut, "Future", |st| Ok(st.is_done()))?
    } else {
        let d = call_method(interp, &fut, "done", &[])?;
        interp.truthy_value(&d)?
    };
    if !done {
        return Err(PyError::named(
            "RuntimeError",
            "Event loop stopped before Future completed.",
        ));
    }
    if has_native_state::<FutureState>(&fut) {
        future_result(interp, &fut)
    } else {
        call_method(interp, &fut, "result", &[])
    }
}

// ── TLS Layer (built on PyRust's pure-Rust `ssl` MemoryBIO + SSLObject) ──────

pub(crate) struct TlsLayer {
    pub(crate) ssl_context: Value,
    pub(crate) incoming_bio: Value,
    pub(crate) outgoing_bio: Value,
    pub(crate) ssl_object: Value,
    pub(crate) handshake_done: bool,
    pub(crate) handshake_waiter: Option<Value>,
    pub(crate) pending_plaintext_writes: Vec<Vec<u8>>,
    pub(crate) call_connection_made_on_handshake: bool,
}

impl TlsLayer {
    pub(crate) fn new(
        interp: &mut Interpreter,
        ssl_context: Value,
        server_side: bool,
        server_hostname: Option<String>,
        handshake_waiter: Option<Value>,
    ) -> Result<Self> {
        Self::new_with_mode(
            interp,
            ssl_context,
            server_side,
            server_hostname,
            handshake_waiter,
            true,
        )
    }

    pub(crate) fn new_with_mode(
        interp: &mut Interpreter,
        ssl_context: Value,
        server_side: bool,
        server_hostname: Option<String>,
        handshake_waiter: Option<Value>,
        call_connection_made_on_handshake: bool,
    ) -> Result<Self> {
        let ssl_mod = interp.load_module("ssl")?;
        let bio_cls = interp.get_attr(&ssl_mod, "MemoryBIO")?;
        let incoming_bio = call_pos(interp, bio_cls.clone(), &[])?;
        let outgoing_bio = call_pos(interp, bio_cls, &[])?;

        let mut kw: Vec<(&str, Value)> = vec![("server_side", Value::bool_(server_side))];
        if !server_side && let Some(h) = server_hostname {
            kw.push(("server_hostname", Value::string(h)));
        }
        let ssl_object = call_method_kw(
            interp,
            &ssl_context,
            "wrap_bio",
            &[incoming_bio.clone(), outgoing_bio.clone()],
            &kw,
        )?;
        Ok(Self {
            ssl_context,
            incoming_bio,
            outgoing_bio,
            ssl_object,
            handshake_done: false,
            handshake_waiter,
            pending_plaintext_writes: Vec::new(),
            call_connection_made_on_handshake,
        })
    }
}

fn is_ssl_want_read_or_write(interp: &mut Interpreter, exc: &Value) -> bool {
    let Ok(ssl_mod) = interp.load_module("ssl") else {
        return false;
    };
    if let Ok(wr) = interp.get_attr(&ssl_mod, "SSLWantReadError")
        && super::is_isinstance(interp, exc, &wr)
    {
        return true;
    }
    if let Ok(ww) = interp.get_attr(&ssl_mod, "SSLWantWriteError")
        && super::is_isinstance(interp, exc, &ww)
    {
        return true;
    }
    false
}

fn is_ssl_zero_return_or_eof(interp: &mut Interpreter, exc: &Value) -> bool {
    let Ok(ssl_mod) = interp.load_module("ssl") else {
        return false;
    };
    if let Ok(zr) = interp.get_attr(&ssl_mod, "SSLZeroReturnError")
        && super::is_isinstance(interp, exc, &zr)
    {
        return true;
    }
    if let Ok(ef) = interp.get_attr(&ssl_mod, "SSLEOFError")
        && super::is_isinstance(interp, exc, &ef)
    {
        return true;
    }
    false
}

/// Resolve the `ssl=` argument into an `ssl.SSLContext` if enabled.
pub(crate) fn resolve_ssl_context(
    interp: &mut Interpreter,
    ssl_arg: Option<&Value>,
    server_side: bool,
) -> Result<Option<Value>> {
    let Some(val) = ssl_arg else {
        return Ok(None);
    };
    if val.is_none() {
        return Ok(None);
    }
    if let ValueKind::Bool(b) = val.kind() {
        if !b {
            return Ok(None);
        }
        if server_side {
            return Err(PyError::named(
                "TypeError",
                "ssl=True is not supported for servers; pass an SSLContext",
            ));
        }
        let ssl_mod = interp.load_module("ssl")?;
        let fn_val = interp.get_attr(&ssl_mod, "create_default_context")?;
        return Ok(Some(call_pos(interp, fn_val, &[])?));
    }
    Ok(Some(val.clone()))
}

// ── Socket Transport (_SelectorSocketTransport) ──────────────────────────────

pub(crate) struct SocketTransportState {
    pub(crate) loop_val: Value,
    pub(crate) sock: Value,
    pub(crate) fd: i64,
    pub(crate) protocol: Value,
    pub(crate) write_buf: Vec<u8>,
    pub(crate) high_water: usize,
    pub(crate) low_water: usize,
    pub(crate) protocol_paused: bool,
    pub(crate) reading_paused: bool,
    pub(crate) closing: bool,
    pub(crate) closed: bool,
    pub(crate) eof_written: bool,
    pub(crate) conn_lost_called: bool,
    pub(crate) extra: Vec<(String, Value)>,
    pub(crate) tls: Option<TlsLayer>,
    pub(crate) waiter: Option<Value>,
}

impl SocketTransportState {
    pub(crate) fn set_write_buffer_limits(
        &mut self,
        high: Option<usize>,
        low: Option<usize>,
    ) -> Result<()> {
        let h = high.unwrap_or_else(|| low.map(|l| l * 4).unwrap_or(65_536));
        let l = low.unwrap_or(h / 4);
        if h < l {
            return Err(PyError::named(
                "ValueError",
                format!("high ({h}) must be >= low ({l})"),
            ));
        }
        self.high_water = h;
        self.low_water = l;
        Ok(())
    }
}

pub(crate) fn create_socket_transport(
    interp: &mut Interpreter,
    loop_val: Value,
    sock: Value,
    protocol: Value,
    tls: Option<TlsLayer>,
    waiter: Option<Value>,
) -> Result<Value> {
    let _ = call_method(interp, &sock, "setblocking", &[Value::bool_(false)])?;
    let fd = super::extract_fd(interp, &sock)?;
    let sockname = call_method(interp, &sock, "getsockname", &[]).ok();
    let peername = call_method(interp, &sock, "getpeername", &[]).ok();

    let mut extra = Vec::new();
    extra.push(("socket".to_string(), sock.clone()));
    if let Some(sn) = sockname {
        extra.push(("sockname".to_string(), sn));
    }
    if let Some(pn) = peername {
        extra.push(("peername".to_string(), pn));
    }

    let inst = alloc_asyncio_instance(interp, "_SelectorSocketTransport")?;
    set_native_state(
        &inst,
        SocketTransportState {
            loop_val: loop_val.clone(),
            sock,
            fd,
            protocol: protocol.clone(),
            write_buf: Vec::new(),
            high_water: 65_536,
            low_water: 16_384,
            protocol_paused: false,
            reading_paused: false,
            closing: false,
            closed: false,
            eof_written: false,
            conn_lost_called: false,
            extra,
            tls,
            waiter,
        },
    )?;

    let has_tls = with_native_state::<SocketTransportState, _>(&inst, "Transport", |st| {
        Ok(st.tls.is_some())
    })?;

    // Register reader watcher on the loop.
    let on_read = interp.get_attr(&inst, "_read_ready")?;
    loop_add_reader(interp, &loop_val, fd, on_read, Vec::new())?;

    if has_tls {
        // Kick off TLS handshake immediately!
        transport_drive_tls(interp, &inst)?;
    } else {
        // Notify protocol.connection_made(transport) on next loop turn (or directly).
        let conn_made = interp.get_attr(&protocol, "connection_made")?;
        loop_call_soon(interp, &loop_val, conn_made, vec![inst.clone()], None)?;
        if let Some(w) =
            with_native_state_mut::<SocketTransportState, _>(&inst, "Transport", |st| {
                Ok(st.waiter.take())
            })?
        {
            let set_res = interp.get_attr(&inst, "_resolve_waiter")?;
            loop_call_soon(interp, &loop_val, set_res, vec![w], None)?;
        }
    }
    Ok(inst)
}

/// Drive non-blocking TLS handshake and buffered reads/writes on a socket transport.
pub(crate) fn transport_drive_tls(interp: &mut Interpreter, transport: &Value) -> Result<()> {
    let mut handshake_just_finished = false;
    let mut handshake_error: Option<Value> = None;
    let mut handshake_waiter: Option<Value> = None;
    let mut plaintext_chunks: Vec<Vec<u8>> = Vec::new();
    let mut saw_tls_eof = false;
    let mut pending_writes_to_encrypt: Vec<Vec<u8>> = Vec::new();

    // Step 1: check handshake state and run do_handshake() if needed.
    let tls_objs = with_native_state::<SocketTransportState, _>(transport, "Transport", |st| {
        let Some(ref tls) = st.tls else {
            return Ok(None);
        };
        Ok(Some((
            tls.ssl_object.clone(),
            tls.outgoing_bio.clone(),
            tls.handshake_done,
            st.protocol.clone(),
            st.loop_val.clone(),
        )))
    })?;

    let Some((ssl_obj, outgoing_bio, mut hs_done, proto, lv)) = tls_objs else {
        return Ok(());
    };
    let protocol = Some(proto);
    let loop_val = Some(lv);

    if !hs_done {
        match call_method(interp, &ssl_obj, "do_handshake", &[]) {
            Ok(_) => {
                hs_done = true;
                handshake_just_finished = true;
            }
            Err(err) => {
                let exc_val = pyerror_to_exc_value(interp, err)?;
                if !is_ssl_want_read_or_write(interp, &exc_val) {
                    handshake_error = Some(exc_val);
                }
            }
        }
        // Flush any handshake records from outgoing_bio into the socket write buffer!
        if let Ok(out_val) = call_method(interp, &outgoing_bio, "read", &[])
            && let ValueKind::Bytes(b) = out_val.kind()
            && !b.is_empty()
        {
            transport_queue_raw_bytes(interp, transport, b.as_slice())?;
        }
    }

    if let Some(exc_val) = handshake_error {
        let w = with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
            Ok(st.tls.as_mut().and_then(|t| t.handshake_waiter.take()))
        })?;
        if let Some(waiter) = w {
            future_set_exception_if_pending(interp, &waiter, exc_val.clone())?;
        }
        transport_force_close(interp, transport, Some(exc_val))?;
        return Ok(());
    }

    let is_handshake_done = if handshake_just_finished {
        with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
            if let Some(ref mut tls) = st.tls {
                tls.handshake_done = true;
                handshake_waiter = tls.handshake_waiter.take();
                pending_writes_to_encrypt = std::mem::take(&mut tls.pending_plaintext_writes);
            }
            Ok(())
        })?;
        true
    } else {
        hs_done
    };

    // Notify protocol & waiter when handshake completes.
    if handshake_just_finished {
        if let Some(ref p) = protocol {
            let should_call_cm =
                with_native_state::<SocketTransportState, _>(transport, "Transport", |st| {
                    Ok(!st.conn_lost_called
                        && st
                            .tls
                            .as_ref()
                            .is_some_and(|t| t.call_connection_made_on_handshake))
                })?;
            if should_call_cm {
                let _ = call_method(
                    interp,
                    p,
                    "connection_made",
                    std::slice::from_ref(transport),
                );
            }
        }
        if let Some(w) = handshake_waiter {
            future_set_result_if_pending(interp, &w, Value::none())?;
        }
    }

    // Encrypt any queued plaintext writes now that handshake is done.
    if is_handshake_done {
        for chunk in pending_writes_to_encrypt {
            if !chunk.is_empty() {
                let _ = call_method(interp, &ssl_obj, "write", &[Value::bytes(chunk)]);
                if let Ok(out_val) = call_method(interp, &outgoing_bio, "read", &[])
                    && let ValueKind::Bytes(b) = out_val.kind()
                    && !b.is_empty()
                {
                    transport_queue_raw_bytes(interp, transport, b.as_slice())?;
                }
            }
        }

        // Read all available decrypted plaintext.
        loop {
            match call_method(interp, &ssl_obj, "read", &[Value::int(65536)]) {
                Ok(data_val) => {
                    if let ValueKind::Bytes(b) = data_val.kind() {
                        if b.is_empty() {
                            saw_tls_eof = true;
                            break;
                        }
                        plaintext_chunks.push(b.as_slice().to_vec());
                    } else {
                        break;
                    }
                }
                Err(err) => {
                    let exc_val = pyerror_to_exc_value(interp, err)?;
                    if is_ssl_want_read_or_write(interp, &exc_val) {
                        break;
                    }
                    if is_ssl_zero_return_or_eof(interp, &exc_val) {
                        saw_tls_eof = true;
                        break;
                    }
                    transport_force_close(interp, transport, Some(exc_val))?;
                    return Ok(());
                }
            }
        }
        // Flush any TLS alerts/records produced during read.
        if let Ok(out_val) = call_method(interp, &outgoing_bio, "read", &[])
            && let ValueKind::Bytes(b) = out_val.kind()
            && !b.is_empty()
        {
            transport_queue_raw_bytes(interp, transport, b.as_slice())?;
        }
    }

    // Deliver decrypted plaintext chunks to protocol.
    if let Some(ref p) = protocol {
        for chunk in plaintext_chunks {
            let _ = deliver_data_to_protocol(interp, p, chunk);
        }
        if saw_tls_eof {
            let keep_open = call_method(interp, p, "eof_received", &[])
                .and_then(|v| interp.truthy_value(&v))
                .unwrap_or(false);
            if !keep_open {
                transport_close(interp, transport)?;
            }
        }
    }
    let _ = loop_val;
    Ok(())
}

/// Deliver received bytes to either a `Protocol` (`data_received`) or
/// `BufferedProtocol` (`get_buffer` + `buffer_updated`).
fn deliver_data_to_protocol(
    interp: &mut Interpreter,
    protocol: &Value,
    data: Vec<u8>,
) -> Result<()> {
    if let Ok(get_buf) = interp.get_attr(protocol, "get_buffer") {
        // Check if this is a BufferedProtocol (defines custom get_buffer).
        if let Ok(buf_proto_cls) = super::asyncio_attr(interp, "BufferedProtocol")
            && super::is_isinstance(interp, protocol, &buf_proto_cls)
        {
            let mut offset = 0usize;
            while offset < data.len() {
                let remaining = data.len() - offset;
                let buf = call_pos(interp, get_buf.clone(), &[Value::int(remaining as i64)])?;
                let (buf_rc, start, end) =
                    crate::builtin_modules::socket::extract_writable_buffer_pub(&buf)?;
                let avail = end.saturating_sub(start);
                let n = remaining.min(avail);
                if n == 0 {
                    break;
                }
                buf_rc.borrow_mut()[start..start + n].copy_from_slice(&data[offset..offset + n]);
                let _ = call_method(interp, protocol, "buffer_updated", &[Value::int(n as i64)])?;
                offset += n;
            }
            return Ok(());
        }
    }
    let _ = call_method(interp, protocol, "data_received", &[Value::bytes(data)])?;
    Ok(())
}

pub(crate) fn transport_queue_raw_bytes(
    interp: &mut Interpreter,
    transport: &Value,
    data: &[u8],
) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let (sock, fd, loop_val, had_buffered) =
        with_native_state::<SocketTransportState, _>(transport, "Transport", |st| {
            Ok((
                st.sock.clone(),
                st.fd,
                st.loop_val.clone(),
                !st.write_buf.is_empty(),
            ))
        })?;

    let mut remaining = data;
    if !had_buffered {
        // Try immediate non-blocking send!
        match call_method(interp, &sock, "send", &[Value::bytes(remaining.to_vec())]) {
            Ok(n_val) => {
                let sent = n_val.as_int().unwrap_or(0) as usize;
                remaining = &remaining[sent.min(remaining.len())..];
            }
            Err(err) => {
                let exc_val = pyerror_to_exc_value(interp, err)?;
                let is_wouldblock = lookup_exc_class("BlockingIOError")
                    .map(|c| super::is_isinstance(interp, &exc_val, &Value::py_class(c)))
                    .unwrap_or(false);
                if !is_wouldblock {
                    transport_force_close(interp, transport, Some(exc_val))?;
                    return Ok(());
                }
            }
        }
    }

    if !remaining.is_empty() {
        let should_pause =
            with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
                st.write_buf.extend_from_slice(remaining);
                if !st.protocol_paused && st.high_water > 0 && st.write_buf.len() > st.high_water {
                    st.protocol_paused = true;
                    return Ok(Some(st.protocol.clone()));
                }
                Ok(None)
            })?;
        let on_write = interp.get_attr(transport, "_write_ready")?;
        loop_add_writer(interp, &loop_val, fd, on_write, Vec::new())?;
        if let Some(proto) = should_pause {
            let _ = call_method(interp, &proto, "pause_writing", &[]);
        }
    }
    Ok(())
}

pub(crate) fn transport_write(
    interp: &mut Interpreter,
    transport: &Value,
    data: &[u8],
) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let (closing, has_tls, hs_done, ssl_info) =
        with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
            if st.closing {
                return Ok((true, false, false, None));
            }
            if let Some(ref mut tls) = st.tls {
                if !tls.handshake_done {
                    tls.pending_plaintext_writes.push(data.to_vec());
                    return Ok((false, true, false, None));
                }
                return Ok((
                    false,
                    true,
                    true,
                    Some((tls.ssl_object.clone(), tls.outgoing_bio.clone())),
                ));
            }
            Ok((false, false, false, None))
        })?;
    if closing {
        return Ok(());
    }
    if has_tls {
        if !hs_done {
            return Ok(());
        }
        if let Some((ssl_obj, outgoing_bio)) = ssl_info {
            let _ = call_method(interp, &ssl_obj, "write", &[Value::bytes(data.to_vec())])?;
            if let Ok(out_val) = call_method(interp, &outgoing_bio, "read", &[])
                && let ValueKind::Bytes(b) = out_val.kind()
                && !b.is_empty()
            {
                transport_queue_raw_bytes(interp, transport, b.as_slice())?;
            }
        }
        return Ok(());
    }
    transport_queue_raw_bytes(interp, transport, data)
}

pub(crate) fn transport_read_ready(interp: &mut Interpreter, transport: &Value) -> Result<()> {
    let (closed, paused, sock, protocol, has_tls, incoming_bio) =
        with_native_state::<SocketTransportState, _>(transport, "Transport", |st| {
            Ok((
                st.closed,
                st.reading_paused,
                st.sock.clone(),
                st.protocol.clone(),
                st.tls.is_some(),
                st.tls.as_ref().map(|t| t.incoming_bio.clone()),
            ))
        })?;
    if closed || paused {
        return Ok(());
    }

    match call_method(interp, &sock, "recv", &[Value::int(65536)]) {
        Ok(data_val) => {
            let bytes = match data_val.kind() {
                ValueKind::Bytes(b) => b.as_slice().to_vec(),
                _ => Vec::new(),
            };
            if has_tls {
                if let Some(bio) = incoming_bio {
                    if bytes.is_empty() {
                        let _ = call_method(interp, &bio, "write_eof", &[]);
                    } else {
                        let _ = call_method(interp, &bio, "write", &[Value::bytes(bytes)]);
                    }
                }
                transport_drive_tls(interp, transport)?;
                return Ok(());
            }
            if bytes.is_empty() {
                // Peer sent EOF.
                let (loop_val, fd) =
                    with_native_state::<SocketTransportState, _>(transport, "Transport", |st| {
                        Ok((st.loop_val.clone(), st.fd))
                    })?;
                let _ = loop_remove_reader(&loop_val, fd);
                let keep_open = call_method(interp, &protocol, "eof_received", &[])
                    .and_then(|v| interp.truthy_value(&v))
                    .unwrap_or(false);
                if !keep_open {
                    transport_close(interp, transport)?;
                }
            } else {
                deliver_data_to_protocol(interp, &protocol, bytes)?;
            }
        }
        Err(err) => {
            let exc_val = pyerror_to_exc_value(interp, err)?;
            let is_wouldblock = lookup_exc_class("BlockingIOError")
                .map(|c| super::is_isinstance(interp, &exc_val, &Value::py_class(c)))
                .unwrap_or(false);
            if !is_wouldblock {
                transport_force_close(interp, transport, Some(exc_val))?;
            }
        }
    }
    Ok(())
}

pub(crate) fn transport_write_ready(interp: &mut Interpreter, transport: &Value) -> Result<()> {
    let (closed, sock, fd, loop_val, buf) =
        with_native_state::<SocketTransportState, _>(transport, "Transport", |st| {
            Ok((
                st.closed,
                st.sock.clone(),
                st.fd,
                st.loop_val.clone(),
                st.write_buf.clone(),
            ))
        })?;
    if closed {
        return Ok(());
    }
    if buf.is_empty() {
        let _ = loop_remove_writer(&loop_val, fd);
        return Ok(());
    }
    match call_method(interp, &sock, "send", &[Value::bytes(buf)]) {
        Ok(n_val) => {
            let sent = n_val.as_int().unwrap_or(0) as usize;
            let (now_empty, should_resume, closing, eof_written) =
                with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
                    let take = sent.min(st.write_buf.len());
                    st.write_buf.drain(..take);
                    let empty = st.write_buf.is_empty();
                    let resume = if st.protocol_paused && st.write_buf.len() <= st.low_water {
                        st.protocol_paused = false;
                        Some(st.protocol.clone())
                    } else {
                        None
                    };
                    Ok((empty, resume, st.closing, st.eof_written))
                })?;
            if let Some(proto) = should_resume {
                let _ = call_method(interp, &proto, "resume_writing", &[]);
            }
            if now_empty {
                let _ = loop_remove_writer(&loop_val, fd);
                if closing {
                    transport_finish_close(interp, transport, None)?;
                } else if eof_written {
                    let _ = call_method(interp, &sock, "shutdown", &[Value::int(1)]); // SHUT_WR
                }
            }
        }
        Err(err) => {
            let exc_val = pyerror_to_exc_value(interp, err)?;
            let is_wouldblock = lookup_exc_class("BlockingIOError")
                .map(|c| super::is_isinstance(interp, &exc_val, &Value::py_class(c)))
                .unwrap_or(false);
            if !is_wouldblock {
                transport_force_close(interp, transport, Some(exc_val))?;
            }
        }
    }
    Ok(())
}

pub(crate) fn transport_write_eof(interp: &mut Interpreter, transport: &Value) -> Result<()> {
    let (closing, eof_written, has_tls, buf_empty, sock) =
        with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
            if st.closing || st.eof_written {
                return Ok((
                    st.closing,
                    true,
                    st.tls.is_some(),
                    st.write_buf.is_empty(),
                    st.sock.clone(),
                ));
            }
            st.eof_written = true;
            Ok((
                false,
                false,
                st.tls.is_some(),
                st.write_buf.is_empty(),
                st.sock.clone(),
            ))
        })?;
    if closing || eof_written {
        return Ok(());
    }
    if has_tls {
        return Ok(()); // TLS transports do not support half-close write_eof (can_write_eof() == False).
    }
    if buf_empty {
        let _ = call_method(interp, &sock, "shutdown", &[Value::int(1)]); // SHUT_WR
    }
    Ok(())
}

pub(crate) fn transport_close(interp: &mut Interpreter, transport: &Value) -> Result<()> {
    let (already_closing, buf_empty, loop_val, fd, tls_shutdown) =
        with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
            if st.closing {
                return Ok((
                    true,
                    st.write_buf.is_empty(),
                    st.loop_val.clone(),
                    st.fd,
                    None,
                ));
            }
            st.closing = true;
            let tls_obj = st
                .tls
                .as_ref()
                .filter(|t| t.handshake_done)
                .map(|t| (t.ssl_object.clone(), t.outgoing_bio.clone()));
            Ok((
                false,
                st.write_buf.is_empty(),
                st.loop_val.clone(),
                st.fd,
                tls_obj,
            ))
        })?;
    if already_closing {
        return Ok(());
    }
    let _ = loop_remove_reader(&loop_val, fd);
    if let Some((ssl_obj, outgoing_bio)) = tls_shutdown {
        let _ = call_method(interp, &ssl_obj, "unwrap", &[]);
        if let Ok(out_val) = call_method(interp, &outgoing_bio, "read", &[])
            && let ValueKind::Bytes(b) = out_val.kind()
            && !b.is_empty()
        {
            let _ = transport_queue_raw_bytes(interp, transport, b.as_slice());
        }
    }
    let now_empty = with_native_state::<SocketTransportState, _>(transport, "Transport", |st| {
        Ok(st.write_buf.is_empty())
    })?;
    if buf_empty && now_empty {
        let _ = loop_remove_writer(&loop_val, fd);
        let call_lost = interp.get_attr(transport, "_call_connection_lost")?;
        loop_call_soon(interp, &loop_val, call_lost, vec![Value::none()], None)?;
    }
    Ok(())
}

pub(crate) fn transport_force_close(
    interp: &mut Interpreter,
    transport: &Value,
    exc: Option<Value>,
) -> Result<()> {
    let (already_closed, loop_val, fd) =
        with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
            let was = st.closed && st.conn_lost_called;
            st.closing = true;
            st.write_buf.clear();
            Ok((was, st.loop_val.clone(), st.fd))
        })?;
    if already_closed {
        return Ok(());
    }
    let _ = loop_remove_reader(&loop_val, fd);
    let _ = loop_remove_writer(&loop_val, fd);
    let call_lost = interp.get_attr(transport, "_call_connection_lost")?;
    loop_call_soon(
        interp,
        &loop_val,
        call_lost,
        vec![exc.unwrap_or_else(Value::none)],
        None,
    )?;
    Ok(())
}

pub(crate) fn transport_finish_close(
    interp: &mut Interpreter,
    transport: &Value,
    exc: Option<Value>,
) -> Result<()> {
    let (should_call, sock, protocol) =
        with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
            if st.conn_lost_called {
                return Ok((false, st.sock.clone(), st.protocol.clone()));
            }
            st.closed = true;
            st.conn_lost_called = true;
            Ok((true, st.sock.clone(), st.protocol.clone()))
        })?;
    if !should_call {
        return Ok(());
    }
    let _ = call_method(interp, &sock, "close", &[]);
    let exc_arg = exc.unwrap_or_else(Value::none);
    let _ = call_method(interp, &protocol, "connection_lost", &[exc_arg]);
    Ok(())
}

pub(crate) fn transport_get_extra_info(
    interp: &mut Interpreter,
    transport: &Value,
    name: &str,
    default: Value,
) -> Result<Value> {
    let (extra, tls_info) =
        with_native_state::<SocketTransportState, _>(transport, "Transport", |st| {
            let found = st
                .extra
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone());
            let tls = st.tls.as_ref().map(|t| {
                (
                    t.ssl_object.clone(),
                    t.ssl_context.clone(),
                    t.handshake_done,
                )
            });
            Ok((found, tls))
        })?;
    if let Some(v) = extra {
        return Ok(v);
    }
    if let Some((ssl_obj, ssl_ctx, hs_done)) = tls_info {
        match name {
            "ssl_object" => return Ok(ssl_obj),
            "sslcontext" => return Ok(ssl_ctx),
            "peercert" => {
                if hs_done {
                    return call_method(interp, &ssl_obj, "getpeercert", &[]).or(Ok(default));
                }
                return Ok(Value::none());
            }
            "cipher" => {
                if hs_done {
                    return call_method(interp, &ssl_obj, "cipher", &[]).or(Ok(default));
                }
                return Ok(Value::none());
            }
            "compression" => {
                if hs_done {
                    return call_method(interp, &ssl_obj, "compression", &[]).or(Ok(default));
                }
                return Ok(Value::none());
            }
            _ => {}
        }
    }
    Ok(default)
}

// ── Datagram Transport (_SelectorDatagramTransport) ──────────────────────────

pub(crate) struct DatagramTransportState {
    pub(crate) loop_val: Value,
    pub(crate) sock: Value,
    pub(crate) fd: i64,
    pub(crate) protocol: Value,
    pub(crate) remote_addr: Option<Value>,
    pub(crate) closing: bool,
    pub(crate) closed: bool,
    pub(crate) extra: Vec<(String, Value)>,
}

pub(crate) fn create_datagram_transport(
    interp: &mut Interpreter,
    loop_val: Value,
    sock: Value,
    protocol: Value,
    remote_addr: Option<Value>,
) -> Result<Value> {
    let _ = call_method(interp, &sock, "setblocking", &[Value::bool_(false)])?;
    let fd = super::extract_fd(interp, &sock)?;
    let sockname = call_method(interp, &sock, "getsockname", &[]).ok();
    let mut extra = vec![("socket".to_string(), sock.clone())];
    if let Some(sn) = sockname {
        extra.push(("sockname".to_string(), sn));
    }
    if let Some(ref ra) = remote_addr {
        extra.push(("peername".to_string(), ra.clone()));
    }
    let inst = alloc_asyncio_instance(interp, "_SelectorDatagramTransport")?;
    set_native_state(
        &inst,
        DatagramTransportState {
            loop_val: loop_val.clone(),
            sock,
            fd,
            protocol: protocol.clone(),
            remote_addr,
            closing: false,
            closed: false,
            extra,
        },
    )?;
    let on_read = interp.get_attr(&inst, "_read_ready")?;
    loop_add_reader(interp, &loop_val, fd, on_read, Vec::new())?;
    let _ = call_method(
        interp,
        &protocol,
        "connection_made",
        std::slice::from_ref(&inst),
    )?;
    Ok(inst)
}

// ── TCP Server (asyncio.Server) ──────────────────────────────────────────────

pub(crate) struct ServerState {
    pub(crate) loop_val: Value,
    pub(crate) sockets: Vec<Value>,
    pub(crate) protocol_factory: Value,
    pub(crate) ssl_context: Option<Value>,
    pub(crate) backlog: i64,
    pub(crate) serving: bool,
    pub(crate) closed: bool,
    pub(crate) waiters: Vec<Value>,
}

pub(crate) fn server_start_serving(interp: &mut Interpreter, server: &Value) -> Result<()> {
    let (should_start, loop_val, sockets, backlog) =
        with_native_state_mut::<ServerState, _>(server, "Server", |st| {
            if st.serving || st.closed {
                return Ok((false, st.loop_val.clone(), Vec::new(), st.backlog));
            }
            st.serving = true;
            Ok((true, st.loop_val.clone(), st.sockets.clone(), st.backlog))
        })?;
    if !should_start {
        return Ok(());
    }
    for sock in sockets {
        let _ = call_method(interp, &sock, "listen", &[Value::int(backlog)]);
        let _ = call_method(interp, &sock, "setblocking", &[Value::bool_(false)]);
        let fd = super::extract_fd(interp, &sock)?;
        let on_accept = interp.get_attr(server, "_accept_connection")?;
        loop_add_reader(interp, &loop_val, fd, on_accept, vec![sock])?;
    }
    Ok(())
}

pub(crate) fn server_accept_connection(
    interp: &mut Interpreter,
    server: &Value,
    listen_sock: &Value,
) -> Result<()> {
    let (closed, loop_val, protocol_factory, ssl_ctx) =
        with_native_state::<ServerState, _>(server, "Server", |st| {
            Ok((
                st.closed,
                st.loop_val.clone(),
                st.protocol_factory.clone(),
                st.ssl_context.clone(),
            ))
        })?;
    if closed {
        return Ok(());
    }
    // Accept all pending connections on the non-blocking listening socket.
    for _ in 0..64 {
        match call_method(interp, listen_sock, "accept", &[]) {
            Ok(pair) => {
                let conn_sock = match pair.kind() {
                    ValueKind::Tuple(t) if !t.is_empty() => t[0].clone(),
                    _ => break,
                };
                let protocol = call_pos(interp, protocol_factory.clone(), &[])?;
                let tls = match ssl_ctx {
                    Some(ref ctx) => Some(TlsLayer::new(interp, ctx.clone(), true, None, None)?),
                    None => None,
                };
                let _ = create_socket_transport(
                    interp,
                    loop_val.clone(),
                    conn_sock,
                    protocol,
                    tls,
                    None,
                )?;
            }
            Err(err) => {
                let exc_val = pyerror_to_exc_value(interp, err)?;
                let is_wouldblock = lookup_exc_class("BlockingIOError")
                    .map(|c| super::is_isinstance(interp, &exc_val, &Value::py_class(c)))
                    .unwrap_or(false);
                if is_wouldblock {
                    break;
                }
                break;
            }
        }
    }
    Ok(())
}

pub(crate) fn server_close(interp: &mut Interpreter, server: &Value) -> Result<()> {
    let (already, loop_val, sockets, waiters) =
        with_native_state_mut::<ServerState, _>(server, "Server", |st| {
            if st.closed {
                return Ok((true, st.loop_val.clone(), Vec::new(), Vec::new()));
            }
            st.closed = true;
            st.serving = false;
            let socks = std::mem::take(&mut st.sockets);
            let ws = std::mem::take(&mut st.waiters);
            Ok((false, st.loop_val.clone(), socks, ws))
        })?;
    if already {
        return Ok(());
    }
    for sock in sockets {
        if let Ok(fd) = super::extract_fd(interp, &sock) {
            let _ = loop_remove_reader(&loop_val, fd);
        }
        let _ = call_method(interp, &sock, "close", &[]);
    }
    for w in waiters {
        future_set_result_if_pending(interp, &w, Value::none())?;
    }
    Ok(())
}

// ── Async Subprocess Transport (_SubprocessTransport) ────────────────────────

pub(crate) enum PipeMsg {
    Data(Vec<u8>),
    Eof,
}

pub(crate) struct SubprocessTransportState {
    pub(crate) protocol: Value,
    pub(crate) child: Child,
    pub(crate) pid: i64,
    pub(crate) returncode: Option<i64>,
    pub(crate) stdin_transport: Option<Value>,
    pub(crate) stdout_transport: Option<Value>,
    pub(crate) stderr_transport: Option<Value>,
    pub(crate) stdout_rx: Option<Receiver<PipeMsg>>,
    pub(crate) stderr_rx: Option<Receiver<PipeMsg>>,
    pub(crate) stdout_eof: bool,
    pub(crate) stderr_eof: bool,
    pub(crate) exit_notified: bool,
    pub(crate) conn_lost_called: bool,
    pub(crate) closed: bool,
}

pub(crate) struct PipeWriteTransportState {
    pub(crate) protocol: Value,
    pub(crate) fd: i64,
    pub(crate) stdin: Option<ChildStdin>,
    pub(crate) closing: bool,
    pub(crate) closed: bool,
}

pub(crate) struct PipeReadTransportState {
    pub(crate) closing: bool,
    pub(crate) closed: bool,
}

fn spawn_pipe_reader_thread<R: Read + Send + 'static>(mut reader: R) -> Receiver<PipeMsg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(PipeMsg::Eof);
                    break;
                }
                Ok(n) => {
                    if tx.send(PipeMsg::Data(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    let _ = tx.send(PipeMsg::Eof);
                    break;
                }
            }
        }
    });
    rx
}

fn stdio_from_py(val: Option<&Value>) -> Result<Stdio> {
    let Some(v) = val else {
        return Ok(Stdio::inherit());
    };
    if v.is_none() {
        return Ok(Stdio::inherit());
    }
    if let Some(n) = v.as_int() {
        return match n {
            PIPE_SENTINEL => Ok(Stdio::piped()),
            DEVNULL_SENTINEL => Ok(Stdio::null()),
            _ => Ok(Stdio::inherit()),
        };
    }
    Ok(Stdio::inherit())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_subprocess_transport(
    interp: &mut Interpreter,
    loop_val: Value,
    protocol: Value,
    program: String,
    args: Vec<String>,
    shell: bool,
    stdin_arg: Option<&Value>,
    stdout_arg: Option<&Value>,
    stderr_arg: Option<&Value>,
    cwd: Option<String>,
    env: Option<Vec<(String, String)>>,
) -> Result<Value> {
    let mut cmd = if shell {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
            let mut c = Command::new(comspec);
            c.raw_arg(format!("/c \"{program}\""));
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(&program);
            c
        }
    } else {
        let mut c = Command::new(&program);
        c.args(&args);
        c
    };

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    if let Some(pairs) = env {
        cmd.env_clear();
        for (k, v) in pairs {
            cmd.env(k, v);
        }
    }

    let merge_stderr_to_stdout = stderr_arg
        .and_then(|v| v.as_int())
        .is_some_and(|n| n == STDOUT_SENTINEL);

    let mut merged_reader: Option<std::io::PipeReader> = None;

    cmd.stdin(stdio_from_py(stdin_arg)?);
    if merge_stderr_to_stdout {
        let (pr, pw) = std::io::pipe().map_err(|e| PyError::from_io_error(&e, None))?;
        let pw2 = pw
            .try_clone()
            .map_err(|e| PyError::from_io_error(&e, None))?;
        cmd.stdout(Stdio::from(pw));
        cmd.stderr(Stdio::from(pw2));
        merged_reader = Some(pr);
    } else {
        cmd.stdout(stdio_from_py(stdout_arg)?);
        cmd.stderr(stdio_from_py(stderr_arg)?);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| PyError::from_io_error(&e, Some(&program)))?;
    let pid = child.id() as i64;

    let stdin_transport = if let Some(child_stdin) = child.stdin.take() {
        let t = alloc_asyncio_instance(interp, "_SubprocessWritePipeTransport")?;
        set_native_state(
            &t,
            PipeWriteTransportState {
                protocol: protocol.clone(),
                fd: 0,
                stdin: Some(child_stdin),
                closing: false,
                closed: false,
            },
        )?;
        Some(t)
    } else {
        None
    };

    let (stdout_transport, stdout_rx, stdout_eof) = if let Some(mr) = merged_reader {
        let t = alloc_asyncio_instance(interp, "_SubprocessReadPipeTransport")?;
        set_native_state(
            &t,
            PipeReadTransportState {
                closing: false,
                closed: false,
            },
        )?;
        (Some(t), Some(spawn_pipe_reader_thread(mr)), false)
    } else if let Some(child_stdout) = child.stdout.take() {
        let t = alloc_asyncio_instance(interp, "_SubprocessReadPipeTransport")?;
        set_native_state(
            &t,
            PipeReadTransportState {
                closing: false,
                closed: false,
            },
        )?;
        (Some(t), Some(spawn_pipe_reader_thread(child_stdout)), false)
    } else {
        (None, None, true)
    };

    let (stderr_transport, stderr_rx, stderr_eof) = if let Some(child_stderr) = child.stderr.take()
    {
        let t = alloc_asyncio_instance(interp, "_SubprocessReadPipeTransport")?;
        set_native_state(
            &t,
            PipeReadTransportState {
                closing: false,
                closed: false,
            },
        )?;
        (Some(t), Some(spawn_pipe_reader_thread(child_stderr)), false)
    } else {
        (None, None, true)
    };

    let transport = alloc_asyncio_instance(interp, "_SubprocessTransport")?;
    set_native_state(
        &transport,
        SubprocessTransportState {
            protocol: protocol.clone(),
            child,
            pid,
            returncode: None,
            stdin_transport,
            stdout_transport,
            stderr_transport,
            stdout_rx,
            stderr_rx,
            stdout_eof,
            stderr_eof,
            exit_notified: false,
            conn_lost_called: false,
            closed: false,
        },
    )?;

    with_native_state_mut::<LoopState, _>(&loop_val, "EventLoop", |st| {
        st.active_subprocesses.push(transport.clone());
        Ok(())
    })?;

    let _ = call_method(
        interp,
        &protocol,
        "connection_made",
        std::slice::from_ref(&transport),
    )?;
    Ok(transport)
}

fn poll_all_subprocesses(interp: &mut Interpreter, loop_val: &Value) -> Result<()> {
    let list = with_native_state::<LoopState, _>(loop_val, "EventLoop", |st| {
        Ok(st.active_subprocesses.clone())
    })?;
    if list.is_empty() {
        return Ok(());
    }
    let mut still_active = Vec::with_capacity(list.len());
    for sp in list {
        let done = poll_one_subprocess(interp, &sp)?;
        if !done {
            still_active.push(sp);
        }
    }
    with_native_state_mut::<LoopState, _>(loop_val, "EventLoop", |st| {
        // Keep any newly spawned subprocesses plus remaining active ones.
        let mut next = still_active;
        for existing in &st.active_subprocesses {
            let id = value_id(existing);
            let is_done = with_native_state::<SubprocessTransportState, _>(
                existing,
                "SubprocessTransport",
                |s| Ok(s.conn_lost_called),
            )
            .unwrap_or(true);
            if !is_done && !next.iter().any(|x| value_id(x) == id) {
                next.push(existing.clone());
            }
        }
        st.active_subprocesses = next;
        Ok(())
    })
}

fn exit_status_to_code(status: std::process::ExitStatus) -> i64 {
    if let Some(code) = status.code() {
        return code as i64;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return -(sig as i64);
        }
    }
    0
}

fn poll_one_subprocess(interp: &mut Interpreter, sp: &Value) -> Result<bool> {
    let mut events: Vec<(i64, Option<Vec<u8>>)> = Vec::new();
    let mut newly_exited = false;
    let mut all_done = false;
    let protocol =
        with_native_state_mut::<SubprocessTransportState, _>(sp, "SubprocessTransport", |st| {
            // Drain stdout channel
            if let Some(ref rx) = st.stdout_rx {
                loop {
                    match rx.try_recv() {
                        Ok(PipeMsg::Data(d)) => events.push((1, Some(d))),
                        Ok(PipeMsg::Eof) | Err(TryRecvError::Disconnected) => {
                            if !st.stdout_eof {
                                st.stdout_eof = true;
                                events.push((1, None));
                            }
                            break;
                        }
                        Err(TryRecvError::Empty) => break,
                    }
                }
                if st.stdout_eof {
                    st.stdout_rx = None;
                }
            }
            // Drain stderr channel
            if let Some(ref rx) = st.stderr_rx {
                loop {
                    match rx.try_recv() {
                        Ok(PipeMsg::Data(d)) => events.push((2, Some(d))),
                        Ok(PipeMsg::Eof) | Err(TryRecvError::Disconnected) => {
                            if !st.stderr_eof {
                                st.stderr_eof = true;
                                events.push((2, None));
                            }
                            break;
                        }
                        Err(TryRecvError::Empty) => break,
                    }
                }
                if st.stderr_eof {
                    st.stderr_rx = None;
                }
            }
            // Check child exit status
            if st.returncode.is_none()
                && let Ok(Some(status)) = st.child.try_wait()
            {
                st.returncode = Some(exit_status_to_code(status));
                newly_exited = true;
            }
            if st.returncode.is_some() && st.stdout_eof && st.stderr_eof && !st.conn_lost_called {
                all_done = true;
            }
            Ok(st.protocol.clone())
        })?;

    for (fd, opt_data) in events {
        match opt_data {
            Some(bytes) => {
                let _ = call_method(
                    interp,
                    &protocol,
                    "pipe_data_received",
                    &[Value::int(fd), Value::bytes(bytes)],
                );
            }
            None => {
                let _ = call_method(
                    interp,
                    &protocol,
                    "pipe_connection_lost",
                    &[Value::int(fd), Value::none()],
                );
            }
        }
    }

    if newly_exited {
        let should_notify = with_native_state_mut::<SubprocessTransportState, _>(
            sp,
            "SubprocessTransport",
            |st| {
                if st.exit_notified {
                    Ok(false)
                } else {
                    st.exit_notified = true;
                    Ok(true)
                }
            },
        )?;
        if should_notify {
            let _ = call_method(interp, &protocol, "process_exited", &[]);
        }
    }

    if all_done {
        with_native_state_mut::<SubprocessTransportState, _>(sp, "SubprocessTransport", |st| {
            st.conn_lost_called = true;
            Ok(())
        })?;
        let _ = call_method(interp, &protocol, "connection_lost", &[Value::none()]);
        return Ok(true);
    }

    let finished =
        with_native_state::<SubprocessTransportState, _>(sp, "SubprocessTransport", |st| {
            Ok(st.conn_lost_called)
        })?;
    Ok(finished)
}

pub(crate) fn pipe_write_bytes(transport: &Value, data: &[u8]) -> Result<()> {
    with_native_state_mut::<PipeWriteTransportState, _>(transport, "WriteTransport", |st| {
        if st.closing || st.closed {
            return Ok(());
        }
        if let Some(ref mut stdin) = st.stdin {
            stdin
                .write_all(data)
                .map_err(|e| PyError::from_io_error(&e, None))?;
            let _ = stdin.flush();
        }
        Ok(())
    })
}

pub(crate) fn pipe_write_close(interp: &mut Interpreter, transport: &Value) -> Result<()> {
    let (should_notify, protocol, fd) =
        with_native_state_mut::<PipeWriteTransportState, _>(transport, "WriteTransport", |st| {
            if st.closed {
                return Ok((false, st.protocol.clone(), st.fd));
            }
            st.closing = true;
            st.closed = true;
            st.stdin = None; // Drops ChildStdin, sending EOF to child!
            Ok((true, st.protocol.clone(), st.fd))
        })?;
    if should_notify {
        let _ = call_method(
            interp,
            &protocol,
            "pipe_connection_lost",
            &[Value::int(fd), Value::none()],
        );
    }
    Ok(())
}

/// Start TLS on an existing `_SelectorSocketTransport` (implements `loop.start_tls`
/// and `StreamWriter.start_tls`).
pub(crate) fn upgrade_transport_to_tls(
    interp: &mut Interpreter,
    transport: &Value,
    protocol: Value,
    ssl_context: Value,
    server_side: bool,
    server_hostname: Option<String>,
    handshake_waiter: Value,
) -> Result<()> {
    let tls = TlsLayer::new_with_mode(
        interp,
        ssl_context,
        server_side,
        server_hostname,
        Some(handshake_waiter),
        false,
    )?;
    with_native_state_mut::<SocketTransportState, _>(transport, "Transport", |st| {
        st.protocol = protocol;
        st.tls = Some(tls);
        Ok(())
    })?;
    transport_drive_tls(interp, transport)
}

/// Check if a Python dict/mapping represents `env` for subprocess.
pub(crate) fn extract_env_pairs(val: &Value) -> Result<Vec<(String, String)>> {
    let Some(dict) = val.as_dict() else {
        return Err(PyError::named("TypeError", "env must be a mapping"));
    };
    let mut pairs = Vec::with_capacity(dict.len());
    for (k, v) in dict.iter() {
        let ks = match k {
            PyKey::Str(s) => s.to_string(),
            _ => return Err(PyError::named("TypeError", "env keys must be str")),
        };
        let vs = match v.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named("TypeError", "env values must be str")),
        };
        pairs.push((ks, vs));
    }
    Ok(pairs)
}
