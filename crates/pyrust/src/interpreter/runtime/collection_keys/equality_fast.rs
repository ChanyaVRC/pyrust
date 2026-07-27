/// Outcome of the borrow-only sequence equality fast path used by
/// `values_user_eq` for list/tuple pairs.
enum SeqFast {
    Resolved(bool),
    NeedsDispatch,
}

fn pair_may_need_dispatch(a: &Value, b: &Value) -> bool {
    matches!(
        a.kind(),
        ValueKind::PyInstance(_)
            | ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::BuiltinObject { .. }
    ) || matches!(
        b.kind(),
        ValueKind::PyInstance(_)
            | ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::BuiltinObject { .. }
    )
}

/// Compare equal-length sequence snapshots without invoking Python code.
fn try_seq_fast_eq(a: &[Value], b: &[Value]) -> SeqFast {
    debug_assert_eq!(a.len(), b.len());
    for (left, right) in a.iter().zip(b.iter()) {
        if left == right || left.is_identical_nan(right) {
            continue;
        }
        if pair_may_need_dispatch(left, right) {
            return SeqFast::NeedsDispatch;
        }
        return SeqFast::Resolved(false);
    }
    SeqFast::Resolved(true)
}

/// Extract a container subclass backing only when it inherits base equality.
fn coerce_container_backing_for_eq(value: &Value) -> Option<Value> {
    let backing = coerce_subclass_backing(value, &["__eq__"])?;
    let is_container = matches!(
        backing.kind(),
        ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::Bytes(_)
    ) || pyrust_builtins::frozenset::as_items(&backing).is_some()
        || pyrust_builtins::bytearray::as_bytearray_snapshot(&backing).is_some();
    is_container.then_some(backing)
}
