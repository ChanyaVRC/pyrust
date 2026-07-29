use super::*;

use crate::ast::{BinaryOp, UnaryOp};
use crate::bytecode::EXC_NO_HANDLER;

fn jump_target(pc: usize, insn: &Insn) -> Option<usize> {
    let offset = match insn {
        Insn::Jump(offset)
        | Insn::JumpIfFalse(_, offset)
        | Insn::JumpIfTrue(_, offset)
        | Insn::JumpIfNotInt(_, offset)
        | Insn::JumpIfIterNotIntRange(_, offset)
        | Insn::JumpIfIterNotIndexedSeq(_, offset)
        | Insn::JumpIfIterNotIntRangeExact(_, offset)
        | Insn::GetItemSeqIntOrExit(_, _, _, offset)
        | Insn::JumpIfNotBuiltinLen(_, offset)
        | Insn::LenSeqOrExit(_, _, offset)
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
    versioned_with_names(input, constants, &[])
}

fn versioned_with_names(
    input: Vec<Insn>,
    constants: &[Value],
    names: &[String],
) -> IntLoopVersioningResult {
    let mut num_regs = 16;
    let mut constants = constants.to_vec();
    pass_int_loop_version(input, &mut constants, names, 0, &mut num_regs)
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
    let mut constants = vec![Value::int(10)];
    let mut num_regs = 2;

    let result = pass_int_loop_version(input, &mut constants, &[], 0, &mut num_regs);

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
    let mut constants = vec![Value::int(10)];
    let mut num_regs = MAX_FRAME_REGS;

    let result = pass_int_loop_version(input.clone(), &mut constants, &[], 0, &mut num_regs);

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
    let mut constants = vec![Value::int(10)];
    let mut num_regs = 2;

    let result = pass_int_loop_version(input.clone(), &mut constants, &[], 0, &mut num_regs);

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

// ── Mid-loop side exits (#2887) ──────────────────────────────────────────────

/// Follow the dispatch `Jump` that terminates the guard chain beginning at
/// `guard_pc`, returning the first instruction index of the fast copy it
/// selects.
fn fast_copy_head(insns: &[Insn], guard_pc: usize) -> usize {
    let mut pc = guard_pc;
    while !matches!(insns[pc], Insn::Jump(_)) {
        pc += 1;
    }
    jump_target(pc, &insns[pc]).expect("a guard chain ends in its dispatch jump")
}

/// Walk a deferred-sync stub, returning its `SyncModuleGlobal` operands and the
/// index its terminal `Jump` resumes at.
fn stub_at(insns: &[Insn], mut pc: usize) -> (Vec<(Reg, u16)>, usize) {
    let mut syncs = Vec::new();
    while let Insn::SyncModuleGlobal(reg, name_idx) = insns[pc] {
        syncs.push((reg, name_idx));
        pc += 1;
    }
    let resume = jump_target(pc, &insns[pc]).expect("a stub ends in its resume jump");
    (syncs, resume)
}

#[test]
fn for_over_a_list_gets_its_own_guarded_copy_with_an_element_side_exit() {
    let code = optimize(compile_fn(
        "xs = [1, 2, 3]\ntotal = 0\nfor x in xs:\n    total += x\n",
    ));
    let insns = &code.insns;

    // The range guard chain stays first, so a `for … in range(…)` still enters
    // its copy through exactly the instructions it used before.
    let range_guard = insns
        .iter()
        .position(|insn| matches!(insn, Insn::JumpIfIterNotIntRange(..)))
        .expect("the machine-int range chain must still be emitted");
    let seq_guard = insns
        .iter()
        .position(|insn| matches!(insn, Insn::JumpIfIterNotIndexedSeq(..)))
        .expect("a canonical list/tuple cursor must get its own guard chain");
    assert!(
        range_guard < seq_guard,
        "the range chain must be tried first: {insns:?}"
    );
    assert_eq!(
        jump_target(range_guard, &insns[range_guard]),
        Some(seq_guard),
        "a rejected range cursor falls through to the sequence chain"
    );

    let head = fast_copy_head(insns, seq_guard);
    let Insn::ForIter(loop_var, _, _) = insns[head] else {
        panic!("the sequence copy must open with its ForIter: {insns:?}");
    };
    let Insn::JumpIfNotInt(guarded, _) = insns[head + 1] else {
        panic!("the element type is a per-iteration fact: {insns:?}");
    };
    assert_eq!(
        guarded, loop_var,
        "the side exit must guard the loop variable"
    );

    // The side exit flushes every deferred sync and resumes the *original*
    // loop after its ForIter, because the shared cursor already advanced.
    let (syncs, resume) = stub_at(
        insns,
        jump_target(head + 1, &insns[head + 1]).expect("the guard jumps to its stub"),
    );
    assert_eq!(
        syncs.len(),
        2,
        "both the loop variable and the accumulator are deferred: {insns:?}"
    );
    assert!(
        resume < head,
        "a side exit resumes the original stream, not the copy: {insns:?}"
    );
    assert!(
        matches!(insns[resume], Insn::SyncModuleGlobal(reg, _) if reg == loop_var),
        "resume lands on the instruction after the original ForIter: {insns:?}"
    );
    assert!(
        matches!(insns[resume - 1], Insn::ForIter(..)),
        "resume lands immediately after the original ForIter: {insns:?}"
    );
}

#[test]
fn a_branching_for_body_keeps_only_the_range_copy() {
    // Interior control flow would let an execution path reach a use of the
    // loop variable without passing its side exit, so only the range cursor —
    // whose elements are ints by construction — is admitted.  Both synced
    // names are pre-bound so the branching body still clears the existing
    // globals-insertion-order condition.
    let code = optimize(compile_fn(
        "item = 0\nseen = 0\nfor item in [1, 2, 3]:\n    if item == 2:\n        seen += 1\n",
    ));

    assert!(
        code.insns
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfIterNotIntRange(..))),
        "the range copy is unaffected: {:?}",
        code.insns
    );
    assert!(
        !code
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfIterNotIndexedSeq(..))),
        "a branching body must not get a sequence copy: {:?}",
        code.insns
    );
}

#[test]
fn a_subscript_side_exit_resumes_the_original_get_item() {
    let code = optimize(compile_fn(
        "xs = [1, 2, 3]\ntotal = 0\nfor i in range(3):\n    total += xs[i]\n",
    ));
    let insns = &code.insns;

    let original_get_item = insns
        .iter()
        .position(|insn| matches!(insn, Insn::GetItem(..)))
        .expect("the original subscript must stay in place as the deopt target");
    let fast_pc = insns
        .iter()
        .position(|insn| matches!(insn, Insn::GetItemSeqIntOrExit(..)))
        .expect("the fast copy must run the deopting subscript form");
    let Insn::GetItemSeqIntOrExit(dst, obj, idx, _) = insns[fast_pc] else {
        unreachable!()
    };
    assert!(
        matches!(insns[original_get_item], Insn::GetItem(d, o, i) if (d, o, i) == (dst, obj, idx)),
        "the deopting form must carry the original operands: {insns:?}"
    );
    // The element type is a per-iteration fact carried by the same
    // instruction: it shares the subscript's deopt target, so it needs no
    // separate `JumpIfNotInt`.
    assert!(
        !matches!(insns[fast_pc + 1], Insn::JumpIfNotInt(guarded, _) if guarded == dst),
        "the element check folds into the read rather than following it: {insns:?}"
    );

    // The out-of-range, non-sequence and non-int-element deopts all resume at
    // the original subscript, which re-reads the same element — or raises with
    // its own line and caret.
    let (syncs, resume) = stub_at(
        insns,
        jump_target(fast_pc, &insns[fast_pc]).expect("a side exit jumps to its stub"),
    );
    assert_eq!(syncs.len(), 2, "every deferred sync is flushed: {insns:?}");
    assert_eq!(
        resume, original_get_item,
        "the subscript side exit must resume at the original subscript: {insns:?}"
    );
}

#[test]
fn a_subscript_clobbering_its_own_operand_is_not_versioned() {
    // Deopting after the fast read would destroy the operand the original
    // GetItem needs, so the whole candidate is rejected.
    let input = vec![
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 4),
        Insn::GetItem(2, 2, 0),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -4),
    ];

    assert_eq!(
        version(input.clone(), &[]),
        input,
        "a self-clobbering subscript must keep the original loop"
    );
}

#[test]
fn side_exit_opcodes_participate_in_register_analysis_and_compaction() {
    let subscript = Insn::GetItemSeqIntOrExit(4, 5, 6, 0);
    let iter_guard = Insn::JumpIfIterNotIndexedSeq(0, 0);
    let length = Insn::LenSeqOrExit(7, 8, 0);
    let len_guard = Insn::JumpIfNotBuiltinLen(9, 0);

    assert!(insn_reads_reg(&subscript, 5));
    assert!(insn_reads_reg(&subscript, 6));
    assert!(!insn_reads_reg(&subscript, 4));
    assert!(!insn_reads_reg(&iter_guard, 0));
    assert!(insn_reads_reg(&length, 8));
    assert!(!insn_reads_reg(&length, 7));
    assert!(insn_reads_reg(&len_guard, 9));
    let mut writes = HashSet::new();
    collect_writes(&subscript, &mut writes);
    assert_eq!(writes, HashSet::from([4]));
    let mut writes = HashSet::new();
    collect_writes(&length, &mut writes);
    assert_eq!(writes, HashSet::from([7]));
    let mut writes = HashSet::new();
    collect_writes(&len_guard, &mut writes);
    assert!(
        writes.is_empty(),
        "a value guard writes nothing: {writes:?}"
    );

    // Removing old pc 3 shifts every forward target by one instruction.
    let compacted = compact(
        vec![
            Insn::JumpIfIterNotIndexedSeq(0, 4),
            Insn::GetItemSeqIntOrExit(1, 2, 3, 3),
            Insn::JumpIfNotBuiltinLen(4, 2),
            Insn::LoadNone(5),
            Insn::LenSeqOrExit(6, 7, 1),
            Insn::ReturnNone,
        ],
        &[true, true, true, false, true, true],
    );
    assert!(matches!(compacted[0], Insn::JumpIfIterNotIndexedSeq(0, 3)));
    assert!(matches!(
        compacted[1],
        Insn::GetItemSeqIntOrExit(1, 2, 3, 2)
    ));
    assert!(matches!(compacted[2], Insn::JumpIfNotBuiltinLen(4, 1)));
    assert!(matches!(compacted[3], Insn::LenSeqOrExit(6, 7, 1)));
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "ran after a late-stage guarded pass")]
fn early_passes_reject_late_stage_opcodes_in_debug() {
    // A driver reorder that fed guarded-pass output back into an early
    // register-rewriting pass would miscompile silently (their kill-set
    // helpers use wildcard arms).  The debug guard must fail loudly instead.
    let insns = vec![Insn::JumpIfNotInt(0, 1), Insn::ReturnNone];
    let _ = pass_copy_prop(insns, 1);
}

#[test]
fn every_guarded_opcode_this_pass_emits_is_classified_late_stage() {
    // The driver's re-entry skip and the early passes' debug assert both key
    // off `is_late_stage_guard_insn`. An opcode this pass emits but that
    // predicate does not recognise would slip past both and reach a
    // register-rewriting pass whose wildcard kill-set arms cannot model it.
    for insn in [
        Insn::JumpIfNotInt(0, 1),
        Insn::JumpIfIterNotIntRange(0, 1),
        Insn::JumpIfIterNotIndexedSeq(0, 1),
        Insn::JumpIfIterNotIntRangeExact(
            Box::new(IntRangeExactGuard {
                slot: 0,
                start: 0,
                stop: 10,
                step: 1,
            }),
            1,
        ),
        Insn::GetItemSeqIntOrExit(0, 1, 2, 1),
        Insn::JumpIfNotBuiltinLen(0, 1),
        Insn::LenSeqOrExit(0, 1, 1),
        Insn::CountCmpJumpTrue(0, BinaryOp::Lt, 1, 1, -1),
        Insn::CountCmpJumpFalse(0, BinaryOp::Lt, 1, 1, -1),
    ] {
        assert!(
            is_late_stage_guard_insn(&insn),
            "{insn:?} is emitted by a late-stage guarded pass but is not classified as one"
        );
    }
}

// ── Closed-form counted loops ─────────────────────────────────────────────

/// The module-scope shape the compiler emits for
/// `for v in range(<args>): <body>`, with the producing call sitting adjacent
/// to the header exactly as the closed-form trace requires.  R2 is the call
/// base, R1 the loop variable, R0 the accumulator.
fn const_range_loop(args: &[u16], body: Vec<Insn>) -> Vec<Insn> {
    let mut insns = vec![Insn::LoadGlobal(2, 0)];
    for (k, &cidx) in args.iter().enumerate() {
        insns.push(Insn::LoadConst(3 + k as Reg, cidx));
    }
    insns.push(Insn::Call(2, args.len() as u8));
    insns.push(Insn::GetIter(0, 2));
    let k = (body.len() + 1) as i32;
    insns.push(Insn::ForIter(1, 0, k));
    insns.extend(body);
    // The back-edge sits at `head + k` and returns to the header.
    insns.push(Insn::Jump(-(k + 1)));
    insns
}

fn range_names() -> Vec<String> {
    vec!["range".to_string(), "total".to_string(), "v".to_string()]
}

/// `for v in range(<args>): total += 1` — the shape
/// `bench/cases/for_range_const.py` compiles to, one sync per assignment.
fn counted_loop(args: &[u16]) -> Vec<Insn> {
    const_range_loop(
        args,
        vec![
            Insn::SyncModuleGlobal(1, 2),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
            Insn::SyncModuleGlobal(0, 1),
        ],
    )
}

/// The appended out-of-line copies, i.e. everything past the source prefix.
fn fast_copies(result: &IntLoopVersioningResult) -> &[Insn] {
    &result.insns[result.source_prefix_len..]
}

fn has_closed_form_guard(result: &IntLoopVersioningResult) -> bool {
    result
        .insns
        .iter()
        .any(|insn| matches!(insn, Insn::JumpIfIterNotIntRangeExact(..)))
}

#[test]
fn closed_form_folds_a_constant_counted_loop_into_a_straight_line_copy() {
    // `for v in range(1000): total += 1`
    let result = versioned_with_names(counted_loop(&[0]), &[Value::int(1000)], &range_names());

    let guard = result
        .insns
        .iter()
        .find_map(|insn| match insn {
            Insn::JumpIfIterNotIntRangeExact(guard, _) => Some(guard.clone()),
            _ => None,
        })
        .expect("a constant counted loop must get a closed-form guard");
    assert_eq!(
        (guard.slot, guard.start, guard.stop, guard.step),
        (0, 0, 1000, 1),
        "the guard must pin the exact traced bounds, not just the cursor kind"
    );

    // The closed-form copy leads the appended copies: the loop variable's last
    // value, one add per accumulator, then straight into the sync stub. No
    // back-edge and no exit jump.
    let copies = fast_copies(&result);
    assert!(
        matches!(copies[0], Insn::LoadConst(1, _)),
        "the loop variable must be bound to the range's last value: {copies:?}"
    );
    assert!(
        matches!(copies[1], Insn::BinOpConst(0, 0, BinaryOp::Add, _, _)),
        "the accumulator must be settled with one add: {copies:?}"
    );
    assert!(
        matches!(copies[2], Insn::SyncModuleGlobal(1, 2)),
        "the copy must fall straight into its deferred-sync stub: {copies:?}"
    );
}

#[test]
fn closed_form_constants_hold_the_exact_fold() {
    // `for v in range(10, 1000, 3): total += v` — the delta is the sum of the
    // yielded values, not a per-step constant.
    let body = vec![
        Insn::SyncModuleGlobal(1, 2),
        Insn::BinOpInPlace(0, 0, BinaryOp::Add, 1),
        Insn::SyncModuleGlobal(0, 1),
    ];
    let mut constants = vec![Value::int(10), Value::int(1000), Value::int(3)];
    let mut num_regs = 16;

    let result = pass_int_loop_version(
        const_range_loop(&[0, 1, 2], body),
        &mut constants,
        &range_names(),
        0,
        &mut num_regs,
    );

    let copies = fast_copies(&result);
    let Insn::LoadConst(1, var_slot) = copies[0] else {
        panic!("expected the loop variable's final binding: {copies:?}")
    };
    let Insn::BinOpConst(0, 0, BinaryOp::Add, delta_slot, _) = copies[1] else {
        panic!("expected the folded accumulator add: {copies:?}")
    };
    // range(10, 1000, 3) yields 330 values, 10 through 997, summing to 166155.
    assert_eq!(constants[usize::from(var_slot)].as_int(), Some(997));
    assert_eq!(
        constants[usize::from(delta_slot)].as_int(),
        Some(166_155),
        "the delta must be the exact sum the iterated adds would produce"
    );
}

#[test]
fn closed_form_declines_a_register_stop() {
    // `range(n)`: the argument is a register, so no triple can be proposed and
    // the guard would have nothing to pin.
    let mut insns = counted_loop(&[0]);
    insns[1] = Insn::Move(3, 7);

    let result = versioned_with_names(insns, &[Value::int(1000)], &range_names());

    assert!(
        !has_closed_form_guard(&result),
        "a non-constant bound must not fold: {:?}",
        result.insns
    );
    assert!(
        result
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfIterNotIntRange(..))),
        "the ordinary per-iteration copy must still be emitted: {:?}",
        result.insns
    );
}

#[test]
fn closed_form_declines_a_zero_trip_range() {
    // `for v in range(0)` binds nothing and accumulates nothing; folding it
    // would hand the exit stub bindings the original never made.
    let result = versioned_with_names(counted_loop(&[0]), &[Value::int(0)], &range_names());

    assert!(
        !has_closed_form_guard(&result),
        "a zero-trip range must be left to the ordinary copy: {:?}",
        result.insns
    );
}

#[test]
fn closed_form_declines_bounds_beyond_the_compact_cursor() {
    // A range whose one-past-the-end cursor leaves i64 becomes `BigRange` at
    // runtime, so an exact machine-int guard could never match it.
    let result = versioned_with_names(
        counted_loop(&[0, 1]),
        &[Value::int(i64::MIN), Value::int(i64::MAX)],
        &range_names(),
    );

    assert!(
        !has_closed_form_guard(&result),
        "a range outside the compact cursor must not fold: {:?}",
        result.insns
    );
}

#[test]
fn closed_form_declines_a_shadowed_range_name() {
    // The producing call resolves some other global. A rebound `range` would
    // still be caught by the runtime guard, but the pass has no reason to
    // propose a triple for a name it cannot recognise.
    let names = vec!["shadow".to_string(), "total".to_string(), "v".to_string()];

    let result = versioned_with_names(counted_loop(&[0]), &[Value::int(1000)], &names);

    assert!(
        !has_closed_form_guard(&result),
        "only a call to the name `range` proposes a triple: {:?}",
        result.insns
    );
}

#[test]
fn closed_form_declines_a_non_linear_body() {
    // `total *= 2` does not accumulate a per-trip delta.
    let body = vec![
        Insn::SyncModuleGlobal(1, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Mul, 2, true),
        Insn::SyncModuleGlobal(0, 1),
    ];

    let result = versioned_with_names(
        const_range_loop(&[0], body),
        &[Value::int(1000)],
        &range_names(),
    );

    assert!(
        !has_closed_form_guard(&result),
        "a non-linear body must not fold: {:?}",
        result.insns
    );
}

#[test]
fn closed_form_declines_a_body_that_rebinds_the_loop_variable() {
    // Writing `v` invalidates the traced sequence the fold is built from.
    let body = vec![
        Insn::SyncModuleGlobal(1, 2),
        Insn::BinOpImm(1, 1, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 1),
    ];

    let result = versioned_with_names(
        const_range_loop(&[0], body),
        &[Value::int(1000)],
        &range_names(),
    );

    assert!(
        !has_closed_form_guard(&result),
        "a body that rebinds the loop variable must not fold: {:?}",
        result.insns
    );
}

#[test]
fn closed_form_guard_leads_the_chain_without_displacing_the_ordinary_copies() {
    let result = versioned_with_names(counted_loop(&[0]), &[Value::int(1000)], &range_names());

    let guards: Vec<usize> = result
        .insns
        .iter()
        .enumerate()
        .filter_map(|(pc, insn)| {
            matches!(
                insn,
                Insn::JumpIfIterNotIntRangeExact(..)
                    | Insn::JumpIfIterNotIntRange(..)
                    | Insn::JumpIfIterNotIndexedSeq(..)
            )
            .then_some(pc)
        })
        .collect();
    assert_eq!(guards.len(), 3, "all three iterator guards must be emitted");
    assert!(
        matches!(
            result.insns[guards[0]],
            Insn::JumpIfIterNotIntRangeExact(..)
        ),
        "the closed form must be tried first: {:?}",
        result.insns
    );

    for (pc, insn) in result.insns.iter().enumerate() {
        if let Some(target) = jump_target(pc, insn) {
            assert!(
                target <= result.insns.len(),
                "pc {pc} has out-of-range target {target}: {insn:?}"
            );
        }
    }
}

#[test]
fn closed_form_traces_a_negated_literal_bound() {
    // `for v in range(100, 0, -7): total += v`.  A negative literal survives as
    // `LoadConst` + `UnaryOp(Neg)` + `Move` because `pass_unary_fold` declines
    // to fold across the loop's back edge, so the trace must interpret the
    // setup rather than assume one instruction per argument.
    let insns = vec![
        Insn::LoadGlobal(2, 0),
        Insn::LoadConst(3, 0),
        Insn::LoadConst(4, 1),
        Insn::LoadConst(6, 2),
        Insn::UnaryOp(6, UnaryOp::Neg, 6),
        Insn::Move(5, 6),
        Insn::Call(2, 3),
        Insn::GetIter(0, 2),
        Insn::ForIter(1, 0, 4),
        Insn::SyncModuleGlobal(1, 2),
        Insn::BinOpInPlace(0, 0, BinaryOp::Add, 1),
        Insn::SyncModuleGlobal(0, 1),
        Insn::Jump(-5),
    ];
    let mut constants = vec![Value::int(100), Value::int(0), Value::int(7)];
    let mut num_regs = 16;

    let result = pass_int_loop_version(insns, &mut constants, &range_names(), 0, &mut num_regs);

    let guard = result
        .insns
        .iter()
        .find_map(|insn| match insn {
            Insn::JumpIfIterNotIntRangeExact(guard, _) => Some(guard.clone()),
            _ => None,
        })
        .expect("a negated constant bound must still fold");
    assert_eq!(
        (guard.slot, guard.start, guard.stop, guard.step),
        (0, 100, 0, -7),
        "the guard must pin the negated step it folded"
    );

    let copies = fast_copies(&result);
    let Insn::LoadConst(1, var_slot) = copies[0] else {
        panic!("expected the loop variable's final binding: {copies:?}")
    };
    let Insn::BinOpConst(0, 0, BinaryOp::Add, delta_slot, _) = copies[1] else {
        panic!("expected the folded accumulator add: {copies:?}")
    };
    // range(100, 0, -7) yields 15 values, 100 down to 2, summing to 765.
    assert_eq!(constants[usize::from(var_slot)].as_int(), Some(2));
    assert_eq!(constants[usize::from(delta_slot)].as_int(), Some(765));
}

#[test]
fn closed_form_declines_a_negated_literal_that_leaves_i64() {
    // `-i64::MIN` promotes to `BigInt`, which the machine-int cursor the guard
    // tests for can never hold.
    let insns = vec![
        Insn::LoadGlobal(2, 0),
        Insn::LoadConst(4, 0),
        Insn::UnaryOp(4, UnaryOp::Neg, 4),
        Insn::Move(3, 4),
        Insn::Call(2, 1),
        Insn::GetIter(0, 2),
        Insn::ForIter(1, 0, 4),
        Insn::SyncModuleGlobal(1, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 1),
        Insn::Jump(-5),
    ];
    let mut constants = vec![Value::int(i64::MIN)];
    let mut num_regs = 16;

    let result = pass_int_loop_version(insns, &mut constants, &range_names(), 0, &mut num_regs);

    assert!(
        !has_closed_form_guard(&result),
        "a bound that leaves i64 under negation must not fold: {:?}",
        result.insns
    );
}

#[test]
fn closed_form_declines_a_computed_bound_in_the_call_setup() {
    // The setup whitelist is closed: an instruction it cannot interpret ends
    // the proposal instead of leaving a stale register to be read as a bound.
    let insns = vec![
        Insn::LoadGlobal(2, 0),
        Insn::LoadConst(4, 0),
        Insn::BinOpImm(3, 4, BinaryOp::Add, 1, false),
        Insn::Call(2, 1),
        Insn::GetIter(0, 2),
        Insn::ForIter(1, 0, 4),
        Insn::SyncModuleGlobal(1, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 1),
        Insn::Jump(-5),
    ];
    let mut constants = vec![Value::int(1000)];
    let mut num_regs = 16;

    let result = pass_int_loop_version(insns, &mut constants, &range_names(), 0, &mut num_regs);

    assert!(
        !has_closed_form_guard(&result),
        "a computed bound must not be traced as a constant: {:?}",
        result.insns
    );
}

fn entry_guarded_registers(insns: &[Insn]) -> Vec<u32> {
    insns
        .iter()
        .filter_map(|insn| match insn {
            Insn::JumpIfNotInt(reg, _) => Some(*reg),
            _ => None,
        })
        .collect()
}

#[test]
fn entry_guards_skip_a_body_temporary_defined_before_it_is_read() {
    // `while i < N: if i % 2 != 0: total += i; i += 1` computes its branch
    // operand into a temporary that is legitimately unset when the loop is
    // first entered.  Guarding that temporary would divert every entry to the
    // original stream and make the fast copy unreachable.
    let input = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 5),
        Insn::BinOpConst(2, 0, BinaryOp::Mod, 1, false),
        Insn::CmpJumpIfFalseConst(2, BinaryOp::Ne, 2, 1),
        Insn::BinOpInPlace(1, 1, BinaryOp::Add, 0),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -5),
        Insn::Return(1),
    ];

    let output = version(input, &[Value::int(10), Value::int(2), Value::int(0)]);
    let guarded = entry_guarded_registers(&output);

    assert!(
        !guarded.is_empty(),
        "the loop must still be versioned: {output:?}"
    );
    assert!(
        !guarded.contains(&2),
        "the body temporary is defined before it is read: {output:?}"
    );
    assert!(
        guarded.contains(&0) && guarded.contains(&1),
        "loop-carried registers still need entry guards: {output:?}"
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
fn entry_guards_keep_a_register_read_before_the_region_redefines_it() {
    // `r2` is read by the header comparison and only then recomputed, so its
    // entry value is live and its guard must stay.
    let input = vec![
        Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 2, 3),
        Insn::BinOpConst(2, 0, BinaryOp::Add, 0, false),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 2, -3),
        Insn::Return(0),
    ];

    let output = version(input, &[Value::int(1)]);

    assert!(
        entry_guarded_registers(&output).contains(&2),
        "a register read before its in-region definition keeps its guard: {output:?}"
    );
}

#[test]
fn entry_guards_stay_when_a_branch_can_skip_the_definition() {
    // The interior branch jumps past the definition of `r2`, so a path reaches
    // the add that reads `r2` without ever running that definition.
    let input = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 6),
        Insn::CmpJumpIfFalseConst(1, BinaryOp::Ne, 2, 2),
        Insn::BinOpConst(2, 0, BinaryOp::Mod, 1, false),
        Insn::BinOpInPlace(1, 1, BinaryOp::Add, 2),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -6),
        Insn::Return(1),
    ];

    let output = version(input, &[Value::int(10), Value::int(2), Value::int(0)]);
    let guarded = entry_guarded_registers(&output);

    assert!(
        guarded.is_empty() || guarded.contains(&2),
        "a definition a branch can skip does not retire its guard: {output:?}"
    );
}

#[test]
fn sync_deferral_declines_two_names_from_one_scratch_register() {
    // `a = i * 2` / `c = i + 1` at module scope both publish from the scratch
    // register the compiler reuses for each expression, so replaying them from
    // an exit stub would bind both names to the register's last value.
    let input = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 8),
        Insn::BinOpConst(5, 0, BinaryOp::Mul, 1, false),
        Insn::Move(1, 5),
        Insn::SyncModuleGlobal(5, 0),
        Insn::BinOpConst(5, 0, BinaryOp::Add, 2, false),
        Insn::Move(2, 5),
        Insn::SyncModuleGlobal(5, 1),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -8),
        Insn::Return(1),
    ];
    let names = ["a".to_string(), "c".to_string()];

    let result = versioned_with_names(
        input.clone(),
        &[Value::int(4), Value::int(2), Value::int(1)],
        &names,
    );

    assert_eq!(
        result.insns, input,
        "a region whose synced register is overwritten before the exit is not versioned"
    );
}

#[test]
fn sync_deferral_accepts_a_source_that_survives_to_the_exit() {
    // The counted shape: each synced name has its own register and the write
    // immediately reaches that name's sync, so the values the stub replays are
    // the ones the original published.
    let input = vec![
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 5),
        Insn::BinOpInPlace(1, 1, BinaryOp::Add, 0),
        Insn::SyncModuleGlobal(1, 1),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -5),
        Insn::Return(1),
    ];
    let names = ["i".to_string(), "total".to_string()];

    let result = versioned_with_names(input.clone(), &[Value::int(40)], &names);

    assert!(
        result.insns.len() > input.len(),
        "the region must still be versioned: {:?}",
        result.insns
    );
}

#[test]
fn a_continue_reentered_header_exit_flushes_the_deferred_syncs() {
    // `while i < N: if i % 3 == 0: i += 1; continue; acc += i; i += 1` leaves
    // through the header whenever its last iteration took the `continue` edge,
    // so that edge must route through a deferred-sync stub rather than jump
    // straight to the original exit target.
    let input = vec![
        Insn::LoadConst(0, 2),
        Insn::SyncModuleGlobal(0, 0),
        Insn::LoadConst(1, 2),
        Insn::SyncModuleGlobal(1, 1),
        Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 8),
        Insn::BinOpConst(3, 0, BinaryOp::Mod, 1, false),
        Insn::CmpJumpIfFalseConst(3, BinaryOp::Eq, 2, 3),
        Insn::BinOpImm(0, 0, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(0, 0),
        Insn::Jump(-6),
        Insn::BinOpInPlace(1, 1, BinaryOp::Add, 0),
        Insn::SyncModuleGlobal(1, 1),
        Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -8),
        Insn::Return(1),
    ];
    let names = ["i".to_string(), "acc".to_string()];

    let result = versioned_with_names(
        input,
        &[Value::int(7), Value::int(3), Value::int(0)],
        &names,
    );
    let insns = &result.insns;
    let fast_head = result.source_prefix_len;
    assert!(
        insns.len() > fast_head,
        "the region must be versioned: {insns:?}"
    );

    let header_exit = jump_target(fast_head, &insns[fast_head])
        .expect("the copied header keeps its conditional exit");
    assert!(
        insns[header_exit..]
            .iter()
            .take_while(|insn| matches!(insn, Insn::SyncModuleGlobal(..)))
            .count()
            > 0,
        "the copied header's exit must land on a deferred-sync stub, not the \
         original exit target: header_exit={header_exit} stream={insns:?}"
    );
}

// ── `while i < len(seq):` headers ─────────────────────────────────────────

fn len_names() -> Vec<String> {
    ["len", "total", "i"].map(String::from).to_vec()
}

/// The module-scope shape the compiler emits for
///
/// ```text
/// while i < len(xs):
///     total += xs[i]
///     i += 1
/// ```
///
/// R0 is the sequence, R1 the accumulator, R2 the cursor, R3 the element
/// scratch, R4 the call base and R5 its argument slot.  `pass_loop_inversion`
/// leaves this shape alone: the back-edge has to re-run the whole call, so it
/// stays an unconditional `Jump` to the `LoadGlobal`.
fn len_header_loop(body: Vec<Insn>) -> Vec<Insn> {
    let region = 3 + body.len() + 1;
    let mut insns = vec![
        Insn::LoadGlobal(4, 0),
        Insn::Move(5, 0),
        Insn::Call(4, 1),
        Insn::CmpJumpIfFalse(2, BinaryOp::Lt, 4, (region - 3) as i32),
    ];
    insns.extend(body);
    insns.push(Insn::Jump(-(region as i32 + 1)));
    insns.push(Insn::Return(1));
    insns
}

fn len_header_body() -> Vec<Insn> {
    vec![
        Insn::GetItem(3, 0, 2),
        Insn::BinOpInPlace(1, 1, BinaryOp::Add, 3),
        Insn::SyncModuleGlobal(1, 1),
        Insn::BinOpImm(2, 2, BinaryOp::Add, 1, true),
        Insn::SyncModuleGlobal(2, 2),
    ]
}

#[test]
fn len_header_copy_reads_the_length_natively_every_iteration() {
    let result = versioned_with_names(
        len_header_loop(len_header_body()),
        &[Value::int(1)],
        &len_names(),
    );
    let insns = &result.insns;
    let fast = &insns[result.source_prefix_len..];

    assert!(
        insns[..result.source_prefix_len]
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfNotBuiltinLen(4, _))),
        "the entry guard must value-check the loaded callee: {insns:?}"
    );
    let len_read = fast
        .iter()
        .position(|insn| matches!(insn, Insn::LenSeqOrExit(4, 0, _)))
        .expect("the header triple must become a native length read");
    assert_eq!(
        len_read, 0,
        "the length read opens the copy, so the back-edge re-runs it: {insns:?}"
    );
    assert!(
        !fast.iter().any(|insn| matches!(insn, Insn::Call(..))),
        "the copy must not keep the call it specializes: {insns:?}"
    );
    // The copy rotates the header down to the latch, so the *loop* carries a
    // second length read: the bound is re-derived on every pass, which is what
    // lets a body that resizes the sequence move it.
    let latch = fast
        .iter()
        .rposition(|insn| matches!(insn, Insn::LenSeqOrExit(4, 0, _)))
        .expect("the latch re-reads the length");
    assert!(
        latch > 0,
        "the latch read must be distinct from the entry read: {insns:?}"
    );
    let back = fast
        .iter()
        .position(|insn| matches!(insn, Insn::CountCmpJumpTrue(2, BinaryOp::Lt, 4, 1, _)))
        .expect("the counter increment fuses into the rotated comparison");
    assert_eq!(
        back,
        latch + 1,
        "the fused latch must follow its length read: {insns:?}"
    );
    assert_eq!(
        jump_target(result.source_prefix_len + back, &fast[back]),
        Some(result.source_prefix_len + 2),
        "the latch must jump back to the body top, past the entry test: {insns:?}"
    );
}

#[test]
fn len_header_latch_deopt_resumes_at_the_counter_increment() {
    // The rotated latch reads the length *above* the increment, so a deopt
    // there must resume the original at the increment — not at the region
    // head, which would re-run the body for an index already consumed.
    let result = versioned_with_names(
        len_header_loop(len_header_body()),
        &[Value::int(1)],
        &len_names(),
    );
    let insns = &result.insns;
    let latch = result.source_prefix_len
        + insns[result.source_prefix_len..]
            .iter()
            .rposition(|insn| matches!(insn, Insn::LenSeqOrExit(4, 0, _)))
            .expect("the latch re-reads the length");

    let stub = jump_target(latch, &insns[latch]).expect("the latch read side-exits");
    let terminal = stub
        + insns[stub..]
            .iter()
            .position(|insn| !matches!(insn, Insn::SyncModuleGlobal(..)))
            .expect("the stub ends in a jump");
    let resume = jump_target(terminal, &insns[terminal]).expect("the stub jumps");
    assert!(
        matches!(insns[resume], Insn::BinOpImm(2, 2, BinaryOp::Add, 1, true)),
        "the latch deopt must resume at the un-run increment: resume={resume} {insns:?}"
    );
}

#[test]
fn len_header_call_base_is_not_entry_guarded() {
    // The call base is unset until the original `LoadGlobal` runs, so an entry
    // `JumpIfNotInt` on it would divert every first entry and the copy would
    // never execute — the #2922 body-temporary failure mode.
    let result = versioned_with_names(
        len_header_loop(len_header_body()),
        &[Value::int(1)],
        &len_names(),
    );
    assert!(
        !result
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfNotInt(4, _))),
        "the call base must not be entry-guarded: {:?}",
        result.insns
    );
}

#[test]
fn len_header_side_exit_resumes_the_original_loop_not_the_guard_block() {
    // Deopting to `jump_target[head]` would re-enter the entry guards, pass
    // them again, and jump straight back into the copy that just failed.
    let result = versioned_with_names(
        len_header_loop(len_header_body()),
        &[Value::int(1)],
        &len_names(),
    );
    let insns = &result.insns;
    let len_pc = result.source_prefix_len;
    let Insn::LenSeqOrExit(..) = insns[len_pc] else {
        panic!("the copy opens with its length read: {insns:?}");
    };

    let stub = jump_target(len_pc, &insns[len_pc]).expect("the length read side-exits");
    let terminal = stub
        + insns[stub..]
            .iter()
            .position(|insn| !matches!(insn, Insn::SyncModuleGlobal(..)))
            .expect("the stub ends in a jump");
    let resume = jump_target(terminal, &insns[terminal]).expect("the stub jumps");
    assert!(
        matches!(insns[resume], Insn::LoadGlobal(4, 0)),
        "the deopt must resume at the original LoadGlobal: resume={resume} {insns:?}"
    );
    assert!(
        !matches!(insns[resume + 1], Insn::JumpIfNotBuiltinLen(..)),
        "resuming inside the guard block would spin on the failed check: {insns:?}"
    );
}

#[test]
fn len_header_declines_when_the_body_touches_the_call_scratch() {
    // The copy never materialises the argument slot and rewrites the call base
    // from the sequence, so a body that reads either would observe a register
    // the copy does not maintain.
    for extra in [Insn::Move(1, 5), Insn::Move(1, 4)] {
        let mut body = len_header_body();
        body.insert(0, extra.clone());
        let input = len_header_loop(body);
        assert_eq!(
            versioned_with_names(input.clone(), &[Value::int(1)], &len_names()).insns,
            input,
            "a body reading {extra:?} must keep the original loop"
        );
    }
}

#[test]
fn len_header_declines_when_the_call_base_aliases_a_fast_local() {
    // With six fast locals the call base R4 and its argument slot R5 are named
    // module bindings, visible through the namespace mirror; the copy leaves
    // both stale, so the shape is not admitted at all.
    let input = len_header_loop(len_header_body());
    let mut constants = vec![Value::int(1)];
    let mut num_regs = 16;
    let names = len_names();
    let result = pass_int_loop_version(input.clone(), &mut constants, &names, 6, &mut num_regs);
    assert_eq!(
        result.insns, input,
        "a fast-local call base must keep the original loop"
    );
}

#[test]
fn len_header_declines_when_the_comparison_ignores_the_length() {
    // Nothing reads the call's result, so specializing the header would elide
    // a call whose only purpose is its side effects.
    let mut input = len_header_loop(len_header_body());
    input[3] = Insn::CmpJumpIfFalse(2, BinaryOp::Lt, 1, 6);
    assert_eq!(
        versioned_with_names(input.clone(), &[Value::int(1)], &len_names()).insns,
        input,
        "a header that does not compare against the length must not version"
    );
}

#[test]
fn len_header_declines_for_a_non_len_global() {
    let names = ["size", "total", "i"].map(String::from).to_vec();
    let input = len_header_loop(len_header_body());
    assert_eq!(
        versioned_with_names(input.clone(), &[Value::int(1)], &names).insns,
        input,
        "only the built-in `len` header shape is admitted"
    );
}
