use super::*;

#[test]
fn binop_const_fusion_skips_binop_that_is_jump_target() {
    use crate::ast::BinaryOp;
    // Ternary-as-right-operand shape (issue #2565):
    //   0 JumpIfFalse(0, 2)        a falsy → jump to the else LoadConst (idx 3)
    //   1 LoadConst(2, 0)          then: t = const#0
    //   2 Jump(1)                  jump past the else load, onto the BinOp (idx 4)
    //   3 LoadConst(2, 1)          else: t = const#1   ← jump target of insn 0
    //   4 BinOp(1, 0, Add, 2)      ← jump target of insn 2 (the then-branch)
    //   5 Return(1)
    // The else-branch LoadConst (idx 3) is adjacent to the BinOp, but the BinOp
    // is a jump target reached by the then-branch without executing that load.
    // Fusing would make the then-branch use the else constant, so it must be
    // skipped.
    let insns = vec![
        Insn::JumpIfFalse(0, 2),
        Insn::LoadConst(2, 0),
        Insn::Jump(1),
        Insn::LoadConst(2, 1),
        Insn::BinOp(1, 0, BinaryOp::Add, 2),
        Insn::Return(1),
    ];
    let out = pass_binop_const_fusion(insns.clone(), &[], 1);
    assert_eq!(out, insns, "must not fuse a BinOp that is a jump target");
}

#[test]
fn binop_const_fusion_fuses_loadconst_binop() {
    use crate::ast::BinaryOp;
    // LoadConst(r=5, c=0)  BinOp(dst=1, lhs=0, Add, r=5)  where num_locals=2
    // r=5 >= num_locals=2, r != dst  → fuse to BinOpConst(1, 0, Add, 0), drop LoadConst
    let insns = vec![
        Insn::LoadConst(5, 0), // temp reg 5, const index 0
        Insn::BinOp(1, 0, BinaryOp::Add, 5),
        Insn::Return(1),
    ];
    let out = pass_binop_const_fusion(insns, &[], 2);
    assert_eq!(out.len(), 2, "LoadConst should be removed");
    assert!(
        matches!(out[0], Insn::BinOpConst(1, 0, BinaryOp::Add, 0, ..)),
        "BinOp should become BinOpConst"
    );
}

#[test]
fn binop_const_fusion_fires_for_reused_scratch_in_loop_body() {
    use crate::ast::BinaryOp;
    // The "reused scratch register" shape that dominates loop bodies: temp
    // reg 3 is LoadConst-ed afresh for each operand, and the loop ends in a
    // back-edge.  The first LoadConst(3)+BinOp(_, _, Mul, 3) pair must fuse
    // because reg 3 is overwritten by the next LoadConst before being read
    // (its value is dead), even though `last_read[3]` is the second BinOp and
    // a back-edge follows.  num_locals = 2.
    let insns = vec![
        Insn::LoadConst(3, 0),                      // t = 2     (scratch)
        Insn::BinOp(2, 1, BinaryOp::Mul, 3),        // r2 = i * t   ← fuse
        Insn::LoadConst(3, 1),                      // t = 1     (overwrites reg 3)
        Insn::BinOp(2, 2, BinaryOp::Sub, 3),        // r2 = r2 - t
        Insn::BinOpInPlace(0, 0, BinaryOp::Add, 2), // s += r2
        Insn::Jump(-6),                             // loop back-edge
    ];
    let out = pass_binop_const_fusion(insns, &[], 2);
    assert!(
        matches!(out[0], Insn::BinOpConst(2, 1, BinaryOp::Mul, 0, false)),
        "first pair fused despite the back-edge (scratch overwritten before \
             read): {:?}",
        out[0]
    );
    // The whole LoadConst is removed, so the stream shrinks by at least one.
    assert!(
        out.len() < 6,
        "at least one LoadConst removed: len {}",
        out.len()
    );
}

#[test]
fn binop_const_fusion_skips_reused_scratch_when_value_is_live() {
    use crate::ast::BinaryOp;
    // Same shape but reg 3 is READ again (by the second BinOp) before being
    // overwritten, so its value is live and the first pair must NOT fuse.
    let insns = vec![
        Insn::LoadConst(3, 0),
        Insn::BinOp(2, 1, BinaryOp::Mul, 3), // reads reg 3
        Insn::BinOp(4, 2, BinaryOp::Sub, 3), // reads reg 3 again → still live
        Insn::Jump(-4),
    ];
    let out = pass_binop_const_fusion(insns, &[], 2);
    assert!(
        matches!(out[0], Insn::LoadConst(3, 0)),
        "no fusion while reg 3 is still live: {:?}",
        out[0]
    );
}

#[test]
fn binop_const_fusion_fires_for_loop_bound_reloaded_each_iteration() {
    use crate::ast::BinaryOp;
    // Issue #2889: the `while i < N: if i % 2 == 0: …` shape.  Scratch reg 3
    // holds the bound at the loop header and is then reloaded with each of the
    // body's operand constants, so `last_read[3]` points past every candidate
    // and `scratch_dead_after` runs into the header's conditional jump.  Both
    // approximations veto fusion, yet reg 3 is block-local — every read is fed
    // by the write immediately before it — so no path can observe a stale
    // value and every pair is safe to fold.  num_locals = 2.
    let insns = vec![
        Insn::LoadConst(3, 0),                        // 0: t = N  ← back-edge target
        Insn::BinOp(2, 0, BinaryOp::Lt, 3),           // 1: r2 = i < t   ← fuse
        Insn::JumpIfFalse(2, 6),                      // 2: exit
        Insn::LoadConst(3, 1),                        // 3: t = 2
        Insn::BinOp(2, 0, BinaryOp::Mod, 3),          // 4: r2 = i % t   ← fuse
        Insn::LoadConst(3, 2),                        // 5: t = 0
        Insn::BinOp(2, 2, BinaryOp::Eq, 3),           // 6: r2 = r2 == t ← fuse
        Insn::JumpIfFalse(2, 1),                      // 7: skip
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true), // 8: i += 1
        Insn::Jump(-10),                              // 9: back-edge to 0
        Insn::Return(0),                              // 10
    ];
    let out = pass_binop_const_fusion(insns, &[], 2);

    assert!(
        !out.iter().any(|insn| matches!(insn, Insn::LoadConst(3, _))),
        "every reloaded scratch constant folds into its consumer: {out:?}"
    );
    assert!(
        matches!(out[0], Insn::BinOpConst(2, 0, BinaryOp::Lt, 0, false)),
        "the loop bound must fuse so the back-edge lands on the comparison: {:?}",
        out[0]
    );
    // The back-edge is retargeted onto the surviving comparison, which is what
    // lets loop inversion and int-loop versioning recognise the loop.
    assert!(
        matches!(out[6], Insn::Jump(-7)),
        "back-edge must target the fused header: {:?}",
        out[6]
    );
}

#[test]
fn binop_const_fusion_skips_scratch_that_crosses_a_jump_target() {
    use crate::ast::BinaryOp;
    // Reg 3 is loaded once before a loop and read inside it, so its value has
    // to survive the back-edge into the next iteration.  The read at index 3 is
    // reached from the jump target at index 1 without re-executing the load, so
    // reg 3 is not block-local and the pair must not fuse.
    let insns = vec![
        Insn::LoadConst(3, 0),               // 0: t = 2
        Insn::BinOp(2, 0, BinaryOp::Lt, 3),  // 1: ← jump target
        Insn::JumpIfFalse(2, 2),             // 2: exit
        Insn::BinOp(4, 0, BinaryOp::Add, 3), // 3: reads t again
        Insn::Jump(-4),                      // 4: back-edge to 1
        Insn::Return(0),                     // 5
    ];
    let out = pass_binop_const_fusion(insns.clone(), &[], 2);
    assert_eq!(
        out, insns,
        "a scratch value that crosses a control-flow edge must not fuse"
    );
}

#[test]
fn binop_const_fusion_handles_wide_temp_indices_past_inline_bitset_and_slot_limits() {
    use crate::ast::BinaryOp;
    // Exercise both heap-spill paths of `load_const_dead_after_use` and
    // `pass_binop_const_fusion` (issue #2905): temp offset > 256 spills
    // `TempBitset` past its 256-bit inline array and spills `TempSlotBuf` /
    // `last_read_inline` past 64 slots.  Reg 302 (offset 300 from num_locals=2)
    // is reloaded as a block-local loop scratch and must fuse; reg 305 (offset
    // 303) crosses the back-edge target and must NOT fuse.
    let num_locals = 2u32;
    let local_scratch = 302u32;
    let escaping_scratch = 305u32;
    let insns = vec![
        Insn::LoadConst(escaping_scratch, 3), // 0: loaded before loop
        Insn::LoadConst(local_scratch, 0),    // 1: ← back-edge target; block-local
        Insn::BinOp(2, 0, BinaryOp::Lt, local_scratch), // 2: fuse
        Insn::JumpIfFalse(2, 6),              // 3: exit
        Insn::LoadConst(local_scratch, 1),    // 4: reload block-local scratch
        Insn::BinOp(2, 0, BinaryOp::Mod, local_scratch), // 5: fuse
        Insn::BinOp(4, 2, BinaryOp::Add, escaping_scratch), // 6: reads escaping_scratch across back-edge
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),       // 7: i += 1
        Insn::Jump(-8),                                     // 8: back-edge to 1
        Insn::Return(0),                                    // 9
    ];
    let out = pass_binop_const_fusion(insns, &[], num_locals);
    assert!(
        !out.iter()
            .any(|insn| matches!(insn, Insn::LoadConst(r, _) if *r == local_scratch)),
        "wide block-local scratch temp must still fuse: {out:?}"
    );
    assert!(
        out.iter()
            .any(|insn| matches!(insn, Insn::LoadConst(r, 3) if *r == escaping_scratch)),
        "wide escaping scratch temp crossing back-edge must NOT fuse: {out:?}"
    );
    assert!(
        out.iter()
            .any(|insn| matches!(insn, Insn::BinOpConst(2, 0, BinaryOp::Lt, 0, false))),
        "loop bound with wide temp index must fuse into BinOpConst: {out:?}"
    );
}

#[test]
fn binop_const_fusion_skips_when_reg_is_local() {
    use crate::ast::BinaryOp;
    // r=1 < num_locals=3  → must NOT fuse (register could be a local variable)
    let insns = vec![
        Insn::LoadConst(1, 0),
        Insn::BinOp(2, 0, BinaryOp::Add, 1),
        Insn::Return(2),
    ];
    let out = pass_binop_const_fusion(insns, &[], 3);
    assert_eq!(out.len(), 3, "no fusion when reg is a local");
    assert!(matches!(out[0], Insn::LoadConst(1, 0)));
}

#[test]
fn binop_const_fusion_skips_when_dst_equals_reg() {
    use crate::ast::BinaryOp;
    // dst == lc_reg: the result overwrites lc_reg, so lc_reg is not live after
    // the BinOp — fusion is safe and should happen.
    let insns = vec![
        Insn::LoadConst(5, 0),
        Insn::BinOp(5, 0, BinaryOp::Add, 5), // dst == rhs == lc_reg
        Insn::Return(5),
    ];
    let out = pass_binop_const_fusion(insns, &[], 2);
    assert_eq!(
        out.len(),
        2,
        "fusion is safe when dst == lc_reg (result overwrites it)"
    );
    assert!(
        matches!(out[0], Insn::BinOpConst(5, 0, BinaryOp::Add, 0, ..)),
        "should fuse to BinOpConst"
    );
}

#[test]
fn binop_const_fusion_on_compiled_code() {
    // Use a function argument so the lhs is not a compile-time constant.
    // pass_binop_const_fusion should still fuse LoadConst(r,5)+BinOp(dst,n,Add,r)
    // → BinOpConst(dst,n,Add,5), which pass_const_fold cannot fold further.
    let code = compile_fn("def f(n):\n    return n + 5\n");
    let optimized = optimize(code);
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(
        has_binopconst,
        "optimizer should fuse LoadConst+BinOp into BinOpConst for n+5"
    );
}

#[test]
fn binop_const_fusion_commutative_lhs_const() {
    use crate::ast::BinaryOp;
    // LoadConst(r=5, c=0)  BinOp(dst=1, lhs=5, Add, rhs=0)  — const on LEFT
    // Even though Add is commutative, swapping would break __radd__ dispatch,
    // so the optimization must be skipped and all 3 instructions kept.
    let insns = vec![
        Insn::LoadConst(5, 0),
        Insn::BinOp(1, 5, BinaryOp::Add, 0),
        Insn::Return(1),
    ];
    let out = pass_binop_const_fusion(insns, &[], 2);
    assert_eq!(
        out.len(),
        3,
        "const-lhs Add should NOT be fused (would break __radd__ dispatch)"
    );
}

#[test]
fn binop_const_fusion_does_not_commute_non_commutative() {
    use crate::ast::BinaryOp;
    // Sub is not commutative — should not fuse when const is on left
    let insns = vec![
        Insn::LoadConst(5, 0),
        Insn::BinOp(1, 5, BinaryOp::Sub, 0),
        Insn::Return(1),
    ];
    let out = pass_binop_const_fusion(insns, &[], 2);
    assert_eq!(
        out.len(),
        3,
        "non-commutative op with const-lhs should not fuse"
    );
}

// ── Unary constant folding (end-to-end via MIR + optimizer pipeline) ──────

#[test]
fn unary_fold_folds_negated_literals_before_and_inside_a_loop() {
    use crate::ast::UnaryOp;
    use crate::value::ValueKind;

    let code = compile_fn(
        r#"def f(n):
    before_int = -5
    before_float = -2.5
    inside_int = 0
    inside_float = 0.0
    while n:
        inside_int = -7
        inside_float = -0.5
        n = n - 1
    return before_int, before_float, inside_int, inside_float
"#,
    );

    let raw = &code.fn_protos[0].code.insns;
    assert!(
        raw.windows(2).any(|pair| matches!(
            pair,
            [Insn::LoadConst(reg, _), Insn::UnaryOp(dst, UnaryOp::Neg, src)]
                if reg == dst && dst == src
        )),
        "the compiler must expose the self-destination literal shape: {raw:?}"
    );

    let optimized = optimize(code);
    let function = &optimized.fn_protos[0].code;
    assert!(
        !function
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::UnaryOp(_, UnaryOp::Neg, _))),
        "literal negations must fold even when a later back edge exists: {:?}",
        function.insns
    );
    for expected in [-5, -7] {
        assert!(
            function
                .consts
                .iter()
                .any(|value| matches!(value.kind(), ValueKind::Int(value) if value == expected)),
            "folded int {expected} must remain in the constant pool: {:?}",
            function.consts
        );
    }
    for expected in [-2.5, -0.5] {
        assert!(
            function
                .consts
                .iter()
                .any(|value| matches!(value.kind(), ValueKind::Float(value) if value == expected)),
            "folded float {expected} must remain in the constant pool: {:?}",
            function.consts
        );
    }
}

#[test]
fn unary_fold_on_compiled_literal() {
    // -5 is a literal unary-neg in the source; the optimizer pipeline folds it
    // into a constant in the pool.
    let code = compile_fn("x = -5\nprint(x)\n");
    let optimized = optimize(code);
    let has_neg5 = optimized
        .consts
        .iter()
        .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(-5)));
    assert!(has_neg5, "constant -5 should appear after unary fold");
}

// ── Dynamic operator semantics (issue #438) ───────────────────────────────
//
// Constant folding must NOT rewrite
// `x + 0` / `x * 1` / `x * 0` / `x ** 0` / `x ** 1` when `x` is a function
// parameter of unknown type, because a user class may override `__add__`
// etc. Any such rewrite would skip dunder dispatch.

#[test]
fn dynamic_add_zero_keeps_protocol_dispatch() {
    // x + 0 inside a function must NOT be rewritten to Move(dst, x),
    // because `x` may be a user instance with a `__add__` override.
    let code = compile_fn("def f(x):\n    return x + 0\n");
    let optimized = optimize(code);
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(
        has_binopconst,
        "x+0 must keep BinOpConst so __add__ dispatch runs at runtime"
    );
}

#[test]
fn dynamic_mul_zero_keeps_protocol_dispatch() {
    // x * 0 must NOT collapse to LoadConst(0) — user `__mul__` may run.
    let code = compile_fn("def f(x):\n    return x * 0\n");
    let optimized = optimize(code);
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(
        has_binopconst,
        "x*0 must keep BinOpConst so __mul__ dispatch runs at runtime"
    );
}

#[test]
fn dynamic_pow_zero_keeps_protocol_dispatch() {
    // x ** 0 must NOT collapse to LoadConst(1) — user `__pow__` may run.
    let code = compile_fn("def f(x):\n    return x ** 0\n");
    let optimized = optimize(code);
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(
        has_binopconst,
        "x**0 must keep BinOpConst so __pow__ dispatch runs at runtime"
    );
}

#[test]
fn constant_add_zero_folds_via_const_fold() {
    // `5 + 0` should still fold to `5` — handled by `pass_const_fold`,
    // not the removed algebraic-simplify pass.
    let code = compile_fn("def f():\n    return 5 + 0\n");
    let optimized = optimize(code);
    // No BinOpConst should remain: const_fold turned it into LoadConst(_, 5).
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(
        !has_binopconst,
        "5+0 must still be constant-folded by pass_const_fold"
    );
}

// ── Constant-fold coverage and conservative dynamic cases ────────────────

#[test]
fn const_fold_known_int_add_zero() {
    // The preceding forward constant pass already knows `x == 5`, so it can
    // fold the operation without a separate algebraic identity pass.
    let code = compile_fn("def f():\n    x = 5\n    return x + 0\n");
    let optimized = optimize(code);
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(
        !has_binopconst,
        "x+0 with x known Int (LoadConst 5) should be simplified"
    );
}

#[test]
fn dynamic_loop_target_add_zero_remains_observable() {
    // The name `range` is resolved at runtime and may produce arbitrary
    // objects. A ForIter destination therefore cannot seed an integer type
    // fact, and `i + 0` must retain its Python addition dispatch.
    let code = compile_fn(
        "def f():\n    t = 0\n    for i in range(100):\n        t = i + 0\n    return t\n",
    );
    let optimized = optimize(code);
    let inner = &optimized.fn_protos[0].code;
    let runtime_add_zero = inner.insns.iter().any(|insn| {
        matches!(insn, Insn::BinOp(_, _, crate::ast::BinaryOp::Add, _))
            || matches!(insn, Insn::BinOpConst(_, _, crate::ast::BinaryOp::Add, c_idx, ..)
                if matches!(inner.consts[*c_idx as usize].kind(),
                            crate::value::ValueKind::Int(0)))
            || matches!(insn, Insn::BinOpImm(_, _, crate::ast::BinaryOp::Add, 0, ..))
    });
    assert!(
        runtime_add_zero,
        "ForIter targets are dynamic; i + 0 must remain an observable operation"
    );
}

#[test]
fn const_fold_known_int_mul_one() {
    let code = compile_fn("def f():\n    x = 7\n    return x * 1\n");
    let optimized = optimize(code);
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(!has_binopconst, "x*1 with x known Int should be simplified");
}

#[test]
fn const_fold_known_int_mul_zero() {
    // `x = 7; return x * 0` — `x` is Int, so `x * 0` becomes LoadConst(0)
    // (no BinOpConst left after the pass).
    let code = compile_fn("def f():\n    x = 7\n    return x * 0\n");
    let optimized = optimize(code);
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(
        !has_binopconst,
        "x*0 with x known Int should fold to LoadConst(0)"
    );
}

#[test]
fn unknown_lhs_still_keeps_binopconst() {
    // Param `x` of unknown type — pass MUST NOT fire (regression pin).
    // This is the original #438 test, restated to make sure the gating
    // works for the bug case.
    let code = compile_fn("def f(x):\n    return x + 0\n");
    let optimized = optimize(code);
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(
        has_binopconst,
        "x+0 with unknown-type x MUST keep BinOpConst (#438)"
    );
}

#[test]
fn float_lhs_keeps_runtime_binop() {
    // `x = 1.5; return x + 0` — x is Float, pass MUST NOT fire because
    // `float * 0` returns `0.0` (not int 0) and `float + 0` preserves
    // NaN/inf semantics.
    let code = compile_fn("def f():\n    x = 1.5\n    return x + 0\n");
    let optimized = optimize(code);
    let has_binopconst = optimized.fn_protos[0]
        .code
        .insns
        .iter()
        .any(|i| matches!(i, Insn::BinOpConst(..)));
    assert!(
        has_binopconst,
        "x+0 with Float x MUST keep BinOpConst (gating works for non-Int)"
    );
}

// ── Constant folding (end-to-end via MIR + optimizer pipeline) ────────────

#[test]
fn const_fold_on_compiled_chain() {
    // Function locals are not published through SyncModuleGlobal, so a pure
    // x = 5; y = x * 2 chain should still fold to 10. Module-level bindings
    // intentionally cross the namespace-storage barrier after each store.
    let code = compile_fn("def folded():\n    x = 5\n    y = x * 2\n    return y\n");
    let optimized = optimize(code);
    let has_10 = optimized.fn_protos[0]
        .code
        .consts
        .iter()
        .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(10)));
    assert!(
        has_10,
        "constant 10 should appear in pool after folding x*2 with x=5"
    );
}

#[test]
fn const_fold_fully_constant_associative_chain() {
    // The SSA constant folding pass handles the full constant chain.
    let code = compile_fn("def f():\n    return (5 + 1) + 2\n");
    let optimized = optimize(code);
    let inner = &optimized.fn_protos[0].code;
    assert!(
        inner
            .consts
            .iter()
            .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(8))),
        "the folded result should be interned in the constant pool"
    );
    assert!(
        !inner
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::BinOpConst(..))),
        "constant folding should eliminate the full constant chain"
    );
}

#[test]
fn const_tuple_fold_keeps_ternary_merge_build() {
    use crate::value::Value;

    // A one-element tuple around a ternary has a shared BuildTuple.  The then
    // arm reaches [4] directly, while only the else arm executes the adjacent
    // LoadConst at [3].  Folding [3..=4] into `(20,)` would corrupt the then
    // path's `(10,)`.
    let mut consts = vec![Value::int(10), Value::int(20)];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),
        Insn::LoadConst(1, 0),
        Insn::Jump(1),
        Insn::LoadConst(1, 1),
        Insn::BuildTuple(2, 1, 1),
        Insn::Return(2),
    ];

    let out = pass_fold_const_tuple(insns, 1, &mut consts);
    assert!(
        matches!(out[3], Insn::LoadConst(1, 1)),
        "the else-arm constant must remain path-local"
    );
    assert!(
        matches!(out[4], Insn::BuildTuple(2, 1, 1)),
        "a shared BuildTuple must not be replaced from one predecessor"
    );
}

#[test]
fn named_local_effect_classifier_is_a_conservative_false_whitelist() {
    use crate::ast::{BinaryOp, UnaryOp};

    for safe in [
        Insn::LoadConst(0, 0),
        Insn::LoadGlobal(0, 0),
        Insn::LoadCell(0, 0),
        Insn::LoadNone(0),
        Insn::LoadNoneRange { start: 0, count: 2 },
        Insn::Move(0, 1),
        Insn::CheckLocal(0, 0),
        Insn::BuildList(0, 1, 1),
        Insn::BuildListReserve(0, 1),
        Insn::BuildTuple(0, 1, 1),
        Insn::BuildString(0, 1, 1),
        Insn::BuildSlice(0, 1),
        Insn::MakeFunction(0),
    ] {
        assert!(
            !may_invalidate_named_locals(&safe),
            "audited register-only instruction became a barrier: {safe:?}"
        );
    }

    for barrier in [
        Insn::UnaryOp(2, UnaryOp::Neg, 1),
        Insn::BinOp(2, 0, BinaryOp::Add, 1),
        Insn::GetAttr(2, 1, 0),
        Insn::GetItem(2, 0, 1),
        Insn::GetSlice(2, 0, 1),
        Insn::GetIter(0, 1),
        Insn::FormatValue(2, 1),
        Insn::FormatValueSpec(2, 1, 0),
        Insn::JumpIfFalse(0, 1),
        Insn::ForIter(2, 0, 1),
        Insn::ImportModule(2, 0),
        Insn::ImportStar(1),
        Insn::MakeClass(0),
        Insn::StoreGlobal(0, 1),
        Insn::StoreCell(0, 1),
        Insn::DeleteName(0),
        Insn::DeleteCell(0, true),
        Insn::LoadClassDeref(0, 0),
        Insn::SyncModuleGlobal(0, 0),
        Insn::DeleteModuleGlobal(0),
        Insn::SetAttr(0, 0, 1),
        Insn::SetItem(0, 1, 2),
        Insn::ListExtend(0, 1),
        Insn::DictUpdate(0, 1),
        Insn::GetAwaitable(2, 1),
        Insn::Concat {
            dst: 2,
            base: 0,
            count: 2,
        },
        Insn::PrintExpr(0),
    ] {
        assert!(
            may_invalidate_named_locals(&barrier),
            "user/storage-dispatching instruction must be a barrier: {barrier:?}"
        );
    }
}

#[test]
fn sccp_preserves_lineno_for_surviving_raise_after_dead_branch_removal() {
    // End-to-end compile + optimize check: dead branch eliminated by MIR SCCP
    // must not shift the line number of a surviving ZeroDivisionError.
    let src = "\
def f():
    x = 1
    if False:
        x = 5
    if x == 5:
        return 100
    return x // 0
";
    let code = compile_fn(src);
    let optimized = optimize(code);
    let inner = &optimized.fn_protos[0].code;
    assert!(
        inner.insns.iter().any(|i| matches!(
            i,
            Insn::BinOp(_, _, BinaryOp::FloorDiv, _)
                | Insn::BinOpConst(_, _, BinaryOp::FloorDiv, _, _)
                | Insn::BinOpImm(_, _, BinaryOp::FloorDiv, _, _)
        )),
        "x // 0 must survive optimization: {:?}",
        inner.insns
    );
    assert!(
        !inner.consts.iter().any(|v| v.as_int() == Some(100)),
        "dead `return 100` branch should be eliminated by SCCP: {:?}",
        inner.consts
    );
}

// ── pass_const_branch_elim ────────────────────────────────────────────────

#[test]
fn const_branch_elim_jumpiffalse_truthy_becomes_jump0() {
    use crate::value::Value;
    // LoadConst(r=0, c=0) [consts[0]=True]  JumpIfFalse(0, 5)
    // Truthy → never jumps → replace with Jump(0)
    let consts = vec![Value::bool_(true)];
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::JumpIfFalse(0, 5),
        Insn::Return(0),
    ];
    let out = pass_const_branch_elim(insns, &consts);
    assert!(
        matches!(out[1], Insn::Jump(0)),
        "truthy JumpIfFalse → Jump(0)"
    );
}

#[test]
fn const_branch_elim_jumpiffalse_falsy_becomes_jump_k() {
    use crate::value::Value;
    // LoadConst(r=0, c=0) [consts[0]=False]  JumpIfFalse(0, 3)
    // Falsy → always jumps → replace with Jump(3)
    let consts = vec![Value::bool_(false)];
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::JumpIfFalse(0, 3),
        Insn::Return(0),
    ];
    let out = pass_const_branch_elim(insns, &consts);
    assert!(
        matches!(out[1], Insn::Jump(3)),
        "falsy JumpIfFalse → Jump(k)"
    );
}

#[test]
fn const_branch_elim_keeps_shared_ternary_merge_test() {
    use crate::value::Value;

    // Both ternary arms define r1, then converge on the JumpIfFalse at [4].
    // The linear predecessor is the falsy else-arm LoadConst, but the then arm
    // reaches [4] through Jump(1) without executing it.  Folding [4] from [3]
    // would therefore make the true arm take the false branch as well.
    //
    //   [0] JumpIfFalse(r0, +2)  -> [3]
    //   [1] LoadConst(r1, True)
    //   [2] Jump(+1)             -> [4]
    //   [3] LoadConst(r1, False)
    //   [4] JumpIfFalse(r1, +1)
    //   [5] Return(r1)
    //   [6] ReturnNone
    let consts = vec![Value::bool_(true), Value::bool_(false)];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),
        Insn::LoadConst(1, 0),
        Insn::Jump(1),
        Insn::LoadConst(1, 1),
        Insn::JumpIfFalse(1, 1),
        Insn::Return(1),
        Insn::ReturnNone,
    ];

    let out = pass_const_branch_elim(insns, &consts);
    assert!(
        matches!(out[4], Insn::JumpIfFalse(1, 1)),
        "a merge-point conditional must not be folded from one predecessor"
    );
}

#[test]
fn const_branch_elim_eliminates_dead_branch_on_compiled_code() {
    // "if True: print(1)\nelse: print(2)" — the else branch should be dead
    let code = compile_fn("if True:\n    print(1)\nelse:\n    print(2)\n");
    let optimized = optimize(code);
    // After optimization, the instruction stream should not contain the
    // dead-branch code that prints 2. Check by verifying only one integer
    // constant (1) is referenced, not 2.
    // Note: the constant 2 may still exist in the pool even if the code
    // referencing it is dead — but the dead code itself should be gone.
    // Instead check that no LoadConst referencing 2 appears in reachable insns.
    // Simpler: check the insn list has no BinOp/conditional jumps (the if collapsed).
    let has_cond_jump = optimized.insns.iter().any(|i| {
        matches!(
            i,
            Insn::JumpIfFalse(..)
                | Insn::JumpIfTrue(..)
                | Insn::CmpJumpIfFalse(..)
                | Insn::CmpJumpIfFalseConst(..)
                | Insn::CmpJumpIfTrue(..)
                | Insn::CmpJumpIfTrueConst(..)
        )
    });
    assert!(
        !has_cond_jump,
        "constant-condition if should have no conditional jumps"
    );
}

// ── pass_cmpjump_fusion ───────────────────────────────────────────────────

#[test]
fn cmpjump_fuses_binopconst_jumpiffalse() {
    use crate::ast::BinaryOp;
    // BinOpConst(r=5, lhs=0, Gt, c=0) + JumpIfFalse(r=5, k=0)
    // k=0: if-false jumps to old_pos 1+1+0=2 = Return.
    // After fusion+compaction: CmpJumpIfFalseConst at new_pos 0, Return at new_pos 1.
    // Rewritten offset: to_new[2]-to_new[1]-1 = 1-0-1 = 0 → same k=0.
    let insns = vec![
        Insn::BinOpConst(5, 0, BinaryOp::Gt, 0, false),
        Insn::JumpIfFalse(5, 0),
        Insn::Return(0),
    ];
    let out = pass_cmpjump_fusion(insns, &[], 2);
    assert_eq!(out.len(), 2, "BinOpConst should be removed");
    assert!(
        matches!(out[0], Insn::CmpJumpIfFalseConst(0, BinaryOp::Gt, 0, 0)),
        "should become CmpJumpIfFalseConst with same offset"
    );
}

#[test]
fn cmpjump_fuses_binop_jumpiftrue() {
    use crate::ast::BinaryOp;
    // BinOp(r=5, lhs=0, Eq, rhs=1) + JumpIfTrue(r=5, k=0)
    // → CmpJumpIfTrue(lhs=0, Eq, rhs=1, k=0)
    let insns = vec![
        Insn::BinOp(5, 0, BinaryOp::Eq, 1),
        Insn::JumpIfTrue(5, 0),
        Insn::Return(0),
    ];
    let out = pass_cmpjump_fusion(insns, &[], 2);
    assert_eq!(out.len(), 2, "BinOp should be removed");
    assert!(
        matches!(out[0], Insn::CmpJumpIfTrue(0, BinaryOp::Eq, 1, 0)),
        "should become CmpJumpIfTrue"
    );
}

#[test]
fn cmpjump_skips_when_reg_is_local() {
    use crate::ast::BinaryOp;
    // r=1 < num_locals=3 → no fusion
    let insns = vec![
        Insn::BinOpConst(1, 0, BinaryOp::Gt, 0, false),
        Insn::JumpIfFalse(1, 1),
        Insn::Return(0),
    ];
    let out = pass_cmpjump_fusion(insns, &[], 3);
    assert_eq!(out.len(), 3, "no fusion when cond reg is a local");
}

#[test]
fn cmpjump_skips_when_jump_targets_the_cond_jump() {
    use crate::ast::BinaryOp;
    // Issue #2088: an `and` short-circuit lands on the trailing conditional
    // jump of the RHS comparison.  Fusing BinOp+JumpIfFalse at index 1/2
    // would make index 2 recompute (5 Ne 6) instead of re-testing the LHS
    // register — wrong on the incoming-jump path.  The JumpIfFalse(4, -1)
    // (short-circuit) targets index 2 (0 + 1 + 1 = 2), so fusion must skip.
    let insns = vec![
        Insn::JumpIfFalse(4, 1), // short-circuit LHS-false jump → targets index 2
        Insn::BinOp(3, 5, BinaryOp::Ne, 6),
        Insn::JumpIfFalse(3, 0), // index 2: jump target — must NOT be fused
        Insn::Return(0),
    ];
    let out = pass_cmpjump_fusion(insns, &[], 2);
    assert_eq!(
        out.len(),
        4,
        "BinOp must survive: its JumpIfFalse is a jump target"
    );
    assert!(
        matches!(out[1], Insn::BinOp(3, 5, BinaryOp::Ne, 6)),
        "BinOp(3, 5, Ne, 6) must be preserved, not fused away"
    );
    assert!(
        matches!(out[2], Insn::JumpIfFalse(3, 0)),
        "trailing JumpIfFalse must remain a plain register test"
    );
}

#[test]
fn cmpjump_fusion_on_compiled_if() {
    let code = compile_fn("def f(x):\n    if x > 3:\n        print(x)\n");
    let optimized = optimize(code);
    let inner = &optimized.fn_protos[0].code;
    let has_cmpjump = inner.insns.iter().any(|i| {
        matches!(
            i,
            Insn::CmpJumpIfFalse(..)
                | Insn::CmpJumpIfTrue(..)
                | Insn::CmpJumpIfFalseConst(..)
                | Insn::CmpJumpIfTrueConst(..)
        )
    });
    assert!(
        has_cmpjump,
        "optimizer should fuse comparison into conditional jump"
    );
}

#[test]
fn not_invert_keeps_shared_ternary_merge_test() {
    use crate::ast::UnaryOp;

    // The then arm jumps directly to the shared condition at [4].  Only the
    // else arm executes UnaryOp(Not) at [3], so replacing [4] with a test of r2
    // would make the then arm read the wrong condition.
    let insns = vec![
        Insn::JumpIfFalse(0, 2),
        Insn::Move(1, 3),
        Insn::Jump(1),
        Insn::UnaryOp(1, UnaryOp::Not, 2),
        Insn::JumpIfFalse(1, 1),
        Insn::Return(1),
        Insn::ReturnNone,
    ];

    let out = pass_not_invert(insns, &[], 1);
    assert!(
        matches!(out[3], Insn::UnaryOp(1, UnaryOp::Not, 2)),
        "the else-arm UnaryOp must survive an incoming edge to the merge test"
    );
    assert!(
        matches!(out[4], Insn::JumpIfFalse(1, 1)),
        "the merge test must continue to read the ternary result"
    );
}

// ── pass_thread_jumps ─────────────────────────────────────────────────────

#[test]
fn thread_jumps_collapses_chain() {
    // [0] Jump(1)  [1] LoadNone(0)  [2] Jump(1)  [3] LoadNone(1)  [4] Return(1)
    // Jump at 0 targets idx 2 (0+1+1=2). idx 2 is Jump(1) → idx 4.
    // After threading: Jump at 0 should target 4 directly → offset = 4-(0+1)=3.
    let insns = vec![
        Insn::Jump(1),     // 0 → 2
        Insn::LoadNone(0), // 1
        Insn::Jump(1),     // 2 → 4
        Insn::LoadNone(1), // 3
        Insn::Return(1),   // 4
    ];
    let out = pass_thread_jumps(insns);
    assert!(
        matches!(out[0], Insn::Jump(3)),
        "Jump(1) at 0 should be threaded to Jump(3) (target idx 4)"
    );
}

#[test]
fn thread_jumps_handles_self_loop() {
    // Jump(-1) loops to itself — threading must not infinite-loop.
    let insns = vec![Insn::Jump(-1)];
    let out = pass_thread_jumps(insns);
    assert_eq!(out.len(), 1);
    assert!(
        matches!(out[0], Insn::Jump(-1)),
        "self-loop must be left unchanged"
    );
}

#[test]
fn thread_jumps_stops_at_backward_jump() {
    // Simulates the nested-loop pattern that triggered issue #966.
    //
    // Layout:
    //   [0] ForIter(v, slot0, off=2)  — inner loop header
    //   [1] LoadNone(0)               — body instruction
    //   [2] Jump(-3)                  — inner back-edge → [0]
    //   [3] Jump(-4)                  — outer back-edge → before [0]
    //   [4] Return(0)
    //
    // Before the fix, follow() would start at [3] (the exit target of
    // ForIter off=2 → idx 0+1+2=3), see Jump(-4) and follow it to
    // idx 0 (3+1+(-4)=0), then see ForIter and stop. The computed
    // exit offset relative to [0] would be 0 - 0 - 1 = -1 (negative).
    //
    // After the fix, follow() stops at [3] because Jump(-4) is a backward jump
    // (k < 0). The offset for ForIter stays 2.
    let insns = vec![
        Insn::ForIter(0, 0, 2), // 0
        Insn::LoadNone(0),      // 1
        Insn::Jump(-3),         // 2  inner back-edge → 0
        Insn::Jump(-4),         // 3  outer back-edge → before [0] (simulated)
        Insn::Return(0),        // 4
    ];
    let out = pass_thread_jumps(insns);
    // The ForIter exit offset must remain 2 (not become negative).
    match out[0] {
        Insn::ForIter(_, _, off) => assert!(
            off >= 0,
            "ForIter off must not be negative after threading; got {off}"
        ),
        _ => panic!("insn[0] should still be ForIter"),
    }
}

#[test]
fn thread_conditional_jump_through_unconditional() {
    // [0] JumpIfFalse(r, 1)   [1] Jump(1)   [2] LoadNone(0)   [3] Return(0)
    // JumpIfFalse at 0 targets 2. idx 2 is Jump(1) targeting idx 4 (past end).
    // After threading JumpIfFalse should target idx 4 as well.
    let insns = vec![
        Insn::JumpIfFalse(0, 1), // 0 → target 2
        Insn::Jump(1),           // 1 → target 3
        Insn::LoadNone(0),       // 2
        Insn::Return(0),         // 3
    ];
    let out = pass_thread_jumps(insns);
    // JumpIfFalse at 0 targeted 2; idx 2 is LoadNone (not a Jump) so no threading there.
    // JumpIfFalse's target is idx 2 which is NOT a Jump — offset stays 1.
    assert!(
        matches!(out[0], Insn::JumpIfFalse(0, 1)),
        "no chain to thread for the conditional"
    );
    // Jump at 1 targets 3 (Return), which is not a Jump.
    assert!(matches!(out[1], Insn::Jump(1)));
}

#[test]
fn thread_jumps_no_change_when_no_chains() {
    // No jump chains → output equals input.
    let insns = vec![
        Insn::LoadNone(0),
        Insn::JumpIfFalse(0, 1),
        Insn::LoadNone(1),
        Insn::Return(0),
    ];
    let out = pass_thread_jumps(insns.clone());
    assert_eq!(out.len(), insns.len());
}

// ── Issue #2824: conditional-const binop must not fold across branches ────

#[test]
fn conditional_const_pow_after_merge_retains_dynamic_binop() {
    for c in ["0", "1"] {
        let src = format!(
            "def g(f, cond):\n    if cond:\n        k = {c}\n    else:\n        k = f\n    return 5 ** k\n"
        );
        let optimized = optimize(compile_fn(&src));
        let inner = &optimized.fn_protos[0].code;
        let has_dynamic_pow = inner
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::BinOp(_, _, crate::ast::BinaryOp::Pow, _)));
        assert!(
            has_dynamic_pow,
            "5 ** k with sibling k = {c} must keep dynamic BinOp(Pow); got {:?}",
            inner.insns
        );
        let has_const_pow = inner.insns.iter().any(|insn| {
            matches!(
                insn,
                Insn::BinOpConst(_, _, crate::ast::BinaryOp::Pow, _, _)
            )
        });
        assert!(
            !has_const_pow,
            "5 ** k with sibling k = {c} must not fuse RHS into BinOpConst; got {:?}",
            inner.insns
        );
    }
}

#[test]
fn conditional_const_sibling_ops_retain_dynamic_binop() {
    for (op_str, expected_op, const_val) in [
        ("**", crate::ast::BinaryOp::Pow, "0"),
        ("+", crate::ast::BinaryOp::Add, "0"),
        ("-", crate::ast::BinaryOp::Sub, "0"),
        ("*", crate::ast::BinaryOp::Mul, "1"),
        ("*", crate::ast::BinaryOp::Mul, "0"),
    ] {
        let src = format!(
            "def g(base, f, cond):\n    if cond:\n        k = {const_val}\n        res = base\n    elif not cond:\n        k = f\n        res = base {op_str} k\n    else:\n        res = 0\n    return res\n"
        );
        let optimized = optimize(compile_fn(&src));
        let inner = &optimized.fn_protos[0].code;
        let has_dynamic_op = inner
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::BinOp(_, _, op, _) if *op == expected_op));
        assert!(
            has_dynamic_op,
            "base {op_str} k with sibling k = {const_val} must keep dynamic BinOp; got {:?}",
            inner.insns
        );
    }
}

#[test]
fn genuine_const_pow_still_folds() {
    for (expr, expected) in [("5 ** 0", 1_i64), ("5 ** 1", 5_i64), ("5 ** 2", 25_i64)] {
        let src = format!("def f():\n    return {expr}\n");
        let optimized = optimize(compile_fn(&src));
        let inner = &optimized.fn_protos[0].code;
        let has_pow = inner.insns.iter().any(|insn| {
            matches!(
                insn,
                Insn::BinOp(_, _, crate::ast::BinaryOp::Pow, _)
                    | Insn::BinOpConst(_, _, crate::ast::BinaryOp::Pow, _, _)
            )
        });
        assert!(
            !has_pow,
            "genuine constant {expr} should fold at compile time; got {:?}",
            inner.insns
        );
        let has_expected_const = inner.consts.iter().any(|v| v.as_int() == Some(expected));
        assert!(
            has_expected_const,
            "folded constant pool for {expr} should contain {expected}"
        );
    }
}

// ── pass_dead_code ────────────────────────────────────────────────────────
