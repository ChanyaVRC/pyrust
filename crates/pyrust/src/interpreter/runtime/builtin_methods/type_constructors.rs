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
    match mapping.get_dict_rc() {
        Some(dictionary) => Ok(pyrust_builtins::mapping_proxy::mapping_proxy_dict(
            Rc::clone(dictionary),
        )),
        None => Err(PyError::named(
            "TypeError",
            format!(
                "mappingproxy() argument must be a mapping, not {}",
                value_type_name_str(mapping)
            ),
        )),
    }
}
