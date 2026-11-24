// ─── LCS alignment & opcode eligibility predicates for source mapping ─────────

/// Lineno-only wrapper over [`remap_lineno_and_col_tables`].  Retained for the
/// unit tests that pin the greedy forward-scan contract (issue #1962/#2002);
/// the production path calls the combined remapper directly.
#[cfg(test)]
fn remap_linenos(old_insns: &[Insn], old_linenos: &[u32], new_insns: &[Insn]) -> Vec<u32> {
    remap_lineno_and_col_tables(old_insns, old_linenos, &[], new_insns).0
}

/// True for opcodes the optimizer fuses **from a generic `BinOp`** (issue
/// #2411): `BinOpConst` / `BinOpImm` (one constant operand) and the
/// `CmpJump*Const` comparison-jump fusions.  Their PEP 657 caret anchor must be
/// recovered from the originating `BinOp`'s column span, since the fused opcode
/// has no counterpart in the pre-optimization stream.  Aug-assign variants
/// (`is_aug`) come from `BinOpInPlace`, not a raising rvalue `BinOp`, and carry
/// no rvalue caret — excluded.
fn insn_is_fused_binop(insn: &Insn) -> bool {
    matches!(
        insn,
        Insn::BinOpConst(_, _, _, _, false) | Insn::BinOpImm(_, _, _, _, false)
    )
}

/// Opcodes eligible for the discriminant-only line fallback in
/// [`remap_lineno_and_col_tables`].  These are the side-effecting / error-raising
/// instructions whose source line surfaces in tracebacks, and which the optimizer
/// **never synthesizes from scratch** — it only ever rewrites their register
/// operands (`pass_copy_prop`, `pass_const_reg_prop`) or moves them.  So when one
/// fails the exact-structural match (because its operands were renumbered), there
/// is guaranteed to be a 1:1 origin of the same opcode still present in the old
/// stream; anchoring to it recovers the true line (issue #2439).
///
/// Conversely, opcodes the optimizer *can* create out of thin air (`LoadConst`
/// from const-folding, `Move`/`Jump`/`LoadNone` from peepholes, fused
/// `BinOpConst`/`CmpJump*`, …) are deliberately excluded: a synthesized one has no
/// origin, and matching it to an unrelated same-opcode occurrence would attribute
/// a misleading line.  Those keep the conservative running-prefix fallback, which
/// preserves the #1962/#2002 greedy-scan contract.
fn insn_anchor_by_discriminant(insn: &Insn) -> bool {
    matches!(
        insn,
        Insn::GetAttr(..)
            | Insn::GetAttrForWith(..)
            | Insn::SetAttr(..)
            | Insn::DeleteAttr(..)
            | Insn::GetItem(..)
            | Insn::GetSlice(..)
            | Insn::SetItem(..)
            | Insn::DeleteItem(..)
            | Insn::BinOp(..)
            | Insn::BinOpInPlace(..)
            | Insn::UnaryOp(..)
            | Insn::Call(..)
            | Insn::CallMemo(..)
            | Insn::CallKw { .. }
            | Insn::CallMethod { .. }
            | Insn::CallMethodExpanded { .. }
            | Insn::CallMethodKw { .. }
            | Insn::RaiseValue(_)
            | Insn::RaiseExceptStarResidual(_)
            | Insn::RaiseFrom(..)
            | Insn::RaiseAssert(_)
            | Insn::GetIter(..)
            | Insn::ForIter(..)
            | Insn::GetAwaitable(..)
            | Insn::YieldFrom { .. }
            | Insn::Yield { .. }
    )
}

/// Order-preserving alignment of `new_insns` onto `old_insns` by structural
/// identity (issue #2432).  Returns one entry per new instruction: `Some(p)` if
/// it is matched to old position `p`, `None` if it has no match (an
/// optimizer-synthesized instruction, or one whose duplicates were all consumed
/// by other new instructions).  The chosen positions are strictly increasing in
/// new-index order, so a later new instruction can never claim an old position
/// before one already claimed by an earlier new instruction — the exact failure
/// the greedy forward scan exhibited on repeated identical instructions.
///
/// Each new instruction is reduced to **at most one** candidate old position by
/// occurrence rank: the j-th occurrence of a value `V` in the new stream pairs
/// only with the j-th occurrence of `V` in the old stream.  Because the
/// optimized stream is a subsequence of the original, equal instructions can only
/// be matched diagonally (the k-th surviving copy descends from the k-th
/// original copy), so this single-candidate reduction yields the same alignment
/// the duplicate-aware LCS would — without offering every (new, old) occurrence
/// pair to the solver, which is `O(N²)` when many instructions are identical
/// (e.g. a module of repeated `raise`/`x = 1 + 2` statements, exactly the class
/// issue #2432 is about).
///
/// The reduced single candidates are then fed, in **new-index** order, to a
/// patience solitaire over old positions; the longest strictly-increasing run is
/// the LCS, and back-pointers recover which new index each chosen old position
/// belongs to.  Cost is `O(N · log N)` regardless of how many instructions
/// repeat.
fn lcs_align(
    old_insns: &[Insn],
    new_insns: &[Insn],
    positions: &HashMap<&Insn, Vec<usize>>,
) -> Vec<Option<usize>> {
    let mut aligned = vec![None; new_insns.len()];
    if old_insns.is_empty() || new_insns.is_empty() {
        return aligned;
    }

    // Patience solitaire over old positions.  `piles[t]` holds the smallest old
    // position that ends an increasing subsequence of length `t + 1`.  For each
    // candidate we also remember which (new_index, old_position) produced it and
    // a back-pointer into the previous pile, so the LCS can be reconstructed.
    let mut piles: Vec<usize> = Vec::new(); // tail old-position per pile (monotone)
    // Parallel record per *placement*: (new_idx, old_pos, prev_record_idx).
    let mut records: Vec<(usize, usize, Option<usize>)> = Vec::new();
    // `pile_record[t]` = index into `records` of the current tail of pile `t`.
    let mut pile_record: Vec<usize> = Vec::new();

    // Per-value total occurrence count in the new stream.
    //
    // When `new_count == old_count` (every copy of `V` survived), occurrence
    // rank pairs the j-th new copy 1:1 with `locs[j]` in O(1) time (preserving
    // the O(N log N) bound for issue #2432).
    //
    // When `new_count < old_count` (an earlier copy was folded or fused away,
    // leaving `slack = old_count - new_count` dropped copies), hard-wiring rank
    // `j` to `locs[j]` can force a later surviving copy onto an earlier dropped
    // copy's old index and collapse the entire LCS interval before it (#2905).
    // For bounded slack (`slack <= 16`), Hunt-Szymanski offers every feasible
    // diagonal candidate `locs[rank ..= rank + slack]` in strictly decreasing
    // order so patience solitaire finds the true global LCS in O(N log N) time.
    //
    // For `Insn::BinOp` with `locs.len() > 1 && new_count < locs.len()`, we
    // still leave it unmatched in LCS: two structurally identical `BinOp`s can
    // differ in their preceding `LoadConst` constant slot (`b // 2` vs
    // `b // (1 - 1)`), which raw structural equality cannot distinguish; the
    // constant-aware `pin_binop_in_interval` matcher resolves them in order.
    const MAX_LCS_SLACK: usize = 16;
    // Only count new occurrences for instructions that actually repeat in
    // `old_insns` (`locs.len() > 1`); unique old instructions always have
    // `slack == 0` and need no map entry.
    let mut new_counts: HashMap<&Insn, usize> = HashMap::new();
    for insn in new_insns {
        if positions.get(insn).is_some_and(|locs| locs.len() > 1) {
            *new_counts.entry(insn).or_insert(0) += 1;
        }
    }
    let mut seen: HashMap<&Insn, usize> = HashMap::new();

    for (new_idx, new_insn) in new_insns.iter().enumerate() {
        let Some(locs) = positions.get(new_insn) else {
            continue;
        };
        let new_count = if locs.len() > 1 {
            new_counts.get(new_insn).copied().unwrap_or(0)
        } else {
            1
        };
        if matches!(new_insn, Insn::BinOp(..)) && locs.len() > 1 && new_count < locs.len() {
            continue;
        }
        let rank = {
            let entry = seen.entry(new_insn).or_insert(0);
            let r = *entry;
            *entry += 1;
            r
        };
        if rank >= locs.len() {
            // More new copies than old copies of this value (a synthesized
            // duplicate): leave it unmatched for the fallbacks below.
            continue;
        }
        let slack = locs.len().saturating_sub(new_count);
        let hi_rank =
            if !matches!(new_insn, Insn::LoadConst(..)) && slack > 0 && slack <= MAX_LCS_SLACK {
                (rank + slack).min(locs.len() - 1)
            } else {
                rank
            };
        // A `LoadConst(r, _)` immediately feeding `BinOp(_, _, op, r)` in the
        // optimized stream is typically synthesized by constant folding (e.g.
        // `b // (2 - 2)` → `LoadConst(t, 0); BinOp(_, b, FloorDiv, t)`).  Never
        // align it to an unrelated `LoadConst(r, _)` from a different operator
        // (e.g. `if b > 0:` whose `LoadConst(t, 0)` was fused into `CmpJump`),
        // or the spurious LCS anchor jumps `old_pos` past the `BinOp` (#2905).
        let next_binop_rhs_op = match (new_insn, new_insns.get(new_idx + 1)) {
            (Insn::LoadConst(r, _), Some(Insn::BinOp(_, _, op, rhs))) if r == rhs => {
                Some((*r, *op))
            }
            _ => None,
        };
        for &old_pos in locs[rank..=hi_rank].iter().rev() {
            if let Some((r, op)) = next_binop_rhs_op {
                let compatible = matches!(
                    old_insns.get(old_pos + 1),
                    Some(Insn::BinOp(_, _, old_op, old_rhs)) if *old_op == op && *old_rhs == r
                );
                if !compatible {
                    continue;
                }
            }
            // Pile to extend: first pile whose tail is `>= old_pos` (strictly
            // increasing subsequence).
            let t = piles.partition_point(|&tail| tail < old_pos);
            let prev = if t == 0 {
                None
            } else {
                Some(pile_record[t - 1])
            };
            let rec_idx = records.len();
            records.push((new_idx, old_pos, prev));
            if t == piles.len() {
                piles.push(old_pos);
                pile_record.push(rec_idx);
            } else {
                piles[t] = old_pos;
                pile_record[t] = rec_idx;
            }
        }
    }

    // Reconstruct the LCS by following back-pointers from the last pile's tail.
    if let Some(&last) = pile_record.last() {
        let mut cur = Some(last);
        while let Some(idx) = cur {
            let (new_idx, old_pos, prev) = records[idx];
            aligned[new_idx] = Some(old_pos);
            cur = prev;
        }
    }

    aligned
}
