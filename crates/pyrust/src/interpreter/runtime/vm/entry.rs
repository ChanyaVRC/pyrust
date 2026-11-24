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
        self.run_bytecode_with_cells(code, regs, CellSlice::empty())
    }

    pub(super) fn run_bytecode_with_cells(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: RegSlice,
        cells: CellSlice,
    ) -> Result<Value> {
        let iters_init = if code.num_iters == 0 {
            ItersBuf::new()
        } else {
            smallvec::smallvec![None; code.num_iters as usize]
        };
        match self.run_bytecode_inner(
            code,
            regs,
            cells,
            iters_init,
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

    /// Push a `VmFrameView` for an executing generator/coroutine frame.
    fn push_generator_frame_view(
        &mut self,
        frame: &mut GeneratorFrame,
    ) -> (std::ptr::NonNull<Value>, usize, CellSlice) {
        let regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(frame.regs.as_mut_ptr()) };
        let regs_len = frame.regs.len();
        let cells_slice = CellSlice::from_slice(&frame.cells);
        self.vm_frame_views.push(VmFrameView::generator_frame(
            regs_ptr,
            regs_len,
            cells_slice,
            std::ptr::NonNull::from(&*frame),
        ));
        (regs_ptr, regs_len, cells_slice)
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

        // CPython (`gen_send_ex2`, FRAME_CREATED): injecting an exception into
        // a just-started (never-resumed) generator, coroutine, or async
        // generator closes it immediately and raises the thrown exception
        // verbatim without executing the body or applying PEP 479 wrapping
        // (issue #2983).
        if frame.pc == 0
            && let Some(exc) = inject_exc
        {
            frame.done = true;
            return Err(exc);
        }

        // PEP 380 throw forwarding: if we are suspended at a YieldFrom instruction
        // and an exception is being injected (generator.throw() / generator.close()),
        // forward the exception to the sub-iterator rather than injecting it into our
        // own body.  This matches CPython's implementation of the PEP 380 algorithm.
        //
        // Push this delegating generator's `VmFrameView` around the forwarding
        // call so `sys._getframe()` / `f_back` walks inside a sub-generator
        // handler observe every delegator frame on the stack (issue #3029).
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
            let previous_env = std::mem::replace(&mut self.env, Rc::clone(&frame.function.env));
            self.push_generator_frame_view(frame);
            let forward_result = self.yield_from_throw_forward(&iter_val, exc.clone());
            self.sync_top_generator_frame_cache_if_present();
            self.vm_frame_views.pop();
            self.env = previous_env;
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
        let previous_env = std::mem::replace(&mut self.env, Rc::clone(&frame.function.env));

        // PEP 3134: hand the generator's persisted exception state into
        // `run_bytecode_inner`, which will push it onto
        // `handled_exc_stack` AFTER capturing the caller's base depth.
        // The matching split-off on a subsequent yield, plus the
        // wrapper's truncate-on-exit, then keep the caller's view of
        // `handled_exc_stack` untouched throughout.
        let (gen_exc_handlers, gen_handled, gen_active, gen_exc_saved_active) =
            frame.take_resume_exc_state();

        // Issue #483/#486/#547: publish this generator's `VmFrameView` before
        // constructing `RegSlice`.
        let (regs_ptr, regs_len, cells_slice) = self.push_generator_frame_view(frame);
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
            cells_slice,
            std::mem::take(&mut frame.iters),
            gen_exc_handlers,
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
            self.record_escaped_vm_frame_traceback(
                &frame.code,
                &frame.function,
                Some(&frame.fn_name),
            );
        } else {
            self.sync_top_generator_frame_cache_if_present();
        }
        self.vm_frame_views.pop();

        // Restore env.
        self.env = previous_env;

        match result {
            Ok(FrameOutcome::Yielded { value, saved }) => {
                // Generator suspended: restore frame state from the outcome.
                let saved = *saved;
                frame.iters = saved.iters;
                frame.pc = saved.pc;
                frame.store_yield_exc_state(
                    saved.exc_handlers,
                    saved.handled_exc_slice,
                    saved.active_exception,
                    saved.exc_saved_active_slice,
                );
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
                if frame.has_frame_cache() {
                    self.link_finished_generator_frame_back(frame);
                }
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
                Err(self.pep479_wrap_stop_iteration(e))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_inner(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: RegSlice,
        cells: CellSlice,
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
        let tramp_arena_entry_len = self.tramp_arena.len();
        let tramp_stack_entry_len = self.tramp_stack.len();
        let gen_drive_entry_len = self.gen_drive_stack.len();
        // Save and reset push_exc_ctx_marks: PushExcContext/PopExcContext are
        // always emitted as matched pairs within a single function body, but if
        // the block raises out of the frame, PopExcContext is never executed.
        // Scoping the marks vector to this frame keeps outer frames intact.
        let saved_push_exc_ctx_marks = std::mem::take(&mut self.cold.push_exc_ctx_marks);

        let result = self.run_bytecode_inner_impl(
            code,
            regs,
            cells,
            iters_init,
            exc_handlers_init,
            start_pc,
            inject_exc,
            gen_handled_slice,
            gen_active_exception,
            gen_exc_saved_active,
        );

        debug_assert_eq!(
            self.tramp_stack.len(),
            tramp_stack_entry_len,
            "tramp_stack imbalanced across run_bytecode_inner_impl"
        );
        debug_assert_eq!(
            self.gen_drive_stack.len(),
            gen_drive_entry_len,
            "gen_drive_stack imbalanced across run_bytecode_inner_impl"
        );
        debug_assert_eq!(
            self.tramp_arena.len(),
            tramp_arena_entry_len,
            "tramp_arena imbalanced across run_bytecode_inner_impl"
        );
        if self.tramp_arena.len() > tramp_arena_entry_len {
            self.tramp_arena.truncate(tramp_arena_entry_len);
        }

        // For `Yielded`, the `Yield` opcode already stripped the generator's
        // exc-stack entries and cleared `active_exception` before returning, so
        // `handled_exc_stack` is already at `exc_ctx_entry_depth` on that path.
        // `truncate` is idempotent so it's safe to call in every branch.
        self.handled_exc_stack.truncate(exc_ctx_entry_depth);
        self.exc_saved_active.truncate(exc_saved_active_entry_depth);
        self.active_exception = saved_active;
        self.cold.push_exc_ctx_marks = saved_push_exc_ctx_marks;
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
    /// `#[inline(never)]` keeps this setup out of the `ForIter` stack frame
    /// without marking it `#[cold]` (which pushed hot generator-drive loops
    /// into `.text.unlikely`, #2906).
    #[inline(never)]
    fn vm_enter_gen_drive(
        &mut self,
        state_rc: Rc<GeneratorCell>,
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
        let gen_cells_slice = CellSlice::from_slice(&gframe.cells);
        // Publish the generator's frame view (mirrors `resume_generator_with_exc`).
        // `gframe` is a `Box<GeneratorFrame>` whose heap allocation is stable
        // across the move into `GenDriveFrame::gframe` below (only the box
        // pointer moves), so the pointer remains valid while this view is on
        // the stack (issues #2445/#2471/#3159).
        self.vm_frame_views.push(VmFrameView::generator_frame(
            gen_regs_ptr,
            gen_regs_len,
            gen_cells_slice,
            std::ptr::NonNull::from(&*gframe),
        ));
        // SAFETY: `gframe` lives inside `GenDriveFrame::gframe` on
        // `gen_drive_stack` for the entire drive (until `Yield`, `YieldFrom`,
        // return, or unwind restores `saved_code_ptr`), keeping `gframe.code`
        // alive without cloning and dropping its `Rc` on every drive cycle.
        let gen_code_ptr: *const crate::bytecode::FnCode = Rc::as_ptr(&gframe.code);
        let gen_num_locals = gframe.code.num_locals;
        let gen_pc = gframe.pc;
        let gen_iters = std::mem::take(&mut gframe.iters);
        let gen_exc_handlers = gframe.take_exc_handlers();
        let gen_env = Rc::clone(&gframe.function.env);
        let gen_iters_len = gen_iters.len();
        let gen_iter_cache = if gen_iters_len == 0 {
            IterCacheBuf::new()
        } else {
            smallvec::smallvec![None; gen_iters_len]
        };
        let tramp_floor = self.tramp_stack.len();
        self.gen_drive_stack.push(GenDriveFrame {
            _depth_guard: CallDepthGuard::enter(),
            state_rc,
            gframe,
            saved_regs: std::mem::replace(st.regs, unsafe {
                RegSlice::from_raw(gen_regs_ptr.as_ptr(), gen_regs_len)
            }),
            saved_cells: std::mem::replace(st.cells, gen_cells_slice),
            saved_pc: *st.pc,
            exit_pc,
            dst,
            saved_base: *st.tramp_active_base,
            saved_code_ptr: *st.code_ptr,
            saved_num_locals: *st.num_locals,
            saved_env: std::mem::replace(&mut self.env, gen_env),
            saved_iters: std::mem::replace(st.iters, gen_iters),
            saved_iter_cache: std::mem::replace(st.iter_next_cache, gen_iter_cache),
            saved_exc_handlers: std::mem::replace(st.exc_handlers, gen_exc_handlers),
            tramp_floor,
        });
        // Rebind the dispatch state to the generator.  `regs` / `iters` /
        // `iter_next_cache` / `exc_handlers` were swapped in place above.
        *st.code_ptr = gen_code_ptr;
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
        let mut line = unsafe { &**st.code_ptr }.line_at((*st.pc).wrapping_sub(1));
        // PEP 657 caret anchor of the raising instruction (#2411).  The VM
        // published it (before handler dispatch) for the *innermost* frame —
        // the one that actually raised — which is the first frame popped here.
        // Outer trampolined frames raised at their own call sites whose anchors
        // we don't track on the trampoline, so only the innermost frame claims
        // this span; the rest stay caret-free (a missing caret beats a wrong one).
        let mut innermost_col = pyrust_core::get_current_vm_col_span();
        loop {
            let floor = if self.gen_drive_stack.len() > st.gen_base {
                self.gen_drive_stack.last().unwrap().tramp_floor
            } else {
                st.tramp_base
            };
            while self.tramp_stack.len() > floor {
                let saved = self.tramp_stack.pop().unwrap();
                if let Some(key) = saved.memo_key.as_deref() {
                    self.cancel_memoized_call(key);
                }
                // PEP 657 stage 2 (#2443): the *caller* of the frame recorded
                // below raised at its call instruction, whose anchor is
                // `col_at(saved_pc - 1)` in the caller's code object
                // (`saved_code_ptr`).  Resolve both its caret span and source
                // line lazily here (#3157).  SAFETY: `saved_code_ptr` points at
                // the caller's `FnCode`, kept alive by the caller's
                // `VmFrameView` / `GenDriveFrame` (or the still-live bottom
                // frame) until this frame is restored.
                let (caller_call_line, caller_call_col) = {
                    let caller_code: &crate::bytecode::FnCode = unsafe { &*saved.saved_code_ptr };
                    let call_pc = saved.saved_pc.wrapping_sub(1);
                    (caller_code.line_at(call_pc), caller_code.col_at(call_pc))
                };
                let tb_lineno = if line == 0 { None } else { Some(line) };
                let top_is_inlined_comp = self
                    .vm_frame_views
                    .last()
                    .and_then(|v| v.function())
                    .and_then(|f| f.precompiled_code.as_ref())
                    .and_then(|rc| Rc::clone(rc).downcast::<crate::bytecode::FnCode>().ok())
                    .is_some_and(|c| c.is_inlined_comp);
                let (tb_locals, tb_frame) = if top_is_inlined_comp {
                    (None, None)
                } else {
                    self.capture_unwinding_traceback_frame(tb_lineno)
                };
                let view = self.detach_vm_frame_view();
                self.tramp_arena.truncate(*st.tramp_active_base);
                *st.tramp_active_base = saved.saved_base;
                *st.cells = saved.saved_cells;
                if let Some(saved_env) = saved.saved_env {
                    self.env = saved_env;
                }
                self.finish_pop_vm_frame_view();
                let mut callee_is_inlined_comp = false;
                if let Some(view) = view
                    && let Some(func) = view.into_function()
                {
                    let callee_code = func
                        .precompiled_code
                        .as_ref()
                        .and_then(|rc| Rc::clone(rc).downcast::<crate::bytecode::FnCode>().ok());
                    if callee_code.as_ref().is_some_and(|c| c.is_inlined_comp) {
                        // PEP 709 (#2993): list/set/dict comprehensions are
                        // inlined into the enclosing frame — do not emit a
                        // `<listcomp>`/`<setcomp>`/`<dictcomp>` traceback frame
                        // and keep the inner PEP 657 caret anchor.
                        callee_is_inlined_comp = true;
                    } else {
                        let frame_col = innermost_col;
                        // Callee's own code-object filename (#2438): a trampolined
                        // call into an imported module's function reports that
                        // module's source file, not the running script's path.
                        let file = callee_code
                            .map(|c| c.filename.clone())
                            .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
                        pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                            filename: file,
                            lineno: tb_lineno,
                            source_line: None,
                            funcname: std::sync::Arc::from(&func.name[..]),
                            globals: Some(pyrust_core::FrameGlobals::for_environment(&func.env)),
                            col_span: frame_col,
                            locals: tb_locals,
                            frame: tb_frame,
                        });
                    }
                }
                if callee_is_inlined_comp {
                    innermost_col = innermost_col.or(caller_call_col);
                    if line == 0 {
                        line = caller_call_line;
                    }
                } else {
                    innermost_col = caller_call_col;
                    line = caller_call_line;
                }
            }
            if self.gen_drive_stack.len() <= st.gen_base {
                break;
            }
            let mut gd = self.gen_drive_stack.pop().unwrap();
            // The error escaped the generator body: finalize it.
            gd.gframe.done = true;
            let tb_lineno = if line == 0 { None } else { Some(line) };
            let (tb_locals, tb_frame) = self.capture_unwinding_traceback_frame(tb_lineno);
            if self.vm_frame_views.pop().is_some() {
                // Generator's own code-object filename (#2438): a generator from
                // an imported module reports that module's source file.
                let file = gd.gframe.code.filename.clone();
                pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                    filename: file,
                    lineno: tb_lineno,
                    source_line: None,
                    funcname: gd.gframe.fn_name.clone(),
                    globals: Some(pyrust_core::FrameGlobals::for_environment(
                        &gd.gframe.function.env,
                    )),
                    // PEP 657 (#2904): `innermost_col` now holds the anchor of the
                    // instruction that propagated the error inside the generator
                    // body — either the raising instruction itself (body raised
                    // directly, e.g. `yield 1/0`) or the body's call site carried
                    // forward by the trampoline loop above (`yield f()`).  The
                    // gen-drive frame used to hardcode `None` and stayed caret-free.
                    col_span: innermost_col,
                    locals: tb_locals,
                    frame: tb_frame,
                });
            }
            // Restore the consumer frame.
            let foriter_pc = gd.saved_pc.wrapping_sub(1);
            *st.regs = gd.saved_regs;
            *st.cells = gd.saved_cells;
            *st.code_ptr = gd.saved_code_ptr;
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
            err = self.pep479_wrap_stop_iteration(err);
            // Re-raise at the consumer's `ForIter` instruction.
            let ccode: &crate::bytecode::FnCode = unsafe { &**st.code_ptr };
            line = ccode.line_at(foriter_pc);
            // PEP 657: from here on the error belongs to the *consumer*, raised at
            // its `ForIter` — so that instruction's anchor is the consumer's caret.
            // Publish it *before* dispatching, because a handler that catches here
            // snapshots `get_current_vm_col_span()` into the deferred traceback's
            // `catch_col_span` (`introspection::tracebacks`) while dispatching; the
            // generator body's own span is still standing otherwise and gets
            // painted onto the consumer's `for _ in g():` line when the caught
            // exception is later re-raised or chained (#2904 review).  It is also
            // the innermost caret for the frames unwound on the next outer-loop
            // iteration when the error escapes the consumer too (#2443).  `ForIter`
            // carries no span of its own, so this normally clears the anchor —
            // matching CPython, which leaves the `for` frame caret-free.
            innermost_col = ccode.col_at(foriter_pc);
            pyrust_core::set_current_vm_col_span(innermost_col);
            match self.handle_vm_error(
                err,
                st.exc_handlers,
                st.exc_ctx_frame_base,
                &ccode.exc_table,
                foriter_pc,
                line,
            ) {
                Ok(h) => {
                    return UnwindOutcome::Resume(h);
                }
                Err(e2) => {
                    err = e2;
                    continue;
                }
            }
        }
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
        // When control is inside a PushExcContext-bracketed block (finally or
        // `except*` handler), do not dedup-pop the pushed context entry or the
        // outer handler entry immediately beneath it; PopExcContext restores
        // the stack cleanly once the inner handler finishes.
        let protected_by_push_ctx = self
            .cold
            .push_exc_ctx_marks
            .last()
            .is_some_and(|&(saved_len, _)| self.handled_exc_stack.len() <= saved_len + 1);
        if !protected_by_push_ctx
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
