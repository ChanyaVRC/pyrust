impl Compiler {
    fn new(
        local_index: Rc<HashMap<String, Reg>>,
        def_bound_mask: u64,
        cell_vars: Vec<CellVar>,
    ) -> Self {
        let n = local_index.len();
        let cell_set: HashSet<String> = cell_vars.into_iter().collect();
        // base_temp must cover ALL local_index slots (including cell vars) so
        // that temp registers never overlap with local-variable slot numbers.
        let base_temp = Reg::try_from(n).unwrap_or(Reg::MAX);

        Self {
            local_index,
            cell_vars: cell_set,
            nonlocal_names: HashSet::new(),
            class_direct_env_names: HashSet::new(),
            insns: Vec::new(),
            lineno_table: Vec::new(),
            col_table: Vec::new(),
            // (0, 0, 0, 0) sentinel = no anchor (#2411).
            current_lineno: 0,
            current_col_span: (0, 0, 0, 0),
            first_lineno: 0,
            filename: std::sync::Arc::from("<unknown>"),
            consts: Vec::new(),
            const_index: HashMap::new(),
            names: Vec::new(),
            name_map: HashMap::new(),
            next_temp: base_temp,
            base_temp,
            iter_depth: 0,
            max_iter: 0,
            max_reg: if n > 0 {
                Reg::try_from(n).unwrap_or(Reg::MAX).saturating_sub(1)
            } else {
                0
            },
            loops: Vec::new(),
            except_cleanups: SmallVec::new(),
            failed: n > Reg::MAX as usize,
            error_msg: if n > Reg::MAX as usize {
                Some(format!("too many local variables (max {})", Reg::MAX))
            } else {
                None
            },
            def_set: def_bound_mask,
            fn_protos: Vec::new(),
            pure_locals: HashSet::new(),
            is_class_body: false,
            is_class_method: false,
            qualname_prefix: String::new(),
            outer_locals: SmallVec::new(),
            is_function_scope: false,
            is_async_function: false,
            is_async_generator_fn: false,
            is_syntax_error: false,
            is_module_scope: false,
            module_namespace_may_be_exposed: false,
            past_future_zone: false,
            future_annotations: false,
            has_dead_yield: false,
            is_set_comp: false,
            is_list_comp: false,
            list_comp_presize: false,
            is_inlined_comp: false,
            comp_enclosing_locals: None,
        }
    }

    /// If this compiler is producing a class body and `reg` is one of the
    /// class-body's local slots, emit a `RecordClassStore(reg)` insn so the
    /// VM can update class-namespace ordering and any materialized live dict.
    /// No-op outside class bodies and for temp / cell registers — keeping
    /// regular function compilation untouched.
    fn maybe_record_class_store(&mut self, reg: Reg) {
        if self.is_class_body && reg < self.base_temp {
            self.emit(Insn::RecordClassStore(reg));
        }
    }

    /// Companion to `maybe_record_class_store`: emit a `RecordClassDel`
    /// after a `DeleteLocal` so the slot is removed from the class-namespace
    /// store-order list while preserving the order of remaining entries.
    fn maybe_record_class_del(&mut self, reg: Reg) {
        if self.is_class_body && reg < self.base_temp {
            self.emit(Insn::RecordClassDel(reg));
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn mark_def(&mut self, reg: Reg) {
        if (reg as usize) < 64 {
            self.def_set |= 1u64 << reg;
        }
    }

    fn mark_target_def(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    self.mark_def(reg);
                }
            }
            AssignTarget::Tuple(targets) => {
                for t in targets {
                    self.mark_target_def(t);
                }
            }
            AssignTarget::Starred(inner) => {
                self.mark_target_def(inner);
            }
            _ => {}
        }
    }

    fn intern_name(&mut self, name: &str) -> u16 {
        if let Some(&idx) = self.name_map.get(name) {
            return idx;
        }
        if self.names.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!("too many distinct names (max {})", u16::MAX));
            }
            return 0;
        }
        let idx = self.names.len() as u16;
        self.names.push(name.to_string());
        self.name_map.insert(name.to_string(), idx);
        idx
    }

    fn intern_const(&mut self, val: Value) -> u16 {
        // PyKey treats `Bool(b)` and `Int(b as i64)` as hash/eq-equal (matching
        // CPython's `True == 1`), so they would collide in the constant pool's
        // hash index even though they are type-distinct values.  Likewise,
        // `Float(1.0)` and `Int(1)` are now hash/eq-equal in PyKey so that
        // dict/set keys respect CPython's numeric equality invariant.  Complex
        // values with zero imaginary part map to `PyKey::Float` via `to_key()`,
        // which would collide with integer-valued floats and ints.  In all these
        // cases the constant pool must keep the values distinct, so we skip the
        // hash-map fast path and fall through to the type-exact linear scan.
        let is_bool = matches!(val.kind(), ValueKind::Bool(_));
        let is_float = matches!(val.kind(), ValueKind::Float(_));
        let is_complex = matches!(val.kind(), ValueKind::Complex(_, _));
        if !is_bool
            && !is_float
            && !is_complex
            && let Some(key) = val.to_key()
        {
            if let Some(&idx) = self.const_index.get(&key) {
                return idx;
            }
            if self.consts.len() >= u16::MAX as usize {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(format!("too many constants (max {})", u16::MAX));
                }
                return 0;
            }
            let idx = self.consts.len() as u16;
            self.const_index.insert(key, idx);
            self.consts.push(val);
            return idx;
        }
        // Non-hashable constants, booleans, and floats: type-exact linear scan.
        for (i, v) in self.consts.iter().enumerate() {
            if const_eq(v, &val) {
                return i as u16;
            }
        }
        if self.consts.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!("too many constants (max {})", u16::MAX));
            }
            return 0;
        }
        let idx = self.consts.len() as u16;
        self.consts.push(val);
        idx
    }

    fn alloc_temp(&mut self) -> Reg {
        let r = self.next_temp;
        if r == Reg::MAX {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!("too many temporaries (max {})", Reg::MAX));
            }
            return 0;
        }
        self.next_temp += 1;
        if r > self.max_reg {
            self.max_reg = r;
        }
        r
    }

    fn free_temp(&mut self, r: Reg) {
        if r >= self.base_temp && self.next_temp > 0 && r + 1 == self.next_temp {
            self.next_temp -= 1;
        }
    }

    fn alloc_iter(&mut self) -> u8 {
        let s = self.iter_depth;
        self.iter_depth += 1;
        if self.iter_depth > self.max_iter {
            self.max_iter = self.iter_depth;
        }
        s
    }

    fn free_iter(&mut self) {
        if self.iter_depth > 0 {
            self.iter_depth -= 1;
        }
    }

    fn emit(&mut self, insn: Insn) -> usize {
        let idx = self.insns.len();
        self.insns.push(insn);
        self.lineno_table.push(self.current_lineno);
        // The armed PEP 657 anchor applies to exactly this instruction (#2426);
        // consume and clear it so it never leaks onto the next emit.
        self.col_table.push(self.current_col_span);
        self.current_col_span = (0, 0, 0, 0);
        idx
    }

    /// Arm a PEP 657 caret anchor (issues #2426 / #2411) for the **next**
    /// emitted instruction.  `None` (a span-less form) clears the anchor — the
    /// formatter then omits the caret row.  Consumed and reset by `emit`.
    fn set_col_span_for_next(&mut self, span: Option<crate::ast::CaretSpan>) {
        self.current_col_span = span.unwrap_or((0, 0, 0, 0));
    }

    /// Set the source line number for all subsequently emitted instructions.
    /// Call this at the start of each statement (when line info is available).
    fn set_lineno(&mut self, lineno: u32) {
        self.current_lineno = lineno;
    }

    fn set_syntax_error(&mut self, msg: &str) {
        self.failed = true;
        self.is_syntax_error = true;
        if self.error_msg.is_none() {
            self.error_msg = Some(msg.to_string());
        }
    }

    fn pc(&self) -> usize {
        self.insns.len()
    }

    fn patch_jump(&mut self, idx: usize) {
        let target = self.insns.len() as i32;
        let after_jump = idx as i32 + 1;
        let offset = target - after_jump;
        match &mut self.insns[idx] {
            Insn::Jump(off)
            | Insn::JumpIfFalse(_, off)
            | Insn::JumpIfTrue(_, off)
            | Insn::ForIter(_, _, off)
            | Insn::SetupExcept(off)
            | Insn::MatchExcept(_, off)
            | Insn::MatchExceptStar(_, _, _, off)
            | Insn::CmpJumpIfFalse(_, _, _, off)
            | Insn::CmpJumpIfTrue(_, _, _, off)
            | Insn::CmpJumpIfFalseConst(_, _, _, off)
            | Insn::CmpJumpIfTrueConst(_, _, _, off) => *off = offset,
            _ => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(
                        "internal compiler error: patch_jump on non-jump instruction".to_string(),
                    );
                }
            }
        }
    }

    /// Try to fuse the last emitted instruction with a conditional jump.
    ///
    /// Emit `JumpIfFalse` or `JumpIfTrue` for `cond_reg` (offset=0, patched later).
    /// `invert=false` → JumpIfFalse, `invert=true` → JumpIfTrue.
    /// BinOp/BinOpConst + conditional-jump fusion is handled by the optimizer.
    fn emit_cond_jump(&mut self, cond_reg: Reg, invert: bool) -> usize {
        if invert {
            self.emit(Insn::JumpIfTrue(cond_reg, 0))
        } else {
            self.emit(Insn::JumpIfFalse(cond_reg, 0))
        }
    }

    /// True if `name` is a cell variable (lives in env, not registers).
    fn is_cell(&self, name: &str) -> bool {
        self.cell_vars.contains(name)
    }

    /// True when a non-register name is guaranteed to resolve to a
    /// **function-scope cell** — either a cell var owned by this scope or a
    /// `nonlocal x` declared here, which binds to an enclosing function's cell
    /// (issue #2339).  Such a name never resolves to a module global or builtin,
    /// so its read/write can use the dedicated `LoadCell`/`StoreCell` opcodes
    /// that skip the `LoadGlobal` inline cache and the module-globals-dict
    /// fallback.  Restricted to function scope: module scope has no cells worth
    /// special-casing, and class bodies keep the name-keyed namespace path
    /// (which `vars()`/`dir()` expose) and the #384 resolution rules untouched.
    fn is_function_cell(&self, name: &str) -> bool {
        self.is_function_scope && (self.is_cell(name) || self.nonlocal_names.contains(name))
    }

    /// Register index for a local variable, or None if the name is global/nonlocal/cell.
    fn local_reg(&self, name: &str) -> Option<Reg> {
        if self.is_cell(name) {
            return None;
        }
        self.local_index.get(name).copied()
    }

    fn compile_block(&mut self, stmts: &[Stmt]) {
        self.compile_block_with_linenos(stmts, &[]);
    }

    /// Like `compile_block` but with per-statement line numbers.  When
    /// `linenos` is shorter than `stmts` (or empty), the missing entries
    /// default to 0 (= keep current lineno).
    fn compile_block_with_linenos(&mut self, stmts: &[Stmt], linenos: &[u32]) {
        for (idx, stmt) in stmts.iter().enumerate() {
            if self.failed {
                return;
            }
            // Update the current line number when info is available.
            if let Some(&ln) = linenos.get(idx)
                && ln != 0
            {
                self.set_lineno(ln);
            }
            self.compile_stmt(stmt);
            // Track whether we have moved past the zone where `from __future__`
            // imports are valid (module-level, before any non-__future__ statement
            // other than the module docstring which is peeled off by compile_script).
            if self.is_module_scope
                && !matches!(stmt, Stmt::ImportFrom { module, .. } if module == "__future__")
            {
                self.past_future_zone = true;
            }
        }
    }

    /// Emit the complete PEP 3110 cleanup for one `except E as name` binding.
    ///
    /// `DeleteLocal` clears the fast-local register. At module scope the
    /// binding may also have been mirrored into the live globals dictionary,
    /// so `DeleteModuleGlobal` must remove that second representation before
    /// later global lookup can observe it.
    fn emit_except_as_delete(&mut self, cleanup: Option<ExceptAsVarDel>) {
        match cleanup {
            Some(ExceptAsVarDel::Local {
                register,
                module_name,
            }) => {
                self.emit(Insn::DeleteLocal(register, u16::MAX));
                if let Some(name_index) = module_name {
                    self.emit(Insn::DeleteModuleGlobal(name_index));
                }
                if (register as usize) < 64 {
                    self.def_set &= !(1u64 << register);
                }
            }
            Some(ExceptAsVarDel::Name(name_index)) => {
                self.emit(Insn::DeleteName(name_index));
            }
            None => {}
        }
    }

    /// Emit cleanup instructions for a `raise` statement that exits an `except`
    /// handler body.  Unlike `emit_early_exit_cleanups` (which is for
    /// `break`/`continue`/`return`), `raise` does NOT emit `EndExcept` because
    /// the raise instruction itself manages `handled_exc_stack`:
    ///
    /// - `RaiseReRaise` explicitly pops `handled_exc_stack` before propagating.
    /// - `RaiseValue`/`RaiseFrom` don't pop, but `handle_vm_error` checks for
    ///   a duplicate top-of-stack entry and removes it automatically.
    ///
    /// So for `raise` we only need to: delete any `as VAR` binding (PEP 3110),
    /// then inline any `finally` stmts.  `TryBody` entries don't need compile-time
    /// cleanup — the VM's `exc_handlers` stack still covers them.
    /// `pending_exc_reg`: when a non-bare `raise X` is in progress, the
    /// register holding the to-be-raised exception value.  If `Some`, it is
    /// pushed onto `handled_exc_stack` before inlining the innermost
    /// ExceptBody's finally block, so that any raise inside the finally sees
    /// the correct implicit context (the to-be-raised exception, not the
    /// currently-handled one).  The push is undone by a `PopExcContext` after
    /// the finally block completes normally.
    fn emit_raise_cleanups(&mut self, pending_exc_reg: Option<Reg>) {
        let total = self.except_cleanups.len();
        // Track whether we have already processed the innermost ExceptBody.
        // The PushExcContext is only needed for that first one.
        let mut innermost_except_body_done = false;
        for i in (0..total).rev() {
            if self.failed {
                return;
            }
            let cleanup = self.except_cleanups[i].clone();
            match cleanup {
                EarlyExitCleanup::TryBody { .. } | EarlyExitCleanup::WithBody { .. } => {
                    // A TryBody/WithBody entry means this `raise` site is inside a
                    // try/with body whose SetupExcept is still live on the VM's
                    // exc_handlers stack.  The VM will dispatch the exception to
                    // that handler at runtime (for `with`, the exception-path
                    // `__exit__` call); no compile-time inlining is needed for
                    // this entry or any outer entries (also covered by their own
                    // SetupExcept).
                    return;
                }
                EarlyExitCleanup::ExceptBody {
                    finally_stmts,
                    as_var_delete,
                } => {
                    // PEP 3110: delete the `as VAR` binding (matches the normal
                    // handler exit path at line ~7427).  Also clear def_set so
                    // that any reference to the variable inside the inlined
                    // finally block correctly emits CheckLocal → UnboundLocalError,
                    // matching CPython's behaviour (the `as` binding is gone before
                    // the finally clause runs).
                    self.emit_except_as_delete(as_var_delete);
                    // Inline the finally block (if any) without EndExcept.
                    // The raise instruction propagates the exception; any further
                    // enclosing ExceptBody entries (outer except handlers whose
                    // SetupExcept was also popped) are processed by the remaining
                    // loop iterations.  TryBody entries at outer scopes still have
                    // live SetupExcept handlers and are handled by the VM.
                    if let Some(stmts) = finally_stmts {
                        // For the innermost ExceptBody: if we have a pending
                        // exception (non-bare raise), temporarily install it as
                        // the active context so that any raise inside the finally
                        // sees it as __context__ rather than the currently-handled
                        // exception on the stack.  This matches CPython, where the
                        // finally runs with the new exception as the active one.
                        let push_exc_ctx = !innermost_except_body_done && pending_exc_reg.is_some();
                        if let (true, Some(r)) = (push_exc_ctx, pending_exc_reg) {
                            self.emit(Insn::PushExcContext(r));
                        }
                        let saved_tail: Vec<EarlyExitCleanup> =
                            self.except_cleanups.drain(i..).collect();
                        self.compile_block(&stmts);
                        self.except_cleanups.extend(saved_tail);
                        if push_exc_ctx {
                            self.emit(Insn::PopExcContext);
                        }
                    }
                    innermost_except_body_done = true;
                    // Continue the loop: there may be enclosing ExceptBody entries
                    // (from outer except handlers) whose finallys also need inlining,
                    // because their outer SetupExcept was likewise popped when the
                    // outer handler was entered.
                }
            }
        }
    }

    /// Emit cleanup instructions for all `EarlyExitCleanup` entries in
    /// `self.except_cleanups[from_depth..]`, iterating innermost-first (i.e.
    /// from the top of the stack downward).
    ///
    /// Called before `break`, `continue`, or `return` to unwind any active
    /// `try`/`except` guards that the early exit crosses.
    ///
    /// While the inlined finally/handler-cleanup block for frame `i` is being
    /// compiled, `except_cleanups` is temporarily truncated to `[..i]` so that
    /// an early exit (e.g. `return`) inside that inlined block does not re-walk
    /// the frame we are currently unwinding — which would cause infinite
    /// recursion (see issue #365: `try: return X finally: return Y`).
    fn emit_early_exit_cleanups(&mut self, from_depth: usize) {
        let total = self.except_cleanups.len();
        if total <= from_depth {
            return;
        }
        // Walk from innermost (top) down to `from_depth`.
        for i in (from_depth..total).rev() {
            if self.failed {
                return;
            }
            // Clone the entry so we can mutate `self` while compiling the
            // inlined finally block.
            let cleanup = self.except_cleanups[i].clone();
            // Shadow the cleanup stack: any nested cleanup emission triggered
            // by `compile_block` below must not see frames `[i..]` (we are
            // already in the process of unwinding them).
            let saved_tail: Vec<EarlyExitCleanup> = self.except_cleanups.drain(i..).collect();
            match cleanup {
                EarlyExitCleanup::TryBody { finally_stmts } => {
                    self.emit(Insn::PopExcept);
                    if let Some(stmts) = finally_stmts {
                        self.compile_block(&stmts);
                    }
                }
                EarlyExitCleanup::ExceptBody {
                    finally_stmts,
                    as_var_delete,
                } => {
                    // PEP 3110: delete the `as VAR` binding before EndExcept,
                    // matching the normal (non-early-exit) handler exit path.
                    self.emit_except_as_delete(as_var_delete);
                    self.emit(Insn::EndExcept);
                    if let Some(stmts) = finally_stmts {
                        self.compile_block(&stmts);
                    }
                }
                EarlyExitCleanup::WithBody { ctx_reg, is_async } => {
                    // `with`/`async with` is `try: BODY finally: __exit__(...)`.
                    // Pop the body's handler, then run the no-exception exit
                    // (`__exit__(None, None, None)` / `await __aexit__(...)`)
                    // before the break/continue/return jump (issue #2295).
                    self.emit(Insn::PopExcept);
                    if is_async {
                        self.emit_async_with_normal_exit(ctx_reg);
                    } else {
                        self.emit_with_normal_exit(ctx_reg);
                    }
                }
            }
            // Restore the cleanup stack so the caller (and any sibling
            // iterations) see the original frames.  Unconditional restore
            // is safe because `compile_block` doesn't return errors — it
            // routes failures through `self.failed`, which the early-return
            // guard above catches on the next loop iteration.
            self.except_cleanups.extend(saved_tail);
        }
    }

    fn finish(self) -> Result<FnCode, PyError> {
        if self.failed {
            let msg = self
                .error_msg
                .unwrap_or_else(|| "compilation failed".to_string());
            if self.is_syntax_error {
                return Err(PyError::named("SyntaxError", msg));
            } else {
                return Err(PyError::Runtime(msg));
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
            return Err(PyError::Runtime(format!(
                "function uses too many registers ({num_regs}); max is {MAX_FRAME_REGS}"
            )));
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
            lineno_table: self.lineno_table,
            col_table: self.col_table,
            first_lineno: self.first_lineno,
            consts: self.consts,
            names: self.names,
            num_regs,
            num_iters: self.max_iter,
            num_locals: self.base_temp,
            fn_protos: self.fn_protos,
            cell_vars: self.cell_vars.into_iter().collect(),
            free_var_candidates: std::cell::OnceCell::new(),
            is_generator,
            is_coroutine: self.is_async_function,
            is_class_method: self.is_class_method,
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
            // Empty until the optimizer's `build_exc_table` pass runs; while
            // empty the VM uses the dynamic SetupExcept/PopExcept handler stack.
            exc_table: Vec::new(),
            // Conservative: un-optimized bytecode is never trampolined.  The
            // optimizer recomputes this from the real `exc_table` (#2234).
            has_exc_handlers: true,
        })
    }

    /// Allocate result register: reuse `candidate` if it's a temp, else fresh.
    fn ensure_dst(&mut self, candidate: Reg) -> Reg {
        if candidate >= self.base_temp {
            candidate
        } else {
            self.alloc_temp()
        }
    }

    /// If `src` is a fastlocal register (not a temp), copy its value into a
    /// fresh temp register and return that temp.  Otherwise return `src` as-is.
    /// Used when a value must survive a `DeleteLocal` on the same register.
    fn ensure_temp(&mut self, src: Reg) -> Reg {
        if src >= self.base_temp {
            src
        } else {
            let dst = self.alloc_temp();
            self.emit(Insn::Move(dst, src));
            dst
        }
    }

    // ── Store helpers ─────────────────────────────────────────────────────────

    /// Emit the appropriate store for `name` from register `src`.
    /// If `container_expr` is a global/cell variable name, write `obj_reg` back
    /// to the env.  Called after SetItem/SetSlice on a container that was loaded
    /// via LoadGlobal (which creates a copy, so the mutation must be committed).
    fn writeback_container_if_global(&mut self, container_expr: &Expr, obj_reg: Reg) {
        if let Expr::Var(name, _) = container_expr
            && self.local_reg(name).is_none()
        {
            let name_idx = self.intern_name(name);
            self.emit(Insn::StoreGlobal(name_idx, obj_reg));
        }
    }

    fn compile_store_name(&mut self, name: &str, src: Reg) {
        if let Some(reg) = self.local_reg(name) {
            if src != reg {
                self.emit(Insn::Move(reg, src));
            }
            // Record the runtime store for class-namespace ordering. We emit
            // even when `src == reg` (no Move emitted) because the store
            // still semantically happened — e.g. `for i in range(3):` writes
            // `i` directly via `ForIter(reg, ...)` and then `compile_for`
            // calls back through here for synthetic stores.
            self.maybe_record_class_store(reg);
            // At module scope, publish the register write to the root namespace
            // so aliases, overlapping EnvValues, and global-cache generations
            // stay coherent. The runtime keeps the no-alias path allocation-free.
            if self.is_module_scope {
                let name_idx = self.intern_name(name);
                self.emit(Insn::SyncModuleGlobal(reg, name_idx));
            }
        } else {
            let idx = self.intern_name(name);
            if self.is_function_cell(name) {
                self.emit(Insn::StoreCell(idx, src));
            } else {
                self.emit(Insn::StoreGlobal(idx, src));
            }
        }
    }

    // ── Statement compilation ─────────────────────────────────────────────────
}
