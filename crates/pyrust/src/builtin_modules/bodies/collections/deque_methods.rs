// Python-visible `collections.deque` method implementations: initialization,
// end/index mutations, searching, representation, equality, ordering, and
// repetition.

fn deque_init(interp: &mut Interpreter, args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    let mut pos_iterable: Option<Value> = None;
    let mut pos_maxlen: Option<Value> = None;
    let mut kw_maxlen: Option<Value> = None;
    for arg in user {
        match &arg.name {
            None => {
                if pos_iterable.is_none() {
                    pos_iterable = Some(arg.value.clone());
                } else if pos_maxlen.is_none() {
                    pos_maxlen = Some(arg.value.clone());
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() takes at most 2 arguments"),
                    ));
                }
            }
            Some(name) if name == "maxlen" => {
                if kw_maxlen.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for 'maxlen'"),
                    ));
                }
                kw_maxlen = Some(arg.value.clone());
            }
            Some(name) if name == "iterable" => {
                if pos_iterable.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for 'iterable'"),
                    ));
                }
                pos_iterable = Some(arg.value.clone());
            }
            Some(name) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{name}'"),
                ));
            }
        }
    }
    if pos_maxlen.is_some() && kw_maxlen.is_some() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() got multiple values for argument 'maxlen'"),
        ));
    }
    let raw_maxlen = kw_maxlen.or(pos_maxlen);
    let maxlen: Option<i64> = if let Some(raw) = raw_maxlen.as_ref()
        && !raw.is_none()
    {
        let normalized = match raw.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => raw.clone(),
            ValueKind::PyInstance(_) => coerce_subclass_backing(raw, &[])
                .filter(|backing| {
                    matches!(
                        backing.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    )
                })
                .ok_or_else(|| PyError::named("TypeError", "an integer is required".to_string()))?,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "an integer is required".to_string(),
                ));
            }
        };
        let value =
            interp.value_to_isize(&normalized, "Python int too large to convert to C ssize_t")?;
        if value < 0 {
            return Err(PyError::named(
                "ValueError",
                "maxlen must be non-negative".to_string(),
            ));
        }
        Some(value)
    } else {
        None
    };
    let maxlen_val = match maxlen {
        Some(n) => Value::int(n),
        None => Value::none(),
    };
    {
        let mut attrs = inst.borrow_mut();
        if let Some(previous) = attrs.attrs.get_slot("_items") {
            pyrust_builtins::deque_storage::bump_mutation_state(previous);
        }
        attrs.attrs.insert_slot(
            "_items",
            pyrust_builtins::deque_storage::deque_storage(Vec::new()),
        );
        attrs.attrs.insert_slot("maxlen", maxlen_val);
    }
    if let Some(iterable) = pos_iterable {
        deque_extend_iterable(interp, &inst, &iterable, false)?;
    }
    Ok(Value::none())
}

fn deque_append_end(args: &[ExpandedCallArg], fn_name: &str, left: bool) -> Result<Value> {
    let (inst, arg) = expect_self_one_arg(args, fn_name)?;
    let x = arg.clone();
    let maxlen = deque_maxlen(&inst);
    if let Some(0) = maxlen {
        deque_bump_state(&inst);
        return Ok(Value::none());
    }
    let items = deque_items_data(&inst)?;
    deque_push_one(&mut items.borrow_mut(), x, maxlen, left);
    deque_bump_state(&inst);
    Ok(Value::none())
}

fn deque_pop_end(args: &[ExpandedCallArg], fn_name: &str, left: bool) -> Result<Value> {
    let inst = expect_self_no_args(args, fn_name)?;
    let items = deque_items_data(&inst)?;
    let popped = {
        let mut guard = items.borrow_mut();
        if left {
            guard.pop_front()
        } else {
            guard.pop_back()
        }
    }
    .ok_or_else(|| PyError::named("IndexError", "pop from an empty deque".to_string()))?;
    deque_bump_state(&inst);
    Ok(popped)
}

fn deque_rotate(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    if user.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes at most 1 argument"),
        ));
    }
    let n = if let Some(arg) = user.first() {
        interp.value_to_isize(&arg.value, "Python int too large to convert to C ssize_t")?
    } else {
        1
    };
    let items = deque_items_data(&inst)?;
    let len = items.borrow().len();
    if len == 0 {
        return Ok(Value::none());
    }
    deque_bump_state(&inst);
    if n == 0 {
        return Ok(Value::none());
    }
    let steps = ((n % len as i64) + len as i64) as usize % len;
    if steps != 0 {
        items.borrow_mut().rotate_right(steps);
    }
    Ok(Value::none())
}

fn deque_clear(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self_no_args(args, fn_name)?;
    let items = deque_items_data(&inst)?;
    let changed = !items.borrow().is_empty();
    if changed {
        items.borrow_mut().clear();
        deque_bump_state(&inst);
    }
    Ok(Value::none())
}

fn deque_reduce(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let inst = validate_collection_no_arg_method(
        args,
        CanonicalCollectionKind::Deque,
        "__reduce__",
        "deque",
        "collections.deque",
    )?;
    let class = Rc::clone(&inst.borrow().class);
    let maxlen_val = inst
        .borrow()
        .attrs
        .get_slot("maxlen")
        .cloned()
        .unwrap_or_else(Value::none);
    let ctor_args = if maxlen_val.is_none() {
        Value::tuple(Vec::new())
    } else {
        Value::tuple(vec![Value::tuple(Vec::new()), maxlen_val])
    };
    let self_val = Value::py_instance(Rc::clone(&inst));
    let state = if let Some(getstate) = lookup_class_attr(&class, "__getstate__") {
        invoke_class_method(interp, getstate, self_val, &[])?
    } else {
        crate::builtin_modules::copy::default_instance_state(&inst)
    };
    let list_iter = deque_iterator(&inst, false)?;
    Ok(Value::tuple(vec![
        Value::py_class(class),
        ctor_args,
        state,
        list_iter,
    ]))
}

fn deque_count(interp: &mut Interpreter, args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let (inst, arg) = expect_self_one_arg(args, fn_name)?;
    let target = arg.clone();
    let (items, mutation_state, version) = deque_items_snapshot_guarded(&inst)?;
    let mut n: i64 = 0;
    for v in &items {
        let equal = interp.values_richcompare_eq(v, &target)?;
        deque_require_unmutated(&mutation_state, version, "RuntimeError")?;
        if equal {
            n += 1;
        }
    }
    Ok(Value::int(n))
}

fn deque_remove(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let (inst, arg) = expect_self_one_arg(args, fn_name)?;
    let target = arg.clone();
    let (items, mutation_state, version) = deque_items_snapshot_guarded(&inst)?;
    let mut found: Option<usize> = None;
    for (i, v) in items.iter().enumerate() {
        let equal = interp.values_richcompare_eq(v, &target)?;
        deque_require_unmutated(&mutation_state, version, "IndexError")?;
        if equal {
            found = Some(i);
            break;
        }
    }
    match found {
        Some(i) => {
            deque_items_data(&inst)?.borrow_mut().remove(i);
            deque_bump_state(&inst);
            Ok(Value::none())
        }
        None => Err(PyError::named(
            "ValueError",
            format!("{} is not in deque", target.repr_raw()),
        )),
    }
}

fn deque_index(interp: &mut Interpreter, args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    if user.is_empty() || user.len() > 3 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes 1 to 3 arguments"),
        ));
    }
    let target = user[0].value.clone();
    let start_arg = if let Some(arg) = user.get(1) {
        Some(deque_resolve_search_bound(interp, &arg.value)?)
    } else {
        None
    };
    let stop_arg = if let Some(arg) = user.get(2) {
        Some(deque_resolve_search_bound(interp, &arg.value)?)
    } else {
        None
    };
    let (items, mutation_state, version) = deque_items_snapshot_guarded(&inst)?;
    let len = items.len();
    let start = start_arg
        .as_ref()
        .map_or(0, |value| deque_normalize_search_bound(value, len));
    let stop = stop_arg
        .as_ref()
        .map_or(len, |value| deque_normalize_search_bound(value, len));
    for (i, item) in items.iter().enumerate().take(stop).skip(start) {
        let equal = interp.values_richcompare_eq(item, &target)?;
        deque_require_unmutated(&mutation_state, version, "RuntimeError")?;
        if equal {
            return Ok(Value::int(i as i64));
        }
    }
    Err(PyError::named(
        "ValueError",
        format!("{} is not in deque", target.repr_raw()),
    ))
}

fn deque_insert(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 2 arguments"),
        ));
    }
    let i = interp.value_to_isize(
        &args[1].value,
        "Python int too large to convert to C ssize_t",
    )?;
    let maxlen = deque_maxlen(&inst);
    let items = deque_items_data(&inst)?;
    let cur_len = items.borrow().len();
    if let Some(ml) = maxlen
        && cur_len >= ml
    {
        return Err(PyError::named(
            "IndexError",
            "deque already at its maximum size".to_string(),
        ));
    }
    let x = args[2].value.clone();
    let idx = if i < 0 {
        (cur_len as i64 + i).max(0) as usize
    } else {
        (i as usize).min(cur_len)
    };
    items.borrow_mut().insert(idx, x);
    deque_bump_state(&inst);
    Ok(Value::none())
}

fn deque_getitem(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let (inst, arg) = expect_self_one_arg(args, fn_name)?;
    let index = deque_index_i64(interp, arg)?;
    let items = deque_items_data(&inst)?;
    let len = items.borrow().len();
    let idx = deque_normalize_index(index, len)?;
    items
        .borrow()
        .get(idx)
        .cloned()
        .ok_or_else(|| PyError::Runtime("internal: resolved deque index disappeared".to_string()))
}

fn deque_setitem(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 2 arguments"),
        ));
    }
    let index = deque_index_i64(interp, &args[1].value)?;
    let items = deque_items_data(&inst)?;
    let len = items.borrow().len();
    let idx = deque_normalize_index(index, len)?;
    items.borrow_mut()[idx] = args[2].value.clone();
    Ok(Value::none())
}

fn deque_delitem(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let (inst, arg) = expect_self_one_arg(args, fn_name)?;
    let index = deque_index_i64(interp, arg)?;
    let items = deque_items_data(&inst)?;
    let len = items.borrow().len();
    let idx = deque_normalize_index(index, len)?;
    items.borrow_mut().remove(idx);
    deque_bump_state(&inst);
    Ok(Value::none())
}

fn deque_contains(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let (inst, arg) = expect_self_one_arg(args, fn_name)?;
    let target = arg.clone();
    let (items, mutation_state, version) = deque_items_snapshot_guarded(&inst)?;
    for v in &items {
        let equal = interp.values_richcompare_eq(v, &target)?;
        deque_require_unmutated(&mutation_state, version, "RuntimeError")?;
        if equal {
            return Ok(Value::bool_(true));
        }
    }
    Ok(Value::bool_(false))
}

fn deque_repr(interp: &mut Interpreter, args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let items = deque_items_snapshot(&inst)?;
    let mut inner: Vec<String> = Vec::with_capacity(items.len());
    for v in &items {
        let r = match v.kind() {
            ValueKind::PyInstance(inst_rc) => {
                let inst_rc = Rc::clone(inst_rc);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__repr__") {
                    let result = invoke_class_method(
                        interp,
                        method_val,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[],
                    )?;
                    match result.kind() {
                        ValueKind::Str(s) => s.to_string(),
                        _ => v.repr_raw(),
                    }
                } else {
                    v.repr_raw()
                }
            }
            _ => v.repr_raw(),
        };
        inner.push(r);
    }
    let class_name = inst.borrow().class.borrow().name.clone();
    let items_repr = format!("[{}]", inner.join(", "));
    let s = match deque_maxlen(&inst) {
        None => format!("{class_name}({items_repr})"),
        Some(ml) => format!("{class_name}({items_repr}, maxlen={ml})"),
    };
    Ok(Value::string(s))
}
