// Calling a class and constructing ordinary instances.
//
// Exception allocation is delegated to `exceptions`; concrete builtin
// constructors and index/sequence protocols live in their own modules.

impl Interpreter {
    /// Invoke a Python-defined `__new__` with the explicit class supplied by
    /// `type.__call__`. A classmethod additionally binds that same class, so
    /// its underlying function receives the class twice (#2958).
    pub(crate) fn call_user_new_expanded(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        new_val: Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        let ValueKind::UserFunction(function) = new_val.kind() else {
            unreachable!("call_user_new_expanded requires UserFunction")
        };
        let function = Rc::clone(function);
        let class_value = Value::py_class(Rc::clone(class));
        if function.kind == pyrust_core::UserFunctionKind::ClassMethod {
            let mut explicit_args = ExpandedArgBuf::with_capacity(args.len() + 1);
            explicit_args.push(ExpandedCallArg {
                name: None,
                value: class_value.clone(),
            });
            explicit_args.extend(args.iter().cloned());
            invoke_class_method(self, new_val, class_value, &explicit_args)
        } else {
            self.call_user_function_expanded(function, args, &[class_value])
        }
    }

    pub(crate) fn call_class_expanded(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        // Issue #1956: `Cls(*args)` is uniformly `type(Cls).__call__(Cls, *args)`.
        // If the metaclass defines a *user* `__call__` override, route through
        // it; its `super().__call__(*args)` chains back to the default
        // `type.__call__` (see `default_construct`).  Ordinary classes (metatype
        // is the built-in `type`) return `None` here and fall through to the
        // existing default construct, preserving the fast path.
        //
        // Issue #2939: bind through the shared descriptor-aware helper rather
        // than prepending the class here, so a `staticmethod` / `classmethod`
        // metaclass `__call__` follows the same descriptor rules as every other
        // implicit dunder.  The `UserFunction` gate is unchanged: the built-in
        // `type.__call__` sentinel is a `BuiltinFunction` and still falls
        // straight through to `default_construct`, keeping the ordinary
        // construction fast path free of any extra dispatch.
        //
        // Issue #2944: a descriptor `__call__` (a `property`, or any object
        // whose class defines `__get__`) joins the same gate, so
        // `invoke_class_method` can bind it. Issue #2947 pre-binds a
        // classmethod using the metaclass lookup provenance while keeping the
        // original direct call for every other slot. `slot_is_descriptor` is
        // false for the `BuiltinFunction` `type.__call__` sentinel, so the
        // ordinary construction fast path still falls straight through.
        if let Some(call_fn) =
            crate::interpreter::metaclass_dunder_for_call(&class, "__call__").transpose()?
            && (matches!(
                call_fn.kind(),
                ValueKind::UserFunction(_) | ValueKind::ClassBoundMethod { .. }
            ) || crate::interpreter::is_class_bound_any_callable(&call_fn)
                || crate::interpreter::slot_is_descriptor(&call_fn))
        {
            return invoke_class_method(self, call_fn, Value::py_class(Rc::clone(&class)), args);
        }

        self.default_construct(class, args)
    }

    /// The default `type.__call__` behaviour: allocate via `__new__` and run
    /// `__init__`.  This is the single construction site reached both by a
    /// plain `Cls(*args)` (when the metaclass does not override `__call__`) and
    /// by `super().__call__(*args)` chaining from a metaclass `__call__`
    /// override.  Issue #1956.
    pub(crate) fn default_construct(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        if is_exception_class(&class) {
            return self.construct_exception_instance(class, args);
        }

        self.instantiate_normal_instance(class, args)
    }
    /// Instantiate a normal (non-exception) class: walk the MRO for a
    /// user-defined `__new__`, then call `__init__`, falling back to the
    /// default allocation + primitive-backing path when neither is defined.
    fn instantiate_normal_instance(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        // Primitive classes never reach this fn — the `PyClass` arm in
        // `call_function_expanded` short-circuits them via
        // `PRIMITIVE_CLASS_DISPATCH` (issue #462).  Subclasses of
        // primitives (`class S(int): pass`) DO reach here but without an
        // inherited `__init__` (helpers.rs deliberately leaves primitive
        // class attrs empty so the BuiltinFunction constructor isn't
        // exposed to PyInstance-based subclass dispatch — see #463
        // Copilot review).  They land in the `None` arm of the
        // init match below.

        // Resolve `__new__`, `__init__`, and the primitive-base classification
        // in a *single* base-chain walk (the common single-inheritance case).
        // Previously each of these did its own full MRO traversal on every
        // instantiation, so the per-construction cost scaled with MRO depth.
        // `resolve_construction_plan` returns `None` for multiply-inherited
        // classes, where attribute resolution must follow the C3 MRO — those
        // fall back to the byte-identical per-attr `lookup_class_attr` walks.
        let plan = resolve_construction_plan_cached(&class);
        let prim = match &plan {
            Some(p) => p.prim,
            None => classify_primitive_layout(&class),
        };

        // `__new__` protocol: walk the MRO for `__new__`, skipping the
        // default `object.__new__` (which the normal allocation path below
        // already implements).  If a user-defined `__new__` (UserFunction) or
        // a non-object BuiltinFunction `__new__` is found, call it with `cls`
        // as the first argument, then call `__init__` on the result if it is
        // an instance of `cls` (CPython parity, issue #1143).
        let mro_new = match &plan {
            Some(p) => p.new_val.clone(),
            None => lookup_class_attr(&class, "__new__"),
        };
        let has_user_new = mro_new.as_ref().is_some_and(|value| {
            !crate::interpreter::value_is_canonical_slot(
                value,
                crate::interpreter::CanonicalSlot::ObjectNew,
            )
        });
        if has_user_new {
            let new_val = mro_new.unwrap();
            let new_result = match new_val.kind() {
                ValueKind::UserFunction(_) => {
                    self.call_user_new_expanded(&class, new_val.clone(), args)?
                }
                ValueKind::BuiltinFunction(_) => {
                    let mut combined: ExpandedArgBuf =
                        ExpandedArgBuf::with_capacity(args.len() + 1);
                    combined.push(ExpandedCallArg {
                        name: None,
                        value: Value::py_class(Rc::clone(&class)),
                    });
                    combined.extend(args.iter().cloned());
                    self.call_builtin_new_value(&new_val, &combined)?
                }
                _ => {
                    return Err(pyrust_core::type_err!(
                        "__new__ must be a callable, not '{}'",
                        pyrust_core::builtin_type_name(&new_val)
                    ));
                }
            };

            // After `__new__` succeeds, call `__init__` on the result if it
            // is a PyInstance whose class is equal to or a subclass of `cls`.
            if let ValueKind::PyInstance(inst_rc) = new_result.kind() {
                let inst_class = inst_rc.borrow().class.clone();
                if class_is_subclass_of(&inst_class, &class) {
                    let init = lookup_class_attr(&inst_class, "__init__");
                    if let Some(init_val) = init
                        && matches!(
                            init_val.kind(),
                            ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
                        )
                    {
                        let result = invoke_class_method(
                            self,
                            init_val,
                            Value::py_instance(Rc::clone(inst_rc)),
                            args,
                        )?;
                        if !result.is_none() {
                            return Err(pyrust_core::type_err!(&format!(
                                "__init__() should return None, not '{}'",
                                pyrust_core::builtin_type_name(&result),
                            )));
                        }
                    }
                }
            }
            // Issue #1385: metaclass protocol — if __new__ returned a PyClass
            // (i.e. a class object was constructed) and the calling class is a
            // metaclass (subclass of type), call __init__ on the new class with
            // the same arguments.  This mirrors type.__call__'s two-phase
            // __new__ + __init__ protocol applied at the metaclass level.
            if let ValueKind::PyClass(new_class_rc) = new_result.kind() {
                let new_class_rc = Rc::clone(new_class_rc);
                let type_cls = type_class_singleton();
                if class_is_subclass_of(&class, &type_cls) {
                    let init = lookup_class_attr(&class, "__init__");
                    if let Some(init_val) = init
                        && matches!(
                            init_val.kind(),
                            ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
                        )
                    {
                        // Skip type.__init__ (the no-op sentinel) to avoid
                        // double-calling when there is no user __init__.
                        let is_type_init = crate::interpreter::value_is_canonical_slot(
                            &init_val,
                            crate::interpreter::CanonicalSlot::TypeInit,
                        );
                        if !is_type_init {
                            let result = invoke_class_method(
                                self,
                                init_val,
                                Value::py_class(Rc::clone(&new_class_rc)),
                                args,
                            )?;
                            if !result.is_none() {
                                return Err(pyrust_core::type_err!(&format!(
                                    "__init__() should return None, not '{}'",
                                    pyrust_core::builtin_type_name(&result),
                                )));
                            }
                        }
                    }
                }
            }
            return Ok(new_result);
        }

        let instance = Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(&class),
            attrs: InstanceAttrs::new(),
        }));

        // `prim` (the primitive-base classification) was resolved together with
        // `__new__` / `__init__` in the single construction-plan walk above.
        // The three primitive categories are mutually exclusive — a class cannot
        // inherit two incompatible primitive storage layouts.
        match prim {
            // Issue #976: dict/list/set — pre-initialise an *empty* backing
            // store *before* calling __init__, so that `self[k] = v` (or any
            // method call on `self`) inside a user-defined __init__ sees a valid
            // __builtin_data__ entry to delegate to.  When there is no __init__,
            // the None arm below replaces it with the args-populated value.
            PrimitiveLayout::Mutable(kind) => {
                let backing = crate::interpreter::construct_primitive_backing(self, kind, &[])?;
                instance
                    .borrow_mut()
                    .attrs
                    .insert(BUILTIN_DATA_ATTR, backing);
            }
            // Issue #994 (frozenset/tuple) and #1204 (str/int/float/bytes/…):
            // immutable / scalar primitives build the backing from the
            // constructor args immediately — the content is fixed at
            // construction and __init__ cannot change it.
            PrimitiveLayout::Immutable(kind) | PrimitiveLayout::Scalar(kind) => {
                let backing = crate::interpreter::construct_primitive_backing(self, kind, args)?;
                instance
                    .borrow_mut()
                    .attrs
                    .insert(BUILTIN_DATA_ATTR, backing);
            }
            PrimitiveLayout::None => {}
        }

        let init = match plan {
            Some(p) => p.init_val,
            None => lookup_class_attr(&class, "__init__"),
        };

        // Issue #2335: CPython's `object_new` (Objects/typeobject.c) checks
        // `type->tp_new != object_new` *first*.  When this class (or an ancestor)
        // ever had its `__new__` slot wrapped — i.e. `__new__` was assigned or
        // `del`'d at runtime (`del Cls.__new__`, `Cls.__new__ = object.__new__`)
        // — that wrapper is sticky and inherited, so even though the attribute
        // now resolves back to `object.__new__` through the MRO, excess args are
        // rejected with the `object.__new__()` wording regardless of whether a
        // custom `__init__` is present.  This branch must precede the bare-class
        // check below to match CPython's message-precedence.  Primitive bases are
        // exempt (handled above / by their backing constructors).
        if prim == PrimitiveLayout::None && !args.is_empty() && class_chain_new_slot_wrapped(&class)
        {
            return Err(pyrust_core::type_err!(
                "object.__new__() takes exactly one argument (the type to instantiate)"
            ));
        }
        // CPython parity (issue #2323): for a plain (non-primitive) class that
        // overrides neither `__new__` nor `__init__`, excess construction
        // arguments are rejected by `object.__new__` with `<Cls>() takes no
        // arguments`.
        let init_is_object = init.as_ref().is_none_or(|value| {
            crate::interpreter::value_is_canonical_slot(
                value,
                crate::interpreter::CanonicalSlot::ObjectInit,
            )
        });
        if prim == PrimitiveLayout::None && init_is_object && !args.is_empty() {
            let class_name = class.borrow().name.clone();
            return Err(pyrust_core::type_err!("{class_name}() takes no arguments"));
        }

        match init {
            // Issue #2944: `__init__` is dispatched by the same rules as every
            // other implicit dunder, so hand the resolved slot straight to
            // `invoke_class_method` instead of re-deciding here which value
            // kinds are acceptable.  This arm previously accepted only
            // `UserFunction` / `BuiltinFunction` and answered everything else
            // with a bare `RuntimeError: __init__ attribute is not callable` —
            // which rejected a descriptor `__init__` (`property`, user
            // `__get__`) that CPython binds and runs, rejected a callable
            // instance that CPython calls (issue #2054), and reported
            // `__init__ = 5` with the wrong exception type and wording where
            // CPython says `TypeError: 'int' object is not callable`
            // (issue #2055).  All three now fall out of the shared path.
            Some(method_val) => {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&instance)),
                    args,
                )?;
                if !result.is_none() {
                    return Err(pyrust_core::type_err!(&format!(
                        "__init__() should return None, not '{}'",
                        pyrust_core::builtin_type_name(&result),
                    )));
                }
            }
            None => {
                // No `__init__` in the MRO.  If the class inherits from a
                // mutable primitive (dict / list / set), call that base's
                // constructor with the provided args and store the result as
                // `__builtin_data__` so subscript / method dispatch can
                // delegate to the backing value (issue #976).
                // NOTE: the empty backing was already inserted above; replace it
                // with the args-populated value now.
                match prim {
                    PrimitiveLayout::Mutable(kind) => {
                        let backing =
                            crate::interpreter::construct_primitive_backing(self, kind, args)?;
                        instance
                            .borrow_mut()
                            .attrs
                            .insert(BUILTIN_DATA_ATTR, backing);
                    }
                    PrimitiveLayout::None => {
                        // Excess args with no `__init__`/`__new__` are already
                        // rejected (as `TypeError`) by the bare-class guard above
                        // (issue #2323); reaching here means `args` is empty.
                    }
                    // Immutable / scalar primitives already populated their
                    // args-based backing above; nothing more to do here.
                    PrimitiveLayout::Immutable(_) | PrimitiveLayout::Scalar(_) => {}
                }
            }
        }

        Ok(Value::py_instance(instance))
    }
}
