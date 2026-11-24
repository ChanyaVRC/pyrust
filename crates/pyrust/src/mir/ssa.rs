//! CFG construction and SSA promotion (`FnCode` -> `MirFn`).
//!
//! Converts compiler-emitted register bytecode into true Cranelift-style SSA
//! MIR with block parameters as phi nodes (issue #457):
//! 1. Splits `FnCode::insns` into basic blocks with explicit CFG edges and
//!    `SetupExcept`/`PopExcept` exception-depth tracking.
//! 2. Computes register liveness and strict `definitely_bound` dataflow so
//!    block parameters are inserted only where a register is guaranteed defined
//!    along every incoming path (preventing spurious `UnboundLocalError` moves).
//! 3. Uses write-through `StoreLocal` for canonical local registers
//!    (`0..num_locals`) and exception-escaping temps so `locals()`, `f_locals`,
//!    tracebacks, `SyncModuleGlobal`, `RecordClassStore`, and `SetupExcept`
//!    handlers always observe exact canonical register state.
//! 4. Places block parameters on dominance frontiers (Cytron et al.) and runs a
//!    single dominator-tree renaming pass to produce verified SSA form.

use std::collections::HashSet;

use smallvec::SmallVec;

use crate::ast::CaretSpan;
use crate::bytecode::{FnCode, Insn, NO_CLASS_LOCAL, NO_KWARGS, Reg};
use crate::value::Value;

use super::ir::{BlockId, MirBlock, MirFn, MirInst, MirOp, Terminator, Val, infer_mir_types};
use super::verify::DominatorTree;

/// Conservative false-whitelist matching `optimizer/effects.rs::may_invalidate_named_locals`.
#[inline]
fn insn_may_invalidate_named_locals(insn: &Insn) -> bool {
    use Insn::*;
    !matches!(
        insn,
        LoadConst(..)
            | LoadGlobal(..)
            | LoadClassName(..)
            | LoadCell(..)
            | LoadNone(..)
            | LoadNoneRange { .. }
            | Move(..)
            | CopyReg(..)
            | CheckLocal(..)
            | BuildList(..)
            | BuildListReserve(..)
            | BuildTuple(..)
            | BuildString(..)
            | BuildSlice(..)
            | MakeFunction(..)
    )
}

#[inline]
fn raw_jump_offset(insn: &Insn) -> Option<i32> {
    match insn {
        Insn::Jump(k)
        | Insn::JumpIfFalse(_, k)
        | Insn::JumpIfTrue(_, k)
        | Insn::CmpJumpIfFalse(_, _, _, k)
        | Insn::CmpJumpIfTrue(_, _, _, k)
        | Insn::CmpJumpIfFalseConst(_, _, _, k)
        | Insn::CmpJumpIfTrueConst(_, _, _, k)
        | Insn::ForIter(_, _, k)
        | Insn::SetupExcept(k)
        | Insn::MatchExcept(_, k)
        | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
        _ => None,
    }
}

#[inline]
fn ends_raw_block(insn: &Insn) -> bool {
    matches!(
        insn,
        Insn::Jump(_)
            | Insn::JumpIfFalse(..)
            | Insn::JumpIfTrue(..)
            | Insn::CmpJumpIfFalse(..)
            | Insn::CmpJumpIfTrue(..)
            | Insn::CmpJumpIfFalseConst(..)
            | Insn::CmpJumpIfTrueConst(..)
            | Insn::ForIter(..)
            | Insn::SetupExcept(_)
            | Insn::MatchExcept(..)
            | Insn::MatchExceptStar(..)
            | Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseValue(_)
            | Insn::RaiseExceptStarResidual(_)
            | Insn::RaiseFrom(..)
            | Insn::RaiseReRaise
            | Insn::RaiseAssert(_)
            | Insn::RaiseAssertNoMsg
    )
}

/// Raw basic block extracted directly from `FnCode::insns` before SSA renaming.
struct RawBlock {
    start_pc: usize,
    /// Non-terminator instructions (inclusive range `[start_pc, body_end)`).
    body_end: usize,
    /// Index of the terminator instruction in `insns`, if the block ends with one.
    term_pc: Option<usize>,
    /// Outgoing block indices: `(target, is_handler_edge, is_for_iter_body, is_except_star_matched)`.
    succs: SmallVec<[RawEdge; 2]>,
    preds: Vec<usize>,
    exc_depth: u16,
    is_handler: bool,
}

#[derive(Clone, Copy)]
struct RawEdge {
    target: usize,
    kind: RawEdgeKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RawEdgeKind {
    Normal,
    SetupExceptHandler,
    ForIterBody,
    ExceptStarMatched,
}

/// Compact bitset over register indices `0..num_regs`.
#[derive(Clone, PartialEq, Eq)]
struct RegBits {
    words: Vec<u64>,
    nbits: usize,
}

impl RegBits {
    fn empty(nbits: usize) -> Self {
        Self {
            words: vec![0; nbits.div_ceil(64)],
            nbits,
        }
    }

    fn all(nbits: usize) -> Self {
        let mut words = vec![u64::MAX; nbits.div_ceil(64)];
        if !nbits.is_multiple_of(64)
            && let Some(last) = words.last_mut()
        {
            *last = (1u64 << (nbits % 64)) - 1;
        }
        Self { words, nbits }
    }

    #[inline]
    fn contains(&self, r: Reg) -> bool {
        let idx = r as usize;
        idx < self.nbits && ((self.words[idx / 64] >> (idx % 64)) & 1) != 0
    }

    #[inline]
    fn insert(&mut self, r: Reg) {
        let idx = r as usize;
        if idx < self.nbits {
            self.words[idx / 64] |= 1u64 << (idx % 64);
        }
    }

    #[inline]
    fn remove(&mut self, r: Reg) {
        let idx = r as usize;
        if idx < self.nbits {
            self.words[idx / 64] &= !(1u64 << (idx % 64));
        }
    }

    fn clear_below(&mut self, limit: Reg) {
        let lim = (limit as usize).min(self.nbits);
        let full_words = lim / 64;
        for w in self.words.iter_mut().take(full_words) {
            *w = 0;
        }
        let rem = lim % 64;
        if rem != 0 && full_words < self.words.len() {
            self.words[full_words] &= !((1u64 << rem) - 1);
        }
    }

    fn intersect_with(&mut self, other: &Self) {
        for (a, b) in self.words.iter_mut().zip(other.words.iter()) {
            *a &= *b;
        }
    }

    fn union_with(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for (a, b) in self.words.iter_mut().zip(other.words.iter()) {
            let next = *a | *b;
            if next != *a {
                *a = next;
                changed = true;
            }
        }
        changed
    }

    fn iter(&self) -> impl Iterator<Item = Reg> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, &w)| {
            let mut rem = w;
            std::iter::from_fn(move || {
                if rem == 0 {
                    None
                } else {
                    let bit = rem.trailing_zeros() as usize;
                    rem &= rem - 1;
                    Some((wi * 64 + bit) as Reg)
                }
            })
        })
    }
}

/// Visit every register read by `insn` (for SSA liveness and renaming).
///
/// Note: instructions whose operand is strictly a canonical local slot
/// (`CheckLocal`, `SyncModuleGlobal`, `RecordClassStore`, `RecordClassDel`,
/// `LoadClassName`'s `local` slot) read the canonical `R[r]` slot directly,
/// which is already kept up to date by write-through `StoreLocal`.
fn visit_ssa_reads(insn: &Insn, mut f: impl FnMut(Reg)) {
    use Insn::*;
    match insn {
        LoadConst(..)
        | LoadGlobal(..)
        | LoadCell(..)
        | LoadClassDeref(..)
        | LoadNone(..)
        | LoadNoneRange { .. }
        | LoadExc(..)
        | ImportModule(..)
        | DeleteName(..)
        | DeleteCell(..)
        | PushTypeParamEnv
        | PopTypeParamEnv
        | DeleteLocal(..)
        | Jump(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | PopExcContext
        | ReturnNone
        | RaiseReRaise
        | RaiseAssertNoMsg
        | ForIter(..)
        | DeleteModuleGlobal(..)
        | CheckLocal(..)
        | RecordClassStore(..)
        | RecordClassDel(..)
        | SyncModuleGlobal(..)
        | LoadClassName(..)
        | MakeTypeVar(..) => {}

        StoreGlobal(_, s)
        | StoreCell(_, s)
        | ImportStar(s)
        | Move(_, s)
        | CopyReg(_, s)
        | UnaryOp(_, _, s)
        | FormatValue(_, s)
        | MatchSeqExcluded(_, s)
        | MatchMapping(_, s)
        | Return(s)
        | PrintExpr(s)
        | RaiseValue(s)
        | RaiseExceptStarResidual(s)
        | RaiseAssert(s)
        | JumpIfFalse(s, _)
        | JumpIfTrue(s, _)
        | GetIter(_, s)
        | GetAwaitable(_, s)
        | BuildListReserve(_, s)
        | Unpack(_, s, _)
        | GetAttr(_, s, _)
        | GetAttrForWith(_, s, _, _)
        | LoadExcTraceback(_, s)
        | ImportFromAttr(_, s, _)
        | DeleteAttr(s, _)
        | BinOpConst(_, s, _, _, _)
        | BinOpImm(_, s, _, _, _)
        | CmpJumpIfFalseConst(s, _, _, _)
        | CmpJumpIfTrueConst(s, _, _, _)
        | MatchExcept(s, _)
        | PushExcContext(s) => f(*s),

        MatchExceptStar(type_r, src, _, _) => {
            f(*type_r);
            f(*src);
        }

        BinOp(_, a, _, b)
        | BinOpInPlace(_, a, _, b)
        | CmpJumpIfFalse(a, _, b, _)
        | CmpJumpIfTrue(a, _, b, _)
        | RaiseFrom(a, b)
        | SetAdd(a, b)
        | ListAppend(a, b)
        | ListExtend(a, b)
        | DictUpdate(a, b)
        | GetItem(_, a, b)
        | FormatValueSpec(_, a, b)
        | DeleteItem(a, b) => {
            f(*a);
            f(*b);
        }

        ListExtendCall { list, src, name } => {
            f(*list);
            f(*src);
            visit_kwcall_name(name, &mut f);
        }
        SetAttr(obj, _, val) | SetTypeVarAttr(obj, _, val) => {
            f(*obj);
            f(*val);
        }
        SetItem(a, b, c) => {
            f(*a);
            f(*b);
            f(*c);
        }
        DictMergeKwCall { dict, src, name } => {
            f(*dict);
            f(*src);
            visit_kwcall_name(name, &mut f);
        }
        SetItemKwCall {
            dict,
            key,
            val,
            name,
        } => {
            f(*dict);
            f(*key);
            f(*val);
            visit_kwcall_name(name, &mut f);
        }
        Call(base, argc) | CallMemo(base, argc) => {
            for r in *base..=(*base + *argc as u32) {
                f(r);
            }
        }
        CallKw { func, total, .. } => {
            for r in *func..=(*func + *total as u32) {
                f(r);
            }
        }
        CallEx { func, npos, kwargs } => {
            for r in *func..=(*func + *npos as u32) {
                f(r);
            }
            f(*kwargs);
        }
        CallExArgs {
            func,
            npos,
            nkw,
            args_splat,
            kwargs,
            ..
        } => {
            for r in *func..=(*func + *npos as u32 + *nkw as u32) {
                f(r);
            }
            f(*args_splat);
            if *kwargs != NO_KWARGS {
                f(*kwargs);
            }
        }
        BuildList(_, base, n) | BuildTuple(_, base, n) => {
            for r in *base..*base + *n {
                f(r);
            }
        }
        BuildString(_, base, n) => {
            for r in *base..*base + *n as u32 {
                f(r);
            }
        }
        BuildSlice(_, base) => {
            for r in *base..*base + 3 {
                f(r);
            }
        }
        GetSlice(_, obj, base) => {
            f(*obj);
            for r in *base..*base + 3 {
                f(r);
            }
        }
        BuildDict(_, base, n, _) => {
            for r in *base..*base + 2 * *n {
                f(r);
            }
        }
        CallMethod {
            obj,
            args_base,
            nargs,
            ..
        } => {
            f(*obj);
            for r in *args_base..*args_base + *nargs as u32 {
                f(r);
            }
        }
        CallMethodKw {
            obj,
            args_base,
            total,
            ..
        } => {
            f(*obj);
            for r in *args_base..*args_base + *total as u32 {
                f(r);
            }
        }
        CallMethodExpanded {
            obj,
            pos_list,
            kw_dict,
            ..
        } => {
            f(*obj);
            f(*pos_list);
            f(*kw_dict);
        }
        MakeFunction(_, _, defs_base, defs_n, annots_base, annots_n) => {
            for r in *defs_base..*defs_base + *defs_n {
                f(r);
            }
            if *annots_n > 0 {
                for r in *annots_base..*annots_base + *annots_n {
                    f(r);
                }
            }
        }
        MakeClass(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n) => {
            for r in *bases_base..*bases_base + *bases_n {
                f(r);
            }
            for r in *kwarg_base..*kwarg_base + *kwarg_n {
                f(r);
            }
        }
        MakeClassMeta(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n, meta_reg) => {
            f(*meta_reg);
            for r in *bases_base..*bases_base + *bases_n {
                f(r);
            }
            for r in *kwarg_base..*kwarg_base + *kwarg_n {
                f(r);
            }
        }
        MakeTypeAlias(_, _, value_reg, params_reg) => {
            f(*value_reg);
            f(*params_reg);
        }
        Yield { src, .. } => f(*src),
        YieldFrom {
            iter_reg, sent_reg, ..
        } => {
            f(*iter_reg);
            f(*sent_reg);
        }
        UnpackEx { src, .. } => f(*src),
        Concat { base, count, .. } => {
            for r in *base..*base + *count as u32 {
                f(r);
            }
        }
        MatchClassPositional { subj, cls, .. } => {
            f(*subj);
            f(*cls);
        }
        // Late-stage guard opcodes never enter MIR.
        JumpIfNotInt(..)
        | JumpIfIterNotIntRange(..)
        | JumpIfIterNotIndexedSeq(..)
        | JumpIfIterNotIntRangeExact(..)
        | GetItemSeqIntOrExit(..)
        | JumpIfNotBuiltinLen(..)
        | LenSeqOrExit(..)
        | CountCmpJumpTrue(..)
        | CountCmpJumpFalse(..)
        | CallInlineBinOp { .. } => {}
    }
}

#[inline]
fn visit_kwcall_name(name: &crate::bytecode::KwCallName, f: &mut impl FnMut(Reg)) {
    match name {
        crate::bytecode::KwCallName::Callee(reg) => f(*reg),
        crate::bytecode::KwCallName::Method { obj, .. } => f(*obj),
    }
}

/// Visit every register written by a non-terminator `insn`.
fn visit_ssa_writes(insn: &Insn, mut f: impl FnMut(Reg)) {
    use Insn::*;
    match insn {
        LoadConst(r, _)
        | LoadGlobal(r, _)
        | LoadClassName(r, _, _)
        | LoadCell(r, _)
        | LoadClassDeref(r, _)
        | LoadNone(r)
        | Move(r, _)
        | CopyReg(r, _)
        | BinOp(r, _, _, _)
        | BinOpConst(r, _, _, _, _)
        | BinOpImm(r, _, _, _, _)
        | BinOpInPlace(r, _, _, _)
        | UnaryOp(r, _, _)
        | FormatValue(r, _)
        | FormatValueSpec(r, _, _)
        | MatchSeqExcluded(r, _)
        | MatchMapping(r, _)
        | GetAttr(r, _, _)
        | GetAttrForWith(r, _, _, _)
        | ImportFromAttr(r, _, _)
        | GetItem(r, _, _)
        | GetSlice(r, _, _)
        | GetAwaitable(r, _)
        | Call(r, _)
        | CallMemo(r, _)
        | CallKw { func: r, .. }
        | CallEx { func: r, .. }
        | CallExArgs { func: r, .. }
        | BuildList(r, _, _)
        | BuildListReserve(r, _)
        | BuildTuple(r, _, _)
        | BuildString(r, _, _)
        | BuildSlice(r, _)
        | BuildDict(r, _, _, _)
        | MakeFunction(r, _, _, _, _, _)
        | ImportModule(r, _)
        | LoadExc(r)
        | LoadExcTraceback(r, _)
        | MakeClass(r, _, _, _, _, _, _)
        | MakeClassMeta(r, _, _, _, _, _, _, _)
        | MakeTypeAlias(r, _, _, _)
        | MakeTypeVar(r, _)
        | CallMethod { dst: r, .. }
        | CallMethodKw { dst: r, .. }
        | CallMethodExpanded { dst: r, .. }
        | Concat { dst: r, .. }
        | Yield { dst: r, .. } => f(*r),
        YieldFrom { result_reg, .. } => f(*result_reg),
        LoadNoneRange { start, count } => {
            for r in *start..(*start + *count as u32) {
                f(r);
            }
        }
        Unpack(base, _, n) => {
            for r in *base..(*base + *n) {
                f(r);
            }
        }
        UnpackEx {
            before,
            after,
            dst_base,
            ..
        } => {
            let total = *before as u32 + 1 + *after;
            for r in *dst_base..(*dst_base + total) {
                f(r);
            }
        }
        MatchClassPositional { dst_base, n, .. } => {
            for r in *dst_base..(*dst_base + *n) {
                f(r);
            }
        }
        _ => {}
    }
}

/// Lower a compiled [`FnCode`] into true SSA [`MirFn`] form.
pub fn lower_fncode_to_mir(code: &FnCode) -> MirFn {
    let insns = &code.insns;
    let n = insns.len();
    let num_locals = code.num_locals;
    let num_regs = (code.num_regs as usize).max(num_locals as usize);
    let aliased_locals = !code.is_function_scope
        || insns.iter().any(|i| {
            matches!(
                i,
                Insn::SyncModuleGlobal(..)
                    | Insn::RecordClassStore(..)
                    | Insn::RecordClassDel(..)
                    | Insn::LoadClassName(..)
            )
        });

    let mut mir = MirFn {
        qualname: code.qualname.clone(),
        filename: code.filename.clone(),
        first_lineno: code.first_lineno,
        blocks: Vec::new(),
        entry: BlockId(0),
        val_types: Vec::new(),
        next_val: 0,
        num_locals,
        num_iters: code.num_iters,
        consts: code.consts.clone(),
        names: code.names.clone(),
        cell_slots: code.cell_slots.clone(),
        own_cell_count: code.own_cell_count,
        fn_protos: code.fn_protos.clone(),
        free_var_candidates: code.free_var_candidates.clone(),
        is_generator: code.is_generator,
        is_coroutine: code.is_coroutine,
        is_class_method: code.is_class_method,
        is_function_scope: code.is_function_scope,
        is_inlined_comp: code.is_inlined_comp,
        comp_enclosing_locals: code.comp_enclosing_locals.clone(),
    };

    if n == 0 {
        let v_none = mir.fresh_val();
        mir.blocks.push(MirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: vec![MirInst {
                def: Some(v_none),
                op: MirOp::Const(Value::none()),
                lineno: 0,
                col_span: (0, 0, 0, 0),
            }],
            term: Terminator::Return(v_none),
            term_lineno: 0,
            term_col_span: (0, 0, 0, 0),
            preds: Vec::new(),
            exc_depth: 0,
        });
        infer_mir_types(&mut mir);
        return mir;
    }

    // ── Step 1: Build raw basic blocks and CFG ───────────────────────────────
    let mut is_start = vec![false; n + 1];
    is_start[0] = true;
    let mut handler_pcs: HashSet<usize> = HashSet::new();

    for (pc, insn) in insns.iter().enumerate() {
        if let Some(off) = raw_jump_offset(insn) {
            let target = (pc as i64 + 1 + off as i64) as usize;
            if target <= n {
                is_start[target] = true;
                if matches!(insn, Insn::SetupExcept(_)) {
                    handler_pcs.insert(target);
                }
            }
        }
        if ends_raw_block(insn) && pc < n {
            is_start[pc + 1] = true;
        }
    }

    let mut pc_to_raw: Vec<usize> = vec![usize::MAX; n + 1];
    let mut starts: Vec<usize> = Vec::new();
    for (pc, &flag) in is_start.iter().enumerate() {
        if flag && pc < n {
            pc_to_raw[pc] = starts.len();
            starts.push(pc);
        }
    }
    // Synthetic exit block index when an edge targets `pc == n` (fall-off-end).
    let synthetic_exit_raw = if is_start[n] || !ends_raw_block(&insns[n - 1]) {
        let idx = starts.len();
        pc_to_raw[n] = idx;
        Some(idx)
    } else {
        None
    };

    let mut raw_blocks: Vec<RawBlock> = Vec::with_capacity(starts.len() + 1);
    for (bi, &start_pc) in starts.iter().enumerate() {
        let end_pc = starts.get(bi + 1).copied().unwrap_or(n);
        let last_pc = end_pc - 1;
        let has_term = ends_raw_block(&insns[last_pc]);
        let (body_end, term_pc) = if has_term {
            (last_pc, Some(last_pc))
        } else {
            (end_pc, None)
        };
        let mut succs: SmallVec<[RawEdge; 2]> = SmallVec::new();
        if let Some(tpc) = term_pc {
            let target_of = |off: i32| -> usize {
                let dst_pc = (tpc as i64 + 1 + off as i64) as usize;
                pc_to_raw[dst_pc.min(n)]
            };
            let fallthrough = pc_to_raw[end_pc.min(n)];
            match &insns[tpc] {
                Insn::Jump(off) => {
                    succs.push(RawEdge {
                        target: target_of(*off),
                        kind: RawEdgeKind::Normal,
                    });
                }
                Insn::JumpIfFalse(_, off)
                | Insn::CmpJumpIfFalse(_, _, _, off)
                | Insn::CmpJumpIfFalseConst(_, _, _, off) => {
                    // then = fallthrough (truthy), else = jump target (falsy)
                    succs.push(RawEdge {
                        target: fallthrough,
                        kind: RawEdgeKind::Normal,
                    });
                    succs.push(RawEdge {
                        target: target_of(*off),
                        kind: RawEdgeKind::Normal,
                    });
                }
                Insn::JumpIfTrue(_, off)
                | Insn::CmpJumpIfTrue(_, _, _, off)
                | Insn::CmpJumpIfTrueConst(_, _, _, off) => {
                    // then = jump target (truthy), else = fallthrough (falsy)
                    succs.push(RawEdge {
                        target: target_of(*off),
                        kind: RawEdgeKind::Normal,
                    });
                    succs.push(RawEdge {
                        target: fallthrough,
                        kind: RawEdgeKind::Normal,
                    });
                }
                Insn::ForIter(_, _, off) => {
                    succs.push(RawEdge {
                        target: fallthrough,
                        kind: RawEdgeKind::ForIterBody,
                    });
                    succs.push(RawEdge {
                        target: target_of(*off),
                        kind: RawEdgeKind::Normal,
                    });
                }
                Insn::SetupExcept(off) => {
                    succs.push(RawEdge {
                        target: fallthrough,
                        kind: RawEdgeKind::Normal,
                    });
                    succs.push(RawEdge {
                        target: target_of(*off),
                        kind: RawEdgeKind::SetupExceptHandler,
                    });
                }
                Insn::MatchExcept(_, off) => {
                    succs.push(RawEdge {
                        target: fallthrough,
                        kind: RawEdgeKind::Normal,
                    });
                    succs.push(RawEdge {
                        target: target_of(*off),
                        kind: RawEdgeKind::Normal,
                    });
                }
                Insn::MatchExceptStar(_, _, _, off) => {
                    succs.push(RawEdge {
                        target: fallthrough,
                        kind: RawEdgeKind::ExceptStarMatched,
                    });
                    succs.push(RawEdge {
                        target: target_of(*off),
                        kind: RawEdgeKind::Normal,
                    });
                }
                Insn::Return(_)
                | Insn::ReturnNone
                | Insn::RaiseValue(_)
                | Insn::RaiseExceptStarResidual(_)
                | Insn::RaiseFrom(..)
                | Insn::RaiseReRaise
                | Insn::RaiseAssert(_)
                | Insn::RaiseAssertNoMsg => {}
                _ => unreachable!("non-terminator in term_pc"),
            }
        } else {
            succs.push(RawEdge {
                target: pc_to_raw[end_pc.min(n)],
                kind: RawEdgeKind::Normal,
            });
        }

        raw_blocks.push(RawBlock {
            start_pc,
            body_end,
            term_pc,
            succs,
            preds: Vec::new(),
            exc_depth: 0,
            is_handler: handler_pcs.contains(&start_pc),
        });
    }

    if let Some(_exit_idx) = synthetic_exit_raw {
        raw_blocks.push(RawBlock {
            start_pc: n,
            body_end: n,
            term_pc: None,
            succs: SmallVec::new(),
            preds: Vec::new(),
            exc_depth: 0,
            is_handler: false,
        });
    }

    // ── Step 2: Reachability & exception-depth propagation from raw block 0 ──
    let mut reachable = vec![false; raw_blocks.len()];
    let mut exc_depth: Vec<Option<u16>> = vec![None; raw_blocks.len()];
    {
        let mut work = vec![0usize];
        reachable[0] = true;
        exc_depth[0] = Some(0);
        while let Some(bi) = work.pop() {
            let mut cur_depth = exc_depth[bi].unwrap_or(0);
            for insn in &insns[raw_blocks[bi].start_pc..raw_blocks[bi].body_end] {
                if matches!(insn, Insn::PopExcept) {
                    cur_depth = cur_depth.saturating_sub(1);
                }
            }
            let is_setup = raw_blocks[bi]
                .term_pc
                .is_some_and(|t| matches!(insns[t], Insn::SetupExcept(_)));
            for edge in raw_blocks[bi].succs.clone() {
                let next_d = if is_setup && edge.kind != RawEdgeKind::SetupExceptHandler {
                    cur_depth.saturating_add(1)
                } else {
                    cur_depth
                };
                if !reachable[edge.target] {
                    reachable[edge.target] = true;
                    exc_depth[edge.target] = Some(next_d);
                    work.push(edge.target);
                }
            }
        }
    }

    // Compact to only reachable raw blocks so every MirBlock is reachable from entry.
    let mut raw_to_mir: Vec<Option<usize>> = vec![None; raw_blocks.len()];
    let mut compact_raw: Vec<RawBlock> = Vec::new();
    for (old_idx, mut rb) in raw_blocks.into_iter().enumerate() {
        if reachable[old_idx] {
            let new_idx = compact_raw.len();
            raw_to_mir[old_idx] = Some(new_idx);
            rb.exc_depth = exc_depth[old_idx].unwrap_or(0);
            compact_raw.push(rb);
        }
    }
    for rb in &mut compact_raw {
        for edge in &mut rb.succs {
            edge.target = raw_to_mir[edge.target].expect("reachable successor must be mapped");
        }
    }
    for bi in 0..compact_raw.len() {
        let succs = compact_raw[bi].succs.clone();
        for edge in succs {
            compact_raw[edge.target].preds.push(bi);
        }
    }

    // ── Step 3: Split critical edges that define terminator values (ForIter / MatchExceptStar) ──
    // If a `ForIter::body_bb` or `MatchExceptStar::matched_bb` has >1 pred,
    // we handle edge-defined writes cleanly via a synthetic empty bridge block.
    // (In compiler output these targets already have 1 pred, so this rarely fires.)
    let nb = compact_raw.len();

    // ── Step 4: Identify pinned registers (write-through to canonical R[r]) ──
    // All named locals (`0..num_locals`) are pinned so `locals()`, `f_locals`,
    // tracebacks, `SyncModuleGlobal`, `RecordClassStore`, `CheckLocal`, and
    // exception handlers always observe up-to-date values in `R[0..num_locals]`.
    // In addition, any temp register written inside an exception region
    // (`exc_depth > 0`) that is read in a handler block without being strictly
    // defined before `SetupExcept` is pinned conservatively.
    let mut pinned = RegBits::empty(num_regs);
    for r in 0..num_locals {
        pinned.insert(r);
    }

    // ── Step 5: Compute `definitely_bound_in` / `definitely_bound_out` ───────
    let mut def_in: Vec<RegBits> = (0..nb)
        .map(|bi| {
            if bi == 0 {
                RegBits::empty(num_regs)
            } else {
                RegBits::all(num_regs)
            }
        })
        .collect();
    let mut def_out: Vec<RegBits> = (0..nb).map(|_| RegBits::all(num_regs)).collect();

    // Precompute per-block transfer helper.
    let apply_block_def_transfer = |bi: usize, mut bits: RegBits| -> RegBits {
        let rb = &compact_raw[bi];
        if rb.is_handler {
            // An exception can enter a handler from any raising op inside the
            // protected try block, so named locals reload from canonical R[r].
            bits.clear_below(num_locals);
        }
        for insn in &insns[rb.start_pc..rb.body_end] {
            if let Insn::DeleteLocal(r, _, _) = insn {
                bits.remove(*r);
                continue;
            }
            visit_ssa_writes(insn, |w| bits.insert(w));
            if aliased_locals && insn_may_invalidate_named_locals(insn) {
                bits.clear_below(num_locals);
            }
        }
        bits
    };

    // Optimistic fixpoint for `definitely_bound`.
    for _ in 0..(nb * 4 + 16) {
        let mut changed = false;
        for bi in 0..nb {
            let next_in = if bi == 0 {
                RegBits::empty(num_regs)
            } else {
                let mut acc = RegBits::all(num_regs);
                for &p in &compact_raw[bi].preds {
                    let mut edge_bits = def_out[p].clone();
                    // Check if the edge from `p -> bi` carries a terminator def
                    // (`ForIter::dst` or `MatchExceptStar`).
                    if let Some(tpc) = compact_raw[p].term_pc {
                        let edge_kind = compact_raw[p]
                            .succs
                            .iter()
                            .find(|e| e.target == bi)
                            .map(|e| e.kind)
                            .unwrap_or(RawEdgeKind::Normal);
                        match (&insns[tpc], edge_kind) {
                            (Insn::ForIter(dst, _, _), RawEdgeKind::ForIterBody) => {
                                edge_bits.insert(*dst);
                            }
                            (
                                Insn::MatchExceptStar(_, src_group, matched_dst, _),
                                RawEdgeKind::ExceptStarMatched,
                            ) => {
                                edge_bits.insert(*matched_dst);
                                edge_bits.insert(*src_group);
                            }
                            _ => {}
                        }
                    }
                    acc.intersect_with(&edge_bits);
                }
                if compact_raw[bi].is_handler {
                    acc.clear_below(num_locals);
                }
                acc
            };
            if next_in != def_in[bi] {
                def_in[bi] = next_in;
                changed = true;
            }
            let next_out = apply_block_def_transfer(bi, def_in[bi].clone());
            if next_out != def_out[bi] {
                def_out[bi] = next_out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Any temp register read at a point where it is not in `definitely_bound`
    // gets pinned so its writes also go through canonical `R[r]`.
    for bi in 0..nb {
        let rb = &compact_raw[bi];
        let mut cur = def_in[bi].clone();
        for insn in &insns[rb.start_pc..rb.body_end] {
            visit_ssa_reads(insn, |r| {
                if r >= num_locals && !cur.contains(r) {
                    pinned.insert(r);
                }
            });
            if let Insn::DeleteLocal(r, _, _) = insn {
                cur.remove(*r);
            } else {
                visit_ssa_writes(insn, |w| cur.insert(w));
                if aliased_locals && insn_may_invalidate_named_locals(insn) {
                    cur.clear_below(num_locals);
                }
            }
        }
        if let Some(tpc) = rb.term_pc {
            visit_ssa_reads(&insns[tpc], |r| {
                if r >= num_locals && !cur.contains(r) {
                    pinned.insert(r);
                }
            });
        }
    }

    // ── Step 6: Compute register liveness (backward bitset) ──────────────────
    let mut use_before_def: Vec<RegBits> = Vec::with_capacity(nb);
    let mut kill_set: Vec<RegBits> = Vec::with_capacity(nb);
    let mut def_blocks: Vec<Vec<usize>> = vec![Vec::new(); num_regs];

    for (bi, rb) in compact_raw.iter().enumerate() {
        let mut used = RegBits::empty(num_regs);
        let mut killed = RegBits::empty(num_regs);
        if rb.is_handler {
            for r in 0..num_locals {
                killed.insert(r);
            }
        }
        for insn in &insns[rb.start_pc..rb.body_end] {
            visit_ssa_reads(insn, |r| {
                if !killed.contains(r) {
                    used.insert(r);
                }
            });
            if let Insn::DeleteLocal(r, _, _) = insn {
                killed.insert(*r);
            } else {
                visit_ssa_writes(insn, |w| {
                    killed.insert(w);
                    if (w as usize) < num_regs && def_blocks[w as usize].last() != Some(&bi) {
                        def_blocks[w as usize].push(bi);
                    }
                });
                if aliased_locals && insn_may_invalidate_named_locals(insn) {
                    for r in 0..num_locals {
                        killed.insert(r);
                    }
                }
            }
        }
        if let Some(tpc) = rb.term_pc {
            let insn = &insns[tpc];
            visit_ssa_reads(insn, |r| {
                if !killed.contains(r) {
                    used.insert(r);
                }
            });
            match insn {
                Insn::ForIter(dst, _, _) => {
                    let body_bb = rb.succs[0].target;
                    if (*dst as usize) < num_regs {
                        if !def_blocks[*dst as usize].contains(&body_bb) {
                            def_blocks[*dst as usize].push(body_bb);
                        }
                        if compact_raw[body_bb].preds.len() > 1
                            && !def_blocks[*dst as usize].contains(&bi)
                        {
                            def_blocks[*dst as usize].push(bi);
                        }
                    }
                }
                Insn::MatchExceptStar(_, src_group, matched_dst, _) => {
                    let matched_bb = rb.succs[0].target;
                    for &w in &[matched_dst, src_group] {
                        if (*w as usize) < num_regs {
                            if !def_blocks[*w as usize].contains(&matched_bb) {
                                def_blocks[*w as usize].push(matched_bb);
                            }
                            if compact_raw[matched_bb].preds.len() > 1
                                && !def_blocks[*w as usize].contains(&bi)
                            {
                                def_blocks[*w as usize].push(bi);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        use_before_def.push(used);
        kill_set.push(killed);
    }

    let mut live_in: Vec<RegBits> = (0..nb).map(|_| RegBits::empty(num_regs)).collect();
    let mut live_out: Vec<RegBits> = (0..nb).map(|_| RegBits::empty(num_regs)).collect();
    for _ in 0..(nb * 4 + 16) {
        let mut changed = false;
        for bi in (0..nb).rev() {
            let mut out_bits = RegBits::empty(num_regs);
            for edge in &compact_raw[bi].succs {
                out_bits.union_with(&live_in[edge.target]);
            }
            live_out[bi] = out_bits;
            let mut in_bits = use_before_def[bi].clone();
            for r in live_out[bi].iter() {
                if !kill_set[bi].contains(r) {
                    in_bits.insert(r);
                }
            }
            if in_bits != live_in[bi] {
                live_in[bi] = in_bits;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // ── Step 7: Build skeleton MirBlocks so DominatorTree can compute DF ─────
    for (bi, rb) in compact_raw.iter().enumerate() {
        let dummy_term = match rb.succs.as_slice() {
            [] => Terminator::Unreachable,
            [e0] => Terminator::Jump {
                target: BlockId(e0.target as u32),
                args: Vec::new(),
            },
            [e0, e1] => Terminator::Branch {
                cond: Val(0),
                then_bb: BlockId(e0.target as u32),
                then_args: Vec::new(),
                else_bb: BlockId(e1.target as u32),
                else_args: Vec::new(),
            },
            _ => unreachable!("raw block has at most 2 successors"),
        };
        mir.blocks.push(MirBlock {
            id: BlockId(bi as u32),
            params: Vec::new(),
            instrs: Vec::new(),
            term: dummy_term,
            term_lineno: 0,
            term_col_span: (0, 0, 0, 0),
            preds: rb.preds.iter().map(|&p| BlockId(p as u32)).collect(),
            exc_depth: rb.exc_depth,
        });
    }

    let dom = DominatorTree::compute(&mir);
    let df = dom.dominance_frontiers(&mir);

    // ── Step 8: Place block-parameter phis on dominance frontiers ────────────
    // For each block `bi`, `phi_regs[bi]` holds the ordered list of registers
    // that have a block parameter in `mir.blocks[bi].params`.
    let mut phi_regs: Vec<Vec<Reg>> = vec![Vec::new(); nb];
    let mut phi_vals: Vec<Vec<Val>> = vec![Vec::new(); nb];

    for (reg_usize, blocks_defining) in def_blocks.iter().enumerate() {
        let r = reg_usize as Reg;
        if blocks_defining.is_empty() {
            continue;
        }
        let mut has_phi = vec![false; nb];
        let mut work: Vec<usize> = blocks_defining.clone();
        while let Some(b) = work.pop() {
            for &y_id in &df[b] {
                let y = y_id.0 as usize;
                if y == 0 || has_phi[y] {
                    continue;
                }
                // Soundness gate: only place a phi when `r` is BOTH live at
                // `y`'s entry AND guaranteed definitely bound along every
                // incoming edge!
                if def_in[y].contains(r) && live_in[y].contains(r) {
                    has_phi[y] = true;
                    let param_val = mir.fresh_val();
                    phi_regs[y].push(r);
                    phi_vals[y].push(param_val);
                    mir.blocks[y].params.push(param_val);
                    work.push(y);
                }
            }
        }
    }

    // ── Step 9: Dominator-tree SSA renaming pass ─────────────────────────────
    let mut stacks: Vec<Vec<Option<Val>>> = vec![Vec::new(); num_regs];
    let lineno_of = |pc: usize| -> u32 { code.lineno_table.get(pc).copied().unwrap_or(0) };
    let span_of =
        |pc: usize| -> CaretSpan { code.col_table.get(pc).copied().unwrap_or((0, 0, 0, 0)) };

    #[allow(clippy::too_many_arguments)]
    fn rename_block(
        bi: usize,
        compact_raw: &[RawBlock],
        insns: &[Insn],
        consts: &[Value],
        num_locals: Reg,
        aliased_locals: bool,
        pinned: &RegBits,
        def_in: &[RegBits],
        def_blocks: &[Vec<usize>],
        phi_regs: &[Vec<Reg>],
        phi_vals: &[Vec<Val>],
        dom: &DominatorTree,
        mir: &mut MirFn,
        stacks: &mut [Vec<Option<Val>>],
        lineno_of: &dyn Fn(usize) -> u32,
        span_of: &dyn Fn(usize) -> CaretSpan,
    ) {
        let mut pushed_regs: Vec<Reg> = Vec::new();
        let push_def = |stacks: &mut [Vec<Option<Val>>],
                        pushed_regs: &mut Vec<Reg>,
                        r: Reg,
                        val: Option<Val>| {
            if (r as usize) < stacks.len() {
                stacks[r as usize].push(val);
                pushed_regs.push(r);
            }
        };

        // At an exception handler block, clear named locals so reads reload from R[r].
        if compact_raw[bi].is_handler {
            for r in 0..num_locals {
                push_def(stacks, &mut pushed_regs, r, None);
            }
        }

        // Soundness guard for non-phi merges (e.g. a function parameter `a`
        // reassigned in only one arm of an `if`): if `bi` is not the entry
        // block and register `r` is assigned somewhere in the function yet is
        // NOT in `def_in[bi]`, `bi` did not receive a phi for `r`, so it must
        // NOT inherit a stale `LoadLocal(r)` from a dominating block across a
        // modifying sibling branch!  Clear `stacks[r]` so reads reload `R[r]`.
        if bi != 0 {
            for (r_usize, defs) in def_blocks.iter().enumerate() {
                if !defs.is_empty() && !def_in[bi].contains(r_usize as Reg) {
                    push_def(stacks, &mut pushed_regs, r_usize as Reg, None);
                }
            }
        }

        // Push block-parameter phis onto the rename stacks.
        for (&r, &v) in phi_regs[bi].iter().zip(phi_vals[bi].iter()) {
            push_def(stacks, &mut pushed_regs, r, Some(v));
        }

        let mut instrs_out: Vec<MirInst> = Vec::new();

        // Read helper: returns the active SSA Val for `r`, or emits `LoadLocal(r)`.
        let read_reg = |r: Reg,
                        lineno: u32,
                        col_span: CaretSpan,
                        mir: &mut MirFn,
                        stacks: &mut [Vec<Option<Val>>],
                        pushed_regs: &mut Vec<Reg>,
                        instrs_out: &mut Vec<MirInst>|
         -> Val {
            if let Some(Some(v)) = stacks.get(r as usize).and_then(|s| s.last()).copied() {
                return v;
            }
            let v = mir.fresh_val();
            instrs_out.push(MirInst {
                def: Some(v),
                op: MirOp::LoadLocal(r),
                lineno,
                col_span,
            });
            // Once LoadLocal(r) succeeds without raising UnboundLocalError, `r`
            // is bound to `v` in this block (until deleted or invalidated).
            if (r as usize) < stacks.len() {
                stacks[r as usize].push(Some(v));
                pushed_regs.push(r);
            }
            v
        };

        // Write helper: binds `r` to `v` in SSA and emits write-through `StoreLocal`
        // whenever `r` is pinned (`0..num_locals` or exception-escaping temp).
        let write_reg = |r: Reg,
                         v: Val,
                         lineno: u32,
                         col_span: CaretSpan,
                         stacks: &mut [Vec<Option<Val>>],
                         pushed_regs: &mut Vec<Reg>,
                         instrs_out: &mut Vec<MirInst>| {
            if (r as usize) < stacks.len() {
                stacks[r as usize].push(Some(v));
                pushed_regs.push(r);
            }
            if pinned.contains(r) {
                instrs_out.push(MirInst {
                    def: None,
                    op: MirOp::StoreLocal { reg: r, val: v },
                    lineno,
                    col_span,
                });
            }
        };

        let rb = &compact_raw[bi];
        for (offset, insn) in insns[rb.start_pc..rb.body_end].iter().enumerate() {
            let pc = rb.start_pc + offset;
            let ln = lineno_of(pc);
            let sp = span_of(pc);

            match insn {
                Insn::LoadConst(dst, c_idx) => {
                    let val = consts[*c_idx as usize].clone();
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::Const(val),
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::LoadNone(dst) => {
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::Const(Value::none()),
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::LoadNoneRange { start, count } => {
                    for offset in 0..(*count as u32) {
                        let v = mir.fresh_val();
                        instrs_out.push(MirInst {
                            def: Some(v),
                            op: MirOp::Const(Value::none()),
                            lineno: ln,
                            col_span: sp,
                        });
                        write_reg(
                            *start + offset,
                            v,
                            ln,
                            sp,
                            stacks,
                            &mut pushed_regs,
                            &mut instrs_out,
                        );
                    }
                }
                Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
                    let sv = read_reg(*src, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::Copy(sv),
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::BinOp(dst, lhs, op, rhs) => {
                    let lv = read_reg(*lhs, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let rv = read_reg(*rhs, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::BinOp {
                            op: *op,
                            lhs: lv,
                            rhs: rv,
                            is_inplace: false,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                    let lv = read_reg(*lhs, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let rv = read_reg(*rhs, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::BinOp {
                            op: *op,
                            lhs: lv,
                            rhs: rv,
                            is_inplace: true,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::BinOpConst(dst, lhs, op, c_idx, is_aug) => {
                    let lv = read_reg(*lhs, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let rv = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(rv),
                        op: MirOp::Const(consts[*c_idx as usize].clone()),
                        lineno: ln,
                        col_span: sp,
                    });
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::BinOp {
                            op: *op,
                            lhs: lv,
                            rhs: rv,
                            is_inplace: *is_aug,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
                    let lv = read_reg(*lhs, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let rv = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(rv),
                        op: MirOp::Const(Value::int(*imm as i64)),
                        lineno: ln,
                        col_span: sp,
                    });
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::BinOp {
                            op: *op,
                            lhs: lv,
                            rhs: rv,
                            is_inplace: *is_aug,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::UnaryOp(dst, op, src) => {
                    let sv = read_reg(*src, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::UnaryOp { op: *op, arg: sv },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::LoadGlobal(dst, name_idx) => {
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::LoadGlobal {
                            name_idx: *name_idx,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::StoreGlobal(name_idx, src) => {
                    let sv = read_reg(*src, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    instrs_out.push(MirInst {
                        def: None,
                        op: MirOp::StoreGlobal {
                            name_idx: *name_idx,
                            val: sv,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                }
                Insn::LoadCell(dst, cell_idx) => {
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::LoadCell {
                            cell_idx: *cell_idx,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::StoreCell(cell_idx, src) => {
                    let sv = read_reg(*src, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    instrs_out.push(MirInst {
                        def: None,
                        op: MirOp::StoreCell {
                            cell_idx: *cell_idx,
                            val: sv,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                }
                Insn::GetAttr(dst, obj, name_idx) => {
                    let ov = read_reg(*obj, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::GetAttr {
                            obj: ov,
                            name_idx: *name_idx,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    let ov = read_reg(*obj, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let vv = read_reg(*val, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    instrs_out.push(MirInst {
                        def: None,
                        op: MirOp::SetAttr {
                            obj: ov,
                            name_idx: *name_idx,
                            val: vv,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                }
                Insn::GetItem(dst, obj, key) => {
                    let ov = read_reg(*obj, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let kv = read_reg(*key, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::GetItem { obj: ov, key: kv },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::SetItem(obj, key, val) => {
                    let ov = read_reg(*obj, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let kv = read_reg(*key, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let vv = read_reg(*val, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    instrs_out.push(MirInst {
                        def: None,
                        op: MirOp::SetItem {
                            obj: ov,
                            key: kv,
                            val: vv,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                }
                Insn::Call(base, argc) | Insn::CallMemo(base, argc) => {
                    let is_memo = matches!(insn, Insn::CallMemo(..));
                    let callee = read_reg(
                        *base,
                        ln,
                        sp,
                        mir,
                        stacks,
                        &mut pushed_regs,
                        &mut instrs_out,
                    );
                    let mut args = Vec::with_capacity(*argc as usize);
                    for i in 0..(*argc as u32) {
                        args.push(read_reg(
                            *base + 1 + i,
                            ln,
                            sp,
                            mir,
                            stacks,
                            &mut pushed_regs,
                            &mut instrs_out,
                        ));
                    }
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::Call {
                            callee,
                            args,
                            is_memo,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*base, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::BuildTuple(dst, base, n_elems) => {
                    let mut elems = Vec::with_capacity(*n_elems as usize);
                    for i in 0..*n_elems {
                        elems.push(read_reg(
                            *base + i,
                            ln,
                            sp,
                            mir,
                            stacks,
                            &mut pushed_regs,
                            &mut instrs_out,
                        ));
                    }
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::BuildTuple(elems),
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::BuildList(dst, base, n_elems) => {
                    let mut elems = Vec::with_capacity(*n_elems as usize);
                    for i in 0..*n_elems {
                        elems.push(read_reg(
                            *base + i,
                            ln,
                            sp,
                            mir,
                            stacks,
                            &mut pushed_regs,
                            &mut instrs_out,
                        ));
                    }
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::BuildList(elems),
                        lineno: ln,
                        col_span: sp,
                    });
                    write_reg(*dst, v, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                }
                Insn::DeleteLocal(reg, _, _) => {
                    instrs_out.push(MirInst {
                        def: None,
                        op: MirOp::SideEffectInsn {
                            insn: insn.clone(),
                            uses: SmallVec::new(),
                            defs: SmallVec::new(),
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    push_def(stacks, &mut pushed_regs, *reg, None);
                }
                other => {
                    let mut uses: SmallVec<[Val; 4]> = SmallVec::new();
                    visit_ssa_reads(other, |r| {
                        let v = read_reg(r, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                        uses.push(v);
                    });
                    let mut write_regs: SmallVec<[Reg; 2]> = SmallVec::new();
                    visit_ssa_writes(other, |w| write_regs.push(w));
                    let mut defs: SmallVec<[Val; 2]> = SmallVec::with_capacity(write_regs.len());
                    for _ in &write_regs {
                        defs.push(mir.fresh_val());
                    }
                    let single_def = if defs.len() == 1 { Some(defs[0]) } else { None };
                    instrs_out.push(MirInst {
                        def: single_def,
                        op: MirOp::SideEffectInsn {
                            insn: other.clone(),
                            uses,
                            defs: defs.clone(),
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    for (&wr, &dv) in write_regs.iter().zip(defs.iter()) {
                        write_reg(wr, dv, ln, sp, stacks, &mut pushed_regs, &mut instrs_out);
                    }
                }
            }

            // At module / class scope, external namespace aliases may mutate
            // named locals during protocol-dispatching instructions.
            if aliased_locals && insn_may_invalidate_named_locals(insn) {
                for r in 0..num_locals {
                    push_def(stacks, &mut pushed_regs, r, None);
                }
            }
        }

        // Helper to collect outgoing block-parameter args for a successor.
        let collect_edge_args = |target_bi: usize,
                                 stacks: &[Vec<Option<Val>>],
                                 override_regs: &[(Reg, Val)]|
         -> Vec<Val> {
            phi_regs[target_bi]
                .iter()
                .map(|&r| {
                    if let Some(&(_, ov)) = override_regs.iter().find(|&&(or, _)| or == r) {
                        return ov;
                    }
                    stacks
                        .get(r as usize)
                        .and_then(|s| s.last())
                        .copied()
                        .flatten()
                        .expect("phi register must be definitely bound on every incoming edge")
                })
                .collect()
        };

        // Build terminator.
        let (term, term_lineno, term_col_span) = if let Some(tpc) = rb.term_pc {
            let t_insn = &insns[tpc];
            let ln = lineno_of(tpc);
            let sp = span_of(tpc);
            let term = match t_insn {
                Insn::Jump(_) => {
                    let target = rb.succs[0].target;
                    let args = collect_edge_args(target, stacks, &[]);
                    Terminator::Jump {
                        target: BlockId(target as u32),
                        args,
                    }
                }
                Insn::JumpIfFalse(cond_r, _) => {
                    let cond = read_reg(
                        *cond_r,
                        ln,
                        sp,
                        mir,
                        stacks,
                        &mut pushed_regs,
                        &mut instrs_out,
                    );
                    let then_t = rb.succs[0].target;
                    let else_t = rb.succs[1].target;
                    Terminator::Branch {
                        cond,
                        then_bb: BlockId(then_t as u32),
                        then_args: collect_edge_args(then_t, stacks, &[]),
                        else_bb: BlockId(else_t as u32),
                        else_args: collect_edge_args(else_t, stacks, &[]),
                    }
                }
                Insn::JumpIfTrue(cond_r, _) => {
                    let cond = read_reg(
                        *cond_r,
                        ln,
                        sp,
                        mir,
                        stacks,
                        &mut pushed_regs,
                        &mut instrs_out,
                    );
                    let then_t = rb.succs[0].target;
                    let else_t = rb.succs[1].target;
                    Terminator::Branch {
                        cond,
                        then_bb: BlockId(then_t as u32),
                        then_args: collect_edge_args(then_t, stacks, &[]),
                        else_bb: BlockId(else_t as u32),
                        else_args: collect_edge_args(else_t, stacks, &[]),
                    }
                }
                Insn::CmpJumpIfFalse(a, op, b, _) | Insn::CmpJumpIfTrue(a, op, b, _) => {
                    let lv = read_reg(*a, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let rv = read_reg(*b, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let cond = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(cond),
                        op: MirOp::BinOp {
                            op: *op,
                            lhs: lv,
                            rhs: rv,
                            is_inplace: false,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    let then_t = rb.succs[0].target;
                    let else_t = rb.succs[1].target;
                    Terminator::Branch {
                        cond,
                        then_bb: BlockId(then_t as u32),
                        then_args: collect_edge_args(then_t, stacks, &[]),
                        else_bb: BlockId(else_t as u32),
                        else_args: collect_edge_args(else_t, stacks, &[]),
                    }
                }
                Insn::CmpJumpIfFalseConst(a, op, c_idx, _)
                | Insn::CmpJumpIfTrueConst(a, op, c_idx, _) => {
                    let lv = read_reg(*a, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let rv = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(rv),
                        op: MirOp::Const(consts[*c_idx as usize].clone()),
                        lineno: ln,
                        col_span: sp,
                    });
                    let cond = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(cond),
                        op: MirOp::BinOp {
                            op: *op,
                            lhs: lv,
                            rhs: rv,
                            is_inplace: false,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                    let then_t = rb.succs[0].target;
                    let else_t = rb.succs[1].target;
                    Terminator::Branch {
                        cond,
                        then_bb: BlockId(then_t as u32),
                        then_args: collect_edge_args(then_t, stacks, &[]),
                        else_bb: BlockId(else_t as u32),
                        else_args: collect_edge_args(else_t, stacks, &[]),
                    }
                }
                Insn::Return(r) => {
                    let v = read_reg(*r, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    Terminator::Return(v)
                }
                Insn::ReturnNone => {
                    let v = mir.fresh_val();
                    instrs_out.push(MirInst {
                        def: Some(v),
                        op: MirOp::Const(Value::none()),
                        lineno: ln,
                        col_span: sp,
                    });
                    Terminator::Return(v)
                }
                Insn::RaiseValue(r) => {
                    let v = read_reg(*r, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    Terminator::Raise(Some(v))
                }
                Insn::RaiseReRaise => Terminator::Raise(None),
                Insn::RaiseFrom(a, b) => {
                    let av = read_reg(*a, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    let bv = read_reg(*b, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    Terminator::RaiseSpecial {
                        insn: t_insn.clone(),
                        uses: smallvec::smallvec![av, bv],
                    }
                }
                Insn::RaiseAssert(r) | Insn::RaiseExceptStarResidual(r) => {
                    let rv = read_reg(*r, ln, sp, mir, stacks, &mut pushed_regs, &mut instrs_out);
                    Terminator::RaiseSpecial {
                        insn: t_insn.clone(),
                        uses: smallvec::smallvec![rv],
                    }
                }
                Insn::RaiseAssertNoMsg => Terminator::RaiseSpecial {
                    insn: t_insn.clone(),
                    uses: SmallVec::new(),
                },
                Insn::ForIter(dst_reg, slot, _) => {
                    let dst_val = mir.fresh_val();
                    let body_t = rb.succs[0].target;
                    let exit_t = rb.succs[1].target;
                    let body_args = collect_edge_args(body_t, stacks, &[(*dst_reg, dst_val)]);
                    let exit_args = collect_edge_args(exit_t, stacks, &[]);
                    Terminator::ForIter {
                        slot: *slot,
                        dst: dst_val,
                        body_bb: BlockId(body_t as u32),
                        body_args,
                        exit_bb: BlockId(exit_t as u32),
                        exit_args,
                    }
                }
                Insn::SetupExcept(_) => {
                    let try_t = rb.succs[0].target;
                    let handler_t = rb.succs[1].target;
                    Terminator::SetupExcept {
                        try_bb: BlockId(try_t as u32),
                        try_args: collect_edge_args(try_t, stacks, &[]),
                        handler_bb: BlockId(handler_t as u32),
                        handler_args: collect_edge_args(handler_t, stacks, &[]),
                    }
                }
                Insn::MatchExcept(type_r, _) => {
                    let tv = read_reg(
                        *type_r,
                        ln,
                        sp,
                        mir,
                        stacks,
                        &mut pushed_regs,
                        &mut instrs_out,
                    );
                    let matched_t = rb.succs[0].target;
                    let nomatch_t = rb.succs[1].target;
                    Terminator::MatchExcept {
                        type_val: tv,
                        matched_bb: BlockId(matched_t as u32),
                        matched_args: collect_edge_args(matched_t, stacks, &[]),
                        nomatch_bb: BlockId(nomatch_t as u32),
                        nomatch_args: collect_edge_args(nomatch_t, stacks, &[]),
                    }
                }
                Insn::MatchExceptStar(type_r, src_group, matched_dst, _) => {
                    let tv = read_reg(
                        *type_r,
                        ln,
                        sp,
                        mir,
                        stacks,
                        &mut pushed_regs,
                        &mut instrs_out,
                    );
                    let sv = read_reg(
                        *src_group,
                        ln,
                        sp,
                        mir,
                        stacks,
                        &mut pushed_regs,
                        &mut instrs_out,
                    );
                    let matched_def = mir.fresh_val();
                    let residual_def = mir.fresh_val();
                    let matched_t = rb.succs[0].target;
                    let nomatch_t = rb.succs[1].target;
                    let matched_args = collect_edge_args(
                        matched_t,
                        stacks,
                        &[(*matched_dst, matched_def), (*src_group, residual_def)],
                    );
                    let nomatch_args = collect_edge_args(nomatch_t, stacks, &[]);
                    Terminator::MatchExceptStar {
                        type_val: tv,
                        src_group: sv,
                        matched_def,
                        residual_def,
                        matched_bb: BlockId(matched_t as u32),
                        matched_args,
                        nomatch_bb: BlockId(nomatch_t as u32),
                        nomatch_args,
                    }
                }
                _ => unreachable!("unexpected terminator"),
            };
            (term, ln, sp)
        } else if let Some(edge) = rb.succs.first() {
            let target = edge.target;
            let args = collect_edge_args(target, stacks, &[]);
            (
                Terminator::Jump {
                    target: BlockId(target as u32),
                    args,
                },
                0,
                (0, 0, 0, 0),
            )
        } else {
            // Synthetic fall-off-end return None.
            let v = mir.fresh_val();
            instrs_out.push(MirInst {
                def: Some(v),
                op: MirOp::Const(Value::none()),
                lineno: 0,
                col_span: (0, 0, 0, 0),
            });
            (Terminator::Return(v), 0, (0, 0, 0, 0))
        };

        mir.blocks[bi].instrs = instrs_out;
        mir.blocks[bi].term = term;
        mir.blocks[bi].term_lineno = term_lineno;
        mir.blocks[bi].term_col_span = term_col_span;

        // Check if this block ended with `ForIter` or `MatchExceptStar`, which
        // defines values on its first (fallthrough) successor ONLY.
        type FallthroughBindings = SmallVec<[(Reg, Val); 2]>;
        let special_fallthrough: Option<(usize, FallthroughBindings)> = if let Some(tpc) =
            rb.term_pc
        {
            match (&insns[tpc], &mir.blocks[bi].term) {
                (Insn::ForIter(dst_reg, _, _), Terminator::ForIter { dst, body_bb, .. }) => {
                    Some((body_bb.0 as usize, smallvec::smallvec![(*dst_reg, *dst)]))
                }
                (
                    Insn::MatchExceptStar(_, src_group, matched_dst, _),
                    Terminator::MatchExceptStar {
                        matched_def,
                        residual_def,
                        matched_bb,
                        ..
                    },
                ) => Some((
                    matched_bb.0 as usize,
                    smallvec::smallvec![(*matched_dst, *matched_def), (*src_group, *residual_def),],
                )),
                _ => None,
            }
        } else {
            None
        };

        // Recurse on dominator-tree children.
        for &child in &dom.children[bi] {
            let child_idx = child.0 as usize;
            let mut extra_pushes = 0usize;
            if let Some((target_idx, ref bindings)) = special_fallthrough
                && child_idx == target_idx
            {
                for &(r, v) in bindings {
                    if (r as usize) < stacks.len() {
                        stacks[r as usize].push(Some(v));
                        extra_pushes += 1;
                    }
                }
            }

            rename_block(
                child_idx,
                compact_raw,
                insns,
                consts,
                num_locals,
                aliased_locals,
                pinned,
                def_in,
                def_blocks,
                phi_regs,
                phi_vals,
                dom,
                mir,
                stacks,
                lineno_of,
                span_of,
            );

            // If `child_idx` is the exclusive fallthrough of `ForIter` or
            // `MatchExceptStar` and a bound reg is pinned, prepend write-through
            // `StoreLocal`s at the top of `child_idx` so canonical `R[r]` is updated!
            if extra_pushes > 0
                && let Some((_, ref bindings)) = special_fallthrough
            {
                let mut prepend = Vec::new();
                let ln = term_lineno;
                let sp = term_col_span;
                for &(r, v) in bindings {
                    if pinned.contains(r) {
                        prepend.push(MirInst {
                            def: None,
                            op: MirOp::StoreLocal { reg: r, val: v },
                            lineno: ln,
                            col_span: sp,
                        });
                    }
                }
                if !prepend.is_empty() {
                    let old = std::mem::take(&mut mir.blocks[child_idx].instrs);
                    prepend.extend(old);
                    mir.blocks[child_idx].instrs = prepend;
                }
                for &(r, _) in bindings.iter().rev() {
                    if (r as usize) < stacks.len() {
                        stacks[r as usize].pop();
                    }
                }
            }
        }

        // Unwind stack pushes from this block.
        for r in pushed_regs.into_iter().rev() {
            stacks[r as usize].pop();
        }
    }

    rename_block(
        0,
        &compact_raw,
        insns,
        &code.consts,
        num_locals,
        aliased_locals,
        &pinned,
        &def_in,
        &def_blocks,
        &phi_regs,
        &phi_vals,
        &dom,
        &mut mir,
        &mut stacks,
        &lineno_of,
        &span_of,
    );

    // Handle the rare case where a `ForIter` body or `MatchExceptStar` matched
    // block had >1 predecessor (so `dom.children[bi]` might not have directly
    // owned it for prepending `StoreLocal`): insert a tiny edge bridge block
    // carrying the pinned `StoreLocal`s!
    let mut bridges: Vec<MirBlock> = Vec::new();
    for bi in 0..nb {
        let Some(tpc) = compact_raw[bi].term_pc else {
            continue;
        };
        let needs_bridge = match (&insns[tpc], &mir.blocks[bi].term) {
            (Insn::ForIter(dst_reg, _, _), Terminator::ForIter { body_bb, .. }) => {
                pinned.contains(*dst_reg) && compact_raw[body_bb.0 as usize].preds.len() > 1
            }
            (
                Insn::MatchExceptStar(_, src_group, matched_dst, _),
                Terminator::MatchExceptStar { matched_bb, .. },
            ) => {
                (pinned.contains(*src_group) || pinned.contains(*matched_dst))
                    && compact_raw[matched_bb.0 as usize].preds.len() > 1
            }
            _ => false,
        };
        if !needs_bridge {
            continue;
        }
        let bridge_id = BlockId((nb + bridges.len()) as u32);
        let ln = mir.blocks[bi].term_lineno;
        let sp = mir.blocks[bi].term_col_span;
        let mut bridge_instrs = Vec::new();
        let (orig_target, orig_args) = match (&insns[tpc], &mut mir.blocks[bi].term) {
            (
                Insn::ForIter(dst_reg, _, _),
                Terminator::ForIter {
                    dst,
                    body_bb,
                    body_args,
                    ..
                },
            ) => {
                if pinned.contains(*dst_reg) {
                    bridge_instrs.push(MirInst {
                        def: None,
                        op: MirOp::StoreLocal {
                            reg: *dst_reg,
                            val: *dst,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                }
                let t = *body_bb;
                let a = std::mem::take(body_args);
                *body_bb = bridge_id;
                (t, a)
            }
            (
                Insn::MatchExceptStar(_, src_group, matched_dst, _),
                Terminator::MatchExceptStar {
                    matched_def,
                    residual_def,
                    matched_bb,
                    matched_args,
                    ..
                },
            ) => {
                if pinned.contains(*matched_dst) {
                    bridge_instrs.push(MirInst {
                        def: None,
                        op: MirOp::StoreLocal {
                            reg: *matched_dst,
                            val: *matched_def,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                }
                if pinned.contains(*src_group) {
                    bridge_instrs.push(MirInst {
                        def: None,
                        op: MirOp::StoreLocal {
                            reg: *src_group,
                            val: *residual_def,
                        },
                        lineno: ln,
                        col_span: sp,
                    });
                }
                let t = *matched_bb;
                let a = std::mem::take(matched_args);
                *matched_bb = bridge_id;
                (t, a)
            }
            _ => continue,
        };
        bridges.push(MirBlock {
            id: bridge_id,
            params: Vec::new(),
            instrs: bridge_instrs,
            term: Terminator::Jump {
                target: orig_target,
                args: orig_args,
            },
            term_lineno: ln,
            term_col_span: sp,
            preds: Vec::new(),
            exc_depth: mir.blocks[bi].exc_depth,
        });
    }
    if !bridges.is_empty() {
        mir.blocks.extend(bridges);
    }

    mir.rebuild_preds();
    infer_mir_types(&mut mir);
    let _ = NO_CLASS_LOCAL; // keep import usage clear
    mir
}
