/// Python object identity for the runtime's mixed inline/heap value layout.
///
/// Heap-backed values compare their shared backing identity. Inline immutable
/// values use their singleton/value-box representation, matching `id()` within
/// the constraints of the compact value representation.
fn values_are_identical(left: &Value, right: &Value) -> bool {
    match (left.kind(), right.kind()) {
        (ValueKind::None, ValueKind::None)
        | (ValueKind::NotImplemented, ValueKind::NotImplemented)
        | (ValueKind::Ellipsis, ValueKind::Ellipsis) => true,
        (ValueKind::Bool(a), ValueKind::Bool(b)) => a == b,
        (ValueKind::Int(a), ValueKind::Int(b)) => a == b,
        (ValueKind::Float(a), ValueKind::Float(b)) => a.to_bits() == b.to_bits(),
        (ValueKind::Complex(ar, ai), ValueKind::Complex(br, bi)) => {
            ar.to_bits() == br.to_bits() && ai.to_bits() == bi.to_bits()
        }
        (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) => Rc::ptr_eq(a, b),
        (ValueKind::PyClass(a), ValueKind::PyClass(b)) => Rc::ptr_eq(a, b),
        (ValueKind::UserFunction(a), ValueKind::UserFunction(b)) => Rc::ptr_eq(a, b),
        (ValueKind::BuiltinFunction(_), ValueKind::BuiltinFunction(_)) => {
            match (left.as_function_rc(), right.as_function_rc()) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            }
        }
        (ValueKind::Generator(a), ValueKind::Generator(b)) => Rc::ptr_eq(a, b),
        (ValueKind::Str(_), ValueKind::Str(_))
        | (ValueKind::BigInt(_), ValueKind::BigInt(_))
        | (ValueKind::List(_), ValueKind::List(_))
        | (ValueKind::Set(_), ValueKind::Set(_))
        | (ValueKind::Dict(_), ValueKind::Dict(_))
        | (ValueKind::Tuple(_), ValueKind::Tuple(_)) => {
            matches!(
                (left.value_id(), right.value_id()),
                (Some(a), Some(b)) if a == b
            )
        }
        (ValueKind::Bytes(a), ValueKind::Bytes(b)) => Rc::ptr_eq(a, b),
        (ValueKind::PyModule(a), ValueKind::PyModule(b)) => Rc::ptr_eq(a, b),
        (ValueKind::BoundMethod { .. }, ValueKind::BoundMethod { .. })
        | (ValueKind::ClassBoundMethod { .. }, ValueKind::ClassBoundMethod { .. })
        | (ValueKind::SuperProxy { .. }, ValueKind::SuperProxy { .. })
        | (ValueKind::SuperProxyClass { .. }, ValueKind::SuperProxyClass { .. })
        | (ValueKind::SuperProxyUnbound { .. }, ValueKind::SuperProxyUnbound { .. }) => {
            matches!(
                (left.value_id(), right.value_id()),
                (Some(a), Some(b)) if a == b
            )
        }
        (
            ValueKind::BuiltinObject {
                state: left_state, ..
            },
            ValueKind::BuiltinObject {
                state: right_state, ..
            },
        ) => {
            Rc::ptr_eq(left_state, right_state)
                || pyrust_builtins::instance_dict::same_proxy_target(left, right)
        }
        _ => false,
    }
}
