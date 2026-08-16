// Canonical Python truth-value protocol.
impl Interpreter {
    /// Apply Python's truth-value protocol, including instance slots and
    /// metaclass slots for class objects.
    pub(crate) fn truthy_value(&mut self, value: &Value) -> Result<bool> {
        match value.kind() {
            ValueKind::PyClass(class) => {
                // Special methods on a class object are resolved on its metaclass,
                // just like len(C) and C() dispatch through the metaclass rather
                // than through C's own namespace.
                if let Some(method) = metaclass_dunder_for_call(class, "__bool__").transpose()? {
                    let result = invoke_class_method(self, method, value.clone(), &[])?;
                    return truthy_bool_slot_result(&result);
                }
                if let Some(method) = metaclass_dunder_for_call(class, "__len__").transpose()? {
                    let result = invoke_class_method(self, method, value.clone(), &[])?;
                    return Ok(self.normalize_len_result(&result)? != 0);
                }
                Ok(true)
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                // Try __bool__ first.
                if let Some(method_val) = lookup_class_attr(&class, "__bool__") {
                    let self_val = if matches!(method_val.kind(), ValueKind::BuiltinFunction(_)) {
                        coerce_numeric(value)
                    } else {
                        Value::py_instance(Rc::clone(&inst_rc))
                    };
                    let result = invoke_class_method(self, method_val, self_val, &[])?;
                    return truthy_bool_slot_result(&result);
                }
                // Fall back to __len__.
                if let Some(method_val) = lookup_class_attr(&class, "__len__") {
                    let self_val = if matches!(method_val.kind(), ValueKind::BuiltinFunction(_)) {
                        coerce_numeric(value)
                    } else {
                        Value::py_instance(Rc::clone(&inst_rc))
                    };
                    let result = invoke_class_method(self, method_val, self_val, &[])?;
                    return Ok(self.normalize_len_result(&result)? != 0);
                }
                // Issue #1204: no __bool__ or __len__ in the user class.
                // For scalar primitive subclasses (MyInt, MyFloat, MyStr,
                // MyBytes), delegate truthiness to the backing value so that
                // `bool(MyInt(0))` returns False as CPython does.
                if let Some(backing) = builtin_data_backing(value) {
                    return Ok(backing.truthy_raw());
                }
                // Non-primitive PyInstance with no __bool__ / __len__: always truthy.
                Ok(true)
            }
            ValueKind::BuiltinObject { ops, state } => {
                if pyrust_builtins::mapping_proxy::is_object_proxy_ops(ops) {
                    // A mappingproxy's truth slot delegates to its owner's
                    // length protocol and deliberately ignores owner __bool__.
                    let owner = pyrust_builtins::mapping_proxy::owner_from_state(state)
                        .expect("object mappingproxy state");
                    Ok(self.object_len(&owner)? != 0)
                } else {
                    Ok(ops.truthy(state))
                }
            }
            _ => Ok(value.truthy_raw()),
        }
    }
}

fn truthy_bool_slot_result(result: &Value) -> Result<bool> {
    match result.kind() {
        ValueKind::Bool(value) => Ok(value),
        _ => Err(pyrust_core::type_err!(
            "__bool__ should return bool, returned {}",
            pyrust_core::builtin_type_name(result),
        )),
    }
}
