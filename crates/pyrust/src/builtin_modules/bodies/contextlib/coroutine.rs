//! Native coroutines returned by `contextlib`'s `async` methods.
//!
//! CPython writes these as `async def` methods. Here each one is a small
//! continuation-passing state machine: a coroutine starts with a closure that
//! returns a [`Flow`], and every `await` is expressed as `Flow::Await` plus the
//! continuation that receives the awaited result (or the error it raised, so
//! `try`/`except`/`finally` around an `await` stay ordinary Rust code).
//!
//! The coroutine objects carry asyncio's `NativeCoroState`, so a running
//! `asyncio` `Task` drives them exactly like its own native coroutines.

use std::cell::RefCell;
use std::rc::Rc;

use crate::builtin_modules::asyncio_rt::awaitable::{CoroBody, ExternalCoroBody, NativeCoroState};
use crate::builtin_modules::macros::native_state::set_native_state;
use crate::error::{PyError, Result};
use crate::interpreter::{CoroStep, Interpreter};
use crate::value::{InstanceAttrs, PyInstance, Value};

use super::registry::coroutine_class;

/// What a native coroutine does next.
pub(crate) enum Flow {
    /// `return value`.
    Return(Value),
    /// `await awaitable`, then continue with the continuation.
    Await(Value, Cont),
}

/// Continuation run with the result of an `await` (or the error it raised).
pub(crate) type Cont = Box<dyn FnOnce(&mut Interpreter, Result<Value>) -> Result<Flow>>;

type Start = Box<dyn FnOnce(&mut Interpreter) -> Result<Flow>>;

struct ChainBody {
    start: Option<Start>,
    pending: Option<(Value, Cont)>,
}

impl ChainBody {
    /// Run `flow` until the coroutine finishes or suspends on a real yield.
    fn run(&mut self, interp: &mut Interpreter, mut flow: Flow) -> Result<CoroStep> {
        loop {
            let (awaitable, cont) = match flow {
                Flow::Return(value) => return Ok(CoroStep::Returned(value)),
                Flow::Await(awaitable, cont) => (awaitable, cont),
            };
            let iterator = match interp.get_awaitable(&awaitable) {
                Ok(iterator) => iterator,
                Err(error) => {
                    flow = cont(interp, Err(error))?;
                    continue;
                }
            };
            match interp.await_step(&iterator, Value::none(), None) {
                Ok(CoroStep::Yielded(value)) => {
                    self.pending = Some((iterator, cont));
                    return Ok(CoroStep::Yielded(value));
                }
                Ok(CoroStep::Returned(value)) => flow = cont(interp, Ok(value))?,
                Err(error) => flow = cont(interp, Err(error))?,
            }
        }
    }
}

impl ExternalCoroBody for ChainBody {
    fn step(
        &mut self,
        interp: &mut Interpreter,
        sent_value: Value,
        inject_exc: Option<PyError>,
    ) -> Result<CoroStep> {
        let Some((iterator, cont)) = self.pending.take() else {
            // First resumption: an exception thrown into an unstarted
            // coroutine is raised at its start.
            let Some(start) = self.start.take() else {
                return Err(PyError::named(
                    "RuntimeError",
                    "cannot reuse already awaited coroutine",
                ));
            };
            if let Some(error) = inject_exc {
                return Err(error);
            }
            let flow = start(interp)?;
            return self.run(interp, flow);
        };
        match interp.await_step(&iterator, sent_value, inject_exc) {
            Ok(CoroStep::Yielded(value)) => {
                self.pending = Some((iterator, cont));
                Ok(CoroStep::Yielded(value))
            }
            Ok(CoroStep::Returned(value)) => {
                let flow = cont(interp, Ok(value))?;
                self.run(interp, flow)
            }
            Err(error) => {
                let flow = cont(interp, Err(error))?;
                self.run(interp, flow)
            }
        }
    }
}

/// Create a native coroutine object whose body is `start`.
pub(crate) fn coroutine(
    start: impl FnOnce(&mut Interpreter) -> Result<Flow> + 'static,
) -> Result<Value> {
    let instance = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: coroutine_class(),
        attrs: InstanceAttrs::new(),
    })));
    let body = ChainBody {
        start: Some(Box::new(start)),
        pending: None,
    };
    set_native_state(
        &instance,
        NativeCoroState {
            body: Some(CoroBody::External(Box::new(body))),
            done: false,
        },
    )?;
    Ok(instance)
}

/// A coroutine that completes immediately with `value` (`async def f(): return value`).
pub(super) fn ready(value: Value) -> Result<Value> {
    coroutine(move |_| Ok(Flow::Return(value)))
}

/// A coroutine that awaits `awaitable` and returns `None`
/// (`async def f(): await awaitable`).
pub(super) fn await_discarding(
    start: impl FnOnce(&mut Interpreter) -> Result<Value> + 'static,
) -> Result<Value> {
    coroutine(move |interp| {
        let awaitable = start(interp)?;
        Ok(Flow::Await(
            awaitable,
            Box::new(|_, result| result.map(|_| Flow::Return(Value::none()))),
        ))
    })
}
