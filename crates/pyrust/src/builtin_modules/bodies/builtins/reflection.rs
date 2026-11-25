use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: issubclass(cls, classinfo) — true if `cls` is a subclass.
    /// <https://docs.python.org/3/library/functions.html#issubclass>
    #[arity_style(expected_got)]
    fn issubclass(
        #[positional_only] cls: PyValueRef,
        #[positional_only] classinfo: PyValueRef,
    ) -> Result<Value> {
        // The `arg 1 must be a class` validation lives inside
        // `issubclass_check`, *after* the `__subclasscheck__` hook is
        // resolved on `type(classinfo)`: CPython only rejects a non-class
        // `cls` when no custom `__subclasscheck__` handles it (and validates
        // lazily per tuple/union leaf), so `issubclass(5, M())` where
        // `type(M())` defines the hook must return the hook's result rather
        // than raising.  See issue #2525.
        let result = issubclass_check(FN_NAME, cls.0, classinfo.0, _interp)?;
        Ok(Value::bool_(result))
    }

    /// CPython: delattr(obj, name) — delete an attribute.
    /// <https://docs.python.org/3/library/functions.html#delattr>
    ///
    /// Migrated to the typed-signature dialect (#400): `PyValueRef` +
    /// `attr_name_arg` preserves CPython's `attribute name must be string,
    /// not '<type>'` wording and `str` subclass acceptance while gaining the
    /// vectorcall fast entry.
    #[arity_style(expected_got)]
    fn delattr(
        #[positional_only] object: PyValueRef,
        #[positional_only] name: PyValueRef,
    ) -> Result<Value> {
        let name = attr_name_arg(name.0)?;
        // Delegate to the canonical delete_attr path so that every value
        // kind (BuiltinFunction, UserFunction, BoundMethod, PyClass, …)
        // raises the correct error type and message instead of the old
        // catch-all "delattr() object has no writable attributes".
        _interp.delete_attr(object.0.clone(), &name)?;
        Ok(Value::none())
    }

    /// CPython: isinstance(obj, classinfo) — type check.
    /// <https://docs.python.org/3/library/functions.html#isinstance>
    // Typed dialect (#builtin-fast-dispatch): two positional-only args, so
    // `isinstance(x, C)` gets the vectorcall fast entry.  `expected_got` arity
    // style reproduces the `isinstance expected 2 arguments, got N` wording the
    // legacy body spelled out.
    #[arity_style(expected_got)]
    fn isinstance(
        #[positional_only] obj: PyValue,
        #[positional_only] classinfo: PyValue,
    ) -> Result<Value> {
        let result = isinstance_check(FN_NAME, &obj.0, &classinfo.0, _interp)?;
        Ok(Value::bool_(result))
    }

    /// CPython: type(object) → type / type(name, bases, namespace) → new class.
    /// <https://docs.python.org/3/library/functions.html#type>
    ///
    /// The 3-arg form runs class-creation hooks (`__set_name__`,
    /// `__init_subclass__`) which may execute arbitrary user code (#2129/#2130).
    fn r#type(args) -> Result<Value> {
        // The 3-arg form `type(name, bases, ns, **kwds)` forwards keyword args
        // to `__init_subclass__` (so a bad kwarg surfaces as
        // `X.__init_subclass__() takes no keyword arguments`, matching CPython);
        // the 1-arg form `type(obj)` takes none.  Split positional / keyword
        // accordingly instead of rejecting all kwargs up front.
        let positional: Vec<&ExpandedCallArg> =
            args.iter().filter(|a| a.name.is_none()).collect();
        let init_subclass_kwargs: Vec<ExpandedCallArg> = args
            .iter()
            .filter(|a| a.name.is_some())
            .cloned()
            .collect();
        if positional.len() == 3 {
            let name = match positional[0].value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "type.__new__() argument 1 must be str, not {}",
                        value_type_name_str(&positional[0].value),
                    ),
                )),
            };
            // Extract all bases from the bases sequence.  Collect into a Vec
            // first (inside a scoped block so the kind() Ref guard drops before
            // we work with the Values — see #450).
            let base_values: Vec<Value> = match positional[1].value.kind() {
                ValueKind::Tuple(items) => items.to_vec(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "type.__new__() argument 2 must be tuple, not {}",
                        value_type_name_str(&positional[1].value),
                    ),
                )),
            };
            // Validate each entry and split into primary base + extra bases.
            // Issue #1453: reject non-subclassable singletons here too, so
            // `type("Foo", (type(None),), {})` raises TypeError just like the
            // `class Foo(type(None)): pass` syntax path does.
            let mut base: Option<Rc<RefCell<PyClass>>> = None;
            let mut extra_bases: Vec<Rc<RefCell<PyClass>>> = Vec::new();
            for (i, entry) in base_values.iter().enumerate() {
                match entry.kind() {
                    ValueKind::PyClass(c) => {
                        let cls = Rc::clone(c);
                        if let Some(tname) =
                            crate::interpreter::non_subclassable_builtin_name(&cls)
                        {
                            return Err(PyError::named(
                                "TypeError",
                                format!("type '{tname}' is not an acceptable base type"),
                            ));
                        }
                        if i == 0 {
                            base = Some(cls);
                        } else {
                            extra_bases.push(cls);
                        }
                    }
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() argument 2 entries must be classes"),
                    )),
                }
            }
            // Issue #1677: reject bases tuples that contain two or more
            // "solid" primitive types (int, str, float, bytes, tuple, list,
            // dict, set, frozenset) or two bases with non-empty `__slots__`
            // (issue #2109).  These have incompatible instance layouts; CPython
            // raises the same error via its `best_base`/`solid_base` check.
            {
                let all_bases: Vec<_> = base.iter().chain(extra_bases.iter()).cloned().collect();
                if crate::interpreter::bases_have_layout_conflict(&all_bases) {
                    return Err(PyError::named(
                        "TypeError",
                        "multiple bases have instance lay-out conflict".to_string(),
                    ));
                }
            }
            let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
            match positional[2].value.kind() {
                ValueKind::Dict(map) => {
                    for (k, v) in map.iter() {
                        if let PyKey::Str(key) = k {
                            attrs.insert(key.as_str().unwrap_or("").to_owned(), v.clone());
                        }
                    }
                }
                ValueKind::BuiltinObject { .. }
                    if pyrust_builtins::mapping_proxy::is_mapping_proxy(&positional[2].value) =>
                {
                    if let Some(class_rc) =
                        pyrust_builtins::mapping_proxy::as_class_rc(&positional[2].value)
                    {
                        let class = class_rc.borrow();
                        for (k, v) in class.attrs.iter() {
                            attrs.insert(k.clone(), v.clone());
                        }
                    }
                }
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "type.__new__() argument 3 must be dict, not {}",
                        value_type_name_str(&positional[2].value),
                    ),
                )),
            }
            // Issues #2129 / #2130: route the 3-arg constructor through the
            // same finalization the `class` statement runs (set __module__,
            // process __slots__, call __set_name__ on descriptors and
            // __init_subclass__ on the base) so a `type()`-built class is not
            // missing hooks a `class`-built one has.  Keyword args are forwarded
            // to __init_subclass__ (CPython routes type()'s kwds there).
            return _interp.build_class_via_type(
                name, base, extra_bases, attrs, None, &init_subclass_kwargs,
            );
        }
        // The 1-arg form `type(obj)` accepts no keyword arguments.
        reject_keyword_args_expanded(FN_NAME, args)?;
        if positional.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes 1 or 3 arguments"),
            ));
        }
        let obj = &positional[0].value;
        // For user-defined class instances return the actual Rc so that
        // `type(x) is type(x)` works via Rc::ptr_eq.
        //
        // Issue #462: the 11 migrated primitives (`int`, `str`, …) return
        // their per-thread `PyClass` singletons from `primitive_class_for_value`,
        // so `type(5).__name__ == "int"`, `bool.__bases__ == (int,)`, and
        // `isinstance(x, T)` work through the standard class machinery.
        //
        // Remaining variants (functions, modules, ranges, generators, …)
        // still emit a `BuiltinFunction(name)` sentinel — they're not part
        // of the primitive-class migration.
        Ok(value_class(obj))
    }

    /// CPython: hasattr(obj, name) — true if `getattr(obj, name)` would succeed.
    /// <https://docs.python.org/3/library/functions.html#hasattr>
    ///
    /// Migrated to the typed-signature dialect (#400): `PyValueRef` borrows
    /// arguments with zero cloning (avoiding the `PyValue` clone overhead) and
    /// enables the vectorcall fast entry.
    #[arity_style(expected_got)]
    fn hasattr(
        #[positional_only] object: PyValueRef,
        #[positional_only] name: PyValueRef,
    ) -> Result<Value> {
        let name = attr_name_arg(name.0)?;
        let result = match _interp.get_attr(object.0, &name) {
            Ok(_) => true,
            Err(ref e) if e.class_name_is("AttributeError") => false,
            Err(e) => return Err(e),
        };
        Ok(Value::bool_(result))
    }

    /// CPython: getattr(obj, name[, default]) — attribute access by name.
    /// <https://docs.python.org/3/library/functions.html#getattr>
    ///
    /// Migrated to the typed-signature dialect (#400): `PyValueRef` +
    /// `OptValueRef` avoid cloning on hit, keep explicit `getattr(o, n, None)`
    /// distinct from omitted `getattr(o, n)`, and enable the vectorcall fast
    /// entry.
    #[arity_style(expected_got)]
    fn getattr(
        #[positional_only] object: PyValueRef,
        #[positional_only] name: PyValueRef,
        #[positional_only] #[default(OptionalArg::Omitted)] default: OptValueRef,
    ) -> Result<Value> {
        let name = attr_name_arg(name.0)?;
        match _interp.get_attr(object.0, &name) {
            Ok(v) => Ok(v),
            Err(ref e) if e.class_name_is("AttributeError") => match default.as_value() {
                Some(fallback) => Ok(fallback.clone()),
                None => Err(e.clone()),
            },
            Err(e) => Err(e),
        }
    }

    /// CPython: setattr(obj, name, value) — attribute assignment by name.
    /// <https://docs.python.org/3/library/functions.html#setattr>
    ///
    /// Migrated to the typed-signature dialect (#400): `PyValueRef` +
    /// `attr_name_arg` preserves CPython's `attribute name must be string,
    /// not '<type>'` wording and `str` subclass acceptance while gaining the
    /// vectorcall fast entry.
    #[arity_style(expected_got)]
    fn setattr(
        #[positional_only] object: PyValueRef,
        #[positional_only] name: PyValueRef,
        #[positional_only] value: PyValueRef,
    ) -> Result<Value> {
        let name = attr_name_arg(name.0)?;
        _interp.assign_attr(object.0.clone(), &name, value.0.clone())?;
        Ok(Value::none())
    }

    /// CPython: vars([object]) — live `__dict__` where one exists, or current
    /// env if no argument.
    /// <https://docs.python.org/3/library/functions.html#vars>
    #[arity_style(expected_got)]
    fn vars(
        #[positional_only] #[default(OptionalArg::Omitted)] object: OptValueRef,
    ) -> Result<Value> {
        let Some(target) = object.as_value() else {
            // vars() with no args == locals(). Module frames return their live
            // globals, class frames their persistent live namespace, and only
            // function frames return a snapshot.
            let is_module_scope = _interp
                .vm_frame_views
                .last()
                .map(|v| v.kind() == crate::interpreter::FrameKind::Script)
                .unwrap_or(true);
            if is_module_scope {
                sync_module_env_to_globals_dict(_interp);
                return Ok(_interp.active_locals_dict());
            }
            return Ok(current_locals_value(_interp));
        };
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                // Issue #2076: a `__slots__` instance with no `__dict__` has no
                // mapping, so `vars()` raises TypeError (CPython parity).
                if class_suppresses_instance_dict(&instance.borrow().class) {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() argument must have __dict__ attribute"),
                    ));
                }
                // Issue #1981: when `__dict__` was replaced wholesale, the live
                // backing dict is `vars(obj)` (so `vars(obj) is obj.__dict__`).
                if let Some(d) = instance.borrow().attrs.dict_ref() {
                    return Ok(d.clone());
                }
                Ok(pyrust_builtins::instance_dict::instance_dict(Rc::clone(
                    instance,
                )))
            }
            ValueKind::PyModule(_) => {
                // `vars(m)` is defined as `m.__dict__`, so read it through the
                // module attribute path instead of re-deriving the namespace
                // here. That path returns the live dict for a `sys`-style or
                // source-backed module and otherwise synthesises the built-in
                // module namespace — including the `__name__` / `__doc__` /
                // `__package__` / `__loader__` / `__spec__` slots CPython
                // exposes, which this arm previously dropped (issue #2918).
                _interp.get_attr(target, "__dict__")
            }
            ValueKind::PyClass(class) => {
                Ok(pyrust_builtins::mapping_proxy::mapping_proxy(Rc::clone(class)))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() argument must have __dict__ attribute"),
            )),
        }
    }

    /// CPython: globals() — the live module namespace dict (issue #706).
    /// <https://docs.python.org/3/library/functions.html#globals>
    ///
    /// Returns the captured root namespace's persistent globals `Value::dict`.
    /// On the first call, syncs current module env values into the dict and
    /// disables that root's opcode caches so arbitrary alias mutations remain
    /// observable. `globals() is globals()` is always `True` (same Rc).
    #[arity_style(takes_no_args)]
    fn globals() -> Result<Value> {
        sync_module_env_to_globals_dict(_interp);
        Ok(_interp.active_globals_dict())
    }

    /// CPython: locals() — mapping for the current local namespace.
    /// <https://docs.python.org/3/library/functions.html#locals>
    ///
    /// At module scope, `locals()` returns the same live dict as `globals()`
    /// (CPython parity: at module level the two namespaces are the same object).
    /// Inside a class body it returns the persistent live class namespace.
    /// Inside a function it returns a snapshot — CPython also snapshots and its
    /// docs warn that mutations to the returned dict aren't guaranteed to
    /// propagate.
    #[arity_style(takes_no_args)]
    fn locals() -> Result<Value> {
        // At module scope (the innermost frame is the Script frame, or there are
        // no function frames), return the persistent root namespace provider —
        // the same object as globals() for a normal module.
        let is_module_scope = _interp
            .vm_frame_views
            .last()
            .map(|v| v.kind() == crate::interpreter::FrameKind::Script)
            .unwrap_or(true);
        if is_module_scope {
            sync_module_env_to_globals_dict(_interp);
            return Ok(_interp.active_locals_dict());
        }
        Ok(current_locals_value(_interp))
    }

    /// CPython: `__build_class__(func, name, /, *bases, metaclass=None, **kwds)` —
    /// internal helper used by the `class` statement and exposed on `builtins`.
    fn __build_class__(args) -> Result<Value> {
        _interp.builtin_build_class(args)
    }

    /// CPython: exec(source, globals=None, locals=None, /, *, closure=None) —
    /// execute Python source code or a code object.
    /// <https://docs.python.org/3/library/functions.html#exec>
    fn exec(args) -> Result<Value> {
        let mut positional: Vec<&Value> = Vec::with_capacity(3);
        let mut closure_val: Option<Value> = None;
        for arg in args {
            match arg.name.as_deref() {
                None => positional.push(&arg.value),
                Some("closure") => {
                    if closure_val.is_some() {
                        return Err(PyError::named(
                            "TypeError",
                            "exec() got multiple values for keyword argument 'closure'".to_string(),
                        ));
                    }
                    closure_val = Some(arg.value.clone());
                }
                Some(other) => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("'{other}' is an invalid keyword argument for exec()"),
                    ));
                }
            }
        }
        if positional.is_empty() {
            return Err(PyError::named(
                "TypeError",
                "exec() takes at least 1 positional argument (0 given)".to_string(),
            ));
        }
        if positional.len() > 3 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "exec() takes at most 3 positional arguments ({} given)",
                    positional.len()
                ),
            ));
        }
        let source_val = positional[0];
        let mut globals_opt = positional
            .get(1)
            .copied()
            .filter(|v| !matches!(v.kind(), ValueKind::None))
            .cloned();
        let mut locals_opt = positional
            .get(2)
            .copied()
            .filter(|v| !matches!(v.kind(), ValueKind::None))
            .cloned();
        let closure_opt = closure_val.filter(|v| !matches!(v.kind(), ValueKind::None));

        if globals_opt.is_none()
            && _interp
                .vm_frame_views
                .last()
                .is_some_and(|view| view.kind() == crate::interpreter::FrameKind::Function)
        {
            let active_env = Rc::clone(&_interp.env);
            globals_opt = Some(_interp.globals_for_environment(&active_env));
            if locals_opt.is_none() {
                locals_opt = Some(_interp.retain_current_function_locals());
            }
        }
        // Inject `__builtins__` into a caller-supplied globals dict, matching
        // CPython's PyEval_EvalCode behaviour.
        if let Some(g) = &globals_opt {
            inject_builtins_into_globals(g);
        }

        // Code object path (both `compile()` objects and `func.__code__`).
        if pyrust_builtins::code::is_code(source_val) {
            let compiled = crate::interpreter::with_code_state(source_val, |cs| {
                (Rc::clone(&cs.code), cs.mode, Rc::clone(&cs.local_index))
            })
            .or_else(|| {
                pyrust_builtins::code::with_raw_code(source_val, |raw| {
                    raw.downcast_ref::<crate::bytecode::FnCode>().map(|fc| {
                        (
                            Rc::new(fc.clone()),
                            crate::interpreter::CodeMode::Exec,
                            Rc::new(std::collections::HashMap::new()),
                        )
                    })
                })
                .flatten()
            });
            let (fn_code, mode, local_index) = compiled.ok_or_else(|| {
                PyError::named("TypeError", "exec() code object has no executable bytecode")
            })?;

            let num_free = fn_code.free_var_slots().len();
            let closure_slots: Option<Vec<pyrust_core::CellSlot>> = if num_free == 0 {
                if closure_opt.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "cannot use a closure with this code object".to_string(),
                    ));
                }
                None
            } else {
                let mut slots = Vec::with_capacity(num_free);
                let ok = match closure_opt.as_ref().map(|v| v.kind()) {
                    Some(ValueKind::Tuple(items)) if items.len() == num_free => {
                        let mut all_cells = true;
                        for item in items.iter() {
                            if let Some(slot) = pyrust_builtins::cell::cell_slot(item) {
                                slots.push(slot);
                            } else {
                                all_cells = false;
                                break;
                            }
                        }
                        all_cells
                    }
                    _ => false,
                };
                if !ok {
                    return Err(PyError::named(
                        "TypeError",
                        format!("code object requires a closure of exactly length {num_free}"),
                    ));
                }
                Some(slots)
            };

            use crate::interpreter::CodeMode;
            match mode {
                CodeMode::Exec | CodeMode::Single => {
                    _interp.run_exec_code_with_closure(
                        fn_code,
                        local_index,
                        globals_opt,
                        locals_opt,
                        closure_slots,
                    )?;
                }
                CodeMode::Eval => {
                    _interp.run_eval_code_dispatch(fn_code, local_index, globals_opt, locals_opt)?;
                }
            }
            return Ok(Value::none());
        }

        if closure_opt.is_some() {
            return Err(PyError::named(
                "TypeError",
                "closure can only be used when source is a code object".to_string(),
            ));
        }

        // String / bytes / bytearray path.
        let source = extract_source_text(_interp, source_val, FN_NAME, "string, bytes or code object")?;
        _interp.exec_source(&source, globals_opt, locals_opt)?;
        Ok(Value::none())
    }

    /// CPython: eval(expression[, globals[, locals]]) — evaluate a Python
    /// expression string, bytes, or code object and return its value.
    /// <https://docs.python.org/3/library/functions.html#eval>
    #[arity_style(expected_got)]
    fn eval(
        #[positional_only] source: PyValueRef,
        #[positional_only] #[default(OptionalArg::Omitted)] globals: OptValueRef,
        #[positional_only] #[default(OptionalArg::Omitted)] locals: OptValueRef,
    ) -> Result<Value> {
        let source_val = source.0;
        let globals_opt = globals
            .as_value()
            .filter(|v| !matches!(v.kind(), ValueKind::None))
            .cloned();
        let locals_opt = locals
            .as_value()
            .filter(|v| !matches!(v.kind(), ValueKind::None))
            .cloned();
        // Inject `__builtins__` into a caller-supplied globals dict, matching
        // CPython's PyEval_EvalCode behaviour.
        if let Some(g) = &globals_opt {
            inject_builtins_into_globals(g);
        }
        // Code object path (from compile() or func.__code__).
        if pyrust_builtins::code::is_code(source_val) {
            if let Some(result) = crate::interpreter::with_code_state(source_val, |cs| {
                use crate::interpreter::CodeMode;
                match cs.mode {
                    CodeMode::Eval => _interp.run_eval_code_dispatch(
                        Rc::clone(&cs.code),
                        Rc::clone(&cs.local_index),
                        globals_opt.clone(),
                        locals_opt.clone(),
                    ),
                    CodeMode::Exec | CodeMode::Single => _interp
                        .run_exec_code(
                            Rc::clone(&cs.code),
                            Rc::clone(&cs.local_index),
                            globals_opt.clone(),
                            locals_opt.clone(),
                        )
                        .map(|()| Value::none()),
                }
            }) {
                return result;
            }
            if let Some(fn_code) = pyrust_builtins::code::with_raw_code(source_val, |raw| {
                raw.downcast_ref::<crate::bytecode::FnCode>().cloned()
            })
            .flatten()
            {
                return _interp.run_eval_code_dispatch(
                    Rc::new(fn_code),
                    Rc::new(std::collections::HashMap::new()),
                    globals_opt,
                    locals_opt,
                );
            }
        }
        // String / bytes / bytearray path.
        let source = extract_source_text(_interp, source_val, FN_NAME, "string, bytes or code object")?;
        _interp.eval_source(&source, globals_opt, locals_opt)
    }

    /// CPython: compile(source, filename, mode, flags=0, dont_inherit=False, optimize=-1) —
    /// compile source (str, bytes, bytearray, or ast.AST) to a code object or AST.
    /// <https://docs.python.org/3/library/functions.html#compile>
    #[arity_style(clinic)]
    fn compile(
        source: PyValueRef,
        filename: PyValueRef,
        mode: PyValueRef,
        #[default(OptionalArg::Omitted)] flags: OptValueRef,
        #[default(OptionalArg::Omitted)] dont_inherit: OptValueRef,
        #[default(OptionalArg::Omitted)] optimize: OptValueRef,
    ) -> Result<Value> {
        const PYCF_ONLY_AST: i64 = 0x0400;
        const PYCF_TYPE_COMMENTS: i64 = 0x1000;
        const PYCF_ALLOW_TOP_LEVEL_AWAIT: i64 = 0x2000;
        const CO_FUTURE_ANNOTATIONS: i64 = 0x100_0000;
        // All recognised CPython 3.12 compiler & __future__ flags.
        const KNOWN_FLAGS_MASK: i64 = 0x0010 // CO_NESTED
            | 0x0100 // PyCF_SOURCE_IS_UTF8
            | 0x0200 // PyCF_DONT_IMPLY_DEDENT
            | PYCF_ONLY_AST
            | 0x0800 // PyCF_IGNORE_COOKIE
            | PYCF_TYPE_COMMENTS
            | PYCF_ALLOW_TOP_LEVEL_AWAIT
            | 0x4000 // PyCF_ALLOW_INCOMPLETE_INPUT
            | 0x2_0000 // CO_FUTURE_DIVISION
            | 0x4_0000 // CO_FUTURE_ABSOLUTE_IMPORT
            | 0x8_0000 // CO_FUTURE_WITH_STATEMENT
            | 0x10_0000 // CO_FUTURE_PRINT_FUNCTION
            | 0x20_0000 // CO_FUTURE_UNICODE_LITERALS
            | 0x40_0000 // CO_FUTURE_BARRY_AS_BDFL
            | 0x80_0000 // CO_FUTURE_GENERATOR_STOP
            | CO_FUTURE_ANNOTATIONS;

        let mut flags_int: i64 = match flags.as_value() {
            None => 0,
            Some(v) => match v.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object cannot be interpreted as an integer",
                            value_type_name_str(v),
                        ),
                    ));
                }
            },
        };
        if (flags_int & !KNOWN_FLAGS_MASK) != 0 {
            return Err(PyError::named(
                "ValueError",
                "compile(): unrecognised flags".to_string(),
            ));
        }

        let dont_inherit_bool = match dont_inherit.as_value() {
            None => false,
            Some(v) => match v.kind() {
                ValueKind::Int(n) => n != 0,
                ValueKind::Bool(b) => b,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object cannot be interpreted as an integer",
                            value_type_name_str(v),
                        ),
                    ));
                }
            },
        };

        let optimize_raw: i64 = match optimize.as_value() {
            None => -1,
            Some(v) => match v.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object cannot be interpreted as an integer",
                            value_type_name_str(v),
                        ),
                    ));
                }
            },
        };
        if !(-1..=2).contains(&optimize_raw) {
            return Err(PyError::named(
                "ValueError",
                "compile(): invalid optimize value".to_string(),
            ));
        }
        let effective_optimize: u8 = if optimize_raw == -1 {
            _interp.compile_optimize_level()
        } else {
            optimize_raw as u8
        };

        if !dont_inherit_bool && calling_frame_has_future_annotations(_interp) {
            flags_int |= CO_FUTURE_ANNOTATIONS;
        }

        let only_ast = (flags_int & PYCF_ONLY_AST) != 0;
        let type_comments = (flags_int & PYCF_TYPE_COMMENTS) != 0;
        let allow_top_level_await = (flags_int & PYCF_ALLOW_TOP_LEVEL_AWAIT) != 0;
        let future_annotations = (flags_int & CO_FUTURE_ANNOTATIONS) != 0;

        let compile_filename = extract_compile_filename(_interp, filename.0)?;

        let mode_val = mode.0;
        let mode_str = mode_val.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() arg 3 must be a string, not '{}'",
                    value_type_name_str(mode_val),
                ),
            )
        })?;
        match mode_str {
            "exec" | "eval" | "single" => {}
            "func_type" => {
                if !only_ast {
                    return Err(PyError::named(
                        "ValueError",
                        "compile() mode 'func_type' requires flag PyCF_ONLY_AST".to_string(),
                    ));
                }
            }
            _ => {
                let msg = if only_ast {
                    "compile() mode must be 'exec', 'eval', 'single' or 'func_type'"
                } else {
                    "compile() mode must be 'exec', 'eval' or 'single'"
                };
                return Err(PyError::named("ValueError", msg.to_string()));
            }
        }

        let source_val = source.0;
        let source_is_ast = is_ast_node(source_val);

        // `ast.PyCF_ONLY_AST`: parse source to an `ast.AST` node (or return an
        // already-parsed AST node after mode validation).
        if only_ast {
            if source_is_ast {
                return Ok(source_val.clone());
            }
            let source_text =
                extract_source_text(_interp, source_val, FN_NAME, "string, bytes or AST object")?;
            // Emit invalid-escape SyntaxWarnings via lexer when parsing non-func_type source.
            if mode_str != "func_type"
                && let Ok(lexer) = crate::lexer::Lexer::new(&source_text)
            {
                let (_, _, _, _, warnings) = lexer.into_tokens_with_pos_and_warnings();
                for (lineno, msg) in warnings {
                    crate::builtin_modules::warnings::emit_syntax_warning(
                        _interp,
                        &msg,
                        &compile_filename,
                        lineno as i64,
                    )?;
                }
            }
            let ast_mod = _interp.load_module("ast")?;
            let parse_fn = _interp.get_attr(&ast_mod, "parse")?;
            return _interp.call_function_expanded(
                parse_fn,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: Value::string(source_text),
                    },
                    ExpandedCallArg {
                        name: Some("filename".to_string()),
                        value: Value::string(compile_filename),
                    },
                    ExpandedCallArg {
                        name: Some("mode".to_string()),
                        value: Value::string(mode_str),
                    },
                    ExpandedCallArg {
                        name: Some("type_comments".to_string()),
                        value: Value::bool_(type_comments),
                    },
                ],
            );
        }

        // Convert `ast.AST` or `str`/`bytes`/`bytearray` into source text (plus
        // optional AST-derived line overrides).
        let (source_text, ast_linenos): (String, Vec<u32>) = if source_is_ast {
            let expected_node = match mode_str {
                "exec" => "Module",
                "eval" => "Expression",
                "single" => "Interactive",
                _ => unreachable!(),
            };
            if !instance_is_ast_subclass(source_val, expected_node) {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "expected {expected_node} node, got {}",
                        value_type_name_str(source_val),
                    ),
                ));
            }
            let explicit_linenos = extract_ast_linenos(_interp, source_val, mode_str);
            let ast_mod = _interp.load_module("ast")?;
            let unparse_fn = _interp.get_attr(&ast_mod, "unparse")?;
            let unparsed = _interp.call_function_expanded(
                unparse_fn,
                &[ExpandedCallArg {
                    name: None,
                    value: source_val.clone(),
                }],
            )?;
            let text = unparsed
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| PyError::runtime("ast.unparse() did not return str".to_string()))?;
            (text, explicit_linenos)
        } else {
            let text =
                extract_source_text(_interp, source_val, FN_NAME, "string, bytes or AST object")?;
            (text, Vec::new())
        };

        let inject_future_annotations = |program: &mut Vec<crate::ast::Stmt>,
                                         linenos: &mut Vec<u32>| {
            if !future_annotations {
                return;
            }
            let future_sym = pyrust_core::Symbol::intern("__future__");
            let annotations_sym = pyrust_core::Symbol::intern("annotations");
            let mut future_end = 0usize;
            while let Some(crate::ast::Stmt::ImportFrom { module, names }) = program.get(future_end)
            {
                if *module != future_sym {
                    break;
                }
                if names.iter().any(|(name, _)| *name == annotations_sym) {
                    return;
                }
                future_end += 1;
            }
            let alias = pyrust_core::Symbol::intern("__pyrust_future_annotations__");
            let first_line = linenos.first().copied().unwrap_or(1);
            program.insert(
                0,
                crate::ast::Stmt::ImportFrom {
                    module: future_sym,
                    names: vec![(annotations_sym, Some(alias))],
                },
            );
            linenos.insert(0, first_line);
            let del_pos = future_end + 1;
            program.insert(
                del_pos,
                crate::ast::Stmt::Delete(vec![crate::ast::Expr::Var(alias, None)]),
            );
            linenos.insert(del_pos, first_line);
        };

        let compile_opts = crate::compiler::CompileOptions {
            optimize: effective_optimize,
            allow_top_level_await,
        };
        let extra_co_flags = if future_annotations {
            CO_FUTURE_ANNOTATIONS
        } else {
            0
        };

        match mode_str {
            "exec" => {
                let (mut program, mut linenos) = _interp
                    .parse_source_to_stmts_with_warnings(&source_text, &compile_filename)?;
                if ast_linenos.len() == linenos.len() {
                    for (dst, &src_ln) in linenos.iter_mut().zip(ast_linenos.iter()) {
                        if src_ln != 0 {
                            *dst = src_ln;
                        }
                    }
                }
                inject_future_annotations(&mut program, &mut linenos);
                let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
                let global_names = crate::interpreter::collect_global_names(&program);
                let local_names = crate::interpreter::collect_local_names(
                    &[],
                    &program,
                    &global_names,
                    &empty,
                );
                let local_index: Rc<
                    std::collections::HashMap<String, crate::bytecode::Reg>,
                > = Rc::new(
                    (0u32..)
                        .zip(local_names.iter())
                        .map(|(i, n)| (n.clone(), i))
                        .collect(),
                );
                let code = crate::compiler::compile_script_with_options(
                    &program,
                    Rc::clone(&local_index),
                    false,
                    &linenos,
                    &compile_filename,
                    compile_opts,
                )
                .map(|c| Rc::new(crate::optimizer::optimize(c)))?;
                Ok(crate::interpreter::value_code_object_with_flags(
                    code,
                    crate::interpreter::CodeMode::Exec,
                    local_index,
                    extra_co_flags,
                ))
            }
            "eval" => {
                let trimmed = source_text.trim();
                let (program, mut linenos) = _interp
                    .parse_source_to_stmts_with_warnings(trimmed, &compile_filename)?;
                if ast_linenos.len() == linenos.len() {
                    for (dst, &src_ln) in linenos.iter_mut().zip(ast_linenos.iter()) {
                        if src_ln != 0 {
                            *dst = src_ln;
                        }
                    }
                }
                let local_index: Rc<std::collections::HashMap<String, crate::bytecode::Reg>> =
                    Rc::new(std::collections::HashMap::new());
                let code = crate::compiler::compile_eval_expr_with_options(
                    &program,
                    Rc::clone(&local_index),
                    &linenos,
                    &compile_filename,
                    compile_opts,
                )
                .map(|c| Rc::new(crate::optimizer::optimize(c)))?;
                Ok(crate::interpreter::value_code_object_with_flags(
                    code,
                    crate::interpreter::CodeMode::Eval,
                    local_index,
                    extra_co_flags,
                ))
            }
            "single" => {
                let (mut program, mut linenos) = _interp
                    .parse_single_source_to_stmts_with_warnings(&source_text, &compile_filename)?;
                if ast_linenos.len() == linenos.len() {
                    for (dst, &src_ln) in linenos.iter_mut().zip(ast_linenos.iter()) {
                        if src_ln != 0 {
                            *dst = src_ln;
                        }
                    }
                }
                inject_future_annotations(&mut program, &mut linenos);
                let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
                let global_names = crate::interpreter::collect_global_names(&program);
                let local_names = crate::interpreter::collect_local_names(
                    &[],
                    &program,
                    &global_names,
                    &empty,
                );
                let local_index: Rc<
                    std::collections::HashMap<String, crate::bytecode::Reg>,
                > = Rc::new(
                    (0u32..)
                        .zip(local_names.iter())
                        .map(|(i, n)| (n.clone(), i))
                        .collect(),
                );
                let code = crate::compiler::compile_script_with_options(
                    &program,
                    Rc::clone(&local_index),
                    true,
                    &linenos,
                    &compile_filename,
                    compile_opts,
                )
                .map(|c| Rc::new(crate::optimizer::optimize(c)))?;
                Ok(crate::interpreter::value_code_object_with_flags(
                    code,
                    crate::interpreter::CodeMode::Single,
                    local_index,
                    extra_co_flags,
                ))
            }
            _ => unreachable!(),
        }
    }

    /// CPython: dir([object]) — list of attribute names.
    /// <https://docs.python.org/3/library/functions.html#dir>
    #[arity_style(expected_got)]
    fn dir(
        #[positional_only] #[default(OptionalArg::Omitted)] object: OptValueRef,
    ) -> Result<Value> {
        let Some(target) = object.as_value() else {
            let mut names: Vec<String> = _interp
                .env
                .borrow()
                .namespace_values()
                .keys()
                .map(String::from)
                .collect();
            names.sort();
            names.dedup();
            return Ok(Value::list(names.into_iter().map(Value::string).collect()));
        };
        // Honour a user-defined `__dir__` override.  CPython's `dir(obj)` is
        // `sorted(type(obj).__dir__(obj))`: it accepts any iterable result,
        // sorts it via the elements' own comparison (so a non-str element only
        // errors if the `<` comparison fails), and does NOT dedup the custom
        // result.  Only `PyInstance` values can carry an overridden `__dir__`;
        // primitives use the default `dir_names` path (issue #1941).
        if let ValueKind::PyInstance(inst) = target.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__dir__") {
                let result =
                    invoke_class_method(_interp, method_val, target.clone(), &[])?;
                let mut items = _interp.collect_iterable(&result)?;
                // Sort the collected values exactly as `sorted()` would,
                // surfacing the element comparison error verbatim.
                let mut sort_err: Option<PyError> = None;
                let has_instance = items
                    .iter()
                    .any(|v| matches!(v.kind(), ValueKind::PyInstance(_)));
                if has_instance {
                    items.sort_by(|a, b| {
                        if sort_err.is_some() {
                            return std::cmp::Ordering::Equal;
                        }
                        match _interp.richcmp_order(a, b) {
                            Ok(ord) => ord,
                            Err(e) => {
                                sort_err = Some(e);
                                std::cmp::Ordering::Equal
                            }
                        }
                    });
                } else {
                    items.sort_by(|a, b| {
                        if sort_err.is_some() {
                            return std::cmp::Ordering::Equal;
                        }
                        match compare_values(a, b) {
                            Ok(ord) => ord,
                            Err(e) => {
                                sort_err = Some(e);
                                std::cmp::Ordering::Equal
                            }
                        }
                    });
                }
                if let Some(e) = sort_err {
                    return Err(e);
                }
                return Ok(Value::list(items));
            }
        }
        if let ValueKind::PyModule(module) = target.kind() {
            _interp.ensure_module_namespace_synced(module);
        }
        let mut names: Vec<String> = dir_names(target);
        names.sort();
        names.dedup();
        Ok(Value::list(names.into_iter().map(Value::string).collect()))
    }

}
