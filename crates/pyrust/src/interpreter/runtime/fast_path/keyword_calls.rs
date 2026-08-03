impl Interpreter {
    #[inline(always)]
    /// Full body of `Insn::CallKw` (issue #2382).  The args occupy
    /// `R[func+1 .. func+1+total]`; the trailing `nkw` of them are keyword args
    /// whose names are the const-pool tuple `consts[kwnames_idx]`.  Returns the
    /// call result (the dispatch arm writes it back to `R[func]`).
    ///
    /// A per-call-site cache ([`KwCallCacheEntry`]) records the keyword→param
    /// slot mapping for a monomorphic, plain-`UserFunction` callee.  On a hit
    /// (same `param_binds` identity) the keyword values bind straight into their
    /// slots with no name scan — `call_user_function_kw_cached`.  On a miss the
    /// general binder (`call_function_expanded`) runs, which owns every
    /// CPython-parity diagnostic; if that call's shape is *simple* the cache is
    /// filled, otherwise it is set to `Fallback` (permanent slow path).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_call_kw(
        &mut self,
        regs: &RegSlice,
        code: &crate::bytecode::FnCode,
        pc: usize,
        func: crate::bytecode::Reg,
        total: u8,
        nkw: u8,
        kwnames_idx: u16,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        use crate::bytecode::KwCallCacheEntry;
        let total = total as u32;
        let nkw = nkw as usize;
        let npos = total as usize - nkw;
        let func_val = vm_read(regs, func, num_locals)?;

        // The fast bind only applies to a plain user function (Regular kind).
        // Bound methods, classes, builtins, class/static methods, etc. take the
        // general `call_function_expanded` path below, which handles them all.
        let user_fn = match func_val.kind() {
            ValueKind::UserFunction(f)
                if matches!(f.kind, pyrust_core::UserFunctionKind::Regular) =>
            {
                Some(Rc::clone(f))
            }
            _ => None,
        };

        // #2395: bypass kw-call cache when __defaults__ has been overridden; the
        // cache was built against compile-time defaults and would serve stale values.
        if let Some(f) = user_fn.filter(|f| f.defaults_override.borrow().is_none()) {
            let pbptr = Rc::as_ptr(&f.param_binds);
            // Cache hit?
            let cached: Option<(u8, smallvec::SmallVec<[u32; 4]>)> = {
                let cache = code.kwcall_cache.borrow();
                match &cache[pc - 1] {
                    KwCallCacheEntry::Simple {
                        param_binds_ptr,
                        npos: cnpos,
                        slots,
                    } if param_binds_ptr.as_ptr() == pbptr && *cnpos as usize == npos => {
                        Some((*cnpos, slots.clone()))
                    }
                    _ => None,
                }
            };
            if let Some((_, slots)) = cached {
                let Some(callee_code) = self.get_or_compile_bytecode(&f) else {
                    return Err(PyError::Runtime(format!("no bytecode for '{}'", f.name)));
                };
                // Read the positional and keyword values from their registers.
                let mut pos_vals: smallvec::SmallVec<[Value; 4]> =
                    smallvec::SmallVec::with_capacity(npos);
                for i in 0..npos as u32 {
                    pos_vals.push(vm_read(regs, func + 1 + i, num_locals)?);
                }
                let mut kw_vals: smallvec::SmallVec<[Value; 4]> =
                    smallvec::SmallVec::with_capacity(nkw);
                for i in 0..nkw as u32 {
                    kw_vals.push(vm_read(regs, func + 1 + npos as u32 + i, num_locals)?);
                }
                return self.call_user_function_kw_cached(
                    &f,
                    &callee_code,
                    npos,
                    &mut pos_vals.into_iter(),
                    &slots,
                    &mut kw_vals.into_iter(),
                );
            }

            // Cache empty (or shape changed): try to resolve this call as simple.
            // Only fill on an `Empty` slot — a `Fallback` slot stays permanent.
            let is_empty = matches!(code.kwcall_cache.borrow()[pc - 1], KwCallCacheEntry::Empty);
            if is_empty {
                let kwnames = match code
                    .consts
                    .get(kwnames_idx as usize)
                    .and_then(|c| c.as_tuple())
                {
                    Some(t) => t.to_vec(),
                    None => {
                        return Err(PyError::Runtime(
                            "bytecode error: CallKw kwnames is not a tuple".to_string(),
                        ));
                    }
                };
                match Self::kwcall_resolve_simple(&f, npos, &kwnames) {
                    Some(slots) => {
                        let mut cache = code.kwcall_cache.borrow_mut();
                        cache[pc - 1] = KwCallCacheEntry::Simple {
                            param_binds_ptr: Rc::downgrade(&f.param_binds),
                            npos: npos as u8,
                            slots: slots.clone(),
                        };
                        drop(cache);
                        let Some(callee_code) = self.get_or_compile_bytecode(&f) else {
                            return Err(PyError::Runtime(format!("no bytecode for '{}'", f.name)));
                        };
                        let mut pos_vals: smallvec::SmallVec<[Value; 4]> =
                            smallvec::SmallVec::with_capacity(npos);
                        for i in 0..npos as u32 {
                            pos_vals.push(vm_read(regs, func + 1 + i, num_locals)?);
                        }
                        let mut kw_vals: smallvec::SmallVec<[Value; 4]> =
                            smallvec::SmallVec::with_capacity(nkw);
                        for i in 0..nkw as u32 {
                            kw_vals.push(vm_read(regs, func + 1 + npos as u32 + i, num_locals)?);
                        }
                        return self.call_user_function_kw_cached(
                            &f,
                            &callee_code,
                            npos,
                            &mut pos_vals.into_iter(),
                            &slots,
                            &mut kw_vals.into_iter(),
                        );
                    }
                    None => {
                        // Not simple — permanent fallback to the general binder.
                        code.kwcall_cache.borrow_mut()[pc - 1] = KwCallCacheEntry::Fallback;
                    }
                }
            }
        }

        // Slow path: build an `ExpandedCallArg` buffer (positionals, then named
        // keyword args) and dispatch through the general call machinery, which
        // owns CPython-parity argument binding + diagnostics for every callee.
        let kwnames = match code
            .consts
            .get(kwnames_idx as usize)
            .and_then(|c| c.as_tuple())
        {
            Some(t) => t.to_vec(),
            None => {
                return Err(PyError::Runtime(
                    "bytecode error: CallKw kwnames is not a tuple".to_string(),
                ));
            }
        };
        let mut buf = std::mem::take(&mut self.call_arg_buf);
        buf.clear();
        for i in 0..npos as u32 {
            buf.push(ExpandedCallArg {
                name: None,
                value: vm_read(regs, func + 1 + i, num_locals)?,
            });
        }
        for (i, name_val) in kwnames.iter().enumerate() {
            let name = name_val
                .as_str()
                .ok_or_else(|| {
                    PyError::Runtime("bytecode error: CallKw kwname is not a str".to_string())
                })?
                .to_string();
            let value = vm_read(regs, func + 1 + npos as u32 + i as u32, num_locals)?;
            buf.push(ExpandedCallArg {
                name: Some(name),
                value,
            });
        }
        let call_result = self.call_function_expanded(func_val, &buf);
        self.call_arg_buf = buf;
        call_result
    }

    /// Full body of `Insn::CallMethodKw` (issue #2392) — a keyword method call
    /// `R[obj].name(<pos…>, k=v…)` with no splats.  The receiver is in `R[obj]`;
    /// the `total` argument values live in `R[args_base .. args_base+total]`, the
    /// trailing `nkw` of them being keyword args named by `consts[kwnames_idx]`.
    /// Returns the call result (the dispatch arm writes it back to `R[dst]`).
    ///
    /// Fast path: a monomorphic inline-cache hit for a plain `Regular`
    /// `UserFunction` method (the same resolution `Insn::CallMethod` uses) plus a
    /// per-call-site keyword-shape cache ([`KwCallCacheEntry::Simple`]).  The
    /// receiver binds to parameter 0 (so the effective positional count is
    /// `1 + (total - nkw)`), and the keyword values bind straight into their
    /// cached slots via the #2382 `call_user_function_kw_cached` — no dict/list
    /// build, no name scan.  The cache key is `param_binds` identity (shared by
    /// every closure from one `def`), exactly as for plain `Insn::CallKw`.
    ///
    /// Slow path: a cache miss (first call at the site, polymorphic receiver
    /// class, shadowed attribute, builtin/backing method, or a structurally
    /// non-simple binding shape) materialises the positional + keyword arguments
    /// and dispatches through `dispatch_method_with_args`, which shares the
    /// general method-call machinery (and its CPython-parity diagnostics) with
    /// `Insn::CallMethodExpanded`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_call_method_kw(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        _dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        args_base: crate::bytecode::Reg,
        total: u8,
        nkw: u8,
        kwnames_idx: u16,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
        _cur_line: u32,
    ) -> Result<Value> {
        use crate::bytecode::KwCallCacheEntry;
        let total = total as u32;
        let nkw = nkw as usize;
        let npos_args = total as usize - nkw;
        // Effective positionals seen by the callee: receiver fills param 0.
        let eff_npos = npos_args + 1;

        // Try the inline method cache (same resolution as `Insn::CallMethod`).  A
        // hit yields a plain `Regular` user method and the resolved receiver.
        if let Some(method) = code.names.get(name_idx as usize)
            && let Some((f, recv_rc)) =
                self.resolve_method_cached(regs, obj, method, code, call_site_pc)
        {
            let pbptr = Rc::as_ptr(&f.param_binds);
            // Keyword-shape cache hit?
            let cached: Option<smallvec::SmallVec<[u32; 4]>> = {
                let cache = code.kwcall_cache.borrow();
                match &cache[call_site_pc] {
                    KwCallCacheEntry::Simple {
                        param_binds_ptr,
                        npos: cnpos,
                        slots,
                    } if param_binds_ptr.as_ptr() == pbptr && *cnpos as usize == eff_npos => {
                        Some(slots.clone())
                    }
                    _ => None,
                }
            };

            // #2395: bypass kw-call cache when __defaults__ has been overridden.
            let resolved: Option<smallvec::SmallVec<[u32; 4]>> =
                if f.defaults_override.borrow().is_some() {
                    None
                } else if let Some(slots) = cached {
                    Some(slots)
                } else if matches!(
                    code.kwcall_cache.borrow()[call_site_pc],
                    KwCallCacheEntry::Empty
                ) {
                    // Fill the shape cache once.  The keyword names map to parameters
                    // *after* the receiver-occupied param 0, so `eff_npos` positionals
                    // (receiver + plain positionals) fill the leading params.
                    let kwnames = match code
                        .consts
                        .get(kwnames_idx as usize)
                        .and_then(|c| c.as_tuple())
                    {
                        Some(t) => t.to_vec(),
                        None => {
                            return Err(PyError::Runtime(
                                "bytecode error: CallMethodKw kwnames is not a tuple".to_string(),
                            ));
                        }
                    };
                    match Self::kwcall_resolve_simple(&f, eff_npos, &kwnames) {
                        Some(slots) => {
                            code.kwcall_cache.borrow_mut()[call_site_pc] =
                                KwCallCacheEntry::Simple {
                                    param_binds_ptr: Rc::downgrade(&f.param_binds),
                                    npos: eff_npos as u8,
                                    slots: slots.clone(),
                                };
                            Some(slots)
                        }
                        None => {
                            // Not simple — permanent fallback to the general binder.
                            code.kwcall_cache.borrow_mut()[call_site_pc] =
                                KwCallCacheEntry::Fallback;
                            None
                        }
                    }
                } else {
                    // Fallback-pinned (or polymorphic on a non-receiver axis): general path.
                    None
                };

            if let Some(slots) = resolved {
                let Some(callee_code) = self.get_or_compile_bytecode(&f) else {
                    return Err(PyError::Runtime(format!("no bytecode for '{}'", f.name)));
                };
                // Positionals seen by the callee: receiver first, then the plain
                // positional args from the register window.
                let mut pos_vals: smallvec::SmallVec<[Value; 4]> =
                    smallvec::SmallVec::with_capacity(eff_npos);
                pos_vals.push(Value::py_instance(recv_rc));
                for i in 0..npos_args as u32 {
                    pos_vals.push(vm_read(regs, args_base + i, num_locals)?);
                }
                let mut kw_vals: smallvec::SmallVec<[Value; 4]> =
                    smallvec::SmallVec::with_capacity(nkw);
                for i in 0..nkw as u32 {
                    kw_vals.push(vm_read(regs, args_base + npos_args as u32 + i, num_locals)?);
                }
                return self.call_user_function_kw_cached(
                    &f,
                    &callee_code,
                    eff_npos,
                    &mut pos_vals.into_iter(),
                    &slots,
                    &mut kw_vals.into_iter(),
                );
            }
        }

        // Slow path: materialise the positionals and keyword map, then dispatch
        // through the shared method-call tail (which fills the inline method
        // cache on the way and owns every CPython-parity diagnostic).
        // Own the method name: `dispatch_method_with_args` borrows `&mut self`,
        // which would conflict with a `&str` still borrowing `code.names`.
        let method: String = code
            .names
            .get(name_idx as usize)
            .ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {name_idx} out of range"
                ))
            })?
            .as_str()
            .to_string();
        let kwnames = match code
            .consts
            .get(kwnames_idx as usize)
            .and_then(|c| c.as_tuple())
        {
            Some(t) => t.to_vec(),
            None => {
                return Err(PyError::Runtime(
                    "bytecode error: CallMethodKw kwnames is not a tuple".to_string(),
                ));
            }
        };
        let mut pos_items: Vec<Value> = Vec::with_capacity(npos_args);
        for i in 0..npos_args as u32 {
            pos_items.push(vm_read(regs, args_base + i, num_locals)?);
        }
        let mut kw_map = PyDict::default();
        for (i, name_val) in kwnames.iter().enumerate() {
            if name_val.as_str().is_none() {
                return Err(PyError::Runtime(
                    "bytecode error: CallMethodKw kwname is not a str".to_string(),
                ));
            }
            let value = vm_read(regs, args_base + npos_args as u32 + i as u32, num_locals)?;
            kw_map.insert(PyKey::Str(name_val.clone()), value);
        }
        // Warm the inline method cache so the *next* call at this site can take
        // the fast bind via `resolve_method_cached` (mirrors `exec_call_method`'s
        // `update_call_method_cache`, which `dispatch_method_with_args` — shared
        // with `CallMethodExpanded` — does not do itself).
        let obj_val = vm_read(regs, obj, num_locals)?;
        self.update_call_method_cache(&obj_val, &method, code, call_site_pc);
        self.dispatch_method_with_args(
            regs,
            num_locals,
            obj,
            &method,
            pos_items,
            kw_map,
            ResolvedMethodCallShape::Fused,
        )
    }
}
