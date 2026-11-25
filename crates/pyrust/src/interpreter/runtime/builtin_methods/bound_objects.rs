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
        if pyrust_builtins::frame::is_frame(receiver) && method == "clear" {
            if !kw.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "frame.clear() takes no keyword arguments".to_string(),
                ));
            }
            if !pos.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("frame.clear() takes no arguments ({} given)", pos.len()),
                ));
            }
            if self.is_frame_currently_executing(receiver) {
                return Err(PyError::named(
                    "RuntimeError",
                    "cannot clear an executing frame".to_string(),
                ));
            }
            pyrust_builtins::frame::clear_inactive_frame(receiver);
            return Ok(Value::none());
        }
        if pyrust_builtins::code::is_code(receiver) {
            match method {
                "co_lines" => {
                    if !kw.is_empty() {
                        return Err(PyError::named(
                            "TypeError",
                            "code.co_lines() takes no keyword arguments".to_string(),
                        ));
                    }
                    if !pos.is_empty() {
                        return Err(PyError::named(
                            "TypeError",
                            format!("code.co_lines() takes no arguments ({} given)", pos.len()),
                        ));
                    }
                    let items: Vec<Value> =
                        pyrust_builtins::code::with_code_state_ref(receiver, |cs| {
                            cs.lines
                                .iter()
                                .map(|&(start, end, ln)| {
                                    Value::tuple(vec![
                                        Value::int(start),
                                        Value::int(end),
                                        ln.map_or_else(Value::none, Value::int),
                                    ])
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    return make_iterator(self, &Value::list(items));
                }
                "co_positions" => {
                    if !kw.is_empty() {
                        return Err(PyError::named(
                            "TypeError",
                            "code.co_positions() takes no keyword arguments".to_string(),
                        ));
                    }
                    if !pos.is_empty() {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "code.co_positions() takes no arguments ({} given)",
                                pos.len()
                            ),
                        ));
                    }
                    let items: Vec<Value> =
                        pyrust_builtins::code::with_code_state_ref(receiver, |cs| {
                            cs.positions
                                .iter()
                                .map(|&(ln, end_ln, col, end_col)| {
                                    Value::tuple(vec![
                                        ln.map_or_else(Value::none, Value::int),
                                        end_ln.map_or_else(Value::none, Value::int),
                                        col.map_or_else(Value::none, Value::int),
                                        end_col.map_or_else(Value::none, Value::int),
                                    ])
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    return make_iterator(self, &Value::list(items));
                }
                "replace" => {
                    return self.call_code_replace(receiver, pos, kw);
                }
                _ => {}
            }
        }
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

    /// Implement CPython 3.12 `code.replace(**changes)`, returning a new `code`
    /// object with the specified fields replaced.
    fn call_code_replace(&mut self, receiver: &Value, pos: &[Value], kw: &PyDict) -> Result<Value> {
        if !pos.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "replace() takes at most 0 positional arguments ({} given)",
                    pos.len()
                ),
            ));
        }
        let extract_tuple = |v: &Value| -> Vec<Value> {
            match v.kind() {
                ValueKind::Tuple(t) => t.to_vec(),
                _ => Vec::new(),
            }
        };
        let Some((
            mut name,
            mut qualname,
            mut argcount,
            mut posonlyargcount,
            mut kwonlyargcount,
            mut nlocals,
            mut stacksize,
            mut varnames,
            mut flags,
            mut filename,
            mut firstlineno,
            mut consts,
            mut names,
            mut freevars,
            mut cellvars,
            mut code_bytes,
            mut linetable,
            mut exceptiontable,
            mut lines,
            mut positions,
            raw_code,
            exec_meta,
        )) = pyrust_builtins::code::with_code_state_ref(receiver, |cs| {
            (
                cs.name.clone(),
                cs.qualname.clone(),
                cs.argcount,
                cs.posonlyargcount,
                cs.kwonlyargcount,
                cs.nlocals,
                cs.stacksize,
                extract_tuple(&cs.varnames),
                cs.flags,
                cs.filename.clone(),
                cs.firstlineno,
                extract_tuple(&cs.consts),
                extract_tuple(&cs.names),
                extract_tuple(&cs.freevars),
                extract_tuple(&cs.cellvars),
                cs.code_bytes.clone(),
                cs.linetable.clone(),
                cs.exceptiontable.clone(),
                cs.lines.clone(),
                cs.positions.clone(),
                cs.raw_code.clone(),
                cs.exec_meta.clone(),
            )
        })
        else {
            return Err(PyError::named("TypeError", "expected code object"));
        };

        let old_firstlineno = firstlineno;
        let mut consts_changed = false;
        let mut names_changed = false;
        let mut linetable_explicit = false;

        let require_int = |key: &str, v: &Value| -> Result<i64> {
            match v.kind() {
                ValueKind::Int(n) => Ok(n),
                ValueKind::Bool(b) => Ok(i64::from(b)),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "replace() argument '{key}' must be int, not {}",
                        value_type_name_str(v)
                    ),
                )),
            }
        };
        let require_str = |key: &str, v: &Value| -> Result<String> {
            v.as_str().map(str::to_owned).ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    format!(
                        "replace() argument '{key}' must be str, not {}",
                        value_type_name_str(v)
                    ),
                )
            })
        };
        let require_bytes = |key: &str, v: &Value| -> Result<Vec<u8>> {
            match v.kind() {
                ValueKind::Bytes(b) => Ok((**b).clone()),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "replace() argument '{key}' must be bytes, not {}",
                        value_type_name_str(v)
                    ),
                )),
            }
        };
        let require_tuple = |key: &str, v: &Value| -> Result<Vec<Value>> {
            match v.kind() {
                ValueKind::Tuple(t) => Ok(t.to_vec()),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "replace() argument '{key}' must be tuple, not {}",
                        value_type_name_str(v)
                    ),
                )),
            }
        };
        let require_str_tuple = |key: &str, v: &Value| -> Result<Vec<Value>> {
            let items = require_tuple(key, v)?;
            for item in &items {
                if item.as_str().is_none() {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "replace() argument '{key}' must contain only str, not {}",
                            value_type_name_str(item)
                        ),
                    ));
                }
            }
            Ok(items)
        };

        for (k, v) in kw.iter() {
            let key = match k {
                PyKey::Str(s) => s.as_str().unwrap_or(""),
                _ => "",
            };
            match key {
                "co_argcount" => argcount = require_int(key, v)?,
                "co_posonlyargcount" => posonlyargcount = require_int(key, v)?,
                "co_kwonlyargcount" => kwonlyargcount = require_int(key, v)?,
                "co_nlocals" => nlocals = require_int(key, v)?,
                "co_stacksize" => stacksize = require_int(key, v)?,
                "co_flags" => flags = require_int(key, v)?,
                "co_firstlineno" => firstlineno = require_int(key, v)?,
                "co_code" => code_bytes = require_bytes(key, v)?,
                "co_consts" => {
                    consts = require_tuple(key, v)?;
                    consts_changed = true;
                }
                "co_names" => {
                    names = require_str_tuple(key, v)?;
                    names_changed = true;
                }
                "co_varnames" => varnames = require_str_tuple(key, v)?,
                "co_freevars" => freevars = require_str_tuple(key, v)?,
                "co_cellvars" => cellvars = require_str_tuple(key, v)?,
                "co_filename" => filename = require_str(key, v)?,
                "co_name" => {
                    let new_name = require_str(key, v)?;
                    if !kw.contains_key(&PyKey::str_from("co_qualname")) && qualname == name {
                        qualname = new_name.clone();
                    }
                    name = new_name;
                }
                "co_qualname" => qualname = require_str(key, v)?,
                "co_linetable" => {
                    linetable = require_bytes(key, v)?;
                    linetable_explicit = true;
                }
                "co_exceptiontable" => exceptiontable = require_bytes(key, v)?,
                other => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("'{other}' is an invalid keyword argument for replace()"),
                    ));
                }
            }
        }

        // Shift cached line/position metadata if co_firstlineno changed.
        let line_delta = firstlineno - old_firstlineno;
        if line_delta != 0 {
            for (_, _, ln) in &mut lines {
                if let Some(l) = ln {
                    *l += line_delta;
                }
            }
            for (ln, end_ln, _, _) in &mut positions {
                if let Some(l) = ln {
                    *l += line_delta;
                }
                if let Some(el) = end_ln {
                    *el += line_delta;
                }
            }
            if !linetable_explicit {
                linetable = pyrust_builtins::code::encode_linetable(&lines, firstlineno);
            }
        }

        // Rebuild cloned FnCode & exec_meta when underlying code exists so
        // `exec(co.replace(...))` observes updated filename/name/consts/names.
        let (new_raw_code, new_exec_meta) = if let Some(raw_rc) = raw_code
            && let Some(orig_fc) = raw_rc.downcast_ref::<crate::bytecode::FnCode>()
        {
            let mut cloned_fc = orig_fc.clone();
            cloned_fc.filename = std::sync::Arc::from(filename.as_str());
            cloned_fc.qualname = std::sync::Arc::from(qualname.as_str());
            cloned_fc.first_lineno = firstlineno.max(0) as u32;
            if consts_changed {
                cloned_fc.consts = consts.clone();
            }
            if names_changed {
                cloned_fc.names = names
                    .iter()
                    .filter_map(|v| v.as_str().map(pyrust_core::Symbol::intern))
                    .collect();
            }
            cloned_fc.cached_code_object = RefCell::new(None);
            let new_fc = Rc::new(cloned_fc);
            let updated_meta = exec_meta.and_then(|meta_rc| {
                meta_rc
                    .downcast_ref::<crate::interpreter::CodeState>()
                    .map(|cs| {
                        let meta: Rc<dyn std::any::Any> = Rc::new(crate::interpreter::CodeState {
                            code: Rc::clone(&new_fc),
                            mode: cs.mode,
                            local_index: Rc::clone(&cs.local_index),
                        });
                        meta
                    })
            });
            (Some(new_fc as Rc<dyn std::any::Any>), updated_meta)
        } else {
            (None, exec_meta)
        };

        Ok(pyrust_builtins::code::CodeBuild {
            name,
            qualname,
            argcount,
            posonlyargcount,
            kwonlyargcount,
            nlocals,
            stacksize,
            varnames,
            flags,
            filename,
            firstlineno,
            consts,
            names,
            freevars,
            cellvars,
            code_bytes,
            linetable,
            exceptiontable,
            lines,
            positions,
            raw_code: new_raw_code,
            exec_meta: new_exec_meta,
        }
        .build())
    }
}
