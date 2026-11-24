// ─── Exit-block inlining ───────────────────────────────────────────────────────

/// Replace an unconditional `Jump(k)` with the instruction it targets when that
/// target is a single-instruction terminal (`Return(r)` or `ReturnNone`).
///
/// ## Rationale
///
/// Compiled `if/else` branches often look like:
///
/// ```text
/// // true branch
/// LoadConst(r, 1)
/// Jump(k)           ← points to epilogue Return
/// // false branch
/// LoadConst(r, 0)
/// Jump(k)           ← same epilogue Return
/// // epilogue
/// Return(r)
/// ```
///
/// Replacing both Jumps with `Return(r)` directly — a 1-for-1 substitution that
/// leaves all other offsets intact — eliminates two taken branches.  The now-dead
/// epilogue `Return` is removed in the subsequent `pass_dead_code`.
///
/// Only single-instruction terminals are inlined to avoid shifting instruction
/// offsets (which would require a separate offset-fixup pass).
fn pass_exit_inline(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    insns
        .iter()
        .enumerate()
        .map(|(i, insn)| {
            if let Insn::Jump(k) = insn {
                let target = (i as i64 + 1 + *k as i64) as usize;
                if target < n
                    && target != i
                    && let t @ (Insn::Return(_) | Insn::ReturnNone) = &insns[target]
                {
                    return t.clone();
                }
            }
            insn.clone()
        })
        .collect()
}

// ─── NOT-inversion ─────────────────────────────────────────────────────────────

/// Absorb `UnaryOp(r, Not, src)` into the following conditional jump by
/// inverting the branch sense, eliminating the boolean intermediate register.
///
/// ## Patterns
///
/// ```text
/// UnaryOp(r, Not, src) + JumpIfFalse(r, k)  →  JumpIfTrue(src, k)
/// UnaryOp(r, Not, src) + JumpIfTrue(r, k)   →  JumpIfFalse(src, k)
/// ```
///
/// ## Guards
/// - `r >= num_locals`: only fuse temp registers (named locals could be inspected
///   after the branch, e.g. in closures).
/// - `!reg_is_read_in(&insns[i+2..], r)`: `r` must be dead after the jump;
///   the liveness check reuses the existing `reg_is_read_in` helper.
/// - the conditional is not an incoming jump target: fallthrough from the
///   UnaryOp must be its only predecessor.
///
/// ## Correctness
/// `not x` returns `bool`; the branch only tests truthiness.  Because
/// `bool(not x)` has the same truthiness as `not x`, inverting the branch
/// and removing the `UnaryOp` is semantically equivalent.
///
/// Reference: Lua `lcode.c` `jumponcond()`.
fn pass_not_invert(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    use crate::ast::UnaryOp;

    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];
    // The trailing conditional may be a merge point reached from another
    // predecessor that did not execute this UnaryOp (notably the shared truth
    // test after a ternary).  Replacing that test with one of the UnaryOp's
    // source register would change the incoming path's condition.  Only fuse
    // when fallthrough from the UnaryOp is the conditional's sole entry.
    let mut jump_targets: HashSet<usize> = HashSet::new();
    for (idx, insn) in transformed.iter().enumerate() {
        if let Some(offset) = insn_jump_off(insn) {
            let target = idx as i64 + 1 + offset as i64;
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
            (Insn::UnaryOp(r, UnaryOp::Not, src), Insn::JumpIfFalse(cond, k))
                if *r == *cond
                    && *r >= num_locals
                    && !reg_is_read_in(&transformed[i + 2..], *r) =>
            {
                Some(Insn::JumpIfTrue(*src, *k))
            }
            (Insn::UnaryOp(r, UnaryOp::Not, src), Insn::JumpIfTrue(cond, k))
                if *r == *cond
                    && *r >= num_locals
                    && !reg_is_read_in(&transformed[i + 2..], *r) =>
            {
                Some(Insn::JumpIfFalse(*src, *k))
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
