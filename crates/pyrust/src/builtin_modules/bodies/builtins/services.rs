use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: bool(x=False) — bool constructor.
    /// <https://docs.python.org/3/library/functions.html#bool>
    ///
    /// Migrated to the typed-signature dialect (#400).  `Option<PyValue>`
    /// + `#[default(None)]` is the natural shape for a single optional
    /// positional: `None` means "no arg" → False, `Some(v)` means
    /// "compute truthiness of v".  Conflating `bool()` with `bool(None)`
    /// is safe because CPython's truthiness of `None` is also False, so
    /// both paths land on the same answer.
    ///
    /// This can run user code by dispatching `__bool__` and
    /// (as fallback) `__len__` on user-defined objects via `truthy_value`.
    fn bool(
        #[positional_only]
        #[default(None)]
        x: Option<PyValue>,
    ) -> Result<Value> {
        match x {
            // No-arg path returns `Value::bool_(false)` directly, skipping
            // `_interp.truthy_value`.  This is equivalent (not incidental):
            // `truthy_value(&Value::none())` would also resolve to False and
            // has no observable side effects, so the shortcut is intentional.
            None => Ok(Value::bool_(false)),
            Some(v) => {
                let result = _interp.truthy_value(&v.0)?;
                Ok(Value::bool_(result))
            }
        }
    }

    /// CPython: dict() — empty dict (rich constructor forms unsupported).
    /// <https://docs.python.org/3/library/functions.html#func-dict>
    fn dict(args) -> Result<Value> {
        // Separate positional and keyword args.
        // CPython: dict([mapping_or_iterable], **kwargs)
        let mut pos_args: Vec<&ExpandedCallArg> = Vec::with_capacity(1);
        let mut kw_pairs: Vec<(String, Value)> = Vec::with_capacity(args.len());
        for a in args {
            match &a.name {
                None => pos_args.push(a),
                Some(n) => kw_pairs.push((n.clone(), a.value.clone())),
            }
        }
        if pos_args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME} takes at most 1 positional argument ({} given)",
                    pos_args.len()
                ),
            ));
        }

        let mut result: PyDict =
            PyDict::with_capacity_and_hasher(kw_pairs.len(), Default::default());

        // Process the optional positional argument.
        if let Some(arg) = pos_args.first() {
            match arg.value.kind() {
                ValueKind::Dict(map) => {
                    result.extend(map.clone());
                }
                ValueKind::BuiltinObject { .. }
                    if pyrust_builtins::mapping_proxy::is_mapping_proxy(&arg.value) =>
                {
                    if let Some(class_rc) =
                        pyrust_builtins::mapping_proxy::as_class_rc(&arg.value)
                    {
                        let class = class_rc.borrow();
                        for (k, v) in class.attrs.iter() {
                            result.insert(PyKey::str_from(k), v.clone());
                        }
                    } else if let Some(dict_rc) =
                        pyrust_builtins::mapping_proxy::as_dict_rc(&arg.value)
                    {
                        // Dict-backed mappingproxy (`d.keys().mapping`, #2679):
                        // copy the parent dict's key/value pairs verbatim.
                        result.extend(dict_rc.borrow().clone());
                    } else if let Some(pairs) =
                        mapping_pairs_via_protocol(_interp, &arg.value)?
                    {
                        for (key, value) in pairs {
                            _interp.dict_insert(&mut result, key, value)?;
                        }
                    }
                }
                // PyInstance with a backing dict (dict subclass).
                ValueKind::PyInstance(inst) => {
                    let inst_rc = Rc::clone(inst);
                    let dict_backing = instance_builtin_data(&inst_rc)
                        .and_then(|backing| backing.as_dict().map(|dict| dict.clone()));
                    if let Some(map) = dict_backing {
                        // PyInstance with a backing dict (dict subclass).
                        result.extend(map);
                    } else if is_dict_subclass_instance(&inst_rc) {
                        // Dict subclasses that keep their mapping in a custom
                        // backing attr rather than `__builtin_data__` — e.g.
                        // `collections.Counter` / `defaultdict` (issue #2010).
                        // CPython's `dict(mapping)` reads via `keys()` +
                        // `__getitem__`; we iterate the keys and subscript via
                        // the class's `__getitem__`.
                        let class = Rc::clone(&inst_rc.borrow().class);
                        let getitem = lookup_class_attr(&class, "__getitem__");
                        let keys = _interp.collect_iterable(&arg.value)?;
                        for k in keys {
                            let v = match getitem.clone() {
                                Some(m) => invoke_class_method(
                                    _interp,
                                    m,
                                    Value::py_instance(Rc::clone(&inst_rc)),
                                    &[ExpandedCallArg { name: None, value: k.clone() }],
                                )?,
                                None => {
                                    return Err(PyError::named(
                                        "TypeError",
                                        format!(
                                            "{FN_NAME}() argument must be a mapping or iterable"
                                        ),
                                    ));
                                }
                            };
                            let key = _interp.value_to_pykey(&k)?;
                            _interp.dict_insert(&mut result, key, v)?;
                        }
                    } else if let Some(pairs) =
                        mapping_pairs_via_protocol(_interp, &arg.value)?
                    {
                        // Any non-dict mapping that follows the duck-typed
                        // protocol (`keys()` + `__getitem__`): `ChainMap`,
                        // `UserDict`, custom mappings (issue #2190).
                        for (key, v) in pairs {
                            _interp.dict_insert(&mut result, key, v)?;
                        }
                    } else {
                        // Treat as iterable of (key, value) pairs.
                        let pairs = _interp.collect_iterable(&arg.value)?;
                        for (idx, pair) in pairs.into_iter().enumerate() {
                            let items = _interp.collect_iterable(&pair).map_err(|e| {
                                // A non-iterable element maps to CPython's
                                // "cannot convert ... to a sequence" TypeError;
                                // an error raised *inside* the element's own
                                // iteration (e.g. a user `__iter__` raising)
                                // propagates unchanged.
                                if is_not_iterable_error(&e) {
                                    PyError::named(
                                        "TypeError",
                                        format!(
                                            "cannot convert dictionary update sequence element #{idx} to a sequence"
                                        ),
                                    )
                                } else {
                                    e
                                }
                            })?;
                            if items.len() != 2 {
                                return Err(PyError::named(
                                    "ValueError",
                                    format!(
                                        "dictionary update sequence element #{idx} has length {}; 2 is required",
                                        items.len()
                                    ),
                                ));
                            }
                            let key = _interp.value_to_pykey(&items[0])?;
                            // #1914: dedup `PyKey::Object` keys via user `__eq__`.
                            _interp.dict_insert(&mut result, key, items[1].clone())?;
                        }
                    }
                }
                _ => {
                    // Treat as iterable of (key, value) pairs.
                    let pairs = _interp.collect_iterable(&arg.value)?;
                    for (idx, pair) in pairs.into_iter().enumerate() {
                        let items = _interp.collect_iterable(&pair).map_err(|e| {
                            // A non-iterable element maps to CPython's
                            // "cannot convert ... to a sequence" TypeError;
                            // an error raised *inside* the element's own
                            // iteration (e.g. a user `__iter__` raising)
                            // propagates unchanged.
                            if is_not_iterable_error(&e) {
                                PyError::named(
                                    "TypeError",
                                    format!(
                                        "cannot convert dictionary update sequence element #{idx} to a sequence"
                                    ),
                                )
                            } else {
                                e
                            }
                        })?;
                        if items.len() != 2 {
                            return Err(PyError::named(
                                "ValueError",
                                format!(
                                    "dictionary update sequence element #{idx} has length {}; 2 is required",
                                    items.len()
                                ),
                            ));
                        }
                        let key = _interp.value_to_pykey(&items[0])?;
                        // #1914: dedup `PyKey::Object` keys via user `__eq__`.
                        _interp.dict_insert(&mut result, key, items[1].clone())?;
                    }
                }
            }
        }

        // Apply keyword arguments.
        for (name, value) in kw_pairs {
            result.insert(PyKey::str_from(&name), value);
        }

        Ok(Value::dict(result))
    }

    /// CPython: input([prompt]) — read a line from stdin, stripping the trailing newline.
    /// <https://docs.python.org/3/library/functions.html#input>
    ///
    /// Accepts 0 or 1 positional argument (the prompt); no keyword arguments.
    /// The prompt (any type — converted to `str`) is printed to stdout without
    /// a trailing newline, with stdout flushed before reading.  Raises
    /// `EOFError` when stdin is at EOF.
    #[arity_style(expected_got)]
    fn input(
        #[positional_only] #[default(OptionalArg::Omitted)] prompt: OptValueRef,
    ) -> Result<Value> {
        // Print the prompt (if any) to stdout without a trailing newline, then flush.
        if let Some(prompt_val) = prompt.as_value() {
            let prompt_str = render_instance_str(_interp, prompt_val)?;
            print!("{}", prompt_str);
            use std::io::Write as _;
            std::io::stdout().flush().ok();
        }
        // Read one line from stdin.
        // CPython raises OSError for real I/O errors and EOFError only for EOF.
        let mut line = String::new();
        let n = std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        if n == 0 {
            return Err(PyError::named(
                "EOFError",
                "EOF when reading a line".to_string(),
            ));
        }
        // CPython strips only the trailing '\n'; it does NOT strip '\r'.
        // On Linux, a \r\n line from stdin should return "hello\r", not "hello".
        if line.ends_with('\n') {
            line.pop();
        }
        Ok(Value::string(line))
    }

    /// CPython: print(*objects, sep=' ', end='\n', file=sys.stdout, flush=False).
    /// <https://docs.python.org/3/library/functions.html#print>
    fn print(args) -> Result<Value> {
        let print_options = _interp.parse_print_options_expanded(args)?;
        let mut rendered = Vec::with_capacity(print_options.values.len());
        for value in &print_options.values {
            rendered.push(render_instance_str(_interp, value)?);
        }
        // No explicit `file=` → CPython prints to the *current* `sys.stdout`.
        // When that has been redirected (e.g. `contextlib.redirect_stdout`),
        // route through the replacement's `write()`; otherwise fall through to
        // the native console fast path below.
        let file = print_options
            .file
            .or_else(|| _interp.redirected_std_stream("stdout"));
        if let Some(file_val) = file {
            // CPython calls file.write() once per item separated by sep,
            // then calls file.write(end), then file.flush() if flush=True.
            let write_fn = _interp.get_attr(&file_val, "write")?;
            let sep = print_options.sep;
            let end = print_options.end;
            for (i, text) in rendered.into_iter().enumerate() {
                if i > 0 {
                    _interp.call_function_expanded(
                        write_fn.clone(),
                        &[ExpandedCallArg { name: None, value: Value::string(sep.clone()) }],
                    )?;
                }
                _interp.call_function_expanded(
                    write_fn.clone(),
                    &[ExpandedCallArg { name: None, value: Value::string(text) }],
                )?;
            }
            _interp.call_function_expanded(
                write_fn,
                &[ExpandedCallArg { name: None, value: Value::string(end) }],
            )?;
            if print_options.flush {
                let flush_fn = _interp.get_attr(&file_val, "flush")?;
                _interp.call_function_expanded(flush_fn, &[])?;
            }
        } else {
            print!("{}{}", rendered.join(&print_options.sep), print_options.end);
            if print_options.flush {
                use std::io::Write as _;
                std::io::stdout().flush().ok();
            }
        }
        Ok(Value::none())
    }

    /// CPython: range(stop) / range(start, stop[, step]).
    /// <https://docs.python.org/3/library/functions.html#func-range>
    #[arity_style(expected_got)]
    fn range(
        #[positional_only] a: PyValueRef,
        #[positional_only] #[default(OptionalArg::Omitted)] b: OptValueRef,
        #[positional_only] #[default(OptionalArg::Omitted)] c: OptValueRef,
    ) -> Result<Value> {
        _interp.call_range_values(a.0, b.as_value(), c.as_value())
    }

    /// CPython: open(file, mode='r', buffering=-1, encoding=None, errors=None,
    /// newline=None, closefd=True, opener=None).
    /// <https://docs.python.org/3/library/functions.html#open>
    ///
    /// First builtin migrated to the typed-signature dialect (#395) — the
    /// macro-emitted prelude rejects unknown kwargs, validates the positional
    /// count, and binds typed Rust locals.  The `encoding`, `buffering`,
    /// `errors`, `newline`, and `closefd` parameters added here to fix #1360.
    fn open(
        path: PyStr,
        #[default("r".into())]
        mode: PyStr,
        #[default(None)]
        buffering: Option<PyValue>,
        #[default(None)]
        encoding: Option<PyStr>,
        #[default(None)]
        errors: Option<PyStr>,
        #[default(None)]
        newline: Option<PyStr>,
        #[default(PyValue(Value::bool_(true)))]
        closefd: PyValue,
    ) -> Result<Value> {
        let _ = buffering; // accepted, not yet implemented (buffering is complex)
        let _ = errors;    // accepted, not yet implemented
        let _ = newline;   // accepted, not yet implemented
        // The default is True, but an explicitly supplied None is an ordinary
        // false value.  Keeping the bound value as PyValue (rather than
        // Option<PyValue>) preserves that omitted-vs-None distinction.
        let closefd_bool = _interp.truthy_value(&closefd.0)?;
        pyrust_builtins::file::open(
            &path,
            &mode,
            encoding.as_deref(),
            closefd_bool,
        )
    }

    /// CPython: format(value[, format_spec]).
    /// <https://docs.python.org/3/library/functions.html#format>
    ///
    /// Migrated to the typed-signature dialect (#400).  Both params are
    /// `#[positional_only]` so the macro emits the fast-path prelude that
    /// skips kwarg validation entirely.  The optional `format_spec` is
    /// encoded as `Option<PyStr>` — absent → `None` (treated as `""`),
    /// present-and-str → `Some(PyStr)`, present-and-non-str → the typed
    /// dialect's standard "must be str or None" TypeError.
    #[arity_style(expected_got)]
    fn format(
        #[positional_only] value: PyValue,
        #[positional_only]
        #[default(PyValue(Value::string("")))]
        format_spec: PyValue,
    ) -> Result<Value> {
        let value = &value.0;
        let spec_owned: String = match format_spec.0.as_str() {
            Some(s) => s.to_owned(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "format() argument 2 must be str, not {}",
                        pyrust_core::py_value_display_name(&format_spec.0),
                    ),
                ));
            }
        };
        let spec: &str = &spec_owned;
        // Delegate to the shared `__format__` dispatcher that f-strings and
        // `str.format` already use (#1370).  It only invokes a *user-defined*
        // `__format__` (skipping the inherited builtin/`object.__format__`),
        // and otherwise extracts the primitive backing and applies the spec.
        // Issue #1935: the previous inline copy here invoked *any* inherited
        // `__format__` — including the builtin one a primitive subclass picks
        // up from its MRO — which rejected a non-empty spec before the
        // backing-extraction branch could run, so `format(MyInt(42), "d")`
        // raised TypeError.  Routing through `dispatch_dunder_format` (which has
        // the `!BuiltinFunction` guard) fixes that and keeps the three format
        // paths in lock-step.
        _interp.dispatch_dunder_format(value, spec)
    }

    /// CPython: classmethod(function) — class-method descriptor.
    /// <https://docs.python.org/3/library/functions.html#classmethod>
    #[arity_style(expected_got)]
    fn classmethod(#[positional_only] function: PyValueRef) -> Result<Value> {
        // CPython 3.12 accepts any object as a classmethod descriptor.
        // When the argument is a UserFunction we use the existing tagged-kind
        // path; for any other value we wrap it in a BuiltinObject descriptor.
        match function.0.kind() {
            ValueKind::UserFunction(f) => Ok(Value::class_method(Rc::clone(f))),
            _ => Ok(pyrust_builtins::classmethod::class_method_any(
                function.0.clone(),
            )),
        }
    }

    /// CPython: staticmethod(function) — static-method descriptor.
    /// <https://docs.python.org/3/library/functions.html#staticmethod>
    #[arity_style(expected_got)]
    fn staticmethod(#[positional_only] function: PyValueRef) -> Result<Value> {
        // CPython 3.12 accepts any object as a staticmethod descriptor.
        // When the argument is a UserFunction we use the existing tagged-kind
        // path; for any other value we wrap it in a BuiltinObject descriptor.
        match function.0.kind() {
            ValueKind::UserFunction(f) => Ok(Value::static_method(Rc::clone(f))),
            _ => Ok(pyrust_builtins::classmethod::static_method_any(
                function.0.clone(),
            )),
        }
    }

    /// CPython: property(fget=None, fset=None, fdel=None, doc=None).
    /// <https://docs.python.org/3/library/functions.html#property>
    #[arity_style(clinic)]
    fn property(
        #[default(OptionalArg::Omitted)] fget: OptValueRef,
        #[default(OptionalArg::Omitted)] fset: OptValueRef,
        #[default(OptionalArg::Omitted)] fdel: OptValueRef,
        #[default(OptionalArg::Omitted)] doc: OptValueRef,
    ) -> Result<Value> {
        let fget = fget.as_value().cloned().unwrap_or_else(Value::none);
        let fset = fset.as_value().cloned().unwrap_or_else(Value::none);
        let fdel = fdel.as_value().cloned().unwrap_or_else(Value::none);
        // doc: store an explicit doc unless it is None (CPython treats
        // `doc=None` as "no explicit doc", falling back to fget's
        // docstring). Issue #1961.
        let doc = doc.as_value().filter(|v| !v.is_none()).cloned();
        Ok(pyrust_builtins::property::property_with_doc(fget, fset, fdel, doc))
    }

    /// CPython: super([type[, object-or-type]]) — proxy object delegating
    /// method calls to a parent or sibling class.
    /// <https://docs.python.org/3/library/functions.html#super>
    ///
    /// The Rust fn is named `super_fn` because `super` is a strict Rust
    /// keyword that is *also* rejected as a raw identifier — `r#super`
    /// won't parse — so the `#[py_name = "super"]` override is the only
    /// way to give this callable its Python-level name.
    #[arity_style(expected_got_parens)]
    #[py_name = "super"]
    fn super_fn(
        #[positional_only] #[default(OptionalArg::Omitted)] t: OptValueRef,
        #[positional_only] #[default(OptionalArg::Omitted)] obj: OptValueRef,
    ) -> Result<Value> {
        let (cls_val, inst_val) = match (t.as_value(), obj.as_value()) {
            (None, _) => {
                // Zero-argument super() — resolve __class__ cell and first param.
                resolve_zero_arg_super(_interp)?
            }
            (Some(cls_val), None) => {
                // One-argument super(cls) — an *unbound* super object that acts as a
                // descriptor (#2704).  `__get__(obj, owner)` binds it to a concrete
                // super(cls, obj).
                let class = match cls_val.kind() {
                    ValueKind::PyClass(c) => Rc::clone(c),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() argument 1 must be a type, not {}",
                            value_type_name_str(cls_val),
                        ),
                    )),
                };
                return Ok(Value::super_proxy_unbound(class));
            }
            (Some(cls_val), Some(obj_val)) if obj_val.is_none() => {
                // CPython 3.12: `super(cls, None)` also constructs an unbound super.
                let class = match cls_val.kind() {
                    ValueKind::PyClass(c) => Rc::clone(c),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() argument 1 must be a type, not {}",
                            value_type_name_str(cls_val),
                        ),
                    )),
                };
                return Ok(Value::super_proxy_unbound(class));
            }
            (Some(cls_val), Some(obj_val)) => (cls_val.clone(), obj_val.clone()),
        };
        let class = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() argument 1 must be a type, not {}",
                    value_type_name_str(&cls_val),
                ),
            )),
        };
        match inst_val.kind() {
            ValueKind::PyInstance(i) => {
                let instance = Rc::clone(i);
                // Bug #199: validate instance is an instance of class.
                if !class_is_subclass_of(&instance.borrow().class, &class) {
                    return Err(PyError::named(
                        "TypeError",
                        "super(type, obj): obj must be an instance or subtype of type".to_string(),
                    ));
                }
                Ok(Value::super_proxy(class, instance))
            }
            ValueKind::PyClass(obj_class) => {
                // Bug #197: classmethod case — second arg is a class.
                let obj_class = Rc::clone(obj_class);
                if class_is_subclass_of(&obj_class, &class) {
                    // Standard case: obj_class is a subclass of class.
                    // e.g. super(Base, Derived) in a classmethod.
                    return Ok(Value::super_proxy_class(class, obj_class));
                }
                // Issue #1385 / #1956: metaclass case — super(Meta, cls) where
                // Meta is a subclass of `type` and `cls` is any class (an
                // "instance" of the metaclass).  In CPython, `type(cls).__mro__`
                // is walked starting after Meta.  We keep `cls` as the proxy's
                // `obj_class` (so e.g. `super().__call__(*a)` binds `cls` as the
                // construction target); env.rs detects the metaclass-method case
                // — Meta is in `type(cls)`'s MRO, not `cls`'s own MRO — and walks
                // the metaclass MRO ([Meta, type, object]) accordingly.
                let type_cls = type_class_singleton();
                if class_is_subclass_of(&class, &type_cls) {
                    return Ok(Value::super_proxy_class(Rc::clone(&class), obj_class));
                }
                Err(PyError::named(
                    "TypeError",
                    "super(type, obj): obj must be an instance or subtype of type".to_string(),
                ))
            }
            _ => Err(PyError::named(
                "TypeError",
                "super(type, obj): obj must be an instance or subtype of type".to_string(),
            )),
        }
    }

    /// CPython: callable(object) — true if the object is callable.
    /// <https://docs.python.org/3/library/functions.html#callable>
    ///
    /// Migrated to the typed-signature dialect (#400).  Mirrors `ascii`
    /// / `id`: a single-body `PyValue` catch-all, since `callable`
    /// accepts every Python object and never raises `TypeError`.
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `callable() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn callable(#[positional_only] obj: PyValue) -> Result<Value> {
        let value = &obj.0;
        let is_callable = match value.kind() {
            ValueKind::UserFunction(_)
            | ValueKind::BuiltinFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. }
            | ValueKind::PyClass(_) => true,
            ValueKind::BuiltinObject { .. } => {
                crate::interpreter::is_builtin_callable_adapter(value)
            }
            ValueKind::PyInstance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                lookup_class_attr(&class, "__call__").is_some()
            }
            _ => false,
        };
        Ok(Value::bool_(is_callable))
    }

    /// CPython: slice(stop) / slice(start, stop[, step]) — construct a slice
    /// object.  Used as both a callable constructor and an `isinstance` target.
    /// <https://docs.python.org/3/library/functions.html#slice>
    #[py_name = "slice.__new__"]
    fn slice_new(args) -> Result<Value> {
        builtin_type_new(_interp, args, BuiltinTypeClass::Slice)
    }

    #[arity_style(expected_got)]
    fn slice(
        #[positional_only] a: PyValueRef,
        #[positional_only] #[default(OptionalArg::Omitted)] b: OptValueRef,
        #[positional_only] #[default(OptionalArg::Omitted)] c: OptValueRef,
    ) -> Result<Value> {
        let (start, stop, step) = match (b.as_value(), c.as_value()) {
            (None, None) => (None, Some(a.0.clone()), None),
            (Some(stop_v), None) => (Some(a.0.clone()), Some(stop_v.clone()), None),
            (Some(stop_v), Some(step_v)) => (
                Some(a.0.clone()),
                Some(stop_v.clone()),
                if step_v.is_none() {
                    None
                } else {
                    Some(step_v.clone())
                },
            ),
            (None, Some(_)) => unreachable!("positional-only slice args cannot have a gap"),
        };
        Ok(pyrust_builtins::slice::make_slice(start, stop, step))
    }

}
