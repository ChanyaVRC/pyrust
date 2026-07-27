// ─── String concat chain merging ──────────────────────────────────────────────

/// Apply the per-instruction transfer function for the `str_regs` dataflow used
/// by [`pass_concat_merge`].  Updates `str_regs` (the set of registers known to
/// hold a `str`) to reflect the effect of executing `insn`.
///
/// Conservative: a register is only *added* when the instruction provably yields
/// a string; every other write to a register *removes* it from the set.  This is
/// intentionally one-directional (no fixpoint over back-edges) — the caller
/// clears the set at every basic-block boundary, so a register staying in the
/// set always means "definitely a str on this straight-line path".
fn apply_str_regs_transfer(
    insn: &Insn,
    consts: &[Value],
    num_locals: u32,
    str_regs: &mut HashSet<u32>,
) {
    use crate::ast::BinaryOp;
    match insn {
        // Only temporary registers are tracked. Named locals remain reachable
        // through live namespace aliases, so even an earlier exact-str store is
        // not a stable fact for a later instruction.
        Insn::LoadConst(dst, c) => {
            if *dst >= num_locals && matches!(consts[*c as usize].kind(), ValueKind::Str(_)) {
                str_regs.insert(*dst);
            } else {
                str_regs.remove(dst);
            }
        }
        // Compiler moves propagate string-ness from source to destination.
        Insn::Move(dst, src) => {
            if *dst >= num_locals && str_regs.contains(src) {
                str_regs.insert(*dst);
            } else {
                str_regs.remove(dst);
            }
        }
        // `str + str` is a `str`; otherwise the op may not yield a string.
        Insn::BinOp(dst, lhs, BinaryOp::Add, rhs) => {
            if *dst >= num_locals && str_regs.contains(lhs) && str_regs.contains(rhs) {
                str_regs.insert(*dst);
            } else {
                str_regs.remove(dst);
            }
        }
        // `str + const`: a string result iff both sides are strings.
        Insn::BinOpConst(dst, lhs, BinaryOp::Add, c, _) => {
            if *dst >= num_locals
                && str_regs.contains(lhs)
                && matches!(consts[*c as usize].kind(), ValueKind::Str(_))
            {
                str_regs.insert(*dst);
            } else {
                str_regs.remove(dst);
            }
        }
        // These always produce a `str`: f-string lowering / format / a prior
        // Concat fusion / str-typed builtin string methods are not modelled here,
        // but the always-string opcodes are.
        Insn::Concat { dst, .. }
        | Insn::BuildString(dst, _, _)
        | Insn::FormatValue(dst, _)
        | Insn::FormatValueSpec(dst, _, _) => {
            if *dst >= num_locals {
                str_regs.insert(*dst);
            } else {
                str_regs.remove(dst);
            }
        }
        // Any other instruction that writes a register clears its string-ness.
        other => {
            if let Some(dst) = writable_dst(other) {
                str_regs.remove(&dst);
            }
        }
    }

    // Keep this analysis aligned with the shared re-entry policy. The result of
    // the current instruction may be a stable temp, but no named-local fact may
    // survive an instruction capable of invoking Python code or namespace
    // synchronization.
    if may_invalidate_named_locals(insn) {
        str_regs.retain(|reg| *reg >= num_locals);
    }
}

/// Merge a chain of `BinOp(Add)` instructions into a single `Concat` instruction
/// that performs the concatenation in one allocation.
///
/// ## Pattern detected (minimum 3 operands, i.e. 2 BinOps in chain)
///
/// ```text
/// BinOp(t1, r0, Add, r1)          ← t1 is a temp; used only once (next BinOp)
/// BinOp(t2, t1, Add, r2)          ← t2 is a temp; used only once (next BinOp)
/// ...
/// BinOp(dst, t_{n-2}, Add, r_{n-1})
/// ```
///
/// is replaced by:
///
/// ```text
/// Move(base+0, r0)
/// Move(base+1, r1)
/// ...
/// Move(base+n-1, r_{n-1})
/// Concat { dst, base, count: n }
/// ```
///
/// The intermediate `BinOp` instructions are removed.  `pass_dead_store_elim`
/// (which runs after this pass in a second pipeline run, or directly afterward)
/// cleans up any now-unused Move instructions.
///
/// ## Guards
///
/// - Chain must be ≥ 2 BinOps (≥ 3 operands): a 2-operand Concat saves no
///   allocation over a plain `BinOp`.
/// - Every leaf operand must be a temp that is statically proven to hold an
///   exact built-in `str`; named operands are never read ahead of Python's
///   normal left-to-right evaluation.
/// - Each intermediate result register must be a temp (`>= num_locals`).
/// - The intermediate result register must be read exactly once (by the
///   immediately following BinOp).
/// - No BB boundary (jump target) between any two BinOps in the chain.
/// - `count ≤ u8::MAX`.
///
/// ## Register window
///
/// Fresh consecutive registers `[num_regs, num_regs+count)` are allocated for
/// the operand window.  The instruction vector is rebuilt with jump-offset
/// rewriting, growing by 2 for the first chain found.
///
/// ## String-only gate (issue #2383)
///
/// The fusion is only profitable for **string** chains, where it collapses N-1
/// allocations into one.  For int (and other primitive) chains the operand
/// window `Move`s are pure overhead — ~19% slower than the equivalent plain
/// `BinOp` chain.  More importantly, preloading later operands is only
/// semantically valid when no earlier `+` can invoke Python and mutate a live
/// namespace.  We therefore fuse only when every leaf operand is a temporary
/// register statically proven to contain an exact built-in `str`.  Named-local
/// facts are deliberately never tracked.  The runtime Concat handler's own
/// string/non-string check is retained as a correctness backstop.
///
/// Run [`pass_concat_merge_once`] to fixed-point: keep merging until no new
/// chain can be fused.  A single pass handles only the first eligible chain;
/// iterating ensures all chains in a function are folded.
fn pass_concat_merge(
    mut insns: Vec<Insn>,
    num_locals: u32,
    num_regs: &mut u32,
    consts: &[Value],
) -> Vec<Insn> {
    loop {
        let (next, changed) = pass_concat_merge_once(insns, num_locals, num_regs, consts);
        insns = next;
        if !changed {
            return insns;
        }
    }
}

/// Single-pass helper for [`pass_concat_merge`].  Finds the first eligible
/// `BinOp(Add)` chain and fuses it into a `Concat` instruction, then returns
/// `(new_insns, true)`.  Returns `(insns, false)` when no chain exists.
fn pass_concat_merge_once(
    insns: Vec<Insn>,
    num_locals: u32,
    num_regs: &mut u32,
    consts: &[Value],
) -> (Vec<Insn>, bool) {
    use crate::ast::BinaryOp;

    let n = insns.len();
    if n < 2 {
        return (insns, false);
    }

    // Mark BB starts (jump targets): we must not merge across them.
    let mut bb_starts: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
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
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target < n {
                bb_starts.insert(target);
            }
        }
    }

    // Compute use-count per register (number of instruction sites that read it).
    let mut use_count: HashMap<u32, usize> = HashMap::new();
    for insn in &insns {
        visit_read_regs(insn, |r| {
            *use_count.entry(r).or_insert(0) += 1;
        });
    }

    // Forward dataflow: `str_regs_before[pc]` is the set of registers statically
    // known to hold a `str` value just before `insns[pc]` executes.  The set is
    // cleared at every basic-block boundary (a register's type can't be trusted
    // across a join).  Seeded from `str`-typed constants and string-producing
    // instructions; mirrors the `int_regs` analysis used by other passes.  Only
    // chains whose every leaf operand is in this set get fused.
    let mut str_regs_before: Vec<HashSet<u32>> = Vec::with_capacity(n);
    {
        let mut str_regs: HashSet<u32> = HashSet::new();
        for (idx, insn) in insns.iter().enumerate() {
            if bb_starts.contains(&idx) {
                str_regs.clear();
            }
            str_regs_before.push(str_regs.clone());
            apply_str_regs_transfer(insn, consts, num_locals, &mut str_regs);
        }
    }

    // Scan for the first mergeable chain.
    let mut i = 0;
    while i < n {
        // First BinOp(Add) with a temp destination.
        let (t0, lhs0, rhs0) = match &insns[i] {
            Insn::BinOp(dst, lhs, BinaryOp::Add, rhs) if *dst >= num_locals => (*dst, *lhs, *rhs),
            _ => {
                i += 1;
                continue;
            }
        };

        // Extend the chain: BinOp(t_{k+1}, t_k, Add, rhs_{k+1}).
        let mut chain_positions: Vec<usize> = vec![i];
        let mut leaf_operands: Vec<u32> = vec![lhs0, rhs0];
        let mut intermediate_dsts: Vec<u32> = vec![t0];

        let mut j = i + 1;
        loop {
            if j >= n || bb_starts.contains(&j) {
                break;
            }
            let prev_dst = *intermediate_dsts.last().unwrap();
            match &insns[j] {
                Insn::BinOp(dst, lhs, BinaryOp::Add, rhs)
                    if *lhs == prev_dst && *dst >= num_locals =>
                {
                    // Guard: the intermediate `prev_dst` must not be read outside
                    // the chain.  Two sub-cases:
                    //
                    // (A) Fresh-dst (prev_dst != dst): `prev_dst` is a distinct
                    //     temp used exactly once — as this BinOp's LHS.  Check
                    //     use_count == 1 to rule out external reads.
                    //
                    // (B) In-place accumulation (prev_dst == dst): the compiler
                    //     reuses the same temp register (`t = t + x`).  Here
                    //     use_count > 1 because the same register is the LHS of
                    //     multiple chain BinOps, but all those reads happen within
                    //     the chain itself and are being removed by the merge.
                    //     Skip the use_count check; correctness is guaranteed by
                    //     the consecutive-chain structure.
                    let safe = if *dst != prev_dst {
                        // Pattern A: distinct fresh temp — must be single-use.
                        use_count.get(&prev_dst).copied().unwrap_or(0) == 1
                    } else {
                        // Pattern B: in-place accumulation — always safe structurally.
                        true
                    };
                    if !safe {
                        break;
                    }
                    chain_positions.push(j);
                    leaf_operands.push(*rhs);
                    intermediate_dsts.push(*dst);
                    j += 1;
                }
                _ => break,
            }
        }

        // Need at least 2 BinOps (3 operands) to be worth merging.
        if chain_positions.len() < 2 {
            i += 1;
            continue;
        }

        // Preloading operands changes the time at which their registers are
        // read. This is only safe when every leaf is an exact built-in string:
        // each original `+` is then non-reentrant, and temp-only facts also
        // guarantee that namespace aliases cannot replace a leaf between
        // operations.
        if leaf_operands
            .iter()
            .any(|reg| !str_regs_before[i].contains(reg))
        {
            i += 1;
            continue;
        }

        let count = leaf_operands.len();
        let Ok(encoded_count) = u8::try_from(count) else {
            i += 1;
            continue;
        };

        let final_dst = *intermediate_dsts.last().unwrap();

        // Sanity: no leaf operand may be an intermediate result register
        // (would create a use-after-free in the chain removal).
        let intermediates_set: HashSet<u32> = intermediate_dsts[..intermediate_dsts.len() - 1]
            .iter()
            .copied()
            .collect();
        if leaf_operands.iter().any(|r| intermediates_set.contains(r)) {
            i += 1;
            continue;
        }

        // Allocate fresh consecutive registers for the operand window.
        let base = *num_regs;
        let Some(next_num_regs) = num_regs.checked_add(u32::from(encoded_count)) else {
            i += 1;
            continue;
        };
        if next_num_regs > MAX_FRAME_REGS {
            i += 1;
            continue;
        }
        *num_regs = next_num_regs;

        // Rebuild the instruction list, inserting count Moves + 1 Concat in
        // place of the chain, growing the vector by 2.
        //
        // Slots consumed: chain_positions.len() = count-1.
        // Slots emitted:  count Moves + 1 Concat = count+1.
        // Delta: +2 instructions.
        //
        // Instructions before first_pos: unchanged position.
        // Instructions in [first_pos, last_pos]: replaced by new_insn_count slots.
        // Instructions after last_pos: shifted by +2.

        let first_pos = chain_positions[0];
        let last_pos = *chain_positions.last().unwrap();
        let chain_len = chain_positions.len(); // count - 1
        let new_insn_count = count + 1; // count Moves + 1 Concat
        // delta = new_insn_count - chain_len = (count+1) - (count-1) = +2
        let delta: i64 = new_insn_count as i64 - chain_len as i64;

        let mut old_to_new = vec![0usize; n + 1];
        for k in 0..=first_pos {
            old_to_new[k] = k;
        }
        // Chain positions [first_pos, last_pos] all redirect to first_pos
        // (jump targets that aimed at any chain instruction land at the first Move).
        for k in first_pos..=last_pos {
            old_to_new[k] = first_pos;
        }
        for k in (last_pos + 1)..=n {
            old_to_new[k] = (k as i64 + delta) as usize;
        }

        let mut new_vec: Vec<Insn> = Vec::with_capacity(n + 2);
        for (k, insn) in insns.iter().enumerate() {
            if k == first_pos {
                // Emit count Moves into the fresh register window.
                for (m, &operand_reg) in leaf_operands.iter().enumerate() {
                    new_vec.push(Insn::Move(base + m as u32, operand_reg));
                }
                // Emit the Concat.
                new_vec.push(Insn::Concat {
                    dst: final_dst,
                    base,
                    count: encoded_count,
                });
            } else if k > first_pos && k <= last_pos {
                // Skip the remaining chain BinOps (already replaced above).
            } else {
                // Rewrite jump offsets for non-chain instructions.
                new_vec.push(rewrite_offsets(insn.clone(), k, &old_to_new));
            }
        }

        return (new_vec, true);
    }

    (insns, false)
}

/// Visits every register *read* by `insn`, calling `f` once per register.
/// Used by `pass_concat_merge` to accumulate per-register use counts without
/// allocating a temporary `Vec`.
fn visit_read_regs(insn: &Insn, mut f: impl FnMut(u32)) {
    use Insn::*;
    match insn {
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
        | DeleteModuleGlobal(..)
        | Jump(..)
        | JumpIfIterNotIntRange(..)
        | JumpIfIterNotIndexedSeq(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | PopExcContext
        | ReturnNone
        | RaiseReRaise
        | RaiseAssertNoMsg
        | ForIter(..) => {}

        BinOpImm(_, a, _, _, _) | SyncModuleGlobal(a, _) => f(*a),

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
        | CmpJumpIfFalseConst(s, _, _, _)
        | CmpJumpIfTrueConst(s, _, _, _)
        | JumpIfNotInt(s, _)
        | MatchExcept(s, _)
        | RecordClassStore(s)
        | RecordClassDel(s)
        | PushExcContext(s) => f(*s),
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
        | GetItemSeqOrExit(_, a, b, _)
        | FormatValueSpec(_, a, b)
        | DeleteItem(a, b) => {
            f(*a);
            f(*b);
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
            match name {
                crate::bytecode::KwCallName::Callee(reg) => f(*reg),
                crate::bytecode::KwCallName::Method { obj, .. } => f(*obj),
            }
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
            match name {
                crate::bytecode::KwCallName::Callee(reg) => f(*reg),
                crate::bytecode::KwCallName::Method { obj, .. } => f(*obj),
            }
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
        BuildDict(_, base, n) => {
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
