//! Register operands and the executable instruction schema.

use crate::ast::{BinaryOp, UnaryOp};

pub type Reg = u32;

/// Sentinel `Reg` marking an absent operand — currently the missing `**kw`
/// mapping of an [`Insn::CallExArgs`] with no double-splat.  No real register is
/// ever allocated at `u32::MAX`, so it can never collide with a live operand.
pub const NO_KWARGS: Reg = Reg::MAX;

/// How a `DictMergeKwCall` / `SetItemKwCall` recovers the callee's qualified
/// name for the `… got multiple values for keyword argument …` error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KwCallName {
    /// The callee value lives in register `R[reg]` (direct `f(**a, **b)` call);
    /// the error uses its `<module>.<qualname>`.
    Callee(Reg),
    /// Method call `obj.m(**a, **b)`: the receiver is in `R[obj]` and the method
    /// name is `names[name_idx]`.  The error uses `<module>.<class>.<method>`
    /// derived from the receiver's class.
    Method { obj: Reg, name_idx: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Insn {
    /// R[dst] = consts[idx]
    LoadConst(Reg, u16),
    /// R[dst] = lookup name through env chain
    LoadGlobal(Reg, u16),
    /// names[name_idx] = R[src]  (write to module / enclosing env)
    StoreGlobal(u16, Reg),
    /// R[dst] = read a **function-scope cell variable** (a name captured by a
    /// nested function, or one declared `nonlocal`) directly from the env chain
    /// (issue #2339).  Unlike `LoadGlobal`, the compiler proves at emit time that
    /// the name resolves to a cell (never a module global / builtin), so the VM
    /// skips the `LoadGlobal` inline-cache probe and the module-globals-dict
    /// fallback entirely — it goes straight to the env-chain cell read.  The
    /// backing store is still the env (sibling cells, suspended generators, and
    /// `__closure__` introspection are unchanged); only the per-access
    /// global/cell multiplexing is removed.
    LoadCell(Reg, u16),
    /// names[name_idx] = R[src]  — write a **function-scope cell variable**
    /// (cell or `nonlocal`) directly into the owning env (issue #2339).  The
    /// compiler proves the target is a cell, so the VM bypasses the module-global
    /// mirror / globals-dict sync path that `StoreGlobal` carries for true
    /// globals; `nonlocal` writes still walk to the enclosing owning env.
    StoreCell(u16, Reg),
    /// R[dst] = None
    LoadNone(Reg),
    /// R[start], R[start+1], ..., R[start+count-1] = None
    /// Equivalent to `count` consecutive `LoadNone` instructions.
    LoadNoneRange { start: Reg, count: u8 },
    /// R[dst] = R[src]
    Move(Reg, Reg),
    /// R[dst] = R[src]  — emitted by the CSE pass; semantically identical to Move
    /// but distinguished so that copy-propagation does not chase through it and
    /// confuse subsequent passes that expect Move for named-variable copies.
    CopyReg(Reg, Reg),
    /// R[dst] = R[lhs] op R[rhs]
    BinOp(Reg, Reg, BinaryOp, Reg),
    /// R[dst] = R[lhs] op= R[rhs]  (tries __i<op>__ before __<op>__)
    BinOpInPlace(Reg, Reg, BinaryOp, Reg),
    /// R[dst] = R[lhs] op consts[const_idx]  (fuses LoadConst + BinOp)
    ///
    /// The trailing `bool` is `is_aug`: `true` when this fused op originated from
    /// an augmented assignment (`x op= c`), in which case the VM applies in-place
    /// `__i<op>__` / mutable-container semantics; `false` for a plain binary
    /// expression that was const-folded, in which case it must behave exactly like
    /// `BinOp` (non-mutating `__<op>__`).  This flag replaces the old `dst == lhs`
    /// heuristic, which mis-fired because `ensure_dst` reuses the lhs temp for
    /// plain binary ops too (issue #1874).
    BinOpConst(Reg, Reg, BinaryOp, u16, bool),
    /// R[dst] = R[lhs] op imm  (carries a small signed integer directly, no const-pool lookup)
    /// Emitted instead of BinOpConst when the constant fits in i16::MIN..=i16::MAX.
    /// The trailing `bool` is `is_aug`; see `BinOpConst`.
    BinOpImm(Reg, Reg, BinaryOp, i16, bool),
    /// R[dst] = unary_op(R[src])
    UnaryOp(Reg, UnaryOp, Reg),
    /// R[dst] = `isinstance(R[subj], (str, bytes, dict, set, frozenset))`
    ///
    /// PEP 634 §3: these five types are excluded from sequence-pattern
    /// matching (str/bytes are text sequences; dict/set/frozenset support
    /// `len()` but not integer indexing).  Emitted once per `Pattern::Sequence`
    /// arm in place of the `5×LoadGlobal + BuildTuple + Call(isinstance)`
    /// sequence so that a `match` inside a tight loop pays a single
    /// allocation-free type check instead of rebuilding the exclusion tuple on
    /// every iteration (issue #1789).  Subclasses are honoured (a `dict`
    /// subclass instance is excluded; a `list` subclass instance is not).
    MatchSeqExcluded(Reg, Reg),
    /// R[dst] = `isinstance(R[subj], collections.abc.Mapping)` — the
    /// mapping-pattern type gate (PEP 634 §3).  Emitted once per
    /// `Pattern::Mapping` arm before any per-key membership test so that a
    /// non-mapping subject (int, str, list, set, …) fails the match instead of
    /// raising on `key in subj` (issue #1879).  In pyrust the only built-in
    /// mapping is `dict`; subclasses are honoured (a `dict` subclass instance
    /// matches).  Mirrors `MatchSeqExcluded` but with inverted polarity (true
    /// means "is a mapping", so codegen jumps to fail on false).
    MatchMapping(Reg, Reg),
    /// R[dst] = R[obj].names[name_idx]
    GetAttr(Reg, Reg, u16),
    /// Like GetAttr but converts any lookup failure to the TypeError CPython
    /// raises for the (a)synchronous context-manager / async-iterator protocols.
    /// The trailing `u8` selects which message:
    ///   0 = `with` `__enter__`  — "does not support the context manager protocol"
    ///   1 = `with` `__exit__`   — adds " (missed __exit__ method)"
    ///   2 = `async for` `__aiter__` — "'async for' requires an object with
    ///        __aiter__ method, got X"
    ///   3 = `async with` `__aenter__` — "does not support the asynchronous
    ///        context manager protocol"
    ///   4 = `async with` `__aexit__`  — adds " (missed __aexit__ method)"
    ///   5 = `async for` `__anext__`  — "'async for' received an object from
    ///        __aiter__ that does not implement __anext__: X"
    GetAttrForWith(Reg, Reg, u16, u8),
    /// R[obj].names[name_idx] = R[val]
    SetAttr(Reg, u16, Reg),
    /// del R[obj].names[name_idx]
    DeleteAttr(Reg, u16),
    /// R[dst] = R[obj][R[idx]]
    GetItem(Reg, Reg, Reg),
    /// R[dst] = R[obj][R[base] : R[base+1] : R[base+2]]  (rvalue slice read).
    /// CPython's BINARY_SLICE analogue: reads the three contiguous bound
    /// registers (start, stop, step; `None` = absent) and slices `obj` directly,
    /// without materialising a `slice` object for built-in sequences (#1964).
    /// User `__getitem__` / BuiltinObject paths still receive a real `slice`.
    GetSlice(Reg, Reg, Reg),
    /// R[obj][R[idx]] = R[val]
    SetItem(Reg, Reg, Reg),
    /// del R[obj][R[idx]]
    DeleteItem(Reg, Reg),
    /// del names[name_idx] from current env
    DeleteName(u16),
    /// PEP 695: push a fresh child environment (parented to the current one)
    /// that holds the type parameters of a generic `def` / `class`.  Subsequent
    /// `StoreGlobal` for a type-param name binds it here instead of in the
    /// enclosing namespace, and a generic function/class created while this
    /// scope is active captures it so its body can still resolve the type
    /// parameter lazily — yet the name never leaks into the enclosing scope
    /// (mirrors CPython's hidden type-param/annotation scope).  Paired with
    /// `PopTypeParamEnv`.
    PushTypeParamEnv,
    /// PEP 695: pop the type-parameter environment pushed by `PushTypeParamEnv`,
    /// restoring the enclosing environment.  Emitted after the generic object is
    /// built (and its `__type_params__` populated) but before the def/class name
    /// is bound in the enclosing scope.
    PopTypeParamEnv,
    /// Clear local register (del for a fastlocal).
    ///
    /// If `name_idx` is not `u16::MAX`, the VM checks whether the register
    /// was already unset before the delete and raises `NameError` (module
    /// scope) or `UnboundLocalError` (function scope) with the variable name
    /// `names[name_idx]`.  Pass `u16::MAX` for compiler-guaranteed-bound
    /// deletions (e.g. PEP 3110 `except E as var:` cleanup) where the check
    /// is unnecessary and should be skipped.
    DeleteLocal(Reg, u16),
    /// pc += offset  (offset 0 = next instruction)
    Jump(i32),
    /// Jump when Python's canonical truth-value protocol reports false.
    JumpIfFalse(Reg, i32),
    /// Jump when Python's canonical truth-value protocol reports true.
    JumpIfTrue(Reg, i32),
    /// Compare, apply canonical truth-value conversion to the result, and jump
    /// when false (exact scalar comparisons retain their allocation-free path).
    CmpJumpIfFalse(Reg, BinaryOp, Reg, i32),
    /// Compare, apply canonical truth-value conversion to the result, and jump
    /// when true.
    CmpJumpIfTrue(Reg, BinaryOp, Reg, i32),
    /// Constant-RHS form of `CmpJumpIfFalse`.
    CmpJumpIfFalseConst(Reg, BinaryOp, u16, i32),
    /// Constant-RHS form of `CmpJumpIfTrue`.
    CmpJumpIfTrueConst(Reg, BinaryOp, u16, i32),
    /// Jump when R[reg] is not an exact built-in integer representable as i64.
    ///
    /// Emitted only by the optimizer's int-loop versioning pass as the entry
    /// guard for an out-of-line specialized loop copy; any value that fails the
    /// guard (bool, out-of-i64 BigInt, str, user object, unset, …) runs the
    /// original in-place loop unchanged, so the guard never changes Python
    /// semantics.
    JumpIfNotInt(Reg, i32),
    /// Fused counted-loop back-edge: R[var] = R[var] + imm, then jump when
    /// `R[var] op R[stop]` is true.
    ///
    /// Exact semantics of the `BinOpImm(var, var, Add, imm, true)` +
    /// `CmpJumpIfTrue(var, op, stop, off)` pair it replaces, including
    /// int→BigInt overflow promotion; the VM arm delegates to the same
    /// helpers those two instructions use.
    CountCmpJumpTrue(Reg, BinaryOp, Reg, i16, i32),
    /// As `CountCmpJumpTrue` but jumps when the comparison is false
    /// (the `BinOpImm` + `CmpJumpIfFalse` pair).
    CountCmpJumpFalse(Reg, BinaryOp, Reg, i16, i32),
    /// Jump when iterator slot `slot` does not currently hold the canonical
    /// machine-int `range` cursor state.
    ///
    /// Emitted only by the int-loop versioning pass as the entry guard for an
    /// out-of-line `for … in range(…)` copy: a canonical int-range cursor is
    /// advanced without invoking any Python-visible protocol, so a body of
    /// proven non-reentrant int operations can defer its module syncs to the
    /// loop exits.  Any other iterator (BigInt range, list, generator, user
    /// object, empty slot) diverts to the original in-place loop unchanged.
    JumpIfIterNotIntRange(Reg, i32),
    /// Guarded numeric leaf-call inlining site (emitted only by the
    /// optimizer, always immediately before the original call sequence it
    /// specializes, which stays in place as the deopt path).
    ///
    /// When R[callee] is the exact `Regular` user function compiled from
    /// `fn_protos[proto]` — compared by code-object identity, so any runtime
    /// rebinding of the source-level name deopts — and both argument
    /// registers hold machine integers, the VM stores `R[a] op R[b]` into
    /// R[dst] (the call-base register) and jumps by `skip` over the call
    /// sequence.  That computation is the entire observable effect of calling
    /// the eligible two-parameter `return a op b` leaf on ints: no user code,
    /// raise, or namespace read is elided, and overflow promotes to BigInt
    /// exactly like the called body would.  On any guard failure execution
    /// falls through into the unmodified call sequence.
    CallInlineBinOp {
        callee: Reg,
        dst: Reg,
        a: Reg,
        op: BinaryOp,
        b: Reg,
        proto: u16,
        skip: i32,
    },
    /// R[func_reg] = call(R[func_reg], R[func_reg+1..func_reg+1+argc]); result in R[func_reg]
    Call(Reg, u8),
    /// Marks a call to a statically memo-pure callee. The VM may reuse a cached
    /// scalar result for supported integer arguments; otherwise it executes
    /// identically to `Call`. The marker never authorizes eliminating the call.
    CallMemo(Reg, u8),
    /// R[dst] = R[obj].names[name_idx](R[args_base..args_base+nargs])
    /// Dispatches directly to pyrust_builtins without going through GetAttr.
    /// Allows the VM to give mutable access to List/Dict registers.
    CallMethod {
        dst: Reg,
        obj: Reg,
        name_idx: u16,
        args_base: Reg,
        nargs: u8,
    },
    /// Like CallMethod but args are pre-built as a positional list and a kwargs dict.
    /// R[dst] = R[obj].names[name_idx](*R[pos_list], **R[kw_dict])
    CallMethodExpanded {
        dst: Reg,
        obj: Reg,
        name_idx: u16,
        pos_list: Reg,
        kw_dict: Reg,
    },
    /// Keyword-argument call with no `*args` / `**kwargs` splats (issue #2382).
    /// Mirrors CPython's `KW_NAMES` + `CALL`: arguments are laid out
    /// contiguously in registers — `total` values in `R[func+1 .. func+1+total]`
    /// — and the *last* `nkw` of them are keyword arguments whose names are the
    /// strings in the constant-pool tuple `consts[kwnames_idx]` (in order).  The
    /// first `total - nkw` are positional.  Result is written back to `R[func]`.
    ///
    /// This replaces the old `BuildList`+`BuildDict`+`SetItem`+hidden-helper
    /// lowering for plain keyword calls, which allocated a dict and a list and
    /// round-tripped through a Python-visible builtin on every invocation.
    CallKw {
        func: Reg,
        total: u8,
        nkw: u8,
        kwnames_idx: u16,
    },
    /// Double-splat keyword-expansion call `f(<pos…>, **d)` (issue #2393).
    /// `npos` plain positional arguments occupy `R[func+1 .. func+1+npos]`, and
    /// `R[kwargs]` holds the single `**d` source mapping.  Result is written back
    /// to `R[func]`.
    ///
    /// This replaces the old `BuildList`+`BuildDict`+`DictUpdate`+hidden-helper
    /// lowering for the common `f(**d)` / `f(a, **d)` shapes, which copied the
    /// whole dict, round-tripped through a Python-visible builtin, and then
    /// linearly name-scanned the parameter list on every call.  A per-call-site
    /// cache ([`KwCallCacheEntry::ExSimple`]) records the dict key-set →
    /// parameter-slot mapping for a monomorphic plain-`UserFunction` callee whose
    /// `**d` keys are stable across calls, so the keyword values bind straight
    /// into their slots (reusing the #2382 `kwcall_resolve_simple` /
    /// `call_user_function_kw_cached` machinery) with no dict copy and no name
    /// scan.  Only emitted for a single trailing `**d` with no `*args` splat and
    /// no literal keywords, non-method callee; every other variadic shape keeps
    /// the generic path.
    CallEx { func: Reg, npos: u8, kwargs: Reg },
    /// Positional-splat expansion call `f(<pos…>, *args[, **kw])` (the
    /// decorator/wrapper shape).  `npos` plain leading positionals occupy
    /// `R[func+1 .. func+1+npos]` contiguously; `R[args_splat]` holds the single
    /// `*args` iterable; `R[kwargs]` holds the single `**kw` mapping, or the
    /// sentinel [`NO_KWARGS`] when the call has no `**kw`.  Result is written back
    /// to `R[func]`.
    ///
    /// This replaces the old `BuildList`+`ListExtend`+`BuildDict`+`DictMergeKwCall`
    /// +hidden-global lookup+`Move`×3+`Call` lowering for the common
    /// single-`*args`(+optional single-`**kw`) shape, which allocated a list and a
    /// dict, copied every splatted element and every kwarg into them, and
    /// round-tripped through a Python-visible builtin on every call. The runtime
    /// handler ([`Interpreter::exec_call_ex_args`]) reads the splat iterable and
    /// the `**kw` mapping directly. A per-call-site cache
    /// ([`KwCallCacheEntry::ExArgs`]) records the callee prototype + total
    /// positional count + `**kw` key-set → parameter-slot mapping for a
    /// monomorphic plain-`UserFunction` callee, so on a hit the values bind
    /// straight into their slots with no intermediate list/dict and no name scan.
    /// Emitted for a single `*args` (as the last positional group), followed by
    /// zero or more literal `kw=v` keyword arguments and an optional single
    /// trailing `**kw`, non-method callee. Generic source-order-sensitive shapes
    /// also use this opcode after the compiler has materialized their positional
    /// list and keyword dict; for that transport form `npos == nkw == 0`.
    ///
    /// The `nkw` literal keyword VALUES occupy `R[func+1+npos ..
    /// func+1+npos+nkw]` (contiguously, right after the leading positionals);
    /// their NAMES are the strings in the constant-pool tuple
    /// `consts[kwnames_idx]` (in the same order).  The fixed-arity slot fast bind
    /// only engages when `nkw == 0`; with literal keywords the call takes the
    /// variadic fast path or the slow path (both fold the literals into the
    /// keyword arguments), so no additional slot-cache shape is needed.
    CallExArgs {
        func: Reg,
        npos: u8,
        nkw: u8,
        kwnames_idx: u16,
        args_splat: Reg,
        kwargs: Reg,
    },
    /// Keyword-argument method call `R[obj].name(<pos…>, k=v…)` with no
    /// `*args` / `**kwargs` splats (issue #2392).  The receiver lives in
    /// `R[obj]`; the `total` argument values occupy `R[args_base ..
    /// args_base+total]` contiguously, the trailing `nkw` of them being keyword
    /// arguments whose names are the constant-pool tuple `consts[kwnames_idx]`
    /// (in order).  The first `total - nkw` are positional.  Result is written
    /// to `R[dst]` (which may equal `R[obj]` for non-fast-local receivers, as
    /// with `CallMethod`).
    ///
    /// Combines the `CallMethod` inline-cache method resolution (#2345) with the
    /// `CallKw` keyword fast-bind (#2382): on a monomorphic cache hit for a
    /// plain Python method the receiver binds to parameter 0 and the keyword
    /// values bind straight into their slots with no dict/list build and no name
    /// scan.  Replaces the old `BuildList`+`BuildDict`+`CallMethodExpanded`
    /// lowering for literal-keyword method calls.  Falls back to the general
    /// method-expansion path (which owns CPython-parity diagnostics) on any
    /// cache miss, a builtin/backing method, or a non-simple binding shape.
    CallMethodKw {
        dst: Reg,
        obj: Reg,
        name_idx: u16,
        args_base: Reg,
        total: u8,
        nkw: u8,
        kwnames_idx: u16,
    },
    /// return R[src]
    Return(Reg),
    /// return None
    ReturnNone,
    /// R[dst] = [R[base], R[base+1], ..., R[base+n-1]]
    BuildList(Reg, Reg, u32),
    /// R[dst] = a fresh empty list, pre-sized to the length hint of R[src].
    ///
    /// Emitted only for the accumulator of a single-clause, unconditional list
    /// comprehension (`[f(x) for x in src]`) where the element count equals the
    /// source length, so the result list can be reserved up front and skip the
    /// geometric-growth reallocations. The hint is read from the source register
    /// (`.0`, the comprehension's iterable parameter) using only length queries
    /// that never invoke user code; an unknown-length source reserves nothing.
    /// Semantically identical to `BuildList(dst, _, 0)` (always an empty list) —
    /// the reservation is purely a capacity optimisation.
    BuildListReserve(Reg, Reg),
    /// R[dst] = (R[base], R[base+1], ..., R[base+n-1])
    BuildTuple(Reg, Reg, u32),
    /// R[dst] = R[base] ++ R[base+1] ++ ... ++ R[base+n-1]
    /// Concatenates `n` consecutive `str` registers into a single string in one
    /// pass over a preallocated buffer. Emitted only by f-string lowering
    /// (`compile_fstring`), where every operand is guaranteed to be a `str`
    /// (literals + formatted interpolations). Mirrors CPython's BUILD_STRING.
    BuildString(Reg, Reg, u8),
    /// R[dst] = format(R[src], "") — the f-string interpolation default
    /// conversion with no `!r/!s/!a` flag and no format spec. Equivalent to the
    /// `format` builtin called with an empty spec, but skips the `format` global
    /// lookup and the generic call frame. Mirrors CPython's FORMAT_VALUE with no
    /// conversion + no spec. User `__format__`/`__str__` dispatch is preserved
    /// for `PyInstance` operands (handled in the VM by delegating to the real
    /// `format` builtin for that rare case).
    FormatValue(Reg, Reg),
    /// R[dst] = format(R[src], R[spec]) — the f-string interpolation with a
    /// format spec but no `!r/!s/!a` conversion (those are lowered to a `repr`/
    /// `str`/`ascii` call before the spec is applied). `R[spec]` is the spec
    /// string register produced by the nested-f-string lowering. Equivalent to
    /// the `format` builtin called with that spec, but skips the `format` global
    /// lookup, the two-register call window, and the call-arg expansion. Mirrors
    /// CPython's FORMAT_VALUE with a spec. User `__format__` dispatch is
    /// preserved for `PyInstance` operands via `dispatch_dunder_format`.
    FormatValueSpec(Reg, Reg, Reg),
    /// R[dst] = slice(R[base], R[base+1], R[base+2])
    /// Emitted by the compiler for slice notation (a[lo:hi:step]).  Always
    /// reads exactly three registers (start, stop, step); `None` means absent.
    /// Using a dedicated instruction (rather than `BuildTuple`) removes the
    /// ambiguity that caused 3-element tuples to be misidentified as slices
    /// in `unpack_slice_key` (issue #931).
    BuildSlice(Reg, Reg),
    /// R[dst] = {R[base]: R[base+1], R[base+2]: R[base+3], ...}  (n key-value pairs)
    BuildDict(Reg, Reg, u32),
    /// R[base..base+n] = iter_values(R[src])
    Unpack(Reg, Reg, u32),
    /// Extended unpack: R[src] is an iterable; store first `before` elements into
    /// R[dst_base..dst_base+before-1], the middle as a list into R[dst_base+before],
    /// and the last `after` elements into R[dst_base+before+1..dst_base+before+after].
    /// Raises ValueError if len(iterable) < before + after.
    UnpackEx {
        src: Reg,
        before: u8,
        after: u32,
        dst_base: Reg,
    },
    /// iters[slot] = iter_values(R[src])
    GetIter(u8, Reg),
    /// if iters[slot] exhausted: pc += offset; else R[dst] = next(iters[slot])
    ForIter(Reg, u8, i32),
    /// error if R[reg] is uninitialised: "cannot access local variable '<name>' ..."
    CheckLocal(Reg, u16),
    /// raise AssertionError(R[msg])  (condition already tested by JumpIfTrue)
    RaiseAssert(Reg),
    /// raise AssertionError() with no args (`assert False` with no message)
    RaiseAssertNoMsg,
    /// raise R[exc]  (coerces class to instance)
    RaiseValue(Reg),
    /// raise R[exc] from R[cause]  (sets __cause__ on the coerced instance)
    RaiseFrom(Reg, Reg),
    /// re-raise active exception (bare `raise`)
    RaiseReRaise,
    /// PEP 654 (#2755): re-raise the residual `except*` group in R[exc].
    /// Behaves like a *bare* re-raise of the leftover group: it keeps the
    /// group's carried traceback without prepending the epilogue frame and
    /// does not attach fresh implicit `__context__` (the residual preserves
    /// whatever `__context__` the original group already carried, and surfaces
    /// with `__suppress_context__ is True`, matching CPython).
    RaiseExceptStarResidual(Reg),
    /// Match positional sub-patterns of a class pattern.
    ///
    /// Loads `R[cls].__match_args__`, validates that it is a tuple or list of
    /// length >= `n`, then for each `i in 0..n` gets the attribute name from
    /// `__match_args__[i]` and stores `getattr(R[subj], name)` into
    /// `R[dst_base + i]`.  Raises `TypeError` if `__match_args__` is absent,
    /// is not a tuple or list, or has fewer than `n` elements.
    MatchClassPositional {
        dst_base: Reg,
        subj: Reg,
        cls: Reg,
        n: u32,
    },
    /// R[dst] = new UserFunction(fn_protos[proto_idx], defaults R[defs_base..+defs_n],
    ///          annotations R[annots_base..+annots_n], env=current).
    /// `annots_n == 0` means no annotations; `annots_base` is ignored in that case.
    /// The annotation names (parallel to the register values) are stored in
    /// `FnProto::annotation_keys`.
    MakeFunction(Reg, u16, Reg, u32, Reg, u32),
    /// R[dst] = load_module(names[name_idx])
    ImportModule(Reg, u16),

    /// Attribute lookup on R[mod_reg] by the name `code.names[name_idx]`, with any
    /// AttributeError re-raised as ImportError. Emitted by compile_import_from so that
    /// missing names produce `ImportError: cannot import name '<name>' from '<module>'`
    /// matching CPython 3.12. Result stored in R[dst].
    ImportFromAttr(Reg, Reg, u16),
    /// Star import: iterate R[mod].__all__ (or all non-underscore attrs when
    /// __all__ is absent) and store each name into the current scope.
    /// Implements `from module import *` at runtime.
    ImportStar(Reg),
    /// Push an exception handler; if an exception is raised before PopExcept,
    /// the active_exception is set and pc jumps to (pc_after_this_insn + offset).
    SetupExcept(i32),
    /// Pop the innermost exception handler (normal exit from try block).
    PopExcept,
    /// R[dst] = current active exception value.
    LoadExc(Reg),
    /// R[dst] = R[exc].__traceback__, materialising the deferred-traceback
    /// placeholder (issue #2351) and writing the real chain back onto the
    /// exception instance so the object the `with`-statement passes to
    /// `__exit__` is identical to the one a later `e.__traceback__` read sees
    /// (issue #2359).  Yields `None` when the exception carries no traceback.
    LoadExcTraceback(Reg, Reg),
    /// if active_exception is NOT an instance of R[type_reg]: pc += offset.
    MatchExcept(Reg, i32),
    /// PEP 654 `except*` filter.
    ///
    /// Reads R[src_group] (must be a BaseExceptionGroup instance).  Filters
    /// its `.exceptions` for instances of R[type_reg].  If no exceptions
    /// match, jump to pc + offset.  Otherwise:
    ///   - R[matched_dst] = new sub-group containing only matching exceptions
    ///   - R[src_group]   = new sub-group containing only non-matching exceptions
    ///     (or None if all exceptions were matched)
    ///
    /// Multiple handlers are compiled sequentially; after each matched handler
    /// runs, the compiler emits code that moves the remaining sub-group back
    /// into R[src_group] so the next MatchExceptStar sees only leftovers.
    MatchExceptStar(Reg, Reg, Reg, i32),
    /// Clear active_exception (end of except handler).
    EndExcept,
    /// Push R[src] onto handled_exc_stack and set active_exception = R[src].
    /// Emitted before an inlined finally block when a new raise is in progress
    /// inside an except handler, so that any raise inside the finally sees the
    /// to-be-raised exception (not the currently-handled one) as __context__.
    PushExcContext(Reg),
    /// Pop the top of handled_exc_stack and restore active_exception to the
    /// new top.  Emitted after an inlined finally block to undo PushExcContext.
    PopExcContext,
    /// R[dst] = create class(fn_protos[proto_idx], bases R[bases_base..+bases_n], name=names[name_idx])
    /// PEP 487: kwarg_n keyword arg values are in R[kwarg_base..kwarg_base+kwarg_n];
    /// names come from fn_protos[proto_idx].class_kwarg_names.
    MakeClass(Reg, u16, Reg, u32, u16, Reg, u32),
    /// R[dst] = metaclass-driven class creation, where the metaclass value is in
    /// R[meta_reg].  Same layout as `MakeClass` plus a trailing `meta_reg`.
    /// Unlike `MakeClass`, this calls `metaclass.__prepare__(name, bases, **kw)`
    /// to obtain the body namespace, runs the class body into it, then calls
    /// `metaclass(name, bases_tuple, namespace, **kw)` — so all class-creation
    /// hooks (`__set_name__`, `__init_subclass__`) run once inside the metaclass
    /// (`type.__new__`) rather than in `MakeClass` (issues #2128/#2130).
    /// Tuple: (dst, proto_idx, bases_base, bases_n, name_idx, kwarg_base,
    /// kwarg_n, meta_reg).
    MakeClassMeta(Reg, u16, Reg, u32, u16, Reg, u32, Reg),
    /// R[dst] = TypeVar(name=consts[name_idx])
    /// PEP 695: construct an (initially unbounded) `TypeVar` object for a generic
    /// type parameter — `__bound__` is `None` and `__constraints__` is `()`.
    /// Any bound/constraint clause is populated lazily, after every type
    /// parameter of the enclosing def/class/alias is in scope, via
    /// `SetTypeVarAttr` on `__bound__` / `__constraints__` (see
    /// `Compiler::emit_typevar_bound`).
    /// This matches PEP 695's lazy evaluation of bounds in an annotation scope,
    /// so a self/forward-referential bound (`def f[T: T]`, `def g[T, U: T]`)
    /// resolves instead of raising `NameError`.
    MakeTypeVar(Reg, u16),
    /// R[obj].names[name_idx] = R[val], bypassing TypeVar's read-only-attribute
    /// guard.  Emitted only by the compiler to populate a PEP 695 TypeVar's
    /// `__bound__` / `__constraints__` after construction.  CPython sets these
    /// fields at the C level (never through `__setattr__`), so user-level
    /// `SetAttr` / `DeleteAttr` on these names raises `AttributeError` while this
    /// internal write succeeds.
    SetTypeVarAttr(Reg, u16, Reg),
    /// R[dst] = TypeAliasType(name=consts[name_idx],
    ///                        value_thunk=R[value_reg],
    ///                        type_params=R[params_reg])
    /// PEP 695: construct a `TypeAliasType` whose zero-argument evaluator runs
    /// and caches its successful result on first `__value__` access.
    MakeTypeAlias(Reg, u16, Reg, Reg),
    /// Print R[src] if not None (REPL expression output).
    PrintExpr(Reg),
    /// R[set].insert(R[val])  — in-place add for set comprehension construction,
    /// emitted by `compile_set_comp`.  Performs `__eq__`/`__hash__` dedup in the
    /// VM, semantically equivalent to `set.add()`.
    SetAdd(Reg, Reg),
    /// R[list].push(R[val])  — in-place append for variadic call construction
    ListAppend(Reg, Reg),
    /// R[list].extend(iter(R[src]))  — in-place extend
    ListExtend(Reg, Reg),
    /// R[dict].update(R[src])  — in-place dict merge
    DictUpdate(Reg, Reg),
    /// R[dict].merge(R[src]) for a `**d` keyword splat in a *call* context.
    /// Like `DictUpdate` but raises `TypeError: <callee>() got multiple values
    /// for keyword argument '<k>'` on a duplicate key (CPython `DICT_MERGE`),
    /// instead of silently overwriting (`dict.update` / `{**a, **b}` literals do
    /// overwrite).  The callee name for the error is resolved from `name`.
    DictMergeKwCall {
        dict: Reg,
        src: Reg,
        name: KwCallName,
    },
    /// R[dict][R[key]] = R[val] for a named (`kw=v`) argument in a *call*
    /// context, raising the same duplicate-key `TypeError` as `DictMergeKwCall`
    /// when `R[key]` is already present (collision with a prior `**d` splat).
    SetItemKwCall {
        dict: Reg,
        key: Reg,
        val: Reg,
        name: KwCallName,
    },
    /// Suspend the generator and yield R[src] to the caller.
    /// The result of the yield expression (sent value) is placed in R[dst].
    Yield { src: Reg, dst: Reg },
    /// PEP 380 `yield from` delegation.
    ///
    /// On each execution:
    /// 1. Reads sent value from `R[sent_reg]` (None on first call).
    /// 2. Calls `R[iter_reg].send(sent_val)` on the sub-iterator.
    /// 3. If the sub-iterator yields V: yields V to the outer caller, suspends
    ///    (pc rewinds to this instruction), and on next resume the caller's
    ///    sent value is written into `R[sent_reg]`.
    /// 4. If StopIteration with value R: writes R into `R[result_reg]`, continues.
    /// 5. Any other exception from the sub-iterator propagates outward.
    ///
    /// Throw forwarding: when the outer generator is thrown at while suspended
    /// here, the exception is forwarded to the sub-iterator via `.throw()`.
    YieldFrom {
        iter_reg: Reg,
        sent_reg: Reg,
        result_reg: Reg,
    },
    /// Resolve `R[src]` to an awaitable iterator for `await` (issue #1039).
    ///
    /// `R[dst] = GET_AWAITABLE(R[src])`:
    /// - a coroutine (an `async def` frame) is its own awaitable → returned as-is;
    /// - an object defining `__await__` → `R[dst] = R[src].__await__()`;
    /// - anything else → `TypeError: object can't be used in 'await' expression`.
    ///
    /// The compiler emits this immediately before a `YieldFrom` over `R[dst]`,
    /// reusing the PEP 380 send/throw drive machinery to suspend the awaiting
    /// coroutine until the awaitable completes.
    GetAwaitable(Reg, Reg),
    /// Record a runtime store to class-body local R[slot] for class-namespace
    /// insertion order.  Emitted **only** inside a class body, immediately after
    /// any instruction that stores into a top-level class-body local.  The VM
    /// appends slot to the active class-store-order list (if not already
    /// present), so MakeClass can later materialise vars(C) in the order
    /// stores actually executed — matching CPython __dict__ ordering.
    RecordClassStore(Reg),
    /// Record a runtime del C.name for class-namespace insertion order.
    /// Emitted only inside a class body, immediately after DeleteLocal(slot).
    /// The VM removes slot from the active class-store-order list so that the
    /// dict produced by MakeClass drops the entry while preserving the order
    /// of the remaining entries.
    RecordClassDel(Reg),
    /// R[dst] = R[base] + R[base+1] + ... + R[base+count-1]  (string concat, one allocation)
    ///
    /// All operands must be strings; if any is not, the VM falls back to
    /// sequential `BinOp(Add)`.  `count` must be ≥ 2.
    Concat { dst: Reg, base: Reg, count: u8 },
    /// Publish a module fast-local write to the root namespace. Emitted
    /// immediately after every module-scope register store so globals aliases,
    /// environment overlap, cache generations, and filesystem-module providers
    /// remain coherent. The runtime avoids dictionary materialization when no
    /// alias or overlapping representation needs the concrete value.
    SyncModuleGlobal(Reg, u16),
    /// Remove names[name_idx] from both module env.values and module_globals_dict.
    /// Emitted at module scope for `del varname` when the name has a fastlocal
    /// register (in addition to DeleteLocal which clears the register).
    /// Does NOT raise NameError — the caller must detect unset register state.
    DeleteModuleGlobal(u16),
}
