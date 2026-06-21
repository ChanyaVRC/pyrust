// `contextlib` module — context management utilities.
//
// Implements the most-used parts of CPython's `contextlib`:
//
// - `suppress(*exceptions)` — context manager that swallows specific exceptions.
// - `contextmanager` — decorator that turns a generator function into a context
//   manager; the generator yields exactly once (its yielded value becomes the
//   `with … as` value), and exceptions thrown into the `with` body are forwarded
//   to the generator via `.throw()`.
// - `closing(thing)` — context manager that calls `thing.close()` on exit.
// - `nullcontext(enter_result=None)` — context manager that is a no-op; useful
//   as a placeholder.
// - `ExitStack` — dynamic stack of context managers and callbacks; allows
//   programmatically managing an arbitrary number of context managers.
//
// ## Context manager protocol
//
// The VM calls `ctx.__enter__()` at the start of the `with` block and
// `ctx.__exit__(exc_type, exc_val, traceback)` at the end.  On normal exit,
// all three arguments are `None`.  On exception exit, `exc_type` is
// `exc.__class__` (a `PyClass`), `exc_val` is the exception instance, and
// `traceback` is `None` (pyrust has no traceback objects).  If `__exit__`
// returns a truthy value the exception is suppressed; otherwise it propagates.
//
// Reference: <https://docs.python.org/3/library/contextlib.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::class_is_subclass_of;
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    // ── suppress ─────────────────────────────────────────────────────────────

    /// CPython: contextlib.suppress(*exceptions).
    /// Returns a context manager that suppresses any of the given exception
    /// types raised in the `with` body and resumes execution after the block.
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.suppress>
    class suppress {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let types: Vec<Value> = args[1..].iter().map(|a| a.value.clone()).collect();
            inst.borrow_mut()
                .attrs
                .insert("_types", Value::list(types));
            let _ = _interp;
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let _ = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::none())
        }

        /// Returns True (suppress) if the exception type is a subclass of any
        /// of the stored types; False otherwise.
        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            // args: [self, exc_type, exc_val, traceback]
            // exc_type is None on normal exit.
            let exc_type = args.get(1).map(|a| a.value.clone()).unwrap_or_else(Value::none);
            if exc_type.is_none() {
                let _ = _interp;
                return Ok(Value::bool_(false));
            }
            let raised_class = match exc_type.kind() {
                ValueKind::PyClass(c) => Rc::clone(c),
                _ => {
                    let _ = _interp;
                    return Ok(Value::bool_(false));
                }
            };
            let types_val = inst.borrow().attrs.get("_types").cloned().unwrap_or_else(|| Value::list(vec![]));
            let types = match types_val.kind() {
                ValueKind::List(items) => items.to_vec(),
                _ => vec![],
            };
            for t in &types {
                if let ValueKind::PyClass(expected) = t.kind()
                    && class_is_subclass_of(&raised_class, expected) {
                        let _ = _interp;
                        return Ok(Value::bool_(true));
                    }
            }
            let _ = _interp;
            Ok(Value::bool_(false))
        }
    }

    // ── _GeneratorContextManager ──────────────────────────────────────────────

    /// Internal context manager produced by `@contextmanager`.
    ///
    /// `_gen` holds the generator value.  `__enter__` advances the generator to
    /// its first yield, returning the yielded value.  `__exit__` either closes
    /// the generator (no exception) or throws the exception into it, suppressing
    /// the result if the generator raises `StopIteration`.
    class _GeneratorContextManager {
        fn __init__(args) -> Result<Value> {
            let _inst = expect_self(args, FN_NAME)?;
            // Private: constructed only by `contextmanager.__call__`.
            // args beyond self must be empty (the factory seeds attrs directly).
            let _ = _interp;
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let generator = inst.borrow().attrs.get("_gen").cloned().ok_or_else(|| {
                PyError::Runtime(format!("internal: {FN_NAME}() _gen missing"))
            })?;
            // Advance to the first yield; the yielded value becomes the `with … as` target.
            match _interp.call_next(&generator, None) {
                Ok(val) => Ok(val),
                Err(e) if is_stop_iteration(&e) => {
                    // Generator returned without yielding — protocol violation.
                    Err(PyError::named(
                        "RuntimeError",
                        "generator didn't yield".to_string(),
                    ))
                }
                Err(e) => Err(e),
            }
        }

        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let generator = inst.borrow().attrs.get("_gen").cloned().ok_or_else(|| {
                PyError::Runtime(format!("internal: {FN_NAME}() _gen missing"))
            })?;
            // args: [self, exc_type, exc_val, traceback]
            let exc_type = args.get(1).map(|a| a.value.clone()).unwrap_or_else(Value::none);
            let exc_val  = args.get(2).map(|a| a.value.clone()).unwrap_or_else(Value::none);
            if exc_type.is_none() {
                // Normal exit: advance the generator past its yield point.
                // The generator body should run until completion and raise
                // StopIteration; if it yields again, that is a protocol violation.
                match _interp.call_next(&generator, None) {
                    Ok(_) => {
                        // Generator yielded again — protocol violation.
                        Err(PyError::named(
                            "RuntimeError",
                            "generator didn't stop".to_string(),
                        ))
                    }
                    Err(e) if is_stop_iteration(&e) => {
                        // Generator finished normally — expected.
                        Ok(Value::bool_(false))
                    }
                    Err(e) => Err(e),
                }
            } else {
                // Exception exit: throw into the generator.
                // `exc_val` may be a PyInstance or None; `exc_type` is a PyClass.
                // We throw the instance (exc_val) if it is one, otherwise exc_type.
                let to_throw = if !exc_val.is_none() {
                    exc_val
                } else {
                    exc_type
                };
                match _interp.call_generator_method(generator, "throw", vec![to_throw]) {
                    Ok(_) => {
                        // Generator yielded again — that means it caught the exception
                        // and yielded a new value, which is forbidden by the protocol.
                        Err(PyError::named(
                            "RuntimeError",
                            "generator didn't stop after throw()".to_string(),
                        ))
                    }
                    Err(e) if is_stop_iteration(&e) => {
                        // Generator handled the exception and returned normally — suppress.
                        Ok(Value::bool_(true))
                    }
                    Err(e) => {
                        // Generator re-raised the exception or raised a new one.
                        // Let it propagate.
                        Err(e)
                    }
                }
            }
        }
    }

    /// CPython: contextlib.contextmanager(func).
    /// A decorator that turns a generator function into a factory of context
    /// managers.  The decorated function must `yield` exactly once.
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.contextmanager>
    fn contextmanager(args) -> Result<Value> {
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let func = args[0].value.clone();
        let _ = _interp;
        Ok(make_cm_factory(func))
    }

    /// Internal callable returned by `@contextmanager`.  When called with the
    /// user's arguments it invokes the wrapped generator function, then wraps
    /// the resulting generator in a `_GeneratorContextManager`.
    class _ContextManagerFactory {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            // Private — constructed only by `make_cm_factory`.
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            Ok(Value::none())
        }

        fn __call__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let func = inst.borrow().attrs.get("_func").cloned().ok_or_else(|| {
                PyError::Runtime(format!("internal: {FN_NAME}() _func missing"))
            })?;
            let user = &args[1..];
            // Call the generator function to get a generator object.
            let generator = _interp.call_function_expanded(func, user)?;
            // Wrap the generator in a _GeneratorContextManager.
            let mut attrs = InstanceAttrs::new();
            attrs.insert("_gen", generator);
            Ok(make_instance("_GeneratorContextManager", attrs))
        }
    }

    // ── closing ───────────────────────────────────────────────────────────────

    /// CPython: contextlib.closing(thing).
    /// Context manager that calls `thing.close()` on exit regardless of
    /// whether an exception occurred.
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.closing>
    class closing {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            inst.borrow_mut()
                .attrs
                .insert("thing", user[0].value.clone());
            let _ = _interp;
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(inst.borrow().attrs.get("thing").cloned().unwrap_or_else(Value::none))
        }

        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let thing = inst.borrow().attrs.get("thing").cloned().unwrap_or_else(Value::none);
            // Call thing.close() with no arguments.
            let close_method = _interp.get_attr(&thing, "close")?;
            _interp.call_function_expanded(close_method, &[])?;
            Ok(Value::bool_(false))
        }
    }

    // ── nullcontext ───────────────────────────────────────────────────────────

    /// CPython: contextlib.nullcontext(enter_result=None).
    /// Context manager that does nothing.  `with nullcontext(x) as y` binds
    /// `y = x` and is a no-op on entry and exit.
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.nullcontext>
    class nullcontext {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            let enter_result = match user.first() {
                Some(a) if a.name.is_none() || a.name.as_deref() == Some("enter_result") => {
                    a.value.clone()
                }
                None => Value::none(),
                Some(a) => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() got an unexpected keyword argument '{}'",
                            a.name.as_deref().unwrap_or("")
                        ),
                    ));
                }
            };
            inst.borrow_mut()
                .attrs
                .insert("enter_result", enter_result);
            let _ = _interp;
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(inst.borrow().attrs.get("enter_result").cloned().unwrap_or_else(Value::none))
        }

        fn __exit__(args) -> Result<Value> {
            let _ = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::bool_(false))
        }
    }

    // ── chdir ───────────────────────────────────────────────────────────────────

    /// CPython: contextlib.chdir(path).
    /// Non-reentrant context manager that temporarily changes the current
    /// working directory to `path`, restoring the previous directory on exit
    /// (whether or not an exception propagated).
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.chdir>
    class chdir {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            inst.borrow_mut().attrs.insert("path", user[0].value.clone());
            // Stack of saved directories, matching CPython's `_old_cwd` list.
            inst.borrow_mut().attrs.insert("_old_cwd", Value::list(vec![]));
            let _ = _interp;
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let path = inst.borrow().attrs.get("path").cloned().unwrap_or_else(Value::none);
            // Snapshot the current directory, then change to the target.
            let cwd = os_call(_interp, "getcwd", &[])?;
            let old_cwd = inst.borrow().attrs.get("_old_cwd").cloned()
                .unwrap_or_else(|| Value::list(vec![]));
            old_cwd.list_push(cwd)?;
            os_call(_interp, "chdir", &[ExpandedCallArg { name: None, value: path }])?;
            Ok(Value::none())
        }

        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let old_cwd = inst.borrow().attrs.get("_old_cwd").cloned()
                .unwrap_or_else(|| Value::list(vec![]));
            let restored = match old_cwd.list_len() {
                Some(n) if n > 0 => old_cwd.list_pop_at(n - 1).unwrap_or_else(|_| Value::none()),
                _ => Value::none(),
            };
            os_call(_interp, "chdir", &[ExpandedCallArg { name: None, value: restored }])?;
            Ok(Value::bool_(false))
        }
    }

    // ── redirect_stdout / redirect_stderr ──────────────────────────────────────

    /// CPython: contextlib.redirect_stdout(new_target).
    /// Context manager that temporarily redirects `sys.stdout` to `new_target`
    /// for the duration of the `with` block, restoring the previous value on
    /// exit.  Reentrant/reusable like CPython's `_RedirectStream`: each
    /// `__enter__` pushes the saved stream and each `__exit__` pops it.
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.redirect_stdout>
    class redirect_stdout {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            inst.borrow_mut().attrs.insert("_new_target", user[0].value.clone());
            inst.borrow_mut().attrs.insert("_old_targets", Value::list(vec![]));
            let _ = _interp;
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            redirect_enter(_interp, &inst, "stdout")
        }

        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            redirect_exit(_interp, &inst, "stdout")
        }
    }

    /// CPython: contextlib.redirect_stderr(new_target).
    /// Like `redirect_stdout`, but for `sys.stderr`.
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.redirect_stderr>
    class redirect_stderr {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            inst.borrow_mut().attrs.insert("_new_target", user[0].value.clone());
            inst.borrow_mut().attrs.insert("_old_targets", Value::list(vec![]));
            let _ = _interp;
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            redirect_enter(_interp, &inst, "stderr")
        }

        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            redirect_exit(_interp, &inst, "stderr")
        }
    }

    // ── ExitStack ─────────────────────────────────────────────────────────────

    /// CPython: contextlib.ExitStack().
    /// Manages a dynamic stack of context managers and callbacks.
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.ExitStack>
    class ExitStack {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            inst.borrow_mut()
                .attrs
                .insert("_callbacks", Value::list(vec![]));
            let _ = _interp;
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::py_instance(inst))
        }

        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            // Pop callbacks in LIFO order, call each with (exc_type, exc_val, traceback).
            // If any callback suppresses the exception (returns truthy), stop propagating.
            let mut exc_type = args.get(1).map(|a| a.value.clone()).unwrap_or_else(Value::none);
            let mut exc_val  = args.get(2).map(|a| a.value.clone()).unwrap_or_else(Value::none);
            let mut tb       = args.get(3).map(|a| a.value.clone()).unwrap_or_else(Value::none);
            let cbs = pop_all_callbacks(&inst);
            let mut suppressed = false;
            let mut pending_err: Option<PyError> = None;
            // Walk LIFO (reverse). CPython runs ALL callbacks regardless of
            // exceptions or suppression; only the final state matters.
            for cb in cbs.into_iter().rev() {
                let result = _interp.call_function_expanded(
                    cb,
                    &[
                        ExpandedCallArg { name: None, value: exc_type.clone() },
                        ExpandedCallArg { name: None, value: exc_val.clone() },
                        ExpandedCallArg { name: None, value: tb.clone() },
                    ],
                );
                match result {
                    Ok(v) => {
                        // Use the interpreter's full truthiness evaluation so that
                        // containers (empty list/tuple/dict/set = falsy) and custom
                        // __bool__ / __len__ are handled correctly, matching CPython.
                        let truthy = match _interp.truthy_value(&v) {
                            Ok(b) => b,
                            Err(e) => {
                                pending_err = Some(e);
                                suppressed = false;
                                exc_type = Value::none();
                                exc_val = Value::none();
                                tb = Value::none();
                                continue;
                            }
                        };
                        if !exc_type.is_none() && truthy {
                            suppressed = true;
                            // Subsequent callbacks see no active exception.
                            exc_type = Value::none();
                            exc_val = Value::none();
                            tb = Value::none();
                        }
                    }
                    Err(e) => {
                        // New exception replaces current; subsequent callbacks
                        // see no exception (we can't round-trip PyError → Value).
                        pending_err = Some(e);
                        suppressed = false;
                        exc_type = Value::none();
                        exc_val = Value::none();
                        tb = Value::none();
                    }
                }
            }
            if let Some(e) = pending_err {
                return Err(e);
            }
            Ok(Value::bool_(suppressed))
        }

        /// `stack.enter_context(cm)` — calls `cm.__enter__()` and pushes
        /// `cm.__exit__` onto the callback stack.  Returns the enter result.
        fn enter_context(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let cm = user[0].value.clone();
            // Retrieve __exit__ before calling __enter__ so that if __enter__
            // fails we don't register a cleanup for a context that never opened.
            let exit_fn = _interp.get_attr(&cm, "__exit__")?;
            // Call cm.__enter__() with no arguments.
            let enter_attr = _interp.get_attr(&cm, "__enter__")?;
            let enter_result = _interp.call_function_expanded(enter_attr, &[])?;
            // Push __exit__ onto the callback stack.
            let callbacks_val = inst.borrow().attrs.get("_callbacks").cloned()
                .unwrap_or_else(|| Value::list(vec![]));
            callbacks_val.list_push(exit_fn)?;
            Ok(enter_result)
        }

        /// `stack.callback(fn, *args, **kwargs)` — push `fn(*args, **kwargs)` as
        /// a no-argument cleanup callback.  The extra args are bound at push time.
        fn callback(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() < 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes at least 1 argument"),
                ));
            }
            let func = args[1].value.clone();
            let bound_args: Vec<ExpandedCallArg> = args[2..].to_vec();
            // Wrap func+bound_args in a no-argument callable (a _StackCallback instance).
            let mut attrs = InstanceAttrs::new();
            attrs.insert("_func", func);
            // Encode bound_args as a list of [name_or_none, value] pairs.
            let encoded: Vec<Value> = bound_args.iter().map(|a| {
                Value::tuple(vec![
                    a.name.as_ref().map(|n| Value::string(n.clone())).unwrap_or_else(Value::none),
                    a.value.clone(),
                ])
            }).collect();
            attrs.insert("_bound_args", Value::list(encoded));
            let wrapper = make_instance("_StackCallback", attrs);
            // Push a closure-like value: a _StackCallback that, when called with
            // (exc_type, exc_val, tb), ignores those and calls func(*bound_args).
            let callbacks_val = inst.borrow().attrs.get("_callbacks").cloned()
                .unwrap_or_else(|| Value::list(vec![]));
            callbacks_val.list_push(wrapper)?;
            let _ = _interp;
            Ok(Value::none())
        }

        /// `stack.close()` — call all registered callbacks in LIFO order,
        /// passing `(None, None, None)`.  Exceptions from callbacks propagate.
        fn close(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let cbs = pop_all_callbacks(&inst);
            let mut pending_err: Option<PyError> = None;
            for cb in cbs.into_iter().rev() {
                let result = _interp.call_function_expanded(
                    cb,
                    &[
                        ExpandedCallArg { name: None, value: Value::none() },
                        ExpandedCallArg { name: None, value: Value::none() },
                        ExpandedCallArg { name: None, value: Value::none() },
                    ],
                );
                if let Err(e) = result {
                    pending_err = Some(e);
                }
            }
            if let Some(e) = pending_err {
                return Err(e);
            }
            Ok(Value::none())
        }
    }

    /// Internal wrapper for `ExitStack.callback(fn, *args)`.
    /// When called with `(exc_type, exc_val, traceback)`, it ignores those and
    /// calls the stored function with the pre-bound arguments.
    class _StackCallback {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            Ok(Value::none())
        }

        /// Called by ExitStack's cleanup loop with (exc_type, exc_val, tb).
        /// Ignores those and invokes the stored function with pre-bound args.
        fn __call__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let func = inst.borrow().attrs.get("_func").cloned().ok_or_else(|| {
                PyError::Runtime(format!("internal: {FN_NAME}() _func missing"))
            })?;
            let encoded_val = inst.borrow().attrs.get("_bound_args").cloned()
                .unwrap_or_else(|| Value::list(vec![]));
            let encoded = match encoded_val.kind() {
                ValueKind::List(items) => items.to_vec(),
                _ => vec![],
            };
            let mut call_args: Vec<ExpandedCallArg> = Vec::new();
            for item in &encoded {
                if let ValueKind::Tuple(pair) = item.kind() {
                    let pair = pair.to_vec();
                    let name = match pair.first().map(|v| v.kind()) {
                        Some(ValueKind::Str(s)) => Some(s.to_string()),
                        _ => None,
                    };
                    let val = pair.get(1).cloned().unwrap_or_else(Value::none);
                    call_args.push(ExpandedCallArg { name, value: val });
                }
            }
            _interp.call_function_expanded(func, &call_args)?;
            Ok(Value::bool_(false))
        }
    }
}

// ── shared helpers ────────────────────────────────────────────────────────────

fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Quick-check whether `err` is a StopIteration error (any variant).
fn is_stop_iteration(err: &PyError) -> bool {
    match err {
        PyError::Named(name, _) => name.as_ref() == "StopIteration",
        PyError::Class(cls, _) => cls.borrow().name == "StopIteration",
        PyError::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => inst.borrow().class.borrow().name == "StopIteration",
            _ => false,
        },
        _ => false,
    }
}

/// Pull out `module_class` from this contextlib module by name.
fn module_class(name: &str) -> Option<Rc<RefCell<crate::value::PyClass>>> {
    let module_val = module();
    let ValueKind::PyModule(m) = module_val.kind() else {
        return None;
    };
    let class_val = m.borrow().attrs.get(name).cloned()?;
    match class_val.kind() {
        ValueKind::PyClass(c) => Some(Rc::clone(c)),
        _ => None,
    }
}

/// Construct a `PyInstance` of `name` with the given attrs, bypassing `__init__`.
fn make_instance(name: &str, attrs: InstanceAttrs) -> Value {
    match module_class(name) {
        Some(class) => Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs }))),
        None => unreachable!(
            "internal: contextlib module did not register class `{name}`",
        ),
    }
}

/// Shared `__enter__` for `redirect_stdout`/`redirect_stderr`.  Snapshots the
/// current `sys.<stream>` onto the instance's `_old_targets` stack, installs the
/// redirect target, and returns it (the `with … as` value).
fn redirect_enter(
    interp: &mut crate::Interpreter,
    inst: &Rc<RefCell<PyInstance>>,
    stream: &str,
) -> Result<Value> {
    let new_target = inst
        .borrow()
        .attrs
        .get("_new_target")
        .cloned()
        .unwrap_or_else(Value::none);
    let old = interp.current_std_stream(stream)?;
    let old_targets = inst
        .borrow()
        .attrs
        .get("_old_targets")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    old_targets.list_push(old)?;
    interp.set_std_stream(stream, new_target.clone())?;
    Ok(new_target)
}

/// Shared `__exit__` for `redirect_stdout`/`redirect_stderr`.  Pops the saved
/// stream off `_old_targets` and restores it as `sys.<stream>`.  Never
/// suppresses exceptions (returns `False`).
fn redirect_exit(
    interp: &mut crate::Interpreter,
    inst: &Rc<RefCell<PyInstance>>,
    stream: &str,
) -> Result<Value> {
    let old_targets = inst
        .borrow()
        .attrs
        .get("_old_targets")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    let restored = match old_targets.list_len() {
        Some(n) if n > 0 => old_targets.list_pop_at(n - 1).unwrap_or_else(|_| Value::none()),
        _ => Value::none(),
    };
    interp.set_std_stream(stream, restored)?;
    Ok(Value::bool_(false))
}

/// Call `os.<func>(*args)` from the contextlib runtime.  Used by `chdir` to
/// reach `os.getcwd()` / `os.chdir(path)` without duplicating their logic.
fn os_call(
    interp: &mut crate::Interpreter,
    func: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let os_module = interp.load_module("os")?;
    let callable = interp.get_attr(&os_module, func)?;
    interp.call_function_expanded(callable, args)
}

/// Construct a `_ContextManagerFactory` instance seeding `_func`.
fn make_cm_factory(func: Value) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_func", func);
    make_instance("_ContextManagerFactory", attrs)
}

/// Drain all callbacks from `_callbacks` into a Vec and clear the stack.
fn pop_all_callbacks(inst: &Rc<RefCell<PyInstance>>) -> Vec<Value> {
    let callbacks_val = inst.borrow().attrs.get("_callbacks").cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    // Collect a snapshot first (list_with drops the Ref before returning),
    // then clear the underlying list.  Doing both in one `match callbacks_val.kind()`
    // arm would hold a `Ref` guard while `list_clear()` tries to `borrow_mut()`,
    // causing a RefCell panic.
    let result = callbacks_val
        .list_with(|items| items.clone())
        .unwrap_or_default();
    let _ = callbacks_val.list_clear();
    result
}
