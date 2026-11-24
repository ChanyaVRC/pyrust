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
fn insn_reads_reg(insn: &Insn, r: u32) -> bool {
    use Insn::*;
    match insn {
        // No register sources.
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
        | JumpIfIterNotIntRange(..)
        | JumpIfIterNotIndexedSeq(..)
        | JumpIfIterNotIntRangeExact(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | PopExcContext
        | ReturnNone
        | RaiseReRaise
        | RaiseAssertNoMsg
        | ForIter(..)
        | DeleteModuleGlobal(..) => false,

        LoadClassName(_, local, _) => *local != crate::bytecode::NO_CLASS_LOCAL && r == *local,

        // One source register.
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
        MatchExceptStar(type_r, src, _, _) => *type_r == r || *src == r,

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

        ListExtendCall { list, src, name } => *list == r || *src == r || kwcall_name_reads(name, r),

        SetAttr(obj, _, val) | SetTypeVarAttr(obj, _, val) => *obj == r || *val == r,

        // Three source registers.
        SetItem(a, b, c) => *a == r || *b == r || *c == r,

        // Call-context kwarg merges read their dict + source + the registers
        // backing the callee-name source.
        DictMergeKwCall { dict, src, name } => {
            *dict == r || *src == r || kwcall_name_reads(name, r)
        }
        SetItemKwCall {
            dict,
            key,
            val,
            name,
        } => *dict == r || *key == r || *val == r || kwcall_name_reads(name, r),

        // Range-based: func + args live in consecutive registers.
        Call(base, argc) | CallMemo(base, argc) => r >= *base && r <= *base + *argc as u32,
        // CallKw reads the callee and `total` consecutive arg registers — the
        // same footprint as `Call(func, total)`.
        CallKw { func, total, .. } => r >= *func && r <= *func + *total as u32,
        // CallEx reads the callee + `npos` positional registers (contiguous) and
        // the separate `kwargs` (`**d`) register.
        CallEx { func, npos, kwargs } => (r >= *func && r <= *func + *npos as u32) || r == *kwargs,
        CallExArgs {
            func,
            npos,
            nkw,
            args_splat,
            kwargs,
            ..
        } => {
            (r >= *func && r <= *func + *npos as u32 + *nkw as u32)
                || r == *args_splat
                || (*kwargs != crate::bytecode::NO_KWARGS && r == *kwargs)
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
        CallMethodKw {
            obj,
            args_base,
            total,
            ..
        } => *obj == r || (r >= *args_base && r < *args_base + *total as u32),
        CallMethodExpanded {
            obj,
            pos_list,
            kw_dict,
            ..
        } => *obj == r || *pos_list == r || *kw_dict == r,

        MakeFunction(_, _, defs_base, defs_n, annots_base, annots_n) => {
            (r >= *defs_base && r < *defs_base + *defs_n)
                || (*annots_n > 0 && r >= *annots_base && r < *annots_base + *annots_n)
        }
        MakeClass(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n) => {
            (r >= *bases_base && r < *bases_base + *bases_n)
                || (*kwarg_n > 0 && r >= *kwarg_base && r < *kwarg_base + *kwarg_n)
        }
        MakeClassMeta(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n, meta_reg) => {
            *meta_reg == r
                || (r >= *bases_base && r < *bases_base + *bases_n)
                || (*kwarg_n > 0 && r >= *kwarg_base && r < *kwarg_base + *kwarg_n)
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
        MatchClassPositional { subj, cls, .. } => r == *subj || r == *cls,

        // Guard reads its callee and both argument registers.
        CallInlineBinOp { callee, a, b, .. } => r == *callee || r == *a || r == *b,
    }
}

/// Visit every register read by `insn` without allocating.  O(1) per
/// instruction (amortised O(ranges) for range-based instructions).
fn for_each_read(insn: &Insn, mut f: impl FnMut(u32)) {
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
        | RaiseAssertNoMsg
        | ForIter(..)
        | DeleteModuleGlobal(..) => {}

        LoadClassName(_, local, _) => {
            if *local != crate::bytecode::NO_CLASS_LOCAL {
                f(*local);
            }
        }

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
        MatchExceptStar(type_r, src, _, _) => {
            f(*type_r);
            f(*src);
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

        ListExtendCall { list, src, name } => {
            f(*list);
            f(*src);
            kwcall_name_for_each(name, &mut f);
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
            kwcall_name_for_each(name, &mut f);
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
            kwcall_name_for_each(name, &mut f);
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
            if *kwargs != crate::bytecode::NO_KWARGS {
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
        MakeTypeVar(_, _, _) => {}
        Yield { src, .. } => f(*src),
        YieldFrom {
            iter_reg, sent_reg, ..
        } => {
            f(*iter_reg);
            f(*sent_reg);
        }
        UnpackEx { src, .. } => f(*src),
        LoadNoneRange { .. } => {}
        Concat { base, count, .. } => {
            for r in *base..*base + *count as u32 {
                f(r);
            }
        }
        // MatchClassPositional reads subj and cls.
        MatchClassPositional { subj, cls, .. } => {
            f(*subj);
            f(*cls);
        }

        CallInlineBinOp { callee, a, b, .. } => {
            f(*callee);
            f(*a);
            f(*b);
        }
    }
}

/// Collect every register read by `insn` into `reads`.  O(1) per instruction
/// (amortised O(ranges) for range-based instructions).  Use this instead of
/// calling `insn_reads_reg` in a loop to avoid the O(n × k) inner loop.
fn collect_reads(insn: &Insn, reads: &mut HashSet<u32>) {
    for_each_read(insn, |r| {
        reads.insert(r);
    });
}
