// `multiprocessing.pool` — 100% native-Rust Python 3.12 process and thread
// worker pools (`Pool`, `ThreadPool`, `AsyncResult`, `ApplyResult`,
// `MapResult`, `IMapIterator`, `IMapUnorderedIterator`, `MaybeEncodingError`,
// `TimeoutError`) (issue #3264).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::builtin_modules::mp_rt::{
    SHUTDOWN_POLL_SLICE, call_pos, call_pos_kw, cpu_count_value, get_executable_path,
    parse_timeout, pickle_dumps, pickle_loads, raise_context_error,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBytesLike, PyIndex, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, current_thread_is_daemon_worker};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, PyModule, Value, ValueKind};

const POOL_WORKER_BOOTSTRAP: &str = r#"
import io, os, pickle, sys, types
from multiprocessing.pool import _pool_worker_recv, _pool_worker_send

sys.stdout = sys.stderr

_mp_main_mod = None

class _PoolUnpickler(pickle.Unpickler):
    def find_class(self, module, name):
        if module == "__main__" and _mp_main_mod is not None:
            return getattr(_mp_main_mod, name)
        return super().find_class(module, name)

def _loads(data):
    return _PoolUnpickler(io.BytesIO(data)).load()

def _dumps_reply(task_id, ok, payload):
    try:
        return pickle.dumps((task_id, ok, payload))
    except Exception:
        if ok:
            return pickle.dumps((task_id, False, TypeError(f"cannot pickle result of type {type(payload).__name__}")))
        return pickle.dumps((task_id, False, RuntimeError(f"{type(payload).__name__}: {payload}")))

_cfg_bytes = _pool_worker_recv()
if _cfg_bytes is None:
    sys.exit(0)

_sys_path, _main_file, _cwd, _init_bytes, _max_tasks = _loads(_cfg_bytes)
if _cwd:
    try:
        os.chdir(_cwd)
    except OSError:
        pass
for _p in reversed(_sys_path):
    if _p not in sys.path:
        sys.path.insert(0, _p)

if _main_file and os.path.isfile(_main_file):
    try:
        _mp_main_mod = types.ModuleType("__mp_main__")
        _mp_main_mod.__file__ = _main_file
        sys.modules["__mp_main__"] = _mp_main_mod
        with open(_main_file, "r", encoding="utf-8") as _f:
            _src = _f.read()
        exec(compile(_src, _main_file, "exec"), _mp_main_mod.__dict__)
    except BaseException as _exc:
        _pool_worker_send(_dumps_reply(0, False, _exc))
        sys.exit(1)

if _init_bytes is not None:
    try:
        _init_fn, _init_args = _loads(_init_bytes)
        _init_fn(*_init_args)
    except BaseException as _exc:
        _pool_worker_send(_dumps_reply(0, False, _exc))
        sys.exit(1)

_pool_worker_send(_dumps_reply(0, True, None))

_done = 0
while True:
    _frame = _pool_worker_recv()
    if _frame is None:
        break
    try:
        _tid, _fn, _args, _kwargs = _loads(_frame)
    except BaseException as _exc:
        _pool_worker_send(_dumps_reply(0, False, _exc))
        break
    try:
        _res = _fn(*_args, **_kwargs)
        _pool_worker_send(_dumps_reply(_tid, True, _res))
    except BaseException as _exc:
        _pool_worker_send(_dumps_reply(_tid, False, _exc))
    _done += 1
    if _max_tasks is not None and _done >= _max_tasks:
        break
"#;

// ── AsyncResult / ApplyResult / MapResult ────────────────────────────────────

struct AsyncResultWaiter {
    done: Mutex<bool>,
    cvar: Condvar,
}

struct AsyncResultPy {
    ready: bool,
    success: bool,
    value: Option<Value>,
    callback: Option<Value>,
    error_callback: Option<Value>,
}

struct AsyncResultState {
    waiter: Arc<AsyncResultWaiter>,
    py: Rc<RefCell<AsyncResultPy>>,
}

fn new_async_result(
    interp: &mut Interpreter,
    cls_name: &str,
    callback: Option<Value>,
    error_callback: Option<Value>,
) -> Result<Value> {
    let m = interp.load_module("multiprocessing.pool")?;
    let cls = interp.get_attr(&m, cls_name)?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named("RuntimeError", "AsyncResult class invalid"));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        AsyncResultState {
            waiter: Arc::new(AsyncResultWaiter {
                done: Mutex::new(false),
                cvar: Condvar::new(),
            }),
            py: Rc::new(RefCell::new(AsyncResultPy {
                ready: false,
                success: false,
                value: None,
                callback,
                error_callback,
            })),
        },
    )?;
    Ok(inst)
}

fn settle_async_result(interp: &mut Interpreter, res_obj: &Value, ok: bool, val: Value) {
    let Ok((waiter, py)) = with_native_state::<AsyncResultState, _>(res_obj, "ApplyResult", |st| {
        Ok((Arc::clone(&st.waiter), Rc::clone(&st.py)))
    }) else {
        return;
    };
    let cb_to_run = {
        let mut p = py.borrow_mut();
        if p.ready {
            return;
        }
        p.ready = true;
        p.success = ok;
        p.value = Some(val.clone());
        if ok {
            p.callback.take()
        } else {
            p.error_callback.take()
        }
    };
    {
        let mut d = waiter
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *d = true;
        waiter.cvar.notify_all();
    }
    if let Some(cb) = cb_to_run {
        let _ = call_pos(interp, cb, &[val]);
    }
}

// ── Map / IMap aggregation ───────────────────────────────────────────────────

struct MapAggregator {
    total: usize,
    completed: usize,
    failed: bool,
    results: Vec<Option<Value>>,
    target_async: Value,
}

struct IMapState {
    ordered: bool,
    total: usize,
    next_emit: usize,
    emitted: usize,
    ready_ordered: HashMap<usize, (bool, Value)>,
    ready_unordered: VecDeque<(bool, Value)>,
    waiter: Arc<AsyncResultWaiter>,
}

enum TaskSink {
    Single(Value),
    MapItem {
        agg: Rc<RefCell<MapAggregator>>,
        index: usize,
    },
    IMapItem {
        imap: Rc<RefCell<IMapState>>,
        index: usize,
    },
}

// ── Process Pool internals ───────────────────────────────────────────────────

struct MpPoolTask {
    task_id: u64,
    payload: Vec<u8>,
}

struct MpWorkQueue {
    pending: VecDeque<MpPoolTask>,
    closed: bool,
    terminated: bool,
    active_io_workers: usize,
    spawned_io_workers: usize,
}

enum MpDoneMsg {
    Reply(Vec<u8>),
    Error(u64, String),
}

struct MpDoneQueue {
    messages: VecDeque<MpDoneMsg>,
    all_workers_done: bool,
}

struct MpPoolShared {
    exe_path: PathBuf,
    config_frame: Vec<u8>,
    processes: usize,
    max_tasks_per_child: Option<usize>,
    work_lock: Mutex<MpWorkQueue>,
    work_cvar: Condvar,
    done_lock: Mutex<MpDoneQueue>,
    done_cvar: Condvar,
    io_handles: Mutex<Vec<JoinHandle<()>>>,
    terminated_flag: AtomicBool,
}

struct MpPoolPy {
    next_id: u64,
    sinks: HashMap<u64, TaskSink>,
    dispatcher_thread: Option<Value>,
}

struct ProcessPoolState {
    shared: Arc<MpPoolShared>,
    py: Rc<RefCell<MpPoolPy>>,
}

struct PoolDispatcherState {
    shared: Arc<MpPoolShared>,
    py: Rc<RefCell<MpPoolPy>>,
}

fn write_pool_frame(stdin: &mut ChildStdin, data: &[u8]) -> std::io::Result<()> {
    stdin.write_all(&(data.len() as u32).to_be_bytes())?;
    stdin.write_all(data)?;
    stdin.flush()
}

fn read_pool_frame(stdout: &mut ChildStdout) -> std::io::Result<Option<Vec<u8>>> {
    let mut hdr = [0u8; 4];
    match stdout.read_exact(&mut hdr) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(hdr) as usize;
    let mut buf = vec![0u8; len];
    if len > 0 {
        stdout.read_exact(&mut buf)?;
    }
    Ok(Some(buf))
}

struct LivePoolWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    tasks_handled: usize,
}

impl LivePoolWorker {
    fn spawn(shared: &MpPoolShared) -> std::result::Result<Self, String> {
        let mut child = Command::new(&shared.exe_path)
            .args(["-c", POOL_WORKER_BOOTSTRAP])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("failed to spawn pool worker: {e}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "pool worker missing stdin".to_string())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "pool worker missing stdout".to_string())?;
        write_pool_frame(&mut stdin, &shared.config_frame)
            .map_err(|e| format!("failed to send pool worker config: {e}"))?;
        match read_pool_frame(&mut stdout) {
            Ok(Some(reply)) if !reply.is_empty() => {
                if let Ok(Some(st)) = child.try_wait()
                    && !st.success()
                {
                    return Err("pool worker failed during initialization".into());
                }
                Ok(Self {
                    child,
                    stdin,
                    stdout,
                    tasks_handled: 0,
                })
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                Err("pool worker terminated during initialization".into())
            }
        }
    }

    fn close(mut self) {
        drop(self.stdin);
        drop(self.stdout);
        let _ = self.child.wait();
    }

    fn kill_now(mut self) {
        drop(self.stdin);
        drop(self.stdout);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn push_pool_done(shared: &MpPoolShared, msg: MpDoneMsg) {
    let mut d = shared
        .done_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    d.messages.push_back(msg);
    shared.done_cvar.notify_all();
}

fn run_pool_io_worker(shared: Arc<MpPoolShared>) {
    let mut live: Option<LivePoolWorker> = None;

    loop {
        let task = {
            let mut w = shared
                .work_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if w.terminated {
                    break None;
                }
                if let Some(t) = w.pending.pop_front() {
                    break Some(t);
                }
                if w.closed {
                    break None;
                }
                w = shared
                    .work_cvar
                    .wait(w)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };

        let Some(task) = task else {
            break;
        };

        if live.is_none() {
            match LivePoolWorker::spawn(&shared) {
                Ok(w) => live = Some(w),
                Err(reason) => {
                    push_pool_done(&shared, MpDoneMsg::Error(task.task_id, reason));
                    continue;
                }
            }
        }

        let proc = live.as_mut().expect("pool worker initialized");
        let io_res = (|| -> std::io::Result<Option<Vec<u8>>> {
            write_pool_frame(&mut proc.stdin, &task.payload)?;
            read_pool_frame(&mut proc.stdout)
        })();

        match io_res {
            Ok(Some(reply_bytes)) => {
                proc.tasks_handled += 1;
                push_pool_done(&shared, MpDoneMsg::Reply(reply_bytes));
                if let Some(lim) = shared.max_tasks_per_child
                    && proc.tasks_handled >= lim
                    && let Some(old) = live.take()
                {
                    old.close();
                }
            }
            Ok(None) | Err(_) => {
                if let Some(old) = live.take() {
                    old.kill_now();
                }
                push_pool_done(
                    &shared,
                    MpDoneMsg::Error(
                        task.task_id,
                        "pool worker process terminated abruptly".into(),
                    ),
                );
            }
        }
    }

    if let Some(old) = live.take() {
        if shared.terminated_flag.load(Ordering::Acquire) {
            old.kill_now();
        } else {
            old.close();
        }
    }

    let all_done = {
        let mut w = shared
            .work_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        w.active_io_workers = w.active_io_workers.saturating_sub(1);
        w.active_io_workers == 0 && (w.closed || w.terminated)
    };
    if all_done {
        let mut d = shared
            .done_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        d.all_workers_done = true;
        shared.done_cvar.notify_all();
    }
}

fn deliver_to_sink(interp: &mut Interpreter, sink: TaskSink, ok: bool, payload: Value) {
    match sink {
        TaskSink::Single(ar) => {
            settle_async_result(interp, &ar, ok, payload);
        }
        TaskSink::MapItem { agg, index } => {
            let settle_now = {
                let mut a = agg.borrow_mut();
                if a.failed {
                    None
                } else if !ok {
                    a.failed = true;
                    Some((a.target_async.clone(), false, payload))
                } else {
                    if index < a.results.len() {
                        a.results[index] = Some(payload);
                    }
                    a.completed += 1;
                    if a.completed >= a.total {
                        let out: Vec<Value> = a
                            .results
                            .iter_mut()
                            .map(|opt| opt.take().unwrap_or_else(Value::none))
                            .collect();
                        Some((a.target_async.clone(), true, Value::list(out)))
                    } else {
                        None
                    }
                }
            };
            if let Some((ar, s_ok, val)) = settle_now {
                settle_async_result(interp, &ar, s_ok, val);
            }
        }
        TaskSink::IMapItem { imap, index } => {
            let waiter = {
                let mut im = imap.borrow_mut();
                if im.ordered {
                    im.ready_ordered.insert(index, (ok, payload));
                } else {
                    im.ready_unordered.push_back((ok, payload));
                }
                Arc::clone(&im.waiter)
            };
            let mut d = waiter
                .done
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *d = true;
            waiter.cvar.notify_all();
        }
    }
}

fn run_pool_dispatcher(
    interp: &mut Interpreter,
    shared: Arc<MpPoolShared>,
    py: Rc<RefCell<MpPoolPy>>,
) -> Result<()> {
    let gil = Arc::clone(&interp.cold.gil_state);
    loop {
        let msg = interp.release_gil_during(|| {
            let mut d = shared
                .done_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if let Some(m) = d.messages.pop_front() {
                    return Some(m);
                }
                if d.all_workers_done {
                    return None;
                }
                if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                    return None;
                }
                let (next_d, _) = shared
                    .done_cvar
                    .wait_timeout(d, SHUTDOWN_POLL_SLICE)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                d = next_d;
            }
        });
        let Some(msg) = msg else {
            break;
        };
        match msg {
            MpDoneMsg::Reply(bytes) => {
                if let Ok(reply) = pickle_loads(interp, &bytes)
                    && let ValueKind::Tuple(items) = reply.kind()
                    && items.len() == 3
                    && let ValueKind::Int(tid) = items[0].kind()
                {
                    let ok = interp.truthy_value(&items[1]).unwrap_or(false);
                    let payload = items[2].clone();
                    if let Some(sink) = py.borrow_mut().sinks.remove(&(tid as u64)) {
                        deliver_to_sink(interp, sink, ok, payload);
                    }
                }
            }
            MpDoneMsg::Error(tid, reason) => {
                if let Some(sink) = py.borrow_mut().sinks.remove(&tid) {
                    let exc = interp
                        .materialize_pyerror(PyError::named("RuntimeError", reason))
                        .unwrap_or_else(|e| Value::string(e.to_string()));
                    deliver_to_sink(interp, sink, false, exc);
                }
            }
        }
    }
    Ok(())
}

fn ensure_pool_started(
    interp: &mut Interpreter,
    shared: &Arc<MpPoolShared>,
    py: &Rc<RefCell<MpPoolPy>>,
) -> Result<()> {
    if py.borrow().dispatcher_thread.is_none() {
        let m = interp.load_module("multiprocessing.pool")?;
        let dcls = interp.get_attr(&m, "_PoolDispatcher")?;
        let dobj = call_pos(interp, dcls, &[])?;
        set_native_state(
            &dobj,
            PoolDispatcherState {
                shared: Arc::clone(shared),
                py: Rc::clone(py),
            },
        )?;
        let th_mod = interp.load_module("threading")?;
        let th_cls = interp.get_attr(&th_mod, "Thread")?;
        let th = interp.call_function_expanded(
            th_cls,
            &[
                ExpandedCallArg {
                    name: Some("name".to_string()),
                    value: Value::string("MpPoolDispatcher"),
                },
                ExpandedCallArg {
                    name: Some("target".to_string()),
                    value: dobj,
                },
                ExpandedCallArg {
                    name: Some("daemon".to_string()),
                    value: Value::bool_(true),
                },
            ],
        )?;
        let start_fn = interp.get_attr(&th, "start")?;
        call_pos(interp, start_fn, &[])?;
        py.borrow_mut().dispatcher_thread = Some(th);
    }

    let spawn_more = {
        let mut w = shared
            .work_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if w.spawned_io_workers < shared.processes {
            w.spawned_io_workers += 1;
            w.active_io_workers += 1;
            true
        } else {
            false
        }
    };
    if spawn_more {
        let s = Arc::clone(shared);
        let jh = thread::spawn(move || run_pool_io_worker(s));
        shared
            .io_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(jh);
    }
    Ok(())
}

fn submit_pool_task(
    interp: &mut Interpreter,
    shared: &Arc<MpPoolShared>,
    py: &Rc<RefCell<MpPoolPy>>,
    func: Value,
    args: Vec<Value>,
    kwds: Value,
    sink: TaskSink,
) -> Result<()> {
    {
        let w = shared
            .work_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if w.closed || w.terminated {
            return Err(PyError::named("ValueError", "Pool not running"));
        }
    }
    let tid = {
        let mut p = py.borrow_mut();
        p.next_id += 1;
        p.next_id
    };
    let req = Value::tuple(vec![Value::int(tid as i64), func, Value::tuple(args), kwds]);
    let payload = match pickle_dumps(interp, req) {
        Ok(b) => b,
        Err(err) => {
            let exc = interp
                .materialize_pyerror(err)
                .unwrap_or_else(|e| Value::string(e.to_string()));
            deliver_to_sink(interp, sink, false, exc);
            return Ok(());
        }
    };
    py.borrow_mut().sinks.insert(tid, sink);
    ensure_pool_started(interp, shared, py)?;
    {
        let mut w = shared
            .work_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        w.pending.push_back(MpPoolTask {
            task_id: tid,
            payload,
        });
        shared.work_cvar.notify_one();
    }
    Ok(())
}

fn build_pool_config_frame(
    interp: &mut Interpreter,
    initializer: Option<Value>,
    initargs: Vec<Value>,
    max_tasks: Option<usize>,
) -> Result<Vec<u8>> {
    let sys_mod = interp.load_module("sys")?;
    let sys_path = interp
        .get_attr(&sys_mod, "path")
        .unwrap_or_else(|_| Value::list(Vec::new()));
    let path_items: Vec<Value> = interp
        .collect_iterable(&sys_path)
        .unwrap_or_default()
        .into_iter()
        .filter(|v| v.as_str().is_some())
        .collect();
    let main_file = if let Ok(modules) = interp.get_attr(&sys_mod, "modules")
        && let Some(dict) = modules.as_dict()
        && let Some(main_mod) = dict.get(&PyKey::str_from("__main__"))
        && let Ok(f) = interp.get_attr(main_mod, "__file__")
        && f.as_str().is_some()
    {
        f
    } else {
        Value::none()
    };
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| Value::string(p.to_string_lossy().as_ref()))
        .unwrap_or_else(Value::none);
    let init_bytes_val = match initializer {
        None => Value::none(),
        Some(f) => {
            let b = pickle_dumps(interp, Value::tuple(vec![f, Value::tuple(initargs)]))?;
            Value::bytes(b)
        }
    };
    let mt_val = max_tasks
        .map(|n| Value::int(n as i64))
        .unwrap_or_else(Value::none);
    pickle_dumps(
        interp,
        Value::tuple(vec![
            Value::list(path_items),
            main_file,
            cwd,
            init_bytes_val,
            mt_val,
        ]),
    )
}

// ── ThreadPool internals ─────────────────────────────────────────────────────

struct TpWorkItem {
    func: Value,
    args: Vec<Value>,
    kwargs: Vec<(String, Value)>,
    sink: TaskSink,
}

struct TpSharedQueue {
    pending: VecDeque<u64>,
    closed: bool,
    terminated: bool,
}

struct TpPyState {
    next_id: u64,
    items: HashMap<u64, TpWorkItem>,
    threads: Vec<Value>,
    initializer: Option<Value>,
    initargs: Vec<Value>,
    max_workers: usize,
    spawned: usize,
}

struct ThreadPoolState {
    shared: Arc<(Mutex<TpSharedQueue>, Condvar)>,
    py: Rc<RefCell<TpPyState>>,
}

struct TpWorkerState {
    shared: Arc<(Mutex<TpSharedQueue>, Condvar)>,
    py: Rc<RefCell<TpPyState>>,
}

fn ensure_tp_worker(
    interp: &mut Interpreter,
    shared: &Arc<(Mutex<TpSharedQueue>, Condvar)>,
    py: &Rc<RefCell<TpPyState>>,
) -> Result<()> {
    let should_spawn = {
        let mut p = py.borrow_mut();
        if p.spawned < p.max_workers {
            p.spawned += 1;
            true
        } else {
            false
        }
    };
    if !should_spawn {
        return Ok(());
    }
    let m = interp.load_module("multiprocessing.pool")?;
    let wcls = interp.get_attr(&m, "_ThreadPoolWorker")?;
    let wobj = call_pos(interp, wcls, &[])?;
    set_native_state(
        &wobj,
        TpWorkerState {
            shared: Arc::clone(shared),
            py: Rc::clone(py),
        },
    )?;
    let th_mod = interp.load_module("threading")?;
    let th_cls = interp.get_attr(&th_mod, "Thread")?;
    let th = interp.call_function_expanded(
        th_cls,
        &[
            ExpandedCallArg {
                name: Some("target".to_string()),
                value: wobj,
            },
            ExpandedCallArg {
                name: Some("daemon".to_string()),
                value: Value::bool_(true),
            },
        ],
    )?;
    let start_fn = interp.get_attr(&th, "start")?;
    call_pos(interp, start_fn, &[])?;
    py.borrow_mut().threads.push(th);
    Ok(())
}

fn submit_tp_task(
    interp: &mut Interpreter,
    shared: &Arc<(Mutex<TpSharedQueue>, Condvar)>,
    py: &Rc<RefCell<TpPyState>>,
    func: Value,
    args: Vec<Value>,
    kwargs: Vec<(String, Value)>,
    sink: TaskSink,
) -> Result<()> {
    {
        let q = shared
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.closed || q.terminated {
            return Err(PyError::named("ValueError", "Pool not running"));
        }
    }
    let id = {
        let mut p = py.borrow_mut();
        p.next_id += 1;
        let id = p.next_id;
        p.items.insert(
            id,
            TpWorkItem {
                func,
                args,
                kwargs,
                sink,
            },
        );
        id
    };
    ensure_tp_worker(interp, shared, py)?;
    {
        let mut q = shared
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        q.pending.push_back(id);
        shared.1.notify_one();
    }
    Ok(())
}

fn make_imap_instance(
    interp: &mut Interpreter,
    cls_name: &str,
    ordered: bool,
    total: usize,
) -> Result<(Value, Rc<RefCell<IMapState>>)> {
    let m = interp.load_module("multiprocessing.pool")?;
    let cls = interp.get_attr(&m, cls_name)?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named("RuntimeError", "IMap class invalid"));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    let st = Rc::new(RefCell::new(IMapState {
        ordered,
        total,
        next_emit: 0,
        emitted: 0,
        ready_ordered: HashMap::new(),
        ready_unordered: VecDeque::new(),
        waiter: Arc::new(AsyncResultWaiter {
            done: Mutex::new(false),
            cvar: Condvar::new(),
        }),
    }));
    set_native_state(&inst, Rc::clone(&st))?;
    Ok((inst, st))
}

fn imap_next_impl(
    interp: &mut Interpreter,
    imap_val: &Value,
    timeout: Option<Duration>,
) -> Result<Value> {
    let st_rc = with_native_state::<Rc<RefCell<IMapState>>, _>(imap_val, "IMapIterator", |r| {
        Ok(Rc::clone(r))
    })?;
    let waiter = Arc::clone(&st_rc.borrow().waiter);
    let deadline = timeout.map(|d| Instant::now() + d);
    let gil = Arc::clone(&interp.cold.gil_state);

    loop {
        let popped = {
            let mut im = st_rc.borrow_mut();
            if im.emitted >= im.total {
                return Err(PyError::named("StopIteration", ""));
            }
            if im.ordered {
                let idx = im.next_emit;
                if let Some(item) = im.ready_ordered.remove(&idx) {
                    im.next_emit += 1;
                    im.emitted += 1;
                    Some(item)
                } else {
                    None
                }
            } else if let Some(item) = im.ready_unordered.pop_front() {
                im.emitted += 1;
                Some(item)
            } else {
                None
            }
        };

        if let Some((ok, val)) = popped {
            if ok {
                return Ok(val);
            }
            let is_inst = matches!(val.kind(), ValueKind::PyInstance(_));
            if is_inst {
                return Err(PyError::raised(val));
            }
            return Err(PyError::named(
                "RuntimeError",
                val.as_str().unwrap_or("remote error").to_string(),
            ));
        }

        if let Some(dl) = deadline
            && Instant::now() >= dl
        {
            return Err(raise_context_error(interp, "TimeoutError", ""));
        }

        let timed_out = interp.release_gil_during(|| {
            let mut g = waiter
                .done
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *g = false;
            let wait_dur = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        return true;
                    }
                    (dl - now).min(SHUTDOWN_POLL_SLICE)
                }
                None => SHUTDOWN_POLL_SLICE,
            };
            if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                return true;
            }
            let _ = waiter
                .cvar
                .wait_timeout(g, wait_dur)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            false
        });
        if timed_out {
            return Err(raise_context_error(interp, "TimeoutError", ""));
        }
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let ctx_mod = interp.load_module("multiprocessing.context")?;
    let timeout_cls = interp.get_attr(&ctx_mod, "TimeoutError")?;
    let mut m = module.borrow_mut();
    m.insert_attr("TimeoutError".to_string(), timeout_cls);
    if let Some(ar) = m.get_attr_value("ApplyResult") {
        m.insert_attr("AsyncResult".to_string(), ar);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![Value::string("Pool"), Value::string("ThreadPool")]),
        "INIT" => Value::string("INIT"),
        "RUN" => Value::string("RUN"),
        "CLOSE" => Value::string("CLOSE"),
        "TERMINATE" => Value::string("TERMINATE"),
    }

    exception MaybeEncodingError;
    exception RemoteTraceback;

    fn _pool_worker_recv() -> Result<Value> {
        let res: std::io::Result<Option<Vec<u8>>> = _interp.release_gil_during(|| {
            let stdin = std::io::stdin();
            let mut lock = stdin.lock();
            let mut hdr = [0u8; 4];
            match lock.read_exact(&mut hdr) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(e),
            }
            let len = u32::from_be_bytes(hdr) as usize;
            let mut buf = vec![0u8; len];
            if len > 0 {
                lock.read_exact(&mut buf)?;
            }
            Ok(Some(buf))
        });
        match res {
            Ok(Some(b)) => Ok(Value::bytes(b)),
            Ok(None) => Ok(Value::none()),
            Err(e) => Err(PyError::from_io_error(&e, None)),
        }
    }

    fn _pool_worker_send(data: PyBytesLike<'_>) -> Result<Value> {
        let payload = data.as_bytes().to_vec();
        let res: std::io::Result<()> = _interp.release_gil_during(|| {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            lock.write_all(&(payload.len() as u32).to_be_bytes())?;
            lock.write_all(&payload)?;
            lock.flush()
        });
        res.map_err(|e| PyError::from_io_error(&e, None))?;
        Ok(Value::none())
    }

    #[native_state(PoolDispatcherState)]
    class _PoolDispatcher {
        fn __call__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            run_pool_dispatcher(_interp, shared, py)?;
            Ok(Value::none())
        }
    }

    #[native_state(TpWorkerState)]
    class _ThreadPoolWorker {
        fn __call__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let init_opt = {
                let p = py.borrow();
                p.initializer.clone().map(|f| (f, p.initargs.clone()))
            };
            if let Some((init_fn, init_args)) = init_opt {
                let _ = call_pos(_interp, init_fn, &init_args);
            }
            let gil = Arc::clone(&_interp.cold.gil_state);
            loop {
                let next_id = _interp.release_gil_during(|| {
                    let mut q = shared
                        .0
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    loop {
                        if q.terminated {
                            return None;
                        }
                        if let Some(id) = q.pending.pop_front() {
                            return Some(id);
                        }
                        if q.closed {
                            return None;
                        }
                        if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                            return None;
                        }
                        let (next_q, _) = shared
                            .1
                            .wait_timeout(q, SHUTDOWN_POLL_SLICE)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        q = next_q;
                    }
                });
                let Some(id) = next_id else {
                    break;
                };
                let Some(item) = py.borrow_mut().items.remove(&id) else {
                    continue;
                };
                match call_pos_kw(_interp, item.func, &item.args, &item.kwargs) {
                    Ok(v) => deliver_to_sink(_interp, item.sink, true, v),
                    Err(err) => {
                        let exc = _interp
                            .materialize_pyerror(err)
                            .unwrap_or_else(|e| Value::string(e.to_string()));
                        deliver_to_sink(_interp, item.sink, false, exc);
                    }
                }
            }
            Ok(Value::none())
        }
    }

    #[native_state(AsyncResultState)]
    class ApplyResult {
        fn ready(#[state] st: &AsyncResultState) -> Result<Value> {
            Ok(Value::bool_(st.py.borrow().ready))
        }

        fn successful(#[state] st: &AsyncResultState) -> Result<Value> {
            let p = st.py.borrow();
            if !p.ready {
                return Err(PyError::named("ValueError", "result is not ready"));
            }
            Ok(Value::bool_(p.success))
        }

        fn wait(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let (waiter, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.waiter), Rc::clone(&st.py)))
            })?;
            if py.borrow().ready {
                return Ok(Value::none());
            }
            let dur = parse_timeout(timeout.as_value(), false)?;
            let deadline = dur.map(|d| Instant::now() + d);
            let gil = Arc::clone(&_interp.cold.gil_state);
            _interp.release_gil_during(|| {
                let mut g = waiter
                    .done
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while !*g {
                    if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                        break;
                    }
                    let wait_dur = match deadline {
                        Some(dl) => {
                            let now = Instant::now();
                            if now >= dl {
                                break;
                            }
                            (dl - now).min(SHUTDOWN_POLL_SLICE)
                        }
                        None => SHUTDOWN_POLL_SLICE,
                    };
                    let (ng, _) = waiter
                        .cvar
                        .wait_timeout(g, wait_dur)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    g = ng;
                }
            });
            Ok(Value::none())
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let wait_fn = _interp.get_attr(self_val.0, "wait")?;
            match timeout.as_value() {
                Some(t) => call_pos(_interp, wait_fn, std::slice::from_ref(t))?,
                None => call_pos(_interp, wait_fn, &[])?,
            };
            let (ready, success, value) = with_state(self_val.0, |st| {
                let p = st.py.borrow();
                Ok((p.ready, p.success, p.value.clone()))
            })?;
            if !ready {
                return Err(raise_context_error(_interp, "TimeoutError", ""));
            }
            let val = value.unwrap_or_else(Value::none);
            let is_inst = matches!(val.kind(), ValueKind::PyInstance(_));
            if success {
                Ok(val)
            } else if is_inst {
                Err(PyError::raised(val))
            } else {
                Err(PyError::named(
                    "RuntimeError",
                    val.as_str().unwrap_or("remote exception").to_string(),
                ))
            }
        }
    }

    #[native_state(AsyncResultState)]
    class MapResult : "ApplyResult" {}

    #[native_state(Rc<RefCell<IMapState>>)]
    class IMapIterator {
        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn next(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let dur = parse_timeout(timeout.as_value(), false)?;
            imap_next_impl(_interp, self_val.0, dur)
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            imap_next_impl(_interp, self_val.0, None)
        }
    }

    #[native_state(Rc<RefCell<IMapState>>)]
    class IMapUnorderedIterator : "IMapIterator" {}

    #[native_state(ProcessPoolState)]
    class Pool {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] processes: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] initializer: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] initargs: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] maxtasksperchild: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = context;
            let num_procs = match processes.as_value() {
                None => cpu_count_value().unwrap_or(1).max(1) as usize,
                Some(v) if v.is_none() => cpu_count_value().unwrap_or(1).max(1) as usize,
                Some(v) => {
                    let n = v.as_int().ok_or_else(|| {
                        PyError::named("TypeError", "processes must be an integer or None")
                    })?;
                    if n < 1 {
                        return Err(PyError::named(
                            "ValueError",
                            "Number of processes must be at least 1",
                        ));
                    }
                    n as usize
                }
            };
            let init_fn = match initializer.as_value() {
                Some(v) if !v.is_none() => Some(v.clone()),
                _ => None,
            };
            let init_args = match initargs.as_value() {
                Some(v) if !v.is_none() => _interp.collect_iterable(v)?,
                _ => Vec::new(),
            };
            let max_tasks = match maxtasksperchild.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => {
                    let n = v.as_int().ok_or_else(|| {
                        PyError::named("TypeError", "maxtasksperchild must be an integer or None")
                    })?;
                    if n < 1 {
                        return Err(PyError::named(
                            "ValueError",
                            "maxtasksperchild must be >= 1",
                        ));
                    }
                    Some(n as usize)
                }
            };
            let exe_path = get_executable_path()?;
            let config_frame =
                build_pool_config_frame(_interp, init_fn, init_args, max_tasks)?;
            set_state(
                self_val.0,
                ProcessPoolState {
                    shared: Arc::new(MpPoolShared {
                        exe_path,
                        config_frame,
                        processes: num_procs,
                        max_tasks_per_child: max_tasks,
                        work_lock: Mutex::new(MpWorkQueue {
                            pending: VecDeque::new(),
                            closed: false,
                            terminated: false,
                            active_io_workers: 0,
                            spawned_io_workers: 0,
                        }),
                        work_cvar: Condvar::new(),
                        done_lock: Mutex::new(MpDoneQueue {
                            messages: VecDeque::new(),
                            all_workers_done: false,
                        }),
                        done_cvar: Condvar::new(),
                        io_handles: Mutex::new(Vec::new()),
                        terminated_flag: AtomicBool::new(false),
                    }),
                    py: Rc::new(RefCell::new(MpPoolPy {
                        next_id: 0,
                        sinks: HashMap::new(),
                        dispatcher_thread: None,
                    })),
                },
            )?;
            Ok(Value::none())
        }

        fn apply_async(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] args: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwds: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] callback: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] error_callback: OptValueRef<'_>,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let arg_vec = match args.as_value() {
                Some(v) if !v.is_none() => _interp.collect_iterable(v)?,
                _ => Vec::new(),
            };
            let kw_dict = match kwds.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => Value::dict(PyDict::default()),
            };
            let cb = callback
                .as_value()
                .filter(|v| !v.is_none())
                .cloned();
            let ecb = error_callback
                .as_value()
                .filter(|v| !v.is_none())
                .cloned();
            let ar = new_async_result(_interp, "ApplyResult", cb, ecb)?;
            submit_pool_task(
                _interp,
                &shared,
                &py,
                func.0.clone(),
                arg_vec,
                kw_dict,
                TaskSink::Single(ar.clone()),
            )?;
            Ok(ar)
        }

        fn apply(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] args: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwds: OptValueRef<'_>,
        ) -> Result<Value> {
            let aa = _interp.get_attr(self_val.0, "apply_async")?;
            let args_v = args
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::tuple(Vec::new()));
            let kw_v = kwds
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            let ar = call_pos(_interp, aa, &[func.0.clone(), args_v, kw_v])?;
            let get_fn = _interp.get_attr(&ar, "get")?;
            call_pos(_interp, get_fn, &[])
        }

        fn map_async(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _chunksize: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] callback: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] error_callback: OptValueRef<'_>,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let items = _interp.collect_iterable(iterable.0)?;
            let cb = callback.as_value().filter(|v| !v.is_none()).cloned();
            let ecb = error_callback.as_value().filter(|v| !v.is_none()).cloned();
            let ar = new_async_result(_interp, "MapResult", cb, ecb)?;
            if items.is_empty() {
                settle_async_result(_interp, &ar, true, Value::list(Vec::new()));
                return Ok(ar);
            }
            let agg = Rc::new(RefCell::new(MapAggregator {
                total: items.len(),
                completed: 0,
                failed: false,
                results: vec![None; items.len()],
                target_async: ar.clone(),
            }));
            for (idx, item) in items.into_iter().enumerate() {
                submit_pool_task(
                    _interp,
                    &shared,
                    &py,
                    func.0.clone(),
                    vec![item],
                    Value::dict(PyDict::default()),
                    TaskSink::MapItem {
                        agg: Rc::clone(&agg),
                        index: idx,
                    },
                )?;
            }
            Ok(ar)
        }

        fn map(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] chunksize: OptValueRef<'_>,
        ) -> Result<Value> {
            let ma = _interp.get_attr(self_val.0, "map_async")?;
            let cs = chunksize.as_value().cloned().unwrap_or_else(Value::none);
            let ar = call_pos(_interp, ma, &[func.0.clone(), iterable.0.clone(), cs])?;
            let get_fn = _interp.get_attr(&ar, "get")?;
            call_pos(_interp, get_fn, &[])
        }

        fn starmap_async(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _chunksize: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] callback: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] error_callback: OptValueRef<'_>,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let items = _interp.collect_iterable(iterable.0)?;
            let cb = callback.as_value().filter(|v| !v.is_none()).cloned();
            let ecb = error_callback.as_value().filter(|v| !v.is_none()).cloned();
            let ar = new_async_result(_interp, "MapResult", cb, ecb)?;
            if items.is_empty() {
                settle_async_result(_interp, &ar, true, Value::list(Vec::new()));
                return Ok(ar);
            }
            let agg = Rc::new(RefCell::new(MapAggregator {
                total: items.len(),
                completed: 0,
                failed: false,
                results: vec![None; items.len()],
                target_async: ar.clone(),
            }));
            for (idx, item) in items.into_iter().enumerate() {
                let item_args = _interp.collect_iterable(&item)?;
                submit_pool_task(
                    _interp,
                    &shared,
                    &py,
                    func.0.clone(),
                    item_args,
                    Value::dict(PyDict::default()),
                    TaskSink::MapItem {
                        agg: Rc::clone(&agg),
                        index: idx,
                    },
                )?;
            }
            Ok(ar)
        }

        fn starmap(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] chunksize: OptValueRef<'_>,
        ) -> Result<Value> {
            let sa = _interp.get_attr(self_val.0, "starmap_async")?;
            let cs = chunksize.as_value().cloned().unwrap_or_else(Value::none);
            let ar = call_pos(_interp, sa, &[func.0.clone(), iterable.0.clone(), cs])?;
            let get_fn = _interp.get_attr(&ar, "get")?;
            call_pos(_interp, get_fn, &[])
        }

        fn imap(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(1i64.into())] _chunksize: PyIndex,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let items = _interp.collect_iterable(iterable.0)?;
            let (iter_obj, st) = make_imap_instance(_interp, "IMapIterator", true, items.len())?;
            for (idx, item) in items.into_iter().enumerate() {
                submit_pool_task(
                    _interp,
                    &shared,
                    &py,
                    func.0.clone(),
                    vec![item],
                    Value::dict(PyDict::default()),
                    TaskSink::IMapItem {
                        imap: Rc::clone(&st),
                        index: idx,
                    },
                )?;
            }
            Ok(iter_obj)
        }

        fn imap_unordered(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(1i64.into())] _chunksize: PyIndex,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let items = _interp.collect_iterable(iterable.0)?;
            let (iter_obj, st) =
                make_imap_instance(_interp, "IMapUnorderedIterator", false, items.len())?;
            for (idx, item) in items.into_iter().enumerate() {
                submit_pool_task(
                    _interp,
                    &shared,
                    &py,
                    func.0.clone(),
                    vec![item],
                    Value::dict(PyDict::default()),
                    TaskSink::IMapItem {
                        imap: Rc::clone(&st),
                        index: idx,
                    },
                )?;
            }
            Ok(iter_obj)
        }

        fn close(#[state] st: &ProcessPoolState) -> Result<Value> {
            let no_workers = {
                let mut w = st
                    .shared
                    .work_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                w.closed = true;
                st.shared.work_cvar.notify_all();
                w.active_io_workers == 0
            };
            if no_workers {
                let mut d = st
                    .shared
                    .done_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                d.all_workers_done = true;
                st.shared.done_cvar.notify_all();
            }
            Ok(Value::none())
        }

        fn terminate(#[state] st: &ProcessPoolState) -> Result<Value> {
            st.shared.terminated_flag.store(true, Ordering::Release);
            let no_workers = {
                let mut w = st
                    .shared
                    .work_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                w.terminated = true;
                w.closed = true;
                w.pending.clear();
                st.shared.work_cvar.notify_all();
                w.active_io_workers == 0
            };
            if no_workers {
                let mut d = st
                    .shared
                    .done_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                d.all_workers_done = true;
                st.shared.done_cvar.notify_all();
            }
            Ok(Value::none())
        }

        fn join(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            {
                let w = shared
                    .work_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !w.closed && !w.terminated {
                    return Err(PyError::named("ValueError", "Pool is still running"));
                }
            }
            let handles: Vec<JoinHandle<()>> = {
                let mut h = shared
                    .io_handles
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                std::mem::take(&mut *h)
            };
            if !handles.is_empty() {
                _interp.release_gil_during(|| {
                    for jh in handles {
                        let _ = jh.join();
                    }
                });
            }
            {
                let mut d = shared
                    .done_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                d.all_workers_done = true;
                shared.done_cvar.notify_all();
            }
            let disp = py.borrow_mut().dispatcher_thread.take();
            if let Some(th) = disp {
                let join_fn = _interp.get_attr(&th, "join")?;
                let _ = call_pos(_interp, join_fn, &[])?;
            }
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            let term = _interp.get_attr(self_val.0, "terminate")?;
            let _ = call_pos(_interp, term, &[]);
            let j = _interp.get_attr(self_val.0, "join")?;
            let _ = call_pos(_interp, j, &[]);
            Ok(Value::none())
        }
    }

    #[native_state(ThreadPoolState)]
    class ThreadPool {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] processes: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] initializer: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] initargs: OptValueRef<'_>,
        ) -> Result<Value> {
            let max_w = match processes.as_value() {
                None => cpu_count_value().unwrap_or(1).max(1) as usize,
                Some(v) if v.is_none() => cpu_count_value().unwrap_or(1).max(1) as usize,
                Some(v) => {
                    let n = v.as_int().ok_or_else(|| {
                        PyError::named("TypeError", "processes must be an integer or None")
                    })?;
                    if n < 1 {
                        return Err(PyError::named(
                            "ValueError",
                            "Number of processes must be at least 1",
                        ));
                    }
                    n as usize
                }
            };
            let init_fn = initializer.as_value().filter(|v| !v.is_none()).cloned();
            let init_args = match initargs.as_value() {
                Some(v) if !v.is_none() => _interp.collect_iterable(v)?,
                _ => Vec::new(),
            };
            set_state(
                self_val.0,
                ThreadPoolState {
                    shared: Arc::new((
                        Mutex::new(TpSharedQueue {
                            pending: VecDeque::new(),
                            closed: false,
                            terminated: false,
                        }),
                        Condvar::new(),
                    )),
                    py: Rc::new(RefCell::new(TpPyState {
                        next_id: 0,
                        items: HashMap::new(),
                        threads: Vec::new(),
                        initializer: init_fn,
                        initargs: init_args,
                        max_workers: max_w,
                        spawned: 0,
                    })),
                },
            )?;
            Ok(Value::none())
        }

        fn apply_async(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] args: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwds: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] callback: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] error_callback: OptValueRef<'_>,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let arg_vec = match args.as_value() {
                Some(v) if !v.is_none() => _interp.collect_iterable(v)?,
                _ => Vec::new(),
            };
            let mut kw_vec = Vec::new();
            if let Some(kv) = kwds.as_value()
                && let Some(d) = kv.as_dict()
            {
                for (k, v) in d.iter() {
                    if let PyKey::Str(s) = k {
                        kw_vec.push((s.to_string(), v.clone()));
                    }
                }
            }
            let cb = callback.as_value().filter(|v| !v.is_none()).cloned();
            let ecb = error_callback.as_value().filter(|v| !v.is_none()).cloned();
            let ar = new_async_result(_interp, "ApplyResult", cb, ecb)?;
            submit_tp_task(
                _interp,
                &shared,
                &py,
                func.0.clone(),
                arg_vec,
                kw_vec,
                TaskSink::Single(ar.clone()),
            )?;
            Ok(ar)
        }

        fn apply(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] args: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwds: OptValueRef<'_>,
        ) -> Result<Value> {
            let aa = _interp.get_attr(self_val.0, "apply_async")?;
            let args_v = args
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::tuple(Vec::new()));
            let kw_v = kwds
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            let ar = call_pos(_interp, aa, &[func.0.clone(), args_v, kw_v])?;
            let get_fn = _interp.get_attr(&ar, "get")?;
            call_pos(_interp, get_fn, &[])
        }

        fn map_async(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _chunksize: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] callback: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] error_callback: OptValueRef<'_>,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let items = _interp.collect_iterable(iterable.0)?;
            let cb = callback.as_value().filter(|v| !v.is_none()).cloned();
            let ecb = error_callback.as_value().filter(|v| !v.is_none()).cloned();
            let ar = new_async_result(_interp, "MapResult", cb, ecb)?;
            if items.is_empty() {
                settle_async_result(_interp, &ar, true, Value::list(Vec::new()));
                return Ok(ar);
            }
            let agg = Rc::new(RefCell::new(MapAggregator {
                total: items.len(),
                completed: 0,
                failed: false,
                results: vec![None; items.len()],
                target_async: ar.clone(),
            }));
            for (idx, item) in items.into_iter().enumerate() {
                submit_tp_task(
                    _interp,
                    &shared,
                    &py,
                    func.0.clone(),
                    vec![item],
                    Vec::new(),
                    TaskSink::MapItem {
                        agg: Rc::clone(&agg),
                        index: idx,
                    },
                )?;
            }
            Ok(ar)
        }

        fn map(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] chunksize: OptValueRef<'_>,
        ) -> Result<Value> {
            let ma = _interp.get_attr(self_val.0, "map_async")?;
            let cs = chunksize.as_value().cloned().unwrap_or_else(Value::none);
            let ar = call_pos(_interp, ma, &[func.0.clone(), iterable.0.clone(), cs])?;
            let get_fn = _interp.get_attr(&ar, "get")?;
            call_pos(_interp, get_fn, &[])
        }

        fn starmap_async(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _chunksize: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] callback: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] error_callback: OptValueRef<'_>,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let items = _interp.collect_iterable(iterable.0)?;
            let cb = callback.as_value().filter(|v| !v.is_none()).cloned();
            let ecb = error_callback.as_value().filter(|v| !v.is_none()).cloned();
            let ar = new_async_result(_interp, "MapResult", cb, ecb)?;
            if items.is_empty() {
                settle_async_result(_interp, &ar, true, Value::list(Vec::new()));
                return Ok(ar);
            }
            let agg = Rc::new(RefCell::new(MapAggregator {
                total: items.len(),
                completed: 0,
                failed: false,
                results: vec![None; items.len()],
                target_async: ar.clone(),
            }));
            for (idx, item) in items.into_iter().enumerate() {
                let item_args = _interp.collect_iterable(&item)?;
                submit_tp_task(
                    _interp,
                    &shared,
                    &py,
                    func.0.clone(),
                    item_args,
                    Vec::new(),
                    TaskSink::MapItem {
                        agg: Rc::clone(&agg),
                        index: idx,
                    },
                )?;
            }
            Ok(ar)
        }

        fn starmap(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] chunksize: OptValueRef<'_>,
        ) -> Result<Value> {
            let sa = _interp.get_attr(self_val.0, "starmap_async")?;
            let cs = chunksize.as_value().cloned().unwrap_or_else(Value::none);
            let ar = call_pos(_interp, sa, &[func.0.clone(), iterable.0.clone(), cs])?;
            let get_fn = _interp.get_attr(&ar, "get")?;
            call_pos(_interp, get_fn, &[])
        }

        fn imap(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(1i64.into())] _chunksize: PyIndex,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let items = _interp.collect_iterable(iterable.0)?;
            let (iter_obj, st) = make_imap_instance(_interp, "IMapIterator", true, items.len())?;
            for (idx, item) in items.into_iter().enumerate() {
                submit_tp_task(
                    _interp,
                    &shared,
                    &py,
                    func.0.clone(),
                    vec![item],
                    Vec::new(),
                    TaskSink::IMapItem {
                        imap: Rc::clone(&st),
                        index: idx,
                    },
                )?;
            }
            Ok(iter_obj)
        }

        fn imap_unordered(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            iterable: PyValueRef<'_>,
            #[default(1i64.into())] _chunksize: PyIndex,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            let items = _interp.collect_iterable(iterable.0)?;
            let (iter_obj, st) =
                make_imap_instance(_interp, "IMapUnorderedIterator", false, items.len())?;
            for (idx, item) in items.into_iter().enumerate() {
                submit_tp_task(
                    _interp,
                    &shared,
                    &py,
                    func.0.clone(),
                    vec![item],
                    Vec::new(),
                    TaskSink::IMapItem {
                        imap: Rc::clone(&st),
                        index: idx,
                    },
                )?;
            }
            Ok(iter_obj)
        }

        fn close(#[state] st: &ThreadPoolState) -> Result<Value> {
            let mut q = st
                .shared
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            q.closed = true;
            st.shared.1.notify_all();
            Ok(Value::none())
        }

        fn terminate(#[state] st: &ThreadPoolState) -> Result<Value> {
            let mut q = st
                .shared
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            q.terminated = true;
            q.closed = true;
            q.pending.clear();
            st.shared.1.notify_all();
            Ok(Value::none())
        }

        fn join(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let threads = with_state(self_val.0, |st| {
                Ok(std::mem::take(&mut st.py.borrow_mut().threads))
            })?;
            for th in threads {
                let join_fn = _interp.get_attr(&th, "join")?;
                let _ = call_pos(_interp, join_fn, &[])?;
            }
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            let term = _interp.get_attr(self_val.0, "terminate")?;
            let _ = call_pos(_interp, term, &[]);
            let j = _interp.get_attr(self_val.0, "join")?;
            let _ = call_pos(_interp, j, &[]);
            Ok(Value::none())
        }
    }
}
