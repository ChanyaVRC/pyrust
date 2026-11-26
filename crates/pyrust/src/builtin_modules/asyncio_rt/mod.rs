//! Core native-Rust `asyncio` runtime state and helpers (issue #3254).
//!
//! All `asyncio` primitives, event loop, transports, protocols, streams,
//! `Runner`, subprocesses, and submodules live 100% natively in Rust without
//! any `@inject` or `*_py.py` files.

pub(crate) mod awaitable;
pub(crate) mod io_engine;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::Instant;

use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state, with_native_state_mut,
};
use crate::error::{PyError, Result};
use crate::interpreter::{
    CoroStep, ExpandedCallArg, Interpreter, instantiate_exception, lookup_exc_class,
    render_value_repr, value_type_name_str,
};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, Value, ValueKind};

pub(crate) const DEFAULT_STREAM_LIMIT: usize = 65_536; // 64 KiB, matches asyncio.streams._DEFAULT_LIMIT

pub(crate) const FIRST_COMPLETED: &str = "FIRST_COMPLETED";
pub(crate) const FIRST_EXCEPTION: &str = "FIRST_EXCEPTION";
pub(crate) const ALL_COMPLETED: &str = "ALL_COMPLETED";

pub(crate) const PIPE_SENTINEL: i64 = -1;
pub(crate) const STDOUT_SENTINEL: i64 = -2;
pub(crate) const DEVNULL_SENTINEL: i64 = -3;

/// Process-wide monotonic clock epoch so `loop.time()` values are monotonic and
/// comparable across runs in the same process.
static MONO_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub(crate) fn monotonic_secs() -> f64 {
    MONO_EPOCH.elapsed().as_secs_f64()
}

thread_local! {
    /// Currently running event loop on this thread.
    static RUNNING_LOOP: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// Active event loop policy on this thread.
    static EVENT_LOOP_POLICY: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// Monotonic counter for default `Task-N` names.
    static TASK_NAME_COUNTER: Cell<u64> = const { Cell::new(0) };
    /// Active (not-yet-finished) Tasks across loops on this thread (insertion order).
    static SCHEDULED_TASKS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    /// Currently executing Task per loop identity (`loop_id -> Task`).
    static CURRENT_TASKS: RefCell<Vec<(usize, Value)>> = const { RefCell::new(Vec::new()) };
    /// Unique sentinel object for `asyncio.coroutines._is_coroutine`.
    static IS_CORO_SENTINEL: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// Stable identity key for Python heap objects (`PyInstance`, `PyClass`, etc.).
pub(crate) fn value_id(v: &Value) -> usize {
    match v.kind() {
        ValueKind::PyInstance(rc) => Rc::as_ptr(rc) as usize,
        ValueKind::PyClass(rc) => Rc::as_ptr(rc) as usize,
        ValueKind::PyModule(rc) => Rc::as_ptr(rc) as usize,
        ValueKind::Generator(rc) => Rc::as_ptr(rc) as usize,
        ValueKind::BoundMethod { function, .. } => Rc::as_ptr(function) as usize,
        _ => v as *const Value as usize,
    }
}

pub(crate) fn same_object(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (ValueKind::PyInstance(x), ValueKind::PyInstance(y)) => Rc::ptr_eq(x, y),
        (ValueKind::PyClass(x), ValueKind::PyClass(y)) => Rc::ptr_eq(x, y),
        (ValueKind::PyModule(x), ValueKind::PyModule(y)) => Rc::ptr_eq(x, y),
        (ValueKind::Generator(x), ValueKind::Generator(y)) => Rc::ptr_eq(x, y),
        (
            ValueKind::BoundMethod {
                function: fx,
                receiver: rx,
            },
            ValueKind::BoundMethod {
                function: fy,
                receiver: ry,
            },
        ) => Rc::ptr_eq(fx, fy) && Rc::ptr_eq(rx, ry),
        (ValueKind::None, ValueKind::None) => true,
        _ => false,
    }
}

/// Build a Python `set` from a `Vec<Value>` using Python's `set()` builtin so
/// custom `__hash__` and instance identity hashing work identically to CPython.
pub(crate) fn make_py_set(interp: &mut Interpreter, vals: Vec<Value>) -> Result<Value> {
    call_pos(interp, Value::builtin_function("set"), &[Value::list(vals)])
}

pub(crate) fn get_running_loop_opt() -> Option<Value> {
    RUNNING_LOOP.with(|c| c.borrow().clone())
}

pub(crate) fn set_running_loop_opt(loop_val: Option<Value>) {
    RUNNING_LOOP.with(|c| {
        *c.borrow_mut() = loop_val;
    });
}

pub(crate) fn get_running_loop_or_err() -> Result<Value> {
    get_running_loop_opt().ok_or_else(|| PyError::named("RuntimeError", "no running event loop"))
}

pub(crate) fn get_event_loop_policy(interp: &mut Interpreter) -> Result<Value> {
    let existing = EVENT_LOOP_POLICY.with(|c| c.borrow().clone());
    if let Some(p) = existing {
        return Ok(p);
    }
    let cls = asyncio_attr(interp, "DefaultEventLoopPolicy")?;
    let policy = call_pos(interp, cls, &[])?;
    EVENT_LOOP_POLICY.with(|c| {
        *c.borrow_mut() = Some(policy.clone());
    });
    Ok(policy)
}

pub(crate) fn set_event_loop_policy(policy: Option<Value>) {
    EVENT_LOOP_POLICY.with(|c| {
        *c.borrow_mut() = policy;
    });
}

pub(crate) fn next_task_name() -> String {
    TASK_NAME_COUNTER.with(|c| {
        let n = c.get() + 1;
        c.set(n);
        format!("Task-{n}")
    })
}

pub(crate) fn register_task(task: Value) {
    let tid = value_id(&task);
    SCHEDULED_TASKS.with(|c| {
        let mut list = c.borrow_mut();
        if !list.iter().any(|t| value_id(t) == tid) {
            list.push(task);
        }
    });
}

pub(crate) fn unregister_task(task: &Value) {
    let tid = value_id(task);
    SCHEDULED_TASKS.with(|c| {
        c.borrow_mut().retain(|t| value_id(t) != tid);
    });
}

pub(crate) fn clear_loop_tasks(loop_val: &Value) {
    let lid = value_id(loop_val);
    CURRENT_TASKS.with(|c| {
        c.borrow_mut().retain(|(id, _)| *id != lid);
    });
    SCHEDULED_TASKS.with(|c| {
        c.borrow_mut().retain(|t| {
            let belongs = with_native_state::<FutureState, _>(t, "Task", |st| {
                Ok(same_object(&st.loop_val, loop_val))
            })
            .unwrap_or(false);
            !belongs
        });
    });
}

pub(crate) fn enter_task(interp: &mut Interpreter, loop_val: &Value, task: &Value) -> Result<()> {
    let lid = value_id(loop_val);
    let existing = CURRENT_TASKS.with(|c| {
        c.borrow()
            .iter()
            .find(|(id, _)| *id == lid)
            .map(|(_, t)| t.clone())
    });
    if let Some(cur) = existing {
        let tr = render_value_repr(interp, task)?;
        let cr = render_value_repr(interp, &cur)?;
        return Err(PyError::named(
            "RuntimeError",
            format!("Cannot enter into task {tr} while another task {cr} is being executed."),
        ));
    }
    CURRENT_TASKS.with(|c| {
        c.borrow_mut().push((lid, task.clone()));
    });
    Ok(())
}

pub(crate) fn leave_task(interp: &mut Interpreter, loop_val: &Value, task: &Value) -> Result<()> {
    let lid = value_id(loop_val);
    let existing = CURRENT_TASKS.with(|c| {
        c.borrow()
            .iter()
            .find(|(id, _)| *id == lid)
            .map(|(_, t)| t.clone())
    });
    match existing {
        Some(ref cur) if same_object(cur, task) => {
            CURRENT_TASKS.with(|c| {
                c.borrow_mut().retain(|(id, _)| *id != lid);
            });
            Ok(())
        }
        other => {
            let tr = render_value_repr(interp, task)?;
            let cr = match other {
                Some(v) => render_value_repr(interp, &v)?,
                None => "None".to_string(),
            };
            Err(PyError::named(
                "RuntimeError",
                format!("Leaving task {tr} does not match the current task {cr}."),
            ))
        }
    }
}

pub(crate) fn swap_current_task(loop_val: &Value, new_task: Option<Value>) -> Option<Value> {
    let lid = value_id(loop_val);
    CURRENT_TASKS.with(|c| {
        let mut list = c.borrow_mut();
        let prev = list
            .iter()
            .position(|(id, _)| *id == lid)
            .map(|idx| list.remove(idx).1);
        if let Some(t) = new_task {
            list.push((lid, t));
        }
        prev
    })
}

pub(crate) fn current_task_for_loop(loop_val: &Value) -> Option<Value> {
    let lid = value_id(loop_val);
    CURRENT_TASKS.with(|c| {
        c.borrow()
            .iter()
            .find(|(id, _)| *id == lid)
            .map(|(_, t)| t.clone())
    })
}

pub(crate) fn all_tasks_for_loop(loop_val: &Value) -> Vec<Value> {
    SCHEDULED_TASKS.with(|c| {
        c.borrow()
            .iter()
            .filter(|t| {
                with_native_state::<FutureState, _>(t, "Task", |st| {
                    Ok(same_object(&st.loop_val, loop_val) && !st.is_done())
                })
                .unwrap_or(false)
            })
            .cloned()
            .collect()
    })
}

pub(crate) fn is_coro_sentinel(interp: &mut Interpreter) -> Result<Value> {
    let existing = IS_CORO_SENTINEL.with(|c| c.borrow().clone());
    if let Some(v) = existing {
        return Ok(v);
    }
    let obj_cls = Value::py_class(crate::interpreter::object_class_singleton());
    let sentinel = call_pos(interp, obj_cls, &[])?;
    IS_CORO_SENTINEL.with(|c| {
        *c.borrow_mut() = Some(sentinel.clone());
    });
    Ok(sentinel)
}

// ── Call & attribute helpers ─────────────────────────────────────────────────

pub(crate) fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

pub(crate) fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded = Vec::with_capacity(pos.len() + kw.len());
    for v in pos {
        expanded.push(ExpandedCallArg {
            name: None,
            value: v.clone(),
        });
    }
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

pub(crate) fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_pos(interp, m, args)
}

pub(crate) fn call_method_kw(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_kw(interp, m, pos, kw)
}

pub(crate) fn asyncio_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("asyncio")?;
    interp.get_attr(&module, name)
}

/// Allocate a raw `PyInstance` of `asyncio.<class_name>` (without calling `__init__`).
pub(crate) fn alloc_asyncio_instance(interp: &mut Interpreter, class_name: &str) -> Result<Value> {
    let cls_val = asyncio_attr(interp, class_name)?;
    let ValueKind::PyClass(cls_rc) = cls_val.kind() else {
        return Err(PyError::runtime(format!(
            "asyncio.{class_name} is not a class"
        )));
    };
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    }))))
}

// ── Contextvars integration ──────────────────────────────────────────────────

pub(crate) fn copy_context(interp: &mut Interpreter) -> Result<Value> {
    let cv = interp.load_module("contextvars")?;
    let f = interp.get_attr(&cv, "copy_context")?;
    call_pos(interp, f, &[])
}

pub(crate) fn run_in_context(
    interp: &mut Interpreter,
    context: &Value,
    callback: &Value,
    args: &[Value],
) -> Result<Value> {
    let run_method = interp.get_attr(context, "run")?;
    let mut all = Vec::with_capacity(1 + args.len());
    all.push(callback.clone());
    all.extend(args.iter().cloned());
    call_pos(interp, run_method, &all)
}

// ── Exception & StopIteration helpers ────────────────────────────────────────

pub(crate) fn make_stop_iteration_with_value(value: Value) -> PyError {
    if let Some(cls) = lookup_exc_class("StopIteration") {
        let args = if value.is_none() {
            Vec::new()
        } else {
            vec![value]
        };
        PyError::raised(instantiate_exception(cls, args))
    } else {
        PyError::named("StopIteration", String::new())
    }
}

pub(crate) fn make_stop_async_iteration() -> PyError {
    if let Some(cls) = lookup_exc_class("StopAsyncIteration") {
        PyError::raised(instantiate_exception(cls, Vec::new()))
    } else {
        PyError::named("StopAsyncIteration", String::new())
    }
}

pub(crate) fn make_asyncio_exc(
    interp: &mut Interpreter,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let cls = asyncio_attr(interp, name)?;
    call_pos(interp, cls, args)
}

pub(crate) fn raise_asyncio_exc(interp: &mut Interpreter, name: &str, msg: &str) -> PyError {
    let args = if msg.is_empty() {
        Vec::new()
    } else {
        vec![Value::string(msg)]
    };
    match make_asyncio_exc(interp, name, &args) {
        Ok(v) => PyError::raised(v),
        Err(e) => e,
    }
}

pub(crate) fn make_cancelled_error_val(
    interp: &mut Interpreter,
    msg: Option<&Value>,
) -> Result<Value> {
    match msg {
        Some(m) if !m.is_none() => {
            make_asyncio_exc(interp, "CancelledError", std::slice::from_ref(m))
        }
        _ => make_asyncio_exc(interp, "CancelledError", &[]),
    }
}

pub(crate) fn pyerror_to_exc_value(interp: &mut Interpreter, err: PyError) -> Result<Value> {
    interp.materialize_pyerror(err)
}

pub(crate) fn is_isinstance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> bool {
    call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls.clone()],
    )
    .and_then(|v| interp.truthy_value(&v))
    .unwrap_or(false)
}

pub(crate) fn is_cancelled_error_val(interp: &mut Interpreter, exc: &Value) -> bool {
    asyncio_attr(interp, "CancelledError")
        .map(|cls| is_isinstance(interp, exc, &cls))
        .unwrap_or(false)
}

pub(crate) fn is_base_fatal_error_val(interp: &mut Interpreter, exc: &Value) -> bool {
    let ki = lookup_exc_class("KeyboardInterrupt")
        .map(|c| is_isinstance(interp, exc, &Value::py_class(c)))
        .unwrap_or(false);
    if ki {
        return true;
    }
    lookup_exc_class("SystemExit")
        .map(|c| is_isinstance(interp, exc, &Value::py_class(c)))
        .unwrap_or(false)
}

/// Set `__cause__` / `__suppress_context__` on `exc` (matching `raise X from Y`).
pub(crate) fn set_exc_cause(interp: &mut Interpreter, exc: &Value, cause: Option<Value>) {
    let cause_val = cause.unwrap_or_else(Value::none);
    let _ = interp.assign_attr(exc.clone(), "__cause__", cause_val);
    let _ = interp.assign_attr(exc.clone(), "__suppress_context__", Value::bool_(true));
}

// ── Coroutine / Future / Awaitable classification & stepping ─────────────────

pub(crate) fn is_native_coro_obj(v: &Value) -> bool {
    has_native_state::<awaitable::NativeCoroState>(v)
}

pub(crate) fn is_coroutine_obj(v: &Value) -> bool {
    (crate::interpreter::is_coroutine_value(v) && !crate::interpreter::is_async_generator_value(v))
        || is_native_coro_obj(v)
}

/// Native function names in `asyncio` that are coroutine functions (`async def`
/// in CPython 3.12). Used by `asyncio.iscoroutinefunction`.
pub(crate) const NATIVE_CORO_FUNCTIONS: &[&str] = &[
    "asyncio.sleep",
    "asyncio.wait",
    "asyncio.wait_for",
    "asyncio.to_thread",
    "asyncio.open_connection",
    "asyncio.start_server",
    "asyncio.create_subprocess_exec",
    "asyncio.create_subprocess_shell",
    "asyncio.staggered.staggered_race",
    "asyncio.streams.open_connection",
    "asyncio.streams.start_server",
    "asyncio.subprocess.create_subprocess_exec",
    "asyncio.subprocess.create_subprocess_shell",
    "asyncio.threads.to_thread",
    "asyncio.tasks.sleep",
    "asyncio.tasks.wait",
    "asyncio.tasks.wait_for",
];

pub(crate) fn is_coroutine_function_obj(interp: &mut Interpreter, func: &Value) -> bool {
    if let ValueKind::BuiltinFunction(name) = func.kind()
        && NATIVE_CORO_FUNCTIONS.contains(&name)
    {
        return true;
    }
    if let Ok(sentinel) = is_coro_sentinel(interp)
        && let Ok(attr) = interp.get_attr(func, "_is_coroutine")
        && same_object(&attr, &sentinel)
    {
        return true;
    }
    if let Ok(inspect_mod) = interp.load_module("inspect")
        && let Ok(icf) = interp.get_attr(&inspect_mod, "iscoroutinefunction")
        && let Ok(res) = call_pos(interp, icf, std::slice::from_ref(func))
        && let Ok(true) = interp.truthy_value(&res)
    {
        return true;
    }
    false
}

pub(crate) fn is_future_obj(interp: &mut Interpreter, obj: &Value) -> bool {
    if has_native_state::<FutureState>(obj) {
        return true;
    }
    let Ok(cls) = interp.get_attr(obj, "__class__") else {
        return false;
    };
    if interp.get_attr(&cls, "_asyncio_future_blocking").is_err() {
        return false;
    }
    match interp.get_attr(obj, "_asyncio_future_blocking") {
        Ok(v) => !v.is_none(),
        Err(_) => false,
    }
}

pub(crate) fn is_awaitable_obj(interp: &mut Interpreter, obj: &Value) -> bool {
    if is_coroutine_obj(obj) || is_future_obj(interp, obj) {
        return true;
    }
    if let ValueKind::PyInstance(inst_rc) = obj.kind() {
        let cls_val = Value::py_class(Rc::clone(&inst_rc.borrow().class));
        if interp.get_attr(&cls_val, "__await__").is_ok() {
            return true;
        }
    }
    false
}

/// Step either a bytecode `GeneratorFrame` coroutine or a native `_NativeCoro`.
pub(crate) fn step_coro_once(
    interp: &mut Interpreter,
    coro: &Value,
    sent_value: Value,
    inject_exc: Option<PyError>,
) -> Result<CoroStep> {
    if is_native_coro_obj(coro) {
        return awaitable::step_native_coro(interp, coro, sent_value, inject_exc);
    }
    interp.coro_step(coro, sent_value, inject_exc)
}

/// Check if a coroutine has already finished execution.
pub(crate) fn is_coro_done(coro: &Value) -> bool {
    if is_native_coro_obj(coro) {
        return with_native_state::<awaitable::NativeCoroState, _>(coro, "coroutine", |st| {
            Ok(st.done)
        })
        .unwrap_or(false);
    }
    if let ValueKind::Generator(state_rc) = coro.kind()
        && let Ok(b) = state_rc.try_borrow()
        && let Some(frame) = b.downcast_ref::<crate::interpreter::GeneratorFrame>()
        && frame.is_coroutine
        && frame.done
    {
        return true;
    }
    false
}

// ── Future & Task native state ───────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FutureStatus {
    Pending,
    Cancelled,
    Finished,
}

impl FutureStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Cancelled => "CANCELLED",
            Self::Finished => "FINISHED",
        }
    }
}

pub(crate) struct TaskInner {
    pub(crate) coro: Value,
    pub(crate) name: String,
    pub(crate) context: Value,
    pub(crate) num_cancels_requested: usize,
    pub(crate) fut_waiter: Option<Value>,
    pub(crate) must_cancel: bool,
}

pub(crate) struct FutureState {
    pub(crate) loop_val: Value,
    pub(crate) status: FutureStatus,
    pub(crate) result: Option<Value>,
    pub(crate) exception: Option<Value>,
    pub(crate) callbacks: Vec<(Value, Value)>, // (callback, context)
    pub(crate) cancel_message: Option<Value>,
    pub(crate) cancelled_exc: Option<Value>,
    pub(crate) future_blocking: bool,
    pub(crate) task: Option<TaskInner>,
    pub(crate) gathering_children: Option<Vec<Value>>,
    pub(crate) gathering_cancel_requested: bool,
}

impl FutureState {
    pub(crate) fn new_future(loop_val: Value) -> Self {
        Self {
            loop_val,
            status: FutureStatus::Pending,
            result: None,
            exception: None,
            callbacks: Vec::new(),
            cancel_message: None,
            cancelled_exc: None,
            future_blocking: false,
            task: None,
            gathering_children: None,
            gathering_cancel_requested: false,
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.status != FutureStatus::Pending
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.status == FutureStatus::Cancelled
    }
}

/// Create a fresh `asyncio.Future` bound to `loop_val`.
pub(crate) fn create_future_on_loop(interp: &mut Interpreter, loop_val: Value) -> Result<Value> {
    let inst = alloc_asyncio_instance(interp, "Future")?;
    set_native_state(&inst, FutureState::new_future(loop_val))?;
    Ok(inst)
}

/// Resolve a `Future` if it is still pending (no-op if already done).
pub(crate) fn future_set_result_if_pending(
    interp: &mut Interpreter,
    fut: &Value,
    val: Value,
) -> Result<()> {
    if has_native_state::<FutureState>(fut) {
        let pending = with_native_state::<FutureState, _>(fut, "Future", |st| Ok(!st.is_done()))?;
        if pending {
            future_set_result(interp, fut, val)?;
        }
        return Ok(());
    }
    let done = call_method(interp, fut, "done", &[])?;
    if !interp.truthy_value(&done)? {
        call_method(interp, fut, "set_result", &[val])?;
    }
    Ok(())
}

/// Set an exception on a `Future` if it is still pending.
pub(crate) fn future_set_exception_if_pending(
    interp: &mut Interpreter,
    fut: &Value,
    exc: Value,
) -> Result<()> {
    if has_native_state::<FutureState>(fut) {
        let pending = with_native_state::<FutureState, _>(fut, "Future", |st| Ok(!st.is_done()))?;
        if pending {
            future_set_exception(interp, fut, exc)?;
        }
        return Ok(());
    }
    let done = call_method(interp, fut, "done", &[])?;
    if !interp.truthy_value(&done)? {
        call_method(interp, fut, "set_exception", &[exc])?;
    }
    Ok(())
}

pub(crate) fn future_schedule_callbacks(interp: &mut Interpreter, fut: &Value) -> Result<()> {
    let (loop_val, cbs) = with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        let cbs = std::mem::take(&mut st.callbacks);
        Ok((st.loop_val.clone(), cbs))
    })?;
    for (cb, ctx) in cbs {
        loop_call_soon(interp, &loop_val, cb, vec![fut.clone()], Some(ctx))?;
    }
    Ok(())
}

pub(crate) fn future_set_result(
    interp: &mut Interpreter,
    fut: &Value,
    result: Value,
) -> Result<()> {
    with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        if st.status != FutureStatus::Pending {
            return Err(PyError::named("InvalidStateError", "invalid state"));
        }
        st.result = Some(result);
        st.status = FutureStatus::Finished;
        Ok(())
    })
    .map_err(|e| {
        if e.class_name_is("InvalidStateError") {
            raise_asyncio_exc(interp, "InvalidStateError", "invalid state")
        } else {
            e
        }
    })?;
    future_schedule_callbacks(interp, fut)
}

pub(crate) fn future_set_exception(
    interp: &mut Interpreter,
    fut: &Value,
    mut exception: Value,
) -> Result<()> {
    // If a class was passed (e.g. `set_exception(ValueError)`), instantiate it.
    if matches!(exception.kind(), ValueKind::PyClass(_)) {
        exception = call_pos(interp, exception, &[])?;
    }
    if let Some(si_cls) = lookup_exc_class("StopIteration")
        && let ValueKind::PyInstance(inst_rc) = exception.kind()
        && Rc::ptr_eq(&inst_rc.borrow().class, &si_cls)
    {
        return Err(PyError::named(
            "TypeError",
            "StopIteration interacts badly with generators and cannot be raised into a Future",
        ));
    }
    with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        if st.status != FutureStatus::Pending {
            return Err(PyError::named("InvalidStateError", "invalid state"));
        }
        st.exception = Some(exception);
        st.status = FutureStatus::Finished;
        Ok(())
    })
    .map_err(|e| {
        if e.class_name_is("InvalidStateError") {
            raise_asyncio_exc(interp, "InvalidStateError", "invalid state")
        } else {
            e
        }
    })?;
    future_schedule_callbacks(interp, fut)
}

pub(crate) fn future_make_cancelled_error(interp: &mut Interpreter, fut: &Value) -> Result<Value> {
    let (saved, msg) = with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        let saved = st.cancelled_exc.take();
        let msg = st.cancel_message.clone();
        Ok((saved, msg))
    })?;
    if let Some(exc) = saved {
        return Ok(exc);
    }
    make_cancelled_error_val(interp, msg.as_ref())
}

pub(crate) fn future_result(interp: &mut Interpreter, fut: &Value) -> Result<Value> {
    let (status, exc, res) = with_native_state::<FutureState, _>(fut, "Future", |st| {
        Ok((st.status, st.exception.clone(), st.result.clone()))
    })?;
    match status {
        FutureStatus::Cancelled => {
            let ce = future_make_cancelled_error(interp, fut)?;
            Err(PyError::raised(ce))
        }
        FutureStatus::Pending => Err(raise_asyncio_exc(
            interp,
            "InvalidStateError",
            "Result is not ready.",
        )),
        FutureStatus::Finished => {
            if let Some(e) = exc {
                Err(PyError::raised(e))
            } else {
                Ok(res.unwrap_or_else(Value::none))
            }
        }
    }
}

pub(crate) fn future_exception(interp: &mut Interpreter, fut: &Value) -> Result<Value> {
    let (status, exc) = with_native_state::<FutureState, _>(fut, "Future", |st| {
        Ok((st.status, st.exception.clone()))
    })?;
    match status {
        FutureStatus::Cancelled => {
            let ce = future_make_cancelled_error(interp, fut)?;
            Err(PyError::raised(ce))
        }
        FutureStatus::Pending => Err(raise_asyncio_exc(
            interp,
            "InvalidStateError",
            "Exception is not set.",
        )),
        FutureStatus::Finished => Ok(exc.unwrap_or_else(Value::none)),
    }
}

pub(crate) fn future_cancel(
    interp: &mut Interpreter,
    fut: &Value,
    msg: Option<Value>,
) -> Result<bool> {
    // Check if this is a `_GatheringFuture`.
    let gathering = with_native_state::<FutureState, _>(fut, "Future", |st| {
        Ok((st.is_done(), st.gathering_children.clone()))
    })?;
    if gathering.0 {
        return Ok(false);
    }
    if let Some(children) = gathering.1 {
        let mut ret = false;
        let msg_arg = msg.unwrap_or_else(Value::none);
        for child in children {
            let cancelled =
                call_method_kw(interp, &child, "cancel", &[], &[("msg", msg_arg.clone())])?;
            if interp.truthy_value(&cancelled)? {
                ret = true;
            }
        }
        if ret {
            let _ = with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
                st.gathering_cancel_requested = true;
                Ok(())
            });
        }
        return Ok(ret);
    }

    // Check if this is a Task.
    let is_task = with_native_state::<FutureState, _>(fut, "Future", |st| Ok(st.task.is_some()))?;
    if is_task {
        return task_cancel(interp, fut, msg);
    }

    with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        st.status = FutureStatus::Cancelled;
        st.cancel_message = msg;
        Ok(())
    })?;
    future_schedule_callbacks(interp, fut)?;
    Ok(true)
}

pub(crate) fn future_add_done_callback(
    interp: &mut Interpreter,
    fut: &Value,
    callback: Value,
    context: Option<Value>,
) -> Result<()> {
    let ctx = match context {
        Some(c) if !c.is_none() => c,
        _ => copy_context(interp)?,
    };
    let (done, loop_val) = with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
        if st.is_done() {
            Ok((true, st.loop_val.clone()))
        } else {
            st.callbacks.push((callback.clone(), ctx.clone()));
            Ok((false, st.loop_val.clone()))
        }
    })?;
    if done {
        loop_call_soon(interp, &loop_val, callback, vec![fut.clone()], Some(ctx))?;
    }
    Ok(())
}

pub(crate) fn future_remove_done_callback(
    interp: &mut Interpreter,
    fut: &Value,
    callback: &Value,
) -> Result<usize> {
    let existing =
        with_native_state::<FutureState, _>(fut, "Future", |st| Ok(st.callbacks.clone()))?;
    let mut kept = Vec::with_capacity(existing.len());
    let mut removed = 0usize;
    let op_eq = interp
        .load_module("operator")
        .and_then(|m| interp.get_attr(&m, "eq"))
        .ok();
    for (cb, ctx) in existing {
        let eq = match &op_eq {
            Some(eq_fn) => call_pos(interp, eq_fn.clone(), &[cb.clone(), callback.clone()])
                .and_then(|v| interp.truthy_value(&v))
                .unwrap_or_else(|_| same_object(&cb, callback)),
            None => same_object(&cb, callback),
        };
        if eq {
            removed += 1;
        } else {
            kept.push((cb, ctx));
        }
    }
    if removed > 0 {
        with_native_state_mut::<FutureState, _>(fut, "Future", |st| {
            st.callbacks = kept;
            Ok(())
        })?;
    }
    Ok(removed)
}

// ── Task execution engine ────────────────────────────────────────────────────

pub(crate) fn task_cancel(
    interp: &mut Interpreter,
    task: &Value,
    msg: Option<Value>,
) -> Result<bool> {
    let waiter = with_native_state_mut::<FutureState, _>(task, "Task", |st| {
        if st.is_done() {
            return Ok(None);
        }
        if let Some(ref mut t) = st.task {
            t.num_cancels_requested += 1;
            return Ok(Some(t.fut_waiter.clone()));
        }
        Ok(None)
    })?;
    let Some(opt_waiter) = waiter else {
        return Ok(false);
    };
    if let Some(w) = opt_waiter {
        let msg_arg = msg.clone().unwrap_or_else(Value::none);
        let res = call_method_kw(interp, &w, "cancel", &[], &[("msg", msg_arg)])?;
        if interp.truthy_value(&res)? {
            return Ok(true);
        }
    }
    with_native_state_mut::<FutureState, _>(task, "Task", |st| {
        if let Some(ref mut t) = st.task {
            t.must_cancel = true;
        }
        st.cancel_message = msg;
        Ok(())
    })?;
    Ok(true)
}

fn task_finish_result(interp: &mut Interpreter, task: &Value, value: Value) -> Result<()> {
    unregister_task(task);
    future_set_result(interp, task, value)
}

fn task_finish_exception(interp: &mut Interpreter, task: &Value, exc: Value) -> Result<()> {
    unregister_task(task);
    future_set_exception(interp, task, exc)
}

fn task_finish_cancel(interp: &mut Interpreter, task: &Value, msg: Option<Value>) -> Result<()> {
    unregister_task(task);
    with_native_state_mut::<FutureState, _>(task, "Task", |st| {
        st.status = FutureStatus::Cancelled;
        st.cancel_message = msg;
        Ok(())
    })?;
    future_schedule_callbacks(interp, task)
}

/// Initialize a `Task` instance (either regular or eager start).
pub(crate) fn init_task_instance(
    interp: &mut Interpreter,
    task: &Value,
    coro: Value,
    loop_val: Value,
    name: Option<String>,
    context: Option<Value>,
    eager_start: bool,
) -> Result<()> {
    if !is_coroutine_obj(&coro) {
        let r = render_value_repr(interp, &coro)?;
        return Err(PyError::named(
            "TypeError",
            format!("a coroutine was expected, got {r}"),
        ));
    }
    let ctx = match context {
        Some(c) if !c.is_none() => c,
        _ => copy_context(interp)?,
    };
    let default_name = next_task_name();
    let task_name = name.unwrap_or(default_name);
    let mut state = FutureState::new_future(loop_val.clone());
    state.task = Some(TaskInner {
        coro,
        name: task_name,
        context: ctx.clone(),
        num_cancels_requested: 0,
        fut_waiter: None,
        must_cancel: false,
    });
    set_native_state(task, state)?;

    if eager_start {
        // CPython 3.12 eager_start=True: swap current_task, execute first step
        // synchronously inside `ctx`, restore previous current_task, and only
        // register the task in `all_tasks` if it suspended (not yet done).
        let prev = swap_current_task(&loop_val, Some(task.clone()));
        let step_cb = interp.get_attr(task, "_step_run_eager")?;
        let res = run_in_context(interp, &ctx, &step_cb, &[]);
        swap_current_task(&loop_val, prev);
        res?;
        let done = with_native_state::<FutureState, _>(task, "Task", |st| Ok(st.is_done()))?;
        if !done {
            register_task(task.clone());
        }
    } else {
        register_task(task.clone());
        let step_cb = interp.get_attr(task, "_step_run")?;
        loop_call_soon(interp, &loop_val, step_cb, Vec::new(), Some(ctx))?;
    }
    Ok(())
}

/// One turn of `Task._step_run(exc=None)` (enters/leaves current_task).
pub(crate) fn task_step_run(
    interp: &mut Interpreter,
    task: &Value,
    exc: Option<Value>,
) -> Result<()> {
    let loop_val = with_native_state::<FutureState, _>(task, "Task", |st| Ok(st.loop_val.clone()))?;
    enter_task(interp, &loop_val, task)?;
    let res = task_step_run_inner(interp, task, exc);
    let leave_res = leave_task(interp, &loop_val, task);
    res?;
    leave_res
}

/// Core step logic used by both `_step_run` and `_step_run_eager` (where
/// `CURRENT_TASKS` has already been swapped).
pub(crate) fn task_step_run_inner(
    interp: &mut Interpreter,
    task: &Value,
    mut exc: Option<Value>,
) -> Result<()> {
    let (done, must_cancel) = with_native_state_mut::<FutureState, _>(task, "Task", |st| {
        if st.is_done() {
            return Ok((true, false));
        }
        let mc = st
            .task
            .as_mut()
            .map(|t| {
                t.fut_waiter = None;
                let prev = t.must_cancel;
                t.must_cancel = false;
                prev
            })
            .unwrap_or(false);
        Ok((false, mc))
    })?;
    if done {
        return Ok(());
    }
    if must_cancel {
        let already_cancelled = exc
            .as_ref()
            .is_some_and(|e| is_cancelled_error_val(interp, e));
        if !already_cancelled {
            exc = Some(future_make_cancelled_error(interp, task)?);
        }
    }
    task_step_run_and_handle_result(interp, task, exc)
}

fn task_step_run_and_handle_result(
    interp: &mut Interpreter,
    task: &Value,
    exc: Option<Value>,
) -> Result<()> {
    let (coro, loop_val, ctx) = with_native_state::<FutureState, _>(task, "Task", |st| {
        let t = st
            .task
            .as_ref()
            .ok_or_else(|| PyError::runtime("Task missing inner state".into()))?;
        Ok((t.coro.clone(), st.loop_val.clone(), t.context.clone()))
    })?;

    let step_res = match exc {
        None => step_coro_once(interp, &coro, Value::none(), None),
        Some(e) => step_coro_once(interp, &coro, Value::none(), Some(PyError::raised(e))),
    };

    match step_res {
        Err(err) => {
            let exc_val = pyerror_to_exc_value(interp, err)?;
            if is_cancelled_error_val(interp, &exc_val) {
                let msg = with_native_state_mut::<FutureState, _>(task, "Task", |st| {
                    st.cancelled_exc = Some(exc_val);
                    Ok(st.cancel_message.clone())
                })?;
                task_finish_cancel(interp, task, msg)?;
                return Ok(());
            }
            if is_base_fatal_error_val(interp, &exc_val) {
                task_finish_exception(interp, task, exc_val.clone())?;
                return Err(PyError::raised(exc_val));
            }
            task_finish_exception(interp, task, exc_val)?;
            Ok(())
        }
        Ok(CoroStep::Returned(val)) => {
            let (must_cancel, msg) = with_native_state_mut::<FutureState, _>(task, "Task", |st| {
                let mc = st
                    .task
                    .as_mut()
                    .map(|t| {
                        let prev = t.must_cancel;
                        t.must_cancel = false;
                        prev
                    })
                    .unwrap_or(false);
                Ok((mc, st.cancel_message.clone()))
            })?;
            if must_cancel {
                task_finish_cancel(interp, task, msg)?;
            } else {
                task_finish_result(interp, task, val)?;
            }
            Ok(())
        }
        Ok(CoroStep::Yielded(yielded)) => {
            if yielded.is_none() {
                // `yield None` (sleep(0) fairness): reschedule on next turn.
                let step_cb = interp.get_attr(task, "_step_run")?;
                loop_call_soon(interp, &loop_val, step_cb, Vec::new(), Some(ctx))?;
                return Ok(());
            }

            // Check `_asyncio_future_blocking` protocol (matches CPython Task.__step).
            let blocking_val = if has_native_state::<FutureState>(&yielded) {
                let b = with_native_state::<FutureState, _>(&yielded, "Future", |st| {
                    Ok(st.future_blocking)
                })?;
                Some(Value::bool_(b))
            } else {
                interp
                    .get_attr(&yielded, "_asyncio_future_blocking")
                    .ok()
                    .filter(|v| !v.is_none())
            };

            if let Some(bval) = blocking_val {
                let yielded_loop = if has_native_state::<FutureState>(&yielded) {
                    with_native_state::<FutureState, _>(&yielded, "Future", |st| {
                        Ok(st.loop_val.clone())
                    })?
                } else {
                    interp
                        .get_attr(&yielded, "_loop")
                        .unwrap_or_else(|_| Value::none())
                };
                if !same_object(&yielded_loop, &loop_val) {
                    let tr = render_value_repr(interp, task)?;
                    let yr = render_value_repr(interp, &yielded)?;
                    let rt_err = instantiate_exception(
                        lookup_exc_class("RuntimeError").expect("RuntimeError"),
                        vec![Value::string(format!(
                            "Task {tr} got Future {yr} attached to a different loop"
                        ))],
                    );
                    let step_cb = interp.get_attr(task, "_step_run")?;
                    loop_call_soon(interp, &loop_val, step_cb, vec![rt_err], Some(ctx))?;
                } else if interp.truthy_value(&bval)? {
                    if same_object(&yielded, task) {
                        let tr = render_value_repr(interp, task)?;
                        let rt_err = instantiate_exception(
                            lookup_exc_class("RuntimeError").expect("RuntimeError"),
                            vec![Value::string(format!("Task cannot await on itself: {tr}"))],
                        );
                        let step_cb = interp.get_attr(task, "_step_run")?;
                        loop_call_soon(interp, &loop_val, step_cb, vec![rt_err], Some(ctx))?;
                    } else {
                        if has_native_state::<FutureState>(&yielded) {
                            let _ =
                                with_native_state_mut::<FutureState, _>(&yielded, "Future", |st| {
                                    st.future_blocking = false;
                                    Ok(())
                                });
                        } else {
                            let _ = interp.assign_attr(
                                yielded.clone(),
                                "_asyncio_future_blocking",
                                Value::bool_(false),
                            );
                        }
                        let (must_cancel, msg) =
                            with_native_state_mut::<FutureState, _>(task, "Task", |st| {
                                let mut mc = false;
                                if let Some(ref mut t) = st.task {
                                    t.fut_waiter = Some(yielded.clone());
                                    mc = t.must_cancel;
                                }
                                Ok((mc, st.cancel_message.clone()))
                            })?;
                        let wakeup_cb = interp.get_attr(task, "_wakeup")?;
                        if has_native_state::<FutureState>(&yielded) {
                            future_add_done_callback(interp, &yielded, wakeup_cb, Some(ctx))?;
                        } else {
                            call_method_kw(
                                interp,
                                &yielded,
                                "add_done_callback",
                                &[wakeup_cb],
                                &[("context", ctx)],
                            )?;
                        }
                        if must_cancel {
                            let msg_arg = msg.unwrap_or_else(Value::none);
                            let res = call_method_kw(
                                interp,
                                &yielded,
                                "cancel",
                                &[],
                                &[("msg", msg_arg)],
                            )?;
                            if interp.truthy_value(&res)? {
                                let _ =
                                    with_native_state_mut::<FutureState, _>(task, "Task", |st| {
                                        if let Some(ref mut t) = st.task {
                                            t.must_cancel = false;
                                        }
                                        Ok(())
                                    });
                            }
                        }
                    }
                } else {
                    let tr = render_value_repr(interp, task)?;
                    let yr = render_value_repr(interp, &yielded)?;
                    let rt_err = instantiate_exception(
                        lookup_exc_class("RuntimeError").expect("RuntimeError"),
                        vec![Value::string(format!(
                            "yield was used instead of yield from in task {tr} with {yr}"
                        ))],
                    );
                    let step_cb = interp.get_attr(task, "_step_run")?;
                    loop_call_soon(interp, &loop_val, step_cb, vec![rt_err], Some(ctx))?;
                }
            } else {
                let yr = render_value_repr(interp, &yielded)?;
                let rt_err = instantiate_exception(
                    lookup_exc_class("RuntimeError").expect("RuntimeError"),
                    vec![Value::string(format!("Task got bad yield: {yr}"))],
                );
                let step_cb = interp.get_attr(task, "_step_run")?;
                loop_call_soon(interp, &loop_val, step_cb, vec![rt_err], Some(ctx))?;
            }
            Ok(())
        }
    }
}

pub(crate) fn task_wakeup(interp: &mut Interpreter, task: &Value, future: &Value) -> Result<()> {
    let _ = with_native_state_mut::<FutureState, _>(task, "Task", |st| {
        if let Some(ref mut t) = st.task {
            t.fut_waiter = None;
        }
        Ok(())
    });
    let res = if has_native_state::<FutureState>(future) {
        future_result(interp, future)
    } else {
        call_method(interp, future, "result", &[])
    };
    match res {
        Ok(_) => task_step_run(interp, task, None),
        Err(err) => {
            let exc_val = pyerror_to_exc_value(interp, err)?;
            task_step_run(interp, task, Some(exc_val))
        }
    }
}

// ── Loop dispatch helpers (works for native & custom Python loops) ───────────

pub(crate) fn loop_call_soon(
    interp: &mut Interpreter,
    loop_val: &Value,
    callback: Value,
    args: Vec<Value>,
    context: Option<Value>,
) -> Result<Value> {
    if has_native_state::<io_engine::LoopState>(loop_val) {
        return io_engine::loop_call_soon_native(interp, loop_val, callback, args, context);
    }
    let mut pos = Vec::with_capacity(1 + args.len());
    pos.push(callback);
    pos.extend(args);
    match context {
        Some(c) => call_method_kw(interp, loop_val, "call_soon", &pos, &[("context", c)]),
        None => call_method(interp, loop_val, "call_soon", &pos),
    }
}

pub(crate) fn loop_call_at(
    interp: &mut Interpreter,
    loop_val: &Value,
    when: f64,
    callback: Value,
    args: Vec<Value>,
    context: Option<Value>,
) -> Result<Value> {
    if has_native_state::<io_engine::LoopState>(loop_val) {
        return io_engine::loop_call_at_native(interp, loop_val, when, callback, args, context);
    }
    let mut pos = Vec::with_capacity(2 + args.len());
    pos.push(Value::float(when));
    pos.push(callback);
    pos.extend(args);
    match context {
        Some(c) => call_method_kw(interp, loop_val, "call_at", &pos, &[("context", c)]),
        None => call_method(interp, loop_val, "call_at", &pos),
    }
}

pub(crate) fn loop_call_later(
    interp: &mut Interpreter,
    loop_val: &Value,
    delay: f64,
    callback: Value,
    args: Vec<Value>,
    context: Option<Value>,
) -> Result<Value> {
    let now = loop_time(interp, loop_val)?;
    loop_call_at(interp, loop_val, now + delay, callback, args, context)
}

pub(crate) fn loop_time(interp: &mut Interpreter, loop_val: &Value) -> Result<f64> {
    if has_native_state::<io_engine::LoopState>(loop_val) {
        return Ok(monotonic_secs());
    }
    let v = call_method(interp, loop_val, "time", &[])?;
    match v.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(i) => Ok(i as f64),
        _ => Ok(monotonic_secs()),
    }
}

pub(crate) fn loop_create_future(interp: &mut Interpreter, loop_val: &Value) -> Result<Value> {
    if has_native_state::<io_engine::LoopState>(loop_val) {
        return create_future_on_loop(interp, loop_val.clone());
    }
    call_method(interp, loop_val, "create_future", &[])
}

pub(crate) fn loop_create_task(
    interp: &mut Interpreter,
    loop_val: &Value,
    coro: Value,
    name: Option<String>,
    context: Option<Value>,
) -> Result<Value> {
    if has_native_state::<io_engine::LoopState>(loop_val) {
        return io_engine::loop_create_task_native(interp, loop_val, coro, name, context);
    }
    let mut kw: Vec<(&str, Value)> = Vec::new();
    if let Some(ref n) = name {
        kw.push(("name", Value::string(n.clone())));
    }
    if let Some(c) = context {
        kw.push(("context", c));
    }
    if kw.is_empty() {
        call_method(interp, loop_val, "create_task", &[coro])
    } else {
        call_method_kw(interp, loop_val, "create_task", &[coro], &kw)
    }
}

/// `ensure_future(coro_or_future, *, loop=None)` implementation.
pub(crate) fn ensure_future_impl(
    interp: &mut Interpreter,
    coro_or_future: Value,
    loop_opt: Option<Value>,
) -> Result<Value> {
    if is_future_obj(interp, &coro_or_future) {
        if let Some(ref lp) = loop_opt
            && !lp.is_none()
        {
            let fut_loop = if has_native_state::<FutureState>(&coro_or_future) {
                with_native_state::<FutureState, _>(&coro_or_future, "Future", |st| {
                    Ok(st.loop_val.clone())
                })?
            } else {
                call_method(interp, &coro_or_future, "get_loop", &[])?
            };
            if !same_object(lp, &fut_loop) {
                return Err(PyError::named(
                    "ValueError",
                    "The future belongs to a different loop than the one specified as the loop argument",
                ));
            }
        }
        return Ok(coro_or_future);
    }

    let coro = if is_coroutine_obj(&coro_or_future) {
        coro_or_future
    } else if is_awaitable_obj(interp, &coro_or_future) {
        awaitable::make_native_coro(
            interp,
            awaitable::CoroBody::WrapAwaitable {
                awaitable: coro_or_future,
                iter: None,
            },
        )?
    } else {
        return Err(PyError::named(
            "TypeError",
            "An asyncio.Future, a coroutine or an awaitable is required",
        ));
    };

    let lp = match loop_opt {
        Some(v) if !v.is_none() => v,
        _ => get_event_loop_compat(interp)?,
    };
    loop_create_task(interp, &lp, coro, None, None)
}

/// `get_event_loop()` matching CPython 3.12 semantics: returns running loop if
/// active, otherwise delegates to `get_event_loop_policy().get_event_loop()`.
pub(crate) fn get_event_loop_compat(interp: &mut Interpreter) -> Result<Value> {
    if let Some(running) = get_running_loop_opt() {
        return Ok(running);
    }
    let policy = get_event_loop_policy(interp)?;
    call_method(interp, &policy, "get_event_loop", &[])
}

// ── Shared Sync / Queue / Timeout / TaskGroup / Stream / Process states ──────

pub(crate) struct LockState {
    pub(crate) loop_val: Option<Value>,
    pub(crate) locked: bool,
    pub(crate) waiters: VecDeque<Value>,
}

pub(crate) struct EventState {
    pub(crate) loop_val: Option<Value>,
    pub(crate) value: bool,
    pub(crate) waiters: VecDeque<Value>,
}

pub(crate) struct ConditionState {
    pub(crate) loop_val: Option<Value>,
    pub(crate) lock: Value,
    pub(crate) waiters: VecDeque<Value>,
}

pub(crate) struct SemaphoreState {
    pub(crate) loop_val: Option<Value>,
    pub(crate) value: i64,
    pub(crate) bound_value: Option<i64>,
    pub(crate) waiters: VecDeque<Value>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BarrierStatus {
    Filling,
    Draining,
    Resetting,
    Broken,
}

impl BarrierStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Filling => "filling",
            Self::Draining => "draining",
            Self::Resetting => "resetting",
            Self::Broken => "broken",
        }
    }
}

pub(crate) struct BarrierState {
    pub(crate) cond: Value,
    pub(crate) parties: i64,
    pub(crate) status: BarrierStatus,
    pub(crate) count: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueKind {
    Fifo,
    Lifo,
    Priority,
}

pub(crate) struct QueueState {
    pub(crate) kind: QueueKind,
    pub(crate) loop_val: Option<Value>,
    pub(crate) maxsize: i64,
    pub(crate) items: VecDeque<Value>,
    pub(crate) getters: VecDeque<Value>,
    pub(crate) putters: VecDeque<Value>,
    pub(crate) unfinished_tasks: i64,
    pub(crate) finished: Value, // asyncio.Event
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimeoutStatus {
    Created,
    Entered,
    Expiring,
    Expired,
    Exited,
}

impl TimeoutStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Entered => "active",
            Self::Expiring => "expiring",
            Self::Expired => "expired",
            Self::Exited => "finished",
        }
    }
}

pub(crate) struct TimeoutState {
    pub(crate) status: TimeoutStatus,
    pub(crate) timeout_handler: Option<Value>,
    pub(crate) task: Option<Value>,
    pub(crate) when: Option<f64>,
    pub(crate) cancelling_at_enter: usize,
}

pub(crate) struct TaskGroupState {
    pub(crate) entered: bool,
    pub(crate) exiting: bool,
    pub(crate) aborting: bool,
    pub(crate) loop_val: Option<Value>,
    pub(crate) parent_task: Option<Value>,
    pub(crate) parent_cancel_requested: bool,
    pub(crate) tasks: Vec<Value>,
    pub(crate) errors: Option<Vec<Value>>,
    pub(crate) base_error: Option<Value>,
    pub(crate) on_completed_fut: Option<Value>,
}

pub(crate) struct StreamReaderState {
    pub(crate) limit: usize,
    pub(crate) loop_val: Option<Value>,
    pub(crate) buffer: Vec<u8>,
    pub(crate) eof: bool,
    pub(crate) waiter: Option<Value>,
    pub(crate) exception: Option<Value>,
    pub(crate) transport: Option<Value>,
    pub(crate) paused: bool,
}

pub(crate) struct StreamWriterState {
    pub(crate) transport: Value,
    pub(crate) protocol: Value,
    pub(crate) reader: Option<Value>,
    pub(crate) loop_val: Value,
}

pub(crate) struct StreamReaderProtocolState {
    pub(crate) stream_reader: Option<Value>,
    pub(crate) client_connected_cb: Option<Value>,
    pub(crate) loop_val: Value,
    pub(crate) over_ssl: bool,
    pub(crate) transport: Option<Value>,
    pub(crate) paused: bool,
    pub(crate) drain_waiter: Option<Value>,
    pub(crate) connection_lost: bool,
    pub(crate) closed_fut: Option<Value>,
    pub(crate) client_handler_task: Option<Value>,
    pub(crate) stream_writer: Option<Value>,
}

pub(crate) struct RunnerState {
    pub(crate) debug: Option<bool>,
    pub(crate) loop_factory: Option<Value>,
    pub(crate) loop_val: Option<Value>,
    pub(crate) context: Option<Value>,
    pub(crate) state: RunnerLifecycle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunnerLifecycle {
    Created,
    Initialized,
    Closed,
}

pub(crate) struct ProcessState {
    pub(crate) transport: Value,
    pub(crate) protocol: Value,
    pub(crate) loop_val: Value,
    pub(crate) stdin: Option<Value>,
    pub(crate) stdout: Option<Value>,
    pub(crate) stderr: Option<Value>,
    pub(crate) pid: i64,
}

pub(crate) struct SubprocessStreamProtocolState {
    pub(crate) limit: usize,
    pub(crate) loop_val: Value,
    pub(crate) stdin: Option<Value>,
    pub(crate) stdout: Option<Value>,
    pub(crate) stderr: Option<Value>,
    pub(crate) transport: Option<Value>,
    pub(crate) exited_fut: Value,
    pub(crate) pipe_closed_count: usize,
    pub(crate) expected_pipes: usize,
    pub(crate) process_exited: bool,
    pub(crate) connection_lost: bool,
}

/// Bind a `_LoopBoundMixin`-style object lazily to the currently running loop.
pub(crate) fn bind_or_check_loop(
    interp: &mut Interpreter,
    self_val: &Value,
    stored: &mut Option<Value>,
) -> Result<Value> {
    let loop_val = get_running_loop_or_err()?;
    if stored.is_none() {
        *stored = Some(loop_val.clone());
    }
    if let Some(ref bound) = *stored
        && !same_object(bound, &loop_val)
    {
        let sr = render_value_repr(interp, self_val)?;
        return Err(PyError::named(
            "RuntimeError",
            format!("{sr} is bound to a different event loop"),
        ));
    }
    Ok(loop_val)
}

/// Helper to build a Python `dict` from `(&str, Value)` pairs.
pub(crate) fn make_py_dict(entries: &[(&str, Value)]) -> Value {
    let mut d = PyDict::default();
    for (k, v) in entries {
        d.insert(PyKey::str_from(k), v.clone());
    }
    Value::dict(d)
}

/// Extract numeric float seconds from int/float/bool.
pub(crate) fn value_to_f64(val: &Value, fn_name: &str, arg_name: &str) -> Result<f64> {
    match val.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(i) => Ok(i as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}(): {arg_name} must be a real number, not {}",
                value_type_name_str(val)
            ),
        )),
    }
}

/// Extract non-negative integer file descriptor from an `int` or object with `.fileno()`.
pub(crate) fn extract_fd(interp: &mut Interpreter, obj: &Value) -> Result<i64> {
    match obj.kind() {
        ValueKind::Int(n) => {
            if n < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("Invalid file descriptor: {n}"),
                ));
            }
            Ok(n)
        }
        _ => {
            let m = interp.get_attr(obj, "fileno")?;
            let fd_val = call_pos(interp, m, &[])?;
            match fd_val.kind() {
                ValueKind::Int(n) if n >= 0 => Ok(n),
                ValueKind::Int(n) => Err(PyError::named(
                    "ValueError",
                    format!("Invalid file descriptor: {n}"),
                )),
                _ => Err(PyError::named(
                    "TypeError",
                    "fileno() returned a non-integer",
                )),
            }
        }
    }
}
