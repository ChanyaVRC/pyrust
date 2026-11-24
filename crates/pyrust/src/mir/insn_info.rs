//! Shared bytecode [`Insn`] operand-mapping and CFG classification helpers
//! used by both SSA construction (`mir::ssa`) and bytecode lowering
//! (`mir::lower_bytecode`).
//!
//! Having a single exhaustive declarative dispatch over [`Insn`] guarantees
//! that SSA read/write extraction and `SideEffectInsn` register
//! materialization always agree on operand ordering and window layouts.

use crate::ast::CaretSpan;
use crate::bytecode::{
    CallExArgsDesc, CallMethodExpandedDesc, CallMethodKwDesc, DescIdx, DictMergeKwCallDesc, Insn,
    InsnDesc, KwCallName, ListExtendCallDesc, MakeClassDesc, MakeFunctionDesc,
    MatchClassPositionalDesc, MatchExceptStarDesc, NO_ARGS_SPLAT, NO_ASSERT_MSG, NO_KWARGS,
    NO_METACLASS, Reg, SetItemKwCallDesc, push_desc,
};

/// Return the descriptor pool index carried by `insn`, if any.
#[inline]
pub(crate) fn insn_desc_idx(insn: &Insn) -> Option<DescIdx> {
    match insn {
        Insn::JumpIfIterNotIntRangeExact(idx, _)
        | Insn::CallInlineBinOp(idx, _)
        | Insn::CallMethodExpanded(idx)
        | Insn::CallExArgs(idx)
        | Insn::CallMethodKw(idx)
        | Insn::MatchClassPositional(idx)
        | Insn::MakeFunction(idx)
        | Insn::MatchExceptStar(idx, _)
        | Insn::MakeClass(idx)
        | Insn::ListExtendCall(idx)
        | Insn::DictMergeKwCall(idx)
        | Insn::SetItemKwCall(idx) => Some(*idx),
        _ => None,
    }
}

/// Update the descriptor pool index carried by `insn`, if any.
#[inline]
pub(crate) fn set_insn_desc_idx(insn: &mut Insn, new_idx: DescIdx) {
    match insn {
        Insn::JumpIfIterNotIntRangeExact(idx, _)
        | Insn::CallInlineBinOp(idx, _)
        | Insn::CallMethodExpanded(idx)
        | Insn::CallExArgs(idx)
        | Insn::CallMethodKw(idx)
        | Insn::MatchClassPositional(idx)
        | Insn::MakeFunction(idx)
        | Insn::MatchExceptStar(idx, _)
        | Insn::MakeClass(idx)
        | Insn::ListExtendCall(idx)
        | Insn::DictMergeKwCall(idx)
        | Insn::SetItemKwCall(idx) => *idx = new_idx,
        _ => {}
    }
}

/// Conservative false-whitelist matching `optimizer/effects.rs::may_invalidate_named_locals`.
#[inline]
pub(crate) fn insn_may_invalidate_named_locals(insn: &Insn) -> bool {
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
            | CheckLocal(..)
            | BuildList(..)
            | BuildListReserve(..)
            | BuildTuple(..)
            | BuildString(..)
            | BuildSlice(..)
            | MakeFunction(..)
    )
}

/// Return the relative jump offset carried by `insn`, if any.
#[inline]
pub(crate) fn jump_offset(insn: &Insn) -> Option<i32> {
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
        | Insn::MatchExceptStar(_, k) => Some(*k),
        _ => None,
    }
}

/// Mutable reference to the relative jump offset carried by `insn`, if any.
#[inline]
pub(crate) fn jump_offset_mut(insn: &mut Insn) -> Option<&mut i32> {
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
        | Insn::MatchExceptStar(_, k) => Some(k),
        _ => None,
    }
}

/// True when `insn` terminates a raw bytecode basic block.
#[inline]
pub(crate) fn ends_raw_block(insn: &Insn) -> bool {
    jump_offset(insn).is_some()
        || matches!(
            insn,
            Insn::Return(_)
                | Insn::ReturnNone
                | Insn::RaiseValue(_)
                | Insn::RaiseExceptStarResidual(_)
                | Insn::RaiseFrom(..)
                | Insn::RaiseReRaise
                | Insn::RaiseAssert(_)
        )
}

/// Single exhaustive operand-slot dispatch over [`Insn`].
///
/// Works identically on `&Insn` (bindings are `&Reg`) and `&mut Insn`
/// (bindings are `&mut Reg`) via match ergonomics, keeping SSA read/write
/// discovery and `SideEffectInsn` materialization strictly in sync.
///
/// Note: instructions whose operand is strictly a canonical local slot
/// (`CheckLocal`, `SyncModuleGlobal`, `RecordClassStore`, `RecordClassDel`,
/// `LoadClassName`'s `local` slot) access canonical `R[r]` directly (which
/// write-through `StoreLocal` keeps current) and are intentionally omitted.
macro_rules! dispatch_insn_operands {
    ($insn:expr, $desc:expr, $v:expr) => {{
        use Insn::*;
        match $insn {
            // Zero-reg / canonical-slot / terminator-def-only / late-stage guards.
            ImportModule(d, _)
            | LoadConst(d, _)
            | LoadGlobal(d, _)
            | LoadClassName(d, _, _)
            | LoadCell(d, _)
            | LoadClassDeref(d, _)
            | LoadNone(d)
            | LoadExc(d)
            | MakeTypeVar(d, _, _) => {
                $v.write_reg(d);
            }
            LoadNoneRange { start, count } => {
                $v.write_window(start, *count as u32);
            }

            DeleteName(..)
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
            | ForIter(..)
            | DeleteModuleGlobal(..)
            | CheckLocal(..)
            | RecordClassStore(..)
            | RecordClassDel(..)
            | SyncModuleGlobal(..)
            // Late-stage guard opcodes never enter MIR.
            | JumpIfNotInt(..)
            | JumpIfIterNotIntRange(..)
            | JumpIfIterNotIndexedSeq(..)
            | JumpIfIterNotIntRangeExact(..)
            | GetItemSeqIntOrExit(..)
            | JumpIfNotBuiltinLen(..)
            | LenSeqOrExit(..)
            | CountCmpJumpTrue(..)
            | CountCmpJumpFalse(..)
            | CallInlineBinOp(..) => {}

            RaiseAssert(s) => {
                if *s != NO_ASSERT_MSG {
                    $v.read_reg(s);
                }
            }

            // Single scalar read, no write.
            StoreGlobal(_, s)
            | StoreCell(_, s)
            | ImportStar(s)
            | Return(s)
            | PrintExpr(s)
            | RaiseValue(s)
            | RaiseExceptStarResidual(s)
            | JumpIfFalse(s, _)
            | JumpIfTrue(s, _)
            | GetIter(_, s)
            | DeleteAttr(s, _)
            | CmpJumpIfFalseConst(s, _, _, _)
            | CmpJumpIfTrueConst(s, _, _, _)
            | MatchExcept(s, _)
            | PushExcContext(s) => {
                $v.read_reg(s);
            }

            // Single scalar read, single scalar write.
            Move(d, s)
            | UnaryOp(d, _, s)
            | FormatValue(d, s)
            | MatchSeqExcluded(d, s)
            | MatchMapping(d, s)
            | GetAwaitable(d, s)
            | BuildListReserve(d, s)
            | GetAttr(d, s, _)
            | GetAttrForWith(d, s, _, _)
            | LoadExcTraceback(d, s)
            | ImportFromAttr(d, s, _)
            | BinOpConst(d, s, _, _, _)
            | BinOpImm(d, s, _, _, _)
            | Yield { src: s, dst: d } => {
                $v.read_reg(s);
                $v.write_reg(d);
            }

            // Two scalar reads, no write (MatchExceptStar writes are edge-defined).
            MatchExceptStar(_, _) => {
                let Some(InsnDesc::MatchExceptStar(MatchExceptStarDesc {
                    type_reg: a,
                    src_group: b,
                    ..
                })) = $desc
                else {
                    unreachable!("MatchExceptStar missing descriptor");
                };
                $v.read_reg(a);
                $v.read_reg(b);
            }
            CmpJumpIfFalse(a, _, b, _)
            | CmpJumpIfTrue(a, _, b, _)
            | RaiseFrom(a, b)
            | SetAdd(a, b)
            | ListAppend(a, b)
            | ListExtend(a, b)
            | DictUpdate(a, b)
            | DeleteItem(a, b)
            | SetAttr(a, _, b)
            | SetTypeVarAttr(a, _, b) => {
                $v.read_reg(a);
                $v.read_reg(b);
            }

            // Two scalar reads, single scalar write.
            BinOp(d, a, _, b)
            | BinOpInPlace(d, a, _, b)
            | GetItem(d, a, b)
            | FormatValueSpec(d, a, b)
            | MakeTypeAlias(d, _, a, b)
            | YieldFrom {
                iter_reg: a,
                sent_reg: b,
                result_reg: d,
            } => {
                $v.read_reg(a);
                $v.read_reg(b);
                $v.write_reg(d);
            }

            // Three scalar reads, no write.
            SetItem(a, b, c) => {
                $v.read_reg(a);
                $v.read_reg(b);
                $v.read_reg(c);
            }

            // Three scalar reads, single scalar write.
            CallMethodExpanded(_) => {
                let Some(InsnDesc::CallMethodExpanded(CallMethodExpandedDesc {
                    dst: d,
                    obj: a,
                    pos_list: b,
                    kw_dict: c,
                    ..
                })) = $desc
                else {
                    unreachable!("CallMethodExpanded missing descriptor");
                };
                $v.read_reg(a);
                $v.read_reg(b);
                $v.read_reg(c);
                $v.write_reg(d);
            }

            // KwCallName helpers.
            ListExtendCall(_) => {
                let Some(InsnDesc::ListExtendCall(ListExtendCallDesc {
                    list: a,
                    src: b,
                    name,
                })) = $desc
                else {
                    unreachable!("ListExtendCall missing descriptor");
                };
                $v.read_reg(a);
                $v.read_reg(b);
                match name {
                    KwCallName::Callee(r) | KwCallName::Method { obj: r, .. } => $v.read_reg(r),
                }
            }
            DictMergeKwCall(_) => {
                let Some(InsnDesc::DictMergeKwCall(DictMergeKwCallDesc {
                    dict: a,
                    src: b,
                    name,
                })) = $desc
                else {
                    unreachable!("DictMergeKwCall missing descriptor");
                };
                $v.read_reg(a);
                $v.read_reg(b);
                match name {
                    KwCallName::Callee(r) | KwCallName::Method { obj: r, .. } => $v.read_reg(r),
                }
            }
            SetItemKwCall(_) => {
                let Some(InsnDesc::SetItemKwCall(SetItemKwCallDesc {
                    dict: a,
                    key: b,
                    val: c,
                    name,
                })) = $desc
                else {
                    unreachable!("SetItemKwCall missing descriptor");
                };
                $v.read_reg(a);
                $v.read_reg(b);
                $v.read_reg(c);
                match name {
                    KwCallName::Callee(r) | KwCallName::Method { obj: r, .. } => $v.read_reg(r),
                }
            }

            // In-place call windows (base is both read window start and dst).
            Call(base, argc) | CallMemo(base, argc) => {
                $v.call_window(base, *argc as u32 + 1);
            }
            CallKw { func, total, .. } => {
                $v.call_window(func, *total as u32 + 1);
            }
            CallExArgs(_) => {
                let Some(InsnDesc::CallExArgs(CallExArgsDesc {
                    func,
                    npos,
                    nkw,
                    args_splat,
                    kwargs,
                    ..
                })) = $desc
                else {
                    unreachable!("CallExArgs missing descriptor");
                };
                $v.call_window(func, 1 + *npos as u32 + *nkw as u32);
                if *args_splat != NO_ARGS_SPLAT {
                    $v.read_reg(args_splat);
                }
                if *kwargs != NO_KWARGS {
                    $v.read_reg(kwargs);
                }
            }

            // Contiguous read windows with separate dst.
            BuildList(d, base, n) | BuildTuple(d, base, n) => {
                $v.read_window(base, *n);
                $v.write_reg(d);
            }
            BuildString(d, base, n) => {
                $v.read_window(base, *n as u32);
                $v.write_reg(d);
            }
            BuildSlice(d, base) => {
                $v.read_window(base, 3);
                $v.write_reg(d);
            }
            GetSlice(d, obj, base) => {
                $v.read_reg(obj);
                $v.read_window(base, 3);
                $v.write_reg(d);
            }
            BuildDict(d, base, n, _) => {
                $v.read_window(base, 2 * *n);
                $v.write_reg(d);
            }
            CallMethod {
                dst: d,
                obj,
                args_base,
                nargs,
                ..
            } => {
                $v.read_reg(obj);
                $v.read_window(args_base, *nargs as u32);
                $v.write_reg(d);
            }
            CallMethodKw(_) => {
                let Some(InsnDesc::CallMethodKw(CallMethodKwDesc {
                    dst: d,
                    obj,
                    args_base,
                    total,
                    ..
                })) = $desc
                else {
                    unreachable!("CallMethodKw missing descriptor");
                };
                $v.read_reg(obj);
                $v.read_window(args_base, *total as u32);
                $v.write_reg(d);
            }
            Concat { dst: d, base, count } => {
                $v.read_window(base, *count as u32);
                $v.write_reg(d);
            }
            MakeFunction(_) => {
                let Some(InsnDesc::MakeFunction(MakeFunctionDesc {
                    dst: d,
                    defs_base,
                    defs_n,
                    annots_base,
                    annots_n,
                    ..
                })) = $desc
                else {
                    unreachable!("MakeFunction missing descriptor");
                };
                $v.read_window(defs_base, *defs_n);
                $v.read_window(annots_base, *annots_n);
                $v.write_reg(d);
            }
            MakeClass(_) => {
                let Some(InsnDesc::MakeClass(MakeClassDesc {
                    dst: d,
                    bases_base,
                    bases_n,
                    kwarg_base,
                    kwarg_n,
                    meta_reg,
                    ..
                })) = $desc
                else {
                    unreachable!("MakeClass missing descriptor");
                };
                if *meta_reg != NO_METACLASS {
                    $v.read_reg(meta_reg);
                }
                $v.read_window(bases_base, *bases_n);
                $v.read_window(kwarg_base, *kwarg_n);
                $v.write_reg(d);
            }

            // Multi-def contiguous write windows.
            Unpack(base, src, n) => {
                $v.read_reg(src);
                $v.write_window(base, *n);
            }
            UnpackEx {
                src,
                before,
                after,
                dst_base,
            } => {
                $v.read_reg(src);
                $v.write_window(dst_base, *before as u32 + 1 + *after);
            }
            MatchClassPositional(_) => {
                let Some(InsnDesc::MatchClassPositional(MatchClassPositionalDesc {
                    dst_base,
                    subj,
                    cls,
                    n,
                })) = $desc
                else {
                    unreachable!("MatchClassPositional missing descriptor");
                };
                $v.read_reg(subj);
                $v.read_reg(cls);
                $v.write_window(dst_base, *n);
            }
        }
    }};
}

struct ReadVisitor<F>(F);

impl<F: FnMut(Reg)> ReadVisitor<F> {
    #[inline]
    fn read_reg(&mut self, r: &Reg) {
        (self.0)(*r);
    }

    #[inline]
    fn read_window(&mut self, base: &Reg, len: u32) {
        for r in *base..(*base + len) {
            (self.0)(r);
        }
    }

    #[inline]
    fn call_window(&mut self, base: &Reg, len: u32) {
        self.read_window(base, len);
    }

    #[inline]
    fn write_reg(&mut self, _r: &Reg) {}

    #[inline]
    fn write_window(&mut self, _base: &Reg, _len: u32) {}
}

struct WriteVisitor<F>(F);

impl<F: FnMut(Reg)> WriteVisitor<F> {
    #[inline]
    fn read_reg(&mut self, _r: &Reg) {}

    #[inline]
    fn read_window(&mut self, _base: &Reg, _len: u32) {}

    #[inline]
    fn call_window(&mut self, base: &Reg, _len: u32) {
        (self.0)(*base);
    }

    #[inline]
    fn write_reg(&mut self, r: &Reg) {
        (self.0)(*r);
    }

    #[inline]
    fn write_window(&mut self, base: &Reg, len: u32) {
        for r in *base..(*base + len) {
            (self.0)(r);
        }
    }
}

/// Visit every register read by `insn` (for SSA liveness, def-analysis, and lifting).
pub(crate) fn visit_ssa_reads(insn: &Insn, descs: &[InsnDesc], f: impl FnMut(Reg)) {
    let mut visitor = ReadVisitor(f);
    let desc = insn_desc_idx(insn).map(|idx| &descs[idx as usize]);
    dispatch_insn_operands!(insn, desc, visitor);
}

/// Visit every register written by a non-terminator `insn`.
pub(crate) fn visit_ssa_writes(insn: &Insn, descs: &[InsnDesc], f: impl FnMut(Reg)) {
    let mut visitor = WriteVisitor(f);
    let desc = insn_desc_idx(insn).map(|idx| &descs[idx as usize]);
    dispatch_insn_operands!(insn, desc, visitor);
}

struct MaterializeVisitor<'a> {
    uses: &'a [Reg],
    defs: &'a [Reg],
    use_cursor: usize,
    def_cursor: usize,
    scratch_cursor: Reg,
    max_reg_used: &'a mut Reg,
    insns: &'a mut Vec<Insn>,
    linenos: &'a mut Vec<u32>,
    cols: &'a mut Vec<CaretSpan>,
    ln: u32,
    sp: CaretSpan,
    post_call_move: Option<(Reg, Reg)>,
    post_write_window: Option<(Reg, u32)>,
}

impl MaterializeVisitor<'_> {
    #[inline]
    fn emit(&mut self, insn: Insn) {
        self.insns.push(insn);
        self.linenos.push(self.ln);
        self.cols.push(self.sp);
    }

    #[inline]
    fn read_reg(&mut self, r: &mut Reg) {
        *r = self.uses[self.use_cursor];
        self.use_cursor += 1;
    }

    fn read_window(&mut self, base: &mut Reg, len: u32) {
        let win_base = self.scratch_cursor;
        *base = win_base;
        if len > 0 {
            let start = self.use_cursor;
            let end = start + len as usize;
            self.use_cursor = end;
            self.scratch_cursor += len;
            *self.max_reg_used = (*self.max_reg_used).max(win_base + len - 1);
            for (idx, &sr) in self.uses[start..end].iter().enumerate() {
                let dr = win_base + idx as u32;
                if dr != sr {
                    self.emit(Insn::Move(dr, sr));
                }
            }
        }
    }

    fn call_window(&mut self, base: &mut Reg, len: u32) {
        self.read_window(base, len);
        let dst = self.defs[self.def_cursor];
        self.def_cursor += 1;
        self.post_call_move = Some((dst, *base));
    }

    #[inline]
    fn write_reg(&mut self, r: &mut Reg) {
        *r = self.defs[self.def_cursor];
        self.def_cursor += 1;
    }

    fn write_window(&mut self, base: &mut Reg, len: u32) {
        let win_base = self.scratch_cursor;
        *base = win_base;
        if len > 0 {
            self.scratch_cursor += len;
            *self.max_reg_used = (*self.max_reg_used).max(win_base + len - 1);
        }
        self.post_write_window = Some((win_base, len));
    }
}

/// Lower a `MirOp::SideEffectInsn` back to one or more register [`Insn`]s,
/// using `scratch_base` whenever a contiguous register window is required.
#[allow(clippy::too_many_arguments)]
pub(crate) fn materialize_side_effect_insn(
    insn: &Insn,
    src_descs: &[InsnDesc],
    dst_descs: &mut Vec<InsnDesc>,
    uses: &[Reg],
    defs: &[Reg],
    scratch_base: Reg,
    max_reg_used: &mut Reg,
    insns: &mut Vec<Insn>,
    linenos: &mut Vec<u32>,
    cols: &mut Vec<CaretSpan>,
    ln: u32,
    sp: CaretSpan,
) {
    // `YieldFrom` overwrites `sent_reg` in-place on resume; isolate both
    // `iter_reg` and `sent_reg` in scratch slots so no live SSA reg is clobbered.
    if matches!(insn, Insn::YieldFrom { .. }) {
        let w_iter = scratch_base;
        let w_sent = scratch_base + 1;
        *max_reg_used = (*max_reg_used).max(w_sent);
        for emitted in [
            Insn::Move(w_iter, uses[0]),
            Insn::Move(w_sent, uses[1]),
            Insn::YieldFrom {
                iter_reg: w_iter,
                sent_reg: w_sent,
                result_reg: defs[0],
            },
        ] {
            insns.push(emitted);
            linenos.push(ln);
            cols.push(sp);
        }
        return;
    }

    let mut cloned = insn.clone();
    let mut cloned_desc = insn_desc_idx(insn).map(|idx| src_descs[idx as usize].clone());
    let mut visitor = MaterializeVisitor {
        uses,
        defs,
        use_cursor: 0,
        def_cursor: 0,
        scratch_cursor: scratch_base,
        max_reg_used,
        insns,
        linenos,
        cols,
        ln,
        sp,
        post_call_move: None,
        post_write_window: None,
    };
    dispatch_insn_operands!(&mut cloned, cloned_desc.as_mut(), visitor);
    debug_assert_eq!(visitor.use_cursor, uses.len());
    if let Some(new_desc) = cloned_desc {
        let new_idx = push_desc(dst_descs, new_desc);
        set_insn_desc_idx(&mut cloned, new_idx);
    }
    visitor.emit(cloned);

    if let Some((dst, base)) = visitor.post_call_move
        && dst != base
    {
        visitor.emit(Insn::Move(dst, base));
    }
    if let Some((base, _len)) = visitor.post_write_window {
        for (idx, &dr) in defs.iter().enumerate() {
            let sr = base + idx as u32;
            if dr != sr {
                visitor.emit(Insn::Move(dr, sr));
            }
        }
    }
}
