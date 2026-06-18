impl Interpreter {
    pub(crate) fn get_attr(&mut self, target: &Value, name: &str) -> Result<Value> {
        // `obj.__class__ is type(obj)` for every value (#2150).  PyInstance has
        // its own `__class__` handling (and `__class__`-reassignment semantics)
        // in `get_attr_instance_raw`, so it is excluded here; every other kind
        // — primitives, containers, None, functions, classes (→ metatype),
        // modules, generators, bound methods — resolves through `value_class`,
        // the same source of truth `type(obj)` uses.
        if name == "__class__" && !matches!(target.kind(), ValueKind::PyInstance(_)) {
            return Ok(crate::builtin_modules::builtins::value_class(target));
        }
        // `obj.__doc__` for a built-in data value (int/str/list/None/…) is the
        // value's *type* docstring (#2151): `(5).__doc__ is int.__doc__`.  Only
        // primitive/builtin data values are routed here; functions, modules,
        // classes, properties keep their own `__doc__` handling below.
        if name == "__doc__"
            && let Some(cls) = crate::interpreter::primitive_class_for_value(target) {
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
                    if let Some(getattribute_val) =
                        lookup_class_attr(&class, "__getattribute__")
                        && matches!(getattribute_val.kind(), ValueKind::UserFunction(_)) {
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
                                    if let Some(getattr_val) =
                                        lookup_class_attr(&class, "__getattr__")
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
                Err(pyrust_core::py_err!("AttributeError", "'super' object has no attribute '{name}'"))
            }
            ValueKind::SuperProxyClass { class, obj_class } => {
                let class = Rc::clone(class);
                let obj_class = Rc::clone(obj_class);
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
                            // `object.__init_subclass__` is a builtin classmethod.
                            // Bind obj_class so the dispatcher prepends it as `cls`
                            // when `super().__init_subclass__(**kwargs)` is called inside
                            // a user-defined `__init_subclass__` (PEP 487 / issue #1080).
                            // Only wrap known builtin classmethods to avoid prepending
                            // a class where an instance is expected (e.g. __init__).
                            //
                            // Issue #1956: `type.__call__` is also bound to
                            // `obj_class` so that `super().__call__(*a)` inside a
                            // metaclass `__call__` override prepends the class
                            // being constructed and runs the default __new__+__init__.
                            None => {
                                let builtin_cm_name = match value.kind() {
                                    ValueKind::BuiltinFunction(fn_name)
                                        if is_builtin_classmethod(fn_name)
                                            || fn_name == "type.__call__" =>
                                    {
                                        Some(fn_name.to_string())
                                    }
                                    _ => None,
                                };
                                match builtin_cm_name {
                                    Some(fn_name) => {
                                        pyrust_builtins::super_bound_builtin::super_bound_builtin(
                                            fn_name,
                                            Value::py_class(Rc::clone(&obj_class)),
                                        )
                                    }
                                    None => value,
                                }
                            }
                        });
                    }
                }
                Err(pyrust_core::py_err!("AttributeError", "'super' object has no attribute '{name}'"))
            }
            // Access .setter / .deleter / .getter on a property descriptor itself.
            // These return a new property with the respective accessor replaced.
            _ if pyrust_builtins::property::property_partial_slot(target)
                == Some(None) =>
            {
                let (fget_val, fset_val, fdel_val, doc_val) =
                    pyrust_builtins::property::with_property(target, |s| {
                        (
                            (*s.fget).clone(),
                            (*s.fset).clone(),
                            (*s.fdel).clone(),
                            s.doc.clone(),
                        )
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
                    // `property.__doc__` is the explicit `doc=` argument if one
                    // was given, otherwise the getter's docstring (CPython
                    // copies `fget.__doc__` into the property).  `None` when
                    // neither is available (issue #1961).
                    "__doc__" => Ok(match doc_val {
                        Some(d) => d,
                        None => match fget_val.kind() {
                            ValueKind::UserFunction(func) => func.doc.borrow().clone(),
                            _ => Value::none(),
                        },
                    }),
                    // Descriptor-protocol dunders.  Accessing `p.__get__` etc.
                    // yields a bound method-wrapper (so `hasattr(p, "__get__")`
                    // is True and `f = p.__get__; f(obj, owner)` works); the
                    // actual dispatch happens when the wrapper is called (see
                    // `calls.rs`).
                    "__get__" => Ok(pyrust_builtins::property::property_method(
                        target.clone(),
                        pyrust_builtins::property::PropertyMethodKind::Get,
                    )),
                    "__set__" => Ok(pyrust_builtins::property::property_method(
                        target.clone(),
                        pyrust_builtins::property::PropertyMethodKind::Set,
                    )),
                    "__delete__" => Ok(pyrust_builtins::property::property_method(
                        target.clone(),
                        pyrust_builtins::property::PropertyMethodKind::Delete,
                    )),
                    _ => Err(pyrust_core::py_err!("AttributeError", "property object has no attribute '{name}'")),
                }
            }
            ValueKind::PyModule(module) => {
                let module = Rc::clone(module);
                // CPython 3.12 module_getattro builds the error message by
                // looking up __name__ in the module's __dict__.  If __name__
                // is absent (e.g. it was deleted), the error omits the module
                // name: "module has no attribute 'X'" rather than
                // "module 'foo' has no attribute 'X'".
                // Precompute this once for both error sites below.
                let name_tombstoned = module
                    .borrow()
                    .attrs
                    .get("__name__")
                    .is_some_and(|v| v.is_unset());
                if let Some(value) = module.borrow().attrs.get(name).cloned() {
                    // A stored Value::unset() is a deletion tombstone written by
                    // delete_attr for synthetic dunders.  Treat it as absent.
                    if value.is_unset() {
                        let msg = if name_tombstoned {
                            format!("module has no attribute '{name}'")
                        } else {
                            let mod_name = module.borrow().name.clone();
                            format!("module '{mod_name}' has no attribute '{name}'")
                        };
                        return Err(pyrust_core::py_err!("AttributeError", msg));
                    }
                    return Ok(value);
                }
                // Synthetic dunder attributes for built-in modules.  These are
                // not stored in the attrs map (to avoid polluting vars(m)) but
                // are synthesised here, mirroring CPython 3.12 module object
                // slot behaviour:
                //   __name__    — the module's dotted name string.
                //   __package__ — empty string for all top-level builtin modules
                //                 (CPython 3.12: `sys.__package__ == ''`).
                //   __loader__  — None; a full BuiltinImporter object is out of
                //                 scope for this implementation.
                //   __spec__    — None; same reason.
                //   __doc__     — None; pyrust does not store module docstrings.
                // Note: __file__ is intentionally absent.  CPython 3.12 raises
                // AttributeError for `sys.__file__`; builtin modules have no
                // file path to report.
                let mod_name = module.borrow().name.clone();
                match name {
                    "__name__" => return Ok(Value::string(mod_name)),
                    "__package__" => return Ok(Value::string(String::new())),
                    "__loader__" | "__spec__" => return Ok(Value::none()),
                    "__doc__" => return Ok(Value::none()),
                    "__dict__" => {
                        // Build a snapshot dict of the module namespace.
                        // Include both the stored attrs and the synthetic
                        // dunder attributes that get_attr synthesises above.
                        // Value::unset() is a deletion tombstone (written by
                        // delete_attr for synthetic dunders); filter it out so
                        // deleted dunders don't appear in __dict__.
                        let attrs_snapshot: HashMap<String, Value> =
                            module.borrow().attrs.clone();
                        let mut d: PyDict = attrs_snapshot
                            .iter()
                            .filter(|(_, v)| !v.is_unset())
                            .map(|(k, v)| (PyKey::str_from(k), v.clone()))
                            .collect();
                        // Synthetic dunders: add only if the key is neither
                        // already present in attrs (user override) nor
                        // tombstoned (explicitly deleted by the user).
                        let is_absent = |key: &str| !attrs_snapshot.contains_key(key);
                        let name_key = PyKey::str_from("__name__");
                        if is_absent("__name__") {
                            d.insert(name_key, Value::string(mod_name));
                        }
                        let pkg_key = PyKey::str_from("__package__");
                        if is_absent("__package__") {
                            d.insert(pkg_key, Value::string(String::new()));
                        }
                        let spec_key = PyKey::str_from("__spec__");
                        if is_absent("__spec__") {
                            d.insert(spec_key, Value::none());
                        }
                        let loader_key = PyKey::str_from("__loader__");
                        if is_absent("__loader__") {
                            d.insert(loader_key, Value::none());
                        }
                        let doc_key = PyKey::str_from("__doc__");
                        if is_absent("__doc__") {
                            d.insert(doc_key, Value::none());
                        }
                        return Ok(Value::dict(d));
                    }
                    _ => {}
                }
                let msg = if name_tombstoned {
                    format!("module has no attribute '{name}'")
                } else {
                    format!("module '{mod_name}' has no attribute '{name}'")
                };
                Err(PyError::attribute_error(
                    msg,
                    Some(name.to_string()),
                    Some(target.clone()),
                ))
            }
            ValueKind::UserFunction(func) => {
                match name {
                    "__name__" => {
                        return Ok(Value::string(func.effective_name()));
                    }
                    "__qualname__" => {
                        let q = func.effective_qualname();
                        return Ok(Value::string(q));
                    }
                    "__module__" => return Ok(func.module_value()),
                    "__doc__" => return Ok(func.doc.borrow().clone()),
                    "__dict__" => {
                        // Return the live dict object — CPython returns the same
                        // object every time, so `d = f.__dict__; d['x'] = 1`
                        // makes `f.x` visible.  Initialise lazily on first access.
                        let attrs_rc = func_attrs_rc(func);
                        return Ok(attrs_rc.borrow().clone());
                    }
                    "__annotations__" => {
                        // Lazily materialised (#2256) and stored, so repeated reads
                        // yield the same object identity, matching CPython:
                        // `f.__annotations__ is f.__annotations__` is True.
                        return Ok(func.annotations_value());
                    }
                    "__defaults__" => {
                        // #2395: tuple of positional defaults, or the per-object
                        // override set via `f.__defaults__ = …`.  `None` when no
                        // positional default exists (CPython semantics).
                        return Ok(func.defaults_value());
                    }
                    "__kwdefaults__" => {
                        // #2395: dict of keyword-only defaults, or the per-object
                        // override set via `f.__kwdefaults__ = …`.  `None` when
                        // none exist (CPython returns `None`, not an empty dict).
                        return Ok(func.kwdefaults_value());
                    }
                    "__globals__" => {
                        // The module global namespace dict.  pyrust keeps a single
                        // live module-globals dict; CPython returns the same dict
                        // object each access (identity preserved).
                        return Ok(self.module_globals_dict.clone());
                    }
                    "__closure__" => {
                        // A tuple of `cell` objects (one per free variable, in
                        // `co_freevars` order), or `None` when the function has
                        // no free variables (issue #2106).  pyrust resolves free
                        // variables through the captured `env` chain rather than
                        // CPython-style cells; `build_closure` recovers the cell
                        // set from that chain.
                        return Ok(self.build_closure(func));
                    }
                    "__code__" => {
                        // A lightweight code object carrying the introspection
                        // attributes most consumers read: co_name / co_argcount /
                        // co_varnames (CPython parameter order), plus the
                        // best-effort co_flags / co_filename / co_firstlineno /
                        // co_consts / co_names (issues #1959, #2171).  co_name is
                        // the original declared name baked into the (immutable)
                        // code object; reassigning `f.__name__` does not change
                        // `f.__code__.co_name`, so `build_code_object` uses
                        // `func.name` rather than the mutable `user_name`.
                        return Ok(self.build_code_object(func));
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
                    "__call__"
                        if !matches!(func.kind, UserFunctionKind::ClassMethod) =>
                    {
                        // Issue #2550: a plain function, lambda, or staticmethod is
                        // callable, so CPython exposes `f.__call__ ==
                        // <method-wrapper '__call__' of function object at 0x...>`
                        // (and `hasattr(f, '__call__') is True`).  Surface a wrapper
                        // bound to the function; calling it re-dispatches onto `f`
                        // (handled by `as_type_call_wrapper` in
                        // `call_function_expanded`).  `classmethod` is excluded —
                        // CPython's classmethod object is not itself callable.
                        return Ok(pyrust_builtins::type_call_wrapper::call_wrapper(
                            target.clone(),
                            "function",
                        ));
                    }
                    _ => {}
                }
                // Fall through to arbitrary dynamic attrs.
                // Short-circuit without initialising if no attrs have been stored yet.
                if let Some(rc) = func.attrs.borrow().as_ref().map(Rc::clone)
                    && let Some(v) = rc.borrow().as_dict().and_then(|d| d.get(&StrKey(name)).cloned()) {
                        return Ok(v);
                    }
                let type_name = match func.kind {
                    UserFunctionKind::StaticMethod => "staticmethod",
                    UserFunctionKind::ClassMethod => "classmethod",
                    _ => "function",
                };
                Err(PyError::attribute_error(
                    format!("'{type_name}' object has no attribute '{name}'"),
                    Some(name.to_string()),
                    Some(target.clone()),
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
                        return Err(pyrust_core::py_err!("AttributeError", "'method_descriptor' object has no attribute '__module__'"));
                    }
                    return Ok(Value::string("builtins"));
                }
                if name == "__call__"
                    && !matches!(func_name, "generator" | "str" | "property")
                {
                    // Issue #2550: builtin functions (`len`, `print`) and method
                    // descriptors (`str.upper`) are callable, so CPython exposes
                    // `len.__call__ == <method-wrapper '__call__' of
                    // builtin_function_or_method object at 0x...>` (and
                    // `... of method_descriptor object ...` for the dotted form),
                    // with `hasattr(len, '__call__') is True`.  Surface a wrapper
                    // bound to the builtin; calling it re-dispatches onto the
                    // builtin (so `len.__call__([1,2,3]) == 3`).  The `generator`
                    // / `str` / `property` type-token names are excluded — they
                    // are handled as type objects below, not as plain builtins.
                    let owner = if func_name.contains('.') {
                        "method_descriptor"
                    } else {
                        "builtin_function_or_method"
                    };
                    return Ok(pyrust_builtins::type_call_wrapper::call_wrapper(
                        target.clone(),
                        owner,
                    ));
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
                        _ => Err(pyrust_core::py_err!("AttributeError", "type object 'str' has no attribute '{name}'")),
                    }
                } else if func_name == "property"
                    && matches!(name, "__get__" | "__set__" | "__delete__")
                {
                    // Issue #1835: the `property` type token (a BuiltinFunction
                    // in pyrust, since `property` is not yet a real PyClass)
                    // exposes the descriptor protocol so that
                    // `hasattr(property, "__get__")` is True like CPython.  The
                    // wrapper is bound to an empty property; this serves
                    // introspection (`hasattr`/`getattr`) — calling it unbound
                    // from the type object is not a supported path.
                    use pyrust_builtins::property::PropertyMethodKind as K;
                    let kind = match name {
                        "__get__" => K::Get,
                        "__set__" => K::Set,
                        _ => K::Delete,
                    };
                    let empty = pyrust_builtins::property::property(
                        Value::none(),
                        Value::none(),
                        Value::none(),
                    );
                    Ok(pyrust_builtins::property::property_method(empty, kind))
                } else {
                    Err(pyrust_core::py_err!("AttributeError", "type object '{}' has no attribute '{name}'", func_name))
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
                    pyrust_builtins::bound_method::as_bound_method(target)
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
                    // `__slots__` member_descriptor exposes its descriptor
                    // protocol as bound methods (`S.x.__get__`, …), issue #2084.
                    if ops.type_name()
                        == pyrust_builtins::member_descriptor::MEMBER_DESCRIPTOR_TYPE_NAME
                        && matches!(name, "__get__" | "__set__" | "__delete__")
                    {
                        return Ok(pyrust_builtins::bound_method::bound_method(
                            name,
                            target.clone(),
                        ));
                    }
                    // Issue #2133: a `GenericAlias` (`list[int]`) proxies
                    // attribute access to its `__origin__`, so `list[int].
                    // __name__ == 'list'`, `.__mro__`, instance methods, etc.
                    // resolve to the origin's.  CPython's `ga_getattro` keeps a
                    // small reserved set on the alias itself (`__origin__`,
                    // `__args__`, `__parameters__` are served by `ops.getattr`
                    // above; `__class__` resolves to the GenericAlias type, not
                    // the origin; `__mro_entries__`/`__reduce__`/`__reduce_ex__`/
                    // `__copy__`/`__deepcopy__` are the alias's own protocol
                    // methods) and forwards everything else to the origin.
                    if ops.type_name() == pyrust_builtins::generic_alias::TYPE_NAME
                        && !matches!(
                            name,
                            "__class__"
                                | "__mro_entries__"
                                | "__reduce__"
                                | "__reduce_ex__"
                                | "__copy__"
                                | "__deepcopy__"
                        )
                        && let Some(origin) = ops.getattr(state, "__origin__") {
                            return self.get_attr(&origin, name);
                        }
                }
                if builtin_has_method(target, name) {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        name,
                        target.clone(),
                    ));
                }
                // Issue #2151: BuiltinObject primitives (frozenset, bytearray,
                // mappingproxy) inherit `object`'s dunders (`__eq__`, `__str__`,
                // `__hash__`, …) via their primitive class, the same as the
                // scalar/container `_` arm below.  Without this fall-through
                // `hasattr(frozenset(), '__eq__')` was False while
                // `dir(frozenset())` listed it.
                if let Some(cls) = crate::interpreter::primitive_class_for_value(target)
                    && let Some(val) = lookup_class_attr(&cls, name) {
                        return Ok(val);
                    }
                let type_name = pyrust_core::builtin_type_name(target);
                Err(PyError::attribute_error(
                    format!("'{type_name}' object has no attribute '{name}'"),
                    Some(name.to_string()),
                    Some(target.clone()),
                ))
            }
            ValueKind::Generator(state_rc) => {
                // Generator introspection attributes (issue #1270).
                // All six attributes exposed by CPython 3.12's generator type:
                //   __name__, __qualname__, gi_running, gi_yieldfrom, gi_frame, gi_code.
                //
                // Async generators (#2280) expose the *asynchronous* iteration
                // protocol (`__aiter__`/`__anext__`/`asend`/`athrow`/`aclose`)
                // and NOT the synchronous one (`__iter__`/`__next__`/`send`).
                // Detect this before the generic method exposure below so the
                // two surfaces don't overlap (CPython's `async_generator` has no
                // `__next__`/`send`/`__iter__`).
                // Classify the generator subtype up-front so the protocol
                // surfaces don't overlap.  CPython exposes a distinct set of
                // methods per subtype:
                //   - plain generator: __iter__/__next__/send/throw/close
                //   - coroutine:       send/throw/close/__await__   (NO
                //     __iter__/__next__ — coroutines are awaitable, not
                //     iterable; issue #2314)
                //   - async generator: __aiter__/__anext__/asend/athrow/aclose
                let (is_async_gen, is_coroutine_only) = {
                    state_rc
                        .try_borrow()
                        .ok()
                        .and_then(|b| {
                            b.downcast_ref::<GeneratorFrame>().map(|f| {
                                (
                                    f.is_async_generator(),
                                    f.is_coroutine && !f.code.is_generator,
                                )
                            })
                        })
                        .unwrap_or((false, false))
                };
                if is_async_gen {
                    match name {
                        "__aiter__" | "__anext__" | "asend" | "athrow" | "aclose" => {
                            return Ok(pyrust_builtins::bound_method::bound_method(
                                name.to_string(),
                                target.clone(),
                            ));
                        }
                        _ => {}
                    }
                }
                if is_coroutine_only {
                    // A coroutine exposes send/throw/close but NOT
                    // __iter__/__next__ (CPython gates those off — you must
                    // `await`, not iterate; issue #2314).  CPython also exposes
                    // `__await__` (returning a `coroutine_wrapper`); pyrust
                    // drives `await` through the native event-loop bridge and
                    // does not surface that wrapper object, so `__await__` is
                    // intentionally not exposed here (documented limitation).
                    match name {
                        "send" | "close" | "throw" => {
                            return Ok(pyrust_builtins::bound_method::bound_method(
                                name.to_string(),
                                target.clone(),
                            ));
                        }
                        _ => {}
                    }
                }
                // Issue #1413: also expose the iteration protocol methods as
                // bound-method values so that hasattr/getattr see them.
                // These apply to all generator subtypes (GeneratorFrame,
                // NativeIterFrame, CallableIter, …), so they are checked
                // before the downcast.  Skipped for async generators and
                // coroutines, which expose their own protocol above instead.
                if !is_async_gen && !is_coroutine_only {
                    match name {
                        "__iter__" | "__next__" | "send" | "close" | "throw" => {
                            return Ok(pyrust_builtins::bound_method::bound_method(
                                name.to_string(),
                                target.clone(),
                            ));
                        }
                        _ => {}
                    }
                }
                let state_rc = Rc::clone(state_rc);
                let borrow = state_rc.borrow();
                if let Some(frame) = borrow.downcast_ref::<GeneratorFrame>() {
                    // CPython exposes introspection attributes under a
                    // type-specific prefix: `gi_*` on a plain generator,
                    // `ag_*` on an async generator, `cr_*` on a coroutine
                    // (issue #2302).  A given object exposes ONLY its own
                    // prefix; the other two raise AttributeError.  The
                    // underlying semantics are shared (suspended frame,
                    // running flag, code stub, awaited/delegated sub-iterator),
                    // so resolve the prefix first, then dispatch on the suffix.
                    //   - async generator: ag_running / ag_frame / ag_code /
                    //     ag_await  (no ag_yieldfrom in CPython).
                    //   - coroutine:       cr_running / cr_frame / cr_code /
                    //     cr_await.
                    //   - plain generator: gi_running / gi_frame / gi_code /
                    //     gi_yieldfrom  (no gi_await in CPython).
                    // `__name__` / `__qualname__` apply to all three.
                    let is_coroutine_only = frame.is_coroutine && !frame.code.is_generator;
                    // Split off a 3-byte introspection prefix. `name.get(..3)`
                    // is UTF-8-boundary-safe (returns None when byte index 3
                    // falls inside a multibyte char, e.g. `getattr(g, "agé")`),
                    // where the old `split_at(3)` panicked.
                    let prefix = name.get(..3).unwrap_or("");
                    let suffix = name.get(3..).unwrap_or("");
                    let prefix_matches = match prefix {
                        "ag_" => is_async_gen,
                        "cr_" => is_coroutine_only,
                        "gi_" => !is_async_gen && !is_coroutine_only,
                        _ => false,
                    };
                    match name {
                        "__name__" => return Ok(Value::string(frame.fn_name.as_ref())),
                        "__qualname__" => return Ok(Value::string(frame.qualname.as_ref())),
                        _ if prefix_matches => match suffix {
                            // running is always False when accessed from outside
                            // the body (True is only observable from within —
                            // pyrust does not expose re-entrant guards, matching
                            // CPython's "False unless currently on the C call
                            // stack" rule).
                            "running" => return Ok(Value::bool_(false)),
                            // frame: the suspended frame object (or None once
                            // exhausted), built from the retained FnCode (#2185).
                            "frame" => return Ok(self.build_generator_frame_object(frame)),
                            // code: pyrust does not expose a standalone code
                            // object here; return None to avoid AttributeError
                            // (issue #1270).
                            "code" => return Ok(Value::none()),
                            // gi_yieldfrom / ag_await / cr_await: the sub-iterator
                            // being delegated to via `yield from`, or awaited via
                            // an inner `await` (both compile to YieldFrom), else
                            // None.  When suspended at a YieldFrom the sub-iterator
                            // sits in iter_reg.  `frame.pc == 0` means the body
                            // hasn't started — don't inspect insns[0] (iter_reg
                            // unloaded).  CPython spells this `yieldfrom` for a
                            // plain generator and `await` for an async gen /
                            // coroutine, but never both on one object.
                            "yieldfrom" if prefix == "gi_" => {
                                if !frame.done && frame.pc != 0
                                    && let Some(crate::bytecode::Insn::YieldFrom {
                                        iter_reg, ..
                                    }) = frame.code.insns.get(frame.pc)
                                {
                                    let sub_iter = frame.regs[*iter_reg as usize].clone();
                                    return Ok(sub_iter);
                                }
                                return Ok(Value::none());
                            }
                            "await" if prefix == "ag_" || prefix == "cr_" => {
                                if !frame.done && frame.pc != 0
                                    && let Some(crate::bytecode::Insn::YieldFrom {
                                        iter_reg, ..
                                    }) = frame.code.insns.get(frame.pc)
                                {
                                    let sub_iter = frame.regs[*iter_reg as usize].clone();
                                    return Ok(sub_iter);
                                }
                                return Ok(Value::none());
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                let obj_name = if is_async_gen {
                    "async_generator"
                } else if borrow
                    .downcast_ref::<GeneratorFrame>()
                    .is_some_and(|f| f.is_coroutine)
                {
                    "coroutine"
                } else {
                    "generator"
                };
                Err(PyError::attribute_error(
                    format!("'{obj_name}' object has no attribute '{name}'"),
                    Some(name.to_string()),
                    Some(target.clone()),
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
                // Type-specific read-only attributes (complex .real/.imag and
                // .conjugate, numeric-tower real/imag/numerator/denominator,
                // range start/stop/step) live in pyrust-builtins so this
                // dispatcher holds no per-type knowledge.
                if let Some(v) = pyrust_builtins::numeric_attrs_descriptor::complex_attr(target, name)
                    .or_else(|| pyrust_builtins::numeric_attrs_descriptor::numeric_tower_attr(target, name))
                    .or_else(|| pyrust_builtins::numeric_attrs_descriptor::range_attr(target, name))
                {
                    return Ok(v);
                }
                // `BuiltinObject` types can expose arbitrary attributes via
                // `BuiltinTypeOps::getattr` (e.g. `GenericAlias.__origin__`,
                // `GenericAlias.__args__`).  Probe before the generic
                // `has_method` bound-method path.
                if let ValueKind::BuiltinObject { ops, state } = target.kind()
                    && let Some(val) = ops.getattr(state, name) {
                        return Ok(val);
                    }
                // Built-in type instance method lookup: list.append, str.upper, etc.
                if builtin_has_method(target, name) {
                    return Ok(pyrust_builtins::bound_method::bound_method(
                        name,
                        target.clone(),
                    ));
                }
                // Issue #2070: CPython sets `__hash__ = None` on the unhashable
                // built-in types (list/dict/set/bytearray), so `[1].__hash__`
                // resolves to `None` (and `[1].__hash__()` raises
                // `'NoneType' object is not callable`).  Intercept before the
                // object-MRO `lookup_class_attr` fallback, which would otherwise
                // surface the inherited `object.__hash__`.
                if name == "__hash__"
                    && matches!(
                        pyrust_core::builtin_type_name(target).as_ref(),
                        "list" | "dict" | "set" | "bytearray"
                    )
                {
                    return Ok(Value::none());
                }
                // Fallback: check the primitive class attrs for classmethods
                // accessible on instances (e.g. `(1.0).fromhex`).
                if let Some(cls) = crate::interpreter::primitive_class_for_value(target)
                    && let Some(val) = lookup_class_attr(&cls, name) {
                        return Ok(val);
                    }
                let type_name = pyrust_core::builtin_type_name(target);
                Err(PyError::attribute_error(
                    format!("'{type_name}' object has no attribute '{name}'"),
                    Some(name.to_string()),
                    Some(target.clone()),
                ))
            }
        }
    }

    /// `Cls.name` attribute access (the `ValueKind::PyClass` arm of
    /// `get_attr`).  Handles `__name__`/`__qualname__`/`__mro__`/`__bases__`,
    /// MRO attribute lookup, classmethod/staticmethod binding, metaclass
    /// descriptors, and the AttributeError fallback.
    fn get_attr_class(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        name: &str,
    ) -> Result<Value> {
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
            // `(object,)` — except for `object` itself, which has no bases
            // and reports `()` (issue #1969).
            if Rc::ptr_eq(&class, &object_class_singleton()) {
                return Ok(Value::tuple(Vec::new()));
            }
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
        if name == "__base__" {
            // `__base__` is the single primary base used for instance layout.
            // For typical single/multiple inheritance this is `bases[0]`.
            // `object` itself has no base and reports `None` (issue #1969).
            if Rc::ptr_eq(&class, &object_class_singleton()) {
                return Ok(Value::none());
            }
            let base = class.borrow().base.clone();
            return Ok(match base {
                Some(b) => Value::py_class(b),
                None => Value::py_class(object_class_singleton()),
            });
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
            let empty = Value::dict(PyDict::default());
            class
                .borrow_mut()
                .attrs
                .insert("__annotations__".to_string(), empty.clone());
            return Ok(empty);
        }
        // Issue #1563: `dict.fromkeys` is a classmethod that must return
        // an instance of `cls`, not always a plain `dict`.  When called
        // on a dict subclass, bind the class as the receiver so that the
        // bound-method dispatch in calls.rs can call `cls()` instead of
        // hard-coding `Value::dict(map)`.
        if name == "fromkeys"
            && !is_primitive_class(&class)
            && class_chain_contains_name(&class, "dict")
        {
            return Ok(pyrust_builtins::bound_method::bound_method(
                "fromkeys",
                Value::py_class(Rc::clone(&class)),
            ));
        }
        // Issue #1617: numeric-tower read-only properties and conjugate
        // are exposed on the `int` and `float` class objects as descriptors
        // (matching CPython 3.12's `getset_descriptor` / `method_descriptor`).
        // `bool` and any other int subclass inherit from `int` and expose
        // the same descriptors with class_name "int" (CPython: `bool.real`
        // says "of 'int' objects").  Use class_chain_contains_name so that
        // user-defined subclasses (`class MyInt(int): pass`) also get the
        // descriptors, matching CPython MRO-based lookup.
        //
        // Check int chain before float chain: int (and bool) takes priority.
        // The attr_name literals must be `'static` — use explicit match arms
        // rather than passing `name` directly.
        {
            let descriptor = if class_chain_contains_name(&class, "int") {
                match name {
                    "real" => Some(
                        pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                            "real", "int",
                        ),
                    ),
                    "imag" => Some(
                        pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                            "imag", "int",
                        ),
                    ),
                    "numerator" => Some(
                        pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                            "numerator", "int",
                        ),
                    ),
                    "denominator" => Some(
                        pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                            "denominator", "int",
                        ),
                    ),
                    "conjugate" => Some(
                        pyrust_builtins::numeric_attrs_descriptor::method_descriptor(
                            "conjugate", "int",
                        ),
                    ),
                    _ => None,
                }
            } else if class_chain_contains_name(&class, "float") {
                match name {
                    "real" => Some(
                        pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                            "real", "float",
                        ),
                    ),
                    "imag" => Some(
                        pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                            "imag", "float",
                        ),
                    ),
                    "conjugate" => Some(
                        pyrust_builtins::numeric_attrs_descriptor::method_descriptor(
                            "conjugate", "float",
                        ),
                    ),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(d) = descriptor {
                return Ok(d);
            }
        }
        // Issue #2081: a *data* descriptor on the metaclass shadows a same-named
        // attribute in the class's own dict.  CPython's `type.__getattribute__`
        // resolves `meta_data_descriptor` BEFORE the class MRO; only non-data
        // metaclass attributes (handled further below, #1956/#2078) are shadowed
        // by class-own entries.  Check the metaclass MRO first and, when the
        // attribute found there is a data descriptor (e.g. a `property`), invoke
        // its `__get__(cls, type(cls))`.
        if let Some(meta_val) = metaclass_dunder(&class, name)
            && is_data_descriptor(&meta_val) {
                return call_descriptor_get(
                    self,
                    &meta_val,
                    Value::py_class(Rc::clone(&class)),
                    Value::py_class(metaclass_of(&class)),
                    name,
                );
            }
        // Issue #2291: the unhashable built-in types set `__hash__ = None` on
        // the *type* (CPython: `list.__hash__ is None`).  Accessing
        // `list.__hash__` / `dict.__hash__` / `set.__hash__` /
        // `bytearray.__hash__` therefore yields `None`, and calling it raises
        // `'NoneType' object is not callable`.  Mirror the instance path
        // (`[1].__hash__` → `None`) here for the class attribute, but only when
        // no closer user override exists (an MRO lookup that lands on the
        // inherited `object.__hash__` sentinel) so a subclass that defines its
        // own `__hash__` still resolves to that function.  Issue #2299:
        // `class_hash_inherits_builtin_none` walks the MRO so that a subclass
        // which re-enables hashing (`__hash__ = object.__hash__`) shadows the
        // builtin's implicit `None` and keeps a callable `__hash__`.
        if name == "__hash__"
            && matches!(
                lookup_class_attr(&class, name).as_ref().map(|v| v.kind()),
                Some(ValueKind::BuiltinFunction("object.__hash__"))
            )
            && class_hash_inherits_builtin_none(&class)
        {
            return Ok(Value::none());
        }
        // Issue #2276: object-level dunders that a primitive type *overrides* in
        // CPython are attributed to the called type, not `object`
        // (`str.__hash__()` → "... of 'str' object ...", not 'object').  When the
        // MRO lookup below would resolve such a dunder to the inherited
        // `object.__X__` sentinel — i.e. there is no closer user override — we
        // instead return a type-qualified `BuiltinFunction("<type>.__X__")`
        // sentinel.  It is synthesised only for direct `T.__X__` attribute
        // access and is deliberately NOT stored in the class `attrs` (which
        // would shadow the `object.__X__` sentinel that the repr/hash rendering
        // paths key on via `lookup_class_attr`).  The matching dispatch arm in
        // `call_function_expanded` validates the receiver (naming the called
        // type) and delegates to the shared `object.__X__` / formatting
        // machinery.  Ownership verified against python3.12
        // (`[c for c in T.__mro__ if '__X__' in c.__dict__]`).
        // Issue #2433: the six rich-comparison slots join `__hash__`/`__repr__`/
        // `__str__`/`__format__` here — every primitive owns its comparisons in
        // CPython, so `list.__eq__` must read `'list'`, not the inherited
        // `object.__eq__`.
        if matches!(
            name,
            "__hash__"
                | "__repr__"
                | "__str__"
                | "__format__"
                | "__eq__"
                | "__ne__"
                | "__lt__"
                | "__le__"
                | "__gt__"
                | "__ge__"
        )
            && let Some(qualified) = primitive_owned_object_dunder(&class, name)
            && matches!(
                lookup_class_attr(&class, name).as_ref().map(|v| v.kind()),
                Some(ValueKind::BuiltinFunction(
                    "object.__hash__"
                        | "object.__repr__"
                        | "object.__str__"
                        | "object.__format__"
                        | "object.__eq__"
                        | "object.__ne__"
                        | "object.__lt__"
                        | "object.__le__"
                        | "object.__gt__"
                        | "object.__ge__"
                ))
            )
        {
            return Ok(Value::builtin_function(qualified));
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
                    // Issue #2479: a non-dunder method *defined on* a builtin
                    // primitive subclass (e.g. `OrderedDict.move_to_end`, a
                    // Python `def` on `OrderedDict`) is a C `method_descriptor`
                    // in CPython, so calling it with an unrelated receiver
                    // (`OrderedDict.move_to_end({1: 2}, 1)`) raises a receiver-
                    // class TypeError.  Wrap it so the call site can enforce.
                    UserFunctionKind::Regular => {
                        wrap_subclass_unbound_method(&class, name, value)
                    }
                    UserFunctionKind::Builtin(_) => value,
                },
                // classmethod(non_fn): CPython returns a method object bound to
                // the class; pyrust returns the wrapped value (no new Value
                // variant for non-UserFunction class-bound values).
                ClassDescTag::ClassMethodAny(w) => w,
                // staticmethod(non_fn): returns the wrapped value directly,
                // matching CPython (`C.s` where `s = staticmethod(42)` → 42).
                ClassDescTag::StaticMethodAny(w) => w,
                // Issue #1080: builtin classmethods (e.g. `object.__init_subclass__`)
                // must be bound to `cls` when accessed on a class, just like
                // user-defined classmethods are bound via ClassBoundMethod.
                // CPython's classmethod_descriptor.__get__ returns a bound
                // builtin_function_or_method with __self__ = cls.
                ClassDescTag::Other => {
                    let builtin_cm_name = match value.kind() {
                        ValueKind::BuiltinFunction(fn_name)
                            if is_builtin_classmethod(fn_name) =>
                        {
                            Some(fn_name.to_string())
                        }
                        _ => None,
                    };
                    match builtin_cm_name {
                        Some(fn_name) => {
                            pyrust_builtins::super_bound_builtin::super_bound_builtin(
                                fn_name,
                                Value::py_class(Rc::clone(&class)),
                            )
                        }
                        // Issue #2479: an inherited primitive method sentinel
                        // (e.g. `OrderedDict.clear` → `BuiltinFunction("dict.clear")`)
                        // accessed on a *proper subclass* of the owning
                        // primitive must reject an unrelated (e.g. plain-`dict`)
                        // receiver, matching CPython's owner-class descriptor
                        // guard.  Wrap so the call site can enforce.
                        None => wrap_subclass_unbound_method(&class, name, value),
                    }
                }
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
            || Rc::ptr_eq(&class, &object_class_singleton())
            || Rc::ptr_eq(&class, &crate::interpreter::method_type_singleton())
            || Rc::ptr_eq(&class, &crate::interpreter::function_type_singleton());
        if name == "__module__" && is_builtin_class {
            return Ok(Value::string("builtins"));
        }
        if name == "__doc__" && is_builtin_class {
            let class_name = class.borrow().name.clone();
            return Ok(match builtin_class_doc(&class_name) {
                Some(doc) => Value::string(doc),
                None => Value::none(),
            });
        }
        // Issue #1956/#1960: on a miss in `cls`'s own MRO, consult the
        // metaclass's MRO for the attribute, mirroring CPython's
        // `type.__getattribute__` (which looks up `name` on `type(cls)` after
        // the class's own dict).  This lets a metaclass method or attribute
        // (e.g. a `_instances` cache used by a singleton `__call__`) be reached
        // via `cls.attr`.  `metaclass_dunder` resolves user-defined attributes
        // on the metaclass MRO, returning `None` for ordinary classes.
        if name != "__getattr__"
            && let Some(meta_val) = metaclass_dunder(&class, name) {
                if let ValueKind::UserFunction(f) = meta_val.kind() {
                    // A metaclass method accessed via `cls.method` binds `cls`
                    // as the receiver (cls is an "instance" of the metaclass),
                    // so the dispatch prepends `cls` as the method's first arg.
                    return Ok(match f.kind {
                        UserFunctionKind::ClassMethod => {
                            // A classmethod on the metaclass receives the
                            // metaclass itself as `cls`.
                            Value::class_bound_method(Rc::clone(f), metaclass_of(&class))
                        }
                        UserFunctionKind::StaticMethod => {
                            if let Some(inner) = f.wrapped_func.as_ref() {
                                Value::user_function(Rc::clone(inner))
                            } else {
                                Value::with_function_kind(Rc::clone(f), UserFunctionKind::Regular)
                            }
                        }
                        _ => Value::class_bound_method(Rc::clone(f), Rc::clone(&class)),
                    });
                }
                return Ok(meta_val);
            }
        // Issue #1960: on an MRO miss, fall back to the metaclass's
        // `__getattr__` (CPython's `type.__getattribute__` ends by invoking
        // `type(cls).__getattr__(cls, name)` if the metaclass defines one).
        // `metaclass_dunder` returns `Some` only for a user override, so
        // ordinary classes keep raising `AttributeError` directly.
        if let Some(getattr_val) = metaclass_dunder(&class, "__getattr__")
            && let ValueKind::UserFunction(f) = getattr_val.kind() {
                let func = Rc::clone(f);
                return self.call_user_function_expanded(
                    func,
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(name),
                    }],
                    &[Value::py_class(Rc::clone(&class))],
                );
            }
        // Issue #2096: every class is callable (to construct instances) via the
        // built-in `type` metaclass slot `type.__call__`.  When a class defines
        // no `__call__` of its own (so the MRO lookup above missed) and its
        // metaclass is the built-in `type` (so the user-metaclass fallback also
        // missed), surface `type.__call__` as a `method-wrapper` bound to the
        // class — exactly as CPython does (`C.__call__ ==
        // <method-wrapper '__call__' of type object at 0x...>`).  This keeps
        // `hasattr(C, '__call__')` consistent with `callable(C)`, and
        // `C.__call__(...)` constructs an instance just like `C(...)`.
        // Restricted to `__call__` so that other `type`-only dunders that pyrust
        // does not model are unaffected, and only when no user metaclass
        // overrides `__call__` (handled by the `metaclass_dunder` path above).
        if name == "__call__" {
            return Ok(pyrust_builtins::type_call_wrapper::type_call_wrapper(
                Value::py_class(Rc::clone(&class)),
            ));
        }
        let class_name = class.borrow().name.clone();
        Err(PyError::attribute_error(
            format!("type object '{}' has no attribute '{}'", class_name, name),
            Some(name.to_string()),
            Some(Value::py_class(Rc::clone(&class))),
        ))
    }

    /// The CPython `slot_tp_getattr_hook` fallback: if `class` defines
    /// `__getattr__`, invoke it as `__getattr__(instance, name)` and return the
    /// result; otherwise return `None` so the caller proceeds (re-raising the
    /// original `AttributeError`, or continuing normal lookup).
    ///
    /// Shared by the three sites in `get_attr_instance_raw` that previously
    /// inlined this identical lookup-and-invoke (two on a descriptor `__get__`
    /// raising `AttributeError`, one as the final no-attribute fallback).
    fn try_invoke_getattr_hook(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        instance: &Rc<RefCell<PyInstance>>,
        name: &str,
    ) -> Option<Result<Value>> {
        let getattr_val = lookup_class_attr(class, "__getattr__")?;
        Some(invoke_class_method(
            self,
            getattr_val,
            Value::py_instance(Rc::clone(instance)),
            &[ExpandedCallArg {
                name: None,
                value: Value::string(name),
            }],
        ))
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
            && is_data_descriptor(&class_val) {
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

        // Step 2: Instance __dict__.  Scope the shared borrow so it is dropped
        // before any materialisation below re-borrows the instance.
        let attr_value = instance.borrow().attrs.get(name).cloned();
        if let Some(value) = attr_value {
            // Issue #2351: `__traceback__` is stored as a deferred placeholder
            // when an exception is caught; materialise it on first read and
            // write the real chain back so later reads (and the `__dict__`
            // proxy) see the same object.
            if name == "__traceback__"
                && let Some(real) = self.materialize_deferred_traceback(&value)
            {
                instance
                    .borrow_mut()
                    .attrs
                    .insert("__traceback__", real.clone());
                return Ok(real);
            }
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
                && let Some(v) =
                    pyrust_builtins::numeric_attrs_descriptor::complex_attr(&backing, name)
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
                // Carries the qualified function name, e.g. "list.append".
                // Used for both type-check enforcement (#1500) and binding
                // decision (has dot + name matches → bind; no dot → don't bind).
                BuiltinFunction(&'static str),
                ClassMethodAny(Value),
                StaticMethodAny(Value),
                Other,
            }
            let tag = match value.kind() {
                ValueKind::UserFunction(f) => AttrKind::UserFunction(Rc::clone(f)),
                ValueKind::BuiltinFunction(fn_name) => AttrKind::BuiltinFunction(fn_name),
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
                // Global builtins (no dot in name, e.g. "len") are never bound —
                // they lack __get__ (#1477/#1495).  Type methods (e.g.
                // "list.append") are bound after verifying the instance's class is
                // a subclass of the defining type (CPython method_descriptor.__get__
                // raises TypeError on mismatch, #1500).
                AttrKind::BuiltinFunction(fn_name) => {
                    if let Some(dot) = fn_name.rfind('.') {
                        let defining_type = &fn_name[..dot];
                        let method_name = &fn_name[dot + 1..];
                        if let Some(defining_class) = primitive_class_by_name(defining_type)
                            && !class_is_subclass_of(&class, &defining_class) {
                                let instance_type = class.borrow().name.clone();
                                return Err(pyrust_core::type_err!("descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
                                        method_name, defining_type, instance_type));
                            }
                        if method_name == name {
                            Ok(pyrust_builtins::bound_method::bound_method(
                                name.to_string(),
                                Value::py_instance(Rc::clone(&instance)),
                            ))
                        } else {
                            Ok(value)
                        }
                    } else {
                        Ok(value)
                    }
                }
                // classmethod/staticmethod wrapping a non-function: apply
                // descriptor protocol same as class-level access (§3.3.2).
                // classmethod returns the wrapped value (best approximation;
                // CPython returns a method object for non-callables but that
                // requires a new Value variant); staticmethod returns directly.
                AttrKind::ClassMethodAny(w) => Ok(w),
                AttrKind::StaticMethodAny(w) => Ok(w),
                AttrKind::Other => Ok(value),
            };
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
        if is_typevar_class(&class)
            && let Some(msg) = typevar_readonly_attr_error(name)
        {
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
            )? {
                return result;
            }
        // Issue #1198: bare `object()` instances have no __dict__.
        if Rc::ptr_eq(&class, &object_class_singleton()) {
            return Err(pyrust_core::py_err!("AttributeError", "'object' object has no attribute '{name}'"));
        }
        // PEP 3134 / issue #1066: validate exception-slot types.
        if is_exception_class(&class) {
            match name {
                "__cause__" | "__context__" => {
                    let ok = match value.kind() {
                        ValueKind::None => true,
                        ValueKind::PyInstance(inst) => is_exception_class(&inst.borrow().class),
                        _ => false,
                    };
                    if !ok {
                        return Err(pyrust_core::type_err!("exception {} must be None or derive from BaseException",
                                if name == "__cause__" { "cause" } else { "context" }));
                    }
                }
                "__suppress_context__"
                    if !matches!(value.kind(), ValueKind::Bool(_)) => {
                        return Err(pyrust_core::type_err!("attribute value type must be bool"));
                    }
                "__traceback__" => {
                    let ok = match value.kind() {
                        ValueKind::None => true,
                        ValueKind::BuiltinObject { ops, .. } => {
                            ops.type_name() == pyrust_builtins::traceback::TYPE_NAME
                        }
                        _ => false,
                    };
                    if !ok {
                        return Err(pyrust_core::type_err!("__traceback__ must be a traceback or None"));
                    }
                }
                _ => {}
            }
        }
        // Issue #1957: `obj.__class__ = NewType` re-types the instance rather
        // than storing a literal attribute.  CPython requires the value to be
        // a class; anything else raises TypeError.  For pyrust's attrs-map
        // instance model, retyping is just pointing the instance at the new
        // class (the layout is always compatible).  `__class__` is a type-level
        // slot, not a per-instance attribute, so this is handled *before*
        // `__slots__` enforcement — a slotted instance can still be re-typed.
        if name == "__class__" {
            return self.retype_instance(&instance, value);
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
                return Err(pyrust_core::py_err!("AttributeError", "'{class_name}' object has no attribute '{name}'"));
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
    fn retype_instance(
        &mut self,
        instance: &Rc<RefCell<PyInstance>>,
        value: Value,
    ) -> Result<()> {
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
        instance.borrow_mut().class = new_class;
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
        if is_typevar_class(&class)
            && let Some(msg) = typevar_readonly_attr_error(name)
        {
            return Err(pyrust_core::py_err!("AttributeError", "{msg}"));
        }
        if let Some(class_val) = lookup_class_attr(&class, name)
            && let Some(result) = call_descriptor_delete(
                self,
                &class_val,
                Value::py_instance(Rc::clone(&instance)),
                name,
            )? {
                return result;
            }
        // CPython 3.12: SyntaxError's structured slots reset to None on
        // delete rather than removing the attribute (issue #1588).
        if class_chain_contains_name(&class, "SyntaxError")
            && matches!(
                name,
                "msg" | "filename" | "lineno" | "offset" | "text" | "end_lineno" | "end_offset"
            )
        {
            instance
                .borrow_mut()
                .attrs
                .insert(name, Value::none());
            return Ok(());
        }
        // CPython 3.12: BaseException.args is a C-level member descriptor
        // with no tp_delete slot; any deletion attempt raises TypeError.
        if name == "args" && class_chain_contains_name(&class, "BaseException") {
            return Err(pyrust_core::type_err!("args may not be deleted"));
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

    pub(crate) fn assign_attr(&mut self, target: Value, name: &str, value: Value) -> Result<()> {
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
                        "__module__" => Err(pyrust_core::py_err!("AttributeError", "'method_descriptor' object has no attribute '__module__'")),
                        "__name__" => Err(pyrust_core::py_err!("AttributeError", "readonly attribute")),
                        "__qualname__" | "__doc__" => Err(pyrust_core::py_err!("AttributeError", "attribute '{name}' of 'method_descriptor' objects is not writable")),
                        _ => Err(pyrust_core::py_err!("AttributeError", "'method_descriptor' object has no attribute '{name}'")),
                    }
                } else {
                    // builtin_function_or_method path
                    match name {
                        "__module__" => {
                            // CPython allows this write; pyrust lacks per-instance storage.
                            // Raise AttributeError so the error class is correct until
                            // mutable __module__ storage is added to BuiltinFunction.
                            Err(pyrust_core::py_err!("AttributeError", "attribute '__module__' of 'builtin_function_or_method' \
                                     objects is not writable"))
                        }
                        "__name__" | "__qualname__" | "__doc__" => Err(pyrust_core::py_err!("AttributeError", "attribute '{name}' of 'builtin_function_or_method' \
                                 objects is not writable")),
                        _ => Err(pyrust_core::py_err!("AttributeError", "'builtin_function_or_method' object has no attribute '{name}'")),
                    }
                }
            }
            ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
                // CPython raises AttributeError (not a generic RuntimeError) when
                // you try to set any attribute on a bound method object.
                Err(pyrust_core::py_err!("AttributeError", "'method' object has no attribute '{name}'"))
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
                module.borrow_mut().attrs.insert(name.to_string(), value);
                Ok(())
            }
            ValueKind::Range { .. } | ValueKind::BigRange { .. } => {
                // Range objects are immutable.  start/stop/step are read-only
                // C-level slots in CPython; the error message is "readonly attribute".
                // Any other attribute gives "has no attribute" (issue #1807).
                match name {
                    "start" | "stop" | "step" => Err(pyrust_core::py_err!("AttributeError", "readonly attribute")),
                    _ => Err(pyrust_core::py_err!("AttributeError", "'range' object has no attribute '{name}'")),
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
                            return Err(pyrust_core::type_err!("{name} must be set to a string object"));
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
                        Err(pyrust_core::py_err!("AttributeError", "attribute '{name}' of 'generator' objects is not writable"))
                    }
                    _ => Err(pyrust_core::py_err!("AttributeError", "'generator' object has no attribute '{name}'")),
                }
            }
            _ => {
                let type_name = pyrust_core::builtin_type_name(&target);
                Err(pyrust_core::py_err!("AttributeError", "'{type_name}' object has no attribute '{name}'"))
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
            if is_typevar_class(&class)
                && let Some(msg) = typevar_readonly_attr_error(name)
            {
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
                let is_object_default = matches!(
                    setattr_val.kind(),
                    ValueKind::BuiltinFunction(n) if n == "object.__setattr__"
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
                && let Some(result) =
                    call_descriptor_set(self, &class_val, Value::py_instance(Rc::clone(instance)), value.clone(), name)?
                {
                    return result;
                }
            // Issue #1198: bare `object()` instances have no __dict__ in
            // CPython.  Only the object singleton itself is blocked; any
            // user-defined class (even `class Foo(object): pass`) gets its
            // own PyClass Rc and is not ptr_eq to the singleton.
            if Rc::ptr_eq(&class, &object_class_singleton()) {
                return Err(pyrust_core::py_err!("AttributeError", "'object' object has no attribute '{name}'"));
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
                            return Err(pyrust_core::type_err!("exception {} must be None or derive from BaseException",
                                    if name == "__cause__" { "cause" } else { "context" }));
                        }
                    }
                    "__suppress_context__"
                        if !matches!(value.kind(), ValueKind::Bool(_)) => {
                            return Err(pyrust_core::type_err!("attribute value type must be bool"));
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
                            return Err(pyrust_core::type_err!("__traceback__ must be a traceback or None"));
                        }
                    }
                    _ => {}
                }
            }
            // Issue #1957: `obj.__class__ = NewType` re-types the instance
            // rather than storing a literal attribute (see `retype_instance`).
            // `__class__` is a type-level slot, not a per-instance attribute,
            // so it is handled *before* `__slots__` enforcement — a slotted
            // instance can still be re-typed.
            if name == "__class__" {
                return self.retype_instance(instance, value);
            }
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
                    return Err(pyrust_core::py_err!("AttributeError", "'{class_name}' object has no attribute '{name}'"));
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
            // Primitive class singletons are shared across every
            // `Interpreter` on the same thread (per-thread
            // `PRIMITIVE_CLASSES` thread_local), so mutating their
            // attrs would leak state across runs.  Match CPython,
            // which raises TypeError on `int.x = 1`.  Copilot
            // review on #463.
            if crate::interpreter::is_primitive_class(class) {
                let n = class.borrow().name.clone();
                return Err(pyrust_core::type_err!("cannot set '{name}' attribute of immutable type '{n}'"));
            }
            // __dict__ is a read-only descriptor on type objects — CPython
            // raises AttributeError on direct assignment.
            if name == "__dict__" {
                return Err(pyrust_core::py_err!("AttributeError", "attribute '__dict__' of 'type' objects is not writable"));
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
                        let type_name =
                            pyrust_core::builtin_type_name(&value).into_owned();
                        return Err(pyrust_core::type_err!("can only assign string to {}.__qualname__, not '{}'",
                                class.borrow().name,
                                type_name,));
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
                    && matches!(
                        value.kind(),
                        ValueKind::BuiltinFunction("object.__new__")
                    )
                    && !class_chain_new_slot_wrapped(class)
                    && lookup_class_attr(class, "__new__").is_none_or(|cur| {
                        matches!(
                            cur.kind(),
                            ValueKind::BuiltinFunction("object.__new__")
                        )
                    });
                let mut cls = class.borrow_mut();
                cls.attrs.insert(name.to_string(), value);
                let v = cls.mutation_version.get().wrapping_add(1);
                cls.mutation_version.set(v);
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
                        None => Err(pyrust_core::type_err!("{name} must be set to a string object")),
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
                        Err(pyrust_core::type_err!("__dict__ must be set to a dictionary, not a '{type_name}'"))
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
                            Value::dict(PyDict::default());
                        Ok(())
                    } else {
                        Err(pyrust_core::type_err!("__annotations__ must be set to a dict object"))
                    }
                }
                // CPython validates these slots and rejects arbitrary values.
                // They are not yet implemented as real fields in pyrust, so
                // validate the type and silently succeed for accepted values
                // (pyrust is already in the "unset" state CPython would be
                // in after the assignment).
                "__code__" => Err(pyrust_core::type_err!("__code__ must be set to a code object")),
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
                        Err(pyrust_core::type_err!("__defaults__ must be set to a tuple object"))
                    }
                }
                "__kwdefaults__" => {
                    // #2395: CPython accepts None or a dict; anything else →
                    // TypeError.  Store the per-object override (see `__defaults__`).
                    if value.is_none() || matches!(value.kind(), ValueKind::Dict(_)) {
                        func.set_kwdefaults_override(value);
                        Ok(())
                    } else {
                        Err(pyrust_core::type_err!("__kwdefaults__ must be set to a dict object"))
                    }
                }
                "__globals__" | "__closure__" => Err(pyrust_core::py_err!("AttributeError", "readonly attribute")),
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


    pub(crate) fn delete_attr(&mut self, target: Value, name: &str) -> Result<()> {
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
                    "__name__" | "__qualname__" => Err(pyrust_core::type_err!("{name} must be set to a string object")),
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
                        *func.annotations.borrow_mut() =
                            Value::dict(PyDict::default());
                        Ok(())
                    }
                    // CPython-matched behaviour for validated-but-unimplemented slots.
                    "__code__" => Err(pyrust_core::type_err!("__code__ must be set to a code object")),
                    "__globals__" | "__closure__" => Err(pyrust_core::py_err!("AttributeError", "readonly attribute")),
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
                                UserFunctionKind::StaticMethod => "staticmethod",
                                UserFunctionKind::ClassMethod => "classmethod",
                                _ => "function",
                            };
                            Err(pyrust_core::py_err!("AttributeError", "'{type_name}' object has no attribute '{name}'"))
                        }
                    }
                }
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                self.delete_attr_kind_class(&class, name)
            }
            ValueKind::BuiltinFunction(func_name) => {
                // Mirror the assign_attr logic: method_descriptors and
                // builtin_function_or_method both raise AttributeError, matching
                // CPython exactly.  (Mutable __module__ storage is a follow-up.)
                if func_name.contains('.') {
                    match name {
                        "__module__" => Err(pyrust_core::py_err!("AttributeError", "'method_descriptor' object has no attribute '__module__'")),
                        "__name__" => Err(pyrust_core::py_err!("AttributeError", "readonly attribute")),
                        "__qualname__" | "__doc__" => Err(pyrust_core::py_err!("AttributeError", "attribute '{name}' of 'method_descriptor' objects is not writable")),
                        _ => Err(pyrust_core::py_err!("AttributeError", "'method_descriptor' object has no attribute '{name}'")),
                    }
                } else {
                    match name {
                        "__module__" | "__name__" | "__qualname__" | "__doc__" => {
                            Err(pyrust_core::py_err!("AttributeError", "attribute '{name}' of 'builtin_function_or_method' \
                                     objects is not writable"))
                        }
                        _ => Err(pyrust_core::py_err!("AttributeError", "'builtin_function_or_method' object has no attribute '{name}'")),
                    }
                }
            }
            ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
                // CPython raises AttributeError when deleting any attribute on a
                // bound method object.
                Err(pyrust_core::py_err!("AttributeError", "'method' object has no attribute '{name}'"))
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
                                .attrs
                                .insert(name.to_string(), Value::unset());
                        } else {
                            module.borrow_mut().attrs.remove(name);
                        }
                        return Ok(());
                    }
                    None if matches!(
                        name,
                        "__name__" | "__package__" | "__loader__" | "__spec__" | "__doc__"
                    ) => {
                        // Synthetic dunders live only in get_attr, not in attrs.
                        // CPython 3.12 allows deleting them (they exist in the
                        // real module __dict__).  Write a Value::unset() tombstone
                        // so get_attr stops synthesising them on future reads.
                        module
                            .borrow_mut()
                            .attrs
                            .insert(name.to_string(), Value::unset());
                        return Ok(());
                    }
                    _ => {}
                }
                Err(pyrust_core::py_err!("AttributeError", "'module' object has no attribute '{name}'"))
            }
            ValueKind::Generator(_) => {
                // CPython 3.12 symmetry with assign_attr: deleting __name__ or
                // __qualname__ raises the same TypeError as assigning a non-string.
                // The read-only gi_* attrs raise AttributeError "not writable".
                // Anything else raises AttributeError "has no attribute".
                match name {
                    "__name__" | "__qualname__" => Err(pyrust_core::type_err!("{name} must be set to a string object")),
                    "gi_running" | "gi_yieldfrom" | "gi_frame" | "gi_code" => {
                        Err(pyrust_core::py_err!("AttributeError", "attribute '{name}' of 'generator' objects is not writable"))
                    }
                    _ => Err(pyrust_core::py_err!("AttributeError", "'generator' object has no attribute '{name}'")),
                }
            }
            _ => {
                let type_name = pyrust_core::builtin_type_name(&target);
                Err(pyrust_core::py_err!("AttributeError", "'{type_name}' object has no attribute '{name}'"))
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
            if is_typevar_class(&class)
                && let Some(msg) = typevar_readonly_attr_error(name)
            {
                return Err(pyrust_core::py_err!("AttributeError", "{msg}"));
            }
            // Check for `__delattr__` first — symmetric with __setattr__
            // in assign_attr (issue #1174).  Skip only the
            // `object.__delattr__` builtin sentinel.
            if let Some(delattr_val) = lookup_class_attr(&class, "__delattr__") {
                let is_object_default = matches!(
                    delattr_val.kind(),
                    ValueKind::BuiltinFunction(n) if n == "object.__delattr__"
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
                && let Some(result) =
                    call_descriptor_delete(self, &class_val, Value::py_instance(Rc::clone(instance)), name)?
                {
                    return result;
                }
            // CPython 3.12: SyntaxError's structured slots are C-level
            // member descriptors that reset to None on delete rather than
            // removing the attribute entirely (issue #1588).  Mirror that
            // here by storing None instead of removing the key.
            if class_chain_contains_name(&class, "SyntaxError")
                && matches!(
                    name,
                    "msg"
                        | "filename"
                        | "lineno"
                        | "offset"
                        | "text"
                        | "end_lineno"
                        | "end_offset"
                )
            {
                instance
                    .borrow_mut()
                    .attrs
                    .insert(name, Value::none());
                return Ok(());
            }
            // CPython 3.12: BaseException.args is a C-level member descriptor
            // with no tp_delete slot; any deletion attempt raises TypeError.
            if name == "args" && class_chain_contains_name(&class, "BaseException") {
                return Err(pyrust_core::type_err!("args may not be deleted"));
            }
            // `shift_remove` keeps the remaining entries in their
            // original insertion order so `vars(obj)` after `del obj.x`
            // still matches CPython's stable ordering contract.
            // CPython raises AttributeError when the attribute is absent.
            if instance.borrow_mut().attrs.shift_remove(name).is_none() {
                let class_name = instance.borrow().class.borrow().name.clone();
                return Err(pyrust_core::py_err!("AttributeError", "'{class_name}' object has no attribute '{name}'"));
            }
            Ok(())
    }

    /// `del Cls.name` for a `PyClass` target. Split out of `delete_attr`.
    fn delete_attr_kind_class(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        name: &str,
    ) -> Result<()> {
            // __dict__ is a read-only descriptor on type objects — CPython
            // raises AttributeError on `del C.__dict__`.
            if name == "__dict__" {
                return Err(pyrust_core::py_err!("AttributeError", "attribute '__dict__' of 'type' objects is not writable"));
            }
            // Issue #553: __qualname__ is a type-level descriptor on `type`
            // in CPython — you cannot delete it.  CPython raises TypeError.
            if name == "__qualname__" {
                let n = class.borrow().name.clone();
                return Err(pyrust_core::type_err!("cannot delete '__qualname__' attribute of immutable type '{n}'"));
            }
            // Issue #737: `del Cls.__annotations__` must raise
            // `AttributeError` when no annotations dict has been
            // materialised yet — matching CPython's descriptor, which
            // refuses to delete a slot that was never written.
            if name == "__annotations__"
                && !class.borrow().attrs.contains_key("__annotations__")
            {
                return Err(pyrust_core::py_err!("AttributeError", "__annotations__"));
            }
            // CPython raises AttributeError when the attribute is absent.
            {
                let mut cls = class.borrow_mut();
                if cls.attrs.shift_remove(name).is_none() {
                    let class_name = cls.name.clone();
                    return Err(pyrust_core::py_err!("AttributeError", "type object '{class_name}' has no attribute '{name}'"));
                }
                let v = cls.mutation_version.get().wrapping_add(1);
                cls.mutation_version.set(v);
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
            // `collections` post-process (issue #2010): `Counter` and
            // `defaultdict` are `dict` subclasses in CPython
            // (`isinstance(Counter(), dict)`, `Counter.__mro__ == (Counter,
            // dict, object)`).  The `pyrust_module!` macro builds every class
            // with `base: None`, so we re-parent them onto the per-thread
            // `dict` singleton here, where the singleton is reachable.  Their
            // own dunders (`__getitem__`, `__iter__`, …) stay ahead of dict's
            // in the MRO, so behaviour is unchanged; only the subclass
            // relationship (and the `dict()` conversion path that keys off it)
            // turns on.
            if name == "collections"
                && let ValueKind::PyModule(m) = val.kind()
                && let Some(dict_class) = crate::interpreter::primitive_class_by_name("dict")
            {
                for cls_name in ["Counter", "defaultdict"] {
                    let cls = m.borrow().attrs.get(cls_name).cloned();
                    if let Some(cls_val) = cls
                        && let ValueKind::PyClass(cls_rc) = cls_val.kind()
                    {
                        let already = cls_rc.borrow().base.is_some();
                        if !already {
                            cls_rc.borrow_mut().base = Some(Rc::clone(&dict_class));
                            dict_class
                                .borrow()
                                .subclasses
                                .borrow_mut()
                                .push(Rc::downgrade(cls_rc));
                        }
                    }
                }
            }
            // `itertools` post-process (issue #2098): tag every iterator class
            // with `__module__ = "itertools"` so `type(islice(...)).__module__`
            // and the generic instance repr render `<itertools.X object at
            // 0x..>` instead of the `__main__` default.  The macro builds the
            // classes with no `__module__` entry.
            if name == "itertools" {
                crate::builtin_modules::itertools::patch_class_modules(&val);
                // Capture the `chain` class so `type(chain.from_iterable(...))`
                // (a native iterator) reports the same class object as
                // `type(chain(...))` (issue #2370).
                if let ValueKind::PyModule(m) = val.kind()
                    && let Some(chain_val) = m.borrow().attrs.get("chain").cloned()
                    && let ValueKind::PyClass(chain_cls) = chain_val.kind()
                {
                    crate::interpreter::set_itertools_chain_class(Rc::clone(chain_cls));
                }
            }
            // `io` post-process (issue #2008): `closed` is a read-only property
            // in CPython (accessed without parens).  The `pyrust_module!` macro
            // can only register `closed` as a plain method, so wrap each class's
            // `closed` getter in a `property` descriptor here, where `property`
            // is reachable.
            if name == "io"
                && let ValueKind::PyModule(m) = val.kind()
            {
                // Expose `io.UnsupportedOperation` (issue #2008): the exception
                // class is registered in the global exception map but, unlike
                // CPython, was not surfaced as an `io` module attribute — so
                // `except io.UnsupportedOperation` (raised by `fileno()`) could
                // not be caught by name.
                if let Some(exc) = crate::interpreter::build_exc_class_map()
                    .get("io.UnsupportedOperation")
                    .map(Rc::clone)
                {
                    m.borrow_mut().attrs.insert(
                        "UnsupportedOperation".to_string(),
                        Value::py_class(exc),
                    );
                }
                for cls_name in ["StringIO", "BytesIO"] {
                    let cls = m.borrow().attrs.get(cls_name).cloned();
                    if let Some(cls_val) = cls
                        && let ValueKind::PyClass(cls_rc) = cls_val.kind()
                    {
                        let getter = cls_rc.borrow().attrs.get("closed").cloned();
                        if let Some(getter) = getter {
                            let prop = pyrust_builtins::property::property(
                                getter,
                                Value::none(),
                                Value::none(),
                            );
                            cls_rc
                                .borrow_mut()
                                .attrs
                                .insert("closed".to_string(), prop);
                        }
                    }
                }
            }
            self.module_cache.borrow_mut().insert(name.to_string(), val.clone());
            // `collections` Python-source members (issue #1884): inject
            // `namedtuple`, `OrderedDict`, `ChainMap`, `UserDict`,
            // `UserList`, and `UserString` by exec'ing their Python source.
            // Done *after* the module is in `module_cache` so the exec (which
            // resolves builtins like `dict`/`tuple`/`property`) sees a
            // consistent import state.
            if name == "collections"
                && let ValueKind::PyModule(m) = val.kind()
            {
                crate::builtin_modules::collections::inject_python_members(
                    self, m,
                )?;
                // `collections` class repr (issue #2228): tag each public
                // collections class with `__module__ = "collections"` so the
                // type repr renders `<class 'collections.Counter'>` and
                // `Counter.__module__ == "collections"`, matching CPython.
                // The native classes (macro-built) carry no `__module__`; the
                // Python-source classes are exec'd in a private namespace and
                // pick up that namespace's `__name__` instead.  Done after
                // `inject_python_members` so every class exists.  `namedtuple`
                // is deliberately excluded — CPython gives namedtuple-created
                // classes the *caller's* `__module__`, not `collections`.
                for cls_name in [
                    "Counter",
                    "defaultdict",
                    "deque",
                    "OrderedDict",
                    "ChainMap",
                    "UserDict",
                    "UserList",
                    "UserString",
                ] {
                    let cls = m.borrow().attrs.get(cls_name).cloned();
                    if let Some(cls_val) = cls
                        && let ValueKind::PyClass(cls_rc) = cls_val.kind()
                    {
                        cls_rc.borrow_mut().attrs.insert(
                            "__module__".to_string(),
                            Value::string("collections"),
                        );
                    }
                }
            }
            // `asyncio` Python-source members (issue #1039): inject `sleep`
            // and `gather`, defined as real coroutines in `asyncio_py.py`, by
            // exec'ing that source after the native `run` is registered.
            if name == "asyncio"
                && let ValueKind::PyModule(m) = val.kind()
            {
                crate::builtin_modules::asyncio::inject_python_members(self, m)?;
            }
            // `string` Python-source members (issue #2515): inject
            // `capwords`, `Template`, and `Formatter`, defined in
            // `string_py.py`, after the native constants are registered.
            if name == "string"
                && let ValueKind::PyModule(m) = val.kind()
            {
                crate::builtin_modules::string::inject_python_members(self, m)?;
            }
            // `operator` Python-source members (issue #2514): the whole module
            // is defined in `operator_py.py` (itemgetter / attrgetter /
            // methodcaller plus the operator-wrapper functions), exec'd here
            // since the native body is empty.
            if name == "operator"
                && let ValueKind::PyModule(m) = val.kind()
            {
                crate::builtin_modules::operator::inject_python_members(self, m)?;
            }
            // `typing` Python-source members (issue #2516): inject
            // `get_type_hints`, `get_origin`, `get_args`, `runtime_checkable`,
            // `reveal_type`, the special-form markers (`Self`, `Never`,
            // `Annotated`, …), `ParamSpec`, `TypeVarTuple`, and the
            // `NamedTuple` helper functions by exec'ing `typing_py.py`.
            if name == "typing"
                && let ValueKind::PyModule(m) = val.kind()
            {
                crate::builtin_modules::typing::inject_python_members(self, m)?;
            }
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
                // The module's own path: tags its code objects' `co_filename` and
                // the traceback `File "..."` line for frames raised inside it,
                // instead of inheriting the importing script's path (#2438).
                let module_filename = full_path.to_string_lossy().into_owned();
                // Parse with per-statement line numbers and per-token positions so
                // frames raised inside the imported module carry the correct
                // `tb_lineno` and PEP 657 caret anchors (#2438); the import path
                // previously parsed without line info, so module frames printed no
                // line number / source line.
                let (tokens, line_nos, cols, cols_end) =
                    Lexer::new(&src)?.into_tokens_with_pos();
                let (program, linenos) =
                    Parser::new_with_pos(tokens, line_nos, cols, cols_end)
                        .parse_program_with_linenos()?;
                // Subinterpreter shares the same module_cache so results are visible to parent
                let mut sub = Interpreter {
                    script_dir: self.script_dir.clone(),
                    script_filename: Some(std::sync::Arc::from(module_filename.as_str())),
                    module_cache: Rc::clone(&self.module_cache),
                    ..Default::default()
                };
                // call_depth is thread_local — sub-interpreter automatically shares the same counter
                sub.exec_program_with_linenos(&program, &linenos, &src, false)?;
                // Harvest all top-level bindings as module attrs
                let attrs: HashMap<String, Value> = sub
                    .env
                    .borrow()
                    .values
                    .iter()
                    .filter(|(k, _)| {
                        !matches!(
                            *k,
                            "Exception"
                                | "RuntimeError"
                                | "TypeError"
                                | "ValueError"
                                | "AssertionError"
                        )
                    })
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect();
                let module = Value::py_module(Rc::new(RefCell::new(PyModule {
                    name: name.to_string(),
                    attrs,
                })));
                self.module_cache.borrow_mut().insert(name.to_string(), module.clone());
                return Ok(module);
            }
        }
        // Relative imports (module name starts with '.') cannot be resolved in
        // pyrust's package-less runtime.  CPython 3.12 raises ImportError (not
        // ModuleNotFoundError) with a specific message in this case.
        if name.starts_with('.') {
            return Err(PyError::import_error(
                "ImportError",
                "attempted relative import with no known parent package".to_string(),
                None,
            ));
        }
        Err(PyError::import_error(
            "ModuleNotFoundError",
            format!("No module named '{name}'"),
            Some(name.to_string()),
        ))
    }

    fn assign_name(&self, name: &str, value: Value) {
        let (is_global, is_nonlocal) = {
            let env = self.env.borrow();
            (env.global_names.contains(name), env.nonlocal_names.contains(name))
        };
        if is_global {
            // Write to the module env HashMap so LoadGlobal / post-run
            // inspection can find the new value.
            module_env(&self.env).borrow_mut().values.insert(name, value.clone());
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
                    PyKey::str_from(name),
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
                && let Some(&slot) = script_view.local_index.get(name) {
                    let slot = slot as usize;
                    if slot < script_view.regs_len {
                        unsafe {
                            *script_view.regs_ptr.add(slot).as_mut() = value;
                        }
                    }
                }
            return;
        }
        if is_nonlocal
            && let Some(env) = find_enclosing_local_env_for_name(&self.env, name) {
                env_assign_local(&env, name, value);
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
                    PyKey::str_from(name),
                    value.clone(),
                );
            }
        }
        env_assign_local(&self.env, name, value);
    }

    /// Resolve `name` against the active scope, reporting an unbound binding as a
    /// plain local (`UnboundLocalError`).  Used by the interpreter unit tests'
    /// `lookup_name` assertions; the VM read opcodes call
    /// [`lookup_name_inner`](Self::lookup_name_inner) directly so they can flag a
    /// captured free variable (issue #2340).
    #[cfg_attr(not(test), allow(dead_code))]
    fn lookup_name(&self, name: &str) -> Result<Option<Value>> {
        self.lookup_name_inner(name, false)
    }

    /// Name resolution for `Insn::LoadGlobal` / `Insn::LoadCell`.
    ///
    /// `is_free` selects the CPython 3.12 error class for an unbound binding
    /// found via the plain env walk (issue #2340): when set, the name is a
    /// captured **free** variable of the currently-executing function (not one
    /// of its own locals / cell vars), so an unbound binding is reported as a
    /// `NameError` ("cannot access free variable ... in enclosing scope")
    /// rather than an `UnboundLocalError`.  A `nonlocal` read always binds to an
    /// enclosing scope and so is treated as free regardless of `is_free`.
    pub(crate) fn lookup_name_inner(&self, name: &str, is_free: bool) -> Result<Option<Value>> {
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
        if is_free {
            return lookup_name_in_env_as_free(&self.env, name);
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
                    coerce_numeric(value)
                } else {
                    Value::py_instance(Rc::clone(&inst_rc))
                };
                let result = invoke_class_method(self, method_val, self_val, &[])?;
                return match result.kind() {
                    ValueKind::Bool(b) => Ok(b),
                    _ => Err(pyrust_core::type_err!("__bool__ should return bool, returned {}",
                            pyrust_core::builtin_type_name(&result),)),
                };
            }
            // Fall back to __len__.
            if let Some(method_val) = lookup_class_attr(&class, "__len__") {
                let self_val = if matches!(method_val.kind(), ValueKind::BuiltinFunction(_)) {
                    coerce_numeric(value)
                } else {
                    Value::py_instance(Rc::clone(&inst_rc))
                };
                let result = invoke_class_method(self, method_val, self_val, &[])?;
                return match result.kind() {
                    ValueKind::Int(n) if n >= 0 => Ok(n != 0),
                    ValueKind::Int(_) => Err(pyrust_core::value_err!("__len__() should return >= 0")),
                    ValueKind::Bool(b) => Ok(b),
                    ValueKind::BigInt(big) => match big.sign() {
                        PyBigIntSign::Minus => Err(pyrust_core::value_err!("__len__() should return >= 0")),
                        PyBigIntSign::NoSign => Ok(false),
                        PyBigIntSign::Plus => {
                            if big.to_usize().is_none() {
                                Err(pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer"))
                            } else {
                                Ok(true)
                            }
                        }
                    },
                    _ => Err(pyrust_core::type_err!("'{}' object cannot be interpreted as an integer",
                            pyrust_core::builtin_type_name(&result),)),
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

/// Issue #2479: wrap an unbound method (`OrderedDict.clear`,
/// `OrderedDict.move_to_end`) accessed on a *proper subclass* of a builtin
/// primitive in an `unbound_method_descriptor`, so the call site can reject an
/// unrelated receiver (`OrderedDict.clear({1: 2})`) with CPython's
/// `descriptor '<m>' for '<owner>' objects doesn't apply to a '<X>' object`
/// TypeError.
///
/// Returns `value` unchanged (no wrapping) when:
///   * `class` *is* the primitive itself (`dict.clear` must accept any dict),
///   * `class` is not in `OrderedDict`'s chain — a user Python `dict` subclass
///     does not re-own inherited C descriptors, and its own `def`s stay plain
///     `function`s, so neither enforces a receiver-class guard,
///   * `name` is not one of the non-dunder methods `OrderedDict` re-owns
///     (`OrderedDict.<m>.__objclass__ is OrderedDict`).
fn wrap_subclass_unbound_method(
    class: &Rc<RefCell<PyClass>>,
    name: &str,
    value: Value,
) -> Value {
    // The primitive's own descriptors (`dict.clear`) accept any receiver of the
    // primitive kind — never wrap them.
    if crate::interpreter::is_primitive_class(class) {
        return value;
    }
    // Issue #2479: only stdlib types that CPython implements in C re-expose
    // inherited primitive methods as *their own* `method_descriptor`s (with the
    // subclass as `__objclass__`), and only those enforce a receiver-class
    // guard.  pyrust models `OrderedDict` in Python, so without this it inherits
    // the shared `dict` sentinels and never enforces.  A *user* Python subclass
    // of `dict` does NOT re-own these — `class MyDict(dict): ...`'s `clear` is
    // still `dict.clear` (objclass `dict`), and its own `def shout(self)` is a
    // plain `function`, neither of which enforces — so we must NOT wrap those.
    //
    // The enforced set is exactly the non-dunder methods `OrderedDict` owns in
    // CPython 3.12 (`OrderedDict.<m>.__objclass__ is OrderedDict`):
    //   clear, pop, popitem, update, setdefault, copy, keys, values, items,
    //   move_to_end.
    // Methods OrderedDict merely inherits (`get`, `__contains__`, …) keep
    // objclass `dict` and accept any dict receiver, so they are excluded.
    //
    // Match the *real* `collections.OrderedDict` (name + module), not any class
    // merely named `OrderedDict`: a user `class OrderedDict(dict)` is a plain
    // Python subclass whose `clear` stays `dict.clear`/`function` in CPython and
    // must NOT be wrapped.
    if ordered_dict_owner(class).is_none() {
        return value;
    }
    let owns = matches!(
        name,
        "clear"
            | "pop"
            | "popitem"
            | "update"
            | "setdefault"
            | "copy"
            | "keys"
            | "values"
            | "items"
            | "move_to_end"
    );
    if !owns {
        return value;
    }
    // Determine the *owning* class for the descriptor error wording.  CPython
    // attributes the descriptor to the type that owns it — e.g. `OD2.clear`
    // reports `collections.OrderedDict`, not `OD2`.  The owner was already
    // located by the guard above (the real `collections.OrderedDict` layer).
    let owner = match ordered_dict_owner(class) {
        Some(o) => o,
        None => return value,
    };
    pyrust_builtins::unbound_method_descriptor::unbound_method_descriptor(
        Value::py_class(owner),
        name.to_string(),
        value,
    )
}

/// Walk `class`'s full base graph (primary `base` *and* `extra_bases`, mirroring
/// [`class_chain_contains_name`]) and return the `collections.OrderedDict` class
/// itself — the descriptor-owning layer reported in CPython's `descriptor '<m>'
/// for '<owner>' objects ...` wording.  For `OD2(OrderedDict)`,
/// `OD3(Mixin, OrderedDict)`, and `OrderedDict` alike this is `OrderedDict`, so
/// the wording stays `collections.OrderedDict` regardless of where on the base
/// graph the `OrderedDict` layer sits.
///
/// Matches on name **and** `__module__ == "collections"` so a user `class
/// OrderedDict(dict)` (a plain Python subclass that CPython does *not* model as
/// a C type re-owning inherited descriptors) is not mistaken for the stdlib
/// type.  Returns `None` when no real `collections.OrderedDict` layer is found.
fn ordered_dict_owner(class: &Rc<RefCell<PyClass>>) -> Option<Rc<RefCell<PyClass>>> {
    fn is_collections_ordered_dict(class: &Rc<RefCell<PyClass>>) -> bool {
        let borrowed = class.borrow();
        borrowed.name == "OrderedDict"
            && matches!(
                borrowed.attrs.get("__module__").map(|m| m.kind()),
                Some(ValueKind::Str(s)) if s == "collections"
            )
    }
    fn find(class: &Rc<RefCell<PyClass>>) -> Option<Rc<RefCell<PyClass>>> {
        if is_collections_ordered_dict(class) {
            return Some(Rc::clone(class));
        }
        let borrowed = class.borrow();
        if let Some(base) = &borrowed.base
            && let Some(found) = find(base)
        {
            return Some(found);
        }
        borrowed.extra_bases.iter().find_map(find)
    }
    find(class)
}

/// CPython's `tp_name`-style display name for a class used in descriptor error
/// messages: `<module>.<qualname>`, dropping the module prefix when it is
/// `builtins` or absent (issue #2479).  `OrderedDict` → `collections.OrderedDict`.
pub(crate) fn class_descriptor_display_name(class: &Rc<RefCell<PyClass>>) -> String {
    let borrowed = class.borrow();
    let qualname = borrowed.qualname.clone();
    let module = borrowed
        .attrs
        .get("__module__")
        .and_then(|m| match m.kind() {
            ValueKind::Str(s) => Some(s.to_string()),
            _ => None,
        });
    match module {
        Some(m) if m != "builtins" && !m.is_empty() => format!("{m}.{qualname}"),
        _ => qualname,
    }
}

/// Issue #2433: the interned `"<type>.<cmp_dunder>"` sentinel for a primitive's
/// rich-comparison slot.  A `match` over `&'static str` literals so the result
/// stays `'static` without per-call allocation (unlike a `format!` + leak).
fn cmp_dunder_sentinel(type_name: &str, dunder: &str) -> &'static str {
    macro_rules! arms {
        ($($t:literal),+ $(,)?) => {
            match (type_name, dunder) {
                $(
                    ($t, "__eq__") => concat!($t, ".__eq__"),
                    ($t, "__ne__") => concat!($t, ".__ne__"),
                    ($t, "__lt__") => concat!($t, ".__lt__"),
                    ($t, "__le__") => concat!($t, ".__le__"),
                    ($t, "__gt__") => concat!($t, ".__gt__"),
                    ($t, "__ge__") => concat!($t, ".__ge__"),
                )+
                _ => unreachable!("cmp_dunder_sentinel: ({type_name}, {dunder})"),
            }
        };
    }
    arms!(
        "list", "tuple", "dict", "set", "frozenset", "bytes", "bytearray", "float", "complex",
    )
}

/// Issue #2276: for a primitive type (or a subclass of one) that *overrides* an
/// object-level dunder in CPython, return the `'static` type-qualified sentinel
/// name (`"str.__hash__"`, `"int.__format__"`, …) so an unbound `T.__X__` access
/// is attributed to the called type rather than `object`.  Returns `None` for a
/// type that inherits the dunder straight from `object`.
///
/// Ownership table verified against python3.12
/// (`[c.__name__ for c in T.__mro__ if '__X__' in c.__dict__]`):
///   * `__hash__` / `__repr__`: every scalar/sequence primitive owns its slot
///     (list/dict/set/bytearray set `__hash__` to `None`, so they are excluded
///     from `__hash__`).
///   * `__str__`: only `str` and `bytes`.
///   * `__format__`: only `int`, `str`, `float`.
///
/// The nearest primitive in the class chain wins (a `class S(str)` subclass is
/// attributed to `str`; `bool` chains to `int`).
fn primitive_owned_object_dunder(
    class: &Rc<RefCell<PyClass>>,
    name: &str,
) -> Option<&'static str> {
    // Issue #2433: every primitive type *owns* the six rich-comparison slots in
    // CPython (`[c for c in list.__mro__ if '__eq__' in c.__dict__][0] is list`),
    // so `list.__eq__` is `<slot wrapper '__eq__' of 'list' objects>`, not
    // `'object'`.  Synthesise a type-qualified sentinel for the most-derived
    // primitive in the class chain.  `int`/`str` already register these as real
    // type attrs (their slot table marks them `SLOT_ATTR`), so they never reach
    // here; this covers list/tuple/dict/set/frozenset/bytes/bytearray/float/
    // complex, whose comparison slots are protocol-only.  `bool` inherits int's
    // comparisons (`bool.__eq__` owner is `int`), so it is excluded.
    if matches!(
        name,
        "__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__"
    ) {
        const CMP_OWNERS: &[&str] = &[
            "list",
            "tuple",
            "dict",
            "set",
            "frozenset",
            "bytes",
            "bytearray",
            "float",
            "complex",
        ];
        for type_name in CMP_OWNERS {
            if class_chain_contains_name(class, type_name) {
                return Some(cmp_dunder_sentinel(type_name, name));
            }
        }
        return None;
    }
    // Walk the primitive bases most-derived first; `bool` before `int` so a
    // bare `bool` attributes `__format__`/`__hash__` to `int` (CPython:
    // `bool.__format__` is inherited from `int`).
    const OWNERS: &[(&str, &[&str])] = &[
        ("bool", &["__repr__"]),
        ("int", &["__hash__", "__repr__", "__format__"]),
        ("float", &["__hash__", "__repr__", "__format__"]),
        ("complex", &["__hash__", "__repr__"]),
        ("str", &["__hash__", "__repr__", "__str__", "__format__"]),
        ("bytes", &["__hash__", "__repr__", "__str__"]),
        ("tuple", &["__hash__", "__repr__"]),
        ("frozenset", &["__hash__", "__repr__"]),
        ("list", &["__repr__"]),
        ("dict", &["__repr__"]),
        ("set", &["__repr__"]),
    ];
    for (type_name, dunders) in OWNERS {
        if dunders.contains(&name) && class_chain_contains_name(class, type_name) {
            return Some(match (*type_name, name) {
                ("bool", "__repr__") => "bool.__repr__",
                ("int", "__hash__") => "int.__hash__",
                ("int", "__repr__") => "int.__repr__",
                ("int", "__format__") => "int.__format__",
                ("float", "__hash__") => "float.__hash__",
                ("float", "__repr__") => "float.__repr__",
                ("float", "__format__") => "float.__format__",
                ("complex", "__hash__") => "complex.__hash__",
                ("complex", "__repr__") => "complex.__repr__",
                ("str", "__hash__") => "str.__hash__",
                ("str", "__repr__") => "str.__repr__",
                ("str", "__str__") => "str.__str__",
                ("str", "__format__") => "str.__format__",
                ("bytes", "__hash__") => "bytes.__hash__",
                ("bytes", "__repr__") => "bytes.__repr__",
                ("bytes", "__str__") => "bytes.__str__",
                ("tuple", "__hash__") => "tuple.__hash__",
                ("tuple", "__repr__") => "tuple.__repr__",
                ("frozenset", "__hash__") => "frozenset.__hash__",
                ("frozenset", "__repr__") => "frozenset.__repr__",
                ("list", "__repr__") => "list.__repr__",
                ("dict", "__repr__") => "dict.__repr__",
                ("set", "__repr__") => "set.__repr__",
                _ => return None,
            });
        }
    }
    None
}

/// Returns the attrs `Rc` for `func`, initialising it lazily on first call.
///
/// Lazy init avoids two heap allocations per function definition for the common
/// case where no attrs are ever set.  Interior mutability (`RefCell`) allows
/// initialization through a shared `Rc<UserFunction>`.
fn func_attrs_rc(func: &UserFunction) -> Rc<RefCell<Value>> {
    let mut slot = func.attrs.borrow_mut();
    if slot.is_none() {
        *slot = Some(Rc::new(RefCell::new(Value::dict(PyDict::default()))));
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
        "__name__" => Some(Ok(Value::string(function.effective_name()))),
        "__qualname__" => Some(Ok(Value::string(function.effective_qualname()))),
        "__module__" => Some(Ok(function.module_value())),
        "__doc__" => Some(Ok(function.doc.borrow().clone())),
        "__dict__" => {
            let attrs_rc = func_attrs_rc(function);
            Some(Ok(attrs_rc.borrow().clone()))
        }
        "__annotations__" => Some(Ok(function.annotations_value())),
        "__defaults__" => {
            // #2395: positional defaults tuple (or per-object override), `None`
            // when none exist — CPython's `f.__defaults__` semantics.
            Some(Ok(function.defaults_value()))
        }
        "__kwdefaults__" => {
            // #2395: keyword-only defaults dict (or per-object override), `None`
            // when none exist — CPython's `f.__kwdefaults__` semantics.
            Some(Ok(function.kwdefaults_value()))
        }
        _ => {
            // Arbitrary dynamic attrs delegate to the underlying function.
            // Short-circuit without initialising if no attrs set yet.
            if let Some(rc) = function.attrs.borrow().as_ref().map(Rc::clone)
                && let Some(v) = rc
                    .borrow()
                    .as_dict()
                    .and_then(|d| d.get(&StrKey(name)).cloned())
                {
                    return Some(Ok(v));
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
pub(crate) fn bump_global_env_version(interp: &Interpreter) {
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
    // A `__slots__` member_descriptor is a data descriptor (it defines
    // __get__/__set__/__delete__), so it takes priority over instance storage
    // and reading an unset slot raises AttributeError (issue #2084).
    if pyrust_builtins::member_descriptor::as_member_descriptor(val).is_some() {
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
/// Execute a directly-invoked property descriptor dunder bound via
/// `p.__get__` / `p.__set__` / `p.__delete__`.
///
/// `prop` is the property object; `kind` selects the dunder; `args` are the
/// call-site arguments.  The "no setter/deleter/getter" errors name the
/// property when it recorded one via `__set_name__` (issue #1846), else use
/// CPython's unnamed form (`property of '<owner>' object has no <which>`).
pub(crate) fn dispatch_property_method(
    interp: &mut Interpreter,
    prop: &Value,
    kind: pyrust_builtins::property::PropertyMethodKind,
    args: &[Value],
) -> Result<Value> {
    use pyrust_builtins::property::PropertyMethodKind as K;
    // CPython's slot wrappers validate arity (and raise TypeError) before the
    // missing-accessor AttributeError. __get__ takes obj + optional owner,
    // __set__ takes obj + value, __delete__ takes obj.
    match kind {
        K::Get => {
            if args.is_empty() || args.len() > 2 {
                return Err(pyrust_core::type_err!("expected 1 or 2 arguments, got {}",
                    args.len()));
            }
            // Class-level access (`obj is None`) returns the property itself,
            // but only when an owner is supplied: CPython rejects
            // `__get__(None)` / `__get__(None, None)` with
            // `__get__(None, None) is invalid`.
            let obj = args[0].clone();
            if obj.is_none() {
                let owner = args.get(1).cloned().unwrap_or_else(Value::none);
                if owner.is_none() {
                    return Err(pyrust_core::type_err!("__get__(None, None) is invalid"));
                }
                return Ok(prop.clone());
            }
            let fget = pyrust_builtins::property::with_property(prop, |s| (*s.fget).clone())
                .unwrap_or_else(Value::none);
            if fget.is_none() {
                return Err(property_accessor_error(prop, &obj, "getter"));
            }
            interp.call_function_expanded(fget, &[ExpandedCallArg { name: None, value: obj }])
        }
        K::Set => {
            if args.len() != 2 {
                return Err(pyrust_core::type_err!("expected 2 arguments, got {}",
                    args.len()));
            }
            let obj = args[0].clone();
            let value = args[1].clone();
            let fset = pyrust_builtins::property::with_property(prop, |s| (*s.fset).clone())
                .unwrap_or_else(Value::none);
            if fset.is_none() {
                return Err(property_accessor_error(prop, &obj, "setter"));
            }
            interp.call_function_expanded(
                fset,
                &[
                    ExpandedCallArg { name: None, value: obj },
                    ExpandedCallArg { name: None, value },
                ],
            )
        }
        K::Delete => {
            if args.len() != 1 {
                return Err(pyrust_core::type_err!("expected 1 argument, got {}",
                    args.len()));
            }
            let obj = args[0].clone();
            let fdel = pyrust_builtins::property::with_property(prop, |s| (*s.fdel).clone())
                .unwrap_or_else(Value::none);
            if fdel.is_none() {
                return Err(property_accessor_error(prop, &obj, "deleter"));
            }
            interp.call_function_expanded(fdel, &[ExpandedCallArg { name: None, value: obj }])
        }
    }
}

/// CPython's property-accessor error:
/// `property '<name>' of '<owner>' object has no <which>`, or the unnamed
/// `property of '<owner>' object has no <which>` when the property was never
/// bound in a class body (no `__set_name__`; issue #1846).
fn property_accessor_error(prop: &Value, instance: &Value, which: &str) -> PyError {
    let owner = value_type_name_str(instance);
    let prop_desc = match pyrust_builtins::property::with_property(prop, |s| s.name.clone())
        .flatten()
    {
        Some(n) => format!("property '{n}'"),
        None => "property".to_string(),
    };
    pyrust_core::py_err!("AttributeError", "{prop_desc} of '{owner}' object has no {which}")
}

/// Read a `__slots__` slot value from an instance's storage (issue #2084).
/// `instance` must be a `PyInstance`; an unset slot raises AttributeError.
fn member_descriptor_get(instance: &Value, slot: &str) -> Result<Value> {
    if let ValueKind::PyInstance(inst) = instance.kind() {
        if let Some(v) = inst.borrow().attrs.get(slot).cloned() {
            return Ok(v);
        }
        let class_name = inst.borrow().class.borrow().name.clone();
        return Err(pyrust_core::py_err!(
            "AttributeError",
            "'{class_name}' object has no attribute '{slot}'"
        ));
    }
    Ok(Value::none())
}

impl Interpreter {
    /// Dispatch a directly-invoked `member_descriptor` descriptor-protocol
    /// method (`S.x.__get__(obj[, owner])` / `S.x.__set__(obj, v)` /
    /// `S.x.__delete__(obj)`), issue #2084.  Mirrors CPython's
    /// `member_get`/`member_set`/`member_delete` arity and behaviour.
    pub(crate) fn member_descriptor_protocol_call(
        &mut self,
        descriptor: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        let (slot, owner_name) =
            pyrust_builtins::member_descriptor::as_member_descriptor_full(&descriptor)
                .expect("member_descriptor receiver");
        // CPython rejects an instance that is not of the owning type (or a
        // subclass) with a `descriptor '<slot>' for '<owner>' objects doesn't
        // apply to a '<T>' object` TypeError.  A `None` instance (class-level
        // `__get__`) is allowed and handled below.  Checking only "is a
        // PyInstance" is insufficient: an instance of an *unrelated* class is
        // not of the owning type, so it must raise the same TypeError (and in
        // particular `__set__` must not silently write into an unrelated
        // instance's storage).  The owning class is identified by name, matching
        // the rest of this file's name-based MRO checks.
        let check_instance = |obj: &Value| -> Result<()> {
            if let ValueKind::PyInstance(inst) = obj.kind() {
                let class = Rc::clone(&inst.borrow().class);
                if class_chain_contains_name(&class, &owner_name) {
                    return Ok(());
                }
            }
            let tname = pyrust_core::builtin_type_name(obj);
            Err(pyrust_core::type_err!(
                "descriptor '{slot}' for '{owner_name}' objects doesn't apply to a '{tname}' object"
            ))
        };
        match method {
            "__get__" => {
                // CPython's wrapper messages carry a leading space (the empty
                // method-name prefix) and split too-few / too-many wording.
                if args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        " expected at least 1 argument, got 0"
                    ));
                }
                if args.len() > 2 {
                    return Err(pyrust_core::type_err!(
                        " expected at most 2 arguments, got {}",
                        args.len()
                    ));
                }
                // Class-level access (`obj is None`) returns the descriptor.
                if args[0].is_none() {
                    return Ok(descriptor);
                }
                check_instance(&args[0])?;
                member_descriptor_get(&args[0], &slot)
            }
            "__set__" => {
                if args.len() != 2 {
                    return Err(pyrust_core::type_err!(
                        " expected 2 arguments, got {}",
                        args.len()
                    ));
                }
                check_instance(&args[0])?;
                match call_descriptor_set(
                    self,
                    &descriptor,
                    args[0].clone(),
                    args[1].clone(),
                    &slot,
                )? {
                    Some(r) => r.map(|_| Value::none()),
                    None => Ok(Value::none()),
                }
            }
            "__delete__" => {
                if args.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "expected 1 argument, got {}",
                        args.len()
                    ));
                }
                check_instance(&args[0])?;
                match call_descriptor_delete(self, &descriptor, args[0].clone(), &slot)? {
                    Some(r) => r.map(|_| Value::none()),
                    None => Ok(Value::none()),
                }
            }
            _ => unreachable!("member_descriptor_protocol_call method {method}"),
        }
    }
}

/// Handles both `property` (BuiltinObject with fget) and user-defined
/// descriptors (PyInstance with a class `__get__` method).
fn call_descriptor_get(
    interp: &mut Interpreter,
    descriptor: &Value,
    instance: Value,
    owner: Value,
    // Retained for call-site clarity; messages use the property's __set_name__
    // name (`prop_name`) rather than the lookup name.
    _attr_name: &str,
) -> Result<Value> {
    // `__slots__` member_descriptor: read the instance's slot storage; an unset
    // slot raises AttributeError (issue #2084).  Class-level access (`S.x`)
    // never reaches here — get_attr_class returns the descriptor itself.
    if let Some(slot) = pyrust_builtins::member_descriptor::as_member_descriptor(descriptor) {
        return member_descriptor_get(&instance, &slot);
    }
    // property special-case: use the stored fget directly.
    if let Some((fget, partial_slot, prop_name)) =
        pyrust_builtins::property::with_property(descriptor, |s| {
            (Rc::clone(&s.fget), s.partial_slot, s.name.clone())
        })
        && partial_slot.is_none()
    {
        return if fget.is_none() {
            // CPython 3.12: `property 'x' of 'C' object has no getter` (issue #1845).
            // The name comes from __set_name__ (issue #1846); anonymous
            // properties (never bound in a class body) use the unnamed form.
            let owner = value_type_name_str(&instance);
            let prop_desc = match &prop_name {
                Some(n) => format!("property '{n}'"),
                None => "property".to_string(),
            };
            Err(pyrust_core::py_err!("AttributeError", "{prop_desc} of '{owner}' object has no getter"))
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
    // Retained for call-site clarity; messages use the property's __set_name__
    // name (`prop_name`) rather than the lookup name.
    _attr_name: &str,
) -> Result<Option<Result<()>>> {
    // `__slots__` member_descriptor: store into the instance's slot storage
    // (issue #2084).
    if let Some(slot) = pyrust_builtins::member_descriptor::as_member_descriptor(class_val) {
        if let ValueKind::PyInstance(inst) = instance.kind() {
            inst.borrow_mut().attrs.insert(slot, value);
            return Ok(Some(Ok(())));
        }
        return Ok(Some(Ok(())));
    }
    // property special-case.
    if let Some((fset, partial_slot, prop_name)) =
        pyrust_builtins::property::with_property(class_val, |s| {
            (Rc::clone(&s.fset), s.partial_slot, s.name.clone())
        })
        && partial_slot.is_none()
    {
        return Ok(Some(if fset.is_none() {
            // CPython 3.12: `property 'x' of 'C' object has no setter` (issue #1845).
            // The name comes from __set_name__ (issue #1846); anonymous
            // properties (never bound in a class body) use the unnamed form.
            let owner = value_type_name_str(&instance);
            let prop_desc = match &prop_name {
                Some(n) => format!("property '{n}'"),
                None => "property".to_string(),
            };
            Err(pyrust_core::py_err!("AttributeError", "{prop_desc} of '{owner}' object has no setter"))
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
        // CPython: a descriptor with __delete__ but no __set__ is still a data
        // descriptor and blocks assignment.  Raise AttributeError: __set__
        // (CPython's exact message) rather than falling through to instance dict.
        if lookup_class_attr(&desc_class, "__delete__").is_some() {
            return Ok(Some(Err(pyrust_core::py_err!("AttributeError", "__set__"))));
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
    // Retained for call-site clarity; messages use the property's __set_name__
    // name (`prop_name`) rather than the lookup name.
    _attr_name: &str,
) -> Result<Option<Result<()>>> {
    // `__slots__` member_descriptor: clear the instance's slot storage; an
    // already-unset slot raises AttributeError (issue #2084).
    if let Some(slot) = pyrust_builtins::member_descriptor::as_member_descriptor(class_val) {
        if let ValueKind::PyInstance(inst) = instance.kind() {
            let removed = inst.borrow_mut().attrs.shift_remove(&slot).is_some();
            if !removed {
                // CPython's `member_delete` raises AttributeError with just the
                // slot name as the message (not the full "'C' object has no
                // attribute 'x'" form), issue #2084.
                return Ok(Some(Err(pyrust_core::py_err!("AttributeError", "{slot}"))));
            }
        }
        return Ok(Some(Ok(())));
    }
    // property special-case.
    if let Some((fdel, partial_slot, prop_name)) =
        pyrust_builtins::property::with_property(class_val, |s| {
            (Rc::clone(&s.fdel), s.partial_slot, s.name.clone())
        })
        && partial_slot.is_none()
    {
        return Ok(Some(if fdel.is_none() {
            // CPython 3.12: `property 'x' of 'C' object has no deleter` (issue #1845).
            // The name comes from __set_name__ (issue #1846); anonymous
            // properties (never bound in a class body) use the unnamed form.
            let owner = value_type_name_str(&instance);
            let prop_desc = match &prop_name {
                Some(n) => format!("property '{n}'"),
                None => "property".to_string(),
            };
            Err(pyrust_core::py_err!("AttributeError", "{prop_desc} of '{owner}' object has no deleter"))
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
                    return Err(pyrust_core::type_err!("Cannot create a consistent method resolution\norder (MRO) for bases {bases_str}"));
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

/// Index in `mro` at which a `super()` lookup should begin: the entry *after*
/// `class` (found by pointer identity), or `0` if `class` is not present.
///
/// Shared by the instance and classmethod `super()` paths, which both walk the
/// receiver's full MRO from the position following the defining class
/// (cooperative multiple inheritance).
fn mro_search_start(mro: &[Value], class: &Rc<RefCell<PyClass>>) -> usize {
    let class_ptr = Rc::as_ptr(class);
    mro.iter()
        .position(|v| match v.kind() {
            ValueKind::PyClass(c) => Rc::as_ptr(c) == class_ptr,
            _ => false,
        })
        .map(|i| i + 1)
        .unwrap_or(0)
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

/// `true` if `name` is an object-protocol method that every built-in data
/// value exposes (#2151).  `__bool__` is only on `None` (other primitives use
/// truthiness via `__len__`/value, not an inherited `object.__bool__`).
pub(crate) fn is_object_protocol_method(target: &Value, name: &str) -> bool {
    match name {
        "__sizeof__" | "__dir__" | "__reduce__" | "__reduce_ex__" | "__getstate__" => true,
        "__bool__" => matches!(target.kind(), ValueKind::None),
        _ => false,
    }
}

/// Returns `true` if `name` is a built-in method on `target`'s type.
/// Used by `get_attr` to produce `BuiltinBoundMethod` values.
fn builtin_has_method(target: &Value, name: &str) -> bool {
    // Issue #1909: container/sequence protocol dunders (`__len__`,
    // `__getitem__`, `__contains__`, `__add__`, …) are advertised by
    // `dir`/`hasattr` and resolvable as bound method-wrappers.  Gated on the
    // `__` prefix so the common method-name lookup (`lst.append`, `s.upper`)
    // pays only a cheap byte comparison before the per-type table below.
    if name.starts_with("__") {
        let type_name = pyrust_core::builtin_type_name(target);
        if is_protocol_dunder(&type_name, name) {
            return true;
        }
        // Issue #2151: object-protocol methods every built-in data value
        // inherits from `object` (`__sizeof__`/`__dir__`/`__reduce__`/
        // `__reduce_ex__`), plus `None.__bool__`.  Resolved as bound
        // method-wrappers so `(5).__sizeof__()` / `None.__bool__()` work;
        // dispatched in `bound_method_dispatch_inner`.  Gated to built-in
        // data values so function/class/bound-method attribute access is
        // untouched.
        if crate::interpreter::primitive_class_for_value(target).is_some()
            && is_object_protocol_method(target, name)
        {
            return true;
        }
        // Issue #2191: `(5).__format__(spec)` / `"hi".__format__(spec)` etc. must
        // route through the real format machinery (same result as
        // `format(self, spec)`).  Expose `__format__` as a bound method-wrapper
        // on every built-in data value so the dispatch lands in
        // `bound_method_dispatch_inner` (which delegates to `apply_format_spec`)
        // rather than the unbound `object.__format__` class-attr that drops
        // `self` and returns the spec verbatim.
        if name == "__format__"
            && crate::interpreter::primitive_class_for_value(target).is_some()
        {
            return true;
        }
    }
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
        ValueKind::Range { .. } | ValueKind::BigRange { .. } => {
            matches!(name, "__iter__" | "__len__" | "count" | "index")
        }
        ValueKind::BuiltinObject { ops, .. } => ops.has_method(name),
        _ => false,
    }
}

impl Interpreter {
    /// Execute `from module import *`.
    ///
    /// Extracted from the `ImportStar` VM dispatch arm so that changes to
    /// star-import semantics (__all__ handling, filtering, etc.) only require
    /// touching this method rather than vm.rs.
    pub(crate) fn exec_import_star(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        mod_reg: crate::bytecode::Reg,
    ) -> Result<()> {
        let mod_val = vm_read(regs, mod_reg, num_locals)?;
        if !matches!(mod_val.kind(), ValueKind::PyModule(_)) {
            return Err(pyrust_core::type_err!("import * requires a module, got {}",
                    pyrust_core::builtin_type_name(&mod_val),));
        }
        let ValueKind::PyModule(m) = mod_val.kind() else {
            unreachable!()
        };
        let pairs: Vec<(String, Value)> = {
            let borrowed = m.borrow();
            if let Some(all_val) = borrowed.attrs.get("__all__") {
                let items: Option<&[Value]> =
                    all_val.as_list().or_else(|| all_val.as_tuple());
                let mod_name = borrowed.name.clone();
                let mut names: Vec<String> = Vec::new();
                let mut err: Option<PyError> = None;
                match items {
                    Some(items) => {
                        for item in items {
                            match item.as_str() {
                                Some(s) => names.push(s.to_string()),
                                None => {
                                    err = Some(pyrust_core::type_err!("Item in {}.__all__ must be str, not {}",
                                            mod_name,
                                            pyrust_core::builtin_type_name(item),));
                                    break;
                                }
                            }
                        }
                    }
                    None => {
                        err = Some(pyrust_core::type_err!("'{}' object does not support indexing",
                                pyrust_core::builtin_type_name(all_val),));
                    }
                }
                drop(borrowed);
                if let Some(e) = err {
                    return Err(e);
                }
                let borrowed2 = m.borrow();
                let mut out = Vec::with_capacity(names.len());
                let mut attr_err: Option<(String, String)> = None;
                for name in &names {
                    match borrowed2.attrs.get(name) {
                        Some(v) => out.push((name.clone(), v.clone())),
                        None => {
                            attr_err = Some((
                                format!(
                                    "module '{}' has no attribute '{}'",
                                    mod_name, name,
                                ),
                                name.clone(),
                            ));
                            break;
                        }
                    }
                }
                drop(borrowed2);
                if let Some((attr_err_msg, attr_err_name)) = attr_err {
                    return Err(PyError::attribute_error(
                        attr_err_msg,
                        Some(attr_err_name),
                        None,
                    ));
                }
                out
            } else {
                borrowed
                    .attrs
                    .iter()
                    .filter(|(k, _)| !k.starts_with('_'))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            }
        };
        for (name, val) in pairs {
            self.assign_name(&name, val);
        }
        Ok(())
    }

    /// Execute `del name` for module-scope and `global`-declared names.
    ///
    /// Extracted from the `DeleteName` VM dispatch arm so that changes to
    /// name-deletion semantics only require touching this method rather than
    /// vm.rs.
    pub(crate) fn exec_delete_name(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        name_idx: u16,
    ) -> Result<()> {
        let name = code.names.get(name_idx as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: name index {} out of range (pool size {})",
                name_idx,
                code.names.len()
            ))
        })?;
        let is_global = self.env.borrow().global_names.contains(name.as_str());
        if is_global {
            let me = module_env(&self.env);
            let from_env = me.borrow_mut().values.remove(name.as_str());
            let in_env = from_env.is_some();
            let from_dict = self
                .module_globals_dict
                .dict_shift_remove(&PyKey::str_from(name.as_str()))
                .ok()
                .flatten();
            let in_dict = from_dict.is_some();
            if !in_env && !in_dict {
                return Err(PyError::name_error(
                    "NameError",
                    format!("name '{}' is not defined", name),
                    Some(name.to_string()),
                ));
            }
            bump_global_env_version(self);
            // SAFETY: same as SetItem / DeleteItem (issue #547, PR #646).
            if let Some(script_view) = self
                .vm_frame_views
                .iter()
                .find(|v| v.kind == FrameKind::Script)
                && let Some(&slot) = script_view.local_index.get(name.as_str()) {
                    let slot = slot as usize;
                    if slot < script_view.regs_len {
                        unsafe {
                            *script_view.regs_ptr.add(slot).as_mut() = Value::unset();
                        }
                    }
                }
            let del_candidate = from_env.or(from_dict);
            if let Some(val) = del_candidate {
                call_del_if_last_binding(self, val, regs, code.num_locals as usize);
            }
        } else {
            let from_env = self.env.borrow_mut().values.remove(name.as_str());
            let in_env = from_env.is_some();
            let is_module_scope = self.env.borrow().parent.is_none();
            let from_dict = if is_module_scope {
                self.module_globals_dict
                    .dict_shift_remove(&PyKey::str_from(name.as_str()))
                    .ok()
                    .flatten()
            } else {
                None
            };
            let in_dict = from_dict.is_some();
            if !in_env && !in_dict {
                return Err(PyError::name_error(
                    "NameError",
                    format!("name '{}' is not defined", name),
                    Some(name.to_string()),
                ));
            }
            if is_module_scope {
                bump_global_env_version(self);
            }
            let del_candidate = from_env.or(from_dict);
            if let Some(val) = del_candidate {
                call_del_if_last_binding(self, val, regs, code.num_locals as usize);
            }
        }
        Ok(())
    }
}
