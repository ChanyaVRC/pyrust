use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: len(s) — number of items in a container.
    /// <https://docs.python.org/3/library/functions.html#len>
    /// The `PyInstance` arm can run user code by dispatching `__len__` via
    /// `invoke_class_method`, which can run arbitrary user code (issue #1526).
    // Typed dialect (#builtin-fast-dispatch): a single positional-only arg, so
    // `len(x)` gets the vectorcall fast entry.  The typed prelude supplies the
    // "takes exactly one argument" arity guard and the "takes no keyword
    // arguments" rejection that the legacy body used to spell out by hand.
    #[arity_style(takes_exactly_one)]
    fn len(#[positional_only] obj: PyValue) -> Result<Value> {
        let value = &obj.0;
        let size = match value.kind() {
            ValueKind::Str(_) => value.str_codepoint_len() as i64,
            ValueKind::List(items) => items.len() as i64,
            ValueKind::Tuple(items) => items.len() as i64,
            ValueKind::Set(items) => items.len() as i64,
            ValueKind::Bytes(rc) => rc.len() as i64,
            ValueKind::Dict(items) => items.len() as i64,
            ValueKind::Range { start, stop, step } => {
                match i64::try_from(range_len(start, stop, step)) {
                    Ok(n) => n,
                    Err(_) => {
                        return Err(PyError::named(
                            "OverflowError",
                            "Python int too large to convert to C ssize_t".to_string(),
                        ));
                    }
                }
            }
            // Arbitrary-precision range (#2118): the *length* must still fit in a
            // Py_ssize_t (i64), matching CPython's `range.__len__` which raises
            // `OverflowError: cannot fit 'int' into an index-sized integer` when
            // it does not — even though the bounds themselves may be big.
            ValueKind::BigRange { start, stop, step } => {
                match pyrust_core::bigrange_len(start, stop, step).to_i64() {
                    Some(n) => n,
                    None => {
                        return Err(PyError::named(
                            "OverflowError",
                            "Python int too large to convert to C ssize_t".to_string(),
                        ));
                    }
                }
            }
            ValueKind::BuiltinObject { ops, state } => match ops.len(state) {
                Some(n) => n as i64,
                None => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "object of type '{}' has no len()",
                            ops.display_error_name()
                        ),
                    ));
                }
            },
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                // Always check for a user-defined __len__ override first.
                // Only fall back to backing primitive data when the class
                // does not define __len__. This matches CPython's dunder
                // dispatch semantics (issue #1448).
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__len__") {
                    let result = invoke_class_method(
                        _interp,
                        method_val,
                        Value::py_instance(inst_rc),
                        &[],
                    )?;
                    _interp.normalize_len_result(&result)?
                } else if let Some(backing) = instance_builtin_data(&inst_rc) {
                    // No user __len__: delegate to backing primitive for
                    // dict/list/set/frozenset/tuple subclasses constructed by
                    // call_class_expanded (issue #976/#994).
                    match backing.kind() {
                        ValueKind::Str(_) => backing.str_codepoint_len() as i64,
                        ValueKind::Bytes(rc) => rc.len() as i64,
                        ValueKind::List(items) => items.len() as i64,
                        ValueKind::Dict(items) => items.len() as i64,
                        ValueKind::Set(items) => items.len() as i64,
                        ValueKind::Tuple(items) => items.len() as i64,
                        ValueKind::BuiltinObject { ops, state }
                            if ops.canonical_class_tag()
                                == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
                        {
                            match ops.len(state) {
                                Some(n) => n as i64,
                                None => {
                                    return Err(PyError::named(
                                        "TypeError",
                                        format!(
                                            "object of type '{}' has no len()",
                                            pyrust_core::error_type_name(&Value::py_instance(
                                                Rc::clone(&inst_rc)
                                            )),
                                        ),
                                    ));
                                }
                            }
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "object of type '{}' has no len()",
                                    pyrust_core::error_type_name(&Value::py_instance(Rc::clone(
                                        &inst_rc
                                    ))),
                                ),
                            ));
                        }
                    }
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "object of type '{}' has no len()",
                            pyrust_core::error_type_name(&Value::py_instance(Rc::clone(&inst_rc))),
                        ),
                    ));
                }
            }
            ValueKind::PyClass(cls) => {
                // A class whose metaclass defines `__len__` (e.g. an `Enum`
                // subclass under `EnumMeta`): `len(Color)` dispatches the
                // metaclass slot with the class as the receiver (#2611).
                let Some(method_val) =
                    crate::interpreter::metaclass_dunder_for_call(cls, "__len__")
                else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "object of type '{}' has no len()",
                            pyrust_core::error_type_name(value),
                        ),
                    ));
                };
                let result = invoke_class_method(_interp, method_val?, value.clone(), &[])?;
                _interp.normalize_len_result(&result)?
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "object of type '{}' has no len()",
                        pyrust_core::error_type_name(value),
                    ),
                ));
            }
        };
        Ok(Value::int(size))
    }
}
