// `concurrent.futures.process` — 100% native-Rust Python 3.12 `ProcessPoolExecutor`
// (issue #3263).
//
// Spawns real worker subprocesses (`sys.executable`) communicating via length-
// prefixed `pickle` frames over stdin/stdout pipes with dedicated pure-Rust I/O
// threads and a lightweight Python dispatcher thread that settles `Future`s.
// Works identically on Ubuntu and Windows.
//
// Reference: <https://docs.python.org/3.12/library/concurrent.futures.html#processpoolexecutor>

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::bodies_helpers_cf_base as cf_base;
use crate::builtin_modules::macros::link_class_bases;
use crate::builtin_modules::macros::native_state::set_native_state;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, current_thread_is_daemon_worker};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

const DISPATCH_POLL_SLICE: Duration = Duration::from_millis(20);
#[cfg(windows)]
const MAX_WINDOWS_WORKERS: usize = 61;

/// Bootstrap script executed by each spawned worker subprocess (`pyrust -c ...`).
///
/// Protocol:
/// - Framed binary messages over stdin/stdout: `[4-byte BE length][pickle bytes]`.
/// - First message from parent: `(sys_path, main_file, init_bytes_or_none, max_tasks_or_none)`.
/// - Initial reply from worker: `(0, True, None)` or `(0, False, exc)`.
/// - Task messages: `(task_id, fn, args, kwargs)` -> reply `(task_id, True, result)`
///   or `(task_id, False, exc)`.
const PROCESS_WORKER_BOOTSTRAP: &str = r#"
import io, os, pickle, sys, types
from concurrent.futures.process import _worker_recv_frame, _worker_send_frame

sys.stdout = sys.stderr

_mp_main_mod = None

class _WorkerUnpickler(pickle.Unpickler):
    def find_class(self, module, name):
        if module == "__main__" and _mp_main_mod is not None:
            return getattr(_mp_main_mod, name)
        return super().find_class(module, name)

def _loads(data):
    return _WorkerUnpickler(io.BytesIO(data)).load()

def _dumps_reply(task_id, ok, payload):
    try:
        return pickle.dumps((task_id, ok, payload))
    except Exception:
        if ok:
            return pickle.dumps((task_id, False, TypeError(f"cannot pickle result of type {type(payload).__name__}")))
        return pickle.dumps((task_id, False, RuntimeError(f"{type(payload).__name__}: {payload}")))

_cfg_bytes = _worker_recv_frame()
if _cfg_bytes is None:
    sys.exit(0)

_sys_path, _main_file, _init_bytes, _max_tasks = _loads(_cfg_bytes)
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
        _worker_send_frame(_dumps_reply(0, False, _exc))
        sys.exit(1)

if _init_bytes is not None:
    try:
        _init_fn, _init_args = _loads(_init_bytes)
        _init_fn(*_init_args)
    except BaseException as _exc:
        _worker_send_frame(_dumps_reply(0, False, _exc))
        sys.exit(1)

_worker_send_frame(_dumps_reply(0, True, None))

_done_tasks = 0
while True:
    _frame = _worker_recv_frame()
    if _frame is None:
        break
    try:
        _task_id, _fn, _args, _kwargs = _loads(_frame)
    except BaseException as _exc:
        _worker_send_frame(_dumps_reply(0, False, _exc))
        break
    try:
        _res = _fn(*_args, **_kwargs)
        _worker_send_frame(_dumps_reply(_task_id, True, _res))
    except BaseException as _exc:
        _worker_send_frame(_dumps_reply(_task_id, False, _exc))
    _done_tasks += 1
    if _max_tasks is not None and _done_tasks >= _max_tasks:
        break
"#;

struct ProcTask {
    task_id: u64,
    payload: Vec<u8>,
}

enum CompletedMsg {
    Reply(Vec<u8>),
    Broken(String),
}

struct ProcSharedQueue {
    pending: VecDeque<ProcTask>,
    shutdown: bool,
    broken: Option<String>,
    active_io_workers: usize,
    spawned_io_workers: usize,
}

struct ProcCompletedQueue {
    messages: VecDeque<CompletedMsg>,
    all_workers_done: bool,
}

struct ProcPoolShared {
    exe_path: std::path::PathBuf,
    config_frame: Vec<u8>,
    max_workers: usize,
    max_tasks_per_child: Option<usize>,
    work_lock: Mutex<ProcSharedQueue>,
    work_cvar: Condvar,
    done_lock: Mutex<ProcCompletedQueue>,
    done_cvar: Condvar,
    io_handles: Mutex<Vec<JoinHandle<()>>>,
}

struct ProcPoolPyState {
    next_id: u64,
    pending_futures: HashMap<u64, Value>,
    dispatcher_thread: Option<Value>,
}

struct ProcessPoolState {
    shared: Arc<ProcPoolShared>,
    py: Rc<RefCell<ProcPoolPyState>>,
}

struct ProcessDispatcherState {
    shared: Arc<ProcPoolShared>,
    py: Rc<RefCell<ProcPoolPyState>>,
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn pickle_dumps(interp: &mut Interpreter, val: Value) -> Result<Vec<u8>> {
    let pickle_mod = interp.load_module("pickle")?;
    let dumps_fn = interp.get_attr(&pickle_mod, "dumps")?;
    let bytes_val = call_pos(interp, dumps_fn, &[val])?;
    match bytes_val.kind() {
        ValueKind::Bytes(b) => Ok(b.as_slice().to_vec()),
        _ => Err(PyError::named(
            "TypeError",
            "pickle.dumps did not return bytes",
        )),
    }
}

fn pickle_loads(interp: &mut Interpreter, data: &[u8]) -> Result<Value> {
    let pickle_mod = interp.load_module("pickle")?;
    let loads_fn = interp.get_attr(&pickle_mod, "loads")?;
    call_pos(interp, loads_fn, &[Value::bytes(data.to_vec())])
}

fn make_broken_process_pool_exc(interp: &mut Interpreter, msg: &str) -> Result<Value> {
    let m = interp.load_module("concurrent.futures.process")?;
    let cls = interp.get_attr(&m, "BrokenProcessPool")?;
    call_pos(interp, cls, &[Value::string(msg)])
}

fn raise_broken_process_pool(interp: &mut Interpreter, msg: &str) -> PyError {
    match make_broken_process_pool_exc(interp, msg) {
        Ok(inst) => PyError::raised(inst),
        Err(e) => e,
    }
}

fn write_frame(stdin: &mut ChildStdin, data: &[u8]) -> std::io::Result<()> {
    let len_bytes = (data.len() as u32).to_be_bytes();
    stdin.write_all(&len_bytes)?;
    stdin.write_all(data)?;
    stdin.flush()
}

fn read_frame(stdout: &mut ChildStdout) -> std::io::Result<Option<Vec<u8>>> {
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

struct LiveWorkerProc {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    tasks_handled: usize,
}

impl LiveWorkerProc {
    fn spawn(shared: &ProcPoolShared) -> std::result::Result<Self, String> {
        let mut child = Command::new(&shared.exe_path)
            .args(["-c", PROCESS_WORKER_BOOTSTRAP])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("failed to spawn worker process: {e}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "worker process missing stdin".to_string())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "worker process missing stdout".to_string())?;
        write_frame(&mut stdin, &shared.config_frame)
            .map_err(|e| format!("failed to configure worker process: {e}"))?;
        match read_frame(&mut stdout) {
            Ok(Some(reply)) => {
                // Verify init status: if worker exited immediately with failure,
                // forward reply bytes to dispatcher or report broken pool.
                if reply.is_empty() {
                    let _ = child.wait();
                    return Err("worker process terminated abruptly during initialization".into());
                }
                // Check if process already exited with non-zero status (initializer failure).
                if let Ok(Some(status)) = child.try_wait()
                    && !status.success()
                {
                    return Err(
                        "A child process terminated abruptly, the process pool is not usable anymore"
                            .into(),
                    );
                }
                Ok(Self {
                    child,
                    stdin,
                    stdout,
                    tasks_handled: 0,
                })
            }
            Ok(None) => {
                let _ = child.wait();
                Err(
                    "A process in the process pool was terminated abruptly while initializing"
                        .into(),
                )
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(format!("worker init I/O error: {e}"))
            }
        }
    }

    fn close(mut self) {
        drop(self.stdin);
        drop(self.stdout);
        let _ = self.child.wait();
    }
}

fn push_completed(shared: &ProcPoolShared, msg: CompletedMsg) {
    let mut d = shared
        .done_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    d.messages.push_back(msg);
    shared.done_cvar.notify_all();
}

fn mark_pool_broken_from_io(shared: &ProcPoolShared, reason: String) {
    {
        let mut w = shared
            .work_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if w.broken.is_none() {
            w.broken = Some(reason.clone());
        }
        w.pending.clear();
        shared.work_cvar.notify_all();
    }
    push_completed(shared, CompletedMsg::Broken(reason));
}

/// Pure-Rust OS thread managing one child worker subprocess (NO GIL needed!).
fn run_process_io_worker(shared: Arc<ProcPoolShared>) {
    let mut live: Option<LiveWorkerProc> = None;

    loop {
        let task = {
            let mut w = shared
                .work_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if w.broken.is_some() {
                    break None;
                }
                if let Some(t) = w.pending.pop_front() {
                    break Some(t);
                }
                if w.shutdown {
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
            match LiveWorkerProc::spawn(&shared) {
                Ok(proc) => live = Some(proc),
                Err(reason) => {
                    mark_pool_broken_from_io(&shared, reason);
                    break;
                }
            }
        }

        let proc = live.as_mut().expect("worker proc just initialized");
        let io_res = (|| -> std::io::Result<Option<Vec<u8>>> {
            write_frame(&mut proc.stdin, &task.payload)?;
            read_frame(&mut proc.stdout)
        })();

        match io_res {
            Ok(Some(reply_bytes)) => {
                proc.tasks_handled += 1;
                push_completed(&shared, CompletedMsg::Reply(reply_bytes));
                if let Some(limit) = shared.max_tasks_per_child
                    && proc.tasks_handled >= limit
                    && let Some(old) = live.take()
                {
                    old.close();
                }
            }
            Ok(None) | Err(_) => {
                if let Some(mut old) = live.take() {
                    let _ = old.child.kill();
                    let _ = old.child.wait();
                }
                let _ = task.task_id;
                mark_pool_broken_from_io(
                    &shared,
                    "A process in the process pool was terminated abruptly while the future was running or pending.".to_string(),
                );
                break;
            }
        }
    }

    if let Some(old) = live.take() {
        old.close();
    }

    let all_done = {
        let mut w = shared
            .work_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        w.active_io_workers = w.active_io_workers.saturating_sub(1);
        w.active_io_workers == 0 && (w.shutdown || w.broken.is_some())
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

/// Python dispatcher thread: unpickles worker replies and settles `Future`s
/// under the GIL so callbacks, `wait()`, and `as_completed()` work seamlessly.
fn run_process_dispatcher_loop(
    interp: &mut Interpreter,
    shared: Arc<ProcPoolShared>,
    py: Rc<RefCell<ProcPoolPyState>>,
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
                    .wait_timeout(d, DISPATCH_POLL_SLICE)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                d = next_d;
            }
        });

        let Some(msg) = msg else {
            break;
        };

        match msg {
            CompletedMsg::Reply(bytes) => match pickle_loads(interp, &bytes) {
                Ok(reply_val) => {
                    if let ValueKind::Tuple(items) = reply_val.kind()
                        && items.len() == 3
                        && let ValueKind::Int(tid) = items[0].kind()
                    {
                        let ok = interp.truthy_value(&items[1]).unwrap_or(false);
                        let payload = items[2].clone();
                        let fut_opt = py.borrow_mut().pending_futures.remove(&(tid as u64));
                        if let Some(fut) = fut_opt {
                            if ok {
                                let _ = cf_base::future_set_result(interp, &fut, payload);
                            } else {
                                let _ = cf_base::future_set_exception(interp, &fut, payload);
                            }
                        }
                    }
                }
                Err(err) => {
                    let reason = format!("failed to unpickle worker reply: {err}");
                    fail_all_pending_futures(interp, &py, &reason);
                }
            },
            CompletedMsg::Broken(reason) => {
                fail_all_pending_futures(interp, &py, &reason);
            }
        }
    }
    Ok(())
}

fn fail_all_pending_futures(
    interp: &mut Interpreter,
    py: &Rc<RefCell<ProcPoolPyState>>,
    reason: &str,
) {
    let remaining: Vec<Value> = {
        let mut p = py.borrow_mut();
        p.pending_futures.drain().map(|(_, v)| v).collect()
    };
    for fut in remaining {
        if let Ok(exc) = make_broken_process_pool_exc(interp, reason) {
            let _ = cf_base::future_set_exception(interp, &fut, exc);
        }
    }
}

fn ensure_process_workers_started(
    interp: &mut Interpreter,
    shared: &Arc<ProcPoolShared>,
    py: &Rc<RefCell<ProcPoolPyState>>,
) -> Result<()> {
    // 1. Start the Python dispatcher thread lazily on first submit.
    if py.borrow().dispatcher_thread.is_none() {
        let proc_mod = interp.load_module("concurrent.futures.process")?;
        let disp_cls = interp.get_attr(&proc_mod, "_ProcessDispatcher")?;
        let disp_obj = call_pos(interp, disp_cls, &[])?;
        set_native_state(
            &disp_obj,
            ProcessDispatcherState {
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
                    value: Value::string("ProcessPoolDispatcher"),
                },
                ExpandedCallArg {
                    name: Some("target".to_string()),
                    value: disp_obj,
                },
                ExpandedCallArg {
                    name: Some("daemon".to_string()),
                    value: Value::bool_(true),
                },
            ],
        )?;
        let start_fn = interp.get_attr(&th_obj, "start")?;
        call_pos(interp, start_fn, &[])?;
        py.borrow_mut().dispatcher_thread = Some(th_obj);
    }

    // 2. Spawn another pure-Rust worker I/O thread up to `max_workers`.
    let should_spawn = {
        let mut w = shared
            .work_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if w.spawned_io_workers < shared.max_workers {
            w.spawned_io_workers += 1;
            w.active_io_workers += 1;
            true
        } else {
            false
        }
    };
    if should_spawn {
        let s = Arc::clone(shared);
        let handle = thread::spawn(move || run_process_io_worker(s));
        shared
            .io_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(handle);
    }
    Ok(())
}

fn default_process_max_workers() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    #[cfg(windows)]
    {
        cpus.min(MAX_WINDOWS_WORKERS)
    }
    #[cfg(not(windows))]
    {
        cpus
    }
}

fn build_worker_config_frame(
    interp: &mut Interpreter,
    initializer: Option<Value>,
    initargs: Vec<Value>,
    max_tasks_per_child: Option<usize>,
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

    let init_bytes_val = match initializer {
        None => Value::none(),
        Some(fn_val) => {
            let tuple = Value::tuple(vec![fn_val, Value::tuple(initargs)]);
            let bytes = pickle_dumps(interp, tuple)?;
            Value::bytes(bytes)
        }
    };

    let max_tasks_val = match max_tasks_per_child {
        Some(n) => Value::int(n as i64),
        None => Value::none(),
    };

    let cfg_tuple = Value::tuple(vec![
        Value::list(path_items),
        main_file,
        init_bytes_val,
        max_tasks_val,
    ]);
    pickle_dumps(interp, cfg_tuple)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let base_mod = interp.load_module("concurrent.futures._base")?;
    let broken_base = interp.get_attr(&base_mod, "BrokenExecutor")?;
    let exec_base = interp.get_attr(&base_mod, "Executor")?;

    let broken_cls = module.borrow().get_attr_value("BrokenProcessPool");
    if let Some(val) = broken_cls
        && let ValueKind::PyClass(cls_rc) = val.kind()
        && let ValueKind::PyClass(base_rc) = broken_base.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(base_rc)]);
    }

    let pool_cls = module.borrow().get_attr_value("ProcessPoolExecutor");
    if let Some(val) = pool_cls
        && let ValueKind::PyClass(cls_rc) = val.kind()
        && let ValueKind::PyClass(base_rc) = exec_base.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(base_rc)]);
    }
    Ok(())
}

pyrust_module! {
    /// Read one length-prefixed binary IPC frame from raw stdin (used by worker subprocesses).
    fn _worker_recv_frame() -> Result<Value> {
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
            Ok(Some(bytes)) => Ok(Value::bytes(bytes)),
            Ok(None) => Ok(Value::none()),
            Err(e) => Err(PyError::from_io_error(&e, None)),
        }
    }

    /// Write one length-prefixed binary IPC frame to raw stdout (used by worker subprocesses).
    fn _worker_send_frame(data: PyBytesLike<'_>) -> Result<Value> {
        let payload = data.as_bytes().to_vec();
        let res: std::io::Result<()> = _interp.release_gil_during(|| {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let hdr = (payload.len() as u32).to_be_bytes();
            lock.write_all(&hdr)?;
            lock.write_all(&payload)?;
            lock.flush()
        });
        res.map_err(|e| PyError::from_io_error(&e, None))?;
        Ok(Value::none())
    }

    /// Raised when a process in a ProcessPoolExecutor terminates abruptly.
    exception BrokenProcessPool(RuntimeError);

    #[native_state(ProcessDispatcherState)]
    class _ProcessDispatcher {
        fn __call__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;
            run_process_dispatcher_loop(_interp, shared, py)?;
            Ok(Value::none())
        }
    }

    #[native_state(ProcessPoolState)]
    class ProcessPoolExecutor {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] max_workers: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] mp_context: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] initializer: OptValueRef<'_>,
            #[default(PyValue(Value::tuple(Vec::new())))] initargs: PyValue,
            #[default(OptionalArg::Omitted)] max_tasks_per_child: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = mp_context;
            let max_w = match max_workers.as_value() {
                None => default_process_max_workers(),
                Some(v) if v.is_none() => default_process_max_workers(),
                Some(v) => {
                    let idx = PyIndex::try_from_value_with_interp(
                        _interp,
                        v,
                        "ProcessPoolExecutor",
                        "max_workers",
                    )?;
                    let n = idx.expect_i64("ProcessPoolExecutor", "max_workers")?;
                    if n <= 0 {
                        return Err(PyError::named(
                            "ValueError",
                            "max_workers must be greater than 0",
                        ));
                    }
                    #[cfg(windows)]
                    if n as usize > MAX_WINDOWS_WORKERS {
                        return Err(PyError::named(
                            "ValueError",
                            format!("max_workers must be <= {MAX_WINDOWS_WORKERS}"),
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

            let max_tasks = match max_tasks_per_child.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => {
                    let idx = PyIndex::try_from_value_with_interp(
                        _interp,
                        v,
                        "ProcessPoolExecutor",
                        "max_tasks_per_child",
                    )?;
                    let n = idx.expect_i64("ProcessPoolExecutor", "max_tasks_per_child")?;
                    if n <= 0 {
                        return Err(PyError::named(
                            "ValueError",
                            "max_tasks_per_child must be >= 1",
                        ));
                    }
                    Some(n as usize)
                }
            };

            let exe_path = std::env::current_exe().map_err(|e| {
                PyError::named("RuntimeError", format!("cannot determine sys.executable: {e}"))
            })?;
            let config_frame = build_worker_config_frame(_interp, init_fn, init_args, max_tasks)?;

            let state = ProcessPoolState {
                shared: Arc::new(ProcPoolShared {
                    exe_path,
                    config_frame,
                    max_workers: max_w,
                    max_tasks_per_child: max_tasks,
                    work_lock: Mutex::new(ProcSharedQueue {
                        pending: VecDeque::new(),
                        shutdown: false,
                        broken: None,
                        active_io_workers: 0,
                        spawned_io_workers: 0,
                    }),
                    work_cvar: Condvar::new(),
                    done_lock: Mutex::new(ProcCompletedQueue {
                        messages: VecDeque::new(),
                        all_workers_done: false,
                    }),
                    done_cvar: Condvar::new(),
                    io_handles: Mutex::new(Vec::new()),
                }),
                py: Rc::new(RefCell::new(ProcPoolPyState {
                    next_id: 1,
                    pending_futures: HashMap::new(),
                    dispatcher_thread: None,
                })),
            };
            set_state(self_val.0, state)?;
            Ok(Value::none())
        }

        #[getter]
        fn _max_workers(#[state] st: &ProcessPoolState) -> Result<Value> {
            Ok(Value::int(st.shared.max_workers as i64))
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
                let w = shared
                    .work_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(ref reason) = w.broken {
                    return Err(raise_broken_process_pool(_interp, reason));
                }
                if w.shutdown {
                    return Err(PyError::named(
                        "RuntimeError",
                        "cannot schedule new futures after shutdown",
                    ));
                }
            }

            let fut = cf_base::new_future_instance(_interp)?;
            let task_id = {
                let mut p = py.borrow_mut();
                let id = p.next_id;
                p.next_id = p.next_id.wrapping_add(1);
                id
            };

            let kw_dict = Value::dict(PyDict::default());
            for (k, v) in kwargs {
                kw_dict.dict_insert(PyKey::str_from(&k), v)?;
            }
            let req_tuple = Value::tuple(vec![
                Value::int(task_id as i64),
                func.0.clone(),
                Value::tuple(args.to_vec()),
                kw_dict,
            ]);

            let payload = match pickle_dumps(_interp, req_tuple) {
                Ok(b) => b,
                Err(err) => {
                    let exc = _interp
                        .materialize_pyerror(err)
                        .unwrap_or_else(|e| Value::string(e.to_string()));
                    let _ = cf_base::future_set_exception(_interp, &fut, exc);
                    return Ok(fut);
                }
            };

            py.borrow_mut().pending_futures.insert(task_id, fut.clone());
            ensure_process_workers_started(_interp, &shared, &py)?;

            {
                let mut w = shared
                    .work_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                w.pending.push_back(ProcTask { task_id, payload });
                shared.work_cvar.notify_one();
            }

            Ok(fut)
        }

        fn map(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValueRef<'_>,
            #[varargs] iterables: &[Value],
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
            #[default(1i64.into())] chunksize: PyIndex,
        ) -> Result<Value> {
            let cs = chunksize.expect_i64("ProcessPoolExecutor.map", "chunksize")?;
            if cs < 1 {
                return Err(PyError::named("ValueError", "chunksize must be >= 1."));
            }
            let base_mod = _interp.load_module("concurrent.futures._base")?;
            let exec_cls = _interp.get_attr(&base_mod, "Executor")?;
            let base_map = _interp.get_attr(&exec_cls, "map")?;
            let mut call_args = Vec::with_capacity(iterables.len() + 4);
            call_args.push(ExpandedCallArg {
                name: None,
                value: self_val.0.clone(),
            });
            call_args.push(ExpandedCallArg {
                name: None,
                value: func.0.clone(),
            });
            for it in iterables {
                call_args.push(ExpandedCallArg {
                    name: None,
                    value: it.clone(),
                });
            }
            if let Some(t) = timeout.as_value() {
                call_args.push(ExpandedCallArg {
                    name: Some("timeout".to_string()),
                    value: t.clone(),
                });
            }
            call_args.push(ExpandedCallArg {
                name: Some("chunksize".to_string()),
                value: Value::int(cs),
            });
            _interp.call_function_expanded(base_map, &call_args)
        }

        fn shutdown(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(true.into())] wait: PyBool,
            #[keyword_only] #[default(false.into())] cancel_futures: PyBool,
        ) -> Result<Value> {
            let (shared, py) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.shared), Rc::clone(&st.py)))
            })?;

            let (cancelled_ids, no_workers) = {
                let mut w = shared
                    .work_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                w.shutdown = true;
                let ids: Vec<u64> = if cancel_futures.0 {
                    w.pending.drain(..).map(|t| t.task_id).collect()
                } else {
                    Vec::new()
                };
                let no_w = w.active_io_workers == 0;
                shared.work_cvar.notify_all();
                (ids, no_w)
            };

            if no_workers {
                let mut d = shared
                    .done_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                d.all_workers_done = true;
                shared.done_cvar.notify_all();
            }

            if !cancelled_ids.is_empty() {
                let mut to_cancel = Vec::with_capacity(cancelled_ids.len());
                {
                    let mut p = py.borrow_mut();
                    for id in cancelled_ids {
                        if let Some(f) = p.pending_futures.remove(&id) {
                            to_cancel.push(f);
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
            }

            Ok(Value::none())
        }
    }
}
