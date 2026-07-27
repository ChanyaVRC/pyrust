// ─── Register liveness helpers ────────────────────────────────────────────────

/// Returns `true` if register `r` is read by any instruction in `insns`.
/// Returns `true` if `insns` contains a backward jump (negative offset).
///
/// A back-edge means the slice re-enters an earlier instruction, so a forward
/// liveness scan alone cannot prove a register is dead — the register may be
/// read on the next loop iteration.  Passes that would remove a `LoadConst`
/// based solely on `reg_is_read_in` must guard with this check.
/// Returns `true` if `insn` is a backward branch (offset `< 0`), i.e. a loop
/// back-edge.
fn insn_is_back_edge(insn: &Insn) -> bool {
    matches!(insn,
        Insn::Jump(k)
        | Insn::JumpIfFalse(_, k)
        | Insn::JumpIfTrue(_, k)
        | Insn::ForIter(_, _, k)
        | Insn::CmpJumpIfFalse(_, _, _, k)
        | Insn::CmpJumpIfTrue(_, _, _, k)
        | Insn::CmpJumpIfFalseConst(_, _, _, k)
        | Insn::CmpJumpIfTrueConst(_, _, _, k)
        if *k < 0
    )
}

fn slice_has_back_edge(insns: &[Insn]) -> bool {
    insns.iter().any(insn_is_back_edge)
}

/// Used as a forward liveness guard before removing a `LoadConst` that produced `r`.
fn reg_is_read_in(insns: &[Insn], r: u32) -> bool {
    insns.iter().any(|insn| insn_reads_reg(insn, r))
}

/// Returns `true` when the value held in register `r` is provably dead starting
/// at `insns[start]` — the first instruction at or after `start` that touches
/// `r` *writes* it without first reading it.  Lets `pass_binop_const_fusion`
/// fuse the "reused scratch register" shape inside loop bodies, which the coarse
/// global `last_read[r]` and `back_edge_after` guards otherwise veto.
///
/// Conservatively returns `false` (treat as live) at the first control-flow
/// instruction: a jump-over could reach a later read of the stale value, so we
/// only reason within a straight-line run.  Scans a tiny fixed window — a
/// scratch temp is always overwritten within a couple of instructions, and an
/// unbounded scan would reintroduce the O(n²) blowup this pass avoids (#2002).
fn scratch_dead_after(insns: &[Insn], start: usize, r: u32) -> bool {
    const WINDOW: usize = 6;
    let end = (start + WINDOW).min(insns.len());
    let mut written = HashSet::new();
    for insn in &insns[start..end] {
        // Any branch / suspend / return ends the straight-line region.
        if matches!(
            insn,
            Insn::Jump(_)
                | Insn::JumpIfFalse(..)
                | Insn::JumpIfTrue(..)
                | Insn::CmpJumpIfFalse(..)
                | Insn::CmpJumpIfTrue(..)
                | Insn::CmpJumpIfFalseConst(..)
                | Insn::CmpJumpIfTrueConst(..)
                | Insn::ForIter(..)
                | Insn::SetupExcept(..)
                | Insn::Yield { .. }
                | Insn::YieldFrom { .. }
                | Insn::Return(..)
                | Insn::ReturnNone
        ) {
            return false;
        }
        if insn_reads_reg(insn, r) {
            return false; // read before any overwrite → value is live
        }
        written.clear();
        collect_writes(insn, &mut written);
        if written.contains(&r) {
            return true; // overwritten before any read → old value is dead
        }
    }
    // Hit the true end of the stream without a read → dead; otherwise (window
    // edge reached mid-stream) be conservative.
    end == insns.len()
}

/// True if the register(s) backing a `KwCallName` include `r`.
fn kwcall_name_reads(name: &crate::bytecode::KwCallName, r: u32) -> bool {
    match name {
        crate::bytecode::KwCallName::Callee(reg) => *reg == r,
        crate::bytecode::KwCallName::Method { obj, .. } => *obj == r,
    }
}

/// Insert the register(s) backing a `KwCallName` into a read set.
fn kwcall_name_insert(name: &crate::bytecode::KwCallName, reads: &mut HashSet<u32>) {
    match name {
        crate::bytecode::KwCallName::Callee(reg) => {
            reads.insert(*reg);
        }
        crate::bytecode::KwCallName::Method { obj, .. } => {
            reads.insert(*obj);
        }
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
        | LoadNone(..)
        | LoadNoneRange { .. }
        | LoadExc(..)
        | ImportModule(..)
        | DeleteName(..)
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
        | DeleteModuleGlobal(..) => false,

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
        | RaiseFrom(a, b)
        | SetAdd(a, b)
        | ListAppend(a, b)
        | ListExtend(a, b)
        | DictUpdate(a, b)
        | GetItem(_, a, b)
        | FormatValueSpec(_, a, b)
        | DeleteItem(a, b) => *a == r || *b == r,

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
        BuildDict(_, base, n) => r >= *base && r < *base + 2 * *n,

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
        MakeTypeVar(_, _) => false,

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
    }
}

/// Collect every register read by `insn` into `reads`.  O(1) per instruction
/// (amortised O(ranges) for range-based instructions).  Use this instead of
/// calling `insn_reads_reg` in a loop to avoid the O(n × k) inner loop.
fn collect_reads(insn: &Insn, reads: &mut HashSet<u32>) {
    use Insn::*;
    match insn {
        LoadConst(..)
        | LoadGlobal(..)
        | LoadCell(..)
        | LoadNone(..)
        | LoadExc(..)
        | ImportModule(..)
        | DeleteName(..)
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
        | DeleteModuleGlobal(..) => {}

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
        | CmpJumpIfTrueConst(s, _, _, _)
        | MatchExcept(s, _)
        | RecordClassStore(s)
        | RecordClassDel(s)
        | PushExcContext(s)
        | SyncModuleGlobal(s, _) => {
            reads.insert(*s);
        }
        MatchExceptStar(type_r, src, _, _) => {
            reads.insert(*type_r);
            reads.insert(*src);
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
            reads.insert(*a);
            reads.insert(*b);
        }

        SetAttr(obj, _, val) | SetTypeVarAttr(obj, _, val) => {
            reads.insert(*obj);
            reads.insert(*val);
        }
        SetItem(a, b, c) => {
            reads.insert(*a);
            reads.insert(*b);
            reads.insert(*c);
        }

        DictMergeKwCall { dict, src, name } => {
            reads.insert(*dict);
            reads.insert(*src);
            kwcall_name_insert(name, reads);
        }
        SetItemKwCall {
            dict,
            key,
            val,
            name,
        } => {
            reads.insert(*dict);
            reads.insert(*key);
            reads.insert(*val);
            kwcall_name_insert(name, reads);
        }

        Call(base, argc) | CallMemo(base, argc) => {
            for r in *base..=(*base + *argc as u32) {
                reads.insert(r);
            }
        }
        CallKw { func, total, .. } => {
            for r in *func..=(*func + *total as u32) {
                reads.insert(r);
            }
        }
        CallEx { func, npos, kwargs } => {
            for r in *func..=(*func + *npos as u32) {
                reads.insert(r);
            }
            reads.insert(*kwargs);
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
                reads.insert(r);
            }
            reads.insert(*args_splat);
            if *kwargs != crate::bytecode::NO_KWARGS {
                reads.insert(*kwargs);
            }
        }
        BuildList(_, base, n) | BuildTuple(_, base, n) => {
            for r in *base..*base + *n {
                reads.insert(r);
            }
        }
        BuildString(_, base, n) => {
            for r in *base..*base + *n as u32 {
                reads.insert(r);
            }
        }
        BuildSlice(_, base) => {
            for r in *base..*base + 3 {
                reads.insert(r);
            }
        }
        GetSlice(_, obj, base) => {
            reads.insert(*obj);
            for r in *base..*base + 3 {
                reads.insert(r);
            }
        }
        BuildDict(_, base, n) => {
            for r in *base..*base + 2 * *n {
                reads.insert(r);
            }
        }
        CallMethod {
            obj,
            args_base,
            nargs,
            ..
        } => {
            reads.insert(*obj);
            for r in *args_base..*args_base + *nargs as u32 {
                reads.insert(r);
            }
        }
        CallMethodKw {
            obj,
            args_base,
            total,
            ..
        } => {
            reads.insert(*obj);
            for r in *args_base..*args_base + *total as u32 {
                reads.insert(r);
            }
        }
        CallMethodExpanded {
            obj,
            pos_list,
            kw_dict,
            ..
        } => {
            reads.insert(*obj);
            reads.insert(*pos_list);
            reads.insert(*kw_dict);
        }
        MakeFunction(_, _, defs_base, defs_n, annots_base, annots_n) => {
            for r in *defs_base..*defs_base + *defs_n {
                reads.insert(r);
            }
            if *annots_n > 0 {
                for r in *annots_base..*annots_base + *annots_n {
                    reads.insert(r);
                }
            }
        }
        MakeClass(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n) => {
            for r in *bases_base..*bases_base + *bases_n {
                reads.insert(r);
            }
            for r in *kwarg_base..*kwarg_base + *kwarg_n {
                reads.insert(r);
            }
        }
        MakeClassMeta(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n, meta_reg) => {
            reads.insert(*meta_reg);
            for r in *bases_base..*bases_base + *bases_n {
                reads.insert(r);
            }
            for r in *kwarg_base..*kwarg_base + *kwarg_n {
                reads.insert(r);
            }
        }
        MakeTypeAlias(_, _, value_reg, params_reg) => {
            reads.insert(*value_reg);
            reads.insert(*params_reg);
        }
        MakeTypeVar(_, _) => {}
        Yield { src, .. } => {
            reads.insert(*src);
        }
        YieldFrom {
            iter_reg, sent_reg, ..
        } => {
            reads.insert(*iter_reg);
            reads.insert(*sent_reg);
        }
        UnpackEx { src, .. } => {
            reads.insert(*src);
        }
        LoadNoneRange { .. } => {}
        Concat { base, count, .. } => {
            for r in *base..*base + *count as u32 {
                reads.insert(r);
            }
        }
        // MatchClassPositional reads subj and cls.
        MatchClassPositional { subj, cls, .. } => {
            reads.insert(*subj);
            reads.insert(*cls);
        }
    }
}
