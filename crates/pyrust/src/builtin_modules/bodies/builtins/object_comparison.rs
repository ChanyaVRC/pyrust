use pyrust_derive::pyrust_module;

pyrust_module! {
    /// Issue #1256: `object.__lt__`, `__le__`, `__gt__`, `__ge__` — ordering
    /// comparisons not defined on object; all return `NotImplemented`.
    ///
    /// CPython signature: `object.__lt__(self, value, /)`
    #[py_name = "object.__lt__"]
    fn object_lt_dunder(_args) -> Result<Value> {
        Ok(Value::not_implemented())
    }

    /// CPython signature: `object.__le__(self, value, /)`
    #[py_name = "object.__le__"]
    fn object_le_dunder(_args) -> Result<Value> {
        Ok(Value::not_implemented())
    }

    /// CPython signature: `object.__gt__(self, value, /)`
    #[py_name = "object.__gt__"]
    fn object_gt_dunder(_args) -> Result<Value> {
        Ok(Value::not_implemented())
    }

    /// CPython signature: `object.__ge__(self, value, /)`
    #[py_name = "object.__ge__"]
    fn object_ge_dunder(_args) -> Result<Value> {
        Ok(Value::not_implemented())
    }

    /// Issue #1256: `object.__format__(self, format_spec)`.
    ///
    /// CPython's default implementation calls `str(self)` then applies the
    /// format spec.
    ///
    /// CPython signature: `object.__format__(self, format_spec, /)`
    #[py_name = "object.__format__"]
    fn object_format_dunder(args) -> Result<Value> {
        // Issue #2299: `object.__format__` / the inherited `bytes.__format__`
        // (both resolve to this slot) take no keyword arguments.  CPython names
        // the slot owner `object` regardless of the calling type, so
        // `bytes.__format__(b"", "", k=1)` reports `object.__format__()`.
        if args.iter().any(|a| a.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                "object.__format__() takes no keyword arguments".to_string(),
            ));
        }
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__format__", "object", method)
        })?;
        let spec_str = if args.len() >= 2 {
            match args[1].value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => return Err(PyError::named(
                    "TypeError",
                    "format_spec must be a string".to_string(),
                )),
            }
        } else {
            String::new()
        };
        if native_iterator_class(&self_val).is_some() {
            if spec_str.is_empty() {
                return Ok(Value::string(render_value_repr(_interp, &self_val)?));
            }
            return Err(pyrust_core::type_err!(
                "unsupported format string passed to {}.__format__",
                full_type_name_str(&self_val)
            ));
        }
        // A builtin subclass (`class I(int)`, `class S(str)`, …) that does not
        // override `__format__` resolves `super().__format__(spec)` and the
        // method-call form `inst.__format__(spec)` to *this* `object.__format__`
        // body, because the backing primitive's `__format__` is not exposed as a
        // distinct class attribute in pyrust's MRO.  CPython instead resolves
        // them to the backing type's `__format__` (`int.__format__`), which
        // formats the underlying value.  Emulate that by delegating to the
        // backing formatter when the receiver carries `__builtin_data__`, so
        // `super().__format__('x')` / `I(255).__format__('x')` → `'ff'`
        // (issues #2211, #2214).  The error names the actual subclass, not the
        // backing primitive (issue #2212).
        if let Some(backing) = builtin_data_backing(&self_val) {
            let owner = value_type_name_str(&self_val);
            return apply_format_spec_named(&backing, &spec_str, Some(&owner));
        }
        // CPython raises TypeError when a non-empty spec is passed to
        // object.__format__ on a value with no backing primitive (a pure user
        // class or `object()` itself).
        if !spec_str.is_empty() {
            let type_name = value_type_name_str(&self_val);
            return Err(PyError::named(
                "TypeError",
                format!("unsupported format string passed to {}.__format__", type_name),
            ));
        }
        let s = render_instance_str(_interp, &self_val)?;
        // apply_format_spec takes &Value; wrap the str result temporarily.
        apply_format_spec(&Value::string(s), &spec_str)
    }
}
