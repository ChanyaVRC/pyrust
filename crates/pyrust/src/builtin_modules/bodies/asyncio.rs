// `asyncio` module — a real single-threaded event loop (issue #2281).
//
// Included into `pub mod asyncio { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs`.
//
// The event loop, `Future`, `Task`, `sleep`, `gather`, `create_task` and
// `ensure_future` are all defined in Python (`asyncio_py.py`), mirroring
// CPython's own mostly-Python asyncio.  This Rust file supplies the small
// native bridge the Python layer needs:
//
//   * `_step(coro, value)`   — resume a coroutine one step (send `value`),
//                              returning `(0, yielded)` if it suspended on an
//                              awaitable, `(1, result)` if it completed.  Any
//                              exception the coroutine raises propagates.
//   * `_throw(coro, exc)`    — like `_step` but injects `exc` at the current
//                              suspension point (used for task cancellation).
//   * `_iscoroutine(obj)`    — True for a real coroutine (not an async gen).
//   * `_monotonic()`         — monotonic clock as float seconds.
//   * `_sleep(seconds)`      — block the thread for `seconds` (the loop's idle
//                              wait when only timers are pending).
//
// The yield protocol: `Future.__await__` does `yield self` while pending; that
// `self` bubbles up the `YieldFrom`/`await` chain to the Task driver via
// `_step`, which the loop suspends on by registering a done-callback that
// re-schedules the Task once the Future resolves.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use crate::error::{PyError, Result};
use crate::interpreter::{CoroStep, ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-level members of the module, defined as real `async def` coroutines /
/// classes so they are driven by the same await machinery as user code.
const ASYNCIO_PY_SOURCE: &str = include_str!("asyncio_py.py");

/// Names from `ASYNCIO_PY_SOURCE` exported onto the `asyncio` module.
const ASYNCIO_PY_EXPORTS: [&str; 8] = [
    "sleep",
    "gather",
    "create_task",
    "ensure_future",
    "Future",
    "Task",
    "CancelledError",
    "_run_main",
];

thread_local! {
    /// Monotonic reference point so `_monotonic()` returns small float seconds
    /// (independent of the wall clock and stable for the process lifetime).
    static MONO_EPOCH: Instant = Instant::now();
}

/// Execute `ASYNCIO_PY_SOURCE` once and copy its public names onto the
/// `asyncio` module's attribute map.  Wired from `env.rs::load_module`'s
/// post-import hook (mirrors the `collections` injection, issue #1884).
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    // Pre-seed the exec namespace with the native bridge helpers so the
    // Python source can call them by bare name (`_step`, `_monotonic`, …).
    // They are registered as attributes of this module by `pyrust_module!`.
    {
        let m = module.borrow();
        ns.dict_with_mut(|d| {
            for name in ["_step", "_throw", "_iscoroutine", "_monotonic", "_sleep"] {
                if let Some(val) = m.attrs.get(name) {
                    d.insert(PyKey::str_from(name), val.clone());
                }
            }
        });
    }
    interp.exec_source(ASYNCIO_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| crate::error::PyError::Runtime("asyncio: exec namespace not a dict".into()))?;
    for name in ASYNCIO_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(())
}

/// Coerce a numeric `Value` (the validated `delay` of `sleep`) to `f64`.
fn arg_to_secs(v: &Value) -> f64 {
    match v.kind() {
        ValueKind::Float(f) => f,
        _ => v.as_int().map(|i| i as f64).unwrap_or(0.0),
    }
}

/// Read a named attribute off the (already-imported) `asyncio` module.
fn asyncio_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("asyncio")?;
    if let ValueKind::PyModule(m) = module.kind()
        && let Some(v) = m.borrow().attrs.get(name)
    {
        return Ok(v.clone());
    }
    Err(PyError::Runtime(format!("asyncio: {name} not loaded")))
}

pyrust_module! {
    /// `asyncio.run(coro)` — run the coroutine `coro` on a fresh event loop and
    /// return its result (issue #1039 / #2281).
    ///
    /// A non-coroutine argument raises
    /// `ValueError: a coroutine was expected, got <repr>` to match CPython.
    /// The `debug=` keyword is accepted and ignored.
    fn run(args) -> Result<Value> {
        let positional: Vec<&ExpandedCallArg> =
            args.iter().filter(|a| a.name.is_none()).collect();
        if positional.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "run() takes 1 positional argument but {} were given",
                    positional.len()
                ),
            ));
        }
        let coro = positional[0].value.clone();
        // An async generator (`async def` containing `yield`, #2280) is
        // coroutine-tagged but is not a runnable coroutine.
        if !crate::builtin_modules::builtins::is_coroutine_value(&coro)
            || crate::builtin_modules::builtins::is_async_generator_value(&coro)
        {
            let r = crate::builtin_modules::builtins::render_value_repr(_interp, &coro)?;
            return Err(PyError::named(
                "ValueError",
                format!("a coroutine was expected, got {r}"),
            ));
        }
        // A coroutine that has already run to completion cannot be re-run:
        // `asyncio.run(c)` on an already-driven coroutine raises
        // `RuntimeError("cannot reuse already awaited coroutine")` (issue
        // #2282).  (`await`-ing a done coroutine inside a running coro is
        // caught separately by `get_awaitable`.)
        if let ValueKind::Generator(state_rc) = coro.kind()
            && let Ok(b) = state_rc.try_borrow()
            && let Some(frame) = b.downcast_ref::<crate::interpreter::GeneratorFrame>()
            && frame.is_coroutine
            && frame.done
        {
            return Err(PyError::named(
                "RuntimeError",
                "cannot reuse already awaited coroutine".to_string(),
            ));
        }
        // Delegate to the Python event loop (`_run_main`), injected onto this
        // module.  `_run_main(coro)` builds a fresh loop, wraps `coro` in the
        // root Task and runs until it completes.
        let run_main = asyncio_attr(_interp, "_run_main")?;
        _interp.call_function_expanded(
            run_main,
            &[ExpandedCallArg { name: None, value: coro }],
        )
    }

    /// `asyncio._iscoroutine(obj)` — True for a true coroutine object (an
    /// `async def` call that is not an async generator).
    fn _iscoroutine(args) -> Result<Value> {
        let obj = &args[0].value;
        let is = crate::builtin_modules::builtins::is_coroutine_value(obj)
            && !crate::builtin_modules::builtins::is_async_generator_value(obj);
        Ok(Value::bool_(is))
    }

    /// `asyncio._monotonic()` — monotonic clock in float seconds.
    fn _monotonic(_args) -> Result<Value> {
        let secs = MONO_EPOCH.with(|e| e.elapsed().as_secs_f64());
        Ok(Value::float(secs))
    }

    /// `asyncio._sleep(seconds)` — block the calling thread for `seconds`.
    /// Used by the loop when it is idle but has a pending timer.
    fn _sleep(args) -> Result<Value> {
        let secs = arg_to_secs(&args[0].value);
        if secs > 0.0 {
            std::thread::sleep(std::time::Duration::from_secs_f64(secs));
        }
        Ok(Value::none())
    }

    /// `asyncio._step(coro, value)` — resume `coro` one step, sending `value`.
    /// Returns `(0, yielded)` on suspension, `(1, result)` on completion; a
    /// coroutine exception propagates.
    fn _step(args) -> Result<Value> {
        let coro = args[0].value.clone();
        let sent = args.get(1).map(|a| a.value.clone()).unwrap_or_else(Value::none);
        match _interp.coro_step(&coro, sent, None)? {
            CoroStep::Yielded(v) => Ok(Value::tuple(vec![Value::int(0), v])),
            CoroStep::Returned(v) => Ok(Value::tuple(vec![Value::int(1), v])),
        }
    }

    /// `asyncio._throw(coro, exc)` — resume `coro` injecting `exc` at its
    /// current suspension point (task cancellation).  Same return contract as
    /// `_step`.
    fn _throw(args) -> Result<Value> {
        let coro = args[0].value.clone();
        let exc = args[1].value.clone();
        let err = PyError::Raised(exc);
        match _interp.coro_step(&coro, Value::none(), Some(err))? {
            CoroStep::Yielded(v) => Ok(Value::tuple(vec![Value::int(0), v])),
            CoroStep::Returned(v) => Ok(Value::tuple(vec![Value::int(1), v])),
        }
    }
}
