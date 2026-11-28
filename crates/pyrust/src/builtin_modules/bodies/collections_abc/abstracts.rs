//! The bodies of the `@abstractmethod`s (reachable through `super()` or an
//! explicit `ABC.method(obj)` call) and the `async def` mixins of
//! `AsyncGenerator`.

use crate::builtin_modules::contextlib::coroutine::{Flow, coroutine};
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

use super::definitions::Abc;
use super::support::{
    GenBody, call, call_method, exc_class, forget_handled_frames, generator, is_exc, raise_new,
};

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

fn bind(sig: &Sig, call_args: &[ExpandedCallArg]) -> Result<Vec<Option<Value>>> {
    let mut bound = args::bind(sig, call_args)?;
    Ok((0..sig.params.len()).map(|i| bound.take(i)).collect())
}

/// The argument shape of one abstract method.
#[derive(Clone, Copy)]
pub(super) enum Shape {
    /// Positional-or-keyword parameters, `self` first, none defaulted.
    Params(&'static [&'static str]),
    /// `(self, typ, val=None, tb=None)`.
    Throw,
    /// `(self, flags, /)`.
    PositionalOne,
    /// `(self, *args, **kwds)`.
    Any,
}

/// What an abstract method's body does once its arguments are bound.
#[derive(Clone, Copy)]
pub(super) enum Body {
    /// `return <value>`.
    ReturnZero,
    ReturnFalse,
    /// `raise <exception>` (no arguments).
    Raise(&'static str),
    /// `raise StopAsyncIteration` inside an `async def`.
    AsyncRaise(&'static str),
    /// The generator `while False: yield None` (an exhausted generator).
    EmptyGenerator,
    /// The generator `yield` (yields `None` once).
    YieldOnce,
    /// `return AsyncIterator()`.
    NewAsyncIterator,
    /// The shared `throw(typ, val=None, tb=None)` body.
    Throw,
    /// `throw` inside an `async def`.
    AsyncThrow,
}

/// Bind and run the body of the abstract method `qualname`.
pub(super) fn call_abstract(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
    qualname: &'static str,
    shape: Shape,
    body: Body,
) -> Result<Value> {
    let bound: Vec<Option<Value>> = match shape {
        Shape::Params(params) => bind(&sig(qualname, params, 0), call_args)?,
        Shape::Throw => bind(&sig(qualname, &["self", "typ", "val", "tb"], 2), call_args)?,
        Shape::PositionalOne => {
            if let Some(arg) = call_args.iter().find(|arg| arg.name.is_some()) {
                let name = arg.name.as_deref().unwrap_or_default();
                return Err(args::type_error(format!(
                    "{qualname}() got some positional-only arguments passed as keyword \
                     arguments: '{name}'"
                )));
            }
            bind(&sig(qualname, &["self", "flags"], 0), call_args)?
        }
        Shape::Any => Vec::new(),
    };
    match body {
        Body::ReturnZero => Ok(Value::int(0)),
        Body::ReturnFalse => Ok(Value::bool_(false)),
        Body::Raise(name) => Err(raise_new(interp, name, &[])),
        Body::AsyncRaise(name) => {
            let error = raise_new(interp, name, &[]);
            coroutine(move |_| Err(error))
        }
        Body::EmptyGenerator => Ok(generator(Once { remaining: 0 })),
        Body::YieldOnce => Ok(generator(Once { remaining: 1 })),
        Body::NewAsyncIterator => call(interp, Abc::AsyncIterator.value(), &[]),
        Body::Throw => Err(throw_error(interp, bound)),
        Body::AsyncThrow => {
            let error = throw_error(interp, bound);
            coroutine(move |_| Err(error))
        }
    }
}

/// Yields `None` `remaining` times.
struct Once {
    remaining: u8,
}

impl GenBody for Once {
    fn step(&mut self, _interp: &mut Interpreter) -> Result<Option<Value>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        Ok(Some(Value::none()))
    }
}

/// The exception `throw(typ, val=None, tb=None)` raises:
///
/// ```python
/// if val is None:
///     if tb is None:
///         raise typ
///     val = typ()
/// if tb is not None:
///     val = val.with_traceback(tb)
/// raise val
/// ```
fn throw_error(interp: &mut Interpreter, mut bound: Vec<Option<Value>>) -> PyError {
    let typ = bound[1].take().unwrap_or_else(Value::none);
    let val = bound[2].take().unwrap_or_else(Value::none);
    let tb = bound[3].take().unwrap_or_else(Value::none);
    let result = (|| -> Result<PyError> {
        let mut val = val;
        if val.is_none() {
            if tb.is_none() {
                return Ok(raise_object(interp, typ));
            }
            val = call(interp, typ, &[])?;
        }
        if !tb.is_none() {
            val = call_method(interp, &val, "with_traceback", &[tb])?;
        }
        Ok(raise_object(interp, val))
    })();
    result.unwrap_or_else(|error| error)
}

/// `raise obj`: a class is instantiated first.
fn raise_object(interp: &mut Interpreter, obj: Value) -> PyError {
    let (is_class, is_instance) = match obj.kind() {
        ValueKind::PyClass(_) => (true, false),
        ValueKind::PyInstance(_) => (false, true),
        _ => (false, false),
    };
    if is_class {
        return match call(interp, obj, &[]) {
            Ok(instance) => PyError::raised(instance),
            Err(error) => error,
        };
    }
    if is_instance {
        return PyError::raised(obj);
    }
    PyError::named("TypeError", "exceptions must derive from BaseException")
}

// ── `AsyncGenerator` mixins ──────────────────────────────────────────────────

/// `async def __anext__(self): return await self.asend(None)`.
pub(super) fn async_generator_anext(
    _interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut bound = bind(&sig("AsyncGenerator.__anext__", &["self"], 0), call_args)?;
    let this = bound[0].take().unwrap_or_else(Value::none);
    coroutine(move |interp| {
        let awaitable = call_method(interp, &this, "asend", &[Value::none()])?;
        Ok(Flow::Await(
            awaitable,
            Box::new(|_, result| result.map(Flow::Return)),
        ))
    })
}

/// `AsyncGenerator.aclose`:
///
/// ```python
/// try:
///     await self.athrow(GeneratorExit)
/// except (GeneratorExit, StopAsyncIteration):
///     pass
/// else:
///     raise RuntimeError("asynchronous generator ignored GeneratorExit")
/// ```
pub(super) fn async_generator_aclose(
    _interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut bound = bind(&sig("AsyncGenerator.aclose", &["self"], 0), call_args)?;
    let this = bound[0].take().unwrap_or_else(Value::none);
    coroutine(move |interp| {
        let saved = (pyrust_core::captured_error_frames_len() > 0)
            .then(pyrust_core::clone_captured_error_frames);
        let awaitable = call_method(interp, &this, "athrow", &[exc_class("GeneratorExit")])?;
        Ok(Flow::Await(
            awaitable,
            Box::new(move |_, result| match result {
                Ok(_) => Err(PyError::named(
                    "RuntimeError",
                    "asynchronous generator ignored GeneratorExit",
                )),
                Err(error)
                    if is_exc(&error, "GeneratorExit") || is_exc(&error, "StopAsyncIteration") =>
                {
                    forget_handled_frames(saved);
                    Ok(Flow::Return(Value::none()))
                }
                Err(error) => Err(error),
            }),
        ))
    })
}
