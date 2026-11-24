// ─── CmpJump fusion ────────────────────────────────────────────────────────────

/// Fuse a comparison result into the following conditional jump:
/// - `BinOp(r, lhs, op, rhs) + JumpIfFalse(r, k)` → `CmpJumpIfFalse(lhs, op, rhs, k)`
/// - `BinOp(r, lhs, op, rhs) + JumpIfTrue(r, k)`  → `CmpJumpIfTrue(lhs, op, rhs, k)`
/// - `BinOpConst(r, lhs, op, c) + JumpIfFalse(r, k)` → `CmpJumpIfFalseConst(lhs, op, c, k)`
/// - `BinOpConst(r, lhs, op, c) + JumpIfTrue(r, k)`  → `CmpJumpIfTrueConst(lhs, op, c, k)`
///
/// Only fuses when `r >= num_locals` (temp register — not a named local).
///
/// ## Jump-target guard (issue #2088)
///
/// The `JumpIfFalse(r, k)` at `i+1` is replaced *in place* by a `CmpJump…` that
/// **recomputes** `lhs op rhs` instead of merely testing the already-computed
/// register `r`.  That is only sound when control reaches `i+1` exclusively by
/// falling through from the `BinOp` at `i`.  If another instruction *jumps to*
/// `i+1`, that incoming edge expected the original `JumpIfFalse` to test `r`
/// (e.g. an `and`/`or` short-circuit jump that lands on the trailing
/// conditional jump to re-use its still-false LHS result).  Recomputing
/// `lhs op rhs` on that path produces the wrong branch decision.  So we skip
/// fusion whenever `i+1` is the target of any jump.
/// True if temp register `reg` may be read before being overwritten along any
/// path starting at `start_pc` (bounded BFS over the instruction stream).
fn temp_reg_live_from(insns: &[Insn], start_pc: usize, reg: u32) -> bool {
    let n = insns.len();
    let mut visited = vec![false; n];
    let mut work = Vec::new();
    if start_pc < n {
        visited[start_pc] = true;
        work.push(start_pc);
    }
    while let Some(pc) = work.pop() {
        let insn = &insns[pc];
        if insn_reads_reg(insn, reg) {
            return true;
        }
        let mut overwritten = false;
        for_each_write(insn, |w| {
            if w == reg {
                overwritten = true;
            }
        });
        if overwritten {
            continue;
        }
        if let Some(off) = insn_jump_off(insn) {
            let target = (pc as i64 + 1 + off as i64) as usize;
            if target < n && !visited[target] {
                visited[target] = true;
                work.push(target);
            }
        }
        let falls_through = !matches!(
            insn,
            Insn::Jump(_)
                | Insn::Return(_)
                | Insn::ReturnNone
                | Insn::RaiseValue(_)
                | Insn::RaiseExceptStarResidual(_)
                | Insn::RaiseFrom(..)
                | Insn::RaiseReRaise
                | Insn::RaiseAssert(_)
                | Insn::RaiseAssertNoMsg
        );
        if falls_through && pc + 1 < n && !visited[pc + 1] {
            visited[pc + 1] = true;
            work.push(pc + 1);
        }
    }
    false
}

fn pass_cmpjump_fusion(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    // Indices that are the target of some (forward or backward) jump.  The
    // trailing conditional jump of a fusion candidate must not be such a target.
    let mut jump_targets: HashSet<usize> = HashSet::new();
    for (idx, insn) in transformed.iter().enumerate() {
        if let Some(k) = insn_jump_off(insn) {
            let target = idx as i64 + 1 + k as i64;
            if target >= 0 && (target as usize) < n {
                jump_targets.insert(target as usize);
            }
        }
    }

    let mut i = 0;
    while i + 1 < n {
        if jump_targets.contains(&(i + 1)) {
            i += 1;
            continue;
        }
        let candidate: Option<(u32, i32, Insn)> = match (&transformed[i], &transformed[i + 1]) {
            (Insn::BinOpConst(r, lhs, op, c, _), Insn::JumpIfFalse(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some((*r, *k, Insn::CmpJumpIfFalseConst(*lhs, *op, *c, *k)))
            }
            (Insn::BinOpConst(r, lhs, op, c, _), Insn::JumpIfTrue(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some((*r, *k, Insn::CmpJumpIfTrueConst(*lhs, *op, *c, *k)))
            }
            (Insn::BinOp(r, lhs, op, rhs), Insn::JumpIfFalse(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some((*r, *k, Insn::CmpJumpIfFalse(*lhs, *op, *rhs, *k)))
            }
            (Insn::BinOp(r, lhs, op, rhs), Insn::JumpIfTrue(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some((*r, *k, Insn::CmpJumpIfTrue(*lhs, *op, *rhs, *k)))
            }
            _ => None,
        };
        if let Some((reg, k, new_insn)) = candidate {
            // Soundness check: fusing drops the write to `reg`.  Only fuse when
            // `reg` is dead on both the fallthrough (`i + 2`) and jump target
            // (e.g. a short-circuit `a or b` branch edge still consumes `reg`!).
            let target_pc = (i as i64 + 2 + k as i64) as usize;
            let live_fallthrough = temp_reg_live_from(&transformed, i + 2, reg);
            let live_target = temp_reg_live_from(&transformed, target_pc, reg);
            if !live_fallthrough && !live_target {
                keep[i] = false;
                transformed[i + 1] = new_insn;
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    compact(transformed, &keep)
}
