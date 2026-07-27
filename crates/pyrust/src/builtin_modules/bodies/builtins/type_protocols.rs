use pyrust_derive::pyrust_module;

pyrust_module! {
    /// Issue #1385: `type.__new__(mcs, name, bases, namespace)` — the metaclass
    /// allocator. Creates a new `PyClass` from the given arguments. Called when
    /// `super().__new__(mcs, name, bases, namespace)` is used inside a custom
    /// metaclass `__new__` method.
    ///
    /// CPython signature: `type.__new__(cls, name, bases, dict, /)`
    #[py_name = "type.__new__"]
    fn type_new_dunder(args) -> Result<Value> {
        // type.__new__ has two call signatures:
        //   type.__new__(mcs, name, bases, namespace, **kwds)  — metaclass alloc
        //   type(obj)                                          — returns type(obj)
        // The one-arg form is handled by the "type" registry entry, not here.
        // The 4 positional args are mcs + name + bases + namespace; any extra
        // keyword args are the PEP 487 class kwargs that get forwarded to
        // `__init_subclass__` (CPython's type.__new__ accepts **kwds).
        let positional: Vec<&ExpandedCallArg> =
            args.iter().filter(|a| a.name.is_none()).collect();
        let init_subclass_kwargs: Vec<ExpandedCallArg> = args
            .iter()
            .filter(|a| a.name.is_some())
            .cloned()
            .collect();
        if positional.len() != 4 {
            // CPython counts positional args excluding the implicit cls arg, so
            // the error says "3 arguments" (name, bases, dict) not "4".
            // With 0 args (no cls at all), CPython uses a different message.
            if positional.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "type.__new__(): not enough arguments".to_string(),
                ));
            }
            return Err(PyError::named(
                "TypeError",
                format!(
                    "type.__new__() takes exactly 3 arguments ({} given)",
                    positional.len() - 1,
                ),
            ));
        }
        let mcs_val = positional[0].value.clone();
        let name_val = positional[1].value.clone();
        let bases_val = positional[2].value.clone();
        let namespace_val = positional[3].value.clone();

        let mcs_rc = match mcs_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "type.__new__(): first argument must be a type".to_string(),
                ));
            }
        };
        let name = match name_val.as_str() {
            Some(s) => s.to_string(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "type.__new__(): second argument must be a str".to_string(),
                ));
            }
        };
        // Parse bases tuple/list into Vec<Rc<RefCell<PyClass>>>.
        let bases: Vec<Value> = if let Some(tuple) = bases_val.as_tuple() {
            tuple.to_vec()
        } else {
            bases_val
                .as_list()
                .map(|list| list.to_vec())
                .unwrap_or_default()
        };
        let mut base: Option<Rc<RefCell<PyClass>>> = None;
        let mut extra_bases: Vec<Rc<RefCell<PyClass>>> = Vec::new();
        for (i, b) in bases.iter().enumerate() {
            match b.kind() {
                ValueKind::PyClass(c) => {
                    if i == 0 {
                        base = Some(Rc::clone(c));
                    } else {
                        extra_bases.push(Rc::clone(c));
                    }
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "type.__new__(): bases must be types".to_string(),
                    ));
                }
            }
        }
        // Issue #1677 / #2109: reject bases with incompatible instance layouts
        // (two C-level primitive bases, or two non-empty `__slots__` bases).
        {
            let all_bases: Vec<_> = base.iter().chain(extra_bases.iter()).cloned().collect();
            if crate::interpreter::bases_have_layout_conflict(&all_bases) {
                return Err(PyError::named(
                    "TypeError",
                    "multiple bases have instance lay-out conflict".to_string(),
                ));
            }
        }
        // Build attrs from the namespace dict.
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        if let Some(map) = namespace_val.as_dict() {
            for (k, v) in map.iter() {
                if let PyKey::Str(s) = k
                    && let Some(key_str) = s.as_str() {
                        attrs.insert(key_str.to_string(), v.clone());
                    }
            }
        }
        // Issue #1626: record the actual metatype on the class so that
        // `type(Foo)` returns the metaclass and `isinstance(Foo, Meta)` works.
        // `type` itself is the default metatype and is represented as `None`
        // to avoid a circular Rc; only a custom metaclass is stored explicitly.
        let metatype = {
            let type_class = type_class_singleton();
            if Rc::ptr_eq(&mcs_rc, &type_class) {
                None
            } else {
                Some(mcs_rc)
            }
        };
        // Issues #2129 / #2130: run the full class-creation finalization
        // (__module__, __slots__, __set_name__, __init_subclass__) so a class
        // built via a metaclass / `type.__new__` matches the `class` statement.
        // The `class`-statement metaclass path now also routes here exactly
        // once (via `exec_make_class_meta`), so hooks fire once, not twice.
        _interp.build_class_via_type(name, base, extra_bases, attrs, metatype, &init_subclass_kwargs)
    }

    /// Issue #1385: `type.__init__(cls, name, bases, namespace)` — the
    /// metaclass initialiser.  In CPython `type.__init__` is effectively a
    /// no-op (the real work happens in `type.__new__`).  Registering it here
    /// lets `super().__init__(name, bases, namespace)` in a custom metaclass
    /// `__init__` resolve and terminate cleanly instead of raising
    /// `AttributeError: super(): parent class has no attribute '__init__'`.
    ///
    /// CPython signature: `type.__init__(cls, name, bases, dict, /)`
    #[py_name = "type.__init__"]
    fn type_init_dunder(_args) -> Result<Value> {
        Ok(Value::none())
    }

    /// Issue #2128: `type.__prepare__(mcs, name, bases, /, **kwds)` — the
    /// default metaclass namespace factory.  CPython exposes this as a
    /// classmethod returning a fresh plain `dict`; a `class` statement (and the
    /// `exec_make_class_meta` path) calls `metaclass.__prepare__(...)` before
    /// running the class body.  Registering it makes `hasattr(type, '__prepare__')`
    /// true and lets `super().__prepare__(...)` resolve in a custom metaclass.
    #[py_name = "type.__prepare__"]
    fn type_prepare_dunder(_args) -> Result<Value> {
        Ok(Value::dict(PyDict::default()))
    }

    /// Issue #1956: `type.__call__(cls, *args, **kwargs)` — the default
    /// instance-construction protocol.  Runs `cls.__new__` + `cls.__init__`.
    /// Reached when `super().__call__(*args)` inside a metaclass `__call__`
    /// override chains (via the metaclass MRO) to the default `type.__call__`
    /// bound to the class being constructed.  This is the same default-construct
    /// path as a plain `Cls()` (both go through `Interpreter::default_construct`).
    ///
    /// CPython signature: `type.__call__(self, /, *args, **kwargs)` where
    /// `self` is the class being instantiated.
    #[py_name = "type.__call__"]
    fn type_call_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                "type.__call__(): not enough arguments".to_string(),
            ));
        }
        let cls_val = args[0].value.clone();
        let class = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "type.__call__(): first argument must be a type".to_string(),
                ));
            }
        };
        _interp.default_construct(class, &args[1..])
    }
}
