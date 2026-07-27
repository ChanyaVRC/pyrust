// Shared-lookahead cursor operations for `groupby` and `_grouper`.

/// Apply `key_fn(item)` when present; otherwise the item is its own key.
fn compute_key(interp: &mut crate::Interpreter, key_fn: &Value, item: &Value) -> Result<Value> {
    if key_fn.is_none() {
        Ok(item.clone())
    } else {
        interp.call_function_expanded(
            key_fn.clone(),
            &[ExpandedCallArg {
                name: None,
                value: item.clone(),
            }],
        )
    }
}

/// Compare group keys through Python equality and truth conversion.
fn keys_equal(interp: &mut crate::Interpreter, left: &Value, right: &Value) -> Result<bool> {
    let equal = interp.eval_binary(left.clone(), crate::ast::BinaryOp::Eq, right.clone())?;
    interp.truthy_value(&equal)
}

/// Read `(has_curr, currkey, has_tgt, tgtkey)` from the shared lookahead.
fn read_groupby_curr(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<(bool, Value, bool, Value)> {
    let attrs = inst.borrow();
    let has_current = matches!(
        attrs.attrs.get("_has_curr").map(|value| value.kind()),
        Some(ValueKind::Bool(true))
    );
    let has_target = matches!(
        attrs.attrs.get("_has_tgt").map(|value| value.kind()),
        Some(ValueKind::Bool(true))
    );
    let current_key = attrs
        .attrs
        .get("_currkey")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    let target_key = attrs
        .attrs
        .get("_tgtkey")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    Ok((has_current, current_key, has_target, target_key))
}

/// Ensure the shared cursor holds one lookahead element. Fetching remains lazy
/// so side-effecting sources advance at the same point as CPython.
fn groupby_ensure_curr(
    interp: &mut crate::Interpreter,
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<bool> {
    let (has_current, iterator, key_function) = {
        let attrs = inst.borrow();
        let has_current = matches!(
            attrs.attrs.get("_has_curr").map(|value| value.kind()),
            Some(ValueKind::Bool(true))
        );
        (
            has_current,
            attrs
                .attrs
                .get("_iter")
                .cloned()
                .ok_or_else(|| internal(fn_name))?,
            attrs
                .attrs
                .get("_keyfn")
                .cloned()
                .ok_or_else(|| internal(fn_name))?,
        )
    };
    if has_current {
        return Ok(true);
    }
    let item = match interp.call_next(&iterator, None) {
        Ok(value) => value,
        Err(error) if is_stop_iteration(&error) => {
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("_has_curr", Value::bool_(false));
            attrs.attrs.insert("_exhausted", Value::bool_(true));
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let key = compute_key(interp, &key_function, &item)?;
    let mut attrs = inst.borrow_mut();
    attrs.attrs.insert("_currvalue", item);
    attrs.attrs.insert("_currkey", key);
    attrs.attrs.insert("_has_curr", Value::bool_(true));
    Ok(true)
}

/// Mark the shared lookahead consumed so the next ensure operation fetches.
fn groupby_clear_curr(inst: &Rc<RefCell<PyInstance>>) {
    inst.borrow_mut()
        .attrs
        .insert("_has_curr", Value::bool_(false));
}
