impl Interpreter {
    /// Materialise a callback-capable bytearray slice RHS before entering the
    /// interpreter-free storage implementation.
    ///
    /// `pyrust-builtins` can validate bytes and already-materialised integer
    /// sequences, but it cannot drive a VM generator or call a user
    /// `__iter__`/`__index__`. Keep those callbacks at this boundary and pass
    /// the storage layer an owned list after every callback has completed.
    fn prepare_bytearray_slice_rhs(&mut self, value: Value) -> Result<Value> {
        let needs_iterator = match value.kind() {
            ValueKind::Generator(_) | ValueKind::PyInstance(_) | ValueKind::PyClass(_) => true,
            ValueKind::BuiltinObject { ops, .. } => {
                pyrust_builtins::mapping_proxy::is_object_proxy_ops(ops)
            }
            _ => false,
        };
        let mut items = if needs_iterator {
            let type_name = value_type_name_str(&value).into_owned();
            let iterator = match crate::interpreter::make_iterator(self, &value) {
                Ok(iterator) => iterator,
                Err(error) if error.class_name_is("TypeError") => {
                    return Err(pyrust_core::type_err!(
                        "cannot convert '{}' object to bytearray",
                        type_name
                    ));
                }
                Err(error) => return Err(error),
            };
            self.collect_iterable(&iterator)?
        } else {
            let materialized = match value.kind() {
                ValueKind::List(items) => Some(items.to_vec()),
                ValueKind::Tuple(items) => Some(items.to_vec()),
                _ => None,
            };
            let Some(materialized) = materialized else {
                return Ok(value);
            };
            materialized
        };

        for item in &mut items {
            if matches!(item.kind(), ValueKind::PyInstance(_)) {
                *item = self.resolve_byte_value(item.clone())?;
            }
        }
        Ok(Value::list(items))
    }

    /// Evaluate `obj[idx]` and return the result.
    ///
    /// Extracted from the `GetItem` VM dispatch arm so that changes to
    /// subscript-access semantics (__getitem__, slice handling, etc.) only
    /// require touching this method rather than vm.rs.
    pub(crate) fn exec_get_item(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        idx: crate::bytecode::Reg,
    ) -> Result<Value> {
        let fast_int_idx = regs[idx as usize].as_int();
        if let Some(raw_i) = fast_int_idx {
            enum Got {
                Item(Value),
                ListOOR,
                TupleOOR,
                None,
            }
            let got = match regs[obj as usize].as_some().map(|v| v.kind()) {
                Some(ValueKind::List(items)) => {
                    let len = items.len() as i64;
                    let j = if raw_i < 0 { raw_i + len } else { raw_i };
                    if j >= 0 && (j as usize) < items.len() {
                        Got::Item(items[j as usize].clone())
                    } else {
                        Got::ListOOR
                    }
                }
                Some(ValueKind::Tuple(items)) => {
                    let len = items.len() as i64;
                    let j = if raw_i < 0 { raw_i + len } else { raw_i };
                    if j >= 0 && (j as usize) < items.len() {
                        Got::Item(items[j as usize].clone())
                    } else {
                        Got::TupleOOR
                    }
                }
                _ => Got::None,
            };
            match got {
                Got::Item(v) => return Ok(v),
                Got::ListOOR => {
                    return Err(pyrust_core::index_err!("list index out of range"));
                }
                Got::TupleOOR => {
                    return Err(pyrust_core::index_err!("tuple index out of range"));
                }
                Got::None => {}
            }
        }

        let idx_val = vm_read(regs, idx, num_locals)?;
        let obj_is_mapping = matches!(
            regs[obj as usize].as_some().map(|v| v.kind()),
            Some(ValueKind::Dict(_) | ValueKind::BuiltinObject { .. })
        );
        if !obj_is_mapping && let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
            let obj_val = vm_read(regs, obj, num_locals)?;
            return self.eval_slice(&obj_val, lo, hi, st);
        }
        enum FastResult {
            Value(Value),
            DictLookup(Value),
            Miss,
        }
        let fast = if let Some(ov) = regs[obj as usize].as_some() {
            match ov.kind() {
                ValueKind::List(items) => {
                    if !matches!(
                        idx_val.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    ) {
                        FastResult::Miss
                    } else {
                        let i = normalize_index(&idx_val, items.len(), "list")?;
                        FastResult::Value(items[i].clone())
                    }
                }
                ValueKind::Tuple(items) => {
                    if !matches!(
                        idx_val.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    ) {
                        FastResult::Miss
                    } else {
                        let i = normalize_index(&idx_val, items.len(), "tuple")?;
                        FastResult::Value(items[i].clone())
                    }
                }
                ValueKind::Dict(_) => FastResult::DictLookup(ov.clone()),
                _ => FastResult::Miss,
            }
        } else {
            FastResult::Miss
        };
        match fast {
            FastResult::Value(r) => Ok(r),
            FastResult::DictLookup(dict_val) => {
                let lookup = if let Some(s) = idx_val.as_str() {
                    self.dict_str_lookup(&dict_val, s)?
                } else {
                    let key = self.value_to_pykey(&idx_val)?;
                    self.dict_lookup(&dict_val, &key)?
                };
                lookup
                    .map(|(_, v)| v)
                    .ok_or_else(|| PyError::key_error(idx_val.clone()))
            }
            FastResult::Miss => {
                let obj_val = vm_read(regs, obj, num_locals)?;
                // bytearray honors __index__ on a non-int subscript like bytes
                // (#1908).  Resolve only for a PyInstance index — this check is
                // reached only after the int/list/tuple fast paths miss, so the
                // hot `ba[i]` path never runs it.
                if matches!(idx_val.kind(), ValueKind::PyInstance(_))
                    && matches!(
                        obj_val.kind(),
                        ValueKind::BuiltinObject { ops, .. }
                            if ops.canonical_class_tag()
                                == Some(pyrust_core::CanonicalClassTag::Bytearray)
                    )
                {
                    let resolved = self.call_index_protocol(&idx_val, "bytearray")?;
                    return self.eval_index(&obj_val, resolved);
                }
                self.eval_index(&obj_val, idx_val)
            }
        }
    }

    /// Execute an rvalue slice read `obj[lo:hi:step]` (the `GetSlice` opcode,
    /// CPython BINARY_SLICE analogue).  Reads the three contiguous bound
    /// registers (`base`, `base+1`, `base+2`) and slices `obj` directly via
    /// `eval_slice`, which only materialises a real `slice` object for the
    /// PyInstance `__getitem__` / BuiltinObject paths — built-in sequences skip
    /// the per-access `slice`-object allocation entirely (#1964).
    pub(crate) fn exec_get_slice(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        base: crate::bytecode::Reg,
    ) -> Result<Value> {
        let start = vm_read(regs, base, num_locals)?;
        let stop = vm_read(regs, base + 1, num_locals)?;
        let step = vm_read(regs, base + 2, num_locals)?;
        let lo = if start.is_none() { None } else { Some(start) };
        let hi = if stop.is_none() { None } else { Some(stop) };
        let st = if step.is_none() { None } else { Some(step) };
        // Tuple payloads are reference counted, so this clone is O(1). Keeping
        // an owned Value also ensures that any user `__index__` callback invoked
        // while resolving a bound may reassign the source register without
        // invalidating a borrow into that register.
        let obj_val = vm_read(regs, obj, num_locals)?;
        // Mapping targets (dict) treat slice notation as a *key lookup*, not a
        // slice: `d[1:2]` builds the slice object and looks it up as a key
        // (KeyError if absent), matching CPython and the prior BuildSlice +
        // GetItem path.  Build a real slice object and dispatch through
        // eval_index so the dict lookup runs.  eval_slice handles every other
        // target (built-in sequences, range, BuiltinObject, PyInstance).
        if matches!(obj_val.kind(), ValueKind::Dict(_)) {
            let slice_val = make_slice_value(lo, hi, st);
            return self.eval_index(&obj_val, slice_val);
        }
        self.eval_slice(&obj_val, lo, hi, st)
    }

    /// Execute `obj[idx] = val`.
    ///
    /// Extracted from the `SetItem` VM dispatch arm so that changes to
    /// subscript-assignment semantics (__setitem__, slice assignment, etc.)
    /// only require touching this method rather than vm.rs.
    pub(crate) fn exec_set_item(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        idx: crate::bytecode::Reg,
        val: crate::bytecode::Reg,
    ) -> Result<()> {
        if let Some(raw_i) = regs[idx as usize].as_int()
            && let Some(len) = regs[obj as usize].list_len()
        {
            let j = if raw_i < 0 { raw_i + len as i64 } else { raw_i };
            if j >= 0 && (j as usize) < len {
                let v = regs[val as usize].clone();
                regs[obj as usize].list_with_mut(|items| {
                    items[j as usize] = v;
                });
            } else {
                return Err(pyrust_core::index_err!(
                    "list assignment index out of range"
                ));
            }
            return Ok(());
        }
        let idx_val = vm_read(regs, idx, num_locals)?;
        let val_val = vm_read(regs, val, num_locals)?;
        let is_list_target = regs[obj as usize].list_len().is_some();
        if is_list_target && let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
            let lo = self.resolve_slice_bound_val(lo)?;
            let hi = self.resolve_slice_bound_val(hi)?;
            let st = self.resolve_slice_bound_val(st)?;
            let new_items: Vec<Value> = match val_val.kind() {
                ValueKind::List(v) => Some(v.to_vec()),
                _ => None,
            }
            .unwrap_or_else(Vec::new);
            let new_items = if !new_items.is_empty() || matches!(val_val.kind(), ValueKind::List(_))
            {
                new_items
            } else {
                self.collect_iterable(&val_val)
                    .map_err(|_| pyrust_core::type_err!("can only assign an iterable"))?
            };
            let updated = regs[obj as usize].list_with_mut(|items| {
                Self::slice_setitem(items, lo.as_ref(), hi.as_ref(), st.as_ref(), new_items)
            });
            return match updated {
                Some(r) => r,
                None => {
                    let tname = value_type_name_str(&regs[obj as usize]);
                    Err(pyrust_core::type_err!(
                        "'{}' object does not support item assignment",
                        tname
                    ))
                }
            };
        }
        let target_kind = regs[obj as usize]
            .as_some()
            .map(|v| match v.kind() {
                ValueKind::List(_) => 1u8,
                ValueKind::Dict(_) => 2u8,
                ValueKind::PyInstance(_) => 3u8,
                ValueKind::BuiltinObject { .. } => 4u8,
                _ => 0u8,
            })
            .unwrap_or(0);
        match target_kind {
            1 => {
                let len = regs[obj as usize].list_len().unwrap_or(0);
                let idx_resolved = self.call_index_protocol(&idx_val, "list")?;
                let i = normalize_index_write(&idx_resolved, len, "list")?;
                regs[obj as usize].list_with_mut(|items| {
                    items[i] = val_val;
                });
            }
            2 => {
                self.set_item_into_dict(regs, obj, idx_val, val_val)?;
            }
            3 => {
                let obj_val = vm_read(regs, obj, num_locals)?;
                if let ValueKind::PyInstance(inst) = obj_val.kind() {
                    let inst_rc = Rc::clone(inst);
                    if let Some(backing) = builtin_data_backing(&obj_val) {
                        enum BkKind {
                            Dict,
                            List,
                            Other,
                        }
                        let bk_kind = match backing.kind() {
                            ValueKind::Dict(_) => BkKind::Dict,
                            ValueKind::List(_) => BkKind::List,
                            _ => BkKind::Other,
                        };
                        // A dict subclass override is observable for every key
                        // type: it may validate, transform, log, or decline the
                        // store altogether.  Restricting this probe to object
                        // keys bypassed user `__setitem__` for int/str keys.
                        if matches!(bk_kind, BkKind::Dict) {
                            let class = Rc::clone(&inst_rc.borrow().class);
                            let user_setitem =
                                lookup_class_attr(&class, "__setitem__").filter(|v| {
                                    inherited_primitive_builtin_slot_kind(&class, "__setitem__", v)
                                        .is_none()
                                });
                            if let Some(method_val) = user_setitem {
                                invoke_class_method(
                                    self,
                                    method_val,
                                    Value::py_instance(inst_rc),
                                    &[
                                        ExpandedCallArg {
                                            name: None,
                                            value: idx_val,
                                        },
                                        ExpandedCallArg {
                                            name: None,
                                            value: val_val,
                                        },
                                    ],
                                )?;
                                return Ok(());
                            }
                        }
                        match bk_kind {
                            BkKind::Dict => {
                                let key = self.value_to_pykey(&idx_val)?;
                                self.dict_insert_value(&backing, key, val_val)?;
                                return Ok(());
                            }
                            BkKind::List => {
                                let len = backing.list_len().unwrap_or(0);
                                let idx_resolved = self.call_index_protocol(&idx_val, "list")?;
                                let i = normalize_index_write(&idx_resolved, len, "list")?;
                                backing.list_with_mut(|items| {
                                    items[i] = val_val;
                                });
                                return Ok(());
                            }
                            BkKind::Other => {}
                        }
                    }
                    let class = Rc::clone(&inst_rc.borrow().class);
                    if let Some(method_val) = lookup_class_attr(&class, "__setitem__") {
                        invoke_class_method(
                            self,
                            method_val,
                            Value::py_instance(inst_rc),
                            &[
                                ExpandedCallArg {
                                    name: None,
                                    value: idx_val,
                                },
                                ExpandedCallArg {
                                    name: None,
                                    value: val_val,
                                },
                            ],
                        )?;
                        return Ok(());
                    }
                    let class_name = class.borrow().name.clone();
                    return Err(pyrust_core::type_err!(
                        "'{}' object does not support item assignment",
                        class_name
                    ));
                }
                let tname = value_type_name_str(&regs[obj as usize]);
                return Err(pyrust_core::type_err!(
                    "'{}' object does not support item assignment",
                    tname
                ));
            }
            4 => {
                // bytearray item assignment honors the __index__ protocol on
                // both the index and the assigned value (#1908). bytearray's
                // receiver-only set_item can't reach user dunders, so resolve
                // here before delegating. The hot `ba[i] = v` int/bool path is
                // untouched: protocol resolution only runs when the index is a
                // PyInstance / slice object or the value is a PyInstance — a
                // plain-int index with a plain-int/bool value skips it entirely
                // and goes straight to set_item, matching master.
                let needs_resolve = matches!(
                    idx_val.kind(),
                    ValueKind::PyInstance(_) | ValueKind::BuiltinObject { .. }
                ) || matches!(val_val.kind(), ValueKind::PyInstance(_));
                let (idx_val, val_val) = if needs_resolve
                    && matches!(
                        vm_read(regs, obj, num_locals)?.kind(),
                        ValueKind::BuiltinObject { ops, .. }
                            if ops.canonical_class_tag()
                                == Some(pyrust_core::CanonicalClassTag::Bytearray)
                    ) {
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        // Slice assignment: resolve the __index__ bounds and
                        // rebuild the slice; element resolution stays in
                        // set_item (#1908).
                        let lo = self.resolve_slice_bound_val(lo)?;
                        let hi = self.resolve_slice_bound_val(hi)?;
                        let st = self.resolve_slice_bound_val(st)?;
                        (make_slice_value(lo, hi, st), val_val)
                    } else {
                        let resolved_idx = self.call_index_protocol(&idx_val, "bytearray")?;
                        // Resolve the assigned value's __index__ only when it is
                        // a PyInstance carrying one; otherwise leave it untouched
                        // so set_item's value_to_byte produces the correct error
                        // ("byte must be in range(0, 256)" / "'X' object cannot be
                        // interpreted as an integer").
                        let resolved_val = self.resolve_byte_value(val_val)?;
                        (resolved_idx, resolved_val)
                    }
                } else {
                    (idx_val, val_val)
                };
                let val_val = if Self::unpack_slice_key(&idx_val).is_some() {
                    self.prepare_bytearray_slice_rhs(val_val)?
                } else {
                    val_val
                };
                let obj_val = vm_read(regs, obj, num_locals)?;
                if let ValueKind::BuiltinObject { ops, state } = obj_val.kind() {
                    ops.set_item(state, &idx_val, val_val)?;
                }
            }
            _ => {
                let tname = value_type_name_str(&regs[obj as usize]);
                return Err(pyrust_core::type_err!(
                    "'{}' object does not support item assignment",
                    tname
                ));
            }
        }
        Ok(())
    }

    /// Assign into a dict target for `obj[idx] = val`, including the
    /// module-globals write-through (issue #970): when the dict is
    /// `module_globals_dict`, mirror the write to the script frame's
    /// fastlocal register and bump the LoadGlobal cache version.
    fn set_item_into_dict(
        &mut self,
        regs: &mut RegSlice,
        obj: crate::bytecode::Reg,
        idx_val: Value,
        val_val: Value,
    ) -> Result<()> {
        let key = self.value_to_pykey(&idx_val)?;
        let dict_val = regs[obj as usize]
            .as_some()
            .cloned()
            .unwrap_or(Value::none());
        self.dict_insert_value(&dict_val, key, val_val)
    }

    /// Execute `del obj[idx]`.
    ///
    /// Extracted from the `DeleteItem` VM dispatch arm so that changes to
    /// subscript-deletion semantics (__delitem__, slice deletion, etc.) only
    /// require touching this method rather than vm.rs.
    pub(crate) fn exec_delete_item(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        idx: crate::bytecode::Reg,
    ) -> Result<()> {
        let idx_val = vm_read(regs, idx, num_locals)?;
        let is_list_target = regs[obj as usize].list_len().is_some();
        if is_list_target && let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
            let lo = self.resolve_slice_bound_val(lo)?;
            let hi = self.resolve_slice_bound_val(hi)?;
            let st = self.resolve_slice_bound_val(st)?;
            let updated = regs[obj as usize].list_with_mut(|items| {
                Self::slice_delitem(items, lo.as_ref(), hi.as_ref(), st.as_ref())
            });
            return match updated {
                Some(r) => r,
                None => {
                    let tname = value_type_name_str(&regs[obj as usize]);
                    Err(pyrust_core::type_err!(
                        "'{}' object does not support item deletion",
                        tname
                    ))
                }
            };
        }
        let target_kind = regs[obj as usize]
            .as_some()
            .map(|v| match v.kind() {
                ValueKind::List(_) => 1u8,
                ValueKind::Dict(_) => 2u8,
                ValueKind::BuiltinObject { .. } => 3u8,
                _ => 0u8,
            })
            .unwrap_or(0);
        if target_kind == 1 {
            let len = regs[obj as usize].list_len().unwrap_or(0);
            let idx_resolved = self.call_index_protocol(&idx_val, "list")?;
            let i = normalize_index_write(&idx_resolved, len, "list")?;
            regs[obj as usize].list_with_mut(|items| {
                if i + 1 == items.len() {
                    items.pop();
                } else {
                    items.remove(i);
                }
            });
            return Ok(());
        }
        if target_kind == 2 {
            let key = self.value_to_pykey(&idx_val)?;
            // Route every key through the shared fast-get/eq-aware lookup so
            // primitive probes can delete an equal stored Object (#2820).
            let dict_val = regs[obj as usize]
                .as_some()
                .cloned()
                .unwrap_or(Value::none());
            let found = self.dict_lookup(&dict_val, &key)?;
            if let Some((idx, _)) = found {
                regs[obj as usize].dict_with_mut(|dict| {
                    dict.shift_remove_index(idx);
                });
            } else {
                return Err(PyError::key_error(idx_val.clone()));
            }
            return Ok(());
        }
        if target_kind == 3 {
            let obj_val = vm_read(regs, obj, num_locals)?;
            if let ValueKind::BuiltinObject { ops, state } = obj_val.kind() {
                ops.delete_item(state, &idx_val)?;
            }
            return Ok(());
        }
        let obj_val = vm_read(regs, obj, num_locals)?;
        if let ValueKind::PyInstance(inst) = obj_val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__delitem__") {
                invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[ExpandedCallArg {
                        name: None,
                        value: idx_val,
                    }],
                )?;
                return Ok(());
            }
            let class_name = class.borrow().name.clone();
            return Err(pyrust_core::type_err!(
                "'{class_name}' object does not support item deletion"
            ));
        }
        let tname = value_type_name_str(&regs[obj as usize]);
        let msg = if Self::unpack_slice_key(&idx_val).is_some() {
            format!("'{}' object does not support item deletion", tname)
        } else {
            format!("'{}' object doesn't support item deletion", tname)
        };
        Err(pyrust_core::type_err!(msg))
    }
}
