use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: enumerate(iterable, start=0) — enumerate iterator.
    /// <https://docs.python.org/3/library/functions.html#enumerate>
    ///
    /// Migrated to the typed-signature dialect (#400).  `iterable` is
    /// `PyValue` (not `PyIterable`) so that user-defined `PyInstance`
    /// iterables reach `make_iterator` (which dispatches `__iter__`) — the
    /// registry-only path cannot dispatch `__iter__` dunders.  `start` is
    /// `Option<PyValue>`
    /// so the body can handle both `int` and `bool` inputs (CPython
    /// accepts both; `bool ⊆ int` in CPython) and produce the
    /// exact CPython `TypeError` wording for non-integer `start`.
    fn enumerate(
        #[positional_only] iterable: PyValue,
        #[default(None)]
        start: Option<PyValue>,
    ) -> Result<Value> {
        // `start` accepts any int (incl. BigInt and bool); a non-int raises the
        // CPython TypeError.  The counter is kept as a `Value` so it promotes to
        // BigInt on overflow instead of wrapping (#2125).
        let start_val: Value = match start {
            None => Value::int(0),
            Some(v) => match v.0.kind() {
                ValueKind::Int(_) | ValueKind::BigInt(_) => v.0.clone(),
                ValueKind::Bool(b) => Value::int(b as i64),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        value_type_name_str(&v.0),
                    ),
                )),
            },
        };
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
        if let ValueKind::PyInstance(inst) = seq.0.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            // Protocol step 1: __reversed__
            if let Some(method_val) = lookup_class_attr(&class, "__reversed__") {
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
        // issue #2093).  The backing IndexMap preserves insertion order, so we
        // build a forward-ordered list of keys / values / (key, value) pairs,
        // reverse it, and wrap it in a `NativeIterFrame` carrying a size-mutation
        // guard keyed to the live backing dict (#2448).  Like CPython's forward
        // view iterators, mutating the dict's size during a `reversed()` walk
        // raises `RuntimeError` on the next `next()` call.
        if let ValueKind::Dict(map) = seq.0.kind() {
            let mut items: Vec<Value> = map.keys().map(|k| key_to_value(k.clone())).collect();
            items.reverse();
            // Bare `reversed(d)` iterates keys: `dict_reversekeyiterator` (#2702).
            let frame = make_reversed_dict_iter(items, seq.0.clone(), "dict_reversekeyiterator");
            return Ok(Value::generator(Box::new(frame)));
        }
        if let Some(kind) = pyrust_builtins::dict_views::view_kind(&seq.0)
            && let Some(rc) = pyrust_builtins::dict_views::as_dict_rc(&seq.0) {
                let mut items: Vec<Value> = {
                    let map = rc.borrow();
                    match kind {
                        // dict_keys
                        pyrust_builtins::dict_views::DictViewKind::Keys => {
                            map.keys().map(|k| key_to_value(k.clone())).collect()
                        }
                        // dict_values
                        pyrust_builtins::dict_views::DictViewKind::Values => {
                            map.values().cloned().collect()
                        }
                        // dict_items
                        pyrust_builtins::dict_views::DictViewKind::Items => map
                            .iter()
                            .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                            .collect(),
                    }
                };
                items.reverse();
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
                let frame = make_reversed_dict_iter(items, seq.0.clone(), type_name);
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
            let mut items = iter_values(&seq.0)?;
            items.reverse();
            // mappingproxy reverses its keys: `dict_reversekeyiterator` (#2702).
            let frame =
                make_reversed_dict_iter(items, seq.0.clone(), "dict_reversekeyiterator");
            return Ok(Value::generator(Box::new(frame)));
        }
        // BuiltinObject types that implement `__reversed__` (e.g. mappingproxy,
        // issue #2684) dispatch to it directly, matching CPython's protocol
        // step 1.  `call_method` already returns the reverse-order iterator.
        if let ValueKind::BuiltinObject { ops, state } = seq.0.kind()
            && ops.has_method("__reversed__")
        {
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
