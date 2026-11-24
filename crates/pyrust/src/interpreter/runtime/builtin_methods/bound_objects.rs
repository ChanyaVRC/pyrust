impl Interpreter {
    fn call_builtin_object_bound_method(
        &mut self,
        receiver: &Value,
        method: &str,
        pos: &mut Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        let ValueKind::BuiltinObject { ops, state } = receiver.kind() else {
            unreachable!("receiver family checked by bound method dispatcher");
        };
        if pyrust_builtins::memoryview::is_memoryview_ops(ops) {
            match method {
                "__enter__" => {
                    if !kw.is_empty() || !pos.is_empty() {
                        return Err(PyError::named(
                            "TypeError",
                            "memoryview.__enter__() takes no arguments".to_string(),
                        ));
                    }
                    if pyrust_builtins::memoryview::is_released(receiver) {
                        return Err(pyrust_builtins::memoryview::released_error());
                    }
                    return Ok(receiver.clone());
                }
                "release" => {
                    if !kw.is_empty() {
                        return Err(PyError::named(
                            "TypeError",
                            "memoryview.release() takes no keyword arguments".to_string(),
                        ));
                    }
                    if !pos.is_empty() {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "memoryview.release() takes no arguments ({} given)",
                                pos.len()
                            ),
                        ));
                    }
                    crate::interpreter::release_memoryview_with_interp(self, receiver)?;
                    return Ok(Value::none());
                }
                "__exit__" => {
                    crate::interpreter::release_memoryview_with_interp(self, receiver)?;
                    return Ok(Value::none());
                }
                _ => {}
            }
        }
        if pyrust_builtins::mapping_proxy::is_object_proxy_ops(ops)
            && matches!(
                method,
                "keys" | "values" | "items" | "get" | "copy" | "__reversed__"
            )
        {
            // Validate the mappingproxy wrapper before owner lookup, then use
            // ordinary attribute lookup/call so instance attributes, dynamic
            // __getattribute__, and missing owner methods remain visible.
            // Clone state data before any user callback runs.
            pyrust_builtins::mapping_proxy::validate_method_call(
                method,
                pos.len(),
                !kw.is_empty(),
            )?;
            let owner = pyrust_builtins::mapping_proxy::owner_from_state(state)
                .expect("object mappingproxy state");
            let mut forwarded = ExpandedArgBuf::with_capacity(pos.len());
            forwarded.extend(
                pos.iter()
                    .cloned()
                    .map(|value| ExpandedCallArg { name: None, value }),
            );
            let callable = self.get_attr(&owner, method)?;
            return self.call_function_expanded(callable, &forwarded);
        }
        // Issue #3019: route exact mappingproxy `.get()` through interpreter
        // key conversion so unhashable keys raise `TypeError: unhashable type`
        // and custom `__hash__`/`__eq__` keys work identically to `dict.get()`.
        if pyrust_builtins::mapping_proxy::is_exact_proxy_ops(ops) && method == "get" {
            pyrust_builtins::mapping_proxy::validate_method_call(
                method,
                pos.len(),
                !kw.is_empty(),
            )?;
            let key_val = pos[0].clone();
            let default = pos.get(1).cloned().unwrap_or_else(Value::none);
            if let Some(dict_rc) = pyrust_builtins::mapping_proxy::as_dict_rc(receiver) {
                let dict_val = Value::dict_shared(dict_rc);
                let found = self.dict_lookup_value(&dict_val, &key_val)?;
                return Ok(found.map_or(default, |(_, v)| v));
            }
            if let Some(class_rc) = pyrust_builtins::mapping_proxy::as_class_rc(receiver) {
                if let Some(s) = key_val.as_str() {
                    let found = class_rc.borrow().attrs.get(s).cloned();
                    return Ok(found.map_or(default, |v| {
                        pyrust_builtins::mapping_proxy::expose_class_attr(&v)
                    }));
                }
                let key = self.value_to_pykey(&key_val)?;
                if let PyKey::Str(s) = key
                    && let Some(name) = s.as_str()
                    && let Some(v) = class_rc.borrow().attrs.get(name).cloned()
                {
                    return Ok(pyrust_builtins::mapping_proxy::expose_class_attr(&v));
                }
                return Ok(default);
            }
        }
        // Validate before bytearray join/extend can consume an iterable. The
        // concrete type modules own the method-name policy; this adapter only
        // selects them by stable runtime identity.
        match ops.canonical_class_tag() {
            Some(pyrust_core::CanonicalClassTag::Bytearray) => {
                pyrust_builtins::bytearray::validate_method_keywords(method, !kw.is_empty())?;
            }
            Some(pyrust_core::CanonicalClassTag::Frozenset) => {
                pyrust_builtins::frozenset::validate_method_keywords(method, !kw.is_empty())?;
            }
            _ if pyrust_builtins::slice::is_slice_ops(ops) => {
                pyrust_builtins::slice::validate_method_keywords(method, !kw.is_empty())?;
            }
            _ => {}
        }
        let mut args_vec: Vec<Value> = std::mem::take(pos);
        let bytearray =
            ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray);
        let splitlines = bytearray && method == "splitlines";
        if bytearray
            && let Some(bound) = self.bind_bytearray_splitlines_keepends(method, &args_vec, kw)?
        {
            args_vec = bound;
        }
        // bytearray methods accept bytes-subclass / bytearray args
        // (#1928); coerce them to a real `Bytes` value before the
        // receiver-only ops extractors (which match exact `Bytes`) see
        // them.  Other BuiltinObject types (frozenset) are untouched.
        if bytearray {
            if method == "join" {
                // #2538: drive a lazy-iterator `join` argument through the
                // interpreter (the ops table can only drain a
                // NativeIterFrame) and coerce its bytes-subclass /
                // bytearray elements.
                args_vec = self.prepare_bytearray_join_args(args_vec)?;
            } else {
                args_vec = coerce_bytes_subclass_method_args(method, args_vec);
            }
            // #2532: drive a lazy-iterator `extend` argument through the
            // interpreter before the receiver-only ops table sees it.
            if method == "extend" {
                args_vec = self.prepare_bytearray_extend_args(args_vec)?;
            }
        }
        let empty_kw = PyDict::default();
        let coerced_kw = if bytearray && !splitlines {
            coerce_bytes_subclass_method_kwargs(kw)
        } else {
            None
        };
        let kw = if splitlines {
            &empty_kw
        } else {
            coerced_kw.as_ref().unwrap_or(kw)
        };
        // Thread any keyword arguments through to the builtin object
        // (e.g. `bytearray.split(maxsplit=1)`); `call_method` keeps its
        // kwargs `String`-keyed.
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
        ops.call_method(state, method, args_vec, &kw_str)
    }
}
