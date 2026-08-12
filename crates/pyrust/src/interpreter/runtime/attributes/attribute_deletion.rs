// Attribute deletion semantics.
impl Interpreter {
    pub(crate) fn delete_attr(&mut self, target: Value, name: &str) -> Result<()> {
        if name == "__module__" {
            if pyrust_builtins::bound_method::as_bound_method(&target).is_some()
                && !pyrust_builtins::bound_method::is_method_wrapper(&target)
            {
                let updated = pyrust_builtins::bound_method::set_module(&target, Value::none());
                debug_assert!(updated);
                return Ok(());
            }
            if pyrust_builtins::native_builtin_callable::as_native_static_builtin(&target).is_some()
            {
                let updated =
                    pyrust_builtins::native_builtin_callable::set_module(&target, Value::none());
                debug_assert!(updated);
                return Ok(());
            }
        }
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                self.delete_attr_kind_instance(&instance, name)
            }
            ValueKind::UserFunction(func) => {
                // CPython raises TypeError for `del f.__name__` / `del f.__qualname__`
                // with the same message as a non-string assignment — these slots exist
                // but cannot be deleted.
                // `del f.__module__` and `del f.__doc__` are allowed; they reset
                // the slot to None (matching CPython).
                // `del f.__dict__` raises TypeError (CPython: "cannot delete __dict__").
                // Arbitrary attrs are removed from the attrs dict; if absent,
                // AttributeError (matching CPython).
                // `del f.__annotations__` resets the dict to empty (matching CPython).
                match name {
                    "__name__" | "__qualname__" => Err(pyrust_core::type_err!(
                        "{name} must be set to a string object"
                    )),
                    "__module__" => {
                        *func.module.borrow_mut() = Value::none();
                        Ok(())
                    }
                    "__doc__" => {
                        *func.doc.borrow_mut() = Value::none();
                        Ok(())
                    }
                    "__dict__" => Err(pyrust_core::type_err!("cannot delete __dict__")),
                    "__annotations__" => {
                        // CPython allows `del f.__annotations__`; it resets the
                        // dict to a fresh empty dict (new object).
                        *func.annotations.borrow_mut() = Value::dict(PyDict::default());
                        Ok(())
                    }
                    // CPython-matched behaviour for validated-but-unimplemented slots.
                    "__code__" => Err(pyrust_core::type_err!(
                        "__code__ must be set to a code object"
                    )),
                    "__globals__" | "__closure__" => {
                        Err(pyrust_core::py_err!("AttributeError", "readonly attribute"))
                    }
                    "__func__"
                        if matches!(
                            func.kind,
                            UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
                        ) =>
                    {
                        Err(pyrust_core::py_err!("AttributeError", "readonly attribute"))
                    }
                    // #2395: CPython allows `del f.__defaults__` /
                    // `del f.__kwdefaults__`; both reset the slot to `None`, which
                    // also clears any compile-time defaults the params carried.
                    // Model this with an explicit `None` override.
                    "__defaults__" => {
                        func.set_defaults_override(Value::none());
                        Ok(())
                    }
                    "__kwdefaults__" => {
                        func.set_kwdefaults_override(Value::none());
                        Ok(())
                    }
                    _ => {
                        // Short-circuit: if attrs were never initialised, there
                        // is nothing to delete — raise AttributeError immediately.
                        let key = PyKey::str_from(name);
                        let removed = func
                            .attrs
                            .borrow()
                            .as_ref()
                            .and_then(|rc| rc.borrow().dict_shift_remove(&key).ok().flatten());
                        if removed.is_some() {
                            Ok(())
                        } else {
                            let type_name = match func.kind {
                                UserFunctionKind::StaticMethod => {
                                    pyrust_builtins::classmethod::STATIC_TYPE_NAME
                                }
                                UserFunctionKind::ClassMethod => {
                                    pyrust_builtins::classmethod::CLASS_TYPE_NAME
                                }
                                _ => "function",
                            };
                            Err(pyrust_core::py_err!(
                                "AttributeError",
                                "'{type_name}' object has no attribute '{name}'"
                            ))
                        }
                    }
                }
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                self.delete_attr_kind_class(&class, name)
            }
            ValueKind::BuiltinFunction(func_name) => {
                // Method descriptors reject deletion. A
                // builtin_function_or_method resets its per-object slot to None.
                let callable_kind =
                    super::builtin_methods::builtin_callable_metadata(func_name).kind;
                if callable_kind == crate::builtin_registry::BuiltinCallableKind::MethodDescriptor {
                    match name {
                        "__module__" => Err(pyrust_core::py_err!(
                            "AttributeError",
                            "'method_descriptor' object has no attribute '__module__'"
                        )),
                        "__name__" => {
                            Err(pyrust_core::py_err!("AttributeError", "readonly attribute"))
                        }
                        "__qualname__" | "__doc__" => Err(pyrust_core::py_err!(
                            "AttributeError",
                            "attribute '{name}' of 'method_descriptor' objects is not writable"
                        )),
                        _ => Err(pyrust_core::py_err!(
                            "AttributeError",
                            "'method_descriptor' object has no attribute '{name}'"
                        )),
                    }
                } else {
                    match name {
                        "__module__" => {
                            let function = target
                                .as_function_rc()
                                .expect("BuiltinFunction must carry Rc<UserFunction>");
                            *function.module.borrow_mut() = Value::none();
                            Ok(())
                        }
                        "__name__" | "__qualname__" | "__doc__" => Err(pyrust_core::py_err!(
                            "AttributeError",
                            "attribute '{name}' of 'builtin_function_or_method' \
                                     objects is not writable"
                        )),
                        _ => Err(pyrust_core::py_err!(
                            "AttributeError",
                            "'builtin_function_or_method' object has no attribute '{name}'"
                        )),
                    }
                }
            }
            ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
                // CPython raises AttributeError when deleting any attribute on a
                // bound method object.
                Err(pyrust_core::py_err!(
                    "AttributeError",
                    "'method' object has no attribute '{name}'"
                ))
            }
            ValueKind::PyModule(module) => {
                // CPython 3.12: module attribute deletion removes the key from
                // __dict__; AttributeError if the name is absent (matching
                // CPython's module_setattro with NULL value path).
                // Note: CPython's delete-path uses "'module' object has no
                // attribute 'X'" (generic), while get-path uses the module name.
                //
                // __dict__ is a read-only slot on module objects — CPython 3.12
                // raises AttributeError("readonly attribute") for `del m.__dict__`.
                if name == "__dict__" {
                    return Err(pyrust_core::py_err!("AttributeError", "readonly attribute"));
                }
                let module = Rc::clone(module);
                if module.borrow().filesystem_namespace().is_some()
                    || module.borrow().live_namespace().is_some()
                {
                    let existing = module.borrow().get_attr_value(name);
                    if existing.as_ref().is_some_and(|value| !value.is_unset()) {
                        module.borrow_mut().remove_attr(name);
                        return Ok(());
                    }
                    return Err(pyrust_core::py_err!(
                        "AttributeError",
                        "'module' object has no attribute '{name}'"
                    ));
                }
                // Peek before removing.  A Value::unset() in attrs is a
                // deletion tombstone for a synthetic dunder that was already
                // deleted — treat it the same as absent so that a second `del`
                // correctly raises AttributeError.
                let existing = module.borrow().attrs.get(name).cloned();
                match existing {
                    Some(v) if !v.is_unset() => {
                        // A real (non-tombstone) value is present.  Remove it,
                        // and if the name is also a synthetic dunder, replace
                        // with a tombstone so get_attr does not fall through to
                        // the synthetic fallback path (CPython 3.12: once you
                        // delete __name__ it stays absent even if you had
                        // previously stored a custom string there).
                        if matches!(
                            name,
                            "__name__" | "__package__" | "__loader__" | "__spec__" | "__doc__"
                        ) {
                            module
                                .borrow_mut()
                                .insert_attr(name.to_string(), Value::unset());
                        } else {
                            module.borrow_mut().remove_attr(name);
                        }
                        return Ok(());
                    }
                    None if matches!(
                        name,
                        "__name__" | "__package__" | "__loader__" | "__spec__" | "__doc__"
                    ) =>
                    {
                        // Synthetic dunders live only in get_attr, not in attrs.
                        // CPython 3.12 allows deleting them (they exist in the
                        // real module __dict__).  Write a Value::unset() tombstone
                        // so get_attr stops synthesising them on future reads.
                        module
                            .borrow_mut()
                            .insert_attr(name.to_string(), Value::unset());
                        return Ok(());
                    }
                    _ => {}
                }
                Err(pyrust_core::py_err!(
                    "AttributeError",
                    "'module' object has no attribute '{name}'"
                ))
            }
            ValueKind::Generator(_) => {
                // CPython 3.12 symmetry with assign_attr: deleting __name__ or
                // __qualname__ raises the same TypeError as assigning a non-string.
                // The read-only gi_* attrs raise AttributeError "not writable".
                // Anything else raises AttributeError "has no attribute".
                match name {
                    "__name__" | "__qualname__" => Err(pyrust_core::type_err!(
                        "{name} must be set to a string object"
                    )),
                    "gi_running" | "gi_yieldfrom" | "gi_frame" | "gi_code" => {
                        Err(pyrust_core::py_err!(
                            "AttributeError",
                            "attribute '{name}' of 'generator' objects is not writable"
                        ))
                    }
                    _ => Err(pyrust_core::py_err!(
                        "AttributeError",
                        "'generator' object has no attribute '{name}'"
                    )),
                }
            }
            _ => {
                let type_name = pyrust_core::error_type_name(&target);
                let data_descriptor_owner = dict_view_mapping_descriptor_owner(&target, name);
                // CPython distinguishes "attribute does not exist" from
                // "attribute exists on the type but is read-only" (issue #2562).
                // `(1).real = 5` resolves `real` to a read-only getset_descriptor
                // and raises "is not writable"; `(1).bit_length = 5` resolves to a
                // method_descriptor and raises "is read-only"; only a genuinely
                // absent attribute keeps "has no attribute".  We reuse the read
                // path to decide which case we are in.  `__class__` keeps its own
                // (current) behaviour — its write semantics are tracked separately.
                if name != "__class__"
                    && let Ok(read) = self.get_attr(&target, name)
                {
                    // Methods (method_descriptor / wrapper_descriptor /
                    // classmethod_descriptor) read back as callables; CPython's
                    // wording for those is "'T' object attribute 'X' is
                    // read-only".  Read-only *data* attributes
                    // (getset_descriptor: real/imag/numerator/denominator) read
                    // back as plain values and use "attribute 'X' of 'T' objects
                    // is not writable".  Built-in methods/method-wrappers surface
                    // as the bound-method `BuiltinObject`, so probe that too.
                    let is_method = matches!(
                        read.kind(),
                        ValueKind::BoundMethod { .. }
                            | ValueKind::ClassBoundMethod { .. }
                            | ValueKind::BuiltinFunction(_)
                            | ValueKind::UserFunction(_)
                    ) || pyrust_builtins::bound_method::is_bound_method(&read);
                    return if is_method {
                        Err(pyrust_core::py_err!(
                            "AttributeError",
                            "'{type_name}' object attribute '{name}' is read-only"
                        ))
                    } else {
                        let owner_name = data_descriptor_owner.unwrap_or(type_name.as_ref());
                        Err(pyrust_core::py_err!(
                            "AttributeError",
                            "attribute '{name}' of '{owner_name}' objects is not writable"
                        ))
                    };
                }
                Err(pyrust_core::py_err!(
                    "AttributeError",
                    "'{type_name}' object has no attribute '{name}'"
                ))
            }
        }
    }

    /// `del obj.name` for a `PyInstance` target. Split out of `delete_attr`.
    fn delete_attr_kind_instance(
        &mut self,
        instance: &Rc<RefCell<PyInstance>>,
        name: &str,
    ) -> Result<()> {
        let class = { Rc::clone(&instance.borrow().class) };
        // PEP 695 / issue #2274: TypeVar's read-only getset descriptors
        // reject deletion with the same AttributeError as assignment,
        // before any existence check.  Arbitrary attributes delete normally.
        if let Some(msg) = typing_object_readonly_attr_error(&class, name) {
            return Err(pyrust_core::py_err!("AttributeError", "{msg}"));
        }
        // Check for `__delattr__` first — symmetric with __setattr__
        // in assign_attr (issue #1174).  Skip only the
        // `object.__delattr__` builtin sentinel.
        if let Some(delattr_val) = lookup_class_attr(&class, "__delattr__") {
            let is_object_default = crate::interpreter::value_is_canonical_slot(
                &delattr_val,
                crate::interpreter::CanonicalSlot::ObjectDelAttr,
            );
            if !is_object_default {
                return invoke_class_method(
                    self,
                    delattr_val,
                    Value::py_instance(Rc::clone(instance)),
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(name),
                    }],
                )
                .map(|_| ());
            }
        }
        // General data descriptor protocol: if the class (or MRO)
        // has a descriptor with __delete__ for this name, call it.
        if let Some(class_val) = lookup_class_attr(&class, name)
            && let Some(result) = call_descriptor_delete(
                self,
                &class_val,
                Value::py_instance(Rc::clone(instance)),
                name,
            )?
        {
            return result;
        }
        if let Some(policy) = active_exception_slot_policy(&class, name) {
            return policy.delete(instance, name);
        }
        // `shift_remove` keeps the remaining entries in their
        // original insertion order so `vars(obj)` after `del obj.x`
        // still matches CPython's stable ordering contract.
        // CPython raises AttributeError when the attribute is absent.
        if instance.borrow_mut().attrs.shift_remove(name).is_none() {
            let class_name = pyrust_core::error_type_name(&Value::py_instance(Rc::clone(instance)));
            return Err(pyrust_core::py_err!(
                "AttributeError",
                "'{class_name}' object has no attribute '{name}'"
            ));
        }
        Ok(())
    }

    /// `del Cls.name` for a `PyClass` target. Split out of `delete_attr`.
    fn delete_attr_kind_class(&mut self, class: &Rc<RefCell<PyClass>>, name: &str) -> Result<()> {
        // Provider-tagged immutable native types use the same diagnostic for
        // deletion as assignment.
        if let Some(error) =
            pyrust_builtins::ordered_mapping::immutable_class_attribute_error(class, name)
        {
            return Err(error);
        }
        if Rc::ptr_eq(class, &object_class_singleton())
            || crate::interpreter::is_primitive_class(class)
        {
            let class_name = class.borrow().name.clone();
            return Err(pyrust_core::type_err!(
                "cannot set '{name}' attribute of immutable type '{class_name}'"
            ));
        }
        // __dict__ is a read-only descriptor on type objects — CPython
        // raises AttributeError on `del C.__dict__`.
        if name == "__dict__" {
            return Err(pyrust_core::py_err!(
                "AttributeError",
                "attribute '__dict__' of 'type' objects is not writable"
            ));
        }
        // Class metadata is implemented by type-level descriptors in CPython.
        // None of these four slots can be deleted, even on a mutable heap type.
        if matches!(name, "__module__" | "__name__" | "__qualname__" | "__doc__") {
            let n = class.borrow().name.clone();
            return Err(pyrust_core::type_err!(
                "cannot delete '{name}' attribute of immutable type '{n}'"
            ));
        }
        // Issue #737: `del Cls.__annotations__` must raise
        // `AttributeError` when no annotations dict has been
        // materialised yet — matching CPython's descriptor, which
        // refuses to delete a slot that was never written.
        if name == "__annotations__" && !class.borrow().attrs.contains_key("__annotations__") {
            return Err(pyrust_core::py_err!("AttributeError", "__annotations__"));
        }
        // CPython raises AttributeError when the attribute is absent.
        {
            let mut cls = class.borrow_mut();
            if cls.attrs.shift_remove(name).is_none() {
                let class_name = cls.name.clone();
                return Err(pyrust_core::py_err!(
                    "AttributeError",
                    "type object '{class_name}' has no attribute '{name}'"
                ));
            }
            cls.bump_mutation_version();
            // Issue #2335: deleting `__new__` leaves CPython's sticky
            // `slot_tp_new` wrapper in place — record it so `object.__new__`
            // keeps rejecting excess args even though the attribute now
            // resolves back to `object.__new__` via the MRO.
            if name == "__new__" {
                cls.new_slot_wrapped.set(true);
            }
        }
        // Bump the global epoch so that caches keyed on subclasses of
        // this class also invalidate after a base-class deletion.
        pyrust_core::bump_class_epoch();
        Ok(())
    }
}
