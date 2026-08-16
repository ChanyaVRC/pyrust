use pyrust_derive::pyrust_module;

/// Whether `reversed(value)` will delegate to a value-owned `__reversed__`
/// implementation.  `reversed.__new__` records this provenance before calling
/// user code: the special method's result must pass through unchanged even
/// when it happens to be an exact generic `reversed` object.
pub(super) fn reversed_uses_special_method(value: &Value) -> bool {
    match value.kind() {
        ValueKind::PyInstance(instance) => {
            let class = Rc::clone(&instance.borrow().class);
            lookup_class_attr(&class, "__reversed__").is_some()
        }
        ValueKind::BuiltinObject { ops, .. } => ops.has_method("__reversed__"),
        _ => false,
    }
}

/// Validate an unbound iterator slot receiver and return its exact built-in
/// backing.  Subclass instances carry that backing in `__builtin_data__`;
/// exact objects are already generator-backed values.
fn builtin_iterator_backing(
    args: &[ExpandedCallArg],
    kind: BuiltinTypeClass,
    method: &str,
    expected_args: usize,
) -> Result<(Value, Value)> {
    let Some((receiver_arg, rest)) = args
        .split_first()
        .filter(|(receiver_arg, _)| receiver_arg.name.is_none())
    else {
        if matches!(kind, BuiltinTypeClass::Reversed)
            && matches!(method, "__length_hint__" | "__reduce__" | "__setstate__")
        {
            return Err(pyrust_core::type_err!(
                "unbound method reversed.{method}() needs an argument"
            ));
        }
        return Err(pyrust_core::type_err!(
            "descriptor '{method}' of '{}' object needs an argument",
            kind.class_name()
        ));
    };
    let receiver = receiver_arg.value.clone();
    let backing = if let ValueKind::PyInstance(instance) = receiver.kind() {
        let class = Rc::clone(&instance.borrow().class);
        let base = kind.singleton();
        if class_is_subclass_of(&class, &base)
            && let Some(backing) = instance_builtin_data(instance)
            && builtin_type_class_isinstance_fast(&backing, &base) == Some(true)
        {
            Some(backing)
        } else {
            None
        }
    } else {
        let base = kind.singleton();
        if builtin_type_class_isinstance_fast(&receiver, &base) == Some(true) {
            Some(receiver.clone())
        } else {
            None
        }
    };

    let Some(backing) = backing else {
        return Err(
            if !matches!(method, "__length_hint__" | "__reduce__" | "__setstate__") {
                pyrust_core::type_err!(
                    "descriptor '{method}' requires a '{}' object but received a '{}'",
                    kind.class_name(),
                    full_type_name_str(&receiver)
                )
            } else {
                pyrust_core::type_err!(
                    "descriptor '{method}' for '{}' objects doesn't apply to a '{}' object",
                    kind.class_name(),
                    full_type_name_str(&receiver)
                )
            },
        );
    };
    if rest.iter().any(|arg| arg.name.is_some()) {
        return Err(pyrust_core::type_err!(
            "{}.{method}() takes no keyword arguments",
            kind.class_name()
        ));
    }
    if rest.len() != expected_args {
        if matches!(kind, BuiltinTypeClass::Reversed) {
            return Err(match method {
                "__length_hint__" | "__reduce__" => pyrust_core::type_err!(
                    "reversed.{method}() takes no arguments ({} given)",
                    rest.len()
                ),
                "__setstate__" => pyrust_core::type_err!(
                    "reversed.__setstate__() takes exactly one argument ({} given)",
                    rest.len()
                ),
                "__getattribute__" => {
                    pyrust_core::type_err!("expected 1 argument, got {}", rest.len())
                }
                _ => {
                    pyrust_core::type_err!("expected {expected_args} arguments, got {}", rest.len())
                }
            });
        }
        return Err(pyrust_core::type_err!(
            "expected {expected_args} arguments, got {}",
            rest.len()
        ));
    }
    Ok((receiver, backing))
}

fn builtin_iterator_iter(args: &[ExpandedCallArg], kind: BuiltinTypeClass) -> Result<Value> {
    let (receiver, _) = builtin_iterator_backing(args, kind, "__iter__", 0)?;
    Ok(receiver)
}

fn builtin_iterator_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    kind: BuiltinTypeClass,
) -> Result<Value> {
    let (_, backing) = builtin_iterator_backing(args, kind, "__next__", 0)?;
    interp.call_next(&backing, None)
}

pyrust_module! {
    #[py_name = "bytearray_iterator.__getattribute__"]
    fn bytearray_iterator_getattribute(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::Bytearray,
            "__getattribute__",
        )
    }

    #[py_name = "bytearray_iterator.__iter__"]
    fn bytearray_iterator_iter(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::Bytearray,
            "__iter__",
        )
    }

    #[py_name = "bytearray_iterator.__next__"]
    fn bytearray_iterator_next(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::Bytearray,
            "__next__",
        )
    }

    #[py_name = "bytearray_iterator.__length_hint__"]
    fn bytearray_iterator_length_hint(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::Bytearray,
            "__length_hint__",
        )
    }

    #[py_name = "bytearray_iterator.__reduce__"]
    fn bytearray_iterator_reduce(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::Bytearray,
            "__reduce__",
        )
    }

    #[py_name = "bytearray_iterator.__setstate__"]
    fn bytearray_iterator_setstate(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::Bytearray,
            "__setstate__",
        )
    }

    #[py_name = "zip.__new__"]
    fn zip_new(args) -> Result<Value> {
        builtin_type_new(_interp, args, BuiltinTypeClass::Zip)
    }

    #[py_name = "zip.__iter__"]
    fn zip_iter(args) -> Result<Value> {
        builtin_iterator_iter(args, BuiltinTypeClass::Zip)
    }

    #[py_name = "zip.__next__"]
    fn zip_next(args) -> Result<Value> {
        builtin_iterator_next(_interp, args, BuiltinTypeClass::Zip)
    }

    #[py_name = "map.__new__"]
    fn map_new(args) -> Result<Value> {
        builtin_type_new(_interp, args, BuiltinTypeClass::Map)
    }

    #[py_name = "map.__iter__"]
    fn map_iter(args) -> Result<Value> {
        builtin_iterator_iter(args, BuiltinTypeClass::Map)
    }

    #[py_name = "map.__next__"]
    fn map_next(args) -> Result<Value> {
        builtin_iterator_next(_interp, args, BuiltinTypeClass::Map)
    }

    #[py_name = "filter.__new__"]
    fn filter_new(args) -> Result<Value> {
        builtin_type_new(_interp, args, BuiltinTypeClass::Filter)
    }

    #[py_name = "filter.__iter__"]
    fn filter_iter(args) -> Result<Value> {
        builtin_iterator_iter(args, BuiltinTypeClass::Filter)
    }

    #[py_name = "filter.__next__"]
    fn filter_next(args) -> Result<Value> {
        builtin_iterator_next(_interp, args, BuiltinTypeClass::Filter)
    }

    #[py_name = "enumerate.__new__"]
    fn enumerate_new(args) -> Result<Value> {
        builtin_type_new(_interp, args, BuiltinTypeClass::Enumerate)
    }

    #[py_name = "enumerate.__iter__"]
    fn enumerate_iter(args) -> Result<Value> {
        builtin_iterator_iter(args, BuiltinTypeClass::Enumerate)
    }

    #[py_name = "enumerate.__next__"]
    fn enumerate_next(args) -> Result<Value> {
        builtin_iterator_next(_interp, args, BuiltinTypeClass::Enumerate)
    }

    #[py_name = "reversed.__new__"]
    fn reversed_new(args) -> Result<Value> {
        builtin_type_new(_interp, args, BuiltinTypeClass::Reversed)
    }

    #[py_name = "reversed.__iter__"]
    fn reversed_iter(args) -> Result<Value> {
        builtin_iterator_iter(args, BuiltinTypeClass::Reversed)
    }

    #[py_name = "reversed.__next__"]
    fn reversed_next(args) -> Result<Value> {
        builtin_iterator_next(_interp, args, BuiltinTypeClass::Reversed)
    }

    #[py_name = "reversed.__getattribute__"]
    fn reversed_getattribute(args) -> Result<Value> {
        let (receiver, _) = builtin_iterator_backing(
            args,
            BuiltinTypeClass::Reversed,
            "__getattribute__",
            1,
        )?;
        let name = args[1].value.as_str().ok_or_else(|| {
            pyrust_core::type_err!(
                "attribute name must be string, not '{}'",
                full_type_name_str(&args[1].value)
            )
        })?;
        match receiver.kind() {
            ValueKind::PyInstance(instance) => {
                _interp.get_attr_instance_raw(Rc::clone(instance), name)
            }
            _ => _interp.get_attr(&receiver, name),
        }
    }

    #[py_name = "reversed.__length_hint__"]
    fn reversed_length_hint(args) -> Result<Value> {
        let (_, backing) = builtin_iterator_backing(
            args,
            BuiltinTypeClass::Reversed,
            "__length_hint__",
            0,
        )?;
        _interp.call_generator_method(backing, "__length_hint__", Vec::new())
    }

    #[py_name = "reversed.__reduce__"]
    fn reversed_reduce(args) -> Result<Value> {
        let (receiver, _) = builtin_iterator_backing(
            args,
            BuiltinTypeClass::Reversed,
            "__reduce__",
            0,
        )?;
        reversed_iterator_reduce(&receiver)
    }

    #[py_name = "reversed.__setstate__"]
    fn reversed_setstate(args) -> Result<Value> {
        let (_, backing) = builtin_iterator_backing(
            args,
            BuiltinTypeClass::Reversed,
            "__setstate__",
            1,
        )?;
        _interp.call_generator_method(
            backing,
            "__setstate__",
            vec![args[1].value.clone()],
        )
    }

    /// CPython: enumerate(iterable, start=0) — enumerate iterator.
    /// <https://docs.python.org/3/library/functions.html#enumerate>
    ///
    /// Migrated to the typed-signature dialect (#400).  `iterable` is
    /// `PyValue` (not `PyIterable`) so that user-defined `PyInstance`
    /// iterables reach `make_iterator` (which dispatches `__iter__`) — the
    /// registry-only path cannot dispatch `__iter__` dunders.  `start` is
    /// `PyValue` so the body can handle both `int` and `bool` inputs (CPython
    /// accepts both; `bool ⊆ int` in CPython) and produce the
    /// exact CPython `TypeError` wording for non-integer `start`.  Keeping the
    /// default as an integer `PyValue` distinguishes an omitted start from an
    /// explicitly supplied `None`.
    fn enumerate(
        #[positional_only] iterable: PyValue,
        #[default(PyValue(Value::int(0)))]
        start: PyValue,
    ) -> Result<Value> {
        // Resolve through the shared index protocol before storing the counter,
        // preserving the invariant that next_enumerate_counter only sees the
        // int family.  Normalize bool to an exact int as CPython does.
        let start_val = _interp.value_to_index(&start.0, |value| {
            PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    value_type_name_str(value),
                ),
            )
        })?;
        let bool_start = match start_val.kind() {
            ValueKind::Bool(value) => Some(value),
            ValueKind::Int(_) | ValueKind::BigInt(_) => None,
            _ => unreachable!("value_to_index guarantees an integer"),
        };
        let start_val = match bool_start {
            Some(value) => Value::int(value as i64),
            None => start_val,
        };
        // The counter stays a `Value` so it promotes to BigInt on overflow
        // instead of wrapping (#2125).
        // Convert the iterable to a lazy iterator without consuming any elements.
        // Elements are pulled lazily by step_enumerate_iter via call_next.
        let source = make_iterator(_interp, &iterable.0)?;
        Ok(Value::generator(Box::new(EnumerateIter {
            source,
            counter: start_val,
            done: false,
        })))
    }

    /// CPython: zip(*iterables, strict=False) — parallel iterator.
    /// `strict=True` raises `ValueError` if lengths differ.
    /// <https://docs.python.org/3/library/functions.html#zip>
    fn zip(args) -> Result<Value> {
        // `strict` is the only accepted keyword arg; everything else is a
        // CPython-style `TypeError`.
        let mut strict = false;
        for a in args.iter() {
            if let Some(name) = a.name.as_deref() {
                if name == "strict" {
                    strict = _interp.truthy_value(&a.value)?;
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() got an unexpected keyword argument '{name}'"),
                    ));
                }
            }
        }
        // Convert each iterable to a lazy iterator without consuming any elements.
        // Elements are pulled lazily by step_zip_iter via call_next.
        let sources = args
            .iter()
            .filter(|a| a.name.is_none())
            .map(|a| make_iterator(_interp, &a.value))
            .collect::<Result<Vec<_>>>()?;
        Ok(Value::generator(Box::new(ZipIter {
            sources,
            strict,
            done: false,
            count: 0,
        })))
    }

    /// CPython: reversed(seq) — reverse iterator.
    /// <https://docs.python.org/3/library/functions.html#reversed>
    ///
    /// CPython's protocol (in order):
    ///   1. `__reversed__` — call it, return the iterator it produces.
    ///   2. `__len__` + `__getitem__` — collect via sequence protocol, reverse.
    ///   3. Otherwise: TypeError "'X' object is not reversible".
    ///
    /// For non-PyInstance values only sequences (list, tuple, str, bytes) and
    /// range are reversible; all other types (Generator, BuiltinObject
    /// iterators, …) raise TypeError.
    fn reversed(#[positional_only] seq: PyValue) -> Result<Value> {
        let uses_special_method = reversed_uses_special_method(&seq.0);
        if let ValueKind::PyInstance(inst) = seq.0.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            // Protocol step 1: __reversed__
            if uses_special_method {
                let method_val = lookup_class_attr(&class, "__reversed__")
                    .expect("special-method provenance must retain its class slot");
                return invoke_class_method(
                    _interp,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[],
                );
            }
            // Protocol step 2: __getitem__ + __len__ (sequence protocol).
            // CPython checks __getitem__ first; if present but __len__ is
            // absent it raises "no len()" rather than "not reversible".
            if let Some(getitem_method) = lookup_class_attr(&class, "__getitem__") {
                let len_method = match lookup_class_attr(&class, "__len__") {
                    Some(m) => m,
                    None => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "object of type '{}' has no len()",
                                class.borrow().name,
                            ),
                        ))
                    }
                };
                let len_val = invoke_class_method(
                    _interp,
                    len_method.clone(),
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?;
                let n = _interp.normalize_len_result(&len_val)?;
                let obj = Value::py_instance(inst_rc);
                return Ok(make_reversed_getitem_iterator(
                    obj,
                    getitem_method,
                    len_method,
                    n as usize,
                ));
            }
            // Protocol step 3: not reversible
            return Err(PyError::named(
                "TypeError",
                format!("'{}' object is not reversible", class.borrow().name),
            ));
        }
        // dict / dict views are reversible by insertion order (CPython 3.8+,
        // issue #2093).  The backing IndexMap preserves insertion order, so the
        // iterator is a cursor descending the live entry positions, carrying a
        // size-mutation guard keyed to the backing dict (#2448).  Like CPython's
        // forward view iterators, mutating the dict's size during a `reversed()`
        // walk raises `RuntimeError` on the next `next()` call; and like
        // CPython's `dictreviter`, each key and value is read when the cursor
        // reaches it rather than snapshotted up front (#2932).
        if seq.0.is_dict() {
            // Bare `reversed(d)` iterates keys: `dict_reversekeyiterator` (#2702).
            let frame = make_reversed_dict_iter(seq.0.clone(), 0, "dict_reversekeyiterator");
            return Ok(Value::generator(Box::new(frame)));
        }
        if let Some(kind) = pyrust_builtins::dict_views::view_kind(&seq.0)
            && pyrust_builtins::dict_views::as_dict_rc(&seq.0).is_some() {
                // CPython names the reverse iterator by view kind (#2702).
                let type_name = match kind {
                    pyrust_builtins::dict_views::DictViewKind::Keys => {
                        "dict_reversekeyiterator"
                    }
                    pyrust_builtins::dict_views::DictViewKind::Values => {
                        "dict_reversevalueiterator"
                    }
                    pyrust_builtins::dict_views::DictViewKind::Items => {
                        "dict_reverseitemiterator"
                    }
                };
                let frame =
                    make_reversed_dict_iter(seq.0.clone(), kind.live_cursor_code(), type_name);
                return Ok(Value::generator(Box::new(frame)));
            }
        // mappingproxy (`vars(C)` / `d.keys().mapping`): reverse like a dict, but
        // with a size-mutation guard keyed to the live proxy so a change mid-walk
        // raises `RuntimeError` (issue #2728).  Handled before the generic
        // `__reversed__` dispatch below because `mapping_proxy::call_method`
        // returns an unguarded list-reverse iterator with no interpreter access
        // to install the guard.
        if pyrust_builtins::mapping_proxy::as_class_rc(&seq.0).is_some()
            || pyrust_builtins::mapping_proxy::as_dict_rc(&seq.0).is_some()
        {
            let items = iter_values(&seq.0)?;
            let frame = make_reversed_mapping_snapshot_iter(items, seq.0.clone());
            return Ok(Value::generator(Box::new(frame)));
        }
        // BuiltinObject types that implement `__reversed__` dispatch to it
        // directly, matching CPython's protocol step 1. Exact mappingproxies
        // were handled above so they retain their guarded reverse snapshot.
        // An object-backed proxy instead calls the named owner method; using
        // reversed(owner) would incorrectly activate the sequence fallback.
        if let ValueKind::BuiltinObject { ops, state } = seq.0.kind()
            && uses_special_method
        {
            if pyrust_builtins::mapping_proxy::is_object_proxy_ops(ops) {
                let owner = pyrust_builtins::mapping_proxy::owner_from_state(state)
                    .expect("object mappingproxy state");
                let method = _interp.get_attr(&owner, "__reversed__")?;
                return _interp.call_function_expanded(method, &[]);
            }
            return ops.call_method(state, "__reversed__", Vec::new(), &indexmap::IndexMap::new());
        }
        // Non-PyInstance: only sequence types and Range are reversible.
        // Generators (including list_iterator, set_iterator, filter, map, …)
        // and all BuiltinObject iterator types are not sequences and must
        // raise TypeError, matching CPython 3.12's check for __reversed__ /
        // (__len__ + __getitem__).
        let is_reversible = match seq.0.kind() {
            ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Str(_)
            | ValueKind::Bytes(_)
            | ValueKind::Range { .. }
            | ValueKind::BigRange { .. } => true,
            // `bytearray` is a mutable sequence (len + getitem) and is
            // reversible in CPython, yielding its bytes as ints (#2005).
            ValueKind::BuiltinObject { ops, .. } => {
                ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray)
            }
            _ => false,
        };
        if !is_reversible {
            let type_name = full_type_name_str(&seq.0);
            return Err(PyError::named(
                "TypeError",
                format!("'{}' object is not reversible", type_name),
            ));
        }
        if matches!(
            seq.0.kind(),
            ValueKind::Range { .. } | ValueKind::BigRange { .. }
        ) {
            return make_reversed_range_iterator(&seq.0);
        }
        make_reversed_sequence_iterator(&seq.0)
    }

    /// CPython: map(func, *iterables) — apply func to corresponding elements
    /// from all iterables in lockstep; stops at the shortest iterable.
    /// Returns a lazy `map` iterator object, not a list.
    /// <https://docs.python.org/3/library/functions.html#map>
    fn map(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() must have at least two arguments."),
            ));
        }
        let func = args[0].value.clone();
        // Convert each iterable argument to an iterator object without
        // consuming any elements.  Elements are pulled lazily by
        // step_map_iter via call_next.
        let sources: Result<IterSrcBuf> = args[1..]
            .iter()
            .map(|a| make_iterator(_interp, &a.value))
            .collect();
        let sources = sources?;
        Ok(Value::generator(Box::new(MapIter {
            func,
            sources,
            done: false,
        })))
    }

    /// CPython: filter(func, iterable) — keep elements where func is truthy.
    /// Returns a lazy `filter` iterator object, not a list.
    /// `func` may be `None` for identity truthiness testing.
    /// <https://docs.python.org/3/library/functions.html#filter>
    fn filter(
        #[positional_only] func: PyValue,
        #[positional_only] iterable: PyValue,
    ) -> Result<Value> {
        // Convert the iterable to an iterator without consuming any elements.
        // Elements are pulled lazily by step_filter_iter via call_next.
        let source = make_iterator(_interp, &iterable.0)?;
        let func_opt = if func.0.is_none() { None } else { Some(func.0) };
        Ok(Value::generator(Box::new(FilterIter {
            func: func_opt,
            source,
            done: false,
        })))
    }

    /// CPython: iter(obj) / iter(callable, sentinel) — return an iterator.
    /// <https://docs.python.org/3/library/functions.html#iter>
    ///
    /// The one-argument form returns an iterator over an iterable object.
    /// The two-argument form returns a callable-iterator that calls
    /// `callable()` on each `next()` and stops when the result equals
    /// `sentinel`.  The two-argument form calls user code on every iteration.
    fn iter(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            2 => {
                // Two-argument form: iter(callable, sentinel).
                let callable = args[0].value.clone();
                let sentinel = args[1].value.clone();
                // Validate that arg 0 is callable — matches CPython's TypeError.
                let is_callable = match callable.kind() {
                    ValueKind::UserFunction(_)
                    | ValueKind::BuiltinFunction(_)
                    | ValueKind::BoundMethod { .. }
                    | ValueKind::ClassBoundMethod { .. }
                    | ValueKind::PyClass(_) => true,
                    ValueKind::BuiltinObject { .. } => {
                        crate::interpreter::is_builtin_callable_adapter(&callable)
                    }
                    ValueKind::PyInstance(inst) => {
                        let class = Rc::clone(&inst.borrow().class);
                        lookup_class_attr(&class, "__call__").is_some()
                    }
                    _ => false,
                };
                if !is_callable {
                    return Err(PyError::named(
                        "TypeError",
                        "iter(object, sentinel): object must be callable".to_string(),
                    ));
                }
                Ok(Value::generator(Box::new(CallableIter {
                    callable,
                    sentinel,
                    done: false,
                })))
            }
            // One canonical owner classifies primitive, builtin-subclass,
            // metaclass, and user-protocol iterables.  Every lazy consumer
            // (`iter`, `map`, `zip`, `tee`, …) now takes this same path.
            1 => make_iterator(_interp, &args[0].value),
            0 => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at least 1 argument, got 0"),
            )),
            n => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 2 arguments, got {n}"),
            )),
        }
    }

    /// CPython: next(iterator[, default]) — fetch the next element.
    /// <https://docs.python.org/3/library/functions.html#next>
    ///
    /// Must stay in `(args)` dialect: `next(it, None)` is semantically
    /// distinct from `next(it)` — the former returns Python None when
    /// exhausted; the latter raises StopIteration.  `Option<PyValue>`
    /// collapses both into Rust None, which breaks the default=None case.
    fn next(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at least 1 argument, got 0"),
            ));
        }
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 2 arguments, got {}", args.len()),
            ));
        }
        let gen_val = args[0].value.clone();
        let default_val = if args.len() == 2 {
            Some(args[1].value.clone())
        } else {
            None
        };
        _interp.call_next(&gen_val, default_val)
    }

}
