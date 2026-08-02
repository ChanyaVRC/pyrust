impl Interpreter {
    fn call_instance_bound_method(
        &mut self,
        receiver: &Value,
        method: &str,
        pos: &mut Vec<Value>,
        kw: &mut PyDict,
    ) -> Result<Value> {
        let ValueKind::PyInstance(inst) = receiver.kind() else {
            unreachable!("receiver family checked by bound method dispatcher");
        };
        // Class method backed by a `BuiltinFunction` — emitted
        // by `pyrust_module!`'s `class { … }` block.  `get_attr`
        // wrapped the method-name-on-instance pair as a
        // bound_method; here we re-resolve the method on the
        // class and dispatch with `self` prepended through the
        // unified helper.
        let class = Rc::clone(&inst.borrow().class);
        let method_val = match lookup_class_attr(&class, method) {
            Some(v) => v,
            None => {
                let class_name = class.borrow().name.clone();
                return Err(PyError::attribute_error(
                    format!("'{class_name}' object has no attribute '{method}'"),
                    Some(method.to_string()),
                    Some(Value::py_instance(Rc::clone(inst))),
                ));
            }
        };
        // Issue #976/#994: if the resolved method is a primitive
        // builtin (e.g. `dict.keys`, `tuple.count`), and the
        // instance has a `__builtin_data__` backing value (set at
        // construction time for subclasses of dict/list/set/
        // frozenset/tuple), dispatch on the backing value directly.
        // This avoids the `kind_ok` type guard in
        // `call_function_expanded` rejecting the PyInstance receiver.
        // Issue #1204: same mechanism for str/int/float/bytes subclasses.
        // Issue #2847: some primitive comparison slots are represented in the
        // subclass MRO by the canonical object sentinel rather than a
        // type-qualified sentinel.  Treat that sentinel as primitive protocol
        // dispatch only when provenance confirms that the subclass truly
        // inherits the slot; an explicit user assignment/override still wins.
        let inherited_primitive_richcmp = matches!(
            method,
            "__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__"
        ) && effective_builtin_receiver(receiver, &[method])
            .is_some();
        if let ValueKind::BuiltinFunction(fn_name) = method_val.kind()
            && (inherited_primitive_richcmp
                || fn_name.split_once('.').is_some_and(|(t, _)| {
                    matches!(
                        t,
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
                    )
                }))
            && let Some(backing) = builtin_data_backing(receiver)
        {
            // Issue #1909: container protocol dunders
            // (`MyList().__len__()`, `MyDict().__getitem__(k)`)
            // operate on the backing primitive — route through
            // the shared dispatcher so they match the
            // plain-primitive form instead of leaking a
            // `RuntimeError` from the per-type `call`.
            if method.starts_with("__")
                && is_protocol_dunder(&pyrust_core::builtin_type_name(&backing), method)
            {
                // Issue #2767: keyword-rejection wording on a
                // builtin-subclass instance must match the
                // plain-primitive paths — named method-wrappers
                // (`float.__round__()`, `list.__getitem__()`)
                // use the type-qualified form, anonymous slot
                // wrappers (`wrapper __len__()`) use the bare
                // form (issue #2291).
                if !kw.is_empty() {
                    let type_name = pyrust_core::builtin_type_name(&backing);
                    return Err(if is_named_protocol_wrapper(method, &type_name) {
                        pyrust_core::type_err!("{type_name}.{method}() takes no keyword arguments")
                    } else {
                        pyrust_core::type_err!("wrapper {method}() takes no keyword arguments")
                    });
                }
                let args_vec: Vec<Value> = std::mem::take(pos);
                return self.dispatch_builtin_protocol_dunder(method, backing, args_vec);
            }
            enum BkKind {
                Dict,
                List,
                Set,
                Frozenset,
                Tuple,
                Str,
                Int,
                Float,
                Bytes,
                Bytearray,
                Other,
            }
            let bk_kind = match backing.kind() {
                ValueKind::Dict(_) => BkKind::Dict,
                ValueKind::List(_) => BkKind::List,
                ValueKind::Set(_) => BkKind::Set,
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
                {
                    BkKind::Frozenset
                }
                // Issue #2324: bytearray backing is a
                // `BuiltinObject`; route its methods (append,
                // upper, …) through the ops table like a plain
                // bytearray receiver.
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
                {
                    BkKind::Bytearray
                }
                ValueKind::Tuple(_) => BkKind::Tuple,
                ValueKind::Str(_) => BkKind::Str,
                ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => BkKind::Int,
                ValueKind::Float(_) => BkKind::Float,
                ValueKind::Bytes(_) => BkKind::Bytes,
                _ => BkKind::Other,
            };
            let args_vec: Vec<Value> = std::mem::take(pos);
            return match bk_kind {
                BkKind::Dict => {
                    let Some(spec) = pyrust_builtins::dict::method_spec(method) else {
                        return pyrust_builtins::dict::call(method, &backing, args_vec, kw);
                    };
                    spec.validate_keywords(!kw.is_empty())?;
                    let resolved = spec.method();
                    // Issue #1563: `fromkeys` is a classmethod; when
                    // called on a subclass instance (`MyDict().fromkeys`)
                    // CPython uses `type(self)` as `cls`, so the result
                    // is a `MyDict`, not a plain `dict`.  Route through
                    // the same class-dispatch path used for
                    // `MyDict.fromkeys` (bound-method on PyClass).
                    if resolved == pyrust_builtins::dict::Method::FromKeys {
                        let bound = pyrust_builtins::bound_method::bound_method(
                            "fromkeys",
                            Value::py_class(Rc::clone(&class)),
                        );
                        let mut expanded: Vec<ExpandedCallArg> = args_vec
                            .into_iter()
                            .map(|v| ExpandedCallArg {
                                name: None,
                                value: v,
                            })
                            .collect();
                        for (k, v) in kw {
                            if let PyKey::Str(s) = k {
                                expanded.push(ExpandedCallArg {
                                    name: Some(s.as_str().unwrap_or("").to_owned()),
                                    value: v.clone(),
                                });
                            }
                        }
                        return self.call_function_expanded(bound, &expanded);
                    }
                    spec.validate_positional_arity(args_vec.len())?;
                    // Issue #2436: `keys`/`values`/`items` on a
                    // dict-subclass instance (OrderedDict, plain
                    // `class D(dict)`, Counter, defaultdict) must
                    // build a live `dict_views` view backed by the
                    // *same* IndexMap `Rc` as the backing dict —
                    // exactly as the plain-dict path does
                    // (`dict::view_method`). The fall-through to
                    // `call_dict_method` materialised a `list`
                    // snapshot instead, so the resulting object was
                    // the wrong type, not live, and (because plain
                    // lists are unguarded) iteration silently
                    // ignored size mutation where CPython raises
                    // `RuntimeError`.  Sharing the `Rc` puts the
                    // view back on the `IterTag::Other` size-
                    // mutation guard with the container-specific
                    // wording.
                    if spec.view_method().is_some() {
                        // OrderedDict (or a subclass) tags the view
                        // so its size-mutation guard reports
                        // "OrderedDict mutated during iteration".
                        let ordered = is_ordered_dict_class_or_subclass(&class);
                        return Self::dict_view_for_backing(&backing, resolved.name(), ordered);
                    }
                    // issue #2465: record an OrderedDict
                    // `clear()` (with its pre-clear size) keyed
                    // by the backing dict's identity, so a guard
                    // hit during iteration reports "changed
                    // size" rather than "mutated".  Only the
                    // OrderedDict subclass tracks this; every
                    // other dict subclass keeps plain-dict
                    // wording.
                    if resolved == pyrust_builtins::dict::Method::Clear
                        && is_ordered_dict_class_or_subclass(&class)
                        && let Some(id) = backing.value_id()
                    {
                        let prelen = backing.dict_with(|d| d.len()).unwrap_or(0);
                        pyrust_builtins::ordered_mapping::note_clear(id, prelen);
                    }
                    self.call_dict_method_resolved(resolved, backing, args_vec, kw)
                }
                BkKind::List => self.dispatch_builtin_container_method(
                    BuiltinContainerKind::List,
                    backing,
                    method,
                    args_vec,
                    kw,
                    false,
                ),
                BkKind::Set => {
                    let Some(spec) = pyrust_builtins::set::method_spec(method) else {
                        return pyrust_builtins::set::call(method, &backing, args_vec);
                    };
                    spec.validate_keywords(!kw.is_empty())?;
                    spec.validate_positional_arity(args_vec.len())?;
                    self.call_set_method_resolved(spec.method(), backing, args_vec)
                }
                BkKind::Frozenset => {
                    pyrust_builtins::frozenset::validate_method_keywords(method, !kw.is_empty())?;
                    self.call_frozenset_method(method, backing, args_vec)
                }
                BkKind::Tuple => self.dispatch_builtin_container_method(
                    BuiltinContainerKind::Tuple,
                    backing,
                    method,
                    args_vec,
                    kw,
                    false,
                ),
                BkKind::Str => {
                    // `str.format` on a subclass receiver: thread
                    // kwargs into the template, mirroring the
                    // plain-str bound-method path above.
                    // `call_str_method` only accepts positional
                    // args, so it can't reach `format` (it would
                    // hit the drift-guard sentinel in
                    // pyrust_builtins::string) and would silently
                    // drop keyword fields (#2376).
                    if method == "format" {
                        let template = match backing.kind() {
                            ValueKind::Str(s) => s.to_string(),
                            _ => unreachable!("BkKind::Str guard above"),
                        };
                        // CPython returns the receiver itself —
                        // subclass identity preserved — when the
                        // template contains no brace markup at all
                        // and is non-empty (CPython's
                        // unicode_result_unchanged; surplus args
                        // are ignored, so this holds regardless
                        // of arguments).
                        if !template.is_empty() && !template.contains(['{', '}']) {
                            return Ok(Value::py_instance(Rc::clone(inst)));
                        }
                        let keyword: Vec<(&str, Value)> = kw
                            .iter()
                            .filter_map(|(k, v)| {
                                if let PyKey::Str(name) = k {
                                    Some((name.as_str().unwrap_or(""), v.clone()))
                                } else {
                                    None
                                }
                            })
                            .collect();
                        self.format_str_template(&template, &args_vec, &keyword)
                    } else {
                        self.dispatch_builtin_container_method(
                            BuiltinContainerKind::Str,
                            backing,
                            method,
                            args_vec,
                            kw,
                            false,
                        )
                    }
                }
                BkKind::Int => {
                    let mut int_args = args_vec;
                    self.resolve_to_bytes_length(method, &mut int_args, kw)?;
                    pyrust_builtins::int::call(method, &backing, &int_args, kw)
                }
                BkKind::Float => {
                    let f = match backing.kind() {
                        ValueKind::Float(f) => f,
                        _ => unreachable!("BkKind::Float guard above"),
                    };
                    // Issue #2767: float methods take no keyword
                    // arguments (subclass-receiver path).
                    if !kw.is_empty() {
                        return Err(pyrust_core::type_err!(
                            "float.{method}() takes no keyword arguments"
                        ));
                    }
                    pyrust_builtins::float::call(method, f, &args_vec)
                }
                BkKind::Bytes => {
                    // Accept bytes-subclass / bytearray args
                    // (#1928); partition/rpartition echo the
                    // original separator object as the middle
                    // element (#2680).
                    self.call_bytes_method_with_protocols(method, &backing, args_vec, kw)
                }
                BkKind::Bytearray => {
                    pyrust_builtins::bytearray::validate_method_keywords(method, !kw.is_empty())?;
                    // Issue #2324: dispatch through the bytearray
                    // ops table, mirroring a plain bytearray
                    // receiver (the `BuiltinObject` arm of
                    // `bound_method_dispatch_inner`).  Coerce
                    // bytes-subclass / bytearray args first
                    // (#1928); `join`'s single iterable arg holds
                    // the items to join.
                    let splitlines = method == "splitlines";
                    let mut args_vec = if let Some(bound) =
                        self.bind_bytearray_splitlines_keepends(method, &args_vec, kw)?
                    {
                        bound
                    } else if method == "join" {
                        // #2538: drive a lazy-iterator `join`
                        // argument through the interpreter and
                        // coerce its elements.
                        self.prepare_bytearray_join_args(args_vec)?
                    } else {
                        coerce_bytes_subclass_method_args(method, args_vec)
                    };
                    // #2532: drive a lazy-iterator `extend`
                    // argument through the interpreter before the
                    // receiver-only ops table sees it.
                    if method == "extend" {
                        args_vec = self.prepare_bytearray_extend_args(args_vec)?;
                    }
                    let empty_kw = PyDict::default();
                    let coerced_kw = if splitlines {
                        None
                    } else {
                        coerce_bytes_subclass_method_kwargs(kw)
                    };
                    let kw = if splitlines {
                        &empty_kw
                    } else {
                        coerced_kw.as_ref().unwrap_or(kw)
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
                    let ValueKind::BuiltinObject { ops, state } = backing.kind() else {
                        unreachable!("BkKind::Bytearray guard above");
                    };
                    ops.call_method(state, method, args_vec, &kw_str)
                }
                BkKind::Other => Err(PyError::Runtime(format!(
                    "internal: unexpected builtin_data kind for method '{method}'"
                ))),
            };
        }
        // Reconstitute kwargs as ExpandedCallArgs (the
        // bound_method dispatch split them into pos+kw maps).
        // Drain pos so its capacity is preserved in the buf.
        let mut combined: ExpandedArgBuf = ExpandedArgBuf::with_capacity(pos.len() + kw.len());
        for v in pos.drain(..) {
            combined.push(ExpandedCallArg {
                name: None,
                value: v,
            });
        }
        for (k, v) in kw {
            if let PyKey::Str(name) = k {
                combined.push(ExpandedCallArg {
                    name: Some(name.as_str().unwrap_or("").to_owned()),
                    value: v.clone(),
                });
            }
        }
        invoke_class_method(
            self,
            method_val,
            Value::py_instance(inst.clone()),
            &combined,
        )
    }
}
