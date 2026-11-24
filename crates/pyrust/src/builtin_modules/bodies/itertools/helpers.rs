// Shared protocol, argument validation, and instance-state helpers for
// `itertools` iterator classes.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyInstance, Value, ValueKind};

/// Method-shared `self` extractor — `args[0]` is the instance.
pub(super) fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|argument| argument.value.kind()) {
        Some(ValueKind::PyInstance(instance)) => Ok(Rc::clone(instance)),
        _ => Err(PyError::runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Read a required cloned attribute from an iterator instance.
pub(super) fn attr_cloned(
    inst: &Rc<RefCell<PyInstance>>,
    name: &str,
    fn_name: &str,
) -> Result<Value> {
    inst.borrow()
        .attrs
        .get(name)
        .cloned()
        .ok_or_else(|| internal(fn_name))
}

/// Read a boolean flag attribute (treating missing/non-bool as `false`).
pub(super) fn attr_bool(inst: &Rc<RefCell<PyInstance>>, name: &str) -> bool {
    matches!(
        inst.borrow().attrs.get(name).map(|v| v.kind()),
        Some(ValueKind::Bool(true))
    )
}

/// Read a required integer attribute from an iterator instance.
pub(super) fn attr_i64(inst: &Rc<RefCell<PyInstance>>, name: &str, fn_name: &str) -> Result<i64> {
    match inst.borrow().attrs.get(name).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => Ok(n),
        _ => Err(internal(fn_name)),
    }
}

/// Raise canonical `StopIteration` when an iterator's boolean state flag is
/// already true.
pub(super) fn check_not_exhausted(inst: &Rc<RefCell<PyInstance>>, flag: &str) -> Result<()> {
    if attr_bool(inst, flag) {
        return Err(PyError::named("StopIteration", String::new()));
    }
    Ok(())
}

/// `count.__init__` validation — start/step must be numeric. BigInt remains
/// accepted because `eval_binary(Add)` handles arbitrary-precision progress.
pub(super) fn require_numeric(value: &Value) -> Result<()> {
    if matches!(
        value.kind(),
        ValueKind::Int(_) | ValueKind::Float(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
    ) {
        Ok(())
    } else {
        Err(PyError::named(
            "TypeError",
            "a number is required".to_string(),
        ))
    }
}

/// Construct an iterator through the interpreter protocol so user `__iter__`
/// implementations are handled uniformly.
pub(super) fn make_iter(interp: &mut Interpreter, iterable: &Value) -> Result<Value> {
    crate::interpreter::make_iterator(interp, iterable)
}

/// Recognise the canonical StopIteration class and its real subclasses.
pub(super) fn is_stop_iteration(error: &PyError) -> bool {
    crate::interpreter::is_stop_iteration_error(error)
}

/// Internal-error shorthand for inaccessible or corrupted private state.
pub(super) fn internal(fn_name: &str) -> PyError {
    PyError::runtime(format!("internal: {fn_name}() instance state corrupted"))
}

/// Shared `__init__` for two-argument `(callable, iterable)` itertools classes
/// (`takewhile`, `dropwhile`, `starmap`, `filterfalse`).
pub(super) fn init_binary_func_iter(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    short_name: &str,
    func_slot: &str,
    extra_flag: Option<(&str, bool)>,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded(short_name, user)?;
    if user.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{short_name} expected 2 arguments, got {}", user.len()),
        ));
    }
    let iter = make_iter(interp, &user[1].value)?;
    let mut a = inst.borrow_mut();
    a.attrs.insert(func_slot, user[0].value.clone());
    a.attrs.insert("_iter", iter);
    if let Some((flag_name, initial)) = extra_flag {
        a.attrs.insert(flag_name, Value::bool_(initial));
    }
    Ok(Value::none())
}
