use super::*;

#[test]
fn exit_inline_jump_to_return() {
    // Jump(0) at index 0 targets index 1 (Return(r)) → replaced with Return(r).
    let insns = vec![Insn::Jump(0), Insn::Return(5)];
    let out = pass_exit_inline(insns);
    assert_eq!(out.len(), 2, "no instructions removed — only inlined");
    assert!(
        matches!(out[0], Insn::Return(5)),
        "Jump targeting Return should be replaced with Return"
    );
}

#[test]
fn exit_inline_jump_to_return_none() {
    let insns = vec![Insn::Jump(1), Insn::LoadConst(0, 0), Insn::ReturnNone];
    let out = pass_exit_inline(insns);
    // Jump(1) at index 0 targets index 2 (ReturnNone)
    assert!(
        matches!(out[0], Insn::ReturnNone),
        "Jump targeting ReturnNone should be replaced with ReturnNone"
    );
}

#[test]
fn exit_inline_skips_non_terminal_target() {
    use crate::ast::BinaryOp;
    // Jump(0) targets LoadConst — not a terminal, must not be replaced.
    let insns = vec![Insn::Jump(0), Insn::LoadConst(3, 0), Insn::Return(3)];
    let out = pass_exit_inline(insns);
    assert!(
        matches!(out[0], Insn::Jump(0)),
        "Jump to non-terminal should be kept as-is"
    );
    // Suppress unused-import lint
    let _ = BinaryOp::Add;
}

#[test]
fn exit_inline_skips_conditional_jumps() {
    // JumpIfFalse is NOT an unconditional Jump — must not be modified.
    let insns = vec![Insn::JumpIfFalse(0, 0), Insn::Return(0)];
    let out = pass_exit_inline(insns);
    assert!(
        matches!(out[0], Insn::JumpIfFalse(0, 0)),
        "conditional jumps must not be inlined"
    );
}

// ── pass_licm ─────────────────────────────────────────────────────────────

/// Build a minimal loop with a back edge:
///
/// ```text
/// [0]  LoadConst(r5, 0)         ← invariant: hoistable
/// [1]  ForIter(r0, slot0, 2)             ← loop header (target of back edge at [3])
///                                         if counter exhausted: jump to [4]
/// [2]  BinOp(r1, r1, Add, r0)   ← body: uses r5 indirectly via BinOp(not hoistable)
/// [3]  Jump(-3)                 ← back edge → [1]
/// [4]  Return(r1)
/// ```
///
/// After LICM, LoadConst(r5, 0) should appear before the loop header (at index 0
/// in the pre-header block, which is before old index 1).
#[test]
fn licm_hoists_loadconst_before_loop() {
    use crate::ast::BinaryOp;

    // Layout (raw, before LICM):
    //  [0] LoadConst(r5, 0)           — invariant
    //  [1] ForIter(r0, slot0, 2)         — header; jumps to [4] when done
    //  [2] BinOp(r1, r1, Add, r0)     — body
    //  [3] Jump(-3)                   — back edge → [1]
    //  [4] Return(r1)
    //
    // Back edge: Jump(-3) at old index 3 → target = 3+1-3 = 1 → header=1, latch=3
    // Write set for [1..=3]: {r0} (ForIter writes r0), {r1} (BinOp writes r1)
    // LoadConst(r5, 0) is at index 0 — OUTSIDE the loop [1..=3], so LICM does
    // nothing here since 0 < header=1.
    //
    // Adjusted test: put LoadConst INSIDE the loop body, so LICM moves it out.
    //
    //  [0] ForIter(r0, slot0, 3)          — header; jumps to [4] when done
    //  [1] LoadConst(r5, 0)               — invariant (inside loop body)
    //  [2] BinOp(r1, r1, Add, r0)         — not invariant (r0 is written by header)
    //  [3] Jump(-4)                        — back edge → [0]
    //  [4] Return(r1)
    let insns = vec![
        Insn::ForIter(0, 0, 3),              // [0] header, exits to [4]
        Insn::LoadConst(5, 0),               // [1] invariant
        Insn::BinOp(1, 1, BinaryOp::Add, 0), // [2] not invariant
        Insn::Jump(-4),                      // [3] back edge → [0]
        Insn::Return(1),                     // [4]
    ];
    // r0..r4 are named locals; r5 is a temp (>= num_locals=5) → hoistable.
    let out = pass_licm(insns, 5);

    // After hoisting LoadConst(r5, 0) before header [0], the new layout is:
    //  [0] LoadConst(r5, 0)               — hoisted
    //  [1] ForIter(r0, slot0, 2)         — header (offset adjusted: was 3, now 2)
    //  [2] BinOp(r1, r1, Add, r0)
    //  [3] Jump(-3)                        — back edge → [1]
    //  [4] Return(r1)
    assert_eq!(out.len(), 5, "instruction count must not change");
    assert!(
        matches!(out[0], Insn::LoadConst(5, 0)),
        "LoadConst should be hoisted to position 0 (before loop header); got {:?}",
        out[0]
    );
    // The loop header must still be present and its exit offset adjusted to
    // land on the Return (still at the end of the 5-instruction list).
    assert!(
        matches!(out[1], Insn::ForIter(0, 0, _)),
        "loop header should remain at position 1"
    );
}

/// `BinOpConst(dst, src, op, c)` must not be hoisted even when `src` is not
/// written inside the loop body.  Arithmetic can dispatch a user protocol, so
/// moving it to the pre-header would change its timing and make it execute for
/// a zero-trip loop.
#[test]
fn licm_does_not_hoist_binopconst_with_invariant_src() {
    use crate::ast::BinaryOp;

    // Loop layout:
    //  [0] ForIter(r0, slot0, 3)          — header, exits to [4]
    //  [1] BinOpConst(r5, r2, Add, 0)      — protocol dispatch; must stay
    //  [2] BinOp(r1, r1, Add, r0)           — uses r0 (written) → not invariant
    //  [3] Jump(-4)                          — back edge → [0]
    //  [4] Return(r1)
    //
    // r0 is written by ForIter; r1 by BinOp; r2 is untouched.
    let insns = vec![
        Insn::ForIter(0, 0, 3),                          // [0]
        Insn::BinOpConst(5, 2, BinaryOp::Add, 0, false), // [1] protocol dispatch → keep
        Insn::BinOp(1, 1, BinaryOp::Add, 0),             // [2] r0 written → keep
        Insn::Jump(-4),                                  // [3]
        Insn::Return(1),                                 // [4]
    ];
    // r0..r4 are named locals and r5 is a temp, but that alone cannot prove the
    // operands are exact built-in values with non-observable arithmetic.
    let out = pass_licm(insns, 5);

    assert_eq!(out.len(), 5);
    assert!(
        matches!(out[0], Insn::ForIter(0, 0, _)),
        "loop header must remain first; got {:?}",
        out[0]
    );
    assert!(
        matches!(out[1], Insn::BinOpConst(5, 2, BinaryOp::Add, 0, ..)),
        "BinOpConst with an invariant register source must remain in the loop body"
    );
}

/// `LoadConst(dst, idx)` where `dst < num_locals` (a named local) must NOT be
/// hoisted: a zero-trip loop must not unconditionally assign named locals.
///
/// This is the core regression guard for issue #580.
#[test]
fn licm_does_not_hoist_loadconst_to_named_local() {
    // Loop layout:
    //  [0] ForIter(r0, slot0, 2)          — header, exits to [3]
    //  [1] LoadConst(r1, 0)                — r1 IS a named local (< num_locals=5)
    //  [2] Jump(-3)                         — back edge → [0]
    //  [3] Return(r1)
    //
    // r0..r4 are named locals; r1 is named local → LoadConst(r1) must NOT be hoisted.
    let insns = vec![
        Insn::ForIter(0, 0, 2), // [0] header
        Insn::LoadConst(1, 0),  // [1] dst=r1, named local
        Insn::Jump(-3),         // [2] back edge → [0]
        Insn::Return(1),        // [3]
    ];
    let before = insns.clone();
    // r1 < num_locals=5 → named local → must not be hoisted.
    let out = pass_licm(insns, 5);

    // Instruction count must be unchanged (nothing hoisted).
    assert_eq!(out.len(), before.len(), "instruction count must not change");
    assert!(
        matches!(out[0], Insn::ForIter(0, 0, _)),
        "loop header must remain at position 0; LoadConst(r1) must not be hoisted before it"
    );
    assert!(
        matches!(out[1], Insn::LoadConst(1, 0)),
        "LoadConst to named local must stay in loop body"
    );
}

/// A `continue` jump is a back edge to the loop header, but it is not the loop's
/// latch: code later in the body may write the same temporary.  LICM must use
/// the complete loop interval when deciding whether `LoadConst` is the sole
/// writer of its destination.
#[test]
fn licm_continue_backedge_uses_complete_loop_write_set() {
    //  [0] ForIter(r0, slot0, 5)  — header, exits to [6]
    //  [1] LoadConst(r5, 0)       — comparison RHS
    //  [2] JumpIfFalse(r1, 1)     — false path skips continue to [4]
    //  [3] Jump(-4)               — continue back edge → [0]
    //  [4] Move(r5, r0)           — later body path reuses r5
    //  [5] Jump(-6)               — actual loop latch → [0]
    //  [6] ReturnNone
    //
    // If [0..=3] is incorrectly treated as a complete loop, r5 appears to
    // have one writer and LoadConst is hoisted.  In the real [0..=5] loop it
    // has two writers and must remain after ForIter on every iteration.
    let insns = vec![
        Insn::ForIter(0, 0, 5),
        Insn::LoadConst(5, 0),
        Insn::JumpIfFalse(1, 1),
        Insn::Jump(-4),
        Insn::Move(5, 0),
        Insn::Jump(-6),
        Insn::ReturnNone,
    ];

    let out = pass_licm(insns, 5);

    assert!(
        matches!(out[0], Insn::ForIter(0, 0, _)),
        "continue must not truncate the loop write set and hoist LoadConst: {out:?}"
    );
    assert!(matches!(out[1], Insn::LoadConst(5, 0)));
}

#[test]
fn licm_collects_only_the_last_latch_for_each_header() {
    // The first loop has two back edges to [0]: an early continue at [1] and
    // its real latch at [3].  The second loop verifies that aggregation remains
    // per-header instead of collapsing distinct loops.
    let insns = vec![
        Insn::ForIter(0, 0, 3),
        Insn::Jump(-2),
        Insn::LoadNone(1),
        Insn::Jump(-4),
        Insn::ForIter(2, 1, 1),
        Insn::Jump(-2),
        Insn::ReturnNone,
    ];

    assert_eq!(
        collect_complete_loop_intervals(&insns),
        vec![(0, 3), (4, 5)]
    );
}

/// `BinOpConst(dst, src, op, c)` where `dst < num_locals` (a named local) must NOT
/// be hoisted: a zero-trip loop must not unconditionally assign named locals.
#[test]
fn licm_does_not_hoist_binopconst_to_named_local() {
    use crate::ast::BinaryOp;

    // Loop layout:
    //  [0] ForIter(r0, slot0, 2)          — header
    //  [1] BinOpConst(r1, r2, Add, 0)      — r1 IS a named local (< num_locals=5)
    //  [2] Jump(-3)                          — back edge → [0]
    //  [3] Return(r1)
    let insns = vec![
        Insn::ForIter(0, 0, 2),                          // [0]
        Insn::BinOpConst(1, 2, BinaryOp::Add, 0, false), // [1] dst=r1 < 5 → keep
        Insn::Jump(-3),                                  // [2]
        Insn::Return(1),                                 // [3]
    ];
    let before = insns.clone();
    let out = pass_licm(insns, 5);

    assert_eq!(out.len(), before.len(), "instruction count must not change");
    assert!(
        matches!(out[0], Insn::ForIter(0, 0, _)),
        "loop header must remain at position 0"
    );
    assert!(
        matches!(out[1], Insn::BinOpConst(1, 2, BinaryOp::Add, 0, ..)),
        "BinOpConst to named local must stay in loop body"
    );
}

/// `BinOpConst(dst, src, op, c)` where `src` IS written in the loop must NOT
/// be hoisted.
#[test]
fn licm_does_not_hoist_binopconst_with_variant_src() {
    use crate::ast::BinaryOp;

    // r0 is the loop item (written by ForIter); BinOpConst reads r0 → variant.
    //  [0] ForIter(r0, slot0, 3)
    //  [1] BinOpConst(r5, r0, Add, 0)      — r0 IS written → NOT invariant
    //  [2] BinOp(r1, r1, Add, r5)
    //  [3] Jump(-4)
    //  [4] Return(r1)
    let insns = vec![
        Insn::ForIter(0, 0, 3),                          // [0]
        Insn::BinOpConst(5, 0, BinaryOp::Add, 0, false), // [1] r0 in write set → keep
        Insn::BinOp(1, 1, BinaryOp::Add, 5),             // [2]
        Insn::Jump(-4),                                  // [3]
        Insn::Return(1),                                 // [4]
    ];
    let before = insns.clone();
    // r5 is a temp but r0 (src) IS written by ForIter → still not hoisted.
    let out = pass_licm(insns, 5);

    // Nothing should move: BinOpConst reads r0 which is written by ForIter.
    assert_eq!(
        out.len(),
        before.len(),
        "instruction count should not change"
    );
    assert!(
        matches!(out[0], Insn::ForIter(0, 0, _)),
        "loop header must remain at position 0 (nothing hoisted)"
    );
    assert!(
        matches!(out[1], Insn::BinOpConst(5, 0, BinaryOp::Add, 0, ..)),
        "variant BinOpConst must stay in loop body"
    );
}

/// Nested loops: an invariant in the inner loop that is also invariant wrt the
/// outer loop ends up hoisted all the way out of both loops (before the outer
/// header).  Verify that it is no longer inside the inner loop body.
#[test]
fn licm_hoists_inner_invariant_out_of_inner_loop() {
    use crate::ast::BinaryOp;

    // Outer loop: back edge at [7] → header [0].
    // Inner loop: back edge at [5] → inner header [2].
    //
    //  [0] ForIter(r0, slot0, 7)           — outer header, exits to [8]
    //  [1] BinOp(r1, r1, Add, r0)           — outer body (r0 written by outer)
    //  [2] ForIter(r3, slot1, 3)            — inner header, exits to [6]
    //  [3] LoadConst(r9, 0)                 — invariant wrt both loops
    //  [4] BinOp(r4, r4, Add, r3)           — uses r3 (written by inner) → variant
    //  [5] Jump(-4)                          — inner back edge → [2]
    //  [6] BinOp(r1, r1, Add, r4)           — outer body (after inner)
    //  [7] Jump(-8)                          — outer back edge → [0]
    //  [8] Return(r1)
    let insns = vec![
        Insn::ForIter(0, 0, 7),              // [0] outer header
        Insn::BinOp(1, 1, BinaryOp::Add, 0), // [1]
        Insn::ForIter(3, 1, 3),              // [2] inner header
        Insn::LoadConst(9, 0),               // [3] invariant wrt inner
        Insn::BinOp(4, 4, BinaryOp::Add, 3), // [4] variant wrt inner (r3 written)
        Insn::Jump(-4),                      // [5] inner back edge → [2]
        Insn::BinOp(1, 1, BinaryOp::Add, 4), // [6]
        Insn::Jump(-8),                      // [7] outer back edge → [0]
        Insn::Return(1),                     // [8]
    ];
    // r0..r4 are named locals; r9 is a temp (>= num_locals=5) → LoadConst(r9) hoistable.
    let out = pass_licm(insns, 5);

    assert_eq!(out.len(), 9, "total instruction count unchanged");

    // LoadConst(r9, 0) is invariant wrt both loops.  The inner loop processes
    // first and hoists it before the inner header; the outer loop then hoists it
    // again before the outer header.  Either way, it must not remain inside the
    // inner loop body [inner_header..inner_latch].
    //
    // Find the inner header (ForIter for r3) and latch (Jump with negative
    // offset targeting the inner header).  LoadConst(r9, _) must not appear
    // between them.
    let inner_header_pos = out
        .iter()
        .position(|i| matches!(i, Insn::ForIter(3, 1, _)))
        .expect("inner header must still exist");
    let inner_latch_pos = out
        .iter()
        .enumerate()
        .position(|(i, insn)| {
            if let Insn::Jump(k) = insn {
                let target = i as i64 + 1 + *k as i64;
                target == inner_header_pos as i64
            } else {
                false
            }
        })
        .expect("inner back-edge Jump must still exist");

    let loadconst_inside_inner = out[inner_header_pos..=inner_latch_pos]
        .iter()
        .any(|i| matches!(i, Insn::LoadConst(9, _)));
    assert!(
        !loadconst_inside_inner,
        "LoadConst(r9) must be hoisted out of the inner loop body \
             (inner_header={inner_header_pos}, inner_latch={inner_latch_pos})"
    );

    // The overall structure must remain intact: outer header and inner header
    // must still exist.
    assert!(
        out.iter().any(|i| matches!(i, Insn::ForIter(0, 0, _))),
        "outer loop header must still exist"
    );
}

// ── pass_cse ──────────────────────────────────────────────────────────────
