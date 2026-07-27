// Dict/set/frozenset method implementations that require Python dispatch.
impl Interpreter {
    /// Dispatch any dict method.  Methods that read or write keys
    /// (`get`/`pop`/`setdefault`/`__contains__`) route through
    /// `dict_lookup`/`dict_insert` so user-defined `__hash__`/`__eq__`
    /// fire (issue #368).  Everything else delegates to the
    /// interpreter-free `pyrust_builtins::dict::call`.
    ///
    /// Name-based compatibility entry point. Hot dispatchers should resolve
    /// and validate a [`pyrust_builtins::dict::MethodSpec`] once, then call
    /// [`Self::call_dict_method_resolved`].
    pub(crate) fn call_dict_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
        kwargs: &PyDict,
    ) -> Result<Value> {
        let Some(spec) = pyrust_builtins::dict::method_spec(method) else {
            return pyrust_builtins::dict::call(method, &receiver, args, kwargs);
        };
        spec.validate_keywords(!kwargs.is_empty())?;
        spec.validate_positional_arity(args.len())?;
        self.call_dict_method_resolved(spec.method(), receiver, args, kwargs)
    }

    /// Dispatch a dict method whose name and signature were already resolved
    /// and validated by the caller.
    pub(crate) fn call_dict_method_resolved(
        &mut self,
        resolved: pyrust_builtins::dict::Method,
        receiver: Value,
        args: Vec<Value>,
        kwargs: &PyDict,
    ) -> Result<Value> {
        self.call_dict_method_resolved_inner(resolved, receiver, args, kwargs)
    }

    fn call_dict_method_resolved_inner(
        &mut self,
        resolved: pyrust_builtins::dict::Method,
        receiver: Value,
        args: Vec<Value>,
        kwargs: &PyDict,
    ) -> Result<Value> {
        use pyrust_builtins::dict::Method as DictMethod;
        match resolved {
            DictMethod::Get | DictMethod::Contains | DictMethod::Pop | DictMethod::SetDefault => {
                let mut iter = args.into_iter();
                let key_val = iter.next().ok_or_else(|| {
                    PyError::Runtime(format!(
                        "dict.{}() requires at least 1 argument",
                        resolved.name()
                    ))
                })?;
                let pk = self.value_to_pykey(&key_val)?;
                match resolved {
                    DictMethod::Get => {
                        let default = iter.next().unwrap_or_else(Value::none);
                        Ok(self
                            .dict_lookup(&receiver, &pk)?
                            .map(|(_, v)| v)
                            .unwrap_or(default))
                    }
                    DictMethod::Contains => {
                        Ok(Value::bool_(self.dict_lookup(&receiver, &pk)?.is_some()))
                    }
                    DictMethod::Pop => match self.dict_lookup(&receiver, &pk)? {
                        Some((idx, v)) => {
                            // `dict_lookup` already dropped its borrow before
                            // running user code, so the index is still valid.
                            receiver.dict_with_mut(|dict| dict.shift_remove_index(idx));
                            Ok(v)
                        }
                        None => {
                            if let Some(default) = iter.next() {
                                Ok(default)
                            } else {
                                Err(PyError::key_error(key_val.clone()))
                            }
                        }
                    },
                    DictMethod::SetDefault => {
                        let default = iter.next().unwrap_or_else(Value::none);
                        if let Some((_, v)) = self.dict_lookup(&receiver, &pk)? {
                            return Ok(v);
                        }
                        receiver
                            .dict_with_mut(|dict| dict.insert(pk, default.clone()))
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected dict".to_string())
                            })?;
                        Ok(default)
                    }
                    _ => unreachable!(),
                }
            }
            // `update` with non-primitive iterables (range, generators,
            // user-defined iterables) — the builtins crate has no interpreter
            // access and falls to its `_` arm raising "'X' object is not
            // iterable" for these types.  Intercept here when the positional
            // arg is not one of the five primitive types the builtins crate
            // already handles (Dict/List/Tuple/Str/Bytes).  Delegate for those
            // types to preserve existing behaviour (including the self-alias
            // snapshot logic in snapshot_update_arg).
            DictMethod::Update => {
                if args.len() > 1 {
                    return Err(pyrust_core::type_err!(
                        "update expected at most 1 argument, got {}",
                        args.len()
                    ));
                }
                // #1914: when the update source is a `dict` (the common
                // `d.update(other_dict)` and `**kwargs`-into-dict path), route
                // through `dict_extend_value_dedup` so `PyKey::Object` keys
                // deduplicate via user `__eq__` (last value wins).  The helper
                // keeps the raw fast path for all-primitive keys.  kwargs are
                // string keys, always primitive — append them after.
                if let Some(arg) = args.first() {
                    // Snapshot the source pairs in a scoped block so the `Dict`
                    // `Ref` borrow is dropped before `dict_extend_value_dedup`
                    // takes a `borrow_mut` — critical for the self-aliased
                    // `d.update(d)` case where `arg` IS `receiver` (#448).
                    let src_pairs: Option<Vec<(PyKey, Value)>> = match arg.kind() {
                        ValueKind::Dict(src) => {
                            Some(src.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        }
                        _ => None,
                    };
                    if let Some(pairs) = src_pairs {
                        self.dict_extend_value_dedup(&receiver, pairs)?;
                        if !kwargs.is_empty() {
                            let kw_pairs: Vec<(PyKey, Value)> =
                                kwargs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                            self.dict_extend_value_dedup(&receiver, kw_pairs)?;
                        }
                        return Ok(Value::none());
                    }
                }
                // Check whether we need to intercept.  If the single positional
                // arg is a primitive type that pyrust_builtins::dict::call already
                // handles correctly, delegate.  `List`/`Tuple` (iterable of
                // pairs) go through the interpreter slow path below so that
                // PyInstance keys hash via user `__hash__` and dedup via user
                // `__eq__` (#1914); `Str`/`Bytes` pairs are always char/byte
                // primitives and stay on the fast builtin path.
                let needs_interp = match args.first() {
                    None => false,
                    Some(arg) => !matches!(
                        arg.kind(),
                        ValueKind::Dict(_) | ValueKind::Str(_) | ValueKind::Bytes(_)
                    ),
                };
                if !needs_interp {
                    return pyrust_builtins::dict::call_resolved(
                        DictMethod::Update,
                        &receiver,
                        args,
                        kwargs,
                    );
                }
                // Intercept: the arg is a non-primitive iterable (Range,
                // Generator, BuiltinObject, PyInstance, …).
                let arg = args.into_iter().next().unwrap();
                // #2222: CPython's `dict.update` first checks for a `keys()`
                // method and, when present, treats the arg as a *mapping*
                // (iterate `keys()` and subscript via `__getitem__`) rather
                // than an iterable of pairs.  Route any keys()-bearing mapping
                // (ChainMap / OrderedDict / Counter / UserDict / custom)
                // through the same protocol helper used by `dict()` and
                // `**`-unpack (#2190), so the two paths stay consistent.
                if crate::interpreter::visit_mapping_pairs_via_protocol(
                    self,
                    &arg,
                    |interp, key, value| interp.dict_insert_value(&receiver, key, value),
                )? {
                    for (k, v) in kwargs {
                        receiver
                            .dict_with_mut(|dict| {
                                dict.insert(k.clone(), v.clone());
                            })
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected dict".to_string())
                            })?;
                    }
                    return Ok(Value::none());
                }
                // `mappingproxy` is a mapping (it has `keys()`), but it is a
                // BuiltinObject, not a PyInstance, so the protocol helper above
                // returns `None` for it.  Treat both proxy variants as mappings
                // and copy their pairs verbatim, matching `dict()` / `{**m}`
                // (#2679, and the pre-existing class-backed `vars(C)` form).
                let proxy_pairs: Option<Vec<(PyKey, Value)>> =
                    if let Some(cls_rc) = pyrust_builtins::mapping_proxy::as_class_rc(&arg) {
                        Some(
                            cls_rc
                                .borrow()
                                .attrs
                                .iter()
                                .map(|(k, v)| (PyKey::str_from(k), v.clone()))
                                .collect(),
                        )
                    } else {
                        pyrust_builtins::mapping_proxy::as_dict_rc(&arg)
                            .map(|dict_rc| dict_rc.borrow().clone().into_iter().collect())
                    };
                if let Some(pairs) = proxy_pairs {
                    self.dict_extend_value_dedup(&receiver, pairs)?;
                    for (k, v) in kwargs {
                        receiver
                            .dict_with_mut(|dict| {
                                dict.insert(k.clone(), v.clone());
                            })
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected dict".to_string())
                            })?;
                    }
                    return Ok(Value::none());
                }
                // Drive the iterable one element at a time and insert each
                // pair into the dict eagerly.  This matches CPython: items
                // yielded before a mid-iteration exception are already in the
                // dict.  Using collect_iterable (materialise-then-process)
                // would silently drop those items when the generator raises.
                let iter = make_iterator(self, &arg)?;
                // Each element must be a length-2 sequence; extract the key and
                // value.  Mirror the logic in pyrust_builtins::dict's push_pair,
                // but use value_to_pykey so user-defined __hash__/__eq__ fire
                // correctly for PyInstance keys.
                let mut idx: usize = 0;
                loop {
                    let elem = match self.call_next(&iter, None) {
                        Ok(v) => v,
                        Err(ref e) if is_stop_iteration_error(e) => break,
                        Err(e) => return Err(e),
                    };
                    let (k_val, v_val): (Value, Value) = match elem.kind() {
                        ValueKind::List(items) => {
                            let len = items.len();
                            if len != 2 {
                                return Err(pyrust_core::value_err!(
                                    "dictionary update sequence element #{idx} has length {len}; 2 is required"
                                ));
                            }
                            (items[0].clone(), items[1].clone())
                        }
                        ValueKind::Tuple(items) => {
                            let len = items.len();
                            if len != 2 {
                                return Err(pyrust_core::value_err!(
                                    "dictionary update sequence element #{idx} has length {len}; 2 is required"
                                ));
                            }
                            (items[0].clone(), items[1].clone())
                        }
                        ValueKind::Str(s) => {
                            let chars: Vec<char> = s.chars().collect();
                            let len = chars.len();
                            if len != 2 {
                                return Err(pyrust_core::value_err!(
                                    "dictionary update sequence element #{idx} has length {len}; 2 is required"
                                ));
                            }
                            (
                                Value::string(chars[0].to_string()),
                                Value::string(chars[1].to_string()),
                            )
                        }
                        _ => {
                            return Err(pyrust_core::type_err!(
                                "cannot convert dictionary update sequence element #{idx} to a sequence"
                            ));
                        }
                    };
                    let pk = self.value_to_pykey(&k_val)?;
                    // Insert this pair immediately so an error from a later
                    // element leaves earlier updates visible.  The single-pair
                    // receiver helper gives primitive keys an O(1) IndexMap
                    // probe; calling the bulk helper with a one-element Vec
                    // rescanned every destination key and made this loop O(n²).
                    // Object keys still dispatch stored-key `__eq__` and keep
                    // the original key object/insertion position (#1914).
                    self.dict_insert_value(&receiver, pk, v_val)?;
                    idx += 1;
                }
                // Apply keyword arguments after the positional iterable,
                // matching CPython's order.
                for (k, v) in kwargs {
                    receiver
                        .dict_with_mut(|dict| {
                            dict.insert(k.clone(), v.clone());
                        })
                        .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                }
                Ok(Value::none())
            }
            // `fromkeys` is a classmethod: ignore the dict receiver and call
            // the registry dispatch directly with the user-supplied args.
            DictMethod::FromKeys => {
                let dispatch =
                    crate::builtin_registry::lookup("dict.fromkeys").ok_or_else(|| {
                        PyError::Runtime("internal: dict.fromkeys not in registry".to_string())
                    })?;
                let expanded: Vec<ExpandedCallArg> = args
                    .into_iter()
                    .map(|v| ExpandedCallArg {
                        name: None,
                        value: v,
                    })
                    .collect();
                dispatch(self, &expanded)
            }
            // keys/values/items must build a LIVE guarded view — dict::call
            // without the backing Rc materialises a list snapshot (wrong type,
            // unguarded).  The #2436 review found this THIRD copy of the view
            // decision via getattr-bound calls; route through the shared
            // constructor like the slow-path and inline-cache sites.
            DictMethod::Keys | DictMethod::Values | DictMethod::Items
                if args.is_empty() && kwargs.is_empty() =>
            {
                Self::dict_view_for_backing(&receiver, resolved.name(), false)
            }
            _ => pyrust_builtins::dict::call_resolved(resolved, &receiver, args, kwargs),
        }
    }
}
