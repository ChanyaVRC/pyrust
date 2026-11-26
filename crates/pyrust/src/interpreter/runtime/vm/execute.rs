include!("trampoline.rs");
include!("control_ops.rs");
include!("match_ops.rs");
include!("cold_ops.rs");
include!("hooks.rs");

impl Interpreter {
    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_inner_impl(
        &mut self,
        code: &crate::bytecode::FnCode,
        mut regs: RegSlice,
        mut cells: CellSlice,
        iters_init: ItersBuf,
        exc_handlers_init: ExcHandlersBuf,
        start_pc: usize,
        inject_exc: Option<PyError>,
        gen_handled_slice: HandledExcBuf,
        gen_active_exception: Option<Value>,
        gen_exc_saved_active: Vec<Option<Value>>,
    ) -> Result<FrameOutcome> {
        use crate::bytecode::Insn;

        let mut code_ptr: *const crate::bytecode::FnCode = code;
        let mut num_locals = code.num_locals;
        let mut iters: ItersBuf = iters_init;
        let mut iter_next_cache: IterCacheBuf = if iters.is_empty() {
            IterCacheBuf::new()
        } else {
            smallvec::smallvec![None; iters.len()]
        };
        let mut exc_handlers: ExcHandlersBuf = exc_handlers_init;
        let mut pc: usize = start_pc;
        // Safepoint epoch this activation last released its dead temporaries at;
        // a resumed generator frame (`start_pc != 0`) starts out stale.
        let mut safepoint_epoch: usize = self
            .cold
            .safepoint_epoch
            .wrapping_sub(usize::from(start_pc != 0));
        let mut pending_inject: Option<PyError> = inject_exc;

        let exc_ctx_frame_base: usize = self.handled_exc_stack.len();
        let exc_saved_active_frame_base: usize = self.exc_saved_active.len();
        let frame_base_env: EnvRef = Rc::clone(&self.env);

        if !gen_handled_slice.is_empty() {
            self.handled_exc_stack.extend(gen_handled_slice);
        }
        if !gen_exc_saved_active.is_empty() {
            self.exc_saved_active.extend(gen_exc_saved_active);
        }
        if gen_active_exception.is_some() {
            self.active_exception = gen_active_exception;
        }

        let tramp_base: usize = self.tramp_stack.len();
        let gen_base: usize = self.gen_drive_stack.len();
        let mut tramp_active_base: usize = usize::MAX;

        'vm: loop {
            // Re-derive the active frame's `code` from `code_ptr` (updated on a
            // trampolined frame switch). Zero-cost: a raw-pointer reborrow.
            let code: &crate::bytecode::FnCode = unsafe { &*code_ptr };

            macro_rules! unwind_state {
                () => {
                    UnwindState {
                        regs: &mut regs,
                        cells: &mut cells,
                        pc: &mut pc,
                        code_ptr: &mut code_ptr,
                        num_locals: &mut num_locals,
                        iters: &mut iters,
                        iter_next_cache: &mut iter_next_cache,
                        exc_handlers: &mut exc_handlers,
                        tramp_active_base: &mut tramp_active_base,
                        tramp_base,
                        gen_base,
                        exc_ctx_frame_base,
                    }
                };
            }
            macro_rules! tramp_return {
                ($v:expr) => {{
                    let __v = $v;
                    let mut __st = unwind_state!();
                    match self.vm_tramp_return(__v, &mut __st) {
                        None => continue 'vm,
                        Some(__ret) => return Ok(FrameOutcome::Returned(__ret)),
                    }
                }};
            }
            macro_rules! tramp_unwind_err {
                ($e:expr) => {{
                    let mut __st = unwind_state!();
                    match self.vm_unwind_error($e, &mut __st) {
                        UnwindOutcome::Resume(__h) => {
                            pc = __h;
                            continue 'vm;
                        }
                        UnwindOutcome::Escape(__err) => return Err(__err),
                    }
                }};
            }
            macro_rules! vm_try {
                ($expr:expr) => {
                    match $expr {
                        Ok(v) => v,
                        Err(e) => {
                            let mut __st = unwind_state!();
                            match self.vm_handle_or_unwind_error(
                                e,
                                code,
                                &frame_base_env,
                                &mut __st,
                            ) {
                                UnwindOutcome::Resume(__h) => {
                                    pc = __h;
                                    continue 'vm;
                                }
                                UnwindOutcome::Escape(__err) => return Err(__err),
                            }
                        }
                    }
                };
            }
            macro_rules! tramp_finish {
                ($res:expr) => {
                    match $res {
                        Ok(TrampEnter::Entered) => continue 'vm,
                        Ok(TrampEnter::Fallback(__v)) => __v,
                        Ok(TrampEnter::UnwindErr(__err)) => tramp_unwind_err!(__err),
                        Ok(TrampEnter::VmErr(__err)) => {
                            vm_try!(Err::<(), _>(__err));
                            unreachable!()
                        }
                        Err(__err) => tramp_unwind_err!(__err),
                    }
                };
            }
            macro_rules! vm_yield {
                ($yielded:expr, $resume_pc:expr, $yield_dst:expr) => {{
                    let __resume_pc = $resume_pc;
                    let __yield_line = code.line_at(pc.wrapping_sub(1));
                    if self.gen_drive_stack.len() > gen_base
                        && self
                            .gen_drive_stack
                            .last()
                            .is_some_and(|__g| __g.tramp_floor == self.tramp_stack.len())
                    {
                        let mut __st = unwind_state!();
                        self.vm_gen_drive_yield(
                            $yielded,
                            __resume_pc,
                            $yield_dst,
                            __yield_line,
                            &mut __st,
                        );
                        continue 'vm;
                    }
                    return Ok(self.vm_native_yield(
                        $yielded,
                        __resume_pc,
                        $yield_dst,
                        __yield_line,
                        exc_ctx_frame_base,
                        exc_saved_active_frame_base,
                        &mut iters,
                        &mut exc_handlers,
                    ));
                }};
            }
            macro_rules! pool_get {
                ($pool:expr, $idx:expr, $tag:literal) => {
                    vm_try!(vm_pool_get(&($pool), $idx, $tag))
                };
            }
            macro_rules! jump_pc {
                ($offset:expr) => {
                    match vm_jump_pc(pc, $offset, code) {
                        Ok(__pc) => __pc,
                        Err(__err) => {
                            if self.tramp_stack.len() > tramp_base
                                || self.gen_drive_stack.len() > gen_base
                            {
                                tramp_unwind_err!(__err);
                            }
                            return Err(__err);
                        }
                    }
                };
            }
            macro_rules! cond_jump {
                ($cond:expr, $want:expr, $offset:expr) => {{
                    if let Some(c) = try_scalar_truthiness_fast(&regs[$cond as usize]) {
                        if c == $want {
                            pc = jump_pc!($offset);
                        }
                        continue;
                    }
                    let cond_val = vm_try!(vm_read(&regs, $cond, num_locals));
                    if vm_try!(self.truthy_value(&cond_val)) == $want {
                        pc = jump_pc!($offset);
                    }
                }};
            }
            macro_rules! cmp_jump {
                ($lhs:expr, $op:expr, $rhs:expr, $want:expr, $offset:expr) => {{
                    if let Some(c) =
                        try_integer_compare_fast(&regs[$lhs as usize], &regs[$rhs as usize], $op)
                    {
                        if c == $want {
                            pc = jump_pc!($offset);
                        }
                        continue;
                    }
                    if vm_try!(self.exec_cmp_jump_slow(&regs, $lhs, $op, $rhs, num_locals)) == $want
                    {
                        pc = jump_pc!($offset);
                    }
                }};
            }
            macro_rules! cmp_jump_const {
                ($lhs:expr, $op:expr, $const_idx:expr, $want:expr, $offset:expr) => {{
                    let cv = pool_get!(code.consts, $const_idx, "const");
                    if let Some(c) = try_constant_compare_fast(&regs[$lhs as usize], cv, $op) {
                        if c == $want {
                            pc = jump_pc!($offset);
                        }
                        continue;
                    }
                    if vm_try!(self.exec_cmp_jump_const_slow(&regs, $lhs, $op, cv, num_locals))
                        == $want
                    {
                        pc = jump_pc!($offset);
                    }
                }};
            }

            if let Some(e) = pending_inject.take() {
                match self.vm_handle_pending_inject(
                    e,
                    code,
                    pc,
                    &mut exc_handlers,
                    exc_ctx_frame_base,
                ) {
                    UnwindOutcome::Resume(h) => {
                        pc = h;
                        continue 'vm;
                    }
                    UnwindOutcome::Escape(err) => return Err(err),
                }
            }
            let Some(insn) = code.insns.get(pc) else {
                if pc == code.insns.len() {
                    tramp_return!(Value::none());
                }
                let err = vm_oob_pc_err(pc, code);
                if self.tramp_stack.len() > tramp_base || self.gen_drive_stack.len() > gen_base {
                    tramp_unwind_err!(err);
                }
                return Err(err);
            };
            let hooks = self.vm_hooks_flags;
            if hooks & VM_HOOK_STEP_MASK != 0 {
                // With only `weakref` / `__del__` armed, visit the cold safepoint
                // just when a temporary can die here, a callback is queued, or
                // the safepoint was switched on since this activation last ran it.
                let epoch = self.cold.safepoint_epoch;
                if safepoint_epoch != epoch
                    || hooks & VM_HOOK_EVERY_STEP_MASK != 0
                    || self.vm_safepoint_due(code, pc)
                {
                    vm_try!(self.vm_dispatch_step_hooks(
                        code,
                        pc,
                        regs.non_null_ptr(),
                        regs.len(),
                        num_locals,
                        safepoint_epoch != epoch,
                    ));
                    // Only the activation's base frame is known released; a caller
                    // frame below stays stale until control returns to it.
                    if self.tramp_stack.len() == tramp_base
                        && self.gen_drive_stack.len() == gen_base
                    {
                        safepoint_epoch = epoch;
                    }
                }
            }
            pc += 1;

            match insn {
                // ── Hot loads & moves ────────────────────────────────────
                Insn::LoadConst(dst, idx) => {
                    regs[*dst as usize] = pool_get!(code.consts, *idx, "const").clone();
                }
                Insn::LoadGlobal(dst, name_idx) => {
                    regs[*dst as usize] = vm_try!(self.load_global_cached(code, *name_idx));
                }
                Insn::StoreGlobal(name_idx, src) => {
                    let name = pool_get!(code.names, *name_idx, "name");
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    self.assign_name(name, val);
                }
                Insn::LoadCell(dst, cell_idx) => match cells.as_slice()[*cell_idx as usize].get() {
                    Some(val) => regs[*dst as usize] = val,
                    None => vm_try!(Err(vm_cell_unbound_err(code, *cell_idx))),
                },
                Insn::StoreCell(cell_idx, src) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    cells.as_slice()[*cell_idx as usize].set(val);
                }
                Insn::LoadNone(dst) => regs[*dst as usize] = Value::none(),
                Insn::Move(dst, src) => {
                    regs[*dst as usize] = vm_try!(vm_read(&regs, *src, num_locals));
                }

                // ── Hot arithmetic, logic & indexing ─────────────────────
                Insn::BinOp(dst, lhs, op, rhs) => {
                    vm_try!(
                        self.exec_binop(&mut regs, code, pc, *dst, *lhs, *op, *rhs, num_locals)
                    );
                }
                Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                    vm_try!(self.exec_binop_in_place(&mut regs, *dst, *lhs, *op, *rhs, num_locals));
                }
                Insn::BinOpConst(dst, lhs, op, const_idx, is_aug) => {
                    let constant = pool_get!(code.consts, *const_idx, "const");
                    vm_try!(self.exec_binop_const(
                        &mut regs, *dst, *lhs, *op, constant, *is_aug, num_locals,
                    ));
                }
                Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
                    vm_try!(self.exec_binop_immediate(
                        &mut regs, *dst, *lhs, *op, *imm, *is_aug, num_locals,
                    ));
                }
                Insn::UnaryOp(dst, op, src) => {
                    let value = vm_try!(vm_read(&regs, *src, num_locals));
                    if let Some(result) = try_tagged_int_unary!(value, *op) {
                        regs[*dst as usize] = result;
                        continue 'vm;
                    }
                    regs[*dst as usize] = vm_try!(self.eval_unary(value, *op));
                }
                Insn::Concat { dst, base, count } => {
                    regs[*dst as usize] =
                        vm_try!(self.eval_concat_fast(&regs, *base, *count, num_locals));
                }
                Insn::GetAttr(dst, obj, name_idx) => {
                    vm_try!(
                        self.exec_get_attr(&mut regs, code, pc, *dst, *obj, *name_idx, num_locals)
                    );
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    vm_try!(
                        self.exec_set_attr(&mut regs, code, pc, *obj, *name_idx, *val, num_locals)
                    );
                }
                Insn::GetItem(dst, obj, idx) => {
                    regs[*dst as usize] =
                        vm_try!(self.exec_get_item(&regs, num_locals, *obj, *idx));
                }
                Insn::GetSlice(dst, obj, base) => {
                    regs[*dst as usize] =
                        vm_try!(self.exec_get_slice(&regs, num_locals, *obj, *base));
                }
                Insn::SetItem(obj, idx, val) => {
                    vm_try!(self.exec_set_item(&mut regs, num_locals, *obj, *idx, *val));
                }

                // ── Hot control flow & specialized guards ────────────────
                Insn::Jump(offset) => pc = jump_pc!(*offset),
                Insn::JumpIfFalse(cond, offset) => cond_jump!(*cond, false, *offset),
                Insn::JumpIfTrue(cond, offset) => cond_jump!(*cond, true, *offset),
                Insn::CmpJumpIfFalse(lhs, op, rhs, offset) => {
                    cmp_jump!(*lhs, *op, *rhs, false, *offset);
                }
                Insn::CmpJumpIfTrue(lhs, op, rhs, offset) => {
                    cmp_jump!(*lhs, *op, *rhs, true, *offset);
                }
                Insn::CmpJumpIfFalseConst(lhs, op, const_idx, offset) => {
                    cmp_jump_const!(*lhs, *op, *const_idx, false, *offset);
                }
                Insn::CmpJumpIfTrueConst(lhs, op, const_idx, offset) => {
                    cmp_jump_const!(*lhs, *op, *const_idx, true, *offset);
                }
                Insn::CountCmpJumpTrue(var, op, stop, imm, offset) => {
                    vm_try!(self.exec_binop_immediate(
                        &mut regs,
                        *var,
                        *var,
                        crate::ast::BinaryOp::Add,
                        *imm,
                        true,
                        num_locals,
                    ));
                    cmp_jump!(*var, *op, *stop, true, *offset);
                }
                Insn::CountCmpJumpFalse(var, op, stop, imm, offset) => {
                    vm_try!(self.exec_binop_immediate(
                        &mut regs,
                        *var,
                        *var,
                        crate::ast::BinaryOp::Add,
                        *imm,
                        true,
                        num_locals,
                    ));
                    cmp_jump!(*var, *op, *stop, false, *offset);
                }
                Insn::JumpIfNotInt(..)
                | Insn::JumpIfIterNotIntRange(..)
                | Insn::JumpIfIterNotIntRangeExact(..)
                | Insn::JumpIfIterNotIndexedSeq(..)
                | Insn::JumpIfNotBuiltinLen(..) => {
                    if let Some(offset) = Self::eval_entry_guard_jump(insn, code, &regs, &iters) {
                        pc = jump_pc!(offset);
                    }
                }
                Insn::LenSeqOrExit(dst, seq, offset) => {
                    match try_builtin_sequence_len(&regs[*seq as usize]) {
                        Some(length) => regs[*dst as usize] = Value::int(length),
                        None => pc = jump_pc!(*offset),
                    }
                }
                Insn::GetItemSeqIntOrExit(dst, obj, idx, offset) => {
                    match try_indexed_sequence_int_element(
                        &regs[*obj as usize],
                        &regs[*idx as usize],
                    ) {
                        Some(element) => regs[*dst as usize] = element,
                        None => pc = jump_pc!(i32::from(*offset)),
                    }
                }
                Insn::CallInlineBinOp(desc_idx, skip) => {
                    let desc = code.descs[*desc_idx as usize].as_call_inline_binop();
                    if self.vm_hooks_flags & (VM_HOOK_TRACE | VM_HOOK_PROFILE) == 0
                        && self.exec_call_inline_binop(
                            &mut regs,
                            code,
                            desc.callee,
                            desc.dst,
                            desc.a,
                            desc.op,
                            desc.b,
                            desc.proto,
                        )
                    {
                        pc = jump_pc!(*skip);
                    }
                }
                Insn::SetupExcept(offset) => exc_handlers.push(jump_pc!(*offset)),
                Insn::PopExcept => {
                    exc_handlers.pop();
                }

                // ── Hot calls ────────────────────────────────────────────
                Insn::Call(func_reg, argc) => {
                    if regs[*func_reg as usize].is_unset() {
                        let owned = vm_read(&regs, *func_reg, num_locals);
                        vm_try!(owned);
                        unreachable!();
                    }
                    let user_fn = match regs[*func_reg as usize].kind() {
                        ValueKind::UserFunction(f) => Some(Rc::clone(f)),
                        _ => None,
                    };
                    if let Some(f) = user_fn {
                        let f = tramp_finish!(self.vm_tramp_try_user_fn(
                            code,
                            *func_reg,
                            *argc,
                            f,
                            &mut unwind_state!(),
                        ));
                        regs[*func_reg as usize] = vm_try!(self.call_positional_cached(
                            &mut regs,
                            num_locals,
                            *func_reg,
                            *argc,
                            Value::user_function(f),
                            code,
                            pc - 1,
                            PositionalCallCacheProbe::Probed(BuiltinCallProbe::Uncacheable),
                        ));
                        continue 'vm;
                    }
                    let probe = {
                        let function = &regs[*func_reg as usize];
                        let profile_disabled = self.vm_hooks_flags & VM_HOOK_PROFILE == 0;
                        if profile_disabled
                            && *argc == 1
                            && let Some(argument) = regs.get((*func_reg + 1) as usize)
                            && let Some(identity) =
                                Self::try_identity_builtin_call(function, argument)
                        {
                            PlainCallProbe::Identity(identity)
                        } else {
                            let argument_base = (*func_reg + 1) as usize;
                            let argument_end = argument_base + *argc as usize;
                            match Self::probe_builtin_vectorcall(
                                code,
                                pc - 1,
                                function,
                                &(*regs)[argument_base..argument_end],
                            ) {
                                BuiltinCallProbe::Vector(dispatch) if profile_disabled => {
                                    PlainCallProbe::Fast(dispatch)
                                }
                                cache_probe => PlainCallProbe::Slow {
                                    function: function.clone(),
                                    cache_probe: if profile_disabled {
                                        cache_probe
                                    } else {
                                        BuiltinCallProbe::Uncacheable
                                    },
                                },
                            }
                        }
                    };
                    let (func_val, cache_probe) = match probe {
                        PlainCallProbe::Identity(identity) => {
                            regs[*func_reg as usize] = identity;
                            continue 'vm;
                        }
                        PlainCallProbe::Fast(dispatch) => {
                            let base = (*func_reg + 1) as usize;
                            let res = dispatch(self, &(*regs)[base..(base + *argc as usize)]);
                            regs[*func_reg as usize] = vm_try!(res);
                            continue 'vm;
                        }
                        PlainCallProbe::Slow {
                            function,
                            cache_probe,
                        } => (function, cache_probe),
                    };
                    if let ValueKind::BoundMethod { function, receiver } = func_val.kind()
                        && vm_tramp_arity_may_fit(function.trampoline_arity, (*argc as usize) + 1)
                    {
                        let f = Rc::clone(function);
                        let recv = Value::py_instance(Rc::clone(receiver));
                        tramp_finish!(self.vm_tramp_try_method(
                            code,
                            *func_reg,
                            &f,
                            recv,
                            *func_reg + 1,
                            *argc,
                            &mut unwind_state!(),
                        ));
                    }
                    regs[*func_reg as usize] = vm_try!(self.call_positional_cached(
                        &mut regs,
                        num_locals,
                        *func_reg,
                        *argc,
                        func_val,
                        code,
                        pc - 1,
                        PositionalCallCacheProbe::Probed(cache_probe),
                    ));
                }

                Insn::CallMemo(func_reg, argc) => {
                    tramp_finish!(self.vm_exec_call_memo(
                        code,
                        *func_reg,
                        *argc,
                        &mut unwind_state!(),
                    ));
                }

                Insn::CallMethod {
                    dst,
                    obj,
                    name_idx,
                    args_base,
                    nargs,
                } => {
                    if let Some(method) = code.names.get(*name_idx as usize)
                        && let Some((f, recv_rc)) =
                            self.resolve_method_cached(&regs, *obj, method, code, pc - 1)
                        && vm_tramp_arity_may_fit(f.trampoline_arity, (*nargs as usize) + 1)
                    {
                        let recv = Value::py_instance(recv_rc);
                        tramp_finish!(self.vm_tramp_try_method(
                            code,
                            *dst,
                            &f,
                            recv,
                            *args_base,
                            *nargs,
                            &mut unwind_state!(),
                        ));
                    }
                    let r = self.exec_call_method(
                        &mut regs,
                        num_locals,
                        *dst,
                        *obj,
                        *name_idx,
                        *args_base,
                        *nargs,
                        code,
                        pc - 1,
                    );
                    regs[*dst as usize] = vm_try!(r);
                }

                // ── Hot returns, append, yield & iteration ───────────────
                Insn::Return(src) => tramp_return!(vm_try!(vm_read(&regs, *src, num_locals))),
                Insn::ReturnNone => tramp_return!(Value::none()),
                Insn::ListAppend(list_reg, val_reg) => {
                    let val = vm_try!(vm_read(&regs, *val_reg, num_locals));
                    vm_try!(regs[*list_reg as usize].list_push(val));
                }
                Insn::Yield { src, dst } => {
                    let yielded = vm_try!(vm_read(&regs, *src, num_locals));
                    regs[*dst as usize] = Value::none();
                    vm_yield!(yielded, pc, *dst);
                }
                Insn::GetIter(slot, src) => {
                    let src_val = vm_try!(vm_read(&regs, *src, num_locals));
                    iters[*slot as usize] = Some(vm_try!(self.make_loop_iter_state(src_val)));
                    iter_next_cache[*slot as usize] = None;
                }
                Insn::ForIter(dst, slot, offset) => {
                    let mut gen_to_drive: Option<Rc<GeneratorCell>> = None;
                    match iters[*slot as usize].as_mut() {
                        Some(state) => {
                            match advance_loop_fast_state(state, code, &mut regs, &mut pc, *dst) {
                                LoopFastOutcome::Advanced => {}
                                LoopFastOutcome::Exhausted => pc = jump_pc!(*offset),
                                LoopFastOutcome::Error(error) => vm_try!(Err(error)),
                                LoopFastOutcome::UserDefined => {
                                    let cached = &mut iter_next_cache[*slot as usize];
                                    match vm_try!(self.exec_for_iter_user(state, cached)) {
                                        ForIterUserOutcome::Item(v) => regs[*dst as usize] = v,
                                        ForIterUserOutcome::Exhausted => pc = jump_pc!(*offset),
                                        ForIterUserOutcome::DriveGenerator(s) => {
                                            gen_to_drive = Some(s);
                                        }
                                    }
                                }
                            }
                        }
                        None => pc = jump_pc!(*offset),
                    }
                    if let Some(state_rc) = gen_to_drive {
                        let exit_pc = jump_pc!(*offset);
                        let mut st = unwind_state!();
                        vm_try!(self.vm_enter_gen_drive(state_rc, *dst, exit_pc, &mut st));
                        continue 'vm;
                    }
                }
                Insn::CheckLocal(reg, name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.check_local_binding(&regs[*reg as usize], name));
                }

                // ── Cold/slow opcodes (out-of-line) ──────────────────────
                other => match vm_try!(self.exec_cold_insn(
                    other,
                    code,
                    &mut regs,
                    cells,
                    pc,
                    num_locals,
                    exc_ctx_frame_base,
                    exc_saved_active_frame_base,
                )) {
                    ColdInsnOutcome::Continue => {}
                    ColdInsnOutcome::Jump(offset) => pc = jump_pc!(offset),
                    ColdInsnOutcome::YieldFrom { yielded, sent_reg } => {
                        vm_yield!(yielded, pc - 1, sent_reg);
                    }
                },
            }
        }
    }
}
