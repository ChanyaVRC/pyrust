// `threading` module — body for the `threading @inject` entry in
// `pyrust_builtin_modules!` (issue #3208).
//
// Single-interpreter-safe Python 3.12 synchronization primitives (`Lock`,
// `RLock`, `Condition`, `Event`, `Semaphore`, `BoundedSemaphore`, `Barrier`,
// `BrokenBarrierError`, `local`, `Thread`, `Timer`, `current_thread`,
// `main_thread`, `get_ident`, `get_native_id`, `active_count`, `enumerate`,
// `TIMEOUT_MAX`, `ThreadError`) backed by a safe Rust thread-id helper.
//
// PyRust's `Interpreter` is single-threaded (`Rc<RefCell<...>>`), so no
// cross-thread `Rc` sharing is performed.
//
// Reference: <https://docs.python.org/3/library/threading.html>

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `threading` module.
const THREADING_PY_SOURCE: &str = include_str!("threading_py.py");

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

/// Return a stable positive integer identifier for the current OS thread
/// without requiring any `unsafe` platform FFI.
fn current_thread_ident() -> i64 {
    let mut hasher = DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    // Keep strictly positive within signed 63-bit range so it fits in `i64`.
    let raw = (hasher.finish() & 0x7fff_ffff_ffff_ffff) as i64;
    if raw == 0 { 1 } else { raw }
}

/// Platform `TIMEOUT_MAX` matching CPython 3.12's `_thread.TIMEOUT_MAX`.
#[cfg(windows)]
const PLATFORM_TIMEOUT_MAX: f64 = 4_294_967.0;

#[cfg(not(windows))]
const PLATFORM_TIMEOUT_MAX: f64 = 9_223_372_036.0;

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
    if let Some(native_ident) = module.borrow().attrs.get("_native_get_ident").cloned() {
        ns.dict_insert(PyKey::str_from("_native_get_ident"), native_ident)?;
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
    module.borrow_mut().attrs.shift_remove("_native_get_ident");
    Ok(Some(ns.clone()))
}

pyrust_module! {
    /// Internal helper returning a positive integer identifier for the current OS thread.
    fn _native_get_ident(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        Ok(Value::int(current_thread_ident()))
    }
}
