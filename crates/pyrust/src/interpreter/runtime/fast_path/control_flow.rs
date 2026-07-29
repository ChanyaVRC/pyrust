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

/// Canonical-sequence element read for `GetItemSeqIntOrExit`: `Some(element)`
/// only when `sequence` is an exact `list`/`tuple`, `index` is a built-in
/// integer inside `0..len`, and the element read out is itself an `int`.
///
/// Every other operand shape — unset slots, negative or out-of-range indices,
/// `bool`, `float`, and huge-integer indices, mappings, and user
/// `__getitem__` receivers — returns `None` so the caller deopts to the
/// original subscript, which owns the raise, the coercion, and the
/// diagnostics.
///
/// The element check is the same per-iteration fact a standalone
/// `JumpIfNotInt` on the destination would establish; folding it in here is
/// sound because both share one deopt target — the original subscript — and a
/// region carrying a subscript admits no branch that could land between them.
#[inline]
pub(super) fn try_indexed_sequence_int_element(sequence: &Value, index: &Value) -> Option<Value> {
    if sequence.is_unset() || index.is_unset() {
        return None;
    }
    // `as_int()` accepts an exact `int` in either machine or big representation
    // and nothing else, so `True`/`False` deopt with the rest.  An integer
    // outside `usize` is out of range for any sequence and deopts too.
    let position = usize::try_from(index.as_int()?).ok()?;
    let element = indexed_sequence_item(sequence, position)?;
    // Matches `JumpIfNotInt`: unset slots and every non-int-family value divert
    // to the original loop, so the copy's arithmetic only ever sees ints.
    (!element.is_unset() && element.as_int().is_some()).then_some(element)
}

/// Whether `value` is the built-in `len` — the entry guard for a loop copy that
/// reads a sequence length natively instead of calling it.
///
/// A built-in callable dispatches purely by the `&'static str` its value kind
/// carries, so matching the name *is* matching the behaviour: every value that
/// answers `true` here calls the same Rust `len` body.  Nothing reachable from
/// Python can produce such a value other than by resolving the built-in, so a
/// `def len` shadow, an assignment, or a `builtins.len` patch all answer
/// `false` and run the original call.
#[inline(always)]
pub(super) fn value_is_builtin_len(value: &Value) -> bool {
    matches!(value.kind(), ValueKind::BuiltinFunction("len"))
}

/// Canonical-sequence length for `LenSeqOrExit`: `Some(n)` only for the exact
/// built-in sequence types whose length is a field read that can neither raise
/// nor re-enter Python.
///
/// Deliberately narrower than `len()` itself.  `dict` / `set` lengths are just
/// as cheap but are not what the guarded loop shape indexes; `range` can
/// overflow `i64` and raise `OverflowError`; `BuiltinObject`, `PyInstance`, and
/// `PyClass` can all reach user `__len__`.  Every one of those returns `None`
/// so the caller deopts and the real `len` runs on the original path.
#[inline]
pub(super) fn try_builtin_sequence_len(sequence: &Value) -> Option<i64> {
    // `list` first and through its own accessor: it is the shape the guarded
    // loop is built for, and it reaches the length without materialising a
    // whole `ValueKind` around the borrow.
    if let Some(length) = sequence.list_len() {
        return Some(length as i64);
    }
    if sequence.is_unset() {
        return None;
    }
    match sequence.kind() {
        ValueKind::Tuple(items) => Some(items.len() as i64),
        ValueKind::Str(_) => Some(sequence.str_codepoint_len() as i64),
        ValueKind::Bytes(rc) => Some(rc.len() as i64),
        _ => None,
    }
}
