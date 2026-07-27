use super::*;

use crate::ast::BinaryOp;
use crate::bytecode::EXC_NO_HANDLER;

fn jump_target(pc: usize, insn: &Insn) -> Option<usize> {
    let offset = match insn {
        Insn::Jump(offset)
        | Insn::JumpIfFalse(_, offset)
        | Insn::JumpIfTrue(_, offset)
        | Insn::JumpIfNotInt(_, offset)
        | Insn::JumpIfIterNotIntRange(_, offset)
        | Insn::CmpJumpIfFalse(_, _, _, offset)
        | Insn::CmpJumpIfTrue(_, _, _, offset)
        | Insn::CmpJumpIfFalseConst(_, _, _, offset)
        | Insn::CmpJumpIfTrueConst(_, _, _, offset)
        | Insn::CountCmpJumpTrue(_, _, _, _, offset)
        | Insn::CountCmpJumpFalse(_, _, _, _, offset)
        | Insn::ForIter(_, _, offset)
        | Insn::SetupExcept(offset)
        | Insn::MatchExcept(_, offset)
        | Insn::MatchExceptStar(_, _, _, offset) => *offset,
        Insn::CallInlineBinOp { skip, .. } => *skip,
        _ => return None,
    };
    Some((pc as i64 + 1 + i64::from(offset)) as usize)
}

fn versioned(input: Vec<Insn>, constants: &[Value]) -> IntLoopVersioningResult {
    let mut num_regs = 16;
    pass_int_loop_version(input, constants, &mut num_regs)
}

fn version(input: Vec<Insn>, constants: &[Value]) -> Vec<Insn> {
    versioned(input, constants).insns
}

#[test]
fn int_loop_version_emits_guarded_copy_and_fused_true_latch() {
    // Inverted straight-line module loop:
    //   while r0 < r1:
    //       r0 += 1
    //       SyncModuleGlobal(r0, "i")
    let input = vec![
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 3),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -3),
        Insn::Return(0),
    ];

    let output = version(input, &[]);

    assert_eq!(
        output
            .iter()
            .filter(|insn| matches!(insn, Insn::JumpIfNotInt(..)))
            .count(),
        2,
        "both register operands need entry guards: {output:?}"
    );
    assert!(
        output
            .iter()
            .any(|insn| matches!(insn, Insn::CountCmpJumpTrue(0, BinaryOp::Lt, 1, 1, _))),
        "the copied add + true latch must fuse: {output:?}"
    );
    for (pc, insn) in output.iter().enumerate() {
        if let Some(target) = jump_target(pc, insn) {
            assert!(
                target <= output.len(),
                "pc {pc} has out-of-range target {target}: {insn:?}; stream={output:?}"
            );
        }
    }
}

#[test]
fn int_loop_version_emits_fused_false_latch() {
    // `while True: if r0 >= r1: break; r0 += 1` after if-break fusion and
    // loop inversion uses a true header and false back-edge.
    let input = vec![
        Insn::CmpJumpIfTrue(0, BinaryOp::Ge, 1, 3),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::CmpJumpIfFalse(0, BinaryOp::Ge, 1, -3),
        Insn::Return(0),
    ];

    let output = version(input, &[]);

    assert!(
        output
            .iter()
            .any(|insn| matches!(insn, Insn::CountCmpJumpFalse(0, BinaryOp::Ge, 1, 1, _))),
        "the copied add + false latch must fuse: {output:?}"
    );
}

#[test]
fn int_loop_version_preserves_the_final_past_end_target() {
    // The candidate itself exits to old pc 4. The unrelated final Jump at old
    // pc 4 exits to old `n` and must still mean bytecode past-the-end after the
    // optimizer appends its out-of-line copy.
    let input = vec![
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 3),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -3),
        Insn::Jump(0),
    ];

    let output = version(input, &[]);
    // Two guards plus their fast-copy jump precede the five original
    // instructions, so old pc 4 is rebuilt at pc 7.
    assert_eq!(
        jump_target(7, &output[7]),
        Some(output.len()),
        "old past-the-end must not become the first appended copy: {output:?}"
    );
}

#[test]
fn int_loop_version_blocks_main_stream_fallthrough_into_fast_copies() {
    // Module frames commonly end by falling off the instruction stream. Once
    // a fast copy is appended, a barrier must preserve that implicit return.
    let input = vec![
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 3),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -3),
    ];

    let output = version(input, &[]);
    // guards(2) + dispatch jump(1) + original stream(4) = barrier pc 7.
    assert_eq!(
        jump_target(7, &output[7]),
        Some(output.len()),
        "main fallthrough barrier must skip every appended copy: {output:?}"
    );
}

#[test]
fn for_range_past_end_exhaustion_keeps_the_deferred_sync_stub() {
    // Module-level:
    //   for r0 in <machine-int range>:
    //       r1 += 1
    //
    // Both assignments have module syncs and the loop exhausts directly at
    // old past-the-end. The fast ForIter must target its sync stub, not be
    // rewritten to final_len by the generic old-past-end patch.
    let input = vec![
        Insn::ForIter(0, 0, 4),
        Insn::SyncModuleGlobal(0, 0),
        Insn::BinOpImm(1, 1, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(1, 1),
        Insn::Jump(-5),
    ];

    let versioned = versioned(input, &[]);
    let output = versioned.insns;
    let fast_for_pc = output
        .iter()
        .enumerate()
        .skip(versioned.source_prefix_len)
        .find_map(|(pc, insn)| matches!(insn, Insn::ForIter(..)).then_some(pc))
        .expect("the guarded for-range candidate must have a fast ForIter copy");
    let stub_pc = jump_target(fast_for_pc, &output[fast_for_pc]).unwrap();

    assert_ne!(
        stub_pc,
        output.len(),
        "fast exhaustion must not bypass deferred module-global syncs: {output:?}"
    );
    assert!(
        matches!(output[stub_pc], Insn::SyncModuleGlobal(0, 0)),
        "target sync must be the first exhaustion-stub operation: {output:?}"
    );
    assert!(
        matches!(output[stub_pc + 1], Insn::SyncModuleGlobal(1, 1)),
        "body sync must remain in the exhaustion stub: {output:?}"
    );
    assert_eq!(
        jump_target(stub_pc + 2, &output[stub_pc + 2]),
        Some(output.len()),
        "only the stub's terminal jump may receive the final-length patch"
    );
}

#[test]
fn int_loop_version_remaps_two_candidates_and_their_exit_edges() {
    let input = vec![
        // Candidate 1 exits to candidate 2's old head at pc 4.
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 3),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -3),
        // Candidate 2 exits past the old stream.
        Insn::CmpJumpIfFalse(2, BinaryOp::Lt, 3, 3),
        Insn::BinOpImm(2, 2, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(2, 1),
        Insn::CmpJumpIfTrue(2, BinaryOp::Lt, 3, -3),
    ];

    let versioned = versioned(input, &[]);
    let output = versioned.insns;

    assert_eq!(
        output
            .iter()
            .filter(|insn| matches!(insn, Insn::CountCmpJumpTrue(..)))
            .count(),
        2,
        "both non-overlapping candidates need a fast copy: {output:?}"
    );
    // Candidate 1: guards at 0..2, original header at 3. Candidate 2's guard
    // block begins at 7, so every old edge to its head must enter the guards.
    assert_eq!(jump_target(3, &output[3]), Some(7));
    // Candidate 2's original header is at 10 and exits past the final stream.
    assert_eq!(jump_target(10, &output[10]), Some(output.len()));
    // Rebuilt main stream ends at pc 14 after both guard blocks.
    assert_eq!(jump_target(14, &output[14]), Some(output.len()));
    assert_eq!(
        versioned.source_prefix_len, 15,
        "source-origin accounting must stop after the main-stream barrier"
    );
    for (pc, insn) in output.iter().enumerate() {
        if let Some(target) = jump_target(pc, insn) {
            assert!(
                target <= output.len(),
                "pc {pc} has out-of-range target {target}: {insn:?}; stream={output:?}"
            );
        }
    }
}

#[test]
fn appended_generic_binop_does_not_pollute_source_origin_counts() {
    let input = vec![
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 4),
        Insn::BinOpInPlace(4, 4, BinaryOp::Add, 0),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -4),
    ];

    let versioned = versioned(input, &[]);
    let prefix_len = versioned.source_prefix_len;
    let output = versioned.insns;
    let count = |stream: &[Insn]| {
        stream
            .iter()
            .filter(|insn| matches!(insn, Insn::BinOpInPlace(..)))
            .count()
    };
    assert_eq!(count(&output[..prefix_len]), 1);
    assert_eq!(
        count(&output),
        2,
        "the test must exercise an appended generic arithmetic duplicate"
    );
}

#[test]
fn appended_fast_copy_cannot_consume_source_origins() {
    let input = vec![
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 4),
        Insn::BinOpInPlace(4, 4, BinaryOp::Add, 0),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -4),
    ];
    let old_linenos = vec![10, 11, 12, 13, 14];
    let old_cols = vec![
        (10, 0, 10, 1),
        (11, 2, 11, 7),
        (12, 2, 12, 8),
        (13, 2, 13, 3),
        (14, 0, 14, 6),
    ];
    let versioned = versioned(input.clone(), &[]);

    let (linenos, cols) = remap_lineno_and_col_tables_with_source_prefix(
        &input,
        &old_linenos,
        &old_cols,
        &versioned.insns,
        versioned.source_prefix_len,
    );

    assert_eq!(linenos.len(), versioned.insns.len());
    assert_eq!(cols.len(), versioned.insns.len());
    assert!(
        versioned.insns[versioned.source_prefix_len..]
            .iter()
            .any(|insn| matches!(insn, Insn::BinOpInPlace(..))),
        "the suffix must contain a structurally duplicated generic binop"
    );
    assert!(
        cols[versioned.source_prefix_len..]
            .iter()
            .all(|&span| span == (0, 0, 0, 0)),
        "out-of-line copies must never inherit a source caret: {cols:?}"
    );
}

#[test]
fn constant_stop_fusion_allocates_one_temporary_and_uses_it() {
    let input = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -2),
    ];
    let constants = vec![Value::int(10)];
    let mut num_regs = 2;

    let result = pass_int_loop_version(input, &constants, &mut num_regs);

    assert_eq!(num_regs, 3);
    assert!(
        result
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::LoadConst(2, 0))),
        "the constant stop must be materialised once in the guard block"
    );
    assert!(
        result
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::CountCmpJumpTrue(0, BinaryOp::Lt, 2, 1, _))),
        "the fused latch must compare against the fresh stop register"
    );
}

#[test]
fn constant_stop_fusion_respects_the_frame_register_limit() {
    let input = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -2),
    ];
    let constants = vec![Value::int(10)];
    let mut num_regs = MAX_FRAME_REGS;

    let result = pass_int_loop_version(input.clone(), &constants, &mut num_regs);

    assert_eq!(num_regs, MAX_FRAME_REGS);
    assert_eq!(result.insns, input);
    assert_eq!(result.source_prefix_len, input.len());
}

#[test]
fn interior_landing_rejects_fusion_without_allocating_a_temporary() {
    // The jump at pc 1 lands on the latch at pc 3. Fusing pc 2 + pc 3 would
    // wrongly execute the increment when entering through that edge.
    let input = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 3),
        Insn::Jump(1),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -3),
    ];
    let constants = vec![Value::int(10)];
    let mut num_regs = 2;

    let result = pass_int_loop_version(input.clone(), &constants, &mut num_regs);

    assert_eq!(num_regs, 2);
    assert_eq!(result.insns, input);
}

#[test]
fn sync_bearing_branching_loop_is_not_versioned() {
    // The first execution assigns y, while the second assigns x.  Deferring
    // both syncs in lexical x/y order would change globals() insertion order.
    let input = vec![
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 6),
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 0, 2),
        Insn::LoadConst(2, 1),
        Insn::SyncModuleGlobal(2, 1),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 2),
        Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -6),
        Insn::Return(0),
    ];
    let constants = vec![Value::int(1), Value::int(7)];

    assert_eq!(
        version(input.clone(), &constants),
        input,
        "sync deferral needs a straight-line body"
    );
}

#[test]
fn exception_table_follows_guard_and_counted_jump_edges() {
    // Both raising sites are reachable only through newly introduced
    // conditional edges. They must retain the try handler after block-setup
    // instructions are stripped into the zero-cost exception table.
    //
    // 0 SetupExcept -> 9
    // 1 JumpIfNotInt -> 6 (original/deopt raise), fallthrough -> fast copy
    // 2 CountCmpJumpTrue -> 5 (fast branch raise), fallthrough -> normal exit
    // 3 Jump -> 8
    // 4 LoadNone (unreachable padding)
    // 5 RaiseValue
    // 6 RaiseValue
    // 7 Jump -> 8
    // 8 PopExcept
    // 9 handler
    let input = vec![
        Insn::SetupExcept(8),
        Insn::JumpIfNotInt(0, 4),
        Insn::CountCmpJumpTrue(0, BinaryOp::Lt, 1, 1, 2),
        Insn::Jump(4),
        Insn::LoadNone(3),
        Insn::RaiseValue(1),
        Insn::RaiseValue(0),
        Insn::Jump(0),
        Insn::PopExcept,
        Insn::LoadNone(9),
        Insn::Return(9),
    ];

    let source_prefix_len = 9;
    let (output, table, remapped_source_prefix_len) =
        build_exc_table_with_source_prefix(input, source_prefix_len);

    assert!(
        !output
            .iter()
            .any(|insn| matches!(insn, Insn::SetupExcept(_) | Insn::PopExcept))
    );
    let handler = output
        .iter()
        .position(|insn| matches!(insn, Insn::LoadNone(9)))
        .expect("handler must survive");
    let protected_raises: Vec<usize> = output
        .iter()
        .enumerate()
        .filter_map(|(pc, insn)| matches!(insn, Insn::RaiseValue(_)).then_some(pc))
        .collect();
    assert_eq!(protected_raises.len(), 2);
    for pc in protected_raises {
        assert_ne!(table[pc], EXC_NO_HANDLER, "raise at pc {pc} lost handler");
        assert_eq!(table[pc] as usize, handler);
    }
    assert_eq!(
        remapped_source_prefix_len, 7,
        "SetupExcept and PopExcept before the boundary must each compact it"
    );
}

#[test]
fn optimized_try_protects_the_production_deopt_target() {
    let code = optimize(compile_fn(
        "try:\n    i = 0\n    while i < 2:\n        i += 1\nexcept Exception:\n    i = -1\n",
    ));
    let (guard_pc, offset) = code
        .insns
        .iter()
        .enumerate()
        .find_map(|(pc, insn)| match insn {
            Insn::JumpIfNotInt(_, offset) => Some((pc, *offset)),
            _ => None,
        })
        .expect("the production pipeline must version the straight-line int loop");
    let deopt_target = (guard_pc as i64 + 1 + i64::from(offset)) as usize;

    assert!(
        !code
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::SetupExcept(_) | Insn::PopExcept)),
        "production optimization should use the zero-cost exception table"
    );
    assert_ne!(
        code.exc_table[deopt_target], EXC_NO_HANDLER,
        "the original loop reached by a failed int guard must remain protected"
    );
}

#[test]
fn counted_false_jump_is_a_conditional_exception_cfg_edge() {
    let input = vec![
        Insn::SetupExcept(4),
        Insn::CountCmpJumpFalse(0, BinaryOp::Lt, 1, 1, 1),
        Insn::Jump(1),
        Insn::RaiseValue(0),
        Insn::PopExcept,
        Insn::LoadNone(1),
        Insn::Return(1),
    ];

    let stacks = analyze_active_handler_stacks(&input).expect("CFG must be consistent");
    assert!(
        stacks[3].as_ref().is_some_and(|stack| !stack.is_empty()),
        "CountCmpJumpFalse branch target must inherit the active handler"
    );
}

#[test]
fn int_loop_opcodes_participate_in_register_analysis_and_compaction() {
    let guard = Insn::JumpIfNotInt(4, 0);
    let counted = Insn::CountCmpJumpFalse(5, BinaryOp::Ge, 6, -1, 0);

    assert!(insn_reads_reg(&guard, 4));
    assert!(insn_reads_reg(&counted, 5));
    assert!(insn_reads_reg(&counted, 6));
    let mut writes = HashSet::new();
    collect_writes(&counted, &mut writes);
    assert_eq!(writes, HashSet::from([5]));

    // Removing old pc 2 shifts both the guard's forward target and the counted
    // jump's backward target by one instruction.
    let compacted = compact(
        vec![
            Insn::LoadNone(0),
            Insn::JumpIfNotInt(0, 2),
            Insn::LoadNone(1),
            Insn::CountCmpJumpFalse(2, BinaryOp::Lt, 3, 1, -3),
            Insn::ReturnNone,
        ],
        &[true, true, false, true, true],
    );
    assert!(matches!(compacted[1], Insn::JumpIfNotInt(0, 1)));
    assert!(matches!(
        compacted[2],
        Insn::CountCmpJumpFalse(2, BinaryOp::Lt, 3, 1, -2)
    ));
}

#[test]
fn compiler_fuses_single_if_break_and_continue_to_direct_conditional_jumps() {
    let break_code = compile_fn("i = 0\nwhile True:\n    if i >= 3:\n        break\n    i += 1\n");
    assert!(
        break_code
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfTrue(_, offset) if *offset > 0)),
        "single if-break should branch directly to the loop exit: {:?}",
        break_code.insns
    );

    let continue_code =
        compile_fn("i = 0\nwhile i < 4:\n    i += 1\n    if i == 2:\n        continue\n");
    assert!(
        continue_code
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfTrue(_, offset) if *offset < 0)),
        "single if-continue should branch directly to the loop header: {:?}",
        continue_code.insns
    );
}

/// Issue #2889: `while i < N: if i % 2 == 0: … continue` at module scope.
///
/// The body reloads one scratch temp with a different constant per operand, so
/// the loop bound's `LoadConst` used to survive on the back-edge target.  That
/// made the region head a `LoadConst` and the back edge unconditional — a shape
/// neither `pass_loop_inversion` nor `pass_int_loop_version` recognises.  With
/// the bound folded into the comparison the back edge lands on the header again
/// and the loop reaches the guarded int version.
#[test]
fn continue_at_loop_top_reaches_int_loop_versioning() {
    let code = compile_fn(
        "i = 0\ntotal = 0\nwhile i < 10:\n    if i % 2 == 0:\n        i += 1\n        continue\n    total += i\n    i += 1\n",
    );
    let optimized = optimize(code);

    assert!(
        optimized
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::CmpJumpIfTrueConst(_, _, _, k) if *k < 0)),
        "the bound folds into the header, so loop inversion turns the \
         unconditional back-edge into a re-check: {:?}",
        optimized.insns
    );
    assert!(
        optimized
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfNotInt(..))),
        "the versioned copy needs entry guards: {:?}",
        optimized.insns
    );
    assert!(
        optimized.insns.iter().any(|insn| matches!(
            insn,
            Insn::CountCmpJumpTrue(..) | Insn::CountCmpJumpFalse(..)
        )),
        "the versioned copy needs a fused counted back-edge: {:?}",
        optimized.insns
    );
}
