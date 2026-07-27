// Protocol and state helpers shared by functools implementations.

/// Extract the implicit instance argument prepended to class methods.
fn expect_self(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|argument| argument.value.kind()) {
        Some(ValueKind::PyInstance(instance)) => Ok(Rc::clone(instance)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Reject public arguments to internally constructed no-op initializers.
fn reject_extra_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments (got {})", args.len() - 1),
        ));
    }
    Ok(Value::none())
}

/// Report inaccessible or corrupted private implementation state.
fn internal(fn_name: &str) -> PyError {
    PyError::Runtime(format!("internal: {fn_name}() instance state corrupted"))
}
