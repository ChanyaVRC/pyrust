// Exception-handling, comparison slow-path, and coroutine/yield-from opcode
// helpers extracted out of the hot VM dispatch loop.

/// Outcome of executing a cold VM instruction out of line.
enum ColdInsnOutcome {
    Continue,
    Jump(i32),
    YieldFrom {
        yielded: Value,
        sent_reg: crate::bytecode::Reg,
    },
}

/// Compute a validated relative jump target PC.
#[inline(always)]
fn vm_jump_pc(pc: usize, offset: i32, code: &crate::bytecode::FnCode) -> Result<usize> {
    let insns_len = code.insns.len();
    let new_pc = pc as i64 + offset as i64;
    if new_pc < 0 || new_pc as usize > insns_len {
        pyrust_core::set_current_vm_line(code.line_at(pc.wrapping_sub(1)));
        return Err(PyError::runtime(format!(
            "internal error: jump to invalid PC {} (insns len {})",
            new_pc, insns_len
        )));
    }
    Ok(new_pc as usize)
}

#[cold]
#[inline(never)]
fn vm_oob_pc_err(pc: usize, code: &crate::bytecode::FnCode) -> PyError {
    pyrust_core::set_current_vm_line(code.line_at(pc.wrapping_sub(1)));
    PyError::runtime(format!(
        "internal error: PC {} out of bounds (insns len {})",
        pc,
        code.insns.len()
    ))
}

#[cold]
#[inline(never)]
fn vm_cell_unbound_err(code: &crate::bytecode::FnCode, cell_idx: u16) -> PyError {
    let meta = code.cell_slots[cell_idx as usize];
    unbound_binding_error(meta.name.as_str(), meta.is_free_error)
}

/// Outcome of advancing a `ForIter` user-defined iterator fallback.
enum ForIterUserOutcome {
    Item(Value),
    Exhausted,
    DriveGenerator(Rc<GeneratorCell>),
}

/// Outcome of advancing a `yield from` sub-iterator by one step.
enum YieldFromStep {
    Yielded(Value),
    Completed(Value),
}

impl Interpreter {
    /// Cold fallback for `Insn::ForIter` when the slot holds a user-defined
    /// iterator or drivable generator.
    #[inline(never)]
    fn exec_for_iter_user(
        &mut self,
        state: &mut IterState,
        cached_next: &mut Option<Box<crate::interpreter::iteration::IterNextCacheEntry>>,
    ) -> Result<ForIterUserOutcome> {
        let IterState::UserDefined(iterator) = state else {
            unreachable!("only user iterators bypass the fast-state step");
        };
        match self.advance_loop_iterator(iterator, cached_next) {
            LoopIteratorAdvance::DriveGenerator(state) => {
                Ok(ForIterUserOutcome::DriveGenerator(state))
            }
            LoopIteratorAdvance::Item(value) => Ok(ForIterUserOutcome::Item(value)),
            LoopIteratorAdvance::Error(error) if is_stop_iteration_error(&error) => {
                Ok(ForIterUserOutcome::Exhausted)
            }
            LoopIteratorAdvance::Error(error) => Err(error),
            LoopIteratorAdvance::NotIterator => Err(pyrust_core::type_err!(
                "iter() returned non-iterator of type '{}'",
                value_type_name_str(iterator),
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn exec_call_inline_binop(
        &self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        callee: crate::bytecode::Reg,
        dst: crate::bytecode::Reg,
        a: crate::bytecode::Reg,
        op: crate::ast::BinaryOp,
        b: crate::bytecode::Reg,
        proto: u16,
    ) -> bool {
        if call_depth() < max_call_depth(self)
            && let Some(result) = try_inline_leaf_binop(
                &regs[callee as usize],
                &regs[a as usize],
                &regs[b as usize],
                op,
                code,
                proto,
            )
        {
            regs[dst as usize] = result;
            return true;
        }
        false
    }
    /// Slow fallback for `CmpJumpIf*` / `CountCmpJump*` when fast integer or
    /// constant comparison declines.
    #[cold]
    #[inline(never)]
    fn exec_cmp_jump_slow(
        &mut self,
        regs: &RegSlice,
        lhs: crate::bytecode::Reg,
        op: crate::ast::BinaryOp,
        rhs: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<bool> {
        let l = vm_read(regs, lhs, num_locals)?;
        let r = vm_read(regs, rhs, num_locals)?;
        let result = self.eval_binary(l, op, r)?;
        self.truthy_value(&result)
    }

    /// Slow fallback for `CmpJumpIf*Const` when fast constant comparison declines.
    #[cold]
    #[inline(never)]
    fn exec_cmp_jump_const_slow(
        &mut self,
        regs: &RegSlice,
        lhs: crate::bytecode::Reg,
        op: crate::ast::BinaryOp,
        constant: &Value,
        num_locals: crate::bytecode::Reg,
    ) -> Result<bool> {
        let l = vm_read(regs, lhs, num_locals)?;
        let result = self.eval_binary(l, op, constant.clone())?;
        self.truthy_value(&result)
    }

    /// Dispatch exception-handling, awaitable, and `yield from` cold opcodes.
    /// Returns `Ok(None)` when `insn` belongs to another cold opcode group.
    #[allow(clippy::too_many_arguments)]
    fn exec_cold_control_insn(
        &mut self,
        insn: &crate::bytecode::Insn,
        code: &crate::bytecode::FnCode,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        exc_ctx_frame_base: usize,
        exc_saved_active_frame_base: usize,
    ) -> Result<Option<ColdInsnOutcome>> {
        use crate::bytecode::Insn;
        match insn {
            Insn::LoadExc(dst) => {
                regs[*dst as usize] = self.exec_load_exc()?;
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::LoadExcTraceback(dst, exc) => {
                regs[*dst as usize] = self.exec_load_exc_traceback(regs, *exc, num_locals)?;
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::MatchExcept(type_reg, offset) => {
                if self.exec_match_except(regs, *type_reg, num_locals)? {
                    Ok(Some(ColdInsnOutcome::Continue))
                } else {
                    Ok(Some(ColdInsnOutcome::Jump(*offset)))
                }
            }
            Insn::MatchExceptStar(desc_idx, offset) => {
                let desc = code.descs[*desc_idx as usize].as_match_except_star();
                if self.exec_match_except_star(
                    regs,
                    desc.type_reg,
                    desc.src_group,
                    desc.matched_dst,
                    num_locals,
                )? {
                    Ok(Some(ColdInsnOutcome::Continue))
                } else {
                    Ok(Some(ColdInsnOutcome::Jump(*offset)))
                }
            }
            Insn::EndExcept => {
                self.exec_end_except(exc_ctx_frame_base, exc_saved_active_frame_base);
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::PushExcContext(src) => {
                self.exec_push_exc_context(regs, *src, num_locals)?;
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::PopExcContext => {
                self.exec_pop_exc_context(exc_ctx_frame_base, exc_saved_active_frame_base);
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::RaiseAssert(msg_reg) => {
                let msg = (*msg_reg != crate::bytecode::NO_ASSERT_MSG).then_some(*msg_reg);
                self.exec_raise_assert(regs, msg, num_locals)?;
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::RaiseValue(src) => {
                self.exec_raise_value(regs, *src, num_locals)?;
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::RaiseExceptStarResidual(src) => {
                self.exec_raise_except_star_residual(
                    regs,
                    *src,
                    num_locals,
                    exc_ctx_frame_base,
                    exc_saved_active_frame_base,
                )?;
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::RaiseFrom(src, cause_reg) => {
                self.exec_raise_from(regs, *src, *cause_reg, num_locals)?;
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::RaiseReRaise => {
                self.exec_raise_reraise(exc_ctx_frame_base, exc_saved_active_frame_base)?;
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::GetAwaitable(dst, src) => {
                regs[*dst as usize] = self.exec_get_awaitable(regs, *src, num_locals)?;
                Ok(Some(ColdInsnOutcome::Continue))
            }
            Insn::YieldFrom {
                iter_reg,
                sent_reg,
                result_reg,
            } => match self.exec_yield_from_step(regs, *iter_reg, *sent_reg, num_locals)? {
                YieldFromStep::Yielded(yielded) => {
                    regs[*sent_reg as usize] = Value::none();
                    Ok(Some(ColdInsnOutcome::YieldFrom {
                        yielded,
                        sent_reg: *sent_reg,
                    }))
                }
                YieldFromStep::Completed(value) => {
                    regs[*result_reg as usize] = value;
                    Ok(Some(ColdInsnOutcome::Continue))
                }
            },
            _ => Ok(None),
        }
    }

    fn exec_load_exc(&self) -> Result<Value> {
        self.active_exception
            .clone()
            .ok_or_else(|| PyError::runtime("no active exception".to_string()))
    }

    fn exec_load_exc_traceback(
        &mut self,
        regs: &RegSlice,
        exc: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let exc_val = vm_read(regs, exc, num_locals)?;
        Ok(self.exception_traceback_value(&exc_val))
    }

    fn exec_match_except(
        &mut self,
        regs: &RegSlice,
        type_reg: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<bool> {
        let type_val = vm_read(regs, type_reg, num_locals)?;
        let exc = self.active_exception.as_ref().ok_or_else(|| {
            PyError::runtime("internal error: MatchExcept with no active exception".to_string())
        })?;
        self.exception_matches(exc, &type_val)
    }

    /// PEP 654 `except*` filter: splits `regs[src_group]` against
    /// `regs[type_reg]`. Returns `Ok(true)` if any sub-exception matched
    /// (writing `matched_dst` and updating `src_group`), `Ok(false)` otherwise.
    fn exec_match_except_star(
        &mut self,
        regs: &mut RegSlice,
        type_reg: crate::bytecode::Reg,
        src_group: crate::bytecode::Reg,
        matched_dst: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<bool> {
        let type_val = vm_read(regs, type_reg, num_locals)?;
        let group_val = vm_read(regs, src_group, num_locals)?;
        match self.split_exception_group(&group_val, &type_val)? {
            None => Ok(false),
            Some((matched_group, remaining)) => {
                regs[matched_dst as usize] = matched_group;
                regs[src_group as usize] = remaining.unwrap_or_else(Value::none);
                Ok(true)
            }
        }
    }

    /// Normal exit from an `except` handler body: pops this handler's context
    /// entry, restores the prior `active_exception`, and clears the captured
    /// error-frame snapshot.
    fn exec_end_except(&mut self, exc_ctx_frame_base: usize, exc_saved_active_frame_base: usize) {
        if self.handled_exc_stack.len() > exc_ctx_frame_base {
            self.handled_exc_stack.pop();
        }
        self.active_exception = if self.exc_saved_active.len() > exc_saved_active_frame_base {
            self.exc_saved_active.pop().unwrap()
        } else {
            None
        };
        pyrust_core::reset_captured_error_frames();
    }

    /// Temporarily install `regs[src]` as the active exception context for
    /// inlined `finally` blocks and PEP 654 `except*` handler scopes.
    fn exec_push_exc_context(
        &mut self,
        regs: &RegSlice,
        src: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let val = vm_read(regs, src, num_locals)?;
        self.cold
            .push_exc_ctx_marks
            .push((self.handled_exc_stack.len(), self.exc_saved_active.len()));
        self.exc_saved_active.push(self.active_exception.clone());
        self.handled_exc_stack.push(val.clone());
        self.active_exception = Some(val);
        Ok(())
    }

    /// Undo a `PushExcContext` after the bracketed block finishes.
    fn exec_pop_exc_context(
        &mut self,
        exc_ctx_frame_base: usize,
        exc_saved_active_frame_base: usize,
    ) {
        if let Some((saved_handled_len, saved_active_len)) = self.cold.push_exc_ctx_marks.pop() {
            let target_handled = saved_handled_len.max(exc_ctx_frame_base);
            self.handled_exc_stack.truncate(target_handled);
            let target_active = saved_active_len.max(exc_saved_active_frame_base);
            if self.exc_saved_active.len() > target_active {
                self.active_exception = self.exc_saved_active[target_active].clone();
                self.exc_saved_active.truncate(target_active);
            } else {
                self.active_exception = self.handled_exc_stack.last().cloned();
            }
        }
    }

    fn exec_raise_assert(
        &mut self,
        regs: &RegSlice,
        msg_reg: Option<crate::bytecode::Reg>,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let msg = match msg_reg {
            Some(r) => Some(vm_read(regs, r, num_locals)?),
            None => None,
        };
        let exc = self.prepare_assertion_error(msg)?;
        Err(PyError::raised(exc))
    }

    fn exec_raise_value(
        &mut self,
        regs: &RegSlice,
        src: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let val = vm_read(regs, src, num_locals)?;
        let exc = self.prepare_explicit_raise(val)?;
        Err(PyError::raised(exc))
    }

    fn exec_raise_except_star_residual(
        &mut self,
        regs: &RegSlice,
        src: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
        exc_ctx_frame_base: usize,
        exc_saved_active_frame_base: usize,
    ) -> Result<()> {
        let packed = vm_read(regs, src, num_locals)?;
        let exc = self.prepare_except_star_reraise(&packed)?;
        // Pop the try-except* handler's own exception context before
        // propagating the final residual / combined group so outer handlers
        // see the pre-try active exception.
        if self.handled_exc_stack.len() > exc_ctx_frame_base {
            self.handled_exc_stack.pop();
        }
        if self.exc_saved_active.len() > exc_saved_active_frame_base {
            self.active_exception = self.exc_saved_active.pop().unwrap();
        }
        Err(PyError::raised(exc))
    }

    fn exec_raise_from(
        &mut self,
        regs: &RegSlice,
        src: crate::bytecode::Reg,
        cause_reg: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let val = vm_read(regs, src, num_locals)?;
        let cause_raw = vm_read(regs, cause_reg, num_locals)?;
        let exc = self.prepare_explicit_raise_from(val, cause_raw)?;
        Err(PyError::raised(exc))
    }

    /// Bare `raise`, unmatched-except fall-through, and implicit `finally`
    /// re-raise (#2405).
    fn exec_raise_reraise(
        &mut self,
        exc_ctx_frame_base: usize,
        exc_saved_active_frame_base: usize,
    ) -> Result<()> {
        let exc = self
            .active_exception
            .take()
            .ok_or_else(|| PyError::runtime("No active exception to reraise".to_string()))?;
        if self.handled_exc_stack.len() > exc_ctx_frame_base {
            self.handled_exc_stack.pop();
        }
        if self.exc_saved_active.len() > exc_saved_active_frame_base {
            self.active_exception = self.exc_saved_active.pop().unwrap();
        }
        self.reraise_is_bare = true;
        self.reset_captured_frames_if_reraise(&exc);
        Err(PyError::raised(exc))
    }

    fn exec_get_awaitable(
        &mut self,
        regs: &RegSlice,
        src: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let awaited = vm_read(regs, src, num_locals)?;
        self.get_awaitable(&awaited)
    }

    /// Cold error-dispatch body invoked by `vm_try!`: publishes PEP 657 caret
    /// anchor, tries in-frame exception handling (restoring PEP 695 type-param
    /// env if caught), and falls through to `vm_unwind_error` on escape.
    #[cold]
    #[inline(never)]
    fn vm_handle_or_unwind_error(
        &mut self,
        err: PyError,
        code: &crate::bytecode::FnCode,
        frame_base_env: &EnvRef,
        st: &mut UnwindState,
    ) -> UnwindOutcome {
        let raise_pc = (*st.pc).wrapping_sub(1);
        let insn_col_span = code.col_at(raise_pc);
        let col_span = match self.cold.inlined_comp_escaped_span.take() {
            Some(comp_span) => comp_span.or(insn_col_span),
            None => insn_col_span,
        };
        pyrust_core::set_current_vm_col_span(col_span);
        let raise_line = code.line_at(raise_pc);
        match self.handle_vm_error(
            err,
            st.exc_handlers,
            st.exc_ctx_frame_base,
            &code.exc_table,
            raise_pc,
            raise_line,
        ) {
            Ok(h) => {
                if !Rc::ptr_eq(&self.env, frame_base_env) {
                    self.env = Rc::clone(frame_base_env);
                    bump_global_struct_version(self);
                }
                UnwindOutcome::Resume(h)
            }
            Err(escaped) => self.vm_unwind_error(escaped, st),
        }
    }

    /// Dispatch a pending injected exception (`generator.throw()` / `close()`)
    /// before fetching the next instruction.
    #[cold]
    #[inline(never)]
    fn vm_handle_pending_inject(
        &mut self,
        err: PyError,
        code: &crate::bytecode::FnCode,
        pc: usize,
        exc_handlers: &mut ExcHandlersBuf,
        exc_ctx_frame_base: usize,
    ) -> UnwindOutcome {
        let inject_pc = pc.wrapping_sub(1);
        let inject_col = code.col_at(inject_pc);
        let inject_line = match code.line_at(inject_pc) {
            0 => pyrust_core::get_current_vm_line(),
            ln => ln,
        };
        pyrust_core::set_current_vm_col_span(inject_col);
        match self.handle_vm_error(
            err,
            exc_handlers,
            exc_ctx_frame_base,
            &code.exc_table,
            inject_pc,
            inject_line,
        ) {
            Ok(h) => UnwindOutcome::Resume(h),
            Err(escaped) => {
                pyrust_core::set_current_vm_line(inject_line);
                pyrust_core::set_current_vm_col_span(inject_col);
                UnwindOutcome::Escape(escaped)
            }
        }
    }

    /// Evaluate loop-entry guard opcodes (`JumpIfNotInt`, `JumpIfIterNot*`,
    /// `JumpIfNotBuiltinLen`), returning `Some(offset)` when the guard fails.
    #[inline(always)]
    fn eval_entry_guard_jump(
        insn: &crate::bytecode::Insn,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        iters: &ItersBuf,
    ) -> Option<i32> {
        use crate::bytecode::Insn;
        match insn {
            Insn::JumpIfNotInt(reg, offset) => {
                let value = &regs[*reg as usize];
                (value.is_unset() || value.as_int().is_none()).then_some(*offset)
            }
            Insn::JumpIfIterNotIntRange(slot, offset) => {
                (!iter_slot_is_int_range(iters[*slot as usize].as_ref())).then_some(*offset)
            }
            Insn::JumpIfIterNotIntRangeExact(desc_idx, offset) => {
                let guard = code.descs[*desc_idx as usize].as_int_range_exact_guard();
                (!iter_slot_is_int_range_exact(iters[guard.slot as usize].as_ref(), guard))
                    .then_some(*offset)
            }
            Insn::JumpIfIterNotIndexedSeq(slot, offset) => {
                (!iter_slot_is_indexed_sequence(iters[*slot as usize].as_ref())).then_some(*offset)
            }
            Insn::JumpIfNotBuiltinLen(reg, offset) => {
                (!value_is_builtin_len(&regs[*reg as usize])).then_some(*offset)
            }
            _ => None,
        }
    }

    /// Advance a PEP 380 `YieldFrom` sub-iterator one step.
    fn exec_yield_from_step(
        &mut self,
        regs: &RegSlice,
        iter_reg: crate::bytecode::Reg,
        sent_reg: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<YieldFromStep> {
        let iter_val = vm_read(regs, iter_reg, num_locals)?;
        let sent_val = vm_read(regs, sent_reg, num_locals)?;
        match self.yield_from_advance(&iter_val, sent_val) {
            Ok(yielded) => Ok(YieldFromStep::Yielded(yielded)),
            Err(ref e) if is_stop_iteration_error(e) => {
                let stop_val = extract_stop_iteration_value(e);
                Ok(YieldFromStep::Completed(
                    stop_val.unwrap_or_else(Value::none),
                ))
            }
            Err(e) => Err(e),
        }
    }
}
