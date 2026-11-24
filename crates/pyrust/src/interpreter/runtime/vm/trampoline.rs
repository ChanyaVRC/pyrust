// VM trampoline entry/exit, generator-drive switch-back, and cold error
// dispatch helpers. Included by `vm/execute.rs` so they share the private VM
// scope and remain guarded by every VM-loop ownership invariant.

/// Result of the execution-free plain `Insn::Call` callee probe.
enum PlainCallProbe {
    Identity(Value),
    Fast(BuiltinVectorcallDispatch),
    Slow {
        function: Value,
        cache_probe: BuiltinCallProbe,
    },
}

/// Outcome of attempting to enter the in-loop call trampoline (#2234 / #2345).
enum TrampEnter<T> {
    /// Active frame switched to the callee; caller should `continue 'vm`.
    Entered,
    /// Trampoline gate declined; caller falls back to the native call path.
    Fallback(T),
    /// Recursion limit reached inside the gate; caller unwinds directly.
    UnwindErr(PyError),
    /// Argument register read failed; caller routes through `vm_try!`.
    VmErr(PyError),
}

/// Gate check shared by plain-function and bound-method trampolining (#2234 / #2345).
///
/// Ensures the callee is a plain, loop-free, handler-free, local-env-free
/// regular function, the caller is outside any `try`, and the pre-reserved
/// `tramp_arena` has room without reallocating (which would dangle saved
/// `RegSlice`s).
#[inline(always)]
fn vm_tramp_check_gate<'a>(
    f: &'a pyrust_core::UserFunction,
    code: &crate::bytecode::FnCode,
    pc: usize,
    exc_handlers: &ExcHandlersBuf,
    nsupplied: usize,
    tramp_arena: &mut Vec<Value>,
) -> Option<(&'a crate::bytecode::FnCode, usize)> {
    if !matches!(f.kind, pyrust_core::UserFunctionKind::Regular) || !f.global_names.is_empty() {
        return None;
    }
    if !exc_handlers.is_empty()
        || (code.has_exc_handlers
            && code
                .exc_table
                .get(pc - 1)
                .copied()
                .is_none_or(|t| t != crate::bytecode::EXC_NO_HANDLER))
    {
        return None;
    }
    if nsupplied != f.params.len()
        || f.params
            .iter()
            .any(|p| p.is_args || p.is_kwargs || p.is_keyword_only)
    {
        return None;
    }
    // Borrow `FnCode` directly out of `f.precompiled_code` without cloning its
    // `Rc`: `VmFrameView::function` keeps `f` (and its immutable
    // `precompiled_code`) alive until `vm_tramp_return` restores `code_ptr` and
    // pops the view (#2924).
    let callee_code = f
        .precompiled_code
        .as_deref()
        .and_then(|any| any.downcast_ref::<crate::bytecode::FnCode>())?;
    // A coroutine function (`async def`, #1039) must build a coroutine object
    // instead of executing inline, even when its body has no `await`.
    if callee_code.is_generator
        || callee_code.is_coroutine
        || callee_code.num_iters != 0
        || callee_code.has_exc_handlers
        || callee_code.has_own_cells()
    {
        return None;
    }
    let num_regs = callee_code.num_regs as usize;
    let base = tramp_arena.len();
    if tramp_arena.is_empty() {
        // Reserve once while no slices are live so later `resize`s never
        // reallocate (which would dangle saved/active `RegSlice`s).
        tramp_arena.reserve(16 * 1024);
    }
    if base + num_regs > tramp_arena.capacity() {
        return None;
    }
    Some((callee_code, base))
}

impl Interpreter {
    /// Finalize a call-trampoline frame switch after parameters have been bound
    /// into `st.tramp_arena[base .. base + num_regs]`.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn vm_tramp_commit_frame(
        &mut self,
        dst: u32,
        base: usize,
        num_regs: usize,
        callee_code_ptr: *const crate::bytecode::FnCode,
        callee_num_locals: u32,
        callee_is_class_method: bool,
        view_fn: Rc<pyrust_core::UserFunction>,
        depth_guard: CallDepthGuard,
        memo_key: Option<MemoKey>,
        st: &mut UnwindState,
    ) {
        // SAFETY: `base + num_regs <= capacity` was verified by the gate, so
        // no reallocation occurred and the pointer stays valid until this
        // frame's `truncate` on return.
        let new_ptr = unsafe { st.tramp_arena.as_mut_ptr().add(base) };
        // Claim only after every gate and register read succeeded. A same-key
        // recursive miss gets no second owner but still enters the trampoline.
        let memo_key = memo_key
            .and_then(|key| self.claim_memoized_call(key))
            .map(Box::new);
        let saved_env = if Rc::ptr_eq(&self.env, &view_fn.env) {
            None
        } else {
            Some(std::mem::replace(&mut self.env, Rc::clone(&view_fn.env)))
        };
        let callee_local_index = Rc::clone(&view_fn.local_index);
        // Freevar-only closures (`own_cell_count == 0`) borrow `f.closure_cells`
        // directly with zero allocation; `VmFrameView::function` keeps `f` alive
        // until return restores `saved_cells` (#452 / #2348).
        let callee_cells = CellSlice::from_slice(&view_fn.closure_cells);
        let callee_nonlocal_names = if view_fn.nonlocal_names.is_empty() {
            None
        } else {
            Some(Rc::clone(&view_fn.nonlocal_names))
        };
        let new_regs = unsafe { RegSlice::from_raw(new_ptr, num_regs) };
        st.tramp_stack.push(TrampFrame {
            _depth_guard: depth_guard,
            saved_regs: std::mem::replace(st.regs, new_regs),
            saved_cells: *st.cells,
            saved_pc: *st.pc,
            dst,
            saved_base: *st.tramp_active_base,
            saved_cur_line: *st.cur_line,
            saved_code_ptr: *st.code_ptr,
            saved_active_code_rc: st.active_code_rc.take(),
            saved_num_locals: *st.num_locals,
            saved_env,
            memo_key,
        });
        // Publish a frame view so `locals()` / `sys._getframe()` / tracebacks
        // observe this trampolined frame like a natively-called one.
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Function,
            regs_ptr: unsafe { std::ptr::NonNull::new_unchecked(new_ptr) },
            regs_len: num_regs,
            cells: callee_cells,
            local_index: callee_local_index,
            nonlocal_names: callee_nonlocal_names,
            env: None,
            is_class_method: callee_is_class_method,
            function: Some(view_fn),
            gen_frame: None,
        });
        *st.tramp_active_base = base;
        *st.code_ptr = callee_code_ptr;
        *st.num_locals = callee_num_locals;
        *st.cells = callee_cells;
        *st.pc = 0;
    }

    /// Direct owned-`Rc<UserFunction>` trampoline entry used by `Insn::Call`
    /// (#2924): moves `func` into `VmFrameView::function` without cloning a
    /// second `Rc`, or hands it back on gate rejection.
    #[inline(always)]
    fn vm_tramp_try_user_fn(
        &mut self,
        code: &crate::bytecode::FnCode,
        func_reg: u32,
        argc: u8,
        func: Rc<pyrust_core::UserFunction>,
        st: &mut UnwindState,
    ) -> Result<TrampEnter<Rc<pyrust_core::UserFunction>>> {
        let Some((callee_code, base)) = vm_tramp_check_gate(
            &func,
            code,
            *st.pc,
            st.exc_handlers,
            argc as usize,
            st.tramp_arena,
        ) else {
            return Ok(TrampEnter::Fallback(func));
        };
        let num_regs = callee_code.num_regs as usize;
        let callee_num_locals = callee_code.num_locals;
        let callee_is_class_method = callee_code.is_class_method;
        let callee_code_ptr: *const crate::bytecode::FnCode = callee_code;
        let Some(depth_guard) = CallDepthGuard::try_enter(max_call_depth(self)) else {
            return Ok(TrampEnter::UnwindErr(self.recursion_limit_error()?));
        };
        let nparams = func.params.len();
        st.tramp_arena.resize(base + num_regs, Value::unset());
        for i in 0..nparams {
            let v = match vm_read(st.regs, func_reg + 1 + i as u32, *st.num_locals) {
                Ok(v) => v,
                Err(e) => {
                    st.tramp_arena.truncate(base);
                    return Ok(TrampEnter::VmErr(e));
                }
            };
            if let pyrust_core::ParamBind::Reg(r) = func.param_binds[i] {
                st.tramp_arena[base + r as usize] = v;
            }
        }
        if let Some(slot) = func.self_bind {
            st.tramp_arena[base + slot as usize] = Value::user_function(Rc::clone(&func));
        }
        self.vm_tramp_commit_frame(
            func_reg,
            base,
            num_regs,
            callee_code_ptr,
            callee_num_locals,
            callee_is_class_method,
            func,
            depth_guard,
            None,
            st,
        );
        Ok(TrampEnter::Entered)
    }

    /// Plain-call trampoline entry (`Insn::Call` slow path & `Insn::CallMemo`).
    #[inline(always)]
    fn vm_tramp_try_plain(
        &mut self,
        code: &crate::bytecode::FnCode,
        func_reg: u32,
        argc: u8,
        func_val: &Value,
        memo_key: Option<MemoKey>,
        st: &mut UnwindState,
    ) -> Result<TrampEnter<()>> {
        let ValueKind::UserFunction(f) = func_val.kind() else {
            return Ok(TrampEnter::Fallback(()));
        };
        let Some((callee_code, base)) = vm_tramp_check_gate(
            f,
            code,
            *st.pc,
            st.exc_handlers,
            argc as usize,
            st.tramp_arena,
        ) else {
            return Ok(TrampEnter::Fallback(()));
        };
        let num_regs = callee_code.num_regs as usize;
        let callee_num_locals = callee_code.num_locals;
        let callee_is_class_method = callee_code.is_class_method;
        let callee_code_ptr: *const crate::bytecode::FnCode = callee_code;
        let Some(depth_guard) = CallDepthGuard::try_enter(max_call_depth(self)) else {
            return Ok(TrampEnter::UnwindErr(self.recursion_limit_error()?));
        };
        let nparams = f.params.len();
        st.tramp_arena.resize(base + num_regs, Value::unset());
        for i in 0..nparams {
            let v = match vm_read(st.regs, func_reg + 1 + i as u32, *st.num_locals) {
                Ok(v) => v,
                Err(e) => {
                    st.tramp_arena.truncate(base);
                    return Ok(TrampEnter::VmErr(e));
                }
            };
            if let pyrust_core::ParamBind::Reg(r) = f.param_binds[i] {
                st.tramp_arena[base + r as usize] = v;
            }
        }
        if let Some(slot) = f.self_bind {
            st.tramp_arena[base + slot as usize] = func_val.clone();
        }
        self.vm_tramp_commit_frame(
            func_reg,
            base,
            num_regs,
            callee_code_ptr,
            callee_num_locals,
            callee_is_class_method,
            Rc::clone(f),
            depth_guard,
            memo_key,
            st,
        );
        Ok(TrampEnter::Entered)
    }

    /// Bound-method trampoline entry (#2345): binds `recv` to param 0 (`self`)
    /// and `args_base .. args_base + argc` to params `1..=argc`.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn vm_tramp_try_method(
        &mut self,
        code: &crate::bytecode::FnCode,
        dst: u32,
        f: &Rc<pyrust_core::UserFunction>,
        recv: Value,
        args_base: u32,
        argc: u8,
        st: &mut UnwindState,
    ) -> Result<TrampEnter<()>> {
        let Some((callee_code, base)) = vm_tramp_check_gate(
            f,
            code,
            *st.pc,
            st.exc_handlers,
            (argc as usize) + 1,
            st.tramp_arena,
        ) else {
            return Ok(TrampEnter::Fallback(()));
        };
        let num_regs = callee_code.num_regs as usize;
        let callee_num_locals = callee_code.num_locals;
        let callee_is_class_method = callee_code.is_class_method;
        let callee_code_ptr: *const crate::bytecode::FnCode = callee_code;
        let Some(depth_guard) = CallDepthGuard::try_enter(max_call_depth(self)) else {
            return Ok(TrampEnter::UnwindErr(self.recursion_limit_error()?));
        };
        st.tramp_arena.resize(base + num_regs, Value::unset());
        if let pyrust_core::ParamBind::Reg(r) = f.param_binds[0] {
            st.tramp_arena[base + r as usize] = recv;
        }
        for i in 0..argc as u32 {
            let v = match vm_read(st.regs, args_base + i, *st.num_locals) {
                Ok(v) => v,
                Err(e) => {
                    st.tramp_arena.truncate(base);
                    return Ok(TrampEnter::VmErr(e));
                }
            };
            if let pyrust_core::ParamBind::Reg(r) = f.param_binds[1 + i as usize] {
                st.tramp_arena[base + r as usize] = v;
            }
        }
        self.vm_tramp_commit_frame(
            dst,
            base,
            num_regs,
            callee_code_ptr,
            callee_num_locals,
            callee_is_class_method,
            Rc::clone(f),
            depth_guard,
            None,
            st,
        );
        Ok(TrampEnter::Entered)
    }

    /// Execute `Insn::CallMemo`, probing the memo cache and entering the VM
    /// trampoline on eligible misses before falling back to native calls.
    #[inline]
    fn vm_exec_call_memo(
        &mut self,
        code: &crate::bytecode::FnCode,
        func_reg: u32,
        argc: u8,
        st: &mut UnwindState,
    ) -> Result<TrampEnter<()>> {
        let num_locals = *st.num_locals;
        let pc = *st.pc;
        let cur_line = *st.cur_line;
        let func_val = match vm_read(st.regs, func_reg, num_locals) {
            Ok(v) => v,
            Err(e) => return Ok(TrampEnter::VmErr(e)),
        };
        let probe = match self.probe_memoized_call(st.regs, func_reg, argc, num_locals, &func_val) {
            Ok(p) => p,
            Err(e) => return Ok(TrampEnter::VmErr(e)),
        };
        match probe {
            MemoCallProbe::Hit(result) => {
                st.regs[func_reg as usize] = result;
                Ok(TrampEnter::Fallback(()))
            }
            MemoCallProbe::Miss(key) => {
                match self.vm_tramp_try_plain(
                    code,
                    func_reg,
                    argc,
                    &func_val,
                    Some(key.clone()),
                    st,
                )? {
                    TrampEnter::Fallback(()) => {}
                    other => return Ok(other),
                }
                match self
                    .call_memoized_miss_native(st.regs, func_reg, argc, num_locals, &func_val, key)
                {
                    Ok(v) => {
                        st.regs[func_reg as usize] = v;
                        Ok(TrampEnter::Fallback(()))
                    }
                    Err(e) => Ok(TrampEnter::VmErr(e)),
                }
            }
            MemoCallProbe::Bypass => {
                match self.vm_tramp_try_plain(code, func_reg, argc, &func_val, None, st)? {
                    TrampEnter::Fallback(()) => {}
                    other => return Ok(other),
                }
                match self.call_positional_cached(
                    st.regs,
                    num_locals,
                    func_reg,
                    argc,
                    func_val,
                    code,
                    pc - 1,
                    cur_line,
                    PositionalCallCacheProbe::Unprobed,
                ) {
                    Ok(v) => {
                        st.regs[func_reg as usize] = v;
                        Ok(TrampEnter::Fallback(()))
                    }
                    Err(e) => Ok(TrampEnter::VmErr(e)),
                }
            }
        }
    }

    /// Return `value` from the active frame: finalize a driven generator (#2253),
    /// pop a call-trampoline frame (#2234), or return `Some(value)` to exit the
    /// bottom VM frame.
    #[inline(always)]
    fn vm_tramp_return(&mut self, value: Value, st: &mut UnwindState) -> Option<Value> {
        if st
            .gen_drive_stack
            .last()
            .is_some_and(|g| g.tramp_floor == st.tramp_stack.len())
        {
            self.vm_gen_drive_return(value, st);
            return None;
        }
        if let Some(saved) = st.tramp_stack.pop() {
            let detached_view = self.detach_vm_frame_view();
            let memo_key = saved.memo_key;
            st.tramp_arena.truncate(*st.tramp_active_base);
            *st.tramp_active_base = saved.saved_base;
            *st.regs = saved.saved_regs;
            *st.cells = saved.saved_cells;
            *st.pc = saved.saved_pc;
            *st.cur_line = saved.saved_cur_line;
            // Restore the caller's code / locals / env BEFORE dropping the
            // detached frame view (whose `function` `Rc<UserFunction>` keeps the
            // callee's `FnCode` alive, #2924) and finalizing any escaping frame
            // cache (#3028).
            *st.code_ptr = saved.saved_code_ptr;
            *st.active_code_rc = saved.saved_active_code_rc;
            *st.num_locals = saved.saved_num_locals;
            if let Some(saved_env) = saved.saved_env {
                self.env = saved_env;
            }
            drop(detached_view);
            self.finish_pop_vm_frame_view();
            if let Some(key) = memo_key {
                self.finish_memoized_call(*key, &value);
            }
            st.regs[saved.dst as usize] = value;
            return None;
        }
        Some(value)
    }

    /// Driven generator returned (`return` / fell off the end): mark it
    /// exhausted, restore its consumer, and jump to the `ForIter` exit PC.
    #[inline(always)]
    fn vm_gen_drive_return(&mut self, value: Value, st: &mut UnwindState) {
        let mut gd = st.gen_drive_stack.pop().unwrap();
        gd.gframe.done = true;
        gd.gframe.last_return_value = Some(value);
        self.sync_top_generator_frame_cache_if_present();
        self.vm_frame_views.pop();
        *st.regs = gd.saved_regs;
        *st.cells = gd.saved_cells;
        *st.pc = gd.exit_pc;
        *st.cur_line = gd.saved_cur_line;
        *st.code_ptr = gd.saved_code_ptr;
        *st.active_code_rc = gd.saved_active_code_rc;
        *st.num_locals = gd.saved_num_locals;
        self.env = gd.saved_env;
        *st.iters = gd.saved_iters;
        *st.iter_next_cache = gd.saved_iter_cache;
        *st.exc_handlers = gd.saved_exc_handlers;
        *st.tramp_active_base = gd.saved_base;
        let boxed: Box<dyn std::any::Any> = gd.gframe;
        *gd.state_rc.borrow_mut() = boxed;
    }

    /// Driven generator suspended (`Yield` / `YieldFrom`, #2253 / #2338): save
    /// its resume state, restore its consumer, and deliver `yielded` to the
    /// consumer's `ForIter` destination register.
    #[inline(always)]
    fn vm_gen_drive_yield(
        &mut self,
        yielded: Value,
        resume_pc: usize,
        yield_dst: crate::bytecode::Reg,
        st: &mut UnwindState,
    ) {
        let mut gd = st.gen_drive_stack.pop().unwrap();
        gd.gframe.pc = resume_pc;
        gd.gframe.iters = std::mem::take(st.iters);
        gd.gframe.exc_handlers = std::mem::take(st.exc_handlers);
        gd.gframe.yield_dst = yield_dst;
        // Issue #2445: persist the yield / yield-from line so a later
        // `generator.throw()` reports it.
        gd.gframe.suspended_line = *st.cur_line;
        self.sync_top_generator_frame_cache_if_present();
        self.vm_frame_views.pop();
        let dst_reg = gd.dst as usize;
        *st.regs = gd.saved_regs;
        *st.cells = gd.saved_cells;
        *st.pc = gd.saved_pc;
        *st.cur_line = gd.saved_cur_line;
        *st.code_ptr = gd.saved_code_ptr;
        *st.active_code_rc = gd.saved_active_code_rc;
        *st.num_locals = gd.saved_num_locals;
        self.env = gd.saved_env;
        *st.iters = gd.saved_iters;
        *st.iter_next_cache = gd.saved_iter_cache;
        *st.exc_handlers = gd.saved_exc_handlers;
        *st.tramp_active_base = gd.saved_base;
        let boxed: Box<dyn std::any::Any> = gd.gframe;
        *gd.state_rc.borrow_mut() = boxed;
        st.regs[dst_reg] = yielded;
    }

    /// Build a native `FrameOutcome::Yielded` suspension, splitting PEP 3134
    /// exception state at this frame's base depths.
    #[allow(clippy::too_many_arguments)]
    fn vm_native_yield(
        &mut self,
        yielded: Value,
        resume_pc: usize,
        yield_dst: crate::bytecode::Reg,
        cur_line: u32,
        exc_ctx_frame_base: usize,
        exc_saved_active_frame_base: usize,
        iters: &mut ItersBuf,
        exc_handlers: &mut ExcHandlersBuf,
    ) -> FrameOutcome {
        let saved_handled_slice: HandledExcBuf =
            self.handled_exc_stack.split_off(exc_ctx_frame_base).into();
        let saved_exc_saved_active = self.exc_saved_active.split_off(exc_saved_active_frame_base);
        let saved_active = self.active_exception.take();
        FrameOutcome::Yielded {
            value: yielded,
            saved: GenSaveState {
                iters: std::mem::take(iters),
                exc_handlers: std::mem::take(exc_handlers),
                pc: resume_pc,
                handled_exc_slice: saved_handled_slice,
                active_exception: saved_active,
                exc_saved_active_slice: saved_exc_saved_active,
                yield_dst,
                suspended_line: cur_line,
            },
        }
    }
}
