/// Inline capacity for the VM's per-frame register file.
///
/// `Value` is a NaN-boxed `u64` (8 bytes), so 16 inline slots = 128 bytes of
/// register storage embedded directly in the stack frame.  Most user functions
/// have fewer than 16 locals + temporaries, so this avoids the per-call heap
/// allocation that a fresh `Vec<Value>` would require.  Functions with more
/// than 16 registers transparently spill onto the heap (same big-O as before).
pub(crate) const VM_REGS_INLINE: usize = 16;

/// Per-frame register file backing for the VM.
pub(crate) type RegsBuf = smallvec::SmallVec<[Value; VM_REGS_INLINE]>;

/// Heap-allocated state for a built-in iterable wrapped by `iter()`.
/// Stored type-erased inside `Value::generator()` via `Box<dyn Any>`,
/// the same slot used for GeneratorFrame.  resume_generator() checks
/// which concrete type it has by downcasting.
pub(crate) struct NativeIterFrame {
    pub(crate) items: Vec<Value>,
    pub(crate) pos: usize,
}

/// Lazy iterator over `obj.__getitem__(0)`, `obj.__getitem__(1)`, …
/// implementing the legacy sequence-iter protocol (#394).  Stored
/// type-erased inside `Value::generator()` like [`NativeIterFrame`].
///
/// Each call to `next()` invokes `__getitem__(index)` with the current
/// `index`, bumps the counter on success, and terminates on a
/// subclass of `IndexError` or `StopIteration` raised from
/// `__getitem__`.  This is the *lazy* path: a `for x in obj: break`
/// only calls `obj.__getitem__(0)` once and never advances.
pub(crate) struct GetItemIter {
    /// The object whose `__getitem__` is being driven (kept alive by
    /// `Rc` cloning — the iterator can outlive any other reference).
    pub(crate) obj: Value,
    /// Cached `__getitem__` method value resolved at construction time,
    /// so the per-tick `lookup_class_attr` call is paid once rather
    /// than every `next()`.
    pub(crate) method: Value,
    /// Next integer index to pass to `__getitem__`.  Wraps at i64::MAX
    /// (impossible in practice — would take ~290 years at 1 ns/step).
    pub(crate) index: i64,
    /// Set once `__getitem__` has raised `IndexError`/`StopIteration`.
    /// Subsequent `next()` calls return StopIteration without invoking
    /// `__getitem__` again (matches CPython's iterator-exhaustion
    /// rule: once exhausted, always exhausted).
    pub(crate) exhausted: bool,
}

/// Heap-allocated execution state for a suspended generator.
/// Stored type-erased inside `Value::generator()` via `Box<dyn Any>`.
pub(crate) struct GeneratorFrame {
    pub(crate) code: Rc<crate::bytecode::FnCode>,
    pub(crate) regs: RegsBuf,
    pub(crate) iters: Vec<Option<IterState>>,
    pub(crate) exc_handlers: Vec<usize>,
    /// Program counter for the NEXT instruction to execute on resumption.
    pub(crate) pc: usize,
    pub(crate) done: bool,
    /// The environment (closure captures) active when the generator was created.
    pub(crate) saved_env: EnvRef,
    /// PEP 3134 per-frame exception state snapshot, persisted across a
    /// `yield` and re-installed on resume.  Stores the slice of
    /// `Interpreter::handled_exc_stack` that belongs to *this* generator
    /// frame (entries pushed above the caller's base depth at yield time).
    /// Without this, a generator that yields while inside an
    /// `except` / `finally` body would leak its handled-exception context
    /// onto the caller's `Interpreter` between `next()` calls.
    pub(crate) handled_exc_slice: Vec<Value>,
    /// The interpreter's `active_exception` at the moment of yield.
    /// Saved and restored alongside `handled_exc_slice` so that resume
    /// can re-establish the suspended frame's exception view without
    /// disturbing the caller's.
    pub(crate) active_exception: Option<Value>,
    /// Name -> fastlocal register slot for this generator's body, cloned
    /// from the originating `UserFunction`.  Published to
    /// `Interpreter::vm_frame_views` for the duration of each resume so
    /// that `locals()` invoked inside the generator body sees the
    /// generator's own fastlocals rather than the caller's frame
    /// (issue #483 review: PR #483 originally pushed function frame
    /// views only in `call_user_function_expanded`, leaving generators
    /// to fall back to the caller's view).
    pub(crate) local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
    /// The destination register of the most recent `Yield` instruction.
    /// On the next resumption this register receives the sent value
    /// (`Value::none()` for `next()`, the caller's argument for `send()`).
    /// Meaningless until the generator has yielded at least once (`pc != 0`).
    pub(crate) yield_dst: crate::bytecode::Reg,
    /// The value from an explicit `return val` inside this generator.
    /// Set by `resume_generator_with_exc` when `FrameOutcome::Returned(val)`
    /// is received, so that `YieldFrom` can capture `StopIteration.value`
    /// from the sub-iterator's frame.
    pub(crate) last_return_value: Option<Value>,
}

/// Explicit suspension state for a generator frame.
///
/// Replaces the old `GEN_SAVE` thread-local + `GenSaveState` tuple alias.
/// All suspension state is now carried as a struct field in `FrameOutcome::Yielded`
/// rather than smuggled through a side-channel.
pub(crate) struct GenSaveState {
    pub(crate) iters: Vec<Option<IterState>>,
    pub(crate) exc_handlers: Vec<usize>,
    pub(crate) pc: usize,
    pub(crate) handled_exc_slice: Vec<Value>,
    pub(crate) active_exception: Option<Value>,
    /// Destination register of the `Yield` that produced this suspension.
    /// Carried here so `resume_generator_with_exc` can persist it onto
    /// `GeneratorFrame::yield_dst` for the subsequent send() call.
    pub(crate) yield_dst: crate::bytecode::Reg,
}

/// Outcome of executing a generator frame (returned by `run_bytecode_inner` and
/// `run_bytecode_inner_impl`).
///
/// Generator outcomes are explicit values, not errors, except for StopIteration
/// which is the user-visible error contract.  Using a dedicated enum here avoids
/// the old pattern of abusing `Result::Err` as a control-flow channel for yield.
///
/// - `Returned(v)` — frame returned (fell off the end or hit `return`).
/// - `Yielded { value, saved }` — frame yielded; resume state is in `saved`.
///
/// For callers that don't execute generators (`run_bytecode`, `run_bytecode_for_fn`),
/// `Yielded` is unreachable and the `Returned` value is extracted directly.
pub(crate) enum FrameOutcome {
    Returned(Value),
    Yielded { value: Value, saved: GenSaveState },
}

#[derive(Clone)]
pub(crate) enum IterState {
    Materialized(Vec<Value>, usize),
    Range { cur: i64, stop: i64, step: i64 },
    /// Lazy: reads directly from the source register on each ForIter call.
    /// Avoids the O(n) upfront clone that Materialized would require for List/Tuple.
    /// Behaves like CPython's list_iterator: checks pos < len each tick; no
    /// mutation detection (appending extends iteration, removing shortens it).
    Indexed { reg: crate::bytecode::Reg, pos: usize },
    /// User-defined iterator: holds the iterator object (result of __iter__).
    /// Each ForIter call invokes __next__() on it and stops on StopIteration.
    UserDefined(Value),
}

fn int_int_fast(a: i64, b: i64, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add    => a.checked_add(b).map(Value::int),
        BinaryOp::Sub    => a.checked_sub(b).map(Value::int),
        BinaryOp::Mul    => a.checked_mul(b).map(Value::int),
        BinaryOp::BitAnd => Some(Value::int(a & b)),
        BinaryOp::BitOr  => Some(Value::int(a | b)),
        BinaryOp::BitXor => Some(Value::int(a ^ b)),
        BinaryOp::LShift => {
            if b < 0 {
                // Negative shift → ValueError; fall through to eval_binary.
                None
            } else if b >= 64 {
                // Shift count ≥ 64: result is BigInt (or 0 for a==0).
                // Fall through to eval_binary which handles BigInt promotion.
                None
            } else {
                let n = b as u32;
                // Shift left then shift right; if we get back the original
                // value no significant bits were lost and the result fits i64.
                let r = a.wrapping_shl(n);
                if r.wrapping_shr(n) == a {
                    Some(Value::int(r))
                } else {
                    // Overflow: fall through for BigInt promotion.
                    None
                }
            }
        }
        BinaryOp::RShift => {
            if b < 0 {
                // Negative shift → ValueError; fall through to eval_binary.
                None
            } else if b >= 64 {
                // Saturate to sign bit (0 for non-negative, -1 for negative).
                // This is safe to handle here without BigInt.
                Some(Value::int(if a < 0 { -1 } else { 0 }))
            } else {
                Some(Value::int(a >> b))
            }
        }
        BinaryOp::Eq  => Some(Value::bool_(a == b)),
        BinaryOp::Ne  => Some(Value::bool_(a != b)),
        BinaryOp::Lt  => Some(Value::bool_(a < b)),
        BinaryOp::Le  => Some(Value::bool_(a <= b)),
        BinaryOp::Gt  => Some(Value::bool_(a > b)),
        BinaryOp::Ge  => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

fn int_cmp(a: i64, b: i64, op: BinaryOp) -> Option<bool> {
    match op {
        BinaryOp::Eq => Some(a == b),
        BinaryOp::Ne => Some(a != b),
        BinaryOp::Lt => Some(a < b),
        BinaryOp::Le => Some(a <= b),
        BinaryOp::Gt => Some(a > b),
        BinaryOp::Ge => Some(a >= b),
        _ => None,
    }
}

/// One iteration step for `ForCount*` opcodes — dedupes the three
/// near-identical opcode arms (`ForCountReg`, `ForCountConst`,
/// `ForCountConstInline`) which differ only in where their `stop` /
/// `step` operands come from.  Expanded via `macro_rules!` so each
/// call site produces identical codegen to the original hand-written
/// per-opcode bodies (a `#[inline(always)] fn` was indistinguishable
/// in measurement — both forms produce the same code — but the macro
/// form is structurally clearer: no out-parameter or `Option<i64>`
/// dance, the writeback / loop-exit happens textually at the call
/// site).
///
/// `checked_add` covers the near-i64::MAX / i64::MIN boundary (#439):
/// on overflow the loop exits cleanly instead of wrapping past `stop`.
macro_rules! for_count_step {
    ($regs:ident, $var:expr, $cur:expr, $stop:expr, $step:expr, $cmp_op:expr, $pc:ident, $offset:expr) => {{
        let cont = match ($cur as i64).checked_add($step as i64) {
            Some(next) => {
                let c = match $cmp_op {
                    BinaryOp::Lt => next < ($stop as i64),
                    BinaryOp::Gt => next > ($stop as i64),
                    _ => unreachable!("ForCount* uses Lt or Gt only"),
                };
                if c {
                    $regs[$var as usize] = Value::int(next);
                }
                c
            }
            None => false,
        };
        if !cont {
            $pc = jump_pc!($offset);
        }
    }};
}

impl Interpreter {
    /// Execute compiled bytecode for a user function.
    ///
    /// `regs` must be pre-sized to `code.num_regs` with parameter slots already filled.
    /// Takes `RegSlice` (raw pointer + len) rather than `&mut [Value]` so that the
    /// `VmFrameView` raw pointer stored before this call does not alias an `&mut [Value]`
    /// that carries LLVM `noalias` (issue #547, PR #646 Copilot review).
    fn run_bytecode(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: RegSlice,
    ) -> Result<Value> {
        match self.run_bytecode_inner(
            code,
            regs,
            vec![None; code.num_iters as usize],
            Vec::new(),
            0,
            None,
            None,
            Vec::new(),
            None,
        )? {
            FrameOutcome::Returned(v) => Ok(v),
            FrameOutcome::Yielded { .. } => {
                unreachable!("run_bytecode called on a non-generator function")
            }
        }
    }

    /// Like `run_bytecode` but also passes the current function's id so that
    /// `TailCall` instructions can perform self-call detection.
    /// Takes `RegSlice` for the same aliasing-soundness reason as `run_bytecode`.
    fn run_bytecode_for_fn(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: RegSlice,
        fn_id: u64,
    ) -> Result<Value> {
        match self.run_bytecode_inner(
            code,
            regs,
            vec![None; code.num_iters as usize],
            Vec::new(),
            0,
            Some(fn_id),
            None,
            Vec::new(),
            None,
        )? {
            FrameOutcome::Returned(v) => Ok(v),
            FrameOutcome::Yielded { .. } => {
                unreachable!("run_bytecode_for_fn called on a non-generator function")
            }
        }
    }

    /// Resume (or initialise) a generator by executing from `frame.pc` until
    /// the next yield or completion.  The sent value is `None` (equivalent to
    /// `next(g)`).  Returns:
    /// - `Ok(val)`  — generator yielded `val`; frame updated in-place
    /// - `Err(Named("StopIteration", _))` — generator returned normally
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
    /// - `Err(Named("StopIteration", _))` — generator returned normally
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
            let exc = if let Some(cls) = self.exc_classes.get("StopIteration") {
                PyError::Raised(instantiate_exception(cls, vec![]))
            } else {
                PyError::named("StopIteration", String::new())
            };
            return Err(exc);
        }

        // PEP 380 throw forwarding: if we are suspended at a YieldFrom instruction
        // and an exception is being injected (generator.throw() / generator.close()),
        // forward the exception to the sub-iterator rather than injecting it into our
        // own body.  This matches CPython's implementation of the PEP 380 algorithm.
        if let Some(ref exc) = inject_exc {
            if let Some(crate::bytecode::Insn::YieldFrom { iter_reg, sent_reg, result_reg }) =
                frame.code.insns.get(frame.pc)
            {
                let iter_reg = *iter_reg;
                let sent_reg = *sent_reg;
                let result_reg = *result_reg;
                let iter_val = frame.regs[iter_reg as usize].clone();
                let forward_result =
                    self.yield_from_throw_forward(&iter_val, exc.clone());
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
                        frame.regs[result_reg as usize] =
                            stop_val.unwrap_or_else(Value::none);
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
        });
        // SAFETY: regs_ptr is valid for regs_len Values for the lifetime of
        // frame.regs (which outlives this call).  No &mut [Value] referencing
        // frame.regs is held while the dispatch loop runs; RegSlice (raw
        // pointer + len) is used instead, removing the LLVM noalias constraint
        // that made the VmFrameView dereferences UB (issue #547).
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        let result = self.run_bytecode_inner(
            &frame.code.clone(),
            regs_slice,
            std::mem::take(&mut frame.iters),
            std::mem::take(&mut frame.exc_handlers),
            frame.pc,
            None,
            inject_exc,
            gen_handled,
            gen_active,
        );
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
                frame.yield_dst = saved.yield_dst;
                Ok(value)
            }
            Ok(FrameOutcome::Returned(ret_val)) => {
                // Generator returned normally (fell off end or hit explicit `return`).
                // Stash the return value so Insn::YieldFrom can extract it as
                // StopIteration.value (PEP 380 §3 step 4).
                frame.last_return_value = Some(ret_val.clone());
                frame.done = true;
                let exc = if let Some(cls) = self.exc_classes.get("StopIteration") {
                    PyError::Raised(instantiate_exception(cls, vec![ret_val]))
                } else {
                    PyError::named("StopIteration", String::new())
                };
                Err(exc)
            }
            Err(e) => {
                // Propagating exception or other error.
                frame.done = true;
                // PEP 479 (enforced since Python 3.7): if a StopIteration (or
                // any subclass) escapes from a generator frame, convert it to
                // RuntimeError("generator raised StopIteration").  The original
                // exception becomes the __cause__ of the RuntimeError.
                Err(pep479_wrap_stop_iteration(&self.env, e))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_inner(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: RegSlice,
        iters_init: Vec<Option<IterState>>,
        exc_handlers_init: Vec<usize>,
        start_pc: usize,
        current_fn_id: Option<u64>,
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
        // `split_off(exc_ctx_frame_base)`.  Pass `Vec::new()` and `None`
        // for fresh, non-generator invocations.
        gen_handled_slice: Vec<Value>,
        gen_active_exception: Option<Value>,
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
        let saved_active = self.active_exception.clone();

        let result = self.run_bytecode_inner_impl(
            code,
            regs,
            iters_init,
            exc_handlers_init,
            start_pc,
            current_fn_id,
            inject_exc,
            gen_handled_slice,
            gen_active_exception,
        );

        // For `Yielded`, the `Yield` opcode already stripped the generator's
        // exc-stack entries and cleared `active_exception` before returning, so
        // `handled_exc_stack` is already at `exc_ctx_entry_depth` on that path.
        // `truncate` is idempotent so it's safe to call in every branch.
        self.handled_exc_stack.truncate(exc_ctx_entry_depth);
        self.active_exception = saved_active;
        result
    }

    /// Materialise a `PyError` into a Python exception value and route it
    /// through the active handler stack.
    ///
    /// Returns `Ok(handler_pc)` when the error is caught — the caller sets
    /// `pc = handler_pc` and continues the VM loop.  Returns `Err(e)` when
    /// no handler is in scope; the caller propagates upward.
    ///
    /// `#[cold]` + `#[inline(never)]` keep the large match and all its
    /// temporaries in a separate Rust frame, preventing them from inflating
    /// `run_bytecode_inner_impl`'s debug-mode stack frame at every `vm_try!`
    /// expansion site.
    #[cold]
    #[inline(never)]
    fn handle_vm_error(
        &mut self,
        e: PyError,
        exc_handlers: &mut Vec<usize>,
        exc_ctx_frame_base: usize,
    ) -> std::result::Result<usize, PyError> {
        let Some(h) = exc_handlers.pop() else {
            return Err(e);
        };
        let exc_val = match e {
            PyError::Raised(v) => v,
            PyError::Runtime(msg) => {
                if let Some(cls) = self.exc_classes.get("RuntimeError") {
                    instantiate_exception(cls, vec![Value::string(msg)])
                } else {
                    match self.instantiate_named_exception("RuntimeError", msg) {
                        Ok(v) => v,
                        Err(e2) => return Err(e2),
                    }
                }
            }
            PyError::Named(cls, msg) => {
                match self.instantiate_named_exception(&cls, msg) {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            PyError::Class(cls, msg) => {
                instantiate_exception(cls, vec![Value::string(msg)])
            }
            PyError::KeyError(key) => {
                match self.instantiate_named_exception_with_value("KeyError", key) {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            other => return Err(other),
        };
        self.attach_implicit_context(&exc_val);
        if self.handled_exc_stack.len() > exc_ctx_frame_base
            && let Some(top) = self.handled_exc_stack.last()
            && let Some(active) = self.active_exception.as_ref()
            && Self::values_are_same_exception(top, active)
        {
            self.handled_exc_stack.pop();
        }
        self.handled_exc_stack.push(exc_val.clone());
        self.active_exception = Some(exc_val);
        Ok(h)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_inner_impl(
        &mut self,
        code: &crate::bytecode::FnCode,
        mut regs: RegSlice,
        iters_init: Vec<Option<IterState>>,
        exc_handlers_init: Vec<usize>,
        start_pc: usize,
        current_fn_id: Option<u64>,
        inject_exc: Option<PyError>,
        // Generator-only: persisted slice + active to push AFTER we've
        // captured `exc_ctx_frame_base`, so they sit strictly above the
        // caller's stack entries and are owned by this frame.
        gen_handled_slice: Vec<Value>,
        gen_active_exception: Option<Value>,
    ) -> Result<FrameOutcome> {
        use crate::bytecode::Insn;
        
        let num_locals = code.num_locals;

        let mut iters: Vec<Option<IterState>> = iters_init;
        let mut exc_handlers: Vec<usize> = exc_handlers_init;
        let mut pc: usize = start_pc;
        // Counts self-tail-call iterations so that infinite tail recursion
        // eventually raises RecursionError instead of looping forever.
        let mut tco_iters: usize = 0;
        let mut pending_inject: Option<PyError> = inject_exc;

        // Depth of `handled_exc_stack` belonging to *caller* frames at the
        // time this VM frame started executing.  Used by `vm_try!` to bound
        // its "propagating out of a handler body" pop so it never reaches
        // into the caller's entries.
        let exc_ctx_frame_base: usize = self.handled_exc_stack.len();

        // PEP 3134: re-install the generator's persisted slice of
        // `handled_exc_stack` and its `active_exception` AFTER fixing the
        // frame base.  These entries are now owned by this frame, and the
        // next `Yield` opcode's `split_off(exc_ctx_frame_base)` will
        // collect them again (plus anything new) into
        // `frame.handled_exc_slice`.  No-op for fresh, non-generator calls.
        if !gen_handled_slice.is_empty() {
            self.handled_exc_stack.extend(gen_handled_slice);
        }
        if gen_active_exception.is_some() {
            self.active_exception = gen_active_exception;
        }

        'vm: loop {
        // Dispatch errors through the active exception handler stack.
        // Defined inside the loop so `continue 'vm` resolves to this loop.
        macro_rules! vm_try {
            ($expr:expr) => {
                match $expr {
                    Ok(v) => v,
                    Err(e) => match self.handle_vm_error(e, &mut exc_handlers, exc_ctx_frame_base) {
                        Ok(h) => { pc = h; continue 'vm; }
                        Err(e) => return Err(e),
                    },
                }
            };
        }
        macro_rules! pool_get {
            ($pool:expr, $idx:expr, $tag:literal) => {
                match ($pool).get($idx as usize) {
                    Some(v) => v,
                    None => {
                        vm_try!(Err(PyError::Runtime(format!(
                            "bytecode error: {} index {} out of range (pool size {})",
                            $tag, $idx, ($pool).len()
                        ))));
                        unreachable!()
                    }
                }
            };
        }
            // Inject a pending exception (set by resume_generator_with_exc for
            // generator.close()/throw()) before dispatching the next
            // instruction.  Routes through the existing handler stack so that
            // try/except/finally inside the generator can observe the throw.
            if let Some(e) = pending_inject.take() {
                vm_try!(Err::<(), _>(e));
            }
            let Some(insn) = code.insns.get(pc) else {
                if pc == code.insns.len() {
                    return Ok(FrameOutcome::Returned(Value::none()));
                }
                return Err(PyError::Runtime(format!(
                    "internal error: PC {} out of bounds (insns len {})",
                    pc,
                    code.insns.len()
                )));
            };
            pc += 1;

            macro_rules! jump_pc {
                ($offset:expr) => {{
                    let new_pc = pc as i64 + $offset as i64;
                    if new_pc < 0 || new_pc as usize > code.insns.len() {
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
                    if let ValueKind::Int(n) = cv.kind() {
                        regs[*dst as usize] = Value::int(n);
                    } else {
                        regs[*dst as usize] = cv.clone();
                    }
                }
                Insn::LoadGlobal(dst, name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name");
                    let val = if let Some(v) = vm_try!(self.lookup_name(name)) {
                        v
                    } else if let Some(v) = self
                        .vm_frame_views
                        .iter()
                        .find(|v| v.kind == FrameKind::Script)
                        .and_then(|script_view| {
                            let slot = *script_view.local_index.get(name)?;
                            let slot = slot as usize;
                            if slot >= script_view.regs_len {
                                return None;
                            }
                            // SAFETY: `script_view.regs_ptr` is a NonNull
                            // pointer to the script frame's register file.
                            // The script frame's dispatch loop uses `RegSlice`
                            // (not `&mut [Value]`), so no LLVM `noalias`
                            // annotation is live on the allocation — forming
                            // `&Value` here does not violate aliasing rules
                            // (issue #547, fixed in PR #646).  `slot < regs_len`
                            // is checked above; `as_ref()` yields `&Value` for
                            // the duration of the `.clone()` call only.
                            let v = unsafe { script_view.regs_ptr.add(slot).as_ref() };
                            if v.is_unset() { None } else { Some(v.clone()) }
                        })
                    {
                        v
                    } else {
                        vm_try!(resolve_builtin(name).ok_or_else(|| {
                            PyError::named(
                                "NameError",
                                format!("name '{}' is not defined", name),
                            )
                        }))
                    };
                    regs[*dst as usize] = val;
                }
                Insn::StoreGlobal(name_idx, src) => {
                    let name = pool_get!(code.names, *name_idx, "name").clone();
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    self.assign_name(name, val);
                }
                Insn::LoadNone(dst) => {
                    regs[*dst as usize] = Value::none();
                }
                Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
                    let v = vm_try!(vm_read(&regs, *src, num_locals));
                    regs[*dst as usize] = v;
                }

                // ── Arithmetic / Logic ───────────────────────────────────
                Insn::BinOp(dst, lhs, op, rhs) => {
                    // Hot path: `as_int()` is a tagged-u64 check that
                    // bypasses `kind()`'s scoped RefCell borrow for the
                    // List/Dict/Set kinds (#450).  Unlike `Insn::Move`
                    // (where #441 showed the int specialization is a
                    // wash), the BinOp fast path also short-circuits the
                    // entire `eval_binary` dispatch for int–int ops, so
                    // the savings are real.
                    if let (Some(a), Some(b)) = (
                        regs[*lhs as usize].as_int(),
                        regs[*rhs as usize].as_int(),
                    ) && let Some(result) = int_int_fast(a, b, *op)
                    {
                        regs[*dst as usize] = result;
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(&regs, *rhs, num_locals));
                    regs[*dst as usize] = vm_try!(self.eval_binary(l, *op, r));
                }
                Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                    if let (Some(a), Some(b)) = (
                        regs[*lhs as usize].as_int(),
                        regs[*rhs as usize].as_int(),
                    ) && let Some(result) = int_int_fast(a, b, *op)
                    {
                        regs[*dst as usize] = result;
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(&regs, *rhs, num_locals));
                    let result = if let Some(v) = vm_try!(self.try_inplace_op(l.clone(), *op, r.clone())) {
                        v
                    } else {
                        vm_try!(self.eval_binary(l, *op, r))
                    };
                    regs[*dst as usize] = result;
                }
                Insn::BinOpConst(dst, lhs, op, const_idx) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let (Some(a), Some(b)) = (
                        regs[*lhs as usize].as_int(),
                        cv.as_int(),
                    ) && let Some(result) = int_int_fast(a, b, *op)
                    {
                        regs[*dst as usize] = result;
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = cv.clone();
                    let result = if let Some(v) = vm_try!(self.try_inplace_op(l.clone(), *op, r.clone())) {
                        v
                    } else {
                        vm_try!(self.eval_binary(l, *op, r))
                    };
                    regs[*dst as usize] = result;
                }
                Insn::UnaryOp(dst, op, src) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    let result = if *op == UnaryOp::Not {
                        // Dispatch __bool__ for instances before falling back to truthy().
                        Value::bool_(!vm_try!(self.truthy_value(&val)))
                    } else {
                        // Try dunder methods on PyInstance before the built-in path.
                        let dunder = match op {
                            UnaryOp::Neg => Some("__neg__"),
                            UnaryOp::Pos => Some("__pos__"),
                            UnaryOp::BitNot => Some("__invert__"),
                            UnaryOp::Not => None,
                        };
                        if let Some(dunder_name) = dunder {
                            if let Some(r) = self.try_dunder_unary(&val, dunder_name) {
                                vm_try!(r)
                            } else {
                                vm_try!(vm_eval_unary(*op, val))
                            }
                        } else {
                            vm_try!(vm_eval_unary(*op, val))
                        }
                    };
                    regs[*dst as usize] = result;
                }

                // ── Attribute / Index ────────────────────────────────────
                Insn::GetAttr(dst, obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    let result = vm_try!(self.get_attr(obj_val, name));
                    regs[*dst as usize] = result;
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let val_val = vm_try!(vm_read(&regs, *val, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.assign_attr(obj_val, name, val_val));
                }
                Insn::DeleteAttr(obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.delete_attr(obj_val, name));
                }
                Insn::GetItem(dst, obj, idx) => {
                    // Fast path: List/Tuple indexed by Int — borrow idx, avoid clone.
                    // `as_int()` bypasses `kind()`'s Ref machinery on the
                    // hot index-extraction path (#450 perf).
                    let fast_int_idx = regs[*idx as usize].as_int();

                    if let Some(raw_i) = fast_int_idx {
                        // Extract the indexed element in a scoped block so
                        // the `kind()` Ref drops before we assign to
                        // `regs[dst]` (#450).
                        enum Got {
                            Item(Value),
                            ListOOR,
                            TupleOOR,
                            None,
                        }
                        let got = match regs[*obj as usize].as_some().map(|v| v.kind()) {
                            Some(ValueKind::List(items)) => {
                                let len = items.len() as i64;
                                let j = if raw_i < 0 { raw_i + len } else { raw_i };
                                if j >= 0 && (j as usize) < items.len() {
                                    Got::Item(items[j as usize].clone())
                                } else {
                                    Got::ListOOR
                                }
                            }
                            Some(ValueKind::Tuple(items)) => {
                                let len = items.len() as i64;
                                let j = if raw_i < 0 { raw_i + len } else { raw_i };
                                if j >= 0 && (j as usize) < items.len() {
                                    Got::Item(items[j as usize].clone())
                                } else {
                                    Got::TupleOOR
                                }
                            }
                            _ => Got::None,
                        };
                        let mut handled = false;
                        match got {
                            Got::Item(v) => {
                                regs[*dst as usize] = v;
                                handled = true;
                            }
                            Got::ListOOR => {
                                vm_try!(Err(PyError::named(
                                    "IndexError",
                                    "list index out of range",
                                )));
                            }
                            Got::TupleOOR => {
                                vm_try!(Err(PyError::named(
                                    "IndexError",
                                    "tuple index out of range",
                                )));
                            }
                            Got::None => {}
                        }
                        if handled { continue; }
                    }

                    let idx_val = vm_try!(vm_read(&regs, *idx, num_locals));
                    // Slice key: tuple of (lo, hi, step) produced by the compiler.
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                        let result = vm_try!(self.eval_slice(obj_val, lo, hi, st));
                        regs[*dst as usize] = result;
                    } else {
                        // Fast path: read directly from the register without cloning
                        // the entire collection (avoids O(n) clone per GetItem call).
                        // The Dict arm is special: looking up an instance key may run
                        // user `__hash__`/`__eq__`, which requires `&mut self`.  We
                        // Rc-clone the dict Value (cheap bump, no IndexMap clone) so
                        // we can drop the register borrow before reentering `self`.
                        enum FastResult {
                            Value(Value),
                            DictLookup(Value),
                            Miss,
                        }
                        let fast = if let Some(ov) = regs[*obj as usize].as_some() {
                            match ov.kind() {
                                ValueKind::List(items) => {
                                    let i = vm_try!(normalize_index(&idx_val, items.len(), "list"));
                                    FastResult::Value(items[i].clone())
                                }
                                ValueKind::Tuple(items) => {
                                    let i = vm_try!(normalize_index(&idx_val, items.len(), "tuple"));
                                    FastResult::Value(items[i].clone())
                                }
                                ValueKind::Dict(_) => FastResult::DictLookup(ov.clone()),
                                _ => FastResult::Miss,
                            }
                        } else { FastResult::Miss };
                        match fast {
                            FastResult::Value(r) => {
                                regs[*dst as usize] = r;
                            }
                            FastResult::DictLookup(dict_val) => {
                                // Fast path for string keys (issue #506): use
                                // `dict_str_lookup` so we probe the map with
                                // `StrKey` and avoid allocating a
                                // `PyKey::Str(String)`.
                                let lookup = if let Some(s) = idx_val.as_str() {
                                    vm_try!(self.dict_str_lookup(&dict_val, s))
                                } else {
                                    let key = vm_try!(self.value_to_pykey(&idx_val));
                                    vm_try!(self.dict_lookup(&dict_val, &key))
                                };
                                let r = vm_try!(
                                    lookup
                                        .map(|(_, v)| v)
                                        .ok_or_else(|| PyError::key_error(idx_val.clone()))
                                );
                                regs[*dst as usize] = r;
                            }
                            FastResult::Miss => {
                                let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                                let r = vm_try!(self.eval_index(obj_val, idx_val));
                                regs[*dst as usize] = r;
                            }
                        }
                    }
                }
                Insn::SetItem(obj, idx, val) => {
                    let idx_val = vm_try!(vm_read(&regs, *idx, num_locals));
                    let val_val = vm_try!(vm_read(&regs, *val, num_locals));
                    // Slice assignment: tuple key on a list.
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        // Drop the kind() Ref before the fallback path
                        // may move `val_val` into `collect_iterable`
                        // (#450).
                        let new_items: Vec<Value> = match val_val.kind() {
                            ValueKind::List(v) => Some(v.to_vec()),
                            _ => None,
                        }
                        .unwrap_or_else(|| Vec::new());
                        let new_items = if !new_items.is_empty()
                            || matches!(val_val.kind(), ValueKind::List(_))
                        {
                            new_items
                        } else {
                            vm_try!(self.collect_iterable(val_val.clone()).map_err(|_| {
                                PyError::named(
                                    "TypeError",
                                    "can only assign an iterable".to_string(),
                                )
                            }))
                        };
                        let updated = regs[*obj as usize].list_with_mut(|items| {
                            Self::slice_setitem(
                                items,
                                lo.as_ref(),
                                hi.as_ref(),
                                st.as_ref(),
                                new_items,
                            )
                        });
                        match updated {
                            Some(r) => vm_try!(r),
                            None => vm_try!(Err(PyError::Runtime(
                                "object does not support slice assignment".to_string(),
                            ))),
                        }
                    } else {
                        // Non-slice set: determine target type first, then mutate
                        let target_kind = regs[*obj as usize].as_some().map(|v| match v.kind() {
                            ValueKind::List(_) => 1u8,
                            ValueKind::Dict(_) => 2u8,
                            ValueKind::PyInstance(_) => 3u8,
                            ValueKind::BuiltinObject { .. } => 4u8,
                            _ => 0u8,
                        }).unwrap_or(0);
                        match target_kind {
                            1 => {
                                let len = regs[*obj as usize].list_len().unwrap_or(0);
                                let i = vm_try!(normalize_index(&idx_val, len, "list"));
                                regs[*obj as usize].list_with_mut(|items| {
                                    items[i] = val_val;
                                });
                            }
                            2 => {
                                // For PyInstance keys we need `__hash__` (and possibly
                                // `__eq__`), both of which require `&mut self`.  Compute
                                // the key first, then take the dict-mut borrow.
                                let key = vm_try!(self.value_to_pykey(&idx_val));
                                if matches!(&key, PyKey::Object { .. }) {
                                    // Object keys may need `__eq__` dispatch to dedup
                                    // against an existing entry.  Rc-clone the dict
                                    // Value (cheap) so `dict_lookup` can run with no
                                    // live alias into the register file; if an entry
                                    // matches, replace its value in place to preserve
                                    // insertion order.
                                    let dict_val = regs[*obj as usize]
                                        .as_some()
                                        .cloned()
                                        .unwrap_or(Value::none());
                                    let existing = vm_try!(self.dict_lookup(&dict_val, &key));
                                    regs[*obj as usize].dict_with_mut(|dict| {
                                        if let Some((idx, _)) = existing {
                                            let existing_key =
                                                dict.get_index(idx).map(|(k, _)| k.clone());
                                            if let Some(k) = existing_key {
                                                dict.insert(k, val_val);
                                            } else {
                                                dict.insert(key, val_val);
                                            }
                                        } else {
                                            dict.insert(key, val_val);
                                        }
                                    });
                                } else {
                                    regs[*obj as usize].dict_with_mut(|dict| {
                                        dict.insert(key, val_val);
                                    });
                                }
                            }
                            3 => {
                                let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                                if let ValueKind::PyInstance(inst) = obj_val.kind() {
                                    let inst_rc = Rc::clone(inst);
                                    let class = Rc::clone(&inst_rc.borrow().class);
                                    if let Some(method_val) =
                                        lookup_class_attr(&class, "__setitem__")
                                    {
                                        vm_try!(invoke_class_method(
                                            self,
                                            method_val,
                                            Value::py_instance(inst_rc),
                                            &[
                                                ExpandedCallArg { name: None, value: idx_val },
                                                ExpandedCallArg { name: None, value: val_val },
                                            ],
                                        ));
                                        continue;
                                    }
                                }
                                vm_try!(Err(PyError::named(
                                    "TypeError",
                                    "object does not support item assignment".to_string(),
                                )));
                            }
                            4 => {
                                // BuiltinObject (Counter, …) — route through
                                // `BuiltinTypeOps::set_item`.  The default
                                // impl returns a TypeError shaped like the
                                // dict-fallback message, so non-mutating
                                // types don't need extra plumbing.
                                let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                                if let ValueKind::BuiltinObject { ops, state } = obj_val.kind() {
                                    vm_try!(ops.set_item(state, &idx_val, val_val));
                                } else {
                                    vm_try!(Err(PyError::Runtime(
                                        "internal: BuiltinObject kind probe drifted".to_string(),
                                    )));
                                }
                            }
                            _ => {
                                vm_try!(Err(PyError::Runtime(
                                    "object does not support item assignment".to_string(),
                                )));
                            }
                        }
                    }
                }
                Insn::DeleteItem(obj, idx) => {
                    let idx_val = vm_try!(vm_read(&regs, *idx, num_locals));
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        let updated = regs[*obj as usize].list_with_mut(|items| {
                            Self::slice_delitem(
                                items,
                                lo.as_ref(),
                                hi.as_ref(),
                                st.as_ref(),
                            )
                        });
                        match updated {
                            Some(r) => vm_try!(r),
                            None => vm_try!(Err(PyError::Runtime(
                                "object does not support slice deletion".to_string(),
                            ))),
                        }
                    } else {
                        let mut handled = false;
                        // Tag the target type without holding a long-lived borrow so
                        // we can call `&mut self` helpers below for instance keys.
                        let target_kind = regs[*obj as usize].as_some().map(|v| match v.kind() {
                            ValueKind::List(_) => 1u8,
                            ValueKind::Dict(_) => 2u8,
                            ValueKind::BuiltinObject { .. } => 3u8,
                            _ => 0u8,
                        }).unwrap_or(0);
                        if target_kind == 1 {
                            let len = regs[*obj as usize].list_len().unwrap_or(0);
                            let i = vm_try!(normalize_index(&idx_val, len, "list"));
                            regs[*obj as usize].list_with_mut(|items| {
                                if i + 1 == items.len() {
                                    items.pop();
                                } else {
                                    items.remove(i);
                                }
                            });
                            handled = true;
                        } else if target_kind == 2 {
                            let key = vm_try!(self.value_to_pykey(&idx_val));
                            // For object keys, resolve via user-eq match first
                            // (Rc-clone the dict Value rather than snapshotting
                            // the IndexMap), then shift-remove by the located
                            // index for O(1)-ish removal.
                            if let PyKey::Object { .. } = &key {
                                let dict_val = regs[*obj as usize]
                                    .as_some()
                                    .cloned()
                                    .unwrap_or(Value::none());
                                let found = vm_try!(self.dict_lookup(&dict_val, &key));
                                if let Some((idx, _)) = found {
                                    regs[*obj as usize].dict_with_mut(|dict| {
                                        dict.shift_remove_index(idx);
                                    });
                                }
                            } else {
                                regs[*obj as usize].dict_with_mut(|dict| {
                                    dict.shift_remove(&key);
                                });
                            }
                            handled = true;
                        } else if target_kind == 3 {
                            // BuiltinObject — delegate to ops.delete_item.  The
                            // default BuiltinTypeOps impl raises TypeError, so
                            // immutable types like mappingproxy don't need extra
                            // plumbing.
                            let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                            if let ValueKind::BuiltinObject { ops, state } = obj_val.kind() {
                                vm_try!(ops.delete_item(state, &idx_val));
                            }
                            handled = true;
                        }
                        if !handled {
                            // Try __delitem__ on user-defined instances.
                            let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                            if let ValueKind::PyInstance(inst) = obj_val.kind() {
                                let inst_rc = Rc::clone(inst);
                                let class = Rc::clone(&inst_rc.borrow().class);
                                if let Some(method_val) =
                                    lookup_class_attr(&class, "__delitem__")
                                {
                                    vm_try!(invoke_class_method(
                                        self,
                                        method_val,
                                        Value::py_instance(inst_rc),
                                        &[ExpandedCallArg { name: None, value: idx_val }],
                                    ));
                                    continue;
                                }
                                let class_name = class.borrow().name.clone();
                                vm_try!(Err(PyError::named(
                                    "TypeError",
                                    format!("'{class_name}' object doesn't support item deletion"),
                                )));
                            }
                            vm_try!(Err(PyError::named(
                                "TypeError",
                                "object does not support item deletion".to_string(),
                            )));
                        }
                    }
                }
                Insn::DeleteName(name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name").clone();
                    let is_global = self.env.borrow().global_names.contains(&name);
                    if is_global {
                        // For `global x; del x` inside a function: target the module
                        // env, not the function's local env.  Raise NameError if the
                        // name is absent (matches CPython 3.12 behaviour).
                        let me = module_env(&self.env);
                        if me.borrow_mut().values.remove(&name).is_none() {
                            vm_try!(Err(PyError::named(
                                "NameError",
                                format!("name '{}' is not defined", name),
                            )));
                        }
                        // Also clear the module-level fastlocal register so the
                        // write-back loop in `program.rs` does not re-insert the
                        // stale value.  Mirrors the write-through that `assign_name`
                        // does for `StoreGlobal` (#520).
                        // SAFETY: `script_view.regs_ptr` points to the script
                        // frame's register file.  The script frame's dispatch
                        // loop uses `RegSlice` (not `&mut [Value]`), so no LLVM
                        // `noalias` annotation covers the allocation — writing
                        // through `NonNull::add(slot).as_mut()` does not violate
                        // aliasing rules (issue #547, fixed in PR #646).
                        // `slot < regs_len` is verified by the inner `if`.
                        if let Some(script_view) = self
                            .vm_frame_views
                            .iter()
                            .find(|v| v.kind == FrameKind::Script)
                        {
                            if let Some(&slot) = script_view.local_index.get(&name) {
                                let slot = slot as usize;
                                if slot < script_view.regs_len {
                                    unsafe {
                                        *script_view.regs_ptr.add(slot).as_mut() = Value::unset();
                                    }
                                }
                            }
                        }
                    } else {
                        // Module-scope `del x` (or non-global local): remove from
                        // current env.  Raise NameError if not present.
                        if self.env.borrow_mut().values.remove(&name).is_none() {
                            vm_try!(Err(PyError::named(
                                "NameError",
                                format!("name '{}' is not defined", name),
                            )));
                        }
                    }
                }
                Insn::DeleteLocal(reg) => {
                    regs[*reg as usize] = Value::unset();
                }

                // ── Control flow ─────────────────────────────────────────
                Insn::Jump(offset) => {
                    pc = jump_pc!(*offset);
                }
                Insn::JumpIfFalse(cond, offset) => {
                    let fast = if let Some(cv) = regs[*cond as usize].as_some() {
                        match cv.kind() {
                            ValueKind::Int(n)  => { if n == 0 { pc = jump_pc!(*offset); } true }
                            ValueKind::Bool(b) => { if !b    { pc = jump_pc!(*offset); } true }
                            _ => false,
                        }
                    } else { false };
                    if fast { continue; }
                    let cond_val = vm_try!(vm_read(&regs, *cond, num_locals));
                    if !vm_try!(self.truthy_value(&cond_val)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::JumpIfTrue(cond, offset) => {
                    let fast = if let Some(cv) = regs[*cond as usize].as_some() {
                        match cv.kind() {
                            ValueKind::Int(n)  => { if n != 0 { pc = jump_pc!(*offset); } true }
                            ValueKind::Bool(b) => { if b      { pc = jump_pc!(*offset); } true }
                            _ => false,
                        }
                    } else { false };
                    if fast { continue; }
                    let cond_val = vm_try!(vm_read(&regs, *cond, num_locals));
                    if vm_try!(self.truthy_value(&cond_val)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::CmpJumpIfFalse(lhs, op, rhs, offset) => {
                    let lv = &regs[*lhs as usize];
                    let rv = &regs[*rhs as usize];
                    if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind())
                        && let Some(cond) = int_cmp(a, b, *op) {
                                if !cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(&regs, *rhs, num_locals));
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfTrue(lhs, op, rhs, offset) => {
                    let lv = &regs[*lhs as usize];
                    let rv = &regs[*rhs as usize];
                    if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind())
                        && let Some(cond) = int_cmp(a, b, *op) {
                                if cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(&regs, *rhs, num_locals));
                    if vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfFalseConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let Some(lv) = regs[*lhs as usize].as_some()
                        && let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), cv.kind())
                            && let Some(cond) = int_cmp(a, b, *op) {
                                if !cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = cv.clone();
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfTrueConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let Some(lv) = regs[*lhs as usize].as_some()
                        && let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), cv.kind())
                            && let Some(cond) = int_cmp(a, b, *op) {
                                if cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = cv.clone();
                    if vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }

                // ── Exception handling ───────────────────────────────────
                Insn::SetupExcept(offset) => {
                    exc_handlers.push(jump_pc!(*offset));
                }
                Insn::PopExcept => {
                    exc_handlers.pop();
                }
                Insn::LoadExc(dst) => {
                    let exc = vm_try!(self.active_exception.clone().ok_or_else(|| {
                        PyError::Runtime("no active exception".to_string())
                    }));
                    regs[*dst as usize] = exc;
                }
                Insn::MatchExcept(type_reg, offset) => {
                    let type_val = vm_try!(vm_read(&regs, *type_reg, num_locals));
                    let exc = vm_try!(self.active_exception.clone().ok_or_else(|| {
                        PyError::Runtime(
                            "internal error: MatchExcept with no active exception".to_string(),
                        )
                    }));
                    if !vm_try!(self.exception_matches(&exc, &type_val)) {
                        pc = jump_pc!(*offset);
                    }
                    // No stack push on match: the dispatch already pushed
                    // the exception onto `handled_exc_stack` when vm_try!
                    // routed us here, so MatchExcept is purely a filter.
                }
                Insn::EndExcept => {
                    // Leaving an `except` handler body — pop the entry
                    // that vm_try! pushed on dispatch.  Restore
                    // `active_exception` to the outer handler's exception
                    // (if any), bounded by this frame's base depth so we
                    // never pop into caller-frame entries.
                    if self.handled_exc_stack.len() > exc_ctx_frame_base {
                        self.handled_exc_stack.pop();
                    }
                    self.active_exception = self.handled_exc_stack.last().cloned();
                }
                Insn::RaiseAssert(msg_reg) => {
                    let msg = vm_try!(vm_read(&regs, *msg_reg, num_locals));
                    let msg_str = if msg.is_none() {
                        String::new()
                    } else {
                        msg.to_py_str()
                    };
                    let exc = if let Some(cls) = self.exc_classes.get("AssertionError") {
                        instantiate_exception(cls, vec![Value::string(msg_str)])
                    } else {
                        vm_try!(self.instantiate_named_exception("AssertionError", msg_str))
                    };
                    self.attach_implicit_context(&exc);
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseValue(src) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    let exc = vm_try!(self.coerce_to_exception(val));
                    self.attach_implicit_context(&exc);
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseFrom(src, cause_reg) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    let cause = vm_try!(vm_read(&regs, *cause_reg, num_locals));
                    let exc = vm_try!(self.coerce_to_exception(val));
                    // PEP 3134: `raise X from Y` sets `__cause__` AND
                    // `__suppress_context__`, but `__context__` is still
                    // populated so that the chain is observable.
                    self.attach_implicit_context(&exc);
                    if let ValueKind::PyInstance(inst) = exc.kind() {
                        inst.borrow_mut().attrs.insert("__cause__".to_string(), cause);
                        inst.borrow_mut().attrs.insert("__suppress_context__".to_string(), Value::bool_(true));
                    }
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseReRaise => {
                    let exc = vm_try!(self.active_exception.clone().ok_or_else(|| {
                        PyError::Runtime("no active exception to re-raise".to_string())
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
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }

                // ── Calls ────────────────────────────────────────────────
                Insn::Call(func_reg, argc) => {
                    let func_val = vm_try!(vm_read(&regs, *func_reg, num_locals));
                    // Fast path for id(x): read the pool pointer directly from the
                    // register without cloning.  Cloning a list/tuple/str creates a
                    // new allocation, so the pointer seen inside call_function_expanded
                    // would differ from the original object's address.
                    if *argc == 1
                        && let ValueKind::BuiltinFunction("id") = func_val.kind() {
                            let maybe_id: Option<i64> = regs
                                .get((*func_reg + 1) as usize)
                                .and_then(|v| v.value_id());
                            if let Some(id_val) = maybe_id {
                                regs[*func_reg as usize] = Value::int(id_val);
                                continue 'vm;
                            }
                        }
                    // Reuse the interpreter-level buffer to avoid a per-call heap
                    // allocation in the common (non-recursive) case.
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    for i in 0..crate::bytecode::Reg::from(*argc) {
                        buf.push(ExpandedCallArg {
                            name: None,
                            value: vm_try!(vm_read(&regs, *func_reg + 1 + i, num_locals)),
                        });
                    }
                    let call_result = self.call_function_expanded(func_val, &buf);
                    self.call_arg_buf = buf;
                    regs[*func_reg as usize] = vm_try!(call_result);
                }

                Insn::CallMemo(func_reg, argc) => {
                    // Cache-first path for known-pure callees.
                    let is_pure_fn = if let Some(fv) = regs[*func_reg as usize].as_some() {
                        if let ValueKind::UserFunction(func) = fv.kind() {
                            func.is_pure
                        } else { false }
                    } else { false };

                    // Extract `fn_id` in a scoped block so the `kind()`
                    // Ref drops before we assign into `regs[func_reg]`
                    // on a cache hit (#450).
                    let fn_id_opt: Option<u64> = if is_pure_fn {
                        regs[*func_reg as usize].as_some().and_then(|fv| {
                            match fv.kind() {
                                ValueKind::UserFunction(func) => Some(func.id),
                                _ => None,
                            }
                        })
                    } else {
                        None
                    };
                    // MemoKey wraps PyKey and includes the ValueKind discriminant so that
                    // Float(1.0) and Int(1) — equal as PyKey but distinct types — are never
                    // treated as the same cache entry (fixes #562).
                    if let Some(fn_id) = fn_id_opt {
                        let mut key = std::mem::take(&mut self.key_scratch);
                        key.clear();
                        let mut all_hashable = true;
                        for i in 0..*argc as usize {
                            match regs[*func_reg as usize + 1 + i].to_key() {
                                Some(k) => key.push(MemoKey(k)),
                                None => {
                                    all_hashable = false;
                                    break;
                                }
                            }
                        }
                        if all_hashable {
                            let lookup = (fn_id, key);
                            let hit = self.fn_cache.get(&lookup).cloned();
                            let (_, key) = lookup;
                            self.key_scratch = key;
                            if let Some(cached) = hit {
                                regs[*func_reg as usize] = cached;
                                continue;
                            }
                        } else {
                            self.key_scratch = key;
                        }
                    }
                    // Cache miss or unhashable args: normal call (call_function_expanded
                    // will store the result in fn_cache on the way back).
                    let func_val = vm_try!(vm_read(&regs, *func_reg, num_locals));
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    for i in 0..crate::bytecode::Reg::from(*argc) {
                        buf.push(ExpandedCallArg {
                            name: None,
                            value: vm_try!(vm_read(&regs, *func_reg + 1 + i, num_locals)),
                        });
                    }
                    let call_result = self.call_function_expanded(func_val, &buf);
                    self.call_arg_buf = buf;
                    regs[*func_reg as usize] = vm_try!(call_result);
                }

                Insn::CallMethod { dst, obj, name_idx, args_base, nargs } => {
                    let r = self.exec_call_method(&mut regs, num_locals, *dst, *obj, *name_idx, *args_base, *nargs, code);
                    regs[*dst as usize] = vm_try!(r);
                }

                Insn::CallMethodExpanded { dst, obj, name_idx, pos_list, kw_dict } => {
                    let r = self.exec_call_method_expanded(&mut regs, num_locals, *dst, *obj, *name_idx, *pos_list, *kw_dict, code);
                    regs[*dst as usize] = vm_try!(r);
                }

                // ── Returns ──────────────────────────────────────────────
                Insn::Return(src) => {
                    return Ok(FrameOutcome::Returned(vm_try!(vm_read(&regs, *src, num_locals))));
                }
                Insn::ReturnNone => {
                    return Ok(FrameOutcome::Returned(Value::none()));
                }

                // ── Tail-call ────────────────────────────────────────────
                Insn::TailCall { args_base, nargs } => {
                    // The function to call lives at func_reg = args_base - 1.
                    let func_reg = args_base - 1;
                    let callee_val = vm_try!(vm_read(&regs, func_reg, num_locals));

                    // Self-call check: if the callee is the same user function as
                    // the one currently executing, and we are not inside a try
                    // block (exc_handlers must be empty for safe frame reuse),
                    // reset the register file and loop back to pc=0.
                    let is_self_call = if let Some(fn_id) = current_fn_id {
                        match callee_val.kind() {
                            ValueKind::UserFunction(f) => f.id == fn_id,
                            _ => false,
                        }
                    } else {
                        false
                    };

                    if is_self_call && exc_handlers.is_empty() {
                        // Guard against infinite tail recursion: treat each
                        // TCO iteration as one "call depth" unit.  This allows
                        // factorial(MAX_CALL_DEPTH * 100) while still raising
                        // RecursionError for truly infinite self-tail-calls.
                        tco_iters += 1;
                        if tco_iters > MAX_CALL_DEPTH * 100 {
                            let exc = if let Some(cls) = self.exc_classes.get("RecursionError") {
                                instantiate_exception(
                                    cls,
                                    vec![Value::string("maximum recursion depth exceeded")],
                                )
                            } else {
                                vm_try!(self.instantiate_named_exception(
                                    "RecursionError",
                                    "maximum recursion depth exceeded".to_string(),
                                ))
                            };
                            return Err(PyError::Raised(exc));
                        }
                        // Collect new argument values before we overwrite any registers.
                        let mut new_args: Vec<Value> =
                            Vec::with_capacity(*nargs as usize);
                        for i in 0..*nargs as u32 {
                            new_args.push(vm_try!(vm_read(&regs, args_base + i, num_locals)));
                        }
                        // Reset all registers to unset.
                        for slot in regs.iter_mut() {
                            *slot = Value::unset();
                        }
                        // Bind new positional args into parameter registers 0..nargs.
                        for (i, arg) in new_args.into_iter().enumerate() {
                            regs[i] = arg;
                        }
                        // Restore the self-reference in its original register so
                        // the recursive body can call itself again.
                        regs[func_reg as usize] = callee_val;
                        // Reset iterator and exception-handler state.
                        for slot in iters.iter_mut() {
                            *slot = None;
                        }
                        // exc_handlers is already empty (checked above).
                        // Jump to the top of the function.
                        pc = 0;
                        continue 'vm;
                    } else {
                        // Fallback: normal call, then return the result.
                        let mut buf = std::mem::take(&mut self.call_arg_buf);
                        buf.clear();
                        for i in 0..*nargs as u32 {
                            buf.push(ExpandedCallArg {
                                name: None,
                                value: vm_try!(vm_read(&regs, args_base + i, num_locals)),
                            });
                        }
                        let call_result = self.call_function_expanded(callee_val, &buf);
                        self.call_arg_buf = buf;
                        return Ok(FrameOutcome::Returned(vm_try!(call_result)));
                    }
                }

                // ── Collection builders ──────────────────────────────────
                Insn::BuildList(dst, base, n) => {
                    let mut items: Vec<Value> = Vec::with_capacity(*n as usize);
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        items.push(vm_try!(vm_read(&regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Value::list(items);
                }
                Insn::BuildTuple(dst, base, n) => {
                    let mut items = Vec::with_capacity(*n as usize);
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        items.push(vm_try!(vm_read(&regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Value::tuple(items);
                }
                Insn::BuildDict(dst, base, n) => {
                    let mut dict = indexmap::IndexMap::new();
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        let k_val = vm_try!(vm_read(&regs, *base + i * 2, num_locals));
                        let v_val = vm_try!(vm_read(&regs, *base + i * 2 + 1, num_locals));
                        let key = vm_try!(self.value_to_pykey(&k_val));
                        vm_try!(self.dict_insert(&mut dict, key, v_val));
                    }
                    regs[*dst as usize] = Value::dict(dict);
                }
                Insn::SetAdd(set_reg, val_reg) => {
                    let val = vm_try!(vm_read(&regs, *val_reg, num_locals));
                    let key = vm_try!(self.value_to_pykey(&val));
                    if let PyKey::Object { .. } = &key {
                        // Object keys need `__eq__` dispatch for dedup.
                        // Rc-clone the set Value (cheap) so `set_lookup` can
                        // run without an alias into the register file.
                        let set_val = regs[*set_reg as usize]
                            .as_some()
                            .cloned()
                            .unwrap_or(Value::none());
                        let found = vm_try!(self.set_lookup(&set_val, &key));
                        if found.is_none() {
                            vm_try!(regs[*set_reg as usize].set_add(key));
                        }
                    } else {
                        vm_try!(regs[*set_reg as usize].set_add(key));
                    }
                }
                Insn::ListAppend(list_reg, val_reg) => {
                    let val = vm_try!(vm_read(&regs, *val_reg, num_locals));
                    vm_try!(regs[*list_reg as usize].list_push(val));
                }
                Insn::ListExtend(list_reg, src_reg) => {
                    let src_val = vm_try!(vm_read(&regs, *src_reg, num_locals));
                    // #446: route through `collect_iterable` so user
                    // `__iter__` / `__getitem__` classes are honoured.
                    // #448: write back via the scoped `list_extend`
                    // operation method (no `&mut Vec` crosses the API
                    // boundary).
                    let items_to_add = vm_try!(self.collect_iterable(src_val));
                    vm_try!(regs[*list_reg as usize].list_extend(items_to_add));
                }
                Insn::DictUpdate(dict_reg, src_reg) => {
                    let src_val = vm_try!(vm_read(&regs, *src_reg, num_locals));
                    let src_dict = match src_val.kind() {
                        ValueKind::Dict(d) => d.clone(),
                        _ => vm_try!(Err(PyError::named(
                            "TypeError",
                            format!(
                                "'{}' object is not a mapping",
                                value_type_name_str(&src_val)
                            ),
                        ))),
                    };
                    let pairs: Vec<(PyKey, Value)> =
                        src_dict.into_iter().collect();
                    vm_try!(regs[*dict_reg as usize].dict_extend(pairs));
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
                    // PEP 3134: split the interpreter's handled-exception
                    // stack at this frame's base.  Entries pushed by THIS
                    // generator frame (above `exc_ctx_frame_base`) are saved
                    // onto the generator, then removed from the interpreter
                    // so the caller's frame sees only its own entries.  The
                    // generator's `active_exception` is likewise stashed and
                    // its slot cleared.  Both are re-installed on resume.
                    let saved_handled_slice: Vec<Value> =
                        self.handled_exc_stack.split_off(exc_ctx_frame_base);
                    let saved_active = self.active_exception.take();
                    // Return an explicit FrameOutcome::Yielded rather than
                    // using the old GEN_SAVE thread-local side-channel.
                    // Use mem::take to move iters and exc_handlers into the
                    // saved state rather than cloning them.  The local
                    // variables are left as empty Vecs, which are then
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
                            yield_dst: *dst,
                        },
                    });
                }

                // ── YieldFrom (PEP 380) ──────────────────────────────────
                Insn::YieldFrom { iter_reg, sent_reg, result_reg } => {
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
                            let saved_handled_slice: Vec<Value> =
                                self.handled_exc_stack.split_off(exc_ctx_frame_base);
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
                                    // The sent value for the next iteration goes into
                                    // sent_reg so the sub-iterator receives it.
                                    yield_dst: *sent_reg,
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
                    let items = vm_try!(self.collect_iterable(src_val));
                    if items.len() < *n as usize {
                        vm_try!(Err::<(), _>(PyError::Runtime(format!(
                            "not enough values to unpack (expected {}, got {})",
                            n,
                            items.len()
                        ))));
                    } else if items.len() > *n as usize {
                        vm_try!(Err::<(), _>(PyError::Runtime(format!(
                            "too many values to unpack (expected {})",
                            n
                        ))));
                    }
                    for (i, v) in items.into_iter().enumerate() {
                        let dst = *base as usize + i;
                        if dst >= regs.len() {
                            vm_try!(Err(PyError::Runtime(format!(
                                "Unpack: register {dst} out of range"
                            ))));
                        }
                        regs[dst] = v;
                    }
                }

                Insn::UnpackEx { src, before, after, dst_base } => {
                    let src_val = vm_try!(vm_read(&regs, *src, num_locals));
                    let items = vm_try!(self.collect_iterable(src_val));
                    let before = *before as usize;
                    let after = *after as usize;
                    let min_len = before + after;
                    if items.len() < min_len {
                        vm_try!(Err::<(), _>(PyError::named(
                            "ValueError",
                            format!(
                                "not enough values to unpack (expected at least {}, got {})",
                                min_len,
                                items.len()
                            ),
                        )));
                    }
                    let base = *dst_base as usize;
                    // First `before` elements
                    for (i, item) in items.iter().take(before).enumerate() {
                        let dst = base + i;
                        if dst >= regs.len() {
                            vm_try!(Err(PyError::Runtime(format!(
                                "UnpackEx: register {dst} out of range"
                            ))));
                        }
                        regs[dst] = item.clone();
                    }
                    // Middle as a list → R[base + before]
                    let star_end = items.len() - after;
                    let middle: Vec<Value> = items[before..star_end].to_vec();
                    let star_dst = base + before;
                    if star_dst >= regs.len() {
                        vm_try!(Err(PyError::Runtime(format!(
                            "UnpackEx: register {star_dst} out of range"
                        ))));
                    }
                    regs[star_dst] = Value::list(middle);
                    // Last `after` elements
                    for i in 0..after {
                        let dst = base + before + 1 + i;
                        if dst >= regs.len() {
                            vm_try!(Err(PyError::Runtime(format!(
                                "UnpackEx: register {dst} out of range"
                            ))));
                        }
                        regs[dst] = items[star_end + i].clone();
                    }
                }

                // ── Iterator ─────────────────────────────────────────────
                Insn::GetIter(slot, src) => {
                    // Range: lazy counter — no Vec needed.
                    // List/Tuple in a LOCAL register: lazy index — avoids the O(n)
                    //   upfront clone; the local slot is stable for the function lifetime.
                    // Temp register or other type: materialise immediately, because
                    //   a temp reg is freed and may be overwritten by the loop body.
                    let is_list_or_tuple_local = if *src < num_locals {
                        if let Some(v) = regs[*src as usize].as_some() {
                            matches!(v.kind(), ValueKind::List(_) | ValueKind::Tuple(_))
                        } else { false }
                    } else { false };

                    let state = if is_list_or_tuple_local {
                        IterState::Indexed { reg: *src, pos: 0 }
                    } else {
                        let src_val = vm_try!(vm_read(&regs, *src, num_locals));
                        // Detect the kind tag in a scoped block so the
                        // kind() Ref drops before we may move src_val
                        // into IterState / iter_values / make_getitem_iter
                        // (#450).
                        enum IterTag {
                            Range(i64, i64, i64),
                            Generator,
                            PyInstance(Rc<RefCell<crate::value::PyInstance>>),
                            BuiltinIterable,
                            Other,
                        }
                        let tag = match src_val.kind() {
                            ValueKind::Range { start, stop, step } => IterTag::Range(start, stop, step),
                            ValueKind::Generator(_) => IterTag::Generator,
                            ValueKind::PyInstance(inst) => IterTag::PyInstance(Rc::clone(inst)),
                            ValueKind::BuiltinObject { ops, .. } if ops.is_iterable() => {
                                IterTag::BuiltinIterable
                            }
                            _ => IterTag::Other,
                        };
                        match tag {
                            IterTag::Range(start, stop, step) => {
                                if step == 0 {
                                    vm_try!(Err(PyError::named(
                                        "ValueError",
                                        "range() arg 3 must not be zero".to_string(),
                                    )));
                                }
                                IterState::Range { cur: start, stop, step }
                            }
                            IterTag::Generator => IterState::UserDefined(src_val),
                            IterTag::PyInstance(inst_rc) => {
                                let class = Rc::clone(&inst_rc.borrow().class);
                                if let Some(method_val) = lookup_class_attr(&class, "__iter__") {
                                    let iter_obj = vm_try!(invoke_class_method(
                                        self,
                                        method_val,
                                        Value::py_instance(inst_rc),
                                        &[],
                                    ));
                                    IterState::UserDefined(iter_obj)
                                } else if lookup_class_attr(&class, "__getitem__").is_some() {
                                    let iter_obj = vm_try!(self.make_getitem_iter(inst_rc));
                                    IterState::UserDefined(iter_obj)
                                } else {
                                    IterState::Materialized(vm_try!(iter_values(src_val)), 0)
                                }
                            }
                            IterTag::BuiltinIterable => IterState::UserDefined(src_val),
                            IterTag::Other => {
                                IterState::Materialized(vm_try!(iter_values(src_val)), 0)
                            }
                        }
                    };
                    iters[*slot as usize] = Some(state);
                }
                Insn::ForIter(dst, slot, offset) => {
                    #[allow(clippy::collapsible_match)]
                    match iters[*slot as usize].as_mut() {
                        // Hot path: indexed iteration over a list/tuple held in a register.
                        // Direct as_list()/as_tuple() accessors skip the kind() decode and
                        // the big ValueKind match that the old implementation went through
                        // on every iteration.
                        Some(IterState::Indexed { reg, pos }) => {
                            let src = *reg as usize;
                            let cur_pos = *pos;
                            let items: Option<&[Value]> = if regs[src].is_unset() {
                                None
                            } else {
                                regs[src].as_list().or_else(|| regs[src].as_tuple())
                            };
                            match items {
                                Some(items) if cur_pos < items.len() => {
                                    // SAFETY: cur_pos < items.len() checked just above.
                                    let v = unsafe { items.get_unchecked(cur_pos).clone() };
                                    *pos = cur_pos + 1;
                                    regs[*dst as usize] = v;
                                }
                                _ => pc = jump_pc!(*offset),
                            }
                        }
                        Some(IterState::Materialized(items, pos)) => {
                            let cur_pos = *pos;
                            if cur_pos < items.len() {
                                // SAFETY: cur_pos < items.len() checked just above.
                                let v = unsafe { items.get_unchecked(cur_pos).clone() };
                                *pos = cur_pos + 1;
                                regs[*dst as usize] = v;
                            } else {
                                pc = jump_pc!(*offset);
                            }
                        }
                        Some(IterState::Range { cur, stop, step }) => {
                            let exhausted =
                                if *step > 0 { *cur >= *stop } else { *cur <= *stop };
                            if exhausted {
                                pc = jump_pc!(*offset);
                            } else {
                                let v = Value::int(*cur);
                                *cur += *step;
                                regs[*dst as usize] = v;
                            }
                        }
                        Some(IterState::UserDefined(iter_obj)) => {
                            // Call __next__() on the iterator object; stop on StopIteration.
                            let iter_val = iter_obj.clone();
                            let next_result: Option<Result<Value>> =
                                if let ValueKind::Generator(state_rc) = iter_val.kind() {
                                    let state_rc = Rc::clone(state_rc);
                                    // Probe for the lazy GetItemIter shape
                                    // first — its step needs &mut self for
                                    // the `__getitem__` invocation, so we
                                    // must release any cell borrow before
                                    // calling.
                                    let is_getitem_iter = state_rc
                                        .borrow()
                                        .downcast_ref::<GetItemIter>()
                                        .is_some();
                                    if is_getitem_iter {
                                        Some(match self.step_getitem_iter(&state_rc) {
                                            Ok(Some(v)) => Ok(v),
                                            Ok(None) => Err(PyError::named(
                                                "StopIteration",
                                                String::new(),
                                            )),
                                            Err(e) => Err(e),
                                        })
                                    } else {
                                        let mut borrow = state_rc.borrow_mut();
                                        if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                                            // Built-in iterator created by iter().
                                            if native.pos >= native.items.len() {
                                                Some(Err(PyError::named(
                                                    "StopIteration",
                                                    String::new(),
                                                )))
                                            } else {
                                                let item = native.items[native.pos].clone();
                                                native.pos += 1;
                                                Some(Ok(item))
                                            }
                                        } else if let Some(frame) = borrow.downcast_mut::<GeneratorFrame>() {
                                            // Resume the generator.
                                            if frame.done {
                                                Some(Err(PyError::named(
                                                    "StopIteration",
                                                    String::new(),
                                                )))
                                            } else {
                                                Some(self.resume_generator(frame))
                                            }
                                        } else {
                                            Some(Err(PyError::Runtime(
                                                "invalid generator state".to_string(),
                                            )))
                                        }
                                    }
                                } else if let ValueKind::PyInstance(inst) = iter_val.kind() {
                                    let inst_rc = Rc::clone(inst);
                                    let class = Rc::clone(&inst_rc.borrow().class);
                                    if let Some(method_val) =
                                        lookup_class_attr(&class, "__next__")
                                    {
                                        Some(invoke_class_method(
                                            self,
                                            method_val,
                                            Value::py_instance(inst_rc),
                                            &[],
                                        ))
                                    } else {
                                        None
                                    }
                                } else if let ValueKind::BuiltinObject { ops, state } =
                                    iter_val.kind()
                                {
                                    Some(ops.iter_next(state).and_then(|opt| {
                                        opt.ok_or_else(|| {
                                            PyError::named(
                                                "StopIteration",
                                                String::new(),
                                            )
                                        })
                                    }))
                                } else { None };
                            match next_result {
                                Some(Ok(val)) => {
                                    regs[*dst as usize] = val;
                                }
                                // class_name_is now walks the hierarchy for Raised,
                                // so StopIteration subclasses terminate the for-loop.
                                Some(Err(ref e)) if e.class_name_is("StopIteration") => {
                                    pc = jump_pc!(*offset);
                                }
                                Some(Err(e)) => { vm_try!(Err(e)); }
                                None => {
                                    vm_try!(Err(PyError::named(
                                        "TypeError",
                                        "iterator has no __next__ method".to_string(),
                                    )));
                                }
                            }
                        }
                        None => {
                            pc = jump_pc!(*offset);
                        }
                    }
                }
                Insn::ForCountReg(var, cmp_op, stop_reg, step_idx, offset) => {
                    let step = pool_get!(code.consts, *step_idx, "const")
                        .as_int()
                        .expect("ForCountReg step must be Int");
                    let pair: Option<(i64, i64)> = regs[*var as usize]
                        .as_int()
                        .zip(regs[*stop_reg as usize].as_int());
                    if let Some((cur, stop)) = pair {
                        for_count_step!(regs, *var, cur, stop, step, cmp_op, pc, *offset);
                    } else {
                        vm_try!(Err(crate::error::PyError::Runtime(
                            "for-range: non-integer counter or stop".into(),
                        )));
                    }
                }
                Insn::ForCountConst(var, cmp_op, stop_idx, step_idx, offset) => {
                    let step = pool_get!(code.consts, *step_idx, "const")
                        .as_int()
                        .expect("ForCountConst step must be Int");
                    let stop = pool_get!(code.consts, *stop_idx, "const")
                        .as_int()
                        .expect("ForCountConst stop must be Int");
                    if let Some(cur) = regs[*var as usize].as_int() {
                        for_count_step!(regs, *var, cur, stop, step, cmp_op, pc, *offset);
                    } else {
                        vm_try!(Err(crate::error::PyError::Runtime(
                            "for-range: non-integer counter".into(),
                        )));
                    }
                }
                Insn::ForCountConstInline(var, cmp_op, stop, step, offset) => {
                    // Same as ForCountConst but with stop/step inlined in
                    // the opcode payload — no per-iteration consts-pool
                    // lookup / `.kind()` decode for them.
                    if let Some(cur) = regs[*var as usize].as_int() {
                        for_count_step!(regs, *var, cur, *stop, *step, cmp_op, pc, *offset);
                    } else {
                        vm_try!(Err(crate::error::PyError::Runtime(
                            "for-range: non-integer counter".into(),
                        )));
                    }
                }
                Insn::CheckLocal(reg, name_idx) => {
                    // is_unset() checks for the slot sentinel (uninitialised
                    // local), not for Python's None — Value::is_none() would
                    // mis-fire on legitimate `x = None` followed by a read.
                    if regs[*reg as usize].is_unset() {
                        let name = pool_get!(code.names, *name_idx, "name");
                        vm_try!(Err::<(), _>(crate::error::PyError::Runtime(format!(
                            "cannot access local variable '{}' where it is not associated with a value",
                            name
                        ))));
                    }
                }

                // ── Function / Class creation ────────────────────────────
                Insn::MakeFunction(dst, proto_idx, defs_base, _defs_n) => {
                    let proto = pool_get!(code.fn_protos, *proto_idx, "fn_proto");
                    // Rc bumps only — no Vec clones for param metadata or local_names.
                    let proto_code = Rc::clone(&proto.code);
                    let proto_name = proto.name.clone();
                    let proto_qualname = proto.qualname.clone();
                    let proto_local_index = Rc::clone(&proto.local_index);
                    let proto_local_names = Rc::clone(&proto.local_names);
                    let proto_global_names = Rc::clone(&proto.global_names);
                    let proto_nonlocal_names = Rc::clone(&proto.nonlocal_names);
                    let param_spec = Rc::clone(&proto.param_spec);
                    let is_pure = proto.is_pure;

                    let mut params = Vec::with_capacity(param_spec.names.len());
                    let mut def_slot = 0u32;
                    for i in 0..param_spec.names.len() {
                        let default = if param_spec.has_default[i] {
                            let v =
                                vm_try!(vm_read(&regs, *defs_base + def_slot, num_locals));
                            def_slot += 1;
                            Some(v)
                        } else {
                            None
                        };
                        params.push(UserFunctionParam {
                            name: param_spec.names[i].clone(),
                            default,
                            is_args: param_spec.is_args[i],
                            is_kwargs: param_spec.is_kwargs[i],
                            is_keyword_only: param_spec.is_keyword_only[i],
                            is_positional_only: param_spec.is_positional_only[i],
                        });
                    }
                    // Validate that every nonlocal name resolves to an enclosing local scope.
                    for name in proto_nonlocal_names.iter() {
                        if !has_local_binding_in_current_or_ancestor(&self.env, name) {
                            let err = PyError::Runtime(format!(
                                "no binding for nonlocal '{}' found",
                                name
                            ));
                            vm_try!(Err(err));
                        }
                    }
                    let func = Rc::new(UserFunction {
                        id: crate::value::next_fn_id(),
                        kind: crate::value::UserFunctionKind::Regular,
                        name: proto_name,
                        qualname: proto_qualname,
                        user_name: std::cell::RefCell::new(None),
                        user_qualname: std::cell::RefCell::new(None),
                        // Module-name tracking is not yet implemented; use "__main__"
                        // for scripts run directly, matching CPython's default.
                        module: std::cell::RefCell::new(Value::string(
                            "__main__".to_string(),
                        )),
                        // Docstring extraction is not yet implemented at compile time;
                        // initialise to None (matching CPython for functions without a
                        // docstring).
                        doc: std::cell::RefCell::new(Value::none()),
                        // Lazy: allocated only when first accessed via __dict__
                        // or attribute assignment.  Avoids two heap allocations
                        // per function definition when no attrs are ever set.
                        attrs: std::cell::RefCell::new(None),
                        params,
                        local_names: proto_local_names,
                        local_index: proto_local_index,
                        global_names: proto_global_names,
                        nonlocal_names: proto_nonlocal_names,
                        env: Rc::clone(&self.env),
                        is_pure,
                        precompiled_code: Some(proto_code),
                    });
                    regs[*dst as usize] = Value::user_function(func);
                }
                Insn::MakeClass(dst, proto_idx, bases_base, bases_n, name_idx) => {
                    let class_name = pool_get!(code.names, *name_idx, "name").clone();
                    let (class_code, local_index, proto_qualname, proto_global_names, proto_nonlocal_names) = {
                        let proto = pool_get!(code.fn_protos, *proto_idx, "fn_proto");
                        (
                            Rc::clone(&proto.code),
                            Rc::clone(&proto.local_index),
                            proto.qualname.clone(),
                            Rc::clone(&proto.global_names),
                            Rc::clone(&proto.nonlocal_names),
                        )
                    };
                    let num_class_regs = class_code.num_regs as usize;
                    let mut class_regs: RegsBuf = smallvec![Value::unset(); num_class_regs];
                    // Issue #546: CPython pre-injects `__qualname__` and
                    // `__module__` into the class namespace before the body
                    // runs.  The compiler (compile_class) now always allocates
                    // register slots for these two names at slots 0 and 1
                    // (unless the user explicitly assigned them, in which case
                    // the slot is also present but may be at a different index).
                    // Pre-populate those slots here so that:
                    //   * reads of `__qualname__` / `__module__` inside the
                    //     class body see the correct values (via CheckLocal),
                    //   * `locals()` at the top of the class body includes them,
                    //   * the resulting class object has `C.__qualname__` and
                    //     `C.__module__` set correctly.
                    // Issue #592: the compiler now computes the full dotted qualname
                    // (e.g. "Outer.Inner") and stores it in FnProto::qualname so
                    // the VM can use it here without any runtime prefix tracking.
                    let qualname_slot = local_index.get("__qualname__").copied();
                    let module_slot = local_index.get("__module__").copied();
                    if let Some(slot) = qualname_slot {
                        let slot = slot as usize;
                        if slot < class_regs.len() {
                            class_regs[slot] = Value::string(proto_qualname.clone());
                        }
                    }
                    if let Some(slot) = module_slot {
                        let slot = slot as usize;
                        if slot < class_regs.len() {
                            // Use "__main__" for top-level scripts.  Module-name
                            // tracking is not yet implemented; this matches the
                            // CPython default for scripts run directly.
                            class_regs[slot] = Value::string("__main__".to_string());
                        }
                    }
                    // Issue #712: if the class body has annotations, the compiler
                    // pre-allocated a register slot for __annotations__.  Pre-seed it
                    // with an empty dict so compile_ann_assign's SetItem finds a live value.
                    let annotations_slot = local_index.get("__annotations__").copied();
                    if let Some(slot) = annotations_slot {
                        let slot = slot as usize;
                        if slot < class_regs.len() {
                            class_regs[slot] =
                                Value::dict(indexmap::IndexMap::new());
                        }
                    }
                    // Push a fresh class-store-order list onto the interpreter
                    // stack so `RecordClassStore` / `RecordClassDel` insns
                    // emitted inside the class body record into *this* class
                    // — supports `class A: class B: ...` nesting cleanly.
                    // CPython exposes __module__ before __qualname__ in the class
                    // namespace (verified against python3.12 `list(locals().keys())`
                    // at the top of a class body).  Only pre-push __module__:
                    // __qualname__ is a type-level descriptor on `type` in CPython
                    // and must NOT appear in `vars(C)` / the class attrs dict.
                    let mut pre_order: Vec<crate::bytecode::Reg> = Vec::new();
                    if let Some(slot) = module_slot {
                        pre_order.push(slot);
                    }
                    // qualname_slot is intentionally not added to pre_order so
                    // __qualname__ never flows into attrs (it's intercepted in
                    // get_attr instead — see issue #553).
                    // annotations_slot: pre-push into pre_order (right after __module__)
                    // so that __annotations__ always appears in the class attrs dict
                    // when the class body was compiled with any annotation, even if
                    // those annotations are inside a branch that never executes at
                    // runtime (e.g. `if False: x: int`).  CPython always seeds
                    // __annotations__ via SETUP_ANNOTATIONS before the class body
                    // runs; we mirror that by making it a pre-order slot.
                    // RecordClassStore calls from compile_ann_assign are still emitted
                    // but are harmless duplicates (deduplication happens naturally
                    // because store_order is built from the same slot registry).
                    if let Some(slot) = annotations_slot {
                        pre_order.push(slot as crate::bytecode::Reg);
                    }
                    self.class_store_order.push(pre_order);
                    // Issue #618: if the class body declares `global x`, we need
                    // `assign_name("x", ...)` to find "x" in `self.env.global_names`
                    // so it writes through to the module env.  Push a child env
                    // that inherits the current scope but has the class body's
                    // global_names set, then restore after the body runs.
                    // Issue #708: similarly, if the class body declares `nonlocal x`,
                    // set `nonlocal_names` on the child env so `assign_name("x", ...)`
                    // routes the store to the enclosing function's env cell rather than
                    // the class namespace.  Class scope is transparent to `nonlocal` —
                    // the store must reach the enclosing *function* binding.
                    let previous_env = if !proto_global_names.is_empty()
                        || !proto_nonlocal_names.is_empty()
                    {
                        let parent = Rc::clone(&self.env);
                        let class_env = self.alloc_env(Some(parent));
                        {
                            let mut e = class_env.borrow_mut();
                            e.global_names = proto_global_names;
                            e.nonlocal_names = proto_nonlocal_names;
                        }
                        Some(std::mem::replace(&mut self.env, class_env))
                    } else {
                        None
                    };
                    // Issue #487: publish a FrameKind::Class view so that
                    // `locals()` called inside the class body returns the
                    // partially-built class attrs dict (the fastlocal register
                    // file) rather than the module globals.  Capture the raw
                    // pointer before constructing RegSlice so both the
                    // VmFrameView and the dispatch loop use raw pointers only
                    // (no &mut [Value] on the allocation; issue #547).
                    let class_regs_ptr = unsafe {
                        std::ptr::NonNull::new_unchecked(class_regs.as_mut_ptr())
                    };
                    let class_regs_len = class_regs.len();
                    self.vm_frame_views.push(VmFrameView {
                        kind: FrameKind::Class,
                        // SAFETY: SmallVec / Vec allocation is always non-null.
                        // `class_regs` lives on this stack frame for the full
                        // duration of `run_bytecode`; the view is popped below
                        // before `class_regs` is dropped.
                        regs_ptr: class_regs_ptr,
                        regs_len: class_regs_len,
                        local_index: Rc::clone(&local_index),
                        nonlocal_names: None,
                        env: None,
                    });
                    // SAFETY: class_regs_ptr is valid for class_regs_len Values
                    // for the lifetime of class_regs (a local on this stack
                    // frame).  No &mut [Value] referencing class_regs is held
                    // while the dispatch loop runs (issue #547, PR #646).
                    let class_regs_slice = unsafe {
                        RegSlice::from_raw(class_regs_ptr.as_ptr(), class_regs_len)
                    };
                    let body_result = self.run_bytecode(&class_code, class_regs_slice);
                    // Always pop both stacks, even on error, to keep them balanced.
                    self.vm_frame_views.pop();
                    if let Some(prev) = previous_env {
                        let class_env = std::mem::replace(&mut self.env, prev);
                        self.free_env(class_env);
                    }
                    let mut store_order = self
                        .class_store_order
                        .pop()
                        .expect("class_store_order stack popped to empty");
                    vm_try!(body_result);
                    // Build a reverse lookup so we can name each slot in the
                    // recorded runtime store order.
                    let mut slot_to_name: Vec<Option<&String>> = vec![None; num_class_regs];
                    for (name, &slot) in local_index.iter() {
                        if (slot as usize) < slot_to_name.len() {
                            slot_to_name[slot as usize] = Some(name);
                        }
                    }
                    let mut attrs = IndexMap::new();
                    for slot in store_order.drain(..) {
                        let Some(name) = slot_to_name.get(slot as usize).and_then(|n| *n) else {
                            continue;
                        };
                        if let Some(v) = class_regs.get(slot as usize)
                            && !v.is_unset()
                        {
                            attrs.insert(name.clone(), v.clone());
                        }
                    }
                    // Issue #553: __qualname__ must NOT live in the class attrs
                    // dict — in CPython it is a descriptor on `type`, not an
                    // entry in the instance dict.  Remove it if the class body
                    // stored it explicitly (e.g. `__qualname__ = "X"`), then
                    // capture its value for the PyClass.qualname field which
                    // get_attr intercepts.  CPython raises TypeError if the
                    // class body assigns a non-str to __qualname__
                    // (Objects/typeobject.c `type_new_set_names`).
                    let qualname = match attrs.shift_remove("__qualname__") {
                        None => proto_qualname,
                        Some(v) => {
                            // Extract while kind() Ref is live, then drop it
                            // before the error path moves `v`.
                            let as_str: Option<String> = if let ValueKind::Str(s) = v.kind() {
                                Some(s.to_string())
                            } else {
                                None
                            };
                            match as_str {
                                Some(s) => s,
                                None => {
                                    // CPython: "type __qualname__ must be a str, not <type>"
                                    let tname =
                                        pyrust_core::builtin_type_name(&v).into_owned();
                                    vm_try!(Err(PyError::named(
                                        "TypeError",
                                        format!(
                                            "type __qualname__ must be a str, not {tname}"
                                        ),
                                    )));
                                    unreachable!()
                                }
                            }
                        }
                    };
                    // Issue #546: Ensure `__module__` is always present in the
                    // class attrs dict.  Under normal operation the pre-populated
                    // slot flows through `store_order` above; this `entry()` guard
                    // is a safety net for edge cases (e.g. `del __module__`).
                    attrs
                        .entry("__module__".to_string())
                        .or_insert_with(|| Value::string("__main__".to_string()));
                    // CPython rule (Objects/typeobject.c `type_new_set_slots`):
                    // if a class defines `__eq__` in its own body without also
                    // defining `__hash__`, implicitly set `__hash__ = None` so
                    // instances become unhashable.  Only the local class dict is
                    // checked here — base-class propagation is handled by the
                    // existing `lookup_class_attr` walk in the `hash()` builtin.
                    if attrs.contains_key("__eq__") && !attrs.contains_key("__hash__") {
                        attrs.insert("__hash__".to_string(), Value::none());
                    }
                    let base = if *bases_n > 0 {
                        let base_val = vm_try!(vm_read(&regs, *bases_base, num_locals));
                        match base_val.kind() {
                            ValueKind::PyClass(c) => Some(Rc::clone(c)),
                            _ => {
                                vm_try!(Err::<(), _>(PyError::Runtime(
                                    "class base must be a class".to_string(),
                                )));
                                unreachable!()
                            }
                        }
                    } else {
                        None
                    };
                    let class = Rc::new(RefCell::new(PyClass {
                        name: class_name,
                        qualname,
                        base,
                        attrs,
                    }));
                    regs[*dst as usize] = Value::py_class(class);
                }

                // ── Import ───────────────────────────────────────────────
                Insn::ImportModule(dst, name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name").clone();
                    let module = vm_try!(self.load_module(&name));
                    regs[*dst as usize] = module;
                }

                // ── REPL output ──────────────────────────────────────────
                Insn::PrintExpr(src) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    if !val.is_none() {
                        println!("{}", val.repr());
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
                    if let Some(order) = self.class_store_order.last_mut() {
                        if !order.contains(slot) {
                            order.push(*slot);
                        }
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

    #[allow(clippy::too_many_arguments)]
    fn exec_call_method(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        _dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        args_base: crate::bytecode::Reg,
        nargs: u8,
        code: &crate::bytecode::FnCode,
    ) -> Result<Value> {
        let method = code.names.get(name_idx as usize)
            .ok_or_else(|| PyError::Runtime(format!("bytecode error: name index {name_idx} out of range")))?
            .clone();
        let mut args: Vec<Value> = Vec::with_capacity(nargs as usize);
        for i in 0..crate::bytecode::Reg::from(nargs) {
            args.push(vm_read(regs, args_base + i, num_locals)?);
        }
        // Check if obj is a List, Dict, Tuple, Str, or Set via kind()
        let obj_kind_tag = regs[obj as usize].as_some().map(|v| match v.kind() {
            ValueKind::List(_) => 1u8,
            ValueKind::Dict(_) => 2u8,
            ValueKind::Tuple(_) => 3u8,
            ValueKind::Str(_) => 4u8,
            ValueKind::Set(_) => 5u8,
            _ => 0u8,
        }).unwrap_or(0);

        // No upfront unalias needed (#448): each builtin scopes its
        // own `RefCell::borrow_mut()` and snapshots iterable args
        // before opening the borrow.  Aliased self-references like
        // `lst.extend(lst)` are now safe by construction.

        match obj_kind_tag {
            1 => {
                let receiver = regs[obj as usize].clone();
                let empty_kw = indexmap::IndexMap::new();
                let args = if method == "index" {
                    self.resolve_seq_index_pos(args)?
                } else {
                    args
                };
                pyrust_builtins::list::call(&method, &receiver, args, &empty_kw)
            }
            2 => {
                if matches!(method.as_str(), "keys" | "values" | "items") {
                    // Lazy views need the Rc to share storage with the
                    // source dict — separate from the regular method
                    // dispatch path, which only sees the Vec<Value> form.
                    let rc = regs[obj as usize]
                        .get_dict_rc()
                        .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?
                        .clone();
                    return match method.as_str() {
                        "keys" => Ok(pyrust_builtins::dict_views::dict_keys(rc)),
                        "values" => Ok(pyrust_builtins::dict_views::dict_values(rc)),
                        "items" => Ok(pyrust_builtins::dict_views::dict_items(rc)),
                        _ => unreachable!(),
                    };
                }
                let receiver = vm_read(regs, obj, num_locals)?;
                self.call_dict_method(method.as_str(), receiver, args)
            }
            3 => {
                if let Some(ValueKind::Tuple(items)) = regs[obj as usize].as_some().map(|v| v.kind()) {
                    let args = if method == "index" {
                        self.resolve_seq_index_pos(args)?
                    } else {
                        args
                    };
                    pyrust_builtins::tuple::call(&method, items, args)
                } else {
                    unreachable!()
                }
            }
            4 => {
                if method == "format" {
                    let template = regs[obj as usize]
                        .as_str()
                        .ok_or_else(|| PyError::Runtime("internal: expected str".to_string()))?;
                    return self.format_str_template(template, &args, &[]);
                }
                if method == "format_map" {
                    if args.len() != 1 {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "str.format_map() takes exactly one argument ({} given)",
                                args.len()
                            ),
                        ));
                    }
                    let mapping = args.into_iter().next().unwrap();
                    let template = regs[obj as usize]
                        .as_str()
                        .ok_or_else(|| PyError::Runtime("internal: expected str".to_string()))?
                        .to_string();
                    return self.format_str_template_map(&template, mapping);
                }
                let receiver = vm_read(regs, obj, num_locals)?;
                self.call_str_method(method.as_str(), receiver, args)
            }
            5 => {
                let receiver = vm_read(regs, obj, num_locals)?;
                self.call_set_method(method.as_str(), receiver, args)
            }
            _ => {
                // Generator methods (close, throw, __next__, __iter__) are
                // dispatched directly here — they need access to the VM/frame
                // and are not regular attributes on the Generator value.
                let is_generator = matches!(
                    regs[obj as usize].as_some().map(|v| v.kind()),
                    Some(ValueKind::Generator(_))
                );
                if is_generator {
                    let obj_val = vm_read(regs, obj, num_locals)?;
                    return self.call_generator_method(obj_val, &method, args);
                }
                let obj_val = vm_read(regs, obj, num_locals)?;
                let method_val = self.get_attr(obj_val, &method)?;
                let mut buf = std::mem::take(&mut self.call_arg_buf);
                buf.clear();
                for arg in args {
                    buf.push(ExpandedCallArg { name: None, value: arg });
                }
                let r = self.call_function_expanded(method_val, &buf);
                self.call_arg_buf = buf;
                r
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_call_method_expanded(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        _dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        pos_list: crate::bytecode::Reg,
        kw_dict: crate::bytecode::Reg,
        code: &crate::bytecode::FnCode,
    ) -> Result<Value> {
        let method = code.names.get(name_idx as usize)
            .ok_or_else(|| PyError::Runtime(format!("bytecode error: name index {name_idx} out of range")))?
            .clone();
        let v = vm_read(regs, pos_list, num_locals)?;
        let pos_items: Vec<Value> = match v.kind() {
            ValueKind::List(items) => items.to_vec(),
            _ => return Err(PyError::Runtime("CallMethodExpanded: pos_list must be a list".to_string())),
        };
        let v = vm_read(regs, kw_dict, num_locals)?;
        let kw_map = match v.kind() {
            ValueKind::Dict(d) => d.clone(),
            _ => return Err(PyError::Runtime("CallMethodExpanded: kw_dict must be a dict".to_string())),
        };

        let obj_kind_tag = regs[obj as usize].as_some().map(|v| match v.kind() {
            ValueKind::List(_) => 1u8,
            ValueKind::Dict(_) => 2u8,
            ValueKind::Tuple(_) => 3u8,
            ValueKind::Str(_) => 4u8,
            ValueKind::Set(_) => 5u8,
            _ => 0u8,
        }).unwrap_or(0);

        // No upfront unalias needed (#448): each builtin scopes its
        // own `borrow_mut()` and snapshots iterables before opening
        // the borrow.

        match obj_kind_tag {
            1 => {
                let receiver = regs[obj as usize].clone();
                // Intercept list.sort here to support key= (needs interpreter access).
                if method == "sort" {
                    // Cache the kwarg PyKeys once so each list.sort() call avoids
                    // the two `String` allocations the literal lookups would
                    // otherwise incur. See issue #277.
                    static KEY_KW: std::sync::LazyLock<PyKey> =
                        std::sync::LazyLock::new(|| PyKey::Str("key".to_string()));
                    static REVERSE_KW: std::sync::LazyLock<PyKey> =
                        std::sync::LazyLock::new(|| PyKey::Str("reverse".to_string()));
                    for k in kw_map.keys() {
                        if let PyKey::Str(s) = k
                            && s != "key" && s != "reverse"
                        {
                            return Err(PyError::named(
                                "TypeError",
                                format!("sort() got an unexpected keyword argument '{s}'"),
                            ));
                        }
                    }
                    let key_fn = kw_map.get(&*KEY_KW).cloned();
                    let reverse = kw_map
                        .get(&*REVERSE_KW)
                        .map(|v| v.truthy())
                        .unwrap_or(false);
                    if let Some(key_fn_val) = key_fn {
                        // Compute keys via the interpreter, then delegate sorting to builtins.
                        let items_snapshot = receiver
                            .list_with(|items| items.clone())
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected list".to_string())
                            })?;
                        let mut keys: Vec<Value> = Vec::with_capacity(items_snapshot.len());
                        for item in &items_snapshot {
                            let key_val = {
                                let mut buf = std::mem::take(&mut self.call_arg_buf);
                                buf.clear();
                                buf.push(ExpandedCallArg {
                                    name: None,
                                    value: item.clone(),
                                });
                                let r = self.call_function_expanded(key_fn_val.clone(), &buf);
                                self.call_arg_buf = buf;
                                r?
                            };
                            keys.push(key_val);
                        }
                        return pyrust_builtins::list::sort_with_precomputed_keys(
                            &receiver, keys, reverse,
                        );
                    }
                    // No key: delegate to builtins (handles reverse kwarg)
                    return pyrust_builtins::list::call(&method, &receiver, pos_items, &kw_map);
                }
                let pos_items = if method == "index" {
                    self.resolve_seq_index_pos(pos_items)?
                } else {
                    pos_items
                };
                pyrust_builtins::list::call(&method, &receiver, pos_items, &kw_map)
            }
            2 => {
                if matches!(method.as_str(), "keys" | "values" | "items") {
                    // Lazy views need the Rc to share storage with the
                    // source dict — see `exec_call_method`.
                    let rc = regs[obj as usize]
                        .get_dict_rc()
                        .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?
                        .clone();
                    return match method.as_str() {
                        "keys" => Ok(pyrust_builtins::dict_views::dict_keys(rc)),
                        "values" => Ok(pyrust_builtins::dict_views::dict_values(rc)),
                        "items" => Ok(pyrust_builtins::dict_views::dict_items(rc)),
                        _ => unreachable!(),
                    };
                }
                let receiver = vm_read(regs, obj, num_locals)?;
                self.call_dict_method(method.as_str(), receiver, pos_items)
            }
            3 => {
                if let Some(ValueKind::Tuple(items)) = regs[obj as usize].as_some().map(|v| v.kind()) {
                    let pos_items = if method == "index" {
                        self.resolve_seq_index_pos(pos_items)?
                    } else {
                        pos_items
                    };
                    pyrust_builtins::tuple::call(&method, items, pos_items)
                } else {
                    Err(PyError::Runtime("internal: expected tuple".to_string()))
                }
            }
            4 => {
                if method == "format" {
                    let mut keyword: Vec<(String, Value)> = Vec::with_capacity(kw_map.len());
                    for (k, v) in &kw_map {
                        if let PyKey::Str(name) = k {
                            keyword.push((name.clone(), v.clone()));
                        }
                    }
                    let template = regs[obj as usize]
                        .as_str()
                        .ok_or_else(|| PyError::Runtime("internal: expected str".to_string()))?;
                    return self.format_str_template(template, &pos_items, &keyword);
                }
                if method == "format_map" {
                    if pos_items.len() != 1 || !kw_map.is_empty() {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "str.format_map() takes exactly one argument ({} given)",
                                pos_items.len() + kw_map.len()
                            ),
                        ));
                    }
                    let mapping = pos_items.into_iter().next().unwrap();
                    let template = regs[obj as usize]
                        .as_str()
                        .ok_or_else(|| PyError::Runtime("internal: expected str".to_string()))?
                        .to_string();
                    return self.format_str_template_map(&template, mapping);
                }
                let receiver = vm_read(regs, obj, num_locals)?;
                self.call_str_method(method.as_str(), receiver, pos_items)
            }
            5 => {
                let receiver = vm_read(regs, obj, num_locals)?;
                self.call_set_method(method.as_str(), receiver, pos_items)
            }
            _ => {
                // Generator methods — see `exec_call_method` for context.
                let is_generator = matches!(
                    regs[obj as usize].as_some().map(|v| v.kind()),
                    Some(ValueKind::Generator(_))
                );
                if is_generator {
                    if !kw_map.is_empty() {
                        return Err(PyError::named(
                            "TypeError",
                            format!("generator.{method}() takes no keyword arguments"),
                        ));
                    }
                    let obj_val = vm_read(regs, obj, num_locals)?;
                    return self.call_generator_method(obj_val, &method, pos_items);
                }
                let obj_val = vm_read(regs, obj, num_locals)?;
                let method_val = self.get_attr(obj_val, &method)?;
                let mut expanded: Vec<ExpandedCallArg> = pos_items
                    .into_iter()
                    .map(|v| ExpandedCallArg { name: None, value: v })
                    .collect();
                for (k, v) in &kw_map {
                    if let PyKey::Str(name) = k {
                        expanded.push(ExpandedCallArg { name: Some(name.clone()), value: v.clone() });
                    }
                }
                let mut buf = std::mem::take(&mut self.call_arg_buf);
                buf.clear();
                buf.extend(expanded);
                let r = self.call_function_expanded(method_val, &buf);
                self.call_arg_buf = buf;
                r
            }
        }
    }

    /// Dispatch a method call on a `Generator` value (`g.close()`, `g.throw()`,
    /// `g.__next__()`, `g.__iter__()`).  Other names raise `AttributeError`.
    pub(crate) fn call_generator_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "__iter__" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "generator.__iter__() takes no arguments".to_string(),
                    ));
                }
                Ok(receiver)
            }
            "__next__" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "generator.__next__() takes no arguments".to_string(),
                    ));
                }
                self.call_next(receiver, None)
            }
            "close" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "generator.close() takes no arguments".to_string(),
                    ));
                }
                self.generator_close(receiver)
            }
            "throw" => {
                if args.is_empty() || args.len() > 3 {
                    return Err(PyError::named(
                        "TypeError",
                        "generator.throw() takes 1 to 3 arguments".to_string(),
                    ));
                }
                // CPython's throw(typ, val=None, tb=None) semantics (3.12):
                //   - 1 arg:  pass through to generator_throw (handles both
                //             class and instance via coerce_to_exception).
                //   - 2+ args: typ=args[0], val=args[1]; traceback (args[2])
                //             is ignored (PEP 3109; deprecated since 3.12).
                //     * val is None          → raise typ() with no message.
                //     * val is instance of typ → use val directly.
                //     * otherwise            → raise typ(val).
                let exc = if args.len() == 1 {
                    args.into_iter().next().unwrap()
                } else {
                    let mut arg_iter = args.into_iter();
                    let typ = arg_iter.next().unwrap();
                    let val = arg_iter.next().unwrap();
                    // Extract the class Rc before consuming `typ`, so the
                    // non-class fall-through branch can still return `typ`.
                    let class_opt = match typ.kind() {
                        ValueKind::PyClass(c) => Some(Rc::clone(c)),
                        _ => None,
                    };
                    if let Some(class) = class_opt {
                        // Two-arg construction: val=None, val=instance, or
                        // val=arbitrary value to be passed to the constructor.
                        if val.is_none() {
                            // throw(ExcType, None) — same as throw(ExcType)
                            instantiate_exception(class, Vec::new())
                        } else {
                            // Check if val is already a subclass instance of
                            // typ; extract the Rc before we potentially move val.
                            let val_inst_rc = match val.kind() {
                                ValueKind::PyInstance(inst) => {
                                    let inst_class = Rc::clone(&inst.borrow().class);
                                    if class_is_subclass_of(&inst_class, &class) {
                                        Some(Rc::clone(inst))
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            };
                            if let Some(inst_rc) = val_inst_rc {
                                // val is already a suitable instance — use directly.
                                Value::py_instance(inst_rc)
                            } else {
                                // Construct typ(val).
                                instantiate_exception(class, vec![val])
                            }
                        }
                    } else {
                        // typ is already an instance.  CPython 3.12 requires
                        // val to be None in this case; any other value is a
                        // TypeError ("instance exception may not have a
                        // separate value").
                        if !val.is_none() {
                            return Err(PyError::named(
                                "TypeError",
                                "instance exception may not have a separate value".to_string(),
                            ));
                        }
                        typ
                    }
                };
                self.generator_throw(receiver, exc)
            }
            "send" => {
                if args.len() != 1 {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "generator.send() takes exactly one argument ({} given)",
                            args.len()
                        ),
                    ));
                }
                let sent_value = args.into_iter().next().unwrap();
                self.generator_send(receiver, sent_value)
            }
            other => Err(PyError::named(
                "AttributeError",
                format!("'generator' object has no attribute '{}'", other),
            )),
        }
    }

    /// Implementation of `generator.close()`.
    ///
    /// Raises `GeneratorExit` at the current yield point.  Returns silently if
    /// the generator finishes (normally, by re-raising `GeneratorExit`, or by
    /// raising `StopIteration`); raises `RuntimeError("generator ignored
    /// GeneratorExit")` if the generator yields again; propagates any other
    /// exception unchanged.
    fn generator_close(&mut self, receiver: Value) -> Result<Value> {
        let state_rc = match receiver.kind() {
            ValueKind::Generator(rc) => Rc::clone(rc),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "generator.close() called on non-generator".to_string(),
                ));
            }
        };

        // Re-entrancy guard: if the generator is currently executing (its
        // state RefCell is already borrowed by an in-flight `resume_*` call),
        // CPython raises ValueError("generator already executing") rather
        // than panicking.  We detect this via `try_borrow_mut`.
        {
            let mut borrow = match state_rc.try_borrow_mut() {
                Ok(b) => b,
                Err(_) => {
                    return Err(PyError::named(
                        "ValueError",
                        "generator already executing".to_string(),
                    ));
                }
            };
            // NativeIterFrame (returned by `iter()` on a built-in): close is a
            // no-op — there's nothing user-visible to clean up.
            if borrow.downcast_mut::<NativeIterFrame>().is_some() {
                return Ok(Value::none());
            }
            // GetItemIter (legacy `__getitem__` protocol, #394): same
            // story — there is no user frame to clean up; mark
            // exhausted so subsequent next() raises StopIteration.
            if let Some(it) = borrow.downcast_mut::<GetItemIter>() {
                it.exhausted = true;
                return Ok(Value::none());
            }
        }

        let mut borrow = match state_rc.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => {
                return Err(PyError::named(
                    "ValueError",
                    "generator already executing".to_string(),
                ));
            }
        };
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
        if frame.done {
            return Ok(Value::none());
        }

        let inject = PyError::named("GeneratorExit", String::new());
        match self.resume_generator_with_exc(frame, Some(inject), Value::none()) {
            // Generator yielded again instead of returning/re-raising — that's
            // an error in CPython.
            Ok(_yielded) => {
                // Mark as done so subsequent calls don't re-execute.
                frame.done = true;
                Err(PyError::named(
                    "RuntimeError",
                    "generator ignored GeneratorExit".to_string(),
                ))
            }
            // Generator returned normally (StopIteration synthesised).
            // class_name_is walks the hierarchy so StopIteration subclasses are
            // also accepted as a normal termination.
            Err(ref e) if e.class_name_is("StopIteration") => Ok(Value::none()),
            // Generator re-raised GeneratorExit — that's the expected close
            // behaviour, swallow it.  Subclasses are equally valid.
            Err(ref e) if e.class_name_is("GeneratorExit") => Ok(Value::none()),
            Err(e) => Err(e),
        }
    }

    /// Implementation of `generator.throw(exc)`.
    ///
    /// Injects `exc` at the current yield point.  If the generator catches it
    /// and yields, returns that value.  If it returns normally, raises
    /// `StopIteration`.  Otherwise propagates the (re-raised or new)
    /// exception.
    fn generator_throw(&mut self, receiver: Value, exc: Value) -> Result<Value> {
        let state_rc = match receiver.kind() {
            ValueKind::Generator(rc) => Rc::clone(rc),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "generator.throw() called on non-generator".to_string(),
                ));
            }
        };

        // Convert `exc` argument into a concrete exception instance so we can
        // hand it to the VM via `PyError::Raised`.  Accepts the same shapes as
        // a `raise` statement: an exception class (auto-instantiates) or an
        // exception instance.  CPython raises `TypeError` (not `RuntimeError`)
        // when the argument is neither, so remap that specific case here.
        let exc_val = self.coerce_to_exception(exc).map_err(|e| match e {
            PyError::Runtime(ref msg) if msg.contains("exceptions must derive") => {
                PyError::named("TypeError", msg.clone())
            }
            other => other,
        })?;

        // Re-entrancy guard: see generator_close for rationale.
        {
            let mut borrow = match state_rc.try_borrow_mut() {
                Ok(b) => b,
                Err(_) => {
                    return Err(PyError::named(
                        "ValueError",
                        "generator already executing".to_string(),
                    ));
                }
            };
            // NativeIterFrame: throw at a built-in iterator simply propagates
            // the exception (matching CPython, where the iterator has no
            // Python frame to inject into).
            if borrow.downcast_mut::<NativeIterFrame>().is_some() {
                return Err(PyError::Raised(exc_val));
            }
            // GetItemIter (#394): same — no Python frame to inject
            // into.  Propagate the thrown exception.
            if borrow.downcast_mut::<GetItemIter>().is_some() {
                return Err(PyError::Raised(exc_val));
            }
        }

        let mut borrow = match state_rc.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => {
                return Err(PyError::named(
                    "ValueError",
                    "generator already executing".to_string(),
                ));
            }
        };
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
        if frame.done {
            // throw() on an exhausted generator re-raises the exception
            // immediately (CPython behaviour).
            return Err(PyError::Raised(exc_val));
        }

        let inject = PyError::Raised(exc_val);
        match self.resume_generator_with_exc(frame, Some(inject), Value::none()) {
            // Generator caught the injected exception and yielded.
            Ok(v) => Ok(v),
            // Generator returned normally: propagate the original StopIteration so
            // .value (set by resume_generator_with_exc via instantiate_exception)
            // is preserved (PEP 380 / issue #600).
            Err(e) if e.class_name_is("StopIteration") => Err(e),
            // Any other propagating error (including a re-raise of the
            // injected exception) flows through unchanged.
            Err(e) => Err(e),
        }
    }

    /// Implementation of `generator.send(value)`.
    ///
    /// Resumes the generator and delivers `sent_value` as the result of the
    /// suspended `yield` expression inside the body.  Equivalent to `next(g)`
    /// when `sent_value` is `None`.
    ///
    /// CPython raises `TypeError: can't send non-None value to a just-started
    /// generator` when called on a generator that has never been advanced to
    /// its first `yield`.
    fn generator_send(&mut self, receiver: Value, sent_value: Value) -> Result<Value> {
        let state_rc = match receiver.kind() {
            ValueKind::Generator(rc) => Rc::clone(rc),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "generator.send() called on non-generator".to_string(),
                ));
            }
        };

        let mut borrow = match state_rc.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => {
                return Err(PyError::named(
                    "ValueError",
                    "generator already executing".to_string(),
                ));
            }
        };

        // NativeIterFrame and GetItemIter do not support send().
        if borrow.downcast_mut::<NativeIterFrame>().is_some()
            || borrow.downcast_mut::<GetItemIter>().is_some()
        {
            return Err(PyError::named(
                "AttributeError",
                "'generator' object has no attribute 'send'".to_string(),
            ));
        }

        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;

        if frame.done {
            // Exhausted generator: StopIteration() with no args → .value is None.
            let exc = if let Some(cls) = self.exc_classes.get("StopIteration") {
                PyError::Raised(instantiate_exception(cls, vec![]))
            } else {
                PyError::named("StopIteration", String::new())
            };
            return Err(exc);
        }

        // CPython: sending a non-None value to a just-started generator is an
        // error.  A just-started generator has pc == 0 (never been resumed).
        if frame.pc == 0 && !sent_value.is_none() {
            return Err(PyError::named(
                "TypeError",
                "can't send non-None value to a just-started generator".to_string(),
            ));
        }

        match self.resume_generator_with_exc(frame, None, sent_value) {
            Ok(yielded) => Ok(yielded),
            // Propagate the original StopIteration so .value is preserved
            // (PEP 380 / issue #600).  Mirrors the same fix in call_next.
            Err(e) if e.class_name_is("StopIteration") => Err(e),
            Err(e) => Err(e),
        }
    }

    /// Advance a `yield from` sub-iterator by one step, forwarding `sent_val`
    /// to the sub-iterator's `send()` method if it is a generator.
    ///
    /// Returns:
    /// - `Ok(v)` — sub-iterator yielded `v`
    /// - `Err(StopIteration)` — sub-iterator exhausted; caller reads `.value`
    ///   from the error to obtain the sub-iterator's return value
    /// - `Err(other)` — exception from the sub-iterator
    fn yield_from_advance(&mut self, iter_val: &Value, sent_val: Value) -> Result<Value> {
        match iter_val.kind() {
            ValueKind::Generator(state_rc) => {
                let state_rc = Rc::clone(state_rc);

                // Check for GetItemIter (lazy __getitem__ iterator).
                let is_getitem = state_rc.borrow().downcast_ref::<GetItemIter>().is_some();
                if is_getitem {
                    // GetItemIter doesn't support send; treat as next().
                    return match self.step_getitem_iter(&state_rc) {
                        Ok(Some(v)) => Ok(v),
                        Ok(None) => Err(PyError::named("StopIteration", String::new())),
                        Err(e) => Err(e),
                    };
                }

                let mut borrow = state_rc.try_borrow_mut().map_err(|_| {
                    PyError::named("ValueError", "generator already executing".to_string())
                })?;

                if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                    // Built-in iterator: no send support, just advance.
                    if native.pos >= native.items.len() {
                        return Err(PyError::named("StopIteration", String::new()));
                    }
                    let item = native.items[native.pos].clone();
                    native.pos += 1;
                    return Ok(item);
                }

                if let Some(frame) = borrow.downcast_mut::<GeneratorFrame>() {
                    if frame.done {
                        return Err(PyError::named("StopIteration", String::new()));
                    }
                    // `yield from` bypasses CPython's "can't send non-None to a
                    // just-started generator" check — the compiler initialises
                    // sent_reg to None so the first call is always next()-equivalent.
                    match self.resume_generator_with_exc(frame, None, sent_val) {
                        Ok(v) => return Ok(v),
                        Err(e) if is_stop_iteration_error(&e) => {
                            // Generator exhausted.  If it returned a non-None value
                            // (stashed by resume_generator_with_exc in
                            // frame.last_return_value), materialise a StopIteration
                            // instance with that value as args[0] so that
                            // extract_stop_iteration_value() can retrieve it in the
                            // YieldFrom handler.  self.env is restored at this point
                            // (resume_generator_with_exc swaps it back on return).
                            if let Some(rv) = frame.last_return_value.clone() {
                                if !rv.is_none() {
                                    if let Some(cls) =
                                        lookup_name_in_module(&self.env, "StopIteration")
                                            .and_then(|v| match v.kind() {
                                                ValueKind::PyClass(c) => Some(Rc::clone(c)),
                                                _ => None,
                                            })
                                    {
                                        let exc = instantiate_exception(cls, vec![rv]);
                                        return Err(PyError::Raised(exc));
                                    }
                                }
                            }
                            return Err(e);
                        }
                        Err(e) => return Err(e),
                    }
                }

                Err(PyError::Runtime("invalid generator state in yield from".to_string()))
            }
            ValueKind::PyInstance(inst_rc) => {
                let inst_rc = Rc::clone(inst_rc);
                let class = Rc::clone(&inst_rc.borrow().class);
                // Try send() first (PEP 342 compliant generators).
                if !sent_val.is_none() {
                    if let Some(send_method) = lookup_class_attr(&class, "send") {
                        return invoke_class_method(
                            self,
                            send_method,
                            Value::py_instance(inst_rc),
                            &[ExpandedCallArg { name: None, value: sent_val }],
                        );
                    }
                }
                // Fall back to __next__().
                if let Some(next_method) = lookup_class_attr(&class, "__next__") {
                    invoke_class_method(self, next_method, Value::py_instance(inst_rc), &[])
                } else {
                    Err(PyError::named(
                        "TypeError",
                        "object is not an iterator".to_string(),
                    ))
                }
            }
            ValueKind::BuiltinObject { ops, state } => {
                let state = state.clone();
                ops.iter_next(&state).and_then(|opt| {
                    opt.ok_or_else(|| PyError::named("StopIteration", String::new()))
                })
            }
            _ => Err(PyError::named(
                "TypeError",
                "object is not iterable".to_string(),
            )),
        }
    }

    /// Forward a thrown exception to a `yield from` sub-iterator (PEP 380 §3).
    ///
    /// Returns:
    /// - `Ok(v)` — sub-iterator caught the exception and yielded `v`
    /// - `Err(StopIteration)` — sub-iterator returned after handling the throw
    /// - `Err(other)` — sub-iterator did not handle it (or raised a new exception)
    fn yield_from_throw_forward(&mut self, iter_val: &Value, exc: PyError) -> Result<Value> {
        match iter_val.kind() {
            ValueKind::Generator(state_rc) => {
                let state_rc = Rc::clone(state_rc);

                // GetItemIter and NativeIterFrame have no Python frame; propagate.
                if state_rc.borrow().downcast_ref::<GetItemIter>().is_some() {
                    return Err(exc);
                }

                let mut borrow = state_rc.try_borrow_mut().map_err(|_| {
                    PyError::named("ValueError", "generator already executing".to_string())
                })?;

                if borrow.downcast_mut::<NativeIterFrame>().is_some() {
                    return Err(exc);
                }

                if let Some(frame) = borrow.downcast_mut::<GeneratorFrame>() {
                    if frame.done {
                        return Err(exc);
                    }
                    match self.resume_generator_with_exc(frame, Some(exc), Value::none()) {
                        Ok(v) => return Ok(v),
                        Err(e) if is_stop_iteration_error(&e) => {
                            // Inner generator returned after handling the throw.
                            // Encode the return value in the StopIteration error.
                            if let Some(rv) = frame.last_return_value.clone() {
                                if !rv.is_none() {
                                    if let Some(cls) =
                                        lookup_name_in_module(&self.env, "StopIteration")
                                            .and_then(|v| match v.kind() {
                                                ValueKind::PyClass(c) => Some(Rc::clone(c)),
                                                _ => None,
                                            })
                                    {
                                        let exc_with_val =
                                            instantiate_exception(cls, vec![rv]);
                                        return Err(PyError::Raised(exc_with_val));
                                    }
                                }
                            }
                            return Err(e);
                        }
                        Err(e) => return Err(e),
                    }
                }

                Err(exc)
            }
            ValueKind::PyInstance(inst_rc) => {
                let inst_rc = Rc::clone(inst_rc);
                let class = Rc::clone(&inst_rc.borrow().class);
                // Try throw() method first.
                if let Some(throw_method) = lookup_class_attr(&class, "throw") {
                    // Materialise the exception into a Value for the throw() call.
                    let exc_val = match exc {
                        PyError::Raised(v) => v,
                        PyError::Named(name, msg) => {
                            match self.instantiate_named_exception(name.as_ref(), msg) {
                                Ok(v) => v,
                                Err(e) => return Err(e),
                            }
                        }
                        other => return Err(other),
                    };
                    invoke_class_method(
                        self,
                        throw_method,
                        Value::py_instance(inst_rc),
                        &[ExpandedCallArg { name: None, value: exc_val }],
                    )
                } else {
                    // No throw() method: propagate the exception.
                    Err(exc)
                }
            }
            // Other iterator types don't have throw() support.
            _ => Err(exc),
        }
    }
}

#[inline]
fn vm_read(regs: &[Value], reg: crate::bytecode::Reg, num_locals: crate::bytecode::Reg) -> crate::interpreter::Result<Value> {
    let v = &regs[reg as usize];
    if v.is_unset() {
        if reg < num_locals {
            return Err(crate::error::PyError::named(
                "NameError",
                "local variable referenced before assignment".to_string(),
            ));
        } else {
            return Err(crate::error::PyError::Runtime(
                "internal: temp register read before write".to_string(),
            ));
        }
    }
    Ok(v.clone())
}

fn vm_eval_unary(op: UnaryOp, val: Value) -> Result<Value> {
    match op {
        UnaryOp::Neg => match val.kind() {
            ValueKind::Int(v) => Ok(match v.checked_neg() {
                Some(r) => Value::int(r),
                None => Value::bigint(-PyBigInt::from(v)),
            }),
            ValueKind::Float(v) => Ok(Value::float(-v)),
            ValueKind::Complex(re, im) => Ok(Value::complex(-re, -im)),
            ValueKind::BigInt(v) => Ok(Value::bigint(-v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { -1 } else { 0 })),
            _ => Err(PyError::named("TypeError", "bad operand type for unary -".to_string())),
        },
        UnaryOp::Not => Ok(Value::bool_(!val.truthy())),
        UnaryOp::BitNot => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(!v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { -2 } else { -1 })),
            ValueKind::BigInt(v) => Ok(Value::bigint(!v)),
            _ => Err(PyError::named(
                "TypeError",
                "bad operand type for unary ~: use integer".to_string(),
            )),
        },
        UnaryOp::Pos => {
            if matches!(val.kind(), ValueKind::BigInt(_)) {
                return Ok(val);
            }
            match val.kind() {
                ValueKind::Int(v) => Ok(Value::int(v)),
                ValueKind::Float(v) => Ok(Value::float(v)),
                ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                _ => Err(PyError::named("TypeError", "bad operand type for unary +".to_string())),
            }
        }
    }
}

/// PEP 479 conversion: if `err` is a `StopIteration` or any subclass, replace
/// it with `RuntimeError("generator raised StopIteration")`.  Otherwise return
/// `err` unchanged.
///
/// Called from the `Err(e)` arm of `resume_generator_with_exc` so that any
/// `StopIteration` that escapes a generator body is wrapped before it reaches
/// the caller.  The check is subclass-aware: a user-defined subclass of
/// `StopIteration` is also wrapped.
///
/// Per CPython 3.12, the original `StopIteration` instance is set as
/// `__cause__` on the new `RuntimeError`, and `__suppress_context__` is set
/// to `True`.  This mirrors `Insn::RaiseFrom` which implements the same
/// `__cause__` / `__suppress_context__` assignment for `raise X from Y`.
/// Returns `true` for any `PyError` variant that represents a `StopIteration`
/// (or a subclass thereof expressed as `PyError::Raised`).  Used in `yield from`
/// to detect sub-iterator exhaustion regardless of how the error was constructed.
fn is_stop_iteration_error(err: &PyError) -> bool {
    match err {
        PyError::Named(cls, _) => cls.as_ref() == "StopIteration",
        PyError::Class(cls, _) => cls.borrow().name == "StopIteration",
        PyError::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => inst.borrow().class.borrow().name == "StopIteration",
            ValueKind::PyClass(cls) => cls.borrow().name == "StopIteration",
            _ => false,
        },
        _ => false,
    }
}

/// Extract the `value` attribute from a `StopIteration` error (PEP 380 §3).
///
/// In CPython, `StopIteration.value` is the first positional argument: when
/// a generator does `return x`, the VM raises `StopIteration(x)` and `x` is
/// accessible as `e.value` and `e.args[0]`.
///
/// We mirror this by extracting `args[0]` from the materialized exception
/// instance, or by using the message string for `PyError::Named` variants.
/// Returns `None` when no value was provided (bare `return` / `return None`).
fn extract_stop_iteration_value(err: &PyError) -> Option<Value> {
    match err {
        PyError::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => {
                // Check for a `value` attribute first (set by our exception
                // machinery), then fall back to args[0].
                let borrow = inst.borrow();
                if let Some(v) = borrow.attrs.get("value") {
                    if !v.is_none() {
                        return Some(v.clone());
                    }
                }
                // Try args[0].
                if let Some(args_val) = borrow.attrs.get("args") {
                    if let Some(args) = args_val.as_tuple().or_else(|| args_val.as_list()) {
                        if let Some(first) = args.first() {
                            if !first.is_none() {
                                return Some(first.clone());
                            }
                        }
                    }
                }
                None
            }
            _ => None,
        },
        PyError::Named(name, msg) if name.as_ref() == "StopIteration" => {
            if msg.is_empty() {
                None
            } else {
                Some(Value::string(msg.clone()))
            }
        }
        _ => None,
    }
}

fn pep479_wrap_stop_iteration(env: &crate::interpreter::EnvRef, err: PyError) -> PyError {
    let built_in_stop = lookup_name_in_module(env, "StopIteration").and_then(|v| match v.kind() {
        ValueKind::PyClass(c) => Some(Rc::clone(c)),
        _ => None,
    });

    let is_stop = match &err {
        // VM-internal named raises (e.g. from builtin code): match by name.
        PyError::Named(cls, _) => cls.as_ref() == "StopIteration",
        // Class-identity raises: exact name check suffices for built-in StopIteration;
        // also walk the base chain for subclasses expressed as Class errors.
        PyError::Class(cls, _) => {
            let cls_rc = Rc::clone(cls);
            match built_in_stop {
                Some(ref base) => class_is_subclass_of(&cls_rc, base),
                // Fallback: name match only (startup before builtins are installed).
                None => cls.borrow().name == "StopIteration",
            }
        }
        // User raise (raise StopIteration / raise MyStop()) — the error carries
        // a fully materialised exception Value.
        PyError::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => {
                let cls = Rc::clone(&inst.borrow().class);
                match built_in_stop {
                    Some(ref base) => class_is_subclass_of(&cls, base),
                    None => cls.borrow().name == "StopIteration",
                }
            }
            ValueKind::PyClass(cls) => {
                let cls = Rc::clone(cls);
                match built_in_stop {
                    Some(ref base) => class_is_subclass_of(&cls, base),
                    None => cls.borrow().name == "StopIteration",
                }
            }
            _ => false,
        },
        _ => false,
    };

    if !is_stop {
        return err;
    }

    // Materialise the original StopIteration error into a Value so it can be
    // attached as __cause__ on the new RuntimeError.
    let cause_val: Option<Value> = match err {
        PyError::Raised(exc) => Some(exc),
        PyError::Class(cls, msg) => {
            let args = if msg.is_empty() {
                vec![]
            } else {
                vec![Value::string(msg)]
            };
            Some(instantiate_exception(cls, args))
        }
        PyError::Named(cls_name, msg) => {
            // Look up the class from the environment and instantiate it.
            lookup_name_in_module(env, cls_name.as_ref())
                .and_then(|v| match v.kind() {
                    ValueKind::PyClass(c) => Some(Rc::clone(c)),
                    _ => None,
                })
                .map(|cls| {
                    let args = if msg.is_empty() {
                        vec![]
                    } else {
                        vec![Value::string(msg)]
                    };
                    instantiate_exception(cls, args)
                })
        }
        _ => None,
    };

    // Build the RuntimeError instance and attach __cause__, __context__, and
    // __suppress_context__, mirroring CPython's PEP 479 behaviour.
    // CPython sets both __cause__ and __context__ to the original StopIteration
    // instance (they are the same object: `e.__context__ is e.__cause__`), and
    // sets __suppress_context__ = True so the "During handling of..." context
    // chain is suppressed in tracebacks.
    if let Some(cause) = cause_val {
        if let Some(rt_cls) = lookup_name_in_module(env, "RuntimeError").and_then(|v| match v.kind() {
            ValueKind::PyClass(c) => Some(Rc::clone(c)),
            _ => None,
        }) {
            let rt_err = instantiate_exception(
                rt_cls,
                vec![Value::string("generator raised StopIteration")],
            );
            if let ValueKind::PyInstance(inst) = rt_err.kind() {
                // Clone before the first insert so both __cause__ and __context__
                // share the same underlying Rc (preserving CPython identity: is).
                let context = cause.clone();
                inst.borrow_mut()
                    .attrs
                    .insert("__cause__".to_string(), cause);
                inst.borrow_mut()
                    .attrs
                    .insert("__context__".to_string(), context);
                inst.borrow_mut()
                    .attrs
                    .insert("__suppress_context__".to_string(), Value::bool_(true));
            }
            return PyError::Raised(rt_err);
        }
    }

    // Fallback: builtins not yet installed (startup) or materialisation failed.
    PyError::named("RuntimeError", "generator raised StopIteration")
}

#[cfg(test)]
mod vm_tests {
    use super::*;
    use crate::bytecode::{FnCode, Insn};
    use crate::interpreter::Interpreter;

    fn empty_code(insns: Vec<Insn>) -> FnCode {
        FnCode {
            insns,
            consts: vec![],
            names: vec![],
            num_regs: 0,
            num_iters: 0,
            num_locals: 0,
            fn_protos: vec![],
            cell_vars: vec![],
            is_generator: false,
        }
    }

    #[test]
    fn matchexcept_with_no_active_exception_returns_error() {
        // MatchExcept must error when no exception is active (compiler bug scenario).
        let mut code = empty_code(vec![]);
        code.num_regs = 1;
        code.insns.push(Insn::LoadNone(0));           // type_reg = None (placeholder)
        code.insns.push(Insn::MatchExcept(0, 1));     // no active_exception → error
        code.insns.push(Insn::ReturnNone);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![Value::unset(); 1];
        // SAFETY (test): regs is alive for the duration of run_bytecode;
        // no VmFrameView is active, so there is no concurrent access.
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(result.is_err(), "expected Err, got {:?}", result);
        assert!(
            result.unwrap_err().to_string().contains("no active exception"),
            "error should mention no active exception"
        );
    }

    #[test]
    fn oob_pc_returns_error_not_none() {
        // Jump(100): new_pc = 1 + 100 = 101 > insns.len() (1) → error
        let code = empty_code(vec![Insn::Jump(100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(result.is_err(), "expected Err for OOB jump, got {:?}", result);
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn negative_jump_returns_error() {
        // Jump(-100): new_pc = 1 + (-100) = -99 → underflow error
        let code = empty_code(vec![Insn::Jump(-100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(result.is_err(), "expected Err for negative jump, got {:?}", result);
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn normal_fallthrough_returns_none() {
        let code = empty_code(vec![Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        assert_eq!(interp.run_bytecode(&code, regs_slice).unwrap(), Value::none());
    }

    #[test]
    fn setup_except_negative_offset_returns_error() {
        // SetupExcept(-100): handler_pc = 1 + (-100) < 0 → error at push time
        let code = empty_code(vec![Insn::SetupExcept(-100), Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let regs_slice = unsafe { RegSlice::from_raw(regs.as_mut_ptr(), regs.len()) };
        let result = interp.run_bytecode(&code, regs_slice);
        assert!(result.is_err(), "expected Err for SetupExcept with OOB offset, got {:?}", result);
    }
}
