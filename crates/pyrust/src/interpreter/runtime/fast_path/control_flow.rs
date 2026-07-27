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
