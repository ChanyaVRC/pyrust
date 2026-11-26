//! `@contextmanager` / `@asynccontextmanager` factories and the shared
//! `_GeneratorContextManagerBase` / `_GeneratorContextManager` implementations.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, is_stop_iteration_error};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

use super::registry::{Sibling, make_instance, make_sibling_instance};
use super::support::{
    bind_required, call, call_method, error_from_none, exception_of, expand_call_args, expect_self,
    is_exception_instance, optional_attr, split_call_args, type_of,
};

const FACTORY_CLASS: &str = "_ContextManagerFactory";

/// `functools.update_wrapper(wrapper, wrapped)` (what `@wraps(wrapped)` does).
pub(super) fn update_wrapper(
    interp: &mut Interpreter,
    wrapper: &Value,
    wrapped: &Value,
) -> Result<()> {
    let functools = interp.load_module("functools")?;
    let update = interp.get_attr(&functools, "update_wrapper")?;
    call(interp, &update, &[wrapper.clone(), wrapped.clone()])?;
    Ok(())
}

/// `contextmanager(func)` / `asynccontextmanager(func)`: the `helper` callable
/// wrapping `func`.
pub(super) fn make_factory(interp: &mut Interpreter, func: Value, is_async: bool) -> Result<Value> {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_func", func.clone());
    attrs.insert("_async", Value::bool_(is_async));
    let factory = make_instance(FACTORY_CLASS, attrs);
    update_wrapper(interp, &factory, &func)?;
    Ok(factory)
}

pub(super) fn factory_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let _ = expect_self(args, fn_name)?;
    Err(PyError::named(
        "TypeError",
        format!("cannot create '{FACTORY_CLASS}' instances"),
    ))
}

/// `helper(*args, **kwds)`: build the generator context manager.
pub(super) fn factory_call(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let (factory_class, func, is_async, bound) = {
        let factory = inst.borrow();
        let func = factory
            .attrs
            .get("_func")
            .cloned()
            .ok_or_else(|| PyError::runtime(format!("internal: {fn_name}() _func missing")))?;
        let is_async = factory
            .attrs
            .get("_async")
            .is_some_and(|flag| matches!(flag.kind(), ValueKind::Bool(true)));
        (
            Rc::clone(&factory.class),
            func,
            is_async,
            factory.attrs.get("_bound").cloned(),
        )
    };
    let mut call_args: Vec<ExpandedCallArg> = Vec::new();
    if let Some(receiver) = bound {
        call_args.push(ExpandedCallArg {
            name: None,
            value: receiver,
        });
    }
    call_args.extend(args[1..].iter().cloned());
    let (positional_args, keywords) = split_call_args(&call_args)?;
    let (kind, fallback) = if is_async {
        (
            Sibling::AsyncGeneratorManager,
            "_AsyncGeneratorContextManager",
        )
    } else {
        (Sibling::GeneratorManager, "_GeneratorContextManager")
    };
    let manager = make_sibling_instance(kind, &factory_class, fallback, InstanceAttrs::new());
    init_generator_manager(interp, &manager, &func, &positional_args, &keywords)?;
    Ok(manager)
}

/// `helper.__get__(obj, objtype=None)`: functions bind as methods.
pub(super) fn factory_get(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let owner = args.get(1).map(|a| a.value.clone());
    let Some(owner) = owner.filter(|owner| !owner.is_none()) else {
        return Ok(Value::py_instance(inst));
    };
    let (class, attrs) = {
        let factory = inst.borrow();
        (Rc::clone(&factory.class), factory.attrs.clone())
    };
    let mut attrs = attrs;
    attrs.insert("_bound", owner);
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// `_GeneratorContextManagerBase.__init__(self, func, args, kwds)`.
fn init_generator_manager(
    interp: &mut Interpreter,
    manager: &Value,
    func: &Value,
    args: &Value,
    kwds: &Value,
) -> Result<()> {
    let call_args = expand_call_args(interp, args, kwds)?;
    let generator = interp.call_function_expanded(func.clone(), &call_args)?;
    interp.assign_attr(manager.clone(), "gen", generator)?;
    interp.assign_attr(manager.clone(), "func", func.clone())?;
    interp.assign_attr(manager.clone(), "args", args.clone())?;
    interp.assign_attr(manager.clone(), "kwds", kwds.clone())?;
    let mut doc = optional_attr(interp, func, "__doc__")?.unwrap_or_else(Value::none);
    if doc.is_none() {
        doc = interp.get_attr(&type_of(manager), "__doc__")?;
    }
    interp.assign_attr(manager.clone(), "__doc__", doc)?;
    Ok(())
}

pub(super) fn base_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let bound = bind_required(args, fn_name, &["func", "args", "kwds"])?;
    let manager = Value::py_instance(inst);
    init_generator_manager(interp, &manager, &bound[0], &bound[1], &bound[2])?;
    Ok(Value::none())
}

/// `_GeneratorContextManagerBase._recreate_cm(self)`.
pub(super) fn recreate_cm(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let manager = Value::py_instance(inst);
    let class = interp.get_attr(&manager, "__class__")?;
    let func = interp.get_attr(&manager, "func")?;
    let positional_args = interp.get_attr(&manager, "args")?;
    let keywords = interp.get_attr(&manager, "kwds")?;
    call(interp, &class, &[func, positional_args, keywords])
}

/// Delete `self.args`, `self.kwds`, `self.func` (they are only needed for
/// recreation, which is impossible once the manager has been entered).
pub(super) fn drop_recreation_state(interp: &mut Interpreter, manager: &Value) -> Result<()> {
    for name in ["args", "kwds", "func"] {
        interp.delete_attr(manager.clone(), name)?;
    }
    Ok(())
}

/// `_GeneratorContextManager.__enter__`.
pub(super) fn gen_cm_enter(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let manager = Value::py_instance(inst);
    drop_recreation_state(interp, &manager)?;
    let generator = interp.get_attr(&manager, "gen")?;
    match interp.call_next(&generator, None) {
        Ok(value) => Ok(value),
        Err(error) if is_stop_iteration_error(&error) => Err(error_from_none(
            interp,
            "RuntimeError",
            "generator didn't yield",
        )),
        Err(error) => Err(error),
    }
}

/// The `finally: self.gen.close()` shared by the "didn't stop" paths: close the
/// generator, letting a failing `close()` replace the pending `RuntimeError`.
fn close_then_raise(interp: &mut Interpreter, generator: &Value, message: &str) -> Result<Value> {
    call_method(interp, generator, "close", &[])?;
    Err(PyError::named("RuntimeError", message.to_string()))
}

/// How an exception that came back out of `throw()` / `athrow()` maps onto the
/// `__exit__` protocol (shared by the sync and async managers).
pub(super) enum ThrowOutcome {
    /// Return this value from `__exit__`.
    Return(bool),
    /// Re-raise this error from `__exit__`.
    Raise(PyError),
}

/// The `except StopIteration / RuntimeError / BaseException` tail of
/// `_GeneratorContextManager.__exit__` (and its async twin when `is_async`).
pub(super) fn classify_throw_error(
    interp: &mut Interpreter,
    error: PyError,
    value: &Value,
    traceback: &Value,
    is_async: bool,
) -> Result<ThrowOutcome> {
    let sync_stop = !is_async && is_stop_iteration_error(&error);
    let (exception, error) = exception_of(interp, error)?;
    let stopped =
        sync_stop || (is_async && is_exception_instance(&exception, &["StopAsyncIteration"]));
    if stopped {
        // Suppress Stop(Async)Iteration *unless* it is the very exception that
        // was passed to throw().
        return Ok(ThrowOutcome::Return(!exception.is_identical_to(value)));
    }
    classify_non_stop(interp, exception, error, value, traceback, is_async)
}

fn classify_non_stop(
    interp: &mut Interpreter,
    exception: Value,
    error: PyError,
    value: &Value,
    traceback: &Value,
    is_async: bool,
) -> Result<ThrowOutcome> {
    if is_exception_instance(&exception, &["RuntimeError"]) {
        // Don't re-raise the passed in exception (issue27122).
        if exception.is_identical_to(value) {
            interp.assign_attr(exception, "__traceback__", traceback.clone())?;
            return Ok(ThrowOutcome::Return(false));
        }
        // Avoid suppressing if a Stop(Async)Iteration exception was passed to
        // throw() and later wrapped into a RuntimeError (PEP 479), but only
        // when the RuntimeError actually wraps that very exception.
        let stop_names: &[&str] = if is_async {
            &["StopIteration", "StopAsyncIteration"]
        } else {
            &["StopIteration"]
        };
        if is_exception_instance(value, stop_names) {
            let cause = interp.get_attr(&exception, "__cause__")?;
            if cause.is_identical_to(value) {
                interp.assign_attr(value.clone(), "__traceback__", traceback.clone())?;
                return Ok(ThrowOutcome::Return(false));
            }
        }
        return Ok(ThrowOutcome::Raise(error));
    }
    // Only re-raise if it is *not* the exception that was passed to throw():
    // __exit__() must not raise unless __exit__() itself failed.
    if !exception.is_identical_to(value) {
        return Ok(ThrowOutcome::Raise(error));
    }
    interp.assign_attr(exception, "__traceback__", traceback.clone())?;
    Ok(ThrowOutcome::Return(false))
}

/// `value = typ()` when the interpreter handed `__exit__` no exception instance.
pub(super) fn materialize_exit_value(
    interp: &mut Interpreter,
    typ: &Value,
    value: &Value,
) -> Result<Value> {
    if value.is_none() {
        call(interp, typ, &[])
    } else {
        Ok(value.clone())
    }
}

/// The `(typ, value, traceback)` triple of an `__exit__` call.
pub(super) fn exit_triple(args: &[ExpandedCallArg]) -> (Value, Value, Value) {
    let arg = |index: usize| {
        args.get(index)
            .map(|a| a.value.clone())
            .unwrap_or_else(Value::none)
    };
    (arg(1), arg(2), arg(3))
}

/// `_GeneratorContextManager.__exit__`.
pub(super) fn gen_cm_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let manager = Value::py_instance(inst);
    let generator = interp.get_attr(&manager, "gen")?;
    let (typ, value, traceback) = exit_triple(args);
    if typ.is_none() {
        return match interp.call_next(&generator, None) {
            Ok(_) => close_then_raise(interp, &generator, "generator didn't stop"),
            Err(error) if is_stop_iteration_error(&error) => Ok(Value::bool_(false)),
            Err(error) => Err(error),
        };
    }
    // Force instantiation so we can reliably tell if we get the same
    // exception back.
    let value = materialize_exit_value(interp, &typ, &value)?;
    match interp.call_generator_method(generator.clone(), "throw", vec![value.clone()]) {
        Ok(_) => close_then_raise(interp, &generator, "generator didn't stop after throw()"),
        Err(error) => match classify_throw_error(interp, error, &value, &traceback, false)? {
            ThrowOutcome::Return(flag) => Ok(Value::bool_(flag)),
            ThrowOutcome::Raise(error) => Err(error),
        },
    }
}
