/// Return the Python class object for any runtime value.
///
/// This is the shared source of truth for `type(obj)` and `obj.__class__`;
/// builtin functions consume it but do not own the object-model mapping.
pub(crate) fn value_class(value: &Value) -> Value {
    if let ValueKind::PyInstance(instance) = value.kind() {
        return Value::py_class(Rc::clone(&instance.borrow().class));
    }
    if let Some(class) = primitive_class_for_value(value) {
        return Value::py_class(class);
    }

    match value.kind() {
        ValueKind::PyClass(class) => {
            let metatype = class.borrow().metatype.clone();
            Value::py_class(metatype.unwrap_or_else(type_class_singleton))
        }
        ValueKind::UserFunction(function) => match function.kind {
            UserFunctionKind::StaticMethod => Value::builtin_function("staticmethod"),
            UserFunctionKind::ClassMethod => Value::builtin_function("classmethod"),
            _ => Value::py_class(function_type_singleton()),
        },
        ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
            Value::py_class(method_type_singleton())
        }
        ValueKind::BuiltinFunction(name) => {
            Value::builtin_function(pyrust_core::builtin_callable_presentation(name).type_name())
        }
        ValueKind::PyModule(_) => Value::builtin_function("module"),
        ValueKind::SuperProxy { .. }
        | ValueKind::SuperProxyClass { .. }
        | ValueKind::SuperProxyUnbound { .. } => Value::builtin_function("super"),
        ValueKind::Generator(cell) => {
            // A generator / coroutine / async generator answers from its
            // immutable kind tag.  Reading the state instead used to abort the
            // process whenever the question was asked from inside the running
            // body (`type(g)` via a callback), because the resume path holds
            // the cell mutably checked out for the whole of it (#2978).
            if let Some(name) = cell.kind().frame_type_name() {
                return Value::builtin_function(name);
            }
            // Built-in iterators drop the cell before running user code, so
            // their concrete cursor type is always readable here.
            let state = cell.borrow();
            if state.downcast_ref::<MapIter>().is_some() {
                Value::py_class(BuiltinTypeClass::Map.singleton())
            } else if state.downcast_ref::<FilterIter>().is_some() {
                Value::py_class(BuiltinTypeClass::Filter.singleton())
            } else if let Some(iterator) = state.downcast_ref::<ProviderIterator>() {
                iterator
                    .class()
                    .map(Value::py_class)
                    .unwrap_or_else(|| Value::builtin_function(iterator.fallback_class_name()))
            } else if state.downcast_ref::<EnumerateIter>().is_some() {
                Value::py_class(BuiltinTypeClass::Enumerate.singleton())
            } else if state.downcast_ref::<ZipIter>().is_some() {
                Value::py_class(BuiltinTypeClass::Zip.singleton())
            } else if state.downcast_ref::<CallableIter>().is_some() {
                Value::builtin_function("callable_iterator")
            } else if let Some(iterator) = state.downcast_ref::<GetItemIter>() {
                // A negative step marks the generic `reversed(seq)` cursor,
                // whose CPython type is the `reversed` class itself.
                if iterator.step < 0 {
                    Value::py_class(BuiltinTypeClass::Reversed.singleton())
                } else {
                    Value::builtin_function("iterator")
                }
            } else if state.downcast_ref::<RangeIter>().is_some() {
                Value::builtin_function("range_iterator")
            } else if state.downcast_ref::<BigRangeIter>().is_some() {
                Value::builtin_function("longrange_iterator")
            } else if let Some(native) = state.downcast_ref::<NativeIterFrame>() {
                // `reversed(tuple/str/bytes/bytearray)` reports the `reversed`
                // class in CPython; the per-type cursors (`list_reverseiterator`,
                // `dict_keyiterator`, …) remain unmigrated name tokens.
                native
                    .class
                    .map(NativeIteratorClass::singleton)
                    .or_else(|| builtin_type_class_by_name(native.type_name))
                    .map(Value::py_class)
                    .unwrap_or_else(|| Value::builtin_function(native.type_name))
            } else if state.downcast_ref::<AsyncGenASend>().is_some() {
                Value::builtin_function("async_generator_asend")
            } else {
                Value::builtin_function("generator")
            }
        }
        ValueKind::BuiltinObject { ops, .. } => {
            if pyrust_builtins::super_bound_builtin::as_super_bound_builtin(value).is_some() {
                return Value::builtin_function("builtin_function_or_method");
            }
            if pyrust_builtins::classmethod::as_class_bound_any(value).is_some() {
                return Value::py_class(method_type_singleton());
            }
            if pyrust_builtins::bound_method::is_method_wrapper(value) {
                return Value::builtin_function("method-wrapper");
            }
            if pyrust_builtins::generic_alias::is_generic_alias(value) {
                return Value::py_class(generic_alias_class_singleton());
            }
            // Issue #3000: `slice` is a real class in CPython, so `type(a[1:2])`
            // must be `<class 'slice'>` rather than a `BuiltinFunction` token.
            if pyrust_builtins::slice::is_slice_ops(ops) {
                return Value::py_class(BuiltinTypeClass::Slice.singleton());
            }
            Value::builtin_function(ops.type_name())
        }
        ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Str(_)
        | ValueKind::List(_)
        | ValueKind::Tuple(_)
        | ValueKind::Dict(_)
        | ValueKind::Set(_)
        | ValueKind::Bytes(_)
        | ValueKind::Complex(_, _)
        | ValueKind::None
        | ValueKind::NotImplemented
        | ValueKind::Ellipsis
        | ValueKind::Range { .. }
        | ValueKind::BigRange { .. }
        | ValueKind::PyInstance(_) => {
            unreachable!("primitive_class_for_value should have handled this variant")
        }
    }
}

/// The `PyClass` that `type(value)` yields, or `None` when this value's type is
/// still modelled as a `BuiltinFunction` name token (`generator`,
/// `list_iterator`, `module`, …).
///
/// Lets `issubclass`-style class walks share [`value_class`]'s mapping instead
/// of maintaining a second, drifting copy of it.
pub(crate) fn value_class_object(value: &Value) -> Option<Rc<RefCell<PyClass>>> {
    match value_class(value).kind() {
        ValueKind::PyClass(class) => Some(Rc::clone(class)),
        _ => None,
    }
}
