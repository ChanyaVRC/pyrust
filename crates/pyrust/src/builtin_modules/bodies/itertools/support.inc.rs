// Shared protocol and state helpers used across iterator classes.

/// Method-shared `self` extractor — `args[0]` is the instance.
fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<std::cell::RefCell<PyInstance>>> {
    match args.first().map(|argument| argument.value.kind()) {
        Some(ValueKind::PyInstance(instance)) => Ok(Rc::clone(instance)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Raise canonical `StopIteration` when an iterator's boolean state flag is
/// already true.
fn check_not_exhausted(inst: &Rc<std::cell::RefCell<PyInstance>>, flag: &str) -> Result<()> {
    if matches!(
        inst.borrow().attrs.get(flag).map(|value| value.kind()),
        Some(ValueKind::Bool(true))
    ) {
        return Err(PyError::named("StopIteration", String::new()));
    }
    Ok(())
}

/// `count.__init__` validation — start/step must be numeric. BigInt remains
/// accepted because `eval_binary(Add)` handles arbitrary-precision progress.
fn require_numeric(value: &Value, _fn_name: &str, _slot: &str) -> Result<()> {
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
fn make_iter(interp: &mut crate::Interpreter, iterable: &Value) -> Result<Value> {
    crate::interpreter::make_iterator(interp, iterable)
}

/// Recognise the canonical StopIteration class and its real subclasses.
fn is_stop_iteration(error: &PyError) -> bool {
    crate::interpreter::is_stop_iteration_error(error)
}

/// Internal-error shorthand for inaccessible or corrupted private state.
fn internal(fn_name: &str) -> PyError {
    PyError::Runtime(format!("internal: {fn_name}() instance state corrupted"))
}
