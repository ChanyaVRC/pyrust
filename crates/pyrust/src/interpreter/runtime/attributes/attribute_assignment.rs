// Attribute assignment semantics.
impl Interpreter {
    pub(crate) fn assign_attr(&mut self, target: Value, name: &str, value: Value) -> Result<()> {
        if name == "__module__" {
            // Only the concrete builtin_function_or_method state categories
            // own this mutable slot. A method-wrapper may reuse the captured
            // method state shape, but CPython does not expose __module__ on
            // that descriptor category.
            if pyrust_builtins::bound_method::as_bound_method(&target).is_some()
                && !pyrust_builtins::bound_method::is_method_wrapper(&target)
            {
                let updated = pyrust_builtins::bound_method::set_module(&target, value);
                debug_assert!(updated);
                return Ok(());
            }
            // Native static/class-bound wrappers are a second typed state
            // category with the same Python presentation. Mutation stays on
            // this cold attribute path; the call adapter remains unchanged.
            if pyrust_builtins::native_builtin_callable::as_native_static_builtin(&target).is_some()
            {
                let updated = pyrust_builtins::native_builtin_callable::set_module(&target, value);
                debug_assert!(updated);
                return Ok(());
            }
        }
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                self.assign_attr_instance(&instance, name, value)
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                self.assign_attr_class(&class, name, value)
            }
            ValueKind::UserFunction(func) => {
                let func = Rc::clone(func);
                Self::assign_attr_function(&func, name, value)
            }
            ValueKind::BuiltinFunction(func_name) => {
                // CPython distinguishes two cases:
                //
                // builtin_function_or_method (e.g. print, len, math.sqrt):
                //   - __module__ is the only writable attribute, stored on the
                //     concrete Rc<UserFunction> object.
                //   - __name__, __qualname__, __doc__ are read-only:
                //     AttributeError: attribute '__X__' of
                //     'builtin_function_or_method' objects is not writable
                //   - anything else:
                //     AttributeError: 'builtin_function_or_method' object has no
                //     attribute 'X'
                //
                // method_descriptor (e.g. str.upper, list.append):
                //   - __module__ is absent:
                //     AttributeError: 'method_descriptor' object has no attribute
                //     '__module__'
                //   - __name__ is read-only:
                //     AttributeError: readonly attribute
                //   - __qualname__, __doc__ are read-only:
                //     AttributeError: attribute '__X__' of 'method_descriptor'
                //     objects is not writable
                //   - anything else:
                //     AttributeError: 'method_descriptor' object has no attribute 'X'
                let callable_kind =
                    super::builtin_methods::builtin_callable_metadata(func_name).kind;
                if callable_kind == crate::builtin_registry::BuiltinCallableKind::MethodDescriptor {
                    // method_descriptor path
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
                    // builtin_function_or_method path
                    match name {
                        "__module__" => {
                            let function = target
                                .as_function_rc()
                                .expect("BuiltinFunction must carry Rc<UserFunction>");
                            *function.module.borrow_mut() = value;
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
                // CPython raises AttributeError (not a generic RuntimeError) when
                // you try to set any attribute on a bound method object.
                Err(pyrust_core::py_err!(
                    "AttributeError",
                    "'method' object has no attribute '{name}'"
                ))
            }
            ValueKind::PyModule(module) => {
                // CPython 3.12: module attribute assignment always writes to
                // the module's __dict__ (tp_setattro → module_setattro →
                // PyObject_GenericSetAttr).  Synthetic dunders (__name__,
                // __package__, etc.) are writable because they live in
                // __dict__ too.
                //
                // __dict__ itself is a read-only C-level slot — CPython 3.12
                // raises AttributeError("readonly attribute") for both
                // `m.__dict__ = x` and `del m.__dict__` (symmetric).
                if name == "__dict__" {
                    return Err(pyrust_core::py_err!("AttributeError", "readonly attribute"));
                }
                let module = Rc::clone(module);
                module.borrow_mut().insert_attr(name.to_string(), value);
                Ok(())
            }
            ValueKind::Range { .. } | ValueKind::BigRange { .. } => {
                // Range objects are immutable.  start/stop/step are read-only
                // C-level slots in CPython; the error message is "readonly attribute".
                // Any other attribute gives "has no attribute" (issue #1807).
                match name {
                    "start" | "stop" | "step" => {
                        Err(pyrust_core::py_err!("AttributeError", "readonly attribute"))
                    }
                    _ => Err(pyrust_core::py_err!(
                        "AttributeError",
                        "'range' object has no attribute '{name}'"
                    )),
                }
            }
            ValueKind::Generator(state_rc) => {
                // CPython 3.12 allows setting __name__ and __qualname__ on
                // generator objects (str only; TypeError on non-string).
                // gi_running, gi_yieldfrom, gi_frame, gi_code are read-only
                // (AttributeError with "not writable").
                // Any other attribute gives "has no attribute".
                match name {
                    "__name__" | "__qualname__" => {
                        let s = if let ValueKind::Str(s) = value.kind() {
                            s.to_string()
                        } else {
                            return Err(pyrust_core::type_err!(
                                "{name} must be set to a string object"
                            ));
                        };
                        let mut borrow = state_rc.borrow_mut();
                        if let Some(frame) = borrow.downcast_mut::<GeneratorFrame>() {
                            if name == "__name__" {
                                frame.fn_name = std::sync::Arc::from(s.as_str());
                            } else {
                                frame.qualname = std::sync::Arc::from(s.as_str());
                            }
                        }
                        Ok(())
                    }
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
                let type_name = pyrust_core::builtin_type_name(&target);
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
                        Err(pyrust_core::py_err!(
                            "AttributeError",
                            "attribute '{name}' of '{type_name}' objects is not writable"
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

    /// `obj.name = value` for a `PyInstance` target. Split out of `assign_attr`.
    fn assign_attr_instance(
        &mut self,
        instance: &Rc<RefCell<PyInstance>>,
        name: &str,
        value: Value,
    ) -> Result<()> {
        let class = { Rc::clone(&instance.borrow().class) };
        // PEP 695 / issue #2274: TypeVar exposes `__name__`, `__bound__`,
        // `__constraints__`, and the variance flags as read-only getset
        // descriptors.  CPython rejects writes to them with AttributeError
        // before any dict write; arbitrary attributes remain writable.
        if let Some(msg) = typing_object_readonly_attr_error(&class, name) {
            return Err(pyrust_core::py_err!("AttributeError", "{msg}"));
        }
        // Check for `__setattr__` first — CPython dispatches
        // __setattr__ before the descriptor protocol (object.__setattr__
        // is what does the descriptor lookup by default).
        // Skip only the `object.__setattr__` builtin sentinel — it
        // IS the default path below, so invoking it would be
        // redundant and cause infinite recursion when called from
        // inside a custom __setattr__ that delegates back to it.
        if let Some(setattr_val) = lookup_class_attr(&class, "__setattr__") {
            let is_object_default = crate::interpreter::value_is_canonical_slot(
                &setattr_val,
                crate::interpreter::CanonicalSlot::ObjectSetAttr,
            );
            if !is_object_default {
                return invoke_class_method(
                    self,
                    setattr_val,
                    Value::py_instance(Rc::clone(instance)),
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: Value::string(name),
                        },
                        ExpandedCallArg { name: None, value },
                    ],
                )
                .map(|_| ());
            }
        }
        // General data descriptor protocol: if the class (or MRO) has
        // a data descriptor (has __set__) for this name, call __set__.
        if let Some(class_val) = lookup_class_attr(&class, name)
            && let Some(result) = call_descriptor_set(
                self,
                &class_val,
                Value::py_instance(Rc::clone(instance)),
                value.clone(),
                name,
            )?
        {
            return result;
        }
        // Issue #1198: bare `object()` instances have no __dict__ in
        // CPython.  Only the object singleton itself is blocked; any
        // user-defined class (even `class Foo(object): pass`) gets its
        // own PyClass Rc and is not ptr_eq to the singleton.
        if Rc::ptr_eq(&class, &object_class_singleton()) {
            return Err(pyrust_core::py_err!(
                "AttributeError",
                "'object' object has no attribute '{name}'"
            ));
        }
        // PEP 3134: __cause__ and __context__ must be None or a
        // BaseException subclass instance.  __suppress_context__ must
        // be a bool.  CPython enforces these in the C slot setters;
        // not enforcing them makes pyrust silently accept bad values
        // that CPython raises TypeError for (issue #1066 review).
        let exception_slot = active_exception_slot_policy(&class, name);
        if let Some(policy) = exception_slot {
            let value = policy.prepare_assignment(self, value)?;
            instance.borrow_mut().attrs.insert_slot(name, value);
            return Ok(());
        }
        // Issue #1957: `obj.__class__ = NewType` re-types the instance
        // rather than storing a literal attribute (see `retype_instance`).
        // `__class__` is a type-level slot, not a per-instance attribute,
        // so it is handled *before* `__slots__` enforcement — a slotted
        // instance can still be re-typed.
        if name == "__class__" {
            return self.retype_instance(instance, value);
        }
        debug_assert!(exception_slot.is_none());
        // Issue #1106: if the class declares `__slots__`, only allow
        // assignment to names in the slot set.  When `__dict__` is
        // explicitly listed as a slot, arbitrary attribute assignment
        // is allowed (CPython behaviour).
        // Also skip when any ancestor class in the MRO has no `__slots__`:
        // a single unslotted ancestor reintroduces `__dict__` for all
        // subclasses (CPython rule).
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
        // Issue #1942: `instance.__dict__ = {...}` replaces the backing
        // attrs map wholesale rather than storing an attribute named
        // "__dict__".  Placed after slots enforcement so a slotted class
        // without a `__dict__` slot still raises AttributeError.
        if name == "__dict__" {
            return replace_instance_dict(instance, &value);
        }
        instance.borrow_mut().attrs.insert(name, value);
        Ok(())
    }

    /// `Cls.name = value` for a `PyClass` target. Split out of `assign_attr`.
    fn assign_attr_class(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        name: &str,
        value: Value,
    ) -> Result<()> {
        // A standard-library provider can attach native immutable-type policy
        // through its registered identity. Proper subclasses remain mutable.
        if let Some(error) =
            pyrust_builtins::ordered_mapping::immutable_class_attribute_error(class, name)
        {
            return Err(error);
        }
        // Primitive class singletons are shared across every
        // `Interpreter` on the same thread (per-thread
        // `PRIMITIVE_CLASSES` thread_local), so mutating their
        // attrs would leak state across runs.  Match CPython,
        // which raises TypeError on `int.x = 1`.  Copilot
        // review on #463.
        if crate::interpreter::is_primitive_class(class) {
            let n = class.borrow().name.clone();
            return Err(pyrust_core::type_err!(
                "cannot set '{name}' attribute of immutable type '{n}'"
            ));
        }
        // __dict__ is a read-only descriptor on type objects — CPython
        // raises AttributeError on direct assignment.
        if name == "__dict__" {
            return Err(pyrust_core::py_err!(
                "AttributeError",
                "attribute '__dict__' of 'type' objects is not writable"
            ));
        }
        // Issue #1970: __name__ is a type-level descriptor on `type` in
        // CPython — assigning it renames the class (updating the field the
        // __name__ getter and repr read), not the class attrs dict.
        // CPython requires a str; anything else raises TypeError.
        if name == "__name__" {
            let as_str: Option<String> = if let ValueKind::Str(s) = value.kind() {
                Some(s.to_string())
            } else {
                None
            };
            match as_str {
                Some(s) => {
                    class.borrow_mut().name = s;
                    return Ok(());
                }
                None => {
                    let type_name = pyrust_core::builtin_type_name(&value).into_owned();
                    return Err(pyrust_core::type_err!(
                        "can only assign string to {}.__name__, not '{}'",
                        class.borrow().name,
                        type_name,
                    ));
                }
            }
        }
        // Issue #553: __qualname__ is a type-level descriptor on `type`
        // in CPython — assigning it updates the descriptor slot, not the
        // class attrs dict.  CPython also requires the value to be a str.
        if name == "__qualname__" {
            // Extract the string while the kind() Ref is alive, then
            // drop the borrow before taking the error path.
            let as_str: Option<String> = if let ValueKind::Str(s) = value.kind() {
                Some(s.to_string())
            } else {
                None
            };
            match as_str {
                Some(s) => {
                    class.borrow_mut().qualname = s;
                    return Ok(());
                }
                None => {
                    let type_name = pyrust_core::builtin_type_name(&value).into_owned();
                    return Err(pyrust_core::type_err!(
                        "can only assign string to {}.__qualname__, not '{}'",
                        class.borrow().name,
                        type_name,
                    ));
                }
            }
        }
        {
            // Issue #2335: assigning `__new__` at runtime installs CPython's
            // sticky `slot_tp_new` wrapper.  The one exception is assigning
            // the *genuine* `object.__new__` to a class whose effective
            // `__new__` is still the genuine `object.__new__` — CPython's
            // `update_one_slot` leaves `tp_new == object_new` (no wrapper) in
            // that case, so excess-arg handling falls back to the bare-class /
            // `__init__`-based rules (`Cls.__new__ = object.__new__` with a
            // custom `__init__` accepts excess args; with no `__init__` it
            // raises `<Cls>() takes no arguments`).  Once the slot has ever
            // held any non-`object.__new__` value (a class-body `def __new__`,
            // a user-function assignment, or a `del`), the wrapper is sticky
            // and re-assigning `object.__new__` does NOT revert it.
            let setting_new = name == "__new__";
            let stays_unwrapped = setting_new
                && crate::interpreter::value_is_canonical_slot(
                    &value,
                    crate::interpreter::CanonicalSlot::ObjectNew,
                )
                && !class_chain_new_slot_wrapped(class)
                && lookup_class_attr(class, "__new__").is_none_or(|cur| {
                    crate::interpreter::value_is_canonical_slot(
                        &cur,
                        crate::interpreter::CanonicalSlot::ObjectNew,
                    )
                });
            let mut cls = class.borrow_mut();
            cls.attrs.insert(name.to_string(), value);
            cls.bump_mutation_version();
            if setting_new && !stays_unwrapped {
                cls.new_slot_wrapped.set(true);
            }
        }
        // Bump the global epoch so that caches keyed on subclasses of
        // this class (which only check their own mutation_version) also
        // invalidate — the epoch guard catches ancestor mutations that
        // the leaf-class version check would miss.
        pyrust_core::bump_class_epoch();
        Ok(())
    }

    /// `func.name = value` for a `UserFunction` target. Split out of `assign_attr`.
    fn assign_attr_function(func: &Rc<UserFunction>, name: &str, value: Value) -> Result<()> {
        // CPython allows assigning to __name__ and __qualname__ on
        // functions — both must be set to a str, otherwise TypeError.
        // __module__ and __doc__ accept any value (CPython imposes no
        // type constraint on these).
        // __dict__ must be set to a dict; any other name goes into
        // the function's dynamic attrs dict.
        match name {
            "__name__" | "__qualname__" => {
                let as_str: Option<String> = if let ValueKind::Str(s) = value.kind() {
                    Some(s.to_string())
                } else {
                    None
                };
                match as_str {
                    Some(s) => {
                        if name == "__name__" {
                            func.set_user_name(s);
                        } else {
                            func.set_user_qualname(s);
                        }
                        Ok(())
                    }
                    None => Err(pyrust_core::type_err!(
                        "{name} must be set to a string object"
                    )),
                }
            }
            "__module__" => {
                *func.module.borrow_mut() = value;
                Ok(())
            }
            "__doc__" => {
                *func.doc.borrow_mut() = value;
                Ok(())
            }
            "__dict__" => {
                // CPython requires the replacement to be a dict.
                if matches!(value.kind(), ValueKind::Dict(_)) {
                    // Replace the inner Value in place through the existing Rc so
                    // that any Rc clones (bound methods, etc.) see the new dict.
                    // If attrs was never initialised, just store a fresh Rc.
                    let mut slot = func.attrs.borrow_mut();
                    if let Some(rc) = slot.as_ref() {
                        *rc.borrow_mut() = value;
                    } else {
                        *slot = Some(Rc::new(RefCell::new(value)));
                    }
                    Ok(())
                } else {
                    let type_name = pyrust_core::builtin_type_name(&value);
                    Err(pyrust_core::type_err!(
                        "__dict__ must be set to a dictionary, not a '{type_name}'"
                    ))
                }
            }
            "__annotations__" => {
                // CPython accepts a dict or None; any other type raises TypeError.
                // Assigning None resets the annotations to an empty dict — CPython
                // stores None internally but the getter coerces it to {} on next
                // access.  We store the empty dict directly so that identity
                // semantics (`f.__annotations__ is f.__annotations__`) still hold.
                if matches!(value.kind(), ValueKind::Dict(_)) {
                    *func.annotations.borrow_mut() = value;
                    Ok(())
                } else if value.is_none() {
                    *func.annotations.borrow_mut() = Value::dict(PyDict::default());
                    Ok(())
                } else {
                    Err(pyrust_core::type_err!(
                        "__annotations__ must be set to a dict object"
                    ))
                }
            }
            // CPython validates these slots and rejects arbitrary values.
            // They are not yet implemented as real fields in pyrust, so
            // validate the type and silently succeed for accepted values
            // (pyrust is already in the "unset" state CPython would be
            // in after the assignment).
            "__code__" => Err(pyrust_core::type_err!(
                "__code__ must be set to a code object"
            )),
            "__defaults__" => {
                // #2395: CPython accepts None or a tuple; anything else →
                // TypeError.  Store the per-object override so subsequent calls
                // and `f.__defaults__` reads observe the reassignment.  A tuple
                // (even `()`) overrides the compile-time defaults; `None` clears
                // them.  CPython does not require the tuple length to match the
                // parameter count — it is aligned to the last n params at call
                // time (see `UserFunction::positional_default`).
                if value.is_none() || matches!(value.kind(), ValueKind::Tuple(_)) {
                    func.set_defaults_override(value);
                    Ok(())
                } else {
                    Err(pyrust_core::type_err!(
                        "__defaults__ must be set to a tuple object"
                    ))
                }
            }
            "__kwdefaults__" => {
                // #2395: CPython accepts None or a dict; anything else →
                // TypeError.  Store the per-object override (see `__defaults__`).
                if value.is_none() || matches!(value.kind(), ValueKind::Dict(_)) {
                    func.set_kwdefaults_override(value);
                    Ok(())
                } else {
                    Err(pyrust_core::type_err!(
                        "__kwdefaults__ must be set to a dict object"
                    ))
                }
            }
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
            _ => {
                // Arbitrary dynamic attribute — insert into the live dict,
                // initialising attrs lazily if this is the first write.
                let attrs_rc = func_attrs_rc(func);
                attrs_rc
                    .borrow()
                    .dict_insert(PyKey::str_from(name), value)
                    .map(|_| ())
            }
        }
    }
}
