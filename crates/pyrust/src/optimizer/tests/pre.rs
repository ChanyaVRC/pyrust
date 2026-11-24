use super::*;
use crate::ast::BinaryOp;

fn count_binops(insns: &[Insn], op: BinaryOp) -> usize {
    insns
        .iter()
        .filter(|i| match i {
            Insn::BinOp(_, _, o, _)
            | Insn::BinOpConst(_, _, o, _, _)
            | Insn::BinOpImm(_, _, o, _, _) => *o == op,
            _ => false,
        })
        .count()
}

#[test]
fn pre_eliminates_partial_redundancy_in_diamond_with_else() {
    // Classic PRE diamond where `a` and `b` are non-constant exact ints
    // (proven via diamond-int assignments, so SCCP cannot fold `a * b` and
    // intra-block CSE cannot eliminate the second `a * b` across the join):
    //
    //   if p: a = 3 else: a = 5
    //   if a == 3: b = 7 else: b = 11
    //   if a < b:
    //       x = a * b
    //   else:
    //       x = 0
    //   y = a * b
    //   return x + y
    //
    // Before PRE: `a * b` is executed TWICE on the `a < b` path (in `then` and
    // in `join`).  After PRE: `a * b` is captured into a temp in `then`,
    // inserted as a compensation copy in `else`, and replaced by `Move` in
    // `join` — so the `then -> join` path executes only ONE multiplication!
    let consts = vec![
        Value::int(3),  // 0
        Value::int(5),  // 1
        Value::int(7),  // 2
        Value::int(11), // 3
        Value::int(0),  // 4
    ];
    // Regs: 0=p (param), 1=a, 2=b, 3=x, 4=y, 5=ret
    let insns = vec![
        Insn::JumpIfFalse(0, 2),                          // 0: if !p goto 3
        Insn::LoadConst(1, 0),                            // 1: a = 3
        Insn::Jump(1),                                    // 2: goto 4
        Insn::LoadConst(1, 1),                            // 3: a = 5
        Insn::CmpJumpIfFalseConst(1, BinaryOp::Eq, 0, 2), // 4: if !(a == 3) goto 7
        Insn::LoadConst(2, 2),                            // 5: b = 7
        Insn::Jump(1),                                    // 6: goto 8
        Insn::LoadConst(2, 3),                            // 7: b = 11
        // Diamond with partial redundancy:
        Insn::CmpJumpIfFalse(1, BinaryOp::Lt, 2, 2), // 8: if !(a < b) goto 11
        Insn::BinOp(3, 1, BinaryOp::Mul, 2),         // 9: x = a * b
        Insn::Jump(1),                               // 10: goto 12
        Insn::LoadConst(3, 4),                       // 11: x = 0 (else)
        Insn::BinOp(4, 1, BinaryOp::Mul, 2),         // 12: y = a * b (join - partially redundant!)
        Insn::BinOp(5, 3, BinaryOp::Add, 4),         // 13: ret = x + y
        Insn::Return(5),                             // 14
    ];

    let mut num_regs = 6;
    let out = pass_pre(insns, &[], &consts, 5, &mut num_regs);

    // The join block's `BinOp(4, 1, Mul, 2)` must be replaced by `Move(4, temp)`.
    assert!(
        out.iter().any(|i| matches!(i, Insn::Move(4, t) if *t >= 6)),
        "join-block `y = a * b` must be replaced by Move from PRE temp: {out:?}"
    );
    assert!(
        !out.iter()
            .any(|i| matches!(i, Insn::BinOp(4, 1, BinaryOp::Mul, 2))),
        "original redundant `y = a * b` must be eliminated: {out:?}"
    );
    assert!(num_regs > 6, "PRE must allocate a fresh temp register");
}

#[test]
fn pre_eliminates_partial_redundancy_in_diamond_without_else() {
    // Classic diamond WITHOUT an `else` body (`if a < b: x = a * b; y = a * b`):
    // `a * b` is anticipated at BOTH successors of the branch (`then` and
    // `join`), so PRE hoists `temp = a * b` right before `if a < b` and
    // replaces BOTH `x = a * b` and `y = a * b` with `Move`!
    let consts = vec![
        Value::int(3), // 0
        Value::int(5), // 1
        Value::int(7), // 2
    ];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),                     // 0
        Insn::LoadConst(1, 0),                       // 1: a = 3
        Insn::Jump(1),                               // 2
        Insn::LoadConst(1, 1),                       // 3: a = 5
        Insn::LoadConst(2, 2),                       // 4: b = 7
        Insn::CmpJumpIfFalse(1, BinaryOp::Lt, 2, 1), // 5: if !(a < b) goto 7
        Insn::BinOp(3, 1, BinaryOp::Mul, 2),         // 6: x = a * b
        Insn::BinOp(4, 1, BinaryOp::Mul, 2),         // 7: y = a * b
        Insn::Return(4),                             // 8
    ];

    let mut num_regs = 5;
    let out = pass_pre(insns, &[], &consts, 5, &mut num_regs);

    // Total `Mul` count drops from 2 down to 1!
    assert_eq!(
        count_binops(&out, BinaryOp::Mul),
        1,
        "diamond without else should hoist a*b once before branch: {out:?}"
    );
}

#[test]
fn pre_hoists_loop_invariant_pure_expression_out_of_do_while_loop() {
    //   a = 3 if p else 5
    //   b = 7
    //   i = 0
    //   while True:
    //       x = a * b       <-- loop-invariant, anticipated at loop header!
    //       i += 1
    //       if i >= 10: break
    //   return x
    //
    // PRE inserts `t = a * b` in the preheader and replaces the in-loop
    // `BinOp(x, a, Mul, b)` with `Move(x, t)`.
    let consts = vec![
        Value::int(3),  // 0
        Value::int(5),  // 1
        Value::int(7),  // 2
        Value::int(0),  // 3
        Value::int(10), // 4
    ];
    let insns = vec![
        Insn::JumpIfFalse(0, 2), // 0
        Insn::LoadConst(1, 0),   // 1: a = 3
        Insn::Jump(1),           // 2
        Insn::LoadConst(1, 1),   // 3: a = 5
        Insn::LoadConst(2, 2),   // 4: b = 7
        Insn::LoadConst(3, 3),   // 5: i = 0
        // Loop header (pc 6):
        Insn::BinOp(4, 1, BinaryOp::Mul, 2), // 6: x = a * b
        Insn::BinOpImm(3, 3, BinaryOp::Add, 1, true), // 7: i += 1
        Insn::CmpJumpIfFalseConst(3, BinaryOp::Ge, 4, -3), // 8: if !(i >= 10) goto 6
        Insn::Return(4),                     // 9: return x
    ];

    let mut num_regs = 5;
    let out = pass_pre(insns, &[], &consts, 5, &mut num_regs);

    assert_eq!(
        count_binops(&out, BinaryOp::Mul),
        1,
        "loop-invariant Mul should be hoisted to preheader: {out:?}"
    );
    // The Mul must now appear BEFORE the loop back-edge target, and `x` inside
    // the loop must be populated via `Move(4, temp)`.
    let mul_pos = out
        .iter()
        .position(|i| matches!(i, Insn::BinOp(_, 1, BinaryOp::Mul, 2)))
        .expect("hoisted Mul must exist");
    let move_pos = out
        .iter()
        .position(|i| matches!(i, Insn::Move(4, t) if *t >= 5))
        .expect("in-loop Move(4, temp) must exist");
    assert!(
        mul_pos < move_pos,
        "Mul (pc {mul_pos}) must be hoisted before in-loop Move (pc {move_pos}): {out:?}"
    );
}

#[test]
fn pre_does_not_hoist_when_operand_is_killed_on_one_branch() {
    //   if a < b:
    //       x = a * b
    //   else:
    //       a = 99      <-- kills `a`!
    //   y = a * b       <-- NOT partially redundant on the else path with old `a`
    let consts = vec![
        Value::int(3),  // 0
        Value::int(5),  // 1
        Value::int(7),  // 2
        Value::int(99), // 3
    ];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),                     // 0
        Insn::LoadConst(1, 0),                       // 1: a = 3
        Insn::Jump(1),                               // 2
        Insn::LoadConst(1, 1),                       // 3: a = 5
        Insn::LoadConst(2, 2),                       // 4: b = 7
        Insn::CmpJumpIfFalse(1, BinaryOp::Lt, 2, 2), // 5: if !(a < b) goto 8
        Insn::BinOp(3, 1, BinaryOp::Mul, 2),         // 6: x = a * b
        Insn::Jump(1),                               // 7: goto 9
        Insn::LoadConst(1, 3),                       // 8: a = 99 (kills a!)
        Insn::BinOp(4, 1, BinaryOp::Mul, 2),         // 9: y = a * b
        Insn::Return(4),                             // 10
    ];

    let mut num_regs = 5;
    let out = pass_pre(insns.clone(), &[], &consts, 5, &mut num_regs);

    // Note: in `else`, `a = 99` kills `a` BEFORE exit, so `ant_in[else] = 0`,
    // `ant_out[else] = 1` (after `a = 99`!).  PRE MAY insert `t = a * b` AFTER
    // `a = 99` in `else` (using the NEW `a = 99`!), which IS sound — verify it
    // NEVER hoists BEFORE `a = 99`!
    let load_99_pos = out
        .iter()
        .position(|i| matches!(i, Insn::LoadConst(1, 3)))
        .expect("a = 99 must remain");
    let jump_to_else = out
        .iter()
        .position(|i| matches!(i, Insn::CmpJumpIfFalse(1, BinaryOp::Lt, 2, _)))
        .expect("branch must remain");
    // No Mul may appear before the branch (that would compute with stale `a`!).
    assert!(
        !out[..jump_to_else]
            .iter()
            .any(|i| matches!(i, Insn::BinOp(_, 1, BinaryOp::Mul, 2))),
        "must NEVER hoist a * b above `a = 99` kill: {out:?}"
    );
    let _ = load_99_pos;
}

#[test]
fn pre_does_not_hoist_when_not_anticipated_on_early_return_branch() {
    //   if a < 5:
    //       return 0   <-- early exit: a * b is NOT anticipated here!
    //   y = a * b
    //   return y
    let consts = vec![
        Value::int(3), // 0
        Value::int(5), // 1
        Value::int(7), // 2
        Value::int(0), // 3
    ];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),                          // 0
        Insn::LoadConst(1, 0),                            // 1: a = 3
        Insn::Jump(1),                                    // 2
        Insn::LoadConst(1, 1),                            // 3: a = 5
        Insn::LoadConst(2, 2),                            // 4: b = 7
        Insn::CmpJumpIfFalseConst(1, BinaryOp::Lt, 1, 2), // 5: if !(a < 5) goto 8
        Insn::LoadConst(3, 3),                            // 6: ret = 0
        Insn::Return(3),                                  // 7: return 0
        Insn::BinOp(4, 1, BinaryOp::Mul, 2),              // 8: y = a * b
        Insn::Return(4),                                  // 9
    ];

    let mut num_regs = 5;
    let out = pass_pre(insns.clone(), &[], &consts, 5, &mut num_regs);

    assert_eq!(
        out, insns,
        "non-anticipated expression must not be hoisted past early return"
    );
}

#[test]
fn pre_never_hoists_across_try_except_boundary() {
    //   try:
    //       x = a * b
    //   except:
    //       x = 0
    //   y = a * b
    //
    // `SetupExcept` / `PopExcept` separate exception regions; PRE must never
    // hoist or share across the try/except boundary.
    let consts = vec![
        Value::int(3), // 0
        Value::int(5), // 1
        Value::int(7), // 2
        Value::int(0), // 3
    ];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),             // 0
        Insn::LoadConst(1, 0),               // 1: a = 3
        Insn::Jump(1),                       // 2
        Insn::LoadConst(1, 1),               // 3: a = 5
        Insn::LoadConst(2, 2),               // 4: b = 7
        Insn::SetupExcept(3),                // 5: handler at 9
        Insn::BinOp(3, 1, BinaryOp::Mul, 2), // 6: x = a * b (inside try)
        Insn::PopExcept,                     // 7
        Insn::Jump(1),                       // 8: goto 10
        Insn::LoadConst(3, 3),               // 9: handler: x = 0
        Insn::BinOp(4, 1, BinaryOp::Mul, 2), // 10: y = a * b (outside try)
        Insn::Return(4),                     // 11
    ];

    let mut num_regs = 5;
    let out = pass_pre(insns.clone(), &[], &consts, 5, &mut num_regs);

    assert_eq!(
        out, insns,
        "PRE must not hoist or deduplicate across SetupExcept/PopExcept regions"
    );
}

#[test]
fn pre_never_hoists_division_by_unknown_or_zero_divisor() {
    //   if a < b:
    //       x = a // b   <-- b could be 0! Must NEVER be hoisted!
    //   else:
    //       x = 0
    //   y = a // b
    let consts = vec![
        Value::int(3), // 0
        Value::int(5), // 1
        Value::int(0), // 2
    ];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),                     // 0
        Insn::LoadConst(1, 0),                       // 1: a = 3
        Insn::Jump(1),                               // 2
        Insn::LoadConst(1, 1),                       // 3: a = 5
        Insn::LoadConst(2, 2),                       // 4: b = 0
        Insn::CmpJumpIfFalse(1, BinaryOp::Lt, 2, 2), // 5
        Insn::BinOp(3, 1, BinaryOp::FloorDiv, 2),    // 6: a // b
        Insn::Jump(1),                               // 7
        Insn::LoadConst(3, 2),                       // 8
        Insn::BinOp(4, 1, BinaryOp::FloorDiv, 2),    // 9: a // b
        Insn::Return(4),                             // 10
    ];

    let mut num_regs = 5;
    let out = pass_pre(insns.clone(), &[], &consts, 5, &mut num_regs);

    assert_eq!(
        out, insns,
        "FloorDiv with non-constant divisor must never be hoisted by PRE"
    );
}

#[test]
fn pre_never_deduplicates_untyped_user_operands() {
    // Untyped parameters `a` (reg 1) and `b` (reg 2) could be instances of a
    // user class with stateful `__mul__`.  PRE must leave BOTH `a * b` intact!
    let consts = vec![Value::int(0)];
    let insns = vec![
        Insn::JumpIfFalse(0, 2),             // 0: if !cond goto 3
        Insn::BinOp(3, 1, BinaryOp::Mul, 2), // 1: x = a * b
        Insn::Jump(1),                       // 2: goto 4
        Insn::LoadConst(3, 0),               // 3: x = 0
        Insn::BinOp(4, 1, BinaryOp::Mul, 2), // 4: y = a * b
        Insn::Return(4),                     // 5
    ];

    let mut num_regs = 5;
    let out = pass_pre(insns.clone(), &[], &consts, 5, &mut num_regs);

    assert_eq!(
        out, insns,
        "PRE must not hoist or eliminate BinOp on untyped operands that may invoke user __mul__"
    );
}
