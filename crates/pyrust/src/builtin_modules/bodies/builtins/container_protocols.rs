use pyrust_derive::pyrust_module;

pyrust_module! {
    /// Issue #988: `list.__init__(self[, iterable])` — resets the backing
    /// store of a list subclass instance.  With no iterable arg the backing
    /// is reset to an empty list (matching CPython where `list.__init__(x)`
    /// clears `x`); with an iterable arg the backing is rebuilt from it.
    ///
    /// CPython signature: `list.__init__(self, iterable=())`
    #[py_name = "list.__init__"]
    fn list_init(args) -> Result<Value> {
        let Some(first) = args.first() else {
            return Ok(Value::none());
        };
        let inst_rc = match first.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => return Ok(Value::none()),
        };
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("list expected at most 1 argument, got {}", args.len() - 1),
            ));
        }
        if instance_builtin_data(&inst_rc).is_some() {
            let list_dispatch = crate::builtin_registry::lookup("list").ok_or_else(|| {
                PyError::Runtime("internal: list constructor not in registry".to_string())
            })?;
            // Pass the iterable arg if present, or empty args to get an empty list.
            let new_backing = list_dispatch(_interp, args.get(1).map_or(&[], std::slice::from_ref))?;
            inst_rc.borrow_mut().attrs.insert("__builtin_data__", new_backing);
        }
        Ok(Value::none())
    }

    /// Issue #988: `dict.__init__(self[, mapping_or_iterable][, **kwargs])` —
    /// updates the backing store of a dict subclass instance.  With no args
    /// beyond self the existing entries remain unchanged; with args, completed
    /// source entries are merged into the live backing before a later source
    /// error, matching CPython's in-place `dict.__init__` behaviour.
    ///
    /// CPython signature: `dict.__init__(self, *args, **kwargs)`
    #[py_name = "dict.__init__"]
    fn dict_init(args) -> Result<Value> {
        let Some(first) = args.first() else {
            return Ok(Value::none());
        };
        let inst_rc = match first.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => return Ok(Value::none()),
        };
        let pos_count = args[1..].iter().filter(|a| a.name.is_none()).count();
        if pos_count > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("dict expected at most 1 argument, got {}", pos_count),
            ));
        }
        if let Some(backing) = instance_builtin_data(&inst_rc) {
            let mut positional = Vec::with_capacity(pos_count);
            let mut kwargs = PyDict::default();
            for arg in &args[1..] {
                if let Some(name) = &arg.name {
                    kwargs.insert(PyKey::str_from(name), arg.value.clone());
                } else {
                    positional.push(arg.value.clone());
                }
            }
            // Reuse the canonical interpreter-aware update path instead of
            // constructing a detached temporary dict.  Besides preserving
            // existing entries, this commits mapping / iterable prefixes
            // before a later lookup or iteration error and snapshots
            // `d.__init__(d)` aliases safely.
            return _interp.call_dict_method("update", backing, positional, &kwargs);
        }
        Ok(Value::none())
    }

    /// Issue #1134: `dict.__getitem__(self, key)` — native dict subscript for
    /// dict subclasses.  Called via `super().__getitem__(key)` when the
    /// subclass routes through the SuperProxy mechanism.  Performs the raw
    /// backing-dict lookup and honours `__missing__` when the key is absent.
    ///
    /// CPython signature: `dict.__getitem__(self, key)`
    #[py_name = "dict.__getitem__"]
    fn dict_getitem(args) -> Result<Value> {
        // CPython exposes dict.__getitem__ as a *method_descriptor* (#2266):
        // missing receiver -> "unbound method ...", wrong receiver type ->
        // "descriptor ... doesn't apply to a '<X>' object".  The receiver check
        // happens before the arity check, matching CPython's slot ordering.
        let self_arg = args.first().ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getitem__", "dict", method)
        })?;
        let inst_rc = match self_arg.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => {
                let actual = pyrust_core::builtin_type_name(&self_arg.value);
                return Err(pyrust_core::descriptor_requires!(
                    "__getitem__", "dict", actual, method
                ));
            }
        };
        let key = match args.get(1) {
            Some(key_arg) => key_arg.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "dict.__getitem__ expected 2 arguments".to_string(),
                ));
            }
        };
        let backing = instance_builtin_data(&inst_rc).ok_or_else(|| {
            pyrust_core::descriptor_requires!("__getitem__", "dict")
        })?;
        // #2657: a PyInstance receiver whose base is not `dict` (e.g. a list
        // subclass) must be rejected with CPython's method_descriptor wording
        // instead of reaching the dict-lookup helper and tripping its
        // "internal: expected dict" assertion.
        if !matches!(backing.kind(), ValueKind::Dict(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "dict", actual, method
            ));
        }
        let lookup = if let Some(s) = key.as_str() {
            _interp.dict_str_lookup(&backing, s)?
        } else {
            let py_key = _interp.value_to_pykey(&key)?;
            _interp.dict_lookup(&backing, &py_key)?
        };
        match lookup {
            Some((_, v)) => Ok(v),
            None => {
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(missing_fn) = lookup_class_attr(&class, "__missing__") {
                    invoke_class_method(
                        _interp,
                        missing_fn,
                        Value::py_instance(inst_rc),
                        &[ExpandedCallArg { name: None, value: key }],
                    )
                } else {
                    Err(PyError::key_error(key))
                }
            }
        }
    }

    /// Issue #1134 (review): `list.__getitem__(self, key)` — native list subscript
    /// for list subclasses.  Called via `super().__getitem__(key)` from a list
    /// subclass override.  Delegates to `eval_index` on the backing primitive.
    ///
    /// CPython signature: `list.__getitem__(self, key)`
    #[py_name = "list.__getitem__"]
    fn list_getitem(args) -> Result<Value> {
        // CPython exposes list.__getitem__ as a *method_descriptor* (#2266):
        // missing receiver -> "unbound method ...", wrong receiver type ->
        // "descriptor ... doesn't apply to a '<X>' object".  The receiver check
        // happens before the arity check, matching CPython's slot ordering.
        let self_arg = args.first().ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getitem__", "list", method)
        })?;
        if !matches!(self_arg.value.kind(), ValueKind::PyInstance(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "list", actual, method
            ));
        }
        let key = match args.get(1) {
            Some(key_arg) => key_arg.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "list.__getitem__ expected 2 arguments".to_string(),
                ));
            }
        };
        let backing = builtin_data_backing(&self_arg.value).ok_or_else(|| {
            pyrust_core::descriptor_requires!("__getitem__", "list")
        })?;
        // #2657: a PyInstance receiver whose base is not `list` (e.g. a tuple
        // subclass) must be rejected with CPython's method_descriptor wording.
        if !matches!(backing.kind(), ValueKind::List(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "list", actual, method
            ));
        }
        _interp.eval_index(&backing, key)
    }

    /// Issue #1134 (review): `tuple.__getitem__(self, key)` — native tuple subscript
    /// for tuple subclasses.  Called via `super().__getitem__(key)` from a tuple
    /// subclass override.  Delegates to `eval_index` on the backing primitive.
    ///
    /// CPython signature: `tuple.__getitem__(self, key)`
    #[py_name = "tuple.__getitem__"]
    fn tuple_getitem(args) -> Result<Value> {
        // CPython exposes tuple.__getitem__ as a *slot wrapper* (#2266/#2276):
        // missing receiver -> "descriptor '__getitem__' of 'tuple' object needs
        // an argument", wrong receiver type -> "descriptor '__getitem__'
        // requires a 'tuple' object but received a '<X>'".  The receiver check
        // precedes the arity check, matching CPython's slot ordering.
        let self_arg = args.first().ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getitem__", "tuple")
        })?;
        if !matches!(self_arg.value.kind(), ValueKind::PyInstance(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "tuple", actual
            ));
        }
        let key = match args.get(1) {
            Some(key_arg) => key_arg.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "tuple.__getitem__ expected 2 arguments".to_string(),
                ));
            }
        };
        let backing = builtin_data_backing(&self_arg.value).ok_or_else(|| {
            pyrust_core::descriptor_requires!("__getitem__", "tuple")
        })?;
        // #2657: a PyInstance receiver whose base is not `tuple` (e.g. a list
        // subclass) must be rejected with CPython's slot-wrapper wording.
        if !matches!(backing.kind(), ValueKind::Tuple(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "tuple", actual
            ));
        }
        _interp.eval_index(&backing, key)
    }

    /// Issue #1134 (review): `bytes.__getitem__(self, key)` — native bytes subscript
    /// for bytes subclasses.  Called via `super().__getitem__(key)` from a bytes
    /// subclass override.  Delegates to `eval_index` on the backing primitive.
    ///
    /// CPython signature: `bytes.__getitem__(self, key)`
    #[py_name = "bytes.__getitem__"]
    fn bytes_getitem(args) -> Result<Value> {
        // CPython exposes bytes.__getitem__ as a *slot wrapper* (#2266/#2276):
        // missing receiver -> "descriptor '__getitem__' of 'bytes' object needs
        // an argument", wrong receiver type -> "descriptor '__getitem__'
        // requires a 'bytes' object but received a '<X>'".  The receiver check
        // precedes the arity check, matching CPython's slot ordering.
        let self_arg = args.first().ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getitem__", "bytes")
        })?;
        if !matches!(self_arg.value.kind(), ValueKind::PyInstance(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "bytes", actual
            ));
        }
        let key = match args.get(1) {
            Some(key_arg) => key_arg.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "bytes.__getitem__ expected 2 arguments".to_string(),
                ));
            }
        };
        let backing = builtin_data_backing(&self_arg.value).ok_or_else(|| {
            pyrust_core::descriptor_requires!("__getitem__", "bytes")
        })?;
        // #2657: a PyInstance receiver whose base is not `bytes` (e.g. a list
        // subclass) must be rejected with CPython's slot-wrapper wording.
        if !matches!(backing.kind(), ValueKind::Bytes(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "bytes", actual
            ));
        }
        _interp.eval_index(&backing, key)
    }

    /// Issue #988: `set.__init__(self[, iterable])` — resets the backing
    /// store of a set subclass instance.  With no iterable arg the backing
    /// is reset to an empty set; with an iterable arg the backing is rebuilt
    /// from it (matching CPython's clearing + re-populating behaviour).
    ///
    /// CPython signature: `set.__init__(self, iterable=())`
    #[py_name = "set.__init__"]
    fn set_init(args) -> Result<Value> {
        let Some(first) = args.first() else {
            return Ok(Value::none());
        };
        let inst_rc = match first.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => return Ok(Value::none()),
        };
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("set expected at most 1 argument, got {}", args.len() - 1),
            ));
        }
        if instance_builtin_data(&inst_rc).is_some() {
            let set_dispatch = crate::builtin_registry::lookup("set").ok_or_else(|| {
                PyError::Runtime("internal: set constructor not in registry".to_string())
            })?;
            // Pass the iterable arg if present, or empty args to get an empty set.
            let new_backing = set_dispatch(_interp, args.get(1).map_or(&[], std::slice::from_ref))?;
            inst_rc.borrow_mut().attrs.insert("__builtin_data__", new_backing);
        }
        Ok(Value::none())
    }

    /// Issue #1004: `frozenset.__init__` — no-op.  frozenset is immutable; the
    /// backing data is fixed at `__new__` time.  Registering this sentinel
    /// allows `super().__init__()` in a frozenset subclass to resolve without
    /// AttributeError (matching CPython 3.12 where frozenset inherits
    /// object.__init__ which ignores all args when __new__ is overridden).
    ///
    /// CPython signature: `frozenset.__init__(self, *args, **kwargs)`
    #[py_name = "frozenset.__init__"]
    fn frozenset_init(_args) -> Result<Value> {
        Ok(Value::none())
    }

    /// Issue #1004: `tuple.__init__` — no-op.  tuple is immutable; the
    /// backing data is fixed at `__new__` time.  Registering this sentinel
    /// allows `super().__init__()` in a tuple subclass to resolve without
    /// AttributeError.
    ///
    /// CPython signature: `tuple.__init__(self, *args, **kwargs)`
    #[py_name = "tuple.__init__"]
    fn tuple_init(_args) -> Result<Value> {
        Ok(Value::none())
    }
}
