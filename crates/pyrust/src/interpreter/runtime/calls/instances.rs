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

    /// Construct an instance of `class` through its validated call-site
    /// [`PlainClassPlan`] (issue #3383).
    ///
    /// The caller must have just checked `plan.is_current(class)`.  The plan
    /// then guarantees that `type.__call__` is the default one and that the
    /// class is neither abstract nor primitive-backed.  For an ordinary class
    /// `__new__` is `object.__new__`, so this goes straight to allocation and
    /// (when present) the ordinary-function `__init__`; for a built-in
    /// exception that carries only `BaseException`'s own state (issue #3426)
    /// the positional arguments become `args`.  Either way these are the same
    /// observable steps as `call_class_expanded`, minus the per-call
    /// re-derivation.
    ///
    /// Anything the plan does not cover is handed back to the generic path,
    /// which stays the single owner of every diagnostic.
    pub(crate) fn construct_plain_class(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        plan: &PlainClassPlan,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        if plan.is_base_layout_exception() {
            // The generic path owns the keyword-argument diagnostic.
            if args.iter().any(|arg| arg.name.is_some()) {
                return self.call_class_expanded(Rc::clone(class), args);
            }
            let values: smallvec::SmallVec<[Value; 4]> =
                args.iter().map(|arg| arg.value.clone()).collect();
            return Ok(instantiate_base_layout_exception(
                Rc::clone(class),
                Value::tuple_from_slice(&values),
            ));
        }
        let Some(init) = &plan.init else {
            if !args.is_empty() {
                // `object.__new__` rejects the excess arguments (#2323).
                return self.call_class_expanded(Rc::clone(class), args);
            }
            return Ok(self.allocate_plain_instance(class, plan));
        };
        let Some(function) = init.function.upgrade() else {
            // A current stamp keeps the function stored on a live class, so
            // this is unreachable; resolving afresh is always correct.
            return self.call_class_expanded(Rc::clone(class), args);
        };

        let instance = self.allocate_plain_instance(class, plan);
        let result = match self.call_plain_init_positional(
            &function,
            init.positional_parameter_count,
            &instance,
            args,
        ) {
            Some(result) => result?,
            None => {
                self.call_user_function_expanded(function, args, std::slice::from_ref(&instance))?
            }
        };
        if !result.is_none() {
            return Err(pyrust_core::type_err!(&format!(
                "__init__() should return None, not '{}'",
                pyrust_core::builtin_type_name(&result),
            )));
        }
        Ok(instance)
    }

    /// Run a plan's `__init__` for an all-positional call by binding `self`,
    /// each argument, and any trailing defaults straight into their
    /// destination registers — the binding the general binder would produce,
    /// without its keyword matching and arity bookkeeping.
    ///
    /// Returns `None`, having run nothing, whenever that binding is not
    /// provably complete (a keyword argument, too many arguments, a missing
    /// argument without a default, a signature with `*args` / `**kwargs` /
    /// keyword-only parameters); the caller then uses the general binder,
    /// which owns every argument diagnostic.
    #[inline]
    fn call_plain_init_positional(
        &mut self,
        function: &Rc<UserFunction>,
        positional_parameter_count: usize,
        instance: &Value,
        args: &[ExpandedCallArg],
    ) -> Option<Result<Value>> {
        let supplied = args.len() + 1;
        // `usize::MAX` marks a signature that always needs the general binder.
        if positional_parameter_count == usize::MAX
            || supplied > positional_parameter_count
            || args.iter().any(|arg| arg.name.is_some())
        {
            return None;
        }
        let code = self.get_or_compile_bytecode(function)?;
        let num_regs = code.num_regs as usize;
        let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];
        let mut cells = allocate_frame_cells(&code, &function.closure_cells);
        let bound: Result<bool> = (|| {
            bind_param_direct(function, num_regs, &mut regs, &cells, 0, instance.clone())?;
            for (index, arg) in args.iter().enumerate() {
                bind_param_direct(
                    function,
                    num_regs,
                    &mut regs,
                    &cells,
                    index + 1,
                    arg.value.clone(),
                )?;
            }
            for index in supplied..positional_parameter_count {
                // Override-aware, so a reassigned `__defaults__` is honoured.
                let Some(default) = function.positional_default(index) else {
                    return Ok(false);
                };
                bind_param_direct(function, num_regs, &mut regs, &cells, index, default)?;
            }
            Ok(true)
        })();
        match bound {
            Ok(true) => {
                Some(self.run_bound_user_frame_inline(function, &code, &mut regs, &mut cells))
            }
            // Nothing ran yet: the frame's own registers and cells are simply
            // dropped and the general binder reports the missing argument.
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        }
    }

    /// Allocate the bare instance of a plain class, counting it for the
    /// finalizer bookkeeping when its class defines `__del__` (#3341).
    #[inline]
    fn allocate_plain_instance(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        plan: &PlainClassPlan,
    ) -> Value {
        if plan.has_finalizer {
            self.note_possible_finalizable_instance(class);
        }
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs: InstanceAttrs::new(),
        })))
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
        // CPython's `object.__new__` refuses a class that still has abstract
        // methods.  Only a class created through a metaclass can carry a
        // non-empty `__abstractmethods__` here, so ordinary classes skip the
        // probe entirely.
        if class.borrow().metatype.is_some() {
            self.reject_abstract_class(&class)?;
        }
        // One stamped classification answers both "is this an exception
        // class?" and which construction rules apply to it (issue #3426).
        let exception_kinds = classify_exception_class(&class);
        if exception_kinds.exception {
            // Exception classes do not use the construction plan, so one
            // that defines `__del__` is counted here (issue #3341).
            if self.vm_hooks_flags & VM_HOOK_FINALIZER_ARMED != 0 {
                self.note_possible_finalizable_instance(&class);
            }
            return self.construct_exception_instance(class, args, exception_kinds);
        }

        self.instantiate_normal_instance(class, args)
    }
    /// Raise CPython 3.12's `TypeError` when `class` still has abstract
    /// methods (issue #3293).
    ///
    /// Mirrors `object.__new__`: only the class's *own* `__abstractmethods__`
    /// counts.  CPython caches it as the per-type `Py_TPFLAGS_IS_ABSTRACT`
    /// flag, which is set by assigning `__abstractmethods__` on that type and
    /// is not inherited; `abc.ABCMeta` assigns it on every class it creates.
    fn reject_abstract_class(&mut self, class: &Rc<RefCell<PyClass>>) -> Result<()> {
        let Some(abstracts) = class.borrow().attrs.get("__abstractmethods__").cloned() else {
            return Ok(());
        };
        if !self.truthy_value(&abstracts)? {
            return Ok(());
        }
        let mut names: Vec<String> = self
            .collect_iterable(&abstracts)?
            .iter()
            .map(|name| match name.as_str() {
                Some(text) => text.to_string(),
                None => name.repr_raw(),
            })
            .collect();
        names.sort();
        let plural = if names.len() > 1 { "s" } else { "" };
        let class_name = class.borrow().name.clone();
        Err(PyError::named(
            "TypeError",
            format!(
                "Can't instantiate abstract class {class_name} without an implementation \
                 for abstract method{plural} '{}'",
                names.join("', '")
            ),
        ))
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
        if plan.is_none() {
            self.note_possible_finalizable_instance(&class);
        }
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
                _ if prim == PrimitiveLayout::None
                    && !class_is_subclass_of(&class, &type_class_singleton()) =>
                {
                    // Ordinary classes resolve an arbitrary `__new__` through
                    // class-level descriptor access. `__new__` is implicitly
                    // static, so the bound result still receives the explicit
                    // class exactly once from `type.__call__`.
                    let class_value = Value::py_class(Rc::clone(&class));
                    let callable =
                        if let Some(bound) = bind_class_level_method_wrapper(&new_val, &class)? {
                            bound
                        } else if slot_is_descriptor(&new_val) {
                            call_descriptor_get(
                                self,
                                &new_val,
                                Value::none(),
                                class_value.clone(),
                                "__new__",
                            )?
                        } else {
                            new_val.clone()
                        };
                    let mut combined: ExpandedArgBuf =
                        ExpandedArgBuf::with_capacity(args.len() + 1);
                    combined.push(ExpandedCallArg {
                        name: None,
                        value: class_value,
                    });
                    combined.extend(args.iter().cloned());
                    call_slot_value_unbound(self, callable, &combined)?
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
                    if let Some(init_val) = init {
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
                instance.borrow_mut().attrs.set_builtin_data(backing);
            }
            // Issue #994 (frozenset/tuple) and #1204 (str/int/float/bytes/…):
            // immutable / scalar primitives build the backing from the
            // constructor args immediately — the content is fixed at
            // construction and __init__ cannot change it.
            PrimitiveLayout::Immutable(kind) | PrimitiveLayout::Scalar(kind) => {
                let backing = crate::interpreter::construct_primitive_backing(self, kind, args)?;
                instance.borrow_mut().attrs.set_builtin_data(backing);
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
                        instance.borrow_mut().attrs.set_builtin_data(backing);
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
