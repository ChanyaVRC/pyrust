use super::*;

#[test]
fn fold_const_tuple_two_consts() {
    use crate::value::Value;
    let mut consts = vec![Value::int(10), Value::int(20)];
    let insns = vec![
        Insn::LoadConst(2, 0),
        Insn::LoadConst(3, 1),
        Insn::BuildTuple(4, 2, 2),
        Insn::Return(4),
    ];
    let out = pass_fold_const_tuple(insns, 2, &mut consts);
    assert_eq!(out.len(), 2);
    assert!(matches!(out[0], Insn::LoadConst(4, _)));
    let new_idx = match out[0] {
        Insn::LoadConst(_, i) => i,
        _ => panic!("expected LoadConst"),
    };
    let elems = consts[new_idx as usize]
        .as_tuple()
        .expect("new constant should be a tuple");
    assert_eq!(elems.len(), 2);
    assert!(matches!(elems[0].kind(), crate::value::ValueKind::Int(10)));
    assert!(matches!(elems[1].kind(), crate::value::ValueKind::Int(20)));
}

#[test]
fn fold_const_tuple_skips_local_regs() {
    use crate::value::Value;
    let mut consts = vec![Value::int(1), Value::int(2)];
    let insns = vec![
        Insn::LoadConst(1, 0),
        Insn::LoadConst(2, 1),
        Insn::BuildTuple(5, 1, 2),
        Insn::Return(5),
    ];
    let out = pass_fold_const_tuple(insns, 3, &mut consts);
    assert_eq!(
        out.len(),
        4,
        "should not fold when base register is a local"
    );
    assert!(matches!(out[2], Insn::BuildTuple(5, 1, 2)));
}

// ── pass_dead_store_elim ──────────────────────────────────────────────────

#[test]
fn dse_removes_overwritten_load_const() {
    // LoadConst(r2, 0) immediately overwritten by LoadConst(r2, 1); first is dead.
    let insns = vec![
        Insn::LoadConst(2, 0), // dead — r2 written again before any read
        Insn::LoadConst(2, 1), // live — r2 used by Return
        Insn::Return(2),
    ];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(out.len(), 2, "dead LoadConst should be removed");
    assert!(matches!(out[0], Insn::LoadConst(2, 1)));
    assert!(matches!(out[1], Insn::Return(2)));
}

#[test]
fn dse_keeps_store_that_is_read() {
    // LoadConst(r2, 0) is read by Return — must be kept.
    let insns = vec![Insn::LoadConst(2, 0), Insn::Return(2)];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(out.len(), 2, "live LoadConst must not be removed");
}

#[test]
fn dse_keeps_local_register_writes() {
    // Register r0 < num_locals=2 — locals must not be eliminated.
    let insns = vec![
        Insn::LoadConst(0, 0), // local reg — keep even if "dead"
        Insn::LoadConst(0, 1), // overwrites r0
        Insn::Return(0),
    ];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(out.len(), 3, "local register writes must not be removed");
}

#[test]
fn dse_skips_when_back_edge_present_and_register_is_read() {
    // LoadConst(r2, 0) followed by a loop back-edge, and r2 IS read (by
    // Return(r2) after the loop) — must be kept because it is live.
    //
    //   [0] LoadConst(r2, 0)   ← r2 read by [2]
    //   [1] Jump(-2)           ← back-edge (target = 1+1-2 = 0)
    //   [2] Return(r2)
    let insns = vec![
        Insn::LoadConst(2, 0), // r2 is live (read by Return)
        Insn::Jump(-2),        // back-edge
        Insn::Return(2),
    ];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(
        out.len(),
        3,
        "must not remove store when register is read globally and back-edge is present"
    );
}

#[test]
fn dse_removes_dead_move() {
    use crate::ast::BinaryOp;
    // Move(r3, r2) followed immediately by BinOp(r3, ...) — Move is dead.
    let insns = vec![
        Insn::LoadConst(2, 0),
        Insn::Move(3, 2),                    // dead — r3 overwritten below
        Insn::BinOp(3, 2, BinaryOp::Add, 2), // overwrites r3
        Insn::Return(3),
    ];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(out.len(), 3, "dead Move should be removed");
    assert!(matches!(out[1], Insn::BinOp(3, 2, BinaryOp::Add, 2)));
}

#[test]
fn dse_keeps_then_branch_write_across_jump() {
    // Issue #361: a ternary's `then`-arm writes a temp, then jumps over the
    // `else`-arm's write to the same temp.  A purely linear scan would see
    // the else-arm's LoadConst as the "next write" and incorrectly delete
    // the then-arm's store — leaving the temp unset on the taken path.
    //
    //   [0] JumpIfFalse(0, 2)     # if !c, skip to [3]
    //   [1] LoadConst(2, 0)       # then-arm: r2 = consts[0]
    //   [2] Jump(1)               # skip [3]
    //   [3] LoadConst(2, 1)       # else-arm: r2 = consts[1]
    //   [4] Return(2)
    //
    // The store at [1] is REACHABLE and consumed by [4] along the taken
    // path (c truthy).  DSE must keep it.
    let insns = vec![
        Insn::JumpIfFalse(0, 2),
        Insn::LoadConst(2, 0), // <-- must NOT be removed
        Insn::Jump(1),
        Insn::LoadConst(2, 1),
        Insn::Return(2),
    ];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(
        out.len(),
        5,
        "the then-arm's LoadConst must survive — its value is read on the taken path",
    );
    assert!(matches!(out[1], Insn::LoadConst(2, 0)));
}

#[test]
fn dse_global_read_count_removes_zero_read_temp_inside_loop() {
    // A LoadConst for a temp register that is never read anywhere in the
    // function body — not even inside the loop — should be removed even
    // though there is a back-edge after it.
    //
    // Before this change, pass_dead_store_elim would see the back-edge
    // (Jump(-N)) after the LoadConst and conservatively keep it.  With
    // the global-read-count pre-scan, it detects that r5 has 0 global
    // reads and removes the instruction unconditionally.
    //
    //   [0] LoadConst(r2, 0)   ← live — used by Return at [2]
    //   [1] LoadConst(r5, 0)   ← dead temp, r5 never read
    //   [2] Jump(-3)           ← back-edge (target = 2+1-3 = 0)
    //   [3] Return(r2)
    //
    // Expected: LoadConst(r5) removed; other instructions kept; Jump offset
    //           updated by compact to reflect the removed slot.
    let insns = vec![
        Insn::LoadConst(2, 0), // r2 — live (read by Return)
        Insn::LoadConst(5, 0), // r5 — dead temp, r5 never read globally
        Insn::Jump(-3),        // back-edge: target = 2+1-3 = 0
        Insn::Return(2),
    ];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(
        out.len(),
        3,
        "LoadConst for zero-global-read temp should be removed even with back-edge present"
    );
    // The remaining instructions are: LoadConst(r2), Jump(updated), Return(r2).
    assert!(
        matches!(out[0], Insn::LoadConst(2, 0)),
        "live LoadConst must survive: {:?}",
        out[0]
    );
    // compact rewrites the jump offset: old target was index 0; in the new
    // array that is still index 0, so new_offset = 0 - (1 + 1) = -2.
    assert!(
        matches!(out[1], Insn::Jump(-2)),
        "back-edge Jump must survive with updated offset: {:?}",
        out[1]
    );
    assert!(
        matches!(out[2], Insn::Return(2)),
        "Return must survive: {:?}",
        out[2]
    );
}

#[test]
fn dse_keeps_dead_call_memo() {
    // A memoizable call remains observable even when its result is dead: its
    // callee can be rebound and it can still raise.
    let insns = vec![
        Insn::LoadGlobal(2, 0), // r2 = some_pure_fn
        Insn::CallMemo(2, 0),   // r2 = some_pure_fn() — result dead
        Insn::ReturnNone,
    ];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(out.len(), 3, "dead CallMemo must be preserved");
    assert!(
        out.iter().any(|i| matches!(i, Insn::CallMemo(..))),
        "dead CallMemo must survive"
    );
}

#[test]
fn comparison_body_is_memo_pure() {
    let code = optimize(compile_fn("def f(a):\n    a < a\n"));
    let f = code
        .fn_protos
        .iter()
        .find(|p| &*p.name == "f")
        .expect("f proto");
    assert!(f.is_memo_pure, "`a < a` body is memo-pure (cacheable)");
}

#[test]
fn unary_body_is_memo_pure() {
    let code = optimize(compile_fn("def f(a):\n    -a\n"));
    let f = code
        .fn_protos
        .iter()
        .find(|p| &*p.name == "f")
        .expect("f proto");
    assert!(f.is_memo_pure, "`-a` body is memo-pure");
}

#[test]
fn nonraising_arithmetic_body_is_memo_pure() {
    let code = optimize(compile_fn("def f(a):\n    return a + 1\n"));
    let f = code
        .fn_protos
        .iter()
        .find(|p| &*p.name == "f")
        .expect("f proto");
    assert!(f.is_memo_pure, "`a + 1` body is memo-pure");
}

#[test]
fn optimizer_keeps_dead_call_to_nested_memo_pure_fn() {
    // Memo purity authorizes result caching, not removal of the call itself.
    let code = optimize(compile_fn(
        "def outer(a):\n    def g(x):\n        return x + 1\n    g(a)\n    return a\n",
    ));
    let outer = code
        .fn_protos
        .iter()
        .find(|p| &*p.name == "outer")
        .expect("outer proto");
    let g = outer
        .code
        .fn_protos
        .iter()
        .find(|p| &*p.name == "g")
        .expect("g proto");
    assert!(g.is_memo_pure, "g (x + 1) must be memo-pure");
    assert!(
        outer
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::Call(..) | Insn::CallMemo(..))),
        "dead call to memo-pure g must remain observable: {:?}",
        outer.code.insns
    );
}

#[test]
fn optimizer_keeps_dead_call_to_nested_comparison_fn() {
    let code = optimize(compile_fn(
        "def outer(a):\n    def g(x):\n        return x < x\n    g(a)\n    return a\n",
    ));
    let outer = code
        .fn_protos
        .iter()
        .find(|p| &*p.name == "outer")
        .expect("outer proto");
    let g = outer
        .code
        .fn_protos
        .iter()
        .find(|p| &*p.name == "g")
        .expect("g proto");
    assert!(
        g.is_memo_pure,
        "g (x < x) is memo-pure (cacheable for int args)"
    );
    assert!(
        outer
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::Call(..) | Insn::CallMemo(..))),
        "dead comparison call must remain observable: {:?}",
        outer.code.insns
    );
}

#[test]
fn dse_keeps_call_memo_whose_result_is_used() {
    // CallMemo(r2, 0) whose result is returned — must NOT be dropped.
    let insns = vec![
        Insn::LoadGlobal(2, 0), // r2 = some_fn
        Insn::CallMemo(2, 0),   // r2 = some_fn() — result used by Return
        Insn::Return(2),
    ];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(out.len(), 3, "live CallMemo must not be removed");
    assert!(matches!(out[1], Insn::CallMemo(2, 0)));
}

#[test]
fn dse_keeps_call_memo_local_reg() {
    // CallMemo targeting a local register (r1 < num_locals=2) — must be kept.
    let insns = vec![
        Insn::LoadGlobal(1, 0), // r1 = fn
        Insn::CallMemo(1, 0),   // r1 = fn() — local, must not be removed
        Insn::ReturnNone,
    ];
    let out = pass_dead_store_elim(insns, 2);
    assert_eq!(out.len(), 3, "CallMemo to local must not be removed");
}

// ── pass_exit_inline ─────────────────────────────────────────────────────
