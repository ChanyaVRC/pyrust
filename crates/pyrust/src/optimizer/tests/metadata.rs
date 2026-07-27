use super::*;

#[test]
fn equal_valued_const_statements_keep_distinct_linenos() {
    use crate::ast::BinaryOp;
    // Two statements whose constant expressions fold to the SAME value.
    // `2 ** 1024` and `(2 ** 512) * (2 ** 512)` both fold to the same BigInt;
    // the optimizer dedups the constant-pool slot.  The `/ 1` divisions are
    // left as runtime BinOps (folding them would raise OverflowError).  The
    // surviving division for the FIRST statement must retain line 1, not
    // inherit the second statement's line — otherwise an exception raised on
    // line 1 is mis-attributed to line 2 (the bug: remap_linenos ran after
    // constant-pool compaction reindexed the LoadConst slots).
    let code = compile_script_with_linenos_for_test(
        "(2 ** 1024) / 1\n(2 ** 512) * (2 ** 512) / 1\n",
        &[1, 2],
    );
    let optimized = optimize(code);

    // Collect the line number of every surviving Div BinOp, in order.
    let div_linenos: Vec<u32> = optimized
        .insns
        .iter()
        .enumerate()
        .filter_map(|(i, ins)| match ins {
            Insn::BinOp(_, _, BinaryOp::Div, _) | Insn::BinOpConst(_, _, BinaryOp::Div, _, _) => {
                Some(optimized.lineno_table.get(i).copied().unwrap_or(0))
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        div_linenos,
        vec![1, 2],
        "each statement's division must keep its own source line; insns: {:?}, linenos: {:?}",
        optimized.insns,
        optimized.lineno_table
    );
}

// ── issue #2002: linear compile on long single-variable def-use chains ──────

/// Brute-force reference for `remap_linenos`'s greedy forward scan.  The
/// indexed implementation must produce byte-identical output.
fn remap_linenos_reference(
    old_insns: &[Insn],
    old_linenos: &[u32],
    new_insns: &[Insn],
) -> Vec<u32> {
    if old_linenos.is_empty() {
        return vec![0u32; new_insns.len()];
    }
    let mut running = 0u32;
    let old_prefix: Vec<u32> = old_linenos
        .iter()
        .map(|&ln| {
            if ln != 0 {
                running = ln;
            }
            running
        })
        .collect();
    let mut old_pos = 0usize;
    let mut result = Vec::with_capacity(new_insns.len());
    'outer: for new_insn in new_insns {
        // `old_pos` advances across outer iterations; the inner range is
        // evaluated once per outer pass and the mutation only takes effect
        // on the next pass, which is the intended scan behaviour.
        #[allow(clippy::mut_range_bound)]
        for i in old_pos..old_insns.len() {
            if &old_insns[i] == new_insn {
                result.push(old_linenos.get(i).copied().unwrap_or(0));
                old_pos = i + 1;
                continue 'outer;
            }
        }
        result.push(old_prefix.get(old_pos).copied().unwrap_or(0));
    }
    result
}

#[test]
fn remap_linenos_indexed_matches_reference() {
    use crate::ast::BinaryOp;
    // A stream with duplicate instructions, optimizer-created instructions
    // that never match, and reordered survivors — the cases where the greedy
    // cursor behaviour matters.
    let old = vec![
        Insn::LoadConst(2, 0),
        Insn::BinOp(0, 0, BinaryOp::Add, 2),
        Insn::LoadConst(2, 0),
        Insn::BinOp(0, 0, BinaryOp::Add, 2),
        Insn::LoadConst(2, 0),
        Insn::Return(0),
    ];
    let old_ln = vec![1, 1, 2, 2, 3, 3];
    let candidates = vec![
        Insn::LoadConst(2, 0),               // matches first occurrence
        Insn::BinOp(0, 0, BinaryOp::Add, 2), // matches
        Insn::LoadConst(9, 5),               // never matches (optimizer-made)
        Insn::LoadConst(2, 0),               // matches the next occurrence
        Insn::Return(0),                     // matches
    ];
    let got = remap_linenos(&old, &old_ln, &candidates);
    let want = remap_linenos_reference(&old, &old_ln, &candidates);
    assert_eq!(got, want);
}

#[test]
fn const_fold_long_chain_folds_to_final_value() {
    // A function-local `x = x + 1 + 2 + ...` straight-line chain must collapse
    // every step and leave the correct final constant in the pool. Keep this
    // inside a function: module-scope stores cross `SyncModuleGlobal`, which is
    // deliberately a named-local fact barrier because the live module
    // namespace can be observed or updated re-entrantly.
    let mut src = String::from("def folded():\n    x = 0\n");
    for i in 1..=20 {
        src.push_str(&format!("    x = x + {i}\n"));
    }
    src.push_str("    return x\n");
    let code = compile_fn(&src);
    let optimized = optimize(code);
    let inner = &optimized.fn_protos[0].code;
    // sum(1..=20) == 210 must appear as a folded Int constant.
    assert!(
        inner
            .consts
            .iter()
            .any(|v| matches!(v.kind(), ValueKind::Int(210))),
        "folded chain must produce the constant 210; consts: {:?}",
        inner.consts
    );
}

#[test]
fn const_index_intern_matches_linear_scan() {
    // The hash-indexed interner must return the same slots as the original
    // linear scan (first-occurrence-wins dedup), including for Bool vs Int
    // (which must never collide) and float bit-equality.
    let mut a = vec![Value::int(1), Value::bool_(true), Value::float(2.0)];
    let mut b = a.clone();
    let mut idx = ConstIndex::build(&a);
    let vals = [
        Value::int(1),       // dedup → 0
        Value::bool_(true),  // dedup → 1 (not Int(1))
        Value::int(5),       // new → 3
        Value::float(2.0),   // dedup → 2
        Value::bool_(false), // new → 4
        Value::int(5),       // dedup → 3
    ];
    for v in vals {
        let indexed = idx.intern(&mut a, v.clone());
        let linear = intern_const_in_pool(&mut b, v);
        assert_eq!(indexed, linear, "indexed vs linear intern diverged");
    }
    assert_eq!(a.len(), b.len());
}

#[test]
fn build_exc_table_no_handlers_is_identity() {
    let insns = vec![Insn::LoadConst(0, 0), Insn::Return(0)];
    let (out, table) = build_exc_table(insns.clone());
    assert_eq!(out, insns, "stream unchanged when no SetupExcept present");
    assert_eq!(table, vec![EXC_NO_HANDLER, EXC_NO_HANDLER]);
    assert!(!has_exception_handlers(&out, &table));
}

#[test]
fn build_exc_table_bail_still_reports_dynamic_handlers() {
    // PC 3 has conflicting incoming handler stacks, forcing the zero-cost
    // analysis to retain the dynamic SetupExcept/PopExcept stream.
    let insns = vec![
        Insn::SetupExcept(4),
        Insn::JumpIfFalse(0, 1),
        Insn::PopExcept,
        Insn::LoadConst(1, 0),
        Insn::Return(1),
        Insn::LoadConst(2, 1),
        Insn::Return(2),
    ];
    let (out, table) = build_exc_table(insns.clone());

    assert_eq!(out, insns, "bail must retain the dynamic handler stream");
    assert!(table.is_empty(), "bail is represented by an empty table");
    assert!(
        has_exception_handlers(&out, &table),
        "fallback SetupExcept code must not be classified as handler-free"
    );
}

#[test]
fn build_exc_table_strips_and_maps_single_try() {
    // Layout (offsets are relative, +1 of the source counting in jump_pc):
    //   0 SetupExcept(+1)   -> handler at pc 3 (the LoadConst below)
    //   1 RaiseValue(0)     try body, raises
    //   2 PopExcept         normal exit (jumped over on raise)
    //   3 LoadConst(0,0)    handler entry
    //   4 Return(0)
    // SetupExcept's absolute target = 0 + 1 + 1 = 2; but pc 2 is PopExcept,
    // which is stripped, so compact redirects the handler to pc 3 → new pc 1.
    let insns = vec![
        Insn::SetupExcept(1),
        Insn::RaiseValue(0),
        Insn::PopExcept,
        Insn::LoadConst(0, 0),
        Insn::Return(0),
    ];
    let (out, table) = build_exc_table(insns);
    // Two instructions (SetupExcept + PopExcept) removed.
    assert_eq!(out.len(), 3);
    assert!(
        !out.iter()
            .any(|i| matches!(i, Insn::SetupExcept(_) | Insn::PopExcept)),
        "block-setup instructions must be stripped"
    );
    // New stream: [RaiseValue(0), LoadConst(0,0), Return(0)].
    // RaiseValue is now at new pc 0 and must dispatch to the handler at new
    // pc 1 (the LoadConst).
    assert!(matches!(out[0], Insn::RaiseValue(0)));
    assert_eq!(table[0], 1, "raise inside try → handler pc 1");
    // The handler body and code after it are not protected.
    assert_eq!(table[1], EXC_NO_HANDLER);
    assert_eq!(table[2], EXC_NO_HANDLER);
}

#[test]
fn build_exc_table_real_compiled_try_is_stripped_and_consistent() {
    // A real compiled try/except: after the optimizer runs (which includes
    // build_exc_table), the FnCode must have no SetupExcept/PopExcept left
    // and a non-empty exc_table whose every protected entry points at a
    // valid in-range handler pc.
    let code = optimize(compile_fn(
        "def f(x):\n    try:\n        return x.attr\n    except AttributeError:\n        return -1\n",
    ));
    // The optimized top-level code holds f as a nested proto; check the proto.
    let proto = &code.fn_protos[0].code;
    assert!(
        !proto
            .insns
            .iter()
            .any(|i| matches!(i, Insn::SetupExcept(_) | Insn::PopExcept)),
        "block-setup instructions must be stripped from optimized try/except"
    );
    assert_eq!(
        proto.exc_table.len(),
        proto.insns.len(),
        "exc_table is parallel to insns"
    );
    // At least one instruction is protected, and every handler target is a
    // valid pc within the (post-strip) stream.
    let protected = proto
        .exc_table
        .iter()
        .filter(|&&t| t != EXC_NO_HANDLER)
        .count();
    assert!(protected > 0, "the try body must be covered by a handler");
    for &t in &proto.exc_table {
        if t != EXC_NO_HANDLER {
            assert!(
                (t as usize) < proto.insns.len(),
                "handler target {t} out of range (len {})",
                proto.insns.len()
            );
        }
    }
}

#[test]
fn nested_finally_cross_jump_keeps_zero_cost_exception_table() {
    // The two finally exception paths both end in structurally identical
    // [Call(print), RaiseReRaise] tails.  The inner copy still has the outer
    // finally handler active while the outer copy has no handler.  Cross-jump
    // must keep those copies distinct; otherwise build_exc_table sees two
    // handler stacks at the shared Call PC and falls back to dynamic handlers.
    let code = optimize(compile_fn(
        r#"def nested_finally():
    try:
        try:
            raise ValueError("v")
        except ValueError:
            raise
        finally:
            print("inner")
    finally:
        print("outer")
"#,
    ));
    let proto = &code.fn_protos[0].code;

    assert!(
        !proto
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::SetupExcept(_) | Insn::PopExcept)),
        "consistent nested handlers must be stripped after cross-jump"
    );
    assert_eq!(
        proto.exc_table.len(),
        proto.insns.len(),
        "zero-cost table must remain parallel to the optimized stream"
    );
    assert!(
        proto
            .exc_table
            .iter()
            .any(|&target| target != EXC_NO_HANDLER),
        "nested finally must retain real protected PCs"
    );
    assert!(
        proto.has_exc_handlers,
        "nested finally must remain ineligible for handler-free trampolines"
    );
}
