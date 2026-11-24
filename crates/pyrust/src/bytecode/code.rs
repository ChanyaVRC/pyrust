//! Compiled code objects and their source/traceback provenance.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use pyrust_core::Symbol;
use smallvec::SmallVec;

use crate::value::Value;

use super::{
    AttrCacheEntry, BinOpCacheEntry, CellSlotMeta, FnProto, GlobalCacheEntry, Insn,
    KwCallCacheEntry, SiteCache, SparsePcCache,
};

/// Maximum register slots allowed in one executable frame.
///
/// The compiler and every optimizer pass that adds scratch registers share this
/// limit so optimization cannot bypass the allocation guard applied before the
/// initial [`FnCode`] is built.
pub(crate) const MAX_FRAME_REGS: u32 = 1 << 20;

#[derive(Debug, Clone)]
pub struct FnCode {
    pub(crate) insns: Vec<Insn>,
    /// Source-file path this code object was compiled from — the code object's
    /// `co_filename`.  Threaded from the compile entry point through every nested
    /// `def`/`class`/`lambda`, so an imported module's functions report their own
    /// file (not the running script's) in tracebacks and `__code__.co_filename`
    /// (issue #2438).  `<unknown>` when no path was supplied (e.g. REPL / eval).
    pub(crate) filename: std::sync::Arc<str>,
    /// Compile-time qualified name for this code object — `co_qualname`
    /// (PEP 3155 / CPython 3.11+).  Fixed at compile time and unaffected by
    /// runtime `__qualname__` reassignment on a function, generator, coroutine,
    /// or async generator (issue #2981).
    pub(crate) qualname: std::sync::Arc<str>,
    /// 1-based source line number for each instruction, parallel to `insns`.
    /// A value of 0 means "unknown / same as the previous instruction".  Set by
    /// the compiler when per-statement line information is available (i.e. when
    /// the script was compiled with line tracking enabled).  Used by the VM to
    /// update the current-line counter when building tracebacks.
    pub(crate) lineno_table: Vec<u32>,
    /// PEP 657 caret anchor for each instruction, parallel to `insns`
    /// (issues #2426 / #2411).  Each entry is
    /// `(full_start, prim_start, prim_end, full_end)` (see
    /// [`crate::ast::CaretSpan`]); `(0, 0, 0, 0)` means "no anchor" — the
    /// formatter then omits the caret row.  Populated by the compiler only for
    /// the highest-value expression forms (bare-name `Var` loads, calls, binary
    /// ops, subscripts); all other entries are `(0, 0, 0, 0)`.  Read **only on
    /// the error path** (when an exception escapes a frame), so it never touches
    /// the per-instruction hot path.
    ///
    /// Offsets are 0-based char columns within the raising instruction's source
    /// line (the `lineno_table` line), measured against the original line text.
    pub(crate) col_table: Vec<crate::ast::CaretSpan>,
    /// 1-based source line of the `def`/`lambda` that produced this code object
    /// — the function's `co_firstlineno`.  `0` for the module-level `<module>`
    /// code or when no line information is available.  Set by the compiler from
    /// the `def` keyword's line (NOT the first body statement, which may be one
    /// or more lines below for a multi-line signature; issue #2185).
    pub(crate) first_lineno: u32,
    /// Constant pool (literals used in the function body)
    pub(crate) consts: Vec<Value>,
    /// Name pool (interned global variable names and attribute names)
    pub(crate) names: Vec<Symbol>,
    /// Number of registers needed (locals + max temporaries)
    pub(crate) num_regs: u32,
    /// Number of iterator slots needed
    pub(crate) num_iters: u8,
    /// Number of local variable slots (registers 0..num_locals are locals; the rest are temps)
    pub(crate) num_locals: u32,
    /// Nested function / class body prototypes
    pub(crate) fn_protos: Vec<FnProto>,
    /// Compile-time cell slot table (issues #452 / #2339 / #2348).
    /// Slots `0..own_cell_count` are this scope's own cell variables (`co_cellvars`,
    /// sorted by name); slots `own_cell_count..` are captured free variables
    /// (`co_freevars`, sorted by name, including `nonlocal` targets and transitive
    /// pass-through captures).
    pub(crate) cell_slots: SmallVec<[CellSlotMeta; 4]>,
    /// Number of leading slots in `cell_slots` owned by this frame (`co_cellvars`).
    pub(crate) own_cell_count: u16,
    /// Memoized result of [`FnCode::free_var_candidates`] — the sorted, distinct
    /// non-cell names this body may read through its defining namespace chain
    /// (e.g. `__class__` in class methods or PEP 695 type parameters). Filled on
    /// first use, never on the dispatch path.
    pub(crate) free_var_candidates: std::cell::OnceCell<Vec<Symbol>>,
    /// True if this function body contains at least one `Yield` instruction.
    /// The VM creates a generator object instead of executing immediately.
    pub(crate) is_generator: bool,
    /// True if this function was declared with `async def` (issue #1039).
    /// Such a function, when called, produces a *coroutine* object rather than
    /// executing immediately.  The coroutine reuses the generator suspend/resume
    /// machinery (`GeneratorFrame`) but is tagged so that `type(coro).__name__`
    /// is `"coroutine"` and it is not iterable with `for`.  An `async def` body
    /// is always a suspendable frame even when it contains no `await` (e.g.
    /// `async def f(): return 1`), so `is_coroutine` implies the
    /// generator-frame creation path independently of `is_generator`.
    pub(crate) is_coroutine: bool,
    /// True when this function was compiled as a direct method inside a class
    /// body (i.e., the enclosing `Compiler` had `is_class_body = true`).
    /// Zero-argument `super()` is valid only in such functions — not in plain
    /// functions or in functions nested inside methods.  Used by
    /// `resolve_zero_arg_super` to identify the correct enclosing frame.
    pub(crate) is_class_method: bool,
    /// True when this code object is a Python function/lambda/comprehension
    /// body (not a module script, `exec`/`eval` frame, or class body).
    /// Function fastlocals (`0..num_locals`) cannot be externally mutated by
    /// module `globals()` or class `locals()` namespace mirrors (issue #457).
    pub(crate) is_function_scope: bool,
    /// True when this code object is the implicit body of a list / set / dict
    /// comprehension — a scope CPython 3.12 inlines into the enclosing frame
    /// (PEP 709).  pyrust runs it as a separate frame, but for error parity an
    /// unbound read of an enclosing local must raise `UnboundLocalError` (as a
    /// local of the enclosing frame) rather than the free-variable `NameError`
    /// a real closure / generator expression produces (issue #2340).
    pub(crate) is_inlined_comp: bool,
    /// For an inlined list/set/dict comprehension (`is_inlined_comp`), the set of
    /// *local* variable names of the comprehension's immediately-enclosing real
    /// function — the frame CPython 3.12 inlines the comp into (PEP 709).
    ///
    /// An unbound read inside the comp surfaces as `UnboundLocalError` only when
    /// the name is a local of that inlining-target frame; a name that is a *free*
    /// variable of the enclosing function (owned by a grandparent scope) must
    /// surface as the free-variable `NameError` instead (issue #2457).  `None`
    /// for every non-comp code object (and for a comp not directly enclosed by a
    /// function, where there are no enclosing locals to distinguish).
    pub(crate) comp_enclosing_locals: Option<Rc<HashSet<Symbol>>>,
    /// Lazily-allocated, site-indexed inline cache for `GetAttr`, `SetAttr`,
    /// `CallMethod`, and `CallMethodKw`.
    ///
    /// Allocates 0 heap bytes until a matching site actually populates an entry,
    /// then packs only matching sites into an O(1) array-indexed table.
    ///
    /// `RefCell` provides interior mutability so the cache can be updated
    /// through a shared `Rc<FnCode>` during dispatch without requiring an
    /// exclusive borrow of the enclosing `FnCode`.
    pub(crate) attr_cache: RefCell<SiteCache<AttrCacheEntry>>,
    /// Per-name inline cache for the final `LoadGlobal` resolution.
    ///
    /// Lazily allocated on first cache fill (0 heap bytes for unexecuted
    /// functions), indexed by `name_idx`, and shared across all invocations of
    /// this function. Environment and canonical-builtins resolutions are
    /// mutually exclusive variants in the same slot, so dispatch probes one
    /// `RefCell`. Both variants validate the root namespace identity;
    /// environment entries additionally validate its value generation, while
    /// builtin-module entries validate its structure generation and the
    /// provider's shared mutation state. Custom `__builtins__` modules/dicts
    /// are never cached.
    pub(crate) global_cache: RefCell<Vec<GlobalCacheEntry>>,
    /// Precomputed per-name Bloom masks used to invalidate only value or
    /// canonical-fallback caches that may exist in this root namespace.
    pub(crate) global_cache_interest_masks: Vec<u64>,
    /// Adaptive inline cache for binary operations (PEP 659 style).
    ///
    /// Lazily allocated on first non-int `BinOp` observation (0 heap bytes for
    /// unexecuted or int-only functions), then indexed directly by `pc`.
    /// Only entries at `BinOp` positions are ever advanced beyond `Empty`;
    /// `BinOpInPlace`, `BinOpConst`, and `BinOpImm` use only the unconditional
    /// int-int fast path and leave their cache slots `Empty`.
    ///
    /// State machine per entry:
    ///   `Empty` → first observation → `Counting { tag, count: 1 }`
    ///   `Counting { tag, count }` + same tag → `count + 1`; if `count + 1 ==
    ///     BINOP_SPEC_THRESHOLD` → `Specialized(tag)`.
    ///   `Counting` + different tag → `Megamorphic`.
    ///   `Specialized(tag)` + same tag → try fast path; mismatch → `Megamorphic`.
    ///   `Megamorphic` → permanently bypass cache, call `eval_binary` directly.
    pub(crate) binop_cache: RefCell<Vec<BinOpCacheEntry>>,
    /// Sparse per-call-site inline cache for `Insn::CallKw`, `Insn::CallMethodKw`,
    /// `Insn::CallEx`, and `Insn::CallExArgs` (issue #2382).
    ///
    /// Allocates 0 heap bytes until a keyword/splat call site populates an
    /// entry; stores only populated `(pc, KwCallCacheEntry)` pairs.  Records
    /// the keyword-name → parameter-slot mapping for a monomorphic call site so
    /// the binder skips the per-call linear name scan; see [`KwCallCacheEntry`].
    pub(crate) kwcall_cache: RefCell<SparsePcCache<KwCallCacheEntry>>,
    /// Sparse per-instruction cache for constant f-string format specs (issues
    /// #2357 / #2372).
    ///
    /// Allocates 0 heap bytes until a `FormatValueSpec` site populates an
    /// entry; stores only populated `(pc, FmtSpecCacheEntry)` pairs.  A
    /// constant spec (`f"{x:.2f}"`) is parsed once and cached here; a dynamic
    /// spec (`f"{x:{w}f}"`) keeps missing (its spec string is freshly allocated
    /// each iteration) and is never cached.  Pc-keyed, so it is immune to the
    /// const remapping `pass_compact_consts` performs.  See
    /// [`FmtSpecCacheEntry`].
    pub(crate) fmt_spec_cache: RefCell<SparsePcCache<crate::interpreter::FmtSpecCacheEntry>>,
    /// Lazily-allocated, site-indexed inline cache for plain built-in callees
    /// (`len`, `ord`, `abs`, …) at `Insn::Call` / `Insn::CallMemo` sites.
    /// Allocates 0 heap bytes until a cacheable builtin/class call populates an
    /// entry, then packs only call sites into an O(1) array-indexed table; a
    /// hit dispatches straight through the cached registry `fn` pointer,
    /// skipping the `call_function_expanded` cascade and registry binary
    /// search. See [`crate::interpreter::CallBuiltinCacheEntry`].
    pub(crate) call_builtin_cache: RefCell<SiteCache<crate::interpreter::CallBuiltinCacheEntry>>,
    /// Weakly cached `types.CodeType` object for this compiled body so
    /// repeated `f.__code__`, `g.gi_code`, and `frame.f_code` reads share one
    /// Python object identity without introducing strong ownership cycles.
    pub(crate) cached_code_object: RefCell<Option<pyrust_core::WeakValueCache>>,
    /// Zero-cost exception table (CPython 3.11 model).  Parallel to `insns`:
    /// `exc_table[pc]` is the absolute target PC of the innermost `try` handler
    /// active when an exception is raised at `pc`, or [`EXC_NO_HANDLER`] for
    /// none.  Populated by the optimizer's `build_exc_table` pass after the
    /// `SetupExcept`/`PopExcept` block-setup instructions have been stripped, so
    /// entering/leaving a `try` is free and only the raise path pays an O(1)
    /// lookup.  Empty when bytecode was built without the optimizer (the VM then
    /// falls back to the dynamic `SetupExcept`/`PopExcept` handler stack).
    pub(crate) exc_table: Vec<u32>,
    /// `true` when `exc_table` contains at least one real handler target (i.e.
    /// the function body has a reachable `try`/`except`/`finally`/`with`).
    /// Precomputed by the optimizer so the self-recursion trampoline (#2234)
    /// can gate on it with an O(1) check: a handler-free function can be
    /// trampolined because an unhandled raise in a trampolined frame correctly
    /// propagates straight out (no frame on the stack could catch it).
    /// Conservatively `true` for un-optimized bytecode (no trampolining).
    pub(crate) has_exc_handlers: bool,
}

impl FnCode {
    /// Own cell slots defined by this function (`0..own_cell_count`, sorted).
    #[inline]
    pub(crate) fn own_cell_slots(&self) -> &[CellSlotMeta] {
        &self.cell_slots[..self.own_cell_count as usize]
    }

    /// Captured free-variable slots (`own_cell_count..`, sorted).
    #[inline]
    pub(crate) fn free_var_slots(&self) -> &[CellSlotMeta] {
        &self.cell_slots[self.own_cell_count as usize..]
    }

    /// True if this function allocates fresh own cells on call entry.
    #[inline]
    pub(crate) fn has_own_cells(&self) -> bool {
        self.own_cell_count > 0
    }

    /// The distinct non-cell names this body reads through the defining
    /// namespace chain, sorted (used for non-cell enclosing namespace captures
    /// such as `__class__` and PEP 695 type parameters).
    pub(crate) fn free_var_candidates(&self) -> &[Symbol] {
        self.free_var_candidates.get_or_init(|| {
            let mut seen: HashSet<Symbol> = HashSet::new();
            let mut names: Vec<Symbol> = Vec::new();
            collect_env_read_names(self, &mut seen, &mut names);
            names.sort_unstable_by_key(|sym| sym.as_str());
            names
        })
    }
}

fn collect_env_read_names(fncode: &FnCode, seen: &mut HashSet<Symbol>, out: &mut Vec<Symbol>) {
    for insn in &fncode.insns {
        let name_idx = match insn {
            Insn::LoadGlobal(_, idx) => *idx,
            Insn::LoadClassName(_, local, idx) if *local == crate::bytecode::NO_CLASS_LOCAL => *idx,
            _ => continue,
        };
        if let Some(&name) = fncode.names.get(name_idx as usize)
            && seen.insert(name)
        {
            out.push(name);
        }
    }
    for proto in &fncode.fn_protos {
        collect_env_read_names(&proto.code, seen, out);
    }
}

pub(crate) use crate::optimizer::EXC_NO_HANDLER;
