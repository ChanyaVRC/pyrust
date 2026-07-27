use super::*;

// ── pass_concat_merge ─────────────────────────────────────────────────────

// A constant pool whose slot 0 is a `str`, used to seed `str_regs` so the
// string-only gate (issue #2383) admits the chain.  Operand registers are
// marked string via `LoadConst(reg, 0)` prefixes in each test.
fn str_consts() -> Vec<Value> {
    vec![Value::string("")]
}

#[test]
fn concat_merge_fuses_three_binop_chain() {
    use crate::ast::BinaryOp;
    // LoadConst(0..) seed temporary operand regs as exact strings so the gate
    // admits the chain. num_locals = 0, so every register is a temp.
    // BinOp(t1, r0, Add, r1)   ← t1 is temp, single-use
    // BinOp(t2, t1, Add, r2)   ← t2 is temp, single-use
    let insns = vec![
        Insn::LoadConst(0, 0), // r0 = "" (str)
        Insn::LoadConst(1, 0), // r1 = "" (str)
        Insn::LoadConst(3, 0), // r2 = "" (str)
        Insn::BinOp(2, 0, BinaryOp::Add, 1),
        Insn::BinOp(4, 2, BinaryOp::Add, 3),
        Insn::Return(4),
    ];
    let mut num_regs = 5u32;
    let out = pass_concat_merge(insns, 0, &mut num_regs, &str_consts());

    // 3 LoadConst + 3 Moves + 1 Concat + 1 Return = 8 instructions.
    assert_eq!(out.len(), 8, "3 LoadConst + 3 Moves + Concat + Return");
    // The chain BinOps are replaced by Moves into the operand window.
    assert!(matches!(out[3], Insn::Move(_, 0)), "Move(base+0, r0)");
    assert!(matches!(out[4], Insn::Move(_, 1)), "Move(base+1, r1)");
    assert!(matches!(out[5], Insn::Move(_, 3)), "Move(base+2, r2)");
    assert!(
        matches!(
            out[6],
            Insn::Concat {
                dst: 4,
                count: 3,
                ..
            }
        ),
        "Concat {{ dst: t2, count: 3 }}"
    );
    // num_regs should have grown by 3 (one per operand).
    assert_eq!(num_regs, 8, "num_regs grew by count=3");
}

#[test]
fn concat_merge_does_not_bypass_frame_register_limit() {
    use crate::ast::BinaryOp;

    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::LoadConst(1, 0),
        Insn::LoadConst(3, 0),
        Insn::BinOp(2, 0, BinaryOp::Add, 1),
        Insn::BinOp(4, 2, BinaryOp::Add, 3),
        Insn::Return(4),
    ];
    let mut num_regs = MAX_FRAME_REGS - 2;

    let out = pass_concat_merge(insns.clone(), 0, &mut num_regs, &str_consts());

    assert_eq!(out, insns);
    assert_eq!(num_regs, MAX_FRAME_REGS - 2);
}

#[test]
fn concat_merge_skips_non_string_chain() {
    use crate::ast::BinaryOp;
    // Issue #2383: an int chain (no `str` evidence on the leading operand)
    // must NOT be fused — the operand-window Moves are pure overhead.
    let insns = vec![
        Insn::BinOp(2, 0, BinaryOp::Add, 1),
        Insn::BinOp(4, 2, BinaryOp::Add, 3),
        Insn::Return(4),
    ];
    let mut num_regs = 5u32;
    // Empty const pool → no register is provably a string.
    let out = pass_concat_merge(insns, 0, &mut num_regs, &[]);
    assert_eq!(out.len(), 3, "int chain left as plain BinOps");
    assert!(matches!(out[0], Insn::BinOp(..)));
    assert!(matches!(out[1], Insn::BinOp(..)));
    assert!(
        !out.iter().any(|i| matches!(i, Insn::Concat { .. })),
        "no Concat for a non-string chain"
    );
    assert_eq!(num_regs, 5, "num_regs unchanged");
}

#[test]
fn concat_merge_requires_two_binops_minimum() {
    use crate::ast::BinaryOp;
    // Single BinOp(Add): only 2 operands, should NOT be merged.
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::LoadConst(1, 0),
        Insn::BinOp(2, 0, BinaryOp::Add, 1),
        Insn::Return(2),
    ];
    let mut num_regs = 3u32;
    let out = pass_concat_merge(insns, 0, &mut num_regs, &str_consts());
    assert_eq!(out.len(), 4, "no merge for 2-operand chain");
    assert!(
        !out.iter().any(|i| matches!(i, Insn::Concat { .. })),
        "BinOp unchanged"
    );
    assert_eq!(num_regs, 3, "num_regs unchanged");
}

#[test]
fn concat_merge_skips_when_intermediate_multi_use() {
    use crate::ast::BinaryOp;
    // t1 is read twice (by BinOp and by Return), so it cannot be removed.
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::LoadConst(1, 0),
        Insn::LoadConst(3, 0),
        Insn::BinOp(2, 0, BinaryOp::Add, 1),
        Insn::BinOp(4, 2, BinaryOp::Add, 3), // reads t1=2
        Insn::Return(2),                     // also reads t1=2 → use_count=2
    ];
    let mut num_regs = 5u32;
    let out = pass_concat_merge(insns, 0, &mut num_regs, &str_consts());
    // Must NOT merge because t1 has use_count=2.
    assert_eq!(out.len(), 6, "no merge when intermediate is multi-use");
    assert!(
        !out.iter().any(|i| matches!(i, Insn::Concat { .. })),
        "no fusion"
    );
    assert_eq!(num_regs, 5, "num_regs unchanged");
}

#[test]
fn concat_merge_does_not_cross_bb_boundary() {
    use crate::ast::BinaryOp;
    // Layout (indices 0-3):
    //   i=0: BinOp(t1, r0, Add, r1)   ← chain start candidate
    //   i=1: BinOp(t2, t1, Add, r2)   ← BB start: target of Jump at i=3
    //   i=2: Return(t2)
    //   i=3: Jump(-3)                  ← target = 3+1+(-3) = 1
    //
    // Because i=1 is a BB start the chain [0,1] straddles a BB boundary
    // and must NOT be fused.  Operands are seeded as strings so only the
    // BB-boundary guard (not the string gate) can prevent the merge.
    let insns = vec![
        Insn::LoadConst(0, 0),               // r0 = "" (str)
        Insn::LoadConst(1, 0),               // r1 = "" (str)
        Insn::LoadConst(3, 0),               // r2 = "" (str)
        Insn::BinOp(2, 0, BinaryOp::Add, 1), // i=3
        Insn::BinOp(4, 2, BinaryOp::Add, 3), // i=4 ← BB start
        Insn::Return(4),                     // i=5
        Insn::Jump(-3),                      // i=6: target = 6+1+(-3) = 4
    ];
    let mut num_regs = 5u32;
    let out = pass_concat_merge(insns, 0, &mut num_regs, &str_consts());
    assert_eq!(out.len(), 7, "no merge across BB boundary");
    assert!(
        !out.iter().any(|i| matches!(i, Insn::Concat { .. })),
        "no fusion across a BB boundary"
    );
    assert_eq!(num_regs, 5, "num_regs unchanged");
}

#[test]
fn concat_merge_skips_compiled_opaque_param_add() {
    // Function parameters have no static type evidence, so the gate
    // (issue #2383) declines to fuse — the chain keeps its plain BinOp form
    // rather than paying for an operand-window Move per operand.
    let code = compile_fn("def f(a, b, c, d):\n    return a + b + c + d\n");
    let optimized = optimize(code);
    let inner = &optimized.fn_protos[0].code;
    assert!(
        !inner.insns.iter().any(|i| matches!(i, Insn::Concat { .. })),
        "opaque-param chain must NOT be fused; insns: {:?}",
        inner.insns
    );
}

#[test]
fn concat_merge_skips_named_string_operand() {
    use crate::ast::BinaryOp;

    // r0 is a named local. Even though bytecode previously stored an exact
    // string there, an explicit/module namespace alias may replace it without
    // an ordinary register write.
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::LoadConst(2, 0),
        Insn::LoadConst(3, 0),
        Insn::BinOp(4, 0, BinaryOp::Add, 2),
        Insn::BinOp(5, 4, BinaryOp::Add, 3),
        Insn::Return(5),
    ];
    let mut num_regs = 6;
    let out = pass_concat_merge(insns.clone(), 2, &mut num_regs, &str_consts());

    assert_eq!(out, insns);
    assert_eq!(num_regs, 6);
}

#[test]
fn concat_merge_requires_every_leaf_to_be_exact_string() {
    use crate::ast::BinaryOp;

    // The leading temp is an exact str, but r0 is an opaque named operand.
    // Fusing would pre-read the final leaf before r0's reflected add can run.
    let insns = vec![
        Insn::LoadConst(2, 0),
        Insn::LoadConst(4, 0),
        Insn::BinOp(3, 2, BinaryOp::Add, 0),
        Insn::BinOp(5, 3, BinaryOp::Add, 4),
        Insn::Return(5),
    ];
    let mut num_regs = 6;
    let out = pass_concat_merge(insns.clone(), 2, &mut num_regs, &str_consts());

    assert_eq!(out, insns);
    assert_eq!(num_regs, 6);
}

// ── pass_loop_inversion ───────────────────────────────────────────────────

#[test]
fn loop_inversion_const_variant_basic() {
    use crate::ast::BinaryOp;
    // Minimal while-loop (CmpJumpIfFalseConst variant):
    //   [0] CmpJumpIfFalseConst(0, Lt, 0, 2)   ; k=2, exit to [3] if false
    //   [1] BinOpImm(0, 0, Add, 1)              ; body: i += 1
    //   [2] Jump(-3)                             ; back-edge to [0]
    //   [3] Return(0)
    //
    // jump_pc!(-(2+1)) at i=2: 2 + 1 + (-3) = 0. ✓ targets [0].
    let insns = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
        Insn::Jump(-3),
        Insn::Return(0),
    ];
    let out = pass_loop_inversion(insns);
    // Length unchanged: we replace Jump in-place, no removal.
    assert_eq!(out.len(), 4);
    // [0] must remain the initial guard.
    assert!(
        matches!(out[0], Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2)),
        "[0] header must be unchanged"
    );
    // [2] must become CmpJumpIfTrueConst(0, Lt, 0, -2).
    // new_offset = -k = -2;  2+1+(-2) = 1 = j+1. ✓
    assert!(
        matches!(out[2], Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -2)),
        "[2] back-edge should be CmpJumpIfTrueConst with offset -2, got {:?}",
        out[2]
    );
}

#[test]
fn loop_inversion_reg_variant_basic() {
    use crate::ast::BinaryOp;
    // CmpJumpIfFalse (register-register) variant:
    //   [0] CmpJumpIfFalse(0, Lt, 1, 2)
    //   [1] BinOpImm(0, 0, BinaryOp::Add, 1)
    //   [2] Jump(-3)
    //   [3] Return(0)
    let insns = vec![
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
        Insn::Jump(-3),
        Insn::Return(0),
    ];
    let out = pass_loop_inversion(insns);
    assert_eq!(out.len(), 4);
    assert!(
        matches!(out[0], Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 2)),
        "[0] header must be unchanged"
    );
    assert!(
        matches!(out[2], Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -2)),
        "[2] back-edge should be CmpJumpIfTrue with offset -2, got {:?}",
        out[2]
    );
}

#[test]
fn loop_inversion_guard_k_too_small() {
    use crate::ast::BinaryOp;
    // k=1: only one instruction between header and Jump, which IS the Jump.
    // The guard `k < 2` must prevent transformation.
    //   [0] CmpJumpIfFalseConst(0, Lt, 0, 1)   ; k=1, exit to [2]
    //   [1] Jump(-2)                             ; back-edge to [0]
    //   [2] Return(0)
    let insns = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 1),
        Insn::Jump(-2),
        Insn::Return(0),
    ];
    let out = pass_loop_inversion(insns.clone());
    // Nothing should change.
    assert_eq!(out, insns, "k=1 must not be transformed");
}

#[test]
fn loop_inversion_guard_not_a_back_edge() {
    use crate::ast::BinaryOp;
    // The Jump at [j+k] targets somewhere other than [j] — must not transform.
    //   [0] CmpJumpIfFalseConst(0, Lt, 0, 2)   ; k=2, exit to [3]
    //   [1] BinOpImm(0, 0, BinaryOp::Add, 1)
    //   [2] Jump(0)                              ; NOT a back-edge (targets [3])
    //   [3] Return(0)
    let insns = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
        Insn::Jump(0), // forward jump, not -(k+1) = -3
        Insn::Return(0),
    ];
    let out = pass_loop_inversion(insns.clone());
    assert_eq!(out, insns, "forward jump must not be transformed");
}

#[test]
fn loop_inversion_does_not_touch_non_jump_at_back() {
    use crate::ast::BinaryOp;
    // If [j+k] is not a Jump at all, leave it alone.
    //   [0] CmpJumpIfFalseConst(0, Lt, 0, 2)
    //   [1] BinOpImm(0, 0, BinaryOp::Add, 1)
    //   [2] Return(0)                           ; not a Jump
    //   [3] Return(0)
    let insns = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
        Insn::Return(0),
        Insn::Return(0),
    ];
    let out = pass_loop_inversion(insns.clone());
    assert_eq!(
        out, insns,
        "non-Jump at back-edge position must not be transformed"
    );
}

#[test]
fn loop_inversion_full_pipeline_while_loop() {
    // End-to-end: compile a while loop whose back-edge reaches loop inversion
    // as a raw Jump.
    //
    // `while a != b: a %= b` — the Ne comparison remains as CmpJumpIfFalse +
    // Jump, and pass_loop_inversion should replace the back-edge Jump with
    // CmpJumpIfTrue.  The `%=` body keeps the loop outside the int-loop
    // versioning whitelist (Mod can raise ZeroDivisionError), so the final
    // stream is the plain inverted loop with no appended guarded copy — which
    // is exactly the shape this end-to-end test pins.
    let code = compile_fn("def f(a, b):\n    while a != b:\n        a %= b\n    return a\n");
    let optimized = optimize(code);
    let inner = &optimized.fn_protos[0].code;
    // The optimized code must not contain an unconditional back-edge Jump.
    let has_back_edge_jump = inner
        .insns
        .iter()
        .any(|i| matches!(i, Insn::Jump(k) if *k < 0));
    assert!(
        !has_back_edge_jump,
        "optimizer should eliminate back-edge Jump in while-ne loop; insns: {:?}",
        inner.insns
    );
    // The optimized code must contain CmpJumpIfTrue* at the loop tail.
    let has_cmpjump_true = inner
        .insns
        .iter()
        .any(|i| matches!(i, Insn::CmpJumpIfTrue(..) | Insn::CmpJumpIfTrueConst(..)));
    assert!(
        has_cmpjump_true,
        "optimizer should introduce CmpJumpIfTrue* at loop tail; insns: {:?}",
        inner.insns
    );
}

#[test]
fn loop_inversion_true_const_variant_basic() {
    use crate::ast::BinaryOp;
    // `while True: if n == 0: break; body` shape (CmpJumpIfTrueConst header):
    //   [0] CmpJumpIfTrueConst(0, Eq, 0, 2)   ; k=2, exit to [3] if n==0 (TRUE)
    //   [1] BinOpImm(0, 0, BinaryOp::Sub, 1)  ; body: n -= 1
    //   [2] Jump(-3)                            ; back-edge to [0]
    //   [3] Return(0)
    //
    // Arithmetic check: jump_pc!(-(2+1)) at i=2: 2+1+(-3) = 0. ✓ targets [0].
    let insns = vec![
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Eq, 0, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Sub, 1, false),
        Insn::Jump(-3),
        Insn::Return(0),
    ];
    let out = pass_loop_inversion(insns);
    assert_eq!(out.len(), 4);
    // [0] must remain the initial guard.
    assert!(
        matches!(out[0], Insn::CmpJumpIfTrueConst(0, BinaryOp::Eq, 0, 2)),
        "[0] header must be unchanged"
    );
    // [2] must become CmpJumpIfFalseConst(0, Eq, 0, -2).
    // new_offset = -k = -2;  2+1+(-2) = 1 = j+1. ✓
    assert!(
        matches!(out[2], Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 0, -2)),
        "[2] back-edge should be CmpJumpIfFalseConst with offset -2, got {:?}",
        out[2]
    );
}

#[test]
fn loop_inversion_true_reg_variant_basic() {
    use crate::ast::BinaryOp;
    // CmpJumpIfTrue (register-register) header variant:
    //   [0] CmpJumpIfTrue(0, Eq, 1, 2)
    //   [1] BinOpImm(0, 0, BinaryOp::Sub, 1)
    //   [2] Jump(-3)
    //   [3] Return(0)
    let insns = vec![
        Insn::CmpJumpIfTrue(0, BinaryOp::Eq, 1, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Sub, 1, false),
        Insn::Jump(-3),
        Insn::Return(0),
    ];
    let out = pass_loop_inversion(insns);
    assert_eq!(out.len(), 4);
    assert!(
        matches!(out[0], Insn::CmpJumpIfTrue(0, BinaryOp::Eq, 1, 2)),
        "[0] header must be unchanged"
    );
    assert!(
        matches!(out[2], Insn::CmpJumpIfFalse(0, BinaryOp::Eq, 1, -2)),
        "[2] back-edge should be CmpJumpIfFalse with offset -2, got {:?}",
        out[2]
    );
}

#[test]
fn loop_inversion_true_const_guard_k_too_small() {
    use crate::ast::BinaryOp;
    // k=1: guard must prevent transformation for CmpJumpIfTrueConst header.
    //   [0] CmpJumpIfTrueConst(0, Eq, 0, 1)   ; k=1, exit to [2]
    //   [1] Jump(-2)                             ; back-edge to [0]
    //   [2] Return(0)
    let insns = vec![
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Eq, 0, 1),
        Insn::Jump(-2),
        Insn::Return(0),
    ];
    let out = pass_loop_inversion(insns.clone());
    assert_eq!(
        out, insns,
        "k=1 must not be transformed for CmpJumpIfTrueConst header"
    );
}

// ── line-number remap across equal-valued constants (issue #1962) ─────────
