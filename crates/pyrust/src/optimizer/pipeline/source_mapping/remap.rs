// ─── Combined lineno and PEP 657 caret-span table remapper ────────────────────

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

    // Index every old instruction by structural identity → ascending positions.
    let mut positions: HashMap<&Insn, Vec<usize>> = HashMap::new();
    for (i, insn) in old_insns.iter().enumerate() {
        positions.entry(insn).or_default().push(i);
    }

    // Secondary index keyed by opcode discriminant only (registers/operands
    // erased), used as a fallback when structural equality fails (issue #2439).
    let mut disc_positions: HashMap<std::mem::Discriminant<Insn>, Vec<usize>> = HashMap::new();
    for (i, insn) in old_insns.iter().enumerate() {
        disc_positions
            .entry(std::mem::discriminant(insn))
            .or_default()
            .push(i);
    }

    // Ascending old positions of the generic `BinOp` opcode (issue #2411).
    let old_binop_positions: Vec<usize> = old_insns
        .iter()
        .enumerate()
        .filter_map(|(i, insn)| matches!(insn, Insn::BinOp(..)).then_some(i))
        .collect();
    let new_binop_origin_count = origin_new_insns
        .iter()
        .filter(|i| matches!(i, Insn::BinOp(..)) || insn_is_fused_binop(i))
        .count();
    let binop_recovery_sound = new_binop_origin_count >= old_binop_positions.len();
    let mut binop_col_cursor: usize = 0;

    let binop_derived_origin = build_binop_derived_origin(old_insns, origin_new_insns);
    let old_binop_infos = build_old_binop_infos(old_insns, &old_binop_positions);
    let old_notjump_operand_cols = build_old_notjump_operand_cols(old_insns, old_cols);
    let new_not_unaryop_survives = origin_new_insns
        .iter()
        .any(|i| matches!(i, Insn::UnaryOp(_, crate::ast::UnaryOp::Not, _)));
    let notjump_recovery_sound = !old_notjump_operand_cols.is_empty() && !new_not_unaryop_survives;
    let mut notjump_col_cursor: usize = 0;

    // Discriminants whose anchored occurrences disagree (#2411) and those whose
    // diagonal recovery is sound because every old copy survived (#2442).
    let mut disc_ambiguous: HashSet<std::mem::Discriminant<Insn>> = HashSet::new();
    let mut disc_diag_sound: HashSet<std::mem::Discriminant<Insn>> = HashSet::new();
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

    // Order-preserving optimal alignment of `new_insns` onto `old_insns` (#2432).
    let aligned = lcs_align(old_insns, new_insns, &positions);

    // For each `new_idx`, the LCS anchor interval `[prev_aligned_old[new_idx],
    // next_aligned_old[new_idx])` in `old_insns`.
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

    let suffix_binop_old_pos = if want_cols && !binop_recovery_sound && new_binop_origin_count > 0 {
        build_suffix_binop_old_pos(
            old_insns,
            &old_prefix,
            old_cols,
            origin_new_insns,
            &old_binop_positions,
            new_binop_origin_count,
            &prev_aligned_old,
            &next_aligned_old,
        )
    } else {
        HashMap::new()
    };

    let mut old_pos: usize = 0;
    let mut linenos = Vec::with_capacity(new_insns.len());
    let mut cols = vec![(0, 0, 0, 0); new_insns.len()];
    for (new_idx, ((out_col, new_insn), aligned_pos)) in
        cols.iter_mut().zip(new_insns).zip(&aligned).enumerate()
    {
        match *aligned_pos {
            Some(i) => {
                linenos.push(old_linenos.get(i).copied().unwrap_or(0));
                if want_cols {
                    *out_col = old_cols.get(i).copied().unwrap_or((0, 0, 0, 0));
                }
                old_pos = i + 1;
            }
            None => {
                let interval_hi = next_aligned_old[new_idx];
                let interval_new_hi = next_aligned_new[new_idx];
                let binop_pinned = if let Insn::BinOp(d, l, op, _) = new_insn {
                    pin_binop_in_interval(
                        &old_binop_infos,
                        new_insns,
                        new_idx,
                        old_pos,
                        interval_hi,
                        interval_new_hi,
                        *d,
                        *l,
                        *op,
                        new_binop_const_idx(new_insns, new_idx, new_insn),
                    )
                } else {
                    None
                };
                let disc_matched = if let Some(i) = binop_pinned {
                    Some((i, true))
                } else if let Insn::BinOp(_, _, op, _) = new_insn {
                    let new_c = new_binop_const_idx(new_insns, new_idx, new_insn);
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
                                    &old_binop_infos,
                                    new_insns,
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
                                    &old_binop_infos,
                                    new_insns,
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
                        let fused_origin = fused_origin
                            .or_else(|| suffix_binop_old_pos.get(&new_idx).copied())
                            .or_else(|| {
                                (insn_is_fused_binop(new_insn) || is_cmp_jump_insn(new_insn))
                                    .then(|| binop_derived_origin.get(&new_idx).copied())
                                    .flatten()
                            });
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
