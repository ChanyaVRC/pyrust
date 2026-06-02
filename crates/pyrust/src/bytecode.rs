use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use smallvec::SmallVec;

use crate::ast::{BinaryOp, UnaryOp};
use crate::value::Value;

/// Identifies a named variable that is captured by a nested function via `nonlocal`.
/// These live in the env (not registers) so nested closures can share them.
pub type CellVar = String;

pub type Reg = u32;

/// Static parameter metadata for a function prototype.  Shared via `Rc` so that
/// `MakeFunction` (which may run on every loop iteration) pays only a refcount
/// bump instead of cloning four separate `Vec`s.
///
/// `SmallVec<[_; 6]>` avoids heap allocation for the common case of functions
/// with six or fewer parameters.
#[derive(Debug, Clone)]
pub struct FnParamSpec {
    pub names: SmallVec<[String; 6]>,
    pub has_default: SmallVec<[bool; 6]>,
    pub is_args: SmallVec<[bool; 6]>,
    pub is_kwargs: SmallVec<[bool; 6]>,
    pub is_keyword_only: SmallVec<[bool; 6]>,
    pub is_positional_only: SmallVec<[bool; 6]>,
}

/// Prototype for a nested function or class body.  Created at compile time,
/// instantiated into a `UserFunction` / class value at runtime via `MakeFunction`
/// / `MakeClass`.
#[derive(Debug, Clone)]
pub struct FnProto {
    pub name: String,
    /// Fully-qualified name (dotted path) computed at compile time.
    /// For a top-level `class Foo`, this equals `name`.
    /// For `class Outer: class Inner`, this equals `"Outer.Inner"`.
    /// For a class defined inside a function, this equals `"fn.<locals>.ClassName"`.
    /// Used by `MakeClass` to pre-populate the `__qualname__` register slot.
    pub qualname: String,
    /// Shared param metadata — `Rc::clone` in `MakeFunction` instead of four `Vec::clone`s.
    pub param_spec: Rc<FnParamSpec>,
    pub code: Rc<FnCode>,
    pub local_index: Rc<HashMap<String, Reg>>,
    /// Precomputed bind target for each parameter (parallel to
    /// `param_spec.names`), resolved once at compile time so the call path binds
    /// positional arguments by register index instead of hashing the parameter
    /// name on every call (issue #1918).  Shared via `Rc` onto every
    /// `UserFunction` built from this prototype.
    pub param_binds: Rc<Vec<pyrust_core::ParamBind>>,
    /// Precomputed self-reference register (the slot the recursive-call name
    /// binds to), or `None` when the function name has no local register slot
    /// or is a cell var.
    pub self_bind: Option<Reg>,
    /// Pre-computed set of local variable names (keys of `local_index`).
    /// Avoids an O(n) `HashSet` rebuild on every `MakeFunction` call.
    pub local_names: Rc<HashSet<String>>,
    pub global_names: Rc<HashSet<String>>,
    pub nonlocal_names: Rc<HashSet<String>>,
    pub is_pure: bool,
    /// Names for annotation registers passed to `MakeFunction`.  Parallel to
    /// the `annots_base..+annots_n` register window: `annotation_keys[i]` is
    /// the dict key (parameter name or `"return"`) for `R[annots_base + i]`.
    /// Empty when the function has no annotations.
    /// `SmallVec<[_; 4]>` avoids heap allocation for the common case of
    /// functions with four or fewer annotated parameters.
    pub annotation_keys: SmallVec<[String; 4]>,
    /// Docstring extracted from the first statement of the body if it is a
    /// bare string literal (`Stmt::Expr(Expr::Str(...))`), matching CPython's
    /// `co_consts[0]` / `__doc__` extraction.  `None` when no docstring
    /// is present.
    pub docstring: Option<String>,
    /// PEP 487 keyword argument names from the class header (e.g. `key` in
    /// `class Foo(Base, key=val)`).  Parallel to the kwarg value registers in
    /// `MakeClass` (`kwarg_base..kwarg_base+kwarg_n`).  Empty for functions.
    /// `SmallVec<[_; 2]>` avoids heap allocation for the typical case of
    /// zero to two keyword arguments in a class header.
    pub class_kwarg_names: SmallVec<[String; 2]>,
}

/// Resolve each parameter's static bind target once at compile time
/// (issue #1918).  A parameter that is captured as a cell var binds into the
/// local env by name; otherwise it binds into its register slot; a parameter
/// with no local slot (an unused variadic placeholder) binds to nothing.
///
/// Mirrors the per-call decision the binding loop used to make, hoisted out of
/// the hot path so the call only does a direct `match` on the precomputed slot.
pub fn compute_param_binds(
    param_spec: &FnParamSpec,
    local_index: &HashMap<String, Reg>,
    cell_vars: &[CellVar],
) -> Vec<pyrust_core::ParamBind> {
    use pyrust_core::ParamBind;
    param_spec
        .names
        .iter()
        .map(|name| {
            if cell_vars.iter().any(|c| c == name) {
                ParamBind::Cell
            } else if let Some(&reg) = local_index.get(name) {
                ParamBind::Reg(reg)
            } else {
                ParamBind::None
            }
        })
        .collect()
}

/// Resolve the self-reference register for recursive calls once at compile
/// time: the slot the function's own name binds to, unless that name is a cell
/// var or has no local register.
pub fn compute_self_bind(
    name: &str,
    local_index: &HashMap<String, Reg>,
    cell_vars: &[CellVar],
) -> Option<Reg> {
    if cell_vars.iter().any(|c| c == name) {
        None
    } else {
        local_index.get(name).copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Insn {
    /// R[dst] = consts[idx]
    LoadConst(Reg, u16),
    /// R[dst] = lookup name through env chain
    LoadGlobal(Reg, u16),
    /// names[name_idx] = R[src]  (write to module / enclosing env)
    StoreGlobal(u16, Reg),
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
    /// Like GetAttr but converts AttributeError to TypeError for the context manager
    /// protocol.  `missed_exit` is true when fetching `__exit__` (object already has
    /// `__enter__`), which appends " (missed __exit__ method)" to match CPython 3.12.
    GetAttrForWith(Reg, Reg, u16, bool),
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
    /// if !R[cond].truthy(): pc += offset
    JumpIfFalse(Reg, i32),
    /// if R[cond].truthy(): pc += offset
    JumpIfTrue(Reg, i32),
    /// if !(R[lhs] op R[rhs]): pc += offset  (integer fast path; avoids bool temp reg)
    CmpJumpIfFalse(Reg, BinaryOp, Reg, i32),
    /// if (R[lhs] op R[rhs]): pc += offset
    CmpJumpIfTrue(Reg, BinaryOp, Reg, i32),
    /// if !(R[lhs] op consts[idx]): pc += offset
    CmpJumpIfFalseConst(Reg, BinaryOp, u16, i32),
    /// if (R[lhs] op consts[idx]): pc += offset
    CmpJumpIfTrueConst(Reg, BinaryOp, u16, i32),
    /// R[func_reg] = call(R[func_reg], R[func_reg+1..func_reg+1+argc]); result in R[func_reg]
    Call(Reg, u8),
    /// Marks a call to a statically-pure callee (emitted for known-pure
    /// callees).  Executes identically to `Call`; the variant is retained so
    /// the optimizer can use the purity guarantee for DCE / TCO.  (The former
    /// result-memoization was removed in #1987.)
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
    /// return R[src]
    Return(Reg),
    /// return None
    ReturnNone,
    /// R[dst] = [R[base], R[base+1], ..., R[base+n-1]]
    BuildList(Reg, Reg, u32),
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
        after: u8,
        dst_base: Reg,
    },
    /// iters[slot] = iter_values(R[src])
    GetIter(u8, Reg),
    /// if iters[slot] exhausted: pc += offset; else R[dst] = next(iters[slot])
    ForIter(Reg, u8, i32),
    /// Integer counter for-range (register bound, signed step in consts[step_idx]).
    /// Semantics: next = R[var] + consts[step_idx]; if (next op R[stop]): R[var]=next; else: pc+=offset
    /// (op is Lt for step>0, Gt for step<0; initialise R[var] = start - step before loop)
    ForCountReg(Reg, BinaryOp, Reg, u16, i32),
    /// Integer counter for-range (constant bound).
    /// Same semantics as ForCountReg but stop comes from consts[stop_idx].
    ForCountConst(Reg, BinaryOp, u16, u16, i32),
    /// Integer counter for-range with stop and step inlined as i32.
    /// Fast path emitted by the compiler when both fit in i32; avoids the
    /// per-iteration consts-pool lookup that ForCountConst would do.
    /// Args: (var_reg, cmp_op, stop, step, jump_offset).
    ForCountConstInline(Reg, BinaryOp, i32, i32, i32),
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
        n: u8,
    },
    /// R[dst] = new UserFunction(fn_protos[proto_idx], defaults R[defs_base..+defs_n],
    ///          annotations R[annots_base..+annots_n], env=current).
    /// `annots_n == 0` means no annotations; `annots_base` is ignored in that case.
    /// The annotation names (parallel to the register values) are stored in
    /// `FnProto::annotation_keys`.
    MakeFunction(Reg, u8, Reg, u8, Reg, u8),
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
    /// if active_exception is NOT an instance of R[type_reg]: pc += offset.
    MatchExcept(Reg, i32),
    /// PEP 654 `except*` filter.
    ///
    /// Reads R[src_group] (must be a BaseExceptionGroup instance).  Filters
    /// its `.exceptions` for instances of R[type_reg].  If no exceptions
    /// match, jump to pc + offset.  Otherwise:
    ///   - R[matched_dst] = new sub-group containing only matching exceptions
    ///   - R[src_group]   = new sub-group containing only non-matching exceptions
    ///                       (or None if all exceptions were matched)
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
    MakeClass(Reg, u8, Reg, u8, u16, Reg, u8),
    /// R[dst] = TypeVar(name=consts[name_idx])
    /// PEP 695: construct a `TypeVar` object for a generic type parameter.
    MakeTypeVar(Reg, u16),
    /// R[dst] = TypeAliasType(name=consts[name_idx], value=R[value_reg], type_params=R[params_reg])
    /// PEP 695: construct a `TypeAliasType` object from a string name, the evaluated value
    /// expression, and a tuple of TypeVar objects (may be an empty tuple).
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
    /// Self-tail-call: reuse current frame by resetting params from R[args_base..args_base+nargs]
    /// and jumping back to pc=0.  Emitted by the optimizer when Call(r,n)+Return(r) is detected
    /// and the callee is the same function as the one being executed.  Falls back to a normal
    /// call+return if the callee turns out to be a different function at runtime.
    TailCall { args_base: Reg, nargs: u8 },
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
    /// Write the value of R[reg] into module_globals_dict[names[name_idx]] only
    /// when globals_accessed == true.  Emitted immediately after Move(reg, src)
    /// for every module-scope store so that globals() stays live without paying
    /// the dict-write cost in the common case (no globals() call in the script).
    /// When globals_accessed == false this instruction is a NOP.
    SyncModuleGlobal(Reg, u16),
    /// Remove names[name_idx] from both module env.values and module_globals_dict.
    /// Emitted at module scope for `del varname` when the name has a fastlocal
    /// register (in addition to DeleteLocal which clears the register).
    /// Does NOT raise NameError — the caller must detect unset register state.
    DeleteModuleGlobal(u16),
}

/// Per-call-site inline cache entry for `Insn::GetAttr` and `Insn::CallMethod`.
///
/// The cache is indexed by the instruction's position (`pc`) in `FnCode::insns`.
/// On every hit the cache checks that (a) the object is still a `PyInstance` of
/// the same class (`class_ptr` pointer equality), (b) that class has not been
/// mutated since the fill (`class_version` matches `PyClass::mutation_version`),
/// and (c) the global class-mutation epoch has not advanced (`epoch` matches
/// `pyrust_core::class_epoch()`).  Guard (c) catches mutations to ancestor
/// classes in the MRO (e.g. `Base.method = new_fn` when the cached instance is
/// a `Child(Base)` — `Child.mutation_version` is unchanged but the global epoch
/// advances, so the cache correctly misses).
///
/// When all guards hold the cached unbound value is rebound to the current
/// instance instead of repeating the `lookup_class_attr` chain walk.
///
/// The value stored is the **unbound** class-attribute value (i.e. what
/// `lookup_class_attr` returns).  `GetAttr` rebinds it on each hit; `CallMethod`
/// passes it to `invoke_class_method` which prepends the receiver.  Storing the
/// unbound value is critical for correctness: a `BoundMethod` captures a specific
/// receiver and would be stale when the same call site is executed for a different
/// object.
///
/// `Megamorphic` is set when two or more distinct class pointers are observed at
/// the same call site.  Once megamorphic the slot is never re-filled — the slow
/// path is cheaper than the overhead of re-checking on every execution.
///
/// # Safety
///
/// `class_ptr` is a raw pointer derived from `Rc::as_ptr(&Rc<RefCell<PyClass>>)`.
/// It is used **only** for identity comparison (pointer equality), never
/// dereferenced.  The interpreter is single-threaded and the `Rc` is kept alive
/// for the lifetime of the class (which outlives any `FnCode` that references
/// it), so the pointer is always valid for the lifetime of this cache entry.
#[derive(Clone)]
pub enum AttrCacheEntry {
    /// No observation yet — slot is uninitialised.
    Empty,
    /// Monomorphic: one class seen.  Validated by pointer + version + epoch check.
    ClassAttr {
        /// Raw pointer to the `RefCell<PyClass>` inside the class's `Rc`.
        /// Used for O(1) identity comparison; never dereferenced.
        class_ptr: *const (),
        /// Value of `PyClass::mutation_version` when the cache was filled.
        class_version: u64,
        /// Value of `pyrust_core::class_epoch()` when the cache was filled.
        /// Any mutation to any class — including ancestor classes in the MRO —
        /// advances the global epoch, causing this guard to fail.
        epoch: u64,
        /// The unbound class-attr value from `lookup_class_attr`.
        value: Value,
    },
    /// Monomorphic instance-attribute read cache (mirrors CPython's
    /// `LOAD_ATTR_INSTANCE_VALUE`).  Filled when a `GetAttr` site resolves to
    /// the instance `__dict__` with **no data descriptor** shadowing the name
    /// on the class MRO, no custom `__getattribute__`, and no numeric-tower /
    /// `__slots__` complications.  On a hit (same class pointer + version +
    /// epoch) the VM probes `inst.attrs.get(name)` directly, skipping the full
    /// `lookup_class_attr` MRO walk in `get_attr_instance_raw`.
    ///
    /// Correctness: the cached fact is "no data descriptor named `name` exists
    /// on this class's MRO".  That fact is invalidated by the existing
    /// `mutation_version` (this class mutated) + `class_epoch` (any ancestor
    /// mutated) guards.  The instance's class pointer is also part of the guard,
    /// so `__class__` reassignment (#1957/#2102) → different pointer → miss.
    /// If on a hit the name is *not* in the instance dict, the VM falls through
    /// to the slow path (the name may resolve to a method / non-data descriptor
    /// / `__getattr__`), so a missing attribute is always handled correctly.
    InstanceAttr {
        class_ptr: *const (),
        class_version: u64,
        epoch: u64,
    },
    /// Monomorphic instance-attribute write cache (mirrors CPython's
    /// `STORE_ATTR_INSTANCE_VALUE`).  Filled when a `SetAttr` site resolves to a
    /// plain instance `__dict__` write: no `__setattr__` override, no `__set__`
    /// data descriptor on the MRO, not bare `object()`, not an exception slot,
    /// no `__slots__` restriction, and the name is not `__class__` / `__dict__`.
    /// On a hit the VM inserts straight into `inst.attrs`, skipping the
    /// `lookup_class_attr` MRO walk in `assign_attr_instance`.  Same invalidation
    /// as `InstanceAttr`.
    SetInstanceAttr {
        class_ptr: *const (),
        class_version: u64,
        epoch: u64,
    },
    /// More than one class seen at this site — disable caching.
    Megamorphic,
}

/// Operand-type tag for the BinOp inline cache.
///
/// Classifies a `(lhs, rhs)` pair at a BinOp call site into one of four
/// categories.  The cache transitions from `Counting` → `Specialized` (after
/// [`BINOP_SPEC_THRESHOLD`] observations of the same tag) → `Megamorphic`
/// (on a tag mismatch).  A Megamorphic site permanently bypasses the cache
/// and falls straight through to `eval_binary`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinopTypeTag {
    Int,
    Float,
    Str,
    Other,
}

/// Adaptive inline-cache state for a single BinOp instruction site.
///
/// Indexed by `pc` inside [`FnCode::binop_cache`] — one entry per instruction
/// position.  Only slots for `BinOp` instructions are ever advanced past
/// `Empty`; all other positions remain `Empty` for the lifetime of the
/// `FnCode`.  (`BinOpInPlace`, `BinOpConst`, and `BinOpImm` use only the
/// unconditional int-int fast path and do not consult the adaptive cache.)
#[derive(Debug, Clone, Copy)]
pub(crate) enum BinOpCacheEntry {
    /// No observation yet.
    Empty,
    /// Seen `count` observations all with the same `tag`.
    Counting { tag: BinopTypeTag, count: u8 },
    /// Specialised: every observation so far matched `tag`.
    Specialized(BinopTypeTag),
    /// Two or more distinct tags observed — skip the cache.
    Megamorphic,
}

/// Number of same-type observations required before a BinOp site transitions
/// from `Counting` to `Specialized`.
pub(crate) const BINOP_SPEC_THRESHOLD: u8 = 8;

// SAFETY: pyrust's interpreter is single-threaded.  `AttrCacheEntry` is only
// ever read or written inside `run_bytecode_inner`, which runs on one thread.
// The raw `class_ptr` is never sent across threads.
unsafe impl Send for AttrCacheEntry {}
unsafe impl Sync for AttrCacheEntry {}

impl std::fmt::Debug for AttrCacheEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttrCacheEntry::Empty => write!(f, "Empty"),
            AttrCacheEntry::ClassAttr {
                class_ptr,
                class_version,
                epoch,
                ..
            } => {
                write!(f, "ClassAttr({class_ptr:?}, v{class_version}, e{epoch})")
            }
            AttrCacheEntry::InstanceAttr {
                class_ptr,
                class_version,
                epoch,
            } => {
                write!(f, "InstanceAttr({class_ptr:?}, v{class_version}, e{epoch})")
            }
            AttrCacheEntry::SetInstanceAttr {
                class_ptr,
                class_version,
                epoch,
            } => {
                write!(
                    f,
                    "SetInstanceAttr({class_ptr:?}, v{class_version}, e{epoch})"
                )
            }
            AttrCacheEntry::Megamorphic => write!(f, "Megamorphic"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FnCode {
    pub(crate) insns: Vec<Insn>,
    /// 1-based source line number for each instruction, parallel to `insns`.
    /// A value of 0 means "unknown / same as the previous instruction".  Set by
    /// the compiler when per-statement line information is available (i.e. when
    /// the script was compiled with line tracking enabled).  Used by the VM to
    /// update the current-line counter when building tracebacks.
    pub(crate) lineno_table: Vec<u32>,
    /// Constant pool (literals used in the function body)
    pub(crate) consts: Vec<Value>,
    /// Name pool (global variable names and attribute names)
    pub(crate) names: Vec<String>,
    /// Number of registers needed (locals + max temporaries)
    pub(crate) num_regs: u32,
    /// Number of iterator slots needed
    pub(crate) num_iters: u8,
    /// Number of local variable slots (registers 0..num_locals are locals; the rest are temps)
    pub(crate) num_locals: u32,
    /// Nested function / class body prototypes
    pub(crate) fn_protos: Vec<FnProto>,
    /// Variables captured by nested functions (stored in env, not registers).
    /// `SmallVec<[_; 4]>` avoids heap allocation for the common case of
    /// functions with four or fewer captured variables.
    pub(crate) cell_vars: SmallVec<[CellVar; 4]>,
    /// True if this function body contains at least one `Yield` instruction.
    /// The VM creates a generator object instead of executing immediately.
    pub(crate) is_generator: bool,
    /// True when this function was compiled as a direct method inside a class
    /// body (i.e., the enclosing `Compiler` had `is_class_body = true`).
    /// Zero-argument `super()` is valid only in such functions — not in plain
    /// functions or in functions nested inside methods.  Used by
    /// `resolve_zero_arg_super` to identify the correct enclosing frame.
    pub(crate) is_class_method: bool,
    /// Per-instruction inline cache for `GetAttr` and `CallMethod`.
    ///
    /// Indexed by instruction position (`pc`) — same length as `insns`.
    /// Only entries at `GetAttr` / `CallMethod` positions are ever populated;
    /// all other entries remain `AttrCacheEntry::Empty`.
    ///
    /// `RefCell` provides interior mutability so the cache can be updated
    /// through a shared `Rc<FnCode>` during dispatch without requiring an
    /// exclusive borrow of the enclosing `FnCode`.
    pub(crate) attr_cache: RefCell<Vec<AttrCacheEntry>>,
    /// Per-name inline cache for `LoadGlobal`.
    ///
    /// Indexed by `name_idx` (the second operand of `Insn::LoadGlobal`).
    /// Each entry is `(cached_env_version, cached_value)`.  A hit requires
    /// `cached_env_version == interpreter.global_env_version` (module env
    /// unchanged since last lookup).  Builtins are cached with the current
    /// `global_env_version` so that a subsequent module-level assignment of
    /// the same name (e.g. `len = my_fn`) invalidates the entry correctly.
    /// Shared across all invocations of this function via `Rc<FnCode>` — the
    /// cache is function-granular, not call-granular.
    pub(crate) global_cache: RefCell<Vec<(u32, Value)>>,
    /// Adaptive inline cache for binary operations (PEP 659 style).
    ///
    /// Indexed by instruction position (`pc`) — same length as `insns`.
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
}

/// Initial version stored in every `global_cache` slot at construction
/// time.  Chosen so that it never matches a real `global_env_version`
/// value (which starts at 0 and increments), guaranteeing a cache miss
/// on the very first `LoadGlobal` execution.
pub(crate) const GLOBAL_CACHE_EMPTY: u32 = u32::MAX - 1;
