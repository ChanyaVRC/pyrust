// ─── Instruction register-read visitors ───────────────────────────────────────

/// True if the register(s) backing a `KwCallName` include `r`.
fn kwcall_name_reads(name: &crate::bytecode::KwCallName, r: u32) -> bool {
    match name {
        crate::bytecode::KwCallName::Callee(reg) => *reg == r,
        crate::bytecode::KwCallName::Method { obj, .. } => *obj == r,
    }
}

/// Visit the register(s) backing a `KwCallName`.
#[inline]
fn kwcall_name_for_each(name: &crate::bytecode::KwCallName, f: &mut impl FnMut(u32)) {
    match name {
        crate::bytecode::KwCallName::Callee(reg) => f(*reg),
        crate::bytecode::KwCallName::Method { obj, .. } => f(*obj),
    }
}

/// Apply a register substitution `s` to the register(s) backing a `KwCallName`.
fn kwcall_name_subst(
    name: crate::bytecode::KwCallName,
    mut s: impl FnMut(u32) -> u32,
) -> crate::bytecode::KwCallName {
    match name {
        crate::bytecode::KwCallName::Callee(reg) => crate::bytecode::KwCallName::Callee(s(reg)),
        crate::bytecode::KwCallName::Method { obj, name_idx } => {
            crate::bytecode::KwCallName::Method {
                obj: s(obj),
                name_idx,
            }
        }
    }
}

/// Returns `true` if `insn` reads the value of register `r`.
fn insn_reads_reg(insn: &Insn, descs: &[InsnDesc], r: u32) -> bool {
    use Insn::*;
    match insn {
        // No register sources.
        LoadConst(..)
        | LoadGlobal(..)
        | LoadCell(..)
        | LoadClassDeref(..)
        | LoadNone(..)
        | LoadExc(..)
        | ImportModule(..)
        | DeleteName(..)
        | DeleteCell(..)
        | PushTypeParamEnv
        | PopTypeParamEnv
        | DeleteLocal(..)
        | Jump(..)
        | JumpIfIterNotIntRange(..)
        | JumpIfIterNotIndexedSeq(..)
        | JumpIfIterNotIntRangeExact(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | PopExcContext
        | ReturnNone
        | RaiseReRaise
        | ForIter(..)
        | DeleteModuleGlobal(..) => false,

        LoadClassName(_, local, _) => *local != crate::bytecode::NO_CLASS_LOCAL && r == *local,
        RaiseAssert(s) => *s != NO_ASSERT_MSG && *s == r,

        // One source register.
        StoreGlobal(_, s)
        | StoreCell(_, s)
        | ImportStar(s)
        | Move(_, s)
        | UnaryOp(_, _, s)
        | FormatValue(_, s)
        | MatchSeqExcluded(_, s)
        | MatchMapping(_, s)
        | Return(s)
        | PrintExpr(s)
        | RaiseValue(s)
        | RaiseExceptStarResidual(s)
        | JumpIfFalse(s, _)
        | JumpIfTrue(s, _)
        | GetIter(_, s)
        | GetAwaitable(_, s)
        | BuildListReserve(_, s)
        | Unpack(_, s, _)
        | CheckLocal(s, _)
        | GetAttr(_, s, _)
        | GetAttrForWith(_, s, _, _)
        | LoadExcTraceback(_, s)
        | ImportFromAttr(_, s, _)
        | DeleteAttr(s, _)
        | BinOpConst(_, s, _, _, _)
        | BinOpImm(_, s, _, _, _)
        | CmpJumpIfFalseConst(s, _, _, _)
        | JumpIfNotInt(s, _)
        | JumpIfNotBuiltinLen(s, _)
        | LenSeqOrExit(_, s, _)
        | CmpJumpIfTrueConst(s, _, _, _)
        | MatchExcept(s, _)
        | RecordClassStore(s)
        | RecordClassDel(s)
        | PushExcContext(s)
        | SyncModuleGlobal(s, _) => *s == r,
        MatchExceptStar(desc_idx, _) => {
            let desc = descs[*desc_idx as usize].as_match_except_star();
            desc.type_reg == r || desc.src_group == r
        }

        // Two source registers.
        BinOp(_, a, _, b)
        | BinOpInPlace(_, a, _, b)
        | CmpJumpIfFalse(a, _, b, _)
        | CmpJumpIfTrue(a, _, b, _)
        | CountCmpJumpTrue(a, _, b, _, _)
        | CountCmpJumpFalse(a, _, b, _, _)
        | RaiseFrom(a, b)
        | SetAdd(a, b)
        | ListAppend(a, b)
        | ListExtend(a, b)
        | DictUpdate(a, b)
        | GetItem(_, a, b)
        | GetItemSeqIntOrExit(_, a, b, _)
        | FormatValueSpec(_, a, b)
        | DeleteItem(a, b) => *a == r || *b == r,

        ListExtendCall(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_list_extend_call();
            desc.list == r || desc.src == r || kwcall_name_reads(&desc.name, r)
        }

        SetAttr(obj, _, val) | SetTypeVarAttr(obj, _, val) => *obj == r || *val == r,

        // Three source registers.
        SetItem(a, b, c) => *a == r || *b == r || *c == r,

        // Call-context kwarg merges read their dict + source + the registers
        // backing the callee-name source.
        DictMergeKwCall(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_dict_merge_kw_call();
            desc.dict == r || desc.src == r || kwcall_name_reads(&desc.name, r)
        }
        SetItemKwCall(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_set_item_kw_call();
            desc.dict == r || desc.key == r || desc.val == r || kwcall_name_reads(&desc.name, r)
        }

        // Range-based: func + args live in consecutive registers.
        Call(base, argc) | CallMemo(base, argc) => r >= *base && r <= *base + *argc as u32,
        // CallKw reads the callee and `total` consecutive arg registers — the
        // same footprint as `Call(func, total)`.
        CallKw { func, total, .. } => r >= *func && r <= *func + *total as u32,
        CallExArgs(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_call_ex_args();
            (r >= desc.func && r <= desc.func + desc.npos as u32 + desc.nkw as u32)
                || (desc.args_splat != NO_ARGS_SPLAT && r == desc.args_splat)
                || (desc.kwargs != NO_KWARGS && r == desc.kwargs)
        }
        BuildList(_, base, n) | BuildTuple(_, base, n) => r >= *base && r < *base + *n,
        BuildString(_, base, n) => r >= *base && r < *base + *n as u32,
        // BuildSlice always reads exactly 3 registers: start, stop, step.
        BuildSlice(_, base) => r >= *base && r < *base + 3,
        // GetSlice reads `obj` plus the 3 contiguous bound registers (start,
        // stop, step) starting at `base`.
        GetSlice(_, obj, base) => *obj == r || (r >= *base && r < *base + 3),
        // BuildDict stores n key-value PAIRS — each pair occupies 2 registers,
        // so the live range is base .. base + 2*n (not base + n).
        BuildDict(_, base, n, _) => r >= *base && r < *base + 2 * *n,

        CallMethod {
            obj,
            args_base,
            nargs,
            ..
        } => *obj == r || (r >= *args_base && r < *args_base + *nargs as u32),
        CallMethodKw(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_call_method_kw();
            desc.obj == r || (r >= desc.args_base && r < desc.args_base + desc.total as u32)
        }
        CallMethodExpanded(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_call_method_expanded();
            desc.obj == r || desc.pos_list == r || desc.kw_dict == r
        }

        MakeFunction(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_make_function();
            (r >= desc.defs_base && r < desc.defs_base + desc.defs_n)
                || (desc.annots_n > 0
                    && r >= desc.annots_base
                    && r < desc.annots_base + desc.annots_n)
        }
        MakeClass(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_make_class();
            (desc.meta_reg != NO_METACLASS && desc.meta_reg == r)
                || (r >= desc.bases_base && r < desc.bases_base + desc.bases_n)
                || (desc.kwarg_n > 0 && r >= desc.kwarg_base && r < desc.kwarg_base + desc.kwarg_n)
        }
        MakeTypeAlias(_, _, value_reg, params_reg) => *value_reg == r || *params_reg == r,
        MakeTypeVar(_, _, _) => false,

        // Yield reads src and writes dst.
        Yield { src, dst: _ } => *src == r,

        // YieldFrom reads iter_reg and sent_reg; writes result_reg and sent_reg.
        YieldFrom {
            iter_reg,
            sent_reg,
            result_reg: _,
        } => *iter_reg == r || *sent_reg == r,

        // UnpackEx reads src.
        UnpackEx { src, .. } => *src == r,

        // Concat reads base..base+count registers.
        Concat { base, count, .. } => r >= *base && r < *base + *count as u32,

        // MatchClassPositional reads subj and cls.
        MatchClassPositional(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_match_class_positional();
            r == desc.subj || r == desc.cls
        }

        // Guard reads its callee and both argument registers.
        CallInlineBinOp(desc_idx, _) => {
            let desc = descs[*desc_idx as usize].as_call_inline_binop();
            r == desc.callee || r == desc.a || r == desc.b
        }
    }
}

/// Visit every register read by `insn` without allocating.  O(1) per
/// instruction (amortised O(ranges) for range-based instructions).
fn for_each_read(insn: &Insn, descs: &[InsnDesc], mut f: impl FnMut(u32)) {
    use Insn::*;
    match insn {
        LoadConst(..)
        | LoadGlobal(..)
        | LoadCell(..)
        | LoadClassDeref(..)
        | LoadNone(..)
        | LoadExc(..)
        | ImportModule(..)
        | DeleteName(..)
        | DeleteCell(..)
        | PushTypeParamEnv
        | PopTypeParamEnv
        | DeleteLocal(..)
        | Jump(..)
        | JumpIfIterNotIntRange(..)
        | JumpIfIterNotIndexedSeq(..)
        | JumpIfIterNotIntRangeExact(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | PopExcContext
        | ReturnNone
        | RaiseReRaise
        | ForIter(..)
        | DeleteModuleGlobal(..) => {}

        LoadClassName(_, local, _) => {
            if *local != crate::bytecode::NO_CLASS_LOCAL {
                f(*local);
            }
        }
        RaiseAssert(s) => {
            if *s != NO_ASSERT_MSG {
                f(*s);
            }
        }

        StoreGlobal(_, s)
        | StoreCell(_, s)
        | ImportStar(s)
        | Move(_, s)
        | UnaryOp(_, _, s)
        | FormatValue(_, s)
        | MatchSeqExcluded(_, s)
        | MatchMapping(_, s)
        | Return(s)
        | PrintExpr(s)
        | RaiseValue(s)
        | RaiseExceptStarResidual(s)
        | JumpIfFalse(s, _)
        | JumpIfTrue(s, _)
        | GetIter(_, s)
        | GetAwaitable(_, s)
        | BuildListReserve(_, s)
        | Unpack(_, s, _)
        | CheckLocal(s, _)
        | GetAttr(_, s, _)
        | GetAttrForWith(_, s, _, _)
        | LoadExcTraceback(_, s)
        | ImportFromAttr(_, s, _)
        | DeleteAttr(s, _)
        | BinOpConst(_, s, _, _, _)
        | BinOpImm(_, s, _, _, _)
        | CmpJumpIfFalseConst(s, _, _, _)
        | JumpIfNotInt(s, _)
        | JumpIfNotBuiltinLen(s, _)
        | LenSeqOrExit(_, s, _)
        | CmpJumpIfTrueConst(s, _, _, _)
        | MatchExcept(s, _)
        | RecordClassStore(s)
        | RecordClassDel(s)
        | PushExcContext(s)
        | SyncModuleGlobal(s, _) => f(*s),
        MatchExceptStar(desc_idx, _) => {
            let desc = descs[*desc_idx as usize].as_match_except_star();
            f(desc.type_reg);
            f(desc.src_group);
        }

        BinOp(_, a, _, b)
        | BinOpInPlace(_, a, _, b)
        | CmpJumpIfFalse(a, _, b, _)
        | CmpJumpIfTrue(a, _, b, _)
        | CountCmpJumpTrue(a, _, b, _, _)
        | CountCmpJumpFalse(a, _, b, _, _)
        | RaiseFrom(a, b)
        | SetAdd(a, b)
        | ListAppend(a, b)
        | ListExtend(a, b)
        | DictUpdate(a, b)
        | GetItem(_, a, b)
        | GetItemSeqIntOrExit(_, a, b, _)
        | FormatValueSpec(_, a, b)
        | DeleteItem(a, b) => {
            f(*a);
            f(*b);
        }

        ListExtendCall(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_list_extend_call();
            f(desc.list);
            f(desc.src);
            kwcall_name_for_each(&desc.name, &mut f);
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

        DictMergeKwCall(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_dict_merge_kw_call();
            f(desc.dict);
            f(desc.src);
            kwcall_name_for_each(&desc.name, &mut f);
        }
        SetItemKwCall(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_set_item_kw_call();
            f(desc.dict);
            f(desc.key);
            f(desc.val);
            kwcall_name_for_each(&desc.name, &mut f);
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
        CallExArgs(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_call_ex_args();
            for r in desc.func..=(desc.func + desc.npos as u32 + desc.nkw as u32) {
                f(r);
            }
            if desc.args_splat != NO_ARGS_SPLAT {
                f(desc.args_splat);
            }
            if desc.kwargs != NO_KWARGS {
                f(desc.kwargs);
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
        CallMethodKw(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_call_method_kw();
            f(desc.obj);
            for r in desc.args_base..desc.args_base + desc.total as u32 {
                f(r);
            }
        }
        CallMethodExpanded(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_call_method_expanded();
            f(desc.obj);
            f(desc.pos_list);
            f(desc.kw_dict);
        }
        MakeFunction(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_make_function();
            for r in desc.defs_base..desc.defs_base + desc.defs_n {
                f(r);
            }
            if desc.annots_n > 0 {
                for r in desc.annots_base..desc.annots_base + desc.annots_n {
                    f(r);
                }
            }
        }
        MakeClass(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_make_class();
            if desc.meta_reg != NO_METACLASS {
                f(desc.meta_reg);
            }
            for r in desc.bases_base..desc.bases_base + desc.bases_n {
                f(r);
            }
            for r in desc.kwarg_base..desc.kwarg_base + desc.kwarg_n {
                f(r);
            }
        }
        MakeTypeAlias(_, _, value_reg, params_reg) => {
            f(*value_reg);
            f(*params_reg);
        }
        MakeTypeVar(_, _, _) => {}
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
        // MatchClassPositional reads subj and cls.
        MatchClassPositional(desc_idx) => {
            let desc = descs[*desc_idx as usize].as_match_class_positional();
            f(desc.subj);
            f(desc.cls);
        }

        CallInlineBinOp(desc_idx, _) => {
            let desc = descs[*desc_idx as usize].as_call_inline_binop();
            f(desc.callee);
            f(desc.a);
            f(desc.b);
        }
    }
}

/// Collect every register read by `insn` into `reads`.  O(1) per instruction
/// (amortised O(ranges) for range-based instructions).  Use this instead of
/// calling `insn_reads_reg` in a loop to avoid the O(n × k) inner loop.
fn collect_reads(insn: &Insn, descs: &[InsnDesc], reads: &mut HashSet<u32>) {
    for_each_read(insn, descs, |r| {
        reads.insert(r);
    });
}
