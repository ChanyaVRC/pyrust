/// Construct the runtime `mappingproxy` type exposed as
/// `types.MappingProxyType`. The `types` module exports the class identity;
/// callable validation belongs to the built-in constructor boundary.
fn construct_mapping_proxy(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "mappingproxy() takes at most 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let mapping = match args.first() {
        Some(argument)
            if argument
                .name
                .as_deref()
                .is_none_or(|name| name == "mapping") =>
        {
            &argument.value
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                "mappingproxy() missing required argument 'mapping' (pos 1)".to_string(),
            ));
        }
    };
    // A plain dict is proxied directly — the proxy's source *is* the proxied
    // object, so there is nothing to delegate `repr` / `copy` to.
    if let Some(dictionary) = mapping.get_dict_rc() {
        return Ok(pyrust_builtins::mapping_proxy::mapping_proxy_dict(
            Rc::clone(dictionary),
        ));
    }
    // CPython retains a nested mappingproxy as the proxied object; reads then
    // forward through each wrapper in turn rather than flattening the chain.
    if pyrust_builtins::mapping_proxy::is_mapping_proxy(mapping) {
        return Ok(pyrust_builtins::mapping_proxy::mapping_proxy_object(
            mapping.clone(),
        ));
    }

    // mappingproxy_new_impl follows PyMapping_Check: a subscript slot is enough
    // (even when its value is None), except that list and tuple — including
    // subclasses — are explicitly refused. Reads stay attached to the object,
    // so dict subclasses keep their Python overrides and non-dict mappings do
    // not need a discoverable backing store.
    if !mapping_proxy_rejects_sequence(mapping) && mapping_proxy_has_subscript(mapping) {
        return Ok(pyrust_builtins::mapping_proxy::mapping_proxy_object(
            mapping.clone(),
        ));
    }
    let argument_type = if let Some(type_name) =
        crate::interpreter::mapping_proxy_typing_rejection_type_name(mapping)
    {
        std::borrow::Cow::Borrowed(type_name)
    } else {
        mapping_proxy_class(mapping)
            .filter(crate::interpreter::is_canonical_deque_class)
            .map_or_else(
                || value_type_name_str(mapping),
                |_| std::borrow::Cow::Borrowed("collections.deque"),
            )
    };
    Err(PyError::named(
        "TypeError",
        format!("mappingproxy() argument must be a mapping, not {argument_type}"),
    ))
}

fn mapping_proxy_class(value: &Value) -> Option<Rc<RefCell<PyClass>>> {
    let class = value_class(value);
    match class.kind() {
        ValueKind::PyClass(class) => Some(Rc::clone(class)),
        _ => None,
    }
}

fn mapping_proxy_rejects_sequence(value: &Value) -> bool {
    let Some(class) = mapping_proxy_class(value) else {
        return false;
    };
    crate::interpreter::PRIMITIVE_CLASSES.with(|classes| {
        class_is_subclass_of(&class, &classes.list_class)
            || class_is_subclass_of(&class, &classes.tuple_class)
    })
}

fn mapping_proxy_has_subscript(value: &Value) -> bool {
    if let Some(accepted) = crate::interpreter::mapping_proxy_typing_subscript_policy(value) {
        return accepted;
    }
    // These built-in values carry CPython mapping slots even though their
    // visible singleton classes do not advertise __getitem__. The slot raises
    // "is not a generic class" when called, but PyMapping_Check still accepts
    // the object, so MappingProxyType must retain it as an authoritative owner.
    if pyrust_builtins::generic_alias::is_generic_alias(value)
        || pyrust_builtins::union_type::is_union_type(value)
    {
        return true;
    }
    mapping_proxy_class(value).is_some_and(|class| mapping_proxy_class_has_subscript(&class))
}

/// Model the part of PyMapping_Check that matters to MappingProxyType.
///
/// A Python class definition of __getitem__ installs a mapping subscript slot,
/// even when the value is None. Native deque is the one implemented PyRust
/// owner whose __getitem__ is sequence-only. A subclass override therefore
/// qualifies, and multiple inheritance still qualifies when a later base
/// contributes a mapping slot even if deque wins ordinary attribute lookup.
fn mapping_proxy_class_has_subscript(class: &Rc<RefCell<PyClass>>) -> bool {
    let (owns_getitem, bases) = {
        let class_ref = class.borrow();
        let mut bases =
            Vec::with_capacity(usize::from(class_ref.base.is_some()) + class_ref.extra_bases.len());
        if let Some(base) = &class_ref.base {
            bases.push(Rc::clone(base));
        }
        bases.extend(class_ref.extra_bases.iter().cloned());
        (class_ref.attrs.contains_key("__getitem__"), bases)
    };
    if owns_getitem {
        return !crate::interpreter::is_canonical_deque_class(class);
    }
    bases.iter().any(mapping_proxy_class_has_subscript)
}
