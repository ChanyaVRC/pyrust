// Derived rich-comparison support for `total_ordering`.

/// Ordering operations in CPython's descending string-priority order.
const ORDERING_OPS: [&str; 4] = ["__lt__", "__le__", "__gt__", "__ge__"];

fn ordering_op_is_root(class: &Rc<RefCell<PyClass>>, op: &str) -> bool {
    match lookup_class_attr(class, op) {
        None => false,
        Some(value) => !matches!(
            value.kind(),
            ValueKind::BuiltinFunction(name) if name == format!("object.{op}")
        ),
    }
}

fn apply_total_ordering(interp: &mut Interpreter, class: &Rc<RefCell<PyClass>>) -> Result<()> {
    if Rc::ptr_eq(class, &object_class_singleton()) {
        return Err(PyError::named(
            "ValueError",
            "must define at least one ordering operation: < > <= >=".to_string(),
        ));
    }
    let root = ORDERING_OPS
        .into_iter()
        .find(|op| ordering_op_is_root(class, op));
    let Some(root) = root else {
        return Err(PyError::named(
            "ValueError",
            "must define at least one ordering operation: < > <= >=".to_string(),
        ));
    };

    let source = derivation_source(root);
    let namespace = Value::dict(PyDict::default());
    interp.exec_source(source, Some(namespace.clone()), None)?;
    for (operation, _) in convert_table(root) {
        if !ordering_op_is_root(class, operation) {
            let function = namespace
                .as_dict()
                .and_then(|dict| dict.get(&PyKey::str_from(operation)).cloned())
                .ok_or_else(|| internal("total_ordering"))?;
            class
                .borrow_mut()
                .attrs
                .insert(operation.to_string(), function);
        }
    }
    class.borrow().bump_mutation_version();
    // The decorator mutates an existing class after subclasses may already
    // have populated inherited-method caches.  The receiver's local version
    // invalidates caches keyed by this class; the global epoch is required for
    // caches whose receiver is a subclass but whose resolved method came from
    // this newly changed base.
    pyrust_core::bump_class_epoch();
    Ok(())
}

fn convert_table(root: &str) -> &'static [(&'static str, &'static str)] {
    match root {
        "__lt__" => &[
            ("__gt__", "__lt__"),
            ("__le__", "__lt__"),
            ("__ge__", "__lt__"),
        ],
        "__le__" => &[
            ("__ge__", "__le__"),
            ("__lt__", "__le__"),
            ("__gt__", "__le__"),
        ],
        "__gt__" => &[
            ("__lt__", "__gt__"),
            ("__ge__", "__gt__"),
            ("__le__", "__gt__"),
        ],
        _ => &[
            ("__le__", "__ge__"),
            ("__gt__", "__ge__"),
            ("__lt__", "__ge__"),
        ],
    }
}

fn derivation_source(root: &str) -> &'static str {
    match root {
        "__lt__" => {
            "\
def __gt__(self, other):
    op_result = type(self).__lt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result and self != other
def __le__(self, other):
    op_result = type(self).__lt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result or self == other
def __ge__(self, other):
    op_result = type(self).__lt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result
"
        }
        "__le__" => {
            "\
def __ge__(self, other):
    op_result = type(self).__le__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result or self == other
def __lt__(self, other):
    op_result = type(self).__le__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result and self != other
def __gt__(self, other):
    op_result = type(self).__le__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result
"
        }
        "__gt__" => {
            "\
def __lt__(self, other):
    op_result = type(self).__gt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result and self != other
def __ge__(self, other):
    op_result = type(self).__gt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result or self == other
def __le__(self, other):
    op_result = type(self).__gt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result
"
        }
        _ => {
            "\
def __le__(self, other):
    op_result = type(self).__ge__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result or self == other
def __gt__(self, other):
    op_result = type(self).__ge__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result and self != other
def __lt__(self, other):
    op_result = type(self).__ge__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result
"
        }
    }
}
