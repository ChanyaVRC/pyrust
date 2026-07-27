use super::*;

#[test]
fn dce_removes_instructions_after_return() {
    // Instructions after Return are unreachable.
    let insns = vec![
        Insn::LoadNone(0),
        Insn::Return(0),
        Insn::LoadNone(1), // unreachable
        Insn::Return(1),   // unreachable
    ];
    let out = pass_dead_code(insns);
    assert_eq!(out.len(), 2);
    assert!(matches!(out[0], Insn::LoadNone(0)));
    assert!(matches!(out[1], Insn::Return(0)));
}

#[test]
fn dce_keeps_instructions_after_conditional_jump() {
    // Both branches of a conditional jump are reachable.
    // [0] JumpIfFalse(r0, 1)  [1] LoadNone(1)  [2] Return(0)
    // Both fallthrough (idx 1) and target (idx 1+1+1=3 — end) are successors,
    // so nothing is removed.
    let insns = vec![
        Insn::JumpIfFalse(0, 1), // target = 0+1+1 = 2 (Return)
        Insn::LoadNone(1),
        Insn::Return(0),
    ];
    let out = pass_dead_code(insns);
    assert_eq!(out.len(), 3, "all instructions reachable");
}

#[test]
fn dce_removes_dead_code_after_unconditional_jump() {
    // [0] Jump(1)   [1] LoadNone(0) <dead>   [2] Return(1)
    let insns = vec![
        Insn::Jump(1),     // jumps to idx 2
        Insn::LoadNone(0), // unreachable
        Insn::Return(1),
    ];
    let out = pass_dead_code(insns);
    assert_eq!(out.len(), 2);
    assert!(
        matches!(out[0], Insn::Jump(0)),
        "offset rewritten: 2→1, new offset = 1-(0+1)=0"
    );
    assert!(matches!(out[1], Insn::Return(1)));
}

#[test]
fn dce_on_compiled_function_with_early_return() {
    let code = compile_fn("def f(x):\n    if x > 0:\n        return 1\n    return 0\n");
    let before = code.fn_protos[0].code.insns.len();
    let optimized = optimize(code);
    let after = optimized.fn_protos[0].code.insns.len();
    assert!(
        after <= before,
        "optimizer should not increase instruction count ({before} → {after})"
    );
}

// ── pass_trivial_nop ──────────────────────────────────────────────────────

#[test]
fn trivial_nop_removes_jump0() {
    // Jump(0) is a no-op: it jumps to the next instruction.
    let insns = vec![
        Insn::LoadNone(0),
        Insn::Jump(0), // <- should be removed
        Insn::Return(0),
    ];
    let out = pass_trivial_nop(insns);
    assert_eq!(out.len(), 2, "Jump(0) should be removed");
    assert!(matches!(out[0], Insn::LoadNone(0)));
    assert!(matches!(out[1], Insn::Return(0)));
}

#[test]
fn trivial_nop_removes_self_move() {
    // Move(r, r) copies a register into itself — no effect.
    let insns = vec![
        Insn::LoadNone(1),
        Insn::Move(2, 2), // <- should be removed
        Insn::Return(1),
    ];
    let out = pass_trivial_nop(insns);
    assert_eq!(out.len(), 2, "Move(r, r) should be removed");
}

#[test]
fn trivial_nop_fixes_jump_over_removed() {
    // A Jump that skips over a removed Jump(0) must have its offset decremented.
    // insns: [0] LoadNone 0   [1] Jump(1)   [2] Jump(0) <removed>   [3] Return 0
    // Jump(1) at idx 1 targets idx 3 (offset 1 = idx 1 + 1 + 1).
    // After removing idx 2: Jump(1) at new-idx 1 should target new-idx 2 (old idx 3),
    // so new offset = 2 - (1+1) = 0.
    let insns = vec![
        Insn::LoadNone(0),
        Insn::Jump(1), // targets idx 3 (Return)
        Insn::Jump(0), // no-op, removed
        Insn::Return(0),
    ];
    let out = pass_trivial_nop(insns);
    assert_eq!(out.len(), 3);
    assert!(
        matches!(out[1], Insn::Jump(0)),
        "offset should decrease by 1"
    );
}

// ── pass_compact_consts ───────────────────────────────────────────────────

#[test]
fn compact_consts_removes_unreferenced_entry() {
    use crate::value::Value;
    // Pool: [10, 99, 20].  Only consts 0 and 2 are referenced.
    // Expected pool after compaction: [10, 20]; indices rewritten.
    let consts = vec![Value::int(10), Value::int(99), Value::int(20)];
    let insns = vec![
        Insn::LoadConst(0, 0), // references pool[0] = 10
        Insn::LoadConst(1, 2), // references pool[2] = 20
        Insn::Return(0),
    ];
    let (out_insns, out_consts) = pass_compact_consts(insns, consts);
    assert_eq!(out_consts.len(), 2, "unreferenced entry should be removed");
    assert!(matches!(
        out_consts[0].kind(),
        crate::value::ValueKind::Int(10)
    ));
    assert!(matches!(
        out_consts[1].kind(),
        crate::value::ValueKind::Int(20)
    ));
    // LoadConst(1, 2) should be rewritten to LoadConst(1, 1)
    assert!(matches!(out_insns[1], Insn::LoadConst(1, 1)));
}

#[test]
fn compact_consts_noop_when_all_referenced() {
    use crate::value::Value;
    let consts = vec![Value::int(1), Value::int(2)];
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::LoadConst(1, 1),
        Insn::Return(0),
    ];
    let (out_insns, out_consts) = pass_compact_consts(insns, consts);
    assert_eq!(out_consts.len(), 2, "no change when all referenced");
    assert!(matches!(out_insns[0], Insn::LoadConst(0, 0)));
    assert!(matches!(out_insns[1], Insn::LoadConst(1, 1)));
}

#[test]
fn compact_consts_on_compiled_dead_branch() {
    // "if True: x=1\nelse: x=2" — the else branch is dead.
    // pass_const_fold+pass_dead_code should eliminate the else body.
    // pass_compact_consts should then remove the orphaned constant 2 from the pool.
    let code = compile_fn("if True:\n    x = 1\nelse:\n    x = 2\n");
    let optimized = optimize(code);
    // After optimization, the constant 2 should not appear in the pool
    // (the dead branch referencing it was removed, then the pool was compacted).
    let has_2 = optimized
        .consts
        .iter()
        .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(2)));
    assert!(
        !has_2,
        "orphaned constant 2 should be removed by pool compaction"
    );
}

// ── pass_not_invert ───────────────────────────────────────────────────────

#[test]
fn not_invert_jumpiffalse_becomes_jumpiftrue() {
    use crate::ast::UnaryOp;
    // [0] UnaryOp(r=5, Not, src=0)   keep=false
    // [1] JumpIfFalse(5, k=1)         target = 1+1+1 = 3 (past-end sentinel)
    // [2] Return(0)
    // After fusion: [0] JumpIfTrue(0, 1)  [1] Return(0)
    // Offset rewrite: old_target=3, to_new[3]=2, new_src=to_new[1]=0 → k=2-0-1=1
    let insns = vec![
        Insn::UnaryOp(5, UnaryOp::Not, 0),
        Insn::JumpIfFalse(5, 1),
        Insn::Return(0),
    ];
    let out = pass_not_invert(insns, 2);
    assert_eq!(out.len(), 2, "UnaryOp should be removed");
    assert!(
        matches!(out[0], Insn::JumpIfTrue(0, 1)),
        "JumpIfFalse(not x) should become JumpIfTrue(x)"
    );
}

#[test]
fn not_invert_jumpiftrue_becomes_jumpiffalse() {
    use crate::ast::UnaryOp;
    // Same layout; k=1 → past-end target.
    let insns = vec![
        Insn::UnaryOp(5, UnaryOp::Not, 0),
        Insn::JumpIfTrue(5, 1),
        Insn::Return(0),
    ];
    let out = pass_not_invert(insns, 2);
    assert_eq!(out.len(), 2, "UnaryOp should be removed");
    assert!(
        matches!(out[0], Insn::JumpIfFalse(0, 1)),
        "JumpIfTrue(not x) should become JumpIfFalse(x)"
    );
}

#[test]
fn not_invert_skips_when_reg_is_local() {
    use crate::ast::UnaryOp;
    // r=1 < num_locals=3 → must not fuse
    let insns = vec![
        Insn::UnaryOp(1, UnaryOp::Not, 0),
        Insn::JumpIfFalse(1, 1),
        Insn::Return(0),
    ];
    let out = pass_not_invert(insns, 3);
    assert_eq!(out.len(), 3, "no fusion when r is a local");
}

#[test]
fn not_invert_skips_when_reg_read_after() {
    use crate::ast::UnaryOp;
    // r=5 is read again after the branch → must not fuse
    let insns = vec![
        Insn::UnaryOp(5, UnaryOp::Not, 0),
        Insn::JumpIfFalse(5, 0),
        Insn::Return(5), // reads r=5 → live
    ];
    let out = pass_not_invert(insns, 2);
    assert_eq!(out.len(), 3, "no fusion when r is live after branch");
}

#[test]
fn not_invert_fuses_when_reg_not_reused() {
    use crate::ast::UnaryOp;
    // Build a case where the Not result register (r=5) is genuinely dead after
    // the branch: src=0 (x), result=5, jump target uses a different register.
    //
    // [0] UnaryOp(5, Not, 0)   r5 = not r0
    // [1] JumpIfFalse(5, 1)    if r5 false: jump past-end
    // [2] Move(2, 0)           r2 = r0  (r5 not read here)
    // [3] Return(2)
    let insns = vec![
        Insn::UnaryOp(5, UnaryOp::Not, 0),
        Insn::JumpIfFalse(5, 1),
        Insn::Move(2, 0),
        Insn::Return(2),
    ];
    let out = pass_not_invert(insns, 2);
    // UnaryOp should be removed; JumpIfFalse→JumpIfTrue
    assert_eq!(out.len(), 3, "UnaryOp should be removed");
    assert!(
        matches!(out[0], Insn::JumpIfTrue(0, _)),
        "JumpIfFalse(not r0) should become JumpIfTrue(r0)"
    );
}

// ── slice_has_back_edge ────────────────────────────────────────────────────

#[test]
fn back_edge_detected_on_negative_jump() {
    // Jump(-2) is a backward edge
    let insns = vec![Insn::Jump(-2)];
    assert!(slice_has_back_edge(&insns));
}

#[test]
fn no_back_edge_in_forward_only_slice() {
    // All jumps are non-negative → no back-edge
    let insns = vec![Insn::JumpIfFalse(0, 1), Insn::Return(0)];
    assert!(!slice_has_back_edge(&insns));
}

#[test]
fn binop_const_fusion_skips_when_back_edge_present() {
    use crate::ast::BinaryOp;
    use crate::value::Value;
    // LoadConst(r5, 0) + BinOp(r3, r2, Mul, r5) + ForIter(r6, 0, -2)
    // The ForIter has a negative offset → back-edge; fusion must not remove LoadConst.
    let consts = vec![Value::int(4)];
    let insns = vec![
        Insn::LoadConst(5, 0),
        Insn::BinOp(3, 2, BinaryOp::Mul, 5),
        Insn::ForIter(6, 0, -2),
        Insn::Return(3),
    ];
    let out = pass_binop_const_fusion(insns, 2);
    // LoadConst must survive — r5 is live on the back-edge
    assert!(
        matches!(out[0], Insn::LoadConst(5, 0)),
        "LoadConst must not be removed when a back-edge is present"
    );
    let _ = consts; // suppress unused warning
}

// ── pass_copy_prop ─────────────────────────────────────────────────────────

#[test]
fn copy_prop_eliminates_move() {
    use crate::ast::BinaryOp;
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::Move(1, 0),
        Insn::BinOp(2, 1, BinaryOp::Add, 3),
        Insn::Return(2),
    ];
    let out = pass_copy_prop(insns, 0);
    assert!(
        matches!(out[2], Insn::BinOp(2, 0, BinaryOp::Add, 3)),
        "r1 should be substituted with r0 in BinOp"
    );
}

#[test]
fn copy_prop_kills_alias_on_move_overwrite() {
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::LoadConst(2, 1),
        Insn::Move(1, 0),
        Insn::Move(0, 2),
        Insn::Return(1),
    ];
    let out = pass_copy_prop(insns, 0);
    assert!(
        matches!(out[4], Insn::Return(1)),
        "r1 alias must be killed when r0 is overwritten"
    );
}

#[test]
fn copy_prop_kills_alias_on_binop_write() {
    use crate::ast::BinaryOp;
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::LoadConst(2, 1),
        Insn::Move(1, 0),
        Insn::BinOp(0, 0, BinaryOp::Add, 2),
        Insn::Return(1),
    ];
    let out = pass_copy_prop(insns, 0);
    assert!(
        matches!(out[4], Insn::Return(1)),
        "r1→r0 alias must be killed when BinOp writes r0"
    );
}

#[test]
fn copy_prop_does_not_substitute_dict_update_receiver() {
    let insns = vec![
        Insn::BuildDict(2, 3, 0),
        Insn::Move(5, 2),
        Insn::BuildDict(4, 3, 0),
        Insn::Move(6, 4),
        Insn::DictUpdate(5, 6),
        Insn::Return(5),
    ];
    let out = pass_copy_prop(insns, 0);
    assert!(
        matches!(out[4], Insn::DictUpdate(5, 4)),
        "DictUpdate: receiver unchanged, src substituted"
    );
}

#[test]
fn copy_prop_invalidates_named_local_alias_on_call() {
    use crate::ast::BinaryOp;
    // Simulates the pattern from issue #671 applied to copy propagation:
    //
    //   [0] LoadConst(r5, 0)      r5 is a temp (>= num_locals=2): consts[0]=5
    //   [1] Move(r0, r5)          r0 is a named local (< num_locals=2)
    //                             copy-prop records copies[r0] = r5
    //   [2] Call(r8, 0)           user call — may write r0 via assign_name
    //                             write-through; copies[r0 → r5] must be evicted
    //   [3] BinOpConst(r3, r0, Add, 0)  must use r0 (not r5)
    //   [4] Return(r3)
    //
    // Without the call-boundary invalidation, copy-prop would replace r0
    // with r5 in [3], producing BinOpConst(r3, r5, Add, 0), which would
    // compute the pre-call value of r5 rather than the updated r0.
    let insns = vec![
        Insn::LoadConst(5, 0),                           // r5 = consts[0]
        Insn::Move(0, 5),                                // r0 (named local) = r5
        Insn::Call(8, 0),                                // call — may clobber r0
        Insn::BinOpConst(3, 0, BinaryOp::Add, 0, false), // r3 = r0 + consts[0]
        Insn::Return(3),
    ];
    // num_locals=2: r0 and r1 are named locals; r2+ are temps.
    let out = pass_copy_prop(insns, 2);
    // After Call, copies[r0 → r5] must be evicted. The BinOpConst at [3]
    // must still use r0, not the aliased r5.
    assert!(
        matches!(out[3], Insn::BinOpConst(3, 0, BinaryOp::Add, 0, ..)),
        "named-local alias r0→r5 must not be propagated past Call: found {:?}",
        out[3]
    );
}

#[test]
fn copy_prop_substitutes_yieldfrom_iter_and_sent_regs() {
    // Regression test for issue #1521: pass_copy_prop had no YieldFrom arm,
    // so iter_reg and sent_reg were never substituted when copy aliases existed.
    //
    // Sequence:
    //   [0] LoadConst(r2, 0)           r2 = some iterator object (const slot 0)
    //   [1] Move(r3, r2)               alias: r3 → r2
    //   [2] LoadNone(r4)               sent_reg initial value
    //   [3] Move(r5, r4)               alias: r5 → r4
    //   [4] LoadNone(r6)               result_reg
    //   [5] YieldFrom { iter_reg: r3, sent_reg: r5, result_reg: r6 }
    //       → after substitution: iter_reg should become r2, sent_reg r4
    //   [6] Return(r6)
    let insns = vec![
        Insn::LoadConst(2, 0),
        Insn::Move(3, 2),
        Insn::LoadNone(4),
        Insn::Move(5, 4),
        Insn::LoadNone(6),
        Insn::YieldFrom {
            iter_reg: 3,
            sent_reg: 5,
            result_reg: 6,
        },
        Insn::Return(6),
    ];
    let out = pass_copy_prop(insns, 0);
    assert!(
        matches!(
            out[5],
            Insn::YieldFrom {
                iter_reg: 2,
                sent_reg: 4,
                result_reg: 6,
            }
        ),
        "iter_reg r3→r2 and sent_reg r5→r4 should be substituted; result_reg must stay r6: found {:?}",
        out[5]
    );
}

// ── pass_fold_const_tuple ─────────────────────────────────────────────────
