use pyrust_derive::pyrust_module;

pyrust_module! {
    /// Issue #1254: `object.__getattribute__(self, name)` — the default
    /// attribute lookup used by all instances that do not override
    /// `__getattribute__`.  Performs the standard descriptor protocol (data
    /// descriptor -> instance dict -> non-data descriptor / class attr ->
    /// __getattr__ fallback) without re-invoking the `__getattribute__`
    /// dispatch, so `object.__getattribute__(self, name)` inside a custom
    /// `__getattribute__` terminates the MRO walk cleanly.
    ///
    /// CPython signature: `object.__getattribute__(self, name, /)`
    #[py_name = "object.__getattribute__"]
    fn object_getattribute(args) -> Result<Value> {
        // CPython error messages for argument count mismatches:
        //   0 args: "descriptor '__getattribute__' of 'object' object needs an argument"
        //   1 arg (self only, 0 name args): "expected 1 argument, got 0"
        //   3+ args: "expected 1 argument, got N" where N = args.len() - 1
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__getattribute__", "object"));
        }
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("expected 1 argument, got {}", args.len() - 1),
            ));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                // CPython: "attribute name must be string, not 'TYPE'"
                let type_name = pyrust_core::error_type_name(&args[1].value);
                return Err(PyError::named(
                    "TypeError",
                    format!("attribute name must be string, not '{type_name}'"),
                ));
            }
        };
        let instance_rc = match args[0].value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => {
                return _interp.get_attr(&args[0].value, &name);
            }
        };
        _interp.get_attr_instance_raw(instance_rc, &name)
    }

    /// Issue #1402: `object.__setattr__(self, name, value)` — the default
    /// attribute setter used by all instances that do not override
    /// `__setattr__`.  Performs the descriptor protocol (__set__) then writes
    /// to the instance __dict__, without re-invoking `__setattr__` dispatch
    /// (which would cause infinite recursion when called from inside a custom
    /// `__setattr__`).
    ///
    /// CPython signature: `object.__setattr__(self, name, value, /)`
    #[py_name = "object.__setattr__"]
    fn object_setattr_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__setattr__", "object"));
        }
        if args.len() != 3 {
            return Err(PyError::named(
                "TypeError",
                format!(" expected 2 arguments, got {}", args.len() - 1),
            ));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                let type_name = value_type_name_str(&args[1].value);
                return Err(PyError::named(
                    "TypeError",
                    format!("attribute name must be string, not '{type_name}'"),
                ));
            }
        };
        let value = args[2].value.clone();
        match args[0].value.kind() {
            ValueKind::PyInstance(rc) => {
                let instance_rc = Rc::clone(rc);
                _interp.assign_attr_instance_raw(instance_rc, &name, value)?;
            }
            _ => {
                // CPython raises AttributeError for non-instance values (int, str,
                // list, etc.) — their slots are immutable from Python.  The
                // general assign_attr catch-all returns RuntimeError here, which
                // is wrong; emit the same message CPython does instead.
                let type_name = pyrust_core::error_type_name(&args[0].value);
                return Err(PyError::named(
                    "AttributeError",
                    format!("'{type_name}' object has no attribute '{name}'"),
                ));
            }
        }
        Ok(Value::none())
    }

    /// Issue #1402: `object.__delattr__(self, name)` — the default attribute
    /// deleter used by all instances that do not override `__delattr__`.
    /// Performs the descriptor protocol (__delete__) then removes from the
    /// instance __dict__, without re-invoking `__delattr__` dispatch.
    ///
    /// CPython signature: `object.__delattr__(self, name, /)`
    #[py_name = "object.__delattr__"]
    fn object_delattr_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__delattr__", "object"));
        }
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("expected 1 argument, got {}", args.len() - 1),
            ));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                let type_name = pyrust_core::error_type_name(&args[1].value);
                return Err(PyError::named(
                    "TypeError",
                    format!("attribute name must be string, not '{type_name}'"),
                ));
            }
        };
        match args[0].value.kind() {
            ValueKind::PyInstance(rc) => {
                let instance_rc = Rc::clone(rc);
                _interp.delete_attr_instance_raw(instance_rc, &name)?;
            }
            _ => {
                // CPython raises AttributeError for non-instance values — same
                // pattern as in object_setattr_dunder; the general delete_attr
                // catch-all returns RuntimeError here, which is wrong.
                let type_name = pyrust_core::error_type_name(&args[0].value);
                return Err(PyError::named(
                    "AttributeError",
                    format!("'{type_name}' object has no attribute '{name}'"),
                ));
            }
        }
        Ok(Value::none())
    }
}
