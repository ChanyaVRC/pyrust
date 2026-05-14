/// Inline capacity for the VM's per-frame register file.
///
/// `Value` is a NaN-boxed `u64` (8 bytes), so 16 inline slots = 128 bytes of
/// register storage embedded directly in the stack frame.  Most user functions
/// have fewer than 16 locals + temporaries, so this avoids the per-call heap
/// allocation that a fresh `Vec<Value>` would require.  Functions with more
/// than 16 registers transparently spill onto the heap (same big-O as before).
pub(crate) const VM_REGS_INLINE: usize = 16;

/// Per-frame register file backing for the VM.
///
/// All VM internals operate on `&mut [Value]` (via `SmallVec`'s `DerefMut`
/// blanket), so call sites that need a mutable slice work unchanged.
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
}

// Thread-local used to pass generator suspension state back from the VM loop
// to the resume_generator() caller without an extra return value or RefCell on
// the hot path.  Set immediately before `return Err(GeneratorYield(...))`.
//
// Fields, in order: saved `iters`, saved `exc_handlers`, saved `pc`, the
// generator-owned slice of `handled_exc_stack` (entries above the caller's
// base depth at the moment of yield), and `active_exception` at the moment
// of yield.  The two trailing fields are what fixes the PEP 3134
// "yield-inside-handler leaks context" gap.
type GenSaveState = (
    Vec<Option<IterState>>,
    Vec<usize>,
    usize,
    Vec<Value>,
    Option<Value>,
);
thread_local! {
    static GEN_SAVE: std::cell::RefCell<Option<GenSaveState>>
        = const { std::cell::RefCell::new(None) };
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
        BinaryOp::LShift => if b < 0 { None } else { Some(Value::int(a << (b & 63))) },
        BinaryOp::RShift => if b < 0 { None } else { Some(Value::int(a >> (b & 63))) },
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
    fn run_bytecode(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &mut [Value],
    ) -> Result<Value> {
        self.run_bytecode_inner(
            code,
            regs,
            vec![None; code.num_iters as usize],
            Vec::new(),
            0,
            None,
            None,
            Vec::new(),
            None,
        )
    }

    /// Like `run_bytecode` but also passes the current function's id so that
    /// `TailCall` instructions can perform self-call detection.
    fn run_bytecode_for_fn(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &mut [Value],
        fn_id: u64,
    ) -> Result<Value> {
        self.run_bytecode_inner(
            code,
            regs,
            vec![None; code.num_iters as usize],
            Vec::new(),
            0,
            Some(fn_id),
            None,
            Vec::new(),
            None,
        )
    }

    /// Resume (or initialise) a generator by executing from `frame.pc` until
    /// the next yield or completion.  Returns:
    /// - `Ok(val)`  — generator returned (StopIteration); frame.done = true
    /// - `Err(GeneratorYield(val))` — generator yielded; frame updated in-place
    /// - `Err(other)` — propagating exception
    pub(crate) fn resume_generator(&mut self, frame: &mut GeneratorFrame) -> Result<Value> {
        self.resume_generator_with_exc(frame, None)
    }

    /// Resume a generator, optionally injecting `inject_exc` as if it had been
    /// raised at the current yield point.  Underpins both `generator.close()`
    /// (with a `GeneratorExit`) and `generator.throw()` (with the user-supplied
    /// exception).
    ///
    /// Returns:
    /// - `Ok(val)` — generator returned (StopIteration); frame.done = true
    /// - `Err(GeneratorYield(val))` — generator yielded; frame updated in-place
    /// - `Err(other)` — propagating exception
    pub(crate) fn resume_generator_with_exc(
        &mut self,
        frame: &mut GeneratorFrame,
        inject_exc: Option<PyError>,
    ) -> Result<Value> {
        if frame.done {
            // A done generator can't be resumed; if the caller is injecting an
            // exception, propagate it unchanged so close()/throw() can decide
            // how to handle it.
            if let Some(e) = inject_exc {
                return Err(e);
            }
            return Err(PyError::named(
                "StopIteration",
                String::new(),
            ));
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
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Function,
            regs_ptr: frame.regs.as_ptr(),
            regs_len: frame.regs.len(),
            local_index: Rc::clone(&frame.local_index),
        });
        let result = self.run_bytecode_inner(
            &frame.code.clone(),
            &mut frame.regs,
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
            Err(PyError::GeneratorYield(val)) => {
                // Retrieve the saved state from the thread-local.
                let saved = GEN_SAVE.with(|cell| cell.borrow_mut().take());
                if let Some((
                    saved_iters,
                    saved_handlers,
                    saved_pc,
                    saved_handled_slice,
                    saved_active,
                )) = saved
                {
                    frame.iters = saved_iters;
                    frame.exc_handlers = saved_handlers;
                    frame.pc = saved_pc;
                    frame.handled_exc_slice = saved_handled_slice;
                    frame.active_exception = saved_active;
                } else {
                    unreachable!("GEN_SAVE must be set before every GeneratorYield");
                }
                Err(PyError::GeneratorYield(val))
            }
            Ok(_return_val) => {
                // Generator returned normally (fell off end or hit explicit `return`).
                // Signal exhaustion as StopIteration so ForIter and call_next handle it uniformly.
                frame.done = true;
                Err(PyError::named("StopIteration", String::new()))
            }
            Err(e) => {
                // Propagating exception or other error.
                frame.done = true;
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_inner(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &mut [Value],
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
    ) -> Result<Value> {
        // Record per-frame exception state at entry.  On any exit path
        // restore so the caller's frame sees the same `active_exception`
        // it left behind, regardless of whether this callee raised,
        // returned, ran past the end, or yielded.
        //
        // For yields, the `Yield` opcode itself has already split the
        // generator's slice of `handled_exc_stack` off into thread-local
        // storage and cleared `active_exception`, so the stack length is
        // back at the caller's depth.  For non-yield exits, truncate
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

        self.handled_exc_stack.truncate(exc_ctx_entry_depth);
        self.active_exception = saved_active;
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_inner_impl(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &mut [Value],
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
    ) -> Result<Value> {
        use crate::bytecode::Insn;
        use std::collections::HashMap;
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
                    Err(e) => {
                        if let Some(h) = exc_handlers.pop() {
                            let exc_val = match e {
                                PyError::Raised(v) => v,
                                PyError::Runtime(msg) => {
                                    match self.instantiate_named_exception("RuntimeError", msg) {
                                        Ok(v) => v,
                                        Err(e2) => return Err(e2),
                                    }
                                }
                                PyError::Named(cls, msg) => {
                                    match self.instantiate_named_exception(&cls, msg) {
                                        Ok(v) => v,
                                        Err(e2) => return Err(e2),
                                    }
                                }
                                other => return Err(other),
                            };
                            // PEP 3134 implicit chaining for *VM-implicit*
                            // raises (e.g. `1/0` producing
                            // `PyError::Named("ZeroDivisionError", ...)`
                            // from a `BinOp` opcode).  Explicit `raise`
                            // opcodes call `attach_implicit_context` at
                            // the raise site; this call covers everything
                            // else by attaching context at the point the
                            // VM materialises the exception value, BEFORE
                            // the pop-then-push reshuffle below removes
                            // the very entry we want to chain from.  The
                            // method is idempotent (it skips when
                            // `__context__` is already set), so the
                            // explicit-raise path is unaffected.
                            self.attach_implicit_context(&exc_val);
                            // If the exception we're now dispatching was
                            // raised *inside* an existing handler body in
                            // THIS frame, we are leaving that body — pop
                            // its context-stack entry so a future raise
                            // here doesn't pick it up.  Detection: the
                            // existing `active_exception` matches the top
                            // of `handled_exc_stack` exactly when control
                            // is currently inside a handler/finally body.
                            // The depth check protects entries belonging
                            // to caller frames.
                            if self.handled_exc_stack.len() > exc_ctx_frame_base
                                && let Some(top) = self.handled_exc_stack.last()
                                && let Some(active) = self.active_exception.as_ref()
                                && Self::values_are_same_exception(top, active)
                            {
                                self.handled_exc_stack.pop();
                            }
                            // Push the new exception so a `raise` inside
                            // the about-to-run handler/finally body sees
                            // it as the implicit `__context__`.
                            self.handled_exc_stack.push(exc_val.clone());
                            self.active_exception = Some(exc_val);
                            pc = h;
                            continue 'vm;
                        } else {
                            return Err(e);
                        }
                    }
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
                    return Ok(Value::none());
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
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    self.assign_name(name, val);
                }
                Insn::LoadNone(dst) => {
                    regs[*dst as usize] = Value::none();
                }
                Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
                    let v = vm_try!(vm_read(regs, *src, num_locals));
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
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
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
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
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
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = cv.clone();
                    let result = if let Some(v) = vm_try!(self.try_inplace_op(l.clone(), *op, r.clone())) {
                        v
                    } else {
                        vm_try!(self.eval_binary(l, *op, r))
                    };
                    regs[*dst as usize] = result;
                }
                Insn::UnaryOp(dst, op, src) => {
                    let val = vm_try!(vm_read(regs, *src, num_locals));
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
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    let result = vm_try!(self.get_attr(obj_val, name));
                    regs[*dst as usize] = result;
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let val_val = vm_try!(vm_read(regs, *val, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.assign_attr(obj_val, name, val_val));
                }
                Insn::DeleteAttr(obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
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

                    let idx_val = vm_try!(vm_read(regs, *idx, num_locals));
                    // Slice key: tuple of (lo, hi, step) produced by the compiler.
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
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
                                let key = vm_try!(self.value_to_pykey(&idx_val));
                                let lookup = vm_try!(self.dict_lookup(&dict_val, &key));
                                let r = vm_try!(
                                    lookup
                                        .map(|(_, v)| v)
                                        .ok_or_else(|| PyError::named("KeyError", idx_val.repr()))
                                );
                                regs[*dst as usize] = r;
                            }
                            FastResult::Miss => {
                                let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                                let r = vm_try!(self.eval_index(obj_val, idx_val));
                                regs[*dst as usize] = r;
                            }
                        }
                    }
                }
                Insn::SetItem(obj, idx, val) => {
                    let idx_val = vm_try!(vm_read(regs, *idx, num_locals));
                    let val_val = vm_try!(vm_read(regs, *val, num_locals));
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
                                let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
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
                                let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
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
                    let idx_val = vm_try!(vm_read(regs, *idx, num_locals));
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
                        }
                        if !handled {
                            // Try __delitem__ on user-defined instances.
                            let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
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
                    self.env.borrow_mut().values.remove(&name);
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
                    let cond_val = vm_try!(vm_read(regs, *cond, num_locals));
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
                    let cond_val = vm_try!(vm_read(regs, *cond, num_locals));
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
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
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
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
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
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
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
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
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
                    let type_val = vm_try!(vm_read(regs, *type_reg, num_locals));
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
                    let msg = vm_try!(vm_read(regs, *msg_reg, num_locals));
                    let msg_str = if msg.is_none() {
                        String::new()
                    } else {
                        msg.to_py_str()
                    };
                    let exc =
                        vm_try!(self.instantiate_named_exception("AssertionError", msg_str));
                    self.attach_implicit_context(&exc);
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseValue(src) => {
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    let exc = vm_try!(self.coerce_to_exception(val));
                    self.attach_implicit_context(&exc);
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseFrom(src, cause_reg) => {
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    let cause = vm_try!(vm_read(regs, *cause_reg, num_locals));
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
                    let func_val = vm_try!(vm_read(regs, *func_reg, num_locals));
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
                            value: vm_try!(vm_read(regs, *func_reg + 1 + i, num_locals)),
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
                    if let Some(fn_id) = fn_id_opt {
                        let mut key = std::mem::take(&mut self.key_scratch);
                        key.clear();
                        let mut all_hashable = true;
                        for i in 0..*argc as usize {
                            match regs[*func_reg as usize + 1 + i].to_key() {
                                Some(k) => key.push(k),
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
                    let func_val = vm_try!(vm_read(regs, *func_reg, num_locals));
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    for i in 0..crate::bytecode::Reg::from(*argc) {
                        buf.push(ExpandedCallArg {
                            name: None,
                            value: vm_try!(vm_read(regs, *func_reg + 1 + i, num_locals)),
                        });
                    }
                    let call_result = self.call_function_expanded(func_val, &buf);
                    self.call_arg_buf = buf;
                    regs[*func_reg as usize] = vm_try!(call_result);
                }

                Insn::CallMethod { dst, obj, name_idx, args_base, nargs } => {
                    let r = self.exec_call_method(regs, num_locals, *dst, *obj, *name_idx, *args_base, *nargs, code);
                    regs[*dst as usize] = vm_try!(r);
                }

                Insn::CallMethodExpanded { dst, obj, name_idx, pos_list, kw_dict } => {
                    let r = self.exec_call_method_expanded(regs, num_locals, *dst, *obj, *name_idx, *pos_list, *kw_dict, code);
                    regs[*dst as usize] = vm_try!(r);
                }

                // ── Returns ──────────────────────────────────────────────
                Insn::Return(src) => {
                    return Ok(vm_try!(vm_read(regs, *src, num_locals)));
                }
                Insn::ReturnNone => {
                    return Ok(Value::none());
                }

                // ── Tail-call ────────────────────────────────────────────
                Insn::TailCall { args_base, nargs } => {
                    // The function to call lives at func_reg = args_base - 1.
                    let func_reg = args_base - 1;
                    let callee_val = vm_try!(vm_read(regs, func_reg, num_locals));

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
                            let exc = vm_try!(self.instantiate_named_exception(
                                "RecursionError",
                                "maximum recursion depth exceeded".to_string(),
                            ));
                            return Err(PyError::Raised(exc));
                        }
                        // Collect new argument values before we overwrite any registers.
                        let mut new_args: Vec<Value> =
                            Vec::with_capacity(*nargs as usize);
                        for i in 0..*nargs as u32 {
                            new_args.push(vm_try!(vm_read(regs, args_base + i, num_locals)));
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
                                value: vm_try!(vm_read(regs, args_base + i, num_locals)),
                            });
                        }
                        let call_result = self.call_function_expanded(callee_val, &buf);
                        self.call_arg_buf = buf;
                        return Ok(vm_try!(call_result));
                    }
                }

                // ── Collection builders ──────────────────────────────────
                Insn::BuildList(dst, base, n) => {
                    let mut items: Vec<Value> = Vec::with_capacity(*n as usize);
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        items.push(vm_try!(vm_read(regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Value::list(items);
                }
                Insn::BuildTuple(dst, base, n) => {
                    let mut items = Vec::with_capacity(*n as usize);
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        items.push(vm_try!(vm_read(regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Value::tuple(items);
                }
                Insn::BuildDict(dst, base, n) => {
                    let mut dict = indexmap::IndexMap::new();
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        let k_val = vm_try!(vm_read(regs, *base + i * 2, num_locals));
                        let v_val = vm_try!(vm_read(regs, *base + i * 2 + 1, num_locals));
                        let key = vm_try!(self.value_to_pykey(&k_val));
                        vm_try!(self.dict_insert(&mut dict, key, v_val));
                    }
                    regs[*dst as usize] = Value::dict(dict);
                }
                Insn::SetAdd(set_reg, val_reg) => {
                    let val = vm_try!(vm_read(regs, *val_reg, num_locals));
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
                    let val = vm_try!(vm_read(regs, *val_reg, num_locals));
                    vm_try!(regs[*list_reg as usize].list_push(val));
                }
                Insn::ListExtend(list_reg, src_reg) => {
                    let src_val = vm_try!(vm_read(regs, *src_reg, num_locals));
                    // #446: route through `collect_iterable` so user
                    // `__iter__` / `__getitem__` classes are honoured.
                    // #448: write back via the scoped `list_extend`
                    // operation method (no `&mut Vec` crosses the API
                    // boundary).
                    let items_to_add = vm_try!(self.collect_iterable(src_val));
                    vm_try!(regs[*list_reg as usize].list_extend(items_to_add));
                }
                Insn::DictUpdate(dict_reg, src_reg) => {
                    let src_val = vm_try!(vm_read(regs, *src_reg, num_locals));
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
                    let yielded = vm_try!(vm_read(regs, *src, num_locals));
                    // Pre-fill dst with None: this is the sent value that the
                    // yield expression evaluates to on resumption.  Proper
                    // send() support would overwrite this in resume_generator.
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
                    // Save current iters/exc_handlers/pc to the thread-local
                    // so that resume_generator() can write them back into the
                    // GeneratorFrame after we unwind.
                    GEN_SAVE.with(|cell| {
                        *cell.borrow_mut() = Some((
                            iters.clone(),
                            exc_handlers.clone(),
                            pc, // already past the Yield instruction
                            saved_handled_slice,
                            saved_active,
                        ));
                    });
                    return Err(PyError::GeneratorYield(yielded));
                }

                // ── Unpack ───────────────────────────────────────────────
                Insn::Unpack(base, src, n) => {
                    let src_val = vm_try!(vm_read(regs, *src, num_locals));
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
                    let src_val = vm_try!(vm_read(regs, *src, num_locals));
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
                        let src_val = vm_try!(vm_read(regs, *src, num_locals));
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
                                Some(Err(PyError::GeneratorYield(val))) => {
                                    regs[*dst as usize] = val;
                                }
                                Some(Err(PyError::Raised(exc))) => {
                                    // Check for StopIteration: covers both `raise StopIteration()`
                                    // (PyInstance) and bare `raise StopIteration` (PyClass).
                                    let is_stop = match exc.kind() {
                                        ValueKind::PyInstance(inst) => {
                                            inst.borrow().class.borrow().name == "StopIteration"
                                        }
                                        ValueKind::PyClass(cls) => {
                                            cls.borrow().name == "StopIteration"
                                        }
                                        _ => false,
                                    };
                                    if is_stop {
                                        pc = jump_pc!(*offset);
                                    } else {
                                        vm_try!(Err(PyError::Raised(exc)));
                                    }
                                }
                                Some(Err(PyError::Named(ref cls, _)))
                                    if cls == "StopIteration" =>
                                {
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
                                vm_try!(vm_read(regs, *defs_base + def_slot, num_locals));
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
                    let (class_code, local_index) = {
                        let proto = pool_get!(code.fn_protos, *proto_idx, "fn_proto");
                        (Rc::clone(&proto.code), Rc::clone(&proto.local_index))
                    };
                    let num_class_regs = class_code.num_regs as usize;
                    let mut class_regs: RegsBuf = smallvec![Value::unset(); num_class_regs];
                    // Push a fresh class-store-order list onto the interpreter
                    // stack so `RecordClassStore` / `RecordClassDel` insns
                    // emitted inside the class body record into *this* class
                    // — supports `class A: class B: ...` nesting cleanly.
                    self.class_store_order.push(Vec::new());
                    let body_result = self.run_bytecode(&class_code, &mut class_regs);
                    // Always pop, even on error, to keep the stack balanced.
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
                    let base = if *bases_n > 0 {
                        let base_val = vm_try!(vm_read(regs, *bases_base, num_locals));
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
                    let class =
                        Rc::new(RefCell::new(PyClass { name: class_name, base, attrs }));
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
                    let val = vm_try!(vm_read(regs, *src, num_locals));
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
        regs: &mut [Value],
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
                    return format_str_template(template, &args, &[]);
                }
                if let Some(v) = regs[obj as usize].as_some() {
                    pyrust_builtins::string::call(&method, v, args)
                } else { unreachable!() }
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
        regs: &mut [Value],
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
                    return format_str_template(template, &pos_items, &keyword);
                }
                if let Some(v) = regs[obj as usize].as_some() {
                    pyrust_builtins::string::call(&method, v, pos_items)
                } else { unreachable!() }
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
                // CPython's throw(type, value=None, traceback=None) — we only
                // honour the first argument (the type or instance) and ignore
                // the legacy 3-arg form's value/traceback.
                let exc = args.into_iter().next().unwrap();
                self.generator_throw(receiver, exc)
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
        match self.resume_generator_with_exc(frame, Some(inject)) {
            // Generator yielded again — that's an error in CPython.
            Err(PyError::GeneratorYield(_)) => {
                // Mark as done so subsequent calls don't re-execute.
                frame.done = true;
                Err(PyError::named(
                    "RuntimeError",
                    "generator ignored GeneratorExit".to_string(),
                ))
            }
            // Generator returned normally (StopIteration synthesised).
            Err(PyError::Named(ref cls, _)) if cls == "StopIteration" => Ok(Value::none()),
            // Generator re-raised GeneratorExit — that's the expected close
            // behaviour, swallow it.
            Err(PyError::Named(ref cls, _)) if cls == "GeneratorExit" => Ok(Value::none()),
            Err(PyError::Raised(ref exc)) => {
                let cls_name = match exc.kind() {
                    ValueKind::PyInstance(inst) => inst.borrow().class.borrow().name.clone(),
                    ValueKind::PyClass(cls) => cls.borrow().name.clone(),
                    _ => String::new(),
                };
                if cls_name == "GeneratorExit" || cls_name == "StopIteration" {
                    Ok(Value::none())
                } else {
                    Err(PyError::Raised(exc.clone()))
                }
            }
            Err(e) => Err(e),
            Ok(_) => unreachable!("resume_generator_with_exc always returns Err"),
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
        match self.resume_generator_with_exc(frame, Some(inject)) {
            Err(PyError::GeneratorYield(v)) => Ok(v),
            // Generator returned normally — convert to StopIteration.
            Err(PyError::Named(ref cls, _)) if cls == "StopIteration" => {
                Err(PyError::named("StopIteration", String::new()))
            }
            // Any other propagating error (including a re-raise of the
            // injected exception) flows through unchanged.
            Err(e) => Err(e),
            Ok(_) => unreachable!("resume_generator_with_exc always returns Err"),
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
            ValueKind::Int(v) => Ok(Value::int(-v)),
            ValueKind::Float(v) => Ok(Value::float(-v)),
            ValueKind::Complex(re, im) => Ok(Value::complex(-re, -im)),
            _ => Err(PyError::named("TypeError", "bad operand type for unary -".to_string())),
        },
        UnaryOp::Not => Ok(Value::bool_(!val.truthy())),
        UnaryOp::BitNot => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(!v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { -2 } else { -1 })),
            _ => Err(PyError::named("TypeError",
                "bad operand type for unary ~: use integer".to_string(),
            )),
        },
        UnaryOp::Pos => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(v)),
            ValueKind::Float(v) => Ok(Value::float(v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
            _ => Err(PyError::named("TypeError", "bad operand type for unary +".to_string())),
        },
    }
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
        let result = interp.run_bytecode(&code, &mut regs);
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
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err for OOB jump, got {:?}", result);
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn negative_jump_returns_error() {
        // Jump(-100): new_pc = 1 + (-100) = -99 → underflow error
        let code = empty_code(vec![Insn::Jump(-100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err for negative jump, got {:?}", result);
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn normal_fallthrough_returns_none() {
        let code = empty_code(vec![Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        assert_eq!(interp.run_bytecode(&code, &mut regs).unwrap(), Value::none());
    }

    #[test]
    fn setup_except_negative_offset_returns_error() {
        // SetupExcept(-100): handler_pc = 1 + (-100) < 0 → error at push time
        let code = empty_code(vec![Insn::SetupExcept(-100), Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err for SetupExcept with OOB offset, got {:?}", result);
    }
}
