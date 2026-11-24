/// True if `v` can serve as an operand in `X | Y` (PEP 604).
/// Valid operands: `PyClass`, `BuiltinFunction` type tokens (like `range`, `generator`),
/// `None` itself (coerced to the `NoneType` PyClass singleton), and existing `UnionType` values.
fn is_union_operand(v: &Value) -> bool {
    match v.kind() {
        ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_) | ValueKind::None => true,
        ValueKind::BuiltinObject { .. } => pyrust_builtins::union_type::is_union_type(v),
        _ => false,
    }
}

/// True if `v` is a "type-like" PEP 604 operand — a `PyClass`, a `BuiltinFunction`
/// acting as a type token, or an existing `UnionType`.  `None` is excluded: it
/// can appear in a union *only* when the other operand is a type, so at least
/// one side must satisfy this stricter predicate.  This matches CPython's
/// behaviour where `None | None` raises TypeError but `int | None` succeeds
/// (dispatched through `type.__or__`).
fn is_strict_type_union_operand(v: &Value) -> bool {
    match v.kind() {
        ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_) => true,
        ValueKind::BuiltinObject { .. } => pyrust_builtins::union_type::is_union_type(v),
        _ => false,
    }
}

/// Convert `None` to the `NoneType` PyClass singleton, leaving all other
/// values unchanged.  Used when assembling union components so that
/// `int | None` stores `NoneType` as the component (matching CPython).
fn coerce_none_to_nonetype(v: Value) -> Value {
    if v.is_none() {
        Value::py_class(crate::interpreter::canonical_class_by_tag(
            pyrust_core::CanonicalClassTag::NoneType,
        ))
    } else {
        v
    }
}
