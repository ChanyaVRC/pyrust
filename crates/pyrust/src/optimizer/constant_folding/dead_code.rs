// ─── Dead code elimination ─────────────────────────────────────────────────────

/// Remove instructions that are unreachable from `pc = 0`.
/// Uses a BFS reachability pass that follows all possible instruction successors
/// (both fallthrough and jump targets, including exception handler targets).
fn pass_dead_code(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    let mut reachable = vec![false; n];
    let mut queue = vec![0usize];

    while let Some(pc) = queue.pop() {
        if pc >= n || reachable[pc] {
            continue;
        }
        reachable[pc] = true;

        let jt = |k: i32| (pc as i64 + 1 + k as i64) as usize;

        match &insns[pc] {
            Insn::Jump(k) => queue.push(jt(*k)),

            Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseValue(_)
            | Insn::RaiseExceptStarResidual(_)
            | Insn::RaiseReRaise
            | Insn::RaiseFrom(_, _)
            | Insn::RaiseAssert(_)
            | Insn::RaiseAssertNoMsg => {}

            Insn::JumpIfFalse(_, k) | Insn::JumpIfTrue(_, k) => {
                queue.push(pc + 1);
                queue.push(jt(*k));
            }
            Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => {
                queue.push(pc + 1);
                queue.push(jt(*k));
            }
            Insn::SetupExcept(k) => {
                queue.push(pc + 1);
                queue.push(jt(*k));
            }
            _ => queue.push(pc + 1),
        }
    }

    compact(insns, &reachable)
}
