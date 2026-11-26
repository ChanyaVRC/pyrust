// `concurrent.futures.thread` — 100% native-Rust Python 3.12 `ThreadPoolExecutor`
// (issue #3263).
//
// Backed by real `threading.Thread` worker OS threads and GIL-releasing
// condition-variable work queues.
//
// Reference: <https://docs.python.org/3.12/library/concurrent.futures.html#threadpoolexecutor>

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::bodies_helpers_cf_base as cf_base;
use crate::builtin_modules::macros::link_class_bases;
use crate::builtin_modules::macros::native_state::set_native_state;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, current_thread_is_daemon_worker};
use crate::value::{PyModule, Value, ValueKind};

static THREAD_POOL_COUNTER: AtomicUsize = AtomicUsize::new(0);
const WORKER_POLL_SLICE: Duration = Duration::from_millis(20);

struct ThreadPoolQueue {
    pending_ids: VecDeque<u64>,
    shutdown: bool,
    broken: Option<String>,
    idle_workers: usize,
    num_workers: usize,
}

struct ThreadPoolShared {
    queue: Mutex<ThreadPoolQueue>,
    cvar: Condvar,
}

struct WorkItem {
    future: Value,
    func: Value,
    args: Vec<ExpandedCallArg>,
}

struct ThreadPoolPyState {
    next_id: u64,
    items: HashMap<u64, WorkItem>,
    threads: Vec<Value>,
    max_workers: usize,
    thread_name_prefix: String,
    initializer: Option<Value>,
    initargs: Vec<Value>,
}

struct ThreadPoolState {
    shared: Arc<ThreadPoolShared>,
    py: Rc<RefCell<ThreadPoolPyState>>,
}

struct WorkerTargetState {
    shared: Arc<ThreadPoolShared>,
    py: Rc<RefCell<ThreadPoolPyState>>,
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn make_broken_thread_pool_exc(interp: &mut Interpreter, msg: &str) -> Result<Value> {
    let m = interp.load_module("concurrent.futures.thread")?;
    let cls = interp.get_attr(&m, "BrokenThreadPool")?;
    call_pos(interp, cls, &[Value::string(msg)])
}

fn raise_broken_thread_pool(interp: &mut Interpreter, msg: &str) -> PyError {
    match make_broken_thread_pool_exc(interp, msg) {
        Ok(inst) => PyError::raised(inst),
        Err(e) => e,
    }
}

fn default_max_workers() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    (cpus + 4).min(32)
}

fn fail_all_pending(
    interp: &mut Interpreter,
    shared: &Arc<ThreadPoolShared>,
    py: &Rc<RefCell<ThreadPoolPyState>>,
    msg: &str,
) {
    let remaining_ids: Vec<u64> = {
        let mut q = shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        q.pending_ids.drain(..).collect()
    };
    let mut items = Vec::with_capacity(remaining_ids.len());
    {
        let mut p = py.borrow_mut();
        for id in remaining_ids {
            if let Some(it) = p.items.remove(&id) {
                items.push(it);
            }
        }
    }
    for it in items {
        if let Ok(exc) = make_broken_thread_pool_exc(interp, msg) {
            let _ = cf_base::future_set_exception(interp, &it.future, exc);
        }
    }
}

fn run_worker_loop(
    interp: &mut Interpreter,
    shared: Arc<ThreadPoolShared>,
    py: Rc<RefCell<ThreadPoolPyState>>,
) -> Result<()> {
    // 1. Run optional initializer(*initargs).
    let init_opt = {
        let p = py.borrow();
        p.initializer.clone().map(|f| (f, p.initargs.clone()))
    };
    if let Some((init_fn, init_args)) = init_opt
        && let Err(_err) = call_pos(interp, init_fn, &init_args)
    {
        let reason = "A thread initializer failed, the thread pool is not usable anymore";
        {
            let mut q = shared
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            q.broken = Some(reason.to_string());
            shared.cvar.notify_all();
        }
        fail_all_pending(interp, &shared, &py, reason);
        return Ok(());
    }

    // 2. Main work loop.
    let gil = Arc::clone(&interp.cold.gil_state);
    loop {
        let next_id: Option<u64> = interp.release_gil_during(|| {
            let mut q = shared
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            q.idle_workers += 1;
            let result = loop {
                if q.broken.is_some() {
                    break None;
                }
                if let Some(id) = q.pending_ids.pop_front() {
                    break Some(id);
                }
                if q.shutdown {
                    break None;
                }
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    break None;
                }
                let (next_q, _) = shared
                    .cvar
                    .wait_timeout(q, WORKER_POLL_SLICE)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                q = next_q;
            };
            q.idle_workers = q.idle_workers.saturating_sub(1);
            result
        });

        let Some(id) = next_id else {
            break;
        };

        let item = py.borrow_mut().items.remove(&id);
        let Some(item) = item else {
            continue;
        };

        if !cf_base::future_try_set_running(&item.future).unwrap_or(false) {
            continue;
        }

        match interp.call_function_expanded(item.func, &item.args) {
            Ok(res) => {
                let _ = cf_base::future_set_result(interp, &item.future, res);
            }
            Err(err) => {
                let exc_val = interp
                    .materialize_pyerror(err)
                    .unwrap_or_else(|e| Value::string(e.to_string()));
                let _ = cf_base::future_set_exception(interp, &item.future, exc_val);
            }
        }
    }

    Ok(())
}

fn adjust_thread_count(
    interp: &mut Interpreter,
    shared: &Arc<ThreadPoolShared>,
    py: &Rc<RefCell<ThreadPoolPyState>>,
) -> Result<()> {
    let (should_spawn, thread_name) = {
        let mut q = shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let p = py.borrow();
        if q.idle_workers > 0 || q.num_workers >= p.max_workers {
            (false, String::new())
        } else {
            let idx = q.num_workers;
            q.num_workers += 1;
            let name = format!("{}_{}", p.thread_name_prefix, idx);
            (true, name)
        }
    };
    if !should_spawn {
        return Ok(());
    }

    let thread_mod = interp.load_module("concurrent.futures.thread")?;
    let worker_cls = interp.get_attr(&thread_mod, "_ThreadPoolWorker")?;
    let worker_obj = call_pos(interp, worker_cls, &[])?;
    set_native_state(
        &worker_obj,
        WorkerTargetState {
            shared: Arc::clone(shared),
            py: Rc::clone(py),
        },
    )?;

    let threading_mod = interp.load_module("threading")?;
    let th_cls = interp.get_attr(&threading_mod, "Thread")?;
    let th_obj = interp.call_function_expanded(
        th_cls,
        &[
            ExpandedCallArg {
                name: Some("name".to_string()),
                value: Value::string(thread_name),
            },
            ExpandedCallArg {
                name: Some("target".to_string()),
                value: worker_obj,
            },
            ExpandedCallArg {
                name: Some("daemon".to_string()),
                value: Value::bool_(true),
            },
        ],
    )?;
    let start_fn = interp.get_attr(&th_obj, "start")?;
    call_pos(interp, start_fn, &[])?;
    py.borrow_mut().threads.push(th_obj);
    Ok(())
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let base_mod = interp.load_module("concurrent.futures._base")?;
    let broken_base = interp.get_attr(&base_mod, "BrokenExecutor")?;
    let exec_base = interp.get_attr(&base_mod, "Executor")?;

    let broken_cls = module.borrow().get_attr_value("BrokenThreadPool");
    if let Some(val) = broken_cls
        && let ValueKind::PyClass(cls_rc) = val.kind()
        && let ValueKind::PyClass(base_rc) = broken_base.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(base_rc)]);
    }

    let pool_cls = module.borrow().get_attr_value("ThreadPoolExecutor");
    if let Some(val) = pool_cls
        && let ValueKind::PyClass(cls_rc) = val.kind()
        && let ValueKind::PyClass(base_rc) = exec_base.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(base_rc)]);
    }
    Ok(())
}

pyrust_module! {
    /// Raised when a worker thread in a ThreadPoolExecutor fails initialization.
    exception BrokenThreadPool(RuntimeError);

    #[native_state(WorkerTargetState)]
    class _ThreadPoolWorker {
        fn __call__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            run_worker_loop(_interp, shared, py)?;
            Ok(Value::none())
        }
    }

    #[native_state(ThreadPoolState)]
    class ThreadPoolExecutor {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] max_workers: OptValueRef<'_>,
            #[default("".into())] thread_name_prefix: PyStr<'_>,
            #[default(OptionalArg::Omitted)] initializer: OptValueRef<'_>,
            #[default(PyValue(Value::tuple(Vec::new())))] initargs: PyValue,
        ) -> Result<Value> {
            let max_w = match max_workers.as_value() {
                None => default_max_workers(),
                Some(v) if v.is_none() => default_max_workers(),
                Some(v) => {
                    let idx = PyIndex::try_from_value_with_interp(
                        _interp,
                        v,
                        "ThreadPoolExecutor",
                        "max_workers",
                    )?;
                    let n = idx.expect_i64("ThreadPoolExecutor", "max_workers")?;
                    if n <= 0 {
                        return Err(PyError::named(
                            "ValueError",
                            "max_workers must be greater than 0",
                        ));
                    }
                    n as usize
                }
            };

            let init_fn = match initializer.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => {
                    let is_callable = _interp.call_function_expanded(
                        Value::builtin_function("callable"),
                        &[ExpandedCallArg {
                            name: None,
                            value: v.clone(),
                        }],
                    )?;
                    if !_interp.truthy_value(&is_callable)? {
                        return Err(PyError::named(
                            "TypeError",
                            "initializer must be a callable",
                        ));
                    }
                    Some(v.clone())
                }
            };

            let init_args = _interp.collect_iterable(&initargs.0)?;

            let prefix = if thread_name_prefix.is_empty() {
                let id = THREAD_POOL_COUNTER.fetch_add(1, Ordering::Relaxed);
                format!("ThreadPoolExecutor-{id}")
            } else {
                thread_name_prefix.to_string()
            };

            let state = ThreadPoolState {
                shared: Arc::new(ThreadPoolShared {
                    queue: Mutex::new(ThreadPoolQueue {
                        pending_ids: VecDeque::new(),
                        shutdown: false,
                        broken: None,
                        idle_workers: 0,
                        num_workers: 0,
                    }),
                    cvar: Condvar::new(),
                }),
                py: Rc::new(RefCell::new(ThreadPoolPyState {
                    next_id: 1,
                    items: HashMap::new(),
                    threads: Vec::new(),
                    max_workers: max_w,
                    thread_name_prefix: prefix,
                    initializer: init_fn,
                    initargs: init_args,
                })),
            };
            set_state(self_val.0, state)?;
            Ok(Value::none())
        }

        #[getter]
        fn _max_workers(#[state] st: &ThreadPoolState) -> Result<Value> {
            Ok(Value::int(st.py.borrow().max_workers as i64))
        }

        #[getter]
        fn _thread_name_prefix(#[state] st: &ThreadPoolState) -> Result<Value> {
            Ok(Value::string(st.py.borrow().thread_name_prefix.clone()))
        }

        #[getter]
        fn _threads(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let threads = with_state(self_val.0, |st| Ok(st.py.borrow().threads.clone()))?;
            call_pos(
                _interp,
                Value::builtin_function("set"),
                &[Value::list(threads)],
            )
        }

        fn submit(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] func: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;

            {
                let q = shared
                    .queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(ref reason) = q.broken {
                    return Err(raise_broken_thread_pool(_interp, reason));
                }
                if q.shutdown {
                    return Err(PyError::named(
                        "RuntimeError",
                        "cannot schedule new futures after shutdown",
                    ));
                }
            }

            let fut = cf_base::new_future_instance(_interp)?;
            let mut call_args = Vec::with_capacity(args.len() + kwargs.len());
            for v in args {
                call_args.push(ExpandedCallArg {
                    name: None,
                    value: v.clone(),
                });
            }
            for (k, v) in kwargs {
                call_args.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }

            let item_id = {
                let mut p = py.borrow_mut();
                let id = p.next_id;
                p.next_id = p.next_id.wrapping_add(1);
                p.items.insert(
                    id,
                    WorkItem {
                        future: fut.clone(),
                        func: func.0.clone(),
                        args: call_args,
                    },
                );
                id
            };

            {
                let mut q = shared
                    .queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                q.pending_ids.push_back(item_id);
                shared.cvar.notify_one();
            }

            adjust_thread_count(_interp, &shared, &py)?;
            Ok(fut)
        }

        fn shutdown(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(true.into())] wait: PyBool,
            #[keyword_only] #[default(false.into())] cancel_futures: PyBool,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;

            let cancelled_ids: Vec<u64> = {
                let mut q = shared
                    .queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                q.shutdown = true;
                let ids = if cancel_futures.0 {
                    q.pending_ids.drain(..).collect()
                } else {
                    Vec::new()
                };
                shared.cvar.notify_all();
                ids
            };

            if !cancelled_ids.is_empty() {
                let mut to_cancel = Vec::with_capacity(cancelled_ids.len());
                {
                    let mut p = py.borrow_mut();
                    for id in cancelled_ids {
                        if let Some(it) = p.items.remove(&id) {
                            to_cancel.push(it.future);
                        }
                    }
                }
                for fut in to_cancel {
                    if cf_base::future_cancel_internal(_interp, &fut).unwrap_or(false) {
                        let _ = cf_base::future_set_running_or_notify_cancel_internal(_interp, &fut);
                    }
                }
            }

            if wait.0 {
                let threads = py.borrow().threads.clone();
                let threading_mod = _interp.load_module("threading")?;
                let cur_fn = _interp.get_attr(&threading_mod, "current_thread")?;
                let cur_th = call_pos(_interp, cur_fn, &[])?;
                for th in threads {
                    let is_self = match (th.kind(), cur_th.kind()) {
                        (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) => Rc::ptr_eq(a, b),
                        _ => false,
                    };
                    if !is_self {
                        let join_fn = _interp.get_attr(&th, "join")?;
                        let _ = call_pos(_interp, join_fn, &[])?;
                    }
                }
            }

            Ok(Value::none())
        }
    }
}
