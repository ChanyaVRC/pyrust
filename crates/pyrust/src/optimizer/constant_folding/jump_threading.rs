// ─── Jump threading ────────────────────────────────────────────────────────────

/// Thread chains of unconditional `Jump`s so that any jump whose target is
/// itself a `Jump` is redirected to the chain's final non-`Jump` destination.
/// Conditional jumps have only their taken-branch target threaded; the
/// fallthrough path is unchanged.  No instructions are removed in this pass.
///
/// Only **forward** `Jump`s (offset ≥ 0) are followed in the chain. Backward
/// jumps (loop back-edges, offset < 0) are treated as opaque non-`Jump`
/// instructions and terminate the chain, preserving the original loop edge.
fn pass_thread_jumps(insns: Vec<Insn>) -> Vec<Insn> {
    // Follow a chain of unconditional forward Jumps from `start`, returning the
    // index of the first instruction that is NOT an unconditional forward Jump.
    // A visited-set guards against infinite loops (self-referential jumps).
    fn follow(insns: &[Insn], start: usize) -> usize {
        let mut pc = start;
        let mut seen = HashSet::new();
        loop {
            if pc >= insns.len() || !seen.insert(pc) {
                break;
            }
            match &insns[pc] {
                Insn::Jump(k) if *k >= 0 => pc = (pc as i64 + 1 + *k as i64) as usize,
                _ => break,
            }
        }
        pc
    }

    insns
        .iter()
        .enumerate()
        .map(|(i, insn)| {
            let thread = |k: i32| -> i32 {
                let raw = (i as i64 + 1 + k as i64) as usize;
                let final_t = follow(&insns, raw);
                (final_t as i64 - i as i64 - 1) as i32
            };
            use Insn::*;
            match insn.clone() {
                Jump(k) => Jump(thread(k)),
                JumpIfFalse(r, k) => JumpIfFalse(r, thread(k)),
                JumpIfTrue(r, k) => JumpIfTrue(r, thread(k)),
                CmpJumpIfFalse(a, op, b, k) => CmpJumpIfFalse(a, op, b, thread(k)),
                CmpJumpIfTrue(a, op, b, k) => CmpJumpIfTrue(a, op, b, thread(k)),
                CmpJumpIfFalseConst(r, op, c, k) => CmpJumpIfFalseConst(r, op, c, thread(k)),
                CmpJumpIfTrueConst(r, op, c, k) => CmpJumpIfTrueConst(r, op, c, thread(k)),
                ForIter(dst, slot, k) => ForIter(dst, slot, thread(k)),
                SetupExcept(k) => SetupExcept(thread(k)),
                MatchExcept(r, k) => MatchExcept(r, thread(k)),
                MatchExceptStar(r, src, dst, k) => MatchExceptStar(r, src, dst, thread(k)),
                other => other,
            }
        })
        .collect()
}
