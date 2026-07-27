// Attribute lookup entry points.
impl Interpreter {
    pub(crate) fn get_attr(&mut self, target: &Value, name: &str) -> Result<Value> {
        // `obj.__class__ is type(obj)` for every value (#2150).  PyInstance has
        // its own `__class__` handling (and `__class__`-reassignment semantics)
        // in `get_attr_instance_raw`, so it is excluded here; every other kind
        // — primitives, containers, None, functions, classes (→ metatype),
        // modules, generators, bound methods — resolves through `value_class`,
        // the same source of truth `type(obj)` uses.
        if name == "__class__" && !matches!(target.kind(), ValueKind::PyInstance(_)) {
            return Ok(value_class(target));
        }
        // `obj.__doc__` for a built-in data value (int/str/list/None/…) is the
        // value's *type* docstring (#2151): `(5).__doc__ is int.__doc__`.  Only
        // primitive/builtin data values are routed here; functions, modules,
        // classes, properties keep their own `__doc__` handling below.
        if name == "__doc__"
            && let Some(cls) = crate::interpreter::primitive_class_for_value(target)
        {
            return self.get_attr_class(cls, "__doc__");
        }
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                // CPython: before any other lookup, check whether type(obj) defines a
                // custom __getattribute__ (anything other than the builtin default on
                // `object`).  If so, call it.  The builtin default
                // (object.__getattribute__) is a BuiltinFunction; user-defined overrides
                // are UserFunction values — so dispatching only for UserFunction covers
                // exactly the "customised" case with no overhead on the common path.
                {
                    let class = { Rc::clone(&instance.borrow().class) };
                    if let Some(getattribute_val) = lookup_class_attr(&class, "__getattribute__")
                        && matches!(getattribute_val.kind(), ValueKind::UserFunction(_))
                    {
                        let result = invoke_class_method(
                            self,
                            getattribute_val,
                            Value::py_instance(Rc::clone(&instance)),
                            &[ExpandedCallArg {
                                name: None,
                                value: Value::string(name),
                            }],
                        );
                        return match result {
                            Err(ref e) if e.class_name_is("AttributeError") => {
                                // __getattribute__ raised AttributeError: fall
                                // through to __getattr__ if defined (CPython
                                // slot_tp_getattr_hook behaviour).
                                if let Some(getattr_val) = lookup_class_attr(&class, "__getattr__")
                                {
                                    invoke_class_method(
                                        self,
                                        getattr_val,
                                        Value::py_instance(Rc::clone(&instance)),
                                        &[ExpandedCallArg {
                                            name: None,
                                            value: Value::string(name),
                                        }],
                                    )
                                } else {
                                    result
                                }
                            }
                            other => other,
                        };
                    }
                }
                self.get_attr_instance_raw(instance, name)
            }
            ValueKind::PyClass(class) => self.get_attr_class(Rc::clone(class), name),
            ValueKind::SuperProxy { class, instance } => {
                let class = Rc::clone(class);
                let instance = Rc::clone(instance);
                // CPython exposes read-only `__thisclass__` / `__self__` /
                // `__self_class__` on every super object (#2704).
                match name {
                    "__thisclass__" => return Ok(Value::py_class(class)),
                    "__self__" => return Ok(Value::py_instance(instance)),
                    "__self_class__" => {
                        return Ok(Value::py_class(Rc::clone(&instance.borrow().class)));
                    }
                    _ => {}
                }
                // CPython super() semantics: look up `name` in the MRO of
                // type(instance), starting from the class *after* `class`.
                // This is necessary for cooperative multiple inheritance: when
                // B.method calls super().method, the next in D's MRO is C (not
                // A), so super() must walk D's full MRO rather than just B.base.
                let instance_class = Rc::clone(&instance.borrow().class);
                let mro = class_mro_items(&instance_class)?;
                // Find `class` in the MRO, then search from the next entry.
                let start = mro_search_start(&mro, &class);
                for mro_entry in mro.iter().skip(start) {
                    let entry_class = match mro_entry.kind() {
                        ValueKind::PyClass(c) => Rc::clone(c),
                        _ => continue,
                    };
                    let value = entry_class.borrow().attrs.get(name).cloned();
                    if let Some(value) = value {
                        let user_fn = match value.kind() {
                            ValueKind::UserFunction(f) => Some(Rc::clone(f)),
                            _ => None,
                        };
                        return Ok(match user_fn {
                            Some(f) => match f.kind {
                                UserFunctionKind::Regular => {
                                    Value::bound_method(Rc::clone(&f), instance)
                                }
                                UserFunctionKind::ClassMethod => {
                                    // CPython binds `cls` to type(instance) (the
                                    // runtime type of self), not the MRO entry
                                    // where the classmethod is defined (issue #2609).
                                    Value::class_bound_method(
                                        Rc::clone(&f),
                                        Rc::clone(&instance_class),
                                    )
                                }
                                UserFunctionKind::StaticMethod => {
                                    if let Some(inner) = f.wrapped_func.as_ref() {
                                        Value::user_function(Rc::clone(inner))
                                    } else {
                                        Value::with_function_kind(
                                            Rc::clone(&f),
                                            UserFunctionKind::Regular,
                                        )
                                    }
                                }
                                UserFunctionKind::Builtin(_) => Value::user_function(Rc::clone(&f)),
                            },
                            // Issue #988: bind BuiltinFunction sentinels (e.g.
                            // `list.__init__`) to the instance so that
                            // `call_function_expanded` prepends `self` before
                            // calling the registry dispatch.  A plain
                            // `BuiltinFunction` sentinel carries no self; we wrap
                            // it in a `super_bound_builtin` BuiltinObject that
                            // the interpreter intercepts in
                            // `call_function_expanded` before the registry probe.
                            None => match value.kind() {
                                ValueKind::BuiltinFunction(fn_name) => {
                                    pyrust_builtins::super_bound_builtin::super_bound_builtin(
                                        fn_name.to_string(),
                                        Value::py_instance(Rc::clone(&instance)),
                                    )
                                }
                                // CPython super.__getattribute__ runs the descriptor
                                // protocol on the resolved base-class attribute: a
                                // `property` / data / non-data descriptor has its
                                // `__get__(instance, type(instance))` invoked
                                // (issue #2598).  `instance` is the receiver, the
                                // owner is its concrete class.
                                _ if is_data_descriptor(&value)
                                    || is_non_data_descriptor(&value) =>
                                {
                                    return call_descriptor_get(
                                        self,
                                        &value,
                                        Value::py_instance(Rc::clone(&instance)),
                                        Value::py_class(Rc::clone(&instance_class)),
                                        name,
                                    );
                                }
                                _ => value.clone(),
                            },
                        });
                    }
                }
                Err(pyrust_core::py_err!(
                    "AttributeError",
                    "'super' object has no attribute '{name}'"
                ))
            }
            ValueKind::SuperProxyClass { class, obj_class } => {
                let class = Rc::clone(class);
                let obj_class = Rc::clone(obj_class);
                // CPython exposes read-only `__thisclass__` / `__self__` /
                // `__self_class__` on every super object (#2704).  `__self__` is
                // always `cls` (the second argument).  `__self_class__` mirrors
                // CPython's `supercheck` `obj_type` (#2712): in the standard
                // classmethod case `super(Base, Derived)` (Derived is a subclass
                // of Base) it is `Derived` itself, but in the metaclass branch
                // `super(Meta, cls)` (cls is an *instance* of Meta, so Meta is in
                // `type(cls)`'s MRO, not `cls`'s own MRO) it is `type(cls)`.
                match name {
                    "__thisclass__" => return Ok(Value::py_class(class)),
                    "__self__" => return Ok(Value::py_class(obj_class)),
                    "__self_class__" => {
                        let in_own_mro = class_mro_items(&obj_class)?.iter().any(
                            |v| matches!(v.kind(), ValueKind::PyClass(c) if Rc::ptr_eq(c, &class)),
                        );
                        let self_class = if in_own_mro {
                            obj_class
                        } else {
                            metaclass_of(&obj_class)
                        };
                        return Ok(Value::py_class(self_class));
                    }
                    _ => {}
                }
                // Two cases (issue #1956):
                //   1. classmethod super(): `class` is in `obj_class`'s own MRO.
                //      Walk `obj_class`'s MRO starting after `class`.
                //   2. metaclass-method super(): `class` (a metaclass) is in
                //      `type(obj_class)`'s MRO, not `obj_class`'s own MRO.  Walk
                //      the metaclass MRO ([Meta, type, object]) starting after
                //      `class`, but still bind `obj_class` (the class being
                //      operated on) as the receiver.
                let own_mro = class_mro_items(&obj_class)?;
                let class_in_own_mro = own_mro.iter().any(|v| match v.kind() {
                    ValueKind::PyClass(c) => Rc::ptr_eq(c, &class),
                    _ => false,
                });
                let mro = if class_in_own_mro {
                    own_mro
                } else {
                    class_mro_items(&metaclass_of(&obj_class))?
                };
                let start = mro_search_start(&mro, &class);
                for mro_entry in mro.iter().skip(start) {
                    let entry_class = match mro_entry.kind() {
                        ValueKind::PyClass(c) => Rc::clone(c),
                        _ => continue,
                    };
                    let value = entry_class.borrow().attrs.get(name).cloned();
                    if let Some(value) = value {
                        let user_fn = match value.kind() {
                            ValueKind::UserFunction(f) => Some(Rc::clone(f)),
                            _ => None,
                        };
                        return Ok(match user_fn {
                            Some(f) => match f.kind {
                                UserFunctionKind::Regular => {
                                    // Metaclass-method super(): `obj_class` (the
                                    // class being operated on) is the receiver
                                    // — it is an "instance" of the metaclass — so
                                    // `super().describe()` inside a metaclass
                                    // method must bind it, mirroring how a plain
                                    // `cls.describe()` binds `cls`.  The
                                    // classmethod-super case (`class_in_own_mro`)
                                    // keeps the unbound function, matching the
                                    // existing instance/classmethod behaviour.
                                    if class_in_own_mro {
                                        Value::user_function(Rc::clone(&f))
                                    } else {
                                        Value::class_bound_method(
                                            Rc::clone(&f),
                                            Rc::clone(&obj_class),
                                        )
                                    }
                                }
                                UserFunctionKind::ClassMethod => {
                                    Value::class_bound_method(Rc::clone(&f), Rc::clone(&obj_class))
                                }
                                UserFunctionKind::StaticMethod => {
                                    if let Some(inner) = f.wrapped_func.as_ref() {
                                        Value::user_function(Rc::clone(inner))
                                    } else {
                                        Value::with_function_kind(
                                            Rc::clone(&f),
                                            UserFunctionKind::Regular,
                                        )
                                    }
                                }
                                UserFunctionKind::Builtin(_) => Value::user_function(Rc::clone(&f)),
                            },
                            None => {
                                if let Some(wrapped) =
                                    pyrust_builtins::classmethod::as_class_method_any(&value)
                                {
                                    // Builtin classmethods are explicit
                                    // descriptors installed by their owner.
                                    pyrust_builtins::classmethod::bind_wrapped_class_method(
                                        wrapped,
                                        Rc::clone(&obj_class),
                                    )?
                                } else {
                                    // A class is an instance of its metaclass.
                                    // Ordinary builtin method descriptors found
                                    // through a metaclass super() bind to that
                                    // class. Through a regular classmethod
                                    // super() they remain unbound, exactly like
                                    // UserFunction::Regular above.
                                    let builtin_name = if class_in_own_mro {
                                        None
                                    } else {
                                        match value.kind() {
                                            ValueKind::BuiltinFunction(fn_name) => {
                                                Some(fn_name.to_string())
                                            }
                                            _ => None,
                                        }
                                    };
                                    if let Some(fn_name) = builtin_name {
                                        pyrust_builtins::super_bound_builtin::super_bound_builtin(
                                            fn_name,
                                            Value::py_class(Rc::clone(&obj_class)),
                                        )
                                    } else if is_data_descriptor(&value)
                                        || is_non_data_descriptor(&value)
                                    {
                                        // Descriptor protocol for class-level
                                        // super() access (issue #2598).
                                        return call_descriptor_get(
                                            self,
                                            &value,
                                            Value::none(),
                                            Value::py_class(Rc::clone(&obj_class)),
                                            name,
                                        );
                                    } else {
                                        value
                                    }
                                }
                            }
                        });
                    }
                }
                Err(pyrust_core::py_err!(
                    "AttributeError",
                    "'super' object has no attribute '{name}'"
                ))
            }
            ValueKind::SuperProxyUnbound { class } => {
                // The unbound `super(cls)` (#2704). It exposes the introspection
                // attributes (with `__self__` / `__self_class__` == None) and the
                // descriptor `__get__`, but cannot resolve methods until bound.
                match name {
                    "__thisclass__" => Ok(Value::py_class(Rc::clone(class))),
                    "__self__" | "__self_class__" => Ok(Value::none()),
                    "__get__" => Ok(pyrust_builtins::bound_method::bound_method(
                        "super.__get__",
                        target.clone(),
                    )),
                    _ => Err(pyrust_core::py_err!(
                        "AttributeError",
                        "'super' object has no attribute '{name}'"
                    )),
                }
            }
            // Access .setter / .deleter / .getter on a property descriptor itself.
            // These return a new property with the respective accessor replaced.
            _ if pyrust_builtins::property::property_partial_slot(target) == Some(None) => {
                self.get_property_attribute(target, name)
            }
            ValueKind::PyModule(module) => self.get_module_attribute(target, module, name),
            ValueKind::UserFunction(function) => {
                self.get_function_attribute(target, function, name)
            }
            ValueKind::BuiltinFunction(function_name) => {
                self.get_builtin_function_attribute(target, function_name, name)
            }
            ValueKind::BuiltinObject { .. } => self.get_builtin_object_attribute(target, name),
            ValueKind::Generator(state) => self.get_generator_attribute(target, state, name),
            _ => {
                if let Some(result) = self.try_get_bound_method_attribute(target, name) {
                    return result;
                }
                self.get_builtin_value_attribute(target, name)
            }
        }
    }
}
