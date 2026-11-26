// `threading` module — body for the `threading @inject` entry in
// `pyrust_builtin_modules!` (issues #3208, #3263).
//
// Real multi-threaded Python 3.12 synchronization primitives (`Lock`, `RLock`,
// `Condition`, `Event`, `Semaphore`, `BoundedSemaphore`, `Barrier`,
// `BrokenBarrierError`, `local`, `Thread`, `Timer`, `current_thread`,
// `main_thread`, `get_ident`, `get_native_id`, `active_count`, `enumerate`,
// `TIMEOUT_MAX`, `ThreadError`) backed by real OS threads (`std::thread`) and
// GIL-releasing native synchronization handles (`NativeLock`, `ThreadJoinState`).
//
// Reference: <https://docs.python.org/3/library/threading.html>

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use pyrust_core::{BuiltinState, BuiltinTypeOps, builtin_ops_is};
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, NativeLock, ThreadJoinState, current_os_native_id,
    current_os_thread_ident, reject_keyword_args_expanded, value_type_name_str,
};
use crate::value::{PyKey, Value, ValueKind};

/// Python-source definitions for the `threading` module.
const THREADING_PY_SOURCE: &str = include_str!("threading_py.py");

/// Private native helpers moved into the exec namespace and removed from
/// `dir(threading)` so the public module surface matches CPython 3.12.
const THREADING_NATIVE_HELPERS: &[&str] = &[
    "_native_get_ident",
    "_native_get_native_id",
    "_native_lock_new",
    "_native_lock_acquire",
    "_native_lock_release",
    "_native_lock_locked",
    "_native_lock_reinit",
    "_native_start_thread",
    "_native_join_thread",
    "_native_thread_is_done",
    "_native_is_shutting_down",
];

/// Public names exported from `THREADING_PY_SOURCE` onto the `threading` module.
const THREADING_PY_EXPORTS: &[&str] = &[
    "__all__",
    "TIMEOUT_MAX",
    "ThreadError",
    "BrokenBarrierError",
    "Lock",
    "RLock",
    "Condition",
    "Event",
    "Semaphore",
    "BoundedSemaphore",
    "Barrier",
    "local",
    "Thread",
    "Timer",
    "ExceptHookArgs",
    "excepthook",
    "__excepthook__",
    "get_ident",
    "get_native_id",
    "current_thread",
    "currentThread",
    "main_thread",
    "active_count",
    "activeCount",
    "enumerate",
    "stack_size",
    "settrace",
    "gettrace",
    "setprofile",
    "getprofile",
    "settrace_all_threads",
    "setprofile_all_threads",
    "_MainThread",
    "_RLock",
    "_PyRLock",
    "_LockType",
    "_allocate_lock",
];

/// Platform `TIMEOUT_MAX` matching CPython 3.12's `_thread.TIMEOUT_MAX`.
#[cfg(windows)]
const PLATFORM_TIMEOUT_MAX: f64 = 4_294_967.0;

#[cfg(not(windows))]
const PLATFORM_TIMEOUT_MAX: f64 = 9_223_372_036.0;

// ── Native lock & thread-join handle objects ─────────────────────────────────

struct NativeLockOps;
static NATIVE_LOCK_OPS: NativeLockOps = NativeLockOps;

impl BuiltinTypeOps for NativeLockOps {
    fn type_name(&self) -> &'static str {
        "_thread._NativeLock"
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let locked = state
            .borrow()
            .downcast_ref::<Arc<NativeLock>>()
            .is_some_and(|l| l.is_locked());
        let status = if locked { "locked" } else { "unlocked" };
        format!("<{status} _thread._NativeLock object>")
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }
}

struct ThreadJoinOps;
static THREAD_JOIN_OPS: ThreadJoinOps = ThreadJoinOps;

impl BuiltinTypeOps for ThreadJoinOps {
    fn type_name(&self) -> &'static str {
        "_thread._ThreadHandle"
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<_thread._ThreadHandle object>".to_string()
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }
}

fn extract_native_lock(val: &Value, fn_name: &str) -> Result<Arc<NativeLock>> {
    if let ValueKind::BuiltinObject { ops, state } = val.kind()
        && builtin_ops_is::<NativeLockOps>(ops)
        && let Some(lock) = state.borrow().downcast_ref::<Arc<NativeLock>>()
    {
        return Ok(Arc::clone(lock));
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "{fn_name}(): expected native lock handle, got '{}'",
            value_type_name_str(val)
        ),
    ))
}

fn extract_thread_handle(val: &Value, fn_name: &str) -> Result<Arc<ThreadJoinState>> {
    if let ValueKind::BuiltinObject { ops, state } = val.kind()
        && builtin_ops_is::<ThreadJoinOps>(ops)
        && let Some(handle) = state.borrow().downcast_ref::<Arc<ThreadJoinState>>()
    {
        return Ok(Arc::clone(handle));
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "{fn_name}(): expected thread handle, got '{}'",
            value_type_name_str(val)
        ),
    ))
}

fn value_to_f64(val: &Value, fn_name: &str) -> Result<f64> {
    match val.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(i) => Ok(i as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}(): must be real number, not {}",
                value_type_name_str(val)
            ),
        )),
    }
}

/// Execute `THREADING_PY_SOURCE` once and copy its exported attributes onto the
/// `threading` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    ns.dict_insert(
        PyKey::str_from("_PLATFORM_TIMEOUT_MAX"),
        Value::float(PLATFORM_TIMEOUT_MAX),
    )?;
    {
        let mut module_ref = module.borrow_mut();
        for &helper in THREADING_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(THREADING_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("threading: exec namespace not a dict".into()))?;
    for name in THREADING_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind()
                && *name != "ThreadError"
            {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("threading"));
            }
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {
    /// Return a positive integer identifier for the current OS thread.
    fn _native_get_ident(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        Ok(Value::int(current_os_thread_ident()))
    }

    /// Return the native OS thread identifier for the current OS thread.
    fn _native_get_native_id(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        Ok(Value::int(current_os_native_id()))
    }

    /// Create a new GIL-releasing native lock handle.
    fn _native_lock_new(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        Ok(Value::builtin_object(&NATIVE_LOCK_OPS, Box::new(NativeLock::new())))
    }

    /// Acquire a native lock handle (`handle, blocking, timeout`).
    fn _native_lock_acquire(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expects 3 arguments ({} given)", args.len()),
            ));
        }
        // Extract Arc and drop all RefCell borrows BEFORE calling `lock.acquire`,
        // which releases the GIL while waiting.
        let lock = extract_native_lock(&args[0].value, FN_NAME)?;
        let blocking = _interp.truthy_value(&args[1].value)?;
        let timeout_f = value_to_f64(&args[2].value, FN_NAME)?;
        let timeout = if timeout_f < 0.0 {
            None
        } else {
            Some(timeout_f)
        };
        let acquired = lock.acquire(_interp, blocking, timeout)?;
        Ok(Value::bool_(acquired))
    }

    /// Release a native lock handle.
    fn _native_lock_release(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expects 1 argument ({} given)", args.len()),
            ));
        }
        let lock = extract_native_lock(&args[0].value, FN_NAME)?;
        lock.release()?;
        Ok(Value::none())
    }

    /// Return whether a native lock handle is currently locked.
    fn _native_lock_locked(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expects 1 argument ({} given)", args.len()),
            ));
        }
        let lock = extract_native_lock(&args[0].value, FN_NAME)?;
        Ok(Value::bool_(lock.is_locked()))
    }

    /// Reset a native lock handle to unlocked (`_at_fork_reinit`).
    fn _native_lock_reinit(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expects 1 argument ({} given)", args.len()),
            ));
        }
        let lock = extract_native_lock(&args[0].value, FN_NAME)?;
        lock.reset_unlocked();
        Ok(Value::none())
    }

    /// Spawn a real OS thread running `bootstrap_fn()`.
    fn _native_start_thread(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expects 3 arguments ({} given)", args.len()),
            ));
        }
        let bootstrap_fn = args[0].value.clone();
        let is_daemon = _interp.truthy_value(&args[1].value)?;
        let stack_size = match args[2].value.kind() {
            ValueKind::Int(n) if n >= 0 => n as usize,
            _ => 0,
        };
        let handle = _interp.spawn_python_thread(bootstrap_fn, is_daemon, stack_size)?;
        Ok(Value::builtin_object(&THREAD_JOIN_OPS, Box::new(handle)))
    }

    /// Join a spawned OS thread handle (`handle, timeout_or_none`).
    fn _native_join_thread(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expects 2 arguments ({} given)", args.len()),
            ));
        }
        let handle = extract_thread_handle(&args[0].value, FN_NAME)?;
        let timeout = if args[1].value.is_none() {
            None
        } else {
            Some(value_to_f64(&args[1].value, FN_NAME)?)
        };
        let finished = handle.join(_interp, timeout)?;
        Ok(Value::bool_(finished))
    }

    /// Return true if a spawned OS thread has finished executing.
    fn _native_thread_is_done(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expects 1 argument ({} given)", args.len()),
            ));
        }
        let handle = extract_thread_handle(&args[0].value, FN_NAME)?;
        Ok(Value::bool_(handle.is_done()))
    }

    /// Return true if the interpreter is shutting down daemon threads.
    fn _native_is_shutting_down(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        Ok(Value::bool_(_interp.cold.gil_state.is_shutting_down()))
    }
}
