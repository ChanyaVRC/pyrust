impl Interpreter {
    pub(crate) fn get_attr(&mut self, target: Value, name: &str) -> Result<Value> {
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if name == "__class__" {
                    return Ok(Value::py_class(Rc::clone(&instance.borrow().class)));
                }
                if name == "__dict__" {
                    // Shared with `vars(obj)` via `instance_attrs_snapshot`
                    // — see that helper for the live-vs-snapshot caveat
                    // (issue #392 follow-up).
                    return Ok(instance_attrs_snapshot(&instance));
                }

                // Check the class first for data descriptors (Property).  A data
                // descriptor takes priority over instance __dict__ — matching CPython.
                let class = { Rc::clone(&instance.borrow().class) };
                if let Some(class_val) = lookup_class_attr(&class, name)
                    && let Some((fget, partial_slot)) =
                        pyrust_builtins::property::with_property(&class_val, |s| {
                            (Rc::clone(&s.fget), s.partial_slot)
                        })
                    && partial_slot.is_none()
                {
                    return if fget.is_none() {
                        Err(PyError::named(
                            "AttributeError",
                            format!("property '{}' has no getter", name),
                        ))
                    } else {
                        let getter = (*fget).clone();
                        self.call_function_expanded(
                            getter,
                            &[ExpandedCallArg {
                                name: None,
                                value: Value::py_instance(Rc::clone(&instance)),
                            }],
                        )
                    };
                }

                if let Some(value) = instance.borrow().attrs.get(name).cloned() {
                    return Ok(value);
                }

                // Single MRO walk for the remaining cases.  Two things need
                // to happen here, and we don't want to walk the MRO twice in
                // the common (regular method/attr) path:
                //
                //   1. `cached_property` — non-data descriptor: only fires
                //      when the instance __dict__ doesn't already have the
                //      attribute (checked above).  Once it runs, the result
                //      is stashed into `instance.attrs` under the descriptor's
                //      own `attr_name` (set via `__set_name__`, or defaulted
                //      to the wrapped function's `__name__` at decoration
                //      time).  This matches CPython's
                //      `cached_property.__get__` semantics — the cache slot
                //      is whatever name `__set_name__` recorded, not the
                //      access-site name.  If `attr_name` differs from the
                //      access-site name the next access still hits the
                //      descriptor and recomputes (also matching CPython).
                //
                //   2. Regular dispatch — UserFunction → BoundMethod,
                //      BuiltinFunction → bound builtin, etc.
                if let Some(value) = lookup_class_attr(&class, name) {
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
                    // Probe kind tag in a scoped block so the `kind()` Ref
                    // drops before the `_ => value` arm may move `value`
                    // (#450).
                    enum AttrKind {
                        UserFunction(Rc<UserFunction>),
                        BuiltinFunction,
                        Other,
                    }
                    let tag = match value.kind() {
                        ValueKind::UserFunction(f) => AttrKind::UserFunction(Rc::clone(f)),
                        ValueKind::BuiltinFunction(_) => AttrKind::BuiltinFunction,
                        _ => AttrKind::Other,
                    };
                    return Ok(match tag {
                        AttrKind::UserFunction(f) => match f.kind {
                            UserFunctionKind::Regular => {
                                Value::bound_method(Rc::clone(&f), instance)
                            }
                            UserFunctionKind::ClassMethod => {
                                Value::class_bound_method(Rc::clone(&f), Rc::clone(&class))
                            }
                            UserFunctionKind::StaticMethod => {
                                Value::user_function(Rc::clone(&f))
                            }
                            UserFunctionKind::Builtin(_) => Value::user_function(Rc::clone(&f)),
                        },
                        AttrKind::BuiltinFunction => {
                            pyrust_builtins::bound_method::bound_method(
                                name.to_string(),
                                Value::py_instance(Rc::clone(&instance)),
                            )
                        }
                        AttrKind::Other => value,
                    });
                }

                // PEP 3134 attributes on exception instances default to
                // `None` when not yet set by the raise machinery.  This
                // mirrors CPython, where `BaseException.__context__` and
                // `BaseException.__cause__` are initialised to `None` on
                // every exception instance.
                if (name == "__context__"
                    || name == "__cause__"
                    || name == "__suppress_context__")
                    && is_exception_class(&class)
                {
                    return Ok(if name == "__suppress_context__" {
                        Value::bool_(false)
                    } else {
                        Value::none()
                    });
                }

                let class_name = class.borrow().name.clone();
                Err(PyError::named(
                    "AttributeError",
                    format!("'{}' object has no attribute '{}'", class_name, name),
                ))
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if name == "__name__" {
                    return Ok(Value::string(class.borrow().name.clone()));
                }
                if name == "__qualname__" {
                    // __qualname__ is a type-level descriptor on `type` in CPython,
                    // not stored in the class attrs dict.  Intercept here so that
                    // C.__qualname__ works without polluting vars(C) (issue #553).
                    return Ok(Value::string(class.borrow().qualname.clone()));
                }
                if name == "__dict__" {
                    // Return a live mappingproxy wrapping the class's attrs dict —
                    // matching CPython 3.12's `type.__dict__` descriptor, which
                    // returns `types.MappingProxyType`.  Reads see the current
                    // attrs (live reference); mutation raises TypeError (issue #726).
                    return Ok(pyrust_builtins::mapping_proxy::mapping_proxy(
                        Rc::clone(&class),
                    ));
                }
                if name == "__bases__" {
                    // `__bases__` reports the immediate parents — a 1-tuple
                    // containing `base` if set, else a 1-tuple containing
                    // the synthetic `object` (CPython: every class without
                    // an explicit base is `(object,)`).  Multi-inheritance
                    // isn't modelled — `PyClass.base` is single-valued.
                    let base = class.borrow().base.clone();
                    let parent = base.unwrap_or_else(object_class_singleton);
                    return Ok(Value::tuple(vec![Value::py_class(parent)]));
                }
                if name == "__mro__" {
                    // Walk the single-inheritance `base` chain to build the
                    // MRO tuple, terminating in the synthetic `object` class.
                    // Multi-inheritance (C3 linearization) is not yet
                    // implemented — pyrust's `PyClass` only stores a single
                    // `base` pointer, so a true C3 walk would have nothing
                    // extra to traverse.
                    let mut items: Vec<Value> = Vec::new();
                    let mut cur = Some(Rc::clone(&class));
                    while let Some(c) = cur {
                        items.push(Value::py_class(Rc::clone(&c)));
                        cur = c.borrow().base.clone();
                    }
                    items.push(Value::py_class(object_class_singleton()));
                    return Ok(Value::tuple(items));
                }
                if name == "__annotations__" {
                    // `type.__annotations__` in CPython is a data descriptor on
                    // `type` itself.  On first access it synthesises an empty dict,
                    // writes it back into the class's own `__dict__`, and returns
                    // that same dict.  Subsequent accesses hit the own-attrs check
                    // and return the stored (potentially mutated) dict — so
                    // `Foo.__annotations__ is Foo.__annotations__` is `True` and
                    // mutations via subscript-assignment persist (issue #737).
                    //
                    // CPython does NOT inherit __annotations__ from base classes:
                    // `B.__annotations__` is always B's own dict, never A's.
                    // Use a direct own-attrs lookup (not lookup_class_attr) here.
                    if let Some(stored) = class.borrow().attrs.get("__annotations__").cloned() {
                        return Ok(stored);
                    }
                    let empty = Value::dict(IndexMap::new());
                    class
                        .borrow_mut()
                        .attrs
                        .insert("__annotations__".to_string(), empty.clone());
                    return Ok(empty);
                }
                if let Some(value) = lookup_class_attr(&class, name) {
                    // Drop the kind() Ref before the `_ => value` arm
                    // may move `value` (#450).
                    let user_fn = match value.kind() {
                        ValueKind::UserFunction(f) => Some(Rc::clone(f)),
                        _ => None,
                    };
                    return Ok(match user_fn {
                        Some(f) => match f.kind {
                            UserFunctionKind::ClassMethod => {
                                Value::class_bound_method(Rc::clone(&f), Rc::clone(&class))
                            }
                            UserFunctionKind::StaticMethod => {
                                Value::user_function(Rc::clone(&f))
                            }
                            UserFunctionKind::Regular => value,
                            UserFunctionKind::Builtin(_) => value,
                        },
                        None => value,
                    });
                }
                let class_name = class.borrow().name.clone();
                Err(PyError::named(
                    "AttributeError",
                    format!("type object '{}' has no attribute '{}'", class_name, name),
                ))
            }
            ValueKind::SuperProxy { class, instance } => {
                let class = Rc::clone(class);
                let instance = Rc::clone(instance);
                // Look up the method starting from class's parent (skip class itself)
                let parent = class.borrow().base.clone();
                let Some(parent_class) = parent else {
                    return Err(PyError::named(
                        "AttributeError",
                        format!("super(): '{}' has no base class", class.borrow().name),
                    ));
                };
                if let Some(value) = lookup_class_attr(&parent_class, name) {
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
                                Value::class_bound_method(Rc::clone(&f), parent_class)
                            }
                            UserFunctionKind::StaticMethod => {
                                Value::user_function(Rc::clone(&f))
                            }
                            UserFunctionKind::Builtin(_) => Value::user_function(Rc::clone(&f)),
                        },
                        None => value,
                    });
                }
                Err(PyError::named(
                    "AttributeError",
                    format!("super(): parent class has no attribute '{name}'"),
                ))
            }
            ValueKind::SuperProxyClass { class, obj_class } => {
                let class = Rc::clone(class);
                let obj_class = Rc::clone(obj_class);
                // classmethod super(): look up from class's parent and bind to obj_class
                let parent = class.borrow().base.clone();
                let Some(parent_class) = parent else {
                    return Err(PyError::named(
                        "AttributeError",
                        format!("super(): '{}' has no base class", class.borrow().name),
                    ));
                };
                if let Some(value) = lookup_class_attr(&parent_class, name) {
                    let user_fn = match value.kind() {
                        ValueKind::UserFunction(f) => Some(Rc::clone(f)),
                        _ => None,
                    };
                    return Ok(match user_fn {
                        Some(f) => match f.kind {
                            UserFunctionKind::Regular => {
                                Value::user_function(Rc::clone(&f))
                            }
                            UserFunctionKind::ClassMethod => {
                                Value::class_bound_method(Rc::clone(&f), obj_class)
                            }
                            UserFunctionKind::StaticMethod => {
                                Value::user_function(Rc::clone(&f))
                            }
                            UserFunctionKind::Builtin(_) => Value::user_function(Rc::clone(&f)),
                        },
                        None => value,
                    });
                }
                Err(PyError::named(
                    "AttributeError",
                    format!("super(): parent class has no attribute '{name}'"),
                ))
            }
            // Access .setter / .deleter / .getter on a property descriptor itself.
            // These return a new property with the respective accessor replaced.
            _ if pyrust_builtins::property::property_partial_slot(&target)
                == Some(None) =>
            {
                let (fget_val, fset_val, fdel_val) =
                    pyrust_builtins::property::with_property(&target, |s| {
                        ((*s.fget).clone(), (*s.fset).clone(), (*s.fdel).clone())
                    })
                    .expect("guard checked above");
                match name {
                    "setter" => Ok(pyrust_builtins::property::property_setter_partial(
                        fget_val, fdel_val,
                    )),
                    "deleter" => Ok(pyrust_builtins::property::property_deleter_partial(
                        fget_val, fset_val,
                    )),
                    "getter" => Ok(pyrust_builtins::property::property_getter_partial(
                        fset_val, fdel_val,
                    )),
                    "fget" => Ok(fget_val),
                    "fset" => Ok(fset_val),
                    "fdel" => Ok(fdel_val),
                    _ => Err(PyError::named(
                        "AttributeError",
                        format!("property object has no attribute '{name}'"),
                    )),
                }
            }
            ValueKind::PyModule(module) => {
                let module = Rc::clone(module);
                if let Some(value) = module.borrow().attrs.get(name).cloned() {
                    return Ok(value);
                }
                let mod_name = module.borrow().name.clone();
                Err(PyError::named(
                    "AttributeError",
                    format!("module '{mod_name}' has no attribute '{name}'"),
                ))
            }
            ValueKind::UserFunction(func) => {
                match name {
                    "__name__" => {
                        let n = func
                            .user_name
                            .borrow()
                            .as_deref()
                            .unwrap_or(&func.name)
                            .to_string();
                        return Ok(Value::string(n));
                    }
                    "__qualname__" => {
                        let q = func
                            .user_qualname
                            .borrow()
                            .as_deref()
                            .unwrap_or(&func.qualname)
                            .to_string();
                        return Ok(Value::string(q));
                    }
                    "__module__" => return Ok(func.module.borrow().clone()),
                    "__doc__" => return Ok(func.doc.borrow().clone()),
                    "__dict__" => {
                        // Return the live dict object — CPython returns the same
                        // object every time, so `d = f.__dict__; d['x'] = 1`
                        // makes `f.x` visible.  Initialise lazily on first access.
                        let attrs_rc = func_attrs_rc(func);
                        return Ok(attrs_rc.borrow().clone());
                    }
                    "__annotations__" => {
                        // Return the stored dict Value directly (Rc-clone) so that
                        // repeated reads yield the same object identity, matching
                        // CPython: `f.__annotations__ is f.__annotations__` is True.
                        return Ok(func.annotations.borrow().clone());
                    }
                    _ => {}
                }
                // Fall through to arbitrary dynamic attrs.
                // Short-circuit without initialising if no attrs have been stored yet.
                if let Some(rc) = func.attrs.borrow().as_ref().map(Rc::clone) {
                    if let Some(v) = rc.borrow().as_dict().and_then(|d| d.get(&StrKey(name)).cloned()) {
                        return Ok(v);
                    }
                }
                Err(PyError::named(
                    "AttributeError",
                    format!("'function' object has no attribute '{name}'"),
                ))
            }
            ValueKind::BuiltinFunction(func_name) => {
                // __name__ / __qualname__ / __module__ on builtin functions.
                //
                // func_name is stored as the qualified form — e.g. "str.upper"
                // for method-style builtins and "print" for top-level ones.
                //
                // __name__     → bare name after the last '.':
                //                "str.upper" → "upper", "print" → "print"
                // __qualname__ → func_name as-is (already the dotted form)
                // __module__   → "builtins" for top-level (non-dotted) builtins
                //                only; CPython's method_descriptor (e.g.
                //                str.upper, list.append) raises AttributeError
                //                for __module__, while builtin_function_or_method
                //                (print, len) exposes it.
                if name == "__name__" {
                    let bare = func_name.rsplit('.').next().unwrap_or(func_name);
                    return Ok(Value::string(bare));
                }
                if name == "__qualname__" {
                    return Ok(Value::string(func_name));
                }
                if name == "__module__" {
                    if func_name.contains('.') {
                        // Method descriptors (str.upper, list.append, …) do not
                        // expose __module__; CPython raises AttributeError with
                        // "'method_descriptor' object has no attribute '__module__'"
                        return Err(PyError::named(
                            "AttributeError",
                            format!("'method_descriptor' object has no attribute '__module__'"),
                        ));
                    }
                    return Ok(Value::string("builtins"));
                }
                if func_name == "str" {
                    match name {
                        "lower"      => Ok(Value::builtin_function("str.lower")),
                        "upper"      => Ok(Value::builtin_function("str.upper")),
                        "strip"      => Ok(Value::builtin_function("str.strip")),
                        "lstrip"     => Ok(Value::builtin_function("str.lstrip")),
                        "rstrip"     => Ok(Value::builtin_function("str.rstrip")),
                        "capitalize" => Ok(Value::builtin_function("str.capitalize")),
                        "split"      => Ok(Value::builtin_function("str.split")),
                        "join"       => Ok(Value::builtin_function("str.join")),
                        "replace"    => Ok(Value::builtin_function("str.replace")),
                        "find"       => Ok(Value::builtin_function("str.find")),
                        "rfind"      => Ok(Value::builtin_function("str.rfind")),
                        "index"      => Ok(Value::builtin_function("str.index")),
                        "rindex"     => Ok(Value::builtin_function("str.rindex")),
                        "count"      => Ok(Value::builtin_function("str.count")),
                        "startswith" => Ok(Value::builtin_function("str.startswith")),
                        "endswith"   => Ok(Value::builtin_function("str.endswith")),
                        "format"     => Ok(Value::builtin_function("str.format")),
                        "format_map" => Ok(Value::builtin_function("str.format_map")),
                        "isdigit"    => Ok(Value::builtin_function("str.isdigit")),
                        "isalpha"    => Ok(Value::builtin_function("str.isalpha")),
                        "isalnum"    => Ok(Value::builtin_function("str.isalnum")),
                        "isspace"    => Ok(Value::builtin_function("str.isspace")),
                        _ => Err(PyError::named(
                            "AttributeError",
                            format!("type object 'str' has no attribute '{name}'"),
                        )),
                    }
                } else {
                    Err(PyError::named(
                        "AttributeError",
                        format!("type object '{}' has no attribute '{name}'", func_name),
                    ))
                }
            }
            ValueKind::BuiltinObject { .. } => {
                // Builtin bound methods (BuiltinObject with BoundMethodState):
                // expose __name__, __qualname__, __self__, __module__, __doc__
                // to match CPython's builtin_function_or_method attributes.
                // __module__ is always None for built-in bound methods (CPython
                // does not set m_module on method_descriptor objects).
                //
                // Kept in its own arm so the as_bound_method check is only
                // reached for BuiltinObject values — not for List/Dict/Set/etc.
                // (which previously fell through to _ and paid the check cost
                // on every method lookup like lst.append).
                if let Some((method_name, receiver)) =
                    pyrust_builtins::bound_method::as_bound_method(&target)
                {
                    match name {
                        "__name__" => return Ok(Value::string(method_name.as_str())),
                        "__qualname__" => {
                            let type_name = pyrust_core::builtin_type_name(&receiver);
                            return Ok(Value::string(format!("{type_name}.{method_name}")));
                        }
                        "__self__" => return Ok(receiver),
                        "__module__" => return Ok(Value::none()),
                        "__doc__" => return Ok(Value::none()),
                        _ => {}
                    }
                }
                // Non-bound-method BuiltinObjects (GenericAlias, frozenset,
                // dict views, file, enumerate, zip, reversed, chain,
                // cached_property, …) also reach this arm.
                // First probe the type's custom `getattr` (e.g. GenericAlias
                // exposes `__origin__` and `__args__` this way), then fall
                // back to builtin method lookup.
                if let ValueKind::BuiltinObject { ops, state } = target.kind() {
                    if let Some(val) = ops.getattr(state, name) {
                        return Ok(val);
                    }
                }
                if builtin_has_method(&target, name) {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        name,
                        target.clone(),
                    ));
                }
                let type_name = pyrust_core::builtin_type_name(&target);
                Err(PyError::named(
                    "AttributeError",
                    format!("'{type_name}' object has no attribute '{name}'"),
                ))
            }
            _ => {
                // BoundMethod / ClassBoundMethod: delegate __name__ / __qualname__ /
                // __module__ / __doc__ / __dict__ and arbitrary dynamic attrs to the
                // underlying function, matching CPython's method proxy semantics.
                match target.kind() {
                    ValueKind::BoundMethod { function, .. }
                    | ValueKind::ClassBoundMethod { function, .. } => match name {
                        "__name__" => {
                            let n = function
                                .user_name
                                .borrow()
                                .as_deref()
                                .unwrap_or(&function.name)
                                .to_string();
                            return Ok(Value::string(n));
                        }
                        "__qualname__" => {
                            let q = function
                                .user_qualname
                                .borrow()
                                .as_deref()
                                .unwrap_or(&function.qualname)
                                .to_string();
                            return Ok(Value::string(q));
                        }
                        "__module__" => return Ok(function.module.borrow().clone()),
                        "__doc__" => return Ok(function.doc.borrow().clone()),
                        "__dict__" => {
                            // Live dict object — same semantics as UserFunction.
                            let attrs_rc = func_attrs_rc(function);
                            return Ok(attrs_rc.borrow().clone());
                        }
                        _ => {
                            // Arbitrary dynamic attrs delegate to the underlying function.
                            // Short-circuit without initialising if no attrs set yet.
                            if let Some(rc) = function.attrs.borrow().as_ref().map(Rc::clone) {
                                if let Some(v) =
                                    rc.borrow().as_dict().and_then(|d| d.get(&StrKey(name)).cloned())
                                {
                                    return Ok(v);
                                }
                            }
                        }
                    },
                    _ => {}
                }
                // Complex .real / .imag attribute access.
                if let ValueKind::Complex(re, im) = target.kind() {
                    match name {
                        "real" => return Ok(Value::float(re)),
                        "imag" => return Ok(Value::float(im)),
                        "conjugate" => {
                            return Ok(pyrust_builtins::bound_method::bound_method(
                                name,
                                target.clone(),
                            ));
                        }
                        _ => {}
                    }
                }
                // `BuiltinObject` types can expose arbitrary attributes via
                // `BuiltinTypeOps::getattr` (e.g. `GenericAlias.__origin__`,
                // `GenericAlias.__args__`).  Probe before the generic
                // `has_method` bound-method path.
                if let ValueKind::BuiltinObject { ops, state } = target.kind() {
                    if let Some(val) = ops.getattr(state, name) {
                        return Ok(val);
                    }
                }
                // Built-in type instance method lookup: list.append, str.upper, etc.
                if builtin_has_method(&target, name) {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        name,
                        target.clone(),
                    ));
                }
                // Fallback: check the primitive class attrs for classmethods
                // accessible on instances (e.g. `(1.0).fromhex`).
                if let Some(cls) = crate::interpreter::primitive_class_for_value(&target) {
                    if let Some(val) = lookup_class_attr(&cls, name) {
                        return Ok(val);
                    }
                }
                let type_name = pyrust_core::builtin_type_name(&target);
                Err(PyError::named(
                    "AttributeError",
                    format!("'{type_name}' object has no attribute '{name}'"),
                ))
            }
        }
    }

    pub(crate) fn assign_attr(&mut self, target: Value, name: &str, value: Value) -> Result<()> {
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                // Check for a property descriptor in the class chain.
                let class = { Rc::clone(&instance.borrow().class) };
                if let Some(class_val) = lookup_class_attr(&class, name)
                    && let Some((fset, partial_slot)) =
                        pyrust_builtins::property::with_property(&class_val, |s| {
                            (Rc::clone(&s.fset), s.partial_slot)
                        })
                    && partial_slot.is_none()
                {
                    return if fset.is_none() {
                        Err(PyError::named(
                            "AttributeError",
                            format!("property '{}' has no setter", name),
                        ))
                    } else {
                        let setter = (*fset).clone();
                        self.call_function_expanded(
                            setter,
                            &[
                                ExpandedCallArg {
                                    name: None,
                                    value: Value::py_instance(Rc::clone(instance)),
                                },
                                ExpandedCallArg { name: None, value },
                            ],
                        )?;
                        Ok(())
                    };
                }
                // Check for a `__setattr__` on the class chain.  CPython calls
                // it for every attribute assignment on instances that define it.
                // We skip this for property descriptors (handled above) and
                // only check when the class has an explicit `__setattr__`
                // (not `object.__setattr__`, which we don't model as a class
                // method).
                if let Some(setattr_val) = lookup_class_attr(&class, "__setattr__") {
                    return invoke_class_method(
                        self,
                        setattr_val,
                        Value::py_instance(Rc::clone(instance)),
                        &[
                            ExpandedCallArg {
                                name: None,
                                value: Value::string(name.to_string()),
                            },
                            ExpandedCallArg { name: None, value },
                        ],
                    )
                    .map(|_| ());
                }
                instance.borrow_mut().attrs.insert(name.to_string(), value);
                Ok(())
            }
            ValueKind::PyClass(class) => {
                // Primitive class singletons are shared across every
                // `Interpreter` on the same thread (per-thread
                // `PRIMITIVE_CLASSES` thread_local), so mutating their
                // attrs would leak state across runs.  Match CPython,
                // which raises TypeError on `int.x = 1`.  Copilot
                // review on #463.
                if crate::interpreter::is_primitive_class(class) {
                    let n = class.borrow().name.clone();
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "cannot set '{name}' attribute of immutable type '{n}'"
                        ),
                    ));
                }
                // __dict__ is a read-only descriptor on type objects — CPython
                // raises AttributeError on direct assignment.
                if name == "__dict__" {
                    return Err(PyError::named(
                        "AttributeError",
                        "attribute '__dict__' of 'type' objects is not writable".to_string(),
                    ));
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
                            let type_name =
                                pyrust_core::builtin_type_name(&value).into_owned();
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "can only assign string to {}.__qualname__, not '{}'",
                                    class.borrow().name,
                                    type_name,
                                ),
                            ));
                        }
                    }
                }
                class.borrow_mut().attrs.insert(name.to_string(), value);
                Ok(())
            }
            ValueKind::UserFunction(func) => {
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
                                    *func.user_name.borrow_mut() = Some(s);
                                } else {
                                    *func.user_qualname.borrow_mut() = Some(s);
                                }
                                Ok(())
                            }
                            None => Err(PyError::named(
                                "TypeError",
                                format!("{name} must be set to a string object"),
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
                            Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__dict__ must be set to a dictionary, not a '{type_name}'"
                                ),
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
                            *func.annotations.borrow_mut() =
                                Value::dict(indexmap::IndexMap::new());
                            Ok(())
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                "__annotations__ must be set to a dict object".to_string(),
                            ))
                        }
                    }
                    // CPython validates these slots and rejects arbitrary values.
                    // They are not yet implemented as real fields in pyrust, so
                    // validate the type and silently succeed for accepted values
                    // (pyrust is already in the "unset" state CPython would be
                    // in after the assignment).
                    "__code__" => Err(PyError::named(
                        "TypeError",
                        "__code__ must be set to a code object".to_string(),
                    )),
                    "__defaults__" => {
                        // CPython accepts None or a tuple; anything else → TypeError.
                        if value.is_none() || matches!(value.kind(), ValueKind::Tuple(_)) {
                            Ok(())
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                "__defaults__ must be set to a tuple object".to_string(),
                            ))
                        }
                    }
                    "__kwdefaults__" => {
                        // CPython accepts None or a dict; anything else → TypeError.
                        if value.is_none() || matches!(value.kind(), ValueKind::Dict(_)) {
                            Ok(())
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                "__kwdefaults__ must be set to a dict object".to_string(),
                            ))
                        }
                    }
                    "__globals__" | "__closure__" => Err(PyError::named(
                        "AttributeError",
                        "readonly attribute".to_string(),
                    )),
                    _ => {
                        // Arbitrary dynamic attribute — insert into the live dict,
                        // initialising attrs lazily if this is the first write.
                        let attrs_rc = func_attrs_rc(func);
                        attrs_rc
                            .borrow()
                            .dict_insert(PyKey::Str(name.to_string()), value)
                            .map(|_| ())
                    }
                }
            }
            ValueKind::BuiltinFunction(func_name) => {
                // CPython distinguishes two cases:
                //
                // builtin_function_or_method (non-dotted, e.g. print, len):
                //   - __module__ is the only writable attribute; CPython stores it in
                //     PyCFunctionObject.m_module.  pyrust does not yet have mutable
                //     per-instance storage for BuiltinFunction values, so we cannot
                //     honour the write today — but we raise AttributeError (not
                //     RuntimeError) and use the correct CPython message so the error
                //     class is right.  Follow-up: add mutable __module__ storage.
                //   - __name__, __qualname__, __doc__ are read-only:
                //     AttributeError: attribute '__X__' of
                //     'builtin_function_or_method' objects is not writable
                //   - anything else:
                //     AttributeError: 'builtin_function_or_method' object has no
                //     attribute 'X'
                //
                // method_descriptor (dotted, e.g. str.upper, list.append):
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
                if func_name.contains('.') {
                    // method_descriptor path
                    match name {
                        "__module__" => Err(PyError::named(
                            "AttributeError",
                            format!("'method_descriptor' object has no attribute '__module__'"),
                        )),
                        "__name__" => Err(PyError::named(
                            "AttributeError",
                            "readonly attribute".to_string(),
                        )),
                        "__qualname__" | "__doc__" => Err(PyError::named(
                            "AttributeError",
                            format!(
                                "attribute '{name}' of 'method_descriptor' objects is not writable"
                            ),
                        )),
                        _ => Err(PyError::named(
                            "AttributeError",
                            format!("'method_descriptor' object has no attribute '{name}'"),
                        )),
                    }
                } else {
                    // builtin_function_or_method path
                    match name {
                        "__module__" => {
                            // CPython allows this write; pyrust lacks per-instance storage.
                            // Raise AttributeError so the error class is correct until
                            // mutable __module__ storage is added to BuiltinFunction.
                            Err(PyError::named(
                                "AttributeError",
                                format!(
                                    "attribute '__module__' of 'builtin_function_or_method' \
                                     objects is not writable"
                                ),
                            ))
                        }
                        "__name__" | "__qualname__" | "__doc__" => Err(PyError::named(
                            "AttributeError",
                            format!(
                                "attribute '{name}' of 'builtin_function_or_method' \
                                 objects is not writable"
                            ),
                        )),
                        _ => Err(PyError::named(
                            "AttributeError",
                            format!("'builtin_function_or_method' object has no attribute '{name}'"),
                        )),
                    }
                }
            }
            ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
                // CPython raises AttributeError (not a generic RuntimeError) when
                // you try to set any attribute on a bound method object.
                Err(PyError::named(
                    "AttributeError",
                    format!("'method' object has no attribute '{name}'"),
                ))
            }
            ValueKind::BuiltinFunction(_) => {
                // CPython: builtin_function_or_method objects have no __dict__
                // and do not support arbitrary attribute assignment.
                Err(PyError::named(
                    "AttributeError",
                    format!(
                        "'builtin_function_or_method' object has no attribute '{name}'"
                    ),
                ))
            }
            _ => Err(PyError::Runtime(format!(
                "object has no writable attribute '{}'",
                name
            ))),
        }
    }

    pub(crate) fn delete_attr(&mut self, target: Value, name: &str) -> Result<()> {
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                let class = { Rc::clone(&instance.borrow().class) };
                if let Some(class_val) = lookup_class_attr(&class, name)
                    && let Some((fdel, partial_slot)) =
                        pyrust_builtins::property::with_property(&class_val, |s| {
                            (Rc::clone(&s.fdel), s.partial_slot)
                        })
                    && partial_slot.is_none()
                {
                    return if fdel.is_none() {
                        Err(PyError::named(
                            "AttributeError",
                            format!("property '{}' has no deleter", name),
                        ))
                    } else {
                        let deleter = (*fdel).clone();
                        self.call_function_expanded(
                            deleter,
                            &[ExpandedCallArg {
                                name: None,
                                value: Value::py_instance(Rc::clone(instance)),
                            }],
                        )?;
                        Ok(())
                    };
                }
                // `shift_remove` keeps the remaining entries in their
                // original insertion order so `vars(obj)` after `del obj.x`
                // still matches CPython's stable ordering contract.
                // CPython raises AttributeError when the attribute is absent.
                if instance.borrow_mut().attrs.shift_remove(name).is_none() {
                    let class_name = instance.borrow().class.borrow().name.clone();
                    return Err(PyError::named(
                        "AttributeError",
                        format!("'{class_name}' object has no attribute '{name}'"),
                    ));
                }
                Ok(())
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
                    "__name__" | "__qualname__" => Err(PyError::named(
                        "TypeError",
                        format!("{name} must be set to a string object"),
                    )),
                    "__module__" => {
                        *func.module.borrow_mut() = Value::none();
                        Ok(())
                    }
                    "__doc__" => {
                        *func.doc.borrow_mut() = Value::none();
                        Ok(())
                    }
                    "__dict__" => Err(PyError::named(
                        "TypeError",
                        "cannot delete __dict__".to_string(),
                    )),
                    "__annotations__" => {
                        // CPython allows `del f.__annotations__`; it resets the
                        // dict to a fresh empty dict (new object).
                        *func.annotations.borrow_mut() =
                            Value::dict(indexmap::IndexMap::new());
                        Ok(())
                    }
                    // CPython-matched behaviour for validated-but-unimplemented slots.
                    "__code__" => Err(PyError::named(
                        "TypeError",
                        "__code__ must be set to a code object".to_string(),
                    )),
                    "__globals__" | "__closure__" => Err(PyError::named(
                        "AttributeError",
                        "readonly attribute".to_string(),
                    )),
                    // CPython allows `del f.__defaults__` / `del f.__kwdefaults__`
                    // (they reset to None).  Since pyrust doesn't implement these slots
                    // yet, silently succeed — the state the caller intended (unset)
                    // already matches pyrust's state.
                    "__defaults__" | "__kwdefaults__" => Ok(()),
                    _ => {
                        // Short-circuit: if attrs were never initialised, there
                        // is nothing to delete — raise AttributeError immediately.
                        let key = PyKey::Str(name.to_string());
                        let removed = func
                            .attrs
                            .borrow()
                            .as_ref()
                            .and_then(|rc| rc.borrow().dict_shift_remove(&key).ok().flatten());
                        if removed.is_some() {
                            Ok(())
                        } else {
                            Err(PyError::named(
                                "AttributeError",
                                format!("'function' object has no attribute '{name}'"),
                            ))
                        }
                    }
                }
            }
            ValueKind::PyClass(class) => {
                // __dict__ is a read-only descriptor on type objects — CPython
                // raises AttributeError on `del C.__dict__`.
                if name == "__dict__" {
                    return Err(PyError::named(
                        "AttributeError",
                        "attribute '__dict__' of 'type' objects is not writable".to_string(),
                    ));
                }
                // Issue #553: __qualname__ is a type-level descriptor on `type`
                // in CPython — you cannot delete it.  CPython raises TypeError.
                if name == "__qualname__" {
                    let n = class.borrow().name.clone();
                    return Err(PyError::named(
                        "TypeError",
                        format!("cannot delete '__qualname__' attribute of immutable type '{n}'"),
                    ));
                }
                // Issue #737: `del Cls.__annotations__` must raise
                // `AttributeError` when no annotations dict has been
                // materialised yet — matching CPython's descriptor, which
                // refuses to delete a slot that was never written.
                if name == "__annotations__"
                    && !class.borrow().attrs.contains_key("__annotations__")
                {
                    return Err(PyError::named(
                        "AttributeError",
                        "__annotations__".to_string(),
                    ));
                }
                // CPython raises AttributeError when the attribute is absent.
                if class.borrow_mut().attrs.shift_remove(name).is_none() {
                    let class_name = class.borrow().name.clone();
                    return Err(PyError::named(
                        "AttributeError",
                        format!("type object '{class_name}' has no attribute '{name}'"),
                    ));
                }
                Ok(())
            }
            ValueKind::BuiltinFunction(func_name) => {
                // Mirror the assign_attr logic: method_descriptors and
                // builtin_function_or_method both raise AttributeError, matching
                // CPython exactly.  (Mutable __module__ storage is a follow-up.)
                if func_name.contains('.') {
                    match name {
                        "__module__" => Err(PyError::named(
                            "AttributeError",
                            format!("'method_descriptor' object has no attribute '__module__'"),
                        )),
                        "__name__" => Err(PyError::named(
                            "AttributeError",
                            "readonly attribute".to_string(),
                        )),
                        "__qualname__" | "__doc__" => Err(PyError::named(
                            "AttributeError",
                            format!(
                                "attribute '{name}' of 'method_descriptor' objects is not writable"
                            ),
                        )),
                        _ => Err(PyError::named(
                            "AttributeError",
                            format!("'method_descriptor' object has no attribute '{name}'"),
                        )),
                    }
                } else {
                    match name {
                        "__module__" | "__name__" | "__qualname__" | "__doc__" => {
                            Err(PyError::named(
                                "AttributeError",
                                format!(
                                    "attribute '{name}' of 'builtin_function_or_method' \
                                     objects is not writable"
                                ),
                            ))
                        }
                        _ => Err(PyError::named(
                            "AttributeError",
                            format!(
                                "'builtin_function_or_method' object has no attribute '{name}'"
                            ),
                        )),
                    }
                }
            }
            ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
                // CPython raises AttributeError when deleting any attribute on a
                // bound method object.
                Err(PyError::named(
                    "AttributeError",
                    format!("'method' object has no attribute '{name}'"),
                ))
            }
            _ => Err(PyError::Runtime(
                "can only delete attributes of class instances".to_string(),
            )),
        }
    }

    fn load_module(&mut self, name: &str) -> Result<Value> {
        if let Some(cached) = self.module_cache.borrow().get(name).cloned() {
            return Ok(cached);
        }
        // Built-in modules — declared in
        // `crates/pyrust/src/builtin_modules/mod.rs::pyrust_builtin_modules!`.
        // Adding a new module is a single-line edit there; this file
        // never has to change.
        let builtin = crate::builtin_modules::load_builtin_module(name);
        if let Some(val) = builtin {
            // `builtins` post-process: replace the auto-generated
            // `BuiltinFunction("int")` / `"str"` / etc. attrs with the
            // per-thread `PyClass` singletons so that
            // `builtins.int is int` and `isinstance(5, builtins.int)`
            // match the global lookup path (issue #462; Copilot review
            // on #463).
            if name == "builtins"
                && let ValueKind::PyModule(m) = val.kind()
            {
                for prim in [
                    "bool", "bytes", "complex", "dict", "float", "frozenset",
                    "int", "list", "set", "str", "tuple",
                ] {
                    if let Some(class) =
                        crate::interpreter::primitive_class_by_name(prim)
                    {
                        m.borrow_mut()
                            .attrs
                            .insert(prim.to_string(), Value::py_class(class));
                    }
                }
            }
            self.module_cache.borrow_mut().insert(name.to_string(), val.clone());
            // Parent-package identity fix-up: a built-in module like
            // `os` declares `path` as a constant via
            // `super::os_path::module()`, which builds a *fresh*
            // os.path Value rather than the one in `module_cache`.
            // Replace each such submodule-shaped attr with the cached
            // value so `os.path is direct_os_path` matches CPython.
            if let ValueKind::PyModule(m) = val.kind() {
                let submodule_attrs: Vec<String> = {
                    let borrowed = m.borrow();
                    borrowed
                        .attrs
                        .iter()
                        .filter_map(|(attr_name, attr_val)| {
                            // Only consider attrs that are themselves
                            // PyModules — primitive constants stay as-is.
                            match attr_val.kind() {
                                ValueKind::PyModule(_) => Some(attr_name.clone()),
                                _ => None,
                            }
                        })
                        .filter(|attr_name| {
                            // And only if there's a registered built-in
                            // by the dotted name — otherwise leave it.
                            let dotted = format!("{name}.{attr_name}");
                            crate::builtin_modules::load_builtin_module(&dotted).is_some()
                        })
                        .collect()
                };
                for attr_name in submodule_attrs {
                    let dotted = format!("{name}.{attr_name}");
                    // Recursive load goes through the cache, so the
                    // first such call (whether triggered here or by an
                    // explicit `import os.path`) wins and subsequent
                    // accesses share its identity.
                    let cached_submodule = self.load_module(&dotted)?;
                    m.borrow_mut().attrs.insert(attr_name, cached_submodule);
                }
            }
            return Ok(val);
        }
        // User .py file: look for <name>.py relative to script_dir
        if let Some(ref dir) = self.script_dir.clone() {
            // Convert dotted name to path: "foo.bar" -> "foo/bar.py"
            let rel_path = name.replace('.', "/") + ".py";
            let full_path = dir.join(&rel_path);
            if full_path.exists() {
                let src = std::fs::read_to_string(&full_path).map_err(|e| {
                    PyError::Runtime(format!("failed to read '{}': {e}", full_path.display()))
                })?;
                let tokens = Lexer::new(&src)?;
                let program = Parser::new(tokens.into_tokens()).parse_program()?;
                // Subinterpreter shares the same module_cache so results are visible to parent
                let mut sub = Interpreter {
                    script_dir: self.script_dir.clone(),
                    module_cache: Rc::clone(&self.module_cache),
                    ..Default::default()
                };
                // call_depth is thread_local — sub-interpreter automatically shares the same counter
                sub.exec_program(&program, false)?;
                // Harvest all top-level bindings as module attrs
                let attrs: HashMap<String, Value> = sub
                    .env
                    .borrow()
                    .values
                    .iter()
                    .filter(|(k, _)| {
                        !matches!(
                            k.as_str(),
                            "Exception"
                                | "RuntimeError"
                                | "TypeError"
                                | "ValueError"
                                | "AssertionError"
                        )
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let module = Value::py_module(Rc::new(RefCell::new(PyModule {
                    name: name.to_string(),
                    attrs,
                })));
                self.module_cache.borrow_mut().insert(name.to_string(), module.clone());
                return Ok(module);
            }
        }
        Err(PyError::Runtime(format!("No module named '{name}'")))
    }

    fn assign_name(&self, name: String, value: Value) {
        let (is_global, is_nonlocal) = {
            let env = self.env.borrow();
            (env.global_names.contains(&name), env.nonlocal_names.contains(&name))
        };
        if is_global {
            // Write to the module env HashMap so LoadGlobal / post-run
            // inspection can find the new value.
            module_env(&self.env).borrow_mut().values.insert(name.clone(), value.clone());
            // Mirror into the live module globals dict only when globals() has
            // been called (globals_accessed == true).  Without this guard,
            // every StoreGlobal pays an extra IndexMap write even for scripts
            // that never use globals() — the primary cause of the ~15x
            // regression introduced by PR #810.  globals() sets the flag and
            // does a one-time sync before returning, so subsequent writes keep
            // the dict live from that point on.
            if self.globals_accessed {
                let _ = self.module_globals_dict.dict_insert(
                    PyKey::Str(name.clone()),
                    value.clone(),
                );
            }
            // Also update the module-level fastlocal register if one exists
            // for this name.  Without this, `print(x)` at module scope reads
            // the stale register value and ignores the StoreGlobal write (#520).
            // NOTE: with all-env mode (empty local_index, issue #706), there are
            // no module-level fastlocal registers, so this loop is a no-op at
            // module scope — kept for the (rare) non-all-env fallback path.
            // SAFETY: `script_view.regs_ptr` points to the script frame's
            // register file.  The script frame's dispatch loop uses `RegSlice`
            // (not `&mut [Value]`), so no LLVM `noalias` annotation covers the
            // allocation — writing through `NonNull::add(slot).as_mut()` does
            // not violate aliasing rules (issue #547, fixed in PR #646).
            // `slot < regs_len` is verified by the inner `if`.
            if let Some(script_view) = self
                .vm_frame_views
                .iter()
                .find(|v| v.kind == FrameKind::Script)
            {
                if let Some(&slot) = script_view.local_index.get(&name) {
                    let slot = slot as usize;
                    if slot < script_view.regs_len {
                        unsafe {
                            *script_view.regs_ptr.add(slot).as_mut() = value;
                        }
                    }
                }
            }
            return;
        }
        if is_nonlocal
            && let Some(env) = find_enclosing_local_env_for_name(&self.env, &name) {
                env_assign_local(&env, &name, value);
                return;
            }
        // Module scope: `self.env` is the root env (no parent).  Mirror into
        // module_globals_dict only when globals() has already been called so
        // the live view stays in sync — see globals_accessed above.
        let is_module_scope = self.env.borrow().parent.is_none();
        if is_module_scope && self.globals_accessed {
            let _ = self.module_globals_dict.dict_insert(
                PyKey::Str(name.clone()),
                value.clone(),
            );
        }
        env_assign_local(&self.env, &name, value);
    }

    fn lookup_name(&self, name: &str) -> Result<Option<Value>> {
        let (is_global, is_nonlocal) = {
            let env = self.env.borrow();
            (env.global_names.contains(name), env.nonlocal_names.contains(name))
        };
        if is_global {
            return Ok(lookup_name_in_module(&self.env, name));
        }
        if is_nonlocal {
            return lookup_name_in_enclosing_local_env(&self.env, name);
        }
        lookup_name_in_env(&self.env, name)
    }

    fn alloc_env(&mut self, parent: Option<EnvRef>) -> EnvRef {
        if let Some(env) = self.env_pool.pop() {
            {
                let mut e = env.borrow_mut();
                e.values.clear();
                e.parent = parent;
                e.local_names = Default::default();
                e.global_names = Default::default();
                e.nonlocal_names = Default::default();
            }
            env
        } else {
            Environment::new(parent)
        }
    }

    fn free_env(&mut self, env: EnvRef) {
        if self.env_pool.len() < ENV_POOL_MAX && Rc::strong_count(&env) == 1 {
            self.env_pool.push(env);
        }
    }

    /// Like `Value::truthy()` but dispatches `__bool__` / `__len__` for instances.
    pub(crate) fn truthy_value(&mut self, value: &Value) -> Result<bool> {
        if let ValueKind::PyInstance(inst) = value.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            // Try __bool__ first.
            if let Some(method_val) = lookup_class_attr(&class, "__bool__") {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?;
                return match result.kind() {
                    ValueKind::Bool(b) => Ok(b),
                    ValueKind::Int(_) => Err(PyError::named(
                        "TypeError",
                        "__bool__ should return bool, not int".to_string(),
                    )),
                    _ => Err(PyError::named(
                        "TypeError",
                        "__bool__ should return bool".to_string(),
                    )),
                };
            }
            // Fall back to __len__.
            if let Some(method_val) = lookup_class_attr(&class, "__len__") {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[],
                )?;
                return match result.kind() {
                    ValueKind::Int(n) => Ok(n != 0),
                    ValueKind::Bool(b) => Ok(b),
                    _ => Err(PyError::named(
                        "TypeError",
                        "__len__ returned non-int".to_string(),
                    )),
                };
            }
            // No __bool__ or __len__: always truthy.
            return Ok(true);
        }
        Ok(value.truthy())
    }

}

/// Returns the attrs `Rc` for `func`, initialising it lazily on first call.
///
/// Lazy init avoids two heap allocations per function definition for the common
/// case where no attrs are ever set.  Interior mutability (`RefCell`) allows
/// initialization through a shared `Rc<UserFunction>`.
fn func_attrs_rc(func: &UserFunction) -> Rc<RefCell<Value>> {
    let mut slot = func.attrs.borrow_mut();
    if slot.is_none() {
        *slot = Some(Rc::new(RefCell::new(Value::dict(IndexMap::new()))));
    }
    Rc::clone(slot.as_ref().unwrap())
}

/// Returns `true` if `name` is a built-in method on `target`'s type.
/// Used by `get_attr` to produce `BuiltinBoundMethod` values.
fn builtin_has_method(target: &Value, name: &str) -> bool {
    match target.kind() {
        // bool is a subclass of int; hasattr(True, "bit_length") must return True.
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => {
            pyrust_builtins::int::has_method(name)
        }
        ValueKind::Float(_) => pyrust_builtins::float::has_method(name),
        ValueKind::Bytes(_) => pyrust_builtins::bytes::has_method(name),
        ValueKind::Str(_) => pyrust_builtins::string::has_method(name),
        ValueKind::List(_) => pyrust_builtins::list::has_method(name),
        ValueKind::Tuple(_) => pyrust_builtins::tuple::has_method(name),
        ValueKind::Dict(_) => pyrust_builtins::dict::has_method(name),
        ValueKind::Set(_) => pyrust_builtins::set::has_method(name),
        ValueKind::BuiltinObject { ops, .. } => ops.has_method(name),
        _ => false,
    }
}
