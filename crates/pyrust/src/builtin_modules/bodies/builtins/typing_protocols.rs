use pyrust_derive::pyrust_module;

pyrust_module! {
    /// Construct `typing.TypeAliasType(name, value, *, type_params=())`.
    #[py_name = "builtins.TypeAliasType.__init__"]
    fn type_alias_type_init(args) -> Result<Value> {
        let _ = _interp;
        let instance = match args.first().map(|arg| arg.value.kind()) {
            Some(ValueKind::PyInstance(instance)) => instance.clone(),
            _ => {
                return Err(PyError::runtime(
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

    /// `ParamSpec(name, *, bound=None, covariant=False, contravariant=False, infer_variance=False)`.
    #[py_name = "builtins.ParamSpec.__init__"]
    fn paramspec_init(args) -> Result<Value> {
        let instance = match args.first().map(|arg| arg.value.kind()) {
            Some(ValueKind::PyInstance(instance)) => instance.clone(),
            _ => {
                return Err(PyError::runtime(
                    "internal: ParamSpec.__init__ self must be a PyInstance".to_string(),
                ));
            }
        };
        let supplied = &args[1..];
        let positional: Vec<&ExpandedCallArg> =
            supplied.iter().filter(|arg| arg.name.is_none()).collect();
        if positional.len() > 1 {
            return Err(pyrust_core::type_err!(
                "paramspec() takes exactly 1 positional argument ({} given)",
                positional.len()
            ));
        }

        let mut name = positional.first().map(|arg| arg.value.clone());
        let mut bound = Value::none();
        let mut covariant = false;
        let mut contravariant = false;
        let mut infer_variance = false;

        for arg in supplied.iter().filter(|arg| arg.name.is_some()) {
            let keyword = arg.name.as_deref().unwrap_or_default();
            match keyword {
                "name" => {
                    if name.is_some() {
                        return Err(pyrust_core::type_err!(
                            "argument for paramspec() given by name ('name') and position (1)"
                        ));
                    }
                    name = Some(arg.value.clone());
                }
                "bound" => bound = arg.value.clone(),
                "covariant" => covariant = _interp.truthy_value(&arg.value)?,
                "contravariant" => contravariant = _interp.truthy_value(&arg.value)?,
                "infer_variance" => infer_variance = _interp.truthy_value(&arg.value)?,
                other => {
                    return Err(pyrust_core::type_err!(
                        "'{other}' is an invalid keyword argument for paramspec()"
                    ));
                }
            }
        }

        let name = name.ok_or_else(|| {
            pyrust_core::type_err!("paramspec() missing required argument 'name' (pos 1)")
        })?;
        let name = match name.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(pyrust_core::type_err!(
                    "paramspec() argument 'name' must be str, not {}",
                    pyrust_core::builtin_type_name(&name)
                ));
            }
        };
        if covariant && contravariant {
            return Err(PyError::named(
                "ValueError",
                "Bivariant types are not supported.".to_string(),
            ));
        }
        if infer_variance && (covariant || contravariant) {
            return Err(PyError::named(
                "ValueError",
                "Variance cannot be specified with infer_variance.".to_string(),
            ));
        }

        let bound = if bound.is_none() {
            crate::interpreter::primitive_class_by_name("NoneType")
                .map(Value::py_class)
                .unwrap_or_else(Value::none)
        } else {
            bound
        };
        let mut borrowed = instance.borrow_mut();
        borrowed.attrs.insert("__name__", Value::string(name));
        borrowed.attrs.insert("__bound__", bound);
        borrowed
            .attrs
            .insert("__module__", Value::string("__main__"));
        borrowed
            .attrs
            .insert("__covariant__", Value::bool_(covariant));
        borrowed
            .attrs
            .insert("__contravariant__", Value::bool_(contravariant));
        borrowed
            .attrs
            .insert("__infer_variance__", Value::bool_(infer_variance));
        Ok(Value::none())
    }

    #[py_name = "builtins.ParamSpec.__repr__"]
    fn paramspec_repr(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "ParamSpec")
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

    #[py_name = "builtins.ParamSpec._get_args"]
    fn paramspec_get_args(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("args", "ParamSpec")
        })?;
        Ok(crate::interpreter::make_paramspec_args_instance(self_val))
    }

    #[py_name = "builtins.ParamSpec._get_kwargs"]
    fn paramspec_get_kwargs(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("kwargs", "ParamSpec")
        })?;
        Ok(crate::interpreter::make_paramspec_kwargs_instance(self_val))
    }

    #[py_name = "builtins.ParamSpec.__or__"]
    fn paramspec_or(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__or__", "ParamSpec")
        })?;
        let other = args.get(1).map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::type_err!("__or__ expected 1 argument, got 0")
        })?;
        let typing_mod = _interp.load_module("typing")?;
        let union = _interp.get_attr(&typing_mod, "Union")?;
        _interp.eval_index(&union, Value::tuple(vec![self_val, other]))
    }

    #[py_name = "builtins.ParamSpec.__ror__"]
    fn paramspec_ror(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__ror__", "ParamSpec")
        })?;
        let other = args.get(1).map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::type_err!("__ror__ expected 1 argument, got 0")
        })?;
        let typing_mod = _interp.load_module("typing")?;
        let union = _interp.get_attr(&typing_mod, "Union")?;
        _interp.eval_index(&union, Value::tuple(vec![other, self_val]))
    }

    #[py_name = "builtins.ParamSpecArgs.__init__"]
    fn paramspec_args_init(args) -> Result<Value> {
        let _ = _interp;
        let instance = match args.first().map(|arg| arg.value.kind()) {
            Some(ValueKind::PyInstance(instance)) => instance.clone(),
            _ => {
                return Err(PyError::runtime(
                    "internal: ParamSpecArgs.__init__ self must be a PyInstance".to_string(),
                ));
            }
        };
        let origin = args.get(1).map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::type_err!("ParamSpecArgs() missing required argument 'origin'")
        })?;
        instance.borrow_mut().attrs.insert("__origin__", origin);
        Ok(Value::none())
    }

    #[py_name = "builtins.ParamSpecArgs.__repr__"]
    fn paramspec_args_repr(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "ParamSpecArgs")
        })?;
        if let ValueKind::PyInstance(inst_rc) = self_val.kind()
            && let Some(origin) = inst_rc.borrow().attrs.get("__origin__").cloned()
            && let ValueKind::PyInstance(orig_rc) = origin.kind()
            && let Some(name) = orig_rc.borrow().attrs.get("__name__").and_then(Value::as_str)
        {
            return Ok(Value::string(format!("{name}.args")));
        }
        Ok(Value::string("ParamSpecArgs"))
    }

    #[py_name = "builtins.ParamSpecArgs.__eq__"]
    fn paramspec_args_eq(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__eq__", "ParamSpecArgs")
        })?;
        let other = args.get(1).map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::type_err!("__eq__ expected 1 argument, got 0")
        })?;
        let (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) =
            (self_val.kind(), other.kind())
        else {
            return Ok(Value::not_implemented());
        };
        if !crate::interpreter::is_paramspec_args_class(&b.borrow().class) {
            return Ok(Value::not_implemented());
        }
        let orig_a = a.borrow().attrs.get("__origin__").cloned();
        let orig_b = b.borrow().attrs.get("__origin__").cloned();
        let same = match (orig_a, orig_b) {
            (Some(oa), Some(ob)) => match (oa.kind(), ob.kind()) {
                (ValueKind::PyInstance(ra), ValueKind::PyInstance(rb)) => {
                    std::rc::Rc::ptr_eq(ra, rb)
                }
                _ => oa == ob,
            },
            _ => false,
        };
        Ok(Value::bool_(same))
    }

    #[py_name = "builtins.ParamSpecKwargs.__init__"]
    fn paramspec_kwargs_init(args) -> Result<Value> {
        let _ = _interp;
        let instance = match args.first().map(|arg| arg.value.kind()) {
            Some(ValueKind::PyInstance(instance)) => instance.clone(),
            _ => {
                return Err(PyError::runtime(
                    "internal: ParamSpecKwargs.__init__ self must be a PyInstance".to_string(),
                ));
            }
        };
        let origin = args.get(1).map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::type_err!("ParamSpecKwargs() missing required argument 'origin'")
        })?;
        instance.borrow_mut().attrs.insert("__origin__", origin);
        Ok(Value::none())
    }

    #[py_name = "builtins.ParamSpecKwargs.__repr__"]
    fn paramspec_kwargs_repr(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "ParamSpecKwargs")
        })?;
        if let ValueKind::PyInstance(inst_rc) = self_val.kind()
            && let Some(origin) = inst_rc.borrow().attrs.get("__origin__").cloned()
            && let ValueKind::PyInstance(orig_rc) = origin.kind()
            && let Some(name) = orig_rc.borrow().attrs.get("__name__").and_then(Value::as_str)
        {
            return Ok(Value::string(format!("{name}.kwargs")));
        }
        Ok(Value::string("ParamSpecKwargs"))
    }

    #[py_name = "builtins.ParamSpecKwargs.__eq__"]
    fn paramspec_kwargs_eq(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__eq__", "ParamSpecKwargs")
        })?;
        let other = args.get(1).map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::type_err!("__eq__ expected 1 argument, got 0")
        })?;
        let (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) =
            (self_val.kind(), other.kind())
        else {
            return Ok(Value::not_implemented());
        };
        if !crate::interpreter::is_paramspec_kwargs_class(&b.borrow().class) {
            return Ok(Value::not_implemented());
        }
        let orig_a = a.borrow().attrs.get("__origin__").cloned();
        let orig_b = b.borrow().attrs.get("__origin__").cloned();
        let same = match (orig_a, orig_b) {
            (Some(oa), Some(ob)) => match (oa.kind(), ob.kind()) {
                (ValueKind::PyInstance(ra), ValueKind::PyInstance(rb)) => {
                    std::rc::Rc::ptr_eq(ra, rb)
                }
                _ => oa == ob,
            },
            _ => false,
        };
        Ok(Value::bool_(same))
    }

    /// `TypeVarTuple(name)`.
    #[py_name = "builtins.TypeVarTuple.__init__"]
    fn typevartuple_init(args) -> Result<Value> {
        let _ = _interp;
        let instance = match args.first().map(|arg| arg.value.kind()) {
            Some(ValueKind::PyInstance(instance)) => instance.clone(),
            _ => {
                return Err(PyError::runtime(
                    "internal: TypeVarTuple.__init__ self must be a PyInstance".to_string(),
                ));
            }
        };
        let supplied = &args[1..];
        if supplied.len() > 1 {
            return Err(pyrust_core::type_err!(
                "typevartuple() takes at most 1 argument ({} given)",
                supplied.len()
            ));
        }
        let mut name = None;
        for arg in supplied {
            match arg.name.as_deref() {
                None | Some("name") => name = Some(arg.value.clone()),
                Some(other) => {
                    return Err(pyrust_core::type_err!(
                        "'{other}' is an invalid keyword argument for typevartuple()"
                    ));
                }
            }
        }
        let name = name.ok_or_else(|| {
            pyrust_core::type_err!("typevartuple() missing required argument 'name' (pos 1)")
        })?;
        let name = match name.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(pyrust_core::type_err!(
                    "typevartuple() argument 'name' must be str, not {}",
                    pyrust_core::builtin_type_name(&name)
                ));
            }
        };
        let mut borrowed = instance.borrow_mut();
        borrowed.attrs.insert("__name__", Value::string(name));
        borrowed
            .attrs
            .insert("__module__", Value::string("__main__"));
        Ok(Value::none())
    }

    #[py_name = "builtins.TypeVarTuple.__repr__"]
    fn typevartuple_repr(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "TypeVarTuple")
        })?;
        if let ValueKind::PyInstance(inst_rc) = self_val.kind()
            && let Some(name_val) = inst_rc.borrow().attrs.get("__name__")
        {
            return Ok(Value::string(name_val.to_string()));
        }
        Ok(Value::string(self_val.repr_raw()))
    }

    #[py_name = "builtins.TypeVarTuple.__iter__"]
    fn typevartuple_iter(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__iter__", "TypeVarTuple")
        })?;
        let typing_mod = _interp.load_module("typing")?;
        let unpack = _interp.get_attr(&typing_mod, "Unpack")?;
        let unpacked = _interp.eval_index(&unpack, self_val)?;
        crate::interpreter::make_iterator(_interp, &Value::tuple(vec![unpacked]))
    }

    /// Build the implicit `typing.Generic[...]` base for a PEP 695 generic
    /// class (`class C[T, *Ts, **P]:`).  Unpacks any `TypeVarTuple` parameters
    /// via `typing.Unpack[Ts]` so `C.__orig_bases__` matches CPython 3.12, and
    /// marks the resulting `_GenericAlias` as PEP-695 implicit so
    /// `__mro_entries__` applies CPython's `Protocol`/`NamedTuple`/`TypedDict`
    /// and duplicate-`Generic` rules.
    #[py_name = "builtins._pep695_generic_base"]
    fn pep695_generic_base(args) -> Result<Value> {
        let params_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            PyError::runtime("internal: _pep695_generic_base missing type_params".to_string())
        })?;
        let typing_mod = _interp.load_module("typing")?;
        let generic = _interp.get_attr(&typing_mod, "Generic")?;
        let unpack = _interp.get_attr(&typing_mod, "Unpack")?;
        let items: Vec<Value> = match params_val.kind() {
            ValueKind::Tuple(ts) => ts.to_vec(),
            _ => vec![params_val.clone()],
        };
        let mut generic_args: Vec<Value> = Vec::with_capacity(items.len());
        for item in items {
            let is_tvt = matches!(
                item.kind(),
                ValueKind::PyInstance(inst)
                    if crate::interpreter::is_typevartuple_class(&inst.borrow().class)
            );
            if is_tvt {
                generic_args.push(_interp.eval_index(&unpack, item)?);
            } else {
                generic_args.push(item);
            }
        }
        let subscript = if generic_args.len() == 1 {
            generic_args.into_iter().next().unwrap()
        } else {
            Value::tuple(generic_args)
        };
        let alias = _interp.eval_index(&generic, subscript)?;
        if let ValueKind::PyInstance(inst) = alias.kind() {
            inst.borrow_mut()
                .attrs
                .insert("_pep695_implicit", Value::bool_(true));
        }
        Ok(alias)
    }
}
