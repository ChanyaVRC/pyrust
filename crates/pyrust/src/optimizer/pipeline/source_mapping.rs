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

    // Per-value running occurrence counter for the new stream, so the j-th new
    // occurrence of a value selects the j-th old occurrence from `positions`.
    let mut seen: HashMap<&Insn, usize> = HashMap::new();

    for (new_idx, new_insn) in new_insns.iter().enumerate() {
        let Some(locs) = positions.get(new_insn) else {
            continue;
        };
        let rank = seen.entry(new_insn).or_insert(0);
        let old_pos = match locs.get(*rank) {
            Some(&p) => p,
            // More new copies than old copies of this value (a synthesized
            // duplicate): leave it unmatched for the fallbacks below.
            None => continue,
        };
        *rank += 1;
        {
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

    // Suffix recovery for the *folded* case (issues #2577 / #2578).  When a
    // const-foldable sub-expression collapses (`a + b` with `a`,`b` known →
    // `LoadConst`), `binop_recovery_sound` is false and the forward cursor above
    // is disabled — yet the surviving raising op (`(a+b) + "s"`, `"s" + (x+2)`)
    // still has a valid origin col in the old stream and must keep its caret.
    //
    // The optimizer's outermost binary op is emitted last (post-order) and folds
    // only ever *remove* binops, so the surviving binops are a subsequence of the
    // old binops in order.  When that subsequence is a contiguous **suffix** of
    // the old binops, the k-th-from-last surviving op descends 1:1 from the
    // k-th-from-last old binop.  We accept the suffix alignment only when the
    // operator sequence of the surviving new binops equals the operator sequence
    // of the last N old binops — a structural cross-check that fails for a
    // *middle* fold (`x + y + (a+b)`, where the surviving ops are not a suffix),
    // keeping such an op caret-free (a missing caret beats a wrong one, #2426).
    let binop_origin_op = |insn: &Insn| -> Option<crate::ast::BinaryOp> {
        match insn {
            Insn::BinOp(_, _, op, _) => Some(*op),
            Insn::BinOpConst(_, _, op, _, false) | Insn::BinOpImm(_, _, op, _, false) => Some(*op),
            _ => None,
        }
    };
    // Old positions where a per-op suffix col can be recovered, paired with the
    // surviving new binop index (set up below only when the suffix cross-check
    // passes); consumed by a flat lookup in the main scan.
    let mut suffix_binop_old_pos: HashMap<usize, usize> = HashMap::new();
    if want_cols && !binop_recovery_sound && new_binop_origin_count > 0 {
        let old_start = old_binop_positions.len() - new_binop_origin_count;
        let surviving: Vec<(usize, crate::ast::BinaryOp)> = origin_new_insns
            .iter()
            .enumerate()
            .filter_map(|(i, insn)| binop_origin_op(insn).map(|op| (i, op)))
            .collect();
        let ops_match = surviving.iter().enumerate().all(|(j, &(_, op))| {
            old_binop_positions
                .get(old_start + j)
                .and_then(|&p| old_insns.get(p))
                .and_then(binop_origin_op)
                == Some(op)
        });
        // The operator-sequence cross-check alone is too weak when the chain's
        // operators repeat (e.g. all `+`): a sibling-subtree fold to the *right*
        // of a surviving op (`(a+b) + "s" + (c+d)`, where `a+b` and `c+d` fold)
        // leaves survivors `{(a+b)+"s", outer}` whose true old indices are not a
        // contiguous suffix — yet every op is `+`, so `ops_match` passes and the
        // raising `(a+b)+"s"` would be mis-anchored to the folded `c+d`'s span.
        //
        // Require additionally that the mapped old binops form a **left spine**:
        // each shares the outermost op's `full_start` (the expression's left
        // edge).  Python `+`/`*`/… are left-associative, so the left-descendant
        // chain from the outermost op down all begins at that same column, while
        // any right-operand subtree (the source of the dangerous interspersed
        // fold) starts further right.  A non-left-spine alignment is rejected,
        // leaving the op caret-free (a missing caret beats a wrong one, #2426).
        let outer_full_start = old_binop_positions
            .last()
            .and_then(|&p| old_cols.get(p))
            .map(|span| span.0);
        let left_spine = outer_full_start.is_some_and(|start| {
            (0..surviving.len()).all(|j| {
                old_binop_positions
                    .get(old_start + j)
                    .and_then(|&p| old_cols.get(p))
                    .map(|span| span.0)
                    == Some(start)
            })
        });
        if ops_match && left_spine {
            for (j, &(new_i, _)) in surviving.iter().enumerate() {
                suffix_binop_old_pos.insert(new_i, old_binop_positions[old_start + j]);
            }
        } else if let Some(start) = outer_full_start {
            // Left-spine recovery for the *multi-fold* case (issue #2586).  When
            // two or more nested sub-expressions both const-fold, the surviving
            // binops are no longer a contiguous suffix of the old binops — an
            // interspersed folded right-subtree (`(a+b) + "s" + (c+d)`, where the
            // sibling `(a+b)` and `(c+d)` both fold) opens a gap, so the suffix
            // alignment above is rejected by the left-spine cross-check.  Yet the
            // raising op (`(a+b)+"s"`) and the outer op still carry valid origin
            // cols and must keep their carets.
            //
            // In a left-associative chain the surviving binops are exactly the
            // ops on the expression's **left spine**: the outermost op and its
            // repeated left-descendants, all anchored at the expression's left
            // edge (`full_start == outer_full_start`).  A right-operand subtree
            // always begins strictly to the right, so it never shares that
            // column.  Collect the old binops at the left-edge column, in order,
            // and pair them 1:1 with the survivors — but ONLY when their count
            // matches the survivor count exactly and the operator sequences agree.
            //
            // The count guard is what keeps this strictly safe (a missing caret
            // beats a wrong one, #2426): if a left-spine op itself folded, or a
            // right-subtree op survived, the left-edge old-binop count diverges
            // from the survivor count and the recovery is abandoned, leaving the
            // ops caret-free rather than risking a mis-anchored span.
            let spine_old: Vec<usize> = old_binop_positions
                .iter()
                .copied()
                .filter(|&p| old_cols.get(p).map(|span| span.0) == Some(start))
                .collect();
            let spine_ops_match = spine_old.len() == surviving.len()
                && surviving.iter().enumerate().all(|(j, &(_, op))| {
                    spine_old
                        .get(j)
                        .and_then(|&p| old_insns.get(p))
                        .and_then(binop_origin_op)
                        == Some(op)
                });
            if spine_ops_match {
                for (j, &(new_i, _)) in surviving.iter().enumerate() {
                    suffix_binop_old_pos.insert(new_i, spine_old[j]);
                }
            }
        }
    }

    // Per-destination-register index of the old `BinOp`s (issue #2580).  The
    // monotone `binop_col_cursor` scan above assumes the surviving fused op is the
    // *next* old `BinOp` in source order — true when fusion is a clean 1:1
    // replacement, but WRONG when const-folding *collapsed an inner* binop into a
    // constant: `"s" + (a + b)` (with `a`/`b` const-known) folds the inner `a + b`
    // away, leaving only the outer `"s" + 3` as a `BinOpConst`.  The surviving op
    // then descends from the *second* old `BinOp`, not the first the cursor points
    // at, so the count guard above disables the (now-misaligned) monotone recovery
    // entirely and the outer op loses its caret.  Fusion preserves the surviving
    // op's destination *and* register operand (folding an operand only rewrites the
    // const side), so its origin old `BinOp` can be pinned by the `(dst, lhs)`
    // register pair: when exactly one old `BinOp` at-or-after the cursor matches,
    // that is unambiguously the origin and its span is safe to carry.  `dst` alone
    // is too coarse — an inner and outer op of one expression routinely reuse the
    // same destination temp (`(a + b) + "s"`) — so the `lhs` register breaks the
    // tie.  Still "never a wrong caret": a non-unique match falls through to no
    // anchor.
    let mut old_binop_reg_positions: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    if want_cols {
        for &p in &old_binop_positions {
            if let Insn::BinOp(d, l, _, _) = old_insns[p] {
                old_binop_reg_positions.entry((d, l)).or_default().push(p);
            }
        }
    }

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
                // The col anchor (#2411) is carried from the *same* matched position
                // `i`: a side-effecting opcode is renumbered, never duplicated or
                // deleted, so the diagonally-aligned `i` is the instruction's true
                // origin — the very position whose line we trust here.  Trusting it
                // for the line but not the column would leave nearly every binary
                // op / subscript / call caret-free (their registers are routinely
                // renumbered), defeating the feature.  Only carry it when the
                // discriminant's anchored occurrences are unambiguous (or the
                // count-preserving `disc_diag_sound` diagonal recovery applies) —
                // here, unlike the LCS-matched branch above, the true 1:1 origin
                // is not known, so a per-opcode ambiguity must suppress the col.
                let disc_matched = if insn_anchor_by_discriminant(new_insn) {
                    disc_positions
                        .get(&std::mem::discriminant(new_insn))
                        .and_then(|locs| {
                            let k = locs.partition_point(|&p| p < old_pos);
                            locs.get(k).copied()
                        })
                } else {
                    None
                };
                match disc_matched {
                    Some(i) => {
                        linenos.push(old_linenos.get(i).copied().unwrap_or(0));
                        let disc = std::mem::discriminant(new_insn);
                        if want_cols
                            && (!disc_ambiguous.contains(&disc) || disc_diag_sound.contains(&disc))
                        {
                            *out_col = old_cols.get(i).copied().unwrap_or((0, 0, 0, 0));
                        }
                        old_pos = i + 1;
                    }
                    None => {
                        // Optimizer-created instruction with no eligible opcode match:
                        // approximate the line from the running prefix; line anchor
                        // stays on the running prefix.  For a fused binary op
                        // (`BinOpConst`/`BinOpImm`), recover the PEP 657 caret
                        // anchor from the originating `BinOp` via a monotone scan
                        // over `old_binop_positions` (issue #2411); col-only, so
                        // the #1962/#2002 line contract is untouched.
                        linenos.push(old_prefix.get(old_pos).copied().unwrap_or(0));
                        if want_cols && binop_recovery_sound && insn_is_fused_binop(new_insn) {
                            while binop_col_cursor < old_binop_positions.len()
                                && old_binop_positions[binop_col_cursor] < old_pos
                            {
                                binop_col_cursor += 1;
                            }
                            if let Some(&i) = old_binop_positions.get(binop_col_cursor) {
                                *out_col = old_cols.get(i).copied().unwrap_or((0, 0, 0, 0));
                                binop_col_cursor += 1;
                            }
                        } else if want_cols && insn_is_fused_binop(new_insn) {
                            // The monotone recovery is unsound (an inner binop was
                            // folded away, so the surviving op is not the next old
                            // `BinOp` in order — issue #2580).  Pin the origin by the
                            // surviving op's `(dst, lhs)` registers: fusion preserves
                            // them, so a *unique* old `BinOp` at-or-after the cursor
                            // with the same register pair is unambiguously the origin.
                            // A non-unique match stays caret-free (never a wrong one).
                            let new_regs = match new_insn {
                                Insn::BinOpConst(d, l, _, _, _) | Insn::BinOpImm(d, l, _, _, _) => {
                                    Some((*d, *l))
                                }
                                _ => None,
                            };
                            if let Some(key) = new_regs
                                && let Some(locs) = old_binop_reg_positions.get(&key)
                            {
                                let candidates: Vec<usize> =
                                    locs.iter().copied().filter(|&p| p >= old_pos).collect();
                                if let [i] = candidates[..] {
                                    *out_col = old_cols.get(i).copied().unwrap_or((0, 0, 0, 0));
                                }
                            }
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
        // binary op whose foldable sibling collapsed gets its origin col from the
        // suffix alignment computed above.  Authoritative for these ops — the
        // exact/discriminant paths cannot have a correct match for a fused
        // `BinOpConst` (it has no old counterpart), and a plain surviving `BinOp`
        // in a folded statement is `disc_ambiguous` so it was left caret-free.
        if want_cols && let Some(&i) = suffix_binop_old_pos.get(&new_idx) {
            *out_col = old_cols.get(i).copied().unwrap_or((0, 0, 0, 0));
        }
    }
    let suffix_line = linenos.last().copied().unwrap_or(0);
    linenos.resize(full_new_len, suffix_line);
    cols.resize(full_new_len, (0, 0, 0, 0));
    (linenos, cols)
}
