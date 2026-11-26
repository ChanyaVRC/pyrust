//! Shared runtime infrastructure for PyRust's 100% native-Rust `multiprocessing`
//! package (issue #3264).
//!
//! Architecture:
//! - [`broker`]: Single-port per-process multiplexed loopback IPC broker with
//!   16-byte CSPRNG auth token, supporting both cross-process `spawn` (Windows
//!   & Unix) and Unix `fork` without leaking OS named semaphores or exhausting
//!   ephemeral ports.
//! - [`ipc`]: Framed binary connections (`Pipe`, `Connection`, `Listener`,
//!   `Client`, `wait`, HMAC challenge-response) with transparent loopback relay
//!   when pickled to spawned child processes.
//! - [`queue`]: Cross-process `Queue`, `SimpleQueue`, and `JoinableQueue`
//!   cores backed by GIL-releasing `Condvar`s in-process and the IPC broker
//!   across processes.
//! - [`sync`]: Cross-process `SemLock` / `Lock` / `RLock` / `Semaphore` /
//!   `BoundedSemaphore` / `Event` / `Condition` / `Barrier` primitives.
//! - [`shm`]: Real kernel shared memory (`shm_open`+`mmap` on Unix;
//!   `CreateFileMappingW`+`MapViewOfFile` on Windows) powering `SharedMemory`,
//!   `ShareableList`, `Value`, and `Array`.
//! - [`proc`]: Cross-platform `'spawn'` subprocess runner (matching CPython's
//!   `__mp_main__` guard semantics) and `#[cfg(unix)]` `'fork'` runner.

pub(crate) mod broker;
pub(crate) mod ipc;
pub(crate) mod proc;
pub(crate) mod queue;
pub(crate) mod shm;
pub(crate) mod sync;

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use pyrust_core::gil_local;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyDict, PyKey, Value, ValueKind};

/// Polling slice for GIL-releasing blocking loops so daemon threads notice
/// interpreter shutdown promptly.
pub(crate) const SHUTDOWN_POLL_SLICE: Duration = Duration::from_millis(20);

/// Platform-matching default start method name.
pub(crate) fn platform_default_start_method() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "fork"
    }
    #[cfg(not(target_os = "linux"))]
    {
        "spawn"
    }
}

/// Platform-matching list of all supported start methods (CPython 3.12 order).
pub(crate) fn platform_all_start_methods() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    {
        &["fork", "spawn", "forkserver"]
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        &["spawn", "fork", "forkserver"]
    }
    #[cfg(windows)]
    {
        &["spawn"]
    }
}

/// Per-interpreter multiprocessing runtime state (shared across Python threads
/// under one GIL via `gil_local!`, isolated across independent interpreters).
pub(crate) struct MpInterpreterState {
    pub start_method: Option<String>,
    pub executable: Option<PathBuf>,
    pub current_process: Option<Value>,
    pub parent_process: Option<Value>,
    pub active_children: Vec<Value>,
    pub process_counter: u64,
    pub authkey: Vec<u8>,
    pub forkserver_preload: Vec<String>,
    pub after_fork_callbacks: Vec<(u64, Value)>,
    pub next_after_fork_id: u64,
    pub temp_dir: Option<PathBuf>,
}

impl MpInterpreterState {
    fn new() -> Self {
        Self {
            start_method: None,
            executable: None,
            current_process: None,
            parent_process: None,
            active_children: Vec::new(),
            process_counter: 0,
            authkey: random_bytes(32),
            forkserver_preload: Vec::new(),
            after_fork_callbacks: Vec::new(),
            next_after_fork_id: 1,
            temp_dir: None,
        }
    }
}

gil_local! {
    pub(crate) static MP_STATE: RefCell<MpInterpreterState> = RefCell::new(MpInterpreterState::new());
}

/// Generate `n` cryptographically strong random bytes.
pub(crate) fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    if getrandom::getrandom(&mut buf).is_err() {
        // Deterministic fallback only if OS CSPRNG somehow fails.
        let seed = std::process::id() as u64 ^ 0x9e37_79b9_7f4a_7c15;
        let mut x = seed;
        for b in &mut buf {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            *b = (x >> 56) as u8;
        }
    }
    buf
}

/// Generate a random 16-byte authentication token.
pub(crate) fn random_token_16() -> [u8; 16] {
    let bytes = random_bytes(16);
    let mut out = [0u8; 16];
    out.copy_from_slice(&bytes);
    out
}

/// Generate a random lowercase hex suffix of `n_bytes` random bytes (`2 * n_bytes` hex chars).
pub(crate) fn random_hex(n_bytes: usize) -> String {
    let bytes = random_bytes(n_bytes);
    let mut out = String::with_capacity(n_bytes * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Resolve the Python executable path (`set_executable` override or `current_exe`).
pub(crate) fn get_executable_path() -> Result<PathBuf> {
    let override_exe = MP_STATE.with(|s| s.borrow().executable.clone());
    if let Some(p) = override_exe {
        return Ok(p);
    }
    std::env::current_exe().map_err(|e| {
        PyError::named(
            "RuntimeError",
            format!("cannot determine sys.executable: {e}"),
        )
    })
}

/// Update the executable used for `'spawn'` child processes.
pub(crate) fn set_executable_path(path: PathBuf) {
    MP_STATE.with(|s| {
        s.borrow_mut().executable = Some(path);
    });
}

/// Get the configured start method, setting it to the platform default unless
/// `allow_none` is true.
pub(crate) fn get_start_method_impl(allow_none: bool) -> Option<String> {
    MP_STATE.with(|s| {
        let mut st = s.borrow_mut();
        if st.start_method.is_none() && !allow_none {
            st.start_method = Some(platform_default_start_method().to_string());
        }
        st.start_method.clone()
    })
}

/// Set the default start method, raising `RuntimeError` if already set and
/// `force == false`, or `ValueError` for unknown methods.
pub(crate) fn set_start_method_impl(method: &str, force: bool) -> Result<()> {
    if !platform_all_start_methods().contains(&method) {
        return Err(PyError::named(
            "ValueError",
            format!("cannot find context for '{method}'"),
        ));
    }
    MP_STATE.with(|s| {
        let mut st = s.borrow_mut();
        if st.start_method.is_some() && !force {
            return Err(PyError::named(
                "RuntimeError",
                "context has already been set",
            ));
        }
        st.start_method = Some(method.to_string());
        Ok(())
    })
}

/// Increment and return the next process counter value (`1, 2, 3, ...`).
pub(crate) fn next_process_number() -> u64 {
    MP_STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.process_counter += 1;
        st.process_counter
    })
}

/// Return the current process's authkey bytes.
pub(crate) fn current_authkey() -> Vec<u8> {
    MP_STATE.with(|s| s.borrow().authkey.clone())
}

/// Set the current process's authkey bytes.
pub(crate) fn set_current_authkey(key: Vec<u8>) {
    MP_STATE.with(|s| {
        s.borrow_mut().authkey = key;
    });
}

/// Call a Python callable with positional arguments.
pub(crate) fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

/// Call a Python callable with positional and keyword arguments.
pub(crate) fn call_pos_kw(
    interp: &mut Interpreter,
    callee: Value,
    args: &[Value],
    kwargs: &[(String, Value)],
) -> Result<Value> {
    let mut expanded = Vec::with_capacity(args.len() + kwargs.len());
    for v in args {
        expanded.push(ExpandedCallArg {
            name: None,
            value: v.clone(),
        });
    }
    for (k, v) in kwargs {
        expanded.push(ExpandedCallArg {
            name: Some(k.clone()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

/// Serialize a Python value with `pickle.dumps`.
pub(crate) fn pickle_dumps(interp: &mut Interpreter, val: Value) -> Result<Vec<u8>> {
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

/// Deserialize a Python value with `pickle.loads`, mapping `__main__` to
/// `__mp_main__` when inside a spawned worker process.
pub(crate) fn pickle_loads(interp: &mut Interpreter, data: &[u8]) -> Result<Value> {
    let sys_mod = interp.load_module("sys")?;
    let has_mp_main = interp
        .get_attr(&sys_mod, "modules")
        .ok()
        .and_then(|m| {
            m.as_dict()
                .map(|d| d.contains_key(&PyKey::str_from("__mp_main__")))
        })
        .unwrap_or(false);

    if has_mp_main {
        let spawn_mod = interp.load_module("multiprocessing.spawn")?;
        if let Ok(loads_fn) = interp.get_attr(&spawn_mod, "_worker_loads") {
            return call_pos(interp, loads_fn, &[Value::bytes(data.to_vec())]);
        }
    }
    let pickle_mod = interp.load_module("pickle")?;
    let loads_fn = interp.get_attr(&pickle_mod, "loads")?;
    call_pos(interp, loads_fn, &[Value::bytes(data.to_vec())])
}

/// Parse an optional Python timeout (`None`, `int`, `float`, `bool`) into
/// `Option<Duration>`. Negative values clamp to zero unless `reject_negative`.
pub(crate) fn parse_timeout(
    val: Option<&Value>,
    reject_negative: bool,
) -> Result<Option<Duration>> {
    let Some(v) = val else {
        return Ok(None);
    };
    if v.is_none() {
        return Ok(None);
    }
    let secs = match v.kind() {
        ValueKind::Float(f) => f,
        ValueKind::Int(i) => i as f64,
        ValueKind::Bool(b) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "timeout must be a number or None, not '{}'",
                    value_type_name_str(v)
                ),
            ));
        }
    };
    if secs.is_nan() {
        return Err(PyError::named("ValueError", "timeout must not be NaN"));
    }
    if secs < 0.0 {
        if reject_negative {
            return Err(PyError::named(
                "ValueError",
                "'timeout' must be a non-negative number",
            ));
        }
        return Ok(Some(Duration::ZERO));
    }
    if secs.is_infinite() {
        return Ok(None);
    }
    Ok(Some(Duration::from_secs_f64(secs.min(1e9))))
}

/// Construct and return a `PyError` raising an exception class from
/// `multiprocessing.context` (`ProcessError`, `BufferTooShort`, `TimeoutError`,
/// `AuthenticationError`).
pub(crate) fn raise_context_error(interp: &mut Interpreter, exc_name: &str, msg: &str) -> PyError {
    match (|| -> Result<Value> {
        let ctx_mod = interp.load_module("multiprocessing.context")?;
        let cls = interp.get_attr(&ctx_mod, exc_name)?;
        if msg.is_empty() {
            call_pos(interp, cls, &[])
        } else {
            call_pos(interp, cls, &[Value::string(msg)])
        }
    })() {
        Ok(inst) => PyError::raised(inst),
        Err(e) => e,
    }
}

/// Raise `queue.Empty` or `queue.Full`.
pub(crate) fn raise_queue_error(interp: &mut Interpreter, exc_name: &str) -> PyError {
    match (|| -> Result<Value> {
        let q_mod = interp.load_module("queue")?;
        let cls = interp.get_attr(&q_mod, exc_name)?;
        call_pos(interp, cls, &[])
    })() {
        Ok(inst) => PyError::raised(inst),
        Err(e) => e,
    }
}

/// Helper to build a Python `dict` from an `IndexMap<String, Value>`.
pub(crate) fn indexmap_to_pydict(map: indexmap::IndexMap<String, Value>) -> Result<Value> {
    let d = Value::dict(PyDict::default());
    for (k, v) in map {
        d.dict_insert(PyKey::str_from(&k), v)?;
    }
    Ok(d)
}

/// Return the number of logical CPUs (`multiprocessing.cpu_count()`).
pub(crate) fn cpu_count_value() -> Result<i64> {
    std::thread::available_parallelism()
        .map(|n| n.get() as i64)
        .map_err(|_| PyError::named("NotImplementedError", "cannot determine number of cpus"))
}

/// Dunder-aware Python equality check (`a == b`).
pub(crate) fn py_eq(interp: &mut Interpreter, a: Value, b: Value) -> Result<bool> {
    let op = interp.load_module("operator")?;
    let eq_fn = interp.get_attr(&op, "eq")?;
    let res = call_pos(interp, eq_fn, &[a, b])?;
    interp.truthy_value(&res)
}

/// Dunder-aware Python membership test (`item in container`).
pub(crate) fn py_contains(interp: &mut Interpreter, container: Value, item: Value) -> Result<bool> {
    let res = interp.eval_in(container, item)?;
    interp.truthy_value(&res)
}

/// Dunder-aware Python indexing (`obj[key]`).
pub(crate) fn py_getitem(interp: &mut Interpreter, obj: Value, key: Value) -> Result<Value> {
    let op = interp.load_module("operator")?;
    let f = interp.get_attr(&op, "getitem")?;
    call_pos(interp, f, &[obj, key])
}

/// Dunder-aware Python item assignment (`obj[key] = val`).
pub(crate) fn py_setitem(
    interp: &mut Interpreter,
    obj: Value,
    key: Value,
    val: Value,
) -> Result<()> {
    let op = interp.load_module("operator")?;
    let f = interp.get_attr(&op, "setitem")?;
    let _ = call_pos(interp, f, &[obj, key, val])?;
    Ok(())
}

/// Dunder-aware Python item deletion (`del obj[key]`).
pub(crate) fn py_delitem(interp: &mut Interpreter, obj: Value, key: Value) -> Result<()> {
    let op = interp.load_module("operator")?;
    let f = interp.get_attr(&op, "delitem")?;
    let _ = call_pos(interp, f, &[obj, key])?;
    Ok(())
}
