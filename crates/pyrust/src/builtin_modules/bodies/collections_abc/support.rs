//! Small protocol helpers shared by the native `collections.abc` mixins.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, ProviderIterator, is_stop_iteration_error, lookup_exc_class,
    make_iterator,
};
use crate::value::Value;

use super::definitions::Abc;

#[inline]
pub(super) fn pos(value: Value) -> ExpandedCallArg {
    ExpandedCallArg { name: None, value }
}

/// `callee(*args)`.
pub(super) fn call(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args.iter().cloned().map(pos).collect();
    interp.call_function_expanded(callee, &expanded)
}

/// `obj.name(*args)`.
pub(super) fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let method = interp.get_attr(obj, name)?;
    call(interp, method, args)
}

/// The built-in exception class `name` as a value.
pub(super) fn exc_class(name: &'static str) -> Value {
    lookup_exc_class(name)
        .map(Value::py_class)
        .unwrap_or_else(|| Value::builtin_function(name))
}

/// `raise Name(*args)`, built through the class so the instance carries
/// exactly CPython's `args` (a bare `raise KeyError` has `args == ()`).
pub(super) fn raise_new(interp: &mut Interpreter, name: &'static str, args: &[Value]) -> PyError {
    match call(interp, exc_class(name), args) {
        Ok(instance) => PyError::raised(instance),
        Err(error) => error,
    }
}

/// `raise Name from None`.
pub(super) fn raise_from_none(interp: &mut Interpreter, name: &'static str) -> PyError {
    match call(interp, exc_class(name), &[]) {
        Ok(instance) => {
            let _ = interp.assign_attr(instance.clone(), "__cause__", Value::none());
            let _ =
                interp.assign_attr(instance.clone(), "__suppress_context__", Value::bool_(true));
            PyError::raised(instance)
        }
        Err(error) => error,
    }
}

/// `except Name:` for one of the built-in exception classes.
#[inline]
pub(super) fn is_exc(error: &PyError, name: &str) -> bool {
    error.class_name_is(name)
}

/// `try: f() except <handled>:` — `Ok(Err(error))` when `handled(&error)`.
///
/// A handled exception's traceback frames (recorded while it unwound through
/// Python code) are dropped, as leaving an `except` clause does, so they cannot
/// leak into a later, unrelated exception. Any other error propagates.
pub(super) fn try_except<T>(
    handled: impl FnOnce(&PyError) -> bool,
    f: impl FnOnce() -> Result<T>,
) -> Result<std::result::Result<T, PyError>> {
    let saved = (pyrust_core::captured_error_frames_len() > 0)
        .then(pyrust_core::clone_captured_error_frames);
    match f() {
        Ok(value) => Ok(Ok(value)),
        Err(error) if handled(&error) => {
            forget_handled_frames(saved);
            Ok(Err(error))
        }
        Err(error) => Err(error),
    }
}

/// Restore the captured traceback frames to the snapshot taken before a
/// handled exception started unwinding.
pub(super) fn forget_handled_frames(saved: Option<Vec<pyrust_core::FrameInfo>>) {
    pyrust_core::reset_captured_error_frames();
    for frame in saved.into_iter().flatten().rev() {
        pyrust_core::record_traceback_frame(frame);
    }
}

/// `try: f() except Name:`.
#[inline]
pub(super) fn try_except_named<T>(
    name: &'static str,
    f: impl FnOnce() -> Result<T>,
) -> Result<std::result::Result<T, PyError>> {
    try_except(|error| is_exc(error, name), f)
}

/// `isinstance(obj, cls)` through the full protocol.
pub(super) fn isinstance(interp: &mut Interpreter, obj: &Value, cls: Value) -> Result<bool> {
    let result = call(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls],
    )?;
    interp.truthy_value(&result)
}

/// `isinstance(obj, <canonical ABC>)`.
pub(super) fn isinstance_abc(interp: &mut Interpreter, obj: &Value, abc: Abc) -> Result<bool> {
    isinstance(interp, obj, abc.value())
}

/// `iter(obj)`.
#[inline]
pub(super) fn iter(interp: &mut Interpreter, obj: &Value) -> Result<Value> {
    make_iterator(interp, obj)
}

/// `next(it)`, mapping exhaustion to `None`.
pub(super) fn next(interp: &mut Interpreter, iterator: &Value) -> Result<Option<Value>> {
    Ok(try_except(is_stop_iteration_error, || interp.call_next(iterator, None))?.ok())
}

/// `a, b = item` with CPython's unpacking errors.
pub(super) fn unpack_pair(interp: &mut Interpreter, item: &Value) -> Result<(Value, Value)> {
    let iterator = match try_except_named("TypeError", || iter(interp, item))? {
        Ok(iterator) => iterator,
        Err(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "cannot unpack non-iterable {} object",
                    crate::interpreter::value_type_name_str(item)
                ),
            ));
        }
    };
    let mut values = Vec::with_capacity(2);
    while values.len() < 3 {
        match next(interp, &iterator)? {
            Some(value) => values.push(value),
            None => break,
        }
    }
    match values.len() {
        2 => {
            let second = values.pop().unwrap_or_else(Value::none);
            let first = values.pop().unwrap_or_else(Value::none);
            Ok((first, second))
        }
        3 => Err(PyError::named(
            "ValueError",
            "too many values to unpack (expected 2)",
        )),
        got => Err(PyError::named(
            "ValueError",
            format!("not enough values to unpack (expected 2, got {got})"),
        )),
    }
}

/// `item in container`.
pub(super) fn contains(interp: &mut Interpreter, container: &Value, item: &Value) -> Result<bool> {
    let found = interp.eval_in(container.clone(), item.clone())?;
    interp.truthy_value(&found)
}

/// `a is b or a == b`.
#[inline]
pub(super) fn is_or_eq(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    interp.values_richcompare_eq(a, b)
}

/// `len(obj)`.
#[inline]
pub(super) fn len(interp: &mut Interpreter, obj: &Value) -> Result<i64> {
    interp.object_len(obj)
}

/// `a <op> b` as a plain binary operation.
#[inline]
pub(super) fn binop(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<Value> {
    interp.eval_binary(a.clone(), op, b.clone())
}

/// `bool(a <op> b)`.
pub(super) fn compare(
    interp: &mut Interpreter,
    a: &Value,
    op: BinaryOp,
    b: &Value,
) -> Result<bool> {
    let result = binop(interp, a, op, b)?;
    interp.truthy_value(&result)
}

/// `obj[key]`.
#[inline]
pub(super) fn getitem(interp: &mut Interpreter, obj: &Value, key: &Value) -> Result<Value> {
    crate::builtin_modules::operator::getitem_value(interp, obj.clone(), key.clone())
}

/// `obj[key] = value`.
#[inline]
pub(super) fn setitem(
    interp: &mut Interpreter,
    obj: &Value,
    key: &Value,
    value: &Value,
) -> Result<()> {
    crate::builtin_modules::operator::setitem_value(interp, obj.clone(), key.clone(), value.clone())
}

/// `del obj[key]`.
#[inline]
pub(super) fn delitem(interp: &mut Interpreter, obj: &Value, key: &Value) -> Result<()> {
    crate::builtin_modules::operator::delitem_value(interp, obj.clone(), key.clone())
}

// ── Lazy generator objects ───────────────────────────────────────────────────

/// One step of a native generator body: `Ok(Some(v))` yields `v`, `Ok(None)`
/// returns. The state is never borrowed while the step runs Python code.
pub(super) trait GenBody: 'static {
    fn step(&mut self, interp: &mut Interpreter) -> Result<Option<Value>>;
}

struct GenCell {
    body: Option<Box<dyn GenBody>>,
}

/// Wrap `body` in a lazy iterator presented as a `generator`, like the
/// generator functions and expressions CPython's `_collections_abc` uses.
pub(super) fn generator(body: impl GenBody) -> Value {
    Value::generator(Box::new(ProviderIterator::new(
        Box::new(GenCell {
            body: Some(Box::new(body)),
        }),
        advance_generator,
        "generator",
        "generator",
        None,
    )))
}

fn advance_generator(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    // Take the body out for the duration of the step so a re-entrant `next()`
    // from inside it sees a running (here: finished) generator instead of a
    // borrow conflict.
    let body = {
        let mut state = state_rc.borrow_mut();
        let cell = state
            .downcast_mut::<GenCell>()
            .ok_or_else(|| PyError::runtime("internal: collections.abc generator state".into()))?;
        cell.body.take()
    };
    let Some(mut body) = body else {
        return Ok(None);
    };
    match body.step(interp) {
        Ok(Some(value)) => {
            if let Some(cell) = state_rc.borrow_mut().downcast_mut::<GenCell>() {
                cell.body = Some(body);
            }
            Ok(Some(value))
        }
        // A generator that returned or raised is finished.
        other => other,
    }
}

/// A generator body that iterates `source` lazily, created on the first step.
pub(super) struct LazyIter {
    source: Value,
    iterator: Option<Value>,
}

impl LazyIter {
    pub(super) fn new(source: Value) -> Self {
        Self {
            source,
            iterator: None,
        }
    }

    /// An already-created iterator (a generator expression evaluates its
    /// outermost iterable eagerly).
    pub(super) fn started(iterator: Value) -> Self {
        Self {
            source: Value::none(),
            iterator: Some(iterator),
        }
    }

    pub(super) fn next(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        let iterator = match &self.iterator {
            Some(iterator) => iterator.clone(),
            None => {
                let iterator = iter(interp, &self.source)?;
                self.iterator = Some(iterator.clone());
                iterator
            }
        };
        next(interp, &iterator)
    }
}
