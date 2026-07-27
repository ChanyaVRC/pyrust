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
        let fused: Option<Insn> = match (&transformed[i], &transformed[i + 1]) {
            (Insn::BinOpConst(r, lhs, op, c, _), Insn::JumpIfFalse(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfFalseConst(*lhs, *op, *c, *k))
            }
            (Insn::BinOpConst(r, lhs, op, c, _), Insn::JumpIfTrue(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfTrueConst(*lhs, *op, *c, *k))
            }
            (Insn::BinOp(r, lhs, op, rhs), Insn::JumpIfFalse(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfFalse(*lhs, *op, *rhs, *k))
            }
            (Insn::BinOp(r, lhs, op, rhs), Insn::JumpIfTrue(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfTrue(*lhs, *op, *rhs, *k))
            }
            _ => None,
        };
        if let Some(new_insn) = fused {
            keep[i] = false;
            transformed[i + 1] = new_insn;
            i += 2;
        } else {
            i += 1;
        }
    }
    compact(transformed, &keep)
}
