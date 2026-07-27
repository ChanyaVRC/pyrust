// Metadata propagation for `wraps`, `update_wrapper`, and cached_property.

fn make_wraps_partial(generation: &Value, wrapped: Value) -> Result<Value> {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__wraps_func", wrapped);
    make_instance(generation, "_wraps_partial", attrs)
}

const WRAPPER_ASSIGNMENTS: [&str; 5] = [
    "__module__",
    "__name__",
    "__qualname__",
    "__annotations__",
    "__doc__",
];

/// Copy wrapper metadata, merge `__dict__`, and set `__wrapped__` last.
fn do_update_wrapper(interp: &mut Interpreter, wrapper: &Value, wrapped: &Value) -> Result<()> {
    for attribute in WRAPPER_ASSIGNMENTS {
        match interp.get_attr(wrapped, attribute) {
            Ok(value) => interp.assign_attr(wrapper.clone(), attribute, value)?,
            Err(error) if error.class_name_is("AttributeError") => {}
            Err(error) => return Err(error),
        }
    }
    if let Ok(destination) = interp.get_attr(wrapper, "__dict__") {
        let source = match interp.get_attr(wrapped, "__dict__") {
            Ok(dict) => dict,
            Err(error) if error.class_name_is("AttributeError") => Value::dict(PyDict::default()),
            Err(error) => return Err(error),
        };
        // Snapshot before mutating the destination. `source` and
        // `destination` may alias, and `as_dict()` now correctly holds a
        // RefCell read guard that cannot overlap `dict_with_mut()`.
        let source_entries = source.as_dict().map(|dict| {
            dict.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>()
        });
        if let Some(source_entries) = source_entries
            && destination.is_dict()
        {
            for (key, value) in source_entries {
                let _ = destination.dict_with_mut(|dict| {
                    dict.insert(key, value);
                });
            }
        }
    }
    interp.assign_attr(wrapper.clone(), "__wrapped__", wrapped.clone())
}

/// Best-effort name extraction used by `cached_property`.
fn function_name(value: &Value) -> Option<String> {
    match value.kind() {
        ValueKind::UserFunction(function) => Some(function.name.to_string()),
        ValueKind::BuiltinFunction(name) => Some(name.to_string()),
        ValueKind::BoundMethod { function, .. } => Some(function.name.to_string()),
        ValueKind::PyClass(class) => Some(class.borrow().name.clone()),
        _ => None,
    }
}
