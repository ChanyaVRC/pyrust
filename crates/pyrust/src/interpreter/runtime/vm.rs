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

// SmallVec-backed types for per-frame collections.
// Most Python functions have ≤2 for-loops and ≤2 nested try/except blocks,
// so inline storage eliminates heap allocations for the common case.
pub(crate) type ItersBuf = smallvec::SmallVec<[Option<IterState>; 2]>;
pub(crate) type ExcHandlersBuf = smallvec::SmallVec<[usize; 2]>;
pub(crate) type HandledExcBuf = smallvec::SmallVec<[Value; 2]>;
pub(crate) type IterCacheBuf = smallvec::SmallVec<[Option<Value>; 4]>;
pub(crate) type IterSrcBuf = smallvec::SmallVec<[Value; 2]>;

/// Heap-allocated state for a built-in iterable wrapped by `iter()`.
/// Stored type-erased inside `Value::generator()` via `Box<dyn Any>`,
/// the same slot used for GeneratorFrame.  resume_generator() checks
/// which concrete type it has by downcasting.
///
/// `type_name` carries the Python-level iterator type name (e.g.
/// "list_iterator", "set_iterator") so that `type()` and error messages
/// report the right name instead of the generic "generator".  Internal
/// iterators that have no CPython-specified name use "generator".
pub(crate) struct NativeIterFrame {
    pub(crate) items: Vec<Value>,
    pub(crate) pos: usize,
    pub(crate) type_name: &'static str,
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

/// Callable-iterator created by `iter(callable, sentinel)` (the two-argument
/// form of the builtin).  Stored type-erased inside `Value::generator()` like
/// [`NativeIterFrame`] and [`GetItemIter`].
///
/// On each `next()` call the interpreter invokes `callable()` with no
/// arguments; if the result compares equal to `sentinel` the iterator is
/// exhausted (StopIteration) and the sentinel value is *not* yielded, matching
/// CPython semantics.  Once `done` is set subsequent `next()` calls return
/// StopIteration immediately without re-calling `callable`.
pub(crate) struct CallableIter {
    pub(crate) callable: Value,
    pub(crate) sentinel: Value,
    pub(crate) done: bool,
}

/// Lazy iterator for `map(func, iter1, iter2, ...)`.
///
/// `sources` holds already-converted iterator objects (result of calling
/// `iter()` on the original arguments at construction time).  Each call to
/// `step_map_iter` advances every source by one element via `call_next` and
/// invokes `func` with the resulting row.  No items are consumed from the
/// sources until the first `next()` call on the map object.
pub(crate) struct MapIter {
    pub(crate) func: Value,
    /// Already-converted iterators (one per positional argument after `func`).
    /// `IterSrcBuf` avoids a heap allocation for the common 1- or 2-argument case.
    pub(crate) sources: IterSrcBuf,
    /// Set to `true` once any source raises `StopIteration`.
    pub(crate) done: bool,
}

/// Lazy iterator for `filter(func, iterable)`.
///
/// `source` is the already-converted iterator object (result of calling
/// `iter()` on the original iterable at construction time).  No items are
/// consumed from the source until the first `next()` call on the filter object.
/// `func` is `None` when the Python caller passed `None` (identity test).
pub(crate) struct FilterIter {
    pub(crate) func: Option<Value>,
    /// Already-converted iterator object.
    pub(crate) source: Value,
    /// Set to `true` once the source raises `StopIteration`.
    pub(crate) done: bool,
}

/// Lazy iterator for `enumerate(iterable, start=0)`.
///
/// `source` is the already-converted iterator object (result of calling
/// `make_iterator()` on the original iterable at construction time).  No items
/// are consumed from the source until the first `next()` call on the enumerate
/// object.  Each step advances `source` by one element via `call_next` and
/// wraps it with the running counter.
pub(crate) struct EnumerateIter {
    /// Already-converted iterator object.
    pub(crate) source: Value,
    /// Current counter value; incremented after each yielded pair.
    pub(crate) counter: i64,
    /// Set to `true` once the source raises `StopIteration`.
    pub(crate) done: bool,
}

/// Lazy iterator for `zip(it1, it2, ..., strict=False)`.
///
/// `sources` holds already-converted iterator objects (result of calling
/// `make_iterator()` on the original arguments at construction time).  Each
/// step advances all sources by one element via `call_next`.  Stops at the
/// shortest source (or raises `ValueError` when `strict=True` and lengths
/// differ).
pub(crate) struct ZipIter {
    /// Already-converted iterators (one per positional argument).
    pub(crate) sources: Vec<Value>,
    /// When `true`, a length mismatch raises `ValueError` matching CPython's
    /// wording.
    pub(crate) strict: bool,
    /// Set to `true` once any source raises `StopIteration`.
    pub(crate) done: bool,
    /// Count of tuples yielded so far; used for `strict` error messages.
    pub(crate) count: usize,
}

/// Heap-allocated execution state for a suspended generator.
/// Stored type-erased inside `Value::generator()` via `Box<dyn Any>`.
pub(crate) struct GeneratorFrame {
    pub(crate) code: Rc<crate::bytecode::FnCode>,
    pub(crate) regs: RegsBuf,
    pub(crate) iters: ItersBuf,
    pub(crate) exc_handlers: ExcHandlersBuf,
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
    pub(crate) handled_exc_slice: HandledExcBuf,
    /// The interpreter's `active_exception` at the moment of yield.
    /// Saved and restored alongside `handled_exc_slice` so that resume
    /// can re-establish the suspended frame's exception view without
    /// disturbing the caller's.
    pub(crate) active_exception: Option<Value>,
    /// Parallel save-stack for `active_exception` across nested `except`
    /// blocks within this generator frame.  Split off at yield time
    /// (alongside `handled_exc_slice`) and re-extended on resume.
    pub(crate) exc_saved_active_slice: Vec<Option<Value>>,
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
    /// The name of the generator function, used to populate traceback frames
    /// when an exception propagates out of the generator body (issue #908).
    /// `Arc<str>` so `.clone()` in `resume_generator_with_exc` is a
    /// reference-count bump rather than a heap allocation on every resume.
    pub(crate) fn_name: std::sync::Arc<str>,
    /// Fully-qualified name of the generator function (issue #1270).
    /// Exposed as `g.__qualname__`.
    pub(crate) qualname: std::sync::Arc<str>,
}

/// Explicit suspension state for a generator frame.
///
/// Replaces the old `GEN_SAVE` thread-local + `GenSaveState` tuple alias.
/// All suspension state is now carried as a struct field in `FrameOutcome::Yielded`
/// rather than smuggled through a side-channel.
pub(crate) struct GenSaveState {
    pub(crate) iters: ItersBuf,
    pub(crate) exc_handlers: ExcHandlersBuf,
    pub(crate) pc: usize,
    pub(crate) handled_exc_slice: HandledExcBuf,
    pub(crate) active_exception: Option<Value>,
    /// Parallel save-stack slice for `exc_saved_active`, split off at yield
    /// time alongside `handled_exc_slice` and re-extended on resume.
    pub(crate) exc_saved_active_slice: Vec<Option<Value>>,
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
    /// Lazy: owns the list/tuple Value (used when the source is a temp register
    /// and cannot be accessed by slot index).  Avoids the extra Vec allocation
    /// and N element clones that `Materialized` would incur via `iter_values`.
    ValueIndexed { value: Value, pos: usize },
    /// User-defined iterator: holds the iterator object (result of __iter__).
    /// Each ForIter call invokes __next__() on it and stops on StopIteration.
    UserDefined(Value),
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
            smallvec::smallvec![None; code.num_iters as usize],
            ExcHandlersBuf::new(),
            0,
            None,
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
            smallvec::smallvec![None; code.num_iters as usize],
            ExcHandlersBuf::new(),
            0,
            Some(fn_id),
            None,
            HandledExcBuf::new(),
            None,
            Vec::new(),
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
                pyrust_core::py_err!("StopIteration", String::new())
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
        });
        // SAFETY: regs_ptr is valid for regs_len Values for the lifetime of
        // frame.regs (which outlives this call).  No &mut [Value] referencing
        // frame.regs is held while the dispatch loop runs; RegSlice (raw
        // pointer + len) is used instead, removing the LLVM noalias constraint
        // that made the VmFrameView dereferences UB (issue #547).
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
        // Push a traceback frame so that exceptions propagating out of the
        // generator body carry the generator function's name in the chain
        // (issue #908: the regular-call path in calls.rs does this for normal
        // functions; the generator resume path was missing it).
        // Cloning an `Arc<str>` is a cheap reference-count bump; no
        // heap allocation per resume.
        let tb_filename = self
            .script_filename
            .clone()
            .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
        pyrust_core::push_traceback_frame(pyrust_core::FrameInfo {
            filename: tb_filename,
            lineno: None,
            source_line: None,
            funcname: frame.fn_name.clone(),
        });
        let result = self.run_bytecode_inner(
            &frame.code,
            regs_slice,
            std::mem::take(&mut frame.iters),
            std::mem::take(&mut frame.exc_handlers),
            frame.pc,
            None,
            inject_exc,
            gen_handled,
            gen_active,
            gen_exc_saved_active,
        );
        // Pop the traceback frame; capture the chain if an error occurred.
        // On yield (Ok(Yielded)) the call succeeded from the traceback's
        // perspective — the error has not propagated yet.
        let is_err = matches!(result, Err(_));
        pyrust_core::pop_traceback_frame(is_err);
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
                    pyrust_core::py_err!("StopIteration", String::new())
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
        iters_init: ItersBuf,
        exc_handlers_init: ExcHandlersBuf,
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
            current_fn_id,
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
        exc_handlers: &mut ExcHandlersBuf,
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
                let args = if msg.is_empty() { vec![] } else { vec![Value::string(msg)] };
                instantiate_exception(cls, args)
            }
            PyError::KeyError(key) => {
                match self.instantiate_named_exception_with_value("KeyError", key) {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            PyError::NameError { class_name, message, name } => {
                match self.instantiate_name_error_exception(class_name, message, name) {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            PyError::AttributeError { message, name, obj } => {
                match self.instantiate_attribute_error_exception(message, name, obj) {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            PyError::ImportError { class_name, message, module_name } => {
                match self.instantiate_import_error_exception(class_name, message, module_name) {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            PyError::OsError {
                class_name,
                errno,
                strerror,
                filename,
                filename2,
            } => {
                match self
                    .instantiate_os_error_exception(class_name, errno, strerror, filename, filename2)
                {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            PyError::UnicodeDecodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => {
                match self.instantiate_unicode_decode_error_exception(
                    encoding, object, start, end, reason,
                ) {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            PyError::UnicodeEncodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => {
                match self.instantiate_unicode_encode_error_exception(
                    encoding, object, start, end, reason,
                ) {
                    Ok(v) => v,
                    Err(e2) => return Err(e2),
                }
            }
            other => return Err(other),
        };
        self.attach_implicit_context(&exc_val);
        // Issue #1441: set __traceback__ on the exception instance when it is
        // caught.  Only PyInstance values (all normal exceptions) have an attrs
        // dict to write to.
        if let ValueKind::PyInstance(inst_rc) = exc_val.kind() {
            inst_rc
                .borrow_mut()
                .attrs
                .insert("__traceback__".to_string(), pyrust_builtins::traceback::make_traceback());
        }
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

    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_inner_impl(
        &mut self,
        code: &crate::bytecode::FnCode,
        mut regs: RegSlice,
        iters_init: ItersBuf,
        exc_handlers_init: ExcHandlersBuf,
        start_pc: usize,
        current_fn_id: Option<u64>,
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

        let num_locals = code.num_locals;

        let mut iters: ItersBuf = iters_init;
        let mut iter_next_cache: IterCacheBuf = smallvec::smallvec![None; iters.len()];
        let mut exc_handlers: ExcHandlersBuf = exc_handlers_init;
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
        // Parallel base for `exc_saved_active`.  Bounded pops/split-off in
        // EndExcept, RaiseReRaise, and Yield mirror the handling above.
        let exc_saved_active_frame_base: usize = self.exc_saved_active.len();

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
            // Update the current-line tracker when the lineno table has a
            // non-zero entry for this instruction.  `0` means "same line as
            // the previous instruction" and is deliberately not updated here.
            if let Some(&ln) = code.lineno_table.get(pc) {
                if ln != 0 {
                    pyrust_core::set_current_vm_line(ln);
                }
            }
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
                    // ── Inline cache fast path ────────────────────────────
                    // Check the per-name cache slot before doing any env-chain
                    // walk.  A hit requires the cached version == global_env_version
                    // (module env unchanged since the value was last resolved).
                    // Builtins are now also cached with the current
                    // global_env_version so that a module-level assignment of the
                    // same name (e.g. `len = my_fn`) correctly invalidates the
                    // cached builtin and the slow path re-resolves to the new
                    // module-scope value.
                    let cur_ver = self.global_env_version.get();
                    {
                        let cache = code.global_cache.borrow();
                        if (*name_idx as usize) < cache.len() {
                            let entry = &cache[*name_idx as usize];
                            if entry.0 == cur_ver {
                                regs[*dst as usize] = entry.1.clone();
                                continue;
                            }
                        }
                    }
                    // ── Slow path: full name resolution ──────────────────
                    let name = pool_get!(code.names, *name_idx, "name");
                    // Issue #706: look up the name through the env chain first
                    // (this covers function-level cell vars, nonlocal bindings,
                    // and module globals already in env.values).  Fall through to
                    // module_globals_dict only when the env chain misses — that
                    // handles names inserted via `globals()["x"] = val` (which
                    // mutates the dict directly and bypasses assign_name /
                    // env.values).  Putting the dict check BEFORE lookup_name
                    // was wrong: a function-level cell var named "g" would have
                    // shadowed by a same-named module global in the dict.
                    let (val, cache_ver) = if let Some(v) = vm_try!(self.lookup_name(name)) {
                        // Value came from the env chain.  Cache it only when the
                        // value actually came from the MODULE env — the only env
                        // whose mutations are tracked by `global_env_version`.
                        //
                        // Two cases where the value came from the module env:
                        //   1. The name is explicitly `global`-declared in the
                        //      current function: `lookup_name` went directly to the
                        //      module env via `lookup_name_in_module`.
                        //   2. `self.env` IS the module env (no parent): we are at
                        //      module scope, so any env-chain hit IS in the module env.
                        //
                        // Critically, we MUST NOT check `lookup_name_in_module(name)`
                        // here.  That check is true whenever the module env has a
                        // name with the same key — even if `lookup_name` found the
                        // value in an INTERMEDIATE env (e.g. an outer function's
                        // cell-var env).  Caching the cell-var value with `cur_ver`
                        // would cause stale results after a nonlocal write that
                        // changes the cell var but does not bump `global_env_version`
                        // (bug found during review: `z = "module_z"` at module scope
                        // combined with `z` as a cell var in an enclosing function
                        // triggered incorrect cache hits in the inner closure).
                        let in_module_env = {
                            let env = self.env.borrow();
                            env.global_names.contains(name) || env.parent.is_none()
                        };
                        let ver = if in_module_env { cur_ver } else { GLOBAL_CACHE_EMPTY };
                        (v, ver)
                    } else if let Some(v) = self
                        .module_globals_dict
                        .dict_with(|d| d.get(&StrKey(name)).cloned())
                        .flatten()
                    {
                        // Fallback: pick up globals()["x"] = val mutations that
                        // write to the dict without going through assign_name.
                        // StrKey avoids a String allocation on every miss.
                        // Do NOT cache this value: we don't track when the dict
                        // is mutated directly, so we can't guarantee correctness.
                        (v, GLOBAL_CACHE_EMPTY)
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
                        // Script-frame register fallback: cache as a regular
                        // env-version value (assign_name bumps the version when
                        // these registers change).
                        (v, cur_ver)
                    } else {
                        // Issue #1810: delegate to a #[cold] helper that checks
                        // whether globals["__builtins__"] is a restricted dict.
                        // Keeping this in a separate function avoids growing
                        // run_bytecode_inner_impl's I-cache footprint on the
                        // common (cache-hit) path.
                        vm_try!(resolve_global_via_builtins(
                            &self.module_globals_dict,
                            name,
                            cur_ver,
                        ))
                    };
                    // Update the cache slot.
                    if cache_ver != GLOBAL_CACHE_EMPTY {
                        code.global_cache.borrow_mut()[*name_idx as usize] =
                            (cache_ver, val.clone());
                    }
                    regs[*dst as usize] = val;
                }
                Insn::StoreGlobal(name_idx, src) => {
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
                    vm_try!(self.exec_binop(&mut regs, code, pc, *dst, *lhs, *op, *rhs, num_locals));
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
                    let result = if let Some(v) = vm_try!(self.try_inplace_op(&l, *op, &r, true)) {
                        v
                    } else {
                        vm_try!(self.eval_binary(l, *op, r))
                    };
                    regs[*dst as usize] = result;
                }
                Insn::BinOpConst(dst, lhs, op, const_idx, is_aug) => {
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
                    // `is_aug` is carried in the opcode: true when this fused op came
                    // from an augmented assignment (emit_aug_binop), false when it
                    // was const-folded from a plain binary expression.  The old
                    // `dst == lhs` heuristic mis-fired because `ensure_dst` reuses
                    // the lhs temp for plain binary ops too (issue #1874).
                    let result = if let Some(v) = vm_try!(self.try_inplace_op(&l, *op, &r, *is_aug)) {
                        v
                    } else {
                        vm_try!(self.eval_binary(l, *op, r))
                    };
                    regs[*dst as usize] = result;
                }
                Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
                    let imm_i64 = *imm as i64;
                    if let Some(a) = regs[*lhs as usize].as_int()
                        && let Some(result) = int_int_fast(a, imm_i64, *op)
                    {
                        regs[*dst as usize] = result;
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = Value::int(imm_i64);
                    // See BinOpConst above: `is_aug` distinguishes augmented assign
                    // from a const-folded plain binary expression (issue #1874).
                    let result = if let Some(v) = vm_try!(self.try_inplace_op(&l, *op, &r, *is_aug)) {
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
                                // Issue #1204: for scalar primitive subclasses
                                // (MyInt, MyFloat, …), extract the backing value
                                // before the built-in unary path, since the
                                // primitive's type slots aren't registered on the
                                // user class.
                                let is_instance = matches!(val.kind(), ValueKind::PyInstance(_));
                                let operand = if is_instance {
                                    if let ValueKind::PyInstance(inst) = val.kind() {
                                        instance_builtin_data(inst).unwrap_or_else(|| val.clone())
                                    } else {
                                        val
                                    }
                                } else {
                                    val
                                };
                                vm_try!(vm_eval_unary(*op, operand))
                            }
                        } else {
                            vm_try!(vm_eval_unary(*op, val))
                        }
                    };
                    regs[*dst as usize] = result;
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
                    let base_idx = *base as usize;
                    let n = *count as usize;
                    // Deref RegSlice → &[Value] so range-indexing returns &[Value].
                    let slice = &(&*regs)[base_idx..base_idx + n];
                    // Fast path: all operands are strings — concatenate in one allocation.
                    if slice.iter().all(|v| v.as_str().is_some()) {
                        let total_len: usize =
                            slice.iter().map(|v| v.as_str().unwrap().len()).sum();
                        let mut result = String::with_capacity(total_len);
                        for v in slice {
                            result.push_str(v.as_str().unwrap());
                        }
                        regs[*dst as usize] = Value::string(result);
                    } else {
                        // Fallback: sequential BinOp(Add) — correct but allocates intermediates.
                        let mut acc = vm_try!(vm_read(&*regs, *base, num_locals));
                        for k in 1..n {
                            let next = vm_try!(vm_read(&*regs, *base + k as u32, num_locals));
                            acc = vm_try!(self.eval_binary(acc, crate::ast::BinaryOp::Add, next));
                        }
                        regs[*dst as usize] = acc;
                    }
                }

                // ── Attribute / Index ────────────────────────────────────
                Insn::GetAttr(dst, obj, name_idx) => {
                    // Full body (InstanceAttr / ClassAttr inline-cache hit +
                    // slow-path get_attr / cache fill / invalidation) lives in
                    // fast_path.rs::exec_get_attr.
                    vm_try!(self.exec_get_attr(&mut regs, code, pc, *dst, *obj, *name_idx, num_locals));
                }
                Insn::GetAttrForWith(dst, obj, name_idx, missed_exit) => {
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    let type_name = value_type_name_str(&obj_val);
                    match self.get_attr(&obj_val, name) {
                        Ok(v) => regs[*dst as usize] = v,
                        Err(_) => {
                            // CPython converts any lookup failure (AttributeError or
                            // otherwise) to TypeError for the context manager protocol.
                            let msg = if *missed_exit {
                                format!(
                                    "'{}' object does not support the context manager protocol \
                                     (missed __exit__ method)",
                                    type_name
                                )
                            } else {
                                format!(
                                    "'{}' object does not support the context manager protocol",
                                    type_name
                                )
                            };
                            vm_try!(Err(pyrust_core::type_err!(msg)));
                        }
                    }
                }
                Insn::ImportFromAttr(dst, mod_reg, name_idx) => {
                    let mod_val = vm_try!(vm_read(&regs, *mod_reg, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    let result = self.get_attr(&mod_val, name);
                    match result {
                        Ok(v) => regs[*dst as usize] = v,
                        Err(e) if e.class_name_is("AttributeError") => {
                            // Get the module name for the error message by reading
                            // directly from the register (no extra clone needed).
                            let mod_name = match regs[*mod_reg as usize].kind() {
                                ValueKind::PyModule(m) => m.borrow().name.clone(),
                                _ => "<unknown>".to_string(),
                            };
                            vm_try!(Err(PyError::import_error(
                                "ImportError",
                                format!(
                                    "cannot import name '{name}' from '{mod_name}' (unknown location)"
                                ),
                                Some(mod_name),
                            )));
                        }
                        Err(e) => vm_try!(Err(e)),
                    }
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    // Full body (SetInstanceAttr write-cache hit + slow-path
                    // assign_attr / cache fill / invalidation, #1998) lives in
                    // fast_path.rs::exec_set_attr.
                    vm_try!(self.exec_set_attr(&mut regs, code, pc, *obj, *name_idx, *val, num_locals));
                }
                Insn::DeleteAttr(obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.delete_attr(obj_val, name));
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
                Insn::DeleteLocal(reg, name_idx) => {
                    // Raise NameError / UnboundLocalError when deleting an
                    // unbound fastlocal, matching CPython semantics (issue #846).
                    // Skip the check (name_idx == u16::MAX) for compiler-
                    // guaranteed-bound deletions such as PEP 3110
                    // `except E as var:` cleanup.
                    //
                    // Use the frame-view stack to detect the actual scope rather
                    // than checking `self.env.parent`.  When a function has no
                    // global/nonlocal/cell vars, `self.env` is set to the
                    // closure's captured env (often the module env, which has
                    // `parent == None`), so the env-parent check gives a false
                    // "module scope" reading inside those functions.
                    let is_module_scope = self
                        .vm_frame_views
                        .last()
                        .map(|v| v.kind == FrameKind::Script)
                        .unwrap_or(false);
                    if *name_idx != u16::MAX && regs[*reg as usize].is_unset() {
                        let name = pool_get!(code.names, *name_idx, "name");
                        if is_module_scope {
                            vm_try!(Err(PyError::name_error(
                                "NameError",
                                format!("name '{}' is not defined", name),
                                Some(name.to_string()),
                            )));
                        } else {
                            vm_try!(Err(PyError::name_error(
                                "UnboundLocalError",
                                format!(
                                    "cannot access local variable '{}' where it is not associated with a value",
                                    name
                                ),
                                None,
                            )));
                        }
                    }
                    // At function scope: the register is the only binding for
                    // this variable (fastlocals are not in env.values unless
                    // they are cell vars, which use env and not registers).
                    // Call __del__ immediately if no other named variable holds
                    // the same instance.
                    //
                    // At module scope: the compiler also emits DeleteModuleGlobal
                    // right after this instruction.  Module-scope fastlocals
                    // can also be in module_globals_dict when globals() was
                    // called (globals_accessed == true).  If globals_accessed is
                    // false the register was the only binding, so we can call
                    // __del__ here.  If globals_accessed is true the dict may
                    // hold a ref, so we defer to DeleteModuleGlobal which will
                    // remove the dict entry and then check.
                    let deleted_val = std::mem::replace(&mut regs[*reg as usize], Value::unset());
                    if (!is_module_scope || !self.globals_accessed) && !deleted_val.is_unset() {
                        call_del_if_last_binding(self, deleted_val, &regs, code.num_locals as usize);
                    }
                }
                Insn::SyncModuleGlobal(reg, name_idx) => {
                    // Always bump global_env_version: this instruction fires on
                    // every module-scope assignment that uses a fastlocal register.
                    // Without the bump, a `LoadGlobal` inline cache that resolved
                    // a name to a builtin at `cur_ver` would never be invalidated
                    // when the same name is later shadowed at module scope
                    // (e.g. `len = my_fn`), because `SyncModuleGlobal` is the
                    // only store instruction executed — `StoreGlobal` (which also
                    // bumps the version) is only used for `global`-declared names.
                    bump_global_env_version(self);
                    if self.globals_accessed {
                        let name = pool_get!(code.names, *name_idx, "name");
                        let val = regs[*reg as usize].clone();
                        if !val.is_unset() {
                            let _ = self.module_globals_dict.dict_insert(
                                PyKey::str_from(name),
                                val,
                            );
                        }
                    }
                }
                Insn::DeleteModuleGlobal(name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name");
                    // Capture the removed value so we can check for __del__ after
                    // all module-level bindings (env + dict) have been released.
                    // `from_env` is Some only for module globals stored via
                    // StoreGlobal (e.g. `global x; x = ...` from inside a function
                    // or module vars without a fastlocal register).
                    let from_env = module_env(&self.env).borrow_mut().values.remove(name);
                    // Always remove from module_globals_dict regardless of
                    // globals_accessed: pre-seeded dunders (__name__, __doc__,
                    // __builtins__) live in the dict unconditionally and must
                    // be cleared when deleted, otherwise they can resurface
                    // through a subsequent globals() lookup (issue #846).
                    // Capture the removed value to check __del__ after.
                    let from_dict = self.module_globals_dict
                        .dict_shift_remove(&PyKey::str_from(name))
                        .ok()
                        .flatten();
                    // Invalidate the LoadGlobal inline cache.
                    bump_global_env_version(self);
                    // The preceding DeleteLocal already cleared the fastlocal
                    // register.  Now that env.values and module_globals_dict have
                    // also been cleared, check whether this was the last
                    // Python-visible binding to the instance.  Use the same
                    // register scan as DeleteLocal so that module-level temp
                    // registers (>= num_locals) do not prevent __del__ from firing.
                    let del_val = from_env.or(from_dict);
                    if let Some(val) = del_val {
                        call_del_if_last_binding(self, val, &regs, code.num_locals as usize);
                    }
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
                    if let (Some(a), Some(b)) = (
                        regs[*lhs as usize].as_int(),
                        regs[*rhs as usize].as_int(),
                    ) && let Some(cond) = int_cmp(a, b, *op) {
                        if !cond { pc = jump_pc!(*offset); }
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(&regs, *rhs, num_locals));
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfTrue(lhs, op, rhs, offset) => {
                    if let (Some(a), Some(b)) = (
                        regs[*lhs as usize].as_int(),
                        regs[*rhs as usize].as_int(),
                    ) && let Some(cond) = int_cmp(a, b, *op) {
                        if cond { pc = jump_pc!(*offset); }
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(&regs, *rhs, num_locals));
                    if vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfFalseConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let (Some(a), Some(b)) = (regs[*lhs as usize].as_int(), cv.as_int())
                        && let Some(cond) = int_cmp(a, b, *op)
                    {
                        if !cond { pc = jump_pc!(*offset); }
                        continue;
                    }
                    let lv = &regs[*lhs as usize];
                    if let (ValueKind::Str(ls), ValueKind::Str(rs)) = (lv.kind(), cv.kind())
                        && let Some(cond) = str_cmp(ls, rs, *op)
                    {
                        if !cond { pc = jump_pc!(*offset); }
                        continue;
                    }
                    let l = vm_try!(vm_read(&regs, *lhs, num_locals));
                    let r = cv.clone();
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfTrueConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let (Some(a), Some(b)) = (regs[*lhs as usize].as_int(), cv.as_int())
                        && let Some(cond) = int_cmp(a, b, *op)
                    {
                        if cond { pc = jump_pc!(*offset); }
                        continue;
                    }
                    let lv = &regs[*lhs as usize];
                    if let (ValueKind::Str(ls), ValueKind::Str(rs)) = (lv.kind(), cv.kind())
                        && let Some(cond) = str_cmp(ls, rs, *op)
                    {
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
                    // `assert False, expr` — pass the raw value as AssertionError(expr).
                    let msg = vm_try!(vm_read(&regs, *msg_reg, num_locals));
                    let exc = if let Some(cls) = self.exc_classes.get("AssertionError") {
                        instantiate_exception(cls, vec![msg])
                    } else {
                        let class = lookup_exc_class("AssertionError").ok_or_else(|| {
                            PyError::Runtime(
                                "built-in exception 'AssertionError' is not defined".to_string(),
                            )
                        });
                        instantiate_exception(vm_try!(class), vec![msg])
                    };
                    self.attach_implicit_context(&exc);
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseAssertNoMsg => {
                    // `assert False` (no message) — AssertionError() with empty args.
                    let exc = if let Some(cls) = self.exc_classes.get("AssertionError") {
                        instantiate_exception(cls, vec![])
                    } else {
                        let class = lookup_exc_class("AssertionError").ok_or_else(|| {
                            PyError::Runtime(
                                "built-in exception 'AssertionError' is not defined".to_string(),
                            )
                        });
                        instantiate_exception(vm_try!(class), vec![])
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
                    let cause_raw = vm_try!(vm_read(&regs, *cause_reg, num_locals));
                    let exc = vm_try!(self.coerce_to_exception(val));
                    // PEP 3134: validate cause before storing.  CPython accepts
                    // `None` or a BaseException instance/subclass; anything else
                    // is a TypeError.  A class cause is auto-instantiated.
                    let cause = vm_try!(self.coerce_to_exception_cause(cause_raw));
                    // PEP 3134: `raise X from Y` sets `__cause__` AND
                    // `__suppress_context__`, but `__context__` is still
                    // populated so that the chain is observable.
                    self.attach_implicit_context(&exc);
                    if let ValueKind::PyInstance(inst) = exc.kind() {
                        inst.borrow_mut().attrs.insert("__cause__".to_string(), cause);
                        inst.borrow_mut()
                            .attrs
                            .insert("__suppress_context__".to_string(), Value::bool_(true));
                    }
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
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }

                Insn::MatchClassPositional { dst_base, subj, cls, n } => {
                    let n = *n as usize;
                    let cls_val = vm_try!(vm_read(&regs, *cls, num_locals));
                    let subj_val = vm_try!(vm_read(&regs, *subj, num_locals));

                    // Determine the class name for TypeError messages.
                    let cls_name = match cls_val.kind() {
                        ValueKind::PyClass(rc) => rc.borrow().name.clone(),
                        _ => "<class>".to_string(),
                    };

                    // Load __match_args__ from the class.
                    let match_args = match self.get_attr(&cls_val, "__match_args__") {
                        Ok(v) => v,
                        Err(e) if e.class_name_is("AttributeError") => {
                            vm_try!(Err(pyrust_core::type_err!("{cls_name}() accepts 0 positional sub-patterns ({n} given)")));
                            unreachable!()
                        }
                        Err(e) => {
                            vm_try!(Err(e));
                            unreachable!()
                        }
                    };

                    // __match_args__ must be a tuple (CPython 3.12 rejects lists).
                    // Use as_tuple() to borrow the slice directly — avoids cloning
                    // all elements into a Vec just to index them.
                    let match_args_len = match match_args.as_tuple() {
                        Some(items) => items.len(),
                        None => {
                            let type_name = value_type_name_str(&match_args);
                            vm_try!(Err(pyrust_core::type_err!("{cls_name}.__match_args__ must be a tuple (got {type_name})")));
                            unreachable!()
                        }
                    };

                    // Length must be >= n.
                    if match_args_len < n {
                        let plural = if match_args_len == 1 { "" } else { "s" };
                        vm_try!(Err(pyrust_core::type_err!("{cls_name}() accepts {} positional sub-pattern{plural} ({n} given)",
                                match_args_len)));
                    }

                    // For each positional index, get the attribute name from
                    // __match_args__[i] and load that attribute from the subject.
                    // Index into the tuple directly — `match_args` is an owned Value
                    // on the stack, so borrowing its slice does not conflict with
                    // the `&mut self` in `get_attr`.
                    for i in 0..n {
                        let attr_name = {
                            let items = match_args.as_tuple().unwrap();
                            let name_val = &items[i];
                            match name_val.as_str() {
                                Some(s) => s.to_string(),
                                None => {
                                    let type_name = value_type_name_str(name_val);
                                    vm_try!(Err(pyrust_core::type_err!("__match_args__ elements must be strings (got {type_name})")));
                                    unreachable!()
                                }
                            }
                        };
                        let attr_val = vm_try!(self.get_attr(&subj_val, &attr_name));
                        regs[(*dst_base as usize) + i] = attr_val;
                    }
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
                    // `CallMemo` marks a call to a statically-pure callee — the
                    // optimizer relies on that purity for DCE / TCO.  The former
                    // result-memoization (fn_cache probe + store) was removed
                    // (#1987): it was a net loss for the common varying-argument
                    // case (it paid a key-build + hash every call for a cache
                    // that essentially never hit) and grew the cache without
                    // bound.  Execution is now identical to a plain `Call`.
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
                    // pc was incremented before dispatch; the instruction position is pc - 1.
                    let r = self.exec_call_method(&mut regs, num_locals, *dst, *obj, *name_idx, *args_base, *nargs, code, pc - 1);
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
                        // TCO iteration as one "call depth" unit, matching
                        // the recursion limit so that RecursionError fires at
                        // the same logical depth whether the call is normal or
                        // tail-call-optimised.
                        tco_iters += 1;
                        if tco_iters > max_call_depth() {
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
                        let mut new_args: smallvec::SmallVec<[Value; 4]> =
                            smallvec::SmallVec::with_capacity(*nargs as usize);
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
                    for i in 0..*n {
                        items.push(vm_try!(vm_read(&regs, *base + i, num_locals)));
                    }
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
                    let count = crate::bytecode::Reg::from(*n);
                    // Sum byte lengths in one pass, then allocate exactly once.
                    let mut total = 0usize;
                    for i in 0..count {
                        let v = vm_try!(vm_read(&regs, *base + i, num_locals));
                        // Parts are guaranteed `str` by the f-string lowering;
                        // `.len()` is the byte length, which is what push_str needs.
                        total += v.as_str().map(str::len).unwrap_or(0);
                    }
                    let mut out = String::with_capacity(total);
                    for i in 0..count {
                        let v = vm_try!(vm_read(&regs, *base + i, num_locals));
                        if let Some(s) = v.as_str() {
                            out.push_str(s);
                        }
                    }
                    regs[*dst as usize] = Value::string(out);
                }
                Insn::FormatValue(dst, src) => {
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    let s = vm_try!(self.format_value_default(&val));
                    regs[*dst as usize] = s;
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
                    regs[*dst as usize] = pyrust_builtins::slice::make_slice(lo, hi, st);
                }
                Insn::BuildDict(dst, base, n) => {
                    let mut dict = indexmap::IndexMap::with_capacity(*n as usize);
                    for i in 0..*n {
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
                    // `Object` keys need `__eq__` dispatch for dedup.
                    // `None` keys also need it for the cross-variant case
                    // (issue #906): a stored PyKey::Object with hash py_hash_none()
                    // that __eq__-matches None must not become a duplicate.
                    //
                    // Fast path for `PyKey::None` (issue #934): only enter the
                    // slow dedup path when the set contains a
                    // `PyKey::Object{hash == py_hash_none()}`.
                    let needs_dedup = match &key {
                        PyKey::Object { .. } => true,
                        PyKey::None => {
                            let none_hash = pyrust_core::py_hash_none() as u64;
                            regs[*set_reg as usize]
                                .as_set()
                                .map(|s| {
                                    s.iter().any(|k| {
                                        matches!(k,
                                            PyKey::Object { hash, .. }
                                            if *hash == none_hash)
                                    })
                                })
                                .unwrap_or(false)
                        }
                        _ => false,
                    };
                    if needs_dedup {
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
                    let items_to_add = vm_try!(self.collect_iterable(&src_val));
                    vm_try!(regs[*list_reg as usize].list_extend(items_to_add));
                }
                Insn::DictUpdate(dict_reg, src_reg) => {
                    let src_val = vm_try!(vm_read(&regs, *src_reg, num_locals));
                    let pairs: Vec<(PyKey, Value)> = match src_val.kind() {
                        ValueKind::Dict(d) => d.clone().into_iter().collect(),
                        // instance_dict proxy: extract visible attrs as (PyKey, Value) pairs.
                        ValueKind::BuiltinObject { ops, .. }
                            if ops.type_name()
                                == pyrust_builtins::instance_dict::TYPE_NAME =>
                        {
                            match pyrust_builtins::instance_dict::as_instance_dict_items(&src_val) {
                                Some(pairs) => pairs,
                                None => vm_try!(Err(PyError::Runtime(
                                    "internal: bad instance_dict state in DictUpdate".to_string(),
                                ))),
                            }
                        }
                        // mappingproxy (**cls.__dict__ or similar): extract via keys.
                        ValueKind::BuiltinObject { ops, .. }
                            if ops.type_name()
                                == pyrust_builtins::mapping_proxy::TYPE_NAME =>
                        {
                            if let Some(cls_rc) =
                                pyrust_builtins::mapping_proxy::as_class_rc(&src_val)
                            {
                                cls_rc
                                    .borrow()
                                    .attrs
                                    .iter()
                                    .map(|(k, v)| (PyKey::str_from(k), v.clone()))
                                    .collect()
                            } else {
                                vm_try!(Err(PyError::Runtime(
                                    "internal: bad mappingproxy state in DictUpdate".to_string(),
                                )))
                            }
                        }
                        _ => vm_try!(Err(pyrust_core::type_err!("'{}' object is not a mapping",
                                value_type_name_str(&src_val)))),
                    };
                    // #1914: route through `dict_extend_value_dedup` so user
                    // `__eq__` deduplicates `PyKey::Object` keys (`dict.update`,
                    // `|=`).  Clone the dict Value (cheap Rc bump) so the helper
                    // can drop the dict borrow before running user code; it
                    // mutates the same backing store in place.
                    let dict_val = regs[*dict_reg as usize]
                        .as_some()
                        .cloned()
                        .unwrap_or(Value::none());
                    vm_try!(self.dict_extend_value_dedup(&dict_val, pairs));
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
                    let items = vm_try!(self.collect_iterable(&src_val));
                    if items.len() < *n as usize {
                        vm_try!(Err::<(), _>(pyrust_core::value_err!("not enough values to unpack (expected {}, got {})",
                                n,
                                items.len())));
                    } else if items.len() > *n as usize {
                        vm_try!(Err::<(), _>(pyrust_core::value_err!("too many values to unpack (expected {})", n)));
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
                    let items = vm_try!(self.collect_iterable(&src_val));
                    let before = *before as usize;
                    let after = *after as usize;
                    let min_len = before + after;
                    if items.len() < min_len {
                        vm_try!(Err::<(), _>(pyrust_core::value_err!("not enough values to unpack (expected at least {}, got {})",
                                min_len,
                                items.len())));
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
                            ListOrTuple,
                            Other,
                        }
                        let tag = match src_val.kind() {
                            ValueKind::Range { start, stop, step } => IterTag::Range(start, stop, step),
                            ValueKind::Generator(_) => IterTag::Generator,
                            ValueKind::PyInstance(inst) => IterTag::PyInstance(Rc::clone(inst)),
                            ValueKind::BuiltinObject { ops, .. } if ops.is_iterable() => {
                                IterTag::BuiltinIterable
                            }
                            ValueKind::List(_) | ValueKind::Tuple(_) => IterTag::ListOrTuple,
                            _ => IterTag::Other,
                        };
                        match tag {
                            IterTag::Range(start, stop, step) => {
                                if step == 0 {
                                    vm_try!(Err(pyrust_core::value_err!("range() arg 3 must not be zero")));
                                }
                                IterState::Range { cur: start, stop, step }
                            }
                            IterTag::Generator => IterState::UserDefined(src_val),
                            IterTag::ListOrTuple => {
                                // Temp-register list/tuple: own the value directly instead
                                // of materializing via iter_values (which allocates a new Vec
                                // and clones all N elements upfront).  Elements are cloned
                                // lazily one-by-one in ForIter, matching the Indexed path.
                                IterState::ValueIndexed { value: src_val, pos: 0 }
                            }
                            IterTag::PyInstance(inst_rc) => {
                                let class = Rc::clone(&inst_rc.borrow().class);
                                if let Some(method_val) = lookup_class_attr(&class, "__iter__") {
                                    let iter_obj = vm_try!(invoke_class_method(
                                        self,
                                        method_val,
                                        Value::py_instance(inst_rc),
                                        &[],
                                    ));
                                    let is_valid_iter = match iter_obj.kind() {
                                        ValueKind::Generator(_) => true,
                                        ValueKind::PyInstance(it) => {
                                            let it_class = Rc::clone(&it.borrow().class);
                                            lookup_class_attr(&it_class, "__next__").is_some()
                                        }
                                        ValueKind::BuiltinObject { ops, .. } => ops.is_iterable(),
                                        _ => false,
                                    };
                                    if !is_valid_iter {
                                        vm_try!(Err(pyrust_core::type_err!("iter() returned non-iterator of type '{}'",
                                                value_type_name_str(&iter_obj),)));
                                    }
                                    IterState::UserDefined(iter_obj)
                                } else if let Some(backing) = instance_builtin_data(&inst_rc) {
                                    // list/dict/set subclass with no user-defined __iter__:
                                    // iterate the backing primitive directly, matching
                                    // CPython's inherited tp_iter slot behaviour.
                                    if matches!(backing.kind(), ValueKind::List(_) | ValueKind::Tuple(_)) {
                                        IterState::ValueIndexed { value: backing, pos: 0 }
                                    } else {
                                        IterState::Materialized(vm_try!(iter_values(&backing)), 0)
                                    }
                                } else if lookup_class_attr(&class, "__getitem__").is_some() {
                                    let iter_obj = vm_try!(self.make_getitem_iter(inst_rc));
                                    IterState::UserDefined(iter_obj)
                                } else {
                                    IterState::Materialized(vm_try!(iter_values(&src_val)), 0)
                                }
                            }
                            IterTag::BuiltinIterable => {
                                // Bytearray: materialise elements up front (like frozenset /
                                // dict-views) since iter_next is not stateful.  Other BuiltinObjects
                                // that implement iter_next properly stay on the UserDefined path.
                                if pyrust_builtins::bytearray::iter_elements(&src_val).is_some() {
                                    IterState::Materialized(vm_try!(iter_values(&src_val)), 0)
                                } else {
                                    IterState::UserDefined(src_val)
                                }
                            }
                            IterTag::Other => {
                                IterState::Materialized(vm_try!(iter_values(&src_val)), 0)
                            }
                        }
                    };
                    iters[*slot as usize] = Some(state);
                    iter_next_cache[*slot as usize] = None;
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
                        Some(IterState::ValueIndexed { value, pos }) => {
                            let cur_pos = *pos;
                            let items: Option<&[Value]> = value.as_list().or_else(|| value.as_tuple());
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
                            let iter_val: &Value = iter_obj;
                            let next_result: Option<Result<Value>> =
                                if let ValueKind::Generator(state_rc) = iter_val.kind() {
                                    let state_rc = Rc::clone(state_rc);
                                    // Probe for shapes that need &mut self
                                    // before taking borrow_mut — these must
                                    // release the cell borrow before calling
                                    // back into the interpreter.
                                    let is_getitem_iter = state_rc
                                        .borrow()
                                        .downcast_ref::<GetItemIter>()
                                        .is_some();
                                    if is_getitem_iter {
                                        Some(match self.step_getitem_iter(&state_rc) {
                                            Ok(Some(v)) => Ok(v),
                                            Ok(None) => Err(pyrust_core::py_err!("StopIteration", String::new())),
                                            Err(e) => Err(e),
                                        })
                                    } else {
                                        let is_callable_iter = state_rc
                                            .borrow()
                                            .downcast_ref::<CallableIter>()
                                            .is_some();
                                        if is_callable_iter {
                                            Some(match self.step_callable_iter(&state_rc) {
                                                Ok(Some(v)) => Ok(v),
                                                Ok(None) => Err(pyrust_core::py_err!("StopIteration", String::new())),
                                                Err(e) => Err(e),
                                            })
                                        } else {
                                        let is_map_iter = state_rc
                                            .borrow()
                                            .downcast_ref::<MapIter>()
                                            .is_some();
                                        if is_map_iter {
                                            Some(match self.step_map_iter(&state_rc) {
                                                Ok(Some(v)) => Ok(v),
                                                Ok(None) => Err(pyrust_core::py_err!("StopIteration", String::new())),
                                                Err(e) => Err(e),
                                            })
                                        } else {
                                        let is_filter_iter = state_rc
                                            .borrow()
                                            .downcast_ref::<FilterIter>()
                                            .is_some();
                                        if is_filter_iter {
                                            Some(match self.step_filter_iter(&state_rc) {
                                                Ok(Some(v)) => Ok(v),
                                                Ok(None) => Err(pyrust_core::py_err!("StopIteration", String::new())),
                                                Err(e) => Err(e),
                                            })
                                        } else {
                                        let is_enumerate_iter = state_rc
                                            .borrow()
                                            .downcast_ref::<EnumerateIter>()
                                            .is_some();
                                        if is_enumerate_iter {
                                            Some(match self.step_enumerate_iter(&state_rc) {
                                                Ok(Some(v)) => Ok(v),
                                                Ok(None) => Err(pyrust_core::py_err!("StopIteration", String::new())),
                                                Err(e) => Err(e),
                                            })
                                        } else {
                                        let is_zip_iter = state_rc
                                            .borrow()
                                            .downcast_ref::<ZipIter>()
                                            .is_some();
                                        if is_zip_iter {
                                            Some(match self.step_zip_iter(&state_rc) {
                                                Ok(Some(v)) => Ok(v),
                                                Ok(None) => Err(pyrust_core::py_err!("StopIteration", String::new())),
                                                Err(e) => Err(e),
                                            })
                                        } else {
                                        let mut borrow = state_rc.borrow_mut();
                                        if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                                            // Built-in iterator created by iter().
                                            if native.pos >= native.items.len() {
                                                Some(Err(pyrust_core::py_err!("StopIteration", String::new())))
                                            } else {
                                                let item = native.items[native.pos].clone();
                                                native.pos += 1;
                                                Some(Ok(item))
                                            }
                                        } else if let Some(frame) = borrow.downcast_mut::<GeneratorFrame>() {
                                            // Resume the generator.
                                            if frame.done {
                                                Some(Err(pyrust_core::py_err!("StopIteration", String::new())))
                                            } else {
                                                Some(self.resume_generator(frame))
                                            }
                                        } else {
                                            Some(Err(PyError::Runtime(
                                                "invalid generator state".to_string(),
                                            )))
                                        }
                                        }   // closes else { let mut borrow = ...
                                        }   // closes is_zip_iter else
                                        }   // closes is_enumerate_iter else
                                        }   // closes is_filter_iter else
                                        }   // closes is_map_iter else
                                    }
                                } else if let ValueKind::PyInstance(inst) = iter_val.kind() {
                                    let inst_rc = Rc::clone(inst);
                                    let cached = &iter_next_cache[*slot as usize];
                                    let method_val = if let Some(mv) = cached {
                                        Some(mv.clone())
                                    } else {
                                        let class = Rc::clone(&inst_rc.borrow().class);
                                        let mv = lookup_class_attr(&class, "__next__");
                                        iter_next_cache[*slot as usize] = mv.clone();
                                        mv
                                    };
                                    if let Some(method_val) = method_val {
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
                                            pyrust_core::py_err!("StopIteration", String::new())
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
                                    vm_try!(Err(pyrust_core::type_err!("iter() returned non-iterator of type '{}'",
                                            value_type_name_str(&iter_val),)));
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
                        // At module/class scope (current_fn_id == None) a
                        // missing name is NameError ("name 'x' is not defined").
                        // Inside a function it is UnboundLocalError ("cannot
                        // access local variable 'x' where it is not associated
                        // with a value").
                        if current_fn_id.is_none() {
                            vm_try!(Err::<(), _>(crate::error::PyError::name_error(
                                "NameError",
                                format!("name '{}' is not defined", name),
                                Some(name.to_string()),
                            )));
                        } else {
                            vm_try!(Err::<(), _>(crate::error::PyError::name_error(
                                "UnboundLocalError",
                                format!(
                                    "cannot access local variable '{}' where it is not associated with a value",
                                    name
                                ),
                                None,
                            )));
                        }
                    }
                }

                // ── Function / Class creation ────────────────────────────
                Insn::MakeFunction(dst, proto_idx, defs_base, _defs_n, annots_base, _annots_n) => {
                    let r = self.exec_make_function(code, &regs, num_locals, *proto_idx, *defs_base, *annots_base);
                    regs[*dst as usize] = vm_try!(r);
                }
                Insn::MakeClass(dst, proto_idx, bases_base, bases_n, name_idx, kwarg_base, _kwarg_n) => {
                    let r = self.exec_make_class(code, &regs, num_locals, *proto_idx, *bases_base, *bases_n, *name_idx, *kwarg_base);
                    regs[*dst as usize] = vm_try!(r);
                }

                // ── PEP 695 type alias ───────────────────────────────────
                Insn::MakeTypeVar(dst, name_idx) => {
                    let name_val = pool_get!(code.consts, *name_idx, "const");
                    let name_str = match name_val.kind() {
                        pyrust_core::ValueKind::Str(s) => s.to_string(),
                        _ => {
                            vm_try!(Err(PyError::Runtime(
                                "MakeTypeVar: name must be a string constant".to_string(),
                            )));
                            unreachable!()
                        }
                    };
                    regs[*dst as usize] = make_typevar_instance(name_str);
                }
                Insn::MakeTypeAlias(dst, name_idx, value_reg, params_reg) => {
                    let name_val = pool_get!(code.consts, *name_idx, "const");
                    let name_str = match name_val.kind() {
                        pyrust_core::ValueKind::Str(s) => s.to_string(),
                        _ => {
                            vm_try!(Err(PyError::Runtime(
                                "MakeTypeAlias: name must be a string constant".to_string(),
                            )));
                            unreachable!()
                        }
                    };
                    let value_val = vm_try!(vm_read(&regs, *value_reg, num_locals)).clone();
                    let params_val = vm_try!(vm_read(&regs, *params_reg, num_locals)).clone();
                    let inst = make_type_alias_instance(name_str, value_val, params_val);
                    regs[*dst as usize] = inst;
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
                        Ok(None) => Err(pyrust_core::py_err!("StopIteration", String::new())),
                        Err(e) => Err(e),
                    };
                }

                let mut borrow = state_rc.try_borrow_mut().map_err(|_| {
                    pyrust_core::value_err!("generator already executing")
                })?;

                if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                    // Built-in iterator: no send support, just advance.
                    if native.pos >= native.items.len() {
                        return Err(pyrust_core::py_err!("StopIteration", String::new()));
                    }
                    let item = native.items[native.pos].clone();
                    native.pos += 1;
                    return Ok(item);
                }

                if let Some(frame) = borrow.downcast_mut::<GeneratorFrame>() {
                    if frame.done {
                        return Err(pyrust_core::py_err!("StopIteration", String::new()));
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
                    Err(pyrust_core::type_err!("object is not an iterator"))
                }
            }
            ValueKind::BuiltinObject { ops, state } => {
                let state = state.clone();
                ops.iter_next(&state).and_then(|opt| {
                    opt.ok_or_else(|| pyrust_core::py_err!("StopIteration", String::new()))
                })
            }
            _ => Err(pyrust_core::type_err!("object is not iterable")),
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
                    pyrust_core::value_err!("generator already executing")
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

/// Issue #1810: cold slow path for `LoadGlobal` after the env-chain,
/// `module_globals_dict`, and script-frame lookups all miss.
///
/// Checks whether `globals["__builtins__"]` is a plain dict.  If it is, the
/// lookup is restricted to that dict only — the hardcoded Rust builtin table
/// is not consulted.  This matches CPython's `PyEval_EvalCode` behaviour: a
/// caller that passes `{"__builtins__": {}}` gets an empty builtin namespace.
///
/// When `__builtins__` is a module or absent, the function delegates to
/// `resolve_builtin()` (the normal execution path).  When `__builtins__` is
/// any other non-dict, non-module value, raises `TypeError` matching
/// CPython 3.12's behaviour when it tries to subscript the value.
///
/// Isolated into a `#[cold] #[inline(never)]` function so that the expanded
/// match body does not inflate the I-cache footprint of the `LoadGlobal`
/// fast path (cache hit).
#[cold]
#[inline(never)]
fn resolve_global_via_builtins(
    module_globals_dict: &Value,
    name: &str,
    cur_ver: u32,
) -> crate::interpreter::Result<(Value, u32)> {
    use crate::bytecode::GLOBAL_CACHE_EMPTY;
    let restricted = module_globals_dict
        .dict_with(|d| d.get(&StrKey("__builtins__")).cloned())
        .flatten();
    if let Some(builtins_val) = restricted {
        match builtins_val.kind() {
            ValueKind::Dict(_) => {
                // __builtins__ is a dict — look up name there only.
                // Do not cache: the dict can be mutated without bumping
                // global_env_version.
                let v = builtins_val
                    .dict_with(|d| d.get(&StrKey(name)).cloned())
                    .flatten()
                    .ok_or_else(|| {
                        PyError::name_error(
                            "NameError",
                            format!("name '{}' is not defined", name),
                            Some(name.to_string()),
                        )
                    })?;
                return Ok((v, GLOBAL_CACHE_EMPTY));
            }
            ValueKind::PyModule(_) => {
                // __builtins__ is a module — fall through to the hardcoded
                // Rust builtin table (normal execution path).
            }
            _ => {
                // __builtins__ is a non-dict, non-module value (e.g. None,
                // int).  CPython 3.12 raises TypeError when it tries to
                // subscript the value to look up a builtin name.
                return Err(pyrust_core::type_err!("'{}' object is not subscriptable",
                        value_type_name_str(&builtins_val),));
            }
        }
    }
    // No dict-restricted builtins: use the hardcoded Rust builtin table.
    // Cache the result with the current env version so that a subsequent
    // module-level assignment of the same name (e.g. `len = my_fn`) bumps
    // global_env_version and forces a re-resolve on the next LoadGlobal.
    let v = resolve_builtin(name).ok_or_else(|| {
        PyError::name_error(
            "NameError",
            format!("name '{}' is not defined", name),
            Some(name.to_string()),
        )
    })?;
    Ok((v, cur_ver))
}

#[inline]
fn vm_read(regs: &[Value], reg: crate::bytecode::Reg, num_locals: crate::bytecode::Reg) -> crate::interpreter::Result<Value> {
    let v = &regs[reg as usize];
    if v.is_unset() {
        if reg < num_locals {
            return Err(pyrust_core::py_err!(
                "NameError",
                "local variable referenced before assignment"
            ));
        } else {
            return Err(crate::error::PyError::Runtime(
                "internal: temp register read before write".to_string(),
            ));
        }
    }
    Ok(v.clone())
}

/// Canonical unary `-`/`+`/`~`/`not` evaluation for built-in operands.
///
/// The single definition of unary-op semantics (i64::MIN negation promoting to
/// BigInt, `~` rejecting Float, `+big` preserving object identity, the
/// `complex` arms).  The optimizer's constant-fold pass calls this directly
/// rather than re-implementing the per-kind arms, so the two cannot drift
/// (issue #458).  Kept as a plain `match` — no trait/slot indirection — because
/// this is a VM hot path.
pub(crate) fn vm_eval_unary(op: UnaryOp, val: Value) -> Result<Value> {
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
            _ => Err(pyrust_core::type_err!(
                "bad operand type for unary -: '{}'",
                value_type_name_str(&val)
            )),
        },
        UnaryOp::Not => Ok(Value::bool_(!val.truthy())),
        UnaryOp::BitNot => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(!v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { -2 } else { -1 })),
            ValueKind::BigInt(v) => Ok(Value::bigint(!v)),
            _ => Err(pyrust_core::type_err!(
                "bad operand type for unary ~: '{}'",
                value_type_name_str(&val)
            )),
        },
        UnaryOp::Pos => {
            if matches!(val.kind(), ValueKind::BigInt(_)) {
                return Ok(val);
            }
            match val.kind() {
                ValueKind::Int(v) => Ok(Value::int(v)),
                ValueKind::Float(v) => Ok(Value::float(v)),
                ValueKind::Complex(re, im) => Ok(Value::complex(re, im)),
                ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                _ => Err(pyrust_core::type_err!(
                    "bad operand type for unary +: '{}'",
                    value_type_name_str(&val)
                )),
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
    pyrust_core::runtime_err!("generator raised StopIteration")
}

// ── PEP 695 TypeAliasType / TypeVar support ──────────────────────────────────

thread_local! {
    /// Class singleton for `TypeAliasType` objects created by `type X = ...`.
    static TYPE_ALIAS_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        // __repr__ is handled by the instance's __name__ attribute via a
        // builtin function registered as "builtins.TypeAliasType.__repr__".
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function("builtins.TypeAliasType.__repr__"),
        );
        Rc::new(RefCell::new(PyClass {
            name: "TypeAliasType".to_string(),
            qualname: "TypeAliasType".to_string(),
            base: None,
            extra_bases: vec![],
            attrs,
            mutation_version: Cell::new(0),
            subclasses: RefCell::new(vec![]),
            metatype: None,
            slots: None,
        }))
    };

    /// Class singleton for `TypeVar` objects created by generic type params.
    static TYPEVAR_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function("builtins.TypeVar.__repr__"),
        );
        Rc::new(RefCell::new(PyClass {
            name: "TypeVar".to_string(),
            qualname: "TypeVar".to_string(),
            base: None,
            extra_bases: vec![],
            attrs,
            mutation_version: Cell::new(0),
            subclasses: RefCell::new(vec![]),
            metatype: None,
            slots: None,
        }))
    };
}

/// Construct a `TypeVar` `PyInstance` with `__name__`, `__constraints__`, and
/// `__bound__` attributes, matching the observable surface of CPython's
/// `typing.TypeVar` as created by PEP 695 type parameter syntax.
pub(crate) fn make_typevar_instance(name: String) -> Value {
    TYPEVAR_CLASS.with(|cls| {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert("__name__".to_string(), Value::string(name));
        attrs.insert(
            "__constraints__".to_string(),
            Value::tuple(vec![]),
        );
        attrs.insert("__bound__".to_string(), Value::none());
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(cls),
            attrs,
        })))
    })
}

/// Construct a `TypeAliasType` `PyInstance` with `__name__`, `__value__`, and
/// `__type_params__` attributes, matching the observable behaviour of CPython's
/// `typing.TypeAliasType`.
pub(crate) fn make_type_alias_instance(name: String, value: Value, type_params: Value) -> Value {
    TYPE_ALIAS_CLASS.with(|cls| {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert("__name__".to_string(), Value::string(name));
        attrs.insert("__value__".to_string(), value);
        attrs.insert("__type_params__".to_string(), type_params);
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(cls),
            attrs,
        })))
    })
}

#[cfg(test)]
mod vm_tests {
    use super::*;
    use crate::bytecode::{FnCode, Insn};
    use crate::interpreter::Interpreter;

    fn empty_code(insns: Vec<Insn>) -> FnCode {
        use crate::bytecode::{AttrCacheEntry, BinOpCacheEntry, GLOBAL_CACHE_EMPTY};
        let n = insns.len();
        FnCode {
            insns,
            lineno_table: vec![0u32; n],
            consts: vec![],
            names: vec![],
            num_regs: 0,
            num_iters: 0,
            num_locals: 0,
            fn_protos: vec![],
            cell_vars: smallvec::smallvec![],
            is_generator: false,
            is_class_method: false,
            attr_cache: std::cell::RefCell::new(vec![AttrCacheEntry::Empty; n]),
            global_cache: std::cell::RefCell::new(vec![(GLOBAL_CACHE_EMPTY, Value::none()); 0]),
            binop_cache: std::cell::RefCell::new(vec![BinOpCacheEntry::Empty; n]),
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
        code.lineno_table.extend([0u32, 0, 0]);
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
