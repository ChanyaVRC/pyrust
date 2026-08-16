/// Copying a built-in iterator object (#2974).
///
/// CPython has no generic iterator copy: `copy.copy` runs the object through
/// `__reduce__`, and every iterator type spells its own reduction. Two shapes
/// cover the built-ins:
///
/// * **Sequence-shaped** — `(iter, (sequence,), index)`. The copy retains the
///   *same* sequence and resumes at the same index, so a later mutation of that
///   sequence is observed by both cursors.
/// * **Cursor-shaped** — `(iter, ([remaining, …],))`. A dict or set cursor
///   cannot be resumed by index, so CPython drains what is left into a plain
///   `list` and reduces to a *`list_iterator` over that list*. The copy has
///   therefore left the mapping entirely: later mutations of the container are
///   invisible to it and can never raise `RuntimeError` at it, while the
///   original keeps its live guard.
///
/// Generators reduce to nothing at all — `copy.copy` of one raises
/// `TypeError: cannot pickle 'generator' object`.
///
/// This domain owns every built-in cursor representation, so it owns the
/// reduce-equivalent state; the `copy` module owns the recursion, the memo, and
/// the Python-visible errors and consumes only [`IteratorCopy`].
pub(crate) enum IteratorCopy {
    /// Not an iterator whose copy this domain defines. The `copy` module keeps
    /// its own rules for the value.
    Unowned,
    /// CPython's copy protocol refuses the object outright. Carries the type
    /// noun for `cannot pickle '<noun>' object`.
    Unpicklable(&'static str),
    /// An independent iterator resuming from the same reduce state.
    Rebuilt(Value),
    /// A live bytearray iterator's `(iter, (carrier,), position)` reduction.
    ///
    /// The copy module must execute this reduction because `carrier` may be a
    /// subclass whose `__iter__` changes the reconstructed iterator type, and
    /// `deepcopy` must recurse into the carrier before memoising the outer
    /// iterator (the ordering CPython's `_reconstruct` uses for cycles).
    BytearrayReduction { carrier: Value, position: usize },
    /// A legacy sequence cursor's or real `reversed` subclass's
    /// `(constructor, (owner,)[, position])` reduction. `constructor` is
    /// `iter`, `reversed`, or the concrete `reversed` subclass that owns the
    /// visible cursor.
    ///
    /// The copy module executes this reduction instead of cloning the frame:
    /// deepcopy must recurse into `owner` before constructing and memoising
    /// the outer iterator, exactly as `copy._reconstruct` does.
    GetItemReduction {
        constructor: Value,
        owner: Value,
        position: Option<i64>,
    },
}

/// Typed identity of one of issue #2934's concrete native iterator frames.
pub(crate) fn native_iterator_class(value: &Value) -> Option<NativeIteratorClass> {
    let ValueKind::Generator(state) = value.kind() else {
        return None;
    };
    let borrow = state.try_borrow().ok()?;
    borrow
        .downcast_ref::<NativeIterFrame>()
        .and_then(|frame| frame.class)
}

/// Resolve the storage cell and Python-visible reconstruction callable for a
/// legacy `__getitem__` cursor. A `reversed` subclass owns a generic reversed
/// backing in `__builtin_data__`; its class, rather than the exact built-in,
/// must be the reducer so copy/pickle preserve the subclass.
fn getitem_iterator_cell(value: &Value) -> Option<(Rc<GeneratorCell>, Value)> {
    match value.kind() {
        ValueKind::Generator(state) => {
            let state = Rc::clone(state);
            let reverse = state.try_borrow().ok()?.downcast_ref::<GetItemIter>()?.step < 0;
            let constructor = if reverse {
                Value::py_class(BuiltinTypeClass::Reversed.singleton())
            } else {
                Value::builtin_function("iter")
            };
            Some((state, constructor))
        }
        ValueKind::PyInstance(instance) => {
            let class = Rc::clone(&instance.borrow().class);
            if !class_is_subclass_of(&class, &BuiltinTypeClass::Reversed.singleton()) {
                return None;
            }
            let backing = instance_builtin_data(instance)?;
            let ValueKind::Generator(state) = backing.kind() else {
                return None;
            };
            let state = Rc::clone(state);
            let is_reverse_getitem = state
                .try_borrow()
                .ok()?
                .downcast_ref::<GetItemIter>()
                .is_some_and(|iterator| iterator.step < 0);
            is_reverse_getitem.then(|| (state, Value::py_class(class)))
        }
        _ => None,
    }
}

/// Whether `value` is an exact legacy cursor or a `reversed` subclass carrying
/// one. Kept as a typed probe so attribute/copy dispatch never relies on the
/// public type-name string.
pub(crate) fn is_getitem_iterator(value: &Value) -> bool {
    getitem_iterator_cell(value).is_some()
}

/// Resolve a genuine instance of the public `reversed` class to its backing
/// and reconstruction callable. Generic native reverse cursors carry
/// `NativeIteratorClass::Reversed`; specialised reverse frames such as
/// `list_reverseiterator` deliberately do not. A Python subclass keeps its
/// concrete class as the callable while storing either backing shape in
/// `__builtin_data__`.
fn reversed_iterator_backing(value: &Value) -> Option<(Value, Value)> {
    let backing_is_reversed = |backing: &Value| {
        let ValueKind::Generator(state) = backing.kind() else {
            return false;
        };
        state.try_borrow().is_ok_and(|borrow| {
            borrow
                .downcast_ref::<GetItemIter>()
                .is_some_and(|iterator| iterator.step < 0)
                || borrow
                    .downcast_ref::<NativeIterFrame>()
                    .is_some_and(|frame| {
                        frame.class == Some(NativeIteratorClass::Reversed)
                            && matches!(
                                &frame.source,
                                NativeIterSource::ReverseIndexed { .. }
                                    | NativeIterSource::Exhausted { .. }
                            )
                    })
        })
    };

    match value.kind() {
        ValueKind::Generator(_) if backing_is_reversed(value) => Some((
            value.clone(),
            Value::py_class(BuiltinTypeClass::Reversed.singleton()),
        )),
        ValueKind::PyInstance(instance) => {
            let class = Rc::clone(&instance.borrow().class);
            if !class_is_subclass_of(&class, &BuiltinTypeClass::Reversed.singleton()) {
                return None;
            }
            let backing = instance_builtin_data(instance)?;
            backing_is_reversed(&backing).then(|| (backing, Value::py_class(class)))
        }
        _ => None,
    }
}

/// Whether `value` is an exact or subclass instance of the real `reversed`
/// class. This is typed cursor/class provenance, not a public type-name test.
pub(crate) fn is_reversed_iterator(value: &Value) -> bool {
    reversed_iterator_backing(value).is_some()
}

fn getitem_iterator_reduction(value: &Value) -> Result<Option<(Value, Value, Option<i64>)>> {
    let Some((state, constructor)) = getitem_iterator_cell(value) else {
        return Ok(None);
    };
    let borrow = state
        .try_borrow()
        .map_err(|_| pyrust_core::value_err!("generator already executing"))?;
    let iterator = borrow
        .downcast_ref::<GetItemIter>()
        .ok_or_else(|| PyError::Runtime("legacy iterator lost its state".to_string()))?;
    if iterator.exhausted {
        return Ok(Some((constructor, Value::tuple(Vec::new()), None)));
    }
    Ok(Some((
        constructor,
        iterator.obj.clone(),
        Some(iterator.index),
    )))
}

/// Python-visible reduction tuple for a legacy forward/reverse sequence
/// iterator. Observed exhaustion releases the source and therefore omits the
/// state; a cursor that merely yielded its final element remains live until a
/// following `next()` observes exhaustion.
pub(crate) fn getitem_iterator_reduce(value: &Value) -> Result<Value> {
    let Some((constructor, owner, position)) = getitem_iterator_reduction(value)? else {
        return Err(PyError::Runtime(
            "legacy iterator lost its state".to_string(),
        ));
    };
    let mut reduction = vec![constructor, Value::tuple(vec![owner])];
    if let Some(position) = position {
        reduction.push(Value::int(position));
    }
    Ok(Value::tuple(reduction))
}

fn reversed_iterator_reduction(value: &Value) -> Result<Option<(Value, Value, Option<i64>)>> {
    let Some((backing, constructor)) = reversed_iterator_backing(value) else {
        return Ok(None);
    };
    let ValueKind::Generator(state) = backing.kind() else {
        return Err(PyError::Runtime(
            "reversed iterator lost its state".to_string(),
        ));
    };
    let borrow = state
        .try_borrow()
        .map_err(|_| pyrust_core::value_err!("generator already executing"))?;
    if let Some(iterator) = borrow.downcast_ref::<GetItemIter>() {
        if iterator.step >= 0 {
            return Err(PyError::Runtime(
                "reversed iterator lost its state".to_string(),
            ));
        }
        return Ok(Some(if iterator.exhausted {
            (constructor, Value::tuple(Vec::new()), None)
        } else {
            (constructor, iterator.obj.clone(), Some(iterator.index))
        }));
    }
    let frame = borrow
        .downcast_ref::<NativeIterFrame>()
        .filter(|frame| frame.class == Some(NativeIteratorClass::Reversed))
        .ok_or_else(|| PyError::Runtime("reversed iterator lost its state".to_string()))?;
    match &frame.source {
        NativeIterSource::ReverseIndexed { value, next_index } if !frame.exhausted => {
            let position = i64::try_from(*next_index)
                .unwrap_or(i64::MAX)
                .saturating_sub(1);
            Ok(Some((constructor, value.clone(), Some(position))))
        }
        NativeIterSource::Exhausted { .. } if frame.exhausted => {
            Ok(Some((constructor, Value::tuple(Vec::new()), None)))
        }
        _ => Err(PyError::Runtime(
            "reversed iterator lost its source".to_string(),
        )),
    }
}

/// Python-visible reduction tuple for any genuine `reversed` instance.
///
/// Legacy user-sequence cursors and their subclasses retain the callable
/// slots captured at construction. Optimised tuple/str/bytes/bytearray
/// cursors retain their typed native owner and descending `next_index`.
/// Observed exhaustion releases either owner and reduces through an empty
/// tuple, while a cursor that only yielded its last value still reports `-1`.
pub(crate) fn reversed_iterator_reduce(value: &Value) -> Result<Value> {
    let Some((constructor, owner, position)) = reversed_iterator_reduction(value)? else {
        return Err(PyError::Runtime(
            "reversed iterator lost its state".to_string(),
        ));
    };
    let mut reduction = vec![constructor, Value::tuple(vec![owner])];
    if let Some(position) = position {
        reduction.push(Value::int(position));
    }
    Ok(Value::tuple(reduction))
}

/// Deque iterators retain their creation-time remaining quota even if the
/// live deque shrinks before the mutation latch is observed. CPython exposes
/// the resulting signed `live_len - remaining` index through `__reduce__`;
/// reconstruction later clamps a negative index to zero.
#[inline]
fn deque_reduce_index(live_len: usize, remaining: usize, failed: bool) -> i128 {
    if failed {
        live_len as i128
    } else {
        live_len as i128 - remaining as i128
    }
}

/// Positional arguments after `self` accepted by the object methods inherited
/// by concrete native iterators. Their GeneratorCell carrier must not bypass
/// the descriptors' ordinary arity checks.
pub(crate) fn native_iterator_object_method_arity(method: &str) -> Option<usize> {
    match method {
        "__sizeof__" | "__dir__" | "__repr__" | "__str__" | "__hash__" => Some(0),
        "__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__" | "__format__"
        | "__getattribute__" => Some(1),
        _ => None,
    }
}

/// Python-visible `__reduce__` value for a concrete native iterator.
pub(crate) fn native_iterator_reduce(
    value: &Value,
    expected: NativeIteratorClass,
) -> Result<Value> {
    if expected == NativeIteratorClass::Reversed {
        return reversed_iterator_reduce(value);
    }
    let ValueKind::Generator(state) = value.kind() else {
        return Err(PyError::Runtime(
            "native iterator lost its state".to_string(),
        ));
    };
    let borrow = state.borrow();
    let frame = borrow
        .downcast_ref::<NativeIterFrame>()
        .filter(|frame| frame.class == Some(expected))
        .ok_or_else(|| PyError::Runtime("native iterator class mismatch".to_string()))?;

    match expected {
        NativeIteratorClass::Bytearray => match &frame.source {
            NativeIterSource::Bytearray { carrier, .. } => Ok(Value::tuple(vec![
                Value::builtin_function("iter"),
                Value::tuple(vec![carrier.clone()]),
                Value::int(frame.pos as i64),
            ])),
            NativeIterSource::Exhausted { .. } => Ok(Value::tuple(vec![
                Value::builtin_function("iter"),
                Value::tuple(vec![Value::tuple(Vec::new())]),
            ])),
            _ => Err(PyError::Runtime(
                "bytearray iterator lost its source".to_string(),
            )),
        },
        NativeIteratorClass::Deque | NativeIteratorClass::DequeReverse => {
            let NativeIterSource::Deque {
                data, remaining, ..
            } = &frame.source
            else {
                return Err(PyError::Runtime(
                    "deque iterator lost its source".to_string(),
                ));
            };
            let live_len = data.borrow().len();
            let failed = frame.guard.as_ref().is_some_and(|guard| guard.failed);
            let reduce_index = deque_reduce_index(live_len, *remaining, failed);
            let owner = frame
                .guard
                .as_ref()
                .map(|guard| guard.container.clone())
                .ok_or_else(|| PyError::Runtime("deque iterator lost its owner".to_string()))?;
            Ok(Value::tuple(vec![
                Value::py_class(expected.singleton()),
                Value::tuple(vec![owner, value_from_bigint(PyBigInt::from(reduce_index))]),
            ]))
        }
        NativeIteratorClass::Reversed => unreachable!("handled before borrowing native state"),
    }
}

/// Restore the position carried in a bytearray iterator reduction.
///
/// Once the iterator has observed exhaustion, CPython has released its source
/// and `__setstate__` cannot resurrect it. A still-live source clamps the
/// requested position to the buffer's current length.
pub(crate) fn native_bytearray_iterator_setstate(value: &Value, position: usize) -> Result<Value> {
    let ValueKind::Generator(state) = value.kind() else {
        return Err(PyError::Runtime(
            "native iterator lost its state".to_string(),
        ));
    };
    let mut borrow = state.borrow_mut();
    let frame = borrow
        .downcast_mut::<NativeIterFrame>()
        .filter(|frame| frame.class == Some(NativeIteratorClass::Bytearray))
        .ok_or_else(|| PyError::Runtime("native iterator class mismatch".to_string()))?;
    if let NativeIterSource::Bytearray { data, .. } = &frame.source {
        frame.pos = position.min(data.borrow().len());
    }
    Ok(Value::none())
}

/// Restore a legacy sequence iterator's signed `Py_ssize_t` cursor state.
/// Forward cursors clamp only at zero. Reverse cursors re-read the live length
/// and clamp into `[-1, len - 1]`, matching CPython's `reversed_setstate`.
pub(crate) fn getitem_iterator_setstate(
    interp: &mut Interpreter,
    value: &Value,
    position: i64,
) -> Result<Value> {
    let Some((state, _)) = getitem_iterator_cell(value) else {
        return Err(PyError::Runtime(
            "legacy iterator lost its state".to_string(),
        ));
    };
    let (object, reverse, exhausted) = {
        let borrow = state.borrow();
        let iterator = borrow
            .downcast_ref::<GetItemIter>()
            .ok_or_else(|| PyError::Runtime("legacy iterator lost its state".to_string()))?;
        (iterator.obj.clone(), iterator.step < 0, iterator.exhausted)
    };
    if exhausted {
        return Ok(Value::none());
    }
    if !reverse {
        let mut borrow = state.borrow_mut();
        let iterator = borrow
            .downcast_mut::<GetItemIter>()
            .ok_or_else(|| PyError::Runtime("legacy iterator lost its state".to_string()))?;
        if !iterator.exhausted {
            iterator.index = position.max(0);
            // Forward cursors normally keep `remaining` empty. After an
            // explicit state restore, reuse it as a countdown to
            // PY_SSIZE_T_MAX so overflow is detected in the pre-existing
            // terminal-count branch rather than on every normal `next()`.
            iterator.remaining = usize::try_from(i64::MAX - iterator.index).ok();
        }
        return Ok(Value::none());
    }

    // Release the iterator state before calling user `__len__` code.
    let length_method = lookup_value_special_method(&object, "__len__")
        .transpose()?
        .ok_or_else(|| {
            pyrust_core::type_err!(
                "object of type '{}' has no len()",
                full_type_name_str(&object)
            )
        })?;
    let length_value = invoke_class_method(interp, length_method, object, &[])?;
    let length = interp.normalize_len_result(&length_value)?;

    let mut borrow = state.borrow_mut();
    let iterator = borrow
        .downcast_mut::<GetItemIter>()
        .ok_or_else(|| PyError::Runtime("legacy iterator lost its state".to_string()))?;
    if !iterator.exhausted {
        let maximum = length.saturating_sub(1);
        iterator.index = position.clamp(-1, maximum);
        iterator.remaining =
            Some(usize::try_from(iterator.index.saturating_add(1)).unwrap_or(usize::MAX));
    }
    Ok(Value::none())
}

/// Restore the signed cursor state of any genuine `reversed` instance.
/// Exact native cursors and subclass backings share the typed
/// `NativeIteratorClass::Reversed` provenance; observed exhaustion remains a
/// no-op and cannot reacquire the released owner.
pub(crate) fn reversed_iterator_setstate(
    interp: &mut Interpreter,
    value: &Value,
    position: i64,
) -> Result<Value> {
    let Some((backing, _)) = reversed_iterator_backing(value) else {
        return Err(PyError::Runtime(
            "reversed iterator lost its state".to_string(),
        ));
    };
    let ValueKind::Generator(state) = backing.kind() else {
        return Err(PyError::Runtime(
            "reversed iterator lost its state".to_string(),
        ));
    };
    let (is_getitem, exhausted) = {
        let borrow = state
            .try_borrow()
            .map_err(|_| pyrust_core::value_err!("generator already executing"))?;
        if let Some(iterator) = borrow.downcast_ref::<GetItemIter>() {
            (true, iterator.exhausted)
        } else {
            let frame = borrow
                .downcast_ref::<NativeIterFrame>()
                .filter(|frame| frame.class == Some(NativeIteratorClass::Reversed))
                .ok_or_else(|| PyError::Runtime("reversed iterator lost its state".to_string()))?;
            (false, frame.exhausted)
        }
    };
    if exhausted {
        return Ok(Value::none());
    }
    if is_getitem {
        return getitem_iterator_setstate(interp, value, position);
    }
    if restore_reduced_iterator_position(&backing, position)? {
        Ok(Value::none())
    } else {
        Err(PyError::Runtime(
            "reversed iterator lost its source".to_string(),
        ))
    }
}

/// Apply the integer state from a sequence-shaped iterator reduction.
///
/// The bytearray copy protocol may reconstruct to a different native sequence
/// iterator when a subclass overrides `__iter__` (most commonly a
/// `list_iterator`). Reducible native sequence cursors clamp `__setstate__` to
/// their live source length, so keep that typed operation beside the frame
/// state. A generator-policy frame may use the same materialized storage but
/// must still follow the generic copy reconstruction failure path.
pub(crate) fn restore_native_iterator_position(value: &Value, position: usize) -> Result<bool> {
    let ValueKind::Generator(state) = value.kind() else {
        return Ok(false);
    };
    let Ok(mut borrow) = state.try_borrow_mut() else {
        return Ok(false);
    };
    if let Some(frame) = borrow.downcast_mut::<NativeIterFrame>() {
        if matches!(frame.copy_policy, NativeIterCopyPolicy::Generator) {
            return Ok(false);
        }
        let restored = match &mut frame.source {
            NativeIterSource::Materialized(items) => Some(position.min(items.len())),
            NativeIterSource::Indexed(value) => Some(position.min(match value.kind() {
                ValueKind::List(items) => items.len(),
                ValueKind::Tuple(items) => items.len(),
                _ => 0,
            })),
            NativeIterSource::ReverseIndexed { value, next_index } => {
                // `reversed.__setstate__` stores the next *forward* index,
                // not a consumed-item count: state 2 resumes with index 2,
                // then 1, then 0. Restore against the full live source rather
                // than the old cursor so a pre-advanced replacement can rewind.
                let live_len = reverse_source_live_len(value).unwrap_or(0);
                let restored_next = position.saturating_add(1).min(live_len);
                *next_index = restored_next;
                let consumed = live_len - restored_next;
                Some(consumed)
            }
            NativeIterSource::Bytes(value) => Some(position.min(match value.kind() {
                ValueKind::Bytes(bytes) => bytes.len(),
                _ => 0,
            })),
            NativeIterSource::Bytearray { data, .. } => Some(position.min(data.borrow().len())),
            NativeIterSource::String { value, byte_pos } => {
                let consumed = position.min(value.str_codepoint_len());
                *byte_pos = value.str_codepoint_byte_offset(consumed);
                Some(consumed)
            }
            _ => None,
        };
        if let Some(restored) = restored {
            frame.pos = restored;
            return Ok(true);
        }
        return Ok(false);
    }
    if let Some(range) = borrow.downcast_mut::<RangeIter>() {
        let remaining = range_len(range.cur, range.stop, range.step);
        let consumed = (position as i128).min(remaining);
        if consumed == remaining {
            range.cur = range.stop;
        } else {
            let advanced = i128::from(range.cur) + i128::from(range.step) * consumed;
            range.cur = i64::try_from(advanced)
                .map_err(|_| PyError::Runtime("range iterator position overflow".into()))?;
        }
        return Ok(true);
    }
    if let Some(range) = borrow.downcast_mut::<BigRangeIter>() {
        let remaining = pyrust_core::bigrange_len(&range.cur, &range.stop, &range.step);
        let consumed = PyBigInt::from(position);
        if consumed >= remaining {
            range.cur = range.stop.clone();
        } else {
            range.cur += &range.step * consumed;
        }
        return Ok(true);
    }
    Ok(false)
}

/// Signed-state adapter used only while executing a dynamic iterator
/// reduction. Native forward iterators clamp negative state to zero; native
/// reverse iterators encode state `-1` as a zero `next_index`.
pub(crate) fn restore_reduced_iterator_position(value: &Value, position: i64) -> Result<bool> {
    if let Ok(position) = usize::try_from(position) {
        return restore_native_iterator_position(value, position);
    }
    if let ValueKind::Generator(state) = value.kind()
        && let Ok(mut borrow) = state.try_borrow_mut()
        && let Some(frame) = borrow.downcast_mut::<NativeIterFrame>()
        && !matches!(frame.copy_policy, NativeIterCopyPolicy::Generator)
        && let NativeIterSource::ReverseIndexed { value, next_index } = &mut frame.source
    {
        let live_len = reverse_source_live_len(value).unwrap_or(0);
        *next_index = 0;
        frame.pos = live_len;
        return Ok(true);
    }
    restore_native_iterator_position(value, 0)
}

/// Build the independent iterator CPython's `__reduce__` round-trip produces.
///
/// The rebuilt frame initially shares retained Python values. For `deepcopy`,
/// the `copy` module recursively copies and re-seats those values through
/// [`iterator_retained_values`] after memoising the result, so subclasses and
/// containers that refer back to their own iterator are both preserved.
pub(crate) fn copy_iterator_object(value: &Value, deep: bool) -> Result<IteratorCopy> {
    use std::any::TypeId;

    if let Some((constructor, owner, position)) = getitem_iterator_reduction(value)? {
        return Ok(IteratorCopy::GetItemReduction {
            constructor,
            owner,
            position,
        });
    }
    // Every genuine `reversed` cursor copies through its dynamic reduction.
    // In particular, an observed-exhausted native cursor reconstructs a fresh
    // live empty cursor, and deepcopy must recurse into the owner before it
    // memoises the outer iterator.
    if let Some((constructor, owner, position)) = reversed_iterator_reduction(value)? {
        return Ok(IteratorCopy::GetItemReduction {
            constructor,
            owner,
            position,
        });
    }

    let ValueKind::Generator(state_rc) = value.kind() else {
        return Ok(IteratorCopy::Unowned);
    };
    // A frame refuses the reduction whatever it is doing: `copy.copy` never
    // touches the frame, it goes straight to `__reduce_ex__`. So a running
    // generator raises the same `TypeError` as a suspended one, rather than
    // #2285's `ValueError` — and now under its exact noun, because the kind
    // tag is readable even while the cell is checked out (#2978).
    if let Some(noun) = state_rc.kind().frame_type_name() {
        return Ok(IteratorCopy::Unpicklable(noun));
    }
    // Only built-in iterators reach here, and they release the cell before
    // running any user code.
    let borrow = state_rc.borrow();
    let tid = {
        let any_ref: &dyn std::any::Any = &**borrow;
        any_ref.type_id()
    };

    if tid == TypeId::of::<NativeIterFrame>() {
        let frame = borrow
            .downcast_ref::<NativeIterFrame>()
            .ok_or_else(|| PyError::Runtime("invalid iterator state".to_string()))?;
        if matches!(frame.copy_policy, NativeIterCopyPolicy::Generator) {
            return Ok(IteratorCopy::Unpicklable("generator"));
        }
        if let NativeIterSource::Bytearray { carrier, .. } = &frame.source {
            return Ok(IteratorCopy::BytearrayReduction {
                carrier: carrier.clone(),
                position: frame.pos,
            });
        }
        let copied = frame.reduced_copy(deep)?;
        return Ok(IteratorCopy::Rebuilt(Value::generator(Box::new(copied))));
    }
    if tid == TypeId::of::<RangeIter>() {
        let it = expect_state::<RangeIter>(&**borrow)?;
        return Ok(rebuilt(RangeIter {
            cur: it.cur,
            stop: it.stop,
            step: it.step,
        }));
    }
    if tid == TypeId::of::<BigRangeIter>() {
        let it = expect_state::<BigRangeIter>(&**borrow)?;
        return Ok(rebuilt(BigRangeIter {
            cur: it.cur.clone(),
            stop: it.stop.clone(),
            step: it.step.clone(),
        }));
    }
    if tid == TypeId::of::<CallableIter>() {
        let it = expect_state::<CallableIter>(&**borrow)?;
        return Ok(rebuilt(CallableIter {
            callable: it.callable.clone(),
            sentinel: it.sentinel.clone(),
            done: it.done,
        }));
    }
    if tid == TypeId::of::<MapIter>() {
        let it = expect_state::<MapIter>(&**borrow)?;
        return Ok(rebuilt(MapIter {
            func: it.func.clone(),
            sources: it.sources.clone(),
            done: it.done,
        }));
    }
    if tid == TypeId::of::<FilterIter>() {
        let it = expect_state::<FilterIter>(&**borrow)?;
        return Ok(rebuilt(FilterIter {
            func: it.func.clone(),
            source: it.source.clone(),
            done: it.done,
        }));
    }
    if tid == TypeId::of::<ZipIter>() {
        let it = expect_state::<ZipIter>(&**borrow)?;
        return Ok(rebuilt(ZipIter {
            sources: it.sources.clone(),
            strict: it.strict,
            done: it.done,
            count: it.count,
        }));
    }
    if tid == TypeId::of::<EnumerateIter>() {
        let it = expect_state::<EnumerateIter>(&**borrow)?;
        return Ok(rebuilt(EnumerateIter {
            source: it.source.clone(),
            counter: it.counter.clone(),
            done: it.done,
        }));
    }
    // A standard-library provider owns its own cursor and reduce policy.
    Ok(IteratorCopy::Unowned)
}

fn expect_state<T: 'static>(state: &dyn std::any::Any) -> Result<&T> {
    state
        .downcast_ref::<T>()
        .ok_or_else(|| PyError::Runtime("invalid iterator state".to_string()))
}

fn rebuilt<T: 'static>(state: T) -> IteratorCopy {
    IteratorCopy::Rebuilt(Value::generator(Box::new(state)))
}

/// The Python values a rebuilt iterator retains, in re-seat order.
///
/// `copy.deepcopy` must copy the source a reduce would have carried, but only
/// after the new iterator is in the memo — otherwise a list holding its own
/// iterator recurses forever. Splitting the rebuild from the re-seat is the
/// same two-step the opaque-storage arm uses for `storage_elements`.
///
/// `None` means the iterator retains nothing a deep copy should replace.
pub(crate) fn iterator_retained_values(value: &Value) -> Option<Vec<Value>> {
    use std::any::TypeId;

    let ValueKind::Generator(state_rc) = value.kind() else {
        return None;
    };
    let borrow = state_rc.try_borrow().ok()?;
    let tid = {
        let any_ref: &dyn std::any::Any = &**borrow;
        any_ref.type_id()
    };

    if tid == TypeId::of::<NativeIterFrame>() {
        return borrow
            .downcast_ref::<NativeIterFrame>()
            .map(NativeIterFrame::retained_values);
    }
    if tid == TypeId::of::<MapIter>() {
        let it = borrow.downcast_ref::<MapIter>()?;
        let mut values = vec![it.func.clone()];
        values.extend(it.sources.iter().cloned());
        return Some(values);
    }
    if tid == TypeId::of::<FilterIter>() {
        let it = borrow.downcast_ref::<FilterIter>()?;
        let mut values = Vec::new();
        values.extend(it.func.clone());
        values.push(it.source.clone());
        return Some(values);
    }
    if tid == TypeId::of::<ZipIter>() {
        return Some(borrow.downcast_ref::<ZipIter>()?.sources.clone());
    }
    if tid == TypeId::of::<EnumerateIter>() {
        return Some(vec![borrow.downcast_ref::<EnumerateIter>()?.source.clone()]);
    }
    if tid == TypeId::of::<CallableIter>() {
        let it = borrow.downcast_ref::<CallableIter>()?;
        return Some(vec![it.callable.clone(), it.sentinel.clone()]);
    }
    // A range cursor holds only integers. Legacy `__getitem__` walks execute
    // their typed reduction in the copy module before reaching this hook.
    None
}

/// Re-seat the values [`iterator_retained_values`] reported. `Ok(false)` when
/// the count no longer matches, which leaves shallow-shared sources in place.
/// A typed source replacement can reject an incompatible copied owner.
pub(crate) fn set_iterator_retained_values(value: &Value, values: Vec<Value>) -> Result<bool> {
    use std::any::TypeId;

    let ValueKind::Generator(state_rc) = value.kind() else {
        return Ok(false);
    };
    let Ok(mut borrow) = state_rc.try_borrow_mut() else {
        return Ok(false);
    };
    let tid = {
        let any_ref: &dyn std::any::Any = &**borrow;
        any_ref.type_id()
    };

    if tid == TypeId::of::<NativeIterFrame>() {
        return borrow
            .downcast_mut::<NativeIterFrame>()
            .map_or(Ok(false), |frame| frame.set_retained_values(values));
    }
    if tid == TypeId::of::<MapIter>() {
        let Some(it) = borrow.downcast_mut::<MapIter>() else {
            return Ok(false);
        };
        if values.len() != it.sources.len() + 1 {
            return Ok(false);
        }
        let mut values = values.into_iter();
        it.func = values.next().expect("length checked above");
        it.sources = values.collect();
        return Ok(true);
    }
    if tid == TypeId::of::<FilterIter>() {
        let Some(it) = borrow.downcast_mut::<FilterIter>() else {
            return Ok(false);
        };
        if values.len() != usize::from(it.func.is_some()) + 1 {
            return Ok(false);
        }
        let mut values = values.into_iter();
        if it.func.is_some() {
            it.func = Some(values.next().expect("length checked above"));
        }
        it.source = values.next().expect("length checked above");
        return Ok(true);
    }
    if tid == TypeId::of::<ZipIter>() {
        let Some(it) = borrow.downcast_mut::<ZipIter>() else {
            return Ok(false);
        };
        if values.len() != it.sources.len() {
            return Ok(false);
        }
        it.sources = values;
        return Ok(true);
    }
    if tid == TypeId::of::<EnumerateIter>() {
        let Some(it) = borrow.downcast_mut::<EnumerateIter>() else {
            return Ok(false);
        };
        let Ok([source]) = <[Value; 1]>::try_from(values) else {
            return Ok(false);
        };
        it.source = source;
        return Ok(true);
    }
    if tid == TypeId::of::<CallableIter>() {
        let Some(it) = borrow.downcast_mut::<CallableIter>() else {
            return Ok(false);
        };
        let Ok([callable, sentinel]) = <[Value; 2]>::try_from(values) else {
            return Ok(false);
        };
        it.callable = callable;
        it.sentinel = sentinel;
        return Ok(true);
    }
    Ok(false)
}

impl NativeIterFrame {
    /// The frame CPython's `__reduce__` round-trip would rebuild.
    ///
    /// Cursor-shaped sources collapse to a `list_iterator` over their remaining
    /// elements; every other source is retained as-is at the same position, so
    /// the two cursors walk one shared sequence independently.
    fn reduced_copy(&self, _deep: bool) -> Result<Self> {
        // A dict / set / dict-view cursor reduces to the list of what is left,
        // so its copy is a plain `list_iterator` with no container, no guard,
        // and no size latch to inherit. An exhausted one reduces the same way,
        // to an empty list — CPython's `dictiter_reduce` does not consult the
        // walk's state — so the released source keeps the shape bit rather than
        // letting the copy fall back to the cursor's own type.
        if self.source.reduces_to_list() {
            return Ok(NativeIterFrame::new(
                self.remaining_snapshot()?,
                "list_iterator",
            ));
        }
        if matches!(
            self.source,
            NativeIterSource::Exhausted {
                copy_kind: ExhaustedCopyKind::TupleIterator
            }
        ) {
            return Ok(NativeIterFrame::new(Vec::new(), "tuple_iterator"));
        }
        if let NativeIterSource::Deque {
            data,
            remaining,
            reverse,
            replacement,
        } = &self.source
        {
            let live_len = data.borrow().len();
            let failed = self.guard.as_ref().is_some_and(|guard| guard.failed);
            let reduce_index = deque_reduce_index(live_len, *remaining, failed);
            let consumed = usize::try_from(reduce_index).unwrap_or(0).min(live_len);
            let guard = self
                .guard
                .as_ref()
                .ok_or_else(|| PyError::Runtime("deque iterator lost its owner".to_string()))?;
            let GuardVersion::DequeState { counter, .. } = &guard.kind else {
                return Err(PyError::Runtime(
                    "deque iterator lost its mutation state".to_string(),
                ));
            };
            let mut copied = NativeIterFrame::guarded_deque(
                Rc::clone(data),
                counter.clone(),
                guard.container.clone(),
                *replacement,
                *reverse,
                consumed,
            );
            if failed && let Some(guard) = &mut copied.guard {
                guard.failed = true;
            }
            return Ok(copied);
        }
        let source = match &self.source {
            NativeIterSource::Indexed(value) => NativeIterSource::Indexed(value.clone()),
            NativeIterSource::ReverseIndexed { value, next_index } => {
                NativeIterSource::ReverseIndexed {
                    value: value.clone(),
                    next_index: *next_index,
                }
            }
            NativeIterSource::Bytes(value) => NativeIterSource::Bytes(value.clone()),
            // Retain the exact reduction carrier here. `deepcopy` memoises the
            // rebuilt iterator first, then recursively copies and re-seats this
            // value through `set_retained_values`, preserving subclasses and
            // cycles without holding the old primitive backing.
            NativeIterSource::Bytearray { carrier, data } => NativeIterSource::Bytearray {
                carrier: carrier.clone(),
                data: Rc::clone(data),
            },
            NativeIterSource::String { value, byte_pos } => NativeIterSource::String {
                value: value.clone(),
                byte_pos: *byte_pos,
            },
            NativeIterSource::Deque { .. } => unreachable!("deque copied above"),
            // `reduces_to_list` ruled out the cursor shape above, so a released
            // sequence walk stays a released sequence walk of its own type.
            NativeIterSource::Exhausted { copy_kind } => NativeIterSource::Exhausted {
                copy_kind: *copy_kind,
            },
            // The cursor-shaped sources returned above. Keeping their shape bit
            // here rather than asserting means a future source added to
            // `reduces_to_list` without an arm still reduces to a list.
            NativeIterSource::Materialized(_)
            | NativeIterSource::LiveKeys { .. }
            | NativeIterSource::InstanceDict { .. }
            | NativeIterSource::ReverseDict(_)
            | NativeIterSource::DictView { .. } => NativeIterSource::Exhausted {
                copy_kind: ExhaustedCopyKind::ListIterator,
            },
        };
        Ok(NativeIterFrame {
            source,
            pos: self.pos,
            type_name: self.type_name,
            class: self.class,
            guard: self.guard.clone(),
            exhausted: self.exhausted,
            copy_policy: self.copy_policy,
        })
    }

    /// What this iterator has left, read without disturbing it.
    ///
    /// CPython's `dictiter_reduce` drains a *copy* of the iterator struct
    /// (`dictiterobject tmp = *di;`), so the original keeps its position and
    /// its live guard while the reduction materialises the remainder. A latched
    /// cursor re-raises out of the drain exactly as it does out of `next()`, so
    /// copying an iterator that already reported a size change raises too.
    fn remaining_snapshot(&self) -> Result<Vec<Value>> {
        let mut probe = self.probe_clone();
        let drained = probe.drain_remaining();
        // The drain releases a cursor that reaches a terminal state, but an
        // early return can leave one live; releasing here retires the watch
        // reference `probe_clone` took in every case.
        if let NativeIterSource::LiveKeys { cursor, .. } = &mut probe.source {
            cursor.release();
        }
        drained
    }

    /// A throwaway frame that reproduces this one's remaining walk.
    fn probe_clone(&self) -> Self {
        let source = match &self.source {
            NativeIterSource::Materialized(items) => NativeIterSource::Materialized(items.clone()),
            NativeIterSource::LiveKeys { container, cursor } => {
                let mut cursor = cursor.clone();
                cursor.adopt_terminal_key_watch();
                NativeIterSource::LiveKeys {
                    container: container.clone(),
                    cursor,
                }
            }
            NativeIterSource::InstanceDict {
                proxy,
                recorded_len,
                size_changed,
            } => NativeIterSource::InstanceDict {
                proxy: proxy.clone(),
                recorded_len: *recorded_len,
                size_changed: *size_changed,
            },
            NativeIterSource::ReverseDict(cursor) => {
                NativeIterSource::ReverseDict(Box::new((**cursor).clone()))
            }
            NativeIterSource::DictView { dict, keys, kind } => NativeIterSource::DictView {
                dict: Rc::clone(dict),
                keys: keys.clone(),
                kind: *kind,
            },
            // An exhausted cursor is probed too — its remainder is empty, and
            // `drain_remaining` reports that without touching the source.
            NativeIterSource::Exhausted { copy_kind } => NativeIterSource::Exhausted {
                copy_kind: *copy_kind,
            },
            // Only the cursor-shaped sources are ever probed.
            _ => NativeIterSource::Exhausted {
                copy_kind: ExhaustedCopyKind::PreserveType,
            },
        };
        NativeIterFrame {
            source,
            pos: self.pos,
            type_name: self.type_name,
            class: self.class,
            guard: self.guard.clone(),
            exhausted: self.exhausted,
            copy_policy: self.copy_policy,
        }
    }

    /// The values a deep copy of this rebuilt frame must replace.
    fn retained_values(&self) -> Vec<Value> {
        match &self.source {
            NativeIterSource::Materialized(items) => items.clone(),
            NativeIterSource::Indexed(value)
            | NativeIterSource::Bytes(value)
            | NativeIterSource::String { value, .. }
            | NativeIterSource::ReverseIndexed { value, .. } => vec![value.clone()],
            NativeIterSource::Bytearray { carrier, .. } => vec![carrier.clone()],
            NativeIterSource::Deque { .. } => self
                .guard
                .as_ref()
                .map(|guard| vec![guard.container.clone()])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn set_retained_values(&mut self, values: Vec<Value>) -> Result<bool> {
        let deque_failed = self.guard.as_ref().is_some_and(|guard| guard.failed);
        match &mut self.source {
            NativeIterSource::Materialized(items) => {
                if items.len() != values.len() {
                    return Ok(false);
                }
                *items = values;
                Ok(true)
            }
            NativeIterSource::Indexed(value)
            | NativeIterSource::Bytes(value)
            | NativeIterSource::String { value, .. }
            | NativeIterSource::ReverseIndexed { value, .. } => {
                let Ok([replacement]) = <[Value; 1]>::try_from(values) else {
                    return Ok(false);
                };
                *value = replacement;
                Ok(true)
            }
            NativeIterSource::Bytearray { carrier, data } => {
                let Ok([replacement]) = <[Value; 1]>::try_from(values) else {
                    return Ok(false);
                };
                let replacement_backing = effective_builtin_receiver(&replacement, &[])
                    .unwrap_or_else(|| replacement.clone());
                let Some(replacement_data) =
                    pyrust_builtins::bytearray::as_bytearray_rc(&replacement_backing)
                else {
                    return Ok(false);
                };
                *carrier = replacement;
                *data = replacement_data;
                Ok(true)
            }
            NativeIterSource::Deque {
                data,
                remaining,
                replacement: replacement_resolver,
                ..
            } => {
                let Ok([replacement_owner]) = <[Value; 1]>::try_from(values) else {
                    return Ok(false);
                };
                let old_len = data.borrow().len();
                let consumed = if deque_failed {
                    old_len
                } else {
                    old_len.saturating_sub(*remaining)
                };
                let Some((replacement_data, replacement_counter)) =
                    replacement_resolver(&replacement_owner)
                else {
                    return Err(pyrust_core::type_err!(
                        "argument 1 must be collections.deque, not {}",
                        pyrust_core::error_type_name(&replacement_owner)
                    ));
                };
                let Some(guard) = &mut self.guard else {
                    return Ok(false);
                };
                let GuardVersion::DequeState { counter, .. } = &mut guard.kind else {
                    return Ok(false);
                };
                let replacement_len = replacement_data.borrow().len();
                let replacement_pos = if deque_failed {
                    replacement_len
                } else {
                    consumed.min(replacement_len)
                };
                *data = replacement_data;
                *remaining = replacement_len - replacement_pos;
                self.pos = replacement_pos;
                guard.container = replacement_owner;
                guard.version = replacement_counter.get();
                *counter = replacement_counter;
                Ok(true)
            }
            _ => Ok(values.is_empty()),
        }
    }
}

impl LiveKeyCursor {
    /// Take this clone's own reference to the terminal-key removal watch.
    ///
    /// The watch is reference-counted per cursor, so a clone that inherited the
    /// flag must register once more or its `release` would retire the watch the
    /// original still relies on. CPython incref's `di_dict` into the struct copy
    /// it drains for the same reason.
    fn adopt_terminal_key_watch(&mut self) {
        if !self.watching_terminal_key {
            return;
        }
        if let (Some(state), Some(key)) = (&self.mutation_state, &self.last_key) {
            state.watch_key_reinsertion(key);
        }
    }
}
