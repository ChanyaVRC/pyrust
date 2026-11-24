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
        return Err(PyError::runtime(
            "native iterator lost its state".to_string(),
        ));
    };
    let borrow = state.borrow();
    let frame = borrow
        .downcast_ref::<NativeIterFrame>()
        .filter(|frame| frame.class == Some(expected))
        .ok_or_else(|| PyError::runtime("native iterator class mismatch".to_string()))?;

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
            _ => Err(PyError::runtime(
                "bytearray iterator lost its source".to_string(),
            )),
        },
        NativeIteratorClass::Deque | NativeIteratorClass::DequeReverse => {
            let NativeIterSource::Deque {
                data, remaining, ..
            } = &frame.source
            else {
                return Err(PyError::runtime(
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
                .ok_or_else(|| PyError::runtime("deque iterator lost its owner".to_string()))?;
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
        return Err(PyError::runtime(
            "native iterator lost its state".to_string(),
        ));
    };
    let mut borrow = state.borrow_mut();
    let frame = borrow
        .downcast_mut::<NativeIterFrame>()
        .filter(|frame| frame.class == Some(NativeIteratorClass::Bytearray))
        .ok_or_else(|| PyError::runtime("native iterator class mismatch".to_string()))?;
    if let NativeIterSource::Bytearray { data, .. } = &frame.source {
        frame.pos = position.min(data.borrow().len());
    }
    Ok(Value::none())
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
                .map_err(|_| PyError::runtime("range iterator position overflow".into()))?;
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
                .ok_or_else(|| PyError::runtime("deque iterator lost its owner".to_string()))?;
            let GuardVersion::DequeState { counter, .. } = &guard.kind else {
                return Err(PyError::runtime(
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
                reverse,
            } => NativeIterSource::InstanceDict {
                proxy: proxy.clone(),
                recorded_len: *recorded_len,
                size_changed: *size_changed,
                reverse: *reverse,
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
