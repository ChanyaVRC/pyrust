impl Compiler {
    fn finish(mut self) -> Result<FnCode, PyError> {
        if self.failed {
            let msg = self
                .error_msg
                .unwrap_or_else(|| "compilation failed".to_string());
            if self.is_syntax_error {
                return Err(PyError::named("SyntaxError", msg));
            } else {
                return Err(PyError::runtime(msg));
            }
        }
        let num_regs = if self.max_reg >= self.base_temp || self.base_temp == 0 {
            self.max_reg.saturating_add(1)
        } else {
            self.base_temp
        };
        // Guard against pathological register counts that would OOM at runtime.
        // Each register slot is one Option<Value>, so 1M slots ~= 8 MB per call frame.
        if num_regs > MAX_FRAME_REGS {
            return Err(PyError::runtime(format!(
                "function uses too many registers ({num_regs}); max is {MAX_FRAME_REGS}"
            )));
        }
        // Sort the free-variable tail of `cell_slots` (`own_cell_count..`)
        // lexicographically by name so `co_freevars` and `__closure__` match
        // CPython's sorted order without any runtime sorting, remapping every
        // emitted cell index and child capture map accordingly.
        let own_n = self.own_cell_count as usize;
        if self.cell_slots.len() > own_n + 1 {
            let free_count = self.cell_slots.len() - own_n;
            let mut order: SmallVec<[usize; 4]> = (0..free_count).collect();
            order.sort_unstable_by_key(|&i| self.cell_slots[own_n + i].name.as_str());
            let already_sorted = order.iter().enumerate().all(|(pos, &orig)| pos == orig);
            if !already_sorted {
                let mut remap: SmallVec<[u16; 8]> = (0..self.cell_slots.len() as u16).collect();
                let mut new_free: SmallVec<[CellSlotMeta; 4]> = SmallVec::with_capacity(free_count);
                for (new_offset, &old_offset) in order.iter().enumerate() {
                    let old_idx = own_n + old_offset;
                    let new_idx = (own_n + new_offset) as u16;
                    remap[old_idx] = new_idx;
                    new_free.push(self.cell_slots[old_idx]);
                }
                self.cell_slots.truncate(own_n);
                self.cell_slots.extend(new_free);
                for insn in &mut self.insns {
                    match insn {
                        Insn::LoadCell(_, idx)
                        | Insn::StoreCell(idx, _)
                        | Insn::DeleteCell(idx, _)
                        | Insn::LoadClassDeref(_, idx) => {
                            *idx = remap[*idx as usize];
                        }
                        _ => {}
                    }
                }
                for proto in &mut self.fn_protos {
                    for src in &mut proto.closure_capture_map {
                        match src {
                            CaptureSource::EnclosingCell(idx) => {
                                *idx = remap[*idx as usize];
                            }
                        }
                    }
                }
            }
        }
        // A function is a generator if it contains any `Yield` or `YieldFrom`
        // instruction OR if any `yield`/`yield from` appears in a dead branch
        // (compile-time-false `if` arm) that was skipped during emission.
        // CPython determines generator status from the AST — the presence of
        // `yield` anywhere in the source makes the function a generator even
        // if that `yield` is unreachable at runtime (issue #1758).
        //
        // In an `async def` body (#2280) the `is_generator` flag distinguishes
        // an *async generator* (`async def` containing `yield`) from a plain
        // coroutine.  `await` lowers to a `GetAwaitable` + `Insn::YieldFrom`
        // pair, so YieldFrom must NOT count here — otherwise every coroutine
        // that awaits anything would be mis-tagged as an async generator.  A
        // bare `yield` (the only thing that makes an `async def` an async
        // generator; `yield from` inside `async def` is a SyntaxError) emits
        // `Insn::Yield`, so for async functions we scan for `Insn::Yield` only.
        let is_generator = if self.is_async_function {
            self.insns.iter().any(|i| matches!(i, Insn::Yield { .. })) || self.has_dead_yield
        } else {
            self.insns
                .iter()
                .any(|i| matches!(i, Insn::Yield { .. } | Insn::YieldFrom { .. }))
                || self.has_dead_yield
        };
        let insns = self.insns;
        let insns_len = insns.len();
        let names_len = self.names.len();
        let global_cache_interest_masks = self
            .names
            .iter()
            .map(|name| crate::bytecode::global_cache_interest_mask(name))
            .collect();
        Ok(FnCode {
            insns,
            filename: self.filename,
            qualname: self.qualname,
            lineno_table: self.lineno_table,
            col_table: self.col_table,
            first_lineno: self.first_lineno,
            consts: self.consts,
            names: self.names,
            num_regs,
            num_iters: self.max_iter,
            num_locals: self.base_temp,
            fn_protos: self.fn_protos,
            cell_slots: self.cell_slots,
            own_cell_count: self.own_cell_count,
            free_var_candidates: std::cell::OnceCell::new(),
            is_generator,
            is_coroutine: self.is_async_function,
            is_class_method: self.is_class_method,
            is_function_scope: self.is_function_scope,
            is_inlined_comp: self.is_inlined_comp,
            comp_enclosing_locals: self.comp_enclosing_locals.clone(),
            attr_cache: std::cell::RefCell::new(vec![AttrCacheEntry::Empty; insns_len]),
            global_cache: RefCell::new(vec![GlobalCacheEntry::Empty; names_len]),
            global_cache_interest_masks,
            binop_cache: RefCell::new(vec![BinOpCacheEntry::Empty; insns_len]),
            kwcall_cache: RefCell::new(vec![KwCallCacheEntry::Empty; insns_len]),
            fmt_spec_cache: RefCell::new(vec![
                crate::interpreter::FmtSpecCacheEntry::Empty;
                insns_len
            ]),
            call_builtin_cache: RefCell::new(vec![
                crate::interpreter::CallBuiltinCacheEntry::Empty;
                insns_len
            ]),
            cached_code_object: RefCell::new(None),
            // Empty until the optimizer's `build_exc_table` pass runs; while
            // empty the VM uses the dynamic SetupExcept/PopExcept handler stack.
            exc_table: Vec::new(),
            // Conservative: un-optimized bytecode is never trampolined.  The
            // optimizer recomputes this from the real `exc_table` (#2234).
            has_exc_handlers: true,
        })
    }
}
