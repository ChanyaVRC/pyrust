//! `_BaseExitStack`, `ExitStack`, and the `_StackCallback` wrapper objects
//! shared with `AsyncExitStack` (`async_exit_stack.rs`).
//!
//! Mirrors CPython 3.12's `Lib/contextlib.py`: the stack stores
//! `(is_sync, callback)` pairs in `self._exit_callbacks`; `__exit__` unwinds
//! them in LIFO order, chaining `__context__` so the result behaves like nested
//! `with` statements.

use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, Value, ValueKind};

use super::coroutine;
use super::registry::{Sibling, make_instance, make_sibling_instance};
use super::support::{
    bind_args, bind_required, call, call_method, display_name, exception_of, expand_call_args,
    expect_self, optional_attr, positional, protocol_error, required_attr, split_call_args,
    type_of,
};

const MODE_BOUND: i64 = 0;
const MODE_CALLBACK: i64 = 1;
const MODE_ASYNC_CALLBACK: i64 = 2;

// ── _BaseExitStack ───────────────────────────────────────────────────────────

pub(super) fn init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    bind_required(args, fn_name, &[])?;
    inst.borrow_mut()
        .attrs
        .insert("_exit_callbacks", Value::list(vec![]));
    Ok(Value::none())
}

fn callbacks_list(interp: &mut Interpreter, stack: &Value) -> Result<Value> {
    interp.get_attr(stack, "_exit_callbacks")
}

/// `self._push_exit_callback(callback, is_sync)`.
fn push_exit_callback(
    interp: &mut Interpreter,
    stack: &Value,
    callback: Value,
    is_sync: bool,
) -> Result<()> {
    call_method(
        interp,
        stack,
        "_push_exit_callback",
        &[callback, Value::bool_(is_sync)],
    )?;
    Ok(())
}

pub(super) fn push_exit_callback_method(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut bound = bind_args(args, fn_name, &["callback"], &["is_sync"])?.into_iter();
    let callback = bound.next().flatten().unwrap_or_else(Value::none);
    let is_sync = bound.next().flatten().unwrap_or_else(|| Value::bool_(true));
    let callbacks = callbacks_list(interp, &Value::py_instance(inst))?;
    callbacks.list_push(Value::tuple(vec![is_sync, callback]))?;
    Ok(Value::none())
}

/// `self._push_cm_exit(cm, cm_exit)`.
pub(super) fn push_cm_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let [cm, cm_exit] = <[Value; 2]>::try_from(bind_required(args, fn_name, &["cm", "cm_exit"])?)
        .unwrap_or_else(|_| unreachable!("bind_required returns one value per parameter"));
    let stack = Value::py_instance(Rc::clone(&inst));
    let wrapper = new_wrapper(
        &inst,
        MODE_BOUND,
        cm_exit,
        Value::tuple(vec![cm]),
        Value::dict(Default::default()),
        None,
    );
    push_exit_callback(interp, &stack, wrapper, true)?;
    Ok(Value::none())
}

fn new_wrapper(
    stack: &Rc<std::cell::RefCell<crate::value::PyInstance>>,
    mode: i64,
    func: Value,
    bound_args: Value,
    bound_kwds: Value,
    wrapped: Option<Value>,
) -> Value {
    let class = Rc::clone(&stack.borrow().class);
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_mode", Value::int(mode));
    attrs.insert("_func", func);
    attrs.insert("_args", bound_args);
    attrs.insert("_kwds", bound_kwds);
    if let Some(wrapped) = wrapped {
        attrs.insert("__wrapped__", wrapped);
    }
    make_sibling_instance(Sibling::StackCallback, &class, "_StackCallback", attrs)
}

/// `_BaseExitStack._create_exit_wrapper(cm, cm_exit)` (a static method:
/// `MethodType(cm_exit, cm)`).
pub(super) fn create_exit_wrapper(cm: Value, cm_exit: Value) -> Value {
    make_wrapper_without_stack(
        MODE_BOUND,
        cm_exit,
        Value::tuple(vec![cm]),
        Value::dict(Default::default()),
        None,
    )
}

/// `_BaseExitStack._create_cb_wrapper(callback, /, *args, **kwds)` /
/// `AsyncExitStack._create_async_cb_wrapper`.
pub(super) fn create_callback_wrapper(
    callback: Value,
    args: &[Value],
    kwds: indexmap::IndexMap<String, Value>,
    is_async: bool,
) -> Result<Value> {
    let bound_kwds = Value::dict(Default::default());
    for (name, value) in kwds {
        bound_kwds.dict_insert(crate::value::PyKey::str_from(&name), value)?;
    }
    let mode = if is_async {
        MODE_ASYNC_CALLBACK
    } else {
        MODE_CALLBACK
    };
    Ok(make_wrapper_without_stack(
        mode,
        callback.clone(),
        Value::tuple(args.to_vec()),
        bound_kwds,
        Some(callback),
    ))
}

fn make_wrapper_without_stack(
    mode: i64,
    func: Value,
    bound_args: Value,
    bound_kwds: Value,
    wrapped: Option<Value>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_mode", Value::int(mode));
    attrs.insert("_func", func);
    attrs.insert("_args", bound_args);
    attrs.insert("_kwds", bound_kwds);
    if let Some(wrapped) = wrapped {
        attrs.insert("__wrapped__", wrapped);
    }
    make_instance("_StackCallback", attrs)
}

/// `self.callback(callback, /, *args, **kwds)` / `push_async_callback`.
pub(super) fn callback(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    is_async: bool,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let Some(callback) = args.get(1) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{}() missing 1 required positional argument: 'callback'",
                display_name(fn_name)
            ),
        ));
    };
    let callback = callback.value.clone();
    let stack = Value::py_instance(Rc::clone(&inst));
    let (bound_args, bound_kwds) = split_call_args(&args[2..])?;
    let mode = if is_async {
        MODE_ASYNC_CALLBACK
    } else {
        MODE_CALLBACK
    };
    let wrapper = new_wrapper(
        &inst,
        mode,
        callback.clone(),
        bound_args,
        bound_kwds,
        Some(callback.clone()),
    );
    push_exit_callback(interp, &stack, wrapper, !is_async)?;
    // Allow use as a decorator.
    Ok(callback)
}

/// `self.pop_all()`.
pub(super) fn pop_all(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let stack = Value::py_instance(inst);
    let new_stack = call(interp, &type_of(&stack), &[])?;
    let callbacks = interp.get_attr(&stack, "_exit_callbacks")?;
    interp.assign_attr(new_stack.clone(), "_exit_callbacks", callbacks)?;
    interp.assign_attr(stack, "_exit_callbacks", Value::list(vec![]))?;
    Ok(new_stack)
}

/// `self.push(exit)`.
pub(super) fn push(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let exit = bind_required(args, fn_name, &["exit"])?.remove(0);
    let stack = Value::py_instance(inst);
    // The unbound method is used rather than a bound one to follow the
    // standard lookup behaviour for special methods.
    match optional_attr(interp, &type_of(&exit), "__exit__")? {
        None => push_exit_callback(interp, &stack, exit.clone(), true)?,
        Some(exit_method) => {
            call_method(
                interp,
                &stack,
                "_push_cm_exit",
                &[exit.clone(), exit_method],
            )?;
        }
    }
    Ok(exit)
}

/// `self.enter_context(cm)`.
pub(super) fn enter_context(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let cm = bind_required(args, fn_name, &["cm"])?.remove(0);
    let class = type_of(&cm);
    // The special methods are looked up on the type to match the with statement.
    let enter = optional_attr(interp, &class, "__enter__")?;
    let exit = optional_attr(interp, &class, "__exit__")?;
    let (Some(enter), Some(exit)) = (enter, exit) else {
        return Err(protocol_error(interp, &cm, "the context manager protocol")?);
    };
    let result = call(interp, &enter, std::slice::from_ref(&cm))?;
    let stack = Value::py_instance(inst);
    call_method(interp, &stack, "_push_cm_exit", &[cm, exit])?;
    Ok(result)
}

// ── _StackCallback ───────────────────────────────────────────────────────────

pub(super) fn stack_callback_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let _ = expect_self(args, fn_name)?;
    Err(PyError::named(
        "TypeError",
        "cannot create '_StackCallback' instances".to_string(),
    ))
}

/// `wrapper(exc_type, exc, tb)`.
pub(super) fn stack_callback_call(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let func = required_attr(&inst, "_func", fn_name)?;
    let bound_args = required_attr(&inst, "_args", fn_name)?;
    let bound_kwds = required_attr(&inst, "_kwds", fn_name)?;
    let mode = match required_attr(&inst, "_mode", fn_name)?.kind() {
        ValueKind::Int(mode) => mode,
        _ => MODE_BOUND,
    };
    let mut call_args = expand_call_args(interp, &bound_args, &bound_kwds)?;
    if mode == MODE_BOUND {
        call_args.extend(args[1..].iter().cloned());
        return interp.call_function_expanded(func, &call_args);
    }
    if args.len() != 4 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "_exit_wrapper() takes 3 positional arguments but {} were given",
                args.len() - 1
            ),
        ));
    }
    if mode == MODE_CALLBACK {
        interp.call_function_expanded(func, &call_args)?;
        return Ok(Value::none());
    }
    coroutine::await_discarding(move |interp| interp.call_function_expanded(func, &call_args))
}

// ── ExitStack ────────────────────────────────────────────────────────────────

pub(super) fn enter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    Ok(Value::py_instance(inst))
}

/// Unwinding state shared by `ExitStack.__exit__` and `AsyncExitStack.__aexit__`.
pub(super) struct Unwind {
    pub(super) stack: Value,
    pub(super) exc_details: Vec<Value>,
    pub(super) received_exc: bool,
    pub(super) frame_exc: Option<Value>,
    pub(super) suppressed_exc: bool,
    pub(super) pending_raise: bool,
}

impl Unwind {
    pub(super) fn new(interp: &Interpreter, stack: Value, exc_details: Vec<Value>) -> Self {
        let received_exc = exc_details.first().is_some_and(|typ| !typ.is_none());
        Self {
            stack,
            exc_details,
            received_exc,
            // `sys.exc_info()[1]`: the exception being handled by the frame
            // that runs the exit stack.
            frame_exc: interp.active_exception.clone(),
            suppressed_exc: false,
            pending_raise: false,
        }
    }

    fn current_exception(&self) -> Value {
        self.exc_details.get(1).cloned().unwrap_or_else(Value::none)
    }

    /// Pop the next `(is_sync, callback)` pair from `self._exit_callbacks`.
    pub(super) fn pop_callback(&self, interp: &mut Interpreter) -> Result<Option<(bool, Value)>> {
        let callbacks = callbacks_list(interp, &self.stack)?;
        if !interp.truthy_value(&callbacks)? {
            return Ok(None);
        }
        let last = match callbacks.list_len() {
            Some(len) if len > 0 => callbacks.list_pop_at(len - 1)?,
            _ => return Ok(None),
        };
        let ValueKind::Tuple(pair) = last.kind() else {
            return Err(PyError::named(
                "TypeError",
                "cannot unpack non-iterable object".to_string(),
            ));
        };
        let is_sync = pair
            .first()
            .is_some_and(|flag| matches!(flag.kind(), ValueKind::Bool(true)));
        let callback = pair.get(1).cloned().unwrap_or_else(Value::none);
        Ok(Some((is_sync, callback)))
    }

    pub(super) fn callback_args(&self) -> Vec<ExpandedCallArg> {
        self.exc_details.iter().cloned().map(positional).collect()
    }

    /// Record the outcome of one callback: a truthy result suppresses the
    /// exception, a raised exception replaces it (with its `__context__`
    /// fixed up as if the `with` statements were nested).
    pub(super) fn record(&mut self, interp: &mut Interpreter, outcome: Result<Value>) {
        let outcome = outcome.and_then(|value| interp.truthy_value(&value));
        match outcome {
            Ok(true) => {
                self.suppressed_exc = true;
                self.pending_raise = false;
                self.exc_details = vec![Value::none(), Value::none(), Value::none()];
            }
            Ok(false) => {}
            Err(error) => {
                let Ok((new_exc, _)) = exception_of(interp, error) else {
                    return;
                };
                self.fix_exception_context(interp, &new_exc);
                self.pending_raise = true;
                let typ = type_of(&new_exc);
                let tb = optional_attr(interp, &new_exc, "__traceback__")
                    .ok()
                    .flatten()
                    .unwrap_or_else(Value::none);
                self.exc_details = vec![typ, new_exc, tb];
            }
        }
    }

    /// `_fix_exception_context(new_exc, old_exc)`.
    fn fix_exception_context(&self, interp: &mut Interpreter, new_exc: &Value) {
        let old_exc = self.current_exception();
        let mut new_exc = new_exc.clone();
        loop {
            let Ok(Some(context)) = optional_attr(interp, &new_exc, "__context__") else {
                return;
            };
            if context.is_none() || context.is_identical_to(&old_exc) {
                // Context is already set correctly (see issue 20317).
                return;
            }
            if self
                .frame_exc
                .as_ref()
                .is_some_and(|frame_exc| context.is_identical_to(frame_exc))
            {
                break;
            }
            new_exc = context;
        }
        // Change the end of the chain to point to the exception we expect it
        // to reference.
        let _ = interp.assign_attr(new_exc, "__context__", old_exc);
    }

    /// The value `__exit__` returns (or the exception it raises) once every
    /// callback has run.
    pub(super) fn finish(self) -> Result<Value> {
        if self.pending_raise {
            // Raising the exception object itself keeps the carefully set-up
            // `__context__` chain intact.
            return Err(PyError::raised(self.current_exception()));
        }
        Ok(Value::bool_(self.received_exc && self.suppressed_exc))
    }
}

pub(super) fn exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let exc_details = args[1..].iter().map(|arg| arg.value.clone()).collect();
    let mut unwind = Unwind::new(interp, Value::py_instance(inst), exc_details);
    // Callbacks are invoked in LIFO order to match the behaviour of nested
    // context managers.
    while let Some((_, callback)) = unwind.pop_callback(interp)? {
        let outcome = interp.call_function_expanded(callback, &unwind.callback_args());
        unwind.record(interp, outcome);
    }
    unwind.finish()
}

/// `self.close()`: `self.__exit__(None, None, None)`.
pub(super) fn close(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let stack = Value::py_instance(inst);
    call_method(
        interp,
        &stack,
        "__exit__",
        &[Value::none(), Value::none(), Value::none()],
    )?;
    Ok(Value::none())
}
