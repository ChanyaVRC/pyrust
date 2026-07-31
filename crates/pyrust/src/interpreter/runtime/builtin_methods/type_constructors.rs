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
    // `mappingproxy(some_proxy)` re-proxies the same live source (CPython
    // accepts a mappingproxy as the mapping argument).
    if let Some(source) = pyrust_builtins::mapping_proxy::source_of(mapping) {
        return Ok(pyrust_builtins::mapping_proxy::mapping_proxy_owned(
            source,
            mapping.clone(),
        ));
    }
    // A dict *subclass* instance — OrderedDict / Counter / defaultdict / a user
    // `class D(dict)` — keeps its entries in a `__builtin_data__` dict.  Proxy
    // that live backing so reads track the subclass, and remember the instance
    // so `repr` and `copy` delegate to it (issue #2936).
    if let Some(dict_rc) = dict_subclass_backing_rc(mapping) {
        return Ok(pyrust_builtins::mapping_proxy::mapping_proxy_owned(
            pyrust_builtins::mapping_proxy::MappingProxySource::Dict(dict_rc),
            mapping.clone(),
        ));
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "mappingproxy() argument must be a mapping, not {}",
            value_type_name_str(mapping)
        ),
    ))
}

/// The live backing `PyDict` of a dict-subclass instance, or `None` if `value`
/// is not one.  A subclass instance that has not stored anything yet has no
/// `__builtin_data__` attribute; install an empty dict so the proxy observes
/// the instance's later mutations instead of a detached snapshot — the same
/// lazy-install the collections bodies do for `Counter` / `defaultdict`.
fn dict_subclass_backing_rc(value: &Value) -> Option<Rc<RefCell<PyDict>>> {
    let instance = value.as_py_instance_rc()?;
    let is_dict_subclass = {
        let class = Rc::clone(&instance.borrow().class);
        crate::interpreter::PRIMITIVE_CLASSES.with(|c| class_is_subclass_of(&class, &c.dict_class))
    };
    if !is_dict_subclass {
        return None;
    }
    if let Some(backing) = instance_builtin_data(instance) {
        return backing.get_dict_rc().cloned();
    }
    let backing = Value::dict(PyDict::default());
    let dict_rc = backing.get_dict_rc().cloned()?;
    instance
        .borrow_mut()
        .attrs
        .insert(BUILTIN_DATA_ATTR, backing);
    Some(dict_rc)
}
