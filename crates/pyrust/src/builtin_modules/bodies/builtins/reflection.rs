use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: issubclass(cls, classinfo) — true if `cls` is a subclass.
    /// <https://docs.python.org/3/library/functions.html#issubclass>
    fn issubclass(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 2 arguments, got {}", args.len()),
            ));
        }
        // The `arg 1 must be a class` validation lives inside
        // `issubclass_check`, *after* the `__subclasscheck__` hook is
        // resolved on `type(classinfo)`: CPython only rejects a non-class
        // `cls` when no custom `__subclasscheck__` handles it (and validates
        // lazily per tuple/union leaf), so `issubclass(5, M())` where
        // `type(M())` defines the hook must return the hook's result rather
        // than raising.  See issue #2525.
        let result = issubclass_check(FN_NAME, &args[0].value, &args[1].value, _interp)?;
        Ok(Value::bool_(result))
    }

    /// CPython: delattr(obj, name) — delete an attribute.
    /// <https://docs.python.org/3/library/functions.html#delattr>
    ///
    /// Kept in the `(args)` dialect (#2350): a non-`str` name must raise
    /// CPython's `attribute name must be string, not '<type>'` (no
    /// function prefix, names the offending type, accepts `str`
    /// subclasses) — a `PyStr` typed binding instead emits the generic
    /// `delattr() argument 'name' must be str, not int` and rejects str
    /// subclasses.  The shared `attr_name_arg` validator matches
    /// `getattr`/`hasattr`/`setattr` byte-for-byte.
    fn delattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 2 arguments, got {}", args.len()),
            ));
        }
        let name = attr_name_arg(&args[1].value)?;
        // Delegate to the canonical delete_attr path so that every value
        // kind (BuiltinFunction, UserFunction, BoundMethod, PyClass, …)
        // raises the correct error type and message instead of the old
        // catch-all "delattr() object has no writable attributes".
        _interp.delete_attr(args[0].value.clone(), &name)?;
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
    /// Kept in the `(args)` dialect (#400/#2331): hasattr is a warm path,
    /// and migrating to a typed `expected_got` signature regressed a tight
    /// `hasattr(o, 'x')` hit/miss loop ~6–8% (the per-arg `PyValue` binding
    /// clone) for zero wording benefit — its arity/kwarg messages already
    /// match CPython.  Bench captured in the #400 batch-1 PR.
    fn hasattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 2 arguments, got {}", args.len()),
            ));
        }
        let name = attr_name_arg(&args[1].value)?;
        let result = match _interp.get_attr(&args[0].value, &name) {
            Ok(_) => true,
            Err(ref e) if e.class_name_is("AttributeError") => false,
            Err(e) => return Err(e),
        };
        Ok(Value::bool_(result))
    }

    /// CPython: getattr(obj, name[, default]) — attribute access by name.
    /// <https://docs.python.org/3/library/functions.html#getattr>
    ///
    /// Must stay in the `(args)` dialect (#400/#2331): `getattr(o, n, None)`
    /// must return Python `None` as the default rather than re-raising the
    /// `AttributeError`, but an `Option<PyValue>` + `#[default(None)]`
    /// trailing param collapses an explicit `None` default and an absent
    /// default into the same Rust `None`, breaking the `default=None` case
    /// (the exact blocker documented on `fn next`).  Its arity/kwarg
    /// wording already matches CPython, so there is no wording gain from a
    /// typed migration either.
    fn getattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at least 2 arguments, got {}", args.len()),
            ));
        }
        if args.len() > 3 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 3 arguments, got {}", args.len()),
            ));
        }
        let name = attr_name_arg(&args[1].value)?;
        match _interp.get_attr(&args[0].value, &name) {
            Ok(v) => Ok(v),
            Err(ref e) if e.class_name_is("AttributeError") && args.len() == 3 => {
                Ok(args[2].value.clone())
            }
            Err(e) => Err(e),
        }
    }

    /// CPython: setattr(obj, name, value) — attribute assignment by name.
    /// <https://docs.python.org/3/library/functions.html#setattr>
    ///
    /// Kept in the `(args)` dialect (#2350): a non-`str` name must raise
    /// CPython's `attribute name must be string, not '<type>'` (no
    /// function prefix, names the offending type, accepts `str`
    /// subclasses) — a `PyStr` typed binding instead emits the generic
    /// `setattr() argument 'name' must be str, not int` and rejects str
    /// subclasses.  The shared `attr_name_arg` validator matches
    /// `getattr`/`hasattr`/`delattr` byte-for-byte.
    fn setattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 3 arguments, got {}", args.len()),
            ));
        }
        let name = attr_name_arg(&args[1].value)?;
        _interp.assign_attr(args[0].value.clone(), &name, args[2].value.clone())?;
        Ok(Value::none())
    }

    /// CPython: vars([object]) — live `__dict__` where one exists, or current
    /// env if no argument.
    /// <https://docs.python.org/3/library/functions.html#vars>
    fn vars(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 1 argument"),
            ));
        }
        if args.is_empty() {
            // vars() with no args == locals(): return a snapshot of the current
            // frame's local namespace.  At module scope that is module globals
            // (CPython parity: vars() is locals() is globals() at top level).
            let is_module_scope = _interp
                .vm_frame_views
                .last()
                .map(|v| v.kind == crate::interpreter::FrameKind::Script)
                .unwrap_or(true);
            if is_module_scope {
                sync_module_env_to_globals_dict(_interp);
                return Ok(_interp.active_locals_dict());
            }
            return Ok(Value::dict(snapshot_current_locals(_interp)));
        }
        match args[0].value.kind() {
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
                _interp.get_attr(&args[0].value, "__dict__")
            }
            ValueKind::PyClass(class) => {
                Ok(pyrust_builtins::mapping_proxy::mapping_proxy(Rc::clone(class)))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() argument must have __dict__ attribute (got '{}')",
                    value_type_name_str(&args[0].value),
                ),
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
    fn globals(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        sync_module_env_to_globals_dict(_interp);
        Ok(_interp.active_globals_dict())
    }

    /// CPython: locals() — dict snapshot of the current local namespace.
    /// <https://docs.python.org/3/library/functions.html#locals>
    ///
    /// At module scope, `locals()` returns the same live dict as `globals()`
    /// (CPython parity: at module level the two namespaces are the same object).
    /// Inside a function body it returns a snapshot of the function's locals —
    /// CPython also snapshots and its docs warn that mutations to the returned
    /// dict aren't guaranteed to propagate.
    fn locals(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        // At module scope (the innermost frame is the Script frame, or there are
        // no function frames), return the persistent root namespace provider —
        // the same object as globals() for a normal module.
        let is_module_scope = _interp
            .vm_frame_views
            .last()
            .map(|v| v.kind == crate::interpreter::FrameKind::Script)
            .unwrap_or(true);
        if is_module_scope {
            sync_module_env_to_globals_dict(_interp);
            return Ok(_interp.active_locals_dict());
        }
        Ok(Value::dict(snapshot_current_locals(_interp)))
    }

    /// CPython: exec(source[, globals[, locals]]) — execute Python source code.
    /// <https://docs.python.org/3/library/functions.html#exec>
    ///
    /// `source` may be a string or a code object returned by `compile()`.
    /// When `globals` and `locals` are omitted the code runs in the current
    /// interpreter's module namespace (assignments become module globals).
    /// When an explicit `globals` dict is supplied the code runs in that
    /// namespace.  Returns `None`.
    fn exec(args) -> Result<Value> {
        let (source_val, globals_opt, locals_opt) = parse_exec_eval_args(FN_NAME, args)?;
        // Inject `__builtins__` into a caller-supplied globals dict, matching
        // CPython's PyEval_EvalCode behaviour.
        if let Some(g) = &globals_opt {
            inject_builtins_into_globals(g);
        }
        // Code object path (from compile()).
        if let Some(result) = crate::interpreter::with_code_state(&source_val, |cs| {
            use crate::interpreter::CodeMode;
            match cs.mode {
                CodeMode::Exec => {
                    _interp.run_exec_code(
                        Rc::clone(&cs.code),
                        Rc::clone(&cs.local_index),
                        globals_opt.clone(),
                        locals_opt.clone(),
                    )
                }
                CodeMode::Eval => {
                    // exec() can take an eval-mode code object too (CPython allows it).
                    _interp.run_eval_code_dispatch(
                        Rc::clone(&cs.code),
                        Rc::clone(&cs.local_index),
                        globals_opt.clone(),
                        locals_opt.clone(),
                    ).map(|_| ())
                }
            }
        }) {
            result?;
            return Ok(Value::none());
        }
        // String path.
        let source = source_val.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() arg 1 must be a string or code object, not '{}'",
                    value_type_name_str(&source_val),
                ),
            )
        })?;
        _interp.exec_source(source, globals_opt, locals_opt)?;
        Ok(Value::none())
    }

    /// CPython: eval(expression[, globals[, locals]]) — evaluate a Python
    /// expression string and return its value.
    /// <https://docs.python.org/3/library/functions.html#eval>
    fn eval(args) -> Result<Value> {
        let (source_val, globals_opt, locals_opt) = parse_exec_eval_args(FN_NAME, args)?;
        // Inject `__builtins__` into a caller-supplied globals dict, matching
        // CPython's PyEval_EvalCode behaviour.
        if let Some(g) = &globals_opt {
            inject_builtins_into_globals(g);
        }
        // Code object path (from compile()).
        if let Some(result) = crate::interpreter::with_code_state(&source_val, |cs| {
            _interp.run_eval_code_dispatch(
                Rc::clone(&cs.code),
                Rc::clone(&cs.local_index),
                globals_opt.clone(),
                locals_opt.clone(),
            )
        }) {
            return result;
        }
        // String path.
        let source = source_val.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() arg 1 must be a string or code object, not '{}'",
                    value_type_name_str(&source_val),
                ),
            )
        })?;
        _interp.eval_source(source, globals_opt, locals_opt)
    }

    /// CPython: compile(source, filename, mode, ...) — compile source to a code
    /// object.
    /// <https://docs.python.org/3/library/functions.html#compile>
    ///
    /// pyrust stores the compiled `FnCode` wrapped in a `Value`; the returned
    /// value can be passed to `exec()` or `eval()`.  Only the `"exec"` and
    /// `"eval"` modes are supported; `"single"` raises `NotImplementedError`.
    fn compile(args) -> Result<Value> {
        // Reject keyword arguments — CPython accepts them but we keep it simple.
        if args.iter().any(|a| a.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() does not accept keyword arguments"),
            ));
        }
        if args.len() < 3 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() requires at least 3 arguments ({} given)",
                    args.len()
                ),
            ));
        }
        let source_val = &args[0].value;
        let source = source_val.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() arg 1 must be a string, not '{}'",
                    value_type_name_str(source_val),
                ),
            )
        })?;
        // filename (arg 2) — CPython tags the resulting code object's
        // `co_filename` with it, so an exception raised inside the compiled code
        // reports this name in its traceback (#2438).  Non-string filenames
        // (CPython also accepts bytes / path-like) fall back to `<unknown>`.
        let compile_filename = args[1]
            .value
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let mode_val = &args[2].value;
        let mode = mode_val.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() arg 3 must be a string, not '{}'",
                    value_type_name_str(mode_val),
                ),
            )
        })?;
        match mode {
            "exec" => {
                // Thread the lexer line table through so errors inside the
                // compiled code report correct internal line numbers (#2245).
                let (program, linenos) =
                    crate::interpreter::Interpreter::parse_source_to_stmts_with_linenos(source)?;
                let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
                let global_names = crate::interpreter::collect_global_names(&program);
                let local_names = crate::interpreter::collect_local_names(
                    &[],
                    &program,
                    &global_names,
                    &empty,
                );
                // A code object may later run under exec(globals, locals).
                // Preserve every ordinary assignment as a fast local so it
                // remains distinguishable from an explicit `global` write,
                // even for code with more than the script runner's 200-name
                // all-env threshold.
                let local_index: Rc<
                    std::collections::HashMap<String, crate::bytecode::Reg>,
                > = Rc::new(
                    (0u32..)
                        .zip(local_names.iter())
                        .map(|(i, n)| (n.clone(), i))
                        .collect(),
                );
                let code = crate::compiler::compile_script_with_linenos(
                    &program,
                    Rc::clone(&local_index),
                    false,
                    &linenos,
                    &compile_filename,
                )
                .map(|c| Rc::new(crate::optimizer::optimize(c)))?;
                Ok(crate::interpreter::value_code_object(
                    code,
                    crate::interpreter::CodeMode::Exec,
                    local_index,
                ))
            }
            "eval" => {
                let trimmed = source.trim();
                let (program, linenos) =
                    crate::interpreter::Interpreter::parse_source_to_stmts_with_linenos(trimmed)?;
                let local_index: Rc<std::collections::HashMap<String, crate::bytecode::Reg>> =
                    Rc::new(std::collections::HashMap::new());
                let code = crate::compiler::compile_eval_expr_with_linenos(
                    &program,
                    Rc::clone(&local_index),
                    &linenos,
                    &compile_filename,
                )
                .map(|c| Rc::new(crate::optimizer::optimize(c)))?;
                Ok(crate::interpreter::value_code_object(
                    code,
                    crate::interpreter::CodeMode::Eval,
                    local_index,
                ))
            }
            "single" => Err(PyError::named(
                "NotImplementedError",
                "compile() mode 'single' is not yet implemented".to_string(),
            )),
            other => Err(PyError::named(
                "ValueError",
                format!("compile() mode must be 'exec', 'eval' or 'single', not {other:?}"),
            )),
        }
    }

    /// CPython: dir([object]) — list of attribute names.
    /// <https://docs.python.org/3/library/functions.html#dir>
    fn dir(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 1 argument"),
            ));
        }
        if args.is_empty() {
            let mut names: Vec<String> =
                _interp.env.borrow().values.keys().map(String::from).collect();
            names.sort();
            names.dedup();
            return Ok(Value::list(names.into_iter().map(Value::string).collect()));
        }
        // Honour a user-defined `__dir__` override.  CPython's `dir(obj)` is
        // `sorted(type(obj).__dir__(obj))`: it accepts any iterable result,
        // sorts it via the elements' own comparison (so a non-str element only
        // errors if the `<` comparison fails), and does NOT dedup the custom
        // result.  Only `PyInstance` values can carry an overridden `__dir__`;
        // primitives use the default `dir_names` path (issue #1941).
        if let ValueKind::PyInstance(inst) = args[0].value.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__dir__") {
                let result =
                    invoke_class_method(_interp, method_val, args[0].value.clone(), &[])?;
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
        let mut names: Vec<String> = dir_names(&args[0].value);
        names.sort();
        names.dedup();
        Ok(Value::list(names.into_iter().map(Value::string).collect()))
    }

}
