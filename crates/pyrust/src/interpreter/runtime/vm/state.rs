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

// Exception stacks are inline for the common shallow-handler case.
pub(crate) type ExcHandlersBuf = smallvec::SmallVec<[usize; 2]>;
pub(crate) type HandledExcBuf = smallvec::SmallVec<[Value; 2]>;

/// Cold exception, async-generator ownership, and frame-introspection state
/// for a [`GeneratorFrame`] (issue #3162).
///
/// Allocated lazily behind `RefCell<Option<Box<GenColdState>>>` only when a
/// generator enters an exception handler, holds an active exception, runs
/// async-generator ownership tracking, or materializes a `VmFrameCache`.
/// Common happy-path yield/resume generators never allocate this struct.
#[derive(Default)]
pub(crate) struct GenColdState {
    pub(crate) exc_handlers: ExcHandlersBuf,
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
    /// The one async-generator awaitable allowed to resume this frame while it
    /// is parked inside an inner `await`. `Some` remains occupied even after
    /// the weak pointer expires: CPython does not release running ownership
    /// when that awaitable is closed or dropped.
    pub(crate) async_gen_running_owner: Option<std::rc::Weak<GeneratorCell>>,
    /// Frame identity belongs to the generator activation, not to one resume.
    /// Kept absent until code inside the generator introspects its frame.
    pub(crate) frame_cache: Option<Box<VmFrameCache>>,
}

/// Heap-allocated execution state for a suspended generator.
/// Stored type-erased inside `Value::generator()` via `Box<dyn Any>`.
pub(crate) struct GeneratorFrame {
    pub(crate) code: Rc<crate::bytecode::FnCode>,
    /// The suspended frame's register file.  A tight `Vec<Value>` (sized to the
    /// body's `num_regs`) rather than the dispatch loop's inline-16 `RegsBuf`
    /// (#2257): a suspended generator keeps its register file alive for its whole
    /// lifetime, and the 128-byte inline buffer wasted ~96 bytes for the common
    /// small-bodied generator.  Never moved out of the frame — the dispatch loop
    /// (native resume and the #2253 trampoline) operates on a `RegSlice` *into*
    /// this buffer — so a `Vec` needs no per-resume conversion; the one-time tight
    /// allocation is paid at generator creation.
    pub(crate) regs: Vec<Value>,
    /// Indexed cell slots (`0..own_cell_count` own cells + `own_cell_count..`
    /// captured closure cells) persisted across generator suspensions (#452).
    /// Stored as a tight `Box<[CellSlot]>` (16 bytes, zero heap allocation when
    /// empty) rather than an inline `SmallVec` so `GeneratorFrame` stays compact.
    pub(crate) cells: Box<[pyrust_core::CellSlot]>,
    pub(crate) iters: ItersBuf,
    /// Program counter for the NEXT instruction to execute on resumption.
    pub(crate) pc: usize,
    /// The value from an explicit `return val` inside this generator.
    /// Set by `resume_generator_with_exc` when `FrameOutcome::Returned(val)`
    /// is received, so that `YieldFrom` can capture `StopIteration.value`
    /// from the sub-iterator's frame.
    pub(crate) last_return_value: Option<Value>,
    /// The name of the generator function, used to populate traceback frames
    /// when an exception propagates out of the generator body (issue #908).
    /// `Arc<str>` so `.clone()` in `resume_generator_with_exc` is a
    /// reference-count bump rather than a heap allocation on every resume.
    /// This is the *compile-time* name behind tracebacks and `co_name`; the
    /// Python-visible `__name__` / `__qualname__` pair a user can reassign
    /// lives on the owning `GeneratorCell`, outside the state this frame
    /// occupies, so that it reads back while the body runs (#2978).
    pub(crate) fn_name: std::sync::Arc<str>,
    /// Originating `UserFunction` for `gi_code` / `f_code` introspection on
    /// both suspended and running generator frames (#2980).
    pub(crate) function: Rc<pyrust_core::UserFunction>,
    /// Shared writable `__name__` / `__qualname__` cell also held by the owning
    /// `GeneratorCell`.  Read on `Drop` so CPython's never-awaited coroutine
    /// warning reports the coroutine's current `__qualname__` (issue #2981).
    pub(crate) display_names: Rc<std::cell::RefCell<pyrust_core::GeneratorNames>>,
    /// Lazily allocated cold exception, async-generator ownership, and
    /// introspection state (issue #3162).
    ///
    /// Interior mutability lets an active `VmFrameView` reach the persistent
    /// `frame_cache` through its shared generator pointer without creating a
    /// raw-derived `&mut GeneratorFrame` or moving the cache through the hot
    /// resume/yield path. Callers with `&mut GeneratorFrame` use
    /// `self.cold.get_mut()` with zero runtime borrow-check overhead.
    pub(crate) cold: std::cell::RefCell<Option<Box<GenColdState>>>,
    /// The destination register of the most recent `Yield` instruction.
    /// On the next resumption this register receives the sent value
    /// (`Value::none()` for `next()`, the caller's argument for `send()`).
    /// Meaningless until the generator has yielded at least once (`pc != 0`).
    pub(crate) yield_dst: crate::bytecode::Reg,
    /// Source line of the `yield` the generator is currently suspended at
    /// (`0` before the first yield).  Restored into the dispatch loop's
    /// `cur_line` on resume so that an exception caught inside the body after a
    /// `generator.throw()` reports the yield line — not the throw call site —
    /// in its traceback (issue #2445).
    pub(crate) suspended_line: u32,
    pub(crate) done: bool,
    /// True when this frame backs a *coroutine* (an `async def` body, issue
    /// #1039) rather than a plain generator.  Coroutines reuse the entire
    /// suspend/resume machinery but report `type(coro).__name__ == "coroutine"`,
    /// render a `<coroutine object …>` repr, and are NOT iterable with `for`.
    pub(crate) is_coroutine: bool,
}

impl GeneratorFrame {
    /// True when this frame backs an *async generator* (`async def` body that
    /// also contains `yield`, issue #2280): `is_coroutine` (async def) AND
    /// `code.is_generator` (has a bare `yield`).  Async generators reuse the
    /// suspend/resume machinery but report `type(g).__name__ == "async_generator"`,
    /// are driven by `__anext__`/`asend` rather than `for`/`next()`, and
    /// distinguish a bare `yield v` (→ produced item) from an inner-`await`
    /// suspension (→ propagate to the event loop).
    pub(crate) fn is_async_generator(&self) -> bool {
        self.is_coroutine && self.code.is_generator
    }

    /// True if this suspended generator carries any persisted PEP 3134
    /// exception context (`handled_exc_slice`, `active_exception`, or
    /// `exc_saved_active_slice`).
    #[inline]
    pub(crate) fn has_exc_state(&self) -> bool {
        self.cold.borrow().as_ref().is_some_and(|cold| {
            !cold.handled_exc_slice.is_empty()
                || cold.active_exception.is_some()
                || !cold.exc_saved_active_slice.is_empty()
        })
    }

    /// True if this generator has materialized a persistent `VmFrameCache`.
    #[inline]
    pub(crate) fn has_frame_cache(&self) -> bool {
        self.cold
            .borrow()
            .as_ref()
            .is_some_and(|cold| cold.frame_cache.is_some())
    }

    /// Read this generator's persistent `VmFrameCache`, if present, without
    /// allocating `GenColdState`.
    #[inline]
    pub(crate) fn with_frame_cache<R>(&self, read: impl FnOnce(Option<&VmFrameCache>) -> R) -> R {
        let cold = self.cold.borrow();
        let cache = cold.as_deref().and_then(|c| c.frame_cache.as_deref());
        read(cache)
    }

    /// Mutably update this generator's persistent `VmFrameCache`, lazily
    /// allocating `GenColdState` if needed.
    #[inline]
    pub(crate) fn with_frame_cache_mut<R>(
        &self,
        update: impl FnOnce(&mut Option<Box<VmFrameCache>>) -> R,
    ) -> R {
        let mut cold = self.cold.borrow_mut();
        let state = cold.get_or_insert_with(|| Box::new(GenColdState::default()));
        update(&mut state.frame_cache)
    }

    /// Take the generator's persisted `exc_handlers` without allocating cold
    /// state on the common handler-free path.
    #[inline]
    pub(crate) fn take_exc_handlers(&mut self) -> ExcHandlersBuf {
        match self.cold.get_mut().as_deref_mut() {
            Some(cold) => std::mem::take(&mut cold.exc_handlers),
            None => ExcHandlersBuf::new(),
        }
    }

    /// Persist `exc_handlers` on suspension without allocating `GenColdState`
    /// when `exc_handlers` is empty.
    #[inline]
    pub(crate) fn store_exc_handlers(&mut self, exc_handlers: ExcHandlersBuf) {
        if exc_handlers.is_empty() {
            if let Some(cold) = self.cold.get_mut().as_deref_mut() {
                cold.exc_handlers.clear();
            }
        } else {
            let cold = self
                .cold
                .get_mut()
                .get_or_insert_with(|| Box::new(GenColdState::default()));
            cold.exc_handlers = exc_handlers;
        }
    }

    /// Take all persisted exception state needed to resume this generator,
    /// without allocating `GenColdState` when none is present.
    #[inline]
    pub(crate) fn take_resume_exc_state(
        &mut self,
    ) -> (
        ExcHandlersBuf,
        HandledExcBuf,
        Option<Value>,
        Vec<Option<Value>>,
    ) {
        match self.cold.get_mut().as_deref_mut() {
            Some(cold) => (
                std::mem::take(&mut cold.exc_handlers),
                std::mem::take(&mut cold.handled_exc_slice),
                cold.active_exception.take(),
                std::mem::take(&mut cold.exc_saved_active_slice),
            ),
            None => (
                ExcHandlersBuf::new(),
                HandledExcBuf::new(),
                None,
                Vec::new(),
            ),
        }
    }

    /// Persist exception state captured at yield time without allocating
    /// `GenColdState` when the generator yielded outside any exception handler.
    #[inline]
    pub(crate) fn store_yield_exc_state(
        &mut self,
        exc_handlers: ExcHandlersBuf,
        handled_exc_slice: HandledExcBuf,
        active_exception: Option<Value>,
        exc_saved_active_slice: Vec<Option<Value>>,
    ) {
        let is_empty = exc_handlers.is_empty()
            && handled_exc_slice.is_empty()
            && active_exception.is_none()
            && exc_saved_active_slice.is_empty();
        if is_empty {
            if let Some(cold) = self.cold.get_mut().as_deref_mut() {
                cold.exc_handlers.clear();
                cold.handled_exc_slice.clear();
                cold.active_exception = None;
                cold.exc_saved_active_slice.clear();
            }
        } else {
            let cold = self
                .cold
                .get_mut()
                .get_or_insert_with(|| Box::new(GenColdState::default()));
            cold.exc_handlers = exc_handlers;
            cold.handled_exc_slice = handled_exc_slice;
            cold.active_exception = active_exception;
            cold.exc_saved_active_slice = exc_saved_active_slice;
        }
    }

    /// Inspect the current async-generator running owner without allocating.
    #[inline]
    pub(crate) fn async_gen_running_owner(&mut self) -> Option<&std::rc::Weak<GeneratorCell>> {
        self.cold
            .get_mut()
            .as_deref()
            .and_then(|cold| cold.async_gen_running_owner.as_ref())
    }

    /// Install the async-generator running owner, lazily allocating
    /// `GenColdState`.
    #[inline]
    pub(crate) fn set_async_gen_running_owner(&mut self, owner: std::rc::Weak<GeneratorCell>) {
        let cold = self
            .cold
            .get_mut()
            .get_or_insert_with(|| Box::new(GenColdState::default()));
        cold.async_gen_running_owner = Some(owner);
    }

    /// Clear the async-generator running owner without allocating.
    #[inline]
    pub(crate) fn clear_async_gen_running_owner(&mut self) {
        if let Some(cold) = self.cold.get_mut().as_deref_mut() {
            cold.async_gen_running_owner = None;
        }
    }
}

impl Drop for GeneratorFrame {
    /// Emit CPython's `RuntimeWarning: coroutine '<name>' was never awaited`
    /// when a *coroutine* object is dropped without ever having been driven
    /// (issue #2306).
    ///
    /// A coroutine that was awaited / driven by the event loop has advanced
    /// past its entry (`pc != 0`) or finished (`done`), so the never-started
    /// condition is `pc == 0 && !done`.  Async generators (`code.is_generator`)
    /// and plain generators (`!is_coroutine`) are excluded — the warning is
    /// coroutine-specific.
    ///
    /// This fires once, when the backing `Rc` drops (cold path — never on the
    /// resume hot path).  We match CPython's interpreter-shutdown GC shape,
    /// `sys:1: RuntimeWarning: ...`, written to stderr; the file/line, source
    /// line, and tracemalloc-hint form (emitted on an immediate `del`) is not
    /// reproduced because pyrust has no deterministic mid-program object
    /// finalisation point.
    fn drop(&mut self) {
        if self.is_coroutine && !self.code.is_generator && self.pc == 0 && !self.done {
            use std::io::Write;
            let qualname = self.display_names.borrow().qualname.clone();
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(
                stderr,
                "sys:1: RuntimeWarning: coroutine '{}' was never awaited",
                qualname
            );
        }
    }
}

/// Awaitable returned by `async_generator.__anext__()` / `.asend(v)` (issue
/// #2280).  Wraps the async generator's state cell plus the value to send into
/// the next resumption (`None` for `__anext__`, the user's argument for
/// `asend`) and the optional exception to inject (`athrow`/`aclose`).  Stored
/// type-erased inside a `Value::generator` so that `get_awaitable` accepts it
/// and `YieldFrom` drives it: each drive step resumes the async generator once
/// and, per the yield/await duality, either completes the await with the
/// produced item (a bare `yield v` → `StopIteration(v)`), propagates an
/// inner-`await` suspension upward (yields the scheduling point), or raises
/// `StopAsyncIteration` when the generator is exhausted.
pub(crate) struct AsyncGenASend {
    /// The async generator's state cell (its `GeneratorFrame`).
    pub(crate) agen: Rc<GeneratorCell>,
    /// Value to send into the next resume (the `asend` argument; `None` for
    /// `__anext__`).  Taken on the first drive step, then `None` thereafter.
    pub(crate) send_value: Option<Value>,
    /// Exception to inject on the first drive step (`athrow`/`aclose`).
    pub(crate) throw_exc: Option<PyError>,
    /// True once the first drive step has run, so subsequent steps (when the
    /// inner `await` re-enters) forward their caller-sent value rather than
    /// re-sending the original argument / re-injecting the exception.
    pub(crate) started: bool,
    /// True once this one-shot awaitable has completed or raised. Further
    /// `send()` attempts raise CPython's reuse RuntimeError.
    pub(crate) done: bool,
    /// Selects CPython's `aclose()/athrow()` reuse diagnostic instead of the
    /// `__anext__()/asend()` spelling after the injection value is consumed.
    pub(crate) is_close_or_throw: bool,
    /// True for the `aclose()` awaitable: a `GeneratorExit` injection whose
    /// `GeneratorExit` / `StopAsyncIteration` / `StopIteration` outcome completes
    /// the await with `None` (rather than propagating), and whose *yield* is a
    /// `RuntimeError("async generator ignored GeneratorExit")`.
    pub(crate) is_aclose: bool,
}

/// Awaitable returned by two-argument `anext(aiter, default)` (issue #2983).
/// Stores the raw value returned by `aiter.__anext__()` and the fallback
/// `default_value`; driving it via `await`/`send`/`__next__`/`throw` resolves
/// `wrapped` with `get_awaitable`, forwards the step, and turns any
/// `StopAsyncIteration` (including subclasses) into `StopIteration(default_value)`
/// without awaiting `default_value`.
pub(crate) struct AnextAwaitable {
    pub(crate) wrapped: Value,
    pub(crate) default_value: Value,
}

/// Call-trampoline frame (#2234): what the caller needs restored when a
/// trampolined Python→Python callee returns.  Owned by `Interpreter::tramp_stack`
/// across nested VM entries (#3159).
pub(crate) struct TrampFrame {
    /// Counts this explicit Python frame in the thread-wide recursion depth,
    /// including when a full arena forces a nested native VM entry.
    _depth_guard: CallDepthGuard,
    /// Caller's register slice (raw pointer into its own — stable — register
    /// buffer; valid until this frame is restored).
    saved_regs: RegSlice,
    /// Caller's cell slice (restored when the trampolined callee returns).
    saved_cells: CellSlice,
    /// Caller's resume pc (already advanced past the call instruction).
    saved_pc: usize,
    /// Caller register that receives the call's result.
    dst: u32,
    /// Caller's base offset in `tramp_arena` (`usize::MAX` for the bottom,
    /// natively-called frame, which uses the param `regs`).
    saved_base: usize,
    /// Caller's source line at the call site (restored for tracebacks).
    saved_cur_line: u32,
    /// Caller's `code` pointer, `num_locals`, and env — restored when the callee
    /// returns.  (Equal to the callee's for a self-recursive call; differ for a
    /// general Python→Python call.)
    saved_code_ptr: *const crate::bytecode::FnCode,
    saved_num_locals: u32,
    /// `None` when callee shares the caller's environment (`Rc::ptr_eq`),
    /// avoiding an `Rc::clone` + `Rc::drop` on every intra-module call (#2924).
    saved_env: Option<EnvRef>,
    /// Cache key owned by this callee evaluation. Boxed so plain `Insn::Call`
    /// frames pay 8 B (null niche) instead of a 48 B inline `MemoKey` (#2924).
    memo_key: Option<Box<MemoKey>>,
}

/// Placeholder left inside a generator's state cell (`Rc<GeneratorCell>`)
/// while its `GeneratorFrame` is checked out and being driven (#2253).  Keeps the
/// cell's contents valid (and the borrow released) during the drive, and lets a
/// re-entrant `ForIter` on the same generator recognise "already executing"
/// (→ `ValueError`) instead of mis-reading it as an exhausted iterator.
pub(super) struct GenDriving;

/// Generator-trampoline frame (#2253): the generator's checked-out frame plus
/// the consumer state restored when the generator yields / returns / raises.
/// Owned by `Interpreter::gen_drive_stack` across nested VM entries (#3159).
pub(crate) struct GenDriveFrame {
    /// Counts this generator frame in the same thread-wide recursion depth as
    /// native and call-trampolined Python frames.
    _depth_guard: CallDepthGuard,
    /// The generator's state cell; the checked-out frame is written back here on
    /// yield / return / error.
    state_rc: Rc<GeneratorCell>,
    /// The checked-out generator frame (a placeholder occupies the cell while it
    /// is driving, so the frame's heap address — and a `RegSlice` into its
    /// `regs` — stays stable).
    gframe: Box<GeneratorFrame>,
    /// Consumer's register slice (stable buffer; restored on switch-back).
    saved_regs: RegSlice,
    /// Consumer's cell slice (restored on switch-back).
    saved_cells: CellSlice,
    /// Consumer pc after the `ForIter` (resume point on a yield).
    saved_pc: usize,
    /// Consumer's `ForIter` loop-exit pc (jumped to on a generator return).
    exit_pc: usize,
    /// Consumer register that receives each yielded value.
    dst: u32,
    /// Consumer's `tramp_active_base`.
    saved_base: usize,
    saved_cur_line: u32,
    saved_code_ptr: *const crate::bytecode::FnCode,
    saved_num_locals: u32,
    saved_env: EnvRef,
    saved_iters: ItersBuf,
    saved_iter_cache: IterCacheBuf,
    saved_exc_handlers: ExcHandlersBuf,
    /// `tramp_stack` depth at switch-in (error-unwind floor for this generator).
    tramp_floor: usize,
}

/// Mutable references to the dispatch loop's live frame state, handed to the
/// cold error unwinder ([`Interpreter::vm_unwind_error`]) so the heavy
/// interleaved unwinding logic lives in a single `#[inline(never)]` function
/// rather than being duplicated into the stack frame at all ~180 `vm_try!` sites
/// (which, in debug builds, inflated the frame enough to overflow the native
/// stack on deep non-trampolined recursion).
struct UnwindState<'a> {
    regs: &'a mut RegSlice,
    cells: &'a mut CellSlice,
    /// Used by `vm_enter_gen_drive` (saved as the consumer resume pc, then set to
    /// the generator's pc); the unwinder reports its resume pc via `Resume`
    /// instead and leaves this untouched.
    pc: &'a mut usize,
    cur_line: &'a mut u32,
    code_ptr: &'a mut *const crate::bytecode::FnCode,
    num_locals: &'a mut u32,
    iters: &'a mut ItersBuf,
    iter_next_cache: &'a mut IterCacheBuf,
    exc_handlers: &'a mut ExcHandlersBuf,
    tramp_active_base: &'a mut usize,
    /// High-water floor in `Interpreter::tramp_stack` for this VM entry (#3159).
    tramp_base: usize,
    /// High-water floor in `Interpreter::gen_drive_stack` for this VM entry (#3159).
    gen_base: usize,
    exc_ctx_frame_base: usize,
}

/// Result of [`Interpreter::vm_unwind_error`].
enum UnwindOutcome {
    /// A generator consumer caught the error: resume the dispatch loop here.
    Resume(usize),
    /// The error escaped every active frame: return it from the VM.
    Escape(PyError),
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
    /// Source line of the `Yield` that produced this suspension, persisted onto
    /// `GeneratorFrame::suspended_line` (issue #2445).
    pub(crate) suspended_line: u32,
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
/// For callers that don't execute generators (`run_bytecode`), `Yielded` is
/// unreachable and the `Returned` value is extracted directly.
///
/// `GenSaveState` is boxed (#3159) so every synchronous `run_bytecode` return
/// passes `FrameOutcome` in two registers (`size_of::<FrameOutcome>() <= 24`)
/// instead of a 168-byte stack sret buffer.
pub(crate) enum FrameOutcome {
    Returned(Value),
    Yielded {
        value: Value,
        saved: Box<GenSaveState>,
    },
}

/// Outcome of a single event-loop step of a coroutine (issue #2281).
/// See [`Interpreter::coro_step`].
pub(crate) enum CoroStep {
    /// Coroutine suspended, yielding this value (the awaited Future).
    Yielded(Value),
    /// Coroutine completed, returning this value.
    Returned(Value),
}
