impl Interpreter {
    /// Execute one non-suspending bytecode frame.
    ///
    /// Script, class-body, and ordinary user-function callers all publish their
    /// own [`VmFrameView`] before entering here; frame-kind policy must be
    /// derived from that view rather than from call-site-specific VM metadata.
    /// `regs` must be pre-sized to `code.num_regs`, with any parameter slots
    /// already filled.
    /// Takes `RegSlice` (raw pointer + len) rather than `&mut [Value]` so that the
    /// `VmFrameView` raw pointer stored before this call does not alias an `&mut [Value]`
    /// that carries LLVM `noalias` (issue #547, PR #646 Copilot review).
    pub(super) fn run_bytecode(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: RegSlice,
    ) -> Result<Value> {
        match self.run_bytecode_inner(
            code,
            regs,
            smallvec::smallvec![None; code.num_iters as usize],
            ExcHandlersBuf::new(),
            0,
            None,
            HandledExcBuf::new(),
            None,
            Vec::new(),
        )? {
            FrameOutcome::Returned(v) => Ok(v),
            FrameOutcome::Yielded { .. } => {
                unreachable!("run_bytecode called on a non-generator function")
            }
        }
    }

    /// Resume (or initialise) a generator by executing from `frame.pc` until
    /// the next yield or completion.  The sent value is `None` (equivalent to
    /// `next(g)`).  Returns:
    /// - `Ok(val)`  — generator yielded `val`; frame updated in-place
    /// - completion error — generator returned normally
    /// - `Err(other)` — propagating exception
    pub(crate) fn resume_generator(&mut self, frame: &mut GeneratorFrame) -> Result<Value> {
        self.resume_generator_with_exc(frame, None, Value::none())
    }

    /// Resume a generator with an explicit sent value, optionally injecting
    /// `inject_exc` as if it had been raised at the current yield point.
    /// Underpins `generator.close()` (inject `GeneratorExit`),
    /// `generator.throw()` (inject user exception), and `generator.send()`
    /// (no injection, non-None `sent_value`).
    ///
    /// The `sent_value` is written to the destination register of the last
    /// `Yield` instruction before execution resumes.  For fresh generators
    /// (`frame.pc == 0`, never yielded yet) the sent value write is skipped
    /// because `yield_dst` is not yet meaningful.
    ///
    /// Returns:
    /// - `Ok(val)` — generator yielded `val`; frame updated in-place
    /// - completion error — generator returned normally
    /// - `Err(other)` — propagating exception
    pub(crate) fn resume_generator_with_exc(
        &mut self,
        frame: &mut GeneratorFrame,
        inject_exc: Option<PyError>,
        sent_value: Value,
    ) -> Result<Value> {
        if frame.done {
            // A done generator can't be resumed; if the caller is injecting an
            // exception, propagate it unchanged so close()/throw() can decide
            // how to handle it.
            if let Some(e) = inject_exc {
                return Err(e);
            }
            // Exhausted generator: StopIteration() with no args → .value is None.
            return Err(self.generator_completion_error(Vec::new())?);
        }

        // PEP 380 throw forwarding: if we are suspended at a YieldFrom instruction
        // and an exception is being injected (generator.throw() / generator.close()),
        // forward the exception to the sub-iterator rather than injecting it into our
        // own body.  This matches CPython's implementation of the PEP 380 algorithm.
        if let Some(ref exc) = inject_exc
            && let Some(crate::bytecode::Insn::YieldFrom {
                iter_reg,
                sent_reg,
                result_reg,
            }) = frame.code.insns.get(frame.pc)
        {
            let iter_reg = *iter_reg;
            let sent_reg = *sent_reg;
            let result_reg = *result_reg;
            let iter_val = frame.regs[iter_reg as usize].clone();
            let forward_result = self.yield_from_throw_forward(&iter_val, exc.clone());
            match forward_result {
                Ok(yielded) => {
                    // Sub-iterator caught the exception and yielded: yield this value
                    // to the outer caller, remaining suspended at YieldFrom.
                    frame.regs[sent_reg as usize] = Value::none();
                    return Ok(yielded);
                }
                Err(ref e) if is_stop_iteration_error(e) => {
                    // Sub-iterator returned after handling the throw.
                    // Extract the return value and store in result_reg, then let
                    // the generator body continue after the YieldFrom.
                    let stop_val = extract_stop_iteration_value(e);
                    frame.regs[result_reg as usize] = stop_val.unwrap_or_else(Value::none);
                    // Advance past the YieldFrom instruction so the VM resumes
                    // at the instruction after it.
                    frame.pc += 1;
                    // Fall through to the normal resume path with no injection
                    // and None sent value (the result is already in result_reg).
                    // We drop inject_exc by reconstructing as None below.
                    return self.resume_generator_with_exc(frame, None, Value::none());
                }
                Err(e) => {
                    // Sub-iterator did not handle the exception (or raised a new
                    // one): inject into our own body via the normal path.
                    // Fall through to the regular VM run with this exception.
                    // We need to inject the exception into the generator body.
                    // Use the normal pending_inject path: advance past YieldFrom
                    // and inject the exception as if it was raised there.
                    frame.pc += 1;
                    return self.resume_generator_with_exc(frame, Some(e), Value::none());
                }
            }
        }

        // Write the sent value into the yield destination register.
        // Skipped for fresh generators (pc == 0) because yield_dst is not
        // yet initialised and the generator body hasn't reached its first yield.
        if frame.pc != 0 {
            frame.regs[frame.yield_dst as usize] = sent_value;
        }

        // Swap the saved env in.
        let previous_env = std::mem::replace(&mut self.env, Rc::clone(&frame.saved_env));

        // PEP 3134: hand the generator's persisted exception state into
        // `run_bytecode_inner`, which will push it onto
        // `handled_exc_stack` AFTER capturing the caller's base depth.
        // The matching split-off on a subsequent yield, plus the
        // wrapper's truncate-on-exit, then keep the caller's view of
        // `handled_exc_stack` untouched throughout.
        let gen_handled = std::mem::take(&mut frame.handled_exc_slice);
        let gen_active = frame.active_exception.take();
        let gen_exc_saved_active = std::mem::take(&mut frame.exc_saved_active_slice);

        // Issue #483 review: publish a Function frame view so `locals()`
        // called inside the generator body sees the generator's own
        // fastlocals instead of the caller's frame.  Popped immediately
        // after `run_bytecode_inner` returns — including on yield, where
        // the regs slice stays valid (it's owned by `frame.regs`) but
        // the view is no longer the innermost frame from the caller's
        // perspective.
        // Issue #486: also capture nonlocal_names + env from the
        // generator's saved_env so `locals()` can surface nonlocal
        // bindings captured at generator-creation time.
        let gen_nonlocal_names = {
            let env = frame.saved_env.borrow();
            if env.nonlocal_names.is_empty() {
                None
            } else {
                Some(Rc::clone(&env.nonlocal_names))
            }
        };
        let gen_env_opt = if gen_nonlocal_names.is_some() {
            Some(Rc::clone(&frame.saved_env))
        } else {
            None
        };
        // Capture the raw pointer and length BEFORE constructing RegSlice so
        // both the VmFrameView and the dispatch loop share the same raw pointer
        // with no &mut [Value] in scope (eliminating the noalias UB, issue #547).
        let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(frame.regs.as_mut_ptr()) };
        let regs_len = frame.regs.len();
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Function,
            // SAFETY: `GeneratorFrame` is heap-allocated inside a
            // `Box<GeneratorFrame>` (owned by the generator `Value`).
            // `regs` lives inside that boxed frame, so whether the
            // SmallVec uses its inline storage (for <= VM_REGS_INLINE
            // registers) or spills to a separate allocation, the
            // pointer is stable across yields — the Box is not moved
            // or dropped while the generator is alive.  SmallVec / Vec
            // allocations are always non-null.  Popped immediately after
            // `run_bytecode_inner` returns (including on yield).
            regs_ptr,
            regs_len,
            local_index: Rc::clone(&frame.local_index),
            nonlocal_names: gen_nonlocal_names,
            env: gen_env_opt,
            is_class_method: frame.code.is_class_method,
            // Generator frames do not retain the originating `UserFunction`;
            // traceback `tb_frame` for a generator-raised exception is built
            // from the lazily-captured `FrameInfo` (which carries `fn_name`).
            function: None,
            // Issue #2445/#2471: store a pointer to this generator frame so a
            // traceback built when an exception is *caught inside* the body
            // (e.g. `generator.throw()`) can recover the generator's name +
            // filename lazily on the cold path and attribute the catching frame
            // to the generator instead of falling back to `<module>`.  `frame`
            // is borrowed for the entire `run_bytecode_inner` call below, and
            // the view is popped before that borrow ends, so the pointer stays
            // valid for the view's lifetime.
            gen_frame: Some(std::ptr::NonNull::from(&*frame)),
        });
        // SAFETY: regs_ptr is valid for regs_len Values for the lifetime of
        // frame.regs (which outlives this call).  No &mut [Value] referencing
        // frame.regs is held while the dispatch loop runs; RegSlice (raw
        // pointer + len) is used instead, removing the LLVM noalias constraint
        // that made the VmFrameView dereferences UB (issue #547).
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        // Issue #2445: when injecting an exception (generator.throw()/close()),
        // seed the dispatch loop's `cur_line` with the suspended yield line so a
        // traceback for an exception caught inside the body reports the yield
        // line rather than the caller's throw call site.  Only relevant once the
        // generator has yielded at least once (pc != 0 ⇒ suspended_line set).
        if inject_exc.is_some() && frame.pc != 0 {
            pyrust_core::set_current_vm_line(frame.suspended_line);
        }
        let result = self.run_bytecode_inner(
            &frame.code,
            regs_slice,
            std::mem::take(&mut frame.iters),
            std::mem::take(&mut frame.exc_handlers),
            frame.pc,
            inject_exc,
            gen_handled,
            gen_active,
            gen_exc_saved_active,
        );
        // Lazy traceback: record the generator frame's `FrameInfo` only when an
        // exception propagated out of the body (issue #908).  On yield
        // (Ok(Yielded)) the body suspended successfully — nothing to record.
        if result.is_err() {
            // Generator's own code-object filename (#2438): a generator defined
            // in an imported module reports its module's source file.
            let tb_filename = frame.code.filename.clone();
            let tb_lineno = match pyrust_core::get_current_vm_line() {
                0 => None,
                n => Some(n),
            };
            pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                filename: tb_filename,
                lineno: tb_lineno,
                source_line: None,
                funcname: frame.fn_name.clone(),
                globals: Some(pyrust_core::FrameGlobals::for_environment(&frame.saved_env)),
                col_span: None,
            });
        }
        self.vm_frame_views.pop();

        // Restore env.
        self.env = previous_env;

        match result {
            Ok(FrameOutcome::Yielded { value, saved }) => {
                // Generator suspended: restore frame state from the outcome.
                frame.iters = saved.iters;
                frame.exc_handlers = saved.exc_handlers;
                frame.pc = saved.pc;
                frame.handled_exc_slice = saved.handled_exc_slice;
                frame.active_exception = saved.active_exception;
                frame.exc_saved_active_slice = saved.exc_saved_active_slice;
                frame.yield_dst = saved.yield_dst;
                frame.suspended_line = saved.suspended_line;
                Ok(value)
            }
            Ok(FrameOutcome::Returned(ret_val)) => {
                // Generator returned normally (fell off end or hit explicit `return`).
                // Stash the return value so Insn::YieldFrom can extract it as
                // StopIteration.value (PEP 380 §3 step 4).
                frame.last_return_value = Some(ret_val.clone());
                frame.done = true;
                // CPython synthesises `StopIteration()` with *empty* args when a
                // generator returns `None` (falls off the end or `return`/`return
                // None`); only a non-None return value is passed as the single
                // arg, so `.args` is `()` rather than `(None,)` in the common
                // case (`.value` is `None` either way).
                let args = if ret_val.is_none() {
                    vec![]
                } else {
                    vec![ret_val]
                };
                Err(self.generator_completion_error(args)?)
            }
            Err(e) => {
                // Propagating exception or other error.
                frame.done = true;
                // PEP 479 (enforced since Python 3.7): if a StopIteration (or
                // any subclass) escapes from a generator frame, convert it to
                // RuntimeError("generator raised StopIteration").  The original
                // exception becomes the __cause__ of the RuntimeError.
                Err(pep479_wrap_stop_iteration(e))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_inner(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: RegSlice,
        iters_init: ItersBuf,
        exc_handlers_init: ExcHandlersBuf,
        start_pc: usize,
        // When `Some`, dispatched through the existing exception-handler stack
        // before executing the first instruction.  Used by
        // `resume_generator_with_exc` to inject `GeneratorExit` (for `close()`)
        // or a user-supplied exception (for `throw()`) at the current yield.
        inject_exc: Option<PyError>,
        // For generator resumes: this frame's persisted slice of
        // `handled_exc_stack` (PEP 3134) and the `active_exception` it had
        // at the last yield, both stashed in the `GeneratorFrame`.  The
        // wrapper pushes the slice onto `self.handled_exc_stack` AFTER
        // capturing the caller's base depth, so the slice is treated as
        // belonging to this frame; the next yield re-captures it via
        // `split_off(exc_ctx_frame_base)`.  Pass `HandledExcBuf::new()` and `None`
        // for fresh, non-generator invocations.
        gen_handled_slice: HandledExcBuf,
        gen_active_exception: Option<Value>,
        // Parallel save-stack for `active_exception` across nested except blocks
        // within this generator frame.  Split off at yield and re-extended on
        // resume.  Pass `Vec::new()` for non-generator invocations.
        gen_exc_saved_active: Vec<Option<Value>>,
    ) -> Result<FrameOutcome> {
        // Record per-frame exception state at entry.  On any exit path
        // restore so the caller's frame sees the same `active_exception`
        // it left behind, regardless of whether this callee raised,
        // returned, ran past the end, or yielded.
        //
        // For yields, the `Yield` opcode itself has already split the
        // generator's slice of `handled_exc_stack` off into the returned
        // `FrameOutcome::Yielded::saved` and cleared `active_exception`,
        // so the stack length is back at the caller's depth.  For non-yield exits, truncate
        // any leftover handler-stack entries.  In both cases re-install
        // the caller's `active_exception`, since the generator's view of
        // it must never persist outside this frame.
        let exc_ctx_entry_depth = self.handled_exc_stack.len();
        let exc_saved_active_entry_depth = self.exc_saved_active.len();
        let saved_active = self.active_exception.clone();
        // Save and reset push_exc_ctx_depth: PushExcContext/PopExcContext are
        // always emitted as matched pairs within a single function body, but if
        // the finally block raises (causing the frame to exit abnormally),
        // PopExcContext is never executed and the depth would remain non-zero.
        // Resetting at frame entry/exit keeps the counter scoped to this frame.
        let saved_push_exc_ctx_depth = std::mem::replace(&mut self.push_exc_ctx_depth, 0);

        let result = self.run_bytecode_inner_impl(
            code,
            regs,
            iters_init,
            exc_handlers_init,
            start_pc,
            inject_exc,
            gen_handled_slice,
            gen_active_exception,
            gen_exc_saved_active,
        );

        // For `Yielded`, the `Yield` opcode already stripped the generator's
        // exc-stack entries and cleared `active_exception` before returning, so
        // `handled_exc_stack` is already at `exc_ctx_entry_depth` on that path.
        // `truncate` is idempotent so it's safe to call in every branch.
        self.handled_exc_stack.truncate(exc_ctx_entry_depth);
        self.exc_saved_active.truncate(exc_saved_active_entry_depth);
        self.active_exception = saved_active;
        self.push_exc_ctx_depth = saved_push_exc_ctx_depth;
        result
    }

    /// Switch the active dispatch frame to a drivable generator (#2253): check
    /// its `GeneratorFrame` out of its `Value` (leaving a placeholder so the
    /// frame's heap address stays stable and a re-entrant self-drive errors
    /// cleanly), publish its frame view, save the consumer onto `gen_drive_stack`,
    /// and rebind the dispatch state to the generator.  On `Ok(())` the caller
    /// `continue`s the loop in the generator; on `Err` (recursion limit) the
    /// caller routes it through the normal error path.
    ///
    /// `#[cold]` + `#[inline(never)]` keeps this (one-off-per-drive-entry) setup
    /// out of the hot `ForIter` arm so it doesn't bloat the dispatch frame or the
    /// per-step iteration code.
    #[cold]
    #[inline(never)]
    fn vm_enter_gen_drive(
        &mut self,
        state_rc: Rc<RefCell<Box<dyn std::any::Any>>>,
        dst: u32,
        exit_pc: usize,
        st: &mut UnwindState,
    ) -> Result<()> {
        // Every native, call-trampolined, and generator-driven frame owns a
        // CallDepthGuard, so the TLS value remains complete even when an arena
        // boundary creates a nested native VM entry.
        if call_depth() >= max_call_depth(self) {
            return Err(self.recursion_limit_error()?);
        }
        // Check the generator frame out of its `Value`.
        let mut gframe: Box<GeneratorFrame> = {
            let mut b = state_rc.borrow_mut();
            let taken = std::mem::replace(&mut *b, Box::new(GenDriving) as Box<dyn std::any::Any>);
            match taken.downcast::<GeneratorFrame>() {
                Ok(g) => g,
                Err(orig) => {
                    *b = orig;
                    unreachable!("vm_enter_gen_drive on a non-GeneratorFrame")
                }
            }
        };
        // Deliver the implicit `None` sent value to a suspended generator
        // (skipped for a fresh one, pc == 0, whose yield_dst is not yet meaningful).
        if gframe.pc != 0 {
            let yd = gframe.yield_dst as usize;
            gframe.regs[yd] = Value::none();
        }
        let gen_regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(gframe.regs.as_mut_ptr()) };
        let gen_regs_len = gframe.regs.len();
        // Publish the generator's frame view (mirrors `resume_generator_with_exc`).
        let gen_nonlocal_names = {
            let env = gframe.saved_env.borrow();
            if env.nonlocal_names.is_empty() {
                None
            } else {
                Some(Rc::clone(&env.nonlocal_names))
            }
        };
        let gen_env_opt = if gen_nonlocal_names.is_some() {
            Some(Rc::clone(&gframe.saved_env))
        } else {
            None
        };
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Function,
            regs_ptr: gen_regs_ptr,
            regs_len: gen_regs_len,
            local_index: Rc::clone(&gframe.local_index),
            nonlocal_names: gen_nonlocal_names,
            env: gen_env_opt,
            is_class_method: gframe.code.is_class_method,
            function: None,
            // Issue #2445/#2471: see the matching site in
            // `resume_generator_with_exc`.  `gframe` is a `Box<GeneratorFrame>`
            // whose heap allocation is stable across the move into
            // `GenDriveFrame::gframe` below (only the box pointer moves), so the
            // pointer remains valid while this view is on the stack.
            gen_frame: Some(std::ptr::NonNull::from(&*gframe)),
        });
        let gen_code_rc = Rc::clone(&gframe.code);
        let gen_code_ptr: *const crate::bytecode::FnCode = Rc::as_ptr(&gen_code_rc);
        let gen_num_locals = gframe.code.num_locals;
        let gen_pc = gframe.pc;
        let gen_iters = std::mem::take(&mut gframe.iters);
        let gen_exc_handlers = std::mem::take(&mut gframe.exc_handlers);
        let gen_env = Rc::clone(&gframe.saved_env);
        let gen_iters_len = gen_iters.len();
        st.gen_drive_stack.push(GenDriveFrame {
            _depth_guard: CallDepthGuard::enter(),
            state_rc,
            gframe,
            saved_regs: std::mem::replace(st.regs, unsafe {
                RegSlice::from_raw(gen_regs_ptr.as_ptr(), gen_regs_len)
            }),
            saved_pc: *st.pc,
            exit_pc,
            dst,
            saved_base: *st.tramp_active_base,
            saved_cur_line: *st.cur_line,
            saved_code_ptr: *st.code_ptr,
            saved_active_code_rc: st.active_code_rc.take(),
            saved_num_locals: *st.num_locals,
            saved_env: std::mem::replace(&mut self.env, gen_env),
            saved_iters: std::mem::replace(st.iters, gen_iters),
            saved_iter_cache: std::mem::replace(
                st.iter_next_cache,
                smallvec::smallvec![None; gen_iters_len],
            ),
            saved_exc_handlers: std::mem::replace(st.exc_handlers, gen_exc_handlers),
            tramp_floor: st.tramp_stack.len(),
        });
        // Rebind the dispatch state to the generator.  `regs` / `iters` /
        // `iter_next_cache` / `exc_handlers` were swapped in place above.
        *st.code_ptr = gen_code_ptr;
        *st.active_code_rc = Some(gen_code_rc);
        *st.num_locals = gen_num_locals;
        *st.tramp_active_base = usize::MAX;
        *st.pc = gen_pc;
        Ok(())
    }

    /// Unwind active trampolined frames on the error path, interleaving the call
    /// trampoline (#2234) and the generator trampoline (#2253).
    ///
    /// Pops call frames (recording each in the traceback, popping its frame view,
    /// restoring the caller's env) down to the current generator's `tramp_floor`;
    /// if a gen-drive frame is then at the boundary, the error belongs to *its*
    /// consumer, so the generator is finalized (marked done, its traceback
    /// recorded, an escaping `StopIteration` PEP 479-wrapped), the consumer is
    /// restored, and the error is re-dispatched at the consumer's `ForIter` site
    /// through the consumer's handler stack — which either catches it
    /// (`Resume(pc)`) or lets it continue unwinding.  With both stacks empty this
    /// is the bottom frame's escape (`Escape(err)`).
    ///
    /// `#[cold]` + `#[inline(never)]`: keeping this whole body out of line is
    /// what prevents the ~180 `vm_try!` expansion sites from each carrying a copy
    /// of it in `run_bytecode_inner_impl`'s (debug) stack frame, which would
    /// overflow the native stack on deep non-trampolined recursion.
    #[cold]
    #[inline(never)]
    fn vm_unwind_error(&mut self, mut err: PyError, st: &mut UnwindState) -> UnwindOutcome {
        let mut line = *st.cur_line;
        // PEP 657 caret anchor of the raising instruction (#2411).  The VM
        // published it (before handler dispatch) for the *innermost* frame —
        // the one that actually raised — which is the first frame popped here.
        // Outer trampolined frames raised at their own call sites whose anchors
        // we don't track on the trampoline, so only the innermost frame claims
        // this span; the rest stay caret-free (a missing caret beats a wrong one).
        let mut innermost_col = pyrust_core::get_current_vm_col_span();
        loop {
            let floor = st.gen_drive_stack.last().map_or(0, |g| g.tramp_floor);
            while st.tramp_stack.len() > floor {
                let saved = st.tramp_stack.pop().unwrap();
                if let Some(key) = saved.memo_key.as_ref() {
                    self.cancel_memoized_call(key);
                }
                // PEP 657 stage 2 (#2443): the *caller* of the frame recorded
                // below raised at its call instruction, whose anchor is
                // `col_table[saved_pc - 1]` in the caller's code object
                // (`saved_code_ptr`).  Compute it now and carry it forward as the
                // next iteration's `innermost_col`, so every trampolined frame —
                // not just the innermost — claims its own caret.  `saved_pc` is the
                // caller's resume pc (already advanced past the call), so the
                // raising instruction's index is `saved_pc - 1`; `(0,0,0,0)` means
                // "no anchor" and stays caret-free.  SAFETY: `saved_code_ptr`
                // points at the caller's `FnCode`, kept alive by
                // `saved_active_code_rc` (or the still-live bottom frame) until this
                // frame is restored.
                let caller_call_col = {
                    let caller_code: &crate::bytecode::FnCode = unsafe { &*saved.saved_code_ptr };
                    caller_code
                        .col_table
                        .get(saved.saved_pc.wrapping_sub(1))
                        .copied()
                        .filter(|&s| s != (0, 0, 0, 0))
                };
                if let Some(view) = self.vm_frame_views.pop()
                    && let Some(func) = view.function
                {
                    let frame_col = innermost_col;
                    // Callee's own code-object filename (#2438): a trampolined
                    // call into an imported module's function reports that
                    // module's source file, not the running script's path.
                    let file = func
                        .precompiled_code
                        .as_ref()
                        .and_then(|rc| Rc::clone(rc).downcast::<crate::bytecode::FnCode>().ok())
                        .map(|c| c.filename.clone())
                        .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
                    pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                        filename: file,
                        lineno: if line == 0 { None } else { Some(line) },
                        source_line: None,
                        funcname: std::sync::Arc::from(&func.name[..]),
                        globals: Some(pyrust_core::FrameGlobals::for_environment(&func.env)),
                        col_span: frame_col,
                    });
                }
                innermost_col = caller_call_col;
                self.env = saved.saved_env;
                line = saved.saved_cur_line;
            }
            let Some(mut gd) = st.gen_drive_stack.pop() else {
                break;
            };
            // The error escaped the generator body: finalize it.
            gd.gframe.done = true;
            if self.vm_frame_views.pop().is_some() {
                // Generator's own code-object filename (#2438): a generator from
                // an imported module reports that module's source file.
                let file = gd.gframe.code.filename.clone();
                pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                    filename: file,
                    lineno: if line == 0 { None } else { Some(line) },
                    source_line: None,
                    funcname: gd.gframe.fn_name.clone(),
                    globals: Some(pyrust_core::FrameGlobals::for_environment(
                        &gd.gframe.saved_env,
                    )),
                    col_span: None,
                });
            }
            // Restore the consumer frame.
            let foriter_pc = gd.saved_pc.wrapping_sub(1);
            *st.regs = gd.saved_regs;
            line = gd.saved_cur_line;
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
            // PEP 479: a `StopIteration` escaping a generator body becomes
            // `RuntimeError` in the *consumer's* env.
            err = pep479_wrap_stop_iteration(err);
            // Re-raise at the consumer's `ForIter` instruction.
            let ccode: &crate::bytecode::FnCode = unsafe { &**st.code_ptr };
            match self.handle_vm_error(
                err,
                st.exc_handlers,
                st.exc_ctx_frame_base,
                &ccode.exc_table,
                foriter_pc,
                line,
            ) {
                Ok(h) => {
                    *st.cur_line = line;
                    return UnwindOutcome::Resume(h);
                }
                Err(e2) => {
                    err = e2;
                    // The error now escapes the *consumer* at its `ForIter` site;
                    // that instruction's anchor is the innermost caret for the
                    // frames unwound on the next outer-loop iteration (#2443).
                    let ccode: &crate::bytecode::FnCode = unsafe { &**st.code_ptr };
                    innermost_col = ccode
                        .col_table
                        .get(foriter_pc)
                        .copied()
                        .filter(|&s| s != (0, 0, 0, 0));
                    continue;
                }
            }
        }
        *st.cur_line = line;
        pyrust_core::set_current_vm_line(line);
        // PEP 657 stage 2 (#2443): publish the call-site anchor of the frame that
        // natively entered this trampoline (computed as the last `caller_call_col`
        // carried into `innermost_col`) so the native caller's `FrameInfo` — built
        // by `calls` (function frame) or `program_execution` (module frame) from
        // `get_current_vm_col_span()` — also claims its caret, not just the
        // trampolined frames recorded above.
        pyrust_core::set_current_vm_col_span(innermost_col);
        UnwindOutcome::Escape(err)
    }

    fn handle_vm_error(
        &mut self,
        e: PyError,
        exc_handlers: &mut ExcHandlersBuf,
        exc_ctx_frame_base: usize,
        // Zero-cost exception table (CPython 3.11 model).  When non-empty it is
        // the source of truth: `exc_table[raise_pc]` gives the handler PC for an
        // exception raised at `raise_pc`, and the dynamic `exc_handlers` stack is
        // unused.  When empty (non-optimized bytecode) the VM falls back to the
        // dynamic stack.
        exc_table: &[u32],
        raise_pc: usize,
        // The register-resident current source line of the catching frame.
        // Used to populate the outermost traceback node's `tb_lineno` /
        // `tb_frame.f_lineno` when an exception is caught here (#2170).
        catch_lineno: u32,
    ) -> std::result::Result<usize, PyError> {
        let h = if exc_table.is_empty() {
            // Fallback: dynamic SetupExcept/PopExcept handler stack.
            match exc_handlers.pop() {
                Some(h) => h,
                None => return Err(self.escape_with_implicit_context(e)),
            }
        } else {
            // Zero-cost table lookup.  `EXC_NO_HANDLER` ⇒ no handler covers this
            // pc ⇒ the exception propagates to the caller frame.
            match exc_table.get(raise_pc).copied() {
                Some(t) if t != crate::bytecode::EXC_NO_HANDLER => t as usize,
                _ => return Err(self.escape_with_implicit_context(e)),
            }
        };
        let exc_val = self.materialize_pyerror(e)?;
        self.attach_implicit_context(&exc_val);
        let is_bare_reraise = std::mem::take(&mut self.reraise_is_bare);
        self.record_caught_exception_traceback(&exc_val, catch_lineno, is_bare_reraise);
        // Issue #2583: now that this exception is caught and its `__traceback__`
        // snapshot has been cloned out of the captured-frame thread-local, clear
        // the snapshot so a *new* exception raised inside this handler body
        // starts capturing from an empty frame list.  Without this reset the
        // second exception's unwind frames prepend onto the first exception's
        // stale frames, merging both tracebacks into one (the bug).  The first
        // exception's frames are already preserved in its deferred
        // `__traceback__`, so nothing is lost.  Explicit re-raises do their own
        // snapshot bookkeeping (`reset_captured_frames_if_reraise`) at the raise
        // site and are unaffected.
        pyrust_core::reset_captured_error_frames();
        // Save the current active_exception BEFORE the dedup-pop below.
        // This is the value that was active when the new exception was raised,
        // i.e. the previous handler's exception (or None if no outer handler).
        // EndExcept and RaiseReRaise pop this to restore the outer handler's
        // exception when the inner except block exits.
        let prev_active = self.active_exception.clone();
        // When control is inside a PushExcContext-bracketed finally block
        // (push_exc_ctx_depth > 0), the top of handled_exc_stack is the
        // to-be-raised exception installed by PushExcContext — not an
        // ordinary handler-dispatch entry.  Skipping the duplicate-pop
        // here preserves that entry so that PopExcContext can remove it
        // cleanly after the finally block completes, and so that
        // attach_implicit_context for the outer RaiseValue sees the correct
        // context (ValueError, not None) after the finally returns.
        if self.push_exc_ctx_depth == 0
            && self.handled_exc_stack.len() > exc_ctx_frame_base
            && let Some(top) = self.handled_exc_stack.last()
            && let Some(active) = self.active_exception.as_ref()
            && Self::values_are_same_exception(top, active)
        {
            self.handled_exc_stack.pop();
        }
        self.handled_exc_stack.push(exc_val.clone());
        // Record what was active before this except block started so that
        // EndExcept / RaiseReRaise can restore it without relying on
        // handled_exc_stack.last() (which may have been disturbed by the
        // dedup-pop above).
        self.exc_saved_active.push(prev_active);
        self.active_exception = Some(exc_val);
        Ok(h)
    }
}
