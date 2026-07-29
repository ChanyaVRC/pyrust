// Remaining-element counts for built-in iterator objects (issue #2920).
//
// CPython gives most of its concrete iterator types a `__length_hint__` slot
// that reports how many elements are still to come.  The count is a property of
// the cursor, so this domain — which owns every built-in cursor representation
// — computes it; `builtin_methods` only routes the Python-visible name onto the
// iterator object, and `PyObject_LengthHint` (see
// `value_protocols::length`) consumes it through the ordinary protocol.
//
// The per-cursor rules mirror CPython's slot implementations:
//   * sequence walks report `live_len - position`, clamped at zero, so a list
//     that grew or shrank mid-walk is observed exactly as `listiter_len` does;
//   * `reversed` reports `index + 1`, or zero once the sequence became shorter
//     than that (`listreviter_len` / `reversed_len`);
//   * dict/set cursors report their original quota minus what they yielded,
//     but collapse to zero the moment the container's *size* changed, matching
//     the `di_used`/`si_used` stamp comparison in `dictiter_len`/`setiter_len`;
//   * an iterator that reached its terminal state reports zero.

/// What a built-in iterator reports for CPython's `__length_hint__`.
pub(crate) enum IteratorLengthHint {
    /// The iterator type has no `__length_hint__` at all: generators,
    /// `map`/`filter`/`zip`/`enumerate`, `callable_iterator`, coroutine
    /// awaitables, and standard-library provider cursors.
    Absent,
    /// A count computed entirely from the cursor's own state.
    Count(Value),
    /// A legacy `__len__` + `__getitem__` sequence walk.  CPython re-reads the
    /// live length slot on every hint, so the call has to happen after the
    /// state borrow is released.
    LiveSequence {
        object: Value,
        /// The index the walk will read next.
        index: i64,
        /// `reversed(seq)` counts down and reports `index + 1`; a forward
        /// `iterator` counts up and reports `live_len - index`.
        reverse: bool,
    },
}

/// Classify a built-in iterator's state cell.
///
/// `state` is the type-erased payload of a `Generator` value.
pub(crate) fn builtin_iterator_length_hint(state: &dyn std::any::Any) -> IteratorLengthHint {
    if let Some(frame) = state.downcast_ref::<NativeIterFrame>() {
        // A provider-tagged mapping presents its own iterator type, and
        // CPython's `odict_iterator` — unlike every plain dict cursor — has no
        // hint slot at all.  `exhaust_first` is the construction-time tag for
        // "a provider iteration policy applied to this frame"; it is set from
        // the same `IterationPolicy` that supplied the iterator's type name.
        if frame
            .guard
            .as_ref()
            .is_some_and(|guard| guard.exhaust_first)
        {
            return IteratorLengthHint::Absent;
        }
        return IteratorLengthHint::Count(Value::int(native_frame_remaining(frame)));
    }
    if let Some(range) = state.downcast_ref::<RangeIter>() {
        return IteratorLengthHint::Count(i128_to_int_value(pyrust_core::range_len(
            range.cur, range.stop, range.step,
        )));
    }
    if let Some(range) = state.downcast_ref::<BigRangeIter>() {
        return IteratorLengthHint::Count(Value::bigint(pyrust_core::bigrange_len(
            &range.cur,
            &range.stop,
            &range.step,
        )));
    }
    if let Some(iterator) = state.downcast_ref::<GetItemIter>() {
        if iterator.exhausted || iterator.remaining == Some(0) {
            return IteratorLengthHint::Count(Value::int(0));
        }
        return IteratorLengthHint::LiveSequence {
            object: iterator.obj.clone(),
            index: iterator.index,
            reverse: iterator.step < 0,
        };
    }
    IteratorLengthHint::Absent
}

/// Whether this iterator state exposes `__length_hint__` to Python.
///
/// Drives `hasattr` / `dir()` so the attribute surface matches CPython even
/// where the count itself needs an interpreter round-trip.
pub(crate) fn builtin_iterator_has_length_hint(state: &dyn std::any::Any) -> bool {
    !matches!(
        builtin_iterator_length_hint(state),
        IteratorLengthHint::Absent
    )
}

/// Whether `value` is a built-in iterator exposing `__length_hint__`.
pub(crate) fn value_has_length_hint(value: &Value) -> bool {
    let ValueKind::Generator(state_rc) = value.kind() else {
        return false;
    };
    state_rc
        .try_borrow()
        .is_ok_and(|borrow| builtin_iterator_has_length_hint(&**borrow))
}

/// Remaining elements behind a native iterator frame.
fn native_frame_remaining(frame: &NativeIterFrame) -> i64 {
    if frame.exhausted {
        return 0;
    }
    // A size-guarded snapshot behaves like a dict/set cursor: CPython compares
    // the container's recorded size and reports zero once it moved.  The deque
    // guard tracks structural state rather than size and leaves its own counter
    // alone, so it is not a hint input.
    if let Some(guard) = &frame.guard
        && matches!(guard.kind, GuardVersion::Size)
        && live_collection_len(&guard.container).map(|len| len as i64) != Some(guard.version)
    {
        return 0;
    }
    let pos = frame.pos;
    let remaining_from = |len: usize| i64::try_from(len.saturating_sub(pos)).unwrap_or(i64::MAX);
    match &frame.source {
        NativeIterSource::Materialized(items) => remaining_from(items.len()),
        NativeIterSource::LiveKeys { container, cursor } => {
            if cursor.exhausted || cursor.size_changed {
                return 0;
            }
            if live_collection_len(container) != Some(cursor.recorded_len) {
                return 0;
            }
            i64::try_from(cursor.recorded_len.saturating_sub(cursor.yielded_len()))
                .unwrap_or(i64::MAX)
        }
        NativeIterSource::InstanceDict {
            proxy,
            recorded_len,
            size_changed,
        } => {
            if *size_changed
                || pyrust_builtins::instance_dict::iter_visible_len(proxy) != Some(*recorded_len)
            {
                return 0;
            }
            remaining_from(*recorded_len)
        }
        NativeIterSource::Indexed(value) => remaining_from(match value.kind() {
            ValueKind::List(items) => items.len(),
            ValueKind::Tuple(items) => items.len(),
            _ => 0,
        }),
        // `reversed`: the walk's own descending cursor is the count, unless the
        // sequence has since become too short to reach it.
        NativeIterSource::ReverseIndexed { value, next_index } => {
            match reverse_source_live_len(value) {
                Some(len) if len >= *next_index => i64::try_from(*next_index).unwrap_or(i64::MAX),
                _ => 0,
            }
        }
        NativeIterSource::DictView { keys, .. } => remaining_from(keys.len()),
        NativeIterSource::Deque(data) => remaining_from(data.borrow().len()),
        NativeIterSource::Bytes(value) => remaining_from(match value.kind() {
            ValueKind::Bytes(bytes) => bytes.len(),
            _ => 0,
        }),
        NativeIterSource::String { value, .. } => remaining_from(value.str_codepoint_len()),
        NativeIterSource::Exhausted => 0,
    }
}

/// Live element count of a sequence a `reversed()` walk is reading.
fn reverse_source_live_len(value: &Value) -> Option<usize> {
    match value.kind() {
        ValueKind::List(items) => Some(items.len()),
        ValueKind::Tuple(items) => Some(items.len()),
        ValueKind::Bytes(bytes) => Some(bytes.len()),
        ValueKind::Str(_) => Some(value.str_codepoint_len()),
        ValueKind::BuiltinObject { ops, state } => ops.len(state),
        _ => None,
    }
}

/// Present a `Py_ssize_t`-sized count as a Python integer, widening only when
/// the exact value does not fit.
fn i128_to_int_value(count: i128) -> Value {
    match i64::try_from(count) {
        Ok(count) => Value::int(count),
        Err(_) => Value::bigint(PyBigInt::from(count)),
    }
}

impl Interpreter {
    /// Resolve a built-in iterator's `__length_hint__` result.
    ///
    /// `None` means the iterator type has no such slot, so the caller should
    /// behave exactly as it would for any other object without one.
    pub(crate) fn builtin_iterator_length_hint_value(
        &mut self,
        target: &Value,
    ) -> Result<Option<Value>> {
        let ValueKind::Generator(state_rc) = target.kind() else {
            return Ok(None);
        };
        let classified = {
            let borrow = state_rc
                .try_borrow()
                .map_err(|_| pyrust_core::value_err!("generator already executing"))?;
            builtin_iterator_length_hint(&**borrow)
        };
        match classified {
            IteratorLengthHint::Absent => Ok(None),
            IteratorLengthHint::Count(count) => Ok(Some(count)),
            IteratorLengthHint::LiveSequence {
                object,
                index,
                reverse,
            } => {
                // CPython's `seqiter` reports NotImplemented when the sequence
                // has no length slot at all; `reversed` only ever wraps one
                // that does.
                let Some(length_method) = lookup_value_special_method(&object, "__len__") else {
                    return Ok(Some(Value::not_implemented()));
                };
                let result = invoke_class_method(self, length_method, object, &[])?;
                let length = self.normalize_len_result(&result)?;
                let remaining = if reverse {
                    let position = index.saturating_add(1);
                    if length < position { 0 } else { position }
                } else {
                    (length - index).max(0)
                };
                Ok(Some(Value::int(remaining)))
            }
        }
    }
}
