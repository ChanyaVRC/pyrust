use super::*;
use crate::ast::BinaryOp;

#[test]
fn sccp_propagates_across_dead_branch_and_folds_downstream_condition() {
    // Equivalent to:
    //   x = 1
    //   if False:
    //       x = 5
    //   if x == 5:
    //       return 100
    //   return x + 10
    //
    // Linear `pass_const_fold` clears its map at the merge after `if False`,
    // so it cannot prove `x == 1` at `if x == 5` or fold `x + 10`.  SCCP marks
    // the `x = 5` arm non-executable, keeps `x = 1` across the join, folds
    // `x == 5` to False, eliminates `return 100`, and folds `x + 10` to `11`.
    let mut consts = vec![
        Value::int(1),       // 0
        Value::bool_(false), // 1
        Value::int(5),       // 2
        Value::int(100),     // 3
        Value::int(10),      // 4
    ];
    let insns = vec![
        Insn::LoadConst(0, 0),                            // 0: x = 1
        Insn::LoadConst(1, 1),                            // 1: t0 = False
        Insn::JumpIfFalse(1, 1),                          // 2: if !t0 goto 4
        Insn::LoadConst(0, 2),                            // 3: x = 5 (dead)
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 2, 2), // 4: if !(x == 5) goto 7
        Insn::LoadConst(2, 3),                            // 5: t1 = 100 (dead)
        Insn::Return(2),                                  // 6: return t1 (dead)
        Insn::BinOpConst(2, 0, BinaryOp::Add, 4, false),  // 7: t1 = x + 10 -> 11
        Insn::Return(2),                                  // 8: return t1
    ];

    let out = pass_sccp(insns, &mut consts, 1);

    assert!(
        !out.iter().any(|i| matches!(
            i,
            Insn::JumpIfFalse(..) | Insn::CmpJumpIfFalseConst(..) | Insn::BinOpConst(..)
        )),
        "SCCP should fold both branches and the downstream addition: {out:?}"
    );
    let folded_return = out.iter().find_map(|i| match i {
        Insn::LoadConst(2, c) => Some(&consts[*c as usize]),
        _ => None,
    });
    assert_eq!(
        folded_return.and_then(Value::as_int),
        Some(11),
        "return temp should hold folded constant 11: {out:?}"
    );
}

#[test]
fn sccp_folds_if_true_else_and_computes_downstream_arithmetic() {
    // Equivalent to:
    //   if True: x = 2
    //   else:    x = 3
    //   return x + 5
    let mut consts = vec![
        Value::bool_(true), // 0
        Value::int(2),      // 1
        Value::int(3),      // 2
        Value::int(5),      // 3
    ];
    let insns = vec![
        Insn::LoadConst(1, 0),                           // 0: t0 = True
        Insn::JumpIfFalse(1, 2),                         // 1: if !t0 goto 4
        Insn::LoadConst(0, 1),                           // 2: x = 2
        Insn::Jump(1),                                   // 3: goto 5
        Insn::LoadConst(0, 2),                           // 4: x = 3 (dead)
        Insn::BinOpConst(2, 0, BinaryOp::Add, 3, false), // 5: t1 = x + 5 -> 7
        Insn::Return(2),                                 // 6: return t1
    ];

    let out = pass_sccp(insns, &mut consts, 1);

    assert!(
        !out.iter().any(|i| matches!(i, Insn::BinOpConst(..))),
        "x + 5 must fold to LoadConst(7): {out:?}"
    );
    let folded_val = out.iter().find_map(|i| match i {
        Insn::LoadConst(2, c) => Some(&consts[*c as usize]),
        _ => None,
    });
    assert_eq!(folded_val.and_then(Value::as_int), Some(7));
}

#[test]
fn sccp_diamond_merge_agrees_on_equal_constants() {
    // Unknown parameter `p` (reg 0):
    //   if p: x = 7
    //   else: x = 7
    //   return x + 1  -> must fold to 8!
    let mut consts = vec![Value::int(7), Value::int(1)];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),                         // 0: if !p goto 3
        Insn::LoadConst(1, 0),                           // 1: x = 7
        Insn::Jump(1),                                   // 2: goto 4
        Insn::LoadConst(1, 0),                           // 3: x = 7
        Insn::BinOpConst(2, 1, BinaryOp::Add, 1, false), // 4: t = x + 1 -> 8
        Insn::Return(2),                                 // 5
    ];

    let out = pass_sccp(insns, &mut consts, 2);

    assert!(
        !out.iter().any(|i| matches!(i, Insn::BinOpConst(..))),
        "diamond join where both reachable arms produce 7 must fold x + 1: {out:?}"
    );
    let folded_val = out.iter().find_map(|i| match i {
        Insn::LoadConst(2, c) => Some(&consts[*c as usize]),
        _ => None,
    });
    assert_eq!(folded_val.and_then(Value::as_int), Some(8));
}

#[test]
fn sccp_diamond_merge_disagrees_to_bottom() {
    // Unknown parameter `p` (reg 0):
    //   if p: x = 7
    //   else: x = 8
    //   return x + 1  -> MUST NOT fold!
    let mut consts = vec![Value::int(7), Value::int(8), Value::int(1)];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),                         // 0: if !p goto 3
        Insn::LoadConst(1, 0),                           // 1: x = 7
        Insn::Jump(1),                                   // 2: goto 4
        Insn::LoadConst(1, 1),                           // 3: x = 8
        Insn::BinOpConst(2, 1, BinaryOp::Add, 2, false), // 4: t = x + 1
        Insn::Return(2),                                 // 5
    ];

    let out = pass_sccp(insns, &mut consts, 2);

    assert!(
        out.iter()
            .any(|i| matches!(i, Insn::BinOpConst(2, 1, BinaryOp::Add, 2, false))),
        "diamond join with disagreeing constants must meet to Bottom: {out:?}"
    );
}

#[test]
fn sccp_loop_header_converges_to_bottom_and_never_folds_induction_var() {
    //   i = 0
    //   while i < 10:
    //       i = i + 1
    //   return i
    //
    // First iteration sees i = 0; back-edge brings i = 1; join at header must
    // drop i to Bottom so `i < 10` and `i + 1` are NEVER folded!
    let mut consts = vec![Value::int(0), Value::int(10), Value::int(1)];
    let insns = vec![
        Insn::LoadConst(0, 0),                            // 0: i = 0
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 1, 2), // 1: header: if !(i < 10) goto 4
        Insn::BinOpConst(0, 0, BinaryOp::Add, 2, true),   // 2: i += 1
        Insn::Jump(-3),                                   // 3: goto 1
        Insn::Return(0),                                  // 4: return i
    ];

    let out = pass_sccp(insns.clone(), &mut consts, 1);

    assert_eq!(
        out, insns,
        "loop induction variable must converge to Bottom and stay untouched"
    );
}

#[test]
fn sccp_eliminates_unreachable_loop_with_false_entry_condition() {
    //   i = 0
    //   while i > 10:
    //       i = i + 1
    //   return i + 5  -> loop body and back-edge are unreachable; folds to 5!
    let mut consts = vec![Value::int(0), Value::int(10), Value::int(1), Value::int(5)];
    let insns = vec![
        Insn::LoadConst(0, 0),                            // 0: i = 0
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Gt, 1, 2), // 1: if !(i > 10) goto 4
        Insn::BinOpConst(0, 0, BinaryOp::Add, 2, true),   // 2: i += 1 (dead)
        Insn::Jump(-3),                                   // 3: back-edge (dead)
        Insn::BinOpConst(1, 0, BinaryOp::Add, 3, false),  // 4: t = i + 5 -> 5
        Insn::Return(1),                                  // 5
    ];

    let out = pass_sccp(insns, &mut consts, 1);

    assert!(
        !out.iter().any(|i| matches!(i, Insn::BinOpConst(..))),
        "unreachable loop back-edge must not taint i; i + 5 folds to 5: {out:?}"
    );
    let folded_val = out.iter().find_map(|i| match i {
        Insn::LoadConst(1, c) => Some(&consts[*c as usize]),
        _ => None,
    });
    assert_eq!(folded_val.and_then(Value::as_int), Some(5));
}

#[test]
fn sccp_preserves_exception_handler_and_never_folds_past_raise() {
    //   try:
    //       x = 1
    //       raise ValueError()  (simulated by RaiseValue(exc))
    //       x = 2
    //   except:
    //       return x + 10
    //
    // `except` MUST remain reachable and `x` at the handler MUST NOT fold to 2
    // (nor unsoundly assume a single value if pre-try was Bottom).
    let mut consts = vec![Value::int(1), Value::int(2), Value::int(10)];
    let insns = vec![
        Insn::SetupExcept(4),                            // 0: handler at 5
        Insn::LoadConst(0, 0),                           // 1: x = 1
        Insn::RaiseValue(1),                             // 2: raise exc
        Insn::LoadConst(0, 1),                           // 3: x = 2 (dead)
        Insn::PopExcept,                                 // 4: dead
        Insn::BinOpConst(2, 0, BinaryOp::Add, 2, false), // 5: handler: t = x + 10
        Insn::Return(2),                                 // 6: return t
    ];

    let out = pass_sccp(insns, &mut consts, 2);

    assert!(
        out.iter().any(|i| matches!(i, Insn::SetupExcept(_))),
        "SetupExcept must be preserved: {out:?}"
    );
    assert!(
        out.iter()
            .any(|i| matches!(i, Insn::BinOpConst(2, 0, BinaryOp::Add, 2, false))),
        "handler must remain reachable and not fold x to 2: {out:?}"
    );
}

#[test]
fn sccp_preserves_unbound_local_when_only_assigned_on_one_branch() {
    //   if cond:
    //       x = 1
    //   CheckLocal(x)
    //   return x + 1
    //
    // Because entry starts at Bottom (not Top), the branch where `x` is never
    // assigned meets with `Const(1)` to produce `Bottom` — preserving both
    // `CheckLocal` and `x + 1` so `UnboundLocalError` is raised when cond=False.
    let mut consts = vec![Value::int(1)];
    let insns = vec![
        Insn::JumpIfFalse(0, 1),                         // 0: if !cond goto 2
        Insn::LoadConst(1, 0),                           // 1: x = 1
        Insn::CheckLocal(1, 0),                          // 2: must stay
        Insn::BinOpConst(2, 1, BinaryOp::Add, 0, false), // 3: must NOT fold!
        Insn::Return(2),                                 // 4
    ];

    let out = pass_sccp(insns.clone(), &mut consts, 2);

    assert_eq!(
        out, insns,
        "conditionally-assigned local must meet to Bottom and preserve CheckLocal"
    );
}

#[test]
fn sccp_preserves_int_vs_bool_and_neg_zero_distinctions() {
    // Diamond 1: if p: x = 1 else: x = True  -> meet(Int(1), Bool(true)) = Bottom!
    let mut consts = vec![Value::int(1), Value::bool_(true)];
    let insns_bool = vec![
        Insn::JumpIfFalse(0, 2),
        Insn::LoadConst(1, 0), // Int(1)
        Insn::Jump(1),
        Insn::LoadConst(1, 1), // Bool(true)
        Insn::Move(2, 1), // MUST NOT rewrite Move to LoadConst(Int(1)) or LoadConst(Bool(true))
        Insn::Return(2),
    ];
    let out_bool = pass_sccp(insns_bool.clone(), &mut consts, 2);
    assert_eq!(
        out_bool, insns_bool,
        "Int(1) and Bool(true) must never unify in SCCP lattice meet"
    );

    // Diamond 2: if p: x = 0.0 else: x = -0.0 -> meet(+0.0, -0.0) = Bottom!
    let mut fconsts = vec![Value::float(0.0), Value::float(-0.0)];
    let insns_zero = vec![
        Insn::JumpIfFalse(0, 2),
        Insn::LoadConst(1, 0), // +0.0
        Insn::Jump(1),
        Insn::LoadConst(1, 1), // -0.0
        Insn::Move(2, 1),      // MUST NOT rewrite Move!
        Insn::Return(2),
    ];
    let out_zero = pass_sccp(insns_zero.clone(), &mut fconsts, 2);
    assert_eq!(
        out_zero, insns_zero,
        "+0.0 and -0.0 must never unify in SCCP lattice meet"
    );
}

#[test]
fn sccp_does_not_fold_division_by_zero() {
    let mut consts = vec![Value::int(1), Value::int(0)];
    let insns = vec![
        Insn::LoadConst(0, 0),                           // 0: x = 1
        Insn::BinOpConst(1, 0, BinaryOp::Div, 1, false), // 1: 1 / 0 -> MUST NOT fold
        Insn::Return(1),                                 // 2
    ];
    let out = pass_sccp(insns.clone(), &mut consts, 1);
    assert_eq!(
        out, insns,
        "division by zero must remain in bytecode to raise ZeroDivisionError at runtime"
    );
}

#[test]
fn sccp_preserves_lineno_for_surviving_raise_after_dead_branch_removal() {
    // End-to-end compile + optimize check: dead branch eliminated by SCCP must
    // not shift the line number of a surviving ZeroDivisionError.
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
    // The surviving floor-division instruction must still be present (not folded).
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
    // Neither the `x = 5` nor `return 100` constant should survive.
    assert!(
        !inner.consts.iter().any(|v| v.as_int() == Some(100)),
        "dead `return 100` branch should be eliminated by SCCP: {:?}",
        inner.consts
    );
}
