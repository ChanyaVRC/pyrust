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

// ── linearized-pass equivalence (issue #2004) ──────────────────────────────

/// Reference dead-store elimination that uses the original O(n) tail-scan
/// (`reg_is_read_before_next_write`).  Used to confirm the linearized
/// `pass_dead_store_elim` produces byte-identical output.
fn dead_store_elim_reference(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut keep = vec![true; n];
    let mut global_read_count: HashMap<u32, usize> = HashMap::new();
    let mut reads_buf: HashSet<u32> = HashSet::new();
    for insn in &insns {
        reads_buf.clear();
        collect_reads(insn, &mut reads_buf);
        for &r in &reads_buf {
            if r >= num_locals {
                *global_read_count.entry(r).or_insert(0) += 1;
            }
        }
    }
    for i in 0..n {
        let dst = match &insns[i] {
            Insn::LoadConst(r, _) | Insn::LoadNone(r) | Insn::Move(r, _) | Insn::CopyReg(r, _)
                if *r >= num_locals =>
            {
                *r
            }
            // `CallMemo` remains observable even when its result is dead, so
            // dead-store elimination never drops it.
            _ => continue,
        };
        if global_read_count.get(&dst).copied().unwrap_or(0) == 0 {
            keep[i] = false;
            continue;
        }
        if slice_has_back_edge(&insns[i + 1..]) {
            continue;
        }
        if !reg_is_read_before_next_write(&insns[i + 1..], dst) {
            keep[i] = false;
        }
    }
    compact(insns, &keep)
}

/// Sample instruction streams that exercise dead-store elimination: loads,
/// moves, copies, fused ops, reads, and control flow (terminators and
/// back-edges).
fn sample_dse_streams() -> Vec<Vec<Insn>> {
    use crate::ast::{BinaryOp, UnaryOp};
    vec![
        // Long single block of distinct LoadConsts feeding one BuildList
        // (the #2004 literal shape).
        {
            let mut v: Vec<Insn> = (0..40u16)
                .map(|i| Insn::LoadConst(2 + i as u32, i))
                .collect();
            v.push(Insn::BuildList(2, 2, 40));
            v.push(Insn::Return(2));
            v
        },
        // Move chain with a read in the middle.
        vec![
            Insn::LoadConst(2, 0),
            Insn::Move(3, 2),
            Insn::UnaryOp(4, UnaryOp::Neg, 3),
            Insn::Move(3, 4),
            Insn::Return(3),
        ],
        // Read-then-write of the same reg in one instruction (BinOp(r,r,..)).
        vec![
            Insn::LoadConst(2, 0),
            Insn::BinOpConst(2, 2, BinaryOp::Add, 1, false),
            Insn::Return(2),
        ],
        // Store with a terminator before any read.
        vec![Insn::LoadConst(2, 0), Insn::ReturnNone],
        // Store, control-flow (forward jump), then read.
        vec![Insn::LoadConst(2, 0), Insn::Jump(0), Insn::Return(2)],
        // Back-edge after a store (loop-carried).
        vec![
            Insn::LoadConst(2, 0),
            Insn::BinOpConst(2, 2, BinaryOp::Add, 1, false),
            Insn::Jump(-2),
        ],
        // CopyReg dead store (never read).
        vec![Insn::LoadConst(2, 0), Insn::CopyReg(3, 2), Insn::Return(2)],
        // Duplicate LoadConst with intervening write.
        vec![
            Insn::LoadConst(2, 0),
            Insn::Move(2, 3),
            Insn::LoadConst(4, 0),
            Insn::Return(4),
        ],
        // Duplicate fused op separated by an unrelated write.
        vec![
            Insn::BinOpConst(2, 0, BinaryOp::Add, 1, false),
            Insn::LoadConst(5, 7),
            Insn::BinOpConst(3, 0, BinaryOp::Add, 1, false),
            Insn::Return(3),
        ],
        // Fused op whose source is overwritten between two occurrences.
        vec![
            Insn::BinOpConst(2, 0, BinaryOp::Add, 1, false),
            Insn::Move(0, 9),
            Insn::BinOpConst(3, 0, BinaryOp::Add, 1, false),
            Insn::Return(3),
        ],
    ]
}

#[test]
fn dead_store_elim_matches_reference() {
    for stream in sample_dse_streams() {
        for num_locals in [0u32, 2, 3] {
            let a = pass_dead_store_elim(stream.clone(), num_locals);
            let b = dead_store_elim_reference(stream.clone(), num_locals);
            assert_eq!(
                a, b,
                "dead_store_elim diverged from reference (num_locals={num_locals}) on {stream:?}"
            );
        }
    }
}

// ── pass_exit_inline ─────────────────────────────────────────────────────
