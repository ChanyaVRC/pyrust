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
