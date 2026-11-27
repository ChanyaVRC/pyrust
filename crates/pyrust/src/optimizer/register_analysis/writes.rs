// ─── Instruction register-write visitors ──────────────────────────────────────

/// Return the single destination register of `insn`, if any.
/// Used to precisely invalidate register-fact maps without clearing them entirely.
fn writable_dst(insn: &Insn, descs: &[InsnDesc]) -> Option<u32> {
    use Insn::*;
    match insn {
        LoadGlobal(r, _)
        | LoadClassName(r, _, _)
        | LoadCell(r, _)
        | LoadClassDeref(r, _)
        | LoadNone(r)
        | DeleteLocal(r, _, _)
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
        // GetAwaitable writes the driving iterator into `r`; without this arm
        // copy-prop fails to kill a `Move(r, src)` alias on `r`, mis-substituting
        // a later read of the iterator (e.g. `YieldFrom.iter_reg`) back to `src`
        // — surfaced by `await f(…, kw=v)`, whose variadic-call lowering emits an
        // arg `Move` into the slot that becomes the await iterator (issue #2298).
        | GetAwaitable(r, _)
        | Call(r, _)
        | CallMemo(r, _)
        | CallKw { func: r, .. }
        | BuildList(r, _, _)
        | BuildListReserve(r, _)
        | BuildTuple(r, _, _)
        | BuildString(r, _, _)
        | BuildSlice(r, _)
        | BuildDict(r, _, _, _)
        | ImportModule(r, _)
        | LoadExc(r)
        | LoadExcTraceback(r, _)
        | MakeTypeAlias(r, _, _, _)
        | MakeTypeVar(r, _, _) => Some(*r),
        CallExArgs(desc_idx) => Some(descs[*desc_idx as usize].as_call_ex_args().func),
        MakeFunction(desc_idx) => Some(descs[*desc_idx as usize].as_make_function().dst),
        MakeClass(desc_idx) => Some(descs[*desc_idx as usize].as_make_class().dst),
        CallMethodKw(desc_idx) => Some(descs[*desc_idx as usize].as_call_method_kw().dst),
        CallMethodExpanded(desc_idx) => {
            Some(descs[*desc_idx as usize].as_call_method_expanded().dst)
        }
        // `LoadSuperMethod` writes two registers and so, like `Unpack`, has no
        // single destination: callers that kill facts handle it explicitly.
        CallSuperMethod(desc_idx) => Some(descs[*desc_idx as usize].as_call_super_method().dst),
        CallMethod { dst, .. }
        | Concat { dst, .. }
        // Yield writes the caller's sent value into `dst` on resume; aliases
        // through `dst` are stale after this instruction.
        | Yield { dst, .. } => Some(*dst),
        // ForIter writes its destination on each successful iteration.
        ForIter(dst, _, _) => Some(*dst),
        _ => None,
    }
}

/// Visit every register *written* (defined) by `insn` without allocating.
fn for_each_write(insn: &Insn, descs: &[InsnDesc], mut f: impl FnMut(u32)) {
    use Insn::*;
    match insn {
        LoadConst(r, _)
        | LoadGlobal(r, _)
        | LoadClassName(r, _, _)
        | LoadCell(r, _)
        | LoadClassDeref(r, _)
        | LoadNone(r)
        | LoadExc(r)
        | LoadExcTraceback(r, _)
        | ImportModule(r, _)
        | MakeTypeAlias(r, _, _, _)
        | MakeTypeVar(r, _, _)
        | BuildList(r, _, _)
        | BuildListReserve(r, _)
        | BuildTuple(r, _, _)
        | BuildString(r, _, _)
        | BuildSlice(r, _)
        | BuildDict(r, _, _, _)
        | BinOp(r, _, _, _)
        | BinOpInPlace(r, _, _, _)
        | BinOpConst(r, _, _, _, _)
        | CountCmpJumpTrue(r, _, _, _, _)
        | CountCmpJumpFalse(r, _, _, _, _)
        | BinOpImm(r, _, _, _, _)
        | UnaryOp(r, _, _)
        | FormatValue(r, _)
        | FormatValueSpec(r, _, _)
        | MatchSeqExcluded(r, _)
        | MatchMapping(r, _)
        | GetAttr(r, _, _)
        | GetAttrForWith(r, _, _, _)
        | ImportFromAttr(r, _, _)
        | GetItem(r, _, _)
        | GetItemSeqIntOrExit(r, _, _, _)
        | LenSeqOrExit(r, _, _)
        | GetSlice(r, _, _)
        | GetAwaitable(r, _)
        | Call(r, _)
        | CallMemo(r, _)
        | CallKw { func: r, .. }
        | Move(r, _)
        | DeleteLocal(r, _, _) => f(*r),
        CallInlineBinOp(desc_idx, _) => f(descs[*desc_idx as usize].as_call_inline_binop().dst),
        CallExArgs(desc_idx) => f(descs[*desc_idx as usize].as_call_ex_args().func),
        MakeFunction(desc_idx) => f(descs[*desc_idx as usize].as_make_function().dst),
        MakeClass(desc_idx) => f(descs[*desc_idx as usize].as_make_class().dst),
        CallMethodKw(desc_idx) => f(descs[*desc_idx as usize].as_call_method_kw().dst),
        CallMethodExpanded(desc_idx) => f(descs[*desc_idx as usize].as_call_method_expanded().dst),
        LoadSuperMethod { func, recv, .. } => {
            f(*func);
            f(*recv);
        }
        CallSuperMethod(desc_idx) => f(descs[*desc_idx as usize].as_call_super_method().dst),
        MatchExceptStar(desc_idx, _) => {
            let desc = descs[*desc_idx as usize].as_match_except_star();
            f(desc.src_group); // src_group is updated to remaining
            f(desc.matched_dst); // matched_dst gets the sub-group
        }
        CallMethod { dst, .. } | Yield { dst, .. } => f(*dst),
        YieldFrom {
            result_reg,
            sent_reg,
            ..
        } => {
            f(*result_reg);
            // sent_reg is also written (to None before suspension and to the
            // sent value on each resume), so the optimizer must not assume it
            // retains its pre-YieldFrom value after this instruction.
            f(*sent_reg);
        }
        ForIter(dst, _, _) => f(*dst),
        Unpack(base, _, n) => {
            for i in 0..*n {
                f(base + i);
            }
        }
        UnpackEx {
            dst_base,
            before,
            after,
            ..
        } => {
            for i in 0..(*before as u32 + 1 + *after) {
                f(dst_base + i);
            }
        }
        Concat { dst, .. } => f(*dst),
        // Instructions that don't write to any register.
        StoreGlobal(..)
        | StoreCell(..)
        | ImportStar(..)
        | SetAttr(..)
        | SetTypeVarAttr(..)
        | SetItem(..)
        | DeleteAttr(..)
        | DeleteItem(..)
        | DeleteName(..)
        | DeleteCell(..)
        | PushTypeParamEnv
        | PopTypeParamEnv
        | GetIter(..)
        | Jump(..)
        | JumpIfIterNotIntRange(..)
        | JumpIfIterNotIndexedSeq(..)
        | JumpIfIterNotIntRangeExact(..)
        | JumpIfFalse(..)
        | JumpIfTrue(..)
        | CmpJumpIfFalse(..)
        | CmpJumpIfTrue(..)
        | CmpJumpIfFalseConst(..)
        | CmpJumpIfTrueConst(..)
        | JumpIfNotInt(..)
        | JumpIfNotBuiltinLen(..)
        | Return(..)
        | ReturnNone
        | RaiseValue(..)
        | RaiseExceptStarResidual(..)
        | RaiseFrom(..)
        | RaiseReRaise
        | RaiseAssert(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | MatchExcept(..)
        | PushExcContext(..)
        | PopExcContext
        | CheckLocal(..)
        | PrintExpr(..)
        | SetAdd(..)
        | SumStep(..)
        | ListAppend(..)
        | ListExtend(..)
        | ListExtendCall(..)
        | DictUpdate(..)
        | DictMergeKwCall(..)
        | SetItemKwCall(..)
        | RecordClassStore(..)
        | RecordClassDel(..)
        | SyncModuleGlobal(..)
        | DeleteModuleGlobal(..) => {}
        // MatchClassPositional writes dst_base..dst_base+n.
        MatchClassPositional(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_match_class_positional();
            for i in 0..desc.window_len() {
                f(desc.dst_base + i);
            }
        }
    }
}

/// Collect all registers *written* (defined) by `insn` into `written`.
fn collect_writes(insn: &Insn, descs: &[InsnDesc], written: &mut HashSet<u32>) {
    for_each_write(insn, descs, |r| {
        written.insert(r);
    });
}
