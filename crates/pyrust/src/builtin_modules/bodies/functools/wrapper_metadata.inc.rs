// Metadata propagation for `wraps`, `update_wrapper`, and cached_property.

fn make_wraps_partial(
    generation: &Value,
    wrapped: Value,
    assigned: Option<Value>,
    updated: Option<Value>,
) -> Result<Value> {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__wraps_func", wrapped);
    if let Some(a) = assigned {
        attrs.insert("__wraps_assigned", a);
    }
    if let Some(u) = updated {
        attrs.insert("__wraps_updated", u);
    }
    make_instance(generation, "_wraps_partial", attrs)
}

const WRAPPER_ASSIGNMENTS: [&str; 6] = [
    "__module__",
    "__name__",
    "__qualname__",
    "__doc__",
    "__annotations__",
    "__type_params__",
];

const WRAPPER_UPDATES: [&str; 1] = ["__dict__"];

fn parse_update_wrapper_args(
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<(Value, Value, Option<Value>, Option<Value>)> {
    let mut positional: Vec<Value> = Vec::new();
    let mut wrapper_kw: Option<Value> = None;
    let mut wrapped_kw: Option<Value> = None;
    let mut assigned_kw: Option<Value> = None;
    let mut updated_kw: Option<Value> = None;
    for a in args {
        match a.name.as_deref() {
            Some("wrapper") => wrapper_kw = Some(a.value.clone()),
            Some("wrapped") => wrapped_kw = Some(a.value.clone()),
            Some("assigned") => assigned_kw = Some(a.value.clone()),
            Some("updated") => updated_kw = Some(a.value.clone()),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    if positional.len() > 4 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes from 2 to 4 positional arguments but {} were given",
                positional.len()
            ),
        ));
    }
    let wrapper = wrapper_kw
        .or_else(|| positional.first().cloned())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!("{fn_name}() missing 1 required positional argument: 'wrapper'"),
            )
        })?;
    let wrapped = wrapped_kw
        .or_else(|| positional.get(1).cloned())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!("{fn_name}() missing 1 required positional argument: 'wrapped'"),
            )
        })?;
    let assigned = assigned_kw.or_else(|| positional.get(2).cloned());
    let updated = updated_kw.or_else(|| positional.get(3).cloned());
    Ok((wrapper, wrapped, assigned, updated))
}

fn parse_wraps_args(
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<(Value, Option<Value>, Option<Value>)> {
    let mut positional: Vec<Value> = Vec::new();
    let mut wrapped_kw: Option<Value> = None;
    let mut assigned_kw: Option<Value> = None;
    let mut updated_kw: Option<Value> = None;
    for a in args {
        match a.name.as_deref() {
            Some("wrapped") => wrapped_kw = Some(a.value.clone()),
            Some("assigned") => assigned_kw = Some(a.value.clone()),
            Some("updated") => updated_kw = Some(a.value.clone()),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    if positional.len() > 3 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes from 1 to 3 positional arguments but {} were given",
                positional.len()
            ),
        ));
    }
    let wrapped = wrapped_kw
        .or_else(|| positional.first().cloned())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!("{fn_name}() missing 1 required positional argument: 'wrapped'"),
            )
        })?;
    let assigned = assigned_kw.or_else(|| positional.get(1).cloned());
    let updated = updated_kw.or_else(|| positional.get(2).cloned());
    Ok((wrapped, assigned, updated))
}

fn collect_attr_names(interp: &mut Interpreter, val: &Value) -> Result<Vec<String>> {
    let items = interp.collect_iterable(val)?;
    let mut names = Vec::with_capacity(items.len());
    for item in items {
        match item.kind() {
            ValueKind::Str(s) => names.push(s.to_string()),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "attribute name must be string".to_string(),
                ));
            }
        }
    }
    Ok(names)
}

/// Copy wrapper metadata, merge `__dict__` (or custom `updated`), and set `__wrapped__` last.
fn do_update_wrapper_with(
    interp: &mut Interpreter,
    wrapper: &Value,
    wrapped: &Value,
    assigned: Option<&Value>,
    updated: Option<&Value>,
) -> Result<()> {
    let assign_names: Vec<String> = match assigned {
        Some(val) => collect_attr_names(interp, val)?,
        None => WRAPPER_ASSIGNMENTS
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    };
    for attribute in &assign_names {
        match interp.get_attr(wrapped, attribute) {
            Ok(value) => interp.assign_attr(wrapper.clone(), attribute, value)?,
            Err(error) if error.class_name_is("AttributeError") => {}
            Err(error) => return Err(error),
        }
    }
    let update_names: Vec<String> = match updated {
        Some(val) => collect_attr_names(interp, val)?,
        None => WRAPPER_UPDATES.iter().map(|s| (*s).to_string()).collect(),
    };
    for attr in &update_names {
        if let Ok(destination) = interp.get_attr(wrapper, attr) {
            let source = match interp.get_attr(wrapped, attr) {
                Ok(dict) => dict,
                Err(error) if error.class_name_is("AttributeError") => {
                    Value::dict(PyDict::default())
                }
                Err(error) => return Err(error),
            };
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
    }
    interp.assign_attr(wrapper.clone(), "__wrapped__", wrapped.clone())
}
