// ─── Loop-Invariant Code Motion (LICM) ────────────────────────────────────────

/// Hoist loop-invariant pure instructions out of loop bodies to just before the
/// loop header.
///
/// ## What is hoisted
///
/// Only instructions that are definitely free of observable side effects and
/// write to a temporary register (`dst >= num_locals`):
/// - `LoadConst(dst, idx)` — loop-invariant when `dst` is a temp.  Named
///   locals (`dst < num_locals`) must not be hoisted: a zero-trip loop must
///   not assign them.
///
/// ## What is NOT hoisted
///
/// Arithmetic, protocol dispatch, calls, attribute/item operations, namespace
/// operations, and all loop/branch/exception instructions are left in place
/// because they may be observable or depend on the iteration context.
///
/// ## Loop detection
///
/// A back edge is any `Jump(k)` whose target is at or before the current
/// instruction.  Multiple back edges may target the same header: an early one
/// can be a `continue`, while the last one is the loop latch.  Those edges are
/// combined into one complete `[header_pc, last_latch_pc]` interval so LICM
/// always computes the write set over the whole compiler-shaped loop body.
/// Nested loops are handled individually: the inner loop's interval is hoisted
/// just before its own header, not before the outer header.
///
/// ## Exception handlers
///
/// If `SetupExcept` or `PopExcept` appears anywhere inside `[header, latch]` the
/// entire loop is skipped — hoisting across exception regions is not safe.
///
/// ## Fixed-point iteration
///
/// The pass repeats the hoist loop until no new instructions are moved, so that
/// an instruction whose invariant inputs were themselves just hoisted can also be
/// hoisted in the same call.
fn collect_complete_loop_intervals(insns: &[Insn]) -> Vec<(usize, usize)> {
    let mut intervals: Vec<(usize, usize)> = Vec::new();
    let mut interval_index_by_header: HashMap<usize, usize> = HashMap::new();

    for (i, insn) in insns.iter().enumerate() {
        if let Insn::Jump(k) = insn {
            let target = (i as i64 + 1 + *k as i64) as usize;
            if target <= i {
                if let Some(&interval_index) = interval_index_by_header.get(&target) {
                    intervals[interval_index].1 = i;
                } else {
                    interval_index_by_header.insert(target, intervals.len());
                    intervals.push((target, i));
                }
            }
        }
    }

    intervals
}

fn pass_licm(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Collect each loop's complete interval as (header_pc, last_latch_pc).
    // A Jump(k) at position i is a back edge when the target (i+1+k) <= i.
    //
    // `continue` is also a backward Jump to the loop header, but its source can
    // occur in the middle of the body.  Treating that source as the loop latch
    // truncates the write set and can make a reused temporary look like it has
    // only one writer.  Keep only the last back edge for each header so the
    // interval covers the whole compiler-shaped loop body.
    let mut back_edges = collect_complete_loop_intervals(&insns);

    if back_edges.is_empty() {
        return insns;
    }

    // Process nested loops from the smallest interval outward.  This preserves
    // the existing fixed-point behaviour where an inner invariant may first
    // move to the outer body and then out of the outer loop.
    back_edges.sort_unstable_by_key(|(header, latch)| latch - header);

    // Work on a mutable copy; `hoist` marks which positions to move out.
    let mut insns = insns;

    for (header, latch) in &back_edges {
        let (header, latch) = (*header, *latch);

        // Skip loops that contain exception handling — hoisting across
        // SetupExcept/PopExcept is not safe.
        let has_except = insns[header..=latch]
            .iter()
            .any(|i| matches!(i, Insn::SetupExcept(_) | Insn::PopExcept));
        if has_except {
            continue;
        }

        // Fixed-point: keep hoisting until nothing new moves.
        //
        // `body_start` tracks the current start of the loop body after successive
        // rounds of hoisting.  Each round moves some instructions to the pre-header
        // block, so the actual loop body shrinks from the front.  `body_end` (=latch)
        // never changes because instructions after the latch are not touched.
        let mut body_start = header;

        loop {
            // Rebuild the write count map for the current loop body [body_start..=latch].
            // `write_count[r]` = number of instructions in the body that write `r`.
            // We need counts (not just a set) so we can check whether a candidate
            // instruction is the *sole* writer of its destination — a necessary
            // condition for safe hoisting.
            let mut write_count: HashMap<u32, usize> = HashMap::new();
            for insn in &insns[body_start..=latch] {
                let mut tmp: HashSet<u32> = HashSet::new();
                collect_writes(insn, &mut tmp);
                for r in tmp {
                    *write_count.entry(r).or_insert(0) += 1;
                }
            }
            // Find the "safe hoist boundary": the exclusive upper bound of the
            // straight-line prefix of the loop body that is guaranteed to execute
            // on every iteration.
            //
            // Starting from `body_start` (the loop header, e.g. ForIter), we
            // advance past the header itself and then scan for the first
            // *additional* conditional branch.  Instructions strictly before that
            // branch are always executed — they dominate the back edge — so they
            // are safe to hoist regardless of runtime values.  Instructions at or
            // after a conditional branch are only executed on some iterations.
            //
            // The ForIter loop header is an implicit conditional (it exits when
            // the iterator is exhausted), but it is not included in the hoist
            // set; we advance past it first.
            let hoist_bound = {
                // Start just after the loop header.
                let mut bound = body_start + 1;
                for pc in (body_start + 1)..=latch {
                    match &insns[pc] {
                        // Unconditional jump is safe to pass through (it's the
                        // back edge or a structural jump, not a branch).
                        Insn::Jump(_) => {}
                        // Any conditional jump ends the safe prefix.
                        Insn::JumpIfFalse(..)
                        | Insn::JumpIfTrue(..)
                        | Insn::CmpJumpIfFalse(..)
                        | Insn::CmpJumpIfTrue(..)
                        | Insn::CmpJumpIfFalseConst(..)
                        | Insn::CmpJumpIfTrueConst(..)
                        | Insn::ForIter(..) => {
                            bound = pc;
                            break;
                        }
                        _ => {
                            bound = pc + 1; // extend bound to include this instruction
                        }
                    }
                }
                bound
            };

            // Collect indices (in order) of instructions to hoist.
            // Only consider instructions strictly within [body_start .. hoist_bound).
            // These are the instructions that dominate the back edge (guaranteed to
            // execute every iteration) and are pure (LoadConst for temps).
            let mut to_hoist: Vec<usize> = Vec::new();
            for pc in body_start..hoist_bound {
                if is_loop_invariant(&insns[pc], &write_count, num_locals) {
                    to_hoist.push(pc);
                }
            }

            if to_hoist.is_empty() {
                break; // fixed-point reached — nothing new to hoist
            }

            // Strategy: reorder instructions so that the hoisted ones appear at
            // [body_start .. body_start+num_hoisted) — i.e. they slide to the
            // very beginning of the current body — and the remaining body follows
            // at [body_start+num_hoisted .. latch+1).  Instructions before
            // `body_start` and after `latch` are untouched (offsets still rewritten).
            let num_hoisted = to_hoist.len();
            let hoist_set: HashSet<usize> = to_hoist.iter().copied().collect();

            // Build old→new index map (size n+1 for the past-the-end sentinel).
            //
            // `placement_map` says where each old instruction physically lands —
            // hoisted ones move to the pre-header region, non-hoisted body
            // instructions slide down by `num_hoisted`.
            //
            // `jump_target_map` is what `rewrite_offsets` consults when fixing
            // jump targets.  It differs from `placement_map` only for hoisted
            // positions: a jump that originally pointed *at* a hoisted
            // instruction (because that instruction was the first body
            // statement, and the compiler emitted the body-entry jump to its
            // address) must be redirected to the new body entry — i.e. the
            // first non-hoisted instruction at-or-after the old target — not
            // to the hoisted instruction's pre-header slot.  Without this
            // distinction a `CmpJumpIfFalse(_, _, _, k)` whose body started
            // with a hoistable LoadConst would, after LICM, loop back forever
            // into the pre-header.  See issue #323.
            let mut placement_map = vec![0usize; n + 1];
            let mut jump_target_map = vec![0usize; n + 1];

            // Before-body region: indices unchanged.
            for i in 0..body_start {
                placement_map[i] = i;
                jump_target_map[i] = i;
            }
            // Non-hoisted body slots first, so we know where the body now
            // starts and where jumps targeting hoisted instructions should
            // redirect.
            {
                let mut slot = body_start + num_hoisted;
                for pc in body_start..=latch {
                    if !hoist_set.contains(&pc) {
                        placement_map[pc] = slot;
                        jump_target_map[pc] = slot;
                        slot += 1;
                    }
                }
            }
            // Hoisted instructions land at [body_start .. body_start+num_hoisted).
            // Their jump-target redirection is to the first non-hoisted body
            // instruction at-or-after the old position.
            //
            // Compute the redirection for each hoisted pc by scanning forward
            // from pc to the first non-hoisted instruction in the body.  If
            // none exists (everything from pc to latch was hoisted, which is
            // unreachable since the back-edge Jump at latch isn't hoistable),
            // fall back to the slot just past the loop.
            for (i, &pc) in to_hoist.iter().enumerate() {
                placement_map[pc] = body_start + i;
                let redirect = ((pc + 1)..=latch)
                    .find(|p| !hoist_set.contains(p))
                    .map(|p| jump_target_map[p])
                    .unwrap_or(latch + 1);
                jump_target_map[pc] = redirect;
            }
            // After-latch region: indices unchanged.
            for i in (latch + 1)..n {
                placement_map[i] = i;
                jump_target_map[i] = i;
            }
            // Past-the-end sentinel.
            placement_map[n] = n;
            jump_target_map[n] = n;

            // Scatter instructions into their new positions and fix jump offsets.
            let mut new_insns: Vec<Insn> = vec![Insn::ReturnNone; n];
            for (old_i, insn) in insns.iter().enumerate() {
                let new_i = placement_map[old_i];
                new_insns[new_i] =
                    rewrite_offsets_with(insn.clone(), old_i, &placement_map, &jump_target_map);
            }
            insns = new_insns;

            // Advance body_start past the just-hoisted instructions: they now live
            // at [old_body_start .. old_body_start+num_hoisted) and are no longer
            // part of the loop body.
            body_start += num_hoisted;

            // Loop again: re-examine the updated body for newly invariant insns
            // whose source registers were themselves just hoisted.
        }
    }

    insns
}

/// Returns `true` if `insn` is a pure, loop-invariant instruction.
///
/// An instruction is loop-invariant when:
/// 1. It is a `LoadConst`.
/// 2. Its *destination* register is a temporary (`>= num_locals`).  Named
///    locals (index `< num_locals`) are visible outside the loop; hoisting a
///    write to a named local would make the assignment unconditional, which is
///    wrong for zero-trip loops.
/// 3. Its *destination* register is written only by this instruction inside the
///    loop body (`write_count[dst] == 1`).  If another instruction in the body
///    also writes `dst`, hoisting would change the value seen by instructions
///    that execute between the hoist point and the in-body write — incorrect.
///
/// Arithmetic is deliberately excluded even when one operand is constant.
/// Without an exact built-in-type proof, `BinOpConst` / `BinOpImm` can dispatch
/// a user dunder, raise, inspect the live frame, or mutate a shared namespace.
/// Hoisting it would change both the number and timing of those observations,
/// including executing the operation once for a zero-trip loop.
fn is_loop_invariant(insn: &Insn, write_count: &HashMap<u32, usize>, num_locals: u32) -> bool {
    // True when `dst` is a temporary register (not a named local).
    let is_temp = |dst: u32| dst >= num_locals;
    // True when `dst` is the sole writer of that register inside the body.
    let sole_writer = |dst: u32| write_count.get(&dst).copied().unwrap_or(0) == 1;

    match insn {
        // LoadConst has no register source; safe to hoist only when dst is a
        // temporary.  Named locals must not be hoisted: a zero-trip loop must
        // not assign them.
        Insn::LoadConst(dst, _) => is_temp(*dst) && sole_writer(*dst),
        // Everything else: not hoisted.
        _ => false,
    }
}
