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
            | ValueKind::Generator(_)
            | ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::BuiltinObject { .. }
    ) || matches!(
        b.kind(),
        ValueKind::PyInstance(_)
            | ValueKind::Generator(_)
            | ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::BuiltinObject { .. }
    )
}

/// Resolve the common scalar pair without building either `ValueKind`.
/// Equal pairs only need the left tag check; an unequal pair verifies both
/// tags before deciding that Python dispatch is impossible.
fn scalar_pair_fast_eq(left: &Value, right: &Value) -> Option<bool> {
    if left.scalar_bits_equal(right) {
        return Some(true);
    }
    if left.cannot_user_eq() {
        if left == right {
            return Some(true);
        }
        if right.cannot_user_eq() {
            return Some(false);
        }
    }
    None
}

fn value_can_use_raw_eq(value: &Value) -> bool {
    if value.cannot_user_eq() {
        return !value.is_identical_nan(value);
    }
    let alias = value.clone();
    !pair_may_need_dispatch(value, &alias) && value == &alias
}

/// Compare equal-length sequence snapshots without invoking Python code.
fn try_seq_fast_eq(a: &[Value], b: &[Value]) -> SeqFast {
    debug_assert_eq!(a.len(), b.len());
    for (left, right) in a.iter().zip(b.iter()) {
        if let Some(equal) = scalar_pair_fast_eq(left, right) {
            if equal {
                continue;
            }
            return SeqFast::Resolved(false);
        }
        if pair_may_need_dispatch(left, right) {
            if values_are_identical(left, right) {
                continue;
            }
            return SeqFast::NeedsDispatch;
        }
        if left == right || left.is_identical_nan(right) {
            continue;
        }
        return SeqFast::Resolved(false);
    }
    SeqFast::Resolved(true)
}

/// Compare exact dictionaries in one pass while every reached key/value pair
/// is safe for core equality. `None` hands recursive, callback-capable, or
/// non-reflexive (NaN) values to the identity-aware guarded path.
fn try_dict_fast_eq(left: &PyDict, right: &PyDict) -> Option<bool> {
    if left.len() != right.len() {
        return Some(false);
    }
    if left.may_have_dynamic_key() || right.may_have_dynamic_key() {
        return None;
    }
    for (key, left_value) in left.iter() {
        let Some(right_value) = right.get(key) else {
            return Some(false);
        };
        if left_value.scalar_bits_equal(right_value) {
            continue;
        }
        if !value_can_use_raw_eq(left_value) {
            return None;
        }
        if left_value == right_value {
            continue;
        }
        if value_can_use_raw_eq(right_value) {
            return Some(false);
        }
        return None;
    }
    Some(true)
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
