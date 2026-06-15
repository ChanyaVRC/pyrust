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
//
// `ItersBuf` is inline-**1** (not 2): the same buffer also backs every
// *suspended* `GeneratorFrame`, which retains it for the generator's whole
// lifetime (#2257).  `Option<IterState>` is 64 B, so inline-2 cost 144 B/frame
// even for the common generator that holds 0 or 1 active for-loops; inline-1
// halves the inline footprint to 80 B (`GeneratorFrame` 360 → 296 B) with no
// per-resume conversion (the suspend/resume path `mem::take`s the buffer as-is).
// A frame with 2+ simultaneously-active for-loops (nested loops) spills its
// extra iterators to the heap, but that is a single push per outer iteration —
// amortised to nothing against the per-element loop work (nested/triple-loop and
// nested-comprehension benches measured neutral; the generator-drive path, which
// copies the frame, measured *faster* from the smaller buffer).
pub(crate) type ItersBuf = smallvec::SmallVec<[Option<IterState>; 1]>;
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
    /// Optional mutation guard (#1988/#1994).  Set only for `deque` iterators
    /// and the manual `iter()` form of dict / set / dict-views.  Boxed so the
    /// common unguarded `NativeIterFrame` grows by just one (null) pointer,
    /// keeping its footprint — and the per-step `downcast`/field access on the
    /// hot iteration path — identical to the pre-guard layout.  The box is
    /// allocated once at iterator creation (cold), never per step.
    pub(crate) guard: Option<Box<NativeIterGuard>>,
}

/// Mutation guard for [`NativeIterFrame`].  Holds the live `container` `Value`
/// and a `version` recorded at iterator creation; [`NativeIterFrame::advance`]
/// re-reads the live version each step and raises `msg` as a `RuntimeError` on
/// a mismatch.  Two flavours:
///   - [`GuardVersion::Size`] for the manual `iter()` form of dict / set /
///     dict-views (#1988; `container` is the collection, version = live size).
///   - [`GuardVersion::DequeState { counter }`] for `deque` iterators (#1994;
///     `container` keeps the `_state` cell alive, version = its mutation
///     counter, so even net-zero-size mutations like `rotate()` are detected).
pub(crate) struct NativeIterGuard {
    pub(crate) container: Value,
    pub(crate) version: i64,
    pub(crate) kind: GuardVersion,
    pub(crate) msg: &'static str,
    /// OrderedDict semantics (#2436 review): skip the guard once the iterator
    /// is exhausted — CPython's odict iterators test exhaustion first.
    pub(crate) exhaust_first: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum GuardVersion {
    /// Version is the container's live element count (dict / set / dict-view).
    Size,
    /// Version is a deque `_state` mutation counter read directly through a
    /// cached pointer to element 0 of the (never-reallocated) one-element
    /// `_state` cell list.  `container` holds the `Rc` that keeps the cell's
    /// backing buffer alive for the iterator's lifetime, so the pointer stays
    /// valid; the read is a single tagged-int load with no decode/borrow,
    /// keeping deque iteration perf-neutral.
    DequeState { counter: *const Value },
}

impl NativeIterFrame {
    /// Construct an unguarded native iterator (the common case).
    pub(crate) fn new(items: Vec<Value>, type_name: &'static str) -> Self {
        NativeIterFrame { items, pos: 0, type_name, guard: None }
    }

    /// Mutation-guard check, run once per `__next__` *only when a guard is
    /// present* (#1988/#1994).  Returns `Err(RuntimeError)` if the guarded
    /// container's version diverged from the value recorded at iterator
    /// creation, else `Ok(())`.  `#[inline]` so the common unguarded iterator
    /// collapses to a single predictable `Option::is_some` branch and the
    /// version-read body is only reached by guarded (deque / dict-iter)
    /// iterators — the per-step fast path is unchanged.
    #[inline]
    fn guard_check(&self) -> Result<()> {
        let Some(guard) = &self.guard else {
            return Ok(());
        };
        let live = match guard.kind {
            GuardVersion::Size => live_collection_len(&guard.container).map(|n| n as i64),
            // SAFETY: `counter` points at element 0 of the deque's `_state`
            // cell list, whose backing buffer is kept alive by `guard.container`
            // (an `Rc` clone) and never reallocates (it is a fixed one-element
            // list; mutations overwrite the element in place).  The pointer
            // therefore stays valid and exclusive-free for the iterator's
            // lifetime.
            GuardVersion::DequeState { counter } => unsafe { (*counter).as_int() },
        };
        if live != Some(guard.version) {
            return Err(PyError::Runtime(guard.msg.to_string()));
        }
        Ok(())
    }

    /// Advance one step, applying the mutation guard if present.
    ///
    /// Returns `Ok(Some(v))` for the next element, `Ok(None)` once exhausted,
    /// or `Err(RuntimeError)` if a guarded container mutated since the iterator
    /// was created.  Used by call sites that already pay a function-call
    /// boundary (`call_next`, `yield_from_advance`); the VM's `ForIter` hot loop
    /// inlines the equivalent steps directly to keep iteration fast.
    pub(crate) fn advance(&mut self) -> Result<Option<Value>> {
        if self.pos >= self.items.len()
            && self.guard.as_ref().is_some_and(|g| g.exhaust_first)
        {
            return Ok(None);
        }
        self.guard_check()?;
        if self.pos >= self.items.len() {
            return Ok(None);
        }
        let item = self.items[self.pos].clone();
        self.pos += 1;
        Ok(Some(item))
    }
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

/// Lazy iterator for `itertools.chain.from_iterable(outer)` (#2362).
///
/// `outer` is the already-`iter()`-ed outer iterator (one element per inner
/// iterable).  `inner` is the current inner iterator (`None` until the first
/// inner is reached, and again after each inner is exhausted).  Each
/// `step_chain_from_iterable` advances `inner` by one element; on inner
/// exhaustion it pulls the next inner iterable from `outer` and `iter()`s it
/// lazily.  Both the outer and each inner are driven *one element at a time*
/// (never materialised wholesale), so an inner generator with interleaved side
/// effects runs exactly in step with the chain consumer — matching CPython's
/// lazy timing (`islice(chain.from_iterable(gens), k)` must not over-consume).
///
/// Replaces the old `_chain_from_iterable` PyInstance class, whose
/// Python-dispatched `__next__` cost a full VM re-entry per element; as a
/// generator-state iterator it gets the dedicated `step_*` dispatch in
/// `ForIter` / `call_next` with no re-entry, plus a direct index-walk fast
/// path for the common `NativeIterFrame` inner (lists/tuples/ranges).
pub(crate) struct ChainFromIterableIter {
    /// Already-converted outer iterator object.
    pub(crate) outer: Value,
    /// Current inner iterator (`None` until the first inner is reached and
    /// after each inner is drained).
    pub(crate) inner: Option<Value>,
    /// Set to `true` once the outer iterator raises `StopIteration`.
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
    /// Current counter value; incremented after each yielded pair.  Held as a
    /// `Value` (inline `int` in the common case, promoting to `BigInt` past
    /// `i64::MAX`) so the counter is arbitrary-precision — matching CPython and
    /// accepting a `BigInt` `start` (#2125).
    pub(crate) counter: Value,
    /// Set to `true` once the source raises `StopIteration`.
    pub(crate) done: bool,
}

/// Lazy iterator over an arbitrary-precision `range` (#2118).  Produced by
/// `iter()` / `enumerate()` / `zip()` on a big-bound range so those callers do
/// not materialize the (potentially enormous) sequence.  `cur` advances by
/// `step` each `next()`; iteration stops when it reaches `stop`.
pub(crate) struct BigRangeIter {
    pub(crate) cur: PyBigInt,
    pub(crate) stop: PyBigInt,
    pub(crate) step: PyBigInt,
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
    /// Caller's `code` pointer, `num_locals`, `fn_id`, and env — restored when
    /// the callee returns.  (Equal to the callee's for a self-recursive call;
    /// differ for a general Python→Python call.)
    saved_code_ptr: *const crate::bytecode::FnCode,
    saved_active_code_rc: Option<Rc<crate::bytecode::FnCode>>,
    saved_num_locals: u32,
    saved_fn_id: Option<u64>,
    saved_env: EnvRef,
}

/// Placeholder left inside a generator's state cell (`Rc<RefCell<Box<dyn Any>>>`)
/// while its `GeneratorFrame` is checked out and being driven (#2253).  Keeps the
/// cell's contents valid (and the borrow released) during the drive, and lets a
/// re-entrant `ForIter` on the same generator recognise "already executing"
/// (→ `ValueError`) instead of mis-reading it as an exhausted iterator.
struct GenDriving;

/// Generator-trampoline frame (#2253): the generator's checked-out frame plus
/// the consumer state restored when the generator yields / returns / raises.
/// Module-scoped for the same reason as [`TrampFrame`].
struct GenDriveFrame {
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
    saved_fn_id: Option<u64>,
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
    current_fn_id: &'a mut Option<u64>,
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

/// Boxed payload for [`IterState::BigRange`] (kept out-of-line so the cold
/// arbitrary-precision range variant doesn't inflate `IterState`).
#[derive(Clone)]
pub(crate) struct BigRangeState {
    pub(crate) cur: PyBigInt,
    pub(crate) stop: PyBigInt,
    pub(crate) step: PyBigInt,
}

#[derive(Clone)]
pub(crate) enum IterState {
    Materialized(Vec<Value>, usize),
    Range { cur: i64, stop: i64, step: i64 },
    /// Lazy arbitrary-precision range iteration (#2118): advances a `BigInt`
    /// cursor by `step` each tick, never materializing the (potentially huge)
    /// sequence.  Only produced for ranges with out-of-i64 bounds.
    ///
    /// Boxed to keep `IterState` small: three inline `PyBigInt`s (96 bytes)
    /// would make this cold, rarely-produced variant the largest, inflating
    /// `ItersBuf` (on every VM frame) and `GeneratorFrame` (per suspended
    /// generator).  The single heap allocation is paid once per big-range
    /// iterator (out-of-`i64` ranges only), never on the common iteration path.
    BigRange(Box<BigRangeState>),
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
    /// Materialized snapshot guarded against size mutation (#1988): dict / set
    /// and their keys/values/items views.  Holds the live `container` Value and
    /// the length recorded at iterator creation; on each `ForIter` the live
    /// length is re-read and compared, raising `RuntimeError` (CPython's
    /// "dictionary/Set changed size during iteration") on a mismatch.  Only the
    /// *size* is guarded — value-only mutations that preserve the key count are
    /// allowed, matching CPython.  The single `usize` compare per step keeps the
    /// iteration hot path unchanged.
    MaterializedGuarded {
        items: Vec<Value>,
        pos: usize,
        container: Value,
        recorded_len: usize,
        msg: &'static str,
        /// OrderedDict semantics: exhaustion is tested BEFORE the size guard,
        /// so mutating on the final step completes silently (CPython's odict
        /// iterators; plain dict checks the guard first).
        exhaust_first: bool,
    },
}

/// Read the live element count of a `dict` / `set` / dict-view `container`,
/// used by the size-mutation guard (#1988).  Returns `None` if `container` is
/// not one of those types (in which case the guard treats the snapshot as
/// non-mutating, since no other guarded source reaches this path).
pub(crate) fn live_collection_len(container: &Value) -> Option<usize> {
    // Unguarded `as_dict` / `as_set` read via `as_ptr` (no RefCell borrow-flag
    // traffic), keeping the per-step iteration guard cheap.
    if let Some(d) = container.as_dict() {
        return Some(d.len());
    }
    if let Some(s) = container.as_set() {
        return Some(s.len());
    }
    // dict-subclass instances (Counter / defaultdict / OrderedDict, #2201):
    // re-resolve the `__builtin_data__` backing dict each step.  Re-reading the
    // instance attr each step (rather than capturing the backing `Rc` at
    // iterator creation) keeps the guard correct regardless of whether a
    // mutation rewrites the backing map in place (`store_backing`, #2447) or
    // replaces the whole `Value`: either way this sees the current backing and
    // detects the size change.  Only reached on the cold guarded path (these
    // three subclasses); the common dict/set/deque guard above is untouched.
    if let ValueKind::PyInstance(inst) = container.kind()
        && let Some(backing) = instance_builtin_data(inst)
    {
        return backing.as_dict().map(|d| d.len());
    }
    pyrust_builtins::dict_views::as_dict_rc(container).map(|rc| rc.borrow().len())
}

/// `true` if `class` is `collections.OrderedDict` or a subclass of it (#2201).
/// Drives the OrderedDict-specific "OrderedDict mutated during iteration"
/// message; every other dict subclass uses dict's wording.  Walks the `base`
/// chain (single-inheritance backbone is sufficient — `collections.OrderedDict`
/// is itself a plain `dict` subclass) and matches on the class name plus a
/// `__module__ == "collections"` tag (set in `env.rs` per #2228), so a
/// user-defined class merely *named* `OrderedDict` in another module is not
/// mistaken for it.
pub(crate) fn class_is_named_ordered_dict(class: &Rc<RefCell<PyClass>>) -> bool {
    let mut cur = Some(Rc::clone(class));
    while let Some(c) = cur {
        let b = c.borrow();
        let is_collections = b
            .attrs
            .get("__module__")
            .and_then(|v| v.as_str())
            .is_some_and(|m| m == "collections");
        if is_collections && b.name == "OrderedDict" {
            return true;
        }
        cur = b.base.clone();
    }
    false
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

/// Arbitrary-precision counterpart of [`for_count_step`] (#2118).  Reached only
/// when the loop counter or stop has promoted past `i64` (e.g. `for i in
/// range(10**19, 10**19+4)`), which the compiler's for-range / while-range
/// rewrite cannot statically rule out because the bounds are runtime values.
/// `cur`/`stop` are read as `BigInt`; `step` is the (always-i64) const.
macro_rules! for_count_step_big {
    ($regs:ident, $var:expr, $cur:expr, $stop:expr, $step:expr, $cmp_op:expr, $pc:ident, $offset:expr) => {{
        let next: PyBigInt = $cur + PyBigInt::from($step);
        let cont = match $cmp_op {
            BinaryOp::Lt => next < $stop,
            BinaryOp::Gt => next > $stop,
            _ => unreachable!("ForCount* uses Lt or Gt only"),
        };
        if cont {
            $regs[$var as usize] = value_from_bigint(next);
        } else {
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
        if let Some(ref exc) = inject_exc
            && let Some(crate::bytecode::Insn::YieldFrom { iter_reg, sent_reg, result_reg }) =
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
        });
        // SAFETY: regs_ptr is valid for regs_len Values for the lifetime of
        // frame.regs (which outlives this call).  No &mut [Value] referencing
        // frame.regs is held while the dispatch loop runs; RegSlice (raw
        // pointer + len) is used instead, removing the LLVM noalias constraint
        // that made the VmFrameView dereferences UB (issue #547).
        let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
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
        // Lazy traceback: record the generator frame's `FrameInfo` only when an
        // exception propagated out of the body (issue #908).  On yield
        // (Ok(Yielded)) the body suspended successfully — nothing to record.
        if result.is_err() {
            let tb_filename = self
                .script_filename
                .clone()
                .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
            let tb_lineno = match pyrust_core::get_current_vm_line() {
                0 => None,
                n => Some(n),
            };
            pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                filename: tb_filename,
                lineno: tb_lineno,
                source_line: None,
                funcname: frame.fn_name.clone(),
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
        // Recursion limit spans native depth + both trampolines.
        if call_depth() + st.tramp_stack.len() + st.gen_drive_stack.len()
            >= max_call_depth()
        {
            let exc = self.instantiate_named_exception(
                "RecursionError",
                "maximum recursion depth exceeded".to_string(),
            )?;
            return Err(PyError::Raised(exc));
        }
        // Check the generator frame out of its `Value`.
        let mut gframe: Box<GeneratorFrame> = {
            let mut b = state_rc.borrow_mut();
            let taken =
                std::mem::replace(&mut *b, Box::new(GenDriving) as Box<dyn std::any::Any>);
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
        let gen_regs_ptr =
            unsafe { std::ptr::NonNull::new_unchecked(gframe.regs.as_mut_ptr()) };
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
            saved_fn_id: *st.current_fn_id,
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
        *st.current_fn_id = None;
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
        loop {
            let floor = st.gen_drive_stack.last().map_or(0, |g| g.tramp_floor);
            while st.tramp_stack.len() > floor {
                let saved = st.tramp_stack.pop().unwrap();
                if let Some(view) = self.vm_frame_views.pop()
                    && let Some(func) = view.function
                {
                    let file = self
                        .script_filename
                        .clone()
                        .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
                    pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                        filename: file,
                        lineno: if line == 0 { None } else { Some(line) },
                        source_line: None,
                        funcname: std::sync::Arc::from(&func.name[..]),
                        col_span: None,
                    });
                }
                self.env = saved.saved_env;
                line = saved.saved_cur_line;
            }
            let Some(mut gd) = st.gen_drive_stack.pop() else {
                break;
            };
            // The error escaped the generator body: finalize it.
            gd.gframe.done = true;
            if self.vm_frame_views.pop().is_some() {
                let file = self
                    .script_filename
                    .clone()
                    .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
                pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                    filename: file,
                    lineno: if line == 0 { None } else { Some(line) },
                    source_line: None,
                    funcname: gd.gframe.fn_name.clone(),
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
            *st.current_fn_id = gd.saved_fn_id;
            self.env = gd.saved_env;
            *st.iters = gd.saved_iters;
            *st.iter_next_cache = gd.saved_iter_cache;
            *st.exc_handlers = gd.saved_exc_handlers;
            *st.tramp_active_base = gd.saved_base;
            let boxed: Box<dyn std::any::Any> = gd.gframe;
            *gd.state_rc.borrow_mut() = boxed;
            // PEP 479: a `StopIteration` escaping a generator body becomes
            // `RuntimeError` in the *consumer's* env.
            err = pep479_wrap_stop_iteration(&self.env, err);
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
                    continue;
                }
            }
        }
        *st.cur_line = line;
        pyrust_core::set_current_vm_line(line);
        UnwindOutcome::Escape(err)
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
                None => return Err(e),
            }
        } else {
            // Zero-cost table lookup.  `EXC_NO_HANDLER` ⇒ no handler covers this
            // pc ⇒ the exception propagates to the caller frame.
            match exc_table.get(raise_pc).copied() {
                Some(t) if t != crate::bytecode::EXC_NO_HANDLER => t as usize,
                _ => return Err(e),
            }
        };
        let exc_val = match e {
            PyError::Raised(v) => v,
            PyError::Runtime(msg) => {
                if let Some(cls) = self.exc_classes.get("RuntimeError") {
                    instantiate_exception(cls, vec![Value::string(msg)])
                } else {
                    self.instantiate_named_exception("RuntimeError", msg)?
                }
            }
            PyError::Named(cls, msg) => {
                self.instantiate_named_exception(&cls, msg)?
            }
            PyError::Class(cls, msg) => {
                let args = if msg.is_empty() { vec![] } else { vec![Value::string(msg)] };
                instantiate_exception(cls, args)
            }
            PyError::KeyError(key) => {
                self.instantiate_named_exception_with_value("KeyError", key)?
            }
            PyError::NameError { class_name, message, name } => {
                self.instantiate_name_error_exception(class_name, message, name)?
            }
            PyError::AttributeError { message, name, obj } => {
                self.instantiate_attribute_error_exception(message, name, obj)?
            }
            PyError::ImportError { class_name, message, module_name } => {
                self.instantiate_import_error_exception(class_name, message, module_name)?
            }
            PyError::OsError {
                class_name,
                errno,
                strerror,
                filename,
                filename2,
            } => {
                self
                    .instantiate_os_error_exception(class_name, errno, strerror, filename, filename2)?
            }
            PyError::UnicodeDecodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => {
                self.instantiate_unicode_decode_error_exception(
                    encoding, object, start, end, reason,
                )?
            }
            PyError::UnicodeEncodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => {
                self.instantiate_unicode_encode_error_exception(
                    encoding, object, start, end, reason,
                )?
            }
            other => return Err(other),
        };
        self.attach_implicit_context(&exc_val);
        // Issue #1441 / #2170: set __traceback__ on the exception instance when
        // it is caught.  Only PyInstance values (all normal exceptions) have an
        // attrs dict to write to.  The traceback object is a real walkable chain
        // built from the lazily-captured unwind frames (#2165) plus the catching
        // frame, so `e.__traceback__.tb_next…` walks every frame in order.
        //
        // Issue #2351: storing a *deferred* placeholder here (cheap snapshot
        // only) instead of eagerly materialising the chain — the dominant cost
        // of the raise/catch path — and materialising on first read of
        // `__traceback__` keeps the hot path off `build_code_object` while
        // preserving identical Python-visible behaviour.
        // Consume the bare-re-raise marker (issue #2367) unconditionally, so it
        // never leaks past the catch that handles the re-raised exception even
        // when the caught value is not a `PyInstance`.  A bare `raise` rebuilds
        // the chain fresh instead of prepending onto the carried chain.
        let is_bare_reraise = std::mem::take(&mut self.reraise_is_bare);
        if let ValueKind::PyInstance(inst_rc) = exc_val.kind() {
            // Issue #2359: if the exception already carries a *materialised* real
            // traceback (because a `with`-statement's `__exit__` — or any earlier
            // read — observed `__traceback__` while it was in flight) and the
            // current catch is in the same frame that built it (same captured
            // unwind-frame snapshot ⇒ identical chain), keep that object so the
            // tb `__exit__` saw is identical to the one an outer `except` in the
            // same frame reads.  Re-deferring here would mint a fresh, equal-but-
            // not-identical chain and break that identity contract.  When the
            // exception has crossed a frame boundary the snapshot grew, so the
            // chain differs and we rebuild (matching CPython, which prepends the
            // new frame and yields a distinct head object).
            // Issue #2367: when the slot already carries a chain (the exception
            // is being re-raised / carried across a frame), CPython prepends the
            // re-raising frame onto the existing chain rather than discarding it.
            // `caught_traceback_value` decides between a plain build (fresh
            // exception, slot is `None`), keeping the existing object (same-frame
            // identity from #2359/#2366), and a prepend.
            //
            // Single key scan: `__traceback__` is pre-initialised to `None` on
            // every exception instance, so the common path (fresh exception) is
            // one `get_mut`, an `is_none()` check, then an overwrite — the same
            // cost as the unconditional insert this replaced.  The frame-count
            // probe and prepend only run once the slot holds a real/deferred
            // chain, i.e. a genuine re-raise.
            let mut inst = inst_rc.borrow_mut();
            if let Some(slot) = inst.attrs.get_mut("__traceback__") {
                if let Some(new_tb) =
                    self.caught_traceback_value(slot, catch_lineno as i64, is_bare_reraise)
                {
                    *slot = new_tb;
                }
            } else {
                let tb = self.build_deferred_traceback(catch_lineno as i64);
                inst.attrs.insert("__traceback__", tb);
            }
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

        // Generalized-trampoline foundation (#2234): the active frame's `code`
        // can change when a Python→Python call is trampolined, so it is held as
        // a raw pointer re-derived at the loop top.  `active_code_rc` keeps a
        // trampolined callee's `FnCode` alive (`None` ⇒ the bottom frame uses
        // the borrowed `code` param).  `num_locals` / `current_fn_id` are
        // likewise rebindable.
        let mut code_ptr: *const crate::bytecode::FnCode = code;
        let mut active_code_rc: Option<Rc<crate::bytecode::FnCode>> = None;
        let mut num_locals = code.num_locals;
        let mut current_fn_id = current_fn_id;

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
        // `run_bytecode_for_fn` (native call machinery, ~156ns/call), push the
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
                current_fn_id = __gd.saved_fn_id;
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
                    tramp_arena.truncate(tramp_active_base);
                    tramp_active_base = __saved.saved_base;
                    regs = __saved.saved_regs;
                    pc = __saved.saved_pc;
                    cur_line = __saved.saved_cur_line;
                    // Restore the caller's code / locals / fn-id / env (a no-op
                    // for a self-recursive call; a real switch for a general
                    // Python→Python call).
                    code_ptr = __saved.saved_code_ptr;
                    active_code_rc = __saved.saved_active_code_rc;
                    num_locals = __saved.saved_num_locals;
                    current_fn_id = __saved.saved_fn_id;
                    self.env = __saved.saved_env;
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
                    Err(e) => match self.handle_vm_error(
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
                                bump_global_env_version(self);
                            }
                            pc = h;
                            continue 'vm;
                        }
                        Err(e) => {
                            // Error escapes this frame: publish the raising
                            // instruction's PEP 657 caret anchor (#2426) for the
                            // traceback formatter.  Read only here on the cold
                            // escape path — never on the per-instruction hot path.
                            // Last writer wins, so the outermost (module) frame's
                            // anchor is what `get_current_vm_col_span` returns.
                            pyrust_core::set_current_vm_col_span(
                                code.col_table.get(pc.wrapping_sub(1)).copied().and_then(
                                    |s| if s == (0, 0) { None } else { Some(s) },
                                ),
                            );
                            // Unwind any active trampolined frames (record their
                            // traceback, pop their views, restore each caller's
                            // env) and publish the line tracker (issues #348,
                            // #2234).
                            tramp_unwind_err!(e);
                        }
                    },
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
                    current_fn_id: &mut current_fn_id,
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
                            $tag, $idx, ($pool).len()
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
            // `$bind` = statements that fill `tramp_arena[base + …]`.
            (@enter $lt:lifetime, $dst:expr, $f:expr, $nsupplied:expr,
                    $fb:ident, $base:ident, $np:ident, $bind:block) => {{
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
                if call_depth() + tramp_stack.len() >= max_call_depth() {
                    let exc = self.instantiate_named_exception(
                        "RecursionError",
                        "maximum recursion depth exceeded".to_string(),
                    )?;
                    tramp_unwind_err!(PyError::Raised(exc));
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
                let callee_fn_id = f.id;
                let callee_env = Rc::clone(&f.env);
                let callee_code_ptr: *const crate::bytecode::FnCode =
                    Rc::as_ptr(&callee_code);
                // SAFETY: base+num_regs <= capacity (checked) ⇒ no realloc;
                // pointer valid until this frame's `truncate` on return.
                let new_ptr = unsafe { tramp_arena.as_mut_ptr().add($base) };
                tramp_stack.push(TrampFrame {
                    saved_regs: regs,
                    saved_pc: pc,
                    dst: $dst,
                    saved_base: tramp_active_base,
                    saved_cur_line: cur_line,
                    saved_code_ptr: code_ptr,
                    saved_active_code_rc: active_code_rc.take(),
                    saved_num_locals: num_locals,
                    saved_fn_id: current_fn_id,
                    saved_env: std::mem::replace(&mut self.env, callee_env),
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
                });
                tramp_active_base = $base;
                active_code_rc = Some(callee_code);
                code_ptr = callee_code_ptr;
                num_locals = callee_num_locals;
                current_fn_id = Some(callee_fn_id);
                regs = unsafe { RegSlice::from_raw(new_ptr, num_regs) };
                pc = 0;
                continue 'vm;
            }};
            // Plain-call form: callee at register `$func_reg`, `$argc`
            // positional args read from `$func_reg + 1 ..`.
            ($func_reg:expr, $argc:expr, $func_val:expr) => {
                'trampoline: {
                    let ValueKind::UserFunction(f0) = $func_val.kind() else {
                        break 'trampoline;
                    };
                    tramp_try!(@enter 'trampoline, $func_reg, f0, $argc, f, base, nparams, {
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
                    tramp_try!(@enter 'trampoline, $dst, $f, ($argc as usize) + 1, f, base, nparams, {
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
                && ln != 0 {
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
                    // Issue #2340: a `LoadGlobal` name is never the current
                    // frame's own register-local (those use Move / CheckLocal)
                    // and — having reached this opcode rather than `LoadCell` — is
                    // not one of this scope's cell vars either.  So if the env
                    // walk finds it as an *unbound local* of an enclosing scope it
                    // is a captured free variable → CPython raises `NameError`,
                    // not `UnboundLocalError`.  list/set/dict comprehensions are
                    // the exception: CPython 3.12 inlines them into the enclosing
                    // frame (PEP 709), so an unbound enclosing-local read there
                    // stays `UnboundLocalError`.  (`current_fn_id` is unreliable
                    // here — it is `None` while a generator/genexpr body resumes.)
                    let is_free = !code.is_inlined_comp;
                    let (val, cache_ver) = if let Some(v) =
                        vm_try!(self.lookup_name_inner(name, is_free))
                    {
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
                Insn::LoadCell(dst, name_idx) => {
                    // Function-scope cell / nonlocal read (issue #2339).  The
                    // compiler proved this name resolves in the env chain, so we
                    // skip the LoadGlobal inline-cache probe entirely and go
                    // straight to `lookup_name`, which routes a `nonlocal` to the
                    // enclosing owning env and a cell to this scope's env.  The
                    // common case (a bound cell) returns `Some` and we are done.
                    let name = pool_get!(code.names, *name_idx, "name");
                    // Issue #2340: an unbound binding is a captured *free*
                    // variable (CPython `NameError`) unless it is a cell var
                    // declared in *this* function (a local captured by a nested
                    // function → `UnboundLocalError`).  `nonlocal` names are not
                    // in `cell_vars`, so they correctly take the free path.  An
                    // inlined list/set/dict comprehension (PEP 709) reads the
                    // enclosing frame's locals, so it keeps `UnboundLocalError`.
                    let is_cell_local = code.cell_vars.iter().any(|c| c == name);
                    let is_free = !is_cell_local && !code.is_inlined_comp;
                    if let Some(v) = vm_try!(self.lookup_name_inner(name, is_free)) {
                        regs[*dst as usize] = v;
                    } else {
                        // Env miss: a `del`-ed / never-bound cell.  Fall through
                        // to the SAME resolution tail LoadGlobal uses (module
                        // dict → restricted/builtins) so the raised error is
                        // byte-identical to the pre-#2339 LoadGlobal path.  This
                        // is rare and cold.
                        let val = vm_try!(self.resolve_cell_miss(name));
                        regs[*dst as usize] = val;
                    }
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
                    } else if let Some(mut iacc) = regs[base_idx].as_int()
                        && let Some(second) = regs[base_idx + 1].as_int()
                        && let Some(sum) = iacc.checked_add(second)
                    {
                        // Int fast path (#2381): a chain `a + b + c + …` over
                        // small ints. `pass_concat_merge` fuses *every* Add chain,
                        // not just string concatenation, so this — not string
                        // building — is the common case (`return a+b+c` in a hot
                        // method body). Accumulate in an i64 with the same
                        // left-to-right semantics as the BinOp chain it replaced,
                        // bailing to `eval_binary` the moment an operand is not a
                        // small int or the running sum overflows i64 (BigInt
                        // promotion / `__add__` dispatch).
                        iacc = sum;
                        let mut k = 2;
                        let mut overflow_or_obj = false;
                        while k < n {
                            match regs[base_idx + k].as_int() {
                                Some(v) => match iacc.checked_add(v) {
                                    Some(s) => iacc = s,
                                    None => {
                                        overflow_or_obj = true;
                                        break;
                                    }
                                },
                                None => {
                                    overflow_or_obj = true;
                                    break;
                                }
                            }
                            k += 1;
                        }
                        if overflow_or_obj {
                            // Resume the slow chain from the accumulated prefix so
                            // BigInt promotion and user `__add__` see the exact
                            // intermediate value CPython would.
                            let mut acc = Value::int(iacc);
                            while k < n {
                                let next = vm_try!(vm_read(&regs, *base + k as u32, num_locals));
                                acc =
                                    vm_try!(self.eval_binary(acc, crate::ast::BinaryOp::Add, next));
                                k += 1;
                            }
                            regs[*dst as usize] = acc;
                        } else {
                            regs[*dst as usize] = Value::int(iacc);
                        }
                    } else {
                        // Fallback: sequential BinOp(Add) — correct but allocates intermediates.
                        let mut acc = vm_try!(vm_read(&regs, *base, num_locals));
                        for k in 1..n {
                            let next = vm_try!(vm_read(&regs, *base + k as u32, num_locals));
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
                Insn::GetAttrForWith(dst, obj, name_idx, proto) => {
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    let type_name = value_type_name_str(&obj_val);
                    match self.get_attr(&obj_val, name) {
                        Ok(v) => regs[*dst as usize] = v,
                        Err(_) => {
                            // CPython converts any lookup failure (AttributeError or
                            // otherwise) to the protocol's TypeError.  `proto`
                            // selects the message (see Insn::GetAttrForWith docs).
                            let msg = match *proto {
                                1 => format!(
                                    "'{}' object does not support the context manager protocol \
                                     (missed __exit__ method)",
                                    type_name
                                ),
                                2 => format!(
                                    "'async for' requires an object with __aiter__ method, got {}",
                                    type_name
                                ),
                                3 => format!(
                                    "'{}' object does not support the asynchronous context \
                                     manager protocol",
                                    type_name
                                ),
                                4 => format!(
                                    "'{}' object does not support the asynchronous context \
                                     manager protocol (missed __aexit__ method)",
                                    type_name
                                ),
                                5 => format!(
                                    "'async for' received an object from __aiter__ that does \
                                     not implement __anext__: {}",
                                    type_name
                                ),
                                _ => format!(
                                    "'{}' object does not support the context manager protocol",
                                    type_name
                                ),
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
                Insn::SetTypeVarAttr(obj, name_idx, val) => {
                    // Internal PEP 695 bound/constraint population — writes
                    // `__bound__` / `__constraints__` directly onto the freshly
                    // created TypeVar, bypassing the user-facing read-only guard
                    // on `Insn::SetAttr` (see `typevar_readonly_attr_error`).
                    let obj_val = vm_try!(vm_read(&regs, *obj, num_locals));
                    let val = vm_try!(vm_read(&regs, *val, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    if let ValueKind::PyInstance(inst) = obj_val.kind() {
                        inst.borrow_mut().attrs.insert(name, val);
                    }
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
                    // PEP 695: enter a dedicated type-parameter scope.  Subsequent
                    // StoreGlobal for the type-param names binds into this child
                    // env (parented to the current one) instead of the enclosing
                    // namespace, so the names never leak; a generic function/class
                    // created while this scope is active captures it and can still
                    // resolve the type parameters lazily in its body.
                    let tp_env = self.alloc_env(Some(Rc::clone(&self.env)));
                    self.env = tp_env;
                    // Invalidate the LoadGlobal inline cache: a type-param name may
                    // shadow a same-named enclosing global, and the cache holds the
                    // enclosing value at the current version (the StoreGlobal that
                    // binds the type param writes to the child env via
                    // env_assign_local, which does not bump the version).  Without
                    // this, an annotation `x: T` that follows a module-level
                    // `T = ...` would read the stale enclosing value from the cache.
                    bump_global_env_version(self);
                }
                Insn::PopTypeParamEnv => {
                    // PEP 695: leave the type-parameter scope, restoring the
                    // enclosing env.  The popped env stays alive via the Rc the
                    // generic object captured at MakeFunction/MakeClass time; if it
                    // was not captured (uncommon) `free_env` reclaims it.
                    let parent = self
                        .env
                        .borrow()
                        .parent
                        .clone()
                        .expect("PopTypeParamEnv without a matching PushTypeParamEnv");
                    let tp_env = std::mem::replace(&mut self.env, parent);
                    self.free_env(tp_env);
                    // Re-invalidate the cache so the enclosing frame's later
                    // references to a name that was shadowed by a type param
                    // re-resolve against the restored enclosing scope.
                    bump_global_env_version(self);
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
                Insn::LoadExcTraceback(dst, exc) => {
                    // Resolve the exception's `__traceback__`, materialising the
                    // deferred placeholder (#2351) so `__exit__` receives a real
                    // `traceback` object (#2359).  Mirrors the get_attr read site
                    // in env.rs: write the materialised chain back onto the
                    // instance so the object passed to `__exit__` is identical to
                    // a later `e.__traceback__` read.
                    let exc_val = vm_try!(vm_read(&regs, *exc, num_locals));
                    let tb = if let Some(inst) = exc_val.as_py_instance_rc() {
                        let stored = inst.borrow().attrs.get("__traceback__").cloned();
                        match stored {
                            Some(v) => match self.materialize_deferred_traceback(&v) {
                                Some(real) => {
                                    inst.borrow_mut()
                                        .attrs
                                        .insert("__traceback__", real.clone());
                                    real
                                }
                                None => v,
                            },
                            None => Value::none(),
                        }
                    } else {
                        Value::none()
                    };
                    regs[*dst as usize] = tb;
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
                    // Issue #2367: `raise e` / `raise e.with_traceback(tb)` on an
                    // exception that already carries a traceback is a re-raise.
                    // CPython keeps the carried chain and *prepends* the frames
                    // this new raise unwinds through.  Reset the captured-frame
                    // snapshot so the stale frames of the original raise are not
                    // re-counted; the catch site links the carried chain on as
                    // the tail (see `caught_traceback_value`).
                    self.reraise_is_bare = false;
                    self.reset_captured_frames_if_reraise(&exc);
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
                        inst.borrow_mut().attrs.insert("__cause__", cause);
                        inst.borrow_mut()
                            .attrs
                            .insert("__suppress_context__", Value::bool_(true));
                    }
                    // Issue #2367: same re-raise prepend handling as RaiseValue.
                    self.reraise_is_bare = false;
                    self.reset_captured_frames_if_reraise(&exc);
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
                    // Heap-tuple args to a builtin: lend by move instead of
                    // clone.  `Value::clone` of a heap tuple deep-copies the
                    // backing `Vec<Value>` (O(N)), so cloning a 1000-element
                    // tuple into the arg buffer for `len(t)`/`sum(t)`/`min(t)`/…
                    // paid an O(N) copy on every call (#2251).  The builtin only
                    // gets a borrowed `&[ExpandedCallArg]`, so it cannot retain
                    // the value without cloning it itself — meaning a builtin
                    // that needs ownership (`list(t)`, `iter(t)`) still copies,
                    // while a read-only builtin pays nothing.  We move each arg
                    // out of its (temporary) register into the buffer and restore
                    // it afterwards, so register state is observably unchanged.
                    //
                    // Gated on `BuiltinFunction` callee + at least one heap-tuple
                    // arg so the hot user-function and scalar-arg call paths take
                    // the byte-identical clone path below with zero added work.
                    let lend_by_move = matches!(func_val.kind(), ValueKind::BuiltinFunction(_))
                        && (0..crate::bytecode::Reg::from(*argc)).any(|i| {
                            regs.get((*func_reg + 1 + i) as usize)
                                .is_some_and(|v| v.is_heap_tuple())
                        });
                    if lend_by_move {
                        // Validate every arg slot is set *before* moving any out,
                        // so an unset arg propagates the exact same error as the
                        // clone path without leaving registers half-emptied.  Done
                        // before the buffer is taken so the error path is trivial.
                        // (`vm_read` on an unset slot always returns `Err`, which
                        // `vm_try!` propagates — control never falls through.)
                        for i in 0..crate::bytecode::Reg::from(*argc) {
                            let reg = *func_reg + 1 + i;
                            if regs[reg as usize].is_unset() {
                                vm_try!(vm_read(&regs, reg, num_locals));
                            }
                        }
                    }
                    // Reuse the interpreter-level buffer to avoid a per-call heap
                    // allocation in the common (non-recursive) case.
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    if lend_by_move {
                        for i in 0..crate::bytecode::Reg::from(*argc) {
                            let reg = *func_reg + 1 + i;
                            let value = std::mem::replace(
                                &mut regs[reg as usize],
                                pyrust_core::Value::unset(),
                            );
                            buf.push(ExpandedCallArg { name: None, value });
                        }
                    } else {
                        for i in 0..crate::bytecode::Reg::from(*argc) {
                            buf.push(ExpandedCallArg {
                                name: None,
                                value: vm_try!(vm_read(&regs, *func_reg + 1 + i, num_locals)),
                            });
                        }
                    }
                    // Publish the register-resident current line so that
                    // `sys._getframe()` reads the line its call is on, giving an
                    // exact `frame.f_lineno` for the innermost frame (issue
                    // #2185).  Gated on the `_getframe` builtin specifically (its
                    // registered name is namespaced, `"sys._getframe"`) — a cheap
                    // name compare that short-circuits for every other callee —
                    // so neither the hot user-function call path nor the common
                    // builtin-call path pays a thread-local write.
                    if matches!(func_val.kind(), ValueKind::BuiltinFunction(n) if n.ends_with("_getframe"))
                    {
                        pyrust_core::set_current_vm_line(cur_line);
                    }
                    let call_result = self.call_function_expanded(func_val, &buf);
                    if lend_by_move {
                        // Move the borrowed args back into their registers before
                        // anything else can observe them (the result write below
                        // overwrites `func_reg`, not the arg temps).
                        for (i, arg) in buf.drain(..).enumerate() {
                            regs[(*func_reg + 1 + i as crate::bytecode::Reg) as usize] = arg.value;
                        }
                    }
                    self.call_arg_buf = buf;
                    regs[*func_reg as usize] = vm_try!(call_result);
                }

                Insn::CallMemo(func_reg, argc) => {
                    // `CallMemo` marks a call to a statically-pure callee.  An
                    // adaptive, bounded result cache memoizes pure functions with
                    // integer args and a value-identity scalar (int/bool/None)
                    // result — collapsing exponential recursion like `fib` to
                    // O(n) (#2234).  Parity-safe: pyrust scalars already compare
                    // by value-identity, so a shared cached result is observably
                    // transparent.  The adaptive gate (`memo_stats`) disables a
                    // function whose hit-rate stays low after a warmup, so the
                    // common varying-argument case pays nothing (the regression
                    // that removed the previous always-on cache, #1987).
                    let func_val = vm_try!(vm_read(&regs, *func_reg, num_locals));
                    'memo: {
                        let fid = match func_val.kind() {
                            ValueKind::UserFunction(f) if f.is_pure => f.id,
                            _ => break 'memo,
                        };
                        if matches!(self.memo_stats.get(&fid), Some((_, _, false))) {
                            break 'memo;
                        }
                        let mut key_args: smallvec::SmallVec<[i64; 3]> =
                            smallvec::SmallVec::new();
                        for i in 0..crate::bytecode::Reg::from(*argc) {
                            match vm_try!(vm_read(&regs, *func_reg + 1 + i, num_locals))
                                .as_int()
                            {
                                Some(n) => key_args.push(n),
                                None => break 'memo,
                            }
                        }
                        let key = (fid, key_args);
                        if let Some(cached) = self.memo_cache.get(&key) {
                            let v = cached.clone();
                            let st = self.memo_stats.entry(fid).or_insert((0, 0, true));
                            st.0 = st.0.saturating_add(1);
                            st.1 = st.1.saturating_add(1);
                            regs[*func_reg as usize] = v;
                            continue 'vm;
                        }
                        // Miss: compute it (native call), then store a scalar
                        // result.  The cache prunes the recursion tree, so misses
                        // are few (e.g. ~n for `fib(n)`).
                        let mut buf = std::mem::take(&mut self.call_arg_buf);
                        buf.clear();
                        for i in 0..crate::bytecode::Reg::from(*argc) {
                            buf.push(ExpandedCallArg {
                                name: None,
                                value: vm_try!(vm_read(&regs, *func_reg + 1 + i, num_locals)),
                            });
                        }
                        let call_result = self.call_function_expanded(func_val.clone(), &buf);
                        self.call_arg_buf = buf;
                        let result = vm_try!(call_result);
                        let st = self.memo_stats.entry(fid).or_insert((0, 0, true));
                        st.0 = st.0.saturating_add(1);
                        // Adaptive: after a warmup, disable functions whose
                        // hit-rate stays below ~25%.
                        if st.0 >= 128 && st.1.saturating_mul(4) < st.0 {
                            st.2 = false;
                        }
                        let store = st.2;
                        if store
                            && self.memo_cache.len() < (1usize << 16)
                            && (matches!(result.kind(), ValueKind::Int(_) | ValueKind::Bool(_))
                                || result.is_none())
                        {
                            self.memo_cache.insert(key, result.clone());
                        }
                        regs[*func_reg as usize] = result;
                        continue 'vm;
                    }
                    tramp_try!(*func_reg, *argc, func_val);
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

                Insn::CallKw { func, total, nkw, kwnames_idx } => {
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

                Insn::CallMethod { dst, obj, name_idx, args_base, nargs } => {
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
                    let r = self.exec_call_method(&mut regs, num_locals, *dst, *obj, *name_idx, *args_base, *nargs, code, pc - 1, cur_line);
                    regs[*dst as usize] = vm_try!(r);
                }

                Insn::CallMethodExpanded { dst, obj, name_idx, pos_list, kw_dict } => {
                    let r = self.exec_call_method_expanded(&mut regs, num_locals, *dst, *obj, *name_idx, *pos_list, *kw_dict, code);
                    regs[*dst as usize] = vm_try!(r);
                }
                Insn::CallMethodKw { dst, obj, name_idx, args_base, total, nkw, kwnames_idx } => {
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

                // ── Tail-call ────────────────────────────────────────────
                Insn::TailCall { args_base, nargs } => {
                    // The function to call lives at func_reg = args_base - 1.
                    let func_reg = args_base - 1;
                    let callee_val = vm_try!(vm_read(&regs, func_reg, num_locals));

                    // Self-call check: if the callee is the same user function as
                    // the one currently executing, and we are not inside a try
                    // block (reusing the frame would discard the active handler),
                    // reset the register file and loop back to pc=0.
                    let is_self_call = if let Some(fn_id) = current_fn_id {
                        match callee_val.kind() {
                            ValueKind::UserFunction(f) => f.id == fn_id,
                            _ => false,
                        }
                    } else {
                        false
                    };

                    // "Not inside a try" in both models: with the zero-cost
                    // exception table the dynamic `exc_handlers` stack is always
                    // empty, so consult `exc_table[pc - 1]` (the TailCall's pc)
                    // instead; with the fallback stack consult `exc_handlers`.
                    let no_active_handler = if code.exc_table.is_empty() {
                        exc_handlers.is_empty()
                    } else {
                        code.exc_table
                            .get(pc - 1)
                            .copied()
                            .is_none_or(|t| t == crate::bytecode::EXC_NO_HANDLER)
                    };

                    if is_self_call && no_active_handler {
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
                        tramp_return!(vm_try!(call_result));
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
                Insn::FormatValueSpec(dst, src, spec_r) => {
                    // f-string interpolation with a format spec (`f"{v:.2f}"`).
                    // The spec register holds the already-built spec `str`
                    // (literal, or a nested-f-string result for `f"{v:{w}}"`).
                    // Dispatch through the same `__format__` path the `format`
                    // builtin uses — preserving user `__format__` for
                    // `PyInstance` — but without the `format` global lookup, the
                    // call frame, or the call-arg expansion (issue companion of
                    // #1926's spec-less `FormatValue`).
                    let val = vm_try!(vm_read(&regs, *src, num_locals));
                    let spec_val = vm_try!(vm_read(&regs, *spec_r, num_locals));
                    let spec = spec_val.as_str().unwrap_or("");
                    let s = vm_try!(self.dispatch_dunder_format(&val, spec));
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
                    let mut dict = PyDict::with_capacity_and_hasher(*n as usize, Default::default());
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
                        // Issue #2059: a set comprehension / literal building a
                        // tuple/frozenset key that nests a user object must dedup
                        // against an `__eq__`-equal-but-distinct element.
                        PyKey::Tuple(_) | PyKey::FrozenSet(_)
                            if nested_object_tuple_key(&key) =>
                        {
                            true
                        }
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
                    let pairs = vm_try!(self.mapping_splat_pairs(&src_val));
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

                // `**d` keyword splat in a call: like `DictUpdate` but raises
                // `TypeError` on a duplicate key (CPython `DICT_MERGE`, #2413).
                Insn::DictMergeKwCall { dict, src, name } => {
                    let src_val = vm_try!(vm_read(&regs, *src, num_locals));
                    let pairs = vm_try!(self.mapping_splat_pairs(&src_val));
                    let dict_val = regs[*dict as usize]
                        .as_some()
                        .cloned()
                        .unwrap_or(Value::none());
                    if let Some(kw) = vm_try!(self.dict_merge_kwcall(&dict_val, pairs)) {
                        let fname = self.kwcall_func_name(&regs, num_locals, name, code);
                        vm_try!(Err(multiple_values_kw_error(fname.as_deref(), &kw)));
                    }
                }

                // Named (`kw=v`) argument in a call that also has a `**d` splat:
                // `SetItem` that raises the same duplicate-key `TypeError`.
                Insn::SetItemKwCall { dict, key, val, name } => {
                    let key_val = vm_try!(vm_read(&regs, *key, num_locals));
                    let val_val = vm_try!(vm_read(&regs, *val, num_locals));
                    // Named-argument keys are always interned strings.
                    let key = vm_try!(key_val.to_key().ok_or_else(|| PyError::Runtime(
                        "internal: non-hashable keyword argument key".to_string()
                    )));
                    let dict_val = regs[*dict as usize]
                        .as_some()
                        .cloned()
                        .unwrap_or(Value::none());
                    if let Some(kw) = vm_try!(self.dict_setitem_kwcall(&dict_val, key, val_val)) {
                        let fname = self.kwcall_func_name(&regs, num_locals, name, code);
                        vm_try!(Err(multiple_values_kw_error(fname.as_deref(), &kw)));
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
                        self.vm_frame_views.pop();
                        let dst_reg = gd.dst as usize;
                        regs = gd.saved_regs;
                        pc = gd.saved_pc;
                        cur_line = gd.saved_cur_line;
                        code_ptr = gd.saved_code_ptr;
                        active_code_rc = gd.saved_active_code_rc;
                        num_locals = gd.saved_num_locals;
                        current_fn_id = gd.saved_fn_id;
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
                                self.vm_frame_views.pop();
                                let dst_reg = gd.dst as usize;
                                regs = gd.saved_regs;
                                pc = gd.saved_pc;
                                cur_line = gd.saved_cur_line;
                                code_ptr = gd.saved_code_ptr;
                                active_code_rc = gd.saved_active_code_rc;
                                num_locals = gd.saved_num_locals;
                                current_fn_id = gd.saved_fn_id;
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
                        // A coroutine (`async def`, issue #1039) — and an async
                        // generator (#2280) — is not iterable: `for x in coro:`
                        // raises TypeError, matching CPython.  The type name is
                        // resolved dynamically so an async generator reports
                        // `'async_generator' object is not iterable`.
                        if crate::builtin_modules::builtins::is_coroutine_value(&src_val) {
                            let tn =
                                crate::builtin_modules::builtins::full_type_name_str_pub(&src_val);
                            vm_try!(Err::<(), _>(pyrust_core::type_err!(
                                "'{tn}' object is not iterable"
                            )));
                        }
                        // Detect the kind tag in a scoped block so the
                        // kind() Ref drops before we may move src_val
                        // into IterState / iter_values / make_getitem_iter
                        // (#450).
                        enum IterTag {
                            Range(i64, i64, i64),
                            BigRange(PyBigInt, PyBigInt, PyBigInt),
                            Generator,
                            PyInstance(Rc<RefCell<crate::value::PyInstance>>),
                            BuiltinIterable,
                            ListOrTuple,
                            Other,
                        }
                        let tag = match src_val.kind() {
                            ValueKind::Range { start, stop, step } => IterTag::Range(start, stop, step),
                            ValueKind::BigRange { start, stop, step } => {
                                IterTag::BigRange(start.clone(), stop.clone(), step.clone())
                            }
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
                            IterTag::BigRange(start, stop, step) => {
                                // step is guaranteed non-zero by `Value::range_big`.
                                IterState::BigRange(Box::new(BigRangeState {
                                    cur: start,
                                    stop,
                                    step,
                                }))
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
                                // Issue #2387: a builtin subclass now resolves
                                // `__iter__` via its inherited primitive slot
                                // (`BuiltinFunction("dict.__iter__")`, …).  For an
                                // instance with a `__builtin_data__` backing that
                                // is *not* a user override — iterate the backing
                                // primitive (preserving the dict/set size-mutation
                                // guard and the OrderedDict message) exactly as a
                                // subclass with no `__iter__` did before the slot
                                // was exposed.  A genuine Python `def __iter__`
                                // (UserFunction) still wins, and a non-backed
                                // builtin class with its own `__iter__` sentinel
                                // (e.g. `collections.deque`, whose body installs a
                                // mutation guard) is left untouched.  Covers the
                                // bytes/bytearray subclass case from #2324.
                                let user_iter = effective_user_iter(&class, &inst_rc);
                                if let Some(method_val) = user_iter {
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
                                    } else if backing.as_dict().is_some() {
                                        // dict subclass (OrderedDict / plain `class
                                        // D(dict)`): guard against size mutation
                                        // during iteration (#2201).  The container
                                        // is the *instance* so `live_collection_len`
                                        // re-resolves the live backing dict each
                                        // step (defensive — even if a future
                                        // `store_items` replaces the backing `Rc`).
                                        // OrderedDict uses its own message; every
                                        // other dict subclass matches plain dict.
                                        let items = vm_try!(iter_values(&backing));
                                        let recorded_len = backing.as_dict().map(|d| d.len()).unwrap_or(0);
                                        let (msg, exhaust_first) =
                                            dict_subclass_iter_semantics(&class);
                                        IterState::MaterializedGuarded {
                                            items,
                                            pos: 0,
                                            container: src_val.clone(),
                                            recorded_len,
                                            msg,
                                            exhaust_first,
                                        }
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
                                // dict / set / dict-views: snapshot but guard
                                // against size mutation during iteration (#1988).
                                // Frozensets, str, bytes, etc. are immutable or
                                // never reach this with a live size, so they stay
                                // on the plain Materialized path.
                                let items = vm_try!(iter_values(&src_val));
                                if let Some(recorded_len) = live_collection_len(&src_val) {
                                    let msg = if src_val.set_len().is_some() {
                                        ("Set changed size during iteration", false)
                                    } else if pyrust_builtins::dict_views::is_ordered_view(
                                        &src_val,
                                    ) {
                                        // OrderedDict-backed view (issue #2436):
                                        // CPython's odict views report their own
                                        // wording on size mutation.
                                        ("OrderedDict mutated during iteration", true)
                                    } else {
                                        ("dictionary changed size during iteration", false)
                                    };
                                    IterState::MaterializedGuarded {
                                        items,
                                        pos: 0,
                                        container: src_val,
                                        recorded_len,
                                        msg: msg.0,
                                        exhaust_first: msg.1,
                                    }
                                } else {
                                    IterState::Materialized(items, 0)
                                }
                            }
                        }
                    };
                    iters[*slot as usize] = Some(state);
                    iter_next_cache[*slot as usize] = None;
                }
                Insn::ForIter(dst, slot, offset) => {
                    // Generator trampoline (#2253): set inside the match when the
                    // iterator is a drivable generator; the switch-in is performed
                    // after the match closes (it needs `&mut iters`, borrowed here).
                    let mut gen_to_drive: Option<Rc<RefCell<Box<dyn std::any::Any>>>> = None;
                    // Set when the iterator is a generator that is *already
                    // executing* (its state cell is borrowed by an enclosing
                    // resume, or holds the `GenDriving` placeholder because it is
                    // being driven up the stack) → CPython `ValueError: generator
                    // already executing`, rather than the re-entrant-borrow panic
                    // the per-step dispatch would hit.
                    let mut gen_executing = false;
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
                        Some(IterState::MaterializedGuarded {
                            items,
                            pos,
                            container,
                            recorded_len,
                            msg,
                            exhaust_first,
                        }) => {
                            // Size-mutation guard (#1988): one usize compare per
                            // step.  Plain dict raises whenever the size differs
                            // — including the step that would otherwise exhaust
                            // the snapshot.  OrderedDict (#2436 review) tests
                            // exhaustion FIRST, matching CPython's odict
                            // iterators: a mutation on the final step completes.
                            if *exhaust_first && *pos >= items.len() {
                                pc = jump_pc!(*offset);
                                continue;
                            }
                            if live_collection_len(container) != Some(*recorded_len) {
                                vm_try!(Err(PyError::Runtime((*msg).to_string())));
                            }
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
                        Some(IterState::BigRange(st)) => {
                            let BigRangeState { cur, stop, step } = &mut **st;
                            let exhausted = if step.sign() == pyrust_core::PyBigIntSign::Plus {
                                *cur >= *stop
                            } else {
                                *cur <= *stop
                            };
                            if exhausted {
                                pc = jump_pc!(*offset);
                            } else {
                                let v = value_from_bigint(cur.clone());
                                *cur += &*step;
                                regs[*dst as usize] = v;
                            }
                        }
                        Some(IterState::UserDefined(iter_obj)) => {
                            // Generator trampoline (#2253): a plain, handler-free
                            // generator suspended at a `Yield` (or fresh) is driven
                            // within this dispatch loop instead of re-entering
                            // `run_bytecode_inner` per element.  Detected here (where
                            // the iterator is in hand); the switch-in happens after
                            // this match closes, since `iters` is borrowed by it.
                            // Falls through to the per-step dispatch for everything
                            // else: generators with try/except/finally, suspended in
                            // `yield from`, the special iterator adapters (map/zip/…),
                            // PyInstance `__next__`, and built-in objects.
                            if let ValueKind::Generator(state_rc) = iter_obj.kind() {
                                // `try_borrow` rather than `borrow`: a generator
                                // already executing up the stack holds its cell
                                // borrowed (native resume) — `borrow` would abort.
                                match state_rc.try_borrow() {
                                    Ok(b) => {
                                        if let Some(g) =
                                            b.downcast_ref::<GeneratorFrame>()
                                        {
                                            let drivable = !g.done
                                                && !g.code.has_exc_handlers
                                                && g.handled_exc_slice.is_empty()
                                                && g.active_exception.is_none()
                                                && g.exc_saved_active_slice.is_empty()
                                                && !matches!(
                                                    g.code.insns.get(g.pc),
                                                    Some(Insn::YieldFrom { .. })
                                                );
                                            if drivable {
                                                drop(b);
                                                gen_to_drive = Some(Rc::clone(state_rc));
                                            }
                                        } else if b.is::<GenDriving>() {
                                            // Checked out by a gen-drive frame up
                                            // the stack ⇒ already executing.
                                            gen_executing = true;
                                        }
                                    }
                                    // Borrowed elsewhere ⇒ executing via a native
                                    // resume in an enclosing frame.
                                    Err(_) => gen_executing = true,
                                }
                            }
                            // Call __next__() on the iterator object; stop on StopIteration.
                            let iter_val: &Value = iter_obj;
                            let next_result: Option<Result<Value>> =
                                if gen_to_drive.is_some() {
                                    // Driven below after the match closes.
                                    None
                                } else if gen_executing {
                                    // CPython: ValueError("generator already executing").
                                    Some(Err(pyrust_core::py_err!(
                                        "ValueError",
                                        "generator already executing".to_string()
                                    )))
                                } else if let ValueKind::Generator(state_rc) = iter_val.kind() {
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
                                        let is_chain_from_iterable = state_rc
                                            .borrow()
                                            .downcast_ref::<ChainFromIterableIter>()
                                            .is_some();
                                        if is_chain_from_iterable {
                                            Some(match self.step_chain_from_iterable(&state_rc) {
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
                                        let is_bigrange_iter = state_rc
                                            .borrow()
                                            .downcast_ref::<BigRangeIter>()
                                            .is_some();
                                        if is_bigrange_iter {
                                            Some(match self.step_bigrange_iter(&state_rc) {
                                                Ok(Some(v)) => Ok(v),
                                                Ok(None) => Err(pyrust_core::py_err!("StopIteration", String::new())),
                                                Err(e) => Err(e),
                                            })
                                        } else {
                                        let mut borrow = state_rc.borrow_mut();
                                        if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                                            // Built-in iterator created by iter().
                                            // Inlined fast path: the common
                                            // unguarded iterator pays only the
                                            // `guard_check` no-op branch, keeping
                                            // the per-step cost identical to the
                                            // pre-guard code.
                                            Some(if native.pos >= native.items.len()
                                                && native
                                                    .guard
                                                    .as_ref()
                                                    .is_some_and(|g| g.exhaust_first)
                                            {
                                                Err(pyrust_core::py_err!("StopIteration", String::new()))
                                            } else { match native.guard_check() {
                                                Err(e) => Err(e),
                                                Ok(()) if native.pos >= native.items.len() => {
                                                    Err(pyrust_core::py_err!("StopIteration", String::new()))
                                                }
                                                Ok(()) => {
                                                    let item = native.items[native.pos].clone();
                                                    native.pos += 1;
                                                    Ok(item)
                                                }
                                            } })
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
                                        }   // closes is_bigrange_iter else
                                        }   // closes is_zip_iter else
                                        }   // closes is_enumerate_iter else
                                        }   // closes is_chain_from_iterable else
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
                                    method_val.map(|method_val| invoke_class_method(
                                            self,
                                            method_val,
                                            Value::py_instance(inst_rc),
                                            &[],
                                        ))
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
                                // `gen_to_drive` set ⇒ next_result is the skip
                                // sentinel; the switch-in happens after the match.
                                None if gen_to_drive.is_some() => {}
                                None => {
                                    vm_try!(Err(pyrust_core::type_err!("iter() returned non-iterator of type '{}'",
                                            value_type_name_str(iter_val),)));
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
                            current_fn_id: &mut current_fn_id,
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
                Insn::ForCountReg(var, cmp_op, stop_reg, step_idx, offset) => {
                    let step = pool_get!(code.consts, *step_idx, "const")
                        .as_int()
                        .expect("ForCountReg step must be Int");
                    let pair: Option<(i64, i64)> = regs[*var as usize]
                        .as_int()
                        .zip(regs[*stop_reg as usize].as_int());
                    if let Some((cur, stop)) = pair {
                        for_count_step!(regs, *var, cur, stop, step, cmp_op, pc, *offset);
                    } else if let (Some(cur), Some(stop)) = (
                        value_to_bigint(&regs[*var as usize]),
                        value_to_bigint(&regs[*stop_reg as usize]),
                    ) {
                        // BigInt counter/stop (#2118): promote the loop arithmetic.
                        for_count_step_big!(regs, *var, cur, stop, step, cmp_op, pc, *offset);
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
                    } else if let Some(cur) = value_to_bigint(&regs[*var as usize]) {
                        // BigInt counter (#2118): promote the loop arithmetic.
                        for_count_step_big!(regs, *var, cur, PyBigInt::from(stop), step, cmp_op, pc, *offset);
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
                    } else if let Some(cur) = value_to_bigint(&regs[*var as usize]) {
                        // BigInt counter (#2118): promote the loop arithmetic.
                        for_count_step_big!(regs, *var, cur, PyBigInt::from(*stop), *step, cmp_op, pc, *offset);
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
                Insn::MakeClassMeta(dst, proto_idx, bases_base, bases_n, name_idx, kwarg_base, kwarg_n, meta_reg) => {
                    let r = self.exec_make_class_meta(
                        code, &regs, num_locals, *proto_idx, *bases_base, *bases_n, *name_idx,
                        *kwarg_base, *kwarg_n, *meta_reg,
                    );
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
                    // The TypeVar is created unbounded; any bound/constraint is
                    // populated lazily via SetAttr once every type parameter is in
                    // scope (PEP 695 lazy evaluation, see emit_typevar_bound).
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
                    if let Some(order) = self.class_store_order.last_mut()
                        && !order.contains(slot) {
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

    /// Cold fallback for `Insn::LoadCell` when the env chain has no binding for
    /// `name` (a `del`-ed or never-assigned cell var; issue #2339).  Reproduces
    /// the exact tail of `Insn::LoadGlobal`'s slow path — module globals dict,
    /// then the script-frame register mirror, then the restricted/builtin
    /// resolver — so the error a `LoadCell` raises on a missing cell is
    /// byte-identical to what the pre-#2339 `LoadGlobal` path produced.  No cache
    /// is written: a cell value is never cached under `global_env_version`.
    #[cold]
    #[inline(never)]
    fn resolve_cell_miss(&mut self, name: &str) -> Result<Value> {
        if let Some(v) = self
            .module_globals_dict
            .dict_with(|d| d.get(&StrKey(name)).cloned())
            .flatten()
        {
            return Ok(v);
        }
        if let Some(v) = self
            .vm_frame_views
            .iter()
            .find(|v| v.kind == FrameKind::Script)
            .and_then(|script_view| {
                let slot = *script_view.local_index.get(name)? as usize;
                if slot >= script_view.regs_len {
                    return None;
                }
                // SAFETY: identical to the LoadGlobal script-frame fallback —
                // `regs_ptr` points to the live script frame's register file,
                // accessed via `RegSlice` (no `noalias`), and `slot < regs_len`
                // is checked above.  We read a shared `&Value` for the clone only.
                let v = unsafe { script_view.regs_ptr.add(slot).as_ref() };
                if v.is_unset() { None } else { Some(v.clone()) }
            })
        {
            return Ok(v);
        }
        let cur_ver = self.global_env_version.get();
        let (v, _) = resolve_global_via_builtins(&self.module_globals_dict, name, cur_ver)?;
        Ok(v)
    }

    /// Resolve an `await` target to the iterator that drives it (issue #1039).
    ///
    /// Mirrors CPython's `GET_AWAITABLE`:
    /// - a coroutine (an `async def` frame) is its own awaitable → returned as-is;
    /// - an object defining `__await__` → returns `obj.__await__()`;
    /// - anything else → `TypeError: object … can't be used in 'await' expression`.
    ///
    /// The resolved value is then driven by the following `YieldFrom`.
    pub(crate) fn get_awaitable(&mut self, awaited: &Value) -> Result<Value> {
        // A coroutine drives itself: `await coro` resolves to the coroutine and
        // the following `YieldFrom` steps it.
        if let ValueKind::Generator(state_rc) = awaited.kind() {
            // Borrow the cell to inspect the coroutine's state.  A busy cell
            // (`try_borrow` → `Err`) means the coroutine is *currently* being
            // awaited / executing — CPython raises `RuntimeError("coroutine is
            // being awaited already")` (the self-await re-entrant case surfaces
            // later as `ValueError("coroutine already executing")` from the
            // resume path; both are coroutine-only).  A done coroutine
            // (`frame.done`) has already run to completion: re-awaiting it must
            // raise `RuntimeError("cannot reuse already awaited coroutine")`
            // rather than silently yielding its stale return value (issue #2282).
            match state_rc.try_borrow() {
                Ok(b) => {
                    // The `asend`/`__anext__` awaitable of an async generator
                    // (#2280) is itself a self-driving awaitable: pass it through
                    // for `YieldFrom` to step.
                    if b.downcast_ref::<AsyncGenASend>().is_some() {
                        return Ok(awaited.clone());
                    }
                    if let Some(frame) = b.downcast_ref::<GeneratorFrame>()
                        && frame.is_coroutine
                    {
                        // An async generator's frame is coroutine-tagged but is
                        // NOT directly awaitable (`await agen()` is a TypeError in
                        // CPython); it is consumed via `async for` / `asend`.
                        if frame.is_async_generator() {
                            return Err(pyrust_core::type_err!(
                                "object async_generator can't be used in 'await' expression"
                            ));
                        }
                        if frame.done {
                            return Err(pyrust_core::runtime_err!(
                                "cannot reuse already awaited coroutine"
                            ));
                        }
                        return Ok(awaited.clone());
                    }
                }
                Err(_) => {
                    // Cell checked out ⇒ the frame is currently executing on
                    // this native stack, so this `await` is re-entrant (a
                    // coroutine awaiting itself, directly or via a helper
                    // chain).  Raise CPython's coroutine wording eagerly
                    // (issue #2285): the generic resume path can only see an
                    // opaque busy cell and would report the *generator*
                    // wording.  The kind is unreadable while the cell is busy,
                    // but the only realistic busy frame reachable from `await`
                    // is a coroutine — a busy sync/async generator here would
                    // require `asyncio.run` nested inside its own body, which
                    // diverges from CPython before this point anyway.
                    return Err(pyrust_core::value_err!("coroutine already executing"));
                }
            }
        }
        // An object with `__await__` (e.g. a future-like awaitable) — call it
        // and drive the returned iterator.
        if let ValueKind::PyInstance(inst_rc) = awaited.kind() {
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(await_method) = lookup_class_attr(&class, "__await__") {
                return invoke_class_method(
                    self,
                    await_method,
                    awaited.clone(),
                    &[],
                );
            }
        }
        Err(pyrust_core::type_err!(
            "object {} can't be used in 'await' expression",
            value_type_name_str(awaited)
        ))
    }

    /// Step a coroutine exactly once for the real event loop (issue #2281).
    ///
    /// Sends `sent_value` (or injects `inject_exc`) into the coroutine and runs
    /// until its next suspension or completion.  Unlike
    /// `drive_coroutine_to_completion`, this does NOT loop — the yielded value
    /// (the awaitable that bubbled up the `YieldFrom` chain, typically an
    /// `asyncio.Future`) is returned to the caller so the Python-level loop can
    /// suspend the task on it.
    ///
    /// Returns:
    /// - `Ok(CoroStep::Yielded(v))` — coroutine suspended, yielding `v`;
    /// - `Ok(CoroStep::Returned(v))` — coroutine completed, returning `v`;
    /// - `Err(e)` — coroutine raised `e` (or was already done / executing).
    pub(crate) fn coro_step(
        &mut self,
        coro: &Value,
        sent_value: Value,
        inject_exc: Option<PyError>,
    ) -> Result<CoroStep> {
        let state_rc = match coro.kind() {
            ValueKind::Generator(s) => Rc::clone(s),
            _ => {
                return Err(pyrust_core::type_err!(
                    "a coroutine was expected, got {}",
                    value_type_name_str(coro)
                ));
            }
        };
        let mut borrow = state_rc
            .try_borrow_mut()
            .map_err(|_| pyrust_core::value_err!("coroutine already executing"))?;
        let frame = borrow
            .downcast_mut::<GeneratorFrame>()
            .ok_or_else(|| pyrust_core::type_err!("a coroutine was expected"))?;
        match self.resume_generator_with_exc(frame, inject_exc, sent_value) {
            Ok(v) => Ok(CoroStep::Yielded(v)),
            Err(e) if is_stop_iteration_error(&e) => {
                let result = frame.last_return_value.clone().unwrap_or_else(Value::none);
                Ok(CoroStep::Returned(result))
            }
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

                // Async-generator `asend`/`__anext__` awaitable (#2280): a
                // dedicated driver that resumes the underlying async generator
                // and distinguishes a bare `yield v` (→ item delivered) from an
                // inner-`await` suspension (→ propagate the scheduling point).
                let is_asend = state_rc
                    .try_borrow()
                    .map(|b| b.downcast_ref::<AsyncGenASend>().is_some())
                    .unwrap_or(false);
                if is_asend {
                    return self.step_async_gen_asend(&state_rc, sent_val);
                }

                // Check for GetItemIter (lazy __getitem__ iterator).
                // `try_borrow`: when the sub-iterator is itself already executing
                // (a `yield from` / `await` self-cycle), the cell is checked out;
                // fall through to the `try_borrow_mut` below, which raises the
                // proper "already executing" ValueError instead of panicking on a
                // re-borrow (issue #1039 — surfaced by `await`-self; the same
                // latent panic affected `yield from`-self generators).
                let is_getitem = state_rc
                    .try_borrow()
                    .map(|b| b.downcast_ref::<GetItemIter>().is_some())
                    .unwrap_or(false);
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

                // A `GenDriving` placeholder means the generator's frame is
                // checked out by the gen-drive trampoline (#2253) up the stack
                // — it is executing, same as the busy-cell case above
                // (issue #2285).
                if borrow.is::<GenDriving>() {
                    return Err(pyrust_core::value_err!("generator already executing"));
                }

                if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                    // Built-in iterator: no send support, just advance.
                    return match native.advance()? {
                        Some(v) => Ok(v),
                        None => Err(pyrust_core::py_err!("StopIteration", String::new())),
                    };
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
                            if let Some(rv) = frame.last_return_value.clone()
                                && !rv.is_none()
                                    && let Some(cls) =
                                        lookup_name_in_module(&self.env, "StopIteration")
                                            .and_then(|v| match v.kind() {
                                                ValueKind::PyClass(c) => Some(Rc::clone(c)),
                                                _ => None,
                                            })
                                    {
                                        let exc = instantiate_exception(cls, vec![rv]);
                                        return Err(PyError::Raised(exc));
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
                if !sent_val.is_none()
                    && let Some(send_method) = lookup_class_attr(&class, "send") {
                        return invoke_class_method(
                            self,
                            send_method,
                            Value::py_instance(inst_rc),
                            &[ExpandedCallArg { name: None, value: sent_val }],
                        );
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

    /// Advance an async-generator `asend`/`__anext__` awaitable by one step
    /// (#2280).  `asend_rc` is the `AsyncGenASend` state cell; `sent_val` is the
    /// value the surrounding `await` machinery is sending into *this* awaitable
    /// (always `None` for the async-for driver — the value the user `asend`s is
    /// stored in the `AsyncGenASend` itself and delivered into the async
    /// generator on its first step).
    ///
    /// Resumes the underlying async generator once and applies the yield/await
    /// duality:
    /// - bare `yield v` inside the async-gen body → the awaitable *completes*
    ///   with `v`: raise `StopIteration(v)` so the consumer's `YieldFrom`
    ///   captures `v` as the produced item.
    /// - inner `await` suspension (the async-gen body is parked at a `YieldFrom`
    ///   awaiting e.g. `asyncio.sleep(0)`) → `Ok(scheduling_value)`: propagate
    ///   the scheduling point upward so the outer event loop steps it and the
    ///   awaitable is re-driven.
    /// - async-gen returned / exhausted → `StopAsyncIteration` (no value).
    fn step_async_gen_asend(
        &mut self,
        asend_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
        _sent_val: Value,
    ) -> Result<Value> {
        // Take the per-step injection state out of the AsyncGenASend.  Only the
        // *first* step delivers the original `asend(v)` value / `athrow` exc;
        // subsequent re-drives (after an inner-await suspension) send None.
        let (agen_rc, send_value, throw_exc, is_aclose) = {
            let mut b = asend_rc.borrow_mut();
            let asend = b.downcast_mut::<AsyncGenASend>().ok_or_else(|| {
                PyError::Runtime("invalid async-generator asend state".to_string())
            })?;
            let send_value = if asend.started {
                None
            } else {
                asend.send_value.take()
            };
            let throw_exc = if asend.started {
                None
            } else {
                asend.throw_exc.take()
            };
            asend.started = true;
            (
                Rc::clone(&asend.agen),
                send_value,
                throw_exc,
                asend.is_aclose,
            )
        };

        // Resume the async generator's frame once.  Re-entrant stepping (the
        // agen's own body driving another `__anext__`/`asend` on itself) is a
        // RuntimeError in CPython 3.12 — with the `anext():` prefix even for
        // `asend()` (issue #2285).
        let mut borrow = agen_rc.try_borrow_mut().map_err(|_| {
            pyrust_core::runtime_err!("anext(): asynchronous generator is already running")
        })?;
        let frame = borrow.downcast_mut::<GeneratorFrame>().ok_or_else(|| {
            PyError::Runtime("invalid async-generator state".to_string())
        })?;
        if frame.done {
            // `aclose()` on an already-finished async generator is a silent
            // no-op (the awaitable completes with None); `__anext__`/`asend`
            // raise StopAsyncIteration.
            if is_aclose {
                return Err(self.make_stop_iteration_with_value(Value::none()));
            }
            return Err(self.make_stop_async_iteration());
        }
        // CPython: sending a non-None value into a *just-started* async
        // generator (one never resumed, `pc == 0`) via `asend(v)` is a
        // TypeError, raised when the awaitable is first driven (#2280).
        // `athrow`/`aclose` (which carry an injected exception) and
        // `__anext__`/`asend(None)` are exempt.
        if frame.pc == 0
            && throw_exc.is_none()
            && send_value.as_ref().is_some_and(|v| !v.is_none())
        {
            return Err(pyrust_core::type_err!(
                "can't send non-None value to a just-started async generator"
            ));
        }
        let resume = self.resume_generator_with_exc(
            frame,
            throw_exc,
            send_value.unwrap_or_else(Value::none),
        );
        match resume {
            Ok(value) => {
                // Suspended.  Distinguish a bare `yield v` from an inner-`await`
                // suspension by inspecting the instruction the frame is now
                // parked at: a `YieldFrom` means we suspended inside an `await`
                // (await lowers to GetAwaitable + YieldFrom), so propagate the
                // scheduling value upward.  Anything else means we suspended at
                // a bare `Insn::Yield` whose pc was advanced past it, so `value`
                // is a produced item: complete the await with it.
                let parked_at_yield_from = matches!(
                    frame.code.insns.get(frame.pc),
                    Some(crate::bytecode::Insn::YieldFrom { .. })
                );
                if parked_at_yield_from {
                    // Inner-await scheduling point: propagate upward unchanged.
                    Ok(value)
                } else if is_aclose {
                    // `aclose()` injected GeneratorExit and the body yielded a
                    // value instead of exiting — CPython raises RuntimeError.
                    frame.done = true;
                    Err(pyrust_core::runtime_err!(
                        "async generator ignored GeneratorExit"
                    ))
                } else {
                    // Bare `yield value`: the awaitable completes with `value`.
                    Err(self.make_stop_iteration_with_value(value))
                }
            }
            // Async generator ran to completion (fell off the end or `return`).
            // CPython: `__anext__`/`asend` then raise StopAsyncIteration with no
            // value (an async-gen `return v` with non-None v is a SyntaxError,
            // so the return value is always None and is discarded).
            Err(ref e) if is_stop_iteration_error(e) => {
                if is_aclose {
                    // aclose: a clean StopAsyncIteration/return means the close
                    // succeeded — complete the await with None.
                    Err(self.make_stop_iteration_with_value(Value::none()))
                } else {
                    Err(self.make_stop_async_iteration())
                }
            }
            // `aclose()`: the body let the injected GeneratorExit propagate
            // (the normal, well-behaved case) — the close succeeded, so the
            // awaitable completes with None rather than re-raising.
            Err(ref e) if is_aclose && e.class_name_is("GeneratorExit") => {
                Err(self.make_stop_iteration_with_value(Value::none()))
            }
            Err(e) => Err(e),
        }
    }

    /// Build a `StopAsyncIteration` error (async-generator exhaustion, #2280).
    fn make_stop_async_iteration(&self) -> PyError {
        if let Some(cls) = self.exc_classes.get("StopAsyncIteration") {
            PyError::Raised(instantiate_exception(cls, vec![]))
        } else {
            pyrust_core::py_err!("StopAsyncIteration", String::new())
        }
    }

    /// Build a `StopIteration(value)` error carrying a produced async-gen item
    /// (#2280).  The consumer's `YieldFrom` reads `.value` to obtain the item.
    fn make_stop_iteration_with_value(&self, value: Value) -> PyError {
        if let Some(cls) = self.exc_classes.get("StopIteration") {
            PyError::Raised(instantiate_exception(cls, vec![value]))
        } else {
            pyrust_core::py_err!("StopIteration", String::new())
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
                            if let Some(rv) = frame.last_return_value.clone()
                                && !rv.is_none()
                                    && let Some(cls) =
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
                            self.instantiate_named_exception(name.as_ref(), msg)?
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

/// Borrow a register's value without cloning, applying the same unset check as
/// [`vm_read`].  Used by read-only dispatch paths (e.g. `GetSlice`) where the
/// source value is only consumed by reference — cloning the source first is
/// wasteful and, for a `tuple`, pathological: `Value::clone` deep-copies the
/// whole backing `Vec`, so slicing an N-element tuple cloned the entire source
/// in O(N) before the slice even ran (#2114).
#[inline]
fn vm_read_ref(
    regs: &[Value],
    reg: crate::bytecode::Reg,
    num_locals: crate::bytecode::Reg,
) -> crate::interpreter::Result<&Value> {
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
    Ok(v)
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
                if let Some(v) = borrow.attrs.get("value")
                    && !v.is_none() {
                        return Some(v.clone());
                    }
                // Try args[0].
                if let Some(args_val) = borrow.attrs.get("args")
                    && let Some(args) = args_val.as_tuple().or_else(|| args_val.as_list())
                        && let Some(first) = args.first()
                            && !first.is_none() {
                                return Some(first.clone());
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
    if let Some(cause) = cause_val
        && let Some(rt_cls) = lookup_name_in_module(env, "RuntimeError").and_then(|v| match v.kind() {
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
                    .insert("__cause__", cause);
                inst.borrow_mut()
                    .attrs
                    .insert("__context__", context);
                inst.borrow_mut()
                    .attrs
                    .insert("__suppress_context__", Value::bool_(true));
            }
            return PyError::Raised(rt_err);
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
        Rc::new(RefCell::new(PyClass::new(
            "TypeAliasType",
            "TypeAliasType",
            None,
            attrs,
        )))
    };

    /// Class singleton for `TypeVar` objects created by generic type params.
    static TYPEVAR_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function("builtins.TypeVar.__repr__"),
        );
        Rc::new(RefCell::new(PyClass::new("TypeVar", "TypeVar", None, attrs)))
    };
}

/// Construct an (initially unbounded) `TypeVar` `PyInstance` with `__name__`,
/// `__constraints__`, and `__bound__` attributes, matching the observable
/// surface of CPython's `typing.TypeVar` as created by PEP 695 type parameter
/// syntax.  `__bound__` starts as `None` and `__constraints__` as `()`; a
/// bounded/constrained parameter's clause is evaluated lazily (after every type
/// parameter is in scope) and written back via `SetTypeVarAttr` — see
/// `Compiler::emit_typevar_bound`.
pub(crate) fn make_typevar_instance(name: String) -> Value {
    TYPEVAR_CLASS.with(|cls| {
        let mut attrs = InstanceAttrs::new();
        attrs.insert("__name__", Value::string(name));
        attrs.insert("__constraints__", Value::tuple(vec![]));
        attrs.insert("__bound__", Value::none());
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(cls),
            attrs,
        })))
    })
}

/// True if `class` is the PEP 695 `TypeVar` singleton.  Used by the attribute
/// assignment / deletion slow paths to enforce CPython's read-only getset
/// descriptors on TypeVar objects.
pub(crate) fn is_typevar_class(class: &Rc<RefCell<PyClass>>) -> bool {
    TYPEVAR_CLASS.with(|cls| Rc::ptr_eq(class, cls))
}

/// Classify a would-be write/delete of `name` on a `TypeVar` instance against
/// CPython 3.12's read-only getset descriptors.  Returns the exact
/// `AttributeError` message CPython raises, or `None` if the name is not a
/// protected descriptor (arbitrary attributes are writable, matching CPython).
///
///   * `__bound__` / `__constraints__` raise
///     `attribute '<name>' of 'typing.TypeVar' objects is not writable`
///   * `__name__` / `__covariant__` / `__contravariant__` /
///     `__infer_variance__` raise the generic `readonly attribute`.
pub(crate) fn typevar_readonly_attr_error(name: &str) -> Option<String> {
    match name {
        "__bound__" | "__constraints__" => Some(format!(
            "attribute '{name}' of 'typing.TypeVar' objects is not writable"
        )),
        "__name__" | "__covariant__" | "__contravariant__" | "__infer_variance__" => {
            Some("readonly attribute".to_string())
        }
        _ => None,
    }
}

/// Construct a `TypeAliasType` `PyInstance` with `__name__`, `__value__`, and
/// `__type_params__` attributes, matching the observable behaviour of CPython's
/// `typing.TypeAliasType`.
pub(crate) fn make_type_alias_instance(name: String, value: Value, type_params: Value) -> Value {
    TYPE_ALIAS_CLASS.with(|cls| {
        let mut attrs = InstanceAttrs::new();
        attrs.insert("__name__", Value::string(name));
        attrs.insert("__value__", value);
        attrs.insert("__type_params__", type_params);
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
        use crate::bytecode::{AttrCacheEntry, BinOpCacheEntry, KwCallCacheEntry};
        let n = insns.len();
        FnCode {
            insns,
            lineno_table: vec![0u32; n],
            col_table: vec![(0, 0); n],
            first_lineno: 0,
            consts: vec![],
            names: vec![],
            num_regs: 0,
            num_iters: 0,
            num_locals: 0,
            fn_protos: vec![],
            cell_vars: smallvec::smallvec![],
            is_generator: false,
            is_coroutine: false,
            is_class_method: false,
            is_inlined_comp: false,
            attr_cache: std::cell::RefCell::new(vec![AttrCacheEntry::Empty; n]),
            global_cache: std::cell::RefCell::new(Vec::new()),
            binop_cache: std::cell::RefCell::new(vec![BinOpCacheEntry::Empty; n]),
            kwcall_cache: std::cell::RefCell::new(vec![KwCallCacheEntry::Empty; n]),
            // Empty: these hand-built test fixtures run unoptimized, so the VM
            // uses the dynamic SetupExcept/PopExcept handler stack.
            exc_table: Vec::new(),
            has_exc_handlers: false,
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
        code.col_table.extend([(0u32, 0u32); 3]);
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
