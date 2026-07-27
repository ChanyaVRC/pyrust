impl Interpreter {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_call_method(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        _dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        args_base: crate::bytecode::Reg,
        nargs: u8,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
        cur_line: u32,
    ) -> Result<Value> {
        let method: &str = code
            .names
            .get(name_idx as usize)
            .ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {name_idx} out of range"
                ))
            })?
            .as_str();
        let mut args: Vec<Value> = Vec::with_capacity(nargs as usize);
        for i in 0..crate::bytecode::Reg::from(nargs) {
            args.push(vm_read(regs, args_base + i, num_locals)?);
        }
        let obj_kind = BuiltinContainerKind::classify(regs[obj as usize].as_some());

        // No upfront unalias needed (#448): each builtin scopes its
        // own `RefCell::borrow_mut()` and snapshots iterable args
        // before opening the borrow.  Aliased self-references like
        // `lst.extend(lst)` are now safe by construction.

        // Tagged builtin containers share their dispatch body with the
        // expanded opcode (#431).  The no-kwargs path passes an empty kwargs
        // map; `IndexMap::new()` does not allocate until the first insert, so
        // the hot path stays cheap.
        if let Some(kind) = obj_kind.filter(|kind| kind.supports_direct_method(method)) {
            let receiver = regs[obj as usize].clone();
            let empty_kw = PyDict::default();
            return self
                .dispatch_builtin_container_method(kind, receiver, method, args, &empty_kw, false);
        }

        {
            // Generator methods (close, throw, __next__, __iter__) are
            // dispatched directly here — they need access to the VM/frame
            // and are not regular attributes on the Generator value.
            let is_generator = matches!(
                regs[obj as usize].as_some().map(|v| v.kind()),
                Some(ValueKind::Generator(_))
            );
            if is_generator {
                let obj_val = vm_read(regs, obj, num_locals)?;
                return self.call_generator_method(obj_val, method, args);
            }

            // Inline cache fast path for user-defined class methods on PyInstance
            // objects (Regular UserFunctions / BuiltinFunctions only).
            if let Some(result) =
                self.try_call_method_cached(regs, obj, method, &args, code, call_site_pc)?
            {
                return Ok(result);
            }

            let obj_val = vm_read(regs, obj, num_locals)?;
            let method_val = self.get_attr(&obj_val, method)?;
            Self::publish_frame_line_for_builtin(&method_val, cur_line);
            let mut buf = std::mem::take(&mut self.call_arg_buf);
            buf.clear();
            for arg in args {
                buf.push(ExpandedCallArg {
                    name: None,
                    value: arg,
                });
            }
            let r = self.call_function_expanded(method_val, &buf);
            self.call_arg_buf = buf;

            // Fill or update the inline cache for this user-object call site.
            self.update_call_method_cache(&obj_val, method, code, call_site_pc);

            r
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_call_method_expanded(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        _dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        pos_list: crate::bytecode::Reg,
        kw_dict: crate::bytecode::Reg,
        code: &crate::bytecode::FnCode,
    ) -> Result<Value> {
        let method: &str = code
            .names
            .get(name_idx as usize)
            .ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {name_idx} out of range"
                ))
            })?
            .as_str();
        let v = vm_read(regs, pos_list, num_locals)?;
        let pos_items: Vec<Value> = match v.kind() {
            ValueKind::List(items) => items.to_vec(),
            _ => {
                return Err(PyError::Runtime(
                    "CallMethodExpanded: pos_list must be a list".to_string(),
                ));
            }
        };
        let v = vm_read(regs, kw_dict, num_locals)?;
        let kw_map = match v.kind() {
            ValueKind::Dict(d) => d.clone(),
            _ => {
                return Err(PyError::Runtime(
                    "CallMethodExpanded: kw_dict must be a dict".to_string(),
                ));
            }
        };
        // CPython: a non-string `**` key is a TypeError (`keywords must be
        // strings`), not a silently dropped keyword argument.
        if kw_map.keys().any(|k| !matches!(k, PyKey::Str(_))) {
            return Err(pyrust_core::type_err!("keywords must be strings"));
        }

        self.dispatch_method_with_args(regs, num_locals, obj, method, pos_items, kw_map)
    }

    /// Shared dispatch tail for a method call whose positional args and keyword
    /// args have already been materialised (`Insn::CallMethodExpanded`'s `*pos`/
    /// `**kw` build, and `Insn::CallMethodKw`'s slow-path fallback, #2392).
    /// `pos_items` are the positional argument values in order; `kw_map` is the
    /// keyword arguments (its keys are guaranteed `str` by the caller).  Routes
    /// through the tagged-container fast dispatch, generator-method dispatch, and
    /// finally the generic `get_attr` + `call_function_expanded` path.
    pub(super) fn dispatch_method_with_args(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        method: &str,
        pos_items: Vec<Value>,
        kw_map: PyDict,
    ) -> Result<Value> {
        let obj_kind = BuiltinContainerKind::classify(regs[obj as usize].as_some());

        // No upfront unalias needed (#448): each builtin scopes its
        // own `borrow_mut()` and snapshots iterables before opening
        // the borrow.

        // Tagged builtin containers share their dispatch body with the
        // no-kwargs opcode (#431).
        if let Some(kind) = obj_kind.filter(|kind| kind.supports_direct_method(method)) {
            let receiver = regs[obj as usize].clone();
            return self.dispatch_builtin_container_method(
                kind, receiver, method, pos_items, &kw_map, false,
            );
        }

        {
            // Generator methods — see `exec_call_method` for context.
            let is_generator = matches!(
                regs[obj as usize].as_some().map(|v| v.kind()),
                Some(ValueKind::Generator(_))
            );
            if is_generator {
                if !kw_map.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "generator.{method}() takes no keyword arguments"
                    ));
                }
                let obj_val = vm_read(regs, obj, num_locals)?;
                return self.call_generator_method(obj_val, method, pos_items);
            }
            let obj_val = vm_read(regs, obj, num_locals)?;
            let method_val = self.get_attr(&obj_val, method)?;
            // Build directly into the reusable call buffer, bypassing the
            // intermediate ExpandedArgBuf allocation.
            let mut buf = std::mem::take(&mut self.call_arg_buf);
            buf.clear();
            buf.extend(pos_items.into_iter().map(|v| ExpandedCallArg {
                name: None,
                value: v,
            }));
            for (k, v) in &kw_map {
                if let PyKey::Str(name) = k {
                    buf.push(ExpandedCallArg {
                        name: Some(name.as_str().unwrap_or("").to_owned()),
                        value: v.clone(),
                    });
                }
            }
            let r = self.call_function_expanded(method_val, &buf);
            self.call_arg_buf = buf;
            r
        }
    }
}
