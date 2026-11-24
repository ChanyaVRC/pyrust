// `collections.deque` receiver guards, storage access, mutation checks,
// iterator constructors, and interpreter-level sequence hooks.
// Python-visible deque method bodies live in `deque_methods.rs`.

fn expect_self_no_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    expect_self(args, fn_name)
}

fn expect_self_one_arg<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
) -> Result<(Rc<RefCell<PyInstance>>, &'a Value)> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    Ok((inst, &args[1].value))
}

fn deque_bump_state(inst: &Rc<RefCell<PyInstance>>) {
    let storage = inst.borrow().attrs.get_slot("_items").cloned();
    if let Some(storage) = storage {
        pyrust_builtins::deque_storage::bump_mutation_state(&storage);
    }
}

/// Whether `value` is a `collections.deque` or subclass instance.
pub(crate) fn is_deque_value(value: &Value) -> bool {
    let ValueKind::PyInstance(inst) = value.kind() else {
        return false;
    };
    let class = Rc::clone(&inst.borrow().class);
    is_canonical_collection_class_or_subclass(&class, CanonicalCollectionKind::Deque)
}

/// Whether `value` is an *exact* `collections.deque` instance (not a subclass).
pub(crate) fn is_exact_deque_value(value: &Value) -> bool {
    let ValueKind::PyInstance(inst) = value.kind() else {
        return false;
    };
    let class = Rc::clone(&inst.borrow().class);
    is_canonical_deque_class(&class)
}

/// Extend a deque instance in place from `iterable` (right end).
pub(crate) fn deque_extend_in_place(
    interp: &mut crate::Interpreter,
    target: &Value,
    iterable: &Value,
) -> Result<()> {
    let ValueKind::PyInstance(inst) = target.kind() else {
        return Err(PyError::runtime(
            "internal: expected deque instance".to_string(),
        ));
    };
    let inst = Rc::clone(inst);
    deque_extend_iterable(interp, &inst, iterable, false)?;
    Ok(())
}

fn deque_extend_iterable(
    interp: &mut crate::Interpreter,
    inst: &Rc<RefCell<PyInstance>>,
    iterable: &Value,
    left: bool,
) -> Result<Value> {
    let snapshot = match iterable.kind() {
        ValueKind::List(items) => Some(items.to_vec()),
        ValueKind::Tuple(items) => Some(items.to_vec()),
        ValueKind::PyInstance(other) if Rc::ptr_eq(inst, other) => {
            let class = Rc::clone(&inst.borrow().class);
            let custom_iter = crate::interpreter::lookup_class_attr_owner(&class, "__iter__")
                .is_some_and(|owner| !is_canonical_deque_class(&owner));
            if custom_iter {
                Some(interp.collect_iterable(iterable)?)
            } else {
                Some(deque_items_snapshot(inst)?)
            }
        }
        _ => None,
    };
    let maxlen = deque_maxlen(inst);
    if let Some(values) = snapshot {
        if maxlen != Some(0) && !values.is_empty() {
            let data = deque_items_data(inst)?;
            let mut data = data.borrow_mut();
            for value in values {
                deque_push_one(&mut data, value, maxlen, left);
            }
            drop(data);
            deque_bump_state(inst);
        }
        return Ok(Value::none());
    }

    let iterator = crate::interpreter::make_iterator(interp, iterable)?;
    let exhausted = Value::list(Vec::new());
    let exhausted_id = exhausted.value_id();
    loop {
        let value = interp.call_next(&iterator, Some(exhausted.clone()))?;
        if value.value_id().is_some() && value.value_id() == exhausted_id {
            break;
        }
        if maxlen == Some(0) {
            continue;
        }
        let data = deque_items_data(inst)?;
        deque_push_one(&mut data.borrow_mut(), value, maxlen, left);
        deque_bump_state(inst);
    }
    Ok(Value::none())
}

fn deque_push_one(
    data: &mut std::collections::VecDeque<Value>,
    value: Value,
    maxlen: Option<usize>,
    left: bool,
) {
    if let Some(limit) = maxlen
        && data.len() >= limit
    {
        if left {
            data.pop_back();
        } else {
            data.pop_front();
        }
    }
    if left {
        data.push_front(value);
    } else {
        data.push_back(value);
    }
}

fn deque_storage_value(inst: &Rc<RefCell<PyInstance>>) -> Result<Value> {
    {
        let borrow = inst.borrow();
        if let Some(value) = borrow.attrs.get_slot("_items") {
            if pyrust_builtins::deque_storage::is_storage(value) {
                return Ok(value.clone());
            }
            return Err(PyError::named(
                "TypeError",
                "deque._items has been overwritten with invalid storage; \
                 don't assign to internal attributes"
                    .to_string(),
            ));
        }
    }

    let storage = pyrust_builtins::deque_storage::deque_storage(Vec::new());
    inst.borrow_mut()
        .attrs
        .insert_slot("_items", storage.clone());
    Ok(storage)
}

fn deque_items_data(
    inst: &Rc<RefCell<PyInstance>>,
) -> Result<pyrust_builtins::deque_storage::DequeData> {
    let storage = deque_storage_value(inst)?;
    pyrust_builtins::deque_storage::data(&storage)
        .ok_or_else(|| PyError::runtime("internal: deque storage lost its buffer".to_string()))
}

fn deque_iterator_frame(
    inst: &Rc<RefCell<PyInstance>>,
    reverse: bool,
    consumed: usize,
) -> Result<NativeIterFrame> {
    let storage = deque_storage_value(inst)?;
    let items = pyrust_builtins::deque_storage::data(&storage)
        .ok_or_else(|| PyError::runtime("internal: deque storage lost its buffer".to_string()))?;
    let counter = pyrust_builtins::deque_storage::mutation_state(&storage).ok_or_else(|| {
        PyError::runtime("internal: deque storage lost its mutation state".to_string())
    })?;
    Ok(NativeIterFrame::guarded_deque(
        items,
        counter,
        Value::py_instance(Rc::clone(inst)),
        deque_iterator_replacement,
        reverse,
        consumed,
    ))
}

fn deque_iterator_at(
    inst: &Rc<RefCell<PyInstance>>,
    reverse: bool,
    consumed: usize,
) -> Result<Value> {
    Ok(Value::generator(Box::new(deque_iterator_frame(
        inst, reverse, consumed,
    )?)))
}

fn deque_iterator(inst: &Rc<RefCell<PyInstance>>, reverse: bool) -> Result<Value> {
    deque_iterator_at(inst, reverse, 0)
}

pub(crate) fn deque_iterator_constructor(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    reverse: bool,
) -> Result<Value> {
    let positional: Vec<&Value> = args
        .iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
        .collect();
    if positional.is_empty() {
        return Err(pyrust_core::type_err!(
            "function takes at least 1 argument (0 given)"
        ));
    }
    if positional.len() > 2 {
        return Err(pyrust_core::type_err!(
            "function takes at most 2 arguments ({} given)",
            positional.len()
        ));
    }
    let ValueKind::PyInstance(inst) = positional[0].kind() else {
        return Err(pyrust_core::type_err!(
            "argument 1 must be collections.deque, not {}",
            crate::interpreter::value_type_name_str(positional[0])
        ));
    };
    if !is_canonical_collection_class_or_subclass(
        &inst.borrow().class,
        CanonicalCollectionKind::Deque,
    ) {
        return Err(pyrust_core::type_err!(
            "argument 1 must be collections.deque, not {}",
            crate::interpreter::value_type_name_str(positional[0])
        ));
    }
    let consumed = if let Some(index) = positional.get(1) {
        interp
            .value_to_isize(index, "Python int too large to convert to C ssize_t")?
            .max(0) as usize
    } else {
        0
    };
    deque_iterator_at(inst, reverse, consumed)
}

pub(crate) fn deque_iterator_replacement(
    value: &Value,
) -> Option<(
    pyrust_builtins::deque_storage::DequeData,
    pyrust_builtins::deque_storage::DequeMutationState,
)> {
    let ValueKind::PyInstance(inst) = value.kind() else {
        return None;
    };
    if !is_canonical_collection_class_or_subclass(
        &inst.borrow().class,
        CanonicalCollectionKind::Deque,
    ) {
        return None;
    }
    let storage = inst.borrow().attrs.get_slot("_items").cloned()?;
    let data = pyrust_builtins::deque_storage::data(&storage)?;
    let counter = pyrust_builtins::deque_storage::mutation_state(&storage)?;
    Some((data, counter))
}

fn deque_items_snapshot(inst: &Rc<RefCell<PyInstance>>) -> Result<Vec<Value>> {
    Ok(deque_items_data(inst)?.borrow().iter().cloned().collect())
}

fn deque_items_snapshot_guarded(
    inst: &Rc<RefCell<PyInstance>>,
) -> Result<(
    Vec<Value>,
    pyrust_builtins::deque_storage::DequeMutationState,
    i64,
)> {
    let storage = deque_storage_value(inst)?;
    let mutation_state =
        pyrust_builtins::deque_storage::mutation_state(&storage).ok_or_else(|| {
            PyError::runtime("internal: deque storage lost its mutation state".to_string())
        })?;
    let version = mutation_state.get();
    let items = pyrust_builtins::deque_storage::snapshot(&storage)
        .ok_or_else(|| PyError::runtime("internal: deque storage lost its buffer".to_string()))?;
    Ok((items, mutation_state, version))
}

fn deque_items_guarded_live(
    inst: &Rc<RefCell<PyInstance>>,
) -> Result<(
    pyrust_builtins::deque_storage::DequeData,
    pyrust_builtins::deque_storage::DequeMutationState,
    i64,
)> {
    let storage = deque_storage_value(inst)?;
    let mutation_state =
        pyrust_builtins::deque_storage::mutation_state(&storage).ok_or_else(|| {
            PyError::runtime("internal: deque storage lost its mutation state".to_string())
        })?;
    let version = mutation_state.get();
    let items = pyrust_builtins::deque_storage::data(&storage)
        .ok_or_else(|| PyError::runtime("internal: deque storage lost its buffer".to_string()))?;
    Ok((items, mutation_state, version))
}

fn deque_require_unmutated(
    mutation_state: &pyrust_builtins::deque_storage::DequeMutationState,
    version: i64,
    exception: &'static str,
) -> Result<()> {
    if mutation_state.get() != version {
        return Err(PyError::named(
            exception,
            "deque mutated during iteration".to_string(),
        ));
    }
    Ok(())
}

fn deque_maxlen(inst: &Rc<RefCell<PyInstance>>) -> Option<usize> {
    let borrow = inst.borrow();
    match borrow.attrs.get_slot("maxlen").map(|v| v.kind()) {
        Some(ValueKind::Int(n)) if n >= 0 => Some(n as usize),
        Some(ValueKind::Bool(b)) => Some(b as usize),
        _ => None,
    }
}

fn deque_items_of(value: &Value) -> Option<Vec<Value>> {
    let ValueKind::PyInstance(inst) = value.kind() else {
        return None;
    };
    let borrow = inst.borrow();
    if !is_canonical_collection_class_or_subclass(&borrow.class, CanonicalCollectionKind::Deque) {
        return None;
    }
    match borrow.attrs.get_slot("_items") {
        Some(value) => pyrust_builtins::deque_storage::snapshot(value),
        None => Some(Vec::new()),
    }
}

fn deque_resolve_search_bound(interp: &mut crate::Interpreter, value: &Value) -> Result<Value> {
    interp.value_to_index(value, |_| {
        pyrust_core::type_err!("slice indices must be integers or have an __index__ method")
    })
}

fn deque_normalize_search_bound(value: &Value, len: usize) -> usize {
    match value.kind() {
        ValueKind::Int(index) => pyrust_builtins::sequence::normalise_index(index, len).min(len),
        ValueKind::Bool(index) => {
            pyrust_builtins::sequence::normalise_index(index as i64, len).min(len)
        }
        ValueKind::BigInt(index) => {
            pyrust_builtins::sequence::normalise_bigint_index(index, len).min(len)
        }
        _ => unreachable!("value_to_index guarantees an integer"),
    }
}

fn deque_index_i64(interp: &mut crate::Interpreter, value: &Value) -> Result<i64> {
    let overflow = format!(
        "cannot fit '{}' into an index-sized integer",
        value_type_name_str(value)
    );
    interp.value_to_isize(value, &overflow)
}

fn deque_normalize_index(i: i64, len: usize) -> Result<usize> {
    let idx = if i < 0 {
        let adjusted = i + len as i64;
        if adjusted < 0 {
            return Err(PyError::named(
                "IndexError",
                "deque index out of range".to_string(),
            ));
        }
        adjusted as usize
    } else {
        i as usize
    };
    if idx >= len {
        return Err(PyError::named(
            "IndexError",
            "deque index out of range".to_string(),
        ));
    }
    Ok(idx)
}

fn validate_collection_no_arg_method(
    args: &[ExpandedCallArg],
    kind: CanonicalCollectionKind,
    method: &str,
    short_owner: &str,
    full_owner: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    let first = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!("unbound method {short_owner}.{method}() needs an argument"),
        )
    })?;
    let inst = match first.value.kind() {
        ValueKind::PyInstance(rc)
            if is_canonical_collection_class_or_subclass(&rc.borrow().class, kind) =>
        {
            Rc::clone(rc)
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor '{method}' for '{full_owner}' objects doesn't apply to a '{}' object",
                    pyrust_core::error_type_name(&first.value)
                ),
            ));
        }
    };
    if args.iter().skip(1).any(|a| a.name.is_some()) {
        return Err(PyError::named(
            "TypeError",
            format!("{short_owner}.{method}() takes no keyword arguments"),
        ));
    }
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{short_owner}.{method}() takes no arguments ({} given)",
                args.len() - 1
            ),
        ));
    }
    Ok(inst)
}

fn deque_shallow_copy_instance(inst: &Rc<RefCell<PyInstance>>) -> Result<Value> {
    let items = deque_items_snapshot(inst)?;
    let maxlen_val = inst
        .borrow()
        .attrs
        .get_slot("maxlen")
        .cloned()
        .unwrap_or_else(Value::none);
    let class = Rc::clone(&inst.borrow().class);
    let mut attrs = InstanceAttrs::new();
    attrs.insert_slot(
        "_items",
        pyrust_builtins::deque_storage::deque_storage(items),
    );
    attrs.insert_slot("maxlen", maxlen_val);
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}
