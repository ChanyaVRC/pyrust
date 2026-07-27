impl Interpreter {
    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_inner_impl(
        &mut self,
        code: &crate::bytecode::FnCode,
        mut regs: RegSlice,
        iters_init: ItersBuf,
        exc_handlers_init: ExcHandlersBuf,
        start_pc: usize,
        inject_exc: Option<PyError>,
        // Generator-only: persisted slice + active to push AFTER we've
        // captured `exc_ctx_frame_base`, so they sit strictly above the
        // caller's stack entries and are owned by this frame.
        gen_handled_slice: HandledExcBuf,
        gen_active_exception: Option<Value>,
        // Generator-only: parallel save-stack for active_exception across
        // nested except blocks, split off at yield and re-extended on resume.
        gen_exc_saved_active: Vec<Option<Value>>,
    ) -> Result<FrameOutcome> {
        use crate::bytecode::Insn;

        // Generalized-trampoline foundation (#2234): the active frame's `code`
        // can change when a Python→Python call is trampolined, so it is held as
        // a raw pointer re-derived at the loop top.  `active_code_rc` keeps a
        // trampolined callee's `FnCode` alive (`None` ⇒ the bottom frame uses
        // the borrowed `code` param).  `num_locals` is likewise rebindable.
        let mut code_ptr: *const crate::bytecode::FnCode = code;
        let mut active_code_rc: Option<Rc<crate::bytecode::FnCode>> = None;
        let mut num_locals = code.num_locals;

        let mut iters: ItersBuf = iters_init;
        let mut iter_next_cache: IterCacheBuf = smallvec::smallvec![None; iters.len()];
        let mut exc_handlers: ExcHandlersBuf = exc_handlers_init;
        let mut pc: usize = start_pc;
        // Register-resident current-line tracker.  Updated cheaply per
        // instruction from `lineno_table` (a non-zero entry means "this
        // instruction starts a new source line"; `0` means "same line as the
        // previous instruction").  Flushed to the thread-local `CURRENT_VM_LINE`
        // (via `set_current_vm_line`) only on the error-propagation paths, so
        // the common hot path pays no TLS / RefCell cost (issue #348).
        let mut cur_line: u32 = pyrust_core::get_current_vm_line();
        let mut pending_inject: Option<PyError> = inject_exc;

        // Depth of `handled_exc_stack` belonging to *caller* frames at the
        // time this VM frame started executing.  Used by `vm_try!` to bound
        // its "propagating out of a handler body" pop so it never reaches
        // into the caller's entries.
        let exc_ctx_frame_base: usize = self.handled_exc_stack.len();
        // Parallel base for `exc_saved_active`.  Bounded pops/split-off in
        // EndExcept, RaiseReRaise, and Yield mirror the handling above.
        let exc_saved_active_frame_base: usize = self.exc_saved_active.len();

        // PEP 695: the only thing that mutates `self.env` *within* a single VM
        // frame is the type-parameter scope (`PushTypeParamEnv`/`PopTypeParamEnv`)
        // emitted around a generic def/class header — class bodies run in their
        // own frame.  When an exception raised inside that header (e.g. an
        // annotation referencing an undefined name) is caught by a `try` in this
        // same frame, the matching `PopTypeParamEnv` never executes, so restore
        // the frame's base env on the in-frame catch path.  Cheap to capture (a
        // single `Rc::clone`) and only compared on the cold catch path.
        let frame_base_env: EnvRef = Rc::clone(&self.env);

        // PEP 3134: re-install the generator's persisted slice of
        // `handled_exc_stack` and its `active_exception` AFTER fixing the
        // frame base.  These entries are now owned by this frame, and the
        // next `Yield` opcode's `split_off(exc_ctx_frame_base)` will
        // collect them again (plus anything new) into
        // `frame.handled_exc_slice`.  No-op for fresh, non-generator calls.
        if !gen_handled_slice.is_empty() {
            self.handled_exc_stack.extend(gen_handled_slice);
        }
        if !gen_exc_saved_active.is_empty() {
            self.exc_saved_active.extend(gen_exc_saved_active);
        }
        if gen_active_exception.is_some() {
            self.active_exception = gen_active_exception;
        }

        // ── Self-recursion trampoline (#2234) ───────────────────────────────
        // A direct self-recursive call (callee == the function currently
        // executing) keeps `code` / env / `num_locals` invariant, so only
        // `regs` and `pc` need swapping.  Instead of recursing through
        // `call_function_expanded` -> `call_user_function_expanded` ->
        // `run_bytecode` (native call machinery, ~156ns/call), push the
        // caller's frame onto `tramp_stack` and loop back to pc=0 in the same
        // dispatch loop — ~2x on recursive workloads (e.g. fib).  Gated (see the
        // `CallMemo` arm) to the simple shape: a handler-free, non-generator,
        // loop-free, non-variadic self-call with no cell/global/nonlocal names,
        // so env is unchanged and an unhandled raise propagates straight out
        // with no cross-frame unwinding.  Every trampolined callee's register
        // file is a contiguous slice of a single `tramp_arena` value stack
        // (CPython's data-stack model): a frame push is a bump (`resize`) and a
        // pop is a `truncate`, with no per-frame heap allocation and good cache
        // locality.  The arena is reserved up front to its recursion-limit
        // bound so it never reallocates (which would dangle the active/saved
        // `RegSlice`s); a call that would exceed the reservation falls back to
        // the native path.  `TrampFrame` (module-scoped) records what the caller
        // needs restored on the callee's `Return`.
        let mut tramp_stack: Vec<TrampFrame> = Vec::new();
        let mut tramp_arena: Vec<Value> = Vec::new();
        // Base offset of the active frame's registers in `tramp_arena`.
        // `usize::MAX` ⇒ the active frame is the bottom (param `regs`).
        let mut tramp_active_base: usize = usize::MAX;

        // ── Generator trampoline (#2253) ───────────────────────────────────
        // When a `ForIter` consumer drives a plain, handler-free generator, the
        // generator's `GeneratorFrame` is switched in as the active frame
        // *within this dispatch loop* (run to its `Yield`, switch back with the
        // value) instead of re-entering `run_bytecode_inner` per element — the
        // generator counterpart of the call trampoline above.  Each
        // `GenDriveFrame` checks the generator's frame *out* of its `Value`
        // (a placeholder `Box` sits in the cell while it is driving) so the
        // frame's `regs` buffer has a stable heap address for a `RegSlice`, and
        // records everything needed to restore the consumer on yield, on the
        // generator's return (→ the `ForIter` loop-exit), or on an escaping
        // error.  `tramp_floor` is the `tramp_stack` depth at switch-in: an
        // error unwinds the call frames above it, then hands the error to this
        // generator's consumer.  The active frame is *this* gen-driven
        // generator iff `gen_drive_stack.last().tramp_floor == tramp_stack.len()`
        // (a deeper `tramp_stack` means a call the generator made is active).
        // `GenDriveFrame` is module-scoped (shared with `vm_unwind_error`).
        let mut gen_drive_stack: Vec<GenDriveFrame> = Vec::new();

        'vm: loop {
            // Re-derive the active frame's `code` from `code_ptr` (updated on a
            // trampolined frame switch).  Zero-cost: a raw-pointer reborrow.
            let code: &crate::bytecode::FnCode = unsafe { &*code_ptr };
            // `true` when the active frame is a generator currently being driven
            // by a `ForIter` consumer (#2253) — i.e. the most recent frame switch
            // was a gen-drive switch, with no call the generator made still active
            // on top of it.  Cheap: a `Vec::is_empty` short-circuit on every
            // non-generator workload (`gen_drive_stack` is empty there).
            macro_rules! active_is_gen_drive {
                () => {
                    gen_drive_stack
                        .last()
                        .is_some_and(|__g| __g.tramp_floor == tramp_stack.len())
                };
            }
            // The driven generator returned (`return` / fell off the end): mark it
            // exhausted, switch back to its consumer, and jump the consumer to the
            // `ForIter` loop-exit (a generator return is `StopIteration` to a
            // `for`).  `$v` is the return value (observable only via `yield from`'s
            // `StopIteration.value`; discarded by a plain `for`, but stashed for a
            // subsequent `next()` which raises immediately since `done`).
            macro_rules! gen_drive_return {
                ($v:expr) => {{
                    let mut __gd = gen_drive_stack.pop().unwrap();
                    __gd.gframe.done = true;
                    __gd.gframe.last_return_value = Some($v);
                    self.vm_frame_views.pop();
                    regs = __gd.saved_regs;
                    pc = __gd.exit_pc;
                    cur_line = __gd.saved_cur_line;
                    code_ptr = __gd.saved_code_ptr;
                    active_code_rc = __gd.saved_active_code_rc;
                    num_locals = __gd.saved_num_locals;
                    self.env = __gd.saved_env;
                    iters = __gd.saved_iters;
                    iter_next_cache = __gd.saved_iter_cache;
                    exc_handlers = __gd.saved_exc_handlers;
                    tramp_active_base = __gd.saved_base;
                    let __boxed: Box<dyn std::any::Any> = __gd.gframe;
                    *__gd.state_rc.borrow_mut() = __boxed;
                    continue 'vm;
                }};
            }
            // Return `$v` from the current frame: if the active frame is a driven
            // generator (#2253), finalize it (→ the consumer's loop-exit); else if a
            // call-trampoline frame is active (#2234), pop it (truncate the arena,
            // restore the caller) and resume the caller with `$v` in its destination
            // register; otherwise return out of the VM.  Used at every `Returned`
            // exit so a trampolined frame is never leaked.
            macro_rules! tramp_return {
                ($v:expr) => {{
                    let __v = $v;
                    if active_is_gen_drive!() {
                        gen_drive_return!(__v);
                    }
                    if let Some(__saved) = tramp_stack.pop() {
                        self.vm_frame_views.pop();
                        let __memo_key = __saved.memo_key;
                        tramp_arena.truncate(tramp_active_base);
                        tramp_active_base = __saved.saved_base;
                        regs = __saved.saved_regs;
                        pc = __saved.saved_pc;
                        cur_line = __saved.saved_cur_line;
                        // Restore the caller's code / locals / env (a no-op for
                        // a self-recursive call; a real switch for a general
                        // Python→Python call).
                        code_ptr = __saved.saved_code_ptr;
                        active_code_rc = __saved.saved_active_code_rc;
                        num_locals = __saved.saved_num_locals;
                        self.env = __saved.saved_env;
                        if let Some(__key) = __memo_key {
                            self.finish_memoized_call(__key, &__v);
                        }
                        regs[__saved.dst as usize] = __v;
                        continue 'vm;
                    }
                    return Ok(FrameOutcome::Returned(__v));
                }};
            }
            // Dispatch errors through the active exception handler stack.
            // Defined inside the loop so `continue 'vm` resolves to this loop.
            macro_rules! vm_try {
                ($expr:expr) => {
                    match $expr {
                        Ok(v) => v,
                        // `pc` was already advanced past the current instruction
                        // (the `pc += 1` below the fetch), so the raising
                        // instruction's index — the exception-table key — is `pc - 1`.
                        Err(e) => {
                            // Publish the raising instruction's PEP 657 caret anchor
                            // (#2411) BEFORE dispatching to the handler, so a deferred
                            // traceback built for a *caught* exception (e.g. `1/0`
                            // caught and later re-raised / chained) records the anchor
                            // for the frame that actually raised — the escape-path
                            // publish below only fires when the error leaves the frame.
                            pyrust_core::set_current_vm_col_span(
                                code.col_table
                                    .get(pc.wrapping_sub(1))
                                    .copied()
                                    .and_then(|s| if s == (0, 0, 0, 0) { None } else { Some(s) }),
                            );
                            match self.handle_vm_error(
                                e,
                                &mut exc_handlers,
                                exc_ctx_frame_base,
                                &code.exc_table,
                                pc.wrapping_sub(1),
                                cur_line,
                            ) {
                                Ok(h) => {
                                    // PEP 695: if the exception was raised inside a
                                    // type-parameter scope (generic def/class header), the
                                    // matching `PopTypeParamEnv` was skipped — restore the
                                    // frame's base env before running the handler.
                                    if !Rc::ptr_eq(&self.env, &frame_base_env) {
                                        self.env = Rc::clone(&frame_base_env);
                                        bump_global_struct_version(self);
                                    }
                                    pc = h;
                                    continue 'vm;
                                }
                                Err(e) => {
                                    // Error escapes this frame.  The raising instruction's
                                    // PEP 657 caret anchor (#2426/#2411) and any inlined
                                    // callee frame (#2569) were already published above
                                    // (before handler dispatch); last writer wins, so the
                                    // outermost (module) frame's anchor is what
                                    // `get_current_vm_col_span` returns.
                                    //
                                    // Unwind any active trampolined frames (record their
                                    // traceback, pop their views, restore each caller's
                                    // env) and publish the line tracker (issues #348,
                                    // #2234).
                                    tramp_unwind_err!(e);
                                }
                            }
                        }
                    }
                };
            }
            // Unwind active trampolined frames on the error path.  Interleaves the
            // call trampoline (#2234) and the generator trampoline (#2253): pop call
            // frames (record each in the traceback, pop its view, restore the
            // caller's env) down to the current generator's `tramp_floor`; if a
            // gen-drive frame is then at the boundary, the error belongs to *its*
            // consumer, so finalize the generator (mark done, record its traceback,
            // PEP 479-wrap an escaping `StopIteration`), restore the consumer, and
            // re-dispatch the error at the consumer's `ForIter` site through the
            // consumer's handler stack — which may catch it or continue unwinding.
            // A no-op fast path (both stacks empty) returns the error directly.
            macro_rules! tramp_unwind_err {
                ($e:expr) => {{
                    // Thin wrapper: hand the live frame state to the cold, out-of-line
                    // `vm_unwind_error` so this expansion stays tiny at every site.
                    let mut __st = UnwindState {
                        regs: &mut regs,
                        pc: &mut pc,
                        cur_line: &mut cur_line,
                        code_ptr: &mut code_ptr,
                        active_code_rc: &mut active_code_rc,
                        num_locals: &mut num_locals,
                        iters: &mut iters,
                        iter_next_cache: &mut iter_next_cache,
                        exc_handlers: &mut exc_handlers,
                        tramp_active_base: &mut tramp_active_base,
                        tramp_stack: &mut tramp_stack,
                        gen_drive_stack: &mut gen_drive_stack,
                        exc_ctx_frame_base,
                    };
                    match self.vm_unwind_error($e, &mut __st) {
                        UnwindOutcome::Resume(__h) => {
                            pc = __h;
                            continue 'vm;
                        }
                        UnwindOutcome::Escape(__err) => {
                            return Err(__err);
                        }
                    }
                }};
            }
            macro_rules! pool_get {
                ($pool:expr, $idx:expr, $tag:literal) => {
                    match ($pool).get($idx as usize) {
                        Some(v) => v,
                        None => {
                            vm_try!(Err(PyError::Runtime(format!(
                                "bytecode error: {} index {} out of range (pool size {})",
                                $tag,
                                $idx,
                                ($pool).len()
                            ))));
                            unreachable!()
                        }
                    }
                };
            }
            // Generalized trampoline (#2234): try to trampoline a Python→Python call
            // at register `$func_reg` with `$argc` positional args (callee already
            // read into `$func_val`), swapping the active frame to the callee and
            // looping instead of recursing through the native call machinery.
            // Subsumes the self-recursion case (callee == caller).  Falls through to
            // the native call path when the gate fails.  Gate keeps it correct
            // without cross-frame exception unwinding: the callee is a plain,
            // loop-free, handler-free, local-env-free function, and the caller's
            // call site is not inside a `try`, so an escaping raise propagates
            // straight out and the callee touches neither `iters` nor the exception
            // state.
            // Call trampoline (#2234, method form #2345): gate + frame-entry tail
            // shared by plain-call and bound-method trampolining via the internal
            // `@enter` rule.  `@enter` introduces `f` / `base` / `nparams` /
            // `num_regs` and then splices the caller-supplied binding statements
            // ($bind) in the *same* macro expansion, so those locals are visible to
            // the binding code (a separate `$bind:block` from another macro would be
            // hygienically isolated from them).  The gate keeps trampolining correct
            // without cross-frame unwinding: the callee is a plain, loop-free,
            // handler-free, local-env-free regular function, and the caller's call
            // site is not inside a `try`, so an escaping raise propagates straight
            // out and the callee touches neither `iters` nor the exception state.
            macro_rules! tramp_try {
            // Internal frame-entry rule.  `$dst` = result register; `$f` =
            // callee `&Rc<UserFunction>`; `$nsupplied` = positional values that
            // must exactly fill the params (receiver included for a method);
            // `$memo_key` = probed `CallMemo` miss key, if any;
            // `$bind` = statements that fill `tramp_arena[base + …]`.
            (@enter $lt:lifetime, $dst:expr, $f:expr, $nsupplied:expr,
                    $fb:ident, $base:ident, $np:ident, $memo_key:expr, $bind:block) => {{
                let $fb = $f;
                let f = $fb;
                if !matches!(f.kind, pyrust_core::UserFunctionKind::Regular)
                    || !f.global_names.is_empty()
                    || !f.nonlocal_names.is_empty()
                {
                    break $lt;
                }
                // Caller must not be inside a `try` at this call site (so an
                // escaping raise propagates straight out).  Covers both the
                // zero-cost `exc_table` model (optimized code) and the
                // dynamic `SetupExcept`/`PopExcept` stack (`exc_handlers`,
                // non-empty ⇒ a dynamic `try` is active).  Skipped entirely
                // for handler-free callers (the common case).
                if !exc_handlers.is_empty()
                    || (code.has_exc_handlers
                        && code
                            .exc_table
                            .get(pc - 1)
                            .copied()
                            .is_none_or(|t| t != crate::bytecode::EXC_NO_HANDLER))
                {
                    break $lt;
                }
                let $np = f.params.len();
                if ($nsupplied as usize) != $np
                    || f.params
                        .iter()
                        .any(|p| p.is_args || p.is_kwargs || p.is_keyword_only)
                {
                    break $lt;
                }
                let callee_code = match self.get_or_compile_bytecode(f) {
                    Some(c) => c,
                    None => break $lt,
                };
                if callee_code.is_generator
                    || callee_code.is_coroutine
                    || callee_code.num_iters != 0
                    || callee_code.has_exc_handlers
                    || !callee_code.cell_vars.is_empty()
                {
                    // A coroutine function (`async def`, issue #1039) must
                    // build a coroutine object instead of executing inline,
                    // even when its body has no `await` (so `is_generator`
                    // is false).
                    break $lt;
                }
                if call_depth() >= max_call_depth(self) {
                    let error = self.recursion_limit_error()?;
                    tramp_unwind_err!(error);
                }
                let num_regs = callee_code.num_regs as usize;
                let $base = tramp_arena.len();
                if tramp_arena.is_empty() {
                    // Reserve once, while no slices are live, so later
                    // `resize`s never reallocate (dangling the saved/active
                    // `RegSlice`s).  Deeper than this falls back to native.
                    tramp_arena.reserve(16 * 1024);
                }
                if $base + num_regs > tramp_arena.capacity() {
                    break $lt;
                }
                tramp_arena.resize($base + num_regs, Value::unset());
                $bind
                let callee_num_locals = callee_code.num_locals;
                let callee_env = Rc::clone(&f.env);
                let callee_code_ptr: *const crate::bytecode::FnCode =
                    Rc::as_ptr(&callee_code);
                // SAFETY: base+num_regs <= capacity (checked) ⇒ no realloc;
                // pointer valid until this frame's `truncate` on return.
                let new_ptr = unsafe { tramp_arena.as_mut_ptr().add($base) };
                // Claim only after every gate and register read succeeded. A
                // same-key recursive miss gets no second owner but still
                // enters the trampoline.
                let memo_key = ($memo_key).and_then(|key| self.claim_memoized_call(key));
                tramp_stack.push(TrampFrame {
                    _depth_guard: CallDepthGuard::enter(),
                    saved_regs: regs,
                    saved_pc: pc,
                    dst: $dst,
                    saved_base: tramp_active_base,
                    saved_cur_line: cur_line,
                    saved_code_ptr: code_ptr,
                    saved_active_code_rc: active_code_rc.take(),
                    saved_num_locals: num_locals,
                    saved_env: std::mem::replace(&mut self.env, callee_env),
                    memo_key,
                });
                // Publish a frame view so `locals()` / `sys._getframe()` /
                // tracebacks observe this trampolined frame like a
                // natively-called one.  Popped on its `Return`, or unwound by
                // `tramp_unwind_err!` on an escaping error.
                self.vm_frame_views.push(VmFrameView {
                    kind: FrameKind::Function,
                    // SAFETY: points into the arena slice just bound; stays
                    // valid (no realloc) until this frame's view is popped.
                    regs_ptr: unsafe { std::ptr::NonNull::new_unchecked(new_ptr) },
                    regs_len: num_regs,
                    local_index: Rc::clone(&f.local_index),
                    nonlocal_names: None,
                    env: None,
                    is_class_method: callee_code.is_class_method,
                    function: Some(Rc::clone(f)),
                    gen_frame: None,
                });
                tramp_active_base = $base;
                active_code_rc = Some(callee_code);
                code_ptr = callee_code_ptr;
                num_locals = callee_num_locals;
                regs = unsafe { RegSlice::from_raw(new_ptr, num_regs) };
                pc = 0;
                continue 'vm;
            }};
            // Plain-call implementation: callee at register `$func_reg`,
            // positional args read from `$func_reg + 1 ..`.
            (@plain $func_reg:expr, $argc:expr, $func_val:expr, $memo_key:expr) => {
                'trampoline: {
                    let ValueKind::UserFunction(f0) = $func_val.kind() else {
                        break 'trampoline;
                    };
                    tramp_try!(@enter 'trampoline, $func_reg, f0, $argc, f, base, nparams, $memo_key, {
                        for i in 0..nparams {
                            let v = vm_try!(vm_read(&regs, $func_reg + 1 + i as u32, num_locals));
                            if let pyrust_core::ParamBind::Reg(r) = f.param_binds[i] {
                                tramp_arena[base + r as usize] = v;
                            }
                        }
                        if let Some(slot) = f.self_bind {
                            tramp_arena[base + slot as usize] = $func_val.clone();
                        }
                    });
                }
            };
            ($func_reg:expr, $argc:expr, $func_val:expr) => {
                tramp_try!(@plain $func_reg, $argc, $func_val, Option::<MemoKey>::None);
            };
            (@memo $func_reg:expr, $argc:expr, $func_val:expr, $memo_key:expr) => {
                tramp_try!(@plain $func_reg, $argc, $func_val, Some($memo_key));
            };
        }

            // Bound-method trampoline (#2345): the unbound regular method `$f`
            // (`&Rc<UserFunction>`), its already-resolved `$recv` receiver, and
            // `$argc` positional args read from `$args_base ..`.  The receiver fills
            // parameter 0 (`self`); the `$argc` args fill parameters `1 ..= argc`.
            // Result is written to `$dst`.  A bound method is never a self-binding
            // closure, so there is no `self_bind` slot to fill (the receiver *is*
            // the first parameter).
            macro_rules! tramp_try_method {
            ($dst:expr, $f:expr, $recv:expr, $args_base:expr, $argc:expr) => {
                'trampoline: {
                    tramp_try!(@enter 'trampoline, $dst, $f, ($argc as usize) + 1, f, base, nparams, Option::<MemoKey>::None, {
                        let _ = nparams;
                        if let pyrust_core::ParamBind::Reg(r) = f.param_binds[0] {
                            tramp_arena[base + r as usize] = $recv;
                        }
                        for i in 0..$argc as u32 {
                            let v = vm_try!(vm_read(&regs, $args_base + i, num_locals));
                            if let pyrust_core::ParamBind::Reg(r) =
                                f.param_binds[1 + i as usize]
                            {
                                tramp_arena[base + r as usize] = v;
                            }
                        }
                    });
                }
            };
        }
            // Inject a pending exception (set by resume_generator_with_exc for
            // generator.throw()/close()) before dispatching the next
            // instruction.  Routes through the existing handler stack so that
            // try/except/finally inside the generator can observe the throw.
            //
            // The generator was suspended at a `Yield`, and `start_pc` (== this
            // `pc`) is the instruction *after* it.  The exception is raised *by
            // the yield expression*, so the exception-table key is the Yield's
            // pc, `pc - 1` — the resume pc itself may already be outside the try
            // region that encloses the yield.  For a non-generator entry with an
            // injected exception at `pc == 0` there is no enclosing handler, so
            // `wrapping_sub` is harmless (no table entry ⇒ propagate).
            if let Some(e) = pending_inject.take() {
                let inject_pc = pc.wrapping_sub(1);
                match self.handle_vm_error(
                    e,
                    &mut exc_handlers,
                    exc_ctx_frame_base,
                    &code.exc_table,
                    inject_pc,
                    cur_line,
                ) {
                    Ok(h) => {
                        pc = h;
                        continue 'vm;
                    }
                    Err(e) => {
                        pyrust_core::set_current_vm_line(cur_line);
                        return Err(e);
                    }
                }
            }
            let Some(insn) = code.insns.get(pc) else {
                if pc == code.insns.len() {
                    // Implicit fall-off-the-end return (no trailing ReturnNone);
                    // pop any active trampoline frame like an explicit return.
                    tramp_return!(Value::none());
                }
                pyrust_core::set_current_vm_line(cur_line);
                return Err(PyError::Runtime(format!(
                    "internal error: PC {} out of bounds (insns len {})",
                    pc,
                    code.insns.len()
                )));
            };
            // Register-resident line tracker (issue #348): update from the
            // lineno table without touching the thread-local.  A non-zero entry
            // marks a new source line; `0` keeps the previous line.
            if let Some(&ln) = code.lineno_table.get(pc)
                && ln != 0
            {
                cur_line = ln;
            }
            pc += 1;

            macro_rules! jump_pc {
                ($offset:expr) => {{
                    let new_pc = pc as i64 + $offset as i64;
                    if new_pc < 0 || new_pc as usize > code.insns.len() {
                        pyrust_core::set_current_vm_line(cur_line);
                        return Err(PyError::Runtime(format!(
                            "internal error: jump to invalid PC {} (insns len {})",
                            new_pc,
                            code.insns.len()
                        )));
                    }
                    new_pc as usize
                }};
            }

            match insn {
                // ── Loads ────────────────────────────────────────────────
                Insn::LoadConst(dst, idx) => {
                    let cv = pool_get!(code.consts, *idx, "const");
                    regs[*dst as usize] = match cv.kind() {
                        ValueKind::Int(n) => Value::int(n),
                        // Use intern_string_value so that long strings (> INTERN_MAX_BYTES)
                        // are returned as a cheap clone of the const-pool Value rather
                        // than allocating a second copy via Value::string(s) (issue #845).
                        ValueKind::Str(s) => intern_string_value(s, cv),
                        _ => cv.clone(),
                    };
                }
                Insn::LoadGlobal(dst, name_idx) => {
                    regs[*dst as usize] = vm_try!(self.load_global_cached(code, *name_idx));
                }
                Insn::StoreGlobal(name_idx, src) => {
                    let name = pool_get!(code.names, *name_idx, "name");
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    self.assign_name(name, val);
                }
                Insn::LoadCell(dst, name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name");
                    regs[*dst as usize] = vm_try!(self.resolve_cell_value(code, name));
                }
                Insn::StoreCell(name_idx, src) => {
                    // Function-scope cell / nonlocal write (issue #2339).  Reuses
                    // `assign_name`, whose nonlocal arm walks to the enclosing
                    // owning env and whose cell arm writes the current env — the
                    // shared mutable slot siblings and suspended frames observe.
                    let name = pool_get!(code.names, *name_idx, "name");
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    self.assign_name(name, val);
                }
                Insn::LoadNone(dst) => {
                    regs[*dst as usize] = Value::none();
                }
                Insn::LoadNoneRange { start, count } => {
                    let s = *start as usize;
                    let e = s + *count as usize;
                    for idx in s..e {
                        regs[idx] = Value::none();
                    }
                }
                Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
                    let v = vm_try!(vm_read(&regs, *src, num_locals));
                    regs[*dst as usize] = v;
                }

                // ── Arithmetic / Logic ───────────────────────────────────
                Insn::BinOp(dst, lhs, op, rhs) => {
                    // Full body (int fast path + adaptive float/str inline
                    // cache) lives in fast_path.rs::exec_binop; `vm_try!` routes
                    // any eval_binary error through the handler stack exactly as
                    // the old inline arm did.
                    vm_try!(
                        self.exec_binop(&mut regs, code, pc, *dst, *lhs, *op, *rhs, num_locals)
                    );
                }
                Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                    vm_try!(
                        self.exec_binop_in_place(&mut regs, *dst, *lhs, *op, *rhs, num_locals,)
                    );
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
                Insn::MatchSeqExcluded(dst, subj) => {
                    // isinstance(subj, (str, bytes, dict, set, frozenset)) —
                    // the sequence-pattern type exclusion (issue #1789).  No
                    // global lookups, no tuple build, no isinstance call.
                    let subj_val = vm_try!(vm_read(&regs, *subj, num_locals));
                    let excluded = crate::interpreter::value_is_seq_excluded(&subj_val);
                    regs[*dst as usize] = Value::bool_(excluded);
                }

                Insn::MatchMapping(dst, subj) => {
                    // isinstance(subj, collections.abc.Mapping) — the
                    // mapping-pattern type gate (issue #1879).  In pyrust the
                    // only built-in mapping is dict (and subclasses).
                    let subj_val = vm_try!(vm_read(&regs, *subj, num_locals));
                    let is_mapping = crate::interpreter::value_is_mapping(&subj_val);
                    regs[*dst as usize] = Value::bool_(is_mapping);
                }

                // ── String concat (single allocation) ────────────────────
                Insn::Concat { dst, base, count } => {
                    regs[*dst as usize] =
                        vm_try!(self.eval_concat_fast(&regs, *base, *count, num_locals));
                }

                // ── Attribute / Index ────────────────────────────────────
                Insn::GetAttr(dst, obj, name_idx) => {
                    // Full body (InstanceAttr / ClassAttr inline-cache hit +
                    // slow-path get_attr / cache fill / invalidation) lives in
                    // fast_path.rs::exec_get_attr.
                    vm_try!(
                        self.exec_get_attr(&mut regs, code, pc, *dst, *obj, *name_idx, num_locals)
                    );
                }
                Insn::GetAttrForWith(dst, obj, name_idx, proto) => {
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    regs[*dst as usize] =
                        vm_try!(self.get_required_protocol_attr(&obj_val, name, *proto));
                }
                Insn::ImportFromAttr(dst, mod_reg, name_idx) => {
                    let mod_val = vm_try!(vm_read(&regs, *mod_reg, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    regs[*dst as usize] = vm_try!(self.import_from_attribute(&mod_val, name));
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    // Full body (SetInstanceAttr write-cache hit + slow-path
                    // assign_attr / cache fill / invalidation, #1998) lives in
                    // fast_path.rs::exec_set_attr.
                    vm_try!(
                        self.exec_set_attr(&mut regs, code, pc, *obj, *name_idx, *val, num_locals)
                    );
                }
                Insn::DeleteAttr(obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.delete_attr(obj_val, name));
                }
                Insn::SetTypeVarAttr(obj, name_idx, val) => {
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let val = vm_try!(vm_read(&regs, *val, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    initialize_typevar_attr(&obj_val, name, val);
                }
                Insn::GetItem(dst, obj, idx) => {
                    let r = self.exec_get_item(&regs, num_locals, *obj, *idx);
                    regs[*dst as usize] = vm_try!(r);
                }
                Insn::GetSlice(dst, obj, base) => {
                    let r = self.exec_get_slice(&regs, num_locals, *obj, *base);
                    regs[*dst as usize] = vm_try!(r);
                }
                Insn::SetItem(obj, idx, val) => {
                    vm_try!(self.exec_set_item(&mut regs, num_locals, *obj, *idx, *val));
                }
                Insn::DeleteItem(obj, idx) => {
                    vm_try!(self.exec_delete_item(&mut regs, num_locals, *obj, *idx));
                }
                Insn::DeleteName(name_idx) => {
                    vm_try!(self.exec_delete_name(code, &regs, *name_idx));
                }
                Insn::PushTypeParamEnv => {
                    self.push_type_parameter_scope();
                }
                Insn::PopTypeParamEnv => {
                    self.pop_type_parameter_scope();
                }
                Insn::DeleteLocal(reg, name_idx) => {
                    vm_try!(self.delete_local_binding(code, &mut regs, *reg, *name_idx));
                }
                Insn::SyncModuleGlobal(reg, name_idx) => {
                    vm_try!(self.sync_module_global_binding(code, &regs, *reg, *name_idx));
                }
                Insn::DeleteModuleGlobal(name_idx) => {
                    vm_try!(self.delete_module_global_binding(code, &regs, *name_idx));
                }

                // ── Control flow ─────────────────────────────────────────
                Insn::Jump(offset) => {
                    pc = jump_pc!(*offset);
                }
                Insn::JumpIfFalse(cond, offset) => {
                    if let Some(condition) = regs[*cond as usize]
                        .as_some()
                        .and_then(try_scalar_truthiness_fast)
                    {
                        if !condition {
                            pc = jump_pc!(*offset);
                        }
                        continue;
                    }
                    let cond_val = vm_try!(vm_read(&regs, *cond, num_locals));
                    if !vm_try!(self.truthy_value(&cond_val)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::JumpIfTrue(cond, offset) => {
                    if let Some(condition) = regs[*cond as usize]
                        .as_some()
                        .and_then(try_scalar_truthiness_fast)
                    {
                        if condition {
                            pc = jump_pc!(*offset);
                        }
                        continue;
                    }
                    let cond_val = vm_try!(vm_read(&regs, *cond, num_locals));
                    if vm_try!(self.truthy_value(&cond_val)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::CmpJumpIfFalse(lhs, op, rhs, offset) => {
                    if let Some(cond) =
                        try_integer_compare_fast(&regs[*lhs as usize], &regs[*rhs as usize], *op)
                    {
                        if !cond {
                            pc = jump_pc!(*offset);
                        }
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(&regs, *rhs, num_locals));
                    let result = vm_try!(self.eval_binary(l, *op, r));
                    if !vm_try!(self.truthy_value(&result)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::CmpJumpIfTrue(lhs, op, rhs, offset) => {
                    if let Some(cond) =
                        try_integer_compare_fast(&regs[*lhs as usize], &regs[*rhs as usize], *op)
                    {
                        if cond {
                            pc = jump_pc!(*offset);
                        }
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(&regs, *rhs, num_locals));
                    let result = vm_try!(self.eval_binary(l, *op, r));
                    if vm_try!(self.truthy_value(&result)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::CmpJumpIfFalseConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    let lv = &regs[*lhs as usize];
                    if let Some(cond) = try_constant_compare_fast(lv, cv, *op) {
                        if !cond {
                            pc = jump_pc!(*offset);
                        }
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = cv.clone();
                    let result = vm_try!(self.eval_binary(l, *op, r));
                    if !vm_try!(self.truthy_value(&result)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::CmpJumpIfTrueConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    let lv = &regs[*lhs as usize];
                    if let Some(cond) = try_constant_compare_fast(lv, cv, *op) {
                        if cond {
                            pc = jump_pc!(*offset);
                        }
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = cv.clone();
                    let result = vm_try!(self.eval_binary(l, *op, r));
                    if vm_try!(self.truthy_value(&result)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::JumpIfNotInt(reg, offset) => {
                    // Entry guard for an out-of-line int-specialized loop copy.
                    // Unset slots must divert to the original loop so its own
                    // CheckLocal / name-error paths stay authoritative.
                    let value = &regs[*reg as usize];
                    if value.is_unset() || value.as_int().is_none() {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::CountCmpJumpTrue(var, op, stop, imm, offset) => {
                    // Exact composition of `BinOpImm(var, var, Add, imm, true)`
                    // followed by `CmpJumpIfTrue(var, op, stop, offset)`.
                    vm_try!(self.exec_binop_immediate(
                        &mut regs,
                        *var,
                        *var,
                        crate::ast::BinaryOp::Add,
                        *imm,
                        true,
                        num_locals,
                    ));
                    if let Some(cond) =
                        try_integer_compare_fast(&regs[*var as usize], &regs[*stop as usize], *op)
                    {
                        if cond {
                            pc = jump_pc!(*offset);
                        }
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *var, num_locals));
                    let r = vm_try!(vm_read(&regs, *stop, num_locals));
                    let result = vm_try!(self.eval_binary(l, *op, r));
                    if vm_try!(self.truthy_value(&result)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::CallInlineBinOp {
                    callee,
                    dst,
                    a,
                    op,
                    b,
                    proto,
                    skip,
                } => {
                    // Guarded leaf-call inline: on success this is the entire
                    // observable effect of the call sequence it precedes; on
                    // any guard failure fall through into that sequence.
                    if let Some(result) = try_inline_leaf_binop(
                        &regs[*callee as usize],
                        &regs[*a as usize],
                        &regs[*b as usize],
                        *op,
                        code,
                        *proto,
                    ) {
                        regs[*dst as usize] = result;
                        pc = jump_pc!(*skip);
                    }
                }
                Insn::CountCmpJumpFalse(var, op, stop, imm, offset) => {
                    // Exact composition of `BinOpImm(var, var, Add, imm, true)`
                    // followed by `CmpJumpIfFalse(var, op, stop, offset)`.
                    vm_try!(self.exec_binop_immediate(
                        &mut regs,
                        *var,
                        *var,
                        crate::ast::BinaryOp::Add,
                        *imm,
                        true,
                        num_locals,
                    ));
                    if let Some(cond) =
                        try_integer_compare_fast(&regs[*var as usize], &regs[*stop as usize], *op)
                    {
                        if !cond {
                            pc = jump_pc!(*offset);
                        }
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *var, num_locals));
                    let r = vm_try!(vm_read(&regs, *stop, num_locals));
                    let result = vm_try!(self.eval_binary(l, *op, r));
                    if !vm_try!(self.truthy_value(&result)) {
                        pc = jump_pc!(*offset);
                    }
                }

                // ── Exception handling ───────────────────────────────────
                Insn::SetupExcept(offset) => {
                    exc_handlers.push(jump_pc!(*offset));
                }
                Insn::PopExcept => {
                    exc_handlers.pop();
                }
                Insn::LoadExc(dst) => {
                    let exc =
                        vm_try!(self.active_exception.clone().ok_or_else(|| {
                            PyError::Runtime("no active exception".to_string())
                        }));
                    regs[*dst as usize] = exc;
                }
                Insn::LoadExcTraceback(dst, exc) => {
                    let exc_val = vm_try!(vm_read(&regs, *exc, num_locals));
                    regs[*dst as usize] = self.exception_traceback_value(&exc_val);
                }
                Insn::MatchExcept(type_reg, offset) => {
                    let type_val = vm_try!(vm_read(&regs, *type_reg, num_locals));
                    let exc = vm_try!(self.active_exception.as_ref().ok_or_else(|| {
                        PyError::Runtime(
                            "internal error: MatchExcept with no active exception".to_string(),
                        )
                    }));
                    if !vm_try!(self.exception_matches(exc, &type_val)) {
                        pc = jump_pc!(*offset);
                    }
                    // No stack push on match: the dispatch already pushed
                    // the exception onto `handled_exc_stack` when vm_try!
                    // routed us here, so MatchExcept is purely a filter.
                }
                Insn::MatchExceptStar(type_reg, src_group, matched_dst, offset) => {
                    // PEP 654 `except*` filter.
                    // Reads R[src_group] (a BaseExceptionGroup), filters for instances
                    // of R[type_reg].  If no match: jump.
                    // If match: R[matched_dst] = sub-group of matching exceptions;
                    //           R[src_group]   = sub-group of remaining (non-matching)
                    //                            exceptions, or None if all matched.
                    let type_val = vm_try!(vm_read(&regs, *type_reg, num_locals));
                    let group_val = vm_try!(vm_read(&regs, *src_group, num_locals));
                    // Gather matching and remaining exceptions from the group.
                    let result = vm_try!(self.split_exception_group(&group_val, &type_val));
                    match result {
                        None => {
                            // No match — jump past the handler.
                            pc = jump_pc!(*offset);
                        }
                        Some((matched_group, remaining)) => {
                            // Store matched sub-group into R[matched_dst].
                            regs[*matched_dst as usize] = matched_group;
                            // Update R[src_group] with remaining (None = exhausted).
                            // The leftover group, if any, is re-raised by the
                            // epilogue via `RaiseExceptStarResidual` (#2755).
                            regs[*src_group as usize] = remaining.unwrap_or_else(Value::none);
                        }
                    }
                }
                Insn::EndExcept => {
                    // Leaving an `except` handler body normally (the exception
                    // was truly handled, not re-raised).  Pop the entry that
                    // vm_try! pushed on dispatch.  Restore `active_exception`
                    // from the parallel save-stack (exc_saved_active) rather
                    // than from handled_exc_stack.last(), because the dedup-pop
                    // inside handle_vm_error may have removed the outer
                    // handler's entry from handled_exc_stack when the inner
                    // exception was dispatched.
                    if self.handled_exc_stack.len() > exc_ctx_frame_base {
                        self.handled_exc_stack.pop();
                    }
                    self.active_exception =
                        if self.exc_saved_active.len() > exc_saved_active_frame_base {
                            self.exc_saved_active.pop().unwrap()
                        } else {
                            None
                        };
                    // Clear the captured frame snapshot only here — on normal
                    // handler exit.  Clearing at handler *entry* (dispatch_exc_handler)
                    // was wrong because a bare `raise` inside the handler would
                    // clear the original frames before the re-raised exception
                    // propagated, producing a traceback that omitted inner frames.
                    pyrust_core::reset_captured_error_frames();
                }
                Insn::PushExcContext(src) => {
                    // Temporarily install R[src] as the active exception context
                    // before running an inlined finally block.  Any raise inside
                    // the finally will call attach_implicit_context and see this
                    // value (the to-be-raised exception) rather than the
                    // currently-handled exception below it on the stack.
                    //
                    // Increment push_exc_ctx_depth so that handle_vm_error's
                    // duplicate-detection check does not prematurely pop this
                    // entry when a new exception is dispatched inside the finally.
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    self.handled_exc_stack.push(val.clone());
                    self.active_exception = Some(val);
                    self.push_exc_ctx_depth += 1;
                }
                Insn::PopExcContext => {
                    // Undo a PushExcContext after the inlined finally block
                    // completes normally.  Bounded by this frame's base depth.
                    if self.push_exc_ctx_depth > 0 {
                        self.push_exc_ctx_depth -= 1;
                    }
                    if self.handled_exc_stack.len() > exc_ctx_frame_base {
                        self.handled_exc_stack.pop();
                    }
                    self.active_exception = self.handled_exc_stack.last().cloned();
                }
                Insn::RaiseAssert(msg_reg) => {
                    let msg = vm_try!(vm_read(&regs, *msg_reg, num_locals));
                    let exc = vm_try!(self.prepare_assertion_error(Some(msg)));
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseAssertNoMsg => {
                    let exc = vm_try!(self.prepare_assertion_error(None));
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseValue(src) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    let exc = vm_try!(self.prepare_explicit_raise(val));
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseExceptStarResidual(src) => {
                    let exc = vm_try!(vm_read(&regs, *src, num_locals));
                    self.prepare_exception_group_residual(&exc);
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseFrom(src, cause_reg) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    let cause_raw = vm_try!(vm_read(&regs, *cause_reg, num_locals));
                    let exc = vm_try!(self.prepare_explicit_raise_from(val, cause_raw));
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseReRaise => {
                    let exc = vm_try!(self.active_exception.take().ok_or_else(|| {
                        PyError::Runtime("No active exception to reraise".to_string())
                    }));
                    // RaiseReRaise is emitted by the compiler at three
                    // logical "exit-this-handler" points: bare `raise`
                    // inside an except, the fall-through after a chain of
                    // unmatched MatchExcepts, and the implicit re-raise
                    // at the end of a finally exception-path.  In all
                    // three cases we are leaving the dispatch / handler
                    // body, so pop the corresponding context-stack entry
                    // pushed by vm_try!.  Bounded by this frame's base
                    // depth so we never reach into caller entries.
                    if self.handled_exc_stack.len() > exc_ctx_frame_base {
                        self.handled_exc_stack.pop();
                    }
                    // Restore active_exception to the value that was active
                    // before this except block started.  This ensures that if
                    // the re-raised exception is caught by an outer handler in
                    // this frame, handle_vm_error sees the correct prev_active
                    // when it saves the outer handler's prior active.
                    if self.exc_saved_active.len() > exc_saved_active_frame_base {
                        self.active_exception = self.exc_saved_active.pop().unwrap();
                    }
                    // Issue #2405: a *bare* `raise` (and the implicit re-raise at
                    // the end of a finally / unmatched-except chain) re-raises the
                    // currently-handled exception without adding a traceback node
                    // for the re-raising frame — CPython keeps the carried chain
                    // unchanged and only prepends the genuinely-outer frames the
                    // exception unwinds through *after* the re-raise.
                    //
                    // Reset the captured-frame snapshot (when the exception carries
                    // a chain) for the same reason as the explicit re-raise: the
                    // carried chain already accounts for the original raise's
                    // frames, so they must not be re-counted in the freshly-built
                    // prefix.  After the reset the snapshot rebuilds from this
                    // point — and the catch site drops its innermost (this
                    // re-raising frame) so the re-raise line never appears as a
                    // node (`caught_traceback_value`).  When the re-raise is caught
                    // in this same frame the snapshot stays empty and the carried
                    // chain — including the `with`/`__exit__` same-frame identity
                    // object (#2359/#2366) — is kept verbatim.
                    self.reraise_is_bare = true;
                    self.reset_captured_frames_if_reraise(&exc);
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }

                Insn::MatchClassPositional {
                    dst_base,
                    subj,
                    cls,
                    n,
                } => {
                    let class = vm_try!(vm_read(&regs, *cls, num_locals));
                    let subject = vm_try!(vm_read(&regs, *subj, num_locals));
                    let values =
                        vm_try!(self.match_class_positional_values(&subject, &class, *n as usize,));
                    for (offset, value) in values.into_iter().enumerate() {
                        regs[*dst_base as usize + offset] = value;
                    }
                }

                // ── Calls ────────────────────────────────────────────────
                Insn::Call(func_reg, argc) => {
                    let func_val = vm_try!(vm_read(&regs, *func_reg, num_locals));
                    if *argc == 1
                        && let Some(argument) = regs.get((*func_reg + 1) as usize)
                        && let Some(identity) = Self::try_identity_builtin_call(&func_val, argument)
                    {
                        regs[*func_reg as usize] = identity;
                        continue 'vm;
                    }
                    let argument_base = (*func_reg + 1) as usize;
                    let argument_end = argument_base + *argc as usize;
                    if let Some(result) = self.try_builtin_vectorcall(
                        code,
                        pc - 1,
                        &func_val,
                        &(*regs)[argument_base..argument_end],
                    ) {
                        regs[*func_reg as usize] = vm_try!(result);
                        continue 'vm;
                    }
                    tramp_try!(*func_reg, *argc, func_val);
                    // Bound-method trampoline (#2345): `f = o.m; f()` calls a
                    // BoundMethod through Insn::Call.  Trampoline the underlying
                    // regular method with the receiver bound to `self`, matching
                    // the speedup plain functions already get above.
                    if let ValueKind::BoundMethod { function, receiver } = func_val.kind()
                        && matches!(function.kind, pyrust_core::UserFunctionKind::Regular)
                    {
                        let f = Rc::clone(function);
                        let recv = Value::py_instance(Rc::clone(receiver));
                        tramp_try_method!(*func_reg, &f, recv, *func_reg + 1, *argc);
                    }
                    regs[*func_reg as usize] = vm_try!(self.call_positional_cached(
                        &mut regs,
                        num_locals,
                        *func_reg,
                        *argc,
                        func_val,
                        code,
                        pc - 1,
                        cur_line,
                    ));
                }

                Insn::CallMemo(func_reg, argc) => {
                    let func_val = vm_try!(vm_read(&regs, *func_reg, num_locals));
                    match vm_try!(
                        self.probe_memoized_call(&regs, *func_reg, *argc, num_locals, &func_val,)
                    ) {
                        MemoCallProbe::Hit(result) => {
                            regs[*func_reg as usize] = result;
                            continue 'vm;
                        }
                        MemoCallProbe::Miss(key) => {
                            // The probe is execution-free: let an eligible
                            // callee enter the VM trampoline and attach this
                            // miss key to its explicit frame. If the gate
                            // rejects it, preserve the native memoized fallback.
                            tramp_try!(@memo *func_reg, *argc, func_val, key.clone());
                            regs[*func_reg as usize] = vm_try!(self.call_memoized_miss_native(
                                &regs, *func_reg, *argc, num_locals, &func_val, key,
                            ));
                        }
                        MemoCallProbe::Bypass => {
                            tramp_try!(*func_reg, *argc, func_val);
                            regs[*func_reg as usize] = vm_try!(self.call_positional_cached(
                                &mut regs,
                                num_locals,
                                *func_reg,
                                *argc,
                                func_val,
                                code,
                                pc - 1,
                                cur_line,
                            ));
                        }
                    }
                }

                Insn::CallKw {
                    func,
                    total,
                    nkw,
                    kwnames_idx,
                } => {
                    // Keyword call with no splats (issue #2382).  The args live in
                    // `R[func+1 .. func+1+total]`; the last `nkw` are keyword args
                    // whose names are the const-pool tuple `consts[kwnames_idx]`.
                    // The result is written back to `R[func]`.  Full body (cache
                    // lookup / fill + fast bind + slow-path fallback) lives in
                    // fast_path.rs::exec_call_kw.
                    let res = self.exec_call_kw(
                        &regs,
                        code,
                        pc,
                        *func,
                        *total,
                        *nkw,
                        *kwnames_idx,
                        num_locals,
                    );
                    regs[*func as usize] = vm_try!(res);
                }

                Insn::CallEx { func, npos, kwargs } => {
                    // Double-splat expansion call `f(<pos…>, **d)` (issue #2393).
                    // `R[func+1 .. func+1+npos]` are positionals; `R[kwargs]` is the
                    // `**d` source mapping.  Result is written back to `R[func]`.
                    // Full body (shape-cache lookup / fill + fast bind + slow-path
                    // fallback) lives in fast_path.rs::exec_call_ex.
                    let res = self.exec_call_ex(&regs, code, pc, *func, *npos, *kwargs, num_locals);
                    regs[*func as usize] = vm_try!(res);
                }

                Insn::CallExArgs {
                    func,
                    npos,
                    nkw,
                    kwnames_idx,
                    args_splat,
                    kwargs,
                } => {
                    // Positional-splat expansion call `f(<pos…>, *args, kw=v…[,
                    // **kw])` (the decorator/wrapper shape).  `R[func+1 ..
                    // func+1+npos]` are leading positionals; `R[func+1+npos ..
                    // +nkw]` are the literal keyword values (names in
                    // `consts[kwnames_idx]`); `R[args_splat]` is the `*args`
                    // iterable; `R[kwargs]` is the `**kw` mapping or `NO_KWARGS`.
                    // Result is written back to `R[func]`.  Full body (shape-cache
                    // lookup / fill + fast bind + slow-path fallback) lives in
                    // fast_path.rs::exec_call_ex_args.
                    let res = self.exec_call_ex_args(
                        &regs,
                        code,
                        pc,
                        *func,
                        *npos,
                        *nkw,
                        *kwnames_idx,
                        *args_splat,
                        *kwargs,
                        num_locals,
                    );
                    regs[*func as usize] = vm_try!(res);
                }

                Insn::CallMethod {
                    dst,
                    obj,
                    name_idx,
                    args_base,
                    nargs,
                } => {
                    // Method-call trampoline (#2345): on an inline-cache hit for
                    // a plain Python method, bind the receiver to `self` and loop
                    // into the callee here, instead of re-entering
                    // `call_user_function_expanded` natively (the same win plain
                    // `Insn::Call` gets from `tramp_try!`).  Falls back to
                    // `exec_call_method` on a cache miss, a builtin/backing
                    // method, or any trampoline-gate failure.
                    if let Some(method) = code.names.get(*name_idx as usize)
                        && let Some((f, recv_rc)) =
                            self.resolve_method_cached(&regs, *obj, method, code, pc - 1)
                    {
                        let recv = Value::py_instance(recv_rc);
                        tramp_try_method!(*dst, &f, recv, *args_base, *nargs);
                    }
                    // pc was incremented before dispatch; the instruction position is pc - 1.
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
                        cur_line,
                    );
                    regs[*dst as usize] = vm_try!(r);
                }

                Insn::CallMethodExpanded {
                    dst,
                    obj,
                    name_idx,
                    pos_list,
                    kw_dict,
                } => {
                    let r = self.exec_call_method_expanded(
                        &mut regs, num_locals, *dst, *obj, *name_idx, *pos_list, *kw_dict, code,
                    );
                    regs[*dst as usize] = vm_try!(r);
                }
                Insn::CallMethodKw {
                    dst,
                    obj,
                    name_idx,
                    args_base,
                    total,
                    nkw,
                    kwnames_idx,
                } => {
                    // Keyword method call (#2392).  The receiver is in `R[obj]`;
                    // the `total` argument values live in `R[args_base ..
                    // args_base+total]` (trailing `nkw` are keyword args named by
                    // `consts[kwnames_idx]`).  On a monomorphic inline-cache hit
                    // for a plain Python method the receiver binds to param 0 and
                    // the keyword values fast-bind into their cached slots; on a
                    // miss it falls back to the general method-expansion path.
                    // Full body lives in fast_path.rs::exec_call_method_kw.
                    let r = self.exec_call_method_kw(
                        &mut regs,
                        num_locals,
                        *dst,
                        *obj,
                        *name_idx,
                        *args_base,
                        *total,
                        *nkw,
                        *kwnames_idx,
                        code,
                        pc - 1,
                        cur_line,
                    );
                    regs[*dst as usize] = vm_try!(r);
                }

                // ── Returns ──────────────────────────────────────────────
                Insn::Return(src) => {
                    let v = vm_try!(vm_read(&regs, *src, num_locals));
                    tramp_return!(v);
                }
                Insn::ReturnNone => {
                    tramp_return!(Value::none());
                }

                // ── Collection builders ──────────────────────────────────
                Insn::BuildList(dst, base, n) => {
                    let mut items: Vec<Value> = Vec::with_capacity(*n as usize);
                    for i in 0..*n {
                        items.push(vm_try!(vm_read(&regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Value::list(items);
                }
                Insn::BuildListReserve(dst, src) => {
                    // Fresh empty list pre-sized to the source's length hint.
                    // Only queries lengths that never run user code; an
                    // unknown-length source (generator, user iterable, …)
                    // reserves nothing.  Behaviour is identical to an empty
                    // `BuildList` — this is a capacity hint only.
                    let src_val = vm_try!(vm_read(&regs, *src, num_locals));
                    let cap = list_reserve_hint(&src_val);
                    let items: Vec<Value> = Vec::with_capacity(cap);
                    regs[*dst as usize] = Value::list(items);
                }
                Insn::BuildTuple(dst, base, n) => {
                    let mut items = Vec::with_capacity(*n as usize);
                    for i in 0..*n {
                        items.push(vm_try!(vm_read(&regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Value::tuple(items);
                }
                Insn::BuildString(dst, base, n) => {
                    regs[*dst as usize] = vm_try!(build_string_fast(&regs, *base, *n, num_locals));
                }
                Insn::FormatValue(dst, src) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    let s = vm_try!(self.format_value_default(&val));
                    regs[*dst as usize] = s;
                }
                Insn::FormatValueSpec(dst, src, spec_r) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    let spec_val = vm_try!(vm_read(&regs, *spec_r, num_locals));
                    regs[*dst as usize] = vm_try!(self.format_value_spec_cached(
                        &val,
                        &spec_val,
                        &code.fmt_spec_cache,
                        pc - 1,
                    ));
                }
                Insn::BuildSlice(dst, base) => {
                    // Reads three contiguous registers (base, base+1, base+2) holding
                    // the start, stop, step bounds.  `None` values mean "absent bound".
                    // Produces a runtime `slice` BuiltinObject rather than a tuple so
                    // that `unpack_slice_key` can distinguish it from a user 3-tuple
                    // (issue #931 fix).
                    let start = vm_try!(vm_read(&regs, *base, num_locals));
                    let stop = vm_try!(vm_read(&regs, *base + 1, num_locals));
                    let step = vm_try!(vm_read(&regs, *base + 2, num_locals));
                    let lo = if start.is_none() { None } else { Some(start) };
                    let hi = if stop.is_none() { None } else { Some(stop) };
                    let st = if step.is_none() { None } else { Some(step) };
                    regs[*dst as usize] = make_slice_value(lo, hi, st);
                }
                Insn::BuildDict(dst, base, n) => {
                    let pairs = (0..*n).map(|offset| {
                        let key = vm_read(&regs, *base + offset * 2, num_locals)?;
                        let value = vm_read(&regs, *base + offset * 2 + 1, num_locals)?;
                        Ok((key, value))
                    });
                    regs[*dst as usize] = vm_try!(self.dict_from_value_pairs(*n as usize, pairs));
                }
                Insn::SetAdd(set_reg, val_reg) => {
                    let set = vm_try!(vm_read(&regs, *set_reg, num_locals));
                    let val = vm_try!(vm_read(&regs, *val_reg, num_locals));
                    vm_try!(self.set_insert_value(&set, val));
                }
                Insn::ListAppend(list_reg, val_reg) => {
                    let val = vm_try!(vm_read(&regs, *val_reg, num_locals));
                    vm_try!(regs[*list_reg as usize].list_push(val));
                }
                Insn::ListExtend(list_reg, src_reg) => {
                    let list = vm_try!(vm_read(&regs, *list_reg, num_locals));
                    let src_val = vm_try!(vm_read(&regs, *src_reg, num_locals));
                    vm_try!(self.extend_list_accumulator(&list, &src_val));
                }
                Insn::DictUpdate(dict_reg, src_reg) => {
                    let dict = vm_try!(vm_read(&regs, *dict_reg, num_locals));
                    let src_val = vm_try!(vm_read(&regs, *src_reg, num_locals));
                    vm_try!(self.update_dict_accumulator(&dict, &src_val));
                }

                // `**d` keyword splat in a call: like `DictUpdate` but raises
                // `TypeError` on a duplicate key (CPython `DICT_MERGE`, #2413).
                Insn::DictMergeKwCall { dict, src, name } => {
                    let src_val = vm_try!(vm_read(&regs, *src, num_locals));
                    let dict_val = regs[*dict as usize]
                        .as_some()
                        .cloned()
                        .unwrap_or(Value::none());
                    if let Some(kw) = vm_try!(self.merge_kwcall_mapping(&dict_val, &src_val)) {
                        let fname = self.kwcall_func_name(&regs, num_locals, name, code);
                        vm_try!(Err(duplicate_keyword_error(fname.as_deref(), &kw)));
                    }
                }

                // Named (`kw=v`) argument in a call that also has a `**d` splat:
                // `SetItem` that raises the same duplicate-key `TypeError`.
                Insn::SetItemKwCall {
                    dict,
                    key,
                    val,
                    name,
                } => {
                    let key_val = vm_try!(vm_read(&regs, *key, num_locals));
                    let val_val = vm_try!(vm_read(&regs, *val, num_locals));
                    let dict_val = regs[*dict as usize]
                        .as_some()
                        .cloned()
                        .unwrap_or(Value::none());
                    if let Some(kw) = vm_try!(self.set_kwcall_value(&dict_val, &key_val, val_val)) {
                        let fname = self.kwcall_func_name(&regs, num_locals, name, code);
                        vm_try!(Err(duplicate_keyword_error(fname.as_deref(), &kw)));
                    }
                }

                // ── Generator yield ──────────────────────────────────────
                Insn::Yield { src, dst } => {
                    // Suspend the generator.  pc has already been incremented
                    // past this instruction, so resumption continues at pc.
                    let yielded = vm_try!(vm_read(&regs, *src, num_locals));
                    // Pre-fill dst with None so the register holds a valid
                    // value while the frame is suspended.  resume_generator_with_exc
                    // overwrites this with the sent value (None for next(),
                    // the caller's argument for send()) before resuming.
                    regs[*dst as usize] = Value::none();
                    // ── Generator trampoline switch-back (#2253) ─────────────
                    // If this generator is being driven by a `ForIter` consumer
                    // within this same dispatch loop, save its resume state, put
                    // its frame back into its `Value`, restore the consumer, and
                    // deliver the yielded value to the `ForIter` destination —
                    // instead of returning `FrameOutcome::Yielded` up through a
                    // native `run_bytecode_inner` re-entry.  The `!has_exc_handlers`
                    // gate means there is no handled-exception slice to split off.
                    if active_is_gen_drive!() {
                        let mut gd = gen_drive_stack.pop().unwrap();
                        gd.gframe.pc = pc;
                        gd.gframe.iters = std::mem::take(&mut iters);
                        gd.gframe.exc_handlers = std::mem::take(&mut exc_handlers);
                        gd.gframe.yield_dst = *dst;
                        // Issue #2445: persist the yield line so a later
                        // `generator.throw()` reports it (mirrors the native
                        // suspend path in `Insn::Yield`).
                        gd.gframe.suspended_line = cur_line;
                        self.vm_frame_views.pop();
                        let dst_reg = gd.dst as usize;
                        regs = gd.saved_regs;
                        pc = gd.saved_pc;
                        cur_line = gd.saved_cur_line;
                        code_ptr = gd.saved_code_ptr;
                        active_code_rc = gd.saved_active_code_rc;
                        num_locals = gd.saved_num_locals;
                        self.env = gd.saved_env;
                        iters = gd.saved_iters;
                        iter_next_cache = gd.saved_iter_cache;
                        exc_handlers = gd.saved_exc_handlers;
                        tramp_active_base = gd.saved_base;
                        let boxed: Box<dyn std::any::Any> = gd.gframe;
                        *gd.state_rc.borrow_mut() = boxed;
                        regs[dst_reg] = yielded;
                        continue 'vm;
                    }
                    // PEP 3134: split the interpreter's handled-exception
                    // stack at this frame's base.  Entries pushed by THIS
                    // generator frame (above `exc_ctx_frame_base`) are saved
                    // onto the generator, then removed from the interpreter
                    // so the caller's frame sees only its own entries.  The
                    // generator's `active_exception` is likewise stashed and
                    // its slot cleared.  Both are re-installed on resume.
                    // The parallel exc_saved_active slice is split off and
                    // saved alongside so that nested-except state persists
                    // across yield points.
                    let saved_handled_slice: HandledExcBuf =
                        self.handled_exc_stack.split_off(exc_ctx_frame_base).into();
                    let saved_exc_saved_active =
                        self.exc_saved_active.split_off(exc_saved_active_frame_base);
                    let saved_active = self.active_exception.take();
                    // Return an explicit FrameOutcome::Yielded rather than
                    // using the old GEN_SAVE thread-local side-channel.
                    // Use mem::take to move iters and exc_handlers into the
                    // saved state rather than cloning them.  The local
                    // variables are left as empty SmallVecs, which are then
                    // dropped for free when the function returns.  On resume,
                    // resume_generator_with_exc moves them back via
                    // std::mem::take on frame.iters / frame.exc_handlers.
                    return Ok(FrameOutcome::Yielded {
                        value: yielded,
                        saved: GenSaveState {
                            iters: std::mem::take(&mut iters),
                            exc_handlers: std::mem::take(&mut exc_handlers),
                            pc, // already past the Yield instruction
                            handled_exc_slice: saved_handled_slice,
                            active_exception: saved_active,
                            exc_saved_active_slice: saved_exc_saved_active,
                            yield_dst: *dst,
                            suspended_line: cur_line,
                        },
                    });
                }

                // ── GetAwaitable (await, issue #1039) ────────────────────
                Insn::GetAwaitable(dst, src) => {
                    let awaited = vm_try!(vm_read(&regs, *src, num_locals));
                    let resolved = vm_try!(self.get_awaitable(&awaited));
                    regs[*dst as usize] = resolved;
                }

                // ── YieldFrom (PEP 380) ──────────────────────────────────
                Insn::YieldFrom {
                    iter_reg,
                    sent_reg,
                    result_reg,
                } => {
                    let iter_val = vm_try!(vm_read(&regs, *iter_reg, num_locals));
                    let sent_val = vm_try!(vm_read(&regs, *sent_reg, num_locals));

                    // Call next()/send() on the sub-iterator.
                    let sub_result = self.yield_from_advance(&iter_val, sent_val);

                    match sub_result {
                        Ok(yielded) => {
                            // Sub-iterator yielded a value: forward to our caller,
                            // suspending at this YieldFrom instruction so the next
                            // resume re-executes it with the caller's sent value.
                            // Set yield_dst = *sent_reg so resume_generator_with_exc
                            // writes the next sent value there.
                            regs[*sent_reg as usize] = Value::none();
                            // ── Generator trampoline switch-back (#2338) ─────────
                            // If THIS generator is itself being driven by a `ForIter`
                            // consumer in this same dispatch loop (i.e. the `yield
                            // from` delegator is the for-loop's own generator), hand
                            // the value back to that consumer rather than returning
                            // `FrameOutcome::Yielded` — which would unwind up through a
                            // native `run_bytecode` that has no generator semantics and
                            // panic (`run_bytecode called on a non-generator
                            // function`).  Mirrors the `Insn::Yield` switch-back, but
                            // suspends at this `YieldFrom` (pc - 1) with yield_dst =
                            // *sent_reg so the next drive re-executes it and the sent
                            // value lands in the sub-iterator's slot.
                            if active_is_gen_drive!() {
                                let mut gd = gen_drive_stack.pop().unwrap();
                                gd.gframe.pc = pc - 1;
                                gd.gframe.iters = std::mem::take(&mut iters);
                                gd.gframe.exc_handlers = std::mem::take(&mut exc_handlers);
                                gd.gframe.yield_dst = *sent_reg;
                                // Issue #2445: persist the yield-from line.
                                gd.gframe.suspended_line = cur_line;
                                self.vm_frame_views.pop();
                                let dst_reg = gd.dst as usize;
                                regs = gd.saved_regs;
                                pc = gd.saved_pc;
                                cur_line = gd.saved_cur_line;
                                code_ptr = gd.saved_code_ptr;
                                active_code_rc = gd.saved_active_code_rc;
                                num_locals = gd.saved_num_locals;
                                self.env = gd.saved_env;
                                iters = gd.saved_iters;
                                iter_next_cache = gd.saved_iter_cache;
                                exc_handlers = gd.saved_exc_handlers;
                                tramp_active_base = gd.saved_base;
                                let boxed: Box<dyn std::any::Any> = gd.gframe;
                                *gd.state_rc.borrow_mut() = boxed;
                                regs[dst_reg] = yielded;
                                continue 'vm;
                            }
                            let saved_handled_slice: HandledExcBuf =
                                self.handled_exc_stack.split_off(exc_ctx_frame_base).into();
                            let saved_exc_saved_active =
                                self.exc_saved_active.split_off(exc_saved_active_frame_base);
                            let saved_active = self.active_exception.take();
                            return Ok(FrameOutcome::Yielded {
                                value: yielded,
                                saved: GenSaveState {
                                    iters: std::mem::take(&mut iters),
                                    exc_handlers: std::mem::take(&mut exc_handlers),
                                    // Rewind pc to point at the YieldFrom instruction
                                    // (pc was already incremented past it).
                                    pc: pc - 1,
                                    handled_exc_slice: saved_handled_slice,
                                    active_exception: saved_active,
                                    exc_saved_active_slice: saved_exc_saved_active,
                                    // The sent value for the next iteration goes into
                                    // sent_reg so the sub-iterator receives it.
                                    yield_dst: *sent_reg,
                                    suspended_line: cur_line,
                                },
                            });
                        }
                        Err(ref e) if is_stop_iteration_error(e) => {
                            // Sub-iterator exhausted.  Extract the StopIteration.value
                            // (the return value from a generator sub-iterator, or the
                            // first arg to `raise StopIteration(val)`).
                            let stop_val = extract_stop_iteration_value(e);
                            regs[*result_reg as usize] = stop_val.unwrap_or_else(Value::none);
                            // Continue executing after the YieldFrom instruction.
                        }
                        Err(e) => {
                            // Other exception: propagate through our own handler stack.
                            vm_try!(Err(e));
                        }
                    }
                }

                // ── Unpack ───────────────────────────────────────────────
                Insn::Unpack(base, src, n) => {
                    let src_val = vm_try!(vm_read(&regs, *src, num_locals));
                    let values = vm_try!(self.unpack_exact_values(&src_val, *n as usize));
                    vm_try!(store_register_values(&mut regs, *base, values, "Unpack"));
                }

                Insn::UnpackEx {
                    src,
                    before,
                    after,
                    dst_base,
                } => {
                    let src_val = vm_try!(vm_read(&regs, *src, num_locals));
                    let values = vm_try!(self.unpack_extended_values(
                        &src_val,
                        *before as usize,
                        *after as usize,
                    ));
                    vm_try!(store_register_values(
                        &mut regs, *dst_base, values, "UnpackEx",
                    ));
                }

                // ── Iterator ─────────────────────────────────────────────
                Insn::GetIter(slot, src) => {
                    let src_val = vm_try!(vm_read(&regs, *src, num_locals));
                    let state = vm_try!(self.make_loop_iter_state(src_val));
                    iters[*slot as usize] = Some(state);
                    iter_next_cache[*slot as usize] = None;
                }
                Insn::ForIter(dst, slot, offset) => {
                    // Generator trampoline (#2253): set inside the match when the
                    // iterator is a drivable generator; the switch-in is performed
                    // after the match closes (it needs `&mut iters`, borrowed here).
                    let mut gen_to_drive: Option<Rc<RefCell<Box<dyn std::any::Any>>>> = None;
                    match iters[*slot as usize].as_mut() {
                        Some(state) => {
                            match advance_loop_fast_state(state, code, &mut regs, &mut pc, *dst) {
                                LoopFastOutcome::Advanced => {}
                                LoopFastOutcome::Exhausted => {
                                    pc = jump_pc!(*offset);
                                }
                                LoopFastOutcome::Error(error) => vm_try!(Err(*error)),
                                LoopFastOutcome::UserDefined => {
                                    let IterState::UserDefined(iterator) = state else {
                                        unreachable!(
                                            "only user iterators bypass the fast-state step"
                                        );
                                    };
                                    let cached_next = &mut iter_next_cache[*slot as usize];
                                    match self.advance_loop_iterator(iterator, cached_next) {
                                        LoopIteratorAdvance::DriveGenerator(state) => {
                                            gen_to_drive = Some(state);
                                        }
                                        LoopIteratorAdvance::Item(Ok(value)) => {
                                            regs[*dst as usize] = value;
                                        }
                                        LoopIteratorAdvance::Item(Err(ref error))
                                            if is_stop_iteration_error(error) =>
                                        {
                                            pc = jump_pc!(*offset);
                                        }
                                        LoopIteratorAdvance::Item(Err(error)) => {
                                            vm_try!(Err(error));
                                        }
                                        LoopIteratorAdvance::NotIterator => {
                                            vm_try!(Err(pyrust_core::type_err!(
                                                "iter() returned non-iterator of type '{}'",
                                                value_type_name_str(iterator),
                                            )));
                                        }
                                    }
                                }
                            }
                        }
                        None => {
                            pc = jump_pc!(*offset);
                        }
                    }
                    // ── Generator trampoline switch-in (#2253) ───────────────
                    // `iters` is no longer borrowed here, so the consumer frame
                    // can be saved and the generator's frame switched in.  The
                    // setup itself lives in the cold, out-of-line
                    // `vm_enter_gen_drive` so it does not bloat this hot arm.
                    if let Some(state_rc) = gen_to_drive {
                        let exit_pc = jump_pc!(*offset);
                        let mut st = UnwindState {
                            regs: &mut regs,
                            pc: &mut pc,
                            cur_line: &mut cur_line,
                            code_ptr: &mut code_ptr,
                            active_code_rc: &mut active_code_rc,
                            num_locals: &mut num_locals,
                            iters: &mut iters,
                            iter_next_cache: &mut iter_next_cache,
                            exc_handlers: &mut exc_handlers,
                            tramp_active_base: &mut tramp_active_base,
                            tramp_stack: &mut tramp_stack,
                            gen_drive_stack: &mut gen_drive_stack,
                            exc_ctx_frame_base,
                        };
                        vm_try!(self.vm_enter_gen_drive(state_rc, *dst, exit_pc, &mut st));
                        continue 'vm;
                    }
                }
                Insn::CheckLocal(reg, name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.check_local_binding(&regs[*reg as usize], name,));
                }

                // ── Function / Class creation ────────────────────────────
                Insn::MakeFunction(dst, proto_idx, defs_base, _defs_n, annots_base, _annots_n) => {
                    let r = self.exec_make_function(
                        code,
                        &regs,
                        num_locals,
                        *proto_idx,
                        *defs_base,
                        *annots_base,
                    );
                    regs[*dst as usize] = vm_try!(r);
                }
                Insn::MakeClass(
                    dst,
                    proto_idx,
                    bases_base,
                    bases_n,
                    name_idx,
                    kwarg_base,
                    kwarg_n,
                ) => {
                    let r = self.exec_make_class(
                        code,
                        &regs,
                        num_locals,
                        *proto_idx,
                        *bases_base,
                        *bases_n,
                        *name_idx,
                        *kwarg_base,
                        *kwarg_n,
                    );
                    regs[*dst as usize] = vm_try!(r);
                }
                Insn::MakeClassMeta(
                    dst,
                    proto_idx,
                    bases_base,
                    bases_n,
                    name_idx,
                    kwarg_base,
                    kwarg_n,
                    meta_reg,
                ) => {
                    let r = self.exec_make_class_meta(
                        code,
                        &regs,
                        num_locals,
                        *proto_idx,
                        *bases_base,
                        *bases_n,
                        *name_idx,
                        *kwarg_base,
                        *kwarg_n,
                        *meta_reg,
                    );
                    regs[*dst as usize] = vm_try!(r);
                }

                // ── PEP 695 type alias ───────────────────────────────────
                Insn::MakeTypeVar(dst, name_idx) => {
                    let name_val = pool_get!(code.consts, *name_idx, "const");
                    regs[*dst as usize] = vm_try!(make_typevar_from_syntax(name_val));
                }
                Insn::MakeTypeAlias(dst, name_idx, value_reg, params_reg) => {
                    let name_val = pool_get!(code.consts, *name_idx, "const");
                    let value_val = vm_try!(vm_read(&regs, *value_reg, num_locals)).clone();
                    let params_val = vm_try!(vm_read(&regs, *params_reg, num_locals)).clone();
                    let module_name = vm_try!(self.defining_module_name());
                    regs[*dst as usize] = vm_try!(make_type_alias_from_syntax(
                        name_val,
                        value_val,
                        params_val,
                        module_name,
                    ));
                }

                // ── Import ───────────────────────────────────────────────
                Insn::ImportModule(dst, name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name");
                    let module = vm_try!(self.load_module(name));
                    regs[*dst as usize] = module;
                }
                Insn::ImportStar(mod_reg) => {
                    vm_try!(self.exec_import_star(&regs, num_locals, *mod_reg));
                }

                // ── REPL output ──────────────────────────────────────────
                Insn::PrintExpr(src) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    if !val.is_none() {
                        println!("{}", val.repr_raw());
                    }
                }

                // ── Class-namespace insertion-order tracking ─────────────
                // Emitted by the compiler only inside class bodies; the
                // surrounding `MakeClass` always sets up the stack frame so
                // the `last_mut()` call below is safe.  If we somehow hit
                // these insns outside a class body the stack will be empty
                // — ignore them silently rather than panic, since reordering
                // / inlining passes could in principle hoist them.
                Insn::RecordClassStore(slot) => {
                    if let Some(order) = self.class_store_order.last_mut()
                        && !order.contains(slot)
                    {
                        order.push(*slot);
                    }
                }
                Insn::RecordClassDel(slot) => {
                    if let Some(order) = self.class_store_order.last_mut() {
                        order.retain(|s| s != slot);
                    }
                }
            }
        }
    }
}
