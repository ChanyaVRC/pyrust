/// Truthiness for tagged scalar values that cannot invoke user code.
#[inline]
pub(super) fn try_scalar_truthiness_fast(value: &Value) -> Option<bool> {
    match value.kind() {
        ValueKind::Int(integer) => Some(integer != 0),
        ValueKind::Bool(boolean) => Some(boolean),
        _ => None,
    }
}

#[inline]
pub(super) fn try_integer_compare_fast(left: &Value, right: &Value, op: BinaryOp) -> Option<bool> {
    int_cmp(left.as_int()?, right.as_int()?, op)
}

#[inline]
pub(super) fn try_constant_compare_fast(left: &Value, right: &Value, op: BinaryOp) -> Option<bool> {
    if let Some(result) = try_integer_compare_fast(left, right, op) {
        return Some(result);
    }
    match (left.kind(), right.kind()) {
        (ValueKind::Str(left), ValueKind::Str(right)) => str_cmp(left, right, op),
        _ => None,
    }
}

/// Whether an iterator slot currently holds the canonical machine-int range
/// cursor — the only iterator kind the int-loop versioning guard admits,
/// because advancing it has no Python-visible effect.
#[inline(always)]
pub(super) fn iter_slot_is_int_range(state: Option<&IterState>) -> bool {
    matches!(state, Some(IterState::Range { .. }))
}

/// Whether an iterator slot currently holds the canonical machine-int range
/// cursor in exactly the state `guard` describes: a cursor still parked at
/// `start`, over the same `stop` and `step`.
///
/// This is the entry guard for a closed-form loop copy, so it must pin the
/// iterated values, not merely the cursor kind.  Matching all three fields
/// does exactly that — the yielded sequence of an `IterState::Range` is a
/// function of nothing else — which is why the copy needs no assumption about
/// how the range was produced.
#[inline(always)]
pub(super) fn iter_slot_is_int_range_exact(
    state: Option<&IterState>,
    guard: &crate::bytecode::IntRangeExactGuard,
) -> bool {
    matches!(
        state,
        Some(IterState::Range { cur, stop, step })
            if *cur == guard.start && *stop == guard.stop && *step == guard.step
    )
}

/// Whether an iterator slot currently holds the canonical list/tuple index
/// cursor — the second iterator kind the int-loop versioning guard admits,
/// because stepping it clones one element and bumps a counter without
/// invoking any Python-visible protocol.
#[inline(always)]
pub(super) fn iter_slot_is_indexed_sequence(state: Option<&IterState>) -> bool {
    matches!(state, Some(IterState::ValueIndexed { .. }))
}

/// Canonical-sequence element read for `GetItemSeqOrExit`: `Some(element)`
/// only when `sequence` is an exact `list`/`tuple` and `index` is a built-in
/// integer inside `0..len`.
///
/// Every other operand shape — unset slots, negative or out-of-range indices,
/// `bool`, `float`, and huge-integer indices, mappings, and user
/// `__getitem__` receivers — returns `None` so the caller deopts to the
/// original subscript, which owns the raise, the coercion, and the
/// diagnostics.
#[inline]
pub(super) fn try_indexed_sequence_element(sequence: &Value, index: &Value) -> Option<Value> {
    if sequence.is_unset() || index.is_unset() {
        return None;
    }
    // `as_int()` accepts an exact `int` in either machine or big representation
    // and nothing else, so `True`/`False` deopt with the rest.  An integer
    // outside `usize` is out of range for any sequence and deopts too.
    let position = usize::try_from(index.as_int()?).ok()?;
    indexed_sequence_item(sequence, position)
}
