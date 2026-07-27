// Instance attribute lookup semantics.
impl Interpreter {
    /// Standard attribute lookup for a `PyInstance` — the body of
    /// CPython's `object.__getattribute__`.  Called by `get_attr` when no
    /// user-defined `__getattribute__` is in the MRO, and directly by the
    /// `object.__getattribute__` builtin to avoid recursive dispatch.
    pub(crate) fn get_attr_instance_raw(
        &mut self,
        instance: Rc<RefCell<PyInstance>>,
        name: &str,
    ) -> Result<Value> {
        if name == "__class__" {
            return Ok(Value::py_class(Rc::clone(&instance.borrow().class)));
        }
        if name == "__dict__" {
            // Issue #2076: a class with `__slots__` (no `'__dict__'` slot, no
            // unslotted ancestor) suppresses the instance `__dict__` entirely —
            // `inst.__dict__` raises AttributeError (so `hasattr` is False).
            if class_suppresses_instance_dict(&instance.borrow().class) {
                let class_name = instance.borrow().class.borrow().name.clone();
                return Err(pyrust_core::py_err!(
                    "AttributeError",
                    "'{class_name}' object has no attribute '__dict__'"
                ));
            }
            // Issue #1981: when `__dict__` was replaced wholesale
            // (`obj.__dict__ = d`), the assigned dict is the live backing store.
            // Return it verbatim so `obj.__dict__ is d` holds.
            if let Some(d) = instance.borrow().attrs.dict_ref() {
                return Ok(d.clone());
            }
            // Return a live mutable proxy so that writes to
            // `obj.__dict__['key'] = val` propagate back to the
            // instance's actual attrs map.  Required for the data-
            // descriptor `__set__` protocol (issues #1271 / #1272).
            return Ok(pyrust_builtins::instance_dict::instance_dict(Rc::clone(
                &instance,
            )));
        }

        // PEP 695 lazy bound/constraints (#2290): a generic type parameter's
        // bound (`T: int`) and constraints (`T: (int, str)`) are evaluated
        // lazily — on first access of `__bound__` / `__constraints__`, not at
        // def/class/alias time — matching CPython's deferred annotation scope.
        // The compiler stores the clause as a zero-arg thunk on the internal
        // `__evaluate_bound__` / `__evaluate_constraints__` slot; here we invoke
        // it once, cache the result onto `__bound__` / `__constraints__`, drop
        // the thunk, and return.  Any exception from the thunk (e.g. a NameError
        // for a name defined later or never) propagates from this access.
        if (name == "__bound__" || name == "__constraints__")
            && is_typevar_class(&instance.borrow().class)
        {
            let thunk_name = if name == "__bound__" {
                "__evaluate_bound__"
            } else {
                "__evaluate_constraints__"
            };
            let thunk = instance.borrow().attrs.get(thunk_name).cloned();
            if let Some(thunk) = thunk {
                let value = self.call_function_expanded(thunk, &[])?;
                let mut inst = instance.borrow_mut();
                inst.attrs.insert(name, value.clone());
                inst.attrs.shift_remove(thunk_name);
                return Ok(value);
            }
        }
        // The lazy-evaluation thunk slots are CPython-internal: `hasattr(T,
        // '__evaluate_bound__')` is False there.  Keep them invisible to direct
        // attribute reads so the only observable surface is `__bound__` /
        // `__constraints__`.
        if (name == "__evaluate_bound__" || name == "__evaluate_constraints__")
            && is_typevar_class(&instance.borrow().class)
        {
            let class_name = instance.borrow().class.borrow().name.clone();
            return Err(pyrust_core::py_err!(
                "AttributeError",
                "'{class_name}' object has no attribute '{name}'"
            ));
        }

        // General descriptor protocol (CPython Data Model §3.3.2):
        //
        // Step 1: Walk the class MRO for a data descriptor (has
        // __set__ OR __delete__).  Data descriptors take priority
        // over instance __dict__.
        //
        // If __get__ raises AttributeError, CPython's
        // slot_tp_getattr_hook falls through to __getattr__ (if
        // defined) rather than propagating immediately — only
        // non-AttributeError exceptions propagate without __getattr__.
        let class = { Rc::clone(&instance.borrow().class) };
        if let Some(class_val) = lookup_class_attr(&class, name)
            && is_data_descriptor(&class_val)
        {
            let desc_result = call_descriptor_get(
                self,
                &class_val,
                Value::py_instance(Rc::clone(&instance)),
                Value::py_class(Rc::clone(&class)),
                name,
            );
            match desc_result {
                Ok(v) => return Ok(v),
                Err(ref e) if e.class_name_is("AttributeError") => {
                    // __get__ raised AttributeError: try __getattr__ if
                    // defined, otherwise re-raise the original error
                    // (CPython slot_tp_getattr_hook behaviour).
                    if let Some(r) = self.try_invoke_getattr_hook(&class, &instance, name) {
                        return r;
                    }
                }
                Err(_) => {}
            }
            return desc_result;
        }

        // Built-in exception C slots are data-descriptor storage even though
        // pyrust does not materialise a descriptor object for every field.
        // Read them before `__dict__`, so a same-named dict key remains visible
        // through the mapping without shadowing `e.args`, `e.errno`, etc.
        // A user subclass class attribute with the same name disables this
        // adapter and falls through to normal Python lookup precedence.
        if active_exception_slot(&class, name) {
            let slot_value = instance.borrow().attrs.get_slot(name).cloned();
            if let Some(value) = slot_value {
                if name == "__traceback__"
                    && let Some(real) = self.materialize_deferred_traceback(&value)
                {
                    instance
                        .borrow_mut()
                        .attrs
                        .insert_slot("__traceback__", real.clone());
                    return Ok(real);
                }
                return Ok(value);
            }
            return Ok(match name {
                "args" => Value::tuple(Vec::new()),
                "__suppress_context__" => Value::bool_(false),
                _ => Value::none(),
            });
        }

        // Step 2: Instance __dict__.  Scope the shared borrow so it is dropped
        // before any materialisation below re-borrows the instance.  `get_cloned`
        // routes through the live `__dict__` for dict-backed instances (#1981).
        let attr_value = instance.borrow().attrs.get_cloned(name);
        if let Some(value) = attr_value {
            return Ok(value);
        }

        // Step 2b: numeric-tower read-only properties for int/float subclasses.
        // CPython defines `real`, `imag`, `numerator`, `denominator` as
        // getset_descriptor data descriptors on `int` and `float`, so int/float
        // subclass instances inherit them.  Pyrust intercepts them via the
        // backing `__builtin_data__` value rather than registering real
        // descriptors on the primitive class (issue #1341).
        if matches!(name, "real" | "imag" | "numerator" | "denominator")
            && let Some(backing) = instance_builtin_data(&instance)
            && let Some(v) =
                pyrust_builtins::numeric_attrs_descriptor::numeric_tower_attr(&backing, name)
        {
            return Ok(v);
        }
        // Complex subclass instances (issue #2544): `complex` exposes
        // `real`/`imag` getset_descriptors and the `conjugate` method, so a
        // subclass instance must resolve them through the `complex` backing.
        if matches!(name, "real" | "imag" | "conjugate")
            && let Some(backing) = instance_builtin_data(&instance)
            && let Some(v) = pyrust_builtins::numeric_attrs_descriptor::complex_attr(&backing, name)
        {
            return Ok(v);
        }

        // Step 3: Non-data descriptor or plain class attribute.
        // `cached_property` and user-defined non-data descriptors
        // (has __get__ but NOT __set__/__delete__) fire here;
        // regular UserFunction / BuiltinFunction attrs bind to the
        // instance as before.
        if let Some(value) = lookup_class_attr(&class, name) {
            // cached_property: non-data descriptor with caching.
            // Must come before the general __get__ check because
            // cached_property's result is stored back into
            // instance.attrs for subsequent accesses.
            if let Some((func, attr_name)) =
                pyrust_builtins::cached_property::with_cached_property(&value, |s| {
                    (s.func.clone(), s.attr_name.clone())
                })
            {
                let result = self.call_function_expanded(
                    func,
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::py_instance(Rc::clone(&instance)),
                    }],
                )?;
                instance
                    .borrow_mut()
                    .attrs
                    .insert(attr_name, result.clone());
                return Ok(result);
            }
            // General non-data descriptor: has __get__ but no __set__/__delete__.
            // Same AttributeError fallthrough to __getattr__ applies.
            if is_non_data_descriptor(&value) {
                let desc_result = call_descriptor_get(
                    self,
                    &value,
                    Value::py_instance(Rc::clone(&instance)),
                    Value::py_class(Rc::clone(&class)),
                    name,
                );
                match desc_result {
                    Ok(v) => return Ok(v),
                    Err(ref e) if e.class_name_is("AttributeError") => {
                        // __get__ raised AttributeError: try __getattr__ if
                        // defined, otherwise re-raise the original error.
                        if let Some(r) = self.try_invoke_getattr_hook(&class, &instance, name) {
                            return r;
                        }
                    }
                    Err(_) => {}
                }
                return desc_result;
            }
            // Plain class attribute: bind functions to the instance.
            // Probe kind tag in a scoped block so the `kind()` Ref
            // drops before the `_ => value` arm may move `value`
            // (#450).
            enum AttrKind {
                UserFunction(Rc<UserFunction>),
                // Carries the callable provider's typed category and immutable
                // canonical owner. Attribute routing must not reparse the
                // registry/display key to decide descriptor semantics.
                BuiltinFunction(crate::builtin_registry::BuiltinCallableMetadata),
                ClassMethodAny(pyrust_builtins::classmethod::ClassMethodBindingSpec),
                StaticMethodAny(Value),
                Other,
            }
            let tag = match value.kind() {
                ValueKind::UserFunction(f) => AttrKind::UserFunction(Rc::clone(f)),
                ValueKind::BuiltinFunction(fn_name) => {
                    AttrKind::BuiltinFunction(builtin_callable_metadata(fn_name))
                }
                _ => {
                    if let Some(w) = pyrust_builtins::classmethod::as_class_method_any(&value) {
                        AttrKind::ClassMethodAny(w)
                    } else if let Some(w) =
                        pyrust_builtins::classmethod::as_static_method_any(&value)
                    {
                        AttrKind::StaticMethodAny(w)
                    } else {
                        AttrKind::Other
                    }
                }
            };
            return match tag {
                AttrKind::UserFunction(f) => Ok(match f.kind {
                    UserFunctionKind::Regular => Value::bound_method(Rc::clone(&f), instance),
                    UserFunctionKind::ClassMethod => {
                        Value::class_bound_method(Rc::clone(&f), Rc::clone(&class))
                    }
                    UserFunctionKind::StaticMethod => {
                        if let Some(inner) = f.wrapped_func.as_ref() {
                            Value::user_function(Rc::clone(inner))
                        } else {
                            Value::with_function_kind(Rc::clone(&f), UserFunctionKind::Regular)
                        }
                    }
                    UserFunctionKind::Builtin(_) => Value::user_function(Rc::clone(&f)),
                }),
                // Module functions never implement the descriptor protocol,
                // even if a user stores one on a class. Method descriptors
                // retain their category and declared method name even under a
                // user-chosen alias, with receiver validation driven by the
                // canonical owner tag emitted at the registration boundary
                // (#1477/#1495/#1500).
                AttrKind::BuiltinFunction(metadata) => {
                    if metadata.kind
                        != crate::builtin_registry::BuiltinCallableKind::MethodDescriptor
                    {
                        Ok(value)
                    } else {
                        if let Some(owner_tag) = metadata.descriptor_owner_tag() {
                            let defining_class = canonical_class_by_tag(owner_tag);
                            if !class_is_subclass_of(&class, &defining_class) {
                                let instance_type = class.borrow().name.clone();
                                return Err(pyrust_core::type_err!(
                                    "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
                                    metadata.python_name(),
                                    owner_tag.canonical_name(),
                                    instance_type
                                ));
                            }
                        }
                        Ok(pyrust_builtins::bound_method::bound_method(
                            metadata.python_name().to_string(),
                            Value::py_instance(Rc::clone(&instance)),
                        ))
                    }
                }
                // Apply the same descriptor binding as class-level access.
                AttrKind::ClassMethodAny(w) => {
                    pyrust_builtins::classmethod::bind_wrapped_class_method(w, Rc::clone(&class))
                }
                AttrKind::StaticMethodAny(w) => Ok(w),
                AttrKind::Other => Ok(value),
            };
        }

        // Step 4: __getattr__ fallback — called when normal lookup
        // finds nothing (CPython slot_tp_getattr_hook).
        if let Some(r) = self.try_invoke_getattr_hook(&class, &instance, name) {
            return r;
        }

        let class_name = class.borrow().name.clone();
        Err(PyError::attribute_error(
            format!("'{}' object has no attribute '{}'", class_name, name),
            Some(name.to_string()),
            Some(Value::py_instance(Rc::clone(&instance))),
        ))
    }

    /// The default `object.__setattr__` implementation: descriptor protocol
    /// then `instance.__dict__` write.  Does NOT call `__setattr__` on the
    /// instance's class (that would cause infinite recursion when called from
    /// inside a custom `__setattr__`).  Called by the `object.__setattr__`
    /// builtin handler; the public `assign_attr` wraps this with the
    /// `__setattr__` dispatch.
    pub(crate) fn assign_attr_instance_raw(
        &mut self,
        instance: Rc<RefCell<PyInstance>>,
        name: &str,
        value: Value,
    ) -> Result<()> {
        let class = { Rc::clone(&instance.borrow().class) };
        // PEP 695 / issue #2274: enforce TypeVar's read-only getset
        // descriptors here too, so `setattr(tv, '__bound__', X)` matches the
        // `tv.__bound__ = X` path (see `assign_attr_instance`).
        if let Some(msg) = typing_object_readonly_attr_error(&class, name) {
            return Err(pyrust_core::py_err!("AttributeError", "{msg}"));
        }
        // General data descriptor protocol: if the class (or MRO) has
        // a data descriptor (has __set__) for this name, call __set__.
        if let Some(class_val) = lookup_class_attr(&class, name)
            && let Some(result) = call_descriptor_set(
                self,
                &class_val,
                Value::py_instance(Rc::clone(&instance)),
                value.clone(),
                name,
            )?
        {
            return result;
        }
        // Issue #1198: bare `object()` instances have no __dict__.
        if Rc::ptr_eq(&class, &object_class_singleton()) {
            return Err(pyrust_core::py_err!(
                "AttributeError",
                "'object' object has no attribute '{name}'"
            ));
        }
        // PEP 3134 / issue #1066: validate native exception-slot types.
        // A user subclass class attribute with the same name restores normal
        // Python dict semantics and is therefore not validated as a C slot.
        let exception_slot = active_exception_slot(&class, name);
        if exception_slot {
            match name {
                "__cause__" | "__context__" => {
                    let ok = match value.kind() {
                        ValueKind::None => true,
                        ValueKind::PyInstance(inst) => is_exception_class(&inst.borrow().class),
                        _ => false,
                    };
                    if !ok {
                        return Err(pyrust_core::type_err!(
                            "exception {} must be None or derive from BaseException",
                            if name == "__cause__" {
                                "cause"
                            } else {
                                "context"
                            }
                        ));
                    }
                }
                "__suppress_context__" if !matches!(value.kind(), ValueKind::Bool(_)) => {
                    return Err(pyrust_core::type_err!("attribute value type must be bool"));
                }
                "__traceback__" => {
                    let ok = value.is_none() || pyrust_builtins::traceback::is_traceback(&value);
                    if !ok {
                        return Err(pyrust_core::type_err!(
                            "__traceback__ must be a traceback or None"
                        ));
                    }
                }
                _ => {}
            }
        }
        // Issue #1957: `obj.__class__ = NewType` re-types the instance rather
        // than storing a literal attribute. CPython requires another mutable
        // class with a compatible dict/weakref/slot layout; the shared retype
        // adapter validates that contract before repointing the class.
        // `__class__` is a type-level slot, so this is handled before ordinary
        // `__slots__` name enforcement.
        if name == "__class__" {
            return self.retype_instance(&instance, value);
        }
        if exception_slot {
            instance.borrow_mut().attrs.insert_slot(name, value);
            return Ok(());
        }
        // Issue #1106: if the class declares `__slots__`, only allow assignment
        // to names in the slot set.  CPython raises AttributeError for names not
        // listed in `__slots__` (when the class has no __dict__ slot).
        // Skip enforcement when `__dict__` is itself one of the declared slots —
        // that allows instances to have arbitrary attributes (CPython behaviour).
        // Also skip when any ancestor class in the MRO has no `__slots__`:
        // a single unslotted ancestor reintroduces `__dict__` for all subclasses
        // (CPython rule).  This covers both non-slotted intermediate classes and
        // exception bases (BaseException has tp_dictoffset in CPython, mirrored
        // here by slots: None on the built-in exception classes).
        {
            let has_slots = class.borrow().slots.is_some();
            if has_slots
                && !mro_slot_allows(&class, "__dict__")
                && !mro_slot_allows(&class, name)
                && !mro_has_unslotted_ancestor(&class)
            {
                let class_name = class.borrow().name.clone();
                return Err(pyrust_core::py_err!(
                    "AttributeError",
                    "'{class_name}' object has no attribute '{name}'"
                ));
            }
        }
        // Issue #1942: `instance.__dict__ = {...}` replaces the backing attrs
        // map wholesale rather than storing an attribute named "__dict__".
        // Placed after slots enforcement so a slotted class without a
        // `__dict__` slot still raises AttributeError (CPython parity).
        if name == "__dict__" {
            return replace_instance_dict(&instance, &value);
        }
        instance.borrow_mut().attrs.insert(name, value);
        Ok(())
    }

    /// Re-type an instance via `obj.__class__ = NewType` (issue #1957).
    /// Validates that the value is a re-typable (mutable, non-primitive) class
    /// and repoints the instance's `class` field.  Shared by the public and
    /// raw setattr paths.
    fn retype_instance(&mut self, instance: &Rc<RefCell<PyInstance>>, value: Value) -> Result<()> {
        let new_class = match value.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                let type_name = pyrust_core::builtin_type_name(&value);
                return Err(pyrust_core::type_err!(
                    "__class__ must be set to a class, not '{type_name}' object"
                ));
            }
        };
        // CPython only allows __class__ reassignment between mutable
        // (heap) types.  Re-typing to a built-in immutable class
        // (int / str / list / object / …) raises TypeError.
        if crate::interpreter::is_primitive_class(&new_class)
            || Rc::ptr_eq(&new_class, &object_class_singleton())
        {
            return Err(pyrust_core::type_err!(
                "__class__ assignment only supported for mutable types or ModuleType subclasses"
            ));
        }
        let old_class = Rc::clone(&instance.borrow().class);
        if !instance_layouts_compatible(&old_class, &new_class) {
            let old_name = old_class.borrow().name.clone();
            let new_name = new_class.borrow().name.clone();
            return Err(pyrust_core::type_err!(
                "__class__ assignment: '{new_name}' object layout differs from '{old_name}'"
            ));
        }
        let remaps = member_slot_retype_remap(&old_class, &new_class);
        let mut instance = instance.borrow_mut();
        instance.attrs.remap_member_slots(&remaps);
        instance.class = new_class;
        Ok(())
    }

    /// The default `object.__delattr__` implementation: descriptor protocol
    /// then `instance.__dict__` removal.  Does NOT call `__delattr__` on the
    /// instance's class.  Called by the `object.__delattr__` builtin handler.
    pub(crate) fn delete_attr_instance_raw(
        &mut self,
        instance: Rc<RefCell<PyInstance>>,
        name: &str,
    ) -> Result<()> {
        let class = { Rc::clone(&instance.borrow().class) };
        // PEP 695 / issue #2274: `object.__delattr__(tv, '__bound__')` rejects
        // TypeVar's read-only descriptors, same as the `del tv.__bound__` path.
        if let Some(msg) = typing_object_readonly_attr_error(&class, name) {
            return Err(pyrust_core::py_err!("AttributeError", "{msg}"));
        }
        if let Some(class_val) = lookup_class_attr(&class, name)
            && let Some(result) = call_descriptor_delete(
                self,
                &class_val,
                Value::py_instance(Rc::clone(&instance)),
                name,
            )?
        {
            return result;
        }
        if active_exception_slot(&class, name) {
            return delete_active_exception_slot(&instance, name);
        }
        if instance.borrow_mut().attrs.shift_remove(name).is_none() {
            let class_name = instance.borrow().class.borrow().name.clone();
            return Err(PyError::attribute_error(
                format!("'{class_name}' object has no attribute '{name}'"),
                Some(name.to_string()),
                Some(Value::py_instance(Rc::clone(&instance))),
            ));
        }
        Ok(())
    }
}
