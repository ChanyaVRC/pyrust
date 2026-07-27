// State decoding for `partial`.

fn read_partial_state(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<(Value, Vec<Value>, PyDict)> {
    let borrow = inst.borrow();
    let func = borrow
        .attrs
        .get("func")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    let args = match borrow.attrs.get("args").map(|value| value.kind()) {
        Some(ValueKind::Tuple(items)) => items.to_vec(),
        _ => return Err(internal(fn_name)),
    };
    let kwargs = match borrow.attrs.get("keywords").map(|value| value.kind()) {
        Some(ValueKind::Dict(dict)) => dict.clone(),
        _ => return Err(internal(fn_name)),
    };
    Ok((func, args, kwargs))
}
