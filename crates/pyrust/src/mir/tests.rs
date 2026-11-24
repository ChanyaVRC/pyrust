//! Unit tests for the SSA MIR representation, verifier, printer, lowering,
//! and all 8 SSA MIR optimization passes (issue #457).

use crate::ast::{BinaryOp, UnaryOp};
use crate::interpreter::Interpreter;
use crate::value::{PyBigInt, Value, ValueKind};

use super::ir::{
    BlockId, MirBlock, MirFn, MirInst, MirOp, MirType, Terminator, Val, infer_mir_types,
};
use super::lower_ast::{compile_source_to_mir, lower_ast_to_mir};
use super::lower_bytecode::lower_mir_to_fncode;
use super::passes::{
    mir_algebraic_simplify, mir_const_branch_elim, mir_const_fold, mir_copy_prop, mir_cse, mir_dce,
    mir_reassociate, mir_sccp, run_mir_passes,
};
use super::print::format_mir_fn;
use super::ssa::lower_fncode_to_mir;
use super::verify::verify_mir;

fn lower_source_fn(src: &str, fn_name: &str) -> MirFn {
    let (stmts, linenos) = Interpreter::parse_source_to_stmts_with_linenos(src).unwrap();
    let module_mir = lower_ast_to_mir(&stmts, &linenos, "<test>").unwrap();
    verify_mir(&module_mir).unwrap();
    for proto in &module_mir.fn_protos {
        if proto.name.as_ref() == fn_name {
            let fn_mir = lower_fncode_to_mir(&proto.code);
            verify_mir(&fn_mir).unwrap();
            return fn_mir;
        }
    }
    panic!("function '{fn_name}' not found in compiled module");
}

fn inst(def: Option<Val>, op: MirOp) -> MirInst {
    MirInst {
        def,
        op,
        lineno: 1,
        col_span: (0, 0, 0, 0),
    }
}

// ── SSA construction & verifier across all Python control-flow shapes ────────

#[test]
fn ssa_construction_and_verify_across_control_flow_constructs() {
    let cases = [
        ("straight", "def f(x):\n    y = x + 1\n    return y * 2\n"),
        (
            "diamond",
            "def f(c):\n    if c:\n        x = 10\n    else:\n        x = 20\n    return x + 1\n",
        ),
        (
            "while_swap_cycle",
            "def f(n):\n    a = 0\n    b = 1\n    i = 0\n    while i < n:\n        a, b = b, a + b\n        i += 1\n    return a\n",
        ),
        (
            "for_loop",
            "def f(xs):\n    s = 0\n    for x in xs:\n        s += x\n    return s\n",
        ),
        (
            "nested_try_except_finally",
            "def f(x):\n    out = 0\n    try:\n        try:\n            out = 10 // x\n        except ZeroDivisionError:\n            out = -1\n        finally:\n            out += 1\n    except Exception:\n        out = -99\n    return out\n",
        ),
        (
            "match_stmt",
            "def f(x):\n    match x:\n        case 0:\n            return 'zero'\n        case [a, b]:\n            return a + b\n        case _:\n            return 'other'\n",
        ),
        (
            "generator",
            "def f(n):\n    i = 0\n    while i < n:\n        yield i * i\n        i += 1\n",
        ),
        (
            "closure",
            "def f(x):\n    def g(y):\n        nonlocal x\n        x += y\n        return x\n    return g(5)\n",
        ),
    ];

    for (label, src) in cases {
        let mut mir = lower_source_fn(src, "f");
        verify_mir(&mir).unwrap_or_else(|e| panic!("{label} failed pre-pass verify: {e}"));
        run_mir_passes(&mut mir);
        verify_mir(&mir).unwrap_or_else(|e| panic!("{label} failed post-pass verify: {e}"));
        let code = lower_mir_to_fncode(&mir);
        assert!(!code.insns.is_empty(), "{label} lowered to empty bytecode");
    }
}

#[test]
fn verify_mir_catches_double_def_and_dominance_violation() {
    // 1. Double definition of v0.
    let mut f = MirFn::empty_for_test("double_def", 0);
    let v0 = f.fresh_val();
    f.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![
            inst(Some(v0), MirOp::Const(Value::int(1))),
            inst(Some(v0), MirOp::Const(Value::int(2))),
        ],
        term: Terminator::Return(v0),
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    assert!(verify_mir(&f).is_err(), "double def must fail verify_mir");

    // 2. Use before definition in same block.
    let mut g = MirFn::empty_for_test("use_before_def", 0);
    let a = g.fresh_val();
    let b = g.fresh_val();
    g.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![
            inst(Some(a), MirOp::Copy(b)),
            inst(Some(b), MirOp::Const(Value::int(5))),
        ],
        term: Terminator::Return(a),
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    assert!(
        verify_mir(&g).is_err(),
        "use before def must fail verify_mir"
    );

    // 3. Mismatched edge argument count.
    let mut h = MirFn::empty_for_test("bad_arity", 0);
    let p = h.fresh_val();
    h.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: Vec::new(),
        term: Terminator::Jump {
            target: BlockId(1),
            args: Vec::new(),
        },
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    h.blocks.push(MirBlock {
        id: BlockId(1),
        params: vec![p],
        instrs: Vec::new(),
        term: Terminator::Return(p),
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(0)],
        exc_depth: 0,
    });
    assert!(
        verify_mir(&h).is_err(),
        "edge arg arity mismatch must fail verify_mir"
    );
}

// ── Pass 1: mir_sccp ─────────────────────────────────────────────────────────

#[test]
fn pass_mir_sccp_folds_diamond_and_prunes_dead_arm() {
    let mut f = MirFn::empty_for_test("sccp_diamond", 0);
    let cond = f.fresh_val();
    let v_then = f.fresh_val();
    let v_else = f.fresh_val();
    let v_phi = f.fresh_val();
    let v_five = f.fresh_val();
    let v_sum = f.fresh_val();

    f.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![inst(Some(cond), MirOp::Const(Value::bool_(true)))],
        term: Terminator::Branch {
            cond,
            then_bb: BlockId(1),
            then_args: Vec::new(),
            else_bb: BlockId(2),
            else_args: Vec::new(),
        },
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(1),
        params: Vec::new(),
        instrs: vec![inst(Some(v_then), MirOp::Const(Value::int(10)))],
        term: Terminator::Jump {
            target: BlockId(3),
            args: vec![v_then],
        },
        term_lineno: 2,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(0)],
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(2),
        params: Vec::new(),
        instrs: vec![inst(Some(v_else), MirOp::Const(Value::int(999)))],
        term: Terminator::Jump {
            target: BlockId(3),
            args: vec![v_else],
        },
        term_lineno: 3,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(0)],
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(3),
        params: vec![v_phi],
        instrs: vec![
            inst(Some(v_five), MirOp::Const(Value::int(5))),
            inst(
                Some(v_sum),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: v_phi,
                    rhs: v_five,
                    is_inplace: false,
                },
            ),
        ],
        term: Terminator::Return(v_sum),
        term_lineno: 4,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(1), BlockId(2)],
        exc_depth: 0,
    });

    verify_mir(&f).unwrap();
    mir_sccp(&mut f);
    verify_mir(&f).unwrap();

    // Unreachable else arm (999) was pruned (3 blocks remain, not 4).
    assert_eq!(f.blocks.len(), 3, "dead else block should be pruned");
    // `v_sum` folded to exact `15`.
    let ret_val = match f.blocks.last().unwrap().term {
        Terminator::Return(v) => v,
        _ => panic!("expected Return"),
    };
    assert_eq!(
        f.val_type(ret_val).as_const(),
        Some(&Value::int(15)),
        "SCCP should fold v_phi + 5 to 15"
    );
}

// ── Pass 2: mir_const_fold (#421 overflow-safe BigInt promotion) ─────────────

#[test]
fn pass_mir_const_fold_promotes_i64_overflow_to_bigint_421() {
    let mut f = MirFn::empty_for_test("overflow_421", 0);
    let a = f.fresh_val();
    let b = f.fresh_val();
    let c = f.fresh_val();
    let half = 1i64 << 62; // 2**62; 2**62 + 2**62 = 2**63 overflows i64!
    f.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![
            inst(Some(a), MirOp::Const(Value::int(half))),
            inst(Some(b), MirOp::Const(Value::int(half))),
            inst(
                Some(c),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: a,
                    rhs: b,
                    is_inplace: false,
                },
            ),
        ],
        term: Terminator::Return(c),
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });

    verify_mir(&f).unwrap();
    mir_const_fold(&mut f);
    verify_mir(&f).unwrap();

    let expected = PyBigInt::from(half) + PyBigInt::from(half);
    match &f.blocks[0].instrs[2].op {
        MirOp::Const(v) => match v.kind() {
            ValueKind::BigInt(bi) => assert_eq!(bi, &expected),
            _ => panic!("expected BigInt promotion for 2**62 + 2**62, got {v:?}"),
        },
        other => panic!("expected folded Const, got {other:?}"),
    }
}

// ── Pass 3: mir_algebraic_simplify (#438 type-proven safety) ─────────────────

#[test]
fn pass_mir_algebraic_simplify_respects_type_provenance_438() {
    // 1. Unknown-type operand `obj + 0` MUST NOT be simplified (#438).
    let mut unknown_fn = MirFn::empty_for_test("unknown_add_zero", 1);
    let obj = unknown_fn.fresh_val();
    let zero = unknown_fn.fresh_val();
    let res = unknown_fn.fresh_val();
    unknown_fn.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![
            inst(Some(obj), MirOp::LoadLocal(0)),
            inst(Some(zero), MirOp::Const(Value::int(0))),
            inst(
                Some(res),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: obj,
                    rhs: zero,
                    is_inplace: false,
                },
            ),
        ],
        term: Terminator::Return(res),
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    verify_mir(&unknown_fn).unwrap();
    mir_algebraic_simplify(&mut unknown_fn);
    verify_mir(&unknown_fn).unwrap();
    assert!(
        matches!(unknown_fn.blocks[0].instrs[2].op, MirOp::BinOp { .. }),
        "unknown-type obj + 0 must NOT be simplified (#438)"
    );

    // 2. Proven-Int operand `(a + 1) + 0` (where `a` is proven Int via a loop
    //    phi initialized to `0`) MUST simplify to `Copy`.
    let mut int_fn = MirFn::empty_for_test("proven_int_add_zero", 1);
    let v_init = int_fn.fresh_val();
    let v_phi = int_fn.fresh_val();
    let v_one = int_fn.fresh_val();
    let v_inc = int_fn.fresh_val();
    let v_zero = int_fn.fresh_val();
    let v_plus_zero = int_fn.fresh_val();
    let v_cond = int_fn.fresh_val();

    int_fn.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![inst(Some(v_init), MirOp::Const(Value::int(0)))],
        term: Terminator::Jump {
            target: BlockId(1),
            args: vec![v_init],
        },
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    int_fn.blocks.push(MirBlock {
        id: BlockId(1),
        params: vec![v_phi],
        instrs: vec![
            inst(Some(v_one), MirOp::Const(Value::int(1))),
            inst(
                Some(v_inc),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: v_phi,
                    rhs: v_one,
                    is_inplace: false,
                },
            ),
            inst(Some(v_zero), MirOp::Const(Value::int(0))),
            inst(
                Some(v_plus_zero),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: v_inc,
                    rhs: v_zero,
                    is_inplace: false,
                },
            ),
            inst(Some(v_cond), MirOp::LoadLocal(0)),
        ],
        term: Terminator::Branch {
            cond: v_cond,
            then_bb: BlockId(1),
            then_args: vec![v_plus_zero],
            else_bb: BlockId(2),
            else_args: Vec::new(),
        },
        term_lineno: 2,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(0), BlockId(1)],
        exc_depth: 0,
    });
    int_fn.blocks.push(MirBlock {
        id: BlockId(2),
        params: Vec::new(),
        instrs: Vec::new(),
        term: Terminator::Return(v_plus_zero),
        term_lineno: 3,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(1)],
        exc_depth: 0,
    });
    verify_mir(&int_fn).unwrap();
    mir_algebraic_simplify(&mut int_fn);
    verify_mir(&int_fn).unwrap();
    assert!(
        matches!(int_fn.blocks[1].instrs[3].op, MirOp::Copy(src) if src == v_inc),
        "proven-Int (a + 1) + 0 must simplify to Copy(v_inc), got {:?}",
        int_fn.blocks[1].instrs[3].op
    );
}

// ── Pass 4: mir_copy_prop ────────────────────────────────────────────────────

#[test]
fn pass_mir_copy_prop_eliminates_copies_and_trivial_phi() {
    let mut f = MirFn::empty_for_test("copy_prop", 1);
    let x = f.fresh_val();
    let c = f.fresh_val();
    let phi = f.fresh_val();
    let cond = f.fresh_val();

    f.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![
            inst(Some(x), MirOp::LoadLocal(0)),
            inst(Some(c), MirOp::Copy(x)),
        ],
        term: Terminator::Jump {
            target: BlockId(1),
            args: vec![c],
        },
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(1),
        params: vec![phi],
        instrs: vec![inst(Some(cond), MirOp::LoadLocal(0))],
        term: Terminator::Branch {
            cond,
            then_bb: BlockId(1),
            then_args: vec![phi], // self-referential trivial phi!
            else_bb: BlockId(2),
            else_args: Vec::new(),
        },
        term_lineno: 2,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(0), BlockId(1)],
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(2),
        params: Vec::new(),
        instrs: Vec::new(),
        term: Terminator::Return(phi),
        term_lineno: 3,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(1)],
        exc_depth: 0,
    });

    verify_mir(&f).unwrap();
    mir_copy_prop(&mut f);
    verify_mir(&f).unwrap();

    assert!(
        f.blocks[1].params.is_empty(),
        "trivial phi should be removed"
    );
    assert!(matches!(f.blocks[2].term, Terminator::Return(v) if v == x));
    assert!(
        !f.blocks[0]
            .instrs
            .iter()
            .any(|i| matches!(i.op, MirOp::Copy(_))),
        "Copy instruction should be eliminated"
    );
}

// ── Pass 5: mir_cse ──────────────────────────────────────────────────────────

#[test]
fn pass_mir_cse_deduplicates_proven_int_expr_across_dominated_block() {
    let mut f = MirFn::empty_for_test("cse_dom", 1);
    let v0 = f.fresh_val();
    let phi = f.fresh_val();
    let one = f.fresh_val();
    let e1 = f.fresh_val();
    let e2 = f.fresh_val();
    let sum = f.fresh_val();
    let cond = f.fresh_val();

    f.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![inst(Some(v0), MirOp::Const(Value::int(10)))],
        term: Terminator::Jump {
            target: BlockId(1),
            args: vec![v0],
        },
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(1),
        params: vec![phi],
        instrs: vec![
            inst(Some(one), MirOp::Const(Value::int(1))),
            inst(
                Some(e1),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: phi,
                    rhs: one,
                    is_inplace: false,
                },
            ),
            inst(Some(cond), MirOp::LoadLocal(0)),
        ],
        term: Terminator::Branch {
            cond,
            then_bb: BlockId(1),
            then_args: vec![e1],
            else_bb: BlockId(2),
            else_args: Vec::new(),
        },
        term_lineno: 2,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(0), BlockId(1)],
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(2),
        params: Vec::new(),
        instrs: vec![
            // Commuted identical expression `1 + phi` in dominated block!
            inst(
                Some(e2),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: one,
                    rhs: phi,
                    is_inplace: false,
                },
            ),
            inst(
                Some(sum),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: e1,
                    rhs: e2,
                    is_inplace: false,
                },
            ),
        ],
        term: Terminator::Return(sum),
        term_lineno: 3,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(1)],
        exc_depth: 0,
    });

    verify_mir(&f).unwrap();
    mir_cse(&mut f);
    verify_mir(&f).unwrap();
    assert!(
        matches!(f.blocks[2].instrs[0].op, MirOp::Copy(v) if v == e1),
        "commuted duplicate `1 + phi` should CSE to Copy(e1)"
    );
}

// ── Pass 6: mir_const_branch_elim ────────────────────────────────────────────

#[test]
fn pass_mir_const_branch_elim_turns_const_branch_into_jump() {
    let mut f = MirFn::empty_for_test("branch_elim", 0);
    let c = f.fresh_val();
    let v1 = f.fresh_val();
    let v2 = f.fresh_val();

    f.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![inst(Some(c), MirOp::Const(Value::int(0)))],
        term: Terminator::Branch {
            cond: c,
            then_bb: BlockId(1),
            then_args: Vec::new(),
            else_bb: BlockId(2),
            else_args: Vec::new(),
        },
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(1),
        params: Vec::new(),
        instrs: vec![inst(Some(v1), MirOp::Const(Value::int(111)))],
        term: Terminator::Return(v1),
        term_lineno: 2,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(0)],
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(2),
        params: Vec::new(),
        instrs: vec![inst(Some(v2), MirOp::Const(Value::int(222)))],
        term: Terminator::Return(v2),
        term_lineno: 3,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(0)],
        exc_depth: 0,
    });

    verify_mir(&f).unwrap();
    mir_const_branch_elim(&mut f);
    verify_mir(&f).unwrap();
    assert_eq!(f.blocks.len(), 2, "dead then-block must be pruned");
    assert!(matches!(f.blocks[0].term, Terminator::Jump { .. }));
}

// ── Pass 7: mir_dce ──────────────────────────────────────────────────────────

#[test]
fn pass_mir_dce_removes_pure_dead_chain_and_keeps_raising_ops() {
    let mut f = MirFn::empty_for_test("dce", 1);
    let a = f.fresh_val();
    let b = f.fresh_val();
    let dead_sum = f.fresh_val();
    let dead_neg = f.fresh_val();
    let unk = f.fresh_val();
    let div_zero = f.fresh_val();
    let ret = f.fresh_val();

    f.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![
            inst(Some(a), MirOp::Const(Value::int(1))),
            inst(Some(b), MirOp::Const(Value::int(2))),
            inst(
                Some(dead_sum),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: a,
                    rhs: b,
                    is_inplace: false,
                },
            ),
            inst(
                Some(dead_neg),
                MirOp::UnaryOp {
                    op: UnaryOp::Neg,
                    arg: dead_sum,
                },
            ),
            inst(Some(unk), MirOp::LoadLocal(0)),
            // Potentially-raising op must NOT be deleted by DCE!
            inst(
                Some(div_zero),
                MirOp::BinOp {
                    op: BinaryOp::Div,
                    lhs: unk,
                    rhs: a,
                    is_inplace: false,
                },
            ),
            inst(Some(ret), MirOp::Const(Value::int(42))),
        ],
        term: Terminator::Return(ret),
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });

    verify_mir(&f).unwrap();
    mir_dce(&mut f);
    verify_mir(&f).unwrap();

    // `dead_sum`, `dead_neg`, and `b` must be eliminated; `unk`, `div_zero`, `a`, `ret` remain.
    let remaining_defs: Vec<Val> = f.blocks[0].instrs.iter().filter_map(|i| i.def).collect();
    assert!(!remaining_defs.contains(&dead_sum));
    assert!(!remaining_defs.contains(&dead_neg));
    assert!(!remaining_defs.contains(&b));
    assert!(remaining_defs.contains(&div_zero));
    assert!(remaining_defs.contains(&ret));
}

// ── Pass 8: mir_reassociate (parked #357) ────────────────────────────────────

#[test]
fn pass_mir_reassociate_folds_proven_int_chains_and_skips_unknown_357() {
    // 1. Proven-Int `(x + 1) + 2` -> `x + 3`.
    let mut f = MirFn::empty_for_test("reassoc_int", 1);
    let init = f.fresh_val();
    let x = f.fresh_val();
    let c1 = f.fresh_val();
    let t1 = f.fresh_val();
    let c2 = f.fresh_val();
    let t2 = f.fresh_val();
    let cond = f.fresh_val();

    f.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![inst(Some(init), MirOp::Const(Value::int(0)))],
        term: Terminator::Jump {
            target: BlockId(1),
            args: vec![init],
        },
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(1),
        params: vec![x],
        instrs: vec![
            inst(Some(c1), MirOp::Const(Value::int(1))),
            inst(
                Some(t1),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: x,
                    rhs: c1,
                    is_inplace: false,
                },
            ),
            inst(Some(c2), MirOp::Const(Value::int(2))),
            inst(
                Some(t2),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: t1,
                    rhs: c2,
                    is_inplace: false,
                },
            ),
            inst(Some(cond), MirOp::LoadLocal(0)),
        ],
        term: Terminator::Branch {
            cond,
            then_bb: BlockId(1),
            then_args: vec![t2],
            else_bb: BlockId(2),
            else_args: Vec::new(),
        },
        term_lineno: 2,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(0), BlockId(1)],
        exc_depth: 0,
    });
    f.blocks.push(MirBlock {
        id: BlockId(2),
        params: Vec::new(),
        instrs: Vec::new(),
        term: Terminator::Return(t2),
        term_lineno: 3,
        term_col_span: (0, 0, 0, 0),
        preds: vec![BlockId(1)],
        exc_depth: 0,
    });

    verify_mir(&f).unwrap();
    mir_reassociate(&mut f);
    mir_dce(&mut f);
    verify_mir(&f).unwrap();

    // `t2` must now be `add x, const 3` directly (and `t1` removed by DCE).
    let t2_inst = f.blocks[1]
        .instrs
        .iter()
        .find(|i| i.def == Some(t2))
        .expect("t2 must remain");
    match t2_inst.op {
        MirOp::BinOp {
            op: BinaryOp::Add,
            lhs,
            rhs,
            is_inplace: false,
        } => {
            assert_eq!(lhs, x);
            assert_eq!(f.val_type(rhs).as_const(), Some(&Value::int(3)));
        }
        ref other => panic!("expected reassociated `add x, 3`, got {other:?}"),
    }

    // 2. Unknown-type `(obj + 1) + 2` MUST NOT be reassociated!
    let mut g = MirFn::empty_for_test("reassoc_unknown", 1);
    let obj = g.fresh_val();
    let u1 = g.fresh_val();
    let ut1 = g.fresh_val();
    let u2 = g.fresh_val();
    let ut2 = g.fresh_val();
    g.blocks.push(MirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: vec![
            inst(Some(obj), MirOp::LoadLocal(0)),
            inst(Some(u1), MirOp::Const(Value::int(1))),
            inst(
                Some(ut1),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: obj,
                    rhs: u1,
                    is_inplace: false,
                },
            ),
            inst(Some(u2), MirOp::Const(Value::int(2))),
            inst(
                Some(ut2),
                MirOp::BinOp {
                    op: BinaryOp::Add,
                    lhs: ut1,
                    rhs: u2,
                    is_inplace: false,
                },
            ),
        ],
        term: Terminator::Return(ut2),
        term_lineno: 1,
        term_col_span: (0, 0, 0, 0),
        preds: Vec::new(),
        exc_depth: 0,
    });
    verify_mir(&g).unwrap();
    infer_mir_types(&mut g);
    assert_eq!(g.val_type(obj), &MirType::Unknown);
    mir_reassociate(&mut g);
    verify_mir(&g).unwrap();
    assert!(
        matches!(
            g.blocks[0].instrs[4].op,
            MirOp::BinOp {
                op: BinaryOp::Add,
                lhs,
                rhs,
                ..
            } if lhs == ut1 && rhs == u2
        ),
        "unknown-type (obj + 1) + 2 must NOT be reassociated"
    );
}

// ── Pretty-printer & `--emit mir` helper ─────────────────────────────────────

#[test]
fn emit_mir_prints_readable_ssa_for_module_and_nested_functions() {
    let src = "\
def add_one(x):
    return x + 1

def fib(n):
    a = 0
    b = 1
    i = 0
    while i < n:
        a, b = b, a + b
        i += 1
    return a
";
    let text = compile_source_to_mir(src).unwrap();
    assert!(
        text.contains("fn <module>:"),
        "missing module header:\n{text}"
    );
    assert!(text.contains("fn add_one:"), "missing add_one:\n{text}");
    assert!(text.contains("fn fib:"), "missing fib:\n{text}");
    assert!(text.contains("bb0:"), "missing bb0:\n{text}");
    assert!(
        text.contains("add "),
        "missing add op in MIR output:\n{text}"
    );
    // Verify `fib` has a loop header block with SSA block parameters!
    let fib_section = text.split("fn fib:").nth(1).unwrap();
    assert!(
        fib_section.contains("bb") && fib_section.contains("(v"),
        "fib loop header should have Cranelift-style block parameters:\n{fib_section}"
    );
    let mir_fib = lower_source_fn(src, "fib");
    let formatted = format_mir_fn(&mir_fib);
    assert!(formatted.starts_with("fn fib:"));
}
