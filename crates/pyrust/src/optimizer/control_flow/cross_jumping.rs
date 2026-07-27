// ─── Cross-jumping (tail merging) ─────────────────────────────────────────────

/// Tail-merge identical block suffixes: when two or more basic blocks end in
/// an identical sequence of ≥ 2 instructions, replace the duplicate copy with
/// an unconditional `Jump` to the surviving copy.  Runs to a fixed point so
/// that N-arm `if/elif/else` chains have all duplicate tails merged in a single
/// `optimize_fn_code` call.
///
/// ## Algorithm
///
/// 1. Collect all **jump-target** instruction indices (anything pointed to by a
///    `Jump`, `JumpIfTrue/False`, `ForIter`, etc.).
/// 2. Scan every pair of **block terminator** positions `(t_keep, t_dup)` where
///    `t_keep < t_dup`.  A terminator is a `Return`, `ReturnNone`, `Jump`, or
///    `Raise*` instruction.
/// 3. Compare instructions backward from each terminator simultaneously, stopping
///    when they first differ.  Count the number of matching instructions `n`.
/// 4. A merge fires when **all** of the following hold:
///    - `n >= MIN_TAIL` (= 2).
///    - None of the `n` instructions in the duplicate tail contains a jump
///      offset field (`Jump`, `JumpIfFalse`, `ForIter`, `SetupExcept`, etc.) —
///      such offsets encode PC-relative targets that would mismatch between the
///      two locations.
///    - None of the duplicate-side tail instruction indices are jump targets
///      (another instruction jumps *into* the middle of the tail we are about to
///      remove — forbidden).
///    - Every pair of corresponding tail instructions has the same active
///      exception-handler stack.  A shared PC can only have one zero-cost
///      handler-table entry.
///    - Every pair carries the same effective source line and PEP 657 caret
///      span, so traceback attribution is unchanged.
/// 5. When a merge fires: keep `[t_keep - n + 1 .. t_keep]` as-is (survivor);
///    mark `[t_dup - n + 1 .. t_dup - 1]` as removed in `keep[]`; replace
///    `insns[t_dup]` (the duplicate terminator) with `Jump(k)` pointing at
///    `t_keep - n + 1` (the survivor tail start).
/// 6. Call `compact` once at the end to fix all jump offsets.
/// 7. Repeat from step 1 until no merge fires (fixed-point).
///
/// ## Conservatism
///
/// Register renaming is NOT performed.  Only structurally identical instruction
/// sequences — same opcode, same register numbers, same constant indices — are
/// merged.  This is conservative but correct.
///
/// ## What is NOT merged
///
/// - Tails of length 1 (`return` alone is not worth a Jump overhead).
/// - Any tail containing an instruction with a jump-offset field.
/// - Any tail where a duplicate-side instruction is itself a jump target.
/// - Tails reached under different exception-handler stacks.
/// - Tails with different line or column metadata.
fn pass_cross_jump(
    mut insns: Vec<Insn>,
    linenos: &[u32],
    cols: &[crate::ast::CaretSpan],
) -> Vec<Insn> {
    // `linenos[i]` is the source line of `insns[i]` (0 = inherit previous).  The
    // pass refuses to merge tails with different effective lines or direct caret
    // spans, so each raising site keeps its exact traceback attribution.  Both
    // slices are compacted alongside `insns` on every fixed-point iteration.
    //
    // Pad/truncate to match `insns` so tests may pass empty slices.  A 0 line
    // inherits the preceding line; a zero caret span means "no anchor".
    let mut linenos = linenos.to_vec();
    linenos.resize(insns.len(), 0);
    let mut cols = cols.to_vec();
    cols.resize(insns.len(), (0, 0, 0, 0));
    loop {
        let (next, next_linenos, next_cols, changed) = pass_cross_jump_once(insns, linenos, cols);
        insns = next;
        linenos = next_linenos;
        cols = next_cols;
        if !changed {
            return insns;
        }
    }
}

/// Single-pass helper for [`pass_cross_jump`].  Finds the first mergeable tail
/// pair, applies the merge, and returns `(new_insns, true)`.  Returns
/// `(insns, false)` when no merge candidate exists (fixed-point reached).
fn pass_cross_jump_once(
    insns: Vec<Insn>,
    linenos: Vec<u32>,
    cols: Vec<crate::ast::CaretSpan>,
) -> (Vec<Insn>, Vec<u32>, Vec<crate::ast::CaretSpan>, bool) {
    const MIN_TAIL: usize = 2;

    let n = insns.len();
    if n < MIN_TAIL * 2 {
        return (insns, linenos, cols, false);
    }

    // A shared instruction PC can carry only one zero-cost exception-table
    // entry.  Recompute the active stacks on every fixed-point iteration
    // because compacting a prior merge retargets absolute handler PCs.  If the
    // input already has conflicting stacks, leave it untouched and let
    // `build_exc_table` use its conservative dynamic-stack fallback.
    let Some(handler_stacks) = analyze_active_handler_stacks(&insns) else {
        return (insns, linenos, cols, false);
    };

    // Effective source line of each instruction: a 0 entry inherits the last
    // non-zero line above it (matching the VM's `cur_line` tracking).  Used to
    // compare the two terminators' lines before merging.
    let eff_line: Vec<u32> = {
        let mut running = 0u32;
        linenos
            .iter()
            .map(|&ln| {
                if ln != 0 {
                    running = ln;
                }
                running
            })
            .collect()
    };

    // Step 1: collect jump-target indices.
    let mut jump_targets: HashSet<usize> = HashSet::new();
    jump_targets.insert(0); // entry point is always a target
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target < n {
                jump_targets.insert(target);
            }
        }
    }

    // Returns true if `insn` contains a PC-relative jump offset field.
    // Such instructions must not appear inside a merged tail because the same
    // offset value would resolve to different targets in two different blocks.
    let has_jump_offset = |insn: &Insn| -> bool {
        matches!(
            insn,
            Insn::Jump(_)
                | Insn::JumpIfFalse(..)
                | Insn::JumpIfTrue(..)
                | Insn::CmpJumpIfFalse(..)
                | Insn::CmpJumpIfTrue(..)
                | Insn::CmpJumpIfFalseConst(..)
                | Insn::CmpJumpIfTrueConst(..)
                | Insn::ForIter(..)
                | Insn::SetupExcept(_)
                | Insn::MatchExcept(..)
                | Insn::MatchExceptStar(..)
        )
    };

    // Returns true if `insn` is a block terminator (ends a basic block).
    let is_terminator = |insn: &Insn| -> bool {
        matches!(
            insn,
            Insn::Return(_)
                | Insn::ReturnNone
                | Insn::Jump(_)
                | Insn::RaiseValue(_)
                | Insn::RaiseExceptStarResidual(_)
                | Insn::RaiseFrom(..)
                | Insn::RaiseReRaise
                | Insn::RaiseAssert(_)
                | Insn::RaiseAssertNoMsg
        )
    };

    // Step 2: collect terminator positions.
    let terminators: Vec<usize> = (0..n).filter(|&i| is_terminator(&insns[i])).collect();

    // Step 3: find a merge candidate.
    // Outer: surviving tail terminator t_keep (earlier in code).
    // Inner: duplicate tail terminator t_dup (later in code).
    for &t_keep in &terminators {
        for &t_dup in &terminators {
            if t_dup <= t_keep {
                continue; // must be strictly later
            }

            // Compare instructions backward from each terminator.
            // Stop when:
            //   (a) instructions differ,
            //   (b) instruction has a jump offset (offset meaning differs
            //       between the two blocks),
            //   (c) we hit a jump target at step > 0 (block boundary —
            //       extending past it would require merging predecessor flow).
            let mut tail_len = 0usize;
            for step in 0usize.. {
                // Bounds check: the tail reached the start of one block; stop
                // scanning but do NOT abort — tail_len already counts the
                // matching instructions up to this point.
                if step > t_keep || step > t_dup {
                    break;
                }
                let i_keep = t_keep - step;
                let i_dup = t_dup - step;

                // A jump target at step > 0 means a block boundary here.
                // (step == 0 is always allowed: terminators can be targets.)
                if step > 0 && (jump_targets.contains(&i_keep) || jump_targets.contains(&i_dup)) {
                    break;
                }

                // Structural equality check (requires PartialEq on Insn).
                if insns[i_keep] != insns[i_dup] {
                    break;
                }

                // Do not include instructions with jump offsets in the tail.
                if has_jump_offset(&insns[i_keep]) {
                    break;
                }

                tail_len += 1;
            }

            if tail_len < MIN_TAIL {
                continue;
            }

            let dup_start = t_dup - tail_len + 1;
            let keep_start = t_keep - tail_len + 1;

            // Guard: none of the *interior* duplicate-side tail indices
            // [dup_start .. t_dup) may be jump targets — removing those
            // instructions would orphan the incoming jump.  t_dup itself is
            // rewritten to Jump(keep_start), not removed, so it is safe for
            // t_dup to be a target (the jump threads straight to the survivor
            // tail on the next pass_thread_jumps invocation).
            if (dup_start..t_dup).any(|i| jump_targets.contains(&i)) {
                continue;
            }

            // Degenerate: identical starting positions.
            if keep_start == dup_start {
                continue;
            }

            // A shared PC can only have one active zero-cost exception handler.
            // Require both instructions to be reachable and their complete
            // outer→inner handler stacks to match.  Comparing only the
            // innermost handler is insufficient: a re-raise can unwind to an
            // outer entry after the inner handler has been consumed.
            if (0..tail_len).any(|step| {
                match (
                    &handler_stacks[t_keep - step],
                    &handler_stacks[t_dup - step],
                ) {
                    (Some(keep_stack), Some(dup_stack)) => keep_stack != dup_stack,
                    _ => true,
                }
            }) {
                continue;
            }

            // Issue #2420 and PEP 657: the survivor copy can hold only one
            // source location.  Refuse a merge when any corresponding
            // instruction has a different effective line or direct caret span.
            if (0..tail_len).any(|step| {
                eff_line[t_keep - step] != eff_line[t_dup - step]
                    || cols[t_keep - step] != cols[t_dup - step]
            }) {
                continue;
            }

            // Apply the merge.
            //
            // Mark [dup_start .. t_dup) as removed; replace the terminator at
            // t_dup with Jump(keep_start - t_dup - 1) so execution falls into
            // the survivor tail.  `compact` rewrites all offsets.
            let raw_offset = keep_start as i64 - t_dup as i64 - 1;
            if raw_offset < i32::MIN as i64 || raw_offset > i32::MAX as i64 {
                continue; // offset overflow — skip (degenerate huge function)
            }

            let mut keep = vec![true; n];
            for i in dup_start..t_dup {
                keep[i] = false;
            }
            let mut transformed = insns;
            transformed[t_dup] = Insn::Jump(raw_offset as i32);
            // Compact the parallel lineno slice with the same `keep` mask so the
            // lines stay 1:1 with the instructions across the fixed-point loop.
            // `compact` only removes entries here (no jump offsets to rewrite).
            let new_linenos: Vec<u32> = linenos
                .iter()
                .zip(keep.iter())
                .filter_map(|(&ln, &k)| k.then_some(ln))
                .collect();
            let new_cols: Vec<crate::ast::CaretSpan> = cols
                .iter()
                .zip(keep.iter())
                .filter_map(|(&span, &k)| k.then_some(span))
                .collect();
            return (compact(transformed, &keep), new_linenos, new_cols, true);
        }
    }

    (insns, linenos, cols, false)
}
