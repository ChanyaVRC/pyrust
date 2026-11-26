//! Register operands and the executable instruction schema.

use crate::ast::{BinaryOp, UnaryOp};

pub type Reg = u32;

/// Index into [`crate::bytecode::FnCode::descs`] for cold wide-operand
/// instructions whose payload lives out-of-line so [`Insn`] stays 16 bytes.
pub type DescIdx = u32;

/// Sentinel `Reg` used by [`Insn::LoadClassName`] when a class-body LOAD_NAME
/// has no compiler-allocated fastlocal slot. No real register can have this
/// value.
pub const NO_CLASS_LOCAL: Reg = Reg::MAX;

/// Sentinel `Reg` marking an absent `**kw` mapping in [`CallExArgsDesc`].
/// No real register is ever allocated at `u32::MAX`, so it can never collide
/// with a live operand.
pub const NO_KWARGS: Reg = Reg::MAX;

/// Sentinel `Reg` marking an absent `*args` splat in [`CallExArgsDesc`]
/// (double-splat `f(<pos…>, **d)` calls).
pub const NO_ARGS_SPLAT: Reg = Reg::MAX;

/// Sentinel `Reg` marking an absent assertion message in [`Insn::RaiseAssert`]
/// (`assert cond` with no message expression).
pub const NO_ASSERT_MSG: Reg = Reg::MAX;

/// Sentinel `Reg` marking an absent explicit `metaclass=` in [`MakeClassDesc`].
pub const NO_METACLASS: Reg = Reg::MAX;

/// Compile-time knowledge about the exact key-table kind for `BuildDict`.
/// Dynamic expressions remain `Unknown` and receive CPython's runtime key
/// scan; literals with a statically decisive key avoid that repeated scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DictKeyKindHint {
    Unknown,
    Unicode,
    General,
}

/// The exact machine-int range cursor state [`Insn::JumpIfIterNotIntRangeExact`]
/// admits. Stored in [`crate::bytecode::FnCode::descs`] so the rare closed-form
/// guard does not widen or over-align `Insn`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IntRangeExactGuard {
    /// Iterator slot the guard inspects.
    pub slot: u8,
    /// Required cursor position — the range's `start`, so a matching slot has
    /// not yet been stepped.
    pub start: i64,
    pub stop: i64,
    pub step: i64,
}

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

/// Out-of-line descriptor for [`Insn::CallInlineBinOp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallInlineBinOpDesc {
    pub callee: Reg,
    pub dst: Reg,
    pub a: Reg,
    pub op: BinaryOp,
    pub b: Reg,
    pub proto: u16,
}

/// Out-of-line descriptor for [`Insn::CallMethodExpanded`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallMethodExpandedDesc {
    pub dst: Reg,
    pub obj: Reg,
    pub name_idx: u16,
    pub pos_list: Reg,
    pub kw_dict: Reg,
}

/// Out-of-line descriptor for [`Insn::CallExArgs`].
///
/// Covers both double-splat calls (`args_splat == NO_ARGS_SPLAT`, `nkw == 0`)
/// and positional-splat calls (`f(<pos…>, *args, kw=v…[, **kw])`, with
/// `kwargs == NO_KWARGS` when no `**kw` is present).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallExArgsDesc {
    pub func: Reg,
    pub npos: u8,
    pub nkw: u8,
    pub kwnames_idx: u16,
    pub args_splat: Reg,
    pub kwargs: Reg,
}

/// Out-of-line descriptor for [`Insn::CallMethodKw`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallMethodKwDesc {
    pub dst: Reg,
    pub obj: Reg,
    pub name_idx: u16,
    pub args_base: Reg,
    pub total: u8,
    pub nkw: u8,
    pub kwnames_idx: u16,
}

/// Out-of-line descriptor for [`Insn::MatchClassPositional`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MatchClassPositionalDesc {
    /// First register of the result window: `n` positional values, then `nkw`
    /// keyword-attribute values, then one trailing boolean "all attributes
    /// found" flag (`false` means the class pattern fails to match).
    pub dst_base: Reg,
    pub subj: Reg,
    pub cls: Reg,
    pub n: u32,
    /// Number of keyword attribute names in `consts[kwnames_idx]`.
    pub nkw: u32,
    pub kwnames_idx: u16,
}

impl MatchClassPositionalDesc {
    /// Total registers written starting at `dst_base` (values + found flag).
    #[inline]
    pub fn window_len(&self) -> u32 {
        self.n + self.nkw + 1
    }

    /// Register receiving the "all attributes found" flag.
    #[inline]
    pub fn found_reg(&self) -> Reg {
        self.dst_base + self.n + self.nkw
    }
}

/// Out-of-line descriptor for [`Insn::MakeFunction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MakeFunctionDesc {
    pub dst: Reg,
    pub proto_idx: u16,
    pub defs_base: Reg,
    pub defs_n: u32,
    pub annots_base: Reg,
    pub annots_n: u32,
}

/// Out-of-line descriptor for [`Insn::MatchExceptStar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MatchExceptStarDesc {
    pub type_reg: Reg,
    pub src_group: Reg,
    pub matched_dst: Reg,
}

/// Out-of-line descriptor for [`Insn::MakeClass`].
///
/// `meta_reg == NO_METACLASS` selects default `type` class creation; any other
/// register selects metaclass-driven creation (`__prepare__` + metaclass call).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MakeClassDesc {
    pub dst: Reg,
    pub proto_idx: u16,
    pub bases_base: Reg,
    pub bases_n: u32,
    pub name_idx: u16,
    pub kwarg_base: Reg,
    pub kwarg_n: u32,
    pub meta_reg: Reg,
}

/// Out-of-line descriptor for [`Insn::ListExtendCall`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListExtendCallDesc {
    pub list: Reg,
    pub src: Reg,
    pub name: KwCallName,
}

/// Out-of-line descriptor for [`Insn::DictMergeKwCall`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DictMergeKwCallDesc {
    pub dict: Reg,
    pub src: Reg,
    pub name: KwCallName,
}

/// Out-of-line descriptor for [`Insn::SetItemKwCall`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SetItemKwCallDesc {
    pub dict: Reg,
    pub key: Reg,
    pub val: Reg,
    pub name: KwCallName,
}

/// Side-table entry stored in [`crate::bytecode::FnCode::descs`] for cold
/// wide-operand instructions so [`Insn`] stays compact (`16 B`, `align = 4`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InsnDesc {
    IntRangeExactGuard(IntRangeExactGuard),
    CallInlineBinOp(CallInlineBinOpDesc),
    CallMethodExpanded(CallMethodExpandedDesc),
    CallExArgs(CallExArgsDesc),
    CallMethodKw(CallMethodKwDesc),
    MatchClassPositional(MatchClassPositionalDesc),
    MakeFunction(MakeFunctionDesc),
    MatchExceptStar(MatchExceptStarDesc),
    MakeClass(MakeClassDesc),
    ListExtendCall(ListExtendCallDesc),
    DictMergeKwCall(DictMergeKwCallDesc),
    SetItemKwCall(SetItemKwCallDesc),
}

impl InsnDesc {
    #[inline(always)]
    pub(crate) fn as_int_range_exact_guard(&self) -> &IntRangeExactGuard {
        match self {
            Self::IntRangeExactGuard(d) => d,
            _ => unreachable!("descriptor mismatch: expected IntRangeExactGuard"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_call_inline_binop(&self) -> &CallInlineBinOpDesc {
        match self {
            Self::CallInlineBinOp(d) => d,
            _ => unreachable!("descriptor mismatch: expected CallInlineBinOp"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_call_method_expanded(&self) -> &CallMethodExpandedDesc {
        match self {
            Self::CallMethodExpanded(d) => d,
            _ => unreachable!("descriptor mismatch: expected CallMethodExpanded"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_call_ex_args(&self) -> &CallExArgsDesc {
        match self {
            Self::CallExArgs(d) => d,
            _ => unreachable!("descriptor mismatch: expected CallExArgs"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_call_method_kw(&self) -> &CallMethodKwDesc {
        match self {
            Self::CallMethodKw(d) => d,
            _ => unreachable!("descriptor mismatch: expected CallMethodKw"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_match_class_positional(&self) -> &MatchClassPositionalDesc {
        match self {
            Self::MatchClassPositional(d) => d,
            _ => unreachable!("descriptor mismatch: expected MatchClassPositional"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_make_function(&self) -> &MakeFunctionDesc {
        match self {
            Self::MakeFunction(d) => d,
            _ => unreachable!("descriptor mismatch: expected MakeFunction"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_match_except_star(&self) -> &MatchExceptStarDesc {
        match self {
            Self::MatchExceptStar(d) => d,
            _ => unreachable!("descriptor mismatch: expected MatchExceptStar"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_make_class(&self) -> &MakeClassDesc {
        match self {
            Self::MakeClass(d) => d,
            _ => unreachable!("descriptor mismatch: expected MakeClass"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_list_extend_call(&self) -> &ListExtendCallDesc {
        match self {
            Self::ListExtendCall(d) => d,
            _ => unreachable!("descriptor mismatch: expected ListExtendCall"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_dict_merge_kw_call(&self) -> &DictMergeKwCallDesc {
        match self {
            Self::DictMergeKwCall(d) => d,
            _ => unreachable!("descriptor mismatch: expected DictMergeKwCall"),
        }
    }

    #[inline(always)]
    pub(crate) fn as_set_item_kw_call(&self) -> &SetItemKwCallDesc {
        match self {
            Self::SetItemKwCall(d) => d,
            _ => unreachable!("descriptor mismatch: expected SetItemKwCall"),
        }
    }
}

/// Append `desc` to a function's descriptor pool and return its [`DescIdx`].
#[inline]
pub(crate) fn push_desc(descs: &mut Vec<InsnDesc>, desc: InsnDesc) -> DescIdx {
    let idx = DescIdx::try_from(descs.len()).expect("FnCode descriptor pool exceeded u32::MAX");
    descs.push(desc);
    idx
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Insn {
    /// R[dst] = consts[idx]
    LoadConst(Reg, u16),
    /// R[dst] = lookup name through env chain
    LoadGlobal(Reg, u16),
    /// Class-body LOAD_NAME. Before `f_locals` escapes this clones the
    /// compiler-known fastlocal `R[local]`, or goes directly to module/builtins
    /// when `local == NO_CLASS_LOCAL`; afterwards it first reads the live class
    /// namespace dict. A missing binding falls through to module/builtins.
    LoadClassName(Reg, Reg, u16),
    /// names[name_idx] = R[src]  (write to module / enclosing env)
    StoreGlobal(u16, Reg),
    /// R[dst] = read a **function-scope cell variable** (own cell or captured
    /// freevar / `nonlocal`) directly from `cells[cell_idx]` in O(1)
    /// (issues #452 / #2339 / #2348).  The second operand is an index into the
    /// active frame's `CellSlot` slice (parallel to `FnCode::cell_slots`), NOT
    /// an index into `FnCode::names`.
    LoadCell(Reg, u16),
    /// `cells[cell_idx] = R[src]` — write a **function-scope cell variable**
    /// (own cell or `nonlocal`) directly into `cells[cell_idx]` in O(1)
    /// (issues #452 / #2339 / #2348).  The first operand is an index into the
    /// active frame's `CellSlot` slice, NOT `FnCode::names`.
    StoreCell(u16, Reg),
    /// Clear `cells[cell_idx]` (issues #452 / #2348).  When `raise_if_missing`
    /// is true (source `del x`), an already-empty cell raises
    /// `UnboundLocalError` (own cell) or free-variable `NameError` (freevar).
    /// When false (PEP 3110 `except E as e:` cleanup), clearing an already-empty
    /// cell is an idempotent no-op.
    DeleteCell(u16, bool),
    /// Class-body `LOAD_CLASSDEREF` (issue #452): read a variable captured from
    /// an enclosing function scope inside a class body.  Checks the live class
    /// namespace dict first (when `f_locals` / metaclass `__prepare__` mapping
    /// has materialized), then reads `cells[cell_idx]`, raising free-variable
    /// `NameError` if empty (never falling back to module globals).
    LoadClassDeref(Reg, u16),
    /// R[dst] = None
    LoadNone(Reg),
    /// R[dst] = R[src]
    Move(Reg, Reg),
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
    /// Deopting form of `GetItem` for the int-loop versioning pass: when
    /// R[obj] is a canonical `list`/`tuple`, R[idx] is a built-in integer in
    /// `0..len`, and the element read out is itself an `int`,
    /// `R[dst] = R[obj][R[idx]]`; otherwise jump and leave R[dst] unchanged.
    ///
    /// The element-type half is the same per-iteration fact a `JumpIfNotInt`
    /// on R[dst] would establish, folded in because both share this
    /// instruction as their deopt target.
    ///
    /// Emitted only inside an out-of-line specialized loop copy, whose jump
    /// target is a side-exit stub that flushes the deferred module syncs and
    /// resumes the original loop at this very subscript.  Reading a canonical
    /// sequence element is a pure, non-reentrant copy, so re-executing the
    /// original `GetItem` after a deopt observes exactly the same state — and
    /// negative, out-of-range, non-int, `dict`, user-`__getitem__`, and unset
    /// operands all raise (or run user code) on the original path with their
    /// own source line and PEP 657 caret span.
    ///
    /// The side-exit offset is `i16` so this hot specialized opcode remains
    /// inline within the 16-byte `Insn` budget (`INT_LOOP_MAX_REGION = 48`
    /// keeps the stub distance well within `i16::MAX`).
    GetItemSeqIntOrExit(Reg, Reg, Reg, i16),
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
    /// `name_idx` is the real index into `FnCode::names`, including for
    /// compiler-generated PEP 3110 cleanup so an exposed class namespace can
    /// remove the corresponding key. `raise_if_missing` is true for a source
    /// `del name`; it is false for except-as cleanup, which is idempotent when
    /// nested handlers reuse the same local slot.
    DeleteLocal(Reg, u16, bool),
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
    JumpIfIterNotIntRange(u8, i32),
    /// Jump when iterator slot `slot` does not currently hold the canonical
    /// list/tuple index cursor.
    ///
    /// The sibling entry guard of `JumpIfIterNotIntRange` for the second
    /// iterator kind the versioning pass admits: stepping an index cursor over
    /// a canonical `list`/`tuple` clones one element and bumps a counter,
    /// invoking no Python-visible protocol.  Unlike a machine-int range the
    /// element *type* is a per-iteration fact, so the specialized copy pairs
    /// this entry guard with a `JumpIfNotInt` side exit on the loop variable.
    JumpIfIterNotIndexedSeq(u8, i32),
    /// Jump unless the iterator slot holds the canonical machine-int `range`
    /// cursor in *exactly* the state the descriptor guard describes — same
    /// `stop`, same `step`, and a cursor still parked at `start`.
    ///
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::IntRangeExactGuard`).
    JumpIfIterNotIntRangeExact(DescIdx, i32),
    /// Jump when R[reg] is not the canonical built-in `len`.
    ///
    /// The entry guard for a `while i < len(xs):` loop copy that reads the
    /// sequence length natively instead of calling `len`.  The optimizer only
    /// ever proposes that copy after seeing `LoadGlobal len` in the header, but
    /// a module-level `def len` / `len = …` / `builtins.len = …` rebinding —
    /// or a namespace-mirror write between loop entries (rule 27) — makes the
    /// name resolve to something else entirely.  Like
    /// `JumpIfIterNotIntRangeExact`, the guard reads the *value* the header
    /// just loaded rather than trusting the name, so any rebinding simply runs
    /// the original loop with its real call.
    JumpIfNotBuiltinLen(Reg, i32),
    /// Deopting native `len()` for the int-loop versioning pass: when R[seq] is
    /// a canonical `list`/`tuple`/`str`/`bytes`, `R[dst] = len(R[seq])`;
    /// otherwise jump and leave R[dst] unchanged.
    ///
    /// Emitted in the entry guard block for an out-of-line specialized loop,
    /// after the header's `LoadGlobal len` and its value guard. The optimizer
    /// reuses the result only after the typed region-admission policy proves
    /// that no copied operation can resize or replace the sequence. Mutating
    /// and protocol-reaching bodies retain the original per-iteration call. On any
    /// other operand shape (user `__len__`, `dict`, `set`, a `range` whose
    /// length overflows `i64`, an unset slot), the jump resumes the original
    /// loop at its `LoadGlobal`, so the real `len` owns the raise, protocol
    /// dispatch, and diagnostics.
    LenSeqOrExit(Reg, Reg, i32),
    /// Guarded numeric leaf-call inlining site (emitted only by the
    /// optimizer, always immediately before the original call sequence it
    /// specializes, which stays in place as the deopt path).
    ///
    /// Operands live in `FnCode::descs[idx]` (`InsnDesc::CallInlineBinOp`);
    /// the skip offset stays inline so control-flow patching needs no table
    /// lookup.
    CallInlineBinOp(DescIdx, i32),
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
    /// R[dst] = R[obj].names[name_idx](*R[pos_list], **R[kw_dict]).
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::CallMethodExpanded`).
    CallMethodExpanded(DescIdx),
    /// Keyword-argument call with no `*args` / `**kwargs` splats (issue #2382).
    /// Mirrors CPython's `KW_NAMES` + `CALL`: arguments are laid out
    /// contiguously in registers — `total` values in `R[func+1 .. func+1+total]`
    /// — and the *last* `nkw` of them are keyword arguments whose names are the
    /// strings in the constant-pool tuple `consts[kwnames_idx]` (in order).  The
    /// first `total - nkw` are positional.  Result is written back to `R[func]`.
    CallKw {
        func: Reg,
        total: u8,
        nkw: u8,
        kwnames_idx: u16,
    },
    /// Splat-expansion call `f(<pos…>, [*args][, kw=v…][, **kw])`.
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::CallExArgs`).
    ///
    /// Consolidates double-splat calls (`args_splat == NO_ARGS_SPLAT`,
    /// `nkw == 0`) and positional-splat calls (`args_splat != NO_ARGS_SPLAT`,
    /// optional `kwargs != NO_KWARGS`) under one opcode.
    CallExArgs(DescIdx),
    /// Keyword-argument method call `R[obj].name(<pos…>, k=v…)` with no
    /// `*args` / `**kwargs` splats (issue #2392).
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::CallMethodKw`).
    CallMethodKw(DescIdx),
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
    BuildDict(Reg, Reg, u32, DictKeyKindHint),
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
    /// raise AssertionError(R[msg]), or AssertionError() when `msg == NO_ASSERT_MSG`.
    RaiseAssert(Reg),
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
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::MatchClassPositional`).
    MatchClassPositional(DescIdx),
    /// R[dst] = new UserFunction(fn_protos[proto_idx], defaults, annotations, env=current).
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::MakeFunction`).
    MakeFunction(DescIdx),
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
    /// Register operands live in `FnCode::descs[idx]` (`InsnDesc::MatchExceptStar`);
    /// the no-match jump offset stays inline.
    MatchExceptStar(DescIdx, i32),
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
    /// R[dst] = create class (plain or metaclass-driven when `meta_reg != NO_METACLASS`).
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::MakeClass`).
    MakeClass(DescIdx),
    /// R[dst] = TypeVar / ParamSpec / TypeVarTuple (name=consts[name_idx], kind)
    /// PEP 695: construct an (initially unbounded) type parameter object for a
    /// generic def/class/alias (`TypeVar` for `T`, `ParamSpec` for `**P`,
    /// `TypeVarTuple` for `*Ts`).  For `TypeVar`, `__bound__` is `None` and
    /// `__constraints__` is `()`; any bound/constraint clause is populated
    /// lazily, after every type parameter of the enclosing def/class/alias is
    /// in scope, via `SetTypeVarAttr` (see `Compiler::emit_typevar_bound`).
    MakeTypeVar(Reg, u16, crate::ast::TypeParamKind),
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
    /// Call-context list extension for one `*` operand.
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::ListExtendCall`).
    ListExtendCall(DescIdx),
    /// R[dict].update(R[src])  — in-place dict merge
    DictUpdate(Reg, Reg),
    /// R[dict].merge(R[src]) for a `**d` keyword splat in a *call* context.
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::DictMergeKwCall`).
    DictMergeKwCall(DescIdx),
    /// R[dict][R[key]] = R[val] for a named (`kw=v`) argument in a *call* context.
    /// Payload lives in `FnCode::descs[idx]` (`InsnDesc::SetItemKwCall`).
    SetItemKwCall(DescIdx),
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
    /// present), and mirrors the value into any materialized class-frame
    /// namespace, so both final attrs and later LOAD_NAME reads stay coherent.
    RecordClassStore(Reg),
    /// Record a runtime del C.name for class-namespace insertion order.
    /// Emitted only inside a class body, immediately after DeleteLocal(slot).
    /// The VM removes slot from the active class-store-order list so that the
    /// final attrs drop the entry while preserving remaining order. DeleteLocal
    /// itself removes the binding from a materialized live namespace.
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

#[cfg(test)]
mod layout_tests {
    use super::{Insn, InsnDesc};

    #[test]
    fn insn_layout_stays_16_bytes_align_4() {
        assert_eq!(
            std::mem::size_of::<Insn>(),
            16,
            "Insn must stay 16 bytes (issue #3161)"
        );
        assert_eq!(
            std::mem::align_of::<Insn>(),
            4,
            "Insn must stay 4-byte aligned (issue #3161)"
        );
        assert!(
            std::mem::size_of::<InsnDesc>() <= 40,
            "InsnDesc should stay compact (was {} B)",
            std::mem::size_of::<InsnDesc>()
        );
    }
}
