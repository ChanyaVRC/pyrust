// Interpreter-aware `str(value)` protocol dispatch.
//
// This lives beside repr and format-spec rendering because it selects and
// validates Python-visible `__str__` / `__repr__` behavior. Built-in method
// routing consumes this service; it must not own formatting policy.

impl Interpreter {
    /// Render a value using the same priority as `str(x)`: `__str__` first,
    /// then `__repr__`, then the default object representation.
    pub(crate) fn render_value_as_str(&mut self, value: &Value) -> Result<String> {
        // `str(cls)` dispatches `type(cls).__str__(cls)` (falling back to the
        // metaclass `__repr__`) when the class has a user metaclass override.
        if let ValueKind::PyClass(cls_rc) = value.kind() {
            let cls_rc = Rc::clone(cls_rc);
            if let Some(result) =
                crate::interpreter::dispatch_metaclass_repr_str(self, &cls_rc, "__str__")
            {
                return result;
            }
        }

        let ValueKind::PyInstance(instance) = value.kind() else {
            // Enforce int_max_str_digits for base-10 bigint rendering. The
            // predicate is tag-fast for ordinary scalar values.
            pyrust_core::check_int_str_conversion(value)?;
            return Ok(value.to_py_str());
        };
        let instance = Rc::clone(instance);
        let class = Rc::clone(&instance.borrow().class);

        // Built-in exception formatting applies only while `__str__` has not
        // been replaced by a user callable.
        if is_exception_class(&class) {
            let has_user_str = lookup_class_attr(&class, "__str__")
                .map(|method| !matches!(method.kind(), ValueKind::BuiltinFunction(_)))
                .unwrap_or(false);
            if !has_user_str {
                return exception_str_with_dispatch(self, value, &instance, &class);
            }
        }

        // Skip object.__str__ for primitive-backed subclasses: their backing
        // value supplies the concrete scalar/container rendering below.
        if let Some(method) = lookup_class_attr(&class, "__str__") {
            let is_object_str =
                matches!(method.kind(), ValueKind::BuiltinFunction("object.__str__"));
            if !is_object_str || builtin_data_backing(value).is_none() {
                let result = invoke_class_method(
                    self,
                    method,
                    Value::py_instance(Rc::clone(&instance)),
                    &[],
                )?;
                return match result.kind() {
                    ValueKind::Str(text) => Ok(text.to_string()),
                    _ => Err(pyrust_core::type_err!(
                        "__str__ returned non-string (type {})",
                        pyrust_core::builtin_type_name(&result)
                    )),
                };
            }
        }

        // str/bytes subclasses return their backing value directly from the
        // inherited native `__str__`, independently of a custom `__repr__`.
        if let Some(backing) = builtin_data_backing(value)
            && matches!(backing.kind(), ValueKind::Str(_) | ValueKind::Bytes(_))
        {
            return Ok(backing.to_py_str());
        }

        if let Some(method) = lookup_class_attr(&class, "__repr__") {
            let is_object_repr =
                matches!(method.kind(), ValueKind::BuiltinFunction("object.__repr__"));
            if !is_object_repr || builtin_data_backing(value).is_none() {
                let result = invoke_class_method(
                    self,
                    method,
                    Value::py_instance(Rc::clone(&instance)),
                    &[],
                )?;
                return match result.kind() {
                    ValueKind::Str(text) => Ok(text.to_string()),
                    _ => Err(pyrust_core::type_err!(
                        "__repr__ returned non-string (type {})",
                        pyrust_core::builtin_type_name(&result)
                    )),
                };
            }
        }

        if let Some(backing) = builtin_data_backing(value) {
            if backing.is_list() || backing.is_dict() || backing.is_tuple() {
                return render_value_repr(self, &backing);
            }
            if let Some(is_empty) = backing.set_len().map(|len| len == 0) {
                let class_name = class.borrow().name.clone();
                if is_empty {
                    return Ok(format!("{class_name}()"));
                }
                let inner = render_value_repr(self, &backing)?;
                return Ok(format!("{class_name}({inner})"));
            }
            match backing.kind() {
                ValueKind::Str(_)
                | ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Bool(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _)
                | ValueKind::Bytes(_) => {
                    pyrust_core::check_int_str_conversion(&backing)?;
                    return Ok(backing.to_py_str());
                }
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
                {
                    let class_name = class.borrow().name.clone();
                    let items = pyrust_builtins::frozenset::as_items(&backing);
                    if items.as_ref().is_none_or(|items| items.is_empty()) {
                        return Ok(format!("{class_name}()"));
                    }
                    let snapshot: Vec<_> = items
                        .expect("non-empty frozenset has backing items")
                        .iter()
                        .cloned()
                        .collect();
                    let mut rendered = Vec::with_capacity(snapshot.len());
                    for key in &snapshot {
                        rendered.push(render_key_repr(self, key)?);
                    }
                    return Ok(format!("{class_name}({{{}}})", rendered.join(", ")));
                }
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
                {
                    return render_value_repr(self, value);
                }
                _ => {}
            }
        }

        Ok(value.repr_raw())
    }
}
