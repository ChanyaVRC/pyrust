// Minimal `asyncio` module (issue #1039) — included into `pub mod asyncio { … }`
// declared by the `pyrust_builtin_modules!` invocation in
// `builtin_modules/mod.rs`.
//
// Scope (MVP): `asyncio.run(coro)` drives a top-level coroutine to completion
// via the generator-based await machinery; `asyncio.sleep` / `asyncio.gather`
// are defined in `asyncio_py.py` (injected by `inject_python_members`).  There
// is no real event loop / timer / I/O scheduling — coroutines that only await
// other coroutines run correctly; `sleep` resolves immediately.  See the PR's
// follow-up issues for the deferred parts.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-level members of the module (`sleep`, `gather`), defined in terms of
/// real `async def` coroutines so they are driven by the same await machinery
/// as user coroutines.
const ASYNCIO_PY_SOURCE: &str = include_str!("asyncio_py.py");

/// Names from `ASYNCIO_PY_SOURCE` exported onto the `asyncio` module.
const ASYNCIO_PY_EXPORTS: [&str; 2] = ["sleep", "gather"];

/// Execute `ASYNCIO_PY_SOURCE` once and copy its public names onto the
/// `asyncio` module's attribute map.  Wired from `env.rs::load_module`'s
/// post-import hook (mirrors the `collections` injection, issue #1884).
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
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

pyrust_module! {
    /// `asyncio.run(coro)` — run the coroutine `coro` to completion and return
    /// its result (issue #1039).
    ///
    /// This is the minimal event loop: it repeatedly steps the coroutine until
    /// it returns.  A non-coroutine argument raises
    /// `ValueError: a coroutine was expected, got <repr>` to match CPython.
    /// The `debug=` keyword is accepted and ignored (there is no debug event
    /// loop to configure in the MVP).
    fn run(args) -> Result<Value> {
        let positional: Vec<&ExpandedCallArg> =
            args.iter().filter(|a| a.name.is_none()).collect();
        if positional.len() != 1 {
            return Err(crate::error::PyError::named(
                "TypeError",
                format!(
                    "run() takes 1 positional argument but {} were given",
                    positional.len()
                ),
            ));
        }
        let coro = positional[0].value.clone();
        // An async generator (`async def` containing `yield`, #2280) is
        // coroutine-tagged but is not a runnable coroutine: CPython's
        // `asyncio.run` rejects it with the same ValueError as any non-coroutine.
        if !crate::builtin_modules::builtins::is_coroutine_value(&coro)
            || crate::builtin_modules::builtins::is_async_generator_value(&coro)
        {
            // CPython raises ValueError with the argument's repr.
            let r = crate::builtin_modules::builtins::render_value_repr(_interp, &coro)?;
            return Err(crate::error::PyError::named(
                "ValueError",
                format!("a coroutine was expected, got {r}"),
            ));
        }
        _interp.drive_coroutine_to_completion(&coro)
    }
}
