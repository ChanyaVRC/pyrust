// ─── Fused BinOp and not-jump origin recovery for source mapping ──────────────

/// Precomputed metadata for each old `BinOp(dst, lhs, op, rhs)` (issues
/// #2580 / #2889 / #2905):
///   - `pos`: index in `old_insns`
///   - `dst`, `lhs`, `op`: original operands and operator
///   - `rhs_const`: if `old_insns[p - 1]` is `LoadConst(rhs, c_idx)`, that
///     exact constant pool slot `Some(c_idx)` (recall `pass_compact_consts`
///     runs *after* this remapper, so constant slots are still un-reindexed).
#[derive(Clone, Copy)]
struct OldBinOpInfo {
    pos: usize,
    dst: u32,
    lhs: u32,
    op: crate::ast::BinaryOp,
    rhs_const: Option<u16>,
}

fn binop_origin_op(insn: &Insn) -> Option<crate::ast::BinaryOp> {
    match insn {
        Insn::BinOp(_, _, op, _) => Some(*op),
        Insn::BinOpConst(_, _, op, _, false) | Insn::BinOpImm(_, _, op, _, false) => Some(*op),
        _ => None,
    }
}

fn binop_derived_op(insn: &Insn) -> Option<crate::ast::BinaryOp> {
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
}

fn is_cmp_jump_insn(insn: &Insn) -> bool {
    matches!(
        insn,
        Insn::CmpJumpIfFalse(..)
            | Insn::CmpJumpIfTrue(..)
            | Insn::CmpJumpIfFalseConst(..)
            | Insn::CmpJumpIfTrueConst(..)
            | Insn::CountCmpJumpTrue(..)
            | Insn::CountCmpJumpFalse(..)
    )
}

/// Exact 1:1 origin map for *binop-derived* opcodes (issue #2889).  Both
/// fusions rewrite a `BinOp` into an opcode that exists nowhere in the old
/// stream — `pass_binop_const_fusion` into `BinOpConst`/`BinOpImm`, comparison
/// -jump fusion into a `Cmp*Jump*` — so neither the structural nor the
/// discriminant match can anchor it.
///
/// Both fusions are 1:1 replacements that preserve source order and the
/// operator, so the old and new binop-derived opcodes can be paired greedily in
/// order.  Every old op must find a partner (otherwise a fold dropped one and
/// the alignment is guesswork); the only new ops allowed to go unpaired are
/// comparison jumps, which loop inversion and the int-loop version guard
/// legitimately duplicate from one source comparison.  Any other surplus means
/// the streams are not a 1:1 rewrite of each other and the whole map is
/// abandoned (a missing anchor beats a wrong one, #2426).
fn build_binop_derived_origin(
    old_insns: &[Insn],
    origin_new_insns: &[Insn],
) -> HashMap<usize, usize> {
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
    let mut ni = 0usize;
    let mut aligned_ok = true;
    for &(old_i, op) in &old_derived {
        while ni < new_derived.len() && new_derived[ni].1 != op {
            if !is_cmp_jump_insn(&origin_new_insns[new_derived[ni].0]) {
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
            .any(|&(new_i, _)| !is_cmp_jump_insn(&origin_new_insns[new_i]))
    {
        binop_derived_origin.clear();
    }
    binop_derived_origin
}

fn build_old_binop_infos(old_insns: &[Insn], old_binop_positions: &[usize]) -> Vec<OldBinOpInfo> {
    old_binop_positions
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
        .collect()
}

/// Old fusion sites for the `not`-invert peephole (issue #2588): a position `i`
/// where `UnaryOp(_, Not, src)` is immediately followed by a conditional jump on
/// that same register.  `pass_not_invert` collapses the pair into a single
/// `JumpIfTrue`/`JumpIfFalse(src, …)`.  Record the operand col per fusion site,
/// keyed by the jump's old position `i + 1`, so a monotone scan recovers each
/// fused jump's anchor.
fn build_old_notjump_operand_cols(
    old_insns: &[Insn],
    old_cols: &[crate::ast::CaretSpan],
) -> Vec<(usize, crate::ast::CaretSpan)> {
    old_insns
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
        .collect()
}

/// Constant-slot lookup for a new binop-origin op: direct slot on `BinOpConst`,
/// or an immediately preceding `LoadConst(rhs, c)` for `BinOp`.
fn new_binop_const_idx(new_insns: &[Insn], new_idx: usize, insn: &Insn) -> Option<u16> {
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
}

fn old_binop_const_at(old_insns: &[Insn], old_p: usize) -> Option<u16> {
    let Insn::BinOp(_, _, _, rhs) = old_insns.get(old_p)? else {
        return None;
    };
    old_p.checked_sub(1).and_then(|prev| match old_insns[prev] {
        Insn::LoadConst(r, c) if r == *rhs => Some(c),
        _ => None,
    })
}

/// Constant-slot compatibility guard between a new binop-origin op and an old
/// `BinOp`: an old `BinOp` whose RHS came directly from `LoadConst(_, old_c)`
/// cannot become a fused or const-RHS op with a different constant slot
/// `new_c` (`pass_compact_consts` has not reindexed slots yet).  An old `BinOp`
/// whose RHS was a computed expression (`rhs_const == None`) CAN receive a newly
/// const-folded slot `new_c` (e.g. `b // (1 - 1)`).
#[inline]
fn const_slots_compatible(new_c: Option<u16>, old_c: Option<u16>) -> bool {
    match (new_c, old_c) {
        (Some(c), Some(oc)) => c == oc,
        _ => true,
    }
}

/// Suffix / left-spine recovery for the *folded* case (issues #2577 / #2578 /
/// #2586).  When a const-foldable sub-expression collapses (`a + b` with
/// `a`,`b` known → `LoadConst`), `binop_recovery_sound` is false and the
/// forward cursor is disabled — yet the surviving raising op (`(a+b) + "s"`,
/// `"s" + (x+2)`, `(a+b) + "s" + (c+d)`) still has a valid origin col in the
/// old stream and must keep its caret.
///
/// Every candidate pair `(new_i, old_p)` must additionally stay inside
/// `new_i`'s LCS anchor interval `[prev_aligned_old[new_i],
/// next_aligned_old[new_i])` and satisfy `const_slots_compatible` so a folded
/// later statement (e.g. `c1 = 10; c2 = 20; c3 = c1 + c2`) can never steal the
/// line or caret of an earlier surviving binop (`res = s + 1`, issue #2905).
#[allow(clippy::too_many_arguments)]
fn build_suffix_binop_old_pos(
    old_insns: &[Insn],
    old_prefix: &[u32],
    old_cols: &[crate::ast::CaretSpan],
    origin_new_insns: &[Insn],
    old_binop_positions: &[usize],
    new_binop_origin_count: usize,
    prev_aligned_old: &[usize],
    next_aligned_old: &[usize],
) -> HashMap<usize, usize> {
    let mut suffix_binop_old_pos: HashMap<usize, usize> = HashMap::new();
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
                new_binop_const_idx(origin_new_insns, new_i, &origin_new_insns[new_i]),
                old_binop_const_at(old_insns, old_p),
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
                    old_insns.get(p).and_then(binop_origin_op) == Some(op) && pair_valid(new_i, p)
                })
            });
        if spine_ops_match {
            for (j, &(new_i, _)) in surviving.iter().enumerate() {
                suffix_binop_old_pos.insert(new_i, spine_old[j]);
            }
        }
    }
    suffix_binop_old_pos
}

/// Pin a fused `BinOpConst` / `BinOpImm` (or renumbered `BinOp`) within the
/// current LCS anchor interval `[lo, hi)`.  Returns `Some(old_p)` only when a
/// sound, unambiguous tier uniquely identifies the origin (or when an exact 1:1
/// count-matched diagonal exists within the interval).
///
/// Uses `(d, l, op, c)` → `(l, op, c)` → `(d, l, op)` → `(l, op)` → exact 1:1
/// diagonal, strictly forbidding cross-constant mismatches and ambiguous
/// multi-matches (#2426 / #2580 / #2889 / #2905).
#[allow(clippy::too_many_arguments)]
fn pin_binop_in_interval(
    old_binop_infos: &[OldBinOpInfo],
    new_insns: &[Insn],
    new_idx: usize,
    lo: usize,
    hi: usize,
    new_hi: usize,
    dst: u32,
    lhs: u32,
    op: crate::ast::BinaryOp,
    const_idx: Option<u16>,
) -> Option<usize> {
    let start = old_binop_infos.partition_point(|info| info.pos < lo);
    let end = old_binop_infos.partition_point(|info| info.pos < hi);
    let slice = &old_binop_infos[start..end];
    if slice.is_empty() {
        return None;
    }
    let const_compatible = |info: &OldBinOpInfo| const_slots_compatible(const_idx, info.rhs_const);
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
    if let Some(p) = unique(&|info| info.lhs == lhs && info.op == op && const_compatible(info)) {
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
}
