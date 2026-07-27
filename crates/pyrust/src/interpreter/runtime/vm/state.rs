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
    /// Source line of the `yield` the generator is currently suspended at
    /// (`0` before the first yield).  Restored into the dispatch loop's
    /// `cur_line` on resume so that an exception caught inside the body after a
    /// `generator.throw()` reports the yield line — not the throw call site —
    /// in its traceback (issue #2445).
    pub(crate) suspended_line: u32,
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
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(
                stderr,
                "sys:1: RuntimeWarning: coroutine '{}' was never awaited",
                self.fn_name
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
    pub(crate) agen: Rc<RefCell<Box<dyn std::any::Any>>>,
    /// Value to send into the next resume (the `asend` argument; `None` for
    /// `__anext__`).  Taken on the first drive step, then `None` thereafter.
    pub(crate) send_value: Option<Value>,
    /// Exception to inject on the first drive step (`athrow`/`aclose`).
    pub(crate) throw_exc: Option<PyError>,
    /// True once the first drive step has run, so subsequent steps (when the
    /// inner `await` re-enters) send `None` rather than re-sending the original
    /// argument / re-injecting the exception.
    pub(crate) started: bool,
    /// True for the `aclose()` awaitable: a `GeneratorExit` injection whose
    /// `GeneratorExit` / `StopAsyncIteration` / `StopIteration` outcome completes
    /// the await with `None` (rather than propagating), and whose *yield* is a
    /// `RuntimeError("async generator ignored GeneratorExit")`.
    pub(crate) is_aclose: bool,
}

/// Call-trampoline frame (#2234): what the caller needs restored when a
/// trampolined Python→Python callee returns.  Module-scoped so the cold error
/// unwinder ([`Interpreter::vm_unwind_error`]) can name it; the dispatch loop's
/// `tramp_stack` is `Vec<TrampFrame>`.
struct TrampFrame {
    /// Counts this explicit Python frame in the thread-wide recursion depth,
    /// including when a full arena forces a nested native VM entry.
    _depth_guard: CallDepthGuard,
    /// Caller's register slice (raw pointer into its own — stable — register
    /// buffer; valid until this frame is restored).
    saved_regs: RegSlice,
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
    saved_active_code_rc: Option<Rc<crate::bytecode::FnCode>>,
    saved_num_locals: u32,
    saved_env: EnvRef,
    /// Cache key owned by this callee evaluation. Present only when this frame
    /// entered from a `CallMemo` miss that was not already in flight.
    memo_key: Option<MemoKey>,
}

/// Placeholder left inside a generator's state cell (`Rc<RefCell<Box<dyn Any>>>`)
/// while its `GeneratorFrame` is checked out and being driven (#2253).  Keeps the
/// cell's contents valid (and the borrow released) during the drive, and lets a
/// re-entrant `ForIter` on the same generator recognise "already executing"
/// (→ `ValueError`) instead of mis-reading it as an exhausted iterator.
pub(super) struct GenDriving;

/// Generator-trampoline frame (#2253): the generator's checked-out frame plus
/// the consumer state restored when the generator yields / returns / raises.
/// Module-scoped for the same reason as [`TrampFrame`].
struct GenDriveFrame {
    /// Counts this generator frame in the same thread-wide recursion depth as
    /// native and call-trampolined Python frames.
    _depth_guard: CallDepthGuard,
    /// The generator's state cell; the checked-out frame is written back here on
    /// yield / return / error.
    state_rc: Rc<RefCell<Box<dyn std::any::Any>>>,
    /// The checked-out generator frame (a placeholder occupies the cell while it
    /// is driving, so the frame's heap address — and a `RegSlice` into its
    /// `regs` — stays stable).
    gframe: Box<GeneratorFrame>,
    /// Consumer's register slice (stable buffer; restored on switch-back).
    saved_regs: RegSlice,
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
    saved_active_code_rc: Option<Rc<crate::bytecode::FnCode>>,
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
    /// Used by `vm_enter_gen_drive` (saved as the consumer resume pc, then set to
    /// the generator's pc); the unwinder reports its resume pc via `Resume`
    /// instead and leaves this untouched.
    pc: &'a mut usize,
    cur_line: &'a mut u32,
    code_ptr: &'a mut *const crate::bytecode::FnCode,
    active_code_rc: &'a mut Option<Rc<crate::bytecode::FnCode>>,
    num_locals: &'a mut u32,
    iters: &'a mut ItersBuf,
    iter_next_cache: &'a mut IterCacheBuf,
    exc_handlers: &'a mut ExcHandlersBuf,
    tramp_active_base: &'a mut usize,
    tramp_stack: &'a mut Vec<TrampFrame>,
    gen_drive_stack: &'a mut Vec<GenDriveFrame>,
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
// Hot-path VM type: this is the return value of the core dispatch loop, matched on
// every frame return. Boxing the rare `Yielded` variant would add heap indirection
// on the generator-suspension path; the size delta lives only on the stack and is
// never duplicated, so a box buys nothing while costing the suspension path.
#[allow(clippy::large_enum_variant)]
pub(crate) enum FrameOutcome {
    Returned(Value),
    Yielded { value: Value, saved: GenSaveState },
}

/// Outcome of a single event-loop step of a coroutine (issue #2281).
/// See [`Interpreter::coro_step`].
pub(crate) enum CoroStep {
    /// Coroutine suspended, yielding this value (the awaited Future).
    Yielded(Value),
    /// Coroutine completed, returning this value.
    Returned(Value),
}
