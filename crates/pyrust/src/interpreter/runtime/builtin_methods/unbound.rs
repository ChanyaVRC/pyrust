// Unregistered, type-qualified builtin function descriptors.
//
// Exact Python type/method names and receiver policies intentionally live
// here. The generic callable router delegates after the registry misses.

impl Interpreter {
    /// Resolve a `collections` container class by its bare name from an
    /// already-imported module. This supports type-qualified builtin
    /// descriptors whose sentinel stores only the class name.
    fn collections_class_by_name(&self, name: &str) -> Option<Rc<RefCell<PyClass>>> {
        let module_val = self.module_cache.borrow().get("collections").cloned()?;
        let ValueKind::PyModule(module) = module_val.kind() else {
            return None;
        };
        let attr = module.borrow().attrs.get(name).cloned()?;
        match attr.kind() {
            ValueKind::PyClass(class) => Some(Rc::clone(class)),
            _ => None,
        }
    }

    pub(super) fn call_unregistered_builtin_function(
        &mut self,
        function: &Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        match function.kind() {
            // PEP 654: BaseExceptionGroup.derive / subgroup / split.  These need
            // interpreter access (to call a user predicate or a subclass's
            // overridden `derive`), so they are intercepted here rather than
            // declared as registry builtins.
            ValueKind::BuiltinFunction("BaseExceptionGroup.derive") => {
                self.exception_group_derive(args)
            }
            ValueKind::BuiltinFunction("BaseExceptionGroup.subgroup") => {
                self.exception_group_subgroup_or_split(args, false)
            }
            ValueKind::BuiltinFunction("BaseExceptionGroup.split") => {
                self.exception_group_subgroup_or_split(args, true)
            }
            // Issue #2276: the unbound type-qualified object-level dunders
            // (`str.__hash__` / `int.__repr__` / `str.__str__` /
            // `int.__format__` / …) synthesised by `get_attr_class` for the
            // primitives that override them in CPython.  CPython attributes the
            // receiver guard to the *called* type (not `object`) and dispatches
            // through the type's own slot; emulate that by validating the
            // receiver here (slot-wrapper wording for `__hash__`/`__repr__`/
            // `__str__`, method_descriptor wording for `__format__`) and then
            // delegating to the shared `object.__X__` body / `apply_format_spec`
            // — the implementations that already render a bare primitive and a
            // subclass `PyInstance` correctly.
            ValueKind::BuiltinFunction(name)
                if name.split_once('.').is_some_and(|(_, m)| {
                    matches!(m, "__hash__" | "__repr__" | "__str__" | "__format__")
                }) && primitive_object_dunder_owner(name).is_some() =>
            {
                let (type_name, method) = name.split_once('.').unwrap();
                self.call_primitive_object_dunder(type_name, method, args)
            }
            // `float.fromhex` is a classmethod: the first positional arg is the
            // string to parse.  It must be dispatched before the generic
            // `"float.*"` arm below so that the arg-0-is-receiver assumption
            // in that arm is not applied here.
            ValueKind::BuiltinFunction("float.fromhex") => {
                // Issue #2767: `float.fromhex` takes no keyword arguments; the
                // kwarg must be rejected before the string is parsed (CPython
                // raises TypeError even for a garbage/absent string arg).
                if args.iter().any(|a| a.name.is_some()) {
                    return Err(pyrust_core::type_err!(
                        "float.fromhex() takes no keyword arguments"
                    ));
                }
                // Accept both `float.fromhex(s)` and `(1.0).fromhex(s)`.
                // Filter out a leading float or class receiver if present, then
                // enforce exactly one remaining positional argument.
                let positional_args: Vec<_> = args
                    .iter()
                    .filter(|a| {
                        a.name.is_none()
                            && !matches!(
                                a.value.kind(),
                                ValueKind::Float(_) | ValueKind::PyClass(_)
                            )
                    })
                    .collect();
                let n_payload = positional_args.len();
                if n_payload == 0 {
                    return Err(pyrust_core::type_err!(
                        "float.fromhex() takes exactly one argument (0 given)"
                    ));
                }
                if n_payload > 1 {
                    return Err(pyrust_core::type_err!(
                        "float.fromhex() takes exactly one argument ({} given)",
                        n_payload
                    ));
                }
                let s_val = positional_args
                    .first()
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        pyrust_core::type_err!(
                            "float.fromhex() takes exactly one argument (0 given)"
                        )
                    })?;
                let s = match s_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        return Err(pyrust_core::type_err!(
                            "bad argument type for built-in operation"
                        ));
                    }
                };
                pyrust_builtins::float::fromhex(&s).map(Value::float)
            }
            // Issue #1413: generator type descriptor methods.
            // `type(gen).__iter__(g)`, `type(gen).__next__(g)`, etc.
            // These are unbound BuiltinFunction sentinels returned by
            // get_attr on BuiltinFunction("generator").  Dispatch to
            // call_generator_method with args[0] as the receiver.
            ValueKind::BuiltinFunction(name)
                if name.split_once('.').is_some_and(|(t, _)| t == "generator") =>
            {
                let (_, method) = name.split_once('.').unwrap();
                // `__next__`/`__iter__` are slot wrappers; `send`/`close`/`throw`
                // are method_descriptors — distinct receiver-guard wording (#2266).
                let is_dunder = method.starts_with("__") && method.ends_with("__");
                let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
                    if is_dunder {
                        pyrust_core::descriptor_needs_arg!(method, "generator")
                    } else {
                        pyrust_core::descriptor_needs_arg!(method, "generator", method)
                    }
                })?;
                if !matches!(self_val.kind(), ValueKind::Generator(_)) {
                    let actual = pyrust_core::builtin_type_name(&self_val);
                    return Err(if is_dunder {
                        pyrust_core::descriptor_requires!(method, "generator", actual)
                    } else {
                        pyrust_core::descriptor_requires!(method, "generator", actual, method)
                    });
                }
                let pos: Vec<Value> = args[1..]
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                self.call_generator_method(self_val, method, pos)
            }
            // float instance methods via descriptor call: `float.is_integer(x)`.
            // The float call fn takes an `f64` receiver directly, so this arm
            // is separate from the generic str/list/… arm below.
            ValueKind::BuiltinFunction(name)
                if name.split_once('.').is_some_and(|(t, _)| t == "float") =>
            {
                let (_, method) = name.split_once('.').unwrap();
                // Issue #2760: `__getnewargs__` takes no keyword arguments; the
                // unbound float arm below drops kwargs silently otherwise.
                if method == "__getnewargs__" && args.iter().any(|a| a.name.is_some()) {
                    return Err(pyrust_core::type_err!(
                        "float.__getnewargs__() takes no keyword arguments"
                    ));
                }
                let self_val = args
                    .first()
                    .map(|a| a.value.clone())
                    .ok_or_else(|| pyrust_core::descriptor_needs_arg!(method, "float", method))?;
                let f = match self_val.kind() {
                    ValueKind::Float(f) => f,
                    _ => {
                        let actual = pyrust_core::builtin_type_name(&self_val);
                        return Err(pyrust_core::type_err!(
                            "descriptor '{method}' for 'float' objects doesn't apply to a '{actual}' object",
                        ));
                    }
                };
                // Issue #2767: float method_descriptors take no keyword
                // arguments; the receiver-only `float::call` discards them, so
                // guard here before delegating.
                if args[1..].iter().any(|a| a.name.is_some()) {
                    return Err(pyrust_core::type_err!(
                        "float.{method}() takes no keyword arguments"
                    ));
                }
                let pos: Vec<Value> = args[1..]
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                pyrust_builtins::float::call(method, f, &pos)
            }
            // PEP 585: `__class_getitem__` classmethods on built-in collection
            // types.  Sentinel names follow the pattern
            // `"<type>.__class_getitem__"` (declared by provider metadata and
            // materialized by primitive-class bootstrap).
            // This arm handles the direct-call path:
            //   `list.__class_getitem__(int)` → GenericAlias(list, (int,))
            // The `list[int]` subscript path is handled directly in
            // `eval_index` without passing through here.
            // This arm must come before the instance-method arm below because
            // `"list.__class_getitem__"` would otherwise match that arm's
            // `"list"` prefix guard and raise a spurious descriptor TypeError.
            ValueKind::BuiltinFunction(name)
                if name
                    .split_once('.')
                    .is_some_and(|(_, m)| m == "__class_getitem__") =>
            {
                let type_name = name.split_once('.').unwrap().0;
                // Recover the origin class value.  Built-in primitives
                // (`list`/`dict`/…) come from the per-thread singleton; the
                // `collections` container classes (issue #2603) are looked up
                // on the already-imported `collections` module by name, since
                // their sentinel carries only the bare class name.
                let origin_class = primitive_class_by_name(type_name)
                    .or_else(|| self.collections_class_by_name(type_name))
                    .ok_or_else(|| {
                        PyError::Runtime(format!(
                            "internal: unknown class for __class_getitem__: {type_name}"
                        ))
                    })?;
                // Accept one positional argument: the type parameter(s).
                //   `list.__class_getitem__(int)`       → args[0] = int
                //   `list.__class_getitem__((str, int))` → args[0] = (str, int)
                let index = args
                    .iter()
                    .find(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        pyrust_core::type_err!(
                            "descriptor '__class_getitem__' of '{type_name}' object \
                                 needs an argument"
                        )
                    })?;
                let is_tuple = matches!(index.kind(), ValueKind::Tuple(_));
                let type_args = if is_tuple {
                    index
                } else {
                    Value::tuple(vec![index])
                };
                Ok(pyrust_builtins::generic_alias::generic_alias(
                    Value::py_class(origin_class),
                    type_args,
                ))
            }
            // `bytes.fromhex` is a classmethod: the first positional arg is the
            // hex string to decode.  Must appear before the generic `bytes.*`
            // arm so that the arg-0-is-receiver assumption there is not applied.
            ValueKind::BuiltinFunction("bytes.fromhex") => {
                pyrust_builtins::bytes::validate_method_keywords(
                    "fromhex",
                    args.iter().any(|arg| arg.name.is_some()),
                )?;
                // Strip any leading bytes receiver or PyClass arg (from instance
                // calls like `b''.fromhex(s)` or `bytes.fromhex(s)`), then
                // enforce exactly one remaining positional argument.
                let positional_args: Vec<_> = args
                    .iter()
                    .filter(|a| {
                        a.name.is_none()
                            && !matches!(
                                a.value.kind(),
                                ValueKind::Bytes(_) | ValueKind::PyClass(_)
                            )
                    })
                    .collect();
                let n_payload = positional_args.len();
                if n_payload == 0 {
                    return Err(pyrust_core::type_err!(
                        "bytes.fromhex() takes exactly one argument (0 given)"
                    ));
                }
                if n_payload > 1 {
                    return Err(pyrust_core::type_err!(
                        "bytes.fromhex() takes exactly one argument ({n_payload} given)"
                    ));
                }
                let s_val = positional_args
                    .first()
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        pyrust_core::type_err!(
                            "bytes.fromhex() takes exactly one argument (0 given)"
                        )
                    })?;
                let s = match s_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        return Err(pyrust_core::type_err!(
                            "fromhex() argument must be str, not {}",
                            pyrust_core::builtin_type_name(&s_val)
                        ));
                    }
                };
                pyrust_builtins::bytes::bytes_fromhex(&s).map(Value::bytes)
            }
            // `bytearray.fromhex` is a classmethod, same pattern as `bytes.fromhex`.
            ValueKind::BuiltinFunction("bytearray.fromhex") => {
                pyrust_builtins::bytearray::validate_method_keywords(
                    "fromhex",
                    args.iter().any(|arg| arg.name.is_some()),
                )?;
                let positional_args: Vec<_> = args
                    .iter()
                    .filter(|a| {
                        a.name.is_none()
                            && !matches!(
                                a.value.kind(),
                                ValueKind::BuiltinObject { ops, .. }
                                    if ops.canonical_class_tag()
                                        == Some(pyrust_core::CanonicalClassTag::Bytearray)
                            )
                            && !matches!(a.value.kind(), ValueKind::PyClass(_))
                    })
                    .collect();
                let n_payload = positional_args.len();
                if n_payload == 0 {
                    return Err(pyrust_core::type_err!(
                        "bytearray.fromhex() takes exactly one argument (0 given)"
                    ));
                }
                if n_payload > 1 {
                    return Err(pyrust_core::type_err!(
                        "bytearray.fromhex() takes exactly one argument ({n_payload} given)"
                    ));
                }
                let s_val = positional_args
                    .first()
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        pyrust_core::type_err!(
                            "bytearray.fromhex() takes exactly one argument (0 given)"
                        )
                    })?;
                let s = match s_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        return Err(pyrust_core::type_err!(
                            "fromhex() argument must be str, not {}",
                            pyrust_core::builtin_type_name(&s_val)
                        ));
                    }
                };
                pyrust_builtins::bytes::bytes_fromhex(&s).map(pyrust_builtins::bytearray::bytearray)
            }
            // `bytes.maketrans` is a staticmethod: args contains only the two
            // from/to bytes arguments (no implicit receiver).  Both `bytes.maketrans(f, t)`
            // and `b''.maketrans(f, t)` resolve to the same unbound BuiltinFunction,
            // so args is always exactly [from, to] without a prepended receiver.
            // Must appear before the generic `bytes.*` arm, which expects args[0]
            // to be the bytes receiver.
            ValueKind::BuiltinFunction("bytes.maketrans") => {
                pyrust_builtins::bytes::validate_method_keywords(
                    "maketrans",
                    args.iter().any(|arg| arg.name.is_some()),
                )?;
                // Coerce bytes/bytearray subclass instances to their backing
                // bytes-like value so `bytes_maketrans` accepts them (#2677).
                let positional: Vec<Value> = args
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                pyrust_builtins::bytes::validate_method_positional_arity(
                    "maketrans",
                    positional.len(),
                )?;
                let positional: Vec<Value> = positional
                    .into_iter()
                    .map(coerce_bytes_subclass_arg)
                    .collect();
                pyrust_builtins::bytes::bytes_maketrans(&positional)
            }
            // `bytearray.maketrans` is a staticmethod that returns `bytes`,
            // identical to `bytes.maketrans`. Same arg-handling pattern: no
            // implicit receiver, delegate to the shared `bytes_maketrans` impl.
            ValueKind::BuiltinFunction("bytearray.maketrans") => {
                pyrust_builtins::bytearray::validate_method_keywords(
                    "maketrans",
                    args.iter().any(|arg| arg.name.is_some()),
                )?;
                let positional: Vec<Value> = args
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                pyrust_builtins::bytearray::validate_method_positional_arity(
                    "maketrans",
                    positional.len(),
                )?;
                let positional: Vec<Value> = positional
                    .into_iter()
                    .map(coerce_bytes_subclass_arg)
                    .collect();
                pyrust_builtins::bytes::bytes_maketrans(&positional)
            }
            // `str.maketrans` is a staticmethod: same pattern as `bytes.maketrans`.
            // Must appear before the generic `str.*` arm.
            ValueKind::BuiltinFunction("str.maketrans") => {
                pyrust_builtins::string::validate_method_keywords(
                    "maketrans",
                    args.iter().any(|arg| arg.name.is_some()),
                )?;
                let positional: Vec<Value> = args
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                pyrust_builtins::string::validate_method_positional_arity(
                    "maketrans",
                    positional.len(),
                )?;
                pyrust_builtins::string::str_maketrans(&positional)
            }
            // `int.from_bytes` is a classmethod.  Must appear before the generic
            // `int.*` arm so that the arg-0-is-receiver assumption there is not
            // applied here.  Both `int.from_bytes(b, 'big')` and
            // `(5).from_bytes(b, 'big')` bind the canonical class through the
            // explicit native-classmethod descriptor. Strip that typed receiver
            // before the interpreter-aware source conversion.
            ValueKind::BuiltinFunction("int.from_bytes") => {
                let mut pos: Vec<Value> = args
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                if pos
                    .first()
                    .is_some_and(|value| matches!(value.kind(), ValueKind::PyClass(_)))
                {
                    pos.remove(0);
                }
                let mut kw: PyDict = PyDict::default();
                for a in args {
                    if let Some(name) = &a.name {
                        kw.insert(PyKey::str_from(name.as_str()), a.value.clone());
                    }
                }
                // The `bytes` source may be any bytes-like object (bytes,
                // bytearray, memoryview) or any iterable of ints in 0..=255.
                // Resolve it to concrete `bytes` here — where the interpreter is
                // available to drive user `__iter__` — before the receiver-only
                // `int_from_bytes` decodes the big-endian/little-endian integer.
                self.resolve_from_bytes_source(&mut pos, &mut kw)?;
                pyrust_builtins::int::int_from_bytes(&pos, &kw)
            }
            // #462: class-method-of-primitive dispatch.  When a primitive
            // class's provider metadata materializes an ordinary
            // `BuiltinFunction("<type>.<method>")`, calling it dispatches like
            // a bound method with `args[0]` as the receiver. Mirrors the bound
            // arm above so `str.upper(s)` and `s.upper()` share a per-type
            // `call` fn. `str.format` is handled by the preceding arm because
            // it threads kwargs into the template.
            ValueKind::BuiltinFunction(name)
                if name.split_once('.').is_some_and(|(t, _)| {
                    matches!(
                        t,
                        "int"
                            | "bool"
                            | "bytes"
                            | "bytearray"
                            | "str"
                            | "list"
                            | "tuple"
                            | "dict"
                            | "set"
                            | "complex"
                            | "frozenset"
                    )
                }) =>
            {
                let (type_name, method) = name.split_once('.').unwrap();
                // CPython exposes most dunders (`str.__getitem__`, `list.__add__`,
                // …) as *slot wrappers* and regular methods (`str.upper`, …) as
                // *method_descriptors*; the two raise differently worded receiver
                // guards.  See issue #2266.  A handful of dunders are *per-type*
                // method_descriptors rather than slot wrappers — e.g.
                // `dict`/`set`/`frozenset`.__contains__ (but `str`/`list`/`tuple`/
                // `bytes`.__contains__ stay slot wrappers).  Treat those as
                // method_descriptors so they get the "unbound method …" / "doesn't
                // apply to …" wording.
                let is_method_descriptor_dunder = matches!(
                    (type_name, method),
                    ("dict" | "set" | "frozenset", "__contains__")
                        // Issue #2297: `int.__round__`/`__trunc__`/`__floor__`/
                        // `__ceil__` are method_descriptors (the "doesn't apply
                        // to" receiver-guard wording); `int.__index__` stays a
                        // slot wrapper.
                        | ("int", "__round__" | "__trunc__" | "__floor__" | "__ceil__")
                        // Issue #2760: `__getnewargs__` is a method_descriptor on
                        // every numeric/immutable-sequence primitive, so its
                        // receiver-guard uses the "doesn't apply to" wording.
                        | (
                            "int" | "bool" | "float" | "complex" | "str" | "bytes" | "tuple",
                            "__getnewargs__"
                        )
                );
                let is_dunder = method.starts_with("__")
                    && method.ends_with("__")
                    && !is_method_descriptor_dunder;
                let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
                    if is_dunder {
                        pyrust_core::descriptor_needs_arg!(method, type_name)
                    } else {
                        pyrust_core::descriptor_needs_arg!(method, type_name, method)
                    }
                })?;
                let mut pos: Vec<Value> = Vec::with_capacity(args.len().saturating_sub(1));
                let mut kw: PyDict = PyDict::default();
                for a in &args[1..] {
                    match &a.name {
                        Some(n) => {
                            kw.insert(PyKey::str_from(n.as_str()), a.value.clone());
                        }
                        None => pos.push(a.value.clone()),
                    }
                }
                // Issue #976/#994: if `self_val` is a PyInstance with a
                // `__builtin_data__` backing value (set at construction for
                // subclasses of dict/list/set/frozenset/tuple), use that
                // backing value as the effective receiver so the kind_ok
                // check and dispatch below see the expected primitive type.
                // Issue #1204: same for str/int/float/bytes subclasses.
                // Whether the ORIGINAL receiver was an OrderedDict (or
                // subclass) instance — the view-constructing dict methods
                // need this AFTER backing normalisation erases it (#2436:
                // the bound `dict.keys` route lost the odict tag).
                let mut receiver_ordered = false;
                let self_val = if matches!(
                    type_name,
                    "dict"
                        | "list"
                        | "set"
                        | "frozenset"
                        | "tuple"
                        | "str"
                        | "int"
                        | "float"
                        | "bytes"
                        | "bytearray"
                        | "complex"
                ) {
                    // Extract the Rc before kind() drops its borrow.
                    let maybe_inst = if let ValueKind::PyInstance(inst) = self_val.kind() {
                        Some(Rc::clone(inst))
                    } else {
                        None
                    };
                    if let Some(inst) = maybe_inst {
                        receiver_ordered =
                            is_ordered_dict_class_or_subclass(&Rc::clone(&inst.borrow().class));
                        builtin_data_backing(&self_val).unwrap_or(self_val)
                    } else {
                        self_val
                    }
                } else {
                    self_val
                };
                // Receiver-type guard: validate `self_val`'s kind matches
                // `type_name` before dispatch — otherwise the per-type call fn
                // surfaces an internal `"expected dict"` / `"receiver is not a
                // list"` runtime error instead of the descriptor TypeError that
                // CPython raises.  See Copilot review on #463.
                let kind_ok = match (type_name, self_val.kind()) {
                    ("int", ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)) => true,
                    // Issue #2424: `bool.__and__(True, False)` — the bool-owned
                    // slot wrappers only accept a `bool` receiver in CPython.
                    ("bool", ValueKind::Bool(_)) => true,
                    ("bytes", ValueKind::Bytes(_)) => true,
                    // Issue #2770: `bytearray` is a `BuiltinObject`, not a
                    // dedicated `ValueKind`; gate on its ops `type_name` so the
                    // unbound form (`bytearray.replace(ba, …)`) accepts a real
                    // bytearray receiver and rejects everything else with the
                    // descriptor TypeError.
                    ("bytearray", ValueKind::BuiltinObject { ops, .. })
                        if ops.canonical_class_tag()
                            == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
                    {
                        true
                    }
                    ("str", ValueKind::Str(_)) => true,
                    ("list", ValueKind::List(_)) => true,
                    ("tuple", ValueKind::Tuple(_)) => true,
                    ("dict", ValueKind::Dict(_)) => true,
                    ("set", ValueKind::Set(_)) => true,
                    ("complex", ValueKind::Complex(_, _)) => true,
                    ("frozenset", ValueKind::BuiltinObject { ops, .. })
                        if ops.canonical_class_tag()
                            == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
                    {
                        true
                    }
                    _ => false,
                };
                if !kind_ok {
                    let actual = pyrust_core::builtin_type_name(&self_val);
                    return Err(if is_dunder {
                        pyrust_core::descriptor_requires!(method, type_name, actual)
                    } else {
                        pyrust_core::descriptor_requires!(method, type_name, actual, method)
                    });
                }
                // Issue #2387 / #1909: protocol dunders (`__iter__`, `__add__`,
                // `__mod__`, `__getitem__`, …) called type-level with a
                // builtin-*subclass* receiver (`list.__iter__(LI([1]))`,
                // `list.__add__(LI([1]), [2])`) reach this arm because the
                // non-subclass form was already intercepted upstream.  Route the
                // unwrapped backing through the shared dispatcher (the per-type
                // `call` below has no body for several of these slots and would
                // leak a RuntimeError).
                if method.starts_with("__") && is_protocol_dunder(type_name, method) {
                    if !kw.is_empty() {
                        // Issue #2398: named method-wrapper vs anonymous slot
                        // wrapper keyword-rejection wording (issue #2291).
                        return Err(if is_named_protocol_wrapper(method, type_name) {
                            pyrust_core::type_err!(
                                "{type_name}.{method}() takes no keyword arguments"
                            )
                        } else {
                            pyrust_core::type_err!("wrapper {method}() takes no keyword arguments")
                        });
                    }
                    return self.dispatch_builtin_protocol_dunder(method, self_val, pos);
                }

                // Exact container methods have one interpreter adapter shared
                // with bound methods and both CallMethod opcodes.  The generic
                // callable router validates the descriptor receiver above, then
                // hands off without owning any list/dict/tuple/str/set names.
                if let Some(kind) = BuiltinContainerKind::from_type_name(type_name)
                    .filter(|kind| kind.supports_direct_method(method))
                {
                    return self.dispatch_builtin_container_method(
                        kind,
                        self_val,
                        method,
                        pos,
                        &kw,
                        receiver_ordered,
                    );
                }

                match type_name {
                    "int" => {
                        self.resolve_to_bytes_length(method, &mut pos, &mut kw)?;
                        pyrust_builtins::int::call(method, &self_val, &pos, &kw)
                    }
                    "bytes" => {
                        // Accept bytes-subclass / bytearray args (#1928);
                        // partition/rpartition echo the original separator
                        // object as the middle element (#2680).
                        self.call_bytes_method_with_protocols(method, &self_val, pos, &kw)
                    }
                    "complex" => {
                        // Issue #2760: `__getnewargs__` takes no keyword
                        // arguments; the receiver-only `complex::call` discards
                        // `kw` otherwise.
                        if method == "__getnewargs__" && !kw.is_empty() {
                            return Err(pyrust_core::type_err!(
                                "complex.__getnewargs__() takes no keyword arguments"
                            ));
                        }
                        // Issue #2767: complex methods take no keyword
                        // arguments; the receiver-only `complex::call` discards
                        // `kw`, so guard here before delegating.
                        if !kw.is_empty() {
                            return Err(pyrust_core::type_err!(
                                "complex.{method}() takes no keyword arguments"
                            ));
                        }
                        pyrust_builtins::complex::call(method, &self_val, pos)
                    }
                    "frozenset" => {
                        pyrust_builtins::frozenset::validate_method_keywords(
                            method,
                            !kw.is_empty(),
                        )?;
                        self.call_frozenset_method(method, self_val, pos)
                    }
                    // Issue #2770: the unbound `bytearray.<method>(ba, …)` form.
                    // `bytearray` is a `BuiltinObject`, so dispatch through its
                    // ops table exactly like the bound `ba.<method>(…)` path
                    // (kwarg rejection + bytes-subclass arg coercion + lazy
                    // iterator driving for `join`/`extend`).
                    "bytearray" => {
                        pyrust_builtins::bytearray::validate_method_keywords(
                            method,
                            !kw.is_empty(),
                        )?;
                        let mut args_vec = pos;
                        let splitlines = method == "splitlines";
                        if let Some(bound) =
                            self.bind_bytearray_splitlines_keepends(method, &args_vec, &kw)?
                        {
                            args_vec = bound;
                        }
                        if method == "join" {
                            args_vec = self.prepare_bytearray_join_args(args_vec)?;
                        } else {
                            args_vec = coerce_bytes_subclass_method_args(method, args_vec);
                        }
                        if method == "extend" {
                            args_vec = self.prepare_bytearray_extend_args(args_vec)?;
                        }
                        let empty_kw = PyDict::default();
                        let coerced_kw = if splitlines {
                            None
                        } else {
                            coerce_bytes_subclass_method_kwargs(&kw)
                        };
                        let kw = if splitlines {
                            &empty_kw
                        } else {
                            coerced_kw.as_ref().unwrap_or(&kw)
                        };
                        let kw_str: indexmap::IndexMap<String, Value> = kw
                            .iter()
                            .map(|(k, v)| {
                                let key = match k {
                                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                                    _ => String::new(),
                                };
                                (key, v.clone())
                            })
                            .collect();
                        match self_val.kind() {
                            ValueKind::BuiltinObject { ops, state } => {
                                ops.call_method(state, method, args_vec, &kw_str)
                            }
                            _ => unreachable!("kind_ok guard above"),
                        }
                    }
                    _ => unreachable!("guard matched type_name above"),
                }
            }
            _ => Err(pyrust_core::type_err!(
                "'{}' object is not callable",
                pyrust_core::builtin_type_name(function)
            )),
        }
    }
}
