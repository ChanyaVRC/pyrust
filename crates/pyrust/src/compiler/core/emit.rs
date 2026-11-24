impl Compiler {
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

    /// If `container_expr` is a global variable name, write `obj_reg` back
    /// to the env.  Called after SetItem/SetSlice on a container that was loaded
    /// via LoadGlobal (which creates a copy, so the mutation must be committed).
    /// Cells hold `Rc`-backed containers directly, so cell-slot containers do
    /// not need a writeback (and must never spill into module globals).
    fn writeback_container_if_global(&mut self, container_expr: &Expr, obj_reg: Reg) {
        if let Expr::Var(name, _) = container_expr
            && self.local_reg(name).is_none()
        {
            if self.resolve_cell_slot_for_read(name).is_some()
                || self.resolve_class_body_deref(name).is_some()
            {
                return;
            }
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
        } else if let Some(cell_idx) = self.resolve_cell_slot_for_write(name) {
            self.emit(Insn::StoreCell(cell_idx, src));
        } else {
            let idx = self.intern_name(name);
            self.emit(Insn::StoreGlobal(idx, src));
        }
    }
}
