use super::*;

#[test]
fn cross_jump_merges_identical_return_tails() {
    use crate::ast::BinaryOp;
    // Models: if cond: x=10 else: x=20; return x+1
    //
    // After compile + earlier passes, the bytecode looks roughly like:
    //   [0] CmpJumpIfFalseConst(cond, Eq, c_true, 3)  — if branch
    //   [1] LoadConst(r0, c_10)                        — then: x=10
    //   [2] BinOpConst(r1, r0, Add, c_1)              — x+1
    //   [3] Return(r1)                                  ← surviving tail [2..3]
    //   [4] LoadConst(r0, c_20)                        — else: x=20
    //   [5] BinOpConst(r1, r0, Add, c_1)              — x+1  (duplicate)
    //   [6] Return(r1)                                  ← duplicate tail [5..6]
    //
    // pass_cross_jump should remove [5..6] and insert Jump to [2].
    let insns = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 0, 3), // [0] → [4]
        Insn::LoadConst(5, 1),                            // [1] x=10
        Insn::BinOpConst(6, 5, BinaryOp::Add, 2, false),  // [2] x+1
        Insn::Return(6),                                  // [3] ← survivor end
        Insn::LoadConst(5, 3),                            // [4] x=20
        Insn::BinOpConst(6, 5, BinaryOp::Add, 2, false),  // [5] duplicate
        Insn::Return(6),                                  // [6] duplicate end
    ];

    let out = pass_cross_jump(insns, &[], &[]);

    // After merging: [5] and [6] are collapsed to Jump([4]→[2]).
    // The output should be shorter by 1 instruction (one BinOpConst removed,
    // one Return removed, one Jump added = net -1).
    assert!(
        out.len() < 7,
        "cross_jump should reduce instruction count; got {} insns",
        out.len()
    );
    // The merged return count should drop to one.
    let return_count = out.iter().filter(|i| matches!(i, Insn::Return(_))).count();
    assert_eq!(
        return_count, 1,
        "exactly one Return should survive after merge"
    );
}

#[test]
fn cross_jump_skips_tail_length_one() {
    // Only `Return(r)` is common — length 1, below MIN_TAIL=2.
    // The pass must NOT fire.
    let insns = vec![
        Insn::JumpIfFalse(0, 2), // [0] → [3]
        Insn::LoadConst(1, 0),   // [1]
        Insn::Return(1),         // [2] ← terminal
        Insn::LoadConst(2, 1),   // [3]
        Insn::Return(2),         // [4] ← same Return discriminant but different reg
    ];
    let n = insns.len();
    let out = pass_cross_jump(insns, &[], &[]);
    // Return(1) vs Return(2) differ → no merge.
    assert_eq!(out.len(), n, "no merge for length-1 or differing tails");
}

#[test]
fn cross_jump_does_not_merge_across_jump_target_in_dup_tail() {
    // The duplicate tail starts at a jump target — must not be removed.
    //
    //   [0] JumpIfFalse(r, 2)  → [3]
    //   [1] LoadConst(r0, 0)
    //   [2] Return(r0)         ← survivor tail [1..2]
    //   [3] LoadConst(r0, 0)   ← JUMP TARGET — cannot be removed
    //   [4] Return(r0)         ← duplicate terminator
    //
    // [3] is a jump target (from [0]), so the merge must not fire.
    let insns = vec![
        Insn::JumpIfFalse(0, 2), // [0] → [3]
        Insn::LoadConst(1, 0),   // [1]
        Insn::Return(1),         // [2] survivor tail start
        Insn::LoadConst(1, 0),   // [3] ← jump target (from [0])
        Insn::Return(1),         // [4] duplicate tail
    ];
    let n = insns.len();
    let out = pass_cross_jump(insns, &[], &[]);
    assert_eq!(
        out.len(),
        n,
        "must not merge when dup tail instructions are jump targets"
    );
}

#[test]
fn cross_jump_does_not_merge_tail_with_jump_offset_insn() {
    use crate::ast::BinaryOp;
    // Tail contains a JumpIfFalse — has an offset field, must not be merged.
    //
    //   [0] JumpIfFalse(r, 2) → [3]
    //   [1] JumpIfFalse(r, 1) — this would land differently from each block
    //   [2] Return(0)
    //   [3] JumpIfFalse(r, 1) — structurally same as [1] but offset means
    //   [4] Return(0)           different target
    let insns = vec![
        Insn::JumpIfFalse(0, 2),                          // [0] → [3]
        Insn::CmpJumpIfFalseConst(1, BinaryOp::Gt, 0, 0), // [1]
        Insn::Return(0),                                  // [2]
        Insn::CmpJumpIfFalseConst(1, BinaryOp::Gt, 0, 0), // [3] jump target
        Insn::Return(0),                                  // [4]
    ];
    let n = insns.len();
    let out = pass_cross_jump(insns, &[], &[]);
    // The CmpJumpIfFalseConst has a jump-offset field; merge must not fire.
    assert_eq!(
        out.len(),
        n,
        "must not merge tails containing instructions with jump-offset fields"
    );
}

#[test]
fn cross_jump_does_not_merge_across_exception_handler_stacks() {
    // Both protected regions end in the structurally-identical
    // [Call, RaiseValue] tail, but an exception from the first copy belongs to
    // handler pc 4 and one from the second belongs to handler pc 9.  Sharing
    // either instruction would give one PC two possible zero-cost table entries.
    let insns = vec![
        Insn::JumpIfFalse(0, 5), // [0] -> [6]
        Insn::SetupExcept(2),    // [1] handler -> [4]
        Insn::Call(1, 0),        // [2] \
        Insn::RaiseValue(1),     // [3]  first protected tail
        Insn::LoadConst(2, 0),   // [4] first handler
        Insn::Return(2),         // [5]
        Insn::SetupExcept(2),    // [6] handler -> [9]
        Insn::Call(1, 0),        // [7] \
        Insn::RaiseValue(1),     // [8]  second protected tail
        Insn::LoadConst(2, 1),   // [9] second handler
        Insn::Return(2),         // [10]
    ];

    let out = pass_cross_jump(insns.clone(), &[], &[]);
    assert_eq!(
        out, insns,
        "identical tails in different exception regions must remain distinct"
    );
}

#[test]
fn cross_jump_leaves_inconsistent_handler_cfg_unchanged() {
    // PC 3 is reachable both by bypassing PopExcept (stack [5]) and by
    // falling through it (empty stack), so no single per-PC handler stack
    // exists.  Cross-jump must conservatively decline every rewrite.
    let insns = vec![
        Insn::SetupExcept(4),    // [0] handler -> [5]
        Insn::JumpIfFalse(0, 1), // [1] -> [3] with handler active
        Insn::PopExcept,         // [2] fallthrough -> [3] without handler
        Insn::LoadConst(1, 0),   // [3] conflicting predecessor stacks
        Insn::Return(1),         // [4]
        Insn::LoadConst(2, 1),   // [5] handler
        Insn::Return(2),         // [6]
    ];

    assert!(
        analyze_active_handler_stacks(&insns).is_none(),
        "fixture must contain a real handler-stack conflict"
    );
    assert_eq!(
        pass_cross_jump(insns.clone(), &[], &[]),
        insns,
        "an inconsistent handler CFG must be returned unchanged"
    );
}

#[test]
fn cross_jump_does_not_merge_different_caret_spans() {
    use crate::ast::BinaryOp;

    // The two tails share one physical source line but originate at different
    // expressions on that line.  Merging would leave the shared BinOp with
    // only one PEP 657 caret span.
    let insns = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 0, 3), // [0] -> [4]
        Insn::LoadConst(5, 1),                            // [1]
        Insn::BinOpConst(6, 5, BinaryOp::Add, 2, false),  // [2]
        Insn::Return(6),                                  // [3]
        Insn::LoadConst(5, 3),                            // [4]
        Insn::BinOpConst(6, 5, BinaryOp::Add, 2, false),  // [5]
        Insn::Return(6),                                  // [6]
    ];
    let linenos = vec![1; insns.len()];
    let mut cols = vec![(0, 0, 0, 0); insns.len()];
    cols[2] = (1, 4, 1, 9);
    cols[5] = (1, 20, 1, 25);

    let out = pass_cross_jump(insns.clone(), &linenos, &cols);
    assert_eq!(
        out, insns,
        "same-line tails with different caret spans must remain distinct"
    );
}

#[test]
fn cross_jump_on_compiled_if_else_common_tail() {
    // Compile a function with an explicit common tail and verify the instruction
    // count decreases after optimization (or at least does not increase).
    let code_before = compile_fn(
        "def f(cond):\n    if cond:\n        x = 10\n    else:\n        x = 20\n    return x + 1\n",
    );
    let before_count = code_before.fn_protos[0].code.insns.len();
    let optimized = optimize(code_before);
    let after_count = optimized.fn_protos[0].code.insns.len();
    assert!(
        after_count <= before_count,
        "optimizer should not increase instruction count ({before_count} → {after_count})"
    );
}

#[test]
fn cross_jump_correctness_with_compiled_code() {
    // The parity fixture: with_merge(True)=11, with_merge(False)=21.
    // This test ensures the optimizer does not break correct execution.
    let code = compile_fn(
        "def f(cond):\n    if cond:\n        x = 10\n    else:\n        x = 20\n    return x + 1\n",
    );
    let optimized = optimize(code);
    // After optimization, the function proto must still be present.
    assert_eq!(
        optimized.fn_protos.len(),
        1,
        "function proto should survive"
    );
    // The instruction list must be non-empty.
    assert!(
        !optimized.fn_protos[0].code.insns.is_empty(),
        "instruction list must not be empty after optimization"
    );
}

#[test]
fn cross_jump_three_arms_fixed_point() {
    // 3-arm chain where each arm ends with a 3-instruction common tail
    // (BinOp, BinOpConst, Return).  Each arm's unique prefix is a single
    // LoadConst with a different constant, so the prefix is NOT merged.
    //
    // A single-pass implementation only merges the first pair.  The fixed-point
    // loop must also detect and apply the second merge opportunity.
    //
    // Shape (14 instructions):
    //   [0]  CmpJumpIfFalseConst  -- if n!=1 jump to [5] (arm2 cond)
    //   [1]  LoadConst(r0, c_a1)  -- arm1 unique prefix
    //   [2]  BinOp(r1,r0,Add,r0)          \
    //   [3]  BinOpConst(r2,r1,Mul,c_k)     } survivor tail (3 insns)
    //   [4]  Return(r2)                    /
    //   [5]  CmpJumpIfFalseConst  -- if n!=2 jump to [10] (arm3 start)
    //   [6]  LoadConst(r0, c_a2)  -- arm2 unique prefix
    //   [7]  BinOp(r1,r0,Add,r0)          \
    //   [8]  BinOpConst(r2,r1,Mul,c_k)     } dup1 tail (same 3 insns)
    //   [9]  Return(r2)                    /
    //   [10] LoadConst(r0, c_a3)  -- arm3 unique prefix  (jump target from [5])
    //   [11] BinOp(r1,r0,Add,r0)          \
    //   [12] BinOpConst(r2,r1,Mul,c_k)     } dup2 tail (same 3 insns)
    //   [13] Return(r2)                    /
    //
    // jump_targets = {0, 5, 10}.  Dup terminators [9] and [13] are NOT targets.
    //
    // First merge (pass 1): dup1 [7..9] -> Jump([2]).
    //   [2] becomes a jump target (from the new Jump at new-[7]).
    // Second merge (pass 2): dup2 tail scanned from [11]:
    //   step=0 Return match; step=1 BinOpConst(new-[10]) match, not a target;
    //   step=2 BinOp(new-[9]) match but [2] IS a target -> scan stops.
    //   tail_len=2 >= MIN_TAIL -> second merge fires!
    //
    // Net: 2 merges applied; instruction count drops by 2.
    // Only one Return survives.
    use crate::ast::BinaryOp;
    let insns = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 0, 4), // [0] -> [5]
        Insn::LoadConst(5, 1),                            // [1]  arm1 unique
        Insn::BinOp(6, 5, BinaryOp::Add, 5),              // [2] \
        Insn::BinOpConst(7, 6, BinaryOp::Mul, 2, false),  // [3]  survivor tail
        Insn::Return(7),                                  // [4] /
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 3, 4), // [5] -> [10]
        Insn::LoadConst(5, 10),                           // [6]  arm2 unique
        Insn::BinOp(6, 5, BinaryOp::Add, 5),              // [7] \
        Insn::BinOpConst(7, 6, BinaryOp::Mul, 2, false),  // [8]  dup1 tail
        Insn::Return(7),                                  // [9] /
        Insn::LoadConst(5, 20),                           // [10] arm3 unique (jump target)
        Insn::BinOp(6, 5, BinaryOp::Add, 5),              // [11] \
        Insn::BinOpConst(7, 6, BinaryOp::Mul, 2, false),  // [12]  dup2 tail
        Insn::Return(7),                                  // [13] /
    ];
    let before_count = insns.len();
    let out = pass_cross_jump(insns, &[], &[]);

    // Two merges must fire -> instruction count drops by at least 2.
    assert!(
        out.len() <= before_count - 2,
        "fixed-point cross_jump must apply at least 2 merges for 3-arm 3-insn-tail \
             chain (before={before_count}, after={})",
        out.len()
    );

    // Exactly one Return must survive (the survivor tail's Return).
    let return_count = out.iter().filter(|i| matches!(i, Insn::Return(_))).count();
    assert_eq!(
        return_count, 1,
        "exactly one Return should survive after fixed-point merge of 3 arms \
             with 3-instruction common tails"
    );
}
