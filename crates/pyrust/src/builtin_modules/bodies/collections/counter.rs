// Counter update, tally, arithmetic, unary, and comparison policy.
//
// Storage/view mechanics live in backing.rs.  Count values are intentionally
// Python objects: only the exact small-int/bool case is handled here, while
// BigInt, float, subclasses, and arbitrary objects go through Python's binary
// and rich-comparison protocols.

/// Apply `update`/`subtract` semantics to an optional positional source and
/// keyword counts.  Each completed item is committed immediately, preserving
/// the visible prefix if a later iterator, hash, comparison, or count operation
/// raises.
fn apply_delta<const SUBTRACT: bool>(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let user = &args[1..];
    let positional: Vec<&ExpandedCallArg> = user.iter().filter(|a| a.name.is_none()).collect();
    let kwargs: Vec<&ExpandedCallArg> = user.iter().filter(|a| a.name.is_some()).collect();
    if positional.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at most one positional argument ({} given)",
                positional.len(),
            ),
        ));
    }
    let backing = counter_backing(args, fn_name)?;
    if let Some(arg) = positional.first() {
        counter_tally_into_backing::<SUBTRACT>(interp, &backing, &arg.value)?;
    }
    counter_apply_kwargs_to_backing::<SUBTRACT>(interp, &backing, &kwargs)?;
    Ok(Value::none())
}

#[inline]
fn exact_small_integer(value: &Value) -> Option<i64> {
    match value.kind() {
        ValueKind::Int(value) => Some(value),
        ValueKind::Bool(value) => Some(value as i64),
        _ => None,
    }
}

/// Exact-int/bool hot path with arbitrary-precision promotion.  Everything
/// else must retain Python's `left + right` dispatch order.
#[inline]
fn counter_add_values(interp: &mut crate::Interpreter, left: Value, right: Value) -> Result<Value> {
    if let (Some(left), Some(right)) = (exact_small_integer(&left), exact_small_integer(&right)) {
        return Ok(match left.checked_add(right) {
            Some(value) => Value::int(value),
            None => value_from_bigint(PyBigInt::from(left) + PyBigInt::from(right)),
        });
    }
    interp.eval_binary(left, BinaryOp::Add, right)
}

/// Exact-int/bool hot path with arbitrary-precision promotion.  Everything
/// else must retain Python's `left - right` dispatch order.
#[inline]
fn counter_sub_values(interp: &mut crate::Interpreter, left: Value, right: Value) -> Result<Value> {
    if let (Some(left), Some(right)) = (exact_small_integer(&left), exact_small_integer(&right)) {
        return Ok(match left.checked_sub(right) {
            Some(value) => Value::int(value),
            None => value_from_bigint(PyBigInt::from(left) - PyBigInt::from(right)),
        });
    }
    interp.eval_binary(left, BinaryOp::Sub, right)
}

#[derive(Copy, Clone)]
enum CountComparison {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Rich-compare two count objects and truth-test the result.  Integer counts
/// stay allocation/dispatch-free; all other values retain reflected dunders
/// and arbitrary truth conversion.
#[inline]
fn counter_compare_values(
    interp: &mut crate::Interpreter,
    left: &Value,
    right: &Value,
    op: CountComparison,
) -> Result<bool> {
    if let (Some(left), Some(right)) = (exact_small_integer(left), exact_small_integer(right)) {
        return Ok(match op {
            CountComparison::Eq => left == right,
            CountComparison::Lt => left < right,
            CountComparison::Le => left <= right,
            CountComparison::Gt => left > right,
            CountComparison::Ge => left >= right,
        });
    }
    let binary_op = match op {
        CountComparison::Eq => BinaryOp::Eq,
        CountComparison::Lt => BinaryOp::Lt,
        CountComparison::Le => BinaryOp::Le,
        CountComparison::Gt => BinaryOp::Gt,
        CountComparison::Ge => BinaryOp::Ge,
    };
    let compared = interp.eval_binary(left.clone(), binary_op, right.clone())?;
    interp.truthy_value(&compared)
}

#[inline]
fn counter_positive(interp: &mut crate::Interpreter, value: &Value) -> Result<bool> {
    if let Some(value) = exact_small_integer(value) {
        return Ok(value > 0);
    }
    counter_compare_values(interp, value, &Value::int(0), CountComparison::Gt)
}

#[inline]
fn counter_negative(interp: &mut crate::Interpreter, value: &Value) -> Result<bool> {
    if let Some(value) = exact_small_integer(value) {
        return Ok(value < 0);
    }
    counter_compare_values(interp, value, &Value::int(0), CountComparison::Lt)
}

/// Apply one mapping count.  The operand order is observable:
/// `update` evaluates `incoming + current`, while `subtract` evaluates
/// `current - incoming`.
fn counter_apply_mapping_entry<const SUBTRACT: bool>(
    interp: &mut crate::Interpreter,
    backing: &Value,
    key: PyKey,
    incoming: Value,
    preserve: bool,
) -> Result<()> {
    if preserve {
        return interp.dict_insert_value(backing, key, incoming);
    }
    let key_value = key_to_value(key);
    let lookup_key = interp.value_to_pykey(&key_value)?;
    let current = interp
        .dict_lookup(backing, &lookup_key)?
        .map(|(_, value)| value)
        .unwrap_or_else(|| Value::int(0));
    let result = if SUBTRACT {
        counter_sub_values(interp, current, incoming)?
    } else {
        counter_add_values(interp, incoming, current)?
    };
    // CPython hashes again for the assignment after `get`.
    let insert_key = interp.value_to_pykey(&key_value)?;
    interp.dict_insert_value(backing, insert_key, result)
}

/// Tally one positional source.  Plain dict/Counter inputs are snapshotted so
/// aliased updates are safe; other mappings use the shared streaming mapping
/// adapter; non-mappings remain lazy element streams.
fn counter_tally_into_backing<const SUBTRACT: bool>(
    interp: &mut crate::Interpreter,
    backing: &Value,
    other: &Value,
) -> Result<()> {
    let preserve = !SUBTRACT
        && backing
            .dict_with(|counts| counts.is_empty())
            .ok_or_else(|| {
                PyError::Runtime("internal: Counter backing is not a dict".to_string())
            })?;

    let mapping_entries = other
        .dict_with(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>()
        })
        .or_else(|| counts_of(other).map(|counts| counts.into_iter().collect()));
    if let Some(entries) = mapping_entries {
        for (key, value) in entries {
            counter_apply_mapping_entry::<SUBTRACT>(interp, backing, key, value, preserve)?;
        }
        return Ok(());
    }

    if crate::interpreter::visit_mapping_pairs_via_protocol(interp, other, |interp, key, value| {
        counter_apply_mapping_entry::<SUBTRACT>(interp, backing, key, value, preserve)
    })? {
        return Ok(());
    }

    let iterator = crate::interpreter::make_iterator(interp, other)?;
    loop {
        let value = match interp.call_next(&iterator, None) {
            Ok(value) => value,
            Err(error) if crate::interpreter::is_stop_iteration_error(&error) => break,
            Err(error) => return Err(error),
        };
        let lookup_key = interp.value_to_pykey(&value)?;
        let current = interp
            .dict_lookup(backing, &lookup_key)?
            .map(|(_, value)| value)
            .unwrap_or_else(|| Value::int(0));
        let one = Value::int(1);
        let result = if SUBTRACT {
            counter_sub_values(interp, current, one)?
        } else {
            counter_add_values(interp, current, one)?
        };
        let insert_key = interp.value_to_pykey(&value)?;
        interp.dict_insert_value(backing, insert_key, result)?;
    }
    Ok(())
}

/// Apply keyword counts as a separate mapping update/subtract, including the
/// empty-Counter verbatim-copy rule used by `Counter.update(kwds)`.
fn counter_apply_kwargs_to_backing<const SUBTRACT: bool>(
    interp: &mut crate::Interpreter,
    backing: &Value,
    kwargs: &[&ExpandedCallArg],
) -> Result<()> {
    if kwargs.is_empty() {
        return Ok(());
    }
    let preserve = !SUBTRACT
        && backing
            .dict_with(|counts| counts.is_empty())
            .ok_or_else(|| {
                PyError::Runtime("internal: Counter backing is not a dict".to_string())
            })?;
    for kw in kwargs {
        counter_apply_mapping_entry::<SUBTRACT>(
            interp,
            backing,
            PyKey::str_from(kw.name.as_deref().unwrap_or("")),
            kw.value.clone(),
            preserve,
        )?;
    }
    Ok(())
}

fn key_to_value(key: PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::int(v),
        PyKey::BigInt(v) => Value::bigint(*v),
        PyKey::Float(bits) => Value::float_from_bits(bits),
        PyKey::Str(s) => s,
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::FrozenSet(key) => pyrust_builtins::frozenset::frozenset_key(key),
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
        PyKey::Bytes(rc) => Value::bytes((*rc).clone()),
        PyKey::Complex(re, im) => Value::complex(re, im),
        PyKey::Object { value, .. } => value,
    }
}

// ── Counter algebra ─────────────────────────────────────────────────────────

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
    match borrow.attrs.get(BUILTIN_DATA_ATTR) {
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
    attrs.insert(BUILTIN_DATA_ATTR, Value::dict(counts));
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// Compute one LHS-key algebra result and apply Counter's positive filter.
///
/// Keeping the exact-int comparison in the same branch as the arithmetic
/// avoids constructing a `Value` only to classify it again on the overwhelmingly
/// common integer-count path.
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
/// order.  In particular, RHS-only add/union entries are copied verbatim and
/// RHS-only subtraction evaluates `0 - count` only after `count < 0`.
///
/// `OP` is monomorphized (0 add, 1 subtract, 2 intersection, 3 union), so the
/// plain-integer loop does not re-dispatch the operation for every key.
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

/// Resolve `other.items()` lazily.  In-place Counter operations intentionally
/// accept ordinary dicts and mapping-like objects, as CPython's Python-level
/// implementation does.
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
            value_type_name_str(other)
        ),
    ))
}

fn counter_mapping_getitem(
    interp: &mut crate::Interpreter,
    other: &Value,
    key: &PyKey,
) -> Result<Value> {
    let key_value = key_to_value(key.clone());
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
            value_type_name_str(other)
        ),
    ))
}

/// Match `_keep_positive`: finish all `count > 0` tests first, then delete the
/// collected keys.  A comparison failure therefore performs no cleanup, while
/// arithmetic mutations completed before cleanup remain visible.
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

/// Evaluate `all(self[e] OP other[e] for c in (self, other) for e in c)`.
/// Shared keys are deliberately visited twice because count methods can make
/// that duplication observable.
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
            // Counter.__eq__ returns NotImplemented for non-Counters, after
            // which the plain dict operand compares the actual stored mapping
            // (without missing-is-zero semantics). Route that fallback through
            // the primitive dict comparator explicitly because the RHS sees
            // this native Counter as a PyInstance wrapper.
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
