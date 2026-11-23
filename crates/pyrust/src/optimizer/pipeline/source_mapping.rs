/// Lineno-only wrapper over [`remap_lineno_and_col_tables`].  Retained for the
/// unit tests that pin the greedy forward-scan contract (issue #1962/#2002);
/// the production path calls the combined remapper directly.
#[cfg(test)]
fn remap_linenos(old_insns: &[Insn], old_linenos: &[u32], new_insns: &[Insn]) -> Vec<u32> {
    remap_lineno_and_col_tables(old_insns, old_linenos, &[], new_insns).0
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
        // optimized stream is typically synthesized by `pass_const_fold` (e.g.
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

/// Remap both the `lineno_table` and the PEP 657 `col_table` (issue #2426) from
/// an old instruction sequence to a new one in a **single shared scan**.
///
/// The lineno half aligns the optimized stream onto the original by an
/// order-preserving longest-common-subsequence match (`lcs_align`, issue
/// #2432): each matched `new_insn` inherits the line of the `old_insn` it pairs
/// with, falling back to the running prefix line for optimizer-created
/// instructions.  LCS replaced the old greedy forward scan, which mis-attributed
/// the lines of two structurally-identical instructions (e.g. two
/// `raise ValueError(str)` in one module frame) when the monotonic cursor
/// skipped the first occurrence.  Error-raising instructions (BinOp, Call,
/// GetAttr, …) are never removed by the optimizer (they have side effects), so
/// their line numbers — and now their anchors — are preserved.
///
/// The col half reuses the very same match: a matched new instruction inherits
/// `old_cols[i]`.  Two safety rules uphold "a wrong caret is worse than no
/// caret": optimizer-created (unmatched) instructions get no anchor, and an
/// instruction whose anchored occurrences disagree also gets none.  Cross-jump
/// now rejects different column spans before merging; the ambiguity guard
/// remains a conservative fallback for every other rewrite (#2431).
///
/// Pass an empty `old_cols` (the `remap_linenos` wrapper does) to skip all col
/// work; the returned col vector is then all-`(0, 0)` at zero extra cost.
fn remap_lineno_and_col_tables(
    old_insns: &[Insn],
    old_linenos: &[u32],
    old_cols: &[crate::ast::CaretSpan],
    new_insns: &[Insn],
) -> (Vec<u32>, Vec<crate::ast::CaretSpan>) {
    remap_lineno_and_col_tables_with_source_prefix(
        old_insns,
        old_linenos,
        old_cols,
        new_insns,
        new_insns.len(),
    )
}

/// Remap source tables while excluding an optimizer-appended suffix from every
/// source-origin proof and match.
///
/// The suffix still receives a stable fallback line so table lengths remain
/// aligned with bytecode, but it receives no PEP 657 caret and cannot consume
/// an old instruction occurrence in the LCS/discriminant recovery. This is
/// required for int-loop versioning, whose fast copy deliberately duplicates
/// generic source instructions.
fn remap_lineno_and_col_tables_with_source_prefix(
    old_insns: &[Insn],
    old_linenos: &[u32],
    old_cols: &[crate::ast::CaretSpan],
    new_insns: &[Insn],
    source_prefix_len: usize,
) -> (Vec<u32>, Vec<crate::ast::CaretSpan>) {
    let full_new_len = new_insns.len();
    debug_assert!(
        source_prefix_len <= full_new_len,
        "source prefix {source_prefix_len} exceeds instruction count {full_new_len}"
    );
    let source_prefix_len = source_prefix_len.min(full_new_len);
    let new_insns = &new_insns[..source_prefix_len];

    if old_linenos.is_empty() {
        return (vec![0u32; full_new_len], vec![(0, 0, 0, 0); full_new_len]);
    }
    // Only do col work when at least one anchor was recorded.
    let want_cols = !old_cols.is_empty() && old_cols.iter().any(|&c| c != (0, 0, 0, 0));
    let origin_new_insns = new_insns;

    // Build a "current best lineno" prefix from old_linenos: for each position
    // in old_insns, the last non-zero lineno seen up to and including that position.
    let mut running: u32 = 0;
    let old_prefix: Vec<u32> = old_linenos
        .iter()
        .map(|&ln| {
            if ln != 0 {
                running = ln;
            }
            running
        })
        .collect();

    // Index every old instruction by structural identity → ascending positions
    // (see the original `remap_linenos` rationale: keeps the scan linear-ish and
    // byte-identical to a naive forward re-scan).
    let mut positions: HashMap<&Insn, Vec<usize>> = HashMap::new();
    for (i, insn) in old_insns.iter().enumerate() {
        positions.entry(insn).or_default().push(i);
    }

    // Secondary index keyed by **opcode discriminant only** (registers/operands
    // erased), so an instruction whose register operands were rewritten by a
    // renumbering pass (`pass_copy_prop`, `pass_const_reg_prop`, …) can still be
    // anchored to its origin line when full structural equality no longer holds
    // (issue #2439: a bare `x.foo` whose `GetAttr` operand was renumbered fell
    // through to the previous statement's line).  Used only as a fallback when the
    // exact-match lookup fails, so the #1962/#2002 exact-match contract is
    // unchanged for instructions the optimizer left byte-identical.
    let mut disc_positions: HashMap<std::mem::Discriminant<Insn>, Vec<usize>> = HashMap::new();
    for (i, insn) in old_insns.iter().enumerate() {
        disc_positions
            .entry(std::mem::discriminant(insn))
            .or_default()
            .push(i);
    }

    // Ascending old positions of the generic `BinOp` opcode (issue #2411): a
    // binary expression that raises is emitted as `BinOp` by the compiler, but
    // the optimizer routinely fuses it into `BinOpConst` / `BinOpImm` / a
    // `CmpJump*` when an operand is a constant.  Those fused opcodes don't exist
    // in the old stream, so they get no structural / discriminant match and
    // would lose their caret anchor.  Fusion is a 1:1 replacement of the `BinOp`
    // (it only also consumes a preceding `LoadConst`), so the surviving fused
    // op descends, in order, from the old `BinOp` at-or-after the cursor — a
    // monotone scan over these positions recovers its origin column.
    let old_binop_positions: Vec<usize> = old_insns
        .iter()
        .enumerate()
        .filter_map(|(i, insn)| matches!(insn, Insn::BinOp(..)).then_some(i))
        .collect();
    // The diagonal recovery is only sound when every old `BinOp` survives (as a
    // plain `BinOp` or a fused `BinOpConst`/`BinOpImm`) in order — i.e. nothing
    // was *folded away*.  Const-folding (`(10+2)*3` → `36`) collapses nested
    // binops, breaking the 1:1 origin mapping and risking a caret on the wrong
    // sub-expression.  Count the binop-origin ops surviving in the new stream;
    // if fewer than the old count, a fold happened — disable the fused-binop col
    // recovery so a fused op stays caret-free (a missing caret beats a wrong
    // one, per #2426).  Plain renumbered `BinOp`s still recover via the exact /
    // discriminant match paths above, unaffected by this guard.
    let new_binop_origin_count = origin_new_insns
        .iter()
        .filter(|i| matches!(i, Insn::BinOp(..)) || insn_is_fused_binop(i))
        .count();
    let binop_recovery_sound = new_binop_origin_count >= old_binop_positions.len();
    // Cursor into `old_binop_positions`, advanced monotonically as fused ops are
    // matched (mirrors the `old_pos` discipline of the main scan).
    let mut binop_col_cursor: usize = 0;

    let binop_origin_op = |insn: &Insn| -> Option<crate::ast::BinaryOp> {
        match insn {
            Insn::BinOp(_, _, op, _) => Some(*op),
            Insn::BinOpConst(_, _, op, _, false) | Insn::BinOpImm(_, _, op, _, false) => Some(*op),
            _ => None,
        }
    };

    // Exact 1:1 origin map for *binop-derived* opcodes (issue #2889).  Both
    // fusions rewrite a `BinOp` into an opcode that exists nowhere in the old
    // stream — `pass_binop_const_fusion` into `BinOpConst`/`BinOpImm`, comparison
    // -jump fusion into a `Cmp*Jump*` — so neither the structural nor the
    // discriminant match can anchor it, and the recoveries above cover only some
    // shapes: the monotone cursor is disabled as soon as a comparison became a
    // `Cmp*Jump*` (that drops the surviving binop count below the old one), and
    // the `(dst, lhs)` pin needs a *unique* candidate, which a loop body reusing
    // one scratch temp rarely has.  A fused op with no origin falls back to the
    // running prefix and reports the enclosing statement's line — after #2889
    // made fusion fire inside loop bodies, that is a raising `b // 0` statement
    // blaming its `while` header.
    //
    // Both fusions are 1:1 replacements that preserve source order and the
    // operator, so the old and new binop-derived opcodes can be paired greedily
    // in order.  Every old op must find a partner (otherwise a fold dropped one
    // and the alignment is guesswork); the only new ops allowed to go unpaired
    // are comparison jumps, which loop inversion and the int-loop version guard
    // legitimately duplicate from one source comparison.  Any other surplus
    // means the streams are not a 1:1 rewrite of each other and the whole map is
    // abandoned (a missing anchor beats a wrong one, #2426).
    let binop_derived_op = |insn: &Insn| -> Option<crate::ast::BinaryOp> {
        match insn {
            Insn::BinOp(_, _, op, _)
            | Insn::BinOpConst(_, _, op, _, false)
            | Insn::BinOpImm(_, _, op, _, false)
            | Insn::CmpJumpIfFalse(_, op, _, _)
            | Insn::CmpJumpIfTrue(_, op, _, _)
            | Insn::CmpJumpIfFalseConst(_, op, _, _)
            | Insn::CmpJumpIfTrueConst(_, op, _, _)
            | Insn::CountCmpJumpTrue(_, op, _, _, _)
            | Insn::CountCmpJumpFalse(_, op, _, _, _) => Some(*op),
            _ => None,
        }
    };
    let is_cmp_jump = |insn: &Insn| {
        matches!(
            insn,
            Insn::CmpJumpIfFalse(..)
                | Insn::CmpJumpIfTrue(..)
                | Insn::CmpJumpIfFalseConst(..)
                | Insn::CmpJumpIfTrueConst(..)
                | Insn::CountCmpJumpTrue(..)
                | Insn::CountCmpJumpFalse(..)
        )
    };
    let old_derived: Vec<(usize, crate::ast::BinaryOp)> = old_insns
        .iter()
        .enumerate()
        .filter_map(|(i, insn)| binop_derived_op(insn).map(|op| (i, op)))
        .collect();
    let new_derived: Vec<(usize, crate::ast::BinaryOp)> = origin_new_insns
        .iter()
        .enumerate()
        .filter_map(|(i, insn)| binop_derived_op(insn).map(|op| (i, op)))
        .collect();
    let mut binop_derived_origin: HashMap<usize, usize> = HashMap::new();
    {
        let mut ni = 0usize;
        let mut aligned_ok = true;
        for &(old_i, op) in &old_derived {
            while ni < new_derived.len() && new_derived[ni].1 != op {
                if !is_cmp_jump(&origin_new_insns[new_derived[ni].0]) {
                    aligned_ok = false;
                    break;
                }
                ni += 1;
            }
            if !aligned_ok || ni == new_derived.len() {
                aligned_ok = false;
                break;
            }
            binop_derived_origin.insert(new_derived[ni].0, old_i);
            ni += 1;
        }
        if !aligned_ok
            || new_derived[ni.min(new_derived.len())..]
                .iter()
                .any(|&(new_i, _)| !is_cmp_jump(&origin_new_insns[new_i]))
        {
            binop_derived_origin.clear();
        }
    }

    // Precomputed metadata for each old `BinOp(dst, lhs, op, rhs)` (issues
    // #2580 / #2889 / #2905):
    //   - `old_pos`: index in `old_insns`
    //   - `dst`, `lhs`, `op`: original operands and operator
    //   - `rhs_const`: if `old_insns[p - 1]` is `LoadConst(rhs, c_idx)`, that
    //     exact constant pool slot `Some(c_idx)` (recall `pass_compact_consts`
    //     runs *after* this remapper, so constant slots are still un-reindexed).
    //
    // `pass_binop_const_fusion` and `pass_const_reg_prop` rewrite a `BinOp` to
    // `BinOpConst(d, l, op, c, false)` without changing `op`, preserving `l`
    // and `c` (unless a sub-expression folded into a *new* constant slot, in
    // which case `rhs_const` on the old `BinOp` is `None`).  `pass_copy_prop`
    // may subsequently retarget `d` onto a local (`Move(local, d)`).
    //
    // Using `(d, l, op, c)` → `(l, op, c)` → `(d, l, op)` → `(l, op)` bounded
    // to the current LCS anchor interval `[old_pos, next_aligned_old[new_idx])`
    // recovers both the source line and PEP 657 caret even when a constant fold
    // elsewhere in the function disables the global 1:1 count guards — while
    // strictly forbidding cross-constant mismatches (`rhs_const == Some(other)`
    // with `other != c`) and ambiguous multi-matches (#2426).
    #[derive(Clone, Copy)]
    struct OldBinOpInfo {
        pos: usize,
        dst: u32,
        lhs: u32,
        op: crate::ast::BinaryOp,
        rhs_const: Option<u16>,
    }
    let old_binop_infos: Vec<OldBinOpInfo> = old_binop_positions
        .iter()
        .filter_map(|&p| {
            let Insn::BinOp(dst, lhs, op, rhs) = old_insns[p] else {
                return None;
            };
            let rhs_const = p.checked_sub(1).and_then(|prev| match old_insns[prev] {
                Insn::LoadConst(r, c) if r == rhs => Some(c),
                _ => None,
            });
            Some(OldBinOpInfo {
                pos: p,
                dst,
                lhs,
                op,
                rhs_const,
            })
        })
        .collect();

    // Old fusion sites for the `not`-invert peephole (issue #2588): a position `i`
    // where `UnaryOp(_, Not, src)` is immediately followed by a conditional jump on
    // that same register.  `pass_not_invert` collapses the pair into a single
    // `JumpIfTrue`/`JumpIfFalse(src, …)` — eliminating the `UnaryOp(Not)`, so the
    // surviving jump exists in neither sense nor register-shape in the old stream
    // and gets no structural / discriminant match (a conditional jump is not in the
    // `insn_anchor_by_discriminant` set), losing its caret.  CPython 3.12 anchors
    // the bool-converting jump at the *operand* span (`if not B():` → `^` under
    // `B()`, not `not B()`), which is the caret the compiler armed on the operand's
    // value-producing instruction — emitted (post-order) immediately before the
    // `UnaryOp(Not)`, i.e. at `i - 1`.  Record that operand col per fusion site,
    // keyed by the jump's old position `i + 1`, so a monotone scan recovers each
    // fused jump's anchor, mirroring the fused-binop recovery above.
    let old_notjump_operand_cols: Vec<(usize, (u32, u32, u32, u32))> = old_insns
        .iter()
        .enumerate()
        .filter_map(|(i, insn)| {
            let next = old_insns.get(i + 1)?;
            match (insn, next) {
                (
                    Insn::UnaryOp(r, crate::ast::UnaryOp::Not, _),
                    Insn::JumpIfFalse(c, _) | Insn::JumpIfTrue(c, _),
                ) if *r == *c => {
                    let operand_col = i
                        .checked_sub(1)
                        .and_then(|j| old_cols.get(j).copied())
                        .unwrap_or((0, 0, 0, 0));
                    Some((i + 1, operand_col))
                }
                _ => None,
            }
        })
        .collect();
    // Sound only when every old fusion site was actually fused — i.e. no
    // `UnaryOp(_, Not, _)` survives in the new stream.  `pass_not_invert` either
    // fuses a site (dropping the `not`) or leaves it untouched (the `UnaryOp(Not)`
    // then survives); requiring zero survivors guarantees the 1:1
    // site→fused-jump mapping the diagonal cursor relies on.
    let new_not_unaryop_survives = origin_new_insns
        .iter()
        .any(|i| matches!(i, Insn::UnaryOp(_, crate::ast::UnaryOp::Not, _)));
    let notjump_recovery_sound = !old_notjump_operand_cols.is_empty() && !new_not_unaryop_survives;
    // Cursor into `old_notjump_operand_cols` (mirrors `binop_col_cursor`).
    let mut notjump_col_cursor: usize = 0;

    // Discriminants (opcode-only) whose anchored occurrences disagree: used by the
    // discriminant-match fallback below to refuse a col anchor when the same
    // opcode carries different spans across the old stream (#2411).
    let mut disc_ambiguous: HashSet<std::mem::Discriminant<Insn>> = HashSet::new();
    if want_cols {
        for (disc, locs) in &disc_positions {
            let first = old_cols.get(locs[0]).copied().unwrap_or((0, 0, 0, 0));
            if locs
                .iter()
                .any(|&p| old_cols.get(p).copied().unwrap_or((0, 0, 0, 0)) != first)
            {
                disc_ambiguous.insert(*disc);
            }
        }
    }

    // Discriminants whose diagonal recovery is sound even when `disc_ambiguous`
    // (issue #2442).  A side-effecting opcode (`GetAttr`, `GetItem`, …) is never
    // deleted or folded by the optimizer — only renumbered — so its k-th
    // surviving copy descends, in order, from the k-th original copy.  When every
    // old occurrence of the opcode still appears in the new stream (count
    // preserved), the monotone `partition_point` scan in the discriminant
    // fallback pairs each renumbered op with its true 1:1 origin, so the
    // per-occurrence span at that position is correct even though the opcode as a
    // whole carries differing spans.  Without this, an `a.b.c` chain — whose inner
    // `GetAttr b` is renumbered by copy-prop and whose two `GetAttr`s have
    // distinct spans (so `GetAttr` is `disc_ambiguous`) — loses the inner caret.
    // If a fold dropped a copy the count guard fails and the conservative
    // `disc_ambiguous` refusal stands (a missing caret beats a wrong one, #2426).
    let mut disc_diag_sound: HashSet<std::mem::Discriminant<Insn>> = HashSet::new();
    if want_cols {
        let mut new_disc_counts: HashMap<std::mem::Discriminant<Insn>, usize> = HashMap::new();
        for insn in origin_new_insns {
            if insn_anchor_by_discriminant(insn) {
                *new_disc_counts
                    .entry(std::mem::discriminant(insn))
                    .or_default() += 1;
            }
        }
        for (disc, locs) in &disc_positions {
            if new_disc_counts.get(disc).copied().unwrap_or(0) >= locs.len() {
                disc_diag_sound.insert(*disc);
            }
        }
    }

    // Order-preserving optimal alignment of `new_insns` onto `old_insns`
    // (issue #2432).  The historical scan used a single monotonic cursor and, for
    // each new instruction, took the *first* structurally-equal old occurrence at
    // or after the cursor.  With two structurally-identical instructions
    // (e.g. two `raise ValueError(str)` in one module frame) an earlier new
    // instruction whose only candidate sat *after* the first occurrence could
    // advance the cursor past it, so a later new instruction was forced onto the
    // wrong (second) occurrence — flipping the two raises' source lines.
    //
    // Since the optimized stream is (modulo a handful of synthesized
    // instructions) a subsequence of the original, the correct mapping is the
    // longest common subsequence by structural identity.  `lcs_align` computes,
    // for each new instruction, the old position it aligns to under an optimal
    // order-preserving matching — repeated identical instructions are paired up
    // 1:1 in source order instead of greedily, which is exactly what fixes the
    // cross-attribution.  Unmatched new instructions (`None`) fall through to the
    // discriminant / running-prefix fallbacks below, anchored to a cursor derived
    // from the surrounding aligned positions so those stay stable too.
    let aligned = lcs_align(old_insns, new_insns, &positions);

    // For each `new_idx`, the LCS anchor interval `[prev_aligned_old[new_idx],
    // next_aligned_old[new_idx])` in `old_insns`.  Any unmatched instruction
    // between two LCS anchors originates strictly inside that interval.
    let mut prev_aligned_old = vec![0usize; new_insns.len()];
    {
        let mut prev_old = 0usize;
        for idx in 0..new_insns.len() {
            prev_aligned_old[idx] = prev_old;
            if let Some(p) = aligned[idx] {
                prev_old = p + 1;
            }
        }
    }
    let mut next_aligned_old = vec![old_insns.len(); new_insns.len()];
    let mut next_aligned_new = vec![new_insns.len(); new_insns.len()];
    {
        let mut next_old = old_insns.len();
        let mut next_new = new_insns.len();
        for idx in (0..new_insns.len()).rev() {
            next_aligned_old[idx] = next_old;
            next_aligned_new[idx] = next_new;
            if let Some(p) = aligned[idx] {
                next_old = p;
                next_new = idx;
            }
        }
    }

    // Constant-slot compatibility guard between a new binop-origin op and an
    // old `BinOp`: an old `BinOp` whose RHS came directly from
    // `LoadConst(_, old_c)` cannot become a fused or const-RHS op with a
    // different constant slot `new_c` (`pass_compact_consts` has not reindexed
    // slots yet).  An old `BinOp` whose RHS was a computed expression
    // (`rhs_const == None`) CAN receive a newly const-folded slot `new_c`
    // (e.g. `b // (1 - 1)`).
    let new_binop_const_idx = |new_idx: usize, insn: &Insn| -> Option<u16> {
        match *insn {
            Insn::BinOpConst(_, _, _, c, false) => Some(c),
            Insn::BinOp(_, _, _, rhs) => {
                new_idx
                    .checked_sub(1)
                    .and_then(|prev| match new_insns[prev] {
                        Insn::LoadConst(r, c) if r == rhs => Some(c),
                        _ => None,
                    })
            }
            _ => None,
        }
    };
    let old_binop_const_at = |old_p: usize| -> Option<u16> {
        let Insn::BinOp(_, _, _, rhs) = old_insns.get(old_p)? else {
            return None;
        };
        old_p.checked_sub(1).and_then(|prev| match old_insns[prev] {
            Insn::LoadConst(r, c) if r == *rhs => Some(c),
            _ => None,
        })
    };
    let const_slots_compatible = |new_c: Option<u16>, old_c: Option<u16>| match (new_c, old_c) {
        (Some(c), Some(oc)) => c == oc,
        _ => true,
    };

    // Suffix / left-spine recovery for the *folded* case (issues #2577 / #2578 /
    // #2586).  When a const-foldable sub-expression collapses (`a + b` with
    // `a`,`b` known → `LoadConst`), `binop_recovery_sound` is false and the
    // forward cursor is disabled — yet the surviving raising op (`(a+b) + "s"`,
    // `"s" + (x+2)`, `(a+b) + "s" + (c+d)`) still has a valid origin col in the
    // old stream and must keep its caret.
    //
    // Every candidate pair `(new_i, old_p)` must additionally stay inside
    // `new_i`'s LCS anchor interval `[prev_aligned_old[new_i],
    // next_aligned_old[new_i])` and satisfy `const_slots_compatible` so a
    // folded later statement (e.g. `c1 = 10; c2 = 20; c3 = c1 + c2`) can never
    // steal the line or caret of an earlier surviving binop (`res = s + 1`,
    // issue #2905).
    let mut suffix_binop_old_pos: HashMap<usize, usize> = HashMap::new();
    if want_cols && !binop_recovery_sound && new_binop_origin_count > 0 {
        let old_start = old_binop_positions.len() - new_binop_origin_count;
        let surviving: Vec<(usize, crate::ast::BinaryOp)> = origin_new_insns
            .iter()
            .enumerate()
            .filter_map(|(i, insn)| binop_origin_op(insn).map(|op| (i, op)))
            .collect();
        let pair_valid = |new_i: usize, old_p: usize| -> bool {
            prev_aligned_old[new_i] <= old_p
                && old_p < next_aligned_old[new_i]
                && const_slots_compatible(
                    new_binop_const_idx(new_i, &origin_new_insns[new_i]),
                    old_binop_const_at(old_p),
                )
        };
        let ops_match = surviving.iter().enumerate().all(|(j, &(new_i, op))| {
            old_binop_positions.get(old_start + j).is_some_and(|&p| {
                old_insns.get(p).and_then(binop_origin_op) == Some(op) && pair_valid(new_i, p)
            })
        });
        let outer_anchor = old_binop_positions.last().and_then(|&p| {
            let start = old_cols.get(p)?.0;
            let lineno = old_prefix.get(p).copied().unwrap_or(0);
            Some((start, lineno))
        });
        let left_spine = outer_anchor.is_some_and(|(start, lineno)| {
            (0..surviving.len()).all(|j| {
                old_binop_positions.get(old_start + j).is_some_and(|&p| {
                    old_cols.get(p).map(|span| span.0) == Some(start)
                        && old_prefix.get(p).copied().unwrap_or(0) == lineno
                })
            })
        });
        if ops_match && left_spine {
            for (j, &(new_i, _)) in surviving.iter().enumerate() {
                suffix_binop_old_pos.insert(new_i, old_binop_positions[old_start + j]);
            }
        } else if let Some((start, lineno)) = outer_anchor {
            let spine_old: Vec<usize> = old_binop_positions
                .iter()
                .copied()
                .filter(|&p| {
                    old_cols.get(p).map(|span| span.0) == Some(start)
                        && old_prefix.get(p).copied().unwrap_or(0) == lineno
                })
                .collect();
            let spine_ops_match = spine_old.len() == surviving.len()
                && surviving.iter().enumerate().all(|(j, &(new_i, op))| {
                    spine_old.get(j).is_some_and(|&p| {
                        old_insns.get(p).and_then(binop_origin_op) == Some(op)
                            && pair_valid(new_i, p)
                    })
                });
            if spine_ops_match {
                for (j, &(new_i, _)) in surviving.iter().enumerate() {
                    suffix_binop_old_pos.insert(new_i, spine_old[j]);
                }
            }
        }
    }

    // Helper: pin a fused `BinOpConst` / `BinOpImm` (or renumbered `BinOp`)
    // within the current LCS anchor interval `[lo, hi)`.  Returns `Some(old_p)`
    // only when a sound, unambiguous tier uniquely identifies the origin (or
    // when an exact 1:1 count-matched diagonal exists within the interval).
    let pin_binop_in_interval = |new_idx: usize,
                                 lo: usize,
                                 hi: usize,
                                 new_hi: usize,
                                 dst: u32,
                                 lhs: u32,
                                 op: crate::ast::BinaryOp,
                                 const_idx: Option<u16>|
     -> Option<usize> {
        let start = old_binop_infos.partition_point(|info| info.pos < lo);
        let end = old_binop_infos.partition_point(|info| info.pos < hi);
        let slice = &old_binop_infos[start..end];
        if slice.is_empty() {
            return None;
        }
        // Compatibility guard: an old `BinOp` whose RHS came directly from
        // `LoadConst(_, old_c)` cannot have fused into a different constant slot
        // `new_c` (`pass_compact_consts` has not reindexed slots yet).  An old
        // `BinOp` whose RHS was a computed expression (`rhs_const == None`) CAN
        // fuse into a newly const-folded slot `new_c` (e.g. `b // (1 - 1)`).
        let const_compatible = |info: &OldBinOpInfo| match (const_idx, info.rhs_const) {
            (Some(c), Some(old_c)) => c == old_c,
            _ => true,
        };
        let unique = |pred: &dyn Fn(&OldBinOpInfo) -> bool| -> Option<usize> {
            let mut found = None;
            for info in slice {
                if pred(info) {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(info.pos);
                }
            }
            found
        };

        // Tier 1: exact `(dst, lhs, op, c)` match.
        if let Some(c) = const_idx
            && let Some(p) = unique(&|info| {
                info.dst == dst && info.lhs == lhs && info.op == op && info.rhs_const == Some(c)
            })
        {
            return Some(p);
        }
        // Tier 2: exact `(lhs, op, c)` match (survives `pass_copy_prop` retargeting `dst`).
        if let Some(c) = const_idx
            && let Some(p) =
                unique(&|info| info.lhs == lhs && info.op == op && info.rhs_const == Some(c))
        {
            return Some(p);
        }
        // Tier 3: `(dst, lhs, op)` with compatible constant slot (covers folded RHS
        // like `b // (1 - 1)` and `BinOpImm` / renumbered `BinOp`).
        if let Some(p) = unique(&|info| {
            info.dst == dst && info.lhs == lhs && info.op == op && const_compatible(info)
        }) {
            return Some(p);
        }
        // Tier 4: `(lhs, op)` with compatible constant slot (folded RHS + copy-propped `dst`).
        if let Some(p) = unique(&|info| info.lhs == lhs && info.op == op && const_compatible(info))
        {
            return Some(p);
        }
        // Tier 5: exact 1:1 count-matched diagonal for repeated identical `(dst, lhs, op, c)`
        // within the same interval `[new_idx, new_hi)`.
        if let Some(c) = const_idx {
            let old_matches: Vec<usize> = slice
                .iter()
                .filter(|info| {
                    info.dst == dst && info.lhs == lhs && info.op == op && info.rhs_const == Some(c)
                })
                .map(|info| info.pos)
                .collect();
            if old_matches.len() > 1 {
                let new_matches = new_insns[new_idx..new_hi]
                    .iter()
                    .filter(|insn| {
                        matches!(
                            insn,
                            Insn::BinOpConst(nd, nl, nop, nc, false)
                                if *nd == dst && *nl == lhs && *nop == op && *nc == c
                        )
                    })
                    .count();
                if new_matches == old_matches.len() {
                    return Some(old_matches[0]);
                }
            }
        }
        None
    };

    let mut old_pos: usize = 0;
    let mut linenos = Vec::with_capacity(new_insns.len());
    let mut cols = vec![(0, 0, 0, 0); new_insns.len()];
    for (new_idx, ((out_col, new_insn), aligned_pos)) in
        cols.iter_mut().zip(new_insns).zip(&aligned).enumerate()
    {
        // Use the LCS-aligned old position when this new instruction was matched;
        // it is guaranteed `>= old_pos`, preserving the monotonic-cursor invariant
        // the fallbacks below rely on.
        let matched = *aligned_pos;
        match matched {
            Some(i) => {
                linenos.push(old_linenos.get(i).copied().unwrap_or(0));
                // LCS alignment pinned this new instruction to the *specific*
                // old position `i` (repeated identical instructions are paired
                // 1:1 in source order — see `lcs_align`), so `old_cols[i]` is the
                // exact per-occurrence anchor and is always safe to carry.  The
                // only-fallback `disc_ambiguous` guard — opcodes whose
                // structurally-identical copies carry differing spans — must NOT
                // gate this branch: there the true origin is unknown, but here
                // the LCS match *is* the origin (issue #2570: a chained subscript
                // `d['a']['b']['c']`'s inner `GetItem`s collapse to identical
                // register operands after copy-prop; a per-opcode ambiguity guard
                // dropped every caret past the first, even though each `GetItem`
                // was LCS-aligned to its own distinct origin).
                if want_cols {
                    *out_col = old_cols.get(i).copied().unwrap_or((0, 0, 0, 0));
                }
                old_pos = i + 1;
            }
            None => {
                // No byte-identical old instruction at-or-after the cursor.  Before
                // falling back to the running prefix line, try to anchor by opcode
                // discriminant alone — but ONLY for the line-critical side-effecting
                // opcodes the optimizer never synthesizes (`GetAttr`, `GetItem`,
                // `Call`, `BinOp`, `RaiseValue`, …).  A renumbering pass
                // (`pass_copy_prop`, `pass_const_reg_prop`) may have rewritten such an
                // instruction's register operands so it no longer matches its origin
                // structurally, yet the opcode is never deleted and still sits at the
                // same relative position; matching it here recovers its true source
                // line (issue #2439: a bare `x.foo` whose `GetAttr` was renumbered
                // reported the previous statement's line).  Restricting to opcodes the
                // optimizer cannot create keeps a genuinely synthesized instruction
                // (a const-folded `LoadConst`, a peephole `Move`/`Jump`) on the
                // running-prefix line — preserving the #1962/#2002 contract for those.
                //
                // For a renumbered `BinOp`, prefer the interval-bounded `(d, l, op)`
                // pin before falling back to discriminant order — otherwise a `BinOp`
                // after an earlier fused `BinOpConst` can claim the earlier op's old
                // `BinOp` slot (#2905).
                let interval_hi = next_aligned_old[new_idx];
                let interval_new_hi = next_aligned_new[new_idx];
                let binop_pinned = if let Insn::BinOp(d, l, op, _) = new_insn {
                    pin_binop_in_interval(
                        new_idx,
                        old_pos,
                        interval_hi,
                        interval_new_hi,
                        *d,
                        *l,
                        *op,
                        new_binop_const_idx(new_idx, new_insn),
                    )
                } else {
                    None
                };
                let disc_matched = if let Some(i) = binop_pinned {
                    Some((i, true))
                } else if let Insn::BinOp(_, _, op, _) = new_insn {
                    let new_c = new_binop_const_idx(new_idx, new_insn);
                    let start = old_binop_infos.partition_point(|info| info.pos < old_pos);
                    old_binop_infos[start..]
                        .iter()
                        .take_while(|info| info.pos < interval_hi)
                        .find(|info| {
                            info.op == *op && const_slots_compatible(new_c, info.rhs_const)
                        })
                        .map(|info| (info.pos, false))
                } else if insn_anchor_by_discriminant(new_insn) {
                    disc_positions
                        .get(&std::mem::discriminant(new_insn))
                        .and_then(|locs| {
                            let k = locs.partition_point(|&p| p < old_pos);
                            locs.get(k)
                                .copied()
                                .filter(|&p| p < interval_hi)
                                .map(|p| (p, false))
                        })
                } else {
                    None
                };
                match disc_matched {
                    Some((i, exact_pin)) => {
                        linenos.push(old_linenos.get(i).copied().unwrap_or(0));
                        let disc = std::mem::discriminant(new_insn);
                        if want_cols
                            && (exact_pin
                                || !disc_ambiguous.contains(&disc)
                                || disc_diag_sound.contains(&disc))
                        {
                            *out_col = old_cols.get(i).copied().unwrap_or((0, 0, 0, 0));
                        }
                        old_pos = i + 1;
                    }
                    None => {
                        // Optimizer-created instruction with no eligible opcode
                        // match.  For a fused binary op (`BinOpConst`/`BinOpImm`)
                        // recover the originating `BinOp`'s position — by a
                        // monotone scan over `old_binop_positions` (issue #2411),
                        // or, when a fold made that scan unsound, by the
                        // interval-bounded `(dst, lhs, op, c_idx)` pin (#2580 / #2905).
                        let fused_origin = if !insn_is_fused_binop(new_insn) {
                            None
                        } else if binop_recovery_sound {
                            while binop_col_cursor < old_binop_positions.len()
                                && old_binop_positions[binop_col_cursor] < old_pos
                            {
                                binop_col_cursor += 1;
                            }
                            let found = old_binop_positions
                                .get(binop_col_cursor)
                                .copied()
                                .filter(|&p| p < interval_hi);
                            if found.is_some() {
                                binop_col_cursor += 1;
                            }
                            found
                        } else {
                            match *new_insn {
                                Insn::BinOpConst(d, l, op, c, false) => pin_binop_in_interval(
                                    new_idx,
                                    old_pos,
                                    interval_hi,
                                    interval_new_hi,
                                    d,
                                    l,
                                    op,
                                    Some(c),
                                ),
                                Insn::BinOpImm(d, l, op, _, false) => pin_binop_in_interval(
                                    new_idx,
                                    old_pos,
                                    interval_hi,
                                    interval_new_hi,
                                    d,
                                    l,
                                    op,
                                    None,
                                ),
                                _ => None,
                            }
                        };
                        // Order of preference for fused binary ops:
                        //   1. Interval-bounded exact pin (`fused_origin`) — uses
                        //      register + operator + constant-slot identity inside
                        //      the current LCS interval.
                        //   2. Folded-statement suffix/left-spine alignment
                        //      (`suffix_binop_old_pos`, #2577/#2578/#2586) — recovers
                        //      folded single-expression chains even if copy-prop
                        //      renamed `lhs`.
                        //   3. Global binop-derived 1:1 diagonal (`binop_derived_origin`,
                        //      #2889) — last resort when no fold occurred and a loop
                        //      header comparison became `Cmp*Jump*`.
                        let fused_origin = fused_origin
                            .or_else(|| suffix_binop_old_pos.get(&new_idx).copied())
                            .or_else(|| {
                                insn_is_fused_binop(new_insn)
                                    .then(|| binop_derived_origin.get(&new_idx).copied())
                                    .flatten()
                            });
                        // Fusion is a 1:1 replacement of the old `BinOp` (it only
                        // additionally consumes the preceding `LoadConst`), so a
                        // pinned origin gives the fused op's true source line, not
                        // just its caret.  Without this a raising bare `a / b`
                        // statement reports the *previous* statement's line once it
                        // fuses — issue #2439's defect re-entered through the fused
                        // opcode, which #2889's precise fusion liveness makes
                        // reachable for ordinary straight-line code.  Everything
                        // else still approximates from the running prefix, keeping
                        // the #1962/#2002 contract.
                        linenos.push(
                            fused_origin
                                .and_then(|i| old_prefix.get(i).copied())
                                .unwrap_or_else(|| old_prefix.get(old_pos).copied().unwrap_or(0)),
                        );
                        if want_cols && let Some(i) = fused_origin {
                            *out_col = old_cols.get(i).copied().unwrap_or((0, 0, 0, 0));
                        }
                        if let Some(i) = fused_origin
                            && i >= old_pos
                            && i < interval_hi
                        {
                            old_pos = i + 1;
                        }
                        // Recover the caret anchor of a conditional jump synthesized
                        // from `UnaryOp(Not) + JumpIf*` by `pass_not_invert` (issue
                        // #2588), mirroring the fused-binop recovery above.  The
                        // anchor is the operand span recorded per old fusion site.
                        if want_cols
                            && notjump_recovery_sound
                            && matches!(new_insn, Insn::JumpIfFalse(..) | Insn::JumpIfTrue(..))
                        {
                            while notjump_col_cursor < old_notjump_operand_cols.len()
                                && old_notjump_operand_cols[notjump_col_cursor].0 < old_pos
                            {
                                notjump_col_cursor += 1;
                            }
                            if let Some(&(_, operand_col)) =
                                old_notjump_operand_cols.get(notjump_col_cursor)
                            {
                                *out_col = operand_col;
                                notjump_col_cursor += 1;
                            }
                        }
                    }
                }
            }
        }
        // Folded-statement suffix recovery (issues #2577 / #2578): a surviving
        // binary op whose foldable sibling collapsed and which had no caret
        // anchor yet (e.g. a plain surviving `BinOp` that was `disc_ambiguous`)
        // gets its origin col from the interval-checked suffix alignment above,
        // provided it matches the line already assigned to this instruction.
        if want_cols
            && *out_col == (0, 0, 0, 0)
            && let Some(&i) = suffix_binop_old_pos.get(&new_idx)
            && old_prefix.get(i).copied().unwrap_or(0) == linenos[new_idx]
        {
            *out_col = old_cols.get(i).copied().unwrap_or((0, 0, 0, 0));
        }
    }
    let suffix_line = linenos.last().copied().unwrap_or(0);
    linenos.resize(full_new_len, suffix_line);
    cols.resize(full_new_len, (0, 0, 0, 0));
    (linenos, cols)
}
