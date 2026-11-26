// `concurrent.futures._base` — 100% native-Rust Python 3.12 futures foundation
// (issue #3263).
//
// Provides `FIRST_COMPLETED`, `FIRST_EXCEPTION`, `ALL_COMPLETED`, `PENDING`,
// `RUNNING`, `CANCELLED`, `CANCELLED_AND_NOTIFIED`, `FINISHED`, `Error`,
// `CancelledError`, `TimeoutError`, `InvalidStateError`, `BrokenExecutor`,
// `DoneAndNotDoneFutures`, `Future`, `Executor`, `wait`, and `as_completed`.
//
// Reference: <https://docs.python.org/3.12/library/concurrent.futures.html>

use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::{
    set_native_state, with_native_state, with_native_state_mut,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyCallable, PyIndex, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, current_thread_is_daemon_worker, lookup_exc_class,
    render_value_repr, value_type_name_str,
};
use crate::value::{Value, ValueKind};

pub(crate) const FIRST_COMPLETED: &str = "FIRST_COMPLETED";
pub(crate) const FIRST_EXCEPTION: &str = "FIRST_EXCEPTION";
pub(crate) const ALL_COMPLETED: &str = "ALL_COMPLETED";

pub(crate) const PENDING: &str = "PENDING";
pub(crate) const RUNNING: &str = "RUNNING";
pub(crate) const CANCELLED: &str = "CANCELLED";
pub(crate) const CANCELLED_AND_NOTIFIED: &str = "CANCELLED_AND_NOTIFIED";
pub(crate) const FINISHED: &str = "FINISHED";

const WAIT_POLL_SLICE: Duration = Duration::from_millis(20);

pyrust_struct_seq! {
    pub(crate) DoneAndNotDoneFutures in "concurrent.futures._base" {
        fields: [done, not_done],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FutureStatus {
    Pending,
    Running,
    Cancelled,
    CancelledAndNotified,
    Finished,
}

impl FutureStatus {
    #[inline]
    pub(crate) fn is_cancelled(self) -> bool {
        matches!(self, Self::Cancelled | Self::CancelledAndNotified)
    }

    #[inline]
    pub(crate) fn is_done(self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::CancelledAndNotified | Self::Finished
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitReturnWhen {
    FirstCompleted,
    FirstException,
    AllCompleted,
}

enum WaiterInner {
    AsCompleted {
        ready_indices: VecDeque<usize>,
        finished_count: usize,
    },
    Wait {
        return_when: WaitReturnWhen,
        done_count: usize,
        total: usize,
        exception_seen: bool,
    },
}

struct FutureWaiter {
    state: Mutex<WaiterInner>,
    cvar: Condvar,
}

impl FutureWaiter {
    fn notify_finished(&self, index: usize, has_exception: bool) {
        let mut g = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *g {
            WaiterInner::AsCompleted {
                ready_indices,
                finished_count,
            } => {
                ready_indices.push_back(index);
                *finished_count += 1;
                self.cvar.notify_all();
            }
            WaiterInner::Wait {
                return_when,
                done_count,
                total,
                exception_seen,
            } => {
                *done_count += 1;
                if has_exception {
                    *exception_seen = true;
                }
                let satisfied = match return_when {
                    WaitReturnWhen::FirstCompleted => *done_count >= 1,
                    WaitReturnWhen::FirstException => *exception_seen || *done_count >= *total,
                    WaitReturnWhen::AllCompleted => *done_count >= *total,
                };
                if satisfied {
                    self.cvar.notify_all();
                }
            }
        }
    }
}

pub(crate) struct FutureSync {
    status: Mutex<FutureStatus>,
    cvar: Condvar,
    waiters: Mutex<Vec<(usize, Arc<FutureWaiter>)>>,
}

impl FutureSync {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(FutureStatus::Pending),
            cvar: Condvar::new(),
            waiters: Mutex::new(Vec::new()),
        })
    }

    #[inline]
    pub(crate) fn status(&self) -> FutureStatus {
        *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn add_waiter(&self, index: usize, waiter: Arc<FutureWaiter>) {
        self.waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((index, waiter));
    }

    fn remove_waiter(&self, waiter: &Arc<FutureWaiter>) {
        self.waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(_, w)| !Arc::ptr_eq(w, waiter));
    }

    fn notify_all_waiters(&self, has_exception: bool) {
        self.cvar.notify_all();
        let waiters: Vec<(usize, Arc<FutureWaiter>)> = {
            let mut g = self
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *g)
        };
        for (idx, w) in waiters {
            w.notify_finished(idx, has_exception);
        }
    }

    fn wait_done(&self, interp: &mut Interpreter, timeout: Option<f64>) -> Result<FutureStatus> {
        let cur = self.status();
        if cur.is_done() {
            return Ok(cur);
        }
        let deadline = timeout_to_deadline(timeout);
        if let Some(dl) = deadline
            && Instant::now() >= dl
        {
            return Ok(self.status());
        }
        let gil = Arc::clone(&interp.cold.gil_state);
        let res = interp.release_gil_during(|| {
            let mut st = self
                .status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if st.is_done() {
                    return Ok(*st);
                }
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    return Err(());
                }
                let wait_dur = match deadline {
                    Some(dl) => {
                        let now = Instant::now();
                        if now >= dl {
                            return Ok(*st);
                        }
                        (dl - now).min(WAIT_POLL_SLICE)
                    }
                    None => WAIT_POLL_SLICE,
                };
                let (next_st, _) = self
                    .cvar
                    .wait_timeout(st, wait_dur)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                st = next_st;
            }
        });
        match res {
            Ok(status) => Ok(status),
            Err(()) => Err(PyError::named("SystemExit", "")),
        }
    }
}

pub(crate) struct FutureState {
    sync: Arc<FutureSync>,
    result: Option<Value>,
    exception: Option<Value>,
    done_callbacks: Vec<Value>,
}

impl FutureState {
    fn new() -> Self {
        Self {
            sync: FutureSync::new(),
            result: None,
            exception: None,
            done_callbacks: Vec::new(),
        }
    }
}

struct MapIterState {
    futures: VecDeque<Value>,
    end_time: Option<Instant>,
    closed: bool,
}

struct AsCompletedIterState {
    futures: Vec<Value>,
    waiter: Arc<FutureWaiter>,
    deadline: Option<Instant>,
    yielded: usize,
    total: usize,
}

// ── Shared helpers ───────────────────────────────────────────────────────────

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn parse_optional_timeout(val: OptValueRef<'_>, fn_name: &str) -> Result<Option<f64>> {
    match val.as_value() {
        None => Ok(None),
        Some(v) if v.is_none() => Ok(None),
        Some(v) => match v.kind() {
            ValueKind::Float(f) => Ok(Some(f)),
            ValueKind::Int(i) => Ok(Some(i as f64)),
            ValueKind::Bool(b) => Ok(Some(if b { 1.0 } else { 0.0 })),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): timeout must be a real number or None, not '{}'",
                    value_type_name_str(v)
                ),
            )),
        },
    }
}

fn timeout_to_deadline(timeout: Option<f64>) -> Option<Instant> {
    timeout.and_then(|secs| {
        if secs.is_infinite() && secs.is_sign_positive() {
            None
        } else if secs <= 0.0 || secs.is_nan() {
            Some(Instant::now())
        } else {
            Some(Instant::now() + Duration::from_secs_f64(secs))
        }
    })
}

fn raise_module_exc(interp: &mut Interpreter, name: &str, msg: &str) -> PyError {
    if let Ok(m) = interp.load_module("concurrent.futures._base")
        && let Ok(cls) = interp.get_attr(&m, name)
    {
        let args: Vec<Value> = if msg.is_empty() {
            Vec::new()
        } else {
            vec![Value::string(msg)]
        };
        if let Ok(inst) = call_pos(interp, cls, &args) {
            return PyError::raised(inst);
        }
    }
    PyError::named("RuntimeError", msg)
}

fn invoke_done_callbacks(interp: &mut Interpreter, fut_val: &Value, callbacks: Vec<Value>) {
    for cb in callbacks {
        if let Err(err) = call_pos(interp, cb, std::slice::from_ref(fut_val)) {
            let fut_repr = render_value_repr(interp, fut_val).unwrap_or_else(|_| "<Future>".into());
            eprintln!("exception calling callback for {fut_repr}: {err}");
        }
    }
}

fn make_py_set(interp: &mut Interpreter, vals: Vec<Value>) -> Result<Value> {
    call_pos(interp, Value::builtin_function("set"), &[Value::list(vals)])
}

fn same_future(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (ValueKind::PyInstance(x), ValueKind::PyInstance(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

// ── Public Rust helpers for sibling `thread` and `process` modules ───────────

pub(crate) fn new_future_instance(interp: &mut Interpreter) -> Result<Value> {
    let base_mod = interp.load_module("concurrent.futures._base")?;
    let fut_cls = interp.get_attr(&base_mod, "Future")?;
    call_pos(interp, fut_cls, &[])
}

pub(crate) fn future_try_set_running(fut: &Value) -> Result<bool> {
    with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        let mut guard = st
            .sync
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *guard {
            FutureStatus::Cancelled => {
                *guard = FutureStatus::CancelledAndNotified;
                drop(guard);
                st.sync.cvar.notify_all();
                Ok(false)
            }
            FutureStatus::Pending => {
                *guard = FutureStatus::Running;
                Ok(true)
            }
            _ => Ok(false),
        }
    })
}

pub(crate) fn future_set_result(
    interp: &mut Interpreter,
    fut: &Value,
    result: Value,
) -> Result<()> {
    let callbacks = with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        let mut guard = st
            .sync
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_done() {
            return Err(raise_module_exc(
                interp,
                "InvalidStateError",
                "invalid state",
            ));
        }
        st.result = Some(result);
        st.exception = None;
        *guard = FutureStatus::Finished;
        drop(guard);
        let cbs = std::mem::take(&mut st.done_callbacks);
        st.sync.notify_all_waiters(false);
        Ok(cbs)
    })?;
    invoke_done_callbacks(interp, fut, callbacks);
    Ok(())
}

pub(crate) fn future_set_exception(
    interp: &mut Interpreter,
    fut: &Value,
    exc: Value,
) -> Result<()> {
    let callbacks = with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        let mut guard = st
            .sync
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_done() {
            return Err(raise_module_exc(
                interp,
                "InvalidStateError",
                "invalid state",
            ));
        }
        st.exception = Some(exc);
        st.result = None;
        *guard = FutureStatus::Finished;
        drop(guard);
        let cbs = std::mem::take(&mut st.done_callbacks);
        st.sync.notify_all_waiters(true);
        Ok(cbs)
    })?;
    invoke_done_callbacks(interp, fut, callbacks);
    Ok(())
}

pub(crate) fn future_cancel_internal(interp: &mut Interpreter, fut: &Value) -> Result<bool> {
    let (cancelled, callbacks) = with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        let mut guard = st
            .sync
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *guard {
            FutureStatus::Running | FutureStatus::Finished => Ok((false, Vec::new())),
            FutureStatus::Cancelled | FutureStatus::CancelledAndNotified => Ok((true, Vec::new())),
            FutureStatus::Pending => {
                *guard = FutureStatus::Cancelled;
                drop(guard);
                let cbs = std::mem::take(&mut st.done_callbacks);
                st.sync.notify_all_waiters(false);
                Ok((true, cbs))
            }
        }
    })?;
    if !callbacks.is_empty() {
        invoke_done_callbacks(interp, fut, callbacks);
    }
    Ok(cancelled)
}

pub(crate) fn future_set_running_or_notify_cancel_internal(
    interp: &mut Interpreter,
    fut: &Value,
) -> Result<bool> {
    with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        let mut guard = st
            .sync
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *guard {
            FutureStatus::Cancelled => {
                *guard = FutureStatus::CancelledAndNotified;
                drop(guard);
                st.sync.cvar.notify_all();
                Ok(false)
            }
            FutureStatus::Pending => {
                *guard = FutureStatus::Running;
                Ok(true)
            }
            _ => Err(raise_module_exc(
                interp,
                "InvalidStateError",
                "Future in unexpected state",
            )),
        }
    })
}

pub(crate) fn future_sync_handle(fut: &Value) -> Result<Arc<FutureSync>> {
    with_native_state::<FutureState, _>(fut, "Future", |st| Ok(Arc::clone(&st.sync)))
}

fn future_has_exception(fut: &Value) -> bool {
    with_native_state::<FutureState, _>(fut, "Future", |st| Ok(st.exception.is_some()))
        .unwrap_or(false)
}

fn cancel_remaining_futures(interp: &mut Interpreter, futures: &mut VecDeque<Value>) {
    while let Some(fut) = futures.pop_front() {
        if let Ok(cancel_fn) = interp.get_attr(&fut, "cancel") {
            let _ = call_pos(interp, cancel_fn, &[]);
        }
    }
}

pyrust_module! {
    constants {
        "FIRST_COMPLETED" => Value::string(FIRST_COMPLETED),
        "FIRST_EXCEPTION" => Value::string(FIRST_EXCEPTION),
        "ALL_COMPLETED" => Value::string(ALL_COMPLETED),
        "PENDING" => Value::string(PENDING),
        "RUNNING" => Value::string(RUNNING),
        "CANCELLED" => Value::string(CANCELLED),
        "CANCELLED_AND_NOTIFIED" => Value::string(CANCELLED_AND_NOTIFIED),
        "FINISHED" => Value::string(FINISHED),
        "TimeoutError" => Value::py_class(
            lookup_exc_class("TimeoutError").expect("TimeoutError builtin must exist")
        ),
        "DoneAndNotDoneFutures" => DoneAndNotDoneFutures::class(),
    }

    /// Base class for all future-related exceptions.
    exception Error;

    /// The Future was cancelled.
    exception CancelledError(Error);

    /// The operation is not allowed in this state.
    exception InvalidStateError(Error);

    /// Raised when a future could not be executed because the executor broke.
    exception BrokenExecutor(RuntimeError);

    #[native_state(FutureState)]
    class Future {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            set_state(self_val.0, FutureState::new())?;
            Ok(Value::none())
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            item: PyValueRef<'_>,
        ) -> Result<Value> {
            let types_mod = _interp.load_module("types")?;
            let ga = _interp.get_attr(&types_mod, "GenericAlias")?;
            call_pos(_interp, ga, &[cls.0.clone(), item.0.clone()])
        }

        fn cancel(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(future_cancel_internal(_interp, self_val.0)?))
        }

        fn cancelled(#[state] st: &FutureState) -> Result<Value> {
            Ok(Value::bool_(st.sync.status().is_cancelled()))
        }

        fn running(#[state] st: &FutureState) -> Result<Value> {
            Ok(Value::bool_(st.sync.status() == FutureStatus::Running))
        }

        fn done(#[state] st: &FutureState) -> Result<Value> {
            Ok(Value::bool_(st.sync.status().is_done()))
        }

        fn add_done_callback(
            #[positional_only] self_val: PyValueRef<'_>,
            fn_cb: PyCallable,
        ) -> Result<Value> {
            let already_done = with_state_mut(self_val.0, |st| {
                if st.sync.status().is_done() {
                    Ok(true)
                } else {
                    st.done_callbacks.push(fn_cb.as_value().clone());
                    Ok(false)
                }
            })?;
            if already_done {
                invoke_done_callbacks(_interp, self_val.0, vec![fn_cb.into_value()]);
            }
            Ok(Value::none())
        }

        fn result(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let timeout_f = parse_optional_timeout(timeout, "Future.result")?;
            let sync = with_state(self_val.0, |st| Ok(Arc::clone(&st.sync)))?;
            let status = sync.wait_done(_interp, timeout_f)?;
            if status.is_cancelled() {
                return Err(raise_module_exc(_interp, "CancelledError", ""));
            }
            if status != FutureStatus::Finished {
                return Err(PyError::named("TimeoutError", ""));
            }
            let (res_opt, exc_opt) = with_state(self_val.0, |st| {
                Ok((st.result.clone(), st.exception.clone()))
            })?;
            if let Some(exc) = exc_opt {
                return Err(PyError::raised(exc));
            }
            Ok(res_opt.unwrap_or_else(Value::none))
        }

        fn exception(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let timeout_f = parse_optional_timeout(timeout, "Future.exception")?;
            let sync = with_state(self_val.0, |st| Ok(Arc::clone(&st.sync)))?;
            let status = sync.wait_done(_interp, timeout_f)?;
            if status.is_cancelled() {
                return Err(raise_module_exc(_interp, "CancelledError", ""));
            }
            if status != FutureStatus::Finished {
                return Err(PyError::named("TimeoutError", ""));
            }
            with_state(self_val.0, |st| {
                Ok(st.exception.clone().unwrap_or_else(Value::none))
            })
        }

        fn set_running_or_notify_cancel(
            #[positional_only] self_val: PyValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::bool_(
                future_set_running_or_notify_cancel_internal(_interp, self_val.0)?,
            ))
        }

        fn set_result(
            #[positional_only] self_val: PyValueRef<'_>,
            result: PyValue,
        ) -> Result<Value> {
            future_set_result(_interp, self_val.0, result.0)?;
            Ok(Value::none())
        }

        fn set_exception(
            #[positional_only] self_val: PyValueRef<'_>,
            exception: PyValue,
        ) -> Result<Value> {
            future_set_exception(_interp, self_val.0, exception.0)?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let addr = match self_val.0.kind() {
                ValueKind::PyInstance(rc) => Rc::as_ptr(rc) as usize,
                _ => 0,
            };
            let (status, has_res, exc_opt, res_opt) = with_state(self_val.0, |st| {
                Ok((
                    st.sync.status(),
                    st.result.is_some(),
                    st.exception.clone(),
                    st.result.clone(),
                ))
            })?;
            let state_str = match status {
                FutureStatus::Pending => "pending",
                FutureStatus::Running => "running",
                FutureStatus::Cancelled | FutureStatus::CancelledAndNotified => "cancelled",
                FutureStatus::Finished => "finished",
            };
            if status == FutureStatus::Finished {
                if let Some(exc) = exc_opt {
                    let tname = value_type_name_str(&exc);
                    return Ok(Value::string(format!(
                        "<Future at {addr:#x} state={state_str} raised {tname}>"
                    )));
                }
                if has_res
                    && let Some(r) = res_opt
                {
                    let tname = value_type_name_str(&r);
                    return Ok(Value::string(format!(
                        "<Future at {addr:#x} state={state_str} returned {tname}>"
                    )));
                }
            }
            Ok(Value::string(format!(
                "<Future at {addr:#x} state={state_str}>"
            )))
        }
    }

    #[native_state(MapIterState)]
    class _MapIterator {
        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (next_fut, end_time) = with_state_mut(self_val.0, |st| {
                if st.closed {
                    return Ok((None, None));
                }
                Ok((st.futures.pop_front(), st.end_time))
            })?;
            let Some(fut) = next_fut else {
                return Err(PyError::named("StopIteration", ""));
            };
            let result_fn = _interp.get_attr(&fut, "result")?;
            let call_res = match end_time {
                None => call_pos(_interp, result_fn, &[]),
                Some(dl) => {
                    let rem = dl
                        .checked_duration_since(Instant::now())
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    call_pos(_interp, result_fn, &[Value::float(rem)])
                }
            };
            match call_res {
                Ok(val) => Ok(val),
                Err(err) => {
                    let mut remaining = with_state_mut(self_val.0, |st| {
                        st.closed = true;
                        Ok(std::mem::take(&mut st.futures))
                    })?;
                    cancel_remaining_futures(_interp, &mut remaining);
                    Err(err)
                }
            }
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut remaining = with_state_mut(self_val.0, |st| {
                st.closed = true;
                Ok(std::mem::take(&mut st.futures))
            })?;
            cancel_remaining_futures(_interp, &mut remaining);
            Ok(Value::none())
        }
    }

    #[native_state(AsCompletedIterState)]
    class _AsCompletedIterator {
        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (waiter, deadline, yielded, total) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.waiter), st.deadline, st.yielded, st.total))
            })?;
            if yielded >= total {
                return Err(PyError::named("StopIteration", ""));
            }

            let gil = Arc::clone(&_interp.cold.gil_state);
            let popped = _interp.release_gil_during(|| {
                let mut g = waiter
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                loop {
                    if let WaiterInner::AsCompleted { ready_indices, .. } = &mut *g
                        && let Some(idx) = ready_indices.pop_front()
                    {
                        return Ok(Some(idx));
                    }
                    if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                        return Err(());
                    }
                    let wait_dur = match deadline {
                        Some(dl) => {
                            let now = Instant::now();
                            if now >= dl {
                                return Ok(None);
                            }
                            (dl - now).min(WAIT_POLL_SLICE)
                        }
                        None => WAIT_POLL_SLICE,
                    };
                    let (next_g, _) = waiter
                        .cvar
                        .wait_timeout(g, wait_dur)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    g = next_g;
                }
            });

            match popped {
                Err(()) => Err(PyError::named("SystemExit", "")),
                Ok(Some(idx)) => {
                    let fut = with_state_mut(self_val.0, |st| {
                        st.yielded += 1;
                        Ok(st.futures[idx].clone())
                    })?;
                    Ok(fut)
                }
                Ok(None) => {
                    let remaining = total.saturating_sub(yielded);
                    Err(PyError::named(
                        "TimeoutError",
                        format!("{remaining} (of {total}) futures unfinished"),
                    ))
                }
            }
        }
    }

    class Executor {
        fn submit(
            #[positional_only] _self: PyValueRef<'_>,
            #[positional_only] _fn: PyValueRef<'_>,
            #[varargs] _args: &[Value],
            #[varkwargs] _kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            Err(PyError::named("NotImplementedError", ""))
        }

        fn map(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            #[varargs] iterables: &[Value],
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
            #[default(1i64.into())] chunksize: PyIndex,
        ) -> Result<Value> {
            let _ = chunksize.expect_i64("Executor.map", "chunksize")?;
            let timeout_f = parse_optional_timeout(timeout, "Executor.map")?;
            let end_time = timeout_to_deadline(timeout_f);

            let zipped = call_pos(_interp, Value::builtin_function("zip"), iterables)?;
            let pairs = _interp.collect_iterable(&zipped)?;

            let submit_fn = _interp.get_attr(self_val.0, "submit")?;
            let mut futures = VecDeque::with_capacity(pairs.len());
            for tuple_val in pairs {
                let mut call_args = Vec::new();
                call_args.push(func.0.clone());
                match tuple_val.kind() {
                    ValueKind::Tuple(items) => call_args.extend(items.iter().cloned()),
                    _ => call_args.push(tuple_val.clone()),
                }
                let fut = call_pos(_interp, submit_fn.clone(), &call_args)?;
                futures.push_back(fut);
            }

            let base_mod = _interp.load_module("concurrent.futures._base")?;
            let iter_cls = _interp.get_attr(&base_mod, "_MapIterator")?;
            let iter_obj = call_pos(_interp, iter_cls, &[])?;
            set_native_state(
                &iter_obj,
                MapIterState {
                    futures,
                    end_time,
                    closed: false,
                },
            )?;
            Ok(iter_obj)
        }

        fn shutdown(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(true.into())] _wait: PyBool,
            #[keyword_only] #[default(false.into())] _cancel_futures: PyBool,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            _exc_type: PyValueRef<'_>,
            _exc_val: PyValueRef<'_>,
            _exc_tb: PyValueRef<'_>,
        ) -> Result<Value> {
            let shutdown_fn = _interp.get_attr(self_val.0, "shutdown")?;
            let _ = _interp.call_function_expanded(
                shutdown_fn,
                &[ExpandedCallArg {
                    name: Some("wait".to_string()),
                    value: Value::bool_(true),
                }],
            )?;
            Ok(Value::bool_(false))
        }
    }

    fn as_completed(
        fs: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
    ) -> Result<Value> {
        let timeout_f = parse_optional_timeout(timeout, "as_completed")?;
        let deadline = timeout_to_deadline(timeout_f);

        let raw_items = _interp.collect_iterable(fs.0)?;
        let mut unique: Vec<Value> = Vec::with_capacity(raw_items.len());
        for item in raw_items {
            if !unique.iter().any(|u| same_future(u, &item)) {
                unique.push(item);
            }
        }
        let total = unique.len();

        let waiter = Arc::new(FutureWaiter {
            state: Mutex::new(WaiterInner::AsCompleted {
                ready_indices: VecDeque::new(),
                finished_count: 0,
            }),
            cvar: Condvar::new(),
        });

        for (idx, fut) in unique.iter().enumerate() {
            let sync = future_sync_handle(fut)?;
            let mut status_guard = sync
                .status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if status_guard.is_done() {
                drop(status_guard);
                waiter.notify_finished(idx, false);
            } else {
                sync.add_waiter(idx, Arc::clone(&waiter));
                let _ = &mut status_guard;
            }
        }

        let base_mod = _interp.load_module("concurrent.futures._base")?;
        let iter_cls = _interp.get_attr(&base_mod, "_AsCompletedIterator")?;
        let iter_obj = call_pos(_interp, iter_cls, &[])?;
        set_native_state(
            &iter_obj,
            AsCompletedIterState {
                futures: unique,
                waiter,
                deadline,
                yielded: 0,
                total,
            },
        )?;
        Ok(iter_obj)
    }

    fn wait(
        fs: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        #[default(PyValue(Value::string(ALL_COMPLETED)))] return_when: PyValue,
    ) -> Result<Value> {
        let timeout_f = parse_optional_timeout(timeout, "wait")?;
        let deadline = timeout_to_deadline(timeout_f);

        let raw_items = _interp.collect_iterable(fs.0)?;
        let mut unique: Vec<Value> = Vec::with_capacity(raw_items.len());
        for item in raw_items {
            if !unique.iter().any(|u| same_future(u, &item)) {
                unique.push(item);
            }
        }

        let total = unique.len();
        if total == 0 {
            let empty1 = make_py_set(_interp, Vec::new())?;
            let empty2 = make_py_set(_interp, Vec::new())?;
            return Ok(DoneAndNotDoneFutures::make([empty1, empty2]));
        }

        let when_str = return_when.0.as_str().ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!("Invalid return condition: {}", return_when.0.repr_raw()),
            )
        })?;
        let mode = match when_str {
            FIRST_COMPLETED => WaitReturnWhen::FirstCompleted,
            FIRST_EXCEPTION => WaitReturnWhen::FirstException,
            ALL_COMPLETED => WaitReturnWhen::AllCompleted,
            other => {
                return Err(PyError::named(
                    "ValueError",
                    format!("Invalid return condition: {other:?}"),
                ));
            }
        };

        let mut initial_done = 0usize;
        let mut initial_exc = false;
        let mut pending_syncs: Vec<Arc<FutureSync>> = Vec::new();
        let waiter = Arc::new(FutureWaiter {
            state: Mutex::new(WaiterInner::Wait {
                return_when: mode,
                done_count: 0,
                total,
                exception_seen: false,
            }),
            cvar: Condvar::new(),
        });

        for (idx, fut) in unique.iter().enumerate() {
            let sync = future_sync_handle(fut)?;
            let status = {
                let guard = sync
                    .status
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !guard.is_done() {
                    sync.add_waiter(idx, Arc::clone(&waiter));
                }
                *guard
            };
            if status.is_done() {
                initial_done += 1;
                if !status.is_cancelled() && future_has_exception(fut) {
                    initial_exc = true;
                }
            } else {
                pending_syncs.push(sync);
            }
        }

        {
            let mut g = waiter
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let WaiterInner::Wait {
                done_count,
                exception_seen,
                ..
            } = &mut *g
            {
                *done_count += initial_done;
                *exception_seen |= initial_exc;
            }
        }

        let already_satisfied = match mode {
            WaitReturnWhen::FirstCompleted => initial_done >= 1,
            WaitReturnWhen::FirstException => initial_exc || initial_done >= total,
            WaitReturnWhen::AllCompleted => initial_done >= total,
        };

        if !already_satisfied {
            let gil = Arc::clone(&_interp.cold.gil_state);
            let wait_res = _interp.release_gil_during(|| {
                let mut g = waiter
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                loop {
                    let satisfied = match &*g {
                        WaiterInner::Wait {
                            return_when,
                            done_count,
                            total,
                            exception_seen,
                        } => match return_when {
                            WaitReturnWhen::FirstCompleted => *done_count >= 1,
                            WaitReturnWhen::FirstException => {
                                *exception_seen || *done_count >= *total
                            }
                            WaitReturnWhen::AllCompleted => *done_count >= *total,
                        },
                        _ => true,
                    };
                    if satisfied {
                        return Ok(());
                    }
                    if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                        return Err(());
                    }
                    let wait_dur = match deadline {
                        Some(dl) => {
                            let now = Instant::now();
                            if now >= dl {
                                return Ok(());
                            }
                            (dl - now).min(WAIT_POLL_SLICE)
                        }
                        None => WAIT_POLL_SLICE,
                    };
                    let (next_g, _) = waiter
                        .cvar
                        .wait_timeout(g, wait_dur)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    g = next_g;
                }
            });
            for sync in &pending_syncs {
                sync.remove_waiter(&waiter);
            }
            if wait_res.is_err() {
                return Err(PyError::named("SystemExit", ""));
            }
        } else {
            for sync in &pending_syncs {
                sync.remove_waiter(&waiter);
            }
        }

        let mut done_vec = Vec::new();
        let mut not_done_vec = Vec::new();
        for fut in unique {
            let sync = future_sync_handle(&fut)?;
            if sync.status().is_done() {
                done_vec.push(fut);
            } else {
                not_done_vec.push(fut);
            }
        }
        let done_set = make_py_set(_interp, done_vec)?;
        let not_done_set = make_py_set(_interp, not_done_vec)?;
        Ok(DoneAndNotDoneFutures::make([done_set, not_done_set]))
    }
}
