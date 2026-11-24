// `collections.defaultdict` method implementations.
// Shared dict-subclass mapping methods (`__setitem__`, `__contains__`,
// `__len__`, `__iter__`, `get`, `keys`, `values`, `items`) live in `backing.rs`.

fn defaultdict_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    let positional: Vec<&ExpandedCallArg> = user.iter().filter(|a| a.name.is_none()).collect();
    let kwargs: Vec<&ExpandedCallArg> = user.iter().filter(|a| a.name.is_some()).collect();
    let factory = positional
        .first()
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    if !factory.is_none() && !value_is_callable(&factory) {
        return Err(PyError::named(
            "TypeError",
            "first argument must be callable or None".to_string(),
        ));
    }
    let dict_positionals = &positional[positional.len().min(1)..];
    if dict_positionals.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "dict expected at most 1 argument, got {}",
                dict_positionals.len()
            ),
        ));
    }
    inst.borrow_mut()
        .attrs
        .insert_slot("default_factory", factory);
    let backing = defaultdict_backing(args, fn_name)?;
    dict_init_into_backing(
        interp,
        &backing,
        dict_positionals.first().map(|a| &a.value),
        &kwargs,
    )?;
    Ok(Value::none())
}

fn defaultdict_getitem(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let key = require_key_arg(args, 1, fn_name)?;
    let backing = defaultdict_backing(args, fn_name)?;
    if let Some((_, value)) = interp.dict_lookup_value(&backing, key)? {
        return Ok(value);
    }
    let class = Rc::clone(&inst.borrow().class);
    if let Some(missing) = lookup_class_attr(&class, "__missing__") {
        return invoke_class_method(
            interp,
            missing,
            Value::py_instance(inst),
            &[args[1].clone()],
        );
    }
    Err(PyError::key_error(args[1].value.clone()))
}

fn defaultdict_missing(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let key_arg = args
        .get(1)
        .cloned()
        .ok_or_else(|| PyError::Runtime(format!("internal: {fn_name}() missing key arg")))?;
    let factory = inst
        .borrow()
        .attrs
        .get_slot("default_factory")
        .cloned()
        .unwrap_or_else(Value::none);
    if factory.is_none() {
        return Err(PyError::key_error(key_arg.value.clone()));
    }
    let new_val = interp.call_function_expanded(factory, &[])?;
    let backing = defaultdict_backing(args, fn_name)?;
    interp.dict_set_value(&backing, key_arg.value, new_val.clone())?;
    Ok(new_val)
}

fn defaultdict_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let items = read_items(args, fn_name)?;
    let factory_repr = inst
        .borrow()
        .attrs
        .get_slot("default_factory")
        .map(|v| v.repr_raw())
        .unwrap_or_else(|| "None".to_string());
    let body: Vec<String> = items
        .iter()
        .map(|(k, v)| format!("{}: {}", key_repr(k), v.repr_raw()))
        .collect();
    let class_name = inst.borrow().class.borrow().name.clone();
    Ok(Value::string(format!(
        "{class_name}({factory_repr}, {{{}}})",
        body.join(", ")
    )))
}

fn defaultdict_shallow_copy_instance(
    inst: &Rc<RefCell<PyInstance>>,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let items = read_items(args, fn_name)?;
    let factory = inst
        .borrow()
        .attrs
        .get_slot("default_factory")
        .cloned()
        .unwrap_or_else(Value::none);
    let class = Rc::clone(&inst.borrow().class);
    let mut attrs = InstanceAttrs::new();
    attrs.insert_slot("default_factory", factory);
    attrs.set_builtin_data(Value::dict(items));
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

fn defaultdict_copy(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_args(args, "copy")?;
    let inst = expect_self(args, fn_name)?;
    defaultdict_shallow_copy_instance(&inst, args, fn_name)
}

fn defaultdict_dunder_copy(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = validate_collection_no_arg_method(
        args,
        CanonicalCollectionKind::DefaultDict,
        "__copy__",
        "defaultdict",
        "collections.defaultdict",
    )?;
    defaultdict_shallow_copy_instance(&inst, args, fn_name)
}

fn defaultdict_reduce(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = validate_collection_no_arg_method(
        args,
        CanonicalCollectionKind::DefaultDict,
        "__reduce__",
        "defaultdict",
        "collections.defaultdict",
    )?;
    let factory = inst
        .borrow()
        .attrs
        .get_slot("default_factory")
        .cloned()
        .unwrap_or_else(Value::none);
    let ctor_args = if factory.is_none() {
        Value::tuple(Vec::new())
    } else {
        Value::tuple(vec![factory])
    };
    let backing = defaultdict_backing(args, fn_name)?;
    let items_view = crate::Interpreter::dict_view_for_backing(&backing, "items", false)?;
    let item_iter = crate::interpreter::make_iterator(interp, &items_view)?;
    let class = Rc::clone(&inst.borrow().class);
    Ok(Value::tuple(vec![
        Value::py_class(class),
        ctor_args,
        Value::none(),
        Value::none(),
        item_iter,
    ]))
}
