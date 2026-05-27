impl Interpreter {
    pub(crate) fn get_attr(&mut self, target: Value, name: &str) -> Result<Value> {
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
                    if let Some(getattribute_val) =
                        lookup_class_attr(&class, "__getattribute__")
                    {
                        if matches!(getattribute_val.kind(), ValueKind::UserFunction(_)) {
                            let result = invoke_class_method(
                                self,
                                getattribute_val,
                                Value::py_instance(Rc::clone(&instance)),
                                &[ExpandedCallArg {
                                    name: None,
                                    value: Value::string(name.to_string()),
                                }],
                            );
                            return match result {
                                Err(ref e) if e.class_name_is("AttributeError") => {
                                    // __getattribute__ raised AttributeError: fall
                                    // through to __getattr__ if defined (CPython
                                    // slot_tp_getattr_hook behaviour).
                                    if let Some(getattr_val) =
                                        lookup_class_attr(&class, "__getattr__")
                                    {
                                        invoke_class_method(
                                            self,
                                            getattr_val,
                                            Value::py_instance(Rc::clone(&instance)),
                                            &[ExpandedCallArg {
                                                name: None,
                                                value: Value::string(name.to_string()),
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
                }
                self.get_attr_instance_raw(instance, name)
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
                    // `__bases__` reports all immediate parents in declaration
                    // order.  If no explicit base was given, CPython reports
                    // `(object,)`.
                    let (base, extra_bases) = {
                        let borrowed = class.borrow();
                        (borrowed.base.clone(), borrowed.extra_bases.clone())
                    };
                    let mut items: Vec<Value> = Vec::new();
                    match base {
                        None => items.push(Value::py_class(object_class_singleton())),
                        Some(b) => {
                            items.push(Value::py_class(b));
                            for eb in extra_bases {
                                items.push(Value::py_class(eb));
                            }
                        }
                    }
                    return Ok(Value::tuple(items));
                }
                if name == "__mro__" {
                    return Ok(Value::tuple(class_mro_items(&class)?));
                }
                if name == "mro" {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        "mro",
                        Value::py_class(Rc::clone(&class)),
                    ));
                }
                if name == "__subclasses__" {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        "__subclasses__",
                        Value::py_class(Rc::clone(&class)),
                    ));
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
                    // Descriptor protocol for class-level access: if the class
                    // attribute is a user-defined descriptor (PyInstance with
                    // __get__), call __get__(None, cls) — CPython Data Model
                    // §3.3.2.  property is handled by its own match arm (above
                    // this ValueKind::PyClass arm) and returns itself on class
                    // access, so we only check PyInstance here.
                    if let ValueKind::PyInstance(desc_inst) = value.kind() {
                        let desc_class = Rc::clone(&desc_inst.borrow().class);
                        if lookup_class_attr(&desc_class, "__get__").is_some() {
                            return call_descriptor_get(
                                self,
                                &value,
                                Value::none(),
                                Value::py_class(Rc::clone(&class)),
                                name,
                            );
                        }
                    }
                    // Drop the kind() Ref before the `_ => value` arm
                    // may move `value` (#450).
                    enum ClassDescTag {
                        UserFunction(Rc<UserFunction>),
                        ClassMethodAny(Value),
                        StaticMethodAny(Value),
                        Other,
                    }
                    let tag = match value.kind() {
                        ValueKind::UserFunction(f) => ClassDescTag::UserFunction(Rc::clone(f)),
                        _ => {
                            if let Some(w) =
                                pyrust_builtins::classmethod::as_class_method_any(&value)
                            {
                                ClassDescTag::ClassMethodAny(w)
                            } else if let Some(w) =
                                pyrust_builtins::classmethod::as_static_method_any(&value)
                            {
                                ClassDescTag::StaticMethodAny(w)
                            } else {
                                ClassDescTag::Other
                            }
                        }
                    };
                    return Ok(match tag {
                        ClassDescTag::UserFunction(f) => match f.kind {
                            UserFunctionKind::ClassMethod => {
                                Value::class_bound_method(Rc::clone(&f), Rc::clone(&class))
                            }
                            UserFunctionKind::StaticMethod => {
                                // CPython __get__ returns the underlying function directly.
                                // Prefer `wrapped_func` to preserve object identity
                                // (`sm.__get__(None, C) is fn` when `sm = staticmethod(fn)`).
                                if let Some(inner) = f.wrapped_func.as_ref() {
                                    Value::user_function(Rc::clone(inner))
                                } else {
                                    Value::with_function_kind(
                                        Rc::clone(&f),
                                        UserFunctionKind::Regular,
                                    )
                                }
                            }
                            UserFunctionKind::Regular => value,
                            UserFunctionKind::Builtin(_) => value,
                        },
                        // classmethod(non_fn): CPython returns a method object bound to
                        // the class; pyrust returns the wrapped value (no new Value
                        // variant for non-UserFunction class-bound values).
                        ClassDescTag::ClassMethodAny(w) => w,
                        // staticmethod(non_fn): returns the wrapped value directly,
                        // matching CPython (`C.s` where `s = staticmethod(42)` → 42).
                        ClassDescTag::StaticMethodAny(w) => w,
                        ClassDescTag::Other => value,
                    });
                }
                // Issue #1275: __module__ and __doc__ on built-in type objects.
                // Primitive classes (int, str, …) are immutable and cannot store
                // attrs like user-defined classes do.  Exception classes and the
                // object singleton share the same gap.  CPython exposes both
                // attributes on every type object; provide the fallback here so
                // that builtin and exception classes behave like their CPython
                // counterparts.
                //
                // User-defined classes always have __module__ and __doc__ in their
                // own attrs dict (set by the VM's MakeClass instruction), so they
                // are handled by the lookup_class_attr path above and never reach
                // this fallback.
                let is_builtin_class = crate::interpreter::is_primitive_class(&class)
                    || is_exception_class(&class)
                    || Rc::ptr_eq(&class, &object_class_singleton());
                if name == "__module__" && is_builtin_class {
                    return Ok(Value::string("builtins".to_string()));
                }
                if name == "__doc__" && is_builtin_class {
                    let class_name = class.borrow().name.clone();
                    return Ok(match builtin_class_doc(&class_name) {
                        Some(doc) => Value::string(doc.to_string()),
                        None => Value::none(),
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
                // CPython super() semantics: look up `name` in the MRO of
                // type(instance), starting from the class *after* `class`.
                // This is necessary for cooperative multiple inheritance: when
                // B.method calls super().method, the next in D's MRO is C (not
                // A), so super() must walk D's full MRO rather than just B.base.
                let instance_class = Rc::clone(&instance.borrow().class);
                let mro = class_mro_items(&instance_class)?;
                let class_ptr = Rc::as_ptr(&class);
                // Find `class` in the MRO, then search from the next entry.
                let start = mro
                    .iter()
                    .position(|v| {
                        if let ValueKind::PyClass(c) = v.kind() {
                            Rc::as_ptr(c) == class_ptr
                        } else {
                            false
                        }
                    })
                    .map(|i| i + 1)
                    .unwrap_or(0);
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
                                    Value::class_bound_method(Rc::clone(&f), entry_class)
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
                                _ => value.clone(),
                            },
                        });
                    }
                }
                Err(PyError::named(
                    "AttributeError",
                    format!("super(): parent class has no attribute '{name}'"),
                ))
            }
            ValueKind::SuperProxyClass { class, obj_class } => {
                let class = Rc::clone(class);
                let obj_class = Rc::clone(obj_class);
                // classmethod super(): use MRO of obj_class, start after `class`.
                let mro = class_mro_items(&obj_class)?;
                let class_ptr = Rc::as_ptr(&class);
                let start = mro
                    .iter()
                    .position(|v| {
                        if let ValueKind::PyClass(c) = v.kind() {
                            Rc::as_ptr(c) == class_ptr
                        } else {
                            false
                        }
                    })
                    .map(|i| i + 1)
                    .unwrap_or(0);
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
                                    Value::user_function(Rc::clone(&f))
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
                            None => value,
                        });
                    }
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
                    "__func__" if matches!(
                        func.kind,
                        UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
                    ) => {
                        // `staticmethod.__func__` and `classmethod.__func__` return the
                        // exact object that was passed to staticmethod()/classmethod(),
                        // preserving identity (`sm.__func__ is f`).
                        // `wrapped_func` holds the original Rc from the wrapping call.
                        // Fall back to stripping the kind tag when there is no stored
                        // `wrapped_func` (compile-time tagging of a Builtin, or any
                        // path that predates this field).
                        return if let Some(inner) = func.wrapped_func.as_ref() {
                            Ok(Value::user_function(Rc::clone(inner)))
                        } else {
                            Ok(Value::with_function_kind(
                                Rc::clone(func),
                                UserFunctionKind::Regular,
                            ))
                        };
                    }
                    "__get__" if func.kind == UserFunctionKind::ClassMethod => {
                        // `classmethod.__get__(instance, owner)` — returns a binder
                        // that, when called, creates a ClassBoundMethod.  The
                        // interpreter's `call_function_expanded` resolves the binder
                        // (see guard arm for `as_class_method_get_binder`).
                        return Ok(pyrust_builtins::classmethod::class_method_get_binder(
                            Rc::clone(func),
                        ));
                    }
                    "__get__" if func.kind == UserFunctionKind::StaticMethod => {
                        // `staticmethod.__get__(instance, owner)` — returns a binder
                        // that, when called, returns the underlying plain function.
                        return Ok(pyrust_builtins::classmethod::static_method_get_binder(
                            Rc::clone(func),
                        ));
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
                let type_name = match func.kind {
                    UserFunctionKind::StaticMethod => "staticmethod",
                    UserFunctionKind::ClassMethod => "classmethod",
                    _ => "function",
                };
                Err(PyError::named(
                    "AttributeError",
                    format!("'{type_name}' object has no attribute '{name}'"),
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
                if func_name == "generator" {
                    // Issue #1413: type(gen).__iter__ and type(gen).__next__.
                    // CPython exposes these as slot wrappers on the generator
                    // type.  Return unbound BuiltinFunction descriptors so that
                    // hasattr(type(gen), '__iter__') is True and calling
                    // type(gen).__iter__(g) works via call_function_expanded.
                    match name {
                        "__iter__" => return Ok(Value::builtin_function("generator.__iter__")),
                        "__next__" => return Ok(Value::builtin_function("generator.__next__")),
                        "send"     => return Ok(Value::builtin_function("generator.send")),
                        "close"    => return Ok(Value::builtin_function("generator.close")),
                        "throw"    => return Ok(Value::builtin_function("generator.throw")),
                        _ => {}
                    }
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
            ValueKind::Generator(state_rc) => {
                // Generator introspection attributes (issue #1270).
                // All six attributes exposed by CPython 3.12's generator type:
                //   __name__, __qualname__, gi_running, gi_yieldfrom, gi_frame, gi_code.
                //
                // Issue #1413: also expose the iteration protocol methods as
                // bound-method values so that hasattr/getattr see them.
                // These apply to all generator subtypes (GeneratorFrame,
                // NativeIterFrame, CallableIter, …), so they are checked
                // before the downcast.
                match name {
                    "__iter__" | "__next__" | "send" | "close" | "throw" => {
                        return Ok(pyrust_builtins::bound_method::bound_method(
                            name.to_string(),
                            target.clone(),
                        ));
                    }
                    _ => {}
                }
                let state_rc = Rc::clone(state_rc);
                let borrow = state_rc.borrow();
                if let Some(frame) = borrow.downcast_ref::<GeneratorFrame>() {
                    match name {
                        "__name__" => return Ok(Value::string(frame.fn_name.as_ref())),
                        "__qualname__" => return Ok(Value::string(frame.qualname.as_ref())),
                        // gi_running is always False when accessed from outside the
                        // generator body (True is only observable from within — pyrust
                        // does not expose re-entrant generator guards, matching CPython's
                        // simple "False unless currently on the C call stack" rule).
                        "gi_running" => return Ok(Value::bool_(false)),
                        // gi_yieldfrom: the sub-iterator being delegated to via
                        // `yield from`, or None.  When the generator is suspended at a
                        // YieldFrom instruction the sub-iterator sits in iter_reg.
                        // `frame.pc == 0` means the generator body hasn't started yet —
                        // don't inspect insns[0] in that case (iter_reg unloaded).
                        "gi_yieldfrom" => {
                            if !frame.done && frame.pc != 0 {
                                if let Some(crate::bytecode::Insn::YieldFrom { iter_reg, .. }) =
                                    frame.code.insns.get(frame.pc)
                                {
                                    let sub_iter = frame.regs[*iter_reg as usize].clone();
                                    return Ok(sub_iter);
                                }
                            }
                            return Ok(Value::none());
                        }
                        // gi_frame and gi_code: pyrust does not expose frame/code
                        // objects; return None to avoid AttributeError (issue #1270).
                        "gi_frame" => return Ok(Value::none()),
                        "gi_code" => return Ok(Value::none()),
                        _ => {}
                    }
                }
                Err(PyError::named(
                    "AttributeError",
                    format!("'generator' object has no attribute '{name}'"),
                ))
            }
            _ => {
                // BoundMethod / ClassBoundMethod: expose __func__, __self__, and
                // forward __name__ / __qualname__ / __module__ / __doc__ /
                // __dict__ / __annotations__ / __defaults__ / __kwdefaults__
                // from the underlying function, matching CPython's method proxy
                // semantics.  __self__ differs between variants (instance vs
                // class), so each has its own arm; shared attrs live in the
                // bound_method_common_attr helper.
                match target.kind() {
                    ValueKind::BoundMethod {
                        function,
                        receiver,
                    } => {
                        if name == "__func__" {
                            return Ok(Value::user_function(Rc::clone(function)));
                        }
                        if name == "__self__" {
                            return Ok(Value::py_instance(Rc::clone(receiver)));
                        }
                        if let Some(v) = bound_method_common_attr(function, name) {
                            return v;
                        }
                    }
                    ValueKind::ClassBoundMethod { function, class } => {
                        if name == "__func__" {
                            return Ok(Value::user_function(Rc::clone(function)));
                        }
                        if name == "__self__" {
                            return Ok(Value::py_class(Rc::clone(class)));
                        }
                        if let Some(v) = bound_method_common_attr(function, name) {
                            return v;
                        }
                    }
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
            // Return a live mutable proxy so that writes to
            // `obj.__dict__['key'] = val` propagate back to the
            // instance's actual attrs map.  Required for the data-
            // descriptor `__set__` protocol (issues #1271 / #1272).
            let is_exc = is_exception_class(&instance.borrow().class);
            return Ok(pyrust_builtins::instance_dict::instance_dict(
                Rc::clone(&instance),
                is_exc,
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
        if let Some(class_val) = lookup_class_attr(&class, name) {
            if is_data_descriptor(&class_val) {
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
                        if let Some(getattr_val) = lookup_class_attr(&class, "__getattr__") {
                            return invoke_class_method(
                                self,
                                getattr_val,
                                Value::py_instance(Rc::clone(&instance)),
                                &[ExpandedCallArg {
                                    name: None,
                                    value: Value::string(name.to_string()),
                                }],
                            );
                        }
                    }
                    Err(_) => {}
                }
                return desc_result;
            }
        }

        // Step 2: Instance __dict__.
        if let Some(value) = instance.borrow().attrs.get(name).cloned() {
            return Ok(value);
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
                instance.borrow_mut().attrs.insert(attr_name, result.clone());
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
                        if let Some(getattr_val) = lookup_class_attr(&class, "__getattr__") {
                            return invoke_class_method(
                                self,
                                getattr_val,
                                Value::py_instance(Rc::clone(&instance)),
                                &[ExpandedCallArg {
                                    name: None,
                                    value: Value::string(name.to_string()),
                                }],
                            );
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
                BuiltinFunction,
                ClassMethodAny(Value),
                StaticMethodAny(Value),
                Other,
            }
            let tag = match value.kind() {
                ValueKind::UserFunction(f) => AttrKind::UserFunction(Rc::clone(f)),
                ValueKind::BuiltinFunction(_) => AttrKind::BuiltinFunction,
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
            return Ok(match tag {
                AttrKind::UserFunction(f) => match f.kind {
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
                },
                AttrKind::BuiltinFunction => pyrust_builtins::bound_method::bound_method(
                    name.to_string(),
                    Value::py_instance(Rc::clone(&instance)),
                ),
                // classmethod/staticmethod wrapping a non-function: apply
                // descriptor protocol same as class-level access (§3.3.2).
                // classmethod returns the wrapped value (best approximation;
                // CPython returns a method object for non-callables but that
                // requires a new Value variant); staticmethod returns directly.
                AttrKind::ClassMethodAny(w) => w,
                AttrKind::StaticMethodAny(w) => w,
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

        // Step 4: __getattr__ fallback — called when normal lookup
        // finds nothing (CPython slot_tp_getattr_hook).
        if let Some(getattr_val) = lookup_class_attr(&class, "__getattr__") {
            return invoke_class_method(
                self,
                getattr_val,
                Value::py_instance(Rc::clone(&instance)),
                &[ExpandedCallArg {
                    name: None,
                    value: Value::string(name.to_string()),
                }],
            );
        }

        let class_name = class.borrow().name.clone();
        Err(PyError::named(
            "AttributeError",
            format!("'{}' object has no attribute '{}'", class_name, name),
        ))
    }

    pub(crate) fn assign_attr(&mut self, target: Value, name: &str, value: Value) -> Result<()> {
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                let class = { Rc::clone(&instance.borrow().class) };
                // Check for `__setattr__` first — CPython dispatches
                // __setattr__ before the descriptor protocol (object.__setattr__
                // is what does the descriptor lookup by default).
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
                // General data descriptor protocol: if the class (or MRO) has
                // a data descriptor (has __set__) for this name, call __set__.
                if let Some(class_val) = lookup_class_attr(&class, name) {
                    if let Some(result) =
                        call_descriptor_set(self, &class_val, Value::py_instance(Rc::clone(instance)), value.clone(), name)?
                    {
                        return result;
                    }
                }
                // Issue #1198: bare `object()` instances have no __dict__ in
                // CPython.  Only the object singleton itself is blocked; any
                // user-defined class (even `class Foo(object): pass`) gets its
                // own PyClass Rc and is not ptr_eq to the singleton.
                if Rc::ptr_eq(&class, &object_class_singleton()) {
                    return Err(PyError::named(
                        "AttributeError",
                        format!("'object' object has no attribute '{name}'"),
                    ));
                }
                // PEP 3134: __cause__ and __context__ must be None or a
                // BaseException subclass instance.  __suppress_context__ must
                // be a bool.  CPython enforces these in the C slot setters;
                // not enforcing them makes pyrust silently accept bad values
                // that CPython raises TypeError for (issue #1066 review).
                if is_exception_class(&class) {
                    match name {
                        "__cause__" | "__context__" => {
                            let ok = match value.kind() {
                                ValueKind::None => true,
                                ValueKind::PyInstance(inst) => {
                                    is_exception_class(&inst.borrow().class)
                                }
                                _ => false,
                            };
                            if !ok {
                                return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "exception {} must be None or derive from BaseException",
                                        if name == "__cause__" { "cause" } else { "context" }
                                    ),
                                ));
                            }
                        }
                        "__suppress_context__" => {
                            if !matches!(value.kind(), ValueKind::Bool(_)) {
                                return Err(PyError::named(
                                    "TypeError",
                                    "attribute value type must be bool",
                                ));
                            }
                        }
                        // Issue #1441: __traceback__ must be None or a traceback
                        // object.  CPython raises TypeError for any other value.
                        "__traceback__" => {
                            let ok = match value.kind() {
                                ValueKind::None => true,
                                ValueKind::BuiltinObject { ops, .. } => {
                                    ops.type_name() == pyrust_builtins::traceback::TYPE_NAME
                                }
                                _ => false,
                            };
                            if !ok {
                                return Err(PyError::named(
                                    "TypeError",
                                    "__traceback__ must be a traceback or None".to_string(),
                                ));
                            }
                        }
                        _ => {}
                    }
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
                {
                    let mut cls = class.borrow_mut();
                    cls.attrs.insert(name.to_string(), value);
                    let v = cls.mutation_version.get().wrapping_add(1);
                    cls.mutation_version.set(v);
                }
                // Bump the global epoch so that caches keyed on subclasses of
                // this class (which only check their own mutation_version) also
                // invalidate — the epoch guard catches ancestor mutations that
                // the leaf-class version check would miss.
                pyrust_core::bump_class_epoch();
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
                    "__func__"
                        if matches!(
                            func.kind,
                            UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
                        ) =>
                    {
                        Err(PyError::named(
                            "AttributeError",
                            "readonly attribute".to_string(),
                        ))
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
                            return Err(PyError::named(
                                "TypeError",
                                format!("{name} must be set to a string object"),
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
                        Err(PyError::named(
                            "AttributeError",
                            format!("attribute '{name}' of 'generator' objects is not writable"),
                        ))
                    }
                    _ => Err(PyError::named(
                        "AttributeError",
                        format!("'generator' object has no attribute '{name}'"),
                    )),
                }
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
                // Check for `__delattr__` first — symmetric with __setattr__
                // in assign_attr (issue #1174).
                if let Some(delattr_val) = lookup_class_attr(&class, "__delattr__") {
                    return invoke_class_method(
                        self,
                        delattr_val,
                        Value::py_instance(Rc::clone(instance)),
                        &[ExpandedCallArg {
                            name: None,
                            value: Value::string(name.to_string()),
                        }],
                    )
                    .map(|_| ());
                }
                // General data descriptor protocol: if the class (or MRO)
                // has a descriptor with __delete__ for this name, call it.
                if let Some(class_val) = lookup_class_attr(&class, name) {
                    if let Some(result) =
                        call_descriptor_delete(self, &class_val, Value::py_instance(Rc::clone(instance)), name)?
                    {
                        return result;
                    }
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
                    "__func__"
                        if matches!(
                            func.kind,
                            UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
                        ) =>
                    {
                        Err(PyError::named(
                            "AttributeError",
                            "readonly attribute".to_string(),
                        ))
                    }
                    // CPython allows `del f.__defaults__` / `del f.__kwdefaults__`
                    // (they reset to None).  Since pyrust doesn't implement these slots
                    // yet, silently succeed — the state the caller intended (unset)
                    // already matches pyrust's state.
                    "__defaults__" | "__kwdefaults__" => Ok(()),
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
                                UserFunctionKind::StaticMethod => "staticmethod",
                                UserFunctionKind::ClassMethod => "classmethod",
                                _ => "function",
                            };
                            Err(PyError::named(
                                "AttributeError",
                                format!("'{type_name}' object has no attribute '{name}'"),
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
                {
                    let mut cls = class.borrow_mut();
                    if cls.attrs.shift_remove(name).is_none() {
                        let class_name = cls.name.clone();
                        return Err(PyError::named(
                            "AttributeError",
                            format!("type object '{class_name}' has no attribute '{name}'"),
                        ));
                    }
                    let v = cls.mutation_version.get().wrapping_add(1);
                    cls.mutation_version.set(v);
                }
                // Bump the global epoch so that caches keyed on subclasses of
                // this class also invalidate after a base-class deletion.
                pyrust_core::bump_class_epoch();
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

    pub(crate) fn load_module(&mut self, name: &str) -> Result<Value> {
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
            //
            // Also insert exception classes (issue #1255): CPython exposes
            // every built-in exception class as an attribute of the `builtins`
            // module (`builtins.ValueError`, `builtins.TypeError`, etc.).
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
                // `type` metaclass (issue #1312): must display as `<class 'type'>`.
                m.borrow_mut().attrs.insert(
                    "type".to_string(),
                    Value::py_class(crate::interpreter::type_class_singleton()),
                );
                // `object` (issue #1313): must display as `<class 'object'>`.
                m.borrow_mut().attrs.insert(
                    "object".to_string(),
                    Value::py_class(crate::interpreter::object_class_singleton()),
                );
                // Insert all built-in exception classes.  Skip names that
                // contain '.' (e.g. "io.UnsupportedOperation" which belongs
                // to the `io` module, not `builtins`).  Also skip bare names
                // that are registered under a dotted alias (e.g.
                // "UnsupportedOperation" is io-module-only).
                let exc_map = crate::interpreter::build_exc_class_map();
                let non_builtin_ptrs: std::collections::HashSet<*const _> = exc_map
                    .iter()
                    .filter(|(n, _)| n.contains('.'))
                    .map(|(_, cls)| Rc::as_ptr(cls))
                    .collect();
                for (exc_name, exc_class) in &exc_map {
                    if !exc_name.contains('.')
                        && !non_builtin_ptrs.contains(&Rc::as_ptr(exc_class))
                    {
                        m.borrow_mut()
                            .attrs
                            .insert(exc_name.to_string(), Value::py_class(Rc::clone(exc_class)));
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
        Err(PyError::import_error(
            "ModuleNotFoundError",
            format!("No module named '{name}'"),
            Some(name.to_string()),
        ))
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
            // Invalidate the LoadGlobal inline cache: any function that cached
            // this global under the current version will re-fetch on its next call.
            bump_global_env_version(self);
            // Mirror into the live module globals dict only when globals() has
            // been called (globals_accessed == true).  Without this guard,
            // every StoreGlobal pays an extra IndexMap write even for scripts
            // that never use globals() — the primary cause of the ~15x
            // regression introduced by PR #810.  globals() sets the flag and
            // does a one-time sync before returning, so subsequent writes keep
            // the dict live from that point on.
            if self.globals_accessed {
                let _ = self.module_globals_dict.dict_insert(
                    PyKey::str_from(&name),
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
        if is_module_scope {
            // Invalidate the LoadGlobal inline cache for module-scope writes.
            bump_global_env_version(self);
            if self.globals_accessed {
                let _ = self.module_globals_dict.dict_insert(
                    PyKey::str_from(&name),
                    value.clone(),
                );
            }
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
                let self_val = if matches!(method_val.kind(), ValueKind::BuiltinFunction(_)) {
                    coerce_numeric(value.clone())
                } else {
                    Value::py_instance(Rc::clone(&inst_rc))
                };
                let result = invoke_class_method(self, method_val, self_val, &[])?;
                return match result.kind() {
                    ValueKind::Bool(b) => Ok(b),
                    _ => Err(PyError::named(
                        "TypeError",
                        format!(
                            "__bool__ should return bool, returned {}",
                            pyrust_core::builtin_type_name(&result),
                        ),
                    )),
                };
            }
            // Fall back to __len__.
            if let Some(method_val) = lookup_class_attr(&class, "__len__") {
                let self_val = if matches!(method_val.kind(), ValueKind::BuiltinFunction(_)) {
                    coerce_numeric(value.clone())
                } else {
                    Value::py_instance(Rc::clone(&inst_rc))
                };
                let result = invoke_class_method(self, method_val, self_val, &[])?;
                return match result.kind() {
                    ValueKind::Int(n) if n >= 0 => Ok(n != 0),
                    ValueKind::Int(_) => Err(PyError::named(
                        "ValueError",
                        "__len__() should return >= 0".to_string(),
                    )),
                    ValueKind::Bool(b) => Ok(b),
                    ValueKind::BigInt(big) => match big.sign() {
                        PyBigIntSign::Minus => Err(PyError::named(
                            "ValueError",
                            "__len__() should return >= 0".to_string(),
                        )),
                        PyBigIntSign::NoSign => Ok(false),
                        PyBigIntSign::Plus => {
                            if big.to_usize().is_none() {
                                Err(PyError::named(
                                    "OverflowError",
                                    "cannot fit 'int' into an index-sized integer".to_string(),
                                ))
                            } else {
                                Ok(true)
                            }
                        }
                    },
                    _ => Err(PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object cannot be interpreted as an integer",
                            pyrust_core::builtin_type_name(&result),
                        ),
                    )),
                };
            }
            // Issue #1204: no __bool__ or __len__ in the user class.
            // For scalar primitive subclasses (MyInt, MyFloat, MyStr,
            // MyBytes), delegate truthiness to the backing value so that
            // `bool(MyInt(0))` returns False as CPython does.
            if let Some(backing) = instance_builtin_data(&inst_rc) {
                return Ok(backing.truthy());
            }
            // Non-primitive PyInstance with no __bool__ / __len__: always truthy.
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

/// Handle attribute lookup on a bound method for attributes that are shared
/// between `BoundMethod` and `ClassBoundMethod` (everything except `__func__`
/// and `__self__` which differ between the two variants).
///
/// Returns `Some(Ok(v))` when the attribute was found, `Some(Err(_))` if it
/// raised, or `None` to signal fall-through to the caller's error path.
fn bound_method_common_attr(function: &UserFunction, name: &str) -> Option<crate::error::Result<Value>> {
    match name {
        "__name__" => {
            let n = function
                .user_name
                .borrow()
                .as_deref()
                .unwrap_or(&function.name)
                .to_string();
            Some(Ok(Value::string(n)))
        }
        "__qualname__" => {
            let q = function
                .user_qualname
                .borrow()
                .as_deref()
                .unwrap_or(&function.qualname)
                .to_string();
            Some(Ok(Value::string(q)))
        }
        "__module__" => Some(Ok(function.module.borrow().clone())),
        "__doc__" => Some(Ok(function.doc.borrow().clone())),
        "__dict__" => {
            let attrs_rc = func_attrs_rc(function);
            Some(Ok(attrs_rc.borrow().clone()))
        }
        "__annotations__" => Some(Ok(function.annotations.borrow().clone())),
        "__defaults__" => {
            // Collect defaults for positional-or-keyword params (not *args,
            // **kwargs, or keyword-only).  Returns None if no defaults exist,
            // matching CPython's `f.__defaults__` semantics.
            let defaults: Vec<Value> = function
                .params
                .iter()
                .filter(|p| !p.is_args && !p.is_kwargs && !p.is_keyword_only)
                .filter_map(|p| p.default.clone())
                .collect();
            if defaults.is_empty() {
                Some(Ok(Value::none()))
            } else {
                Some(Ok(Value::tuple(defaults)))
            }
        }
        "__kwdefaults__" => {
            // Collect defaults for keyword-only params.  Returns None if none
            // exist, matching CPython's `f.__kwdefaults__` semantics.
            let kwdefaults: IndexMap<PyKey, Value> = function
                .params
                .iter()
                .filter(|p| p.is_keyword_only)
                .filter_map(|p| {
                    p.default
                        .as_ref()
                        .map(|v| (PyKey::str_from(&p.name), v.clone()))
                })
                .collect();
            if kwdefaults.is_empty() {
                Some(Ok(Value::none()))
            } else {
                Some(Ok(Value::dict(kwdefaults)))
            }
        }
        _ => {
            // Arbitrary dynamic attrs delegate to the underlying function.
            // Short-circuit without initialising if no attrs set yet.
            if let Some(rc) = function.attrs.borrow().as_ref().map(Rc::clone) {
                if let Some(v) = rc
                    .borrow()
                    .as_dict()
                    .and_then(|d| d.get(&StrKey(name)).cloned())
                {
                    return Some(Ok(v));
                }
            }
            None
        }
    }
}

/// Increment `Interpreter::global_env_version`, skipping the sentinel
/// value `GLOBAL_CACHE_EMPTY` (u32::MAX - 1) that the `LoadGlobal` inline
/// cache reserves as the "not yet populated" marker.  Wraps through 0 so
/// that the counter never collides with the sentinel.
#[inline]
fn bump_global_env_version(interp: &Interpreter) {
    let v = interp.global_env_version.get().wrapping_add(1);
    // Skip GLOBAL_CACHE_EMPTY (u32::MAX - 1); wrap back to 0.
    let v = if v == GLOBAL_CACHE_EMPTY { 0 } else { v };
    interp.global_env_version.set(v);
}

/// Returns `true` if `val` is a data descriptor: it defines `__set__` or
/// `__delete__` on its type.  Data descriptors take priority over instance
/// `__dict__` in CPython's attribute lookup (PEP 3107 / Data Model §3.3.2).
///
/// For `property` (a BuiltinObject), it is always a data descriptor.
/// For user `PyInstance` values, we look up `__set__` or `__delete__` on the
/// instance's class.  Other value kinds are never descriptors.
fn is_data_descriptor(val: &Value) -> bool {
    // property is always a data descriptor (has fget/fset/fdel slots).
    if pyrust_builtins::property::property_partial_slot(val) == Some(None) {
        return true;
    }
    if let ValueKind::PyInstance(inst) = val.kind() {
        let class = Rc::clone(&inst.borrow().class);
        return lookup_class_attr(&class, "__set__").is_some()
            || lookup_class_attr(&class, "__delete__").is_some();
    }
    false
}

/// Returns `true` if `val` is a non-data descriptor: defines `__get__` on
/// its type but NOT `__set__` or `__delete__`.  Non-data descriptors are
/// checked after instance `__dict__` in CPython's lookup order.
///
/// `cached_property` is already handled before this path so we don't need
/// to special-case it here — it would return `true` but is intercepted
/// earlier.
fn is_non_data_descriptor(val: &Value) -> bool {
    if let ValueKind::PyInstance(inst) = val.kind() {
        let class = Rc::clone(&inst.borrow().class);
        return lookup_class_attr(&class, "__get__").is_some()
            && lookup_class_attr(&class, "__set__").is_none()
            && lookup_class_attr(&class, "__delete__").is_none();
    }
    false
}

/// Call `descriptor.__get__(instance, owner)` and return the result.
///
/// Handles both `property` (BuiltinObject with fget) and user-defined
/// descriptors (PyInstance with a class `__get__` method).
fn call_descriptor_get(
    interp: &mut Interpreter,
    descriptor: &Value,
    instance: Value,
    owner: Value,
    attr_name: &str,
) -> Result<Value> {
    // property special-case: use the stored fget directly.
    if let Some((fget, partial_slot)) =
        pyrust_builtins::property::with_property(descriptor, |s| {
            (Rc::clone(&s.fget), s.partial_slot)
        })
        && partial_slot.is_none()
    {
        return if fget.is_none() {
            Err(PyError::named(
                "AttributeError",
                format!("property '{}' has no getter", attr_name),
            ))
        } else {
            let getter = (*fget).clone();
            interp.call_function_expanded(
                getter,
                &[ExpandedCallArg {
                    name: None,
                    value: instance,
                }],
            )
        };
    }
    // General user-defined descriptor: look up __get__ on the descriptor's class.
    if let ValueKind::PyInstance(inst) = descriptor.kind() {
        let desc_class = Rc::clone(&inst.borrow().class);
        if let Some(get_fn) = lookup_class_attr(&desc_class, "__get__") {
            return invoke_class_method(
                interp,
                get_fn,
                descriptor.clone(),
                &[
                    ExpandedCallArg {
                        name: None,
                        value: instance,
                    },
                    ExpandedCallArg {
                        name: None,
                        value: owner,
                    },
                ],
            );
        }
    }
    // Fallback: return the descriptor itself (shouldn't happen if callers
    // check is_data_descriptor / is_non_data_descriptor first, but be safe).
    Ok(descriptor.clone())
}

/// Try to call `descriptor.__set__(instance, value)` for a data descriptor.
///
/// Returns `Some(Ok(()))` if the descriptor handled the set,
/// `Some(Err(_))` if it raised, or `None` if the class attribute is not a
/// data descriptor (caller should fall through to instance dict write).
fn call_descriptor_set(
    interp: &mut Interpreter,
    class_val: &Value,
    instance: Value,
    value: Value,
    attr_name: &str,
) -> Result<Option<Result<()>>> {
    // property special-case.
    if let Some((fset, partial_slot)) =
        pyrust_builtins::property::with_property(class_val, |s| {
            (Rc::clone(&s.fset), s.partial_slot)
        })
        && partial_slot.is_none()
    {
        return Ok(Some(if fset.is_none() {
            Err(PyError::named(
                "AttributeError",
                format!("property '{}' has no setter", attr_name),
            ))
        } else {
            let setter = (*fset).clone();
            interp.call_function_expanded(
                setter,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: instance,
                    },
                    ExpandedCallArg { name: None, value },
                ],
            )?;
            Ok(())
        }));
    }
    // General user-defined data descriptor: look up __set__ on the descriptor's class.
    if let ValueKind::PyInstance(inst) = class_val.kind() {
        let desc_class = Rc::clone(&inst.borrow().class);
        if let Some(set_fn) = lookup_class_attr(&desc_class, "__set__") {
            let result = invoke_class_method(
                interp,
                set_fn,
                class_val.clone(),
                &[
                    ExpandedCallArg {
                        name: None,
                        value: instance,
                    },
                    ExpandedCallArg { name: None, value },
                ],
            );
            return Ok(Some(result.map(|_| ())));
        }
    }
    Ok(None)
}

/// Try to call `descriptor.__delete__(instance)` for a data descriptor.
///
/// Returns `Some(Ok(()))` if handled, `Some(Err(_))` if it raised, or
/// `None` if no `__delete__` is found (caller falls through to instance
/// dict removal).
fn call_descriptor_delete(
    interp: &mut Interpreter,
    class_val: &Value,
    instance: Value,
    attr_name: &str,
) -> Result<Option<Result<()>>> {
    // property special-case.
    if let Some((fdel, partial_slot)) =
        pyrust_builtins::property::with_property(class_val, |s| {
            (Rc::clone(&s.fdel), s.partial_slot)
        })
        && partial_slot.is_none()
    {
        return Ok(Some(if fdel.is_none() {
            Err(PyError::named(
                "AttributeError",
                format!("property '{}' has no deleter", attr_name),
            ))
        } else {
            let deleter = (*fdel).clone();
            interp.call_function_expanded(
                deleter,
                &[ExpandedCallArg {
                    name: None,
                    value: instance,
                }],
            )?;
            Ok(())
        }));
    }
    // General user-defined data descriptor: look up __delete__ on the descriptor's class.
    if let ValueKind::PyInstance(inst) = class_val.kind() {
        let desc_class = Rc::clone(&inst.borrow().class);
        if let Some(del_fn) = lookup_class_attr(&desc_class, "__delete__") {
            let result = invoke_class_method(
                interp,
                del_fn,
                class_val.clone(),
                &[ExpandedCallArg {
                    name: None,
                    value: instance,
                }],
            );
            return Ok(Some(result.map(|_| ())));
        }
    }
    Ok(None)
}

/// Compute the MRO (method resolution order) for a class using C3 linearization.
///
/// Implements the C3 superclass linearization algorithm as used by CPython:
///
///   L[C(B1, B2, ...)] = C + merge(L[B1], L[B2], ..., [B1, B2, ...])
///
/// The merge operation repeatedly selects the head of the first list whose
/// head does not appear in the tail of any other list.  If no such head
/// exists the bases are inconsistent and a TypeError is returned.
///
/// Used by both `__mro__` (returns a tuple) and `mro()` (returns a list).
fn class_mro_items(class: &Rc<RefCell<PyClass>>) -> Result<Vec<Value>> {
    /// Compute L[c] recursively.  Returns a `Vec` of class pointers in MRO
    /// order; the first element is always `c` itself.
    fn c3_linearize(
        c: &Rc<RefCell<PyClass>>,
        obj_ptr: *const RefCell<PyClass>,
    ) -> Result<Vec<Rc<RefCell<PyClass>>>> {
        let (base, extra_bases) = {
            let borrowed = c.borrow();
            (borrowed.base.clone(), borrowed.extra_bases.clone())
        };

        // Collect all direct bases in declaration order.
        let mut all_bases: Vec<Rc<RefCell<PyClass>>> = Vec::new();
        if let Some(ref b) = base {
            all_bases.push(Rc::clone(b));
        }
        for eb in &extra_bases {
            all_bases.push(Rc::clone(eb));
        }

        if all_bases.is_empty() {
            // No explicit bases: just [c].  The object singleton will be
            // appended by the outer function after the merge.
            return Ok(vec![Rc::clone(c)]);
        }

        // Build the lists to merge: L[B1], L[B2], ..., [B1, B2, ...]
        let mut lists: Vec<Vec<Rc<RefCell<PyClass>>>> = Vec::new();
        for b in &all_bases {
            lists.push(c3_linearize(b, obj_ptr)?);
        }
        // The final list is the sequence of direct bases.
        lists.push(all_bases.clone());

        // C3 merge.
        let mut result: Vec<Rc<RefCell<PyClass>>> = vec![Rc::clone(c)];
        loop {
            // Remove all empty lists.
            lists.retain(|l| !l.is_empty());
            if lists.is_empty() {
                break;
            }

            // Find a good head: first element of some list that does not
            // appear in the tail of any other list.
            let mut chosen: Option<Rc<RefCell<PyClass>>> = None;
            'outer: for list in &lists {
                let head_ptr = Rc::as_ptr(&list[0]);
                // Check that head_ptr does not appear in the tail of any list.
                for other in &lists {
                    for tail_item in other.iter().skip(1) {
                        if Rc::as_ptr(tail_item) == head_ptr {
                            continue 'outer;
                        }
                    }
                }
                chosen = Some(Rc::clone(&list[0]));
                break;
            }

            let chosen = match chosen {
                Some(c) => c,
                None => {
                    // No consistent linearization exists.
                    // Collect base names for the error message (skip object).
                    let base_names: Vec<String> = all_bases
                        .iter()
                        .filter(|b| Rc::as_ptr(b) != obj_ptr)
                        .map(|b| b.borrow().name.clone())
                        .collect();
                    let bases_str = base_names.join(", ");
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "Cannot create a consistent method resolution\norder (MRO) for bases {bases_str}"
                        ),
                    ));
                }
            };

            let chosen_ptr = Rc::as_ptr(&chosen);
            result.push(chosen);
            // Remove chosen from the front of every list where it appears.
            for list in &mut lists {
                if !list.is_empty() && Rc::as_ptr(&list[0]) == chosen_ptr {
                    list.remove(0);
                }
            }
        }

        Ok(result)
    }

    let obj = object_class_singleton();
    let obj_ptr = Rc::as_ptr(&obj);
    let mut mro = c3_linearize(class, obj_ptr)?;

    // Append the `object` singleton if it is not already present.
    if !mro.iter().any(|c| Rc::as_ptr(c) == obj_ptr) {
        mro.push(obj);
    }

    Ok(mro.into_iter().map(Value::py_class).collect())
}

/// Returns the list of direct subclasses of `class`, pruning stale weak refs.
/// Used by `__subclasses__()` dispatch (issue #1354).
fn class_direct_subclasses(class: &Rc<RefCell<PyClass>>) -> Vec<Value> {
    let borrowed = class.borrow();
    let mut subclasses = borrowed.subclasses.borrow_mut();
    // Retain only live weak refs and collect as Values.
    let mut result = Vec::new();
    subclasses.retain(|weak| {
        if let Some(rc) = weak.upgrade() {
            result.push(Value::py_class(rc));
            true
        } else {
            false
        }
    });
    result
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
        ValueKind::Range { .. } => name == "__iter__",
        ValueKind::BuiltinObject { ops, .. } => ops.has_method(name),
        _ => false,
    }
}
