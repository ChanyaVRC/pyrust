//! `AsyncExitStack` (native `async` methods over the shared `_BaseExitStack`
//! machinery in `exit_stack.rs`), `aclosing`, and the async context manager
//! support classes (`_AsyncGeneratorContextManager`).

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

use super::coroutine::{self, Flow};
use super::decorators::{
    ThrowOutcome, classify_throw_error, drop_recreation_state, exit_triple, materialize_exit_value,
};
use super::exit_stack::Unwind;
use super::support::{
    bind_required, call, call_method, error_from_none, exception_of, expect_self,
    is_exception_instance, optional_attr, protocol_error, type_of,
};

/// `gen.<method>(*args)` for an async generator (or any object with the
/// async-generator protocol).
fn call_generator(
    interp: &mut Interpreter,
    generator: &Value,
    method: &str,
    values: Vec<Value>,
) -> Result<Value> {
    if matches!(generator.kind(), crate::value::ValueKind::Generator(_)) {
        interp.call_generator_method(generator.clone(), method, values)
    } else {
        call_method(interp, generator, method, &values)
    }
}

// ── AsyncExitStack ───────────────────────────────────────────────────────────

/// `await self.enter_async_context(cm)`.
pub(super) fn enter_async_context(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let cm = bind_required(args, fn_name, &["cm"])?.remove(0);
    let stack = Value::py_instance(inst);
    coroutine::coroutine(move |interp| {
        let class = type_of(&cm);
        let enter = optional_attr(interp, &class, "__aenter__")?;
        let exit = optional_attr(interp, &class, "__aexit__")?;
        let (Some(enter), Some(exit)) = (enter, exit) else {
            return Err(protocol_error(
                interp,
                &cm,
                "the asynchronous context manager protocol",
            )?);
        };
        let entering = call(interp, &enter, std::slice::from_ref(&cm))?;
        Ok(Flow::Await(
            entering,
            Box::new(move |interp, result| {
                let result = result?;
                call_method(interp, &stack, "_push_async_cm_exit", &[cm, exit])?;
                Ok(Flow::Return(result))
            }),
        ))
    })
}

/// `self._push_async_cm_exit(cm, cm_exit)`.
pub(super) fn push_async_cm_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let bound = bind_required(args, fn_name, &["cm", "cm_exit"])?;
    let stack = Value::py_instance(inst);
    let wrapper = call_method(interp, &stack, "_create_async_exit_wrapper", &bound)?;
    call_method(
        interp,
        &stack,
        "_push_exit_callback",
        &[wrapper, Value::bool_(false)],
    )?;
    Ok(Value::none())
}

/// `self.push_async_exit(exit)`.
pub(super) fn push_async_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let exit = bind_required(args, fn_name, &["exit"])?.remove(0);
    let stack = Value::py_instance(inst);
    match optional_attr(interp, &type_of(&exit), "__aexit__")? {
        // Not an async context manager, so assume it's a coroutine function.
        None => {
            call_method(
                interp,
                &stack,
                "_push_exit_callback",
                &[exit.clone(), Value::bool_(false)],
            )?;
        }
        Some(exit_method) => {
            call_method(
                interp,
                &stack,
                "_push_async_cm_exit",
                &[exit.clone(), exit_method],
            )?;
        }
    }
    Ok(exit)
}

/// `await self.aclose()`.
pub(super) fn aclose(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let stack = Value::py_instance(inst);
    coroutine::await_discarding(move |interp| {
        call_method(
            interp,
            &stack,
            "__aexit__",
            &[Value::none(), Value::none(), Value::none()],
        )
    })
}

pub(super) fn aenter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    coroutine::ready(Value::py_instance(inst))
}

/// Run the callbacks of `unwind` until one needs to be awaited or none remain.
fn unwind_callbacks(interp: &mut Interpreter, mut unwind: Unwind) -> Result<Flow> {
    while let Some((is_sync, callback)) = unwind.pop_callback(interp)? {
        let outcome = interp.call_function_expanded(callback, &unwind.callback_args());
        if is_sync || outcome.is_err() {
            unwind.record(interp, outcome);
            continue;
        }
        let awaitable = outcome?;
        return Ok(Flow::Await(
            awaitable,
            Box::new(move |interp, result| {
                let mut unwind = unwind;
                unwind.record(interp, result);
                unwind_callbacks(interp, unwind)
            }),
        ));
    }
    unwind.finish().map(Flow::Return)
}

/// `await self.__aexit__(*exc_details)`.
pub(super) fn aexit(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let stack = Value::py_instance(inst);
    let exc_details: Vec<Value> = args[1..].iter().map(|arg| arg.value.clone()).collect();
    coroutine::coroutine(move |interp| {
        let unwind = Unwind::new(interp, stack, exc_details);
        unwind_callbacks(interp, unwind)
    })
}

// ── aclosing ─────────────────────────────────────────────────────────────────

pub(super) fn aclosing_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let thing = bind_required(args, fn_name, &["thing"])?.remove(0);
    inst.borrow_mut().attrs.insert("thing", thing);
    Ok(Value::none())
}

pub(super) fn aclosing_aenter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let receiver = Value::py_instance(inst);
    coroutine::coroutine(move |interp| {
        let thing = interp.get_attr(&receiver, "thing")?;
        Ok(Flow::Return(thing))
    })
}

pub(super) fn aclosing_aexit(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let receiver = Value::py_instance(inst);
    coroutine::await_discarding(move |interp| {
        let thing = interp.get_attr(&receiver, "thing")?;
        call_method(interp, &thing, "aclose", &[])
    })
}

// ── _AsyncGeneratorContextManager ────────────────────────────────────────────

/// `await self.gen.aclose()` and then raise `RuntimeError(message)`.
fn aclose_then_raise(
    interp: &mut Interpreter,
    generator: &Value,
    message: &'static str,
) -> Result<Flow> {
    let closing = call_generator(interp, generator, "aclose", Vec::new())?;
    Ok(Flow::Await(
        closing,
        Box::new(move |_, result| {
            result?;
            Err(PyError::named("RuntimeError", message.to_string()))
        }),
    ))
}

pub(super) fn gen_cm_aenter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let manager = Value::py_instance(inst);
    coroutine::coroutine(move |interp| {
        drop_recreation_state(interp, &manager)?;
        let generator = interp.get_attr(&manager, "gen")?;
        let advancing = call_generator(interp, &generator, "__anext__", Vec::new())?;
        Ok(Flow::Await(
            advancing,
            Box::new(|interp, result| match result {
                Ok(value) => Ok(Flow::Return(value)),
                Err(error) => {
                    let (exception, error) = exception_of(interp, error)?;
                    if is_exception_instance(&exception, &["StopAsyncIteration"]) {
                        Err(error_from_none(
                            interp,
                            "RuntimeError",
                            "generator didn't yield",
                        ))
                    } else {
                        Err(error)
                    }
                }
            }),
        ))
    })
}

pub(super) fn gen_cm_aexit(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let manager = Value::py_instance(inst);
    let (typ, value, traceback) = exit_triple(args);
    coroutine::coroutine(move |interp| {
        let generator = interp.get_attr(&manager, "gen")?;
        if typ.is_none() {
            let advancing = call_generator(interp, &generator, "__anext__", Vec::new())?;
            return Ok(Flow::Await(
                advancing,
                Box::new(move |interp, result| match result {
                    Ok(_) => aclose_then_raise(interp, &generator, "generator didn't stop"),
                    Err(error) => {
                        let (exception, error) = exception_of(interp, error)?;
                        if is_exception_instance(&exception, &["StopAsyncIteration"]) {
                            Ok(Flow::Return(Value::bool_(false)))
                        } else {
                            Err(error)
                        }
                    }
                }),
            ));
        }
        // Force instantiation so we can reliably tell if we get the same
        // exception back.
        let value = materialize_exit_value(interp, &typ, &value)?;
        let throwing = call_generator(interp, &generator, "athrow", vec![value.clone()]);
        let handle = move |interp: &mut Interpreter, result: Result<Value>| match result {
            Ok(_) => aclose_then_raise(interp, &generator, "generator didn't stop after athrow()"),
            Err(error) => match classify_throw_error(interp, error, &value, &traceback, true)? {
                ThrowOutcome::Return(flag) => Ok(Flow::Return(Value::bool_(flag))),
                ThrowOutcome::Raise(error) => Err(error),
            },
        };
        match throwing {
            Ok(awaitable) => Ok(Flow::Await(awaitable, Box::new(handle))),
            Err(error) => handle(interp, Err(error)),
        }
    })
}
