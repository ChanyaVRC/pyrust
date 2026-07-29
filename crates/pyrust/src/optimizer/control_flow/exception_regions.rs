// ─── Active exception-handler regions ─────────────────────────────────────────

/// Compute the active exception-handler stack at entry to every instruction.
///
/// Each stack stores absolute handler PCs, outermost first and innermost last.
/// `None` means that the instruction is unreachable by normal control flow.
/// Exception edges themselves are not followed: `SetupExcept` explicitly seeds
/// its handler target with the pre-push stack and its fallthrough with the
/// handler pushed.
///
/// Returns `None` when two normal-flow predecessors reach one PC with different
/// stacks.  Such a stream cannot be represented by a single per-PC zero-cost
/// exception table, and transformations such as cross-jumping must leave it
/// unchanged rather than making the ambiguity worse.
fn analyze_active_handler_stacks(insns: &[Insn]) -> Option<Vec<Option<Vec<usize>>>> {
    let n = insns.len();
    let mut stack_in: Vec<Option<Vec<usize>>> = vec![None; n];
    let mut work = Vec::new();
    if n > 0 {
        stack_in[0] = Some(Vec::new());
        work.push(0);
    }

    // Propagate `stack` to successor `target`, queueing it if newly reached.
    // An edge just past the stream is a normal function exit.
    fn propagate(
        stack_in: &mut [Option<Vec<usize>>],
        work: &mut Vec<usize>,
        target: usize,
        stack: &[usize],
    ) -> bool {
        if target >= stack_in.len() {
            return true;
        }
        match &stack_in[target] {
            None => {
                stack_in[target] = Some(stack.to_vec());
                work.push(target);
                true
            }
            Some(existing) => existing.as_slice() == stack,
        }
    }

    while let Some(pc) = work.pop() {
        let current = stack_in[pc].clone().expect("queued PC must be reachable");
        let jump_target = |offset: i32| -> usize { (pc as i64 + 1 + i64::from(offset)) as usize };
        let mut propagate_to =
            |target: usize, stack: &[usize]| propagate(&mut stack_in, &mut work, target, stack);

        let consistent = match &insns[pc] {
            // The protected fallthrough sees the new handler.  The handler
            // block itself lives outside its own protected region.
            Insn::SetupExcept(offset) => {
                let handler = jump_target(*offset);
                let mut pushed = current.clone();
                pushed.push(handler);
                propagate_to(pc + 1, &pushed) && propagate_to(handler, &current)
            }
            // Keep the historical tolerant pop semantics: a handler target may
            // deliberately point at a PopExcept that compacting later removes.
            Insn::PopExcept => {
                let mut popped = current.clone();
                popped.pop();
                propagate_to(pc + 1, &popped)
            }
            Insn::Jump(offset) => propagate_to(jump_target(*offset), &current),
            Insn::JumpIfFalse(_, offset)
            | Insn::JumpIfTrue(_, offset)
            | Insn::JumpIfNotInt(_, offset)
            | Insn::JumpIfIterNotIntRange(_, offset)
            | Insn::JumpIfIterNotIndexedSeq(_, offset)
            | Insn::JumpIfIterNotIntRangeExact(_, offset)
            | Insn::GetItemSeqIntOrExit(_, _, _, offset)
            | Insn::JumpIfNotBuiltinLen(_, offset)
            | Insn::LenSeqOrExit(_, _, offset)
            | Insn::CmpJumpIfFalse(_, _, _, offset)
            | Insn::CmpJumpIfTrue(_, _, _, offset)
            | Insn::CmpJumpIfFalseConst(_, _, _, offset)
            | Insn::CmpJumpIfTrueConst(_, _, _, offset)
            | Insn::CountCmpJumpTrue(_, _, _, _, offset)
            | Insn::CountCmpJumpFalse(_, _, _, _, offset)
            | Insn::CallInlineBinOp { skip: offset, .. }
            | Insn::ForIter(_, _, offset)
            | Insn::MatchExcept(_, offset)
            | Insn::MatchExceptStar(_, _, _, offset) => {
                propagate_to(jump_target(*offset), &current) && propagate_to(pc + 1, &current)
            }
            Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseAssert(_)
            | Insn::RaiseAssertNoMsg
            | Insn::RaiseValue(_)
            | Insn::RaiseExceptStarResidual(_)
            | Insn::RaiseFrom(_, _)
            | Insn::RaiseReRaise => true,
            _ => propagate_to(pc + 1, &current),
        };

        if !consistent {
            return None;
        }
    }

    Some(stack_in)
}
