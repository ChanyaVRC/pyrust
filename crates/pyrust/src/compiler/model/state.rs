// ─── Compiler struct ──────────────────────────────────────────────────────────

struct LoopCtx {
    /// Instruction indices of `Jump(0)` placeholders for `break` statements;
    /// patched to jump past the loop once the loop end is known.
    /// `SmallVec<[usize; 2]>` avoids heap allocation for the common case of
    /// zero or one `break` per loop.
    break_patches: SmallVec<[usize; 2]>,
    /// None when the continue target is not yet known (e.g. counter-range loop
    /// where the increment comes after the body).  Patched before the increment.
    continue_target: Option<usize>,
    /// Indices of Jump(0) instructions emitted for `continue` when continue_target
    /// was None; fixed up once continue_target is established.
    /// `SmallVec<[usize; 2]>` avoids heap allocation for the common case of
    /// zero or one `continue` before the target is known.
    continue_patches: SmallVec<[usize; 2]>,
    /// Depth of `Compiler::except_cleanups` at the point this loop was entered.
    /// `break` and `continue` must emit cleanups for entries above this depth.
    cleanup_depth: usize,
}

/// Describes the cleanup that must be emitted before an early exit
/// (`break`, `continue`, or `return`) that crosses a guarded block boundary.
// The shared `Body` postfix (`TryBody`/`ExceptBody`/`WithBody`) names the kind
// of guarded body each entry tracks; keeping it is clearer than the lint's
// suggested rename.
#[allow(clippy::enum_variant_names)]
#[derive(Clone)]
enum EarlyExitCleanup {
    /// Inside a try-body that has an active `SetupExcept` on the handler stack.
    /// Early exit must emit `PopExcept` then optionally inline the finally block.
    TryBody { finally_stmts: Option<Vec<Stmt>> },
    /// Inside an except-handler body where `active_exception` is set.
    /// Early exit must emit the PEP 3110 as-var delete (if any), then
    /// `EndExcept`.  An enclosing `finally` is not part of this entry: its
    /// handler stays live across the handler body and is tracked by a
    /// `TryBody` entry beneath this one (#3357).
    ExceptBody {
        /// PEP 3110: how to delete the `except E as var` binding on early exit.
        /// `Local` clears the register and, at module scope, its live globals
        /// dictionary entry.
        /// `Name(name_idx)` \u2192 `DeleteName(name_idx)` (var lives in env).
        /// `None` \u2192 no `as VAR` clause.
        as_var_delete: Option<ExceptAsVarDel>,
    },
    /// Inside a `with`/`async with` body whose `SetupExcept` is live on the
    /// handler stack.  A `break`/`continue`/`return` that leaves the body must
    /// emit `PopExcept` then call `__exit__(None, None, None)` (sync) or
    /// `await __aexit__(None, None, None)` (async) before jumping (issue #2295).
    /// The exception path is handled separately by the body's `SetupExcept`,
    /// so — like `TryBody` — a `raise` stops the early-exit walk here.
    WithBody {
        /// Register holding the context-manager object (lives for the body).
        ctx_reg: Reg,
        /// `true` for `async with` (drives `await __aexit__`), `false` for `with`.
        is_async: bool,
    },
}

/// Describes how to emit the PEP 3110 except-as variable deletion on early exit.
#[derive(Clone)]
enum ExceptAsVarDel {
    /// Variable lives in a fastlocal register. A module binding may also have
    /// been mirrored into the live globals dict after `globals()` exposure.
    Local {
        register: Reg,
        /// Real index into `FnCode::names`, used by `DeleteLocal` to remove the
        /// binding from a materialized class-frame namespace.
        name_index: u16,
        delete_module_global: bool,
    },
    /// Variable lives in a function-scope cell slot; emit `DeleteCell(idx, false)`.
    Cell(u16),
    /// Variable lives in env (no local slot); emit `DeleteName(name_idx)`.
    Name(u16),
}

struct Compiler {
    local_index: Rc<HashMap<String, Reg>>,
    /// Names kept off fastlocal registers in this scope:
    /// - For function scopes: this function's own cell variables (`0..own_cell_count`).
    /// - For module scopes: module names that stay in `module_env.namespace_values`
    ///   (never emitted into `FnCode::cell_slots`).
    cell_vars: HashSet<Symbol>,
    /// Positional cell-slot table built for this scope (issues #452 / #2339 / #2348).
    /// Slots `0..own_cell_count` are own cells (pre-sorted); slots `own_cell_count..`
    /// are free variables (sorted & remapped in `finish()`).
    cell_slots: SmallVec<[CellSlotMeta; 4]>,
    /// Leading slice of `cell_slots` owned by this frame (`co_cellvars`).
    own_cell_count: u16,
    /// O(1) lookup from cell/freevar name to its current slot index in `cell_slots`.
    cell_slot_map: HashMap<Symbol, u16>,
    /// Names backed by a cell in some enclosing function scope (available to
    /// capture as free variables unless shadowed by a local, `global`, or
    /// PEP 695 type parameter). Empty at module scope.
    enclosing_cell_names: HashSet<Symbol>,
    /// Names explicitly declared `global` in this scope (must never resolve to
    /// an enclosing function cell).
    global_names: HashSet<Symbol>,
    /// Active PEP 695 type-parameter names temporarily in scope while compiling
    /// annotations, base classes, or type-alias / TypeVar-bound thunks. These
    /// names resolve via `LoadGlobal` in the active `TypeParamEnv`, never via
    /// an enclosing function's same-named cell or local.
    shadowed_type_params: SmallVec<[Symbol; 4]>,
    /// Names declared `nonlocal` in this function or class body (issue #2339).
    /// Empty for module scopes (`nonlocal` is invalid there).
    nonlocal_names: HashSet<String>,
    /// Names declared `global` or `nonlocal` in a class body. Unlike ordinary
    /// unresolved class names, these must bypass the class namespace and keep
    /// true environment lookup semantics even after `f_locals` is exposed.
    /// Empty outside class-body compilers.
    class_direct_env_names: HashSet<String>,
    insns: Vec<Insn>,
    descs: Vec<InsnDesc>,
    /// Per-instruction 1-based source line numbers, parallel to `insns`.
    /// Filled by `emit()` from `current_lineno`.  0 = unknown.
    lineno_table: Vec<u32>,
    /// Per-instruction PEP 657 caret anchor, parallel to `insns` (issues #2426 /
    /// #2411).  Filled by `emit()` from `current_col_span`.  Each entry is
    /// `(full_start, prim_start, prim_end, full_end)` (see [`crate::ast::CaretSpan`]);
    /// `(0, 0, 0, 0)` = no anchor.
    col_table: Vec<crate::ast::CaretSpan>,
    /// 1-based line number of the statement currently being compiled.
    /// Set by `set_lineno()` before each `compile_stmt` call when line
    /// information is available.  0 when no line info is known.
    current_lineno: u32,
    /// PEP 657 caret anchor stamped onto the next emitted instruction(s)
    /// (issues #2426 / #2411).  Set transiently by `compile_expr` around the
    /// instruction that loads a plumbed sub-expression (bare-name `Var`, call,
    /// binary op, subscript), then cleared back to `(0, 0, 0, 0)`.
    /// `(0, 0, 0, 0)` means "no anchor".
    current_col_span: crate::ast::CaretSpan,
    /// 1-based source line of the `def`/`lambda` this compiler is the body of
    /// — emitted into `FnCode::first_lineno` (the function's `co_firstlineno`).
    /// 0 for the module-level compiler (issue #2185).
    first_lineno: u32,
    /// Source-file path the code being compiled comes from — emitted into
    /// `FnCode::filename` (the code object's `co_filename`).  Threaded into every
    /// nested function/class body so an imported module's functions report their
    /// own file in tracebacks and `__code__.co_filename` (issue #2438), rather
    /// than the running script's path.  `<unknown>` until a compile entry point
    /// sets it.
    filename: std::sync::Arc<str>,
    /// Compile-time qualified name for the code object being compiled — emitted
    /// into `FnCode::qualname` (`co_qualname`, issue #2981).  Defaults to
    /// `"<module>"` for module-level compilation.
    qualname: std::sync::Arc<str>,
    consts: Vec<Value>,
    const_index: HashMap<crate::value::PyKey, u16>,
    names: Vec<Symbol>,
    name_map: HashMap<Symbol, u16>,
    next_temp: Reg,
    base_temp: Reg,
    iter_depth: u8,
    max_iter: u8,
    max_reg: Reg,
    loops: Vec<LoopCtx>,
    /// Stack of cleanup actions needed by early exits (`break`/`continue`/`return`)
    /// that cross a `try`/`except` boundary.  Entries are pushed when entering a
    /// guarded block and popped when leaving it normally.
    /// `SmallVec<[_; 4]>` avoids heap allocation for the common case of at most
    /// four nested try/except levels.
    except_cleanups: SmallVec<[EarlyExitCleanup; 4]>,
    failed: bool,
    error_msg: Option<String>,
    def_set: u64,
    fn_protos: Vec<FnProto>,
    /// Names of *memo-pure* functions defined in this scope — callees whose
    /// result may be cached/reused (drives `CallMemo` emission).  See issue
    /// #2523.
    pure_locals: HashSet<String>,
    /// True when this Compiler is producing the body of a `class` block.
    /// In that mode, every store into a top-level class-body local is
    /// instrumented with `Insn::RecordClassStore(slot)` so the VM can
    /// recover **runtime** insertion order for `vars(C)` / `C.__dict__`.
    /// CPython guarantees class-namespace order follows the order names
    /// are first bound at runtime — not source-walk / slot-allocation order.
    is_class_body: bool,
    /// True when this Compiler is producing a function that was defined directly
    /// inside a class body.  Set by `compile_def` using `self.is_class_body` of
    /// the enclosing compiler.  Only direct class methods get this flag — nested
    /// functions inside methods do not, which mirrors CPython's `__class__` cell
    /// propagation rule: only the directly-defining function gets the cell.
    is_class_method: bool,
    /// The qualname prefix for classes/functions defined in this scope.
    /// Empty for the top-level scope.  When entering a class `Foo`, the child
    /// compiler's prefix becomes `"Foo"` (or `"Outer.Foo"` if nested).  When
    /// entering a function `fn_name`, the child compiler's prefix becomes
    /// `"fn_name.<locals>"` so that classes inside functions get the CPython
    /// `"fn_name.<locals>.ClassName"` form.
    qualname_prefix: String,
    /// Chain of `local_index` maps for every enclosing **function** scope
    /// (not module scope, not class scope — class scope is transparent to
    /// `nonlocal`).  Innermost enclosing function is at the end of the Vec.
    /// Used at compile time to validate `nonlocal` declarations in nested
    /// function bodies.
    /// `SmallVec<[_; 4]>` avoids heap allocation for typical nesting depths (≤ 4).
    outer_locals: SmallVec<[Rc<HashMap<String, Reg>>; 4]>,
    /// True when this Compiler is producing the body of a function `def`
    /// (or a comprehension, which implicitly creates a function scope).
    /// False for module-level compilation and class-body compilation.
    /// Used to determine whether `self.local_index` counts as an enclosing
    /// function scope for `nonlocal` validation in child compilers.
    is_function_scope: bool,
    /// True when this Compiler is producing the body of an `async def` function.
    /// Used to distinguish `'await' outside async function` (inside a non-async
    /// `def`) from `'await' outside function` (at module or class scope).
    is_async_function: bool,
    /// True when this function is an *async generator* (`async def` whose body
    /// contains a bare `yield`, #2280).  Computed from the body AST when the
    /// sub-compiler is set up.  CPython rejects `return <value>` in an async
    /// generator with `SyntaxError: 'return' with value in async generator`.
    is_async_generator_fn: bool,
    /// True when a compile-time `SyntaxError` has been detected (e.g. a
    /// `nonlocal` declaration with no enclosing binding).  Controls whether
    /// `finish()` emits `PyError::Named("SyntaxError", …)` or `PyError::Runtime`.
    is_syntax_error: bool,
    /// True when this Compiler is producing the top-level module/script body.
    /// In that mode, every local-register store emits a `SyncModuleGlobal`
    /// immediately after the `Move`, so that `module_globals_dict` stays live
    /// after `globals()` has been called.  Child compilers for functions and
    /// class bodies set this to false — they write to fastlocals only.
    is_module_scope: bool,
    /// True when compiling interactive (`compile(..., 'single')` or REPL)
    /// module-scope code: every `Stmt::Expr` at module scope (including inside
    /// `if`/`for`/`while`/`try`/`with`/`match`, but not inside child `def`/
    /// `class`/comprehension compilers) emits `Insn::PrintExpr` to invoke
    /// `sys.displayhook`.
    print_exprs: bool,
    /// True when syntax anywhere in the top-level compile unit can retain or
    /// mutate its live namespace mapping. Used to keep discarded, definitely
    /// bound module-name reads free when no such alias can exist.
    module_namespace_may_be_exposed: bool,
    /// True once we have compiled a statement that is neither a module docstring
    /// nor a `from __future__ import ...`.  After this point any `from __future__`
    /// import must be rejected with
    /// `SyntaxError: from __future__ imports must occur at the beginning of the file`.
    past_future_zone: bool,
    /// True when `from __future__ import annotations` has been seen (PEP 563).
    /// When set, annotation expressions are NOT evaluated; instead, their source
    /// text is stored as a string literal in `__annotations__`.
    future_annotations: bool,
    /// True when any `yield` / `yield from` expression appears in the function
    /// body inside a compile-time-false branch (i.e. `if False: yield`).
    /// Such expressions are never emitted as `Insn::Yield` / `Insn::YieldFrom`,
    /// so the post-compilation `is_generator` scan misses them.  This flag
    /// ensures the function is still treated as a generator, matching CPython
    /// where the presence of `yield` in the source — even in dead code — makes
    /// the enclosing function a generator function (issue #1758).
    has_dead_yield: bool,
    /// True when this Compiler is producing the implicit function body of a
    /// **set comprehension**.  In that mode the synthesized accumulator add
    /// `.acc.add(elt)` is lowered directly to `Insn::SetAdd(acc, elt)` instead
    /// of a full attribute-lookup + method-call dispatch, mirroring CPython's
    /// dedicated `SET_ADD` opcode (issue #1861).
    is_set_comp: bool,
    /// True when this Compiler is producing the implicit function body of a
    /// **list comprehension**.  In that mode the synthesized accumulator append
    /// `.acc.append(elt)` is lowered directly to `Insn::ListAppend(acc, elt)`
    /// instead of a full attribute-lookup + method-call dispatch, mirroring
    /// CPython's dedicated `LIST_APPEND` opcode (issue #1862).
    is_list_comp: bool,
    /// True when this Compiler is producing the reduction frame of an inlined
    /// `sum(<genexpr>)` (#3384).  In that mode the synthesized accumulator
    /// step `.acc.sum_step(.v)` is lowered directly to `Insn::SumStep`.
    is_sum_reduction: bool,
    /// True when this Compiler is a **list comprehension** body whose single
    /// unconditional clause lets the accumulator be pre-sized from the source
    /// length (`[f(x) for x in src]`): the element count equals `len(src)`, so
    /// the synthesized `.acc = []` init is lowered to `Insn::BuildListReserve`,
    /// reserving capacity up front to skip the geometric-growth reallocations.
    /// Set only when there is exactly one clause, no `if` condition, and the
    /// comprehension is not async (see `compile_list_comp`).
    list_comp_presize: bool,
    /// True when this Compiler is producing the implicit function body of a
    /// list / set / dict comprehension — the forms CPython 3.12 *inlines* into
    /// the enclosing frame (PEP 709).  pyrust still runs them as a separate
    /// frame, but for error parity an unbound read of an enclosing local must
    /// surface as `UnboundLocalError` (as if local to the enclosing frame), not
    /// the free-variable `NameError` a real closure / generator expression gets
    /// (issue #2340).
    is_inlined_comp: bool,
    /// For an inlined comprehension, the local-variable names of the
    /// immediately-enclosing real function (the PEP 709 inlining target).  Used
    /// at runtime to decide whether an unbound read is a local of that frame
    /// (`UnboundLocalError`) or a free variable owned by a grandparent scope
    /// (`NameError`) — see issue #2457.  `None` outside an inlined comp.
    comp_enclosing_locals: Option<Rc<HashSet<Symbol>>>,
    /// PEP 657 caret span covering an inlined comprehension (`[...]` / `{...}`),
    /// attached to its outermost `.0` iteration (`GetIter`/`ForIter` or
    /// `async for` await drive) so exceptions from iterating the comprehension
    /// underline the comprehension expression in the enclosing frame (#2993).
    comp_outer_for_span: Option<crate::ast::CaretSpan>,
    /// Optimization level (`0`, `1` omits `assert` and folds `__debug__` to
    /// `False`, `2` also strips module/class/function docstrings).
    optimize: u8,
    /// True when `ast.PyCF_ALLOW_TOP_LEVEL_AWAIT` is active at module/eval/single
    /// scope (not inherited by nested `def`/`class` scopes).
    allow_top_level_await: bool,
    /// Set to true when a top-level `await` / `async for` / `async with` was
    /// compiled under `allow_top_level_await`, marking the resulting `FnCode`
    /// with `is_coroutine = true`.
    saw_top_level_await: bool,
}
