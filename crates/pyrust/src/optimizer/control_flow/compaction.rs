// ─── Compaction helper ─────────────────────────────────────────────────────────

/// Remove instructions where `keep[i]` is `false` and rewrite all jump offsets.
///
/// Removed instructions are treated as transparent: any jump whose target is a
/// removed instruction is redirected to the first kept instruction that follows it.
/// This is correct for no-op removals where "jumping to the no-op" is equivalent
/// to "jumping to whatever comes after it".
pub(crate) fn compact(insns: Vec<Insn>, keep: &[bool]) -> Vec<Insn> {
    let n = insns.len();
    debug_assert_eq!(n, keep.len());

    // to_new[i] = new index of the first kept instruction at or after old index i.
    // to_new[n] = total kept count (past-the-end sentinel for jumps to code.insns.len()).
    let mut to_new = vec![0usize; n + 1];
    let mut cnt = 0usize;
    for i in 0..n {
        to_new[i] = cnt;
        if keep[i] {
            cnt += 1;
        }
    }
    to_new[n] = cnt;

    insns
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(old_i, insn)| rewrite_offsets(insn, old_i, &to_new))
        .collect()
}

/// Rewrite all jump offsets in `insn` using the old→new index table.
/// `old_i` is the pre-compaction index of `insn` (which is guaranteed to be kept).
pub(crate) fn rewrite_offsets(insn: Insn, old_i: usize, to_new: &[usize]) -> Insn {
    rewrite_offsets_with(insn, old_i, to_new, to_new)
}

/// Like [`rewrite_offsets`] but with a separate map for jump targets.  LICM
/// uses this to redirect jumps that targeted a hoisted instruction so they
/// land at the new body start rather than at the hoisted instruction's
/// pre-header position.  See issue #323.
pub(crate) fn rewrite_offsets_with(
    insn: Insn,
    old_i: usize,
    placement_map: &[usize],
    jump_target_map: &[usize],
) -> Insn {
    let fix = |k: i32| -> i32 {
        let old_target = (old_i as i64 + 1 + k as i64) as usize;
        let new_src = placement_map[old_i];
        let new_target = jump_target_map[old_target];
        (new_target as i64 - new_src as i64 - 1) as i32
    };
    use Insn::*;
    match insn {
        Jump(k) => Jump(fix(k)),
        JumpIfFalse(r, k) => JumpIfFalse(r, fix(k)),
        JumpIfTrue(r, k) => JumpIfTrue(r, fix(k)),
        CmpJumpIfFalse(a, op, b, k) => CmpJumpIfFalse(a, op, b, fix(k)),
        CmpJumpIfTrue(a, op, b, k) => CmpJumpIfTrue(a, op, b, fix(k)),
        CmpJumpIfFalseConst(r, op, c, k) => CmpJumpIfFalseConst(r, op, c, fix(k)),
        CmpJumpIfTrueConst(r, op, c, k) => CmpJumpIfTrueConst(r, op, c, fix(k)),
        ForIter(dst, slot, k) => ForIter(dst, slot, fix(k)),
        SetupExcept(k) => SetupExcept(fix(k)),
        MatchExcept(r, k) => MatchExcept(r, fix(k)),
        MatchExceptStar(r, src, dst, k) => MatchExceptStar(r, src, dst, fix(k)),
        other => other,
    }
}
