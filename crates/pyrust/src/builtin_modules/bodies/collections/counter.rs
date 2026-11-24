// Counter class method implementations, tallying, and count arithmetic
// primitives. Multiset binary/unary/in-place algebra and comparisons live in
// `counter_algebra.rs`.

fn counter_repeat_entry(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    if args.len() != 1 || args[0].name.is_some() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly one positional argument"),
        ));
    }
    let (element, count) = match args[0].value.kind() {
        ValueKind::Tuple(items) if items.len() == 2 => (items[0].clone(), items[1].clone()),
        _ => {
            return Err(PyError::Runtime(
                "internal: Counter.items() yielded a non-pair".to_string(),
            ));
        }
    };
    let count = interp.value_to_isize(&count, "Python int too large to convert to C ssize_t")?;
    Ok(native_iterators::repeat(element, count.max(0) as usize))
}

fn counter_fromkeys(args: &[ExpandedCallArg]) -> Result<Value> {
    // The classmethod descriptor prepends `cls`.
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            "Counter.fromkeys() missing 1 required positional argument: 'iterable'",
        ));
    }
    if args.len() > 3 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "Counter.fromkeys() takes from 2 to 3 positional arguments but {} were given",
                args.len()
            ),
        ));
    }
    Err(PyError::named(
        "NotImplementedError",
        "Counter.fromkeys() is undefined.  Use Counter(iterable) instead.",
    ))
}

fn counter_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    apply_delta::<false>(interp, args, fn_name)
}

fn counter_getitem(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let backing = counter_backing(args, fn_name)?;
    let key = require_key_arg(args, 1, fn_name)?;
    Ok(interp
        .dict_lookup_value(&backing, key)?
        .map(|(_, value)| value)
        .unwrap_or_else(|| Value::int(0)))
}

fn counter_delitem(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    expect_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let key = require_key_arg(args, 1, fn_name)?;
    let backing = counter_backing(args, fn_name)?;
    if let Some((index, _)) = interp.dict_lookup_value(&backing, key)? {
        backing
            .dict_with_mut(|counts| {
                counts.shift_remove_index(index);
            })
            .ok_or_else(|| {
                PyError::Runtime("internal: Counter backing is not a dict".to_string())
            })?;
    }
    Ok(Value::none())
}

fn counter_repr(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let backing = counter_backing(args, fn_name)?;
    let pairs: Vec<(PyKey, Value)> = backing
        .dict_with(|counts| {
            counts
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .ok_or_else(|| PyError::Runtime("internal: Counter backing is not a dict".to_string()))?;
    if pairs.is_empty() {
        return Ok(Value::string(format!(
            "{}()",
            inst.borrow().class.borrow().name
        )));
    }
    let ordered = match most_common::select(interp, pairs.clone(), None) {
        Ok(ordered) => ordered,
        Err(error) if error.class_name_is("TypeError") => pairs,
        Err(error) => return Err(error),
    };
    let inner: Vec<String> = ordered
        .iter()
        .map(|(k, v)| format!("{}: {}", key_repr(k), v.repr_raw()))
        .collect();
    Ok(Value::string(format!(
        "{}({{{}}})",
        inst.borrow().class.borrow().name,
        inner.join(", ")
    )))
}

fn counter_most_common(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let user = &args[1..];
    if user.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            "most_common() takes at most 1 argument".to_string(),
        ));
    }
    let backing = counter_backing(args, fn_name)?;
    let pairs: Vec<(PyKey, Value)> = backing
        .dict_with(|counts| {
            counts
                .iter()
                .map(|(key, count)| (key.clone(), count.clone()))
                .collect()
        })
        .ok_or_else(|| PyError::Runtime("internal: Counter backing is not a dict".to_string()))?;
    let selected =
        most_common::select(interp, pairs, user.first().map(|argument| &argument.value))?;
    Ok(Value::list(
        selected
            .into_iter()
            .map(|(key, count)| Value::tuple(vec![crate::interpreter::key_to_value(key), count]))
            .collect(),
    ))
}

fn counter_elements(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let user = &args[1..];
    if !user.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "elements() takes no arguments".to_string(),
        ));
    }
    let counter_class = Rc::clone(&expect_self(args, fn_name)?.borrow().class);
    let chain_class = counter_elements_chain_class(&counter_class);
    let backing = counter_backing(args, fn_name)?;
    let items_view = Interpreter::dict_view_for_backing(&backing, "items", false)?;
    let items_iter = crate::interpreter::make_iterator(interp, &items_view)?;
    let mut sources = IterSrcBuf::new();
    sources.push(items_iter);
    let repeaters = Value::generator(Box::new(MapIter {
        func: Value::builtin_function("collections._counter_repeat_entry"),
        sources,
        done: false,
    }));
    Ok(native_iterators::elements(chain_class, repeaters))
}

fn counter_total(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, "total")?;
    let backing = counter_backing(args, fn_name)?;
    let values = Interpreter::dict_view_for_backing(&backing, "values", false)?;
    interp.call_function_expanded(
        Value::builtin_function("sum"),
        &[ExpandedCallArg {
            name: None,
            value: values,
        }],
    )
}

fn counter_copy(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let counts = snapshot_counts(args, fn_name)?;
    let user = &args[1..];
    if !user.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "copy() takes no arguments".to_string(),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let class = Rc::clone(&inst.borrow().class);
    let mut attrs = InstanceAttrs::new();
    attrs.set_builtin_data(Value::dict(counts));
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// Apply `update`/`subtract` semantics to an optional positional source and
/// keyword counts.
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

/// Apply one mapping count (`update`: `incoming + current`, `subtract`: `current - incoming`).
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
    let key_value = crate::interpreter::key_to_value(key);
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

/// Tally one positional source into a Counter's backing dict.
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

    let exposes_getitem = match other.kind() {
        ValueKind::PyInstance(instance) => {
            let class = Rc::clone(&instance.borrow().class);
            lookup_class_attr(&class, "__getitem__").is_some()
        }
        _ => false,
    };
    if exposes_getitem
        && crate::interpreter::visit_mapping_pairs_via_protocol(
            interp,
            other,
            |interp, key, value| {
                counter_apply_mapping_entry::<SUBTRACT>(interp, backing, key, value, preserve)
            },
        )?
    {
        return Ok(());
    }

    let iterator = crate::interpreter::make_iterator(interp, other)?;
    loop {
        let value = match interp.call_next(&iterator, None) {
            Ok(value) => value,
            Err(error) if crate::interpreter::is_stop_iteration_error(&error) => break,
            Err(error) => return Err(error),
        };
        let current = interp
            .dict_lookup_value(backing, &value)?
            .map(|(_, value)| value)
            .unwrap_or_else(|| Value::int(0));
        let one = Value::int(1);
        let result = if SUBTRACT {
            counter_sub_values(interp, current, one)?
        } else {
            counter_add_values(interp, current, one)?
        };
        interp.dict_set_value(backing, value, result)?;
    }
    Ok(())
}

/// Apply keyword counts as a separate mapping update/subtract.
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
