//! `ContextDecorator` / `AsyncContextDecorator` and the `_DecoratedCallable`
//! objects their `__call__` produces (CPython's `inner` closures), plus native
//! `with` / `async with` statement drivers shared by those callables.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

use super::coroutine::{self, Flow};
use super::decorators::update_wrapper;
use super::registry::{Sibling, make_sibling_instance};
use super::support::{
    bind_required, call, call_method, exception_of, expect_self, optional_attr, required_attr,
    type_of,
};

fn none_triple() -> [Value; 3] {
    [Value::none(), Value::none(), Value::none()]
}

fn with_protocol_error(cm: &Value, protocol: &str) -> PyError {
    PyError::named(
        "TypeError",
        format!(
            "'{}' object does not support the {protocol}protocol",
            value_type_name_str(cm)
        ),
    )
}

/// `type(cm).<enter>` / `type(cm).<exit>`, looked up on the type like the
/// `with` statement does.
fn lookup_protocol(
    interp: &mut Interpreter,
    cm: &Value,
    enter: &str,
    exit: &str,
    protocol: &str,
) -> Result<(Value, Value)> {
    let class = type_of(cm);
    let enter_method = optional_attr(interp, &class, enter)?;
    let exit_method = optional_attr(interp, &class, exit)?;
    match (enter_method, exit_method) {
        (Some(enter_method), Some(exit_method)) => Ok((enter_method, exit_method)),
        _ => Err(with_protocol_error(cm, protocol)),
    }
}

fn traceback_of(interp: &mut Interpreter, exception: &Value) -> Result<Value> {
    Ok(optional_attr(interp, exception, "__traceback__")?.unwrap_or_else(Value::none))
}

/// `with cm: return body()`.
pub(super) fn run_with(
    interp: &mut Interpreter,
    cm: &Value,
    body: impl FnOnce(&mut Interpreter) -> Result<Value>,
) -> Result<Value> {
    let (enter, exit) = lookup_protocol(interp, cm, "__enter__", "__exit__", "context manager ")?;
    call(interp, &enter, std::slice::from_ref(cm))?;
    match body(interp) {
        Ok(value) => {
            let [typ, exc, tb] = none_triple();
            call(interp, &exit, &[cm.clone(), typ, exc, tb])?;
            Ok(value)
        }
        Err(error) => {
            let (exception, error) = exception_of(interp, error)?;
            let typ = type_of(&exception);
            let tb = traceback_of(interp, &exception)?;
            let outcome = call(interp, &exit, &[cm.clone(), typ, exception, tb])?;
            if interp.truthy_value(&outcome)? {
                // The exception was suppressed: the `with` block (and the
                // `return` inside it) is abandoned and execution falls
                // through.
                Ok(Value::none())
            } else {
                Err(error)
            }
        }
    }
}

struct AsyncWith {
    cm: Value,
    exit: Value,
}

/// `__aexit__` after the body of an `async with` finished with `outcome`.
fn finish_async_with(
    interp: &mut Interpreter,
    context: AsyncWith,
    outcome: Result<Value>,
) -> Result<Flow> {
    match outcome {
        Ok(value) => {
            let [typ, exc, tb] = none_triple();
            let awaitable = call(interp, &context.exit, &[context.cm, typ, exc, tb])?;
            Ok(Flow::Await(
                awaitable,
                Box::new(move |_, result| result.map(|_| Flow::Return(value))),
            ))
        }
        Err(error) => {
            let (exception, error) = exception_of(interp, error)?;
            let typ = type_of(&exception);
            let tb = traceback_of(interp, &exception)?;
            let awaitable = call(interp, &context.exit, &[context.cm, typ, exception, tb])?;
            Ok(Flow::Await(
                awaitable,
                Box::new(move |interp, result| {
                    if interp.truthy_value(&result?)? {
                        Ok(Flow::Return(Value::none()))
                    } else {
                        Err(error)
                    }
                }),
            ))
        }
    }
}

/// Run `__aexit__` once the coroutine body described by `flow` completes.
fn guard_async_with(
    interp: &mut Interpreter,
    flow: Result<Flow>,
    context: AsyncWith,
) -> Result<Flow> {
    match flow {
        Ok(Flow::Return(value)) => finish_async_with(interp, context, Ok(value)),
        Err(error) => finish_async_with(interp, context, Err(error)),
        Ok(Flow::Await(awaitable, cont)) => Ok(Flow::Await(
            awaitable,
            Box::new(move |interp, result| {
                let next = cont(interp, result);
                guard_async_with(interp, next, context)
            }),
        )),
    }
}

/// `async with cm: <body>` for a native coroutine body.
pub(super) fn async_with(
    interp: &mut Interpreter,
    cm: &Value,
    body: impl FnOnce(&mut Interpreter) -> Result<Flow> + 'static,
) -> Result<Flow> {
    let (enter, exit) = lookup_protocol(
        interp,
        cm,
        "__aenter__",
        "__aexit__",
        "asynchronous context manager ",
    )?;
    let entering = call(interp, &enter, std::slice::from_ref(cm))?;
    let context = AsyncWith {
        cm: cm.clone(),
        exit,
    };
    Ok(Flow::Await(
        entering,
        Box::new(move |interp, result| {
            result?;
            let flow = body(interp);
            guard_async_with(interp, flow, context)
        }),
    ))
}

/// `ContextDecorator._recreate_cm(self)` / `AsyncContextDecorator._recreate_cm`.
pub(super) fn recreate_cm(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    Ok(Value::py_instance(inst))
}

/// `ContextDecorator.__call__(self, func)`: the decorated callable.
pub(super) fn decorate(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    is_async: bool,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let func = bind_required(args, fn_name, &["func"])?.remove(0);
    let class = Rc::clone(&inst.borrow().class);
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_cm", Value::py_instance(inst));
    attrs.insert("_func", func.clone());
    attrs.insert("_async", Value::bool_(is_async));
    let decorated = make_sibling_instance(
        Sibling::DecoratedCallable,
        &class,
        "_DecoratedCallable",
        attrs,
    );
    update_wrapper(interp, &decorated, &func)?;
    Ok(decorated)
}

/// `inner.__get__(obj, objtype=None)`: decorated functions bind as methods.
pub(super) fn decorated_get(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let owner = args.get(1).map(|a| a.value.clone());
    let Some(owner) = owner.filter(|owner| !owner.is_none()) else {
        return Ok(Value::py_instance(inst));
    };
    let (class, mut attrs) = {
        let decorated = inst.borrow();
        (Rc::clone(&decorated.class), decorated.attrs.clone())
    };
    attrs.insert("_bound", owner);
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// `inner(*args, **kwds)`.
pub(super) fn decorated_call(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let owner = super::support::instance_attr(&inst, "_cm");
    let func = required_attr(&inst, "_func", fn_name)?;
    let owner =
        owner.ok_or_else(|| PyError::runtime(format!("internal: {fn_name}() _cm missing")))?;
    let is_async = super::support::instance_attr(&inst, "_async")
        .is_some_and(|flag| matches!(flag.kind(), ValueKind::Bool(true)));
    let mut call_args: Vec<ExpandedCallArg> = Vec::new();
    if let Some(bound) = super::support::instance_attr(&inst, "_bound") {
        call_args.push(ExpandedCallArg {
            name: None,
            value: bound,
        });
    }
    call_args.extend(args[1..].iter().cloned());
    if is_async {
        return coroutine::coroutine(move |interp| {
            let cm = call_method(interp, &owner, "_recreate_cm", &[])?;
            async_with(interp, &cm, move |interp| {
                let awaitable = interp.call_function_expanded(func, &call_args)?;
                Ok(Flow::Await(
                    awaitable,
                    Box::new(|_, result| result.map(Flow::Return)),
                ))
            })
        });
    }
    let cm = call_method(interp, &owner, "_recreate_cm", &[])?;
    run_with(interp, &cm, move |interp| {
        interp.call_function_expanded(func, &call_args)
    })
}
