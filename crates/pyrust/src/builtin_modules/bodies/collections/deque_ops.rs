// `collections.deque` concatenation, repetition, equality, and lexicographic
// ordering operations (both interpreter-level hooks and dunder bodies).

/// Concatenate two deques into a new deque preserving `left`'s class and `maxlen`.
pub(crate) fn deque_concat_values(left: &Value, right: &Value) -> Result<Value> {
    let ValueKind::PyInstance(inst) = left.kind() else {
        return Err(PyError::Runtime(
            "internal: expected deque instance".to_string(),
        ));
    };
    let inst = Rc::clone(inst);
    let other_items = match deque_items_of(right) {
        Some(v) => v,
        None => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "can only concatenate deque (not \"{}\") to deque",
                    crate::interpreter::value_type_name_str(right),
                ),
            ));
        }
    };
    let mut items = deque_items_snapshot(&inst)?;
    items.extend(other_items);
    let maxlen = deque_maxlen(&inst);
    Ok(deque_from_items(&inst, items, maxlen))
}

/// Repeat a deque `n` times into a new deque preserving `target`'s class and `maxlen`.
pub(crate) fn deque_mul_by_i64(target: &Value, n: i64) -> Result<Value> {
    let ValueKind::PyInstance(inst) = target.kind() else {
        return Err(PyError::Runtime(
            "internal: expected deque instance".to_string(),
        ));
    };
    let inst = Rc::clone(inst);
    let maxlen = deque_maxlen(&inst);
    let base = deque_items_snapshot(&inst)?;
    let items = deque_repeat_items(&base, n, maxlen)?;
    Ok(deque_from_items(&inst, items, maxlen))
}

/// Repeat a deque `n` times *in place*.
pub(crate) fn deque_imul_by_i64(target: &Value, n: i64) -> Result<()> {
    let ValueKind::PyInstance(inst) = target.kind() else {
        return Err(PyError::Runtime(
            "internal: expected deque instance".to_string(),
        ));
    };
    let inst = Rc::clone(inst);
    deque_repeat_in_place(&inst, n)
}

fn deque_repeat_items(base: &[Value], n: i64, maxlen: Option<usize>) -> Result<Vec<Value>> {
    let reps = usize::try_from(n.max(0)).unwrap_or(usize::MAX);
    if base.is_empty() || reps == 0 || maxlen == Some(0) {
        return Ok(Vec::new());
    }
    if let Some(limit) = maxlen {
        let total = base.len().saturating_mul(reps);
        let keep = total.min(limit);
        let start = (base.len() - keep % base.len()) % base.len();
        return Ok((0..keep)
            .map(|offset| base[(start + offset) % base.len()].clone())
            .collect());
    }
    let total = base
        .len()
        .checked_mul(reps)
        .ok_or_else(|| PyError::named("MemoryError", String::new()))?;
    let mut items = Vec::new();
    items
        .try_reserve_exact(total)
        .map_err(|_| PyError::named("MemoryError", String::new()))?;
    for _ in 0..reps {
        items.extend(base.iter().cloned());
    }
    Ok(items)
}

fn deque_repeat_in_place(inst: &Rc<RefCell<PyInstance>>, n: i64) -> Result<()> {
    let maxlen = deque_maxlen(inst);
    let reps = usize::try_from(n.max(0)).unwrap_or(usize::MAX);
    let base = deque_items_snapshot(inst)?;
    if reps == 0 || maxlen == Some(0) {
        if !base.is_empty() {
            let data = deque_items_data(inst)?;
            data.borrow_mut().clear();
            deque_bump_state(inst);
        }
        return Ok(());
    }
    if base.is_empty() || reps == 1 {
        return Ok(());
    }
    let items = deque_repeat_items(&base, n, maxlen)?;
    let data = deque_items_data(inst)?;
    *data.borrow_mut() = items.into();
    deque_bump_state(inst);
    Ok(())
}

fn deque_from_items(
    proto: &Rc<RefCell<PyInstance>>,
    mut items: Vec<Value>,
    maxlen: Option<usize>,
) -> Value {
    if let Some(ml) = maxlen
        && items.len() > ml
    {
        let drop = items.len() - ml;
        items.drain(..drop);
    }
    let maxlen_val = match maxlen {
        Some(n) => Value::int(n as i64),
        None => Value::none(),
    };
    let class = Rc::clone(&proto.borrow().class);
    let mut attrs = InstanceAttrs::new();
    attrs.insert_slot(
        "_items",
        pyrust_builtins::deque_storage::deque_storage(items),
    );
    attrs.insert_slot("maxlen", maxlen_val);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

fn deque_resolve_repeat_count(interp: &mut Interpreter, multiplier: &Value) -> Result<i64> {
    let resolved = interp.value_to_index(multiplier, |value| {
        pyrust_core::type_err!(
            "'{}' object cannot be interpreted as an integer",
            pyrust_core::builtin_type_name(value)
        )
    })?;
    let overflow = format!(
        "cannot fit '{}' into an index-sized integer",
        crate::interpreter::error_type_name_str(multiplier)
    );
    interp.value_to_isize(&resolved, &overflow)
}

fn deque_repeat(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let (inst, multiplier) = expect_self_one_arg(args, fn_name)?;
    let n = deque_resolve_repeat_count(interp, multiplier)?;
    let maxlen = deque_maxlen(&inst);
    let base = deque_items_snapshot(&inst)?;
    let items = deque_repeat_items(&base, n, maxlen)?;
    Ok(deque_from_items(&inst, items, maxlen))
}

fn deque_imul(interp: &mut Interpreter, args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let (inst, multiplier) = expect_self_one_arg(args, fn_name)?;
    let n = deque_resolve_repeat_count(interp, multiplier)?;
    deque_repeat_in_place(&inst, n)?;
    Ok(args[0].value.clone())
}

fn deque_eq(interp: &mut Interpreter, args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 {
        return Ok(Value::not_implemented());
    }
    let other = &args[1].value;
    let other_inst = match other.kind() {
        ValueKind::PyInstance(other_inst)
            if is_canonical_collection_class_or_subclass(
                &other_inst.borrow().class,
                CanonicalCollectionKind::Deque,
            ) =>
        {
            Rc::clone(other_inst)
        }
        _ => return Ok(Value::not_implemented()),
    };
    let (self_items, self_state, self_version) = deque_items_guarded_live(&inst)?;
    let (other_items, other_state, other_version) = deque_items_guarded_live(&other_inst)?;
    let len = self_items.borrow().len();
    if len != other_items.borrow().len() {
        return Ok(Value::bool_(false));
    }
    let scalar_result = if interp.has_active_comparison() {
        None
    } else {
        let self_items = self_items.borrow();
        let other_items = other_items.borrow();
        let mut resolved = Some(true);
        for (a, b) in self_items.iter().zip(other_items.iter()) {
            match Interpreter::try_scalar_richcompare_eq(a, b) {
                Some(true) => {}
                Some(false) => {
                    resolved = Some(false);
                    break;
                }
                None => {
                    resolved = None;
                    break;
                }
            }
        }
        resolved
    };
    if let Some(equal) = scalar_result {
        return Ok(Value::bool_(equal));
    }
    interp.with_comparison_pair(&args[0].value, other, |interp| {
        for index in 0..len {
            let a = self_items
                .borrow()
                .get(index)
                .cloned()
                .expect("deque length is guarded during comparison");
            let b = other_items
                .borrow()
                .get(index)
                .cloned()
                .expect("deque length is guarded during comparison");
            if !interp.values_richcompare_eq(&a, &b)? {
                return Ok(Value::bool_(false));
            }
            deque_require_unmutated(&self_state, self_version, "RuntimeError")?;
            deque_require_unmutated(&other_state, other_version, "RuntimeError")?;
        }
        Ok(Value::bool_(true))
    })
}

fn deque_ordering_compare(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    op: BinaryOp,
) -> Result<Value> {
    let method_name = fn_name.rsplit('.').next().unwrap_or(fn_name);
    let Some(receiver) = args.first().filter(|arg| arg.name.is_none()) else {
        return Err(PyError::named(
            "TypeError",
            format!("descriptor '{method_name}' of 'collections.deque' object needs an argument"),
        ));
    };
    let inst = match receiver.value.kind() {
        ValueKind::PyInstance(inst)
            if is_canonical_collection_class_or_subclass(
                &inst.borrow().class,
                CanonicalCollectionKind::Deque,
            ) =>
        {
            Rc::clone(inst)
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor '{method_name}' requires a 'collections.deque' object but received a '{}'",
                    crate::interpreter::value_type_name_str(&receiver.value)
                ),
            ));
        }
    };
    if args.iter().any(|arg| arg.name.is_some()) {
        return Err(PyError::named(
            "TypeError",
            format!("wrapper {method_name}() takes no keyword arguments"),
        ));
    }
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("expected 1 argument, got {}", args.len().saturating_sub(1)),
        ));
    }
    let other_inst = match args[1].value.kind() {
        ValueKind::PyInstance(other_inst)
            if is_canonical_collection_class_or_subclass(
                &other_inst.borrow().class,
                CanonicalCollectionKind::Deque,
            ) =>
        {
            other_inst
        }
        _ => return Ok(Value::not_implemented()),
    };

    let (self_items, self_state, self_version) = deque_items_guarded_live(&inst)?;
    let (other_items, other_state, other_version) = deque_items_guarded_live(other_inst)?;
    let self_len = self_items.borrow().len();
    let other_len = other_items.borrow().len();
    let common_len = self_len.min(other_len);
    interp.with_comparison_pair(&receiver.value, &args[1].value, |interp| {
        for index in 0..common_len {
            let left = self_items
                .borrow()
                .get(index)
                .cloned()
                .expect("deque length is guarded during comparison");
            let right = other_items
                .borrow()
                .get(index)
                .cloned()
                .expect("deque length is guarded during comparison");
            if !interp.values_richcompare_eq(&left, &right)? {
                let compared = interp.eval_binary(left, op, right)?;
                return Ok(Value::bool_(interp.truthy_value(&compared)?));
            }
            deque_require_unmutated(&self_state, self_version, "RuntimeError")?;
            deque_require_unmutated(&other_state, other_version, "RuntimeError")?;
        }

        let ordered = match op {
            BinaryOp::Lt => self_len < other_len,
            BinaryOp::Le => self_len <= other_len,
            BinaryOp::Gt => self_len > other_len,
            BinaryOp::Ge => self_len >= other_len,
            _ => unreachable!("deque ordering helper received a non-ordering operation"),
        };
        Ok(Value::bool_(ordered))
    })
}
