// deque receiver guards, storage access, mutation checks, and sequence
// operations.
//
// All helpers here operate on deque's opaque native storage; module assembly
// and Python-visible method declarations remain in the parent file.

/// Arity guard for a deque method that takes no positional arguments
/// (`pop`, `popleft`, `clear`, `copy`, `reverse`).  The argument-count
/// check runs *before* `expect_self`, so a wrong-arity call wins over a
/// bad-self call — matching the original open-coded order.
fn expect_self_no_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    expect_self(args, fn_name)
}

/// Arity guard for a deque method that takes exactly one positional
/// argument.  `expect_self` runs *before* the argument-count check, so a
/// bad-self call wins over a wrong-arity call — matching the original
/// open-coded order.  Returns both the receiver and a borrow of the lone
/// argument value.
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

/// Bump the deque's mutation-state counter (#1994).  Called by every structural
/// mutation so the iterator's snapshotted state diverges and `__next__` raises
/// `RuntimeError: deque mutated during iteration`.
fn deque_bump_state(inst: &Rc<RefCell<PyInstance>>) {
    let storage = inst.borrow().attrs.get("_items").cloned();
    if let Some(storage) = storage {
        pyrust_builtins::deque_storage::bump_mutation_state(&storage);
    }
}

/// Extend a deque one source element at a time.
///
/// General iterators may run Python code or fail part-way through; CPython
/// keeps every element appended before that failure. Streaming also prevents a
/// bounded deque from allocating an unbounded input snapshot. Plain
/// list/tuple inputs and direct self-extension use a safe snapshot fast path:
/// they cannot fail while being read, and self-extension must duplicate the
/// original contents instead of tripping the deque's own mutation guard.
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
            Some(deque_items_snapshot(inst)?)
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
        // maxlen=0 still consumes the source and propagates its errors, but
        // deliberately performs no structural mutation.
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

/// Return the deque's opaque storage object, installing an empty one for a raw
/// instance whose `__init__` has not run yet.
fn deque_storage_value(inst: &Rc<RefCell<PyInstance>>) -> Result<Value> {
    {
        let borrow = inst.borrow();
        if let Some(value) = borrow.attrs.get("_items") {
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
    inst.borrow_mut().attrs.insert("_items", storage.clone());
    Ok(storage)
}

/// Return the deque's shared `VecDeque` buffer.
fn deque_items_data(
    inst: &Rc<RefCell<PyInstance>>,
) -> Result<pyrust_builtins::deque_storage::DequeData> {
    let storage = deque_storage_value(inst)?;
    pyrust_builtins::deque_storage::data(&storage)
        .ok_or_else(|| PyError::Runtime("internal: deque storage lost its buffer".to_string()))
}

/// Build either concrete deque iterator frame over one shared storage generation.
fn deque_iterator_frame(
    inst: &Rc<RefCell<PyInstance>>,
    reverse: bool,
    consumed: usize,
) -> Result<NativeIterFrame> {
    let storage = deque_storage_value(inst)?;
    let items = pyrust_builtins::deque_storage::data(&storage)
        .ok_or_else(|| PyError::Runtime("internal: deque storage lost its buffer".to_string()))?;
    let counter = pyrust_builtins::deque_storage::mutation_state(&storage).ok_or_else(|| {
        PyError::Runtime("internal: deque storage lost its mutation state".to_string())
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

/// Resolve the data and mutation generation owned by a replacement deque.
/// Iterator deepcopy uses this typed provider boundary to re-seat every native
/// reference together, without teaching the generic copy module about deque.
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
    let storage = inst.borrow().attrs.get("_items").cloned()?;
    let data = pyrust_builtins::deque_storage::data(&storage)?;
    let counter = pyrust_builtins::deque_storage::mutation_state(&storage)?;
    Some((data, counter))
}

/// Snapshot the deque's items as a `Vec<Value>` for read-only work.
fn deque_items_snapshot(inst: &Rc<RefCell<PyInstance>>) -> Result<Vec<Value>> {
    Ok(deque_items_data(inst)?.borrow().iter().cloned().collect())
}

/// Snapshot the deque together with the opaque structural-mutation version
/// that was current when the snapshot was taken.
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
            PyError::Runtime("internal: deque storage lost its mutation state".to_string())
        })?;
    let version = mutation_state.get();
    let items = pyrust_builtins::deque_storage::snapshot(&storage)
        .ok_or_else(|| PyError::Runtime("internal: deque storage lost its buffer".to_string()))?;
    Ok((items, mutation_state, version))
}

/// Borrowable live deque storage plus the structural-mutation version at the
/// start of a callback-capable equality comparison.
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
            PyError::Runtime("internal: deque storage lost its mutation state".to_string())
        })?;
    let version = mutation_state.get();
    let items = pyrust_builtins::deque_storage::data(&storage)
        .ok_or_else(|| PyError::Runtime("internal: deque storage lost its buffer".to_string()))?;
    Ok((items, mutation_state, version))
}

/// User equality may run arbitrary Python and mutate the deque.  CPython
/// checks its opaque state after every successful comparison instead of
/// continuing over stale elements.
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

/// Shared lexicographic body for deque's four ordering comparisons.
///
/// CPython probes each common-prefix pair with identity-then-equality, checks
/// both deque mutation states after every successful equality, and applies the
/// requested ordering operation to the first unequal pair.  Length decides
/// only when one deque is a proper prefix of the other.  A non-deque operand
/// returns `NotImplemented` so normal reflected comparison dispatch and error
/// reporting remain in charge.
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

/// Read the maxlen from `self.maxlen` — returns `None` for unbounded.
fn deque_maxlen(inst: &Rc<RefCell<PyInstance>>) -> Option<usize> {
    let borrow = inst.borrow();
    match borrow.attrs.get("maxlen").map(|v| v.kind()) {
        Some(ValueKind::Int(n)) if n >= 0 => Some(n as usize),
        Some(ValueKind::Bool(b)) => Some(b as usize),
        _ => None,
    }
}

/// Extract the `_items` storage snapshot from a Value that is a deque instance.
/// Returns `None` if the Value is not a deque `PyInstance`.  Used by `__eq__`.
fn deque_items_of(value: &Value) -> Option<Vec<Value>> {
    let ValueKind::PyInstance(inst) = value.kind() else {
        return None;
    };
    let borrow = inst.borrow();
    if !is_canonical_collection_class_or_subclass(&borrow.class, CanonicalCollectionKind::Deque) {
        return None;
    }
    match borrow.attrs.get("_items") {
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

/// Resolve a Python index (possibly negative) into a `usize` for a deque of
/// length `len`. Raises `IndexError` if out of range.
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

/// Build a new deque `PyInstance` (same class as `proto`) holding `items`,
/// with the given `maxlen`.  If `maxlen` is set and `items` is longer, the
/// rightmost `maxlen` elements are kept — matching CPython's `+`/`*` result
/// trimming (#2011).
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
    attrs.insert(
        "_items",
        pyrust_builtins::deque_storage::deque_storage(items),
    );
    attrs.insert("maxlen", maxlen_val);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Shared body for deque `__mul__` / `__rmul__` (#2011).  Repeats the deque
/// `n` times into a new deque; `n <= 0` yields empty.  The result inherits
/// `self`'s maxlen (trimmed).  A non-int multiplier raises `TypeError`.
fn deque_repeat(
    _interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let multiplier = &args[1].value;
    let resolved = _interp.value_to_index(multiplier, |value| {
        pyrust_core::type_err!(
            "can't multiply sequence by non-int of type '{}'",
            pyrust_core::error_type_name(value)
        )
    })?;
    let overflow = format!(
        "cannot fit '{}' into an index-sized integer",
        value_type_name_str(multiplier)
    );
    let n = _interp.value_to_isize(&resolved, &overflow)?;
    let base = deque_items_snapshot(&inst)?;
    let reps = usize::try_from(n.max(0)).unwrap_or(usize::MAX);
    let maxlen = deque_maxlen(&inst);
    let items = if base.is_empty() || reps == 0 || maxlen == Some(0) {
        Vec::new()
    } else if let Some(limit) = maxlen {
        // A bounded result retains only the conceptual repeated sequence's
        // rightmost `limit` elements.  Construct exactly that suffix instead
        // of allocating `base.len() * reps` and trimming it afterwards.
        let total = base.len().saturating_mul(reps);
        let keep = total.min(limit);
        let start = (base.len() - keep % base.len()) % base.len();
        (0..keep)
            .map(|offset| base[(start + offset) % base.len()].clone())
            .collect()
    } else {
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
        items
    };
    Ok(deque_from_items(&inst, items, maxlen))
}
