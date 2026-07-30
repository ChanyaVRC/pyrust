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
}

/// Build the independent iterator CPython's `__reduce__` round-trip produces.
///
/// `deep` detaches storage cells that are not Python values — a `bytearray`'s
/// buffer — because `copy.deepcopy` copies the source a reduce would have
/// carried. Retained *values* stay shared here; the `copy` module re-seats them
/// through [`iterator_retained_values`] after memoising the result, so a
/// container that refers back to its own iterator terminates.
pub(crate) fn copy_iterator_object(value: &Value, deep: bool) -> Result<IteratorCopy> {
    use std::any::TypeId;

    let ValueKind::Generator(state_rc) = value.kind() else {
        return Ok(IteratorCopy::Unowned);
    };
    // A cell checked out by its own running body cannot be read; CPython
    // reports the same re-entrancy for any operation on an executing
    // generator (#2285).
    let borrow = state_rc
        .try_borrow()
        .map_err(|_| pyrust_core::value_err!("generator already executing"))?;
    let tid = {
        let any_ref: &dyn std::any::Any = &**borrow;
        any_ref.type_id()
    };

    if tid == TypeId::of::<NativeIterFrame>() {
        let frame = borrow
            .downcast_ref::<NativeIterFrame>()
            .ok_or_else(|| PyError::Runtime("invalid iterator state".to_string()))?;
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
    if tid == TypeId::of::<GetItemIter>() {
        let it = expect_state::<GetItemIter>(&**borrow)?;
        return Ok(rebuilt(GetItemIter {
            obj: it.obj.clone(),
            method: it.method.clone(),
            length_method: it.length_method.clone(),
            index: it.index,
            step: it.step,
            remaining: it.remaining,
            exhausted: it.exhausted,
        }));
    }
    if tid == TypeId::of::<GeneratorFrame>() {
        let frame = expect_state::<GeneratorFrame>(&**borrow)?;
        return Ok(IteratorCopy::Unpicklable(if frame.is_async_generator() {
            "async_generator"
        } else if frame.is_coroutine {
            "coroutine"
        } else {
            "generator"
        }));
    }
    // The gen-drive trampoline parked the frame here while the body runs
    // (#2253), so the object is mid-execution just as above.
    if tid == TypeId::of::<GenDriving>() {
        return Err(pyrust_core::value_err!("generator already executing"));
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
    // A range cursor holds only integers, and a legacy `__getitem__` walk holds
    // the object *and* two methods already bound to it — re-seating the object
    // alone would leave the walk calling the original's slots.
    None
}

/// Re-seat the values [`iterator_retained_values`] reported. `false` when the
/// count no longer matches, which leaves the shallow-shared sources in place.
pub(crate) fn set_iterator_retained_values(value: &Value, values: Vec<Value>) -> bool {
    use std::any::TypeId;

    let ValueKind::Generator(state_rc) = value.kind() else {
        return false;
    };
    let Ok(mut borrow) = state_rc.try_borrow_mut() else {
        return false;
    };
    let tid = {
        let any_ref: &dyn std::any::Any = &**borrow;
        any_ref.type_id()
    };

    if tid == TypeId::of::<NativeIterFrame>() {
        return borrow
            .downcast_mut::<NativeIterFrame>()
            .is_some_and(|frame| frame.set_retained_values(values));
    }
    if tid == TypeId::of::<MapIter>() {
        let Some(it) = borrow.downcast_mut::<MapIter>() else {
            return false;
        };
        if values.len() != it.sources.len() + 1 {
            return false;
        }
        let mut values = values.into_iter();
        it.func = values.next().expect("length checked above");
        it.sources = values.collect();
        return true;
    }
    if tid == TypeId::of::<FilterIter>() {
        let Some(it) = borrow.downcast_mut::<FilterIter>() else {
            return false;
        };
        if values.len() != usize::from(it.func.is_some()) + 1 {
            return false;
        }
        let mut values = values.into_iter();
        if it.func.is_some() {
            it.func = Some(values.next().expect("length checked above"));
        }
        it.source = values.next().expect("length checked above");
        return true;
    }
    if tid == TypeId::of::<ZipIter>() {
        let Some(it) = borrow.downcast_mut::<ZipIter>() else {
            return false;
        };
        if values.len() != it.sources.len() {
            return false;
        }
        it.sources = values;
        return true;
    }
    if tid == TypeId::of::<EnumerateIter>() {
        let Some(it) = borrow.downcast_mut::<EnumerateIter>() else {
            return false;
        };
        let Ok([source]) = <[Value; 1]>::try_from(values) else {
            return false;
        };
        it.source = source;
        return true;
    }
    if tid == TypeId::of::<CallableIter>() {
        let Some(it) = borrow.downcast_mut::<CallableIter>() else {
            return false;
        };
        let Ok([callable, sentinel]) = <[Value; 2]>::try_from(values) else {
            return false;
        };
        it.callable = callable;
        it.sentinel = sentinel;
        return true;
    }
    false
}

impl NativeIterFrame {
    /// The frame CPython's `__reduce__` round-trip would rebuild.
    ///
    /// Cursor-shaped sources collapse to a `list_iterator` over their remaining
    /// elements; every other source is retained as-is at the same position, so
    /// the two cursors walk one shared sequence independently.
    fn reduced_copy(&self, deep: bool) -> Result<Self> {
        let source = match &self.source {
            // A dict / set / dict-view cursor reduces to the list of what is
            // left, so its copy is a plain `list_iterator` with no container,
            // no guard, and no size latch to inherit.
            NativeIterSource::Materialized(_)
            | NativeIterSource::LiveKeys { .. }
            | NativeIterSource::InstanceDict { .. }
            | NativeIterSource::ReverseDict(_)
            | NativeIterSource::DictView { .. } => {
                return Ok(NativeIterFrame::new(
                    self.remaining_snapshot()?,
                    "list_iterator",
                ));
            }
            NativeIterSource::Indexed(value) => NativeIterSource::Indexed(value.clone()),
            NativeIterSource::ReverseIndexed { value, next_index } => {
                NativeIterSource::ReverseIndexed {
                    value: value.clone(),
                    next_index: *next_index,
                }
            }
            NativeIterSource::Bytes(value) => NativeIterSource::Bytes(value.clone()),
            // The buffer cell *is* the retained source (#2921): a deep copy
            // therefore detaches it rather than re-seating a value.
            NativeIterSource::Bytearray(data) => NativeIterSource::Bytearray(if deep {
                Rc::new(RefCell::new(data.borrow().clone()))
            } else {
                Rc::clone(data)
            }),
            NativeIterSource::String { value, byte_pos } => NativeIterSource::String {
                value: value.clone(),
                byte_pos: *byte_pos,
            },
            // A `deque` cursor shares its ring the way CPython's
            // `_deque_iterator` shares the deque it reduces with.
            NativeIterSource::Deque(data) => NativeIterSource::Deque(Rc::clone(data)),
            NativeIterSource::Exhausted => NativeIterSource::Exhausted,
        };
        Ok(NativeIterFrame {
            source,
            pos: self.pos,
            type_name: self.type_name,
            guard: self.guard.clone(),
            exhausted: self.exhausted,
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
            // Only the cursor-shaped sources are ever probed.
            _ => NativeIterSource::Exhausted,
        };
        NativeIterFrame {
            source,
            pos: self.pos,
            type_name: self.type_name,
            guard: self.guard.clone(),
            exhausted: self.exhausted,
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
            _ => Vec::new(),
        }
    }

    fn set_retained_values(&mut self, values: Vec<Value>) -> bool {
        match &mut self.source {
            NativeIterSource::Materialized(items) => {
                if items.len() != values.len() {
                    return false;
                }
                *items = values;
                true
            }
            NativeIterSource::Indexed(value)
            | NativeIterSource::Bytes(value)
            | NativeIterSource::String { value, .. }
            | NativeIterSource::ReverseIndexed { value, .. } => {
                let Ok([replacement]) = <[Value; 1]>::try_from(values) else {
                    return false;
                };
                *value = replacement;
                true
            }
            _ => values.is_empty(),
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
