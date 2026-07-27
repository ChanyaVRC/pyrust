/// Identify the lightweight `__class_getitem__` sentinels installed on native
/// collection classes. Typing special forms register real callables instead.
pub(crate) fn is_builtin_class_getitem_sentinel(method: &Value) -> bool {
    matches!(
        method.kind(),
        ValueKind::BuiltinFunction(name) if name.ends_with(".__class_getitem__")
    )
}

pub(crate) fn make_builtin_generic_alias(class: Rc<RefCell<PyClass>>, index: Value) -> Value {
    let type_arguments = if matches!(index.kind(), ValueKind::Tuple(_)) {
        index
    } else {
        Value::tuple(vec![index])
    };
    pyrust_builtins::generic_alias::generic_alias(Value::py_class(class), type_arguments)
}
