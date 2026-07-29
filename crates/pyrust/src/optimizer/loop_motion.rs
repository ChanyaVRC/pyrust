include!("loop_motion/int_loop_versioning.rs");

// ─── Exit-block inlining ───────────────────────────────────────────────────────

/// Replace an unconditional `Jump(k)` with the instruction it targets when that
/// target is a single-instruction terminal (`Return(r)` or `ReturnNone`).
///
/// ## Rationale
///
/// Compiled `if/else` branches often look like:
///
/// ```text
/// // true branch
/// LoadConst(r, 1)
/// Jump(k)           ← points to epilogue Return
/// // false branch
/// LoadConst(r, 0)
/// Jump(k)           ← same epilogue Return
/// // epilogue
/// Return(r)
/// ```
///
/// Replacing both Jumps with `Return(r)` directly — a 1-for-1 substitution that
/// leaves all other offsets intact — eliminates two taken branches.  The now-dead
/// epilogue `Return` is removed in the subsequent `pass_dead_code`.
///
/// Only single-instruction terminals are inlined to avoid shifting instruction
/// offsets (which would require a separate offset-fixup pass).
fn pass_exit_inline(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    insns
        .iter()
        .enumerate()
        .map(|(i, insn)| {
            if let Insn::Jump(k) = insn {
                let target = (i as i64 + 1 + *k as i64) as usize;
                if target < n
                    && target != i
                    && let t @ (Insn::Return(_) | Insn::ReturnNone) = &insns[target]
                {
                    return t.clone();
                }
            }
            insn.clone()
        })
        .collect()
}

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
            // execute every iteration) and are pure (LoadConst for temps, safe BinOpConst).
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

/// Collect all registers *written* (defined) by `insn` into `written`.
fn collect_writes(insn: &Insn, written: &mut HashSet<u32>) {
    use Insn::*;
    match insn {
        LoadConst(r, _)
        | LoadGlobal(r, _)
        | LoadCell(r, _)
        | LoadNone(r)
        | LoadExc(r)
        | LoadExcTraceback(r, _)
        | ImportModule(r, _)
        | MakeFunction(r, _, _, _, _, _)
        | MakeClass(r, _, _, _, _, _, _)
        | MakeClassMeta(r, _, _, _, _, _, _, _)
        | MakeTypeAlias(r, _, _, _)
        | MakeTypeVar(r, _)
        | BuildList(r, _, _)
        | BuildListReserve(r, _)
        | BuildTuple(r, _, _)
        | BuildString(r, _, _)
        | BuildSlice(r, _)
        | BuildDict(r, _, _)
        | BinOp(r, _, _, _)
        | BinOpInPlace(r, _, _, _)
        | BinOpConst(r, _, _, _, _)
        | CountCmpJumpTrue(r, _, _, _, _)
        | CountCmpJumpFalse(r, _, _, _, _)
        | CallInlineBinOp { dst: r, .. }
        | BinOpImm(r, _, _, _, _)
        | UnaryOp(r, _, _)
        | FormatValue(r, _)
        | FormatValueSpec(r, _, _)
        | MatchSeqExcluded(r, _)
        | MatchMapping(r, _)
        | GetAttr(r, _, _)
        | GetAttrForWith(r, _, _, _)
        | ImportFromAttr(r, _, _)
        | GetItem(r, _, _)
        | GetItemSeqOrExit(r, _, _, _)
        | GetSlice(r, _, _)
        | GetAwaitable(r, _)
        | Call(r, _)
        | CallMemo(r, _)
        | CallKw { func: r, .. }
        | CallEx { func: r, .. }
        | CallExArgs { func: r, .. }
        | Move(r, _)
        | CopyReg(r, _)
        | DeleteLocal(r, _) => {
            written.insert(*r);
        }
        MatchExceptStar(_, src, dst, _) => {
            written.insert(*src); // src_group is updated to remaining
            written.insert(*dst); // matched_dst gets the sub-group
        }
        LoadNoneRange { start, count } => {
            for i in 0..*count as u32 {
                written.insert(start + i);
            }
        }
        CallMethod { dst, .. }
        | CallMethodKw { dst, .. }
        | CallMethodExpanded { dst, .. }
        | Yield { dst, .. } => {
            written.insert(*dst);
        }
        YieldFrom {
            result_reg,
            sent_reg,
            ..
        } => {
            written.insert(*result_reg);
            // sent_reg is also written (to None before suspension and to the
            // sent value on each resume), so the optimizer must not assume it
            // retains its pre-YieldFrom value after this instruction.
            written.insert(*sent_reg);
        }
        ForIter(dst, _, _) => {
            written.insert(*dst);
        }
        Unpack(base, _, n) => {
            for i in 0..*n {
                written.insert(base + i);
            }
        }
        UnpackEx {
            dst_base,
            before,
            after,
            ..
        } => {
            for i in 0..(*before as u32 + 1 + *after) {
                written.insert(dst_base + i);
            }
        }
        Concat { dst, .. } => {
            written.insert(*dst);
        }
        // Instructions that don't write to any register.
        StoreGlobal(..)
        | StoreCell(..)
        | ImportStar(..)
        | SetAttr(..)
        | SetTypeVarAttr(..)
        | SetItem(..)
        | DeleteAttr(..)
        | DeleteItem(..)
        | DeleteName(..)
        | PushTypeParamEnv
        | PopTypeParamEnv
        | GetIter(..)
        | Jump(..)
        | JumpIfIterNotIntRange(..)
        | JumpIfIterNotIndexedSeq(..)
        | JumpIfIterNotIntRangeExact(..)
        | JumpIfFalse(..)
        | JumpIfTrue(..)
        | CmpJumpIfFalse(..)
        | CmpJumpIfTrue(..)
        | CmpJumpIfFalseConst(..)
        | CmpJumpIfTrueConst(..)
        | JumpIfNotInt(..)
        | Return(..)
        | ReturnNone
        | RaiseValue(..)
        | RaiseExceptStarResidual(..)
        | RaiseFrom(..)
        | RaiseReRaise
        | RaiseAssert(..)
        | RaiseAssertNoMsg
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | MatchExcept(..)
        | PushExcContext(..)
        | PopExcContext
        | CheckLocal(..)
        | PrintExpr(..)
        | SetAdd(..)
        | ListAppend(..)
        | ListExtend(..)
        | DictUpdate(..)
        | DictMergeKwCall { .. }
        | SetItemKwCall { .. }
        | RecordClassStore(..)
        | RecordClassDel(..)
        | SyncModuleGlobal(..)
        | DeleteModuleGlobal(..) => {}
        // MatchClassPositional writes dst_base..dst_base+n.
        MatchClassPositional { dst_base, n, .. } => {
            for i in 0..*n {
                written.insert(dst_base + i);
            }
        }
    }
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

// ─── Trivial no-op removal ─────────────────────────────────────────────────────

// Remove instructions that have no observable effect:
// - `Jump(0)` — offset 0 means the next instruction; equivalent to falling through
// - `Move(r, r)` — a register copied into itself
// ─── NOT-inversion ─────────────────────────────────────────────────────────────

/// Absorb `UnaryOp(r, Not, src)` into the following conditional jump by
///   inverting the branch sense, eliminating the boolean intermediate register.
///
/// ## Patterns
///
/// ```text
/// UnaryOp(r, Not, src) + JumpIfFalse(r, k)  →  JumpIfTrue(src, k)
/// UnaryOp(r, Not, src) + JumpIfTrue(r, k)   →  JumpIfFalse(src, k)
/// ```
///
/// ## Guards
/// - `r >= num_locals`: only fuse temp registers (named locals could be inspected
///   after the branch, e.g. in closures).
/// - `!reg_is_read_in(&insns[i+2..], r)`: `r` must be dead after the jump;
///   the liveness check reuses the existing `reg_is_read_in` helper.
/// - the conditional is not an incoming jump target: fallthrough from the
///   UnaryOp must be its only predecessor.
///
/// ## Correctness
/// `not x` returns `bool`; the branch only tests truthiness.  Because
/// `bool(not x)` has the same truthiness as `not x`, inverting the branch
/// and removing the `UnaryOp` is semantically equivalent.
///
/// Reference: Lua `lcode.c` `jumponcond()`.
fn pass_not_invert(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    use crate::ast::UnaryOp;

    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];
    // The trailing conditional may be a merge point reached from another
    // predecessor that did not execute this UnaryOp (notably the shared truth
    // test after a ternary).  Replacing that test with one of the UnaryOp's
    // source register would change the incoming path's condition.  Only fuse
    // when fallthrough from the UnaryOp is the conditional's sole entry.
    let mut jump_targets: HashSet<usize> = HashSet::new();
    for (idx, insn) in transformed.iter().enumerate() {
        if let Some(offset) = insn_jump_off(insn) {
            let target = idx as i64 + 1 + offset as i64;
            if target >= 0 && (target as usize) < n {
                jump_targets.insert(target as usize);
            }
        }
    }

    let mut i = 0;
    while i + 1 < n {
        if jump_targets.contains(&(i + 1)) {
            i += 1;
            continue;
        }
        let fused: Option<Insn> = match (&transformed[i], &transformed[i + 1]) {
            (Insn::UnaryOp(r, UnaryOp::Not, src), Insn::JumpIfFalse(cond, k))
                if *r == *cond
                    && *r >= num_locals
                    && !reg_is_read_in(&transformed[i + 2..], *r) =>
            {
                Some(Insn::JumpIfTrue(*src, *k))
            }
            (Insn::UnaryOp(r, UnaryOp::Not, src), Insn::JumpIfTrue(cond, k))
                if *r == *cond
                    && *r >= num_locals
                    && !reg_is_read_in(&transformed[i + 2..], *r) =>
            {
                Some(Insn::JumpIfFalse(*src, *k))
            }
            _ => None,
        };
        if let Some(new_insn) = fused {
            keep[i] = false;
            transformed[i + 1] = new_insn;
            i += 2;
        } else {
            i += 1;
        }
    }
    compact(transformed, &keep)
}

// ─── Dead store elimination ────────────────────────────────────────────────────

/// Is register `r` read by any instruction in `insns` before the first
/// instruction that writes `r`?  This is strictly more precise than
/// `reg_is_read_in` for dead-store analysis: it stops as soon as it sees a
/// write to `r`, because that write kills any previous value.
///
/// ## Control-flow caveat
///
/// The scan walks `insns` linearly.  Branching control-flow instructions
/// (Jump, JumpIf*, ForIter, SetupExcept, Yield, etc.) are treated as "read"
/// — i.e. conservatively keep the candidate store.  This is required for
/// correctness when the candidate store sits inside one arm of a branch
/// (e.g. a ternary's `then`-arm): the unconditional Jump that ends the arm
/// skips the other arm's write, so the "next write" found by a linear scan
/// does not actually kill the candidate value along the taken execution path.
/// Reads further down (after the branch merge) would otherwise see an unset
/// register.
///
/// *Terminating* instructions (Return, ReturnNone, Raise*) have no
/// fallthrough; after `insn_reads_reg` confirms they do not read `r`, we can
/// safely conclude `r` is dead and return `false`.
#[cfg(test)]
fn reg_is_read_before_next_write(insns: &[Insn], r: u32) -> bool {
    for insn in insns {
        if insn_reads_reg(insn, r) {
            return true;
        }
        // Terminating instructions: no fallthrough path can read r.
        if is_terminator(insn) {
            return false;
        }
        // Any other control-flow disruption invalidates the linear "next write
        // kills the value" reasoning — conservatively report a read so the
        // candidate store is preserved.
        if is_control_flow(insn) {
            return true;
        }
        // Stop at the next write to r (the old value is dead from here on).
        if writable_dst(insn) == Some(r) {
            return false;
        }
        if matches!(insn, Insn::LoadConst(dst, _) | Insn::LoadNone(dst) | Insn::LoadGlobal(dst, _)
                         | Insn::LoadCell(dst, _)
                         | Insn::Move(dst, _) | Insn::CopyReg(dst, _) if *dst == r)
        {
            return false;
        }
    }
    false
}

/// Returns `true` for instructions that unconditionally terminate the current
/// execution path with no fallthrough: `Return`, `ReturnNone`, and `Raise*`.
/// After checking `insn_reads_reg`, a terminator guarantees that no later
/// instruction in the linear sequence can read the candidate register.
fn is_terminator(insn: &Insn) -> bool {
    use Insn::*;
    matches!(
        insn,
        Return(..)
            | ReturnNone
            | RaiseAssert(..)
            | RaiseAssertNoMsg
            | RaiseValue(..)
            | RaiseFrom(..)
            | RaiseReRaise
    )
}

/// Does `insn` change control flow non-trivially?  Used by
/// `reg_is_read_before_next_write` to bail before incorrectly concluding
/// that a later write kills an earlier store along all paths.
fn is_control_flow(insn: &Insn) -> bool {
    use Insn::*;
    matches!(
        insn,
        Jump(..)
            | JumpIfNotInt(..)
            | JumpIfIterNotIntRange(..)
            | JumpIfIterNotIndexedSeq(..)
            | JumpIfIterNotIntRangeExact(..)
            | GetItemSeqOrExit(..)
            | CountCmpJumpTrue(..)
            | CountCmpJumpFalse(..)
            | CallInlineBinOp { .. }
            | JumpIfFalse(..)
            | JumpIfTrue(..)
            | CmpJumpIfFalse(..)
            | CmpJumpIfTrue(..)
            | CmpJumpIfFalseConst(..)
            | CmpJumpIfTrueConst(..)
            | ForIter(..)
            | Return(..)
            | ReturnNone
            | RaiseAssert(..)
            | RaiseAssertNoMsg
            | RaiseValue(..)
            | RaiseFrom(..)
            | RaiseReRaise
            | SetupExcept(..)
            | PopExcept
            | EndExcept
            | MatchExcept(..)
            | Yield { .. }
    )
}
