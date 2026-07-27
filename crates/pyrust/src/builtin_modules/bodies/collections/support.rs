// Receiver validation shared by the native collections implementations.
//
// Method-specific argument rules remain with the owning Counter/deque helper.

/// Extract `self` (the first arg, conventionally) as a `PyInstance` Rc.
/// Every method declared inside `class Counter { … }` receives the
/// instance as `args[0]` by macro convention — this helper centralises
/// the downcast + error path so each method's first line stays clean.
fn expect_self(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    let first = args
        .first()
        .ok_or_else(|| PyError::Runtime(format!("internal: {fn_name}() called without self")))?;
    match first.value.kind() {
        ValueKind::PyInstance(rc) => Ok(Rc::clone(rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}
