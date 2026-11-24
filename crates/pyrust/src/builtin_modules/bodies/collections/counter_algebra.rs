// Counter multiset algebra: binary (`+`, `-`, `&`, `|`), unary (`+`, `-`),
// in-place (`+=`, `-=`, `&=`, `|=`), and rich-comparison operations.

#[derive(Copy, Clone)]
enum CounterOp {
    Add,
    Sub,
    And,
    Or,
}

/// Extract a stable Counter backing snapshot.  Regular multiset operations
/// accept Counter instances only; a non-Counter receives `NotImplemented`.
fn counts_of(other: &Value) -> Option<PyDict> {
    let ValueKind::PyInstance(inst) = other.kind() else {
        return None;
    };
    let borrow = inst.borrow();
    if !is_canonical_collection_class_or_subclass(&borrow.class, CanonicalCollectionKind::Counter) {
        return None;
    }
    match borrow.attrs.builtin_data() {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Some(map.clone()),
            _ => Some(PyDict::default()),
        },
        None => Some(PyDict::default()),
    }
}

fn fresh_base_counter(receiver: &Rc<RefCell<PyInstance>>, counts: PyDict) -> Result<Value> {
    let receiver_class = Rc::clone(&receiver.borrow().class);
    let Some(class) =
        canonical_collection_base_for_receiver(&receiver_class, CanonicalCollectionKind::Counter)
    else {
        return Err(PyError::Runtime(
            "internal: canonical Counter class is not registered".to_string(),
        ));
    };
    let mut attrs = InstanceAttrs::new();
    attrs.set_builtin_data(Value::dict(counts));
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// Compute one LHS-key algebra result and apply Counter's positive filter.
#[inline]
fn merge_positive_count<const OP: u8>(
    interp: &mut crate::Interpreter,
    left: &Value,
    right: &Value,
) -> Result<Option<Value>> {
    if let (Some(left), Some(right)) = (exact_small_integer(left), exact_small_integer(right)) {
        let small = match OP {
            0 => left.checked_add(right),
            1 => left.checked_sub(right),
            2 => Some(left.min(right)),
            3 => Some(left.max(right)),
            _ => unreachable!(),
        };
        if let Some(value) = small {
            return Ok((value > 0).then(|| Value::int(value)));
        }
        let value = match OP {
            0 => value_from_bigint(PyBigInt::from(left) + PyBigInt::from(right)),
            1 => value_from_bigint(PyBigInt::from(left) - PyBigInt::from(right)),
            _ => unreachable!(),
        };
        return Ok(counter_positive(interp, &value)?.then_some(value));
    }

    let value = match OP {
        0 => counter_add_values(interp, left.clone(), right.clone())?,
        1 => counter_sub_values(interp, left.clone(), right.clone())?,
        2 => {
            if counter_compare_values(interp, left, right, CountComparison::Lt)? {
                left.clone()
            } else {
                right.clone()
            }
        }
        3 => {
            if counter_compare_values(interp, left, right, CountComparison::Lt)? {
                right.clone()
            } else {
                left.clone()
            }
        }
        _ => unreachable!(),
    };
    Ok(counter_positive(interp, &value)?.then_some(value))
}

/// Merge two Counter snapshots using CPython's operation-specific evaluation
/// order.  `OP` is monomorphized (0 add, 1 subtract, 2 intersection, 3 union).
fn merge_counts_for<const OP: u8>(
    interp: &mut crate::Interpreter,
    lhs: &PyDict,
    rhs: &PyDict,
) -> Result<PyDict> {
    let mut out = PyDict::default();
    for (key, left) in lhs {
        let right = map_get_eq(interp, rhs, key)?.unwrap_or_else(|| Value::int(0));
        if let Some(result) = merge_positive_count::<OP>(interp, left, &right)? {
            out.insert(key.clone(), result);
        }
    }

    if OP == 2 {
        return Ok(out);
    }
    for (key, right) in rhs {
        if map_contains_eq(interp, lhs, key)? {
            continue;
        }
        match OP {
            0 | 3 => {
                if counter_positive(interp, right)? {
                    out.insert(key.clone(), right.clone());
                }
            }
            1 => {
                if counter_negative(interp, right)? {
                    let result = counter_sub_values(interp, Value::int(0), right.clone())?;
                    out.insert(key.clone(), result);
                }
            }
            2 => unreachable!(),
            _ => unreachable!(),
        }
    }
    Ok(out)
}

fn merge_counts(
    interp: &mut crate::Interpreter,
    lhs: &PyDict,
    rhs: &PyDict,
    op: CounterOp,
) -> Result<PyDict> {
    match op {
        CounterOp::Add => merge_counts_for::<0>(interp, lhs, rhs),
        CounterOp::Sub => merge_counts_for::<1>(interp, lhs, rhs),
        CounterOp::And => merge_counts_for::<2>(interp, lhs, rhs),
        CounterOp::Or => merge_counts_for::<3>(interp, lhs, rhs),
    }
}

fn counter_binop(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    op: CounterOp,
) -> Result<Value> {
    let receiver = expect_self(args, "Counter.__binop__")?;
    let lhs = snapshot_counts(args, "Counter.__binop__")?;
    let user = &args[1..];
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "Counter arithmetic op takes exactly 1 argument".to_string(),
        ));
    }
    let rhs = match counts_of(&user[0].value) {
        Some(map) => map,
        None => return Ok(Value::not_implemented()),
    };
    fresh_base_counter(&receiver, merge_counts(interp, &lhs, &rhs, op)?)
}

#[derive(Copy, Clone)]
enum CounterUnaryOp {
    Positive,
    Negative,
}

fn counter_unary(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    op: CounterUnaryOp,
) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "Counter unary operation takes no arguments".to_string(),
        ));
    }
    let receiver = expect_self(args, "Counter.__unary__")?;
    let counts = snapshot_counts(args, "Counter.__unary__")?;
    let mut out = PyDict::default();
    for (key, count) in counts {
        match op {
            CounterUnaryOp::Positive if counter_positive(interp, &count)? => {
                out.insert(key, count);
            }
            CounterUnaryOp::Negative if counter_negative(interp, &count)? => {
                let negated = counter_sub_values(interp, Value::int(0), count)?;
                out.insert(key, negated);
            }
            _ => {}
        }
    }
    fresh_base_counter(&receiver, out)
}

// ── In-place algebra ────────────────────────────────────────────────────────

fn unpack_counter_item(interp: &mut crate::Interpreter, pair: Value) -> Result<(PyKey, Value)> {
    let values = match pair.kind() {
        ValueKind::Tuple(values) => values.to_vec(),
        ValueKind::List(values) => values.iter().cloned().collect(),
        _ => interp.collect_iterable(&pair)?,
    };
    if values.len() < 2 {
        return Err(PyError::named(
            "ValueError",
            format!(
                "not enough values to unpack (expected 2, got {})",
                values.len()
            ),
        ));
    }
    if values.len() > 2 {
        return Err(PyError::named(
            "ValueError",
            "too many values to unpack (expected 2)".to_string(),
        ));
    }
    let key = interp.value_to_pykey(&values[0])?;
    Ok((key, values[1].clone()))
}

fn visit_counter_item_iterator(
    interp: &mut crate::Interpreter,
    iterator: &Value,
    mut visit: impl FnMut(&mut crate::Interpreter, PyKey, Value) -> Result<()>,
) -> Result<()> {
    loop {
        let pair = match interp.call_next(iterator, None) {
            Ok(pair) => pair,
            Err(error) if crate::interpreter::is_stop_iteration_error(&error) => break,
            Err(error) => return Err(error),
        };
        let (key, count) = unpack_counter_item(interp, pair)?;
        visit(interp, key, count)?;
    }
    Ok(())
}

fn counter_operand_items_iterator(interp: &mut crate::Interpreter, other: &Value) -> Result<Value> {
    if other.dict_with(|_| ()).is_some() {
        let view = Interpreter::dict_view_for_backing(other, "items", false)?;
        return crate::interpreter::make_iterator(interp, &view);
    }
    if let ValueKind::PyInstance(inst) = other.kind() {
        let class = Rc::clone(&inst.borrow().class);
        if let Some(method) = lookup_class_attr(&class, "items") {
            let items = invoke_class_method(interp, method, other.clone(), &[])?;
            return crate::interpreter::make_iterator(interp, &items);
        }
    }
    Err(PyError::named(
        "AttributeError",
        format!(
            "'{}' object has no attribute 'items'",
            pyrust_core::error_type_name(other)
        ),
    ))
}

fn counter_mapping_getitem(
    interp: &mut crate::Interpreter,
    other: &Value,
    key: &PyKey,
) -> Result<Value> {
    let key_value = crate::interpreter::key_to_value(key.clone());
    if other.dict_with(|_| ()).is_some() {
        let lookup_key = interp.value_to_pykey(&key_value)?;
        return interp
            .dict_lookup(other, &lookup_key)?
            .map(|(_, value)| value)
            .ok_or_else(|| PyError::key_error(key_value));
    }
    if let ValueKind::PyInstance(inst) = other.kind() {
        let class = Rc::clone(&inst.borrow().class);
        if let Some(method) = lookup_class_attr(&class, "__getitem__") {
            return invoke_class_method(
                interp,
                method,
                other.clone(),
                &[ExpandedCallArg {
                    name: None,
                    value: key_value,
                }],
            );
        }
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "'{}' object is not subscriptable",
            pyrust_core::error_type_name(other)
        ),
    ))
}

/// Match `_keep_positive`: finish all `count > 0` tests first, then delete the
/// collected keys.
fn counter_keep_positive(interp: &mut crate::Interpreter, backing: &Value) -> Result<()> {
    let items_view = Interpreter::dict_view_for_backing(backing, "items", false)?;
    let iterator = crate::interpreter::make_iterator(interp, &items_view)?;
    let mut nonpositive = Vec::new();
    visit_counter_item_iterator(interp, &iterator, |interp, key, count| {
        if !counter_positive(interp, &count)? {
            nonpositive.push(key);
        }
        Ok(())
    })?;
    backing.dict_shift_remove_many(nonpositive)?;
    Ok(())
}

fn counter_inplace_op(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    op: CounterOp,
) -> Result<Value> {
    let inst = expect_self(args, "Counter.__inplace__")?;
    let user = &args[1..];
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "Counter arithmetic op takes exactly 1 argument".to_string(),
        ));
    }
    let other = user[0].value.clone();
    let backing = counter_backing(args, "Counter.__inplace__")?;

    match op {
        CounterOp::Add | CounterOp::Sub | CounterOp::Or => {
            let iterator = counter_operand_items_iterator(interp, &other)?;
            visit_counter_item_iterator(interp, &iterator, |interp, key, incoming| {
                let current = interp
                    .dict_lookup(&backing, &key)?
                    .map(|(_, value)| value)
                    .unwrap_or_else(|| Value::int(0));
                let result = match op {
                    CounterOp::Add => counter_add_values(interp, current, incoming)?,
                    CounterOp::Sub => counter_sub_values(interp, current, incoming)?,
                    CounterOp::Or => {
                        if counter_compare_values(interp, &incoming, &current, CountComparison::Gt)?
                        {
                            incoming
                        } else {
                            return Ok(());
                        }
                    }
                    CounterOp::And => unreachable!(),
                };
                interp.dict_insert_value(&backing, key, result)
            })?;
        }
        CounterOp::And => {
            let items_view = Interpreter::dict_view_for_backing(&backing, "items", false)?;
            let iterator = crate::interpreter::make_iterator(interp, &items_view)?;
            visit_counter_item_iterator(interp, &iterator, |interp, key, current| {
                let incoming = counter_mapping_getitem(interp, &other, &key)?;
                if counter_compare_values(interp, &incoming, &current, CountComparison::Lt)? {
                    interp.dict_insert_value(&backing, key, incoming)?;
                }
                Ok(())
            })?;
        }
    }
    counter_keep_positive(interp, &backing)?;
    Ok(Value::py_instance(inst))
}

// ── Missing-is-zero rich comparisons ───────────────────────────────────────

#[derive(Copy, Clone)]
enum CounterCompareOp {
    Eq,
    Ne,
    Le,
    Lt,
    Ge,
    Gt,
}

fn counter_all_relation(
    interp: &mut crate::Interpreter,
    lhs: &PyDict,
    rhs: &PyDict,
    op: CountComparison,
) -> Result<bool> {
    for (key, left) in lhs {
        let right = map_get_eq(interp, rhs, key)?.unwrap_or_else(|| Value::int(0));
        if !counter_compare_values(interp, left, &right, op)? {
            return Ok(false);
        }
    }
    for (key, right) in rhs {
        let left = map_get_eq(interp, lhs, key)?.unwrap_or_else(|| Value::int(0));
        if !counter_compare_values(interp, &left, right, op)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn counter_compare(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    op: CounterCompareOp,
) -> Result<Value> {
    let lhs = snapshot_counts(args, "Counter.__compare__")?;
    let user = &args[1..];
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "Counter comparison takes exactly 1 argument".to_string(),
        ));
    }
    let rhs = match counts_of(&user[0].value) {
        Some(map) => map,
        None if matches!(op, CounterCompareOp::Eq | CounterCompareOp::Ne)
            && user[0].value.dict_with(|_| ()).is_some() =>
        {
            return interp.eval_binary(
                Value::dict(lhs),
                if matches!(op, CounterCompareOp::Eq) {
                    BinaryOp::Eq
                } else {
                    BinaryOp::Ne
                },
                user[0].value.clone(),
            );
        }
        None => return Ok(Value::not_implemented()),
    };
    let result = match op {
        CounterCompareOp::Eq => counter_all_relation(interp, &lhs, &rhs, CountComparison::Eq)?,
        CounterCompareOp::Ne => !counter_all_relation(interp, &lhs, &rhs, CountComparison::Eq)?,
        CounterCompareOp::Le => counter_all_relation(interp, &lhs, &rhs, CountComparison::Le)?,
        CounterCompareOp::Lt => {
            counter_all_relation(interp, &lhs, &rhs, CountComparison::Le)?
                && !counter_all_relation(interp, &lhs, &rhs, CountComparison::Eq)?
        }
        CounterCompareOp::Ge => counter_all_relation(interp, &lhs, &rhs, CountComparison::Ge)?,
        CounterCompareOp::Gt => {
            counter_all_relation(interp, &lhs, &rhs, CountComparison::Ge)?
                && !counter_all_relation(interp, &lhs, &rhs, CountComparison::Eq)?
        }
    };
    Ok(Value::bool_(result))
}
