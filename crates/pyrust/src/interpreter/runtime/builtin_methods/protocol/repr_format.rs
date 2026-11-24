// Primitive object-level dunders (`__format__`, `__hash__`, `__repr__`,
// `__str__`) and builtin-subclass in-place wrapper restoration.

#[inline]
pub(super) fn is_mutable_builtin_inplace_dunder(method: &str) -> bool {
    matches!(
        method,
        "__iadd__" | "__imul__" | "__ior__" | "__iand__" | "__isub__" | "__ixor__"
    )
}

/// Restore the Python-visible receiver after an inherited mutable builtin
/// in-place wrapper returned its raw backing value.
///
/// Callers invoke this only after builtin protocol dispatch. The method and
/// identity guards keep user results, `NotImplemented`, ordinary binary
/// dunders, and fresh base-type results untouched.
fn restore_builtin_subclass_inplace_result(
    method: &str,
    receiver: &Value,
    backing: &Value,
    result: Value,
) -> Value {
    if matches!(receiver.kind(), ValueKind::PyInstance(_))
        && is_mutable_builtin_inplace_dunder(method)
        && values_are_identical(&result, backing)
    {
        receiver.clone()
    } else {
        result
    }
}

impl Interpreter {
    /// Dispatch an unregistered primitive-owned object-level dunder sentinel
    /// (`int.__format__`, `str.__format__`, `str.__hash__`, `int.__repr__`, …)
    /// when invoked through the shared slot router (`invoke_class_method`).
    ///
    /// Returns `None` when `registry_key` is not a primitive object dunder.
    pub(crate) fn try_invoke_primitive_object_dunder(
        &mut self,
        registry_key: &str,
        instance: &Value,
        args: &[ExpandedCallArg],
    ) -> Option<Result<Value>> {
        let (type_name, method) = primitive_object_dunder_owner(registry_key)?;
        let mut combined = Vec::with_capacity(args.len() + 1);
        combined.push(ExpandedCallArg {
            name: None,
            value: instance.clone(),
        });
        combined.extend(args.iter().cloned());
        Some(self.call_primitive_object_dunder(type_name, method, &combined))
    }

    /// Dispatch an unbound type-qualified object-level dunder
    /// (`str.__hash__(x)`, `int.__format__(5, 'x')`, …) whose owner is the
    /// primitive `type_name`. Validates the receiver against the called type
    /// and delegates slot wrappers to the object registry or `__format__` to
    /// the shared format implementation.
    pub(super) fn call_primitive_object_dunder(
        &mut self,
        type_name: &str,
        method: &str,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        let is_format = method == "__format__";
        // Receiver presence guard.  Slot wrappers use "descriptor '<m>' of
        // '<type>' object needs an argument"; `__format__` is a method_descriptor
        // ("unbound method <type>.__format__() needs an argument").
        let self_arg = args.first().ok_or_else(|| {
            if is_format {
                pyrust_core::descriptor_needs_arg!(method, type_name, method)
            } else {
                pyrust_core::descriptor_needs_arg!(method, type_name)
            }
        })?;
        // Receiver-type guard: accept a bare primitive of `type_name` or a
        // subclass `PyInstance` whose backing data is that type.
        let recv_ok = match (type_name, self_arg.value.kind()) {
            ("int" | "bool", ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)) => true,
            ("float", ValueKind::Float(_)) => true,
            ("complex", ValueKind::Complex(_, _)) => true,
            ("str", ValueKind::Str(_)) => true,
            ("bytes", ValueKind::Bytes(_)) => true,
            ("bytearray", ValueKind::BuiltinObject { ops, .. })
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
            {
                true
            }
            ("list", ValueKind::List(_)) => true,
            ("tuple", ValueKind::Tuple(_)) => true,
            ("dict", ValueKind::Dict(_)) => true,
            ("set", ValueKind::Set(_)) => true,
            ("frozenset", ValueKind::BuiltinObject { ops, .. })
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
            {
                true
            }
            (_, ValueKind::PyInstance(_)) => builtin_data_backing(&self_arg.value)
                .is_some_and(|b| pyrust_core::builtin_type_name(&b) == type_name),
            _ => false,
        };
        if !recv_ok {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(if is_format {
                pyrust_core::descriptor_requires!(method, type_name, actual, method)
            } else {
                pyrust_core::descriptor_requires!(method, type_name, actual)
            });
        }
        if is_format {
            if args.iter().any(|a| a.name.is_some()) {
                return Err(pyrust_core::type_err!(
                    "{type_name}.__format__() takes no keyword arguments"
                ));
            }
            if args.len() != 2 {
                return Err(pyrust_core::type_err!(
                    "{type_name}.__format__() takes exactly one argument ({} given)",
                    args.len() - 1
                ));
            }
            let spec = match args[1].value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => {
                    return Err(pyrust_core::type_err!(
                        "__format__() argument must be str, not {}",
                        pyrust_core::builtin_type_name(&args[1].value)
                    ));
                }
            };
            if let Some(backing) = builtin_data_backing(&self_arg.value) {
                let owner = value_type_name_str(&self_arg.value);
                return apply_format_spec_named(&backing, &spec, Some(&owner));
            }
            return apply_format_spec(&self_arg.value, &spec);
        }
        // __hash__/__repr__/__str__ take no further positional/keyword args.
        // These are slot wrappers, so CPython's keyword-rejection message uses
        // the anonymous "wrapper <name>() takes no keyword arguments" form
        // (issue #2291), not the type-qualified `<type>.<name>()` form used by
        // the `__format__` method_descriptor above.
        if args.iter().skip(1).any(|a| a.name.is_some()) {
            return Err(pyrust_core::type_err!(
                "wrapper {method}() takes no keyword arguments"
            ));
        }
        // `__hash__` routes through `hash_value_with_interp` (the same path the
        // `hash()` builtin uses) rather than `object.__hash__`, so a scalar
        // subclass instance (`int.__hash__(MyInt(5))`) hashes its backing value
        // to `5` as CPython does — `object.__hash__` returns the identity hash.
        if method == "__hash__" {
            let h = hash_value_with_interp(self, &self_arg.value)?;
            return Ok(Value::int(h));
        }
        if method == "__repr__" {
            let self_val = &self_arg.value;
            if let ValueKind::PyInstance(inst_rc) = self_val.kind() {
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(backing) = effective_builtin_receiver(self_val, &[]) {
                    let rendered = match render_builtin_subclass_repr(self, &class, &backing)? {
                        Some(s) => s,
                        None => crate::interpreter::render_value_repr(self, &backing)?,
                    };
                    return Ok(Value::string(rendered));
                }
            }
            return Ok(Value::string(crate::interpreter::render_value_repr(
                self, self_val,
            )?));
        }
        let dispatch = crate::builtin_registry::lookup("object.__str__")
            .expect("object.__str__ must be in the registry");
        dispatch(self, args)
    }

    /// Dispatch an inherited protocol dunder on a builtin-subclass backing.
    /// Mutable in-place wrappers return that backing value; expose the original
    /// Python receiver when (and only when) the wrapper returned the backing
    /// itself, matching CPython's `return self` contract.
    pub(crate) fn dispatch_builtin_subclass_protocol_dunder(
        &mut self,
        method: &str,
        receiver: &Value,
        backing: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if method == "__iter__" && pyrust_builtins::bytearray::as_bytearray_rc(&backing).is_some() {
            if !args.is_empty() {
                return Err(pyrust_core::type_err!(
                    "expected 0 arguments, got {}",
                    args.len()
                ));
            }
            let frame = NativeIterFrame::bytearray_with_carrier(
                receiver.clone(),
                &backing,
                "bytearray_iterator",
            )
            .ok_or_else(|| PyError::Runtime("bytearray iterator lost its backing".into()))?;
            return Ok(Value::generator(Box::new(frame)));
        }
        if !is_mutable_builtin_inplace_dunder(method) {
            return self.dispatch_builtin_protocol_dunder(method, backing, args);
        }
        let result = self.dispatch_builtin_protocol_dunder(method, backing.clone(), args)?;
        Ok(restore_builtin_subclass_inplace_result(
            method, receiver, &backing, result,
        ))
    }
}
