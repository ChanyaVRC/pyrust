use pyrust_derive::pyrust_module;

pyrust_module! {
    /// Construct `typing.TypeAliasType(name, value, *, type_params=())`.
    #[py_name = "builtins.TypeAliasType.__init__"]
    fn type_alias_type_init(args) -> Result<Value> {
        let _ = _interp;
        let instance = match args.first().map(|arg| arg.value.kind()) {
            Some(ValueKind::PyInstance(instance)) => instance.clone(),
            _ => {
                return Err(PyError::Runtime(
                    "internal: TypeAliasType.__init__ self must be a PyInstance".to_string(),
                ));
            }
        };

        let supplied = &args[1..];
        let positional: Vec<&ExpandedCallArg> =
            supplied.iter().filter(|arg| arg.name.is_none()).collect();
        if positional.len() > 2 {
            return Err(pyrust_core::type_err!(
                "typealias() takes exactly 2 positional arguments ({} given)",
                positional.len()
            ));
        }

        let mut name = positional.first().map(|arg| arg.value.clone());
        let mut value = positional.get(1).map(|arg| arg.value.clone());
        let mut type_params = Value::tuple(vec![]);

        for arg in supplied.iter().filter(|arg| arg.name.is_some()) {
            let keyword = arg.name.as_deref().unwrap_or_default();
            match keyword {
                "name" => {
                    if name.is_some() {
                        return Err(pyrust_core::type_err!(
                            "argument for typealias() given by name ('name') and position (1)"
                        ));
                    }
                    name = Some(arg.value.clone());
                }
                "value" => {
                    if value.is_some() {
                        return Err(pyrust_core::type_err!(
                            "argument for typealias() given by name ('value') and position (2)"
                        ));
                    }
                    value = Some(arg.value.clone());
                }
                "type_params" => type_params = arg.value.clone(),
                other => {
                    return Err(pyrust_core::type_err!(
                        "'{other}' is an invalid keyword argument for typealias()"
                    ));
                }
            }
        }

        let name = name.ok_or_else(|| {
            pyrust_core::type_err!("typealias() missing required argument 'name' (pos 1)")
        })?;
        let value = value.ok_or_else(|| {
            pyrust_core::type_err!("typealias() missing required argument 'value' (pos 2)")
        })?;
        let name = match name.kind() {
            ValueKind::Str(name) => name.to_string(),
            _ => {
                return Err(pyrust_core::type_err!(
                    "typealias() argument 'name' must be str, not {}",
                    pyrust_core::builtin_type_name(&name)
                ));
            }
        };
        if !matches!(type_params.kind(), ValueKind::Tuple(_)) {
            return Err(pyrust_core::type_err!("type_params must be a tuple"));
        }

        let mut borrowed = instance.borrow_mut();
        borrowed.attrs.insert("__name__", Value::string(name));
        borrowed.attrs.insert("__value__", value);
        borrowed.attrs.insert("__type_params__", type_params);
        borrowed
            .attrs
            .insert("__module__", Value::string("__main__"));
        Ok(Value::none())
    }

    /// PEP 695: `TypeAliasType.__repr__` — returns the alias name string.
    /// CPython: `print(Vector)` outputs just `Vector` (the alias name).
    #[py_name = "builtins.TypeAliasType.__repr__"]
    fn type_alias_type_repr(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "TypeAliasType")
        })?;
        if let ValueKind::PyInstance(inst_rc) = self_val.kind() {
            let borrowed = inst_rc.borrow();
            if let Some(name_val) = borrowed.attrs.get("__name__") {
                return Ok(Value::string(name_val.to_string()));
            }
        }
        Ok(Value::string(self_val.repr_raw()))
    }

    /// PEP 695: `TypeAliasType.__getitem__` — subscripting a generic alias
    /// (`Pair.__getitem__(int)`) returns a `types.GenericAlias` with the alias
    /// as origin, matching the operator path in `eval_index`.  A non-generic
    /// alias raises CPython's "Only generic type aliases are subscriptable".
    /// The operator form `Pair[int]` is served by the inline fast path; this
    /// slot exists so `hasattr(alias, "__getitem__")` is True and the explicit
    /// `alias.__getitem__(x)` call works, matching CPython 3.12 (issue #2779).
    #[py_name = "builtins.TypeAliasType.__getitem__"]
    fn type_alias_type_getitem(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getitem__", "TypeAliasType")
        })?;
        let key = args
            .get(1)
            .map(|a| a.value.clone())
            .ok_or_else(|| pyrust_core::type_err!("__getitem__ expected 1 argument, got 0"))?;
        let is_generic_alias = match self_val.kind() {
            ValueKind::PyInstance(inst_rc) => inst_rc
                .borrow()
                .attrs
                .get("__type_params__")
                .is_some_and(|p| matches!(p.kind(), ValueKind::Tuple(t) if !t.is_empty())),
            _ => false,
        };
        if !is_generic_alias {
            return Err(pyrust_core::type_err!(
                "Only generic type aliases are subscriptable"
            ));
        }
        let type_args = if matches!(key.kind(), ValueKind::Tuple(_)) {
            key
        } else {
            Value::tuple(vec![key])
        };
        Ok(pyrust_builtins::generic_alias::generic_alias(
            self_val, type_args,
        ))
    }

    /// `TypeVar.__repr__`: PEP 695 inferred-variance parameters use the bare
    /// name, while manually-created TypeVars use `~`, `+`, or `-`.
    #[py_name = "builtins.TypeVar.__repr__"]
    fn typevar_repr(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "TypeVar")
        })?;
        if let ValueKind::PyInstance(inst_rc) = self_val.kind() {
            let borrowed = inst_rc.borrow();
            if let Some(name_val) = borrowed.attrs.get("__name__") {
                let name = name_val.to_string();
                let flag = |attr: &str| {
                    borrowed
                        .attrs
                        .get(attr)
                        .is_some_and(Value::truthy_raw)
                };
                if flag("__infer_variance__") {
                    return Ok(Value::string(name));
                }
                let prefix = if flag("__covariant__") {
                    '+'
                } else if flag("__contravariant__") {
                    '-'
                } else {
                    '~'
                };
                return Ok(Value::string(format!("{prefix}{name}")));
            }
        }
        Ok(Value::string(self_val.repr_raw()))
    }
}
