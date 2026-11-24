// `object` protocol method execution (`__sizeof__`, `__dir__`, `__getstate__`,
// `__reduce__`, `__reduce_ex__`) shared by bound, unbound, and container paths.

impl Interpreter {
    /// Compute the result of an object-protocol method call (#2151 / #2946) on
    /// a built-in data value whose receiver is already bound.  Single source of
    /// truth shared by the bound-method dispatch, the container method-call
    /// fast path, and the `object.__reduce*` / `object.__getstate__` registry
    /// handlers.  `method` must satisfy `is_object_protocol_method(receiver, method)`.
    pub(crate) fn call_object_protocol_method(
        &mut self,
        method: &str,
        receiver: &Value,
        pos: &[Value],
        has_kw: bool,
    ) -> Result<Value> {
        match method {
            // Implementation-specific size; tests assert the int type only.
            "__sizeof__" => {
                if has_kw {
                    return Err(pyrust_core::type_err!(
                        "object.__sizeof__() takes no keyword arguments"
                    ));
                }
                if !pos.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "object.__sizeof__() takes no arguments ({} given)",
                        pos.len()
                    ));
                }
                Ok(Value::int(std::mem::size_of::<Value>() as i64))
            }
            "__dir__" => {
                if has_kw {
                    return Err(pyrust_core::type_err!(
                        "object.__dir__() takes no keyword arguments"
                    ));
                }
                if !pos.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "object.__dir__() takes no arguments ({} given)",
                        pos.len()
                    ));
                }
                let mut names = dir_names(receiver);
                names.sort();
                names.dedup();
                Ok(Value::list(names.into_iter().map(Value::string).collect()))
            }
            "__reduce__" => {
                if has_kw {
                    return Err(pyrust_core::type_err!(
                        "object.__reduce__() takes no keyword arguments"
                    ));
                }
                if !pos.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "object.__reduce__() takes no arguments ({} given)",
                        pos.len()
                    ));
                }
                self.eval_object_reduce(receiver, None)
            }
            "__reduce_ex__" => {
                if has_kw {
                    return Err(pyrust_core::type_err!(
                        "object.__reduce_ex__() takes no keyword arguments"
                    ));
                }
                if pos.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "object.__reduce_ex__() takes exactly one argument ({} given)",
                        pos.len()
                    ));
                }
                let protocol =
                    self.value_to_isize(&pos[0], "Python int too large to convert to C int")?;
                let protocol = i32::try_from(protocol).map_err(|_| {
                    pyrust_core::overflow_err!("Python int too large to convert to C int")
                })?;
                self.eval_object_reduce(receiver, Some(protocol))
            }
            "__bool__" => {
                if has_kw {
                    return Err(pyrust_core::type_err!(
                        "NoneType.__bool__() takes no keyword arguments"
                    ));
                }
                if !pos.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "expected 0 arguments, got {}",
                        pos.len()
                    ));
                }
                Ok(Value::bool_(false))
            }
            // `__getstate__()` returns None for objects with no instance state.
            "__getstate__" => {
                if has_kw {
                    return Err(pyrust_core::type_err!(
                        "object.__getstate__() takes no keyword arguments"
                    ));
                }
                if !pos.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "object.__getstate__() takes no arguments ({} given)",
                        pos.len()
                    ));
                }
                match receiver.kind() {
                    ValueKind::PyInstance(rc) => Ok(default_instance_state(rc)),
                    _ => Ok(Value::none()),
                }
            }
            _ => unreachable!("is_object_protocol_method guard"),
        }
    }

    /// Unified CPython 3.12.3 `object.__reduce__` (`protocol == None`) and
    /// `object.__reduce_ex__(protocol)` (`protocol == Some(proto)`) semantics (#2946).
    pub(crate) fn eval_object_reduce(
        &mut self,
        receiver: &Value,
        protocol: Option<i32>,
    ) -> Result<Value> {
        // MappingProxyType is unpicklable for ALL protocols.
        if pyrust_builtins::mapping_proxy::is_mapping_proxy(receiver) {
            return Err(pyrust_core::type_err!(
                "cannot pickle 'mappingproxy' object"
            ));
        }

        let newobj_fn = || Value::builtin_function("copyreg.__newobj__");
        let reconstructor_fn = || Value::builtin_function("copyreg._reconstructor");
        let proto_v2_plus = protocol.is_some_and(|p| p >= 2);
        let proto_v3_plus = protocol.is_some_and(|p| p >= 3);

        // Built-in singletons & range.
        match receiver.kind() {
            ValueKind::Ellipsis => return Ok(Value::string("Ellipsis")),
            ValueKind::NotImplemented => return Ok(Value::string("NotImplemented")),
            ValueKind::Range { start, stop, step } => {
                return Ok(Value::tuple(vec![
                    value_class(receiver),
                    Value::tuple(vec![Value::int(start), Value::int(stop), Value::int(step)]),
                ]));
            }
            ValueKind::BigRange { start, stop, step } => {
                let start = value_from_bigint(start.clone());
                let stop = value_from_bigint(stop.clone());
                let step = value_from_bigint(step.clone());
                return Ok(Value::tuple(vec![
                    value_class(receiver),
                    Value::tuple(vec![start, stop, step]),
                ]));
            }
            // Exact `set`: `(set, (list(self),), None)` for all protocols.
            ValueKind::Set(_) => {
                let items = self.collect_iterable(receiver)?;
                return Ok(Value::tuple(vec![
                    value_class(receiver),
                    Value::tuple(vec![Value::list(items)]),
                    Value::none(),
                ]));
            }
            // Exact `frozenset` and `bytearray` (BuiltinObject).
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
            {
                let items = self.collect_iterable(receiver)?;
                return Ok(Value::tuple(vec![
                    value_class(receiver),
                    Value::tuple(vec![Value::list(items)]),
                    Value::none(),
                ]));
            }
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
            {
                let bytes =
                    pyrust_builtins::bytearray::as_bytearray_snapshot(receiver).unwrap_or_default();
                let args_tuple = if bytes.is_empty() {
                    Value::tuple(Vec::new())
                } else if proto_v3_plus {
                    Value::tuple(vec![Value::bytes(bytes)])
                } else {
                    let latin1: String = bytes.iter().map(|&b| b as char).collect();
                    Value::tuple(vec![Value::string(latin1), Value::string("latin-1")])
                };
                return Ok(Value::tuple(vec![
                    value_class(receiver),
                    args_tuple,
                    Value::none(),
                ]));
            }
            // Exact built-in types that reject `__reduce__()` and `__reduce_ex__(proto < 2)`:
            // `dict`, `list`, `tuple`, `int`, `float`, `bool`, `complex`, `str`, `bytes`, `None`.
            ValueKind::Dict(_) => {
                if !proto_v2_plus {
                    return Err(pyrust_core::type_err!("cannot pickle 'dict' object"));
                }
                let items_view = Self::dict_view_for_backing(receiver, "items", false)?;
                let dict_iter = make_iterator(self, &items_view)?;
                return Ok(Value::tuple(vec![
                    newobj_fn(),
                    Value::tuple(vec![value_class(receiver)]),
                    Value::none(),
                    Value::none(),
                    dict_iter,
                ]));
            }
            ValueKind::List(_) => {
                if !proto_v2_plus {
                    return Err(pyrust_core::type_err!("cannot pickle 'list' object"));
                }
                let list_iter = make_iterator(self, receiver)?;
                return Ok(Value::tuple(vec![
                    newobj_fn(),
                    Value::tuple(vec![value_class(receiver)]),
                    Value::none(),
                    list_iter,
                    Value::none(),
                ]));
            }
            ValueKind::Tuple(_)
            | ValueKind::Int(_)
            | ValueKind::BigInt(_)
            | ValueKind::Float(_)
            | ValueKind::Str(_)
            | ValueKind::Bytes(_) => {
                if !proto_v2_plus {
                    let type_name = pyrust_core::builtin_type_name(receiver);
                    return Err(pyrust_core::type_err!("cannot pickle '{type_name}' object"));
                }
                return Ok(Value::tuple(vec![
                    newobj_fn(),
                    Value::tuple(vec![value_class(receiver), receiver.clone()]),
                    Value::none(),
                    Value::none(),
                    Value::none(),
                ]));
            }
            ValueKind::Bool(b) => {
                if !proto_v2_plus {
                    return Err(pyrust_core::type_err!("cannot pickle 'bool' object"));
                }
                return Ok(Value::tuple(vec![
                    newobj_fn(),
                    Value::tuple(vec![value_class(receiver), Value::int(i64::from(b))]),
                    Value::none(),
                    Value::none(),
                    Value::none(),
                ]));
            }
            ValueKind::Complex(re, im) => {
                if !proto_v2_plus {
                    return Err(pyrust_core::type_err!("cannot pickle 'complex' object"));
                }
                return Ok(Value::tuple(vec![
                    newobj_fn(),
                    Value::tuple(vec![
                        value_class(receiver),
                        Value::float(re),
                        Value::float(im),
                    ]),
                    Value::none(),
                    Value::none(),
                    Value::none(),
                ]));
            }
            ValueKind::None => {
                if !proto_v2_plus {
                    return Err(pyrust_core::type_err!("cannot pickle 'NoneType' object"));
                }
                return Ok(Value::tuple(vec![
                    newobj_fn(),
                    Value::tuple(vec![value_class(receiver)]),
                    Value::none(),
                    Value::none(),
                    Value::none(),
                ]));
            }
            ValueKind::PyInstance(rc) => {
                let rc = Rc::clone(rc);
                let class = Rc::clone(&rc.borrow().class);
                // CPython `object.__reduce_ex__` delegates to an overridden
                // `__reduce__` when the class defines one other than `object.__reduce__`,
                // resolving via normal attribute lookup on `self` (#3095).
                if protocol.is_some()
                    && lookup_class_attr(&class, "__reduce__").is_some_and(|reduce_method| {
                        !value_is_canonical_slot(&reduce_method, CanonicalSlot::ObjectReduce)
                    })
                {
                    let reducer = self.get_attr(receiver, "__reduce__")?;
                    return self.call_function_expanded(reducer, &[]);
                }

                // Capture state through `__getstate__` if overridden, else default.
                let state = if let Some(getstate) = lookup_class_attr(&class, "__getstate__")
                    && !matches!(
                        getstate.kind(),
                        ValueKind::BuiltinFunction("object.__getstate__")
                    ) {
                    invoke_class_method(self, getstate, Value::py_instance(Rc::clone(&rc)), &[])?
                } else {
                    default_instance_state(&rc)
                };

                let cls_val = Value::py_class(Rc::clone(&class));
                if let Some(backing) = effective_builtin_receiver(receiver, &[]) {
                    // `set` / `frozenset` subclasses: 3-tuple for all protocols.
                    if matches!(backing.kind(), ValueKind::Set(_))
                        || matches!(
                            backing.kind(),
                            ValueKind::BuiltinObject { ops, .. }
                                if ops.canonical_class_tag()
                                    == Some(pyrust_core::CanonicalClassTag::Frozenset)
                        )
                    {
                        let items = self.collect_iterable(&backing)?;
                        return Ok(Value::tuple(vec![
                            cls_val,
                            Value::tuple(vec![Value::list(items)]),
                            state,
                        ]));
                    }
                    // `bytearray` subclasses: 3-tuple for all protocols.
                    if let Some(bytes) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing)
                    {
                        let args_tuple = if bytes.is_empty() {
                            Value::tuple(Vec::new())
                        } else if proto_v3_plus {
                            Value::tuple(vec![Value::bytes(bytes)])
                        } else {
                            let latin1: String = bytes.iter().map(|&b| b as char).collect();
                            Value::tuple(vec![Value::string(latin1), Value::string("latin-1")])
                        };
                        return Ok(Value::tuple(vec![cls_val, args_tuple, state]));
                    }

                    // Other built-in subclasses (`dict`, `list`, `tuple`, `int`,
                    // `float`, `str`, `bytes`, `complex`): succeed for all protocols.
                    if proto_v2_plus {
                        return match backing.kind() {
                            ValueKind::Dict(_) => {
                                let items_view =
                                    Self::dict_view_for_backing(&backing, "items", false)?;
                                let dict_iter = make_iterator(self, &items_view)?;
                                Ok(Value::tuple(vec![
                                    newobj_fn(),
                                    Value::tuple(vec![cls_val]),
                                    state,
                                    Value::none(),
                                    dict_iter,
                                ]))
                            }
                            ValueKind::List(_) => {
                                let list_iter = make_iterator(self, &backing)?;
                                Ok(Value::tuple(vec![
                                    newobj_fn(),
                                    Value::tuple(vec![cls_val]),
                                    state,
                                    list_iter,
                                    Value::none(),
                                ]))
                            }
                            ValueKind::Complex(re, im) => Ok(Value::tuple(vec![
                                newobj_fn(),
                                Value::tuple(vec![cls_val, Value::float(re), Value::float(im)]),
                                state,
                                Value::none(),
                                Value::none(),
                            ])),
                            _ => Ok(Value::tuple(vec![
                                newobj_fn(),
                                Value::tuple(vec![cls_val, backing.clone()]),
                                state,
                                Value::none(),
                                Value::none(),
                            ])),
                        };
                    }

                    // Protocol < 2 / `__reduce__()` on built-in subclass:
                    // `(copyreg._reconstructor, (cls, base, base_state)[, state])`.
                    let base_cls = value_class(&backing);
                    let base_state = match backing.kind() {
                        ValueKind::Dict(d) => Value::dict(d.clone()),
                        ValueKind::List(items) => Value::list(items.iter().cloned().collect()),
                        _ => backing.clone(),
                    };
                    let rec_args = Value::tuple(vec![cls_val, base_cls, base_state]);
                    return Ok(if state.is_none() {
                        Value::tuple(vec![reconstructor_fn(), rec_args])
                    } else {
                        Value::tuple(vec![reconstructor_fn(), rec_args, state])
                    });
                }

                // Plain user instance or `object()`.
                if proto_v2_plus {
                    return Ok(Value::tuple(vec![
                        newobj_fn(),
                        Value::tuple(vec![cls_val]),
                        state,
                        Value::none(),
                        Value::none(),
                    ]));
                }
                let rec_args = Value::tuple(vec![
                    cls_val,
                    Value::py_class(object_class_singleton()),
                    Value::none(),
                ]);
                return Ok(if state.is_none() {
                    Value::tuple(vec![reconstructor_fn(), rec_args])
                } else {
                    Value::tuple(vec![reconstructor_fn(), rec_args, state])
                });
            }
            _ => {}
        }

        Ok(Value::tuple(vec![
            value_class(receiver),
            Value::tuple(Vec::new()),
        ]))
    }
}
