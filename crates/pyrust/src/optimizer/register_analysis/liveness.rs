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
        | Insn::JumpIfNotInt(_, k)
        | Insn::JumpIfIterNotIntRange(_, k)
        | Insn::JumpIfIterNotIndexedSeq(_, k)
        | Insn::JumpIfIterNotIntRangeExact(_, k)
        | Insn::GetItemSeqIntOrExit(_, _, _, k)
        | Insn::JumpIfNotBuiltinLen(_, k)
        | Insn::LenSeqOrExit(_, _, k)
        | Insn::CountCmpJumpTrue(_, _, _, _, k)
        | Insn::CountCmpJumpFalse(_, _, _, _, k)
        | Insn::CallInlineBinOp { skip: k, .. }
        if *k < 0
    )
}

#[cfg(test)]
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
        let mut overwrote = false;
        for_each_write(insn, |w| {
            if w == r {
                overwrote = true;
            }
        });
        if overwrote {
            return true; // overwritten before any read → old value is dead
        }
    }
    // Hit the true end of the stream without a read → dead; otherwise (window
    // edge reached mid-stream) be conservative.
    end == insns.len()
}

/// Stack-backed bitset for dense temp-register offsets (`r - num_locals`).
/// Covers the first 256 temps with zero heap allocations and spills to a
/// `Vec<u64>` only for unusually wide frames (issue #2905).
struct TempBitset {
    inline: [u64; 4],
    overflow: Vec<u64>,
}

impl TempBitset {
    #[inline]
    fn new() -> Self {
        Self {
            inline: [0; 4],
            overflow: Vec::new(),
        }
    }

    #[inline]
    fn clear(&mut self) {
        self.inline = [0; 4];
        if !self.overflow.is_empty() {
            self.overflow.fill(0);
        }
    }

    #[inline]
    fn insert(&mut self, bit: usize) {
        if bit < 256 {
            self.inline[bit >> 6] |= 1u64 << (bit & 63);
        } else {
            let word = (bit - 256) >> 6;
            if word >= self.overflow.len() {
                self.overflow.resize(word + 1, 0);
            }
            self.overflow[word] |= 1u64 << (bit & 63);
        }
    }

    #[inline]
    fn contains(&self, bit: usize) -> bool {
        if bit < 256 {
            (self.inline[bit >> 6] & (1u64 << (bit & 63))) != 0
        } else {
            let word = (bit - 256) >> 6;
            self.overflow
                .get(word)
                .is_some_and(|&w| (w & (1u64 << (bit & 63))) != 0)
        }
    }
}

/// Small stack-backed `bool` buffer (zero heap allocation for functions with
/// `<= 128` instructions, issue #2905).
enum InlineBoolBuf {
    Inline([bool; 128]),
    Heap(Vec<bool>),
}

impl InlineBoolBuf {
    #[inline]
    fn new(len: usize, init: bool) -> Self {
        if len <= 128 {
            Self::Inline([init; 128])
        } else {
            Self::Heap(vec![init; len])
        }
    }

    #[inline]
    fn as_slice(&self) -> &[bool] {
        match self {
            Self::Inline(arr) => arr.as_slice(),
            Self::Heap(vec) => vec.as_slice(),
        }
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [bool] {
        match self {
            Self::Inline(arr) => arr.as_mut_slice(),
            Self::Heap(vec) => vec.as_mut_slice(),
        }
    }
}

/// Small stack-backed `usize` array for per-temp liveness slots (zero heap
/// allocation for functions with `<= 64` temporaries, issue #2905).
struct TempSlotBuf {
    inline: [usize; 64],
    heap: Vec<usize>,
}

impl TempSlotBuf {
    #[inline]
    fn new(len: usize, init: usize) -> Self {
        if len <= 64 {
            Self {
                inline: [init; 64],
                heap: Vec::new(),
            }
        } else {
            Self {
                inline: [0; 64],
                heap: vec![init; len],
            }
        }
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [usize] {
        if self.heap.is_empty() {
            self.inline.as_mut_slice()
        } else {
            self.heap.as_mut_slice()
        }
    }
}

/// For every index `i`, whether a `LoadConst` at `i` writes a scratch temp
/// whose loaded value is provably unread after `i + 1` on *every* path.
///
/// `pass_binop_const_fusion` folds `LoadConst(r, c); BinOp(dst, lhs, op, r)`
/// into one `BinOpConst` and drops the load, which needs exactly that fact.
/// Its other guards answer the question with whole-function approximations —
/// "is `r` read anywhere later" plus "does a back edge appear later" — and the
/// shape every `while COND: … EXPR == K …` loop compiles to always fails them:
/// the compiler reloads one scratch temp with a different constant per operand,
/// so a later reload counts as a later read, and the loop's own back edge is
/// always "later".  The surviving `LoadConst` of the loop bound then sits on
/// the back-edge target, which is what stops LICM (the temp is no longer
/// single-writer), loop inversion and int-loop versioning from recognising the
/// loop at all (issue #2889).
///
/// Two linear scans compute the precise fact:
///
/// 1. **Block-local temps.**  A temp is *block-local* when every read of it is
///    reached from its defining write by pure fall-through — no jump target
///    lies between the write and the read.  `insn_jump_off` reports
///    exception-handler entries (`SetupExcept` / `MatchExcept` /
///    `MatchExceptStar`) as well as ordinary branches, so a handler counts as
///    an entry edge too.  A block-local temp never carries a value across a
///    control-flow edge, so reaching definitions for it can be decided by a
///    straight-line walk without following jumps.
/// 2. **Reaching scan.**  For a block-local `r` defined at `i`, the loaded
///    value is observable only at indices reached from `i + 2` by fall-through,
///    up to the first jump target (past which any read is fed by its own,
///    later, definition) or the next write of `r`.  If no read of `r` comes
///    first, the definition is dead after `i + 1`.
///
/// Registers below `num_locals` are never reported: named locals are visible
/// through `globals()` / `locals()` frame views without a bytecode read.
fn load_const_dead_after_use(
    insns: &[Insn],
    num_locals: u32,
    is_jump_target: &[bool],
) -> InlineBoolBuf {
    let n = insns.len();
    let mut dead_buf = InlineBoolBuf::new(n, false);
    if n == 0 {
        return dead_buf;
    }

    // Pass 1 (forward): which temps are block-local, and how many temp-register
    // slots the reverse pass needs.  Uses zero-allocation `for_each_read` /
    // `for_each_write` visitors and stack-backed bitsets (issue #2905).
    let mut has_temp = false;
    let mut max_temp: u32 = num_locals;
    let mut defined = TempBitset::new();
    let mut escaping = TempBitset::new();
    for (idx, insn) in insns.iter().enumerate() {
        // Control can enter here from elsewhere, so nothing carries over.
        if is_jump_target[idx] {
            defined.clear();
        }
        for_each_read(insn, |r| {
            if r >= num_locals {
                has_temp = true;
                max_temp = max_temp.max(r);
                let bit = (r - num_locals) as usize;
                if !defined.contains(bit) {
                    escaping.insert(bit);
                }
            }
        });
        // Reads are folded in before writes so an instruction that both reads
        // and writes `r` is judged against the *previous* definition.
        for_each_write(insn, |w| {
            if w >= num_locals {
                has_temp = true;
                max_temp = max_temp.max(w);
                defined.insert((w - num_locals) as usize);
            }
        });
    }
    if !has_temp {
        return dead_buf;
    }

    // Pass 2 (reverse): nearest upcoming read / write per temp register, and
    // the nearest upcoming jump target.
    let dead = dead_buf.as_mut_slice();
    let temp_slots = (max_temp - num_locals) as usize + 1;
    let mut next_read_buf = TempSlotBuf::new(temp_slots, n);
    let mut next_write_buf = TempSlotBuf::new(temp_slots, n);
    let next_read = next_read_buf.as_mut_slice();
    let next_write = next_write_buf.as_mut_slice();
    let mut next_target = n;
    for idx in (0..n).rev() {
        // The running state describes positions `> idx`.  At `idx` that is
        // exactly the region a `LoadConst` at `idx - 1`, consumed by
        // `insns[idx]`, must stay dead in.
        if idx > 0
            && let Insn::LoadConst(r, _) = insns[idx - 1]
            && r >= num_locals
        {
            let slot = (r - num_locals) as usize;
            if !escaping.contains(slot) {
                let (read_at, write_at) = (next_read[slot], next_write[slot]);
                // Live only if a read comes strictly before the first jump target
                // (a read at the target is reached by an incoming edge, so it sees
                // a different definition) and at or before the next write (a
                // same-instruction read+write reads the old value).
                dead[idx - 1] = !(read_at < next_target && read_at <= write_at);
            }
        }
        if is_jump_target[idx] {
            next_target = idx;
        }
        for_each_read(&insns[idx], |r| {
            if r >= num_locals {
                next_read[(r - num_locals) as usize] = idx;
            }
        });
        for_each_write(&insns[idx], |w| {
            if w >= num_locals {
                next_write[(w - num_locals) as usize] = idx;
            }
        });
    }
    dead_buf
}

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
        MakeTypeVar(_, _) => {}
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
