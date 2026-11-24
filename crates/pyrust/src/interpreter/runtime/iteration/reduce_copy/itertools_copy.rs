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

/// Copy or deep-copy a standard-library [`ProviderIterator`] through its
/// provider-registered copy hook (issue #2967).
///
/// Returns `Ok(None)` when `value` is not a `ProviderIterator` with a copy
/// hook. The caller supplies `copy_child`, `memo_get`, and `memo_insert` so
/// deep copies stay cycle-safe and shared-state providers (`itertools._tee`)
/// can share their deep-copied backing across sibling branches.
pub(crate) fn copy_provider_iterator(
    value: &Value,
    deep: bool,
    copy_child: &mut dyn FnMut(Value) -> Result<Value>,
    memo_get: &mut dyn FnMut(i64) -> Option<Value>,
    memo_insert: &mut dyn FnMut(i64, Value),
    outer_obj_id: Option<i64>,
) -> Result<Option<Value>> {
    let ValueKind::Generator(state_rc) = value.kind() else {
        return Ok(None);
    };
    let borrow = state_rc.borrow();
    let Some(provider) = borrow.downcast_ref::<ProviderIterator>() else {
        return Ok(None);
    };
    let Some(copy_fn) = provider.copy_fn() else {
        return Ok(None);
    };
    copy_fn(
        provider,
        deep,
        copy_child,
        memo_get,
        memo_insert,
        outer_obj_id,
    )
    .map(Some)
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
