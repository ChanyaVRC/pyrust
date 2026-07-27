impl Compiler {
    fn compile_while(
        &mut self,
        cond: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
    ) {
        if is_const_false_expr(cond) {
            // The while body is statically unreachable, but CPython still
            // validates context-sensitive syntax inside it.  The body counts
            // as a loop context (break/continue inside it are valid), but
            // return/yield are still gated by is_function_scope.
            self.check_dead_block(body, true);
            if self.failed {
                return;
            }
            // A `yield`/`yield from` in a dead `while False` body still makes
            // the enclosing function a generator (CPython parity, issue #1758).
            if self.is_function_scope && stmts_contain_yield(body) {
                self.has_dead_yield = true;
            }
            if let Some(else_stmts) = else_branch {
                self.compile_block_with_linenos(else_stmts, else_linenos);
            }
            return;
        }

        let is_infinite = matches!(cond, Expr::Bool(true) | Expr::Int(1));

        // Keep source-shaped counter loops on the ordinary while path. A
        // syntactic `while i < stop: ...; i += 1` does not prove that `i` and
        // `stop` are exact integers, that evaluating `stop` is pure/invariant,
        // or that the body cannot replace/delete `i`. The source comparison
        // and increment must remain observable Python operations.

        // Re-evaluate every non-constant condition at the loop header. A
        // syntactically unchanged expression can still dispatch __bool__,
        // comparison, or arithmetic protocols on each iteration, and those
        // calls may mutate shared namespaces or expose their call count.
        let (loop_start, exit_jmp) = if is_infinite {
            (self.pc(), None)
        } else {
            let start = self.pc();
            let cond_reg = self.compile_expr(cond);
            let jmp = self.emit_cond_jump(cond_reg, false);
            self.free_temp(cond_reg);
            (start, Some(jmp))
        };

        self.loops.push(LoopCtx {
            break_patches: SmallVec::new(),
            continue_target: Some(loop_start),
            continue_patches: SmallVec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved = self.def_set;
        self.compile_block_with_linenos(body, body_linenos);
        self.def_set = saved;
        if self.failed {
            return;
        }
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));
        if let Some(jmp) = exit_jmp {
            self.patch_jump(jmp);
        }
        let ctx = self.loops.pop().unwrap();
        // For an infinite while (e.g. `while True:`) the else clause is unreachable:
        // the loop never exits naturally, and `break` deliberately skips the else.
        // Skip the emit entirely — semantics-preserving, avoids dead bytecode.
        if !is_infinite && let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
    }

    fn compile_for(
        &mut self,
        target: &AssignTarget,
        iter_expr: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
        body_linenos: &[u32],
        else_linenos: &[u32],
    ) {
        // Always evaluate and call the iterator expression.  In particular, a
        // syntactic `range(...)` is not proof that the callee is the canonical
        // builtin: module globals, function globals, and the builtins mapping
        // can all shadow it at runtime.  Once a real range object reaches
        // GetIter, the runtime still selects its dedicated allocation-light
        // range IterState fast path.
        let iter_slot = self.alloc_iter();
        let src = self.compile_expr(iter_expr);
        self.emit(Insn::GetIter(iter_slot, src));
        self.free_temp(src);
        let loop_start = self.pc();
        // For a local-variable target, write ForIter directly into the local register
        // to avoid an extra Move per iteration. For all other cases, use a temp.
        let for_dst = if let AssignTarget::Name(n) = target {
            self.local_reg(n).unwrap_or_else(|| self.alloc_temp())
        } else {
            self.alloc_temp()
        };
        let exit_jmp = self.emit(Insn::ForIter(for_dst, iter_slot, 0));
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    // local case: for_dst == reg, already written — no Move needed.
                    // Still record the store so class-body for-loops register the
                    // iteration variable in `vars(C)`.
                    self.maybe_record_class_store(reg);
                    // Issue #820: sync into module_globals_dict at module scope.
                    if self.is_module_scope {
                        let name_idx = self.intern_name(name);
                        self.emit(Insn::SyncModuleGlobal(reg, name_idx));
                    }
                } else {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::StoreGlobal(name_idx, for_dst));
                    self.free_temp(for_dst);
                }
            }
            AssignTarget::Tuple(targets) => {
                let star_pos = targets
                    .iter()
                    .position(|t| matches!(t, AssignTarget::Starred(_)));
                let n = targets.len() as u32;
                let base = for_dst + 1;

                if let Some(star_idx) = star_pos {
                    // Extended unpack: for a, *b, c in ...
                    let before = match u8::try_from(star_idx) {
                        Ok(count) => count,
                        Err(_) => {
                            self.failed = true;
                            self.is_syntax_error = true;
                            if self.error_msg.is_none() {
                                self.error_msg = Some(
                                    "too many expressions in star-unpacking assignment".into(),
                                );
                            }
                            return;
                        }
                    };
                    let after = (targets.len() - star_idx - 1) as u32;
                    if base.checked_add(n).is_none() {
                        self.failed = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(format!("too many unpack targets ({})", n));
                        }
                        return;
                    }
                    self.next_temp = base + n;
                    if self.next_temp - 1 > self.max_reg {
                        self.max_reg = self.next_temp - 1;
                    }
                    self.emit(Insn::UnpackEx {
                        src: for_dst,
                        before,
                        after,
                        dst_base: base,
                    });
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        let inner = match t {
                            AssignTarget::Starred(inner) => inner.as_ref(),
                            other => other,
                        };
                        self.compile_store_unpack_target(inner, base + i);
                    }
                } else {
                    if base.checked_add(n).is_none() {
                        self.failed = true;
                        if self.error_msg.is_none() {
                            self.error_msg = Some(format!("too many unpack targets ({})", n));
                        }
                        return;
                    }
                    self.next_temp = base + n;
                    if self.next_temp - 1 > self.max_reg {
                        self.max_reg = self.next_temp - 1;
                    }
                    self.emit(Insn::Unpack(base, for_dst, n));
                    for (i, t) in (0u32..).zip(targets.iter()) {
                        self.compile_store_unpack_target(t, base + i);
                        if self.failed {
                            return;
                        }
                    }
                }
                self.next_temp = for_dst;
            }
            _ => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("unsupported for-loop target".to_string());
                }
                return;
            }
        }
        self.loops.push(LoopCtx {
            break_patches: SmallVec::new(),
            continue_target: Some(loop_start),
            continue_patches: SmallVec::new(),
            cleanup_depth: self.except_cleanups.len(),
        });
        let saved_def_set = self.def_set;
        self.mark_target_def(target);
        self.compile_block_with_linenos(body, body_linenos);
        self.def_set = saved_def_set;
        if self.failed {
            return;
        }
        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));
        self.patch_jump(exit_jmp);
        let ctx = self.loops.pop().unwrap();
        self.free_iter();
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
        }
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }
    }

    // ── Raise / Delete / Import ───────────────────────────────────────────────
}
