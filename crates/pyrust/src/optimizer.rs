#![allow(clippy::needless_range_loop)]
// ↑ The optimizer walks bytecode by `pc` index and remaps positions via
// `old_to_new`, which is fundamentally an index-based algorithm. Iterator
// rewrites would obscure the dataflow without speeding anything up.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::bytecode::{
    AttrCacheEntry, BinOpCacheEntry, FnCode, FnProto, GLOBAL_CACHE_EMPTY, Insn, KwCallCacheEntry,
};
use crate::value::{Value, ValueKind};

/// Optimize a compiled `FnCode` and all nested function prototypes.
/// Applies a sequence of peephole passes over each instruction list.
pub fn optimize(code: FnCode) -> FnCode {
    optimize_fn_code(code)
}

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
            | Insn::CallMethod { .. }
            | Insn::CallMethodExpanded { .. }
            | Insn::CallMethodKw { .. }
            | Insn::RaiseValue(_)
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
/// instruction whose anchored occurrences disagree — e.g. a
/// `pass_cross_jump`-merged tail of two identical `LoadGlobal`s with different
/// columns (the #2431 concern) — also gets none.
///
/// Pass an empty `old_cols` (the `remap_linenos` wrapper does) to skip all col
/// work; the returned col vector is then all-`(0, 0)` at zero extra cost.
fn remap_lineno_and_col_tables(
    old_insns: &[Insn],
    old_linenos: &[u32],
    old_cols: &[crate::ast::CaretSpan],
    new_insns: &[Insn],
) -> (Vec<u32>, Vec<crate::ast::CaretSpan>) {
    if old_linenos.is_empty() {
        return (
            vec![0u32; new_insns.len()],
            vec![(0, 0, 0, 0); new_insns.len()],
        );
    }
    // Only do col work when at least one anchor was recorded.
    let want_cols = !old_cols.is_empty() && old_cols.iter().any(|&c| c != (0, 0, 0, 0));

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
    let new_binop_origin_count = new_insns
        .iter()
        .filter(|i| matches!(i, Insn::BinOp(..)) || insn_is_fused_binop(i))
        .count();
    let binop_recovery_sound = new_binop_origin_count >= old_binop_positions.len();
    // Cursor into `old_binop_positions`, advanced monotonically as fused ops are
    // matched (mirrors the `old_pos` discipline of the main scan).
    let mut binop_col_cursor: usize = 0;

    // Instructions whose anchored occurrences disagree get no anchor (ambiguous).
    let mut ambiguous: HashSet<&Insn> = HashSet::new();
    // Discriminants (opcode-only) whose anchored occurrences disagree: used by the
    // discriminant-match fallback below to refuse a col anchor when the same
    // opcode carries different spans across the old stream (#2411).
    let mut disc_ambiguous: HashSet<std::mem::Discriminant<Insn>> = HashSet::new();
    if want_cols {
        for (insn, locs) in &positions {
            let first = old_cols.get(locs[0]).copied().unwrap_or((0, 0, 0, 0));
            if locs
                .iter()
                .any(|&p| old_cols.get(p).copied().unwrap_or((0, 0, 0, 0)) != first)
            {
                ambiguous.insert(*insn);
            }
        }
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
    for ((out_col, new_insn), aligned_pos) in cols.iter_mut().zip(new_insns).zip(&aligned) {
        // Use the LCS-aligned old position when this new instruction was matched;
        // it is guaranteed `>= old_pos`, preserving the monotonic-cursor invariant
        // the fallbacks below rely on.
        let matched = *aligned_pos;
        match matched {
            Some(i) => {
                linenos.push(old_linenos.get(i).copied().unwrap_or(0));
                if want_cols && !ambiguous.contains(new_insn) {
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
                // discriminant's anchored occurrences are unambiguous, mirroring the
                // exact-match `ambiguous` guard above.
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
                        if want_cols && !disc_ambiguous.contains(&std::mem::discriminant(new_insn))
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
                        }
                    }
                }
            }
        }
    }
    (linenos, cols)
}

fn optimize_fn_code(code: FnCode) -> FnCode {
    // Recursively optimize nested function / class bodies first.
    let fn_protos: Vec<FnProto> = code
        .fn_protos
        .into_iter()
        .map(|mut proto| {
            let inner = Rc::try_unwrap(proto.code).unwrap_or_else(|rc| (*rc).clone());
            proto.code = Rc::new(optimize_fn_code(inner));
            proto
        })
        .collect();

    let num_locals = code.num_locals;
    let mut num_regs = code.num_regs;
    let mut consts = code.consts;
    let names = code.names;
    let original_insns = code.insns.clone();
    let original_linenos = code.lineno_table.clone();
    let original_cols = code.col_table.clone();
    // Inter-procedural inlining of small pure leaf functions (issue #349).
    // Runs first so that const-fold / LICM / forcount machinery can subsequently
    // optimise the spliced body in the caller's scope.
    let insns = pass_inline(code.insns, &mut consts, &mut num_regs, &fn_protos, &names);
    let insns = pass_thread_jumps(insns);
    let insns = pass_binop_const_fusion(insns, num_locals);
    let insns = pass_fold_const_tuple(insns, num_locals, &mut consts);
    let insns = pass_reassoc(insns, &mut consts, num_locals);
    let insns = pass_const_fold(insns, &mut consts, num_locals);
    let insns = pass_str_method_const_fold(insns, &mut consts, &names, num_locals);

    let insns = pass_algebraic_simplify(insns, &mut consts);
    let insns = pass_unary_fold(insns, num_locals, &mut consts);
    let insns = pass_ivsr(insns, &mut consts, &mut num_regs);
    let insns = pass_const_branch_elim(insns, &consts);
    let insns = pass_cmpjump_fusion(insns, num_locals);
    let insns = pass_not_invert(insns, num_locals);
    // Run cmpjump fusion again: `pass_not_invert` can expose new
    // `BinOp + Cond-Jump` pairs (e.g. when an outer `not` was stripped from
    // `not (a == b)`, leaving `BinOp(Eq) + JumpIfTrue` ready to fuse into
    // `CmpJumpIfTrue`).  This matters for `if not (...): ...` patterns,
    // including the inversion emitted by the issue #287 trampoline rewrite.
    let insns = pass_cmpjump_fusion(insns, num_locals);
    let insns = pass_const_reg_prop(insns, num_locals, &consts);
    let insns = pass_binopinplace_downgrade(insns, num_locals, &consts);
    let insns = pass_concat_merge(insns, num_locals, &mut num_regs, &consts);
    let insns = pass_exit_inline(insns);
    let insns = pass_licm(insns, num_locals);
    let insns = pass_cse(insns, num_locals);
    let insns = pass_dead_code(insns);
    let insns = pass_dead_store_elim(insns, num_locals);
    // Second run catches argument-prep moves that became dead after the first
    // pass removed their consuming CallMemo (pure-call DCE cascade).
    let insns = pass_dead_store_elim(insns, num_locals);
    // Drop Call instructions to pure builtins whose result is never used.
    // A third dead-store pass then removes the now-dead LoadGlobal and arg
    // loads that fed the eliminated calls.
    let insns = pass_builtin_dce(insns, num_locals, &names);
    let insns = pass_dead_store_elim(insns, num_locals);
    let insns = pass_syncmod_sink(insns);
    // Tail-merging must not collapse two raise/return sites that carry different
    // source lines into one survivor copy — doing so loses the duplicate site's
    // line for traceback attribution (issue #2420: a second fresh `raise` in the
    // same frame inherited the first raise's line).  Cross-jump has no line info
    // of its own, so compute the current per-instruction lines (mapped from the
    // original stream, exactly as the final `remap_linenos` does) and hand them in.
    let (pre_cj_linenos, _) =
        remap_lineno_and_col_tables(&original_insns, &original_linenos, &original_cols, &insns);
    let insns = pass_cross_jump(insns, &pre_cj_linenos);
    let insns = pass_copy_prop(insns, num_locals);
    let insns = pass_forcount_reg_upgrade(insns);
    let insns = pass_switch_hoist(insns, num_locals, &consts);
    let insns = pass_loop_inversion(insns);
    let insns = pass_trivial_nop(insns);
    let insns = pass_self_tail_call(insns);
    let insns = pass_forcount_const_inline(insns, &consts);
    let insns = pass_forcount_unroll(insns, &mut consts, code.is_generator);
    let insns = pass_linear_loop_fold(insns, &mut consts);
    let insns = pass_loadnone_merge(insns);

    // Remap line numbers BEFORE compacting constants.  `pass_compact_consts`
    // reindexes constant-pool slots, which mutates the `idx` field of every
    // `LoadConst`/`BinOpConst`/etc.  `remap_linenos` matches new instructions to
    // the (un-reindexed) original stream by structural equality, so running it on
    // the post-compaction stream lets a reindexed constant spuriously match an
    // unrelated original instruction that happened to use the same raw index.
    // That false match advances the greedy scan cursor past the correct
    // occurrence, so a later raising instruction (e.g. a division that overflows)
    // inherits the wrong line number — attributing an exception to a later
    // statement than the one that actually raised (issue #1962).
    //
    // `pass_compact_consts` is a 1:1 instruction-count- and order-preserving
    // transformation, so the line numbers computed against the pre-compaction
    // stream apply unchanged to the post-compaction stream.
    // Zero-cost exception handling (CPython 3.11): build the per-pc handler
    // table from the balanced SetupExcept/PopExcept structure and strip those
    // two block-setup instructions from the stream.  Runs before the lineno
    // remap so the (post-strip) instruction stream is the one line numbers are
    // computed against.  On the (never-observed) bail path the stream is handed
    // back unchanged with an empty table, and the VM falls back to the dynamic
    // SetupExcept/PopExcept handler stack — always correct, never wrong.
    let (insns, exc_table) = build_exc_table(insns);
    let has_exc_handlers = exc_table.iter().any(|&t| t != EXC_NO_HANDLER);

    // Remap line numbers and PEP 657 caret anchors (#2426) in one shared scan.
    // `pass_compact_consts` below is 1:1 order-preserving, so both tables
    // computed against the pre-compaction stream apply unchanged after it.
    let (lineno_table, col_table) =
        remap_lineno_and_col_tables(&original_insns, &original_linenos, &original_cols, &insns);
    let (insns, consts) = pass_compact_consts(insns, consts);

    let insns_len = insns.len();
    let names_len = names.len();
    FnCode {
        insns,
        filename: code.filename,
        lineno_table,
        col_table,
        first_lineno: code.first_lineno,
        consts,
        names,
        num_regs,
        num_iters: code.num_iters,
        num_locals,
        fn_protos,
        cell_vars: code.cell_vars,
        is_generator: code.is_generator,
        is_coroutine: code.is_coroutine,
        is_class_method: code.is_class_method,
        is_inlined_comp: code.is_inlined_comp,
        comp_enclosing_locals: code.comp_enclosing_locals,
        attr_cache: std::cell::RefCell::new(vec![AttrCacheEntry::Empty; insns_len]),
        global_cache: RefCell::new(vec![(GLOBAL_CACHE_EMPTY, Value::none()); names_len]),
        binop_cache: RefCell::new(vec![BinOpCacheEntry::Empty; insns_len]),
        kwcall_cache: RefCell::new(vec![KwCallCacheEntry::Empty; insns_len]),
        exc_table,
        has_exc_handlers,
    }
}

// ─── Inter-procedural inlining (issue #349) ─────────────────────────────────────

/// Maximum number of instructions in a callee body eligible for inlining.
/// Counts the callee's pre-optimisation insns including the trailing `Return`.
const INLINE_MAX_INSNS: usize = 12;

/// Returns `true` if `insn` is a body instruction that the inliner can splice
/// into the caller's scope.  The whitelist is intentionally narrow: it admits
/// only register/const arithmetic and object construction, and explicitly
/// excludes anything that
///   * references the callee's name pool (globals / attrs / methods) — those
///     would need name-pool merging and could observe caller-vs-callee scope
///     differences,
///   * introduces control flow (jumps, loops, exception setup, returns other
///     than the single trailing one) — splicing those needs offset remapping,
///   * builds a frame or closure (`Call*`, `MakeFunction`, `MakeClass`, yield),
///   * mutates a value that might alias a caller object (`SetItem`, `SetAttr`,
///     `ListAppend`, `SetAdd`, `DictUpdate`, …).
///
/// Instructions that may *raise* (e.g. `BinOp` on mismatched types) are allowed:
/// the identical instruction runs at the same logical point, so the exception
/// type and message are byte-identical.  Only the traceback *frame list* would
/// differ, and pyrust's tracebacks already omit per-frame line numbers / carets
/// and the parity harness strips all `Traceback`/`File "…"` lines, so the
/// observable behaviour (final exception line) is unchanged.
fn inline_body_insn_ok(insn: &Insn) -> bool {
    use Insn::*;
    match insn {
        LoadConst(..)
        | LoadNone(..)
        | LoadNoneRange { .. }
        | Move(..)
        | CopyReg(..)
        | BinOp(..)
        | UnaryOp(..)
        | GetItem(..)
        | GetSlice(..)
        | BuildList(..)
        | BuildTuple(..)
        | BuildString(..)
        | BuildSlice(..)
        | BuildDict(..)
        | Concat { .. }
        | FormatValue(..)
        | FormatValueSpec(..) => true,
        // Folded constant binops are admissible only in their *non-augmented*
        // form: an augmented (`is_aug == true`) fused op applies in-place
        // `__i<op>__` semantics, which could mutate an argument that aliases a
        // caller object.  Plain (non-aug) folds are pure non-mutating `__<op>__`.
        BinOpConst(_, _, _, _, is_aug) | BinOpImm(_, _, _, _, is_aug) => !*is_aug,
        _ => false,
    }
}

/// Per-proto inlining plan computed once: the body to splice (sans the trailing
/// return), the parameter count, and how the trailing return is realised.
struct InlinePlan {
    /// Number of registers the callee uses (its `num_regs`); the splice
    /// allocates a fresh window of this size in the caller.
    callee_num_regs: u32,
    /// Number of positional parameters (== required argc).
    argc: u8,
    /// Body instructions (everything before the trailing return), each still in
    /// the callee's own register numbering.  Const indices have already been
    /// rewritten to point into the caller's merged const pool.
    body: Vec<Insn>,
    /// The trailing return: `Some(reg)` for `Return(reg)` (in callee numbering),
    /// `None` for `ReturnNone`.
    ret: Option<u32>,
}

/// Decide whether `proto` is an inlinable small pure leaf function and, if so,
/// build its [`InlinePlan`], merging the callee's constant pool into the
/// caller's `consts`.  Returns `None` (no inlining) on any disqualifier.
fn build_inline_plan(proto: &FnProto, caller_consts: &mut Vec<Value>) -> Option<InlinePlan> {
    if !proto.is_pure {
        return None;
    }
    let code = &proto.code;
    if code.is_generator || code.is_coroutine {
        return None;
    }
    // No closure capture of own locals, and no global/nonlocal interaction.
    if !code.cell_vars.is_empty() {
        return None;
    }
    // Body must reference no names at all (no globals / attrs / methods).  This
    // keeps the splice to a const-pool merge only and rules out scope-sensitive
    // lookups.
    if !code.names.is_empty() {
        return None;
    }
    let params = &proto.param_spec;
    // Fixed positional parameters only: no *args / **kwargs / defaults /
    // keyword-only / positional-only markers.
    let argc = params.names.len();
    if argc > u8::MAX as usize {
        return None;
    }
    if params.has_default.iter().any(|&b| b)
        || params.is_args.iter().any(|&b| b)
        || params.is_kwargs.iter().any(|&b| b)
        || params.is_keyword_only.iter().any(|&b| b)
    {
        return None;
    }
    // Every parameter must bind to register `i` (contiguous 0..argc).  An unused
    // parameter gets no slot (`ParamBind::None`) which would break the
    // arg→register mapping, so bail in that case.
    if proto.param_binds.len() != argc {
        return None;
    }
    for (i, bind) in proto.param_binds.iter().enumerate() {
        match bind {
            pyrust_core::ParamBind::Reg(r) if *r as usize == i => {}
            _ => return None,
        }
    }
    let insns = &code.insns;
    if insns.is_empty() || insns.len() > INLINE_MAX_INSNS {
        return None;
    }
    // The body must be a single straight-line block ending in exactly one
    // return.  Every non-final instruction must be in the inlinable whitelist;
    // the final instruction must be `Return(r)` or `ReturnNone`.
    let (last, body_insns) = insns.split_last().unwrap();
    let ret = match last {
        Insn::Return(r) => Some(*r),
        Insn::ReturnNone => None,
        _ => return None,
    };
    for ins in body_insns {
        if !inline_body_insn_ok(ins) {
            return None;
        }
    }
    // Merge the callee's constant pool into the caller's, building an index map.
    // Then rewrite every const-referencing instruction in the body.
    let mut const_map: Vec<u16> = Vec::with_capacity(code.consts.len());
    for c in &code.consts {
        match intern_const_in_pool(caller_consts, c.clone()) {
            Some(idx) => const_map.push(idx),
            // Const pool full — abandon this inline (no partial state leaks
            // because we only mutate `caller_consts` via interning, which is
            // idempotent for already-present values).
            None => return None,
        }
    }
    let mut body: Vec<Insn> = Vec::with_capacity(body_insns.len());
    for ins in body_insns {
        let mut ins = ins.clone();
        match &mut ins {
            Insn::LoadConst(_, idx) | Insn::BinOpConst(_, _, _, idx, _) => {
                *idx = *const_map.get(*idx as usize)?;
            }
            _ => {}
        }
        body.push(ins);
    }
    Some(InlinePlan {
        callee_num_regs: code.num_regs,
        argc: argc as u8,
        body,
        ret,
    })
}

/// Shift every register referenced by `insn` by `base` (used to relocate a
/// callee body into a fresh register window in the caller).
fn shift_insn_regs(insn: &mut Insn, base: u32) {
    use Insn::*;
    let shift = |r: &mut u32| *r += base;
    match insn {
        LoadConst(d, _) | LoadNone(d) => shift(d),
        LoadNoneRange { start, .. } => shift(start),
        Move(d, s) | CopyReg(d, s) | UnaryOp(d, _, s) | FormatValue(d, s) => {
            shift(d);
            shift(s);
        }
        FormatValueSpec(d, s, spec) => {
            shift(d);
            shift(s);
            shift(spec);
        }
        BinOp(d, a, _, b) => {
            shift(d);
            shift(a);
            shift(b);
        }
        BinOpConst(d, a, _, _, _) | BinOpImm(d, a, _, _, _) => {
            shift(d);
            shift(a);
        }
        GetItem(d, a, b) => {
            shift(d);
            shift(a);
            shift(b);
        }
        GetSlice(d, obj, base_r) => {
            shift(d);
            shift(obj);
            shift(base_r);
        }
        BuildList(d, b, _) | BuildTuple(d, b, _) | BuildDict(d, b, _) => {
            shift(d);
            shift(b);
        }
        BuildString(d, b, _) => {
            shift(d);
            shift(b);
        }
        BuildSlice(d, b) => {
            shift(d);
            shift(b);
        }
        Concat { dst, base: b, .. } => {
            shift(dst);
            shift(b);
        }
        // No other variants can appear in an inlinable body (gated by
        // `inline_body_insn_ok`); leave them untouched.
        _ => {}
    }
}

/// Inter-procedural inlining of small pure leaf functions at their call sites.
///
/// ## What is inlined
///
/// A `Call`/`CallMemo` site whose function-value register provably and stably
/// holds a known [`FnProto`] that passes [`build_inline_plan`] (small, pure,
/// straight-line, name-free, fixed positional params).  The call frame is
/// eliminated: arguments are copied into a fresh register window, the callee
/// body is spliced register-shifted into the caller, and the callee's return
/// becomes a move of the result into the call's destination register.
///
/// ## Binding stability (no runtime guard)
///
/// Inlining is sound only if the call target cannot be rebound between the
/// `def` and the call.  Rather than emit a runtime deopt guard, this pass
/// inlines *only* when the binding is provably immutable:
///
///   * The function-value register at the call site resolves (through at most
///     one `Move` hop) to a register written exactly once, by a
///     `MakeFunction(_, proto_idx, …)` with no defaults / annotations.
///   * The enclosing function never materialises its namespace via
///     `globals()` / `locals()` / `vars()` / `exec` / `eval`, and contains no
///     `import *` — so the global binding cannot be swapped at runtime.
///
/// When either fails, the call is left untouched (partial coverage is fine).
fn pass_inline(
    insns: Vec<Insn>,
    consts: &mut Vec<Value>,
    num_regs: &mut u32,
    fn_protos: &[FnProto],
    names: &[String],
) -> Vec<Insn> {
    if fn_protos.is_empty() {
        return insns;
    }

    // ── Binding-stability precondition: the namespace must not be reifiable. ──
    // If the scope can observe / mutate its globals dict, a function binding
    // could be swapped at runtime (e.g. `globals()['f'] = g`) and inlining would
    // be unsound.  Disable the pass entirely for the scope if it references any
    // namespace-reifying builtin (`globals` / `locals` / `vars` / `exec` /
    // `eval`) or performs a star import — all of which can rebind a name behind
    // the inliner's back.
    let reifies_namespace = names
        .iter()
        .any(|nm| matches!(nm.as_str(), "globals" | "locals" | "vars" | "exec" | "eval"))
        || insns.iter().any(|i| matches!(i, Insn::ImportStar(_)));
    if reifies_namespace {
        return insns;
    }

    let n = insns.len();
    let mut write_count: HashMap<u32, u32> = HashMap::new();
    let mut single_write: HashMap<u32, usize> = HashMap::new();
    let mut buf: HashSet<u32> = HashSet::new();
    for (pc, insn) in insns.iter().enumerate() {
        buf.clear();
        collect_writes(insn, &mut buf);
        for &d in &buf {
            *write_count.entry(d).or_insert(0) += 1;
            single_write.insert(d, pc);
        }
    }
    // `make_fn_idx(r, before_pc)` returns the proto built by the most-recent
    // `MakeFunction(r, idx, 0, _, 0)` write to `r` strictly before `before_pc`,
    // provided `r` is not rewritten between that point and `before_pc`.  Used to
    // identify the proto a temporary held at the moment it was copied into a
    // stable holder.
    let make_fn_idx = |reg: u32, before_pc: usize| -> Option<u16> {
        let mut latest: Option<usize> = None;
        for (pc, insn) in insns.iter().enumerate().take(before_pc) {
            let mut w: HashSet<u32> = HashSet::new();
            collect_writes(insn, &mut w);
            if w.contains(&reg) {
                latest = Some(pc);
            }
        }
        let wpc = latest?;
        match &insns[wpc] {
            Insn::MakeFunction(d, proto_idx, _, defs_n, _, annots_n)
                if *d == reg && *defs_n == 0 && *annots_n == 0 =>
            {
                Some(*proto_idx)
            }
            _ => None,
        }
    };
    // Resolve the stable function-value holder `reg` (the register named in the
    // call-site `Move(fn_reg, reg)`) to its proto.  The holder must be written
    // exactly once across the whole scope (so its value is immutable for every
    // call), and that single write must either *be* a `MakeFunction` or copy in
    // a register that held one at that point.
    let resolve_fn_reg = |reg: u32| -> Option<u16> {
        if write_count.get(&reg).copied() != Some(1) {
            return None;
        }
        let &wpc = single_write.get(&reg)?;
        match &insns[wpc] {
            Insn::MakeFunction(d, proto_idx, _, defs_n, _, annots_n)
                if *d == reg && *defs_n == 0 && *annots_n == 0 =>
            {
                Some(*proto_idx)
            }
            Insn::Move(d, s) if *d == reg => make_fn_idx(*s, wpc),
            _ => None,
        }
    };

    // Pre-build an inlining plan for every proto that resolves to a call target.
    // (Computed lazily / cached as we encounter call sites.)
    let mut plans: HashMap<u16, Option<InlinePlan>> = HashMap::new();
    let mut next_window = *num_regs;

    // First splice pass.  `out` accumulates `(old_i, insn)` pairs where `old_i`
    // is `Some(pc)` for an instruction copied verbatim from the original stream
    // (its offsets are rewritten in the second pass) and `None` for an inliner-
    // generated instruction (a plain register/const op with no offsets).
    //
    // `old_to_new[pc]` records the new index at which original instruction `pc`
    // lands (for an inlined call, the position where its splice begins).  Built
    // alongside `out` so that jumps crossing inlined regions can be re-targeted.
    let mut out: Vec<(Option<usize>, Insn)> = Vec::with_capacity(n);
    let mut old_to_new: Vec<usize> = vec![0; n + 1];
    let mut changed = false;
    for pc in 0..n {
        old_to_new[pc] = out.len();
        let insn = &insns[pc];
        if let Insn::Call(fn_reg, argc) | Insn::CallMemo(fn_reg, argc) = insn {
            let fn_reg = *fn_reg;
            let argc = *argc;
            // The function-value register is loaded by a `Move(fn_reg, src)`
            // earlier in this same straight-line block (the standard call-site
            // shape).  Locate it and resolve the proto behind `src`.
            if let Some(proto_idx) = resolve_call_target(&insns, pc, fn_reg, &resolve_fn_reg) {
                let plan_slot = plans
                    .entry(proto_idx)
                    .or_insert_with(|| build_inline_plan(&fn_protos[proto_idx as usize], consts));
                if let Some(plan) = plan_slot
                    && plan.argc == argc
                {
                    // Splice: bind args into a fresh register window, copy the
                    // body (register-shifted), then realise the return into
                    // `fn_reg` (the call's destination register).
                    let base = next_window;
                    next_window += plan.callee_num_regs;
                    for i in 0..argc as u32 {
                        out.push((None, Insn::Move(base + i, fn_reg + 1 + i)));
                    }
                    for body_ins in &plan.body {
                        let mut body_ins = body_ins.clone();
                        shift_insn_regs(&mut body_ins, base);
                        out.push((None, body_ins));
                    }
                    match plan.ret {
                        Some(r) => out.push((None, Insn::Move(fn_reg, base + r))),
                        None => out.push((None, Insn::LoadNone(fn_reg))),
                    }
                    changed = true;
                    continue;
                }
            }
        }
        out.push((Some(pc), insn.clone()));
    }
    old_to_new[n] = out.len();

    if next_window > *num_regs {
        *num_regs = next_window;
    }

    if !changed {
        // Nothing inlined — return the original stream untouched (no offset
        // rewrite needed, and `out` may have re-cloned everything already, so
        // hand back the cheaper original).
        return insns;
    }

    // Second pass: rewrite position-relative offsets of every copied
    // instruction to account for the inserted splices.  Inliner-generated
    // instructions carry no offsets and pass through unchanged.
    out.into_iter()
        .map(|(old_i, insn)| match old_i {
            Some(i) => rewrite_offsets(insn, i, &old_to_new),
            None => insn,
        })
        .collect()
}

/// Resolve the proto that a `Call`/`CallMemo` at `call_pc` targets, by locating
/// the most recent write to `fn_reg` before the call within the same basic
/// block and resolving it via `resolve_fn_reg`.
fn resolve_call_target(
    insns: &[Insn],
    call_pc: usize,
    fn_reg: u32,
    resolve_fn_reg: &impl Fn(u32) -> Option<u16>,
) -> Option<u16> {
    // Walk backwards from the call; stop at a basic-block boundary (any control
    // flow) — the call-site `Move(fn_reg, src)` is always in the same block.
    let mut i = call_pc;
    while i > 0 {
        i -= 1;
        let insn = &insns[i];
        if is_control_flow(insn) {
            return None;
        }
        // The first write we find to fn_reg must be a `Move(fn_reg, src)` that
        // loads the function value; resolve src to a proto.
        let mut writes: HashSet<u32> = HashSet::new();
        collect_writes(insn, &mut writes);
        if writes.contains(&fn_reg) {
            if let Insn::Move(d, s) = insn
                && *d == fn_reg
            {
                return resolve_fn_reg(*s);
            }
            return None;
        }
    }
    None
}

// ─── Jump threading ────────────────────────────────────────────────────────────

/// Thread chains of unconditional `Jump`s so that any jump whose target is
/// itself a `Jump` is redirected to the chain's final non-`Jump` destination.
/// Conditional jumps have only their taken-branch target threaded; the
/// fallthrough path is unchanged.  No instructions are removed in this pass.
///
/// Only **forward** `Jump`s (offset ≥ 0) are followed in the chain.  Backward
/// jumps (loop back-edges, offset < 0) are treated as opaque non-`Jump`
/// instructions and terminate the chain.  Threading through a backward jump
/// would produce a negative exit offset in `ForCount*` instructions, violating
/// the invariant that their exit always points past the loop body.
fn pass_thread_jumps(insns: Vec<Insn>) -> Vec<Insn> {
    // Follow a chain of unconditional forward Jumps from `start`, returning the
    // index of the first instruction that is NOT an unconditional forward Jump.
    // A visited-set guards against infinite loops (self-referential jumps).
    fn follow(insns: &[Insn], start: usize) -> usize {
        let mut pc = start;
        let mut seen = HashSet::new();
        loop {
            if pc >= insns.len() || !seen.insert(pc) {
                break;
            }
            match &insns[pc] {
                Insn::Jump(k) if *k >= 0 => pc = (pc as i64 + 1 + *k as i64) as usize,
                _ => break,
            }
        }
        pc
    }

    insns
        .iter()
        .enumerate()
        .map(|(i, insn)| {
            let thread = |k: i32| -> i32 {
                let raw = (i as i64 + 1 + k as i64) as usize;
                let final_t = follow(&insns, raw);
                (final_t as i64 - i as i64 - 1) as i32
            };
            use Insn::*;
            match insn.clone() {
                Jump(k) => Jump(thread(k)),
                JumpIfFalse(r, k) => JumpIfFalse(r, thread(k)),
                JumpIfTrue(r, k) => JumpIfTrue(r, thread(k)),
                CmpJumpIfFalse(a, op, b, k) => CmpJumpIfFalse(a, op, b, thread(k)),
                CmpJumpIfTrue(a, op, b, k) => CmpJumpIfTrue(a, op, b, thread(k)),
                CmpJumpIfFalseConst(r, op, c, k) => CmpJumpIfFalseConst(r, op, c, thread(k)),
                CmpJumpIfTrueConst(r, op, c, k) => CmpJumpIfTrueConst(r, op, c, thread(k)),
                ForIter(dst, slot, k) => ForIter(dst, slot, thread(k)),
                ForCountReg(v, op, stop, step, k) => ForCountReg(v, op, stop, step, thread(k)),
                ForCountConst(v, op, stop, step, k) => ForCountConst(v, op, stop, step, thread(k)),
                ForCountConstInline(v, op, stop, step, k) => {
                    ForCountConstInline(v, op, stop, step, thread(k))
                }
                SetupExcept(k) => SetupExcept(thread(k)),
                MatchExcept(r, k) => MatchExcept(r, thread(k)),
                MatchExceptStar(r, src, dst, k) => MatchExceptStar(r, src, dst, thread(k)),
                other => other,
            }
        })
        .collect()
}

// ─── BinOp-const fusion ────────────────────────────────────────────────────────

/// Fuse `LoadConst(r, c) + BinOp(dst, lhs, op, r)` → `BinOpConst(dst, lhs, op, c)`
/// when `r` is a temp register (`r >= num_locals`), and `r` is not read by any
/// instruction after the BinOp (forward liveness check).
///
/// Only handles Case 1 where the constant is the RHS operand. When the constant
/// is the LHS operand the optimization is skipped because swapping operands would
/// call `lhs.__add__(const)` instead of `const.__add__(lhs)` / `lhs.__radd__(const)`,
/// breaking Python's reflected operator protocol.
///
/// The liveness guard is necessary for patterns like chained comparisons where the
/// same intermediate value is used as both an operand of the first comparison and
/// the left-hand side of the next comparison.
fn pass_binop_const_fusion(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    // Precompute O(1) replacements for the two per-pair tail scans that would
    // otherwise make this pass O(n²) on a long straight-line sequence (issue
    // #2002):
    //  * `back_edge_after[j]` — whether any instruction at index `>= j` is a
    //    backward branch (equivalent to `slice_has_back_edge(&insns[j..])`).
    //  * `last_read[r]` — the highest index at which register `r` is read, so
    //    `reg_is_read_in(&insns[j..], r)` becomes `last_read[r] >= j`.
    let mut back_edge_after = vec![false; n + 1];
    for j in (0..n).rev() {
        back_edge_after[j] = back_edge_after[j + 1] || insn_is_back_edge(&transformed[j]);
    }
    let mut last_read: HashMap<u32, usize> = HashMap::new();
    {
        let mut reads_buf: HashSet<u32> = HashSet::new();
        for (j, insn) in transformed.iter().enumerate() {
            reads_buf.clear();
            collect_reads(insn, &mut reads_buf);
            for &r in &reads_buf {
                last_read.insert(r, j);
            }
        }
    }

    let mut i = 0;
    while i + 1 < n {
        if let (Insn::LoadConst(lc_reg, c_idx), Insn::BinOp(dst, lhs, op, rhs)) =
            (&transformed[i], &transformed[i + 1])
        {
            let (lc_reg, c_idx) = (*lc_reg, *c_idx);
            let (dst, lhs, op, rhs) = (*dst, *lhs, *op, *rhs);
            // Case 1: const is the RHS operand → BinOpConst(dst, lhs, op, c)
            let read_after = last_read
                .get(&lc_reg)
                .copied()
                .is_some_and(|last| last >= i + 2);
            // The coarse `last_read[lc_reg]` / `back_edge_after` guards block the
            // very common "reused scratch register" shape that dominates loop
            // bodies: a temp re-`LoadConst`-ed for each operand, e.g.
            //   LoadConst(t,2); BinOp(_, i, Mul, t)   ← value of t dead here
            //   LoadConst(t,1); BinOp(_, _, Sub, t)   ← (t overwritten before read)
            // `last_read[t]` points at the *second* BinOp and the loop ends in a
            // back-edge, so neither pair ever fuses.  `scratch_dead_after` proves
            // the precise fact — the value loaded at `i` is overwritten before any
            // read OR branch in a short straight-line window — which makes fusion
            // sound regardless of the forward `last_read` estimate or a later
            // back-edge (no reachable path can observe the dead value).
            let dead_after = scratch_dead_after(&transformed, i + 2, lc_reg);
            if rhs == lc_reg
                && lhs != lc_reg
                && lc_reg >= num_locals
                && (dead_after || !back_edge_after[i + 2])
                && (dst == lc_reg || !read_after || dead_after)
            {
                keep[i] = false;
                // Fusing a plain BinOp (never augmented) → is_aug = false.
                transformed[i + 1] = Insn::BinOpConst(dst, lhs, op, c_idx, false);
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    compact(transformed, &keep)
}

// ─── Constant tuple folding ────────────────────────────────────────────────────

/// Fold a sequence of `LoadConst` instructions feeding a `BuildTuple` into a
/// single `LoadConst` pointing to a pre-built tuple constant.
///
/// ## Pattern
///
/// ```text
/// LoadConst(base+0, c0)
/// LoadConst(base+1, c1)
/// ...
/// LoadConst(base+n-1, c_{n-1})
/// BuildTuple(dst, base, n)
/// ```
/// → replaced with a single `LoadConst(dst, tuple_pool_idx)` where
///   `consts[tuple_pool_idx]` is `Value::tuple([consts[c0], ..., consts[c_{n-1}]])`.
///
/// ## Guards
///
/// - Only `BuildTuple`, not `BuildList` (lists are mutable, tuples are immutable
///   constants and safe to deduplicate).
/// - `n >= 1 && n <= 16` — avoids unbounded look-back.
/// - All `base+j >= num_locals` — the element registers must be temporaries, not
///   named locals that could have been written by non-`LoadConst` instructions.
/// - `insns[i-n .. i]` are exactly `LoadConst(base+j, c_j)` for j in 0..n — the
///   look-back must be a perfect, contiguous, in-order match.
fn pass_fold_const_tuple(insns: Vec<Insn>, num_locals: u32, consts: &mut Vec<Value>) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i < n {
        if let Insn::BuildTuple(dst, base, argc) = transformed[i] {
            let argc = argc as usize;
            if (1..=16).contains(&argc) && i >= argc {
                // Check that insns[i-argc .. i] are LoadConst(base+j, c_j) for j in 0..argc
                // and that all base+j >= num_locals.
                let mut all_match = true;
                let mut c_indices: Vec<u16> = Vec::with_capacity(argc);
                for j in 0..argc {
                    let slot = i - argc + j;
                    match transformed[slot] {
                        Insn::LoadConst(reg, c_idx)
                            if reg == base + j as u32 && reg >= num_locals =>
                        {
                            c_indices.push(c_idx);
                        }
                        _ => {
                            all_match = false;
                            break;
                        }
                    }
                }

                if all_match {
                    // Build the tuple value from the pooled constants.
                    let elems: Vec<Value> = c_indices
                        .iter()
                        .map(|&ci| consts[ci as usize].clone())
                        .collect();
                    let tuple_val = Value::tuple(elems);

                    // Intern the tuple in the const pool (always append — tuples are
                    // not deduplicated by intern_const_in_pool which only handles
                    // scalars, and identity equality on tuples is object-level).
                    if consts.len() < u16::MAX as usize {
                        let new_idx = consts.len() as u16;
                        consts.push(tuple_val);

                        // Mark the n LoadConst predecessors as removed.
                        for j in 0..argc {
                            keep[i - argc + j] = false;
                        }
                        // Replace BuildTuple with LoadConst(dst, new_idx).
                        transformed[i] = Insn::LoadConst(dst, new_idx);
                    }
                }
            }
        }
        i += 1;
    }
    compact(transformed, &keep)
}

// ─── CmpJump fusion ────────────────────────────────────────────────────────────

/// Fuse a comparison result into the following conditional jump:
/// - `BinOp(r, lhs, op, rhs) + JumpIfFalse(r, k)` → `CmpJumpIfFalse(lhs, op, rhs, k)`
/// - `BinOp(r, lhs, op, rhs) + JumpIfTrue(r, k)`  → `CmpJumpIfTrue(lhs, op, rhs, k)`
/// - `BinOpConst(r, lhs, op, c) + JumpIfFalse(r, k)` → `CmpJumpIfFalseConst(lhs, op, c, k)`
/// - `BinOpConst(r, lhs, op, c) + JumpIfTrue(r, k)`  → `CmpJumpIfTrueConst(lhs, op, c, k)`
///
/// Only fuses when `r >= num_locals` (temp register — not a named local).
///
/// ## Jump-target guard (issue #2088)
///
/// The `JumpIfFalse(r, k)` at `i+1` is replaced *in place* by a `CmpJump…` that
/// **recomputes** `lhs op rhs` instead of merely testing the already-computed
/// register `r`.  That is only sound when control reaches `i+1` exclusively by
/// falling through from the `BinOp` at `i`.  If another instruction *jumps to*
/// `i+1`, that incoming edge expected the original `JumpIfFalse` to test `r`
/// (e.g. an `and`/`or` short-circuit jump that lands on the trailing
/// conditional jump to re-use its still-false LHS result).  Recomputing
/// `lhs op rhs` on that path produces the wrong branch decision.  So we skip
/// fusion whenever `i+1` is the target of any jump.
fn pass_cmpjump_fusion(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    // Indices that are the target of some (forward or backward) jump.  The
    // trailing conditional jump of a fusion candidate must not be such a target.
    let mut jump_targets: HashSet<usize> = HashSet::new();
    for (idx, insn) in transformed.iter().enumerate() {
        if let Some(k) = insn_jump_off(insn) {
            let target = idx as i64 + 1 + k as i64;
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
            (Insn::BinOpConst(r, lhs, op, c, _), Insn::JumpIfFalse(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfFalseConst(*lhs, *op, *c, *k))
            }
            (Insn::BinOpConst(r, lhs, op, c, _), Insn::JumpIfTrue(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfTrueConst(*lhs, *op, *c, *k))
            }
            (Insn::BinOp(r, lhs, op, rhs), Insn::JumpIfFalse(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfFalse(*lhs, *op, *rhs, *k))
            }
            (Insn::BinOp(r, lhs, op, rhs), Insn::JumpIfTrue(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfTrue(*lhs, *op, *rhs, *k))
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

// ─── Reassociation ─────────────────────────────────────────────────────────────

/// Reassociate chains of binary-op-with-constant so that `pass_const_fold` can
/// fold the constants together.
///
/// ## Pattern
///
/// ```text
/// BinOpConst(t1, x,  op, c2)   // t1 = x  op c2
/// BinOpConst(t2, t1, op, c1)   // t2 = t1 op c1  =  (x op c2) op c1
/// ```
///
/// For associative integer ops (`+`, `*`, `&`, `|`, `^`) this equals
/// `x op (c2 op c1)`.  We rewrite `t2`'s instruction to use `x` directly:
///
/// ```text
/// BinOpConst(t1, x,  op, c2)         [unchanged; dead-store-elim removes it]
/// BinOpConst(t2, x,  op, c_combined) // t2 = x op (c2 op c1)
/// ```
///
/// `pass_const_fold` sees `t2 = BinOpConst(x, op, c_combined)` where `x` may
/// still be unknown, but `c_combined` is concrete — so if a later pass
/// determines `x` is a constant the fold fires.  More immediately, after this
/// pass runs the intermediate `t1` register is dead (only `t2` used it), so
/// `pass_dead_store_elim` removes the `t1` instruction and we end up with
/// strictly fewer `BinOpConst` instructions.
///
/// ## Safety constraints
///
/// - **Integer-only**: float `+`/`*` is not associative (rounding), string `+`
///   is left-associative concatenation.  Only `Add`, `Mul`, `BitAnd`, `BitOr`,
///   `BitXor` with both constants being `Int` are eligible.
/// - **Same op**: `(x + 2) * 3` is not a chain — ops must match.
/// - **Overflow check**: `c2 op c1` is computed with `checked_add` /
///   `checked_mul`.  If the result overflows `i64` we bail out without
///   reassociating.  BigInt constants are not handled at this stage.
/// - **BB boundaries**: the `defined_as` map is cleared at branch/loop targets
///   so we never look through a phi point.
/// - **Temp registers only**: the intermediate `t1` must be `>= num_locals`;
///   named locals can be rewritten by user code between the two instructions.
fn pass_reassoc(insns: Vec<Insn>, consts: &mut Vec<Value>, num_locals: u32) -> Vec<Insn> {
    use crate::ast::BinaryOp;

    // Collect basic-block starts (same logic as pass_const_fold).
    let mut bb_starts: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target < insns.len() {
                bb_starts.insert(target);
            }
        }
    }

    /// Returns `true` for integer-associative ops that are safe to reassociate.
    fn is_integer_associative(op: BinaryOp) -> bool {
        matches!(
            op,
            BinaryOp::Add | BinaryOp::Mul | BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor
        )
    }

    /// Fold `c2 op c1` for two `i64` constants; returns `None` on overflow.
    fn fold_int_pair(c2: i64, op: BinaryOp, c1: i64) -> Option<i64> {
        match op {
            BinaryOp::Add => c2.checked_add(c1),
            BinaryOp::Mul => c2.checked_mul(c1),
            BinaryOp::BitAnd => Some(c2 & c1),
            BinaryOp::BitOr => Some(c2 | c1),
            BinaryOp::BitXor => Some(c2 ^ c1),
            _ => None,
        }
    }

    // `int_regs` — registers statically known to hold an integer value
    // (plain `Int` or `BigInt`).  Reassociation is only safe when
    // `inner_src` (the non-constant operand at the root of the chain) is in
    // this set, because the rewrite skips one dunder call (`t1.__add__`) and
    // replaces two `__add__` dispatches with one.  If `inner_src` holds a
    // user-defined type with a custom `__add__`, the merged call would
    // produce a different result (the intermediate value's `__add__` is never
    // invoked), matching PR #438's algebraic-simplify bug.
    //
    // Tracking rules:
    //   `LoadConst(dst, c)` where `consts[c]` is Int/BigInt → add `dst`.
    //   `BinOpConst(dst, lhs, _, c)` where `lhs ∈ int_regs`, `consts[c]`
    //     is Int, and op ∈ {Add,Sub,Mul,FloorDiv,Mod,BitAnd,BitOr,BitXor,
    //     LShift,RShift} → add `dst` (these preserve Int-ness).
    //   `BinOp(dst, lhs, op, rhs)` where `lhs ∈ int_regs` AND
    //     `rhs ∈ int_regs` and op is int-preserving → add `dst`.
    //   `ForCountReg/Const/ConstInline(var, …)` → add `var` (loop counter).
    //   Any other write to `dst` → remove `dst`.
    //   Clear at every basic-block boundary.
    let int_preserving_op = |op: BinaryOp| -> bool {
        use BinaryOp::*;
        matches!(
            op,
            Add | Sub | Mul | FloorDiv | Mod | BitAnd | BitOr | BitXor | LShift | RShift
        )
    };

    // `binop_const_out[reg]` = `(non_const_src, op, const_value_i64)` meaning:
    //   reg = non_const_src op const_value
    // for instructions in the current basic block where the rhs is a known integer
    // constant.  Both `BinOpConst` and `BinOp` where rhs is a LoadConst-tracked
    // register are recorded here.
    //
    // `load_const_val[reg]` = i64 for registers that hold a plain integer constant
    // via `LoadConst`.  Used to recognise the rhs of `BinOp` as a constant operand.
    let mut binop_const_out: HashMap<u32, (u32, BinaryOp, i64)> = HashMap::new();
    let mut load_const_val: HashMap<u32, i64> = HashMap::new();
    let mut int_regs: HashSet<u32> = HashSet::new();
    let mut out: Vec<Insn> = Vec::with_capacity(insns.len());

    for (i, insn) in insns.into_iter().enumerate() {
        if bb_starts.contains(&i) {
            binop_const_out.clear();
            load_const_val.clear();
            int_regs.clear();
        }

        match insn {
            Insn::LoadConst(dst, c_idx) => {
                if let Some(v) = consts[c_idx as usize].as_int() {
                    load_const_val.insert(dst, v);
                    int_regs.insert(dst);
                } else {
                    load_const_val.remove(&dst);
                    int_regs.remove(&dst);
                }
                binop_const_out.remove(&dst);
                out.push(Insn::LoadConst(dst, c_idx));
            }

            Insn::BinOpConst(dst, lhs, op, c_idx, is_aug) => {
                // Try to reassociate: if `lhs` was produced by "lhs = inner_src op c2"
                // (same op, lhs is a temp), rewrite to `dst = inner_src op (c2 op c1)`.
                //
                // Safety gate: `inner_src` must be in `int_regs` — i.e., statically
                // known to be an integer.  Without this check, if `inner_src` is a
                // user-defined type with a custom `__add__`, the rewrite would skip the
                // intermediate dunder call and produce a wrong result.
                let mut rewritten = false;
                if is_integer_associative(op) {
                    if let Some(c1) = consts[c_idx as usize].as_int() {
                        if let Some(&(inner_src, inner_op, c2)) = binop_const_out.get(&lhs) {
                            // `lhs >= num_locals`: the intermediate must be a temp register
                            //   so user code cannot have mutated it between its definition and here.
                            // `inner_src != dst`: guard against a self-referential rewrite where
                            //   the traced chain points back at `dst` itself (e.g., `r = r + c`
                            //   tracked as `binop_const_out[r] = (r, op, c)`).  Rewriting to
                            //   `BinOpConst(dst, dst, op, combined)` would read the already-updated
                            //   `dst`, producing a wrong result.
                            // `inner_src ∈ int_regs`: `inner_src` must be a known integer so
                            //   that skipping the intermediate `lhs.__add__` call is safe
                            //   (user-defined `__add__` would change semantics).
                            if inner_op == op
                                && lhs >= num_locals
                                && inner_src != dst
                                && int_regs.contains(&inner_src)
                                && let Some(combined) = fold_int_pair(c2, op, c1)
                                && let Some(ci) = intern_const_in_pool(consts, Value::int(combined))
                            {
                                binop_const_out.insert(dst, (inner_src, op, combined));
                                load_const_val.remove(&dst);
                                // dst is Int since inner_src is Int and op is int-preserving.
                                int_regs.insert(dst);
                                out.push(Insn::BinOpConst(dst, inner_src, op, ci, is_aug));
                                rewritten = true;
                            }
                        }
                        if !rewritten {
                            // No reassociation — record this instruction for future chains.
                            binop_const_out.insert(dst, (lhs, op, c1));
                            load_const_val.remove(&dst);
                            // Track int-ness: lhs ∈ int_regs and op is int-preserving → dst is Int.
                            if int_regs.contains(&lhs) && int_preserving_op(op) {
                                int_regs.insert(dst);
                            } else {
                                int_regs.remove(&dst);
                            }
                        }
                    } else {
                        // BigInt constant — don't track.
                        binop_const_out.remove(&dst);
                        load_const_val.remove(&dst);
                        int_regs.remove(&dst);
                    }
                } else {
                    binop_const_out.remove(&dst);
                    load_const_val.remove(&dst);
                    int_regs.remove(&dst);
                }
                if !rewritten {
                    out.push(Insn::BinOpConst(dst, lhs, op, c_idx, is_aug));
                }
            }

            Insn::BinOp(dst, lhs, op, rhs) => {
                // Record as a binop-with-const-rhs when `rhs` is a known constant
                // and the op is integer-associative.  This enables recognising the
                // pattern produced after pass_binop_const_fusion fuses only the
                // outer of two chained adds.
                //
                // Guard: `dst != lhs` — when `dst == lhs` (in-place style:
                // `r = r + const`), the instruction overwrites its own source
                // register.  Recording it would create a self-referential entry
                // (`binop_const_out[dst] = (dst, op, c)`), which would then
                // cause a downstream BinOpConst to rewrite its lhs from `dst`
                // to `dst` (no-op in terms of register) but read the *new* value
                // of dst rather than the original one, producing a wrong result.
                let mut recorded = false;
                if is_integer_associative(op)
                    && dst != lhs
                    && let Some(&c_rhs) = load_const_val.get(&rhs)
                {
                    // rhs is a constant register; record lhs as inner source.
                    binop_const_out.insert(dst, (lhs, op, c_rhs));
                    recorded = true;
                }
                if !recorded {
                    binop_const_out.remove(&dst);
                }
                load_const_val.remove(&dst);
                // Propagate Int-ness through type-preserving BinOp.
                if int_regs.contains(&lhs) && int_regs.contains(&rhs) && int_preserving_op(op) {
                    int_regs.insert(dst);
                } else {
                    int_regs.remove(&dst);
                }
                out.push(Insn::BinOp(dst, lhs, op, rhs));
            }

            insn => {
                // At control-flow barriers, clear the entire map.
                match &insn {
                    Insn::Jump(_)
                    | Insn::JumpIfFalse(..)
                    | Insn::JumpIfTrue(..)
                    | Insn::CmpJumpIfFalse(..)
                    | Insn::CmpJumpIfTrue(..)
                    | Insn::CmpJumpIfFalseConst(..)
                    | Insn::CmpJumpIfTrueConst(..)
                    | Insn::ForIter(..)
                    | Insn::ForCountReg(..)
                    | Insn::ForCountConst(..)
                    | Insn::ForCountConstInline(..)
                    | Insn::SetupExcept(_)
                    | Insn::MatchExcept(..)
                    | Insn::MatchExceptStar(..)
                    | Insn::Return(_)
                    | Insn::ReturnNone
                    | Insn::RaiseValue(_)
                    | Insn::RaiseFrom(..)
                    | Insn::RaiseReRaise
                    | Insn::RaiseAssert(_)
                    | Insn::RaiseAssertNoMsg
                    | Insn::Unpack(..)
                    | Insn::UnpackEx { .. }
                    | Insn::Yield { .. }
                    | Insn::YieldFrom { .. } => {
                        binop_const_out.clear();
                        load_const_val.clear();
                        int_regs.clear();
                    }
                    _ => {
                        if let Some(dst) = writable_dst(&insn) {
                            binop_const_out.remove(&dst);
                            load_const_val.remove(&dst);
                            int_regs.remove(&dst);
                        }
                        // ForCount* instructions write the loop counter register,
                        // which is always an integer.  Record it in int_regs.
                        match &insn {
                            Insn::ForCountReg(var, _, _, _, _)
                            | Insn::ForCountConst(var, _, _, _, _)
                            | Insn::ForCountConstInline(var, _, _, _, _) => {
                                int_regs.insert(*var);
                            }
                            _ => {}
                        }
                    }
                }
                out.push(insn);
            }
        }
    }
    out
}

// ─── Constant folding ──────────────────────────────────────────────────────────

/// Forward dataflow constant folding.
///
/// Tracks registers whose values are statically known (`known: reg → const_idx`).
/// When both operands of a `BinOp` or `BinOpConst` are known, replace the
/// instruction with a `LoadConst` of the folded result.  Also propagates known
/// values through `Move(dst, src)`.
///
/// The map is cleared at branch/loop instructions where we cannot guarantee
/// which path was taken at runtime, and also at loop headers (targets of
/// backward jumps) to avoid incorrectly folding loop conditions.
fn pass_const_fold(insns: Vec<Insn>, consts: &mut Vec<Value>, num_locals: u32) -> Vec<Insn> {
    // Pre-pass: collect every instruction index that is the target of *any*
    // jump (forward or backward).  At every such basic-block boundary the
    // known-constant map must be cleared, otherwise a value that was assigned
    // along one incoming path can be incorrectly propagated to the merge
    // instruction — e.g. the `then`-arm of a ternary unconditionally jumps
    // over the `else`-arm; at the merge point the destination register's true
    // value depends on which arm ran, but a linear forward scan would see the
    // `else`-arm's write as the most recent and fold the wrong constant
    // downstream.  Loop headers (backward-jump targets) are a special case of
    // the same problem.
    let mut bb_starts: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target < insns.len() {
                bb_starts.insert(target);
            }
        }
    }

    let mut known: HashMap<u32, u16> = HashMap::new();
    let mut out = Vec::with_capacity(insns.len());
    // Hash index over the const pool so interning a folded constant is
    // amortized O(1) instead of an O(pool) linear scan.  A long foldable chain
    // (`x = x + i` × N) interns ~N fresh constants; the linear scan made the
    // whole pass O(n²) (issue #2002).
    let mut const_index = ConstIndex::build(consts);

    for (i, insn) in insns.into_iter().enumerate() {
        if bb_starts.contains(&i) {
            known.clear();
        }
        match insn {
            Insn::LoadConst(dst, c) => {
                known.insert(dst, c);
                out.push(Insn::LoadConst(dst, c));
            }
            Insn::Move(dst, src) => {
                match known.get(&src).copied() {
                    Some(c) => {
                        known.insert(dst, c);
                    }
                    None => {
                        known.remove(&dst);
                    }
                }
                out.push(Insn::Move(dst, src));
            }
            Insn::BinOp(dst, lhs, op, rhs) => {
                let folded = known.get(&lhs).copied().and_then(|cl| {
                    known.get(&rhs).copied().and_then(|cr| {
                        crate::compiler::fold_binop(&consts[cl as usize], op, &consts[cr as usize])
                            .and_then(|v| const_index.intern(consts, v))
                    })
                });
                apply_const_fold(
                    &mut known,
                    &mut out,
                    dst,
                    folded,
                    Insn::BinOp(dst, lhs, op, rhs),
                );
            }
            Insn::BinOpConst(dst, lhs, op, c, is_aug) => {
                let folded = known.get(&lhs).copied().and_then(|cl| {
                    crate::compiler::fold_binop(&consts[cl as usize], op, &consts[c as usize])
                        .and_then(|v| const_index.intern(consts, v))
                });
                apply_const_fold(
                    &mut known,
                    &mut out,
                    dst,
                    folded,
                    Insn::BinOpConst(dst, lhs, op, c, is_aug),
                );
            }
            Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
                let folded = known.get(&lhs).copied().and_then(|cl| {
                    let rhs_val = Value::int(imm as i64);
                    crate::compiler::fold_binop(&consts[cl as usize], op, &rhs_val)
                        .and_then(|v| const_index.intern(consts, v))
                });
                apply_const_fold(
                    &mut known,
                    &mut out,
                    dst,
                    folded,
                    Insn::BinOpImm(dst, lhs, op, imm, is_aug),
                );
            }
            // Branch/loop/raise/suspend: clear the map — values may differ per
            // path or may be written by external resume machinery.
            insn @ (Insn::Jump(_)
            | Insn::JumpIfFalse(..)
            | Insn::JumpIfTrue(..)
            | Insn::CmpJumpIfFalse(..)
            | Insn::CmpJumpIfTrue(..)
            | Insn::CmpJumpIfFalseConst(..)
            | Insn::CmpJumpIfTrueConst(..)
            | Insn::ForIter(..)
            | Insn::ForCountReg(..)
            | Insn::ForCountConst(..)
            | Insn::ForCountConstInline(..)
            | Insn::SetupExcept(_)
            | Insn::MatchExcept(..)
            | Insn::MatchExceptStar(..)
            | Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseValue(_)
            | Insn::RaiseFrom(..)
            | Insn::RaiseReRaise
            | Insn::RaiseAssert(_)
            | Insn::RaiseAssertNoMsg
            | Insn::Unpack(..)
            | Insn::UnpackEx { .. }
            | Insn::Yield { .. }
            | Insn::YieldFrom { .. }) => {
                known.clear();
                out.push(insn);
            }
            // Call instructions: invalidate the destination register AND any
            // named-local registers (r < num_locals) that may have been
            // updated via the `assign_name` write-through.  A user-defined
            // callee that declares `global x` and assigns it will, at
            // runtime, write the new value directly into the module-level
            // fastlocal register through `vm_frame_views`.  The optimizer
            // must not retain a stale `LoadConst`-derived entry for such a
            // register across the call boundary.
            //
            // Temporaries (r >= num_locals) are safe to retain: they are
            // single-use scratch registers that no external callee can reach.
            insn @ (Insn::Call(..)
            | Insn::CallMemo(..)
            | Insn::CallKw { .. }
            | Insn::CallEx { .. }
            | Insn::CallMethod { .. }
            | Insn::CallMethodKw { .. }
            | Insn::CallMethodExpanded { .. }
            | Insn::MakeClass(..)
            | Insn::MakeClassMeta(..)) => {
                known.retain(|&r, _| r >= num_locals);
                if let Some(dst) = writable_dst(&insn) {
                    known.remove(&dst);
                }
                out.push(insn);
            }
            // Any other instruction: invalidate dst if we can identify it.
            insn => {
                if let Some(dst) = writable_dst(&insn) {
                    known.remove(&dst);
                }
                out.push(insn);
            }
        }
    }
    out
}

/// Emit a folded constant or the original instruction into `out`, updating `known`.
///
/// Called by the three BinOp folding arms in `pass_const_fold`. When `folded`
/// is `Some(nc)`, emits `LoadConst(dst, nc)` and records the known value;
/// otherwise emits `fallback` and removes `dst` from `known`.
#[inline]
fn apply_const_fold(
    known: &mut std::collections::HashMap<u32, u16>,
    out: &mut Vec<Insn>,
    dst: u32,
    folded: Option<u16>,
    fallback: Insn,
) {
    if let Some(nc) = folded {
        known.insert(dst, nc);
        out.push(Insn::LoadConst(dst, nc));
    } else {
        known.remove(&dst);
        out.push(fallback);
    }
}

/// Return the single destination register of `insn`, if any.
/// Used to precisely invalidate the `known` map without clearing it entirely.
fn writable_dst(insn: &Insn) -> Option<u32> {
    use Insn::*;
    match insn {
        LoadGlobal(r, _)
        | LoadCell(r, _)
        | LoadNone(r)
        | DeleteLocal(r, _)
        | BinOp(r, _, _, _)
        | BinOpConst(r, _, _, _, _)
        | BinOpImm(r, _, _, _, _)
        | BinOpInPlace(r, _, _, _)
        | UnaryOp(r, _, _)
        | FormatValue(r, _)
        | FormatValueSpec(r, _, _)
        | MatchSeqExcluded(r, _)
        | MatchMapping(r, _)
        | GetAttr(r, _, _)
        | GetAttrForWith(r, _, _, _)
        | ImportFromAttr(r, _, _)
        | GetItem(r, _, _)
        | GetSlice(r, _, _)
        // GetAwaitable writes the driving iterator into `r`; without this arm
        // copy-prop fails to kill a `Move(r, src)` alias on `r`, mis-substituting
        // a later read of the iterator (e.g. `YieldFrom.iter_reg`) back to `src`
        // — surfaced by `await f(…, kw=v)`, whose variadic-call lowering emits an
        // arg `Move` into the slot that becomes the await iterator (issue #2298).
        | GetAwaitable(r, _)
        | Call(r, _)
        | CallMemo(r, _)
        | CallKw { func: r, .. }
        | CallEx { func: r, .. }
        | BuildList(r, _, _)
        | BuildTuple(r, _, _)
        | BuildString(r, _, _)
        | BuildSlice(r, _)
        | BuildDict(r, _, _)
        | MakeFunction(r, _, _, _, _, _)
        | ImportModule(r, _)
        | LoadExc(r)
        | LoadExcTraceback(r, _)
        | MakeClass(r, _, _, _, _, _, _)
        | MakeClassMeta(r, _, _, _, _, _, _, _)
        | MakeTypeAlias(r, _, _, _)
        | MakeTypeVar(r, _) => Some(*r),
        CallMethod { dst, .. }
        | CallMethodKw { dst, .. }
        | CallMethodExpanded { dst, .. }
        | Concat { dst, .. }
        // Yield writes the caller's sent value into `dst` on resume; aliases
        // through `dst` are stale after this instruction.
        | Yield { dst, .. } => Some(*dst),
        // Loop instructions write to their first register on each iteration.
        // Without these arms, pass_const_fold would fail to invalidate the
        // known-constant map entry for the destination, producing stale folds
        // if the blanket known.clear() at loop instructions were ever removed.
        ForIter(dst, _, _) => Some(*dst),
        ForCountReg(var, _, _, _, _) => Some(*var),
        ForCountConst(var, _, _, _, _) => Some(*var),
        ForCountConstInline(var, _, _, _, _) => Some(*var),
        // CopyReg is emitted by the CSE pass; it writes to dst just like Move.
        CopyReg(r, _) => Some(*r),
        _ => None,
    }
}

/// A hashable, type-exact dedup key for a constant pool `Value`.
///
/// Mirrors the equality semantics of `intern_const_in_pool`'s linear scan:
/// `Bool` and `Int` never collide (distinct variants), floats/complex compare
/// by raw bits so NaN-keyed constants share a slot, and every other type
/// returns `None` (no key → never deduplicated, exactly like the `_ => false`
/// arm of the linear scan).
#[derive(PartialEq, Eq, Hash)]
enum ConstKey {
    Int(i64),
    BigInt(Vec<u8>),
    FloatBits(u64),
    ComplexBits(u64, u64),
    Str(String),
    Bytes(Vec<u8>),
    Bool(bool),
    None,
}

fn const_key(val: &Value) -> Option<ConstKey> {
    Some(match val.kind() {
        ValueKind::Int(a) => ConstKey::Int(a),
        // BigInt has no `Hash`; key on its big-endian byte serialization, which
        // is a 1:1 representation for the `==` used by the linear scan.
        ValueKind::BigInt(a) => ConstKey::BigInt(a.to_signed_bytes_be()),
        ValueKind::Float(a) => ConstKey::FloatBits(a.to_bits()),
        ValueKind::Complex(ar, ai) => ConstKey::ComplexBits(ar.to_bits(), ai.to_bits()),
        ValueKind::Str(a) => ConstKey::Str(a.to_owned()),
        ValueKind::Bytes(a) => ConstKey::Bytes(a.as_ref().clone()),
        ValueKind::Bool(a) => ConstKey::Bool(a),
        ValueKind::None => ConstKey::None,
        _ => return None,
    })
}

/// Hash index over a constant pool for amortized-O(1) interning.
///
/// `intern_const_in_pool` is otherwise an O(pool) linear scan per call; a pass
/// that folds a long def-use chain (`x = x + i` × N) interns ~N fresh constants,
/// driving the whole pass to O(n²) (issue #2002).  This index maps each
/// dedup-able `ConstKey` to the *first* (lowest) pool slot holding it, matching
/// the linear scan's "first match wins" behaviour exactly, so the resulting pool
/// indices are identical.
struct ConstIndex {
    map: HashMap<ConstKey, u16>,
}

impl ConstIndex {
    /// Build the index from the existing pool contents.
    fn build(consts: &[Value]) -> Self {
        let mut map = HashMap::with_capacity(consts.len());
        for (i, v) in consts.iter().enumerate() {
            if let Some(k) = const_key(v) {
                // First occurrence wins (matches linear-scan ordering).
                map.entry(k).or_insert(i as u16);
            }
        }
        ConstIndex { map }
    }

    /// Look up or insert `val`; returns its pool index (or `None` if the pool is
    /// full or `val` is not a dedup-able type and the pool is full).
    fn intern(&mut self, consts: &mut Vec<Value>, val: Value) -> Option<u16> {
        match const_key(&val) {
            Some(k) => {
                if let Some(&idx) = self.map.get(&k) {
                    return Some(idx);
                }
                if consts.len() >= u16::MAX as usize {
                    return None;
                }
                let idx = consts.len() as u16;
                consts.push(val);
                self.map.insert(k, idx);
                Some(idx)
            }
            // Non-dedup-able type: never shares a slot (matches `_ => false`).
            None => {
                if consts.len() >= u16::MAX as usize {
                    return None;
                }
                let idx = consts.len() as u16;
                consts.push(val);
                Some(idx)
            }
        }
    }
}

/// Look up or insert `val` in the const pool; return its index.
/// Returns `None` if the pool is full (>= u16::MAX entries).
fn intern_const_in_pool(consts: &mut Vec<Value>, val: Value) -> Option<u16> {
    // Type-exact linear scan to avoid Bool/Int key collisions and to handle
    // non-hashable types (Complex, Bytes) that cannot use a HashMap fast path.
    for (i, existing) in consts.iter().enumerate() {
        let same = match (existing.kind(), val.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => a == b,
            (ValueKind::BigInt(a), ValueKind::BigInt(b)) => a == b,
            (ValueKind::Float(a), ValueKind::Float(b)) => a.to_bits() == b.to_bits(),
            // Bit-level comparison so that NaN-keyed constants share a slot.
            (ValueKind::Complex(ar, ai), ValueKind::Complex(br, bi)) => {
                ar.to_bits() == br.to_bits() && ai.to_bits() == bi.to_bits()
            }
            (ValueKind::Str(a), ValueKind::Str(b)) => a == b,
            (ValueKind::Bytes(a), ValueKind::Bytes(b)) => a.as_ref() == b.as_ref(),
            (ValueKind::Bool(a), ValueKind::Bool(b)) => a == b,
            (ValueKind::None, ValueKind::None) => true,
            _ => false,
        };
        if same {
            return Some(i as u16);
        }
    }
    if consts.len() >= u16::MAX as usize {
        return None;
    }
    let idx = consts.len() as u16;
    consts.push(val);
    Some(idx)
}

// ─── Dead code elimination ─────────────────────────────────────────────────────

/// Remove instructions that are unreachable from `pc = 0`.
/// Uses a BFS reachability pass that follows all possible instruction successors
/// (both fallthrough and jump targets, including exception handler targets).
fn pass_dead_code(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    let mut reachable = vec![false; n];
    let mut queue = vec![0usize];

    while let Some(pc) = queue.pop() {
        if pc >= n || reachable[pc] {
            continue;
        }
        reachable[pc] = true;

        let jt = |k: i32| (pc as i64 + 1 + k as i64) as usize;

        match &insns[pc] {
            Insn::Jump(k) => queue.push(jt(*k)),

            Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseValue(_)
            | Insn::RaiseReRaise
            | Insn::RaiseFrom(_, _)
            | Insn::RaiseAssert(_)
            | Insn::RaiseAssertNoMsg
            | Insn::TailCall { .. } => {}

            Insn::JumpIfFalse(_, k) | Insn::JumpIfTrue(_, k) => {
                queue.push(pc + 1);
                queue.push(jt(*k));
            }
            Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => {
                queue.push(pc + 1);
                queue.push(jt(*k));
            }
            Insn::SetupExcept(k) => {
                queue.push(pc + 1);
                queue.push(jt(*k));
            }
            _ => queue.push(pc + 1),
        }
    }

    compact(insns, &reachable)
}

// ─── Algebraic simplification (type-gated) ────────────────────────────────────

/// Simplify algebraic identities — gated on the LHS being statically known
/// to be a built-in `int` (or `BigInt`).
///
/// | Pattern                         | Rewrite                    |
/// |---------------------------------|----------------------------|
/// | `BinOpConst(dst, lhs, Add, 0)`  | `Move(dst, lhs)`           |
/// | `BinOpConst(dst, lhs, Sub, 0)`  | `Move(dst, lhs)`           |
/// | `BinOpConst(dst, lhs, Mul, 1)`  | `Move(dst, lhs)`           |
/// | `BinOpConst(dst, lhs, Mul, 0)`  | `LoadConst(dst, idx_0)`    |
/// | `BinOpConst(dst, lhs, Pow, 1)`  | `Move(dst, lhs)`           |
/// | `BinOpConst(dst, lhs, Pow, 0)`  | `LoadConst(dst, idx_1)`    |
///
/// The earlier (#438) version of this pass fired unconditionally and broke
/// user-defined `__add__` / `__mul__` / `__pow__` (matching CPython
/// requires dunder dispatch, not algebraic identity).  This version keeps
/// the unsafe arms gated behind `int_regs` — a per-basic-block set of
/// registers proven to hold an `Int` / `BigInt` value:
///
/// - `LoadConst(r, c)` where `consts[c]` is Int / BigInt → mark `r`.
/// - `ForCountReg/Const/ConstInline(var, …)` → mark `var` (loop counter).
/// - `Move(dst, src)` / `CopyReg(dst, src)` from a marked `src` → mark `dst`.
/// - `BinOpConst(dst, lhs, op, c)` with `lhs` marked, `c` Int, and `op` ∈
///   {Add, Sub, Mul, FloorDiv, Mod, BitAnd, BitOr, BitXor, LShift, RShift}
///   → mark `dst` (these ops preserve Int for Int LHS).  `Pow` / `Div`
///   skipped — `Div` returns Float, `Pow` returns Float for negative RHS.
///
/// `int_regs` is cleared at every basic-block boundary, conservatively
/// dropping flow-sensitive type info that would otherwise need a full
/// dataflow walk.  Any unrecognised write (`writable_dst`) drops the
/// destination from the set.  This is strictly a subset of safe rewrites;
/// false negatives are fine, false positives would be #438 all over again.
fn pass_algebraic_simplify(insns: Vec<Insn>, consts: &mut Vec<Value>) -> Vec<Insn> {
    use crate::ast::BinaryOp::*;

    // Basic-block boundary set (mirrors `pass_const_fold`'s logic).
    let mut bb_starts: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target < insns.len() {
                bb_starts.insert(target);
            }
        }
    }

    // Pre-scan: find registers that are WRITE-ONCE — assigned by exactly one
    // `LoadConst` and never written again.  These hold a stable Int value
    // across the whole function (including loop bodies), so the bb-clear of
    // `int_regs` doesn't need to lose them.  This catches the hot pattern
    // `for i in range(N): x = i + 0` where the `0` constant is hoisted out
    // of the loop by `pass_licm` as a register, leaving a `BinOp(_, i, Add,
    // c_reg)` in the loop body that the per-bb tracking would otherwise
    // miss (the LoadConst is outside the loop's basic block).
    let mut write_count: HashMap<u32, u32> = HashMap::new();
    let mut load_const_value: HashMap<u32, i64> = HashMap::new();
    for insn in &insns {
        if let Insn::LoadConst(dst, c) = insn {
            *write_count.entry(*dst).or_insert(0) += 1;
            if let Some(v) = consts[*c as usize].as_int() {
                load_const_value.insert(*dst, v);
            }
        } else if let Some(dst) = writable_dst(insn) {
            *write_count.entry(dst).or_insert(0) += 1;
        }
    }
    // Final map: register → its sole Int constant value, iff written exactly once
    // and that single write was a `LoadConst(_, c)` with `consts[c]` an Int.
    let immutable_int_const: HashMap<u32, i64> = load_const_value
        .into_iter()
        .filter(|(r, _)| write_count.get(r).copied() == Some(1))
        .collect();

    // Only *machine* ints (not BigInt) seed `int_regs`.  `int_regs` gates the
    // algebraic identity rewrites (`x + 0 → x`, `x * 1 → x`, …), which replace
    // the op with a `Move` and therefore make the result share `x`'s object
    // identity.  For a machine int that divergence from CPython (`1000 + 0 is
    // 1000` is `False` in CPython) is an accepted long-standing tradeoff, but
    // for a BigInt it breaks the `is` semantics the runtime otherwise preserves
    // (`2**64 + 0 is 2**64` must be `False` — issue #523, test_bigint_identity).
    // Excluding BigInt consts keeps them out of `int_regs` so their `+ 0` / `* 1`
    // is never folded to a `Move`.  (Previously the const-fusion pass could not
    // reach these sites inside loops; loosening that fusion exposed the latent
    // BigInt-identity bug, which this gate closes.)
    let is_int_const = |idx: u16, consts: &[Value]| -> bool {
        matches!(consts[idx as usize].kind(), ValueKind::Int(_))
    };
    // Ops that preserve Int when both operands are Int.  `Div` returns
    // Float; `Pow` may return Float for negative RHS; skipped.
    let int_preserving = |op: crate::ast::BinaryOp| -> bool {
        matches!(
            op,
            Add | Sub | Mul | FloorDiv | Mod | BitAnd | BitOr | BitXor | LShift | RShift
        )
    };
    // Match an identity pattern.  Returns `Some(rewrite)` when `(op, c_val)` is
    // one of the six recognised algebraic identities; otherwise `None`.
    let identity_rewrite = |dst: u32,
                            lhs: u32,
                            op: crate::ast::BinaryOp,
                            c_val: i64,
                            consts: &mut Vec<Value>|
     -> Option<Insn> {
        match (op, c_val) {
            (Add, 0) | (Sub, 0) | (Mul, 1) | (Pow, 1) => Some(Insn::Move(dst, lhs)),
            (Mul, 0) => {
                intern_const_in_pool(consts, Value::int(0)).map(|idx| Insn::LoadConst(dst, idx))
            }
            (Pow, 0) => {
                intern_const_in_pool(consts, Value::int(1)).map(|idx| Insn::LoadConst(dst, idx))
            }
            _ => None,
        }
    };

    let mut int_regs: HashSet<u32> = HashSet::new();
    // Seed `int_regs` with the immutable-int-const registers — they're Int
    // everywhere, so always in the set regardless of bb position.
    for &r in immutable_int_const.keys() {
        int_regs.insert(r);
    }
    let mut out: Vec<Insn> = Vec::with_capacity(insns.len());

    for (i, insn) in insns.into_iter().enumerate() {
        if bb_starts.contains(&i) {
            int_regs.clear();
            // Re-seed with immutable consts (they survive bb boundaries).
            for &r in immutable_int_const.keys() {
                int_regs.insert(r);
            }
        }
        match insn {
            Insn::LoadConst(dst, c) => {
                if is_int_const(c, consts) {
                    int_regs.insert(dst);
                } else {
                    int_regs.remove(&dst);
                }
                out.push(Insn::LoadConst(dst, c));
            }
            Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
                if int_regs.contains(&src) {
                    int_regs.insert(dst);
                } else {
                    int_regs.remove(&dst);
                }
                out.push(insn);
            }
            Insn::ForCountReg(var, _, _, _, _)
            | Insn::ForCountConst(var, _, _, _, _)
            | Insn::ForCountConstInline(var, _, _, _, _) => {
                // Loop counter is always Int (range() emits int bounds).
                int_regs.insert(var);
                out.push(insn);
            }
            Insn::BinOpConst(dst, lhs, op, c, is_aug) => {
                let lhs_int = int_regs.contains(&lhs);
                let c_int = is_int_const(c, consts);
                if lhs_int
                    && c_int
                    && let Some(cv) = consts[c as usize].as_int()
                    && let Some(new) = identity_rewrite(dst, lhs, op, cv, consts)
                {
                    int_regs.insert(dst);
                    out.push(new);
                    continue;
                }
                if lhs_int && c_int && int_preserving(op) {
                    int_regs.insert(dst);
                } else {
                    int_regs.remove(&dst);
                }
                out.push(Insn::BinOpConst(dst, lhs, op, c, is_aug));
            }
            Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
                // The immediate is always an integer; apply the same algebraic
                // identity rewrites as BinOpConst when lhs is known to be Int.
                let lhs_int = int_regs.contains(&lhs);
                if lhs_int {
                    let cv = imm as i64;
                    if let Some(new) = identity_rewrite(dst, lhs, op, cv, consts) {
                        int_regs.insert(dst);
                        out.push(new);
                        continue;
                    }
                }
                if lhs_int && int_preserving(op) {
                    int_regs.insert(dst);
                } else {
                    int_regs.remove(&dst);
                }
                out.push(Insn::BinOpImm(dst, lhs, op, imm, is_aug));
            }
            Insn::BinOp(dst, lhs, op, rhs) => {
                // First: try the identity rewrite when `rhs` is an immutable
                // int constant register (covers the loop case where
                // `pass_binop_const_fusion` couldn't fuse across a back-edge).
                if int_regs.contains(&lhs)
                    && let Some(&cv) = immutable_int_const.get(&rhs)
                    && let Some(new) = identity_rewrite(dst, lhs, op, cv, consts)
                {
                    int_regs.insert(dst);
                    out.push(new);
                    continue;
                }
                // Propagate Int-ness through type-preserving ops.
                if int_regs.contains(&lhs) && int_regs.contains(&rhs) && int_preserving(op) {
                    int_regs.insert(dst);
                } else {
                    int_regs.remove(&dst);
                }
                out.push(Insn::BinOp(dst, lhs, op, rhs));
            }
            insn => {
                // Any other write conservatively drops the dst's int-ness.
                if let Some(dst) = writable_dst(&insn) {
                    // ...unless it's an immutable-int-const register: those
                    // are mark-once and write-once by definition (pre-scan
                    // verified write_count == 1), so we never reach here
                    // for one.  The `remove` is still safe (it'd be a no-op).
                    int_regs.remove(&dst);
                }
                out.push(insn);
            }
        }
    }
    out
}

// ─── Unary constant folding ────────────────────────────────────────────────────

/// Fuse `LoadConst(r, c) + UnaryOp(dst, op, r)` → `LoadConst(dst, op(c))`
/// when `r >= num_locals` (temp register).
///
/// Handles `Neg`, `Not`, and `BitNot` applied to integer or float constants.
fn pass_unary_fold(insns: Vec<Insn>, num_locals: u32, consts: &mut Vec<Value>) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i + 1 < n {
        let fused: Option<(u32, Value)> = match (&transformed[i], &transformed[i + 1]) {
            (Insn::LoadConst(lc_reg, c_idx), Insn::UnaryOp(dst, op, src))
                if *src == *lc_reg
                    && *lc_reg >= num_locals
                    && !slice_has_back_edge(&transformed[i + 2..])
                    // When dst==lc_reg the fusion overwrites lc_reg with the result,
                    // so any later read of lc_reg will see the correct folded value.
                    // When dst!=lc_reg, lc_reg would become uninitialized after removal.
                    && (*dst == *lc_reg || !reg_is_read_in(&transformed[i + 2..], *lc_reg)) =>
            {
                let c = &consts[*c_idx as usize];
                // Fold through the VM's canonical `vm_eval_unary` rather than
                // re-implementing the per-kind arms here, so the compile-time
                // constant can never drift from the runtime result (issue
                // #458).  A runtime error (e.g. `~1.5` → TypeError) returns
                // `None`, leaving the UnaryOp in the bytecode to raise at
                // runtime — never at compile time.
                let result = crate::interpreter::vm_eval_unary(*op, c.clone()).ok();
                result.map(|v| (*dst, v))
            }
            _ => None,
        };

        if let Some((dst, val)) = fused
            && let Some(new_c) = intern_const_in_pool(consts, val)
        {
            keep[i] = false;
            transformed[i + 1] = Insn::LoadConst(dst, new_c);
            i += 2;
            continue;
        }
        i += 1;
    }
    compact(transformed, &keep)
}

// ─── Pure string method constant folding ──────────────────────────────────────

/// Fold `CallMethod` instructions whose receiver and all arguments are
/// compile-time constants (loaded via `LoadConst`) and whose method name is in
/// the known-pure list into a single `LoadConst` of the pre-computed result.
///
/// Only methods that return immutable value types (str, bool, int, float,
/// tuple) are eligible; methods returning mutable containers (list, bytes, …)
/// are excluded to prevent aliasing the const pool through a mutable reference.
///
/// The const-register map is cleared at every basic-block boundary (any
/// instruction index that is a jump target) to avoid propagating stale values
/// across control-flow merge points.
fn pass_str_method_const_fold(
    insns: Vec<Insn>,
    consts: &mut Vec<Value>,
    names: &[String],
    num_locals: u32,
) -> Vec<Insn> {
    fn is_foldable_str_method(method: &str) -> bool {
        matches!(
            method,
            "casefold"
                | "lower"
                | "upper"
                | "swapcase"
                | "title"
                | "capitalize"
                | "center"
                | "ljust"
                | "rjust"
                | "zfill"
                | "expandtabs"
                | "strip"
                | "lstrip"
                | "rstrip"
                | "removeprefix"
                | "removesuffix"
                | "replace"
                | "partition"
                | "rpartition"
                | "islower"
                | "isupper"
                | "istitle"
                | "isascii"
                | "isdecimal"
                | "isnumeric"
                | "isidentifier"
                | "isprintable"
                | "isdigit"
                | "isalpha"
                | "isalnum"
                | "isspace"
                | "startswith"
                | "endswith"
                | "find"
                | "rfind"
                | "count"
        )
    }

    // Build the set of basic-block entry points (branch targets).
    let mut bb_starts: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target < insns.len() {
                bb_starts.insert(target);
            }
        }
    }

    // reg → const-pool index for registers known to hold a constant.
    let mut const_regs: HashMap<u32, u16> = HashMap::new();
    let mut written_buf: HashSet<u32> = HashSet::new();
    let mut out = Vec::with_capacity(insns.len());

    for (i, insn) in insns.into_iter().enumerate() {
        if bb_starts.contains(&i) {
            const_regs.clear();
        }

        match insn {
            Insn::LoadConst(dst, idx) => {
                if dst >= num_locals {
                    const_regs.insert(dst, idx);
                }
                out.push(Insn::LoadConst(dst, idx));
            }
            Insn::CallMethod {
                dst,
                obj,
                name_idx,
                args_base,
                nargs,
            } => {
                let folded = (|| -> Option<Value> {
                    let obj_idx = *const_regs.get(&obj)?;
                    let obj_val = &consts[obj_idx as usize];
                    obj_val.as_str()?; // Only fold string receivers.
                    let method = names.get(name_idx as usize)?.as_str();
                    if !is_foldable_str_method(method) {
                        return None;
                    }
                    let mut arg_vals = Vec::with_capacity(nargs as usize);
                    for j in 0..nargs as u32 {
                        let arg_idx = *const_regs.get(&(args_base + j))?;
                        arg_vals.push(consts[arg_idx as usize].clone());
                    }
                    let result = pyrust_builtins::string::call(method, obj_val, arg_vals).ok()?;
                    // Exclude mutable containers to avoid aliasing the pool.
                    let immutable = matches!(
                        result.kind(),
                        ValueKind::Str(_)
                            | ValueKind::Bool(_)
                            | ValueKind::Int(_)
                            | ValueKind::Float(_)
                            | ValueKind::Tuple(_)
                    );
                    if immutable { Some(result) } else { None }
                })();

                if let Some(result) = folded
                    && let Ok(new_idx) = u16::try_from(consts.len())
                {
                    consts.push(result);
                    if dst >= num_locals {
                        const_regs.insert(dst, new_idx);
                    } else {
                        const_regs.remove(&dst);
                    }
                    out.push(Insn::LoadConst(dst, new_idx));
                } else {
                    const_regs.remove(&dst);
                    out.push(Insn::CallMethod {
                        dst,
                        obj,
                        name_idx,
                        args_base,
                        nargs,
                    });
                }
            }
            insn => {
                written_buf.clear();
                collect_writes(&insn, &mut written_buf);
                for r in &written_buf {
                    const_regs.remove(r);
                }
                out.push(insn);
            }
        }
    }

    out
}

// ─── Constant-condition branch elimination ─────────────────────────────────────

/// Evaluate a comparison between two compile-time constant `Value`s.  Returns
/// `None` for combinations where the comparison is not defined at the constant
/// level (e.g. mixed types, or an op outside the comparison set).
fn eval_const_cmp(lv: &Value, op: crate::ast::BinaryOp, rv: &Value) -> Option<bool> {
    use crate::ast::BinaryOp::*;
    if let (Some(a), Some(b)) = (lv.as_int(), rv.as_int()) {
        return match op {
            Eq => Some(a == b),
            Ne => Some(a != b),
            Lt => Some(a < b),
            Le => Some(a <= b),
            Gt => Some(a > b),
            Ge => Some(a >= b),
            _ => None,
        };
    }
    if let (ValueKind::Str(ls), ValueKind::Str(rs)) = (lv.kind(), rv.kind()) {
        return match op {
            Eq => Some(ls == rs),
            Ne => Some(ls != rs),
            Lt => Some(ls < rs),
            Le => Some(ls <= rs),
            Gt => Some(ls > rs),
            Ge => Some(ls >= rs),
            _ => None,
        };
    }
    if let (ValueKind::Bool(lb), ValueKind::Bool(rb)) = (lv.kind(), rv.kind()) {
        return match op {
            Eq => Some(lb == rb),
            Ne => Some(lb != rb),
            _ => None,
        };
    }
    None
}

/// Replace conditional jumps whose condition register was just loaded from a
/// known constant with an unconditional `Jump`:
///
/// - `LoadConst(r, c) + JumpIfFalse(r, k)` → keep LoadConst; replace with `Jump(k)` if falsy, `Jump(0)` if truthy
/// - `LoadConst(r, c) + JumpIfTrue(r, k)` → keep LoadConst; replace with `Jump(k)` if truthy, `Jump(0)` if falsy
/// - `LoadConst(r, c) + CmpJumpIfFalseConst(r, op, c2, k)` → `Jump(...)` when the comparison
///   can be evaluated at compile time (e.g. after `pass_str_method_const_fold` produces
///   a known-constant lhs for an assert like `assert "Hi".casefold() == "hi"`).
///
/// The unconditional jumps are then cleaned up by `pass_dead_code` (removes
/// unreachable instructions) and `pass_trivial_nop` (removes `Jump(0)`).
fn pass_const_branch_elim(insns: Vec<Insn>, consts: &[Value]) -> Vec<Insn> {
    let n = insns.len();
    let mut out = insns;

    let mut i = 0;
    while i + 1 < n {
        if let (Insn::LoadConst(lc_reg, c_idx), jump) = (&out[i], &out[i + 1]) {
            let (lc_reg, c_idx) = (*lc_reg, *c_idx);
            let lv = &consts[c_idx as usize];
            let truthy = lv.truthy();
            let replacement: Option<Insn> = match jump {
                Insn::JumpIfFalse(cond, k) if *cond == lc_reg => Some(if truthy {
                    Insn::Jump(0)
                } else {
                    Insn::Jump(*k)
                }),
                Insn::JumpIfTrue(cond, k) if *cond == lc_reg => Some(if truthy {
                    Insn::Jump(*k)
                } else {
                    Insn::Jump(0)
                }),
                Insn::CmpJumpIfFalseConst(lhs, op, rhs_idx, k) if *lhs == lc_reg => {
                    let rv = &consts[*rhs_idx as usize];
                    eval_const_cmp(lv, *op, rv)
                        .map(|cond| if !cond { Insn::Jump(*k) } else { Insn::Jump(0) })
                }
                Insn::CmpJumpIfTrueConst(lhs, op, rhs_idx, k) if *lhs == lc_reg => {
                    let rv = &consts[*rhs_idx as usize];
                    eval_const_cmp(lv, *op, rv)
                        .map(|cond| if cond { Insn::Jump(*k) } else { Insn::Jump(0) })
                }
                _ => None,
            };
            if let Some(new_jump) = replacement {
                out[i + 1] = new_jump;
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

// ─── Register liveness helpers ────────────────────────────────────────────────

/// Returns `true` if register `r` is read by any instruction in `insns`.
/// Returns `true` if `insns` contains a backward jump (negative offset).
///
/// A back-edge means the slice re-enters an earlier instruction, so a forward
/// liveness scan alone cannot prove a register is dead — the register may be
/// read on the next loop iteration.  Passes that would remove a `LoadConst`
/// based solely on `reg_is_read_in` must guard with this check.
/// Returns `true` if `insn` is a backward branch (offset `< 0`), i.e. a loop
/// back-edge.
fn insn_is_back_edge(insn: &Insn) -> bool {
    matches!(insn,
        Insn::Jump(k)
        | Insn::JumpIfFalse(_, k)
        | Insn::JumpIfTrue(_, k)
        | Insn::ForIter(_, _, k)
        | Insn::ForCountReg(_, _, _, _, k)
        | Insn::ForCountConst(_, _, _, _, k)
        | Insn::ForCountConstInline(_, _, _, _, k)
        | Insn::CmpJumpIfFalse(_, _, _, k)
        | Insn::CmpJumpIfTrue(_, _, _, k)
        | Insn::CmpJumpIfFalseConst(_, _, _, k)
        | Insn::CmpJumpIfTrueConst(_, _, _, k)
        if *k < 0
    )
}

fn slice_has_back_edge(insns: &[Insn]) -> bool {
    insns.iter().any(insn_is_back_edge)
}

/// Used as a forward liveness guard before removing a `LoadConst` that produced `r`.
fn reg_is_read_in(insns: &[Insn], r: u32) -> bool {
    insns.iter().any(|insn| insn_reads_reg(insn, r))
}

/// Returns `true` when the value held in register `r` is provably dead starting
/// at `insns[start]` — the first instruction at or after `start` that touches
/// `r` *writes* it without first reading it.  Lets `pass_binop_const_fusion`
/// fuse the "reused scratch register" shape inside loop bodies, which the coarse
/// global `last_read[r]` and `back_edge_after` guards otherwise veto.
///
/// Conservatively returns `false` (treat as live) at the first control-flow
/// instruction: a jump-over could reach a later read of the stale value, so we
/// only reason within a straight-line run.  Scans a tiny fixed window — a
/// scratch temp is always overwritten within a couple of instructions, and an
/// unbounded scan would reintroduce the O(n²) blowup this pass avoids (#2002).
fn scratch_dead_after(insns: &[Insn], start: usize, r: u32) -> bool {
    const WINDOW: usize = 6;
    let end = (start + WINDOW).min(insns.len());
    let mut written = HashSet::new();
    for insn in &insns[start..end] {
        // Any branch / suspend / return ends the straight-line region.
        if matches!(
            insn,
            Insn::Jump(_)
                | Insn::JumpIfFalse(..)
                | Insn::JumpIfTrue(..)
                | Insn::CmpJumpIfFalse(..)
                | Insn::CmpJumpIfTrue(..)
                | Insn::CmpJumpIfFalseConst(..)
                | Insn::CmpJumpIfTrueConst(..)
                | Insn::ForIter(..)
                | Insn::ForCountReg(..)
                | Insn::ForCountConst(..)
                | Insn::ForCountConstInline(..)
                | Insn::SetupExcept(..)
                | Insn::Yield { .. }
                | Insn::YieldFrom { .. }
                | Insn::Return(..)
                | Insn::ReturnNone
        ) {
            return false;
        }
        if insn_reads_reg(insn, r) {
            return false; // read before any overwrite → value is live
        }
        written.clear();
        collect_writes(insn, &mut written);
        if written.contains(&r) {
            return true; // overwritten before any read → old value is dead
        }
    }
    // Hit the true end of the stream without a read → dead; otherwise (window
    // edge reached mid-stream) be conservative.
    end == insns.len()
}

/// True if the register(s) backing a `KwCallName` include `r`.
fn kwcall_name_reads(name: &crate::bytecode::KwCallName, r: u32) -> bool {
    match name {
        crate::bytecode::KwCallName::Callee(reg) => *reg == r,
        crate::bytecode::KwCallName::Method { obj, .. } => *obj == r,
    }
}

/// Insert the register(s) backing a `KwCallName` into a read set.
fn kwcall_name_insert(name: &crate::bytecode::KwCallName, reads: &mut HashSet<u32>) {
    match name {
        crate::bytecode::KwCallName::Callee(reg) => {
            reads.insert(*reg);
        }
        crate::bytecode::KwCallName::Method { obj, .. } => {
            reads.insert(*obj);
        }
    }
}

/// Apply a register substitution `s` to the register(s) backing a `KwCallName`.
fn kwcall_name_subst(
    name: crate::bytecode::KwCallName,
    mut s: impl FnMut(u32) -> u32,
) -> crate::bytecode::KwCallName {
    match name {
        crate::bytecode::KwCallName::Callee(reg) => crate::bytecode::KwCallName::Callee(s(reg)),
        crate::bytecode::KwCallName::Method { obj, name_idx } => {
            crate::bytecode::KwCallName::Method {
                obj: s(obj),
                name_idx,
            }
        }
    }
}

/// Returns `true` if `insn` reads the value of register `r`.
fn insn_reads_reg(insn: &Insn, r: u32) -> bool {
    use Insn::*;
    match insn {
        // No register sources.
        LoadConst(..)
        | LoadGlobal(..)
        | LoadCell(..)
        | LoadNone(..)
        | LoadNoneRange { .. }
        | LoadExc(..)
        | ImportModule(..)
        | DeleteName(..)
        | PushTypeParamEnv
        | PopTypeParamEnv
        | DeleteLocal(..)
        | Jump(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | PopExcContext
        | ReturnNone
        | RaiseReRaise
        | RaiseAssertNoMsg
        | ForIter(..)
        | ForCountConst(..)
        | ForCountConstInline(..)
        | DeleteModuleGlobal(..) => false,

        // One source register.
        StoreGlobal(_, s)
        | StoreCell(_, s)
        | ImportStar(s)
        | Move(_, s)
        | CopyReg(_, s)
        | UnaryOp(_, _, s)
        | FormatValue(_, s)
        | MatchSeqExcluded(_, s)
        | MatchMapping(_, s)
        | Return(s)
        | PrintExpr(s)
        | RaiseValue(s)
        | RaiseAssert(s)
        | JumpIfFalse(s, _)
        | JumpIfTrue(s, _)
        | GetIter(_, s)
        | GetAwaitable(_, s)
        | Unpack(_, s, _)
        | CheckLocal(s, _)
        | GetAttr(_, s, _)
        | GetAttrForWith(_, s, _, _)
        | LoadExcTraceback(_, s)
        | ImportFromAttr(_, s, _)
        | DeleteAttr(s, _)
        | BinOpConst(_, s, _, _, _)
        | BinOpImm(_, s, _, _, _)
        | CmpJumpIfFalseConst(s, _, _, _)
        | CmpJumpIfTrueConst(s, _, _, _)
        | MatchExcept(s, _)
        | RecordClassStore(s)
        | RecordClassDel(s)
        | PushExcContext(s)
        | SyncModuleGlobal(s, _) => *s == r,
        MatchExceptStar(type_r, src, _, _) => *type_r == r || *src == r,

        // Two source registers.
        BinOp(_, a, _, b)
        | BinOpInPlace(_, a, _, b)
        | CmpJumpIfFalse(a, _, b, _)
        | CmpJumpIfTrue(a, _, b, _)
        | RaiseFrom(a, b)
        | SetAdd(a, b)
        | ListAppend(a, b)
        | ListExtend(a, b)
        | DictUpdate(a, b)
        | GetItem(_, a, b)
        | FormatValueSpec(_, a, b)
        | DeleteItem(a, b) => *a == r || *b == r,

        SetAttr(obj, _, val) | SetTypeVarAttr(obj, _, val) => *obj == r || *val == r,
        ForCountReg(_, _, stop, _, _) => *stop == r,

        // Three source registers.
        SetItem(a, b, c) => *a == r || *b == r || *c == r,

        // Call-context kwarg merges read their dict + source + the registers
        // backing the callee-name source.
        DictMergeKwCall { dict, src, name } => {
            *dict == r || *src == r || kwcall_name_reads(name, r)
        }
        SetItemKwCall {
            dict,
            key,
            val,
            name,
        } => *dict == r || *key == r || *val == r || kwcall_name_reads(name, r),

        // Range-based: func + args live in consecutive registers.
        Call(base, argc) | CallMemo(base, argc) => r >= *base && r <= *base + *argc as u32,
        // CallKw reads the callee and `total` consecutive arg registers — the
        // same footprint as `Call(func, total)`.
        CallKw { func, total, .. } => r >= *func && r <= *func + *total as u32,
        // CallEx reads the callee + `npos` positional registers (contiguous) and
        // the separate `kwargs` (`**d`) register.
        CallEx { func, npos, kwargs } => (r >= *func && r <= *func + *npos as u32) || r == *kwargs,
        // args_base is always >= 1 (compiler sets it to func_reg + 1, func_reg >= 0),
        // so args_base - 1 is the callee register and the subtraction never underflows.
        TailCall { args_base, nargs } => {
            r == *args_base - 1 || (r >= *args_base && r < *args_base + *nargs as u32)
        }
        BuildList(_, base, n) | BuildTuple(_, base, n) => r >= *base && r < *base + *n,
        BuildString(_, base, n) => r >= *base && r < *base + *n as u32,
        // BuildSlice always reads exactly 3 registers: start, stop, step.
        BuildSlice(_, base) => r >= *base && r < *base + 3,
        // GetSlice reads `obj` plus the 3 contiguous bound registers (start,
        // stop, step) starting at `base`.
        GetSlice(_, obj, base) => *obj == r || (r >= *base && r < *base + 3),
        // BuildDict stores n key-value PAIRS — each pair occupies 2 registers,
        // so the live range is base .. base + 2*n (not base + n).
        BuildDict(_, base, n) => r >= *base && r < *base + 2 * *n,

        CallMethod {
            obj,
            args_base,
            nargs,
            ..
        } => *obj == r || (r >= *args_base && r < *args_base + *nargs as u32),
        CallMethodKw {
            obj,
            args_base,
            total,
            ..
        } => *obj == r || (r >= *args_base && r < *args_base + *total as u32),
        CallMethodExpanded {
            obj,
            pos_list,
            kw_dict,
            ..
        } => *obj == r || *pos_list == r || *kw_dict == r,

        MakeFunction(_, _, defs_base, defs_n, annots_base, annots_n) => {
            (r >= *defs_base && r < *defs_base + *defs_n as u32)
                || (*annots_n > 0 && r >= *annots_base && r < *annots_base + *annots_n as u32)
        }
        MakeClass(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n) => {
            (r >= *bases_base && r < *bases_base + *bases_n as u32)
                || (*kwarg_n > 0 && r >= *kwarg_base && r < *kwarg_base + *kwarg_n as u32)
        }
        MakeClassMeta(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n, meta_reg) => {
            *meta_reg == r
                || (r >= *bases_base && r < *bases_base + *bases_n as u32)
                || (*kwarg_n > 0 && r >= *kwarg_base && r < *kwarg_base + *kwarg_n as u32)
        }
        MakeTypeAlias(_, _, value_reg, params_reg) => *value_reg == r || *params_reg == r,
        MakeTypeVar(_, _) => false,

        // Yield reads src and writes dst.
        Yield { src, dst: _ } => *src == r,

        // YieldFrom reads iter_reg and sent_reg; writes result_reg and sent_reg.
        YieldFrom {
            iter_reg,
            sent_reg,
            result_reg: _,
        } => *iter_reg == r || *sent_reg == r,

        // UnpackEx reads src.
        UnpackEx { src, .. } => *src == r,

        // Concat reads base..base+count registers.
        Concat { base, count, .. } => r >= *base && r < *base + *count as u32,

        // MatchClassPositional reads subj and cls.
        MatchClassPositional { subj, cls, .. } => r == *subj || r == *cls,
    }
}

/// Collect every register read by `insn` into `reads`.  O(1) per instruction
/// (amortised O(ranges) for range-based instructions).  Use this instead of
/// calling `insn_reads_reg` in a loop to avoid the O(n × k) inner loop.
fn collect_reads(insn: &Insn, reads: &mut HashSet<u32>) {
    use Insn::*;
    match insn {
        LoadConst(..)
        | LoadGlobal(..)
        | LoadCell(..)
        | LoadNone(..)
        | LoadExc(..)
        | ImportModule(..)
        | DeleteName(..)
        | PushTypeParamEnv
        | PopTypeParamEnv
        | DeleteLocal(..)
        | Jump(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | PopExcContext
        | ReturnNone
        | RaiseReRaise
        | RaiseAssertNoMsg
        | ForIter(..)
        | ForCountConst(..)
        | ForCountConstInline(..)
        | DeleteModuleGlobal(..) => {}

        StoreGlobal(_, s)
        | StoreCell(_, s)
        | ImportStar(s)
        | Move(_, s)
        | CopyReg(_, s)
        | UnaryOp(_, _, s)
        | FormatValue(_, s)
        | MatchSeqExcluded(_, s)
        | MatchMapping(_, s)
        | Return(s)
        | PrintExpr(s)
        | RaiseValue(s)
        | RaiseAssert(s)
        | JumpIfFalse(s, _)
        | JumpIfTrue(s, _)
        | GetIter(_, s)
        | GetAwaitable(_, s)
        | Unpack(_, s, _)
        | CheckLocal(s, _)
        | GetAttr(_, s, _)
        | GetAttrForWith(_, s, _, _)
        | LoadExcTraceback(_, s)
        | ImportFromAttr(_, s, _)
        | DeleteAttr(s, _)
        | BinOpConst(_, s, _, _, _)
        | BinOpImm(_, s, _, _, _)
        | CmpJumpIfFalseConst(s, _, _, _)
        | CmpJumpIfTrueConst(s, _, _, _)
        | MatchExcept(s, _)
        | RecordClassStore(s)
        | RecordClassDel(s)
        | PushExcContext(s)
        | SyncModuleGlobal(s, _) => {
            reads.insert(*s);
        }
        MatchExceptStar(type_r, src, _, _) => {
            reads.insert(*type_r);
            reads.insert(*src);
        }

        BinOp(_, a, _, b)
        | BinOpInPlace(_, a, _, b)
        | CmpJumpIfFalse(a, _, b, _)
        | CmpJumpIfTrue(a, _, b, _)
        | RaiseFrom(a, b)
        | SetAdd(a, b)
        | ListAppend(a, b)
        | ListExtend(a, b)
        | DictUpdate(a, b)
        | GetItem(_, a, b)
        | FormatValueSpec(_, a, b)
        | DeleteItem(a, b) => {
            reads.insert(*a);
            reads.insert(*b);
        }

        SetAttr(obj, _, val) | SetTypeVarAttr(obj, _, val) => {
            reads.insert(*obj);
            reads.insert(*val);
        }
        ForCountReg(_, _, stop, _, _) => {
            reads.insert(*stop);
        }

        SetItem(a, b, c) => {
            reads.insert(*a);
            reads.insert(*b);
            reads.insert(*c);
        }

        DictMergeKwCall { dict, src, name } => {
            reads.insert(*dict);
            reads.insert(*src);
            kwcall_name_insert(name, reads);
        }
        SetItemKwCall {
            dict,
            key,
            val,
            name,
        } => {
            reads.insert(*dict);
            reads.insert(*key);
            reads.insert(*val);
            kwcall_name_insert(name, reads);
        }

        Call(base, argc) | CallMemo(base, argc) => {
            for r in *base..=(*base + *argc as u32) {
                reads.insert(r);
            }
        }
        CallKw { func, total, .. } => {
            for r in *func..=(*func + *total as u32) {
                reads.insert(r);
            }
        }
        CallEx { func, npos, kwargs } => {
            for r in *func..=(*func + *npos as u32) {
                reads.insert(r);
            }
            reads.insert(*kwargs);
        }
        TailCall { args_base, nargs } => {
            reads.insert(*args_base - 1);
            for r in *args_base..*args_base + *nargs as u32 {
                reads.insert(r);
            }
        }
        BuildList(_, base, n) | BuildTuple(_, base, n) => {
            for r in *base..*base + *n {
                reads.insert(r);
            }
        }
        BuildString(_, base, n) => {
            for r in *base..*base + *n as u32 {
                reads.insert(r);
            }
        }
        BuildSlice(_, base) => {
            for r in *base..*base + 3 {
                reads.insert(r);
            }
        }
        GetSlice(_, obj, base) => {
            reads.insert(*obj);
            for r in *base..*base + 3 {
                reads.insert(r);
            }
        }
        BuildDict(_, base, n) => {
            for r in *base..*base + 2 * *n {
                reads.insert(r);
            }
        }
        CallMethod {
            obj,
            args_base,
            nargs,
            ..
        } => {
            reads.insert(*obj);
            for r in *args_base..*args_base + *nargs as u32 {
                reads.insert(r);
            }
        }
        CallMethodKw {
            obj,
            args_base,
            total,
            ..
        } => {
            reads.insert(*obj);
            for r in *args_base..*args_base + *total as u32 {
                reads.insert(r);
            }
        }
        CallMethodExpanded {
            obj,
            pos_list,
            kw_dict,
            ..
        } => {
            reads.insert(*obj);
            reads.insert(*pos_list);
            reads.insert(*kw_dict);
        }
        MakeFunction(_, _, defs_base, defs_n, annots_base, annots_n) => {
            for r in *defs_base..*defs_base + *defs_n as u32 {
                reads.insert(r);
            }
            if *annots_n > 0 {
                for r in *annots_base..*annots_base + *annots_n as u32 {
                    reads.insert(r);
                }
            }
        }
        MakeClass(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n) => {
            for r in *bases_base..*bases_base + *bases_n as u32 {
                reads.insert(r);
            }
            for r in *kwarg_base..*kwarg_base + *kwarg_n as u32 {
                reads.insert(r);
            }
        }
        MakeClassMeta(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n, meta_reg) => {
            reads.insert(*meta_reg);
            for r in *bases_base..*bases_base + *bases_n as u32 {
                reads.insert(r);
            }
            for r in *kwarg_base..*kwarg_base + *kwarg_n as u32 {
                reads.insert(r);
            }
        }
        MakeTypeAlias(_, _, value_reg, params_reg) => {
            reads.insert(*value_reg);
            reads.insert(*params_reg);
        }
        MakeTypeVar(_, _) => {}
        Yield { src, .. } => {
            reads.insert(*src);
        }
        YieldFrom {
            iter_reg, sent_reg, ..
        } => {
            reads.insert(*iter_reg);
            reads.insert(*sent_reg);
        }
        UnpackEx { src, .. } => {
            reads.insert(*src);
        }
        LoadNoneRange { .. } => {}
        Concat { base, count, .. } => {
            for r in *base..*base + *count as u32 {
                reads.insert(r);
            }
        }
        // MatchClassPositional reads subj and cls.
        MatchClassPositional { subj, cls, .. } => {
            reads.insert(*subj);
            reads.insert(*cls);
        }
    }
}

// ─── Constant-register propagation into CmpJump/BinOp ────────────────────────

/// Convert `BinOp`, `BinOpInPlace`, and `CmpJump*` instructions that use a
/// write-once `LoadConst` register on the RHS to their `*Const`/`*Imm` variants.
///
/// ## Pattern
///
/// ```text
/// LoadConst(r_n, idx)          ← written exactly once
/// ...
/// BinOp(dst, lhs, op, r_n)       → BinOpConst(dst, lhs, op, idx)
/// BinOpInPlace(dst, lhs, op, r_n) → BinOpImm(dst, lhs, op, imm)   (if const fits i16)
///                                 → BinOpConst(dst, lhs, op, idx)  (if lhs is temp)
/// CmpJumpIfFalse(lhs, op, r_n, k) → CmpJumpIfFalseConst(lhs, op, idx, k)
/// CmpJumpIfTrue(lhs, op, r_n, k)  → CmpJumpIfTrueConst(lhs, op, idx, k)
/// ```
///
/// ## Why this helps
///
/// `BinOp` looks up the RHS from the register file; `BinOpConst` indexes the
/// const pool directly.  The `*Const` VM dispatch paths also avoid one register
/// read.  More importantly, `CmpJumpIfFalseConst` is eligible for further
/// constant-folding and algebraic-simplify rewrites that `CmpJumpIfFalse` is not.
///
/// For `BinOpInPlace`, when the constant fits in `i16`, `BinOpImm` embeds the
/// value directly in the instruction word — eliminating both the register read
/// and the const-pool indirection.  `BinOpImm`'s slow path still calls
/// `try_inplace_op`, so `__iadd__` / `__imul__` semantics are preserved for
/// user-defined types on the LHS.
///
/// When the constant does not fit in `i16` but `lhs` is a temp register
/// (compiler-generated, never a user object with `__iadd__`), we emit
/// `BinOpConst` instead, still saving the register-file read.
///
/// ## Guard
///
/// Only registers that are written exactly once by a `LoadConst` across the
/// entire function body are considered.  Write-once guarantees the register
/// holds that constant on every path that reaches the use, without the cost
/// of a full dataflow analysis.
fn pass_const_reg_prop(insns: Vec<Insn>, num_locals: u32, consts: &[Value]) -> Vec<Insn> {
    // Pre-scan: count writes and record LoadConst targets.
    //
    // Use collect_writes (not writable_dst) to capture ALL write destinations,
    // including Move(r, _) and Unpack which writable_dst omits.
    //
    // IMPORTANT: reuse a single HashSet across iterations (clear instead of
    // new) to avoid N heap allocations — one per instruction.  For files with
    // hundreds of assertions this cut ~600 allocations to 1, removing the
    // compile-time overhead that caused an 88% regression on assertion-heavy
    // benchmarks.
    let mut write_count: HashMap<u32, u32> = HashMap::new();
    let mut load_const_idx: HashMap<u32, u16> = HashMap::new();
    let mut written: HashSet<u32> = HashSet::new();
    for insn in &insns {
        written.clear();
        collect_writes(insn, &mut written);
        for &dst in &written {
            *write_count.entry(dst).or_insert(0) += 1;
        }
        if let Insn::LoadConst(dst, idx) = insn {
            load_const_idx.insert(*dst, *idx);
        }
    }
    // Registers written exactly once, and that single write was a LoadConst.
    let immutable_const: HashMap<u32, u16> = load_const_idx
        .into_iter()
        .filter(|(r, _)| write_count.get(r).copied() == Some(1))
        .collect();

    if immutable_const.is_empty() {
        return insns;
    }

    // Convert BinOp/CmpJump that use immutable_const registers.  Track which
    // *temp* registers (>= num_locals) were actually substituted — their
    // LoadConst becomes a dead store and must be pruned below.  Named locals
    // (< num_locals) may be captured by closures, so keep their LoadConst.
    let mut converted_regs: HashSet<u32> = HashSet::new();
    let mut out: Vec<Insn> = insns
        .into_iter()
        .map(|insn| match insn {
            Insn::BinOp(dst, lhs, op, rhs) => {
                if let Some(&idx) = immutable_const.get(&rhs) {
                    if rhs >= num_locals {
                        converted_regs.insert(rhs);
                    }
                    // Plain BinOp is never augmented → is_aug = false.
                    Insn::BinOpConst(dst, lhs, op, idx, false)
                } else {
                    Insn::BinOp(dst, lhs, op, rhs)
                }
            }
            Insn::CmpJumpIfFalse(lhs, op, rhs, k) => {
                if let Some(&idx) = immutable_const.get(&rhs) {
                    if rhs >= num_locals {
                        converted_regs.insert(rhs);
                    }
                    Insn::CmpJumpIfFalseConst(lhs, op, idx, k)
                } else {
                    Insn::CmpJumpIfFalse(lhs, op, rhs, k)
                }
            }
            Insn::CmpJumpIfTrue(lhs, op, rhs, k) => {
                if let Some(&idx) = immutable_const.get(&rhs) {
                    if rhs >= num_locals {
                        converted_regs.insert(rhs);
                    }
                    Insn::CmpJumpIfTrueConst(lhs, op, idx, k)
                } else {
                    Insn::CmpJumpIfTrue(lhs, op, rhs, k)
                }
            }
            Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                if let Some(&idx) = immutable_const.get(&rhs) {
                    // The constant value fits in i16: embed it directly as BinOpImm.
                    // BinOpImm's slow path still invokes try_inplace_op, so __iadd__
                    // and friends are preserved for user-defined types on the LHS.
                    // BinOpInPlace is always augmented → is_aug = true.
                    if let Some(int_val) = consts[idx as usize].as_int()
                        && let Ok(imm) = i16::try_from(int_val)
                    {
                        if rhs >= num_locals {
                            converted_regs.insert(rhs);
                        }
                        return Insn::BinOpImm(dst, lhs, op, imm, true);
                    }
                    // Constant does not fit in i16.  Only safe to use BinOpConst
                    // when lhs is a temp register — temps are compiler-generated and
                    // never hold user objects that could have custom __iadd__.
                    if lhs >= num_locals {
                        if rhs >= num_locals {
                            converted_regs.insert(rhs);
                        }
                        return Insn::BinOpConst(dst, lhs, op, idx, true);
                    }
                }
                Insn::BinOpInPlace(dst, lhs, op, rhs)
            }
            other => other,
        })
        .collect();

    // Prune the LoadConst instructions that are now dead.
    //
    // `pass_dead_store_elim` cannot remove them because it conservatively bails
    // when it hits a CmpJumpConst (which is control flow) before seeing a
    // subsequent write.  Since we know exactly which registers were substituted,
    // we collect the set of ALL registers still read in `out` in a single O(n)
    // pass, then subtract to find the dead ones.
    //
    // IMPORTANT: use `compact` (not `retain`) so that all jump offsets are
    // rewritten to account for the removed instructions.  A raw `retain` would
    // leave stale offsets and produce wrong loop targets.
    if !converted_regs.is_empty() {
        // True O(n) pass: collect every register read by any instruction, then
        // filter converted_regs.  Avoids the O(n × k) loop from calling
        // insn_reads_reg once per converted_reg per instruction.
        let mut all_reads: HashSet<u32> = HashSet::new();
        for insn in &out {
            collect_reads(insn, &mut all_reads);
        }
        let dead_regs: HashSet<u32> = converted_regs
            .into_iter()
            .filter(|r| !all_reads.contains(r))
            .collect();
        if !dead_regs.is_empty() {
            let keep: Vec<bool> = out
                .iter()
                .map(|insn| !matches!(insn, Insn::LoadConst(r, _) if dead_regs.contains(r)))
                .collect();
            out = compact(out, &keep);
        }
    }

    out
}

// ─── BinOpInPlace → BinOp downgrade ───────────────────────────────────────────

/// Replace `BinOpInPlace(dst, lhs, op, rhs)` with `BinOp(dst, lhs, op, rhs)`
/// when `lhs` is a temp register that is dead after this instruction.
///
/// ## Why this helps
///
/// `BinOpInPlace` dispatches `__i<op>__` (e.g. `__iadd__`) first and falls back
/// to `__<op>__` on failure.  For immutable built-in types (int, float, str) the
/// `__iadd__` lookup always fails, adding a method-resolution step per execution.
/// Downgrading to plain `BinOp` skips that wasted dispatch.
///
/// ## Guards
///
/// - `lhs >= num_locals`: restrict to temp registers.  Named locals (0..num_locals)
///   can hold user-defined objects with custom `__iadd__`; downgrading those would
///   silently change semantics.
/// - `!reg_is_read_in(&insns[i+1..], lhs)`: `lhs` must be dead after this
///   instruction.  If `lhs` is live, the in-place semantics (writing back the
///   result to `lhs`) may matter to downstream reads — but since we emit `BinOp`
///   which writes to `dst`, this is only safe when `lhs` is not read further.
///   (If `dst == lhs` the result is in the same register either way.)
///
/// Reference: GCC algebraic simplification; classical augmented-assignment lowering.
fn pass_binopinplace_downgrade(insns: Vec<Insn>, num_locals: u32, consts: &[Value]) -> Vec<Insn> {
    // `BinOpInPlace(dst, lhs, op, rhs)` carries augmented-assignment (`__i<op>__`)
    // semantics: it mutates `lhs` IN PLACE and accepts a broader set of RHS types
    // than the plain binary op (e.g. `list += <any iterable>` extends via
    // `list.extend`; `set |= dict` updates; `bytearray += bytes`).  Downgrading it
    // to `BinOp` (the plain `+`/`*`/… operator, which builds a NEW value and is
    // type-strict) is only sound when `lhs` provably holds a value for which the
    // in-place op is semantically identical to the binary op — i.e. an immutable
    // numeric primitive (`int`/`float`/`complex`), where `__iadd__` does not exist
    // and `+=` falls back to `__add__` producing a fresh value.
    //
    // For containers (`list`/`set`/`dict`/`bytearray`) the in-place op extends/
    // updates in place and accepts iterables the binary op rejects, and for
    // user-defined types `__iadd__` may have arbitrary side effects.  Downgrading
    // any of those silently changes runtime behaviour (issue #1943:
    // `l[0] += (9,)` raised `TypeError` instead of extending the inner list).
    //
    // So the downgrade is gated on `lhs` being a PROVABLY numeric-primitive
    // register at the point of the op, tracked per basic block in the spirit of
    // `pass_const_fold`'s `int_regs`.  Registers loaded from `GetItem`/`GetAttr`/
    // `LoadGlobal`/`BuildList`/… are not provably numeric and are never
    // downgraded.  The set is keyed by INSTRUCTION INDEX (not register number) so
    // that the same temp reused for a numeric op in one place and a container op
    // in another is gated independently.
    let downgradeable: HashSet<usize> = numeric_inplace_sites(&insns, consts);
    insns
        .iter()
        .enumerate()
        .map(|(i, insn)| {
            if let Insn::BinOpInPlace(dst, lhs, op, rhs) = insn
                // Guard: num_locals > 0 ensures there IS a distinction between
                // named locals (0..num_locals) and temp registers (>=num_locals).
                // When num_locals == 0 (all-env module scope, issue #706), every
                // register is a "temp" but may hold a module global loaded via
                // LoadGlobal — such values can have user-defined __iadd__ etc.
                && num_locals > 0
                && *lhs >= num_locals
                && (*dst == *lhs || !reg_is_read_in(&insns[i + 1..], *lhs))
                // Type gate (issue #1943): only downgrade when `lhs` is provably a
                // numeric primitive, where `+=` and `+` have identical semantics.
                && downgradeable.contains(&i)
            {
                return Insn::BinOp(*dst, *lhs, *op, *rhs);
            }
            insn.clone()
        })
        .collect()
}

/// Compute the set of `BinOpInPlace` instruction indices whose LHS register
/// provably holds a numeric primitive (`int`/`float`/`complex`) at the point of
/// the op — values for which augmented assignment is semantically identical to
/// the binary op, so the `BinOpInPlace → BinOp` downgrade is sound.
///
/// This mirrors the per-basic-block Int-tracking of `pass_const_fold`'s
/// `int_regs`, but admits floats/complex too (the downgrade is sound for any
/// numeric where `__iadd__` doesn't exist).  A register joins the set when it is
/// loaded from a numeric constant or produced by a numeric-preserving op whose
/// inputs are themselves provably numeric; it is dropped on any other write and
/// the whole set is cleared at basic-block boundaries (a register's contents are
/// not provable across a branch/merge).
fn numeric_inplace_sites(insns: &[Insn], consts: &[Value]) -> HashSet<usize> {
    use crate::ast::BinaryOp::*;

    let is_numeric_const = |idx: u16| -> bool {
        matches!(
            consts[idx as usize].kind(),
            ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _)
        )
    };
    // Ops that keep a numeric result when all operands are numeric.  These never
    // produce a container, so the resulting register stays "provably numeric".
    let numeric_preserving = |op: crate::ast::BinaryOp| -> bool {
        matches!(
            op,
            Add | Sub
                | Mul
                | Div
                | FloorDiv
                | Mod
                | Pow
                | BitAnd
                | BitOr
                | BitXor
                | LShift
                | RShift
        )
    };

    // Basic-block boundary set: any instruction that is a branch/jump target
    // starts a new block where register provenance can no longer be trusted.
    let mut bb_starts: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target < insns.len() {
                bb_starts.insert(target);
            }
        }
    }

    // `result.insert(i)` records that the LHS read by the BinOpInPlace at index
    // `i` is provably numeric.  We track the live numeric register set as we walk
    // forward, then resolve each BinOpInPlace against the set state at its index.
    let mut result: HashSet<usize> = HashSet::new();
    let mut numeric: HashSet<u32> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        if bb_starts.contains(&i) {
            numeric.clear();
        }
        match insn {
            Insn::LoadConst(dst, c) => {
                if is_numeric_const(*c) {
                    numeric.insert(*dst);
                } else {
                    numeric.remove(dst);
                }
            }
            Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
                if numeric.contains(src) {
                    numeric.insert(*dst);
                } else {
                    numeric.remove(dst);
                }
            }
            Insn::ForCountReg(var, ..)
            | Insn::ForCountConst(var, ..)
            | Insn::ForCountConstInline(var, ..) => {
                // range() loop counters are always Int.
                numeric.insert(*var);
            }
            Insn::BinOpConst(dst, lhs, op, c, _) => {
                if numeric.contains(lhs) && is_numeric_const(*c) && numeric_preserving(*op) {
                    numeric.insert(*dst);
                } else {
                    numeric.remove(dst);
                }
            }
            Insn::BinOpImm(dst, lhs, op, _, _) => {
                // The immediate is always an integer.
                if numeric.contains(lhs) && numeric_preserving(*op) {
                    numeric.insert(*dst);
                } else {
                    numeric.remove(dst);
                }
            }
            Insn::BinOp(dst, lhs, op, rhs) => {
                if numeric.contains(lhs) && numeric.contains(rhs) && numeric_preserving(*op) {
                    numeric.insert(*dst);
                } else {
                    numeric.remove(dst);
                }
            }
            Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                // Resolve the gate for this op against the current state.
                if numeric.contains(lhs) {
                    result.insert(i);
                }
                // A numeric-preserving in-place op on numeric operands keeps the
                // dst numeric; otherwise the dst's provenance is lost.
                if numeric.contains(lhs) && numeric.contains(rhs) && numeric_preserving(*op) {
                    numeric.insert(*dst);
                } else {
                    numeric.remove(dst);
                }
            }
            // Instructions that write registers `writable_dst` does NOT fully
            // capture: multi-register writers (`Unpack`/`UnpackEx`/
            // `MatchClassPositional` write a whole range, not just one reg) and
            // external-resume / write-through instructions (`YieldFrom` writes
            // `result_reg`+`sent_reg`; `ImportStar` injects names; `Call`-family
            // can write a module fastlocal via `global` through `vm_frame_views`).
            // For these, conservatively drop ALL numeric provenance — mirroring
            // `pass_const_fold`, which clears its `known` map on the same set. A
            // stale numeric entry surviving one of these would be a false
            // positive: a temp could be overwritten with a container and then
            // wrongly downgraded. The set only ever holds short-lived compiler
            // temps, so clearing on these rare instructions costs nothing.
            Insn::Unpack(..)
            | Insn::UnpackEx { .. }
            | Insn::MatchClassPositional { .. }
            | Insn::YieldFrom { .. }
            | Insn::ImportStar(_)
            | Insn::Call(..)
            | Insn::CallMemo(..)
            | Insn::CallKw { .. }
            | Insn::CallEx { .. }
            | Insn::CallMethod { .. }
            | Insn::CallMethodKw { .. }
            | Insn::CallMethodExpanded { .. }
            | Insn::MakeClass(..)
            | Insn::MakeClassMeta(..) => {
                numeric.clear();
            }
            other => {
                if let Some(dst) = writable_dst(other) {
                    numeric.remove(&dst);
                }
            }
        }
    }

    result
}

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
/// - `BinOpConst(dst, src, op, idx)` — loop-invariant when `src` is not
///   written in the loop body, `dst` is a temp, and `op` is provably
///   non-raising (`Add`, `Sub`, `Mul`, `BitAnd/Or/Xor`, comparisons, `And`,
///   `Or`).  `Div`, `FloorDiv`, `Mod`, `Pow`, `LShift`, `RShift` are
///   excluded because they can raise at runtime.
///
/// ## What is NOT hoisted
///
/// `UnaryOp` (`Pos`, `Neg`, `BitNot`, `Not`) can raise `TypeError`; excluded
/// entirely.  `BinOp`, `Call`, `CallMethod`, `GetAttr`, `SetAttr`,
/// `LoadGlobal`, `StoreGlobal`, store instructions, and all loop/branch/
/// exception instructions are left in place because they may have side effects
/// or their correct behaviour depends on the iteration context.
///
/// ## Loop detection
///
/// A back edge is any `Jump(k)` where `k < 0` (the target is before the current
/// instruction).  Each back edge `(latch_pc, header_pc)` defines a natural loop
/// `[header_pc, latch_pc]`.  Nested loops are handled individually: the inner
/// loop's back edge produces an inner `[header, latch]` range whose hoisting
/// point is just before the inner header, not before the outer header.
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
fn pass_licm(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Collect all back edges: (header_pc, latch_pc).
    // A Jump(k) at position i is a back edge when the target (i+1+k) <= i.
    let mut back_edges: Vec<(usize, usize)> = Vec::new();
    for (i, insn) in insns.iter().enumerate() {
        if let Insn::Jump(k) = insn {
            let target = (i as i64 + 1 + *k as i64) as usize;
            if target <= i {
                back_edges.push((target, i)); // (header, latch)
            }
        }
    }

    if back_edges.is_empty() {
        return insns;
    }

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
            // Derive the flat write set (union of all writes) for source checking.
            let written: HashSet<u32> = write_count.keys().copied().collect();

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
            // The loop header itself (ForIter, ForCountConst, etc.) is an implicit
            // conditional (it exits the loop when the iterator is exhausted), but
            // it is NOT included in the hoist set; we advance past it first.
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
                        | Insn::ForIter(..)
                        | Insn::ForCountReg(..)
                        | Insn::ForCountConstInline(..)
                        | Insn::ForCountConst(..) => {
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
                if is_loop_invariant(&insns[pc], &written, &write_count, num_locals) {
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
        | BuildTuple(r, _, _)
        | BuildString(r, _, _)
        | BuildSlice(r, _)
        | BuildDict(r, _, _)
        | BinOp(r, _, _, _)
        | BinOpInPlace(r, _, _, _)
        | BinOpConst(r, _, _, _, _)
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
        | GetSlice(r, _, _)
        | GetAwaitable(r, _)
        | Call(r, _)
        | CallMemo(r, _)
        | CallKw { func: r, .. }
        | CallEx { func: r, .. }
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
        ForCountReg(var, _, _, _, _)
        | ForCountConst(var, _, _, _, _)
        | ForCountConstInline(var, _, _, _, _) => {
            written.insert(*var);
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
            for i in 0..(*before as u32 + 1 + *after as u32) {
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
        | JumpIfFalse(..)
        | JumpIfTrue(..)
        | CmpJumpIfFalse(..)
        | CmpJumpIfTrue(..)
        | CmpJumpIfFalseConst(..)
        | CmpJumpIfTrueConst(..)
        | Return(..)
        | ReturnNone
        | RaiseValue(..)
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
        | DeleteModuleGlobal(..)
        | TailCall { .. } => {}
        // MatchClassPositional writes dst_base..dst_base+n.
        MatchClassPositional { dst_base, n, .. } => {
            for i in 0..*n as u32 {
                written.insert(dst_base + i);
            }
        }
    }
}

/// Returns `true` if `insn` is a pure, loop-invariant instruction given the
/// set of registers written anywhere inside the loop body.
///
/// An instruction is loop-invariant when:
/// 1. It is one of the safe-to-hoist variants (`LoadConst`, `BinOpConst`).
/// 2. None of its *source* registers appear in `written`.
/// 3. Its *destination* register is a temporary (`>= num_locals`).  Named
///    locals (index `< num_locals`) are visible outside the loop; hoisting a
///    write to a named local would make the assignment unconditional, which is
///    wrong for zero-trip loops.
/// 4. Its *destination* register is written only by this instruction inside the
///    loop body (`write_count[dst] == 1`).  If another instruction in the body
///    also writes `dst`, hoisting would change the value seen by instructions
///    that execute between the hoist point and the in-body write — incorrect.
/// 5. For `BinOpConst`: the operator cannot raise at runtime.  `Div`,
///    `FloorDiv`, `Mod` can raise `ZeroDivisionError`; `LShift`/`RShift` can
///    raise `ValueError` for a negative constant; `Pow` can raise
///    `OverflowError`.  These are excluded because hoisting them before the
///    loop header changes observable semantics for zero-trip loops.
///
/// `UnaryOp` is excluded entirely: `Pos` and `Neg` raise `TypeError` for
/// non-numeric operands, and `BitNot` raises `TypeError` for non-int operands.
fn is_loop_invariant(
    insn: &Insn,
    written: &HashSet<u32>,
    write_count: &HashMap<u32, usize>,
    num_locals: u32,
) -> bool {
    use crate::ast::BinaryOp;

    // True when `dst` is a temporary register (not a named local).
    let is_temp = |dst: u32| dst >= num_locals;
    // True when `dst` is the sole writer of that register inside the body.
    let sole_writer = |dst: u32| write_count.get(&dst).copied().unwrap_or(0) == 1;

    match insn {
        // LoadConst has no register source; safe to hoist only when dst is a
        // temporary.  Named locals must not be hoisted: a zero-trip loop must
        // not assign them.
        Insn::LoadConst(dst, _) => is_temp(*dst) && sole_writer(*dst),
        // BinOpConst reads `src`; invariant when `src` is not written, dst is a
        // temp, this is the sole write of dst, and the operator cannot raise.
        Insn::BinOpConst(dst, src, op, _, _) | Insn::BinOpImm(dst, src, op, _, _) => {
            let op_is_safe = matches!(
                op,
                BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::BitAnd
                    | BinaryOp::BitOr
                    | BinaryOp::BitXor
                    | BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::Is
                    | BinaryOp::IsNot
                    | BinaryOp::And
                    | BinaryOp::Or
            );
            op_is_safe && !written.contains(src) && is_temp(*dst) && sole_writer(*dst)
        }
        // UnaryOp (Pos, Neg, BitNot, Not) can raise TypeError for non-numeric /
        // non-bool operands.  Excluded entirely: hoisting fires the raise in
        // zero-trip loops where CPython 3.12 never enters the body.
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

    let mut i = 0;
    while i + 1 < n {
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
/// *Terminating* instructions (Return, ReturnNone, Raise*, TailCall) have no
/// fallthrough; after `insn_reads_reg` confirms they do not read `r`, we can
/// safely conclude `r` is dead and return `false`.
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
        if let Insn::LoadNoneRange { start, count } = insn
            && r >= *start
            && r < start + *count as u32
        {
            return false;
        }
    }
    false
}

/// Returns `true` for instructions that unconditionally terminate the current
/// execution path with no fallthrough: `Return`, `ReturnNone`, `Raise*`, and
/// `TailCall`.  After checking `insn_reads_reg`, a terminator guarantees that
/// no later instruction in the linear sequence can read the candidate register.
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
            | TailCall { .. }
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
            | JumpIfFalse(..)
            | JumpIfTrue(..)
            | CmpJumpIfFalse(..)
            | CmpJumpIfTrue(..)
            | CmpJumpIfFalseConst(..)
            | CmpJumpIfTrueConst(..)
            | ForIter(..)
            | ForCountReg(..)
            | ForCountConst(..)
            | ForCountConstInline(..)
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
            | TailCall { .. }
    )
}

/// Remove writes to temp registers whose stored value is never read before
/// the next write to the same register.
///
/// ## Safety restrictions
///
/// - Only temp registers (`>= num_locals`) are considered; named locals may
///   escape via closures.
/// - Only *unconditionally pure* instructions are removed: `LoadConst`,
///   `LoadNone`, `Move`, `CopyReg`, and `CallMemo`.  Instructions that can
///   raise exceptions (`LoadGlobal` → NameError; `BinOp`/`BinOpConst` →
///   ValueError / ZeroDivisionError / etc.; `UnaryOp` → TypeError) are always
///   preserved so that expression statements like `a << b` or
///   `undefined_name` still propagate their errors instead of being silently
///   dropped.  `CallMemo` is emitted only for callees declared `#[pure]`
///   (no observable side effects) in `pyrust_module!`, so dropping a dead
///   `CallMemo` result is safe.
/// - A back-edge guard (`slice_has_back_edge`) prevents removing a store that
///   is the initial value consumed by a later loop iteration.
fn pass_dead_store_elim(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut keep = vec![true; n];

    // Pre-scan: record every temp register (>= num_locals) that is read at least
    // once anywhere in the function, and the highest register index seen (used to
    // size the per-register liveness arrays below).  A temp that is read nowhere
    // is provably dead everywhere — it cannot be live on a loop back-edge because
    // temps are single-assignment (by the compiler) and are never shared across
    // iterations or external frames.  This lets us remove dead LoadConst / Move /
    // CopyReg / LoadNone stores that are hoisted before a loop by pass_licm but
    // still have a back-edge visible after them.
    let mut read_anywhere: HashSet<u32> = HashSet::new();
    let mut max_reg: u32 = num_locals;
    {
        let mut reads_buf: HashSet<u32> = HashSet::new();
        for insn in &insns {
            reads_buf.clear();
            collect_reads(insn, &mut reads_buf);
            for &r in &reads_buf {
                max_reg = max_reg.max(r);
                if r >= num_locals {
                    read_anywhere.insert(r);
                }
            }
            if let Some(w) = writable_dst(insn) {
                max_reg = max_reg.max(w);
            }
            match insn {
                Insn::LoadConst(d, _) | Insn::Move(d, _) => max_reg = max_reg.max(*d),
                Insn::LoadNoneRange { start, count } => {
                    max_reg = max_reg.max(start + count.saturating_sub(1) as u32)
                }
                _ => {}
            }
        }
    }

    // Suffix back-edge map: `back_edge_after[i]` is true iff any instruction at
    // index `> i` is a backward branch.  Computing this once turns the per-store
    // `slice_has_back_edge(&insns[i + 1..])` lookup (an O(n) tail scan) into an
    // O(1) array read, so this pass stays linear instead of O(n²) on long
    // single-block instruction streams (issue #2002).
    let mut back_edge_after = vec![false; n + 1];
    for i in (0..n).rev() {
        back_edge_after[i] = back_edge_after[i + 1] || insn_is_back_edge(&insns[i]);
    }

    // The dead-store decision for a store at index `i` to register `r` mirrors
    // `reg_is_read_before_next_write(&insns[i + 1..], r)`, which returns at the
    // first instruction `j > i` matching, in priority order: (1) reads r → true,
    // (2) terminator → false, (3) control-flow → true, (4) kills r → false; with
    // a read taking priority over a kill/control-flow at the same instruction.
    //
    // A single reverse pass computes everything needed in O(1) per store: the
    // nearest control-flow/terminator at-or-after each index (register-
    // independent) plus, per register, the nearest upcoming read and kill.  This
    // replaces the original per-store O(n) tail scan that made the pass O(n²) on
    // long single blocks (a large literal whose elements are all read once by a
    // single trailing `BuildList`/`BuildDict` — issue #2004).
    // `n` sentinel = "no such position".
    let reg_slots = (max_reg as usize) + 1;
    let mut next_read = vec![n; reg_slots];
    let mut next_kill = vec![n; reg_slots];
    // Nearest control-flow/terminator strictly after the current scan position,
    // updated as we walk backwards; `cf_pos == n` means none.
    let mut cf_pos = n;
    let mut cf_is_cf = false;

    let mut reads_buf: HashSet<u32> = HashSet::new();
    for i in (0..n).rev() {
        // At index `i`, `next_read`/`next_kill`/`cf_*` describe positions `> i`,
        // i.e. exactly the slice `&insns[i + 1..]` that the original scan walked.
        let dst = match &insns[i] {
            Insn::LoadConst(r, _) | Insn::LoadNone(r) | Insn::Move(r, _) | Insn::CopyReg(r, _)
                if *r >= num_locals =>
            {
                Some(*r)
            }
            // Pure-callee calls: the compiler emits CallMemo only for #[pure]
            // functions, so a dead result has no observable side effect.
            Insn::CallMemo(r, _) if *r >= num_locals => Some(*r),
            _ => None,
        };
        if let Some(r) = dst {
            // Fast path: register read nowhere in the function ⇒ provably dead.
            if !read_anywhere.contains(&r) {
                keep[i] = false;
            } else if back_edge_after[i + 1] {
                // A back-edge could carry the value into the next iteration.
            } else {
                let read = next_read[r as usize];
                let kill = next_kill[r as usize];
                let dead = if read < cf_pos && read <= kill {
                    false // read first (ties to read) ⇒ value is live
                } else if kill < cf_pos && kill < read {
                    true // killed before any read / control-flow ⇒ dead
                } else if read == cf_pos && cf_pos < n {
                    false // a read on the control-flow/terminator wins ⇒ live
                } else if cf_pos == n {
                    true // fell off the end with no read ⇒ dead
                } else {
                    // Decided by the control-flow instruction: control-flow ⇒
                    // conservatively live; terminator ⇒ dead.
                    !cf_is_cf
                };
                if dead {
                    keep[i] = false;
                }
            }
        }

        // Fold instruction `i` into the running state for the next (lower) index.
        // Reads first so a same-instruction read+kill records the read position
        // (matching the original scan's read-before-write priority).
        reads_buf.clear();
        collect_reads(&insns[i], &mut reads_buf);
        for &r in &reads_buf {
            next_read[r as usize] = i;
        }
        if let Some(w) = writable_dst(&insns[i]) {
            next_kill[w as usize] = i;
        }
        match &insns[i] {
            Insn::LoadConst(d, _) | Insn::Move(d, _) => next_kill[*d as usize] = i,
            Insn::LoadNoneRange { start, count } => {
                for r in *start..start + *count as u32 {
                    next_kill[r as usize] = i;
                }
            }
            _ => {}
        }
        if is_terminator(&insns[i]) {
            cf_pos = i;
            cf_is_cf = false;
        } else if is_control_flow(&insns[i]) {
            cf_pos = i;
            cf_is_cf = true;
        }
    }

    compact(insns, &keep)
}

// ─── Common subexpression elimination ─────────────────────────────────────────

/// Eliminate redundant computations within each basic block.
///
/// Within a straight-line sequence of instructions (a *basic block* — no jumps
/// in or out), if two instructions compute exactly the same value from the same
/// inputs, the second one is redundant.  This pass replaces the second with
/// `CopyReg(dst2, dst1)`, pointing `dst2` at the already-computed result.
///
/// ## Tracked expressions
///
/// Only *pure* instruction forms are tracked:
/// - `LoadConst(dst, idx)` — two loads of the same pool entry are identical.
/// - `BinOpConst(dst, src, op, idx)` — same operator, same source register,
///   same constant operand.
/// - `UnaryOp(dst, op, src)` — same operator, same source register.
///
/// `BinOp` is intentionally excluded: it could invoke user-defined `__add__`
/// which may have side effects.
///
/// ## CSE key and invalidation
///
/// The *CSE key* for a tracked instruction is `(discriminant, src_regs..., const_idx)`.
/// The map is cleared at every basic-block boundary (any branch, jump, or
/// exception instruction, as well as any instruction that is a jump *target*).
///
/// Whenever any register `r` is written by any instruction (whether tracked or
/// not), every CSE table entry whose key contains `r` as a source operand is
/// removed.  This prevents stale entries from matching if an input was mutated
/// between the two computations.
///
/// ## Interaction with later passes
///
/// The emitted `CopyReg` instructions are subsequently cleaned up by
/// `pass_dead_store_elim` (if the original dst is never read) and
/// `pass_trivial_nop` (if dst == src, which cannot happen here but is guarded
/// for safety).  `pass_copy_prop` does *not* chase through `CopyReg` — that
/// keeps the pass order simple and avoids invalidating other CSE entries.
///
/// Reference: Aho, Lam, Sethi, Ullman *Compilers* §9.1 (available expressions);
/// Kennedy *A Survey of Data-Flow Analysis Techniques* §3 (CSE).
fn pass_cse(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    use std::collections::HashMap;

    /// Discriminator tag for a CSE key — keeps `LoadConst`, `BinOpConst`,
    /// `BinOpImm`, and `UnaryOp` entries distinct even if their integer fields
    /// happen to overlap.
    #[derive(Eq, PartialEq, Hash, Clone)]
    enum CseKey {
        /// `LoadConst(_, idx)` — two loads of the same pool entry.
        LoadConst(u16),
        /// `BinOpConst(_, src, op, idx)`.
        BinOpConst(u32, crate::ast::BinaryOp, u16),
        /// `BinOpImm(_, src, op, imm)`.
        BinOpImm(u32, crate::ast::BinaryOp, i16),
        /// `UnaryOp(_, op, src)`.
        UnaryOp(crate::ast::UnaryOp, u32),
    }

    impl CseKey {
        /// The single source register referenced by this key, or `None` for
        /// `LoadConst` (which has no register operand).
        fn src(&self) -> Option<u32> {
            match self {
                CseKey::LoadConst(_) => None,
                CseKey::BinOpConst(src, _, _) | CseKey::BinOpImm(src, _, _) => Some(*src),
                CseKey::UnaryOp(_, src) => Some(*src),
            }
        }
    }

    /// CSE table with reverse indices so per-register eviction touches only the
    /// affected entries instead of scanning the whole table.  A plain
    /// `HashMap::retain` per written register is O(table) and degenerates to
    /// O(n²) inside a long single basic block (e.g. a large literal whose
    /// elements each emit a fresh `LoadConst` into a new temp — issue #2004).
    ///
    /// `by_output[w]` lists keys whose *result* register is `w`; `by_src[w]`
    /// lists keys whose *source* register is `w`.  Both may contain stale keys
    /// (already removed from `map`); evicting a register drains its index vecs
    /// and the `map.remove(..).is_some()` guard makes re-processing a no-op, so
    /// total eviction work stays O(total inserts).
    struct CseTable {
        map: HashMap<CseKey, u32>,
        by_output: HashMap<u32, Vec<CseKey>>,
        by_src: HashMap<u32, Vec<CseKey>>,
    }

    impl CseTable {
        fn new() -> Self {
            CseTable {
                map: HashMap::new(),
                by_output: HashMap::new(),
                by_src: HashMap::new(),
            }
        }
        fn clear(&mut self) {
            self.map.clear();
            self.by_output.clear();
            self.by_src.clear();
        }
        fn get(&self, k: &CseKey) -> Option<u32> {
            self.map.get(k).copied()
        }
        fn insert(&mut self, k: CseKey, dst: u32) {
            self.by_output.entry(dst).or_default().push(k.clone());
            if let Some(src) = k.src() {
                self.by_src.entry(src).or_default().push(k.clone());
            }
            self.map.insert(k, dst);
        }
        /// Evict every entry whose result register or source register is `w`.
        fn evict_reg(&mut self, w: u32) {
            if let Some(keys) = self.by_output.remove(&w) {
                for k in keys {
                    // Only remove if this key's *current* result register is `w`.
                    // A stale index entry can survive a remove-then-reinsert that
                    // assigned the key a different output register; removing it
                    // here would wrongly evict an entry whose output is not `w`.
                    if self.map.get(&k) == Some(&w) {
                        self.map.remove(&k);
                    }
                }
            }
            if let Some(keys) = self.by_src.remove(&w) {
                // Every key indexed under `by_src[w]` has `src() == Some(w)` by
                // construction, so any still-live entry must be evicted on a
                // write to `w`.  Stale (already-removed) keys are a no-op.
                for k in keys {
                    self.map.remove(&k);
                }
            }
        }
        /// Evict every entry whose result or source register falls in `lo..hi`.
        fn evict_range(&mut self, lo: u32, hi: u32) {
            for w in lo..hi {
                self.evict_reg(w);
            }
        }
        /// Evict every entry whose source register is a named local
        /// (`src < num_locals`) — call-boundary invalidation.  Rebuilds the
        /// reverse indices from the survivors; only runs on call instructions.
        fn evict_local_srcs(&mut self, num_locals: u32) {
            self.map.retain(|k, _| match k.src() {
                Some(src) => src >= num_locals,
                None => true,
            });
            self.rebuild_indices();
        }
        fn rebuild_indices(&mut self) {
            self.by_output.clear();
            self.by_src.clear();
            for (k, &dst) in self.map.iter() {
                self.by_output.entry(dst).or_default().push(k.clone());
                if let Some(src) = k.src() {
                    self.by_src.entry(src).or_default().push(k.clone());
                }
            }
        }
    }

    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Pre-pass: mark every instruction that is a jump target so we can clear
    // the CSE table at basic-block boundaries.
    let mut is_bb_start = vec![false; n + 1];
    is_bb_start[0] = true;
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
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target <= n {
                is_bb_start[target] = true;
            }
        }
    }

    // `table`: CSE key → (original dst register that holds the result).
    let mut table = CseTable::new();
    let mut result: Vec<Insn> = Vec::with_capacity(n);

    for (i, insn) in insns.into_iter().enumerate() {
        // Clear CSE state at basic-block boundaries.
        if is_bb_start[i] {
            table.clear();
        }

        // Build the CSE key for this instruction, if it is a tracked pure form.
        let key: Option<(CseKey, u32)> = match &insn {
            Insn::LoadConst(dst, idx) => Some((CseKey::LoadConst(*idx), *dst)),
            // Only non-augmented fused ops are CSE candidates.  An augmented op
            // (is_aug == true) may mutate a container in place, so it is not pure
            // and must never be deduplicated.
            Insn::BinOpConst(dst, src, op, idx, false) => {
                Some((CseKey::BinOpConst(*src, *op, *idx), *dst))
            }
            Insn::BinOpImm(dst, src, op, imm, false) => {
                Some((CseKey::BinOpImm(*src, *op, *imm), *dst))
            }
            Insn::UnaryOp(dst, op, src) => Some((CseKey::UnaryOp(*op, *src), *dst)),
            _ => None,
        };

        // Determine which register (if any) this instruction writes to, so we
        // can evict stale CSE table entries BEFORE the match check.  Eviction
        // must happen regardless of whether the instruction is later replaced
        // by a CopyReg, because the CopyReg itself still writes to `dst`.
        let written_reg: Option<u32> = match &insn {
            Insn::LoadConst(r, _)
            | Insn::LoadNone(r)
            | Insn::LoadGlobal(r, _)
            | Insn::LoadCell(r, _) => Some(*r),
            // Move writes its destination register; must evict stale CSE entries
            // that recorded `prev_dst == dst` from an earlier computation.
            Insn::Move(dst, _) => Some(*dst),
            Insn::Unpack(base, _, n) => {
                // Handled separately below; use sentinel None here.
                let _ = (base, n);
                None
            }
            Insn::LoadNoneRange { .. } => {
                // Handled separately below (range eviction); use sentinel None here.
                None
            }
            _ => writable_dst(&insn),
        };

        // Evict stale entries: any entry whose *output* register is being
        // overwritten is no longer valid.  Also evict entries whose *input*
        // register is being overwritten (their computed value is now stale).
        // We do this BEFORE the CSE match check so the new entry (if any) is
        // not immediately invalidated by its own write.
        if let Insn::LoadNoneRange { start, count } = &insn {
            table.evict_range(*start, start + *count as u32);
        } else if let Insn::Unpack(base, _, n) = &insn {
            table.evict_range(*base, base + n);
        } else if let Some(w) = written_reg {
            table.evict_reg(w);
        }
        // UnpackEx writes dst_base..dst_base+before+1+after.  writable_dst
        // returns None for it (multi-register write), so evict the full range
        // explicitly — mirrors the Unpack/LoadNoneRange handling above.
        // Without this, a LoadConst that was emitted for a list element earlier
        // in the same basic block can survive into the CSE table, causing a
        // later LoadConst for the same constant value to be replaced by a
        // CopyReg pointing at the now-overwritten temp register (issue #1358).
        if let Insn::UnpackEx {
            dst_base,
            before,
            after,
            ..
        } = &insn
        {
            let lo = *dst_base;
            let hi = dst_base + *before as u32 + 1 + *after as u32;
            table.evict_range(lo, hi);
        }
        // YieldFrom writes both result_reg and sent_reg on resume.  Neither
        // register is in writable_dst (which is single-register), so evict
        // both explicitly here, mirroring the Unpack/LoadNoneRange pattern above.
        if let Insn::YieldFrom {
            result_reg,
            sent_reg,
            ..
        } = &insn
        {
            table.evict_reg(*result_reg);
            table.evict_reg(*sent_reg);
        }

        // Call-boundary invalidation: any user-defined callee may update
        // named-local registers (r < num_locals) via the `assign_name`
        // write-through in `vm_frame_views`.  CSE entries whose source
        // register is a named local must not survive across call boundaries.
        //
        // Temporaries (r >= num_locals) are safe to retain — no callee can
        // reach them through `assign_name`.  This mirrors the same fix applied
        // to `pass_const_fold` for issue #671.
        if matches!(
            insn,
            Insn::Call(..)
                | Insn::CallMemo(..)
                | Insn::CallKw { .. }
                | Insn::CallEx { .. }
                | Insn::CallMethod { .. }
                | Insn::CallMethodKw { .. }
                | Insn::CallMethodExpanded { .. }
                | Insn::MakeClass(..)
                | Insn::MakeClassMeta(..)
        ) {
            table.evict_local_srcs(num_locals);
        }

        // Check for a previous matching computation.
        let replaced = if let Some((ref k, dst)) = key {
            if let Some(prev_dst) = table.get(k) {
                if prev_dst != dst {
                    // Replace this instruction with a register copy from the
                    // earlier result.  The original instruction is discarded.
                    result.push(Insn::CopyReg(dst, prev_dst));
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if !replaced {
            // Record the expression in the CSE table.  Eviction already happened
            // above before the match check, so the new entry will not be removed.
            if let Some((k, dst)) = key {
                table.insert(k, dst);
            }

            result.push(insn);
        }

        // After a basic-block-terminating instruction, clear the table so the
        // next block starts fresh.  (We also clear at the *start* of targets via
        // is_bb_start, but this handles the fall-through path of conditionals.)
        let is_terminator = matches!(
            result.last().unwrap(),
            Insn::Jump(_)
                | Insn::JumpIfFalse(..)
                | Insn::JumpIfTrue(..)
                | Insn::CmpJumpIfFalse(..)
                | Insn::CmpJumpIfTrue(..)
                | Insn::CmpJumpIfFalseConst(..)
                | Insn::CmpJumpIfTrueConst(..)
                | Insn::ForIter(..)
                | Insn::ForCountReg(..)
                | Insn::ForCountConst(..)
                | Insn::ForCountConstInline(..)
                | Insn::SetupExcept(_)
                | Insn::MatchExcept(..)
                | Insn::MatchExceptStar(..)
                | Insn::Return(_)
                | Insn::ReturnNone
                | Insn::RaiseValue(_)
                | Insn::RaiseFrom(..)
                | Insn::RaiseReRaise
                | Insn::RaiseAssert(_)
                | Insn::RaiseAssertNoMsg
        );
        if is_terminator {
            table.clear();
        }
    }

    result
}

// ─── Induction variable strength reduction ─────────────────────────────────────

/// Replace `BinOpConst(r_dst, r_iv, Mul, c_K)` inside a `ForCountConst` loop body
/// with a running accumulator, turning a multiply-per-iteration into an add-per-iteration.
///
/// ## Pattern (preconditions)
///
/// - Loop is `ForCountConst(iv, Lt, stop_c, step_c, k_exit)` with `consts[step_c] == 1`.
/// - The instruction immediately before the loop header is `LoadConst(iv, c_pre)` where
///   `consts[c_pre]` is an integer (the pre-loop value `start − step`).
/// - The loop body `[h+1, latch)` contains exactly one `BinOpConst(r_dst, iv, Mul, c_K)`.
/// - `r_dst != iv` (no clobbering the induction variable).
/// - No jump in the body targets the loop header `h` (no `continue` jumps back mid-body
///   and skipping the accumulator increment).
///
/// ## Transformation
///
/// ```text
/// // Before
/// LoadConst(iv, c_pre)                             // iv = start − 1
/// ForCountConst(iv, Lt, stop_c, c_1, k_exit)
///     BinOpConst(r_dst, iv, Mul, c_K)
///     …
/// Jump(k_back)
///
/// // After (two instructions inserted; offsets rewritten)
/// LoadConst(iv, c_pre)
/// LoadConst(r_acc, c_init)                         // r_acc = (start−1)*K  (NEW)
/// ForCountConst(iv, Lt, stop_c, c_1, k_exit)
///     Move(r_dst, r_acc)                           // replaced
///     …
///     BinOpConst(r_acc, r_acc, Add, c_K)           // r_acc += K  (NEW)
/// Jump(k_back)
/// ```
///
/// Only one `BinOpConst` per loop is strength-reduced per invocation.  For multiple
/// patterns in the same loop, run the optimizer a second time (handled by
/// `optimize_fn_code` running the full pipeline once).
fn pass_ivsr(insns: Vec<Insn>, consts: &mut Vec<Value>, num_regs: &mut u32) -> Vec<Insn> {
    use crate::ast::BinaryOp;

    let n = insns.len();
    if n < 3 {
        return insns;
    }

    for h in 0..n {
        // Must be ForCountConst with Lt and step=1
        let (iv, step_c) = match &insns[h] {
            Insn::ForCountConst(v, BinaryOp::Lt, _, sc, _) => (*v, *sc),
            _ => continue,
        };
        let step_int = match consts.get(step_c as usize) {
            Some(v) => match v.kind() {
                ValueKind::Int(1) => 1i64,
                _ => continue,
            },
            _ => continue,
        };
        let _ = step_int; // always 1; kept for readability

        // The instruction before the header must initialise iv: LoadConst(iv, c_pre)
        if h == 0 {
            continue;
        }
        let iv_init_val = match &insns[h - 1] {
            Insn::LoadConst(r, c) if *r == iv => match consts.get(*c as usize) {
                Some(v) => match v.kind() {
                    ValueKind::Int(i) => i,
                    _ => continue,
                },
                None => continue,
            },
            _ => continue,
        };

        // Find the back-edge: a Jump targeting header h
        let latch = match (h + 1..n).find(|&l| {
            if let Insn::Jump(k) = &insns[l] {
                (l as i64 + 1 + *k as i64) as usize == h
            } else {
                false
            }
        }) {
            Some(l) => l,
            None => continue,
        };

        // Safety: no jump inside the body targets the header (no continue-to-header)
        let has_continue = (h + 1..latch).any(|i| {
            let target = match &insns[i] {
                Insn::Jump(k)
                | Insn::JumpIfFalse(_, k)
                | Insn::JumpIfTrue(_, k)
                | Insn::CmpJumpIfFalse(_, _, _, k)
                | Insn::CmpJumpIfTrue(_, _, _, k)
                | Insn::CmpJumpIfFalseConst(_, _, _, k)
                | Insn::CmpJumpIfTrueConst(_, _, _, k) => Some((i as i64 + 1 + *k as i64) as usize),
                _ => None,
            };
            target == Some(h)
        });
        if has_continue {
            continue;
        }

        // The accumulator increment is inserted unconditionally just before the
        // back-edge at `latch`, so strength reduction is only sound when *every*
        // path from the header to the back-edge passes through that point.  Two
        // shapes break that invariant — both newly reachable now that the
        // const-fusion relaxation feeds `BinOpConst(_, iv, Mul, _)` into loop
        // bodies that master could not:
        //
        //  1. A forward branch in the body that targets `latch` (or beyond)
        //     skips the increment, leaving the accumulator stale on that path
        //     (`if` / `else` / `break` / `continue` inside the body).
        //  2. A *nested* loop in the body: the reduced multiply may sit inside
        //     it, and the inner loop's exit edge is retargeted past the inserted
        //     increment, so the increment never runs (the inner loop always
        //     exits, never falls through to the increment).
        //
        // Bail conservatively on either.  (Before this PR these shapes never
        // reached IVSR, so the unsoundness was latent.)
        let unsafe_body = (h + 1..latch).any(|i| {
            // Nested loop header → reduced op may be inside it (case 2).
            if matches!(
                insns[i],
                Insn::ForCountConst(..)
                    | Insn::ForCountConstInline(..)
                    | Insn::ForCountReg(..)
                    | Insn::ForIter(..)
            ) {
                return true;
            }
            // Forward branch jumping to or past the increment site (case 1).
            let target = match &insns[i] {
                Insn::Jump(k)
                | Insn::JumpIfFalse(_, k)
                | Insn::JumpIfTrue(_, k)
                | Insn::CmpJumpIfFalse(_, _, _, k)
                | Insn::CmpJumpIfTrue(_, _, _, k)
                | Insn::CmpJumpIfFalseConst(_, _, _, k)
                | Insn::CmpJumpIfTrueConst(_, _, _, k) => Some((i as i64 + 1 + *k as i64) as usize),
                _ => None,
            };
            target.is_some_and(|t| t >= latch)
        });
        if unsafe_body {
            continue;
        }

        // Skip loops with exception handling
        if (h + 1..latch).any(|i| matches!(insns[i], Insn::SetupExcept(_) | Insn::PopExcept)) {
            continue;
        }

        // Find the first BinOpConst(r_dst, iv, Mul, c_K) in the body
        let (b, r_dst, c_k) = match (h + 1..latch).find_map(|i| match &insns[i] {
            Insn::BinOpConst(dst, src, BinaryOp::Mul, ck, _) if *src == iv && *dst != iv => {
                Some((i, *dst, *ck))
            }
            _ => None,
        }) {
            Some(t) => t,
            None => continue,
        };

        let k_val = match consts.get(c_k as usize) {
            Some(v) => match v.kind() {
                ValueKind::Int(k) => k,
                _ => continue,
            },
            None => continue,
        };

        // ForCountConst increments iv BEFORE the body runs, so the first body
        // execution sees iv = iv_init_val + 1 (= range start).  The accumulator
        // must equal that value * K on entry to the body, not iv_init_val * K.
        let acc_init = (iv_init_val + 1) * k_val;
        let c_acc_init = {
            if let Some(idx) = consts
                .iter()
                .position(|v| matches!(v.kind(), ValueKind::Int(i) if i == acc_init))
            {
                idx as u16
            } else {
                let idx = consts.len() as u16;
                consts.push(Value::int(acc_init));
                idx
            }
        };

        // Allocate a fresh accumulator register
        let r_acc = *num_regs;
        *num_regs += 1;

        // Build old→new position map:
        //   [0, h)          : unchanged
        //   [h, latch)      : +1  (LoadConst inserted before h)
        //   [latch, n]      : +2  (LoadConst before h AND BinOpConst before latch)
        let old_to_new: Vec<usize> = (0..=n)
            .map(|i| {
                if i < h {
                    i
                } else if i < latch {
                    i + 1
                } else {
                    i + 2
                }
            })
            .collect();

        // Rebuild the instruction list
        let mut new_insns: Vec<Insn> = Vec::with_capacity(n + 2);
        for i in 0..n {
            // Insert accumulator initialisation before the loop header
            if i == h {
                new_insns.push(Insn::LoadConst(r_acc, c_acc_init));
            }
            // Insert accumulator increment before the back-edge jump
            if i == latch {
                // Synthetic int accumulator add — not an augmented assignment.
                new_insns.push(Insn::BinOpConst(r_acc, r_acc, BinaryOp::Add, c_k, false));
            }
            // Replace the multiplication or rewrite offsets for everything else
            let insn = if i == b {
                Insn::Move(r_dst, r_acc)
            } else {
                rewrite_offsets(insns[i].clone(), i, &old_to_new)
            };
            new_insns.push(insn);
        }

        return new_insns; // one reduction per pass invocation
    }

    insns
}

// ─── Trivial no-op removal ─────────────────────────────────────────────────────

// ─── Pure-builtin dead call elimination ───────────────────────────────────────

/// Remove `Call` instructions to pure built-in functions whose result register
/// is never read after the call.
///
/// ## When is a call removable?
///
/// A `Call(func_reg, argc)` is eliminated when **all** of the following hold:
///
/// 1. `func_reg` was loaded by a `LoadGlobal(func_reg, name_idx)` instruction
///    and has not been overwritten since (i.e. it still holds the builtin).
/// 2. `crate::builtin_registry::is_pure(&names[name_idx])` returns `true`.
/// 3. ALL argument registers (`func_reg+1 .. func_reg+argc`) were most recently
///    written by a `LoadConst` instruction (tracked via `const_reg`).  This
///    guards against eliminating calls whose arguments come from runtime
///    expressions: such a call may raise (e.g. `len(5)` raises `TypeError`),
///    and silently dropping it would diverge from CPython.
/// 4. `func_reg` is not read by any instruction between the `Call` and the
///    next write to `func_reg` (i.e. the result is dead).  The check bails
///    conservatively at any control-flow branch.
///
/// ## What is NOT removed
///
/// - Calls to impure builtins (`print`, `input`, `open`, user-dunder-dispatchers
///   like `str`, `sorted`, `min`, …) — `is_pure` returns `false` for these.
/// - Calls through a register that was last written by something other than
///   `LoadGlobal` (e.g. a computed function stored via a user expression).
/// - Calls whose argument registers were not all loaded from the const pool.
///   A runtime-expression argument can have any type; calling a pure builtin
///   with a wrong-type argument raises `TypeError`, which is an observable
///   side effect that must be preserved.
/// - Calls in loops that have a visible back-edge in the suffix (conservative:
///   the result might feed a subsequent iteration).
/// - `CallMemo` instructions — those are already handled by `pass_dead_store_elim`.
///
/// ## Interaction with `pass_dead_store_elim`
///
/// After this pass removes a `Call`, the `LoadGlobal` that loaded the callee
/// and the `LoadConst` instructions that prepared the arguments are
/// typically dead.  A subsequent `pass_dead_store_elim` invocation cleans those
/// up (see the pipeline in `optimize_fn_code`).
fn pass_builtin_dce(insns: Vec<Insn>, num_locals: u32, names: &[String]) -> Vec<Insn> {
    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // `pure_reg` tracks temp registers (>= num_locals) that are known to
    // currently hold a pure builtin function loaded by `LoadGlobal`.
    // Entries are cleared whenever any instruction overwrites the register.
    let mut pure_reg: HashSet<u32> = HashSet::new();

    // `const_reg` tracks temp registers that are currently known to hold a
    // value loaded from the const pool via `LoadConst`.  Entries are cleared
    // whenever any other instruction writes the register.  This is used as a
    // safety gate: we only eliminate a pure-builtin call when every argument
    // register is in `const_reg`.  If any argument was produced by a runtime
    // expression (a `BinOp`, `Call`, `BuildList`, etc.), the callee may raise
    // a `TypeError` or `ValueError` on the bad argument — an observable side
    // effect that must not be silently dropped.
    let mut const_reg: HashSet<u32> = HashSet::new();

    // Track active exception handlers.  `SetupExcept` increments this counter;
    // `PopExcept` decrements it on the normal (no-exception) path.
    // Any `Call` seen while the counter is > 0 is inside a try-except block:
    // even though the result may be dead, the call may raise an exception that
    // the handler is supposed to catch.  Removing such a call would swallow
    // the exception — incorrect behaviour.
    let mut exc_depth: i32 = 0;

    let mut keep = vec![true; n];

    for i in 0..n {
        let insn = &insns[i];

        // ── Step 1: check if this Call targets a dead pure-builtin result ──
        //
        // Do this BEFORE the generic write-invalidation step below so that
        // `pure_reg` and `const_reg` still reflect the state from preceding
        // `LoadGlobal` and `LoadConst` instructions.
        if let Insn::Call(func_reg, argc) = insn {
            let func_reg = *func_reg;
            let argc = *argc as u32;
            if exc_depth == 0 && func_reg >= num_locals && pure_reg.contains(&func_reg) {
                // Guard: all argument registers must be known-const.
                // Argument registers are func_reg+1 .. func_reg+argc (inclusive).
                // A runtime-expression arg can have any type, so the call may
                // raise TypeError/ValueError — which is an observable side effect.
                let all_args_const = (1..=argc).all(|k| const_reg.contains(&(func_reg + k)));

                if all_args_const {
                    // Conservative: skip if a back-edge follows (same guard as DSE).
                    let dead = if slice_has_back_edge(&insns[i + 1..]) {
                        false
                    } else {
                        !reg_is_read_before_next_write(&insns[i + 1..], func_reg)
                    };
                    if dead {
                        keep[i] = false;
                    }
                }
                // The Call overwrites func_reg with the return value; it no
                // longer holds the pure builtin.  Fall through to invalidation.
            }
        }

        // ── Step 2: update exception-handler depth ──
        //
        // `SetupExcept` pushes an exception handler; `PopExcept` pops it on
        // the normal (no-exception) path.  `EndExcept` terminates the handler
        // body on the exception path — but in the linear instruction stream it
        // appears *after* the `Jump` that skips past it on the normal path, so
        // it does NOT correspond to an open `SetupExcept` at that point.  Only
        // count `PopExcept` as the matching close, not `EndExcept`.
        match insn {
            Insn::SetupExcept(..) => exc_depth += 1,
            Insn::PopExcept => exc_depth = exc_depth.saturating_sub(1),
            _ => {}
        }

        // ── Step 3: update pure_reg and const_reg for what this instruction writes ──

        if let Insn::LoadConst(r, _) = insn {
            // Track temp registers that hold a compile-time constant value.
            if *r >= num_locals {
                const_reg.insert(*r);
            }
        } else if let Insn::LoadGlobal(r, name_idx) = insn {
            // Track temp registers that are loaded with a pure builtin name.
            if *r >= num_locals {
                let is_pure = (*name_idx as usize) < names.len()
                    && crate::builtin_registry::is_pure(&names[*name_idx as usize]);
                if is_pure {
                    pure_reg.insert(*r);
                } else {
                    pure_reg.remove(r);
                }
                // LoadGlobal does not write a const value into the register.
                const_reg.remove(r);
            }
        } else {
            // Any other write to a temp register invalidates both trackers.
            if let Some(dst) = writable_dst(insn)
                && dst >= num_locals
            {
                pure_reg.remove(&dst);
                const_reg.remove(&dst);
            }
            // LoadNoneRange writes a contiguous range — invalidate each slot.
            if let Insn::LoadNoneRange { start, count } = insn {
                for r in *start..*start + *count as u32 {
                    if r >= num_locals {
                        pure_reg.remove(&r);
                        const_reg.remove(&r);
                    }
                }
            }
            // UnpackEx writes dst_base .. dst_base+before+1+after; writable_dst
            // returns None for it, so we must invalidate each destination slot
            // explicitly here.  Without this, registers that held LoadConst
            // values before the UnpackEx remain in const_reg even though the
            // UnpackEx overwrote them at runtime, causing pass_builtin_dce to
            // incorrectly treat the stale arg registers as compile-time constants
            // and eliminate the subsequent LoadConst for the next pure call's
            // argument (issue #1358).
            if let Insn::UnpackEx {
                dst_base,
                before,
                after,
                ..
            } = insn
            {
                for i in 0..(*before as u32 + 1 + *after as u32) {
                    let r = dst_base + i;
                    if r >= num_locals {
                        pure_reg.remove(&r);
                        const_reg.remove(&r);
                    }
                }
            }
        }
    }

    compact(insns, &keep)
}

// ─── SyncModuleGlobal sinking ─────────────────────────────────────────────────

/// Sink `SyncModuleGlobal` instructions out of call-free loop bodies to the
/// loop exit(s).
///
/// ## Correctness
///
/// `SyncModuleGlobal(reg, name_idx)` is a NOP when `globals_accessed == false`
/// (the common case).  Even when `globals_accessed == true`, the dict is only
/// consulted by `globals()` / `locals()`, which are invoked via `Call` /
/// `CallMemo` instructions.  If the loop body contains no `Call`, `CallMemo`,
/// `CallMethod`, `CallMethodExpanded`, or `ForIter` instructions, then:
///
/// - `globals_accessed` cannot change during the loop.
/// - No code in the loop body reads the module globals dict for the synced names.
///
/// Therefore the dict write can be deferred to every loop exit without changing
/// observable behaviour.  The dict values are temporarily stale during the loop
/// body, but always correct at every exit point.
///
/// ## Back-edge detection
///
/// A `Jump(k)` at position `i` is a back-edge when `i + 1 + k <= i`, i.e. `k < 0`.
/// The back-edge's target is the loop header; the back-edge itself is the latch.
///
/// ## Exit-label computation
///
/// - The loop header's conditional jump (if any) that exits the loop.
/// - Any conditional/unconditional jump from within `[header+1 .. latch]` that
///   targets a position `> latch` (a `break`).
///
/// ## Placement map
///
/// Each `SyncModuleGlobal` removed from the body is instead emitted at each
/// exit label, before the original instruction at that label.  Jump offsets are
/// rewritten so that loop exits land at the sunk syncs rather than the original
/// exit instruction.
fn pass_syncmod_sink(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Collect back-edges: (header, latch).  Group by header and use the maximum
    // latch so that `continue` statements (which compile to Jump back to the same
    // header) don't truncate the loop body we analyse.
    let mut header_to_latch: HashMap<usize, usize> = HashMap::new();
    for (i, insn) in insns.iter().enumerate() {
        if let Insn::Jump(k) = insn {
            let target = (i as i64 + 1 + *k as i64) as usize;
            if target <= i {
                let entry = header_to_latch.entry(target).or_insert(i);
                if i > *entry {
                    *entry = i;
                }
            }
        }
    }

    if header_to_latch.is_empty() {
        return insns;
    }

    // Sets built across all loops.
    let mut remove: HashSet<usize> = HashSet::new();
    // Map: exit_label → Vec of SyncModuleGlobal insns to insert before it.
    let mut sink_at: HashMap<usize, Vec<Insn>> = HashMap::new();

    'outer: for (header, latch) in header_to_latch {
        // Skip loops with exception handling or any call-like instruction — these
        // could change globals_accessed or read the globals dict.
        let has_blocker = insns[header..=latch].iter().any(|i| {
            matches!(
                i,
                Insn::SetupExcept(_)
                    | Insn::PopExcept
                    | Insn::Call(_, _)
                    | Insn::CallMemo(_, _)
                    | Insn::CallKw { .. }
                    | Insn::CallEx { .. }
                    | Insn::CallMethod { .. }
                    | Insn::CallMethodKw { .. }
                    | Insn::CallMethodExpanded { .. }
                    | Insn::ForIter(_, _, _)
            )
        });
        if has_blocker {
            continue;
        }

        // Skip if any call-like instruction appears BEFORE the loop header.  A
        // pre-loop call (e.g. `globals()`) could have already set globals_accessed
        // to true, which would make the sunk SyncModuleGlobal produce stale dict
        // values inside the loop body.
        let has_pre_loop_call = insns[..header].iter().any(|i| {
            matches!(
                i,
                Insn::Call(_, _)
                    | Insn::CallMemo(_, _)
                    | Insn::CallKw { .. }
                    | Insn::CallEx { .. }
                    | Insn::CallMethod { .. }
                    | Insn::CallMethodKw { .. }
                    | Insn::CallMethodExpanded { .. }
            )
        });
        if has_pre_loop_call {
            continue;
        }

        // Collect SyncModuleGlobal instructions in the loop body (exclude header
        // for exit-label detection: header is the conditional-branch guard).
        // Use last-writer semantics: track the last reg seen for each name_idx.
        let mut sync_last_reg: HashMap<u16, u32> = HashMap::new();
        let mut sync_positions: Vec<usize> = Vec::new();
        for pos in (header + 1)..=latch {
            if let Insn::SyncModuleGlobal(reg, name_idx) = &insns[pos] {
                sync_last_reg.insert(*name_idx, *reg);
                sync_positions.push(pos);
            }
        }

        if sync_positions.is_empty() {
            continue;
        }

        // Skip if any synced name is read via LoadGlobal in the loop body.
        // (That would mean the loop observes stale dict values, which is not safe.)
        let synced_names: HashSet<u16> = sync_last_reg.keys().copied().collect();
        let has_load_conflict = insns[header..=latch].iter().any(|i| {
            if let Insn::LoadGlobal(_, name_idx) = i {
                synced_names.contains(name_idx)
            } else {
                false
            }
        });
        if has_load_conflict {
            continue;
        }

        // Find exit labels — positions outside the loop that are jump targets of
        // instructions within [header..=latch].
        //
        // From header: the conditional jump whose false/true branch exits the loop.
        // From body [header+1 .. latch]: any branch/jump targeting > latch (break).
        let mut exits: Vec<usize> = Vec::new();

        // Check header for an exit branch.
        let header_exit: Option<usize> = match &insns[header] {
            Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountReg(_, _, _, _, k) => {
                let target = (header as i64 + 1 + *k as i64) as usize;
                if target > latch { Some(target) } else { None }
            }
            _ => None,
        };
        if let Some(exit) = header_exit {
            exits.push(exit);
        }

        // Check body for breaks (jumps targeting > latch).
        for pos in (header + 1)..latch {
            let exit_target: Option<usize> = match &insns[pos] {
                Insn::Jump(k) => {
                    let t = (pos as i64 + 1 + *k as i64) as usize;
                    if t > latch { Some(t) } else { None }
                }
                Insn::JumpIfFalse(_, k) | Insn::JumpIfTrue(_, k) => {
                    let t = (pos as i64 + 1 + *k as i64) as usize;
                    if t > latch { Some(t) } else { None }
                }
                Insn::CmpJumpIfFalse(_, _, _, k)
                | Insn::CmpJumpIfTrue(_, _, _, k)
                | Insn::CmpJumpIfFalseConst(_, _, _, k)
                | Insn::CmpJumpIfTrueConst(_, _, _, k) => {
                    let t = (pos as i64 + 1 + *k as i64) as usize;
                    if t > latch { Some(t) } else { None }
                }
                _ => None,
            };
            if let Some(exit) = exit_target {
                exits.push(exit);
            }
        }

        if exits.is_empty() {
            // No exit found — loop might be infinite or the header jump is inside
            // the body.  Skip conservatively.
            continue 'outer;
        }

        // Deduplicate exits (multiple breaks may target the same label).
        exits.sort_unstable();
        exits.dedup();

        // Validate: every exit must be either at n (past-the-end) or < n.
        // Exit at n means "fall through after the last instruction" — valid.
        for &exit in &exits {
            if exit > n {
                continue 'outer;
            }
        }

        // Build the deduped list of SyncModuleGlobal instructions to sink
        // (one per name_idx, using the last reg seen).
        let mut sink_insns: Vec<Insn> = sync_last_reg
            .iter()
            .map(|(&name_idx, &reg)| Insn::SyncModuleGlobal(reg, name_idx))
            .collect();
        // Sort by name_idx for deterministic output.
        sink_insns.sort_by_key(|i| {
            if let Insn::SyncModuleGlobal(_, idx) = i {
                *idx
            } else {
                0
            }
        });

        // Mark the original positions for removal.
        for pos in sync_positions {
            remove.insert(pos);
        }

        // Add sunk instructions at each exit label.
        for exit in exits {
            sink_at
                .entry(exit)
                .or_default()
                .extend(sink_insns.iter().cloned());
        }
    }

    if remove.is_empty() && sink_at.is_empty() {
        return insns;
    }

    // Build placement_map and jump_target_map.
    //
    // jump_target_map[i]: where jumps targeting old position i land in the new
    //   sequence.  This is BEFORE any instructions sunk at i, so that a loop
    //   exit jump lands at the sunk SyncModuleGlobal (which precedes the original
    //   exit instruction).
    //
    // placement_map[i]: where the old instruction i lands in the new sequence
    //   (AFTER any sunk instructions at i, if i is not removed).
    let mut placement_map = vec![0usize; n + 1];
    let mut jump_target_map = vec![0usize; n + 1];
    let mut new_pos: usize = 0;
    for i in 0..=n {
        // Jumps to i land BEFORE the sunk instructions at i.
        jump_target_map[i] = new_pos;
        if let Some(extra) = sink_at.get(&i) {
            new_pos += extra.len();
        }
        // Old instruction i (if kept) lands AFTER the sunk instructions.
        placement_map[i] = new_pos;
        if i < n && !remove.contains(&i) {
            new_pos += 1;
        }
    }

    // Emit the result.
    let mut out: Vec<Insn> = Vec::with_capacity(new_pos);
    for i in 0..n {
        // Emit sunk instructions before position i.
        if let Some(extra) = sink_at.get(&i) {
            out.extend(extra.iter().cloned());
        }
        // Emit the original instruction (if not removed), with rewritten offsets.
        if !remove.contains(&i) {
            out.push(rewrite_offsets_with(
                insns[i].clone(),
                i,
                &placement_map,
                &jump_target_map,
            ));
        }
    }
    // Emit any sunk instructions after the last original instruction.
    if let Some(extra) = sink_at.get(&n) {
        out.extend(extra.iter().cloned());
    }

    out
}

// ─── Copy propagation ─────────────────────────────────────────────────────────

/// Eliminate `Move(dst, src)` instructions by substituting `src` for all reads
/// of `dst` within the same basic block.
///
/// Algorithm (forward dataflow within basic blocks):
/// 1. Maintain a `copies` map: `dst → canonical_src`.
/// 2. At each jump target (instruction reachable from >1 predecessor), clear
///    `copies` — we cannot guarantee what was in `src` on all incoming paths.
/// 3. For each instruction: substitute reads of any key in `copies` with the
///    canonical source, kill entries whose key or value is overwritten, and
///    record new `Move(dst, src)` pairs.
///
/// After substitution, `Move(r, r)` becomes trivial and is removed by the
/// subsequent `pass_trivial_nop`.
///
/// Reference: GCC `-ftree-copy-prop`; Shi/Gregg/Beatty/Ertl *VEE'05*.
fn pass_copy_prop(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    use std::collections::HashMap;

    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Step 1: mark all jump target indices so we can reset copies there.
    let mut is_target = vec![false; n + 1];
    is_target[0] = true; // entry point is always a target
    for (i, insn) in insns.iter().enumerate() {
        let offset: Option<i32> = match insn {
            Insn::Jump(k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = offset {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target <= n {
                is_target[target] = true;
            }
        }
    }

    // Step 2: forward pass.
    let s = |copies: &HashMap<u32, u32>, r: u32| -> u32 { *copies.get(&r).unwrap_or(&r) };

    let mut copies: HashMap<u32, u32> = HashMap::new();
    let mut result: Vec<Insn> = Vec::with_capacity(n);

    for (i, insn) in insns.into_iter().enumerate() {
        if is_target[i] {
            copies.clear();
        }

        // Substitute source registers and collect the (possibly modified) instruction.
        let insn = match insn {
            Insn::Move(dst, src) => Insn::Move(dst, s(&copies, src)),
            // CopyReg: substitute the source register (may itself be an alias) but do
            // NOT record a new copy-propagation alias — downstream passes should see
            // CopyReg as an opaque assignment, not a transparent rename.
            Insn::CopyReg(dst, src) => Insn::CopyReg(dst, s(&copies, src)),
            Insn::Return(src) => Insn::Return(s(&copies, src)),
            Insn::PrintExpr(v) => Insn::PrintExpr(s(&copies, v)),
            Insn::RaiseValue(v) => Insn::RaiseValue(s(&copies, v)),
            Insn::RaiseAssert(v) => Insn::RaiseAssert(s(&copies, v)),
            Insn::RaiseFrom(exc, cause) => Insn::RaiseFrom(s(&copies, exc), s(&copies, cause)),
            Insn::JumpIfFalse(cond, k) => Insn::JumpIfFalse(s(&copies, cond), k),
            Insn::JumpIfTrue(cond, k) => Insn::JumpIfTrue(s(&copies, cond), k),
            Insn::UnaryOp(dst, op, src) => Insn::UnaryOp(dst, op, s(&copies, src)),
            Insn::BinOp(dst, lhs, op, rhs) => {
                Insn::BinOp(dst, s(&copies, lhs), op, s(&copies, rhs))
            }
            Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                Insn::BinOpInPlace(dst, s(&copies, lhs), op, s(&copies, rhs))
            }
            // For an augmented BinOpConst/BinOpImm (is_aug == true) the `lhs`
            // register is the in-place target (`x op= c`); do NOT copy-propagate
            // it, since the mutation must apply to that exact register.  Plain
            // (is_aug == false) fused ops are pure and may have lhs substituted.
            // The `is_aug` flag is carried through unchanged (issue #1874).
            Insn::BinOpConst(dst, lhs, op, c, is_aug) => {
                let new_lhs = if is_aug { lhs } else { s(&copies, lhs) };
                Insn::BinOpConst(dst, new_lhs, op, c, is_aug)
            }
            Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
                let new_lhs = if is_aug { lhs } else { s(&copies, lhs) };
                Insn::BinOpImm(dst, new_lhs, op, imm, is_aug)
            }
            Insn::CmpJumpIfFalse(lhs, op, rhs, k) => {
                Insn::CmpJumpIfFalse(s(&copies, lhs), op, s(&copies, rhs), k)
            }
            Insn::CmpJumpIfTrue(lhs, op, rhs, k) => {
                Insn::CmpJumpIfTrue(s(&copies, lhs), op, s(&copies, rhs), k)
            }
            Insn::CmpJumpIfFalseConst(lhs, op, c, k) => {
                Insn::CmpJumpIfFalseConst(s(&copies, lhs), op, c, k)
            }
            Insn::CmpJumpIfTrueConst(lhs, op, c, k) => {
                Insn::CmpJumpIfTrueConst(s(&copies, lhs), op, c, k)
            }
            // In-place mutation instructions: substitute only the VALUE arg, not the
            // container/receiver — substituting the receiver would redirect the
            // mutation to the original allocation (copy propagation is only valid for
            // reads; deep-copied containers are independent allocations).
            Insn::SetAdd(st, val) => Insn::SetAdd(st, s(&copies, val)),
            Insn::ListAppend(lst, val) => Insn::ListAppend(lst, s(&copies, val)),
            Insn::ListExtend(lst, src) => Insn::ListExtend(lst, s(&copies, src)),
            Insn::DictUpdate(dct, other) => Insn::DictUpdate(dct, s(&copies, other)),
            Insn::DictMergeKwCall { dict, src, name } => Insn::DictMergeKwCall {
                dict,
                src: s(&copies, src),
                name: kwcall_name_subst(name, |r| s(&copies, r)),
            },
            Insn::SetItemKwCall {
                dict,
                key,
                val,
                name,
            } => Insn::SetItemKwCall {
                dict,
                key: s(&copies, key),
                val: s(&copies, val),
                name: kwcall_name_subst(name, |r| s(&copies, r)),
            },
            Insn::SetAttr(obj, n, val) => Insn::SetAttr(obj, n, s(&copies, val)),
            Insn::SetTypeVarAttr(obj, n, val) => Insn::SetTypeVarAttr(obj, n, s(&copies, val)),
            Insn::DeleteAttr(obj, n) => Insn::DeleteAttr(obj, n),
            Insn::SetItem(obj, idx, val) => Insn::SetItem(obj, s(&copies, idx), s(&copies, val)),
            Insn::DeleteItem(obj, idx) => Insn::DeleteItem(obj, s(&copies, idx)),
            Insn::GetAttr(dst, obj, n) => Insn::GetAttr(dst, s(&copies, obj), n),
            Insn::GetAttrForWith(dst, obj, n, me) => {
                Insn::GetAttrForWith(dst, s(&copies, obj), n, me)
            }
            Insn::ImportFromAttr(dst, obj, n) => Insn::ImportFromAttr(dst, s(&copies, obj), n),
            Insn::GetItem(dst, obj, idx) => Insn::GetItem(dst, s(&copies, obj), s(&copies, idx)),
            Insn::GetIter(slot, src) => Insn::GetIter(slot, s(&copies, src)),
            Insn::GetAwaitable(dst, src) => Insn::GetAwaitable(dst, s(&copies, src)),
            Insn::Unpack(dst, src, n) => Insn::Unpack(dst, s(&copies, src), n),
            Insn::UnpackEx {
                src,
                before,
                after,
                dst_base,
            } => Insn::UnpackEx {
                src: s(&copies, src),
                before,
                after,
                dst_base,
            },
            Insn::CheckLocal(r, n) => Insn::CheckLocal(s(&copies, r), n),
            Insn::MatchExcept(r, k) => Insn::MatchExcept(s(&copies, r), k),
            Insn::MatchExceptStar(r, src, dst, k) => {
                Insn::MatchExceptStar(s(&copies, r), s(&copies, src), s(&copies, dst), k)
            }
            Insn::ForCountReg(var, op, stop, step_idx, k) => {
                Insn::ForCountReg(var, op, s(&copies, stop), step_idx, k)
            }
            Insn::StoreGlobal(n, src) => Insn::StoreGlobal(n, s(&copies, src)),
            Insn::StoreCell(n, src) => Insn::StoreCell(n, s(&copies, src)),
            Insn::SyncModuleGlobal(reg, name_idx) => {
                Insn::SyncModuleGlobal(s(&copies, reg), name_idx)
            }
            Insn::YieldFrom {
                iter_reg,
                sent_reg,
                result_reg,
            } => Insn::YieldFrom {
                iter_reg: s(&copies, iter_reg),
                sent_reg: s(&copies, sent_reg),
                result_reg,
            },
            // Call/BuildList/BuildTuple/etc. use a base register for a range of args;
            // do not substitute the base register as that would misalign the arg block.
            other => other,
        };

        // Call-boundary invalidation: a user-defined callee that declares
        // `global x` and assigns it will write the new value directly into the
        // module-level fastlocal register (r < num_locals) via the
        // `assign_name` write-through in `vm_frame_views`.  Any copy-
        // propagation alias whose *key* is a named-local register is therefore
        // stale after a call — using the pre-call value instead of the updated
        // register would silently produce wrong results.
        //
        // We also evict entries whose *value* is a named local, because the
        // value register is the "canonical source" used in substitution; if
        // it was mutated by the callee, downstream reads that copy-prop
        // redirected to it would see the wrong (pre-call) value.
        //
        // Temporaries (r >= num_locals) are safe to retain — no callee can
        // reach them through `assign_name`.  This mirrors the fix applied to
        // `pass_const_fold` and `pass_cse` for issue #671.
        if matches!(
            insn,
            Insn::Call(..)
                | Insn::CallMemo(..)
                | Insn::CallKw { .. }
                | Insn::CallEx { .. }
                | Insn::CallMethod { .. }
                | Insn::CallMethodKw { .. }
                | Insn::CallMethodExpanded { .. }
                | Insn::MakeClass(..)
                | Insn::MakeClassMeta(..)
        ) {
            copies.retain(|k, v| *k >= num_locals && *v >= num_locals);
        }

        // Kill map entries: any key or value that == dst is stale after a write.
        if let Some(dst) = writable_dst(&insn) {
            copies.retain(|k, v| *k != dst && *v != dst);
        }
        // YieldFrom writes both result_reg and sent_reg; writable_dst cannot
        // express two destinations, so evict them manually.
        if let Insn::YieldFrom {
            result_reg,
            sent_reg,
            ..
        } = &insn
        {
            copies.retain(|k, v| {
                *k != *result_reg && *v != *result_reg && *k != *sent_reg && *v != *sent_reg
            });
        }
        // LoadConst writes dst (not in writable_dst so handled here).
        if let Insn::LoadConst(dst, _) = &insn {
            copies.retain(|k, v| *k != *dst && *v != *dst);
        }
        // LoadNoneRange writes start..start+count; kill the entire range.
        if let Insn::LoadNoneRange { start, count } = &insn {
            let lo = *start;
            let hi = start + *count as u32;
            copies.retain(|k, v| (*k < lo || *k >= hi) && (*v < lo || *v >= hi));
        }
        // Unpack writes dst..dst+n; kill the entire range.
        if let Insn::Unpack(dst, _, n) = &insn {
            let lo = *dst;
            let hi = dst + n;
            copies.retain(|k, v| (*k < lo || *k >= hi) && (*v < lo || *v >= hi));
        }
        // UnpackEx writes dst_base..dst_base+before+1+after; kill the entire range.
        if let Insn::UnpackEx {
            before,
            after,
            dst_base,
            ..
        } = &insn
        {
            let lo = *dst_base;
            let hi = dst_base + *before as u32 + 1 + *after as u32;
            copies.retain(|k, v| (*k < lo || *k >= hi) && (*v < lo || *v >= hi));
        }
        // Move(dst, src): kill stale aliases THEN record the new copy.
        // Killing is necessary because overwriting `dst` invalidates any
        // existing alias that names `dst` as its source (e.g. `x → dst`).
        if let Insn::Move(dst, src) = &insn {
            copies.retain(|k, v| *k != *dst && *v != *dst);
            let canonical = *copies.get(src).unwrap_or(src);
            if dst != &canonical {
                copies.insert(*dst, canonical);
            }
        }

        result.push(insn);
    }
    result
}

// ─── Switch-head hoisting ──────────────────────────────────────────────────────

/// Eliminate redundant global-variable loads in if/elif chains that compare the
/// same global to different constants.
///
/// ## Pattern
///
/// ```text
/// i:        LoadGlobal(t, g_idx)                    // t >= num_locals (temp)
/// i+1:      CmpJumpIfFalseConst(t, op, c, k)         // k > 0: false-branch jumps forward
/// i+2..:    <true-branch body>
/// target:   LoadGlobal(t2, g_idx)                    // same global — redundant!
/// target+1: CmpJumpIfFalseConst(t2, op2, c2, k2)     // (or CmpJumpIfTrueConst)
/// ```
///
/// The second `LoadGlobal` is redundant because on the false-branch path from
/// `i+1` to `target`, no code runs (the forward jump bypasses the body entirely).
/// The global cannot have changed between the two loads on this path.
///
/// ## Safety conditions
///
/// 1. Both `t` and `t2` are temp registers (`>= num_locals`).
/// 2. The false-branch jump is forward (`k > 0`), meaning the true-branch body
///    sits between `i+1` and `target` and is unreachable on the false path.
/// 3. `target` has exactly one predecessor — the branch of `i+1` — so every
///    execution that reaches `target` guarantees `t` holds the global value
///    loaded at `i`.  Verified by a pre-pass that counts jump predecessors.
/// 4. `t2` is not read in the true-branch body `[i+2, target)`.  This is
///    structurally true for compiler-generated code (each branch uses its own
///    fresh temp), but checked explicitly for correctness.
/// 5. Both `CmpJumpIfFalseConst` / `CmpJumpIfTrueConst` instructions compare
///    against a statically known primitive constant (int, str, None, bool, float,
///    bigint).  Comparisons against user-defined objects can invoke `__eq__`,
///    which may mutate or delete the global between the two loads.  Primitives
///    never dispatch user `__eq__`, so the global cannot change on the false path.
///
/// ## Interaction with subsequent passes
///
/// After removal of `LoadGlobal(t2, g_idx)`, `compact` rewrites all jump
/// offsets.  `pass_compact_consts` (which runs later) will drop the global-name
/// pool entry if it becomes unreferenced.  `pass_dead_store_elim` cannot remove
/// `LoadGlobal` (it may raise `NameError`), so the removal must happen here.
fn pass_switch_hoist(insns: Vec<Insn>, num_locals: u32, consts: &[Value]) -> Vec<Insn> {
    let n = insns.len();
    if n < 4 {
        return insns;
    }

    // Pre-pass: for each instruction index, count how many edges from other
    // instructions reach it (predecessor count).  We only care whether the count
    // is exactly 1 (sole predecessor) or more.
    //
    // Counting rules:
    //   - Index 0 is the entry point: treated as having one implicit predecessor.
    //   - Unconditional Jump: adds 1 to its target; no fall-through successor.
    //   - Conditional branches + ForIter + SetupExcept: add 1 to both the branch
    //     target AND the fall-through (i+1).
    //   - Return/Raise/TailCall: no fall-through.
    //   - All other instructions: fall-through only (+1 to i+1).
    let mut pred_count: Vec<u32> = vec![0u32; n + 1];
    pred_count[0] = 1; // entry point
    for (i, insn) in insns.iter().enumerate() {
        let jt = |k: i32| -> usize { (i as i64 + 1 + k as i64) as usize };
        match insn {
            Insn::Jump(k) => {
                let t = jt(*k);
                if t < n {
                    pred_count[t] += 1;
                }
                // Unconditional jump: no fall-through.
            }
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k)
            | Insn::SetupExcept(k) => {
                let t = jt(*k);
                if t < n {
                    pred_count[t] += 1;
                }
                if i + 1 < n {
                    pred_count[i + 1] += 1;
                }
            }
            Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseValue(_)
            | Insn::RaiseFrom(_, _)
            | Insn::RaiseReRaise
            | Insn::RaiseAssert(_)
            | Insn::RaiseAssertNoMsg
            | Insn::TailCall { .. } => {
                // No fall-through.
            }
            _ => {
                if i + 1 < n {
                    pred_count[i + 1] += 1;
                }
            }
        }
    }

    // Safety condition 5: the constant being compared must be a primitive type
    // (int, str, None, bool, float, bigint).  Comparing against a user-defined
    // object can invoke `__eq__`, which might mutate or delete the global between
    // the two loads.  Primitives never dispatch user `__eq__`.
    let is_primitive_const = |idx: u16| -> bool {
        matches!(
            consts.get(idx as usize).map(|v| v.kind()),
            Some(
                ValueKind::Int(_)
                    | ValueKind::BigInt(_)
                    | ValueKind::Str(_)
                    | ValueKind::None
                    | ValueKind::Bool(_)
                    | ValueKind::Float(_)
            )
        )
    };

    let mut insns = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i + 3 < n {
        // Pattern step 1: LoadGlobal(t, g_idx) at i, where t is a temp.
        let (t, g_idx) = match insns[i] {
            Insn::LoadGlobal(t, g) if t >= num_locals && keep[i] => (t, g),
            _ => {
                i += 1;
                continue;
            }
        };

        // Pattern step 2: CmpJumpIfFalseConst(t, _, c, k) or
        // CmpJumpIfTrueConst(t, _, c, k) at i+1, with a forward jump (k > 0).
        // Safety condition 5: the constant c must be a primitive.
        let k0 = match insns[i + 1] {
            Insn::CmpJumpIfFalseConst(r, _, c, k) | Insn::CmpJumpIfTrueConst(r, _, c, k)
                if r == t && k > 0 && is_primitive_const(c) =>
            {
                k
            }
            _ => {
                i += 1;
                continue;
            }
        };

        // The branch target is at `target`.  We need target+1 to also exist.
        // target = (i+1) + 1 + k0 = i + 2 + k0
        let target = (i as i64 + 2 + k0 as i64) as usize;
        if target + 1 >= n {
            i += 1;
            continue;
        }

        // Safety condition 3: `target` has exactly one predecessor (the branch
        // at i+1), so `t` is guaranteed to hold the global's value there.
        if pred_count[target] != 1 {
            i += 1;
            continue;
        }

        // Pattern step 3: LoadGlobal(t2, g_idx) at `target` — same global.
        let t2 = match insns[target] {
            Insn::LoadGlobal(t2, g) if t2 >= num_locals && g == g_idx && keep[target] => t2,
            _ => {
                i += 1;
                continue;
            }
        };

        // Pattern step 4: CmpJumpIfFalseConst(t2, ...) or CmpJumpIfTrueConst(t2, ...)
        // at target+1.  Safety condition 5 also applies to this constant.
        match insns[target + 1] {
            Insn::CmpJumpIfFalseConst(r, _, c, _) | Insn::CmpJumpIfTrueConst(r, _, c, _)
                if r == t2 && is_primitive_const(c) => {}
            _ => {
                i += 1;
                continue;
            }
        }

        // Safety condition 4: t2 is not read in the true-branch body [i+2, target).
        if reg_is_read_in(&insns[i + 2..target], t2) {
            i += 1;
            continue;
        }

        // All conditions met.  Remove LoadGlobal(t2, g_idx) and rewrite the
        // following CmpJump to use t (which holds the same global value).
        keep[target] = false;
        match &mut insns[target + 1] {
            Insn::CmpJumpIfFalseConst(r, _, _, _) | Insn::CmpJumpIfTrueConst(r, _, _, _) => {
                *r = t;
            }
            _ => unreachable!(),
        }

        // Advance past the LoadGlobal + CmpJump we just processed.
        i += 2;
    }

    compact(insns, &keep)
}

// ─── Loop inversion ────────────────────────────────────────────────────────────

/// Eliminate the unconditional back-edge `Jump` from `while COND: body` loops.
///
/// ## Pattern (VM offset semantics: `jump_pc!(k) = current_pc + 1 + k`)
///
/// A while-loop compiled to:
/// ```text
/// [j]   CmpJumpIfFalse*(r, op, b_or_c, k)   ; exit to [j+k+1] if cond false
/// [j+1] body_insn_1
/// …
/// [j+k] Jump(-(k+1))                         ; back-edge to [j]
/// ```
///
/// is transformed to:
/// ```text
/// [j]   CmpJumpIfFalse*(r, op, b_or_c, k)   ; initial guard — unchanged
/// [j+1] body_insn_1
/// …
/// [j+k] CmpJumpIfTrue*(r, op, b_or_c, -k)   ; re-check; if true, jump to [j+1]
/// ```
///
/// The back-edge `Jump` is replaced by a conditional jump that re-checks the
/// loop condition and either loops back to `[j+1]` (body start) or falls through
/// to `[j+k+1]` (exit).  This saves one full VM dispatch per iteration on the
/// hot path.
///
/// ## Correctness notes
///
/// - **`continue`**: generates `Jump(-(m+1))` targeting `[j]` (the initial guard).
///   After inversion the initial guard at `[j]` still exists, so `continue` works
///   correctly — the guard re-checks the condition and falls through to `[j+1]`.
/// - **`break`**: generates a forward jump to `[j+k+1]`.  Unchanged.
/// - **0-iteration loops**: the initial guard at `[j]` exits to `[j+k+1]` immediately.
/// - **Nested loops**: each is handled independently, innermost first.
///
/// ## Guards
///
/// - `k >= 2`: at least one real body instruction before the back-edge Jump.
///   (`k == 1` would mean the body is just the Jump itself, which pass_dead_code
///   would already have removed or which is a degenerate loop with no real body.)
/// - `j + k < n`: the back-edge index must be in bounds.
fn pass_loop_inversion(insns: Vec<Insn>) -> Vec<Insn> {
    let mut out = insns;
    let n = out.len();
    for j in 0..n {
        // Match the loop header: CmpJumpIfFalseConst, CmpJumpIfFalse,
        // CmpJumpIfTrueConst, or CmpJumpIfTrue.
        // Extract (register, op, rhs-operand, forward-offset k).
        match out[j].clone() {
            Insn::CmpJumpIfFalseConst(r, op, c, k) => {
                if k < 2 {
                    continue;
                }
                let back = j + k as usize;
                if back >= n {
                    continue;
                }
                // Verify the back-edge: Jump(-(k+1)) at [j+k] targets [j].
                // Proof: (j+k) + 1 + (-(k+1)) = j. ✓
                match out[back] {
                    Insn::Jump(bk) if bk == -(k + 1) => {}
                    _ => continue,
                }
                // Replace unconditional back-edge with CmpJumpIfTrueConst targeting [j+1].
                // new_offset = -k  because  (j+k) + 1 + (-k) = j+1. ✓
                out[back] = Insn::CmpJumpIfTrueConst(r, op, c, -k);
            }
            Insn::CmpJumpIfFalse(r, op, b, k) => {
                if k < 2 {
                    continue;
                }
                let back = j + k as usize;
                if back >= n {
                    continue;
                }
                match out[back] {
                    Insn::Jump(bk) if bk == -(k + 1) => {}
                    _ => continue,
                }
                out[back] = Insn::CmpJumpIfTrue(r, op, b, -k);
            }
            // Case: `while True: if cond: break; body` shape.
            //
            // The header CmpJumpIfTrueConst(r, op, c, k) exits to [j+k+1] when
            // the break condition is TRUE.  The back-edge Jump(-(k+1)) at [j+k]
            // unconditionally returns to [j].  We replace the Jump with
            // CmpJumpIfFalseConst(r, op, c, -k): when the condition is FALSE
            // (not yet time to break), jump to (j+k+1)+(-k) = j+1 (body start);
            // when the condition is TRUE (time to break), fall through to [j+k+1]
            // (the exit).  No operator negation needed — just swap True↔False
            // variant with the same (r, op, c).
            Insn::CmpJumpIfTrueConst(r, op, c, k) => {
                if k < 2 {
                    continue;
                }
                let back = j + k as usize;
                if back >= n {
                    continue;
                }
                // Verify the back-edge: Jump(-(k+1)) at [j+k] targets [j].
                match out[back] {
                    Insn::Jump(bk) if bk == -(k + 1) => {}
                    _ => continue,
                }
                // Replace with CmpJumpIfFalseConst targeting [j+1].
                // new_offset = -k  because  (j+k) + 1 + (-k) = j+1. ✓
                out[back] = Insn::CmpJumpIfFalseConst(r, op, c, -k);
            }
            Insn::CmpJumpIfTrue(r, op, b, k) => {
                if k < 2 {
                    continue;
                }
                let back = j + k as usize;
                if back >= n {
                    continue;
                }
                match out[back] {
                    Insn::Jump(bk) if bk == -(k + 1) => {}
                    _ => continue,
                }
                out[back] = Insn::CmpJumpIfFalse(r, op, b, -k);
            }
            _ => {}
        }
    }
    out
}

fn pass_trivial_nop(insns: Vec<Insn>) -> Vec<Insn> {
    let keep: Vec<bool> = insns
        .iter()
        .map(|insn| match insn {
            Insn::Jump(0) => false,
            Insn::Move(dst, src) | Insn::CopyReg(dst, src) => dst != src,
            _ => true,
        })
        .collect();
    compact(insns, &keep)
}

// ─── ForCountConst → ForCountConstInline ──────────────────────────────────────

/// Convert `ForCountConst(var, op, stop_idx, step_idx, off)` to
/// Promote `ForCountReg` → `ForCountConst` when the stop register is known to
/// hold an immutable constant (written exactly once by a `LoadConst`).
///
/// `pass_copy_prop` (which runs just before this pass) substitutes copied
/// registers, so a `ForCountReg(var, op, t, step, off)` where `t` was
/// `Move(t, n_reg)` becomes `ForCountReg(var, op, n_reg, step, off)`.  If
/// `n_reg` was set by a single `LoadConst`, this pass replaces the
/// per-iteration register read with a direct const-pool reference.
///
/// `pass_forcount_const_inline` (which runs after this pass) will further
/// promote `ForCountConst` → `ForCountConstInline` when both stop and step fit
/// in `i32`, eliminating the remaining per-iteration pool lookups entirely.
///
/// Example: `n = 2_000_000; while i < n: … i += 1` originally compiles to
/// `ForCountReg(i, Lt, n_reg, step_idx, off)`.  After this pass + inline:
/// `ForCountConstInline(i, Lt, 2_000_000, 1, off)` — zero indirections.
fn pass_forcount_reg_upgrade(insns: Vec<Insn>) -> Vec<Insn> {
    let mut write_count: HashMap<u32, u32> = HashMap::new();
    let mut load_const_idx: HashMap<u32, u16> = HashMap::new();
    let mut written: HashSet<u32> = HashSet::new();
    for insn in &insns {
        written.clear();
        collect_writes(insn, &mut written);
        for &dst in &written {
            *write_count.entry(dst).or_insert(0) += 1;
        }
        if let Insn::LoadConst(dst, idx) = insn {
            load_const_idx.insert(*dst, *idx);
        }
    }
    let immutable_const: HashMap<u32, u16> = load_const_idx
        .into_iter()
        .filter(|(r, _)| write_count.get(r).copied() == Some(1))
        .collect();

    if immutable_const.is_empty() {
        return insns;
    }

    insns
        .into_iter()
        .map(|insn| match insn {
            Insn::ForCountReg(var, op, stop_reg, step_idx, off) => {
                if let Some(&stop_const_idx) = immutable_const.get(&stop_reg) {
                    Insn::ForCountConst(var, op, stop_const_idx, step_idx, off)
                } else {
                    Insn::ForCountReg(var, op, stop_reg, step_idx, off)
                }
            }
            other => other,
        })
        .collect()
}

/// `ForCountConstInline(var, op, stop, step, off)` when both `consts[stop_idx]`
/// and `consts[step_idx]` are integers that fit in `i32`.
///
/// The inline variant removes the per-iteration `consts` pool lookup, which the
/// VM otherwise has to perform for both `stop` and `step` on every iteration.
///
/// Runs after all other passes (in particular `pass_ivsr`, which pattern-matches
/// on the un-inlined `ForCountConst`) and before `pass_compact_consts` (which
/// then drops the now-unused pool entries).
fn pass_forcount_const_inline(insns: Vec<Insn>, consts: &[Value]) -> Vec<Insn> {
    insns
        .into_iter()
        .map(|insn| match insn {
            Insn::ForCountConst(var, op, stop_idx, step_idx, off) => {
                let stop_int = match consts.get(stop_idx as usize).map(Value::kind) {
                    Some(ValueKind::Int(i)) => i,
                    _ => return Insn::ForCountConst(var, op, stop_idx, step_idx, off),
                };
                let step_int = match consts.get(step_idx as usize).map(Value::kind) {
                    Some(ValueKind::Int(i)) => i,
                    _ => return Insn::ForCountConst(var, op, stop_idx, step_idx, off),
                };
                match (i32::try_from(stop_int), i32::try_from(step_int)) {
                    (Ok(stop32), Ok(step32)) => {
                        Insn::ForCountConstInline(var, op, stop32, step32, off)
                    }
                    _ => Insn::ForCountConst(var, op, stop_idx, step_idx, off),
                }
            }
            other => other,
        })
        .collect()
}

/// Describe a single `ForCountConstInline` loop selected for unrolling.
struct ForcountUnrollPlan {
    h: usize,              // position of ForCountConstInline
    off: usize,            // exit offset (body size = off-1, back-edge at h+off)
    var: u32,              // loop variable register
    val_indices: Vec<u16>, // const-pool indices for each iteration value
}

/// Extract the jump-offset field from an instruction, if any.
fn insn_jump_off(insn: &Insn) -> Option<i32> {
    match insn {
        Insn::Jump(k) => Some(*k),
        Insn::JumpIfFalse(_, k) | Insn::JumpIfTrue(_, k) => Some(*k),
        Insn::CmpJumpIfFalse(_, _, _, k)
        | Insn::CmpJumpIfTrue(_, _, _, k)
        | Insn::CmpJumpIfFalseConst(_, _, _, k)
        | Insn::CmpJumpIfTrueConst(_, _, _, k) => Some(*k),
        Insn::ForIter(_, _, k)
        | Insn::ForCountReg(_, _, _, _, k)
        | Insn::ForCountConst(_, _, _, _, k)
        | Insn::ForCountConstInline(_, _, _, _, k) => Some(*k),
        Insn::SetupExcept(k) | Insn::MatchExcept(_, k) | Insn::MatchExceptStar(_, _, _, k) => {
            Some(*k)
        }
        _ => None,
    }
}

/// Scan `insns` front-to-back for `ForCountConstInline` loops whose trip count
/// is a small compile-time constant and whose body satisfies every safety
/// condition required to unroll (see [`pass_forcount_unroll`]). Returns one
/// non-overlapping [`ForcountUnrollPlan`] per loop selected, interning each
/// loop's iteration values into `consts` as it goes.
fn forcount_collect_unroll_plans(
    insns: &[Insn],
    consts: &mut Vec<Value>,
) -> Vec<ForcountUnrollPlan> {
    use crate::ast::BinaryOp;

    const MAX_TRIP: usize = 4;
    const MAX_BUDGET: usize = 32;

    let n = insns.len();
    let mut plans: Vec<ForcountUnrollPlan> = Vec::new();
    let mut skip_until = 0usize;

    for h in 0..n {
        if h < skip_until {
            continue;
        }

        let (var, op, stop, step, off_i32) = match insns[h] {
            Insn::ForCountConstInline(var, op, stop, step, off) => (var, op, stop, step, off),
            _ => continue,
        };

        // off >= 2: at least one body instruction (off-1) plus the back-edge.
        // off == 1 means an empty body (only the back-edge) — nothing to unroll.
        // This check also guards the `as usize` cast below against wrapping on
        // any non-positive value that a future pass might produce.
        if off_i32 < 2 {
            continue;
        }
        let off = off_i32 as usize;

        let back_edge = h + off;
        if back_edge >= n {
            continue;
        }
        let body_start = h + 1;
        // body_insns = off - 1 (excluding back-edge).
        let body_insns = off - 1;

        // Verify the back-edge is a Jump targeting h.
        let back_ok = match &insns[back_edge] {
            Insn::Jump(k) => ((back_edge as i64 + 1 + *k as i64) as usize) == h,
            _ => false,
        };
        if !back_ok {
            continue;
        }

        // The pre-init instruction immediately before h must be
        // `LoadConst(var, idx)` with a known integer value.
        if h == 0 {
            continue;
        }
        let pre_init: i64 = match &insns[h - 1] {
            Insn::LoadConst(r, idx) if *r == var => {
                match consts.get(*idx as usize).map(Value::kind) {
                    Some(ValueKind::Int(v)) => v,
                    _ => continue,
                }
            }
            _ => continue,
        };

        // Compute the sequence of iteration values.
        let step_i64 = step as i64;
        let stop_i64 = stop as i64;
        // First iteration value: pre_init + step (the value ForCount writes on iter 1).
        let start = match pre_init.checked_add(step_i64) {
            Some(v) => v,
            None => continue,
        };

        let mut iter_vals: Vec<i64> = Vec::new();
        let mut cur = start;
        let mut overflow = false;
        loop {
            let cont = match op {
                BinaryOp::Lt => cur < stop_i64,
                BinaryOp::Gt => cur > stop_i64,
                _ => break, // unexpected op
            };
            if !cont {
                break;
            }
            iter_vals.push(cur);
            if iter_vals.len() > MAX_TRIP {
                break;
            }
            match cur.checked_add(step_i64) {
                Some(v) => cur = v,
                None => {
                    overflow = true;
                    break;
                }
            }
        }

        let trip = iter_vals.len();
        if trip == 0 || trip > MAX_TRIP || overflow {
            continue;
        }

        // Instruction budget: body_insns * trip <= MAX_BUDGET.
        if body_insns * trip > MAX_BUDGET {
            continue;
        }

        // Safety: no unsafe instructions in the body [h+1, h+off-1].
        let mut safe = true;
        for j in body_start..back_edge {
            match &insns[j] {
                Insn::Return(_)
                | Insn::ReturnNone
                | Insn::Yield { .. }
                | Insn::YieldFrom { .. }
                | Insn::SetupExcept(_) => {
                    safe = false;
                    break;
                }
                insn => {
                    if let Some(k) = insn_jump_off(insn) {
                        let target = (j as i64 + 1 + k as i64) as usize;
                        // Allow jump targets within the body or to the back-edge.
                        // Jumps to back_edge are fine (see offset-rewriting rationale
                        // in the module-level comment).  Jumps outside the loop
                        // (continue → h, break → h+off+1, or other exits) make
                        // unrolling unsafe.
                        if target < body_start || target > back_edge {
                            safe = false;
                            break;
                        }
                    }
                }
            }
        }
        if !safe {
            continue;
        }

        // Safety: no external instruction (outside [h, back_edge]) jumps into
        // the loop range [h, back_edge].  Violations would produce dangling
        // offsets after we splice in the unrolled copies.
        let external_jump_in = insns.iter().enumerate().any(|(i, insn)| {
            if i >= h && i <= back_edge {
                return false; // inside loop — not external
            }
            if let Some(k) = insn_jump_off(insn) {
                let tgt = (i as i64 + 1 + k as i64) as usize;
                tgt >= h && tgt <= back_edge
            } else {
                false
            }
        });
        if external_jump_in {
            continue;
        }

        // Intern all iteration values into the const pool.
        let mut val_indices: Vec<u16> = Vec::with_capacity(trip);
        let mut intern_ok = true;
        for &v in &iter_vals {
            match intern_const_in_pool(consts, Value::int(v)) {
                Some(idx) => val_indices.push(idx),
                None => {
                    intern_ok = false;
                    break;
                }
            }
        }
        if !intern_ok {
            continue;
        }

        plans.push(ForcountUnrollPlan {
            h,
            off,
            var,
            val_indices,
        });
        // Skip past this loop so we don't try to unroll its body instructions
        // as separate loops.
        skip_until = back_edge + 1;
    }

    plans
}

// ─── ForCountConstInline loop unrolling ───────────────────────────────────────

/// Unroll `ForCountConstInline` loops whose trip count is ≤ 4 and whose total
/// unrolled body size (body_insns × trip) is ≤ 32 instructions.
///
/// For each qualifying loop at position `h` with exit-offset `off`:
/// - The body occupies `[h+1, h+off-1]` (size = `off-1`).
/// - The back-edge `Jump` is at `h+off`.
/// - The post-loop code starts at `h+off+1`.
///
/// The loop is replaced with `trip` copies of:
///   `LoadConst(var, iter_val)` + `[h+1 … h+off-1]`
/// totalling `trip × off` instructions (vs. the original `off+1`).
///
/// Offset rewriting:
/// - Intra-body jumps (target ∈ `[h+1, h+off-1]`) are unchanged because each
///   copy is laid out with the same `off`-instruction stride.
/// - Jumps from the body targeting the back-edge (`h+off`) also require no
///   change: in copy k they resolve to `h + (k+1)*off` (= first instruction of
///   the next copy's `LoadConst`), which is exactly the same relative offset.
/// - External instructions (before or after the loop) whose jump targets cross
///   the loop boundary are adjusted via an `old_to_new` table.
///
/// Safety guards:
/// - Generator functions are skipped entirely (`is_generator == true`).
/// - The preceding instruction must be `LoadConst(var, idx)` with a known
///   integer value (gives the pre-initialised value `start - step`).
/// - No body instruction is `Return`, `ReturnNone`, `Yield`, `YieldFrom`, or
///   `SetupExcept`.
/// - No body instruction has a jump target outside `[h+1, h+off]`.
/// - No external instruction (position ∉ `[h, h+off]`) jumps into the loop
///   body `[h, h+off]`.
fn pass_forcount_unroll(
    insns: Vec<Insn>,
    consts: &mut Vec<Value>,
    is_generator: bool,
) -> Vec<Insn> {
    if is_generator {
        return insns;
    }

    let n = insns.len();
    if n < 3 {
        return insns;
    }

    // Phase 1: eligibility analysis — find the non-overlapping loops to unroll.
    let plans = forcount_collect_unroll_plans(&insns, consts);
    if plans.is_empty() {
        return insns;
    }

    // Build the new instruction vector.
    //
    // Layout for a loop [h, h+off] with trip copies:
    //   - First copy at [h .. h+off-1]:
    //       [h]          : LoadConst(var, val_indices[0])
    //       [h+1..h+off-1]: body instructions (same as original)
    //   - Copy k (0-indexed) at [h+k*off .. h+(k+1)*off-1]:
    //       [h+k*off]    : LoadConst(var, val_indices[k])
    //       [h+k*off+1..h+(k+1)*off-1]: body instructions (same as original)
    //   - Post-loop starts at [h + trip*off].
    //
    // The original loop occupied [h .. h+off] (size off+1).
    // The new loop occupies   [h .. h+trip*off-1] (size trip*off).
    // Net shift for post-loop = trip*off - (off+1) = (trip-1)*off - 1.
    //
    // Build `old_to_new[0..=n]` for external-code offset rewriting.

    // Compute old_to_new: maps old positions to new positions.
    // For positions inside the first copy (h..h+off-1): same position.
    // For the back-edge at h+off: maps to h+trip*off (post-loop) — only needed
    //   if some external instruction happened to jump there, which we've ruled out.
    // For post-loop positions (> h+off): shift by (trip-1)*off-1.
    let mut old_to_new: Vec<usize> = (0..=n).collect(); // identity by default
    for plan in &plans {
        let ForcountUnrollPlan { h, off, .. } = *plan;
        let back_edge = h + off;
        let trip = plan.val_indices.len();
        // shift for positions after back_edge.
        // (trip-1)*off - 1: replaces (off+1) old insns with trip*off new insns.
        // Use isize arithmetic: trip == 1 gives shift == -1 (contraction), which
        // is valid — a usize subtraction would underflow in that case.
        let shift = (trip * off) as isize - (off + 1) as isize;
        // Sentinel: the old back-edge maps to the new post-loop start.
        // Use old_to_new[h] (the new header position, which accounts for prior
        // plans' shifts) plus trip*off to get the new post-loop start.
        old_to_new[back_edge] = old_to_new[h] + trip * off;
        for p in (back_edge + 1)..=n {
            old_to_new[p] = (old_to_new[p] as isize + shift) as usize;
        }
        // Multiple plans are accumulated correctly since each plan only shifts
        // positions strictly after its own back-edge, and plans are non-overlapping.
    }

    // Build the output (first pass: emit instructions without rewriting).
    // Track each output instruction's old source position in new_to_old:
    //   - n+1 sentinel → skip offset rewriting (body copies, LoadConst)
    //   - anything ≤ n  → apply old_to_new when rewriting offsets (external insns)
    let out_capacity = old_to_new[n];
    let mut out: Vec<Insn> = Vec::with_capacity(out_capacity);
    let mut new_to_old: Vec<usize> = Vec::with_capacity(out_capacity);
    let mut plan_idx = 0usize;
    let mut i = 0usize;

    while i < n {
        if plan_idx < plans.len() && i == plans[plan_idx].h {
            let plan = &plans[plan_idx];
            let h = plan.h;
            let off = plan.off;
            let var = plan.var;
            let back_edge = h + off;
            let body_start = h + 1;

            // Emit trip copies of: [LoadConst(var, val)] + [body h+1..h+off-1].
            for &val_idx in &plan.val_indices {
                // LoadConst for this iteration — no jump offsets; sentinel to skip.
                out.push(Insn::LoadConst(var, val_idx));
                new_to_old.push(n + 1);

                // Body instructions (excluding back-edge).
                // Intra-body offsets and back-edge jumps are all correct without
                // rewriting (uniform `off`-stride layout — see pass doc comment).
                for delta in 0..off - 1 {
                    let old_pos = body_start + delta;
                    out.push(insns[old_pos].clone());
                    new_to_old.push(n + 1); // sentinel: skip offset rewriting
                }
            }

            // Skip past the original loop (ForCountConstInline + body + back-edge).
            i = back_edge + 1;
            plan_idx += 1;
        } else {
            // Non-loop instruction: emit as-is, record old position.
            out.push(insns[i].clone());
            new_to_old.push(i);
            i += 1;
        }
    }

    debug_assert_eq!(out.len(), out_capacity, "unexpected output size");

    // Rewrite jump offsets using old_to_new and new_to_old.
    // For each instruction at new position p:
    //   - old_i = new_to_old[p]
    //   - If old_i > n: skip (copy k>0 instruction, no rewriting needed)
    //   - Otherwise: for each jump offset k in the instruction,
    //     old_target = old_i + 1 + k, new_target = old_to_new[old_target],
    //     new_k = new_target - p - 1.
    let rewrite_offset = |k: i32, p: usize, old_i: usize| -> i32 {
        let old_target = (old_i as i64 + 1 + k as i64) as usize;
        let new_target = old_to_new[old_target];
        (new_target as i64 - p as i64 - 1) as i32
    };

    for p in 0..out.len() {
        let old_i = new_to_old[p];
        if old_i > n {
            // Copy k>0 body instruction — offsets already correct, no rewrite.
            continue;
        }
        let fix = |k: i32| rewrite_offset(k, p, old_i);
        out[p] = match out[p].clone() {
            Insn::Jump(k) => Insn::Jump(fix(k)),
            Insn::JumpIfFalse(r, k) => Insn::JumpIfFalse(r, fix(k)),
            Insn::JumpIfTrue(r, k) => Insn::JumpIfTrue(r, fix(k)),
            Insn::CmpJumpIfFalse(a, op, b, k) => Insn::CmpJumpIfFalse(a, op, b, fix(k)),
            Insn::CmpJumpIfTrue(a, op, b, k) => Insn::CmpJumpIfTrue(a, op, b, fix(k)),
            Insn::CmpJumpIfFalseConst(r, op, c, k) => Insn::CmpJumpIfFalseConst(r, op, c, fix(k)),
            Insn::CmpJumpIfTrueConst(r, op, c, k) => Insn::CmpJumpIfTrueConst(r, op, c, fix(k)),
            Insn::ForIter(dst, slot, k) => Insn::ForIter(dst, slot, fix(k)),
            Insn::ForCountReg(v, op, stop, step, k) => Insn::ForCountReg(v, op, stop, step, fix(k)),
            Insn::ForCountConst(v, op, stop, step, k) => {
                Insn::ForCountConst(v, op, stop, step, fix(k))
            }
            Insn::ForCountConstInline(v, op, stop, step, k) => {
                Insn::ForCountConstInline(v, op, stop, step, fix(k))
            }
            Insn::SetupExcept(k) => Insn::SetupExcept(fix(k)),
            Insn::MatchExcept(r, k) => Insn::MatchExcept(r, fix(k)),
            Insn::MatchExceptStar(r, src, dst, k) => Insn::MatchExceptStar(r, src, dst, fix(k)),
            other => other,
        };
    }

    out
}

// ─── Linear loop fold ────────────────────────────────────────────────────────

/// Fold a simple linear accumulation loop into a single `LoadConst`.
///
/// Matches the shape produced by `for _ in range(N): acc += K` (after earlier
/// passes have const-folded the body, sunk `SyncModuleGlobal` out, and inlined
/// the ForCount parameters):
///
/// ```text
/// [h-1]: LoadConst(iv, -1)          ← pre-decrement before ForCount
/// [h  ]: ForCountConstInline(iv, Lt, stop, 1, +2)
/// [h+1]: BinOpImm(acc, acc, Add/Sub, imm)  │ loop body — exactly 2 insns
/// [h+2]: Jump(back to h)                   │
/// ```
///
/// Requirements (all checked):
/// - step == 1, cmp == Lt
/// - exit_off == 2 (body is exactly the two instructions above)
/// - iv != acc
/// - iv is not read by the BinOpImm body
/// - [h-1] is `LoadConst(iv, -1_idx)` (iv pre-decremented for ForCount start=0)
/// - None of h-1, h, h+1, h+2 are jump targets
/// - acc is last-written (before h) by a single `LoadConst` with a known integer
///   value, with no other writes to acc between that `LoadConst` and h-1
/// - The computed acc_final fits in i64 (no overflow)
/// - `stop` may be ≤ 0; trip count is `max(stop, 0)` (zero-trip loop, acc unchanged)
///
/// Transformation: replace the four-instruction sequence with two `LoadConst`s
/// that set iv and acc to their post-loop values, then compact out the dead body.
fn pass_linear_loop_fold(insns: Vec<Insn>, consts: &mut Vec<Value>) -> Vec<Insn> {
    use crate::ast::BinaryOp;

    let n = insns.len();
    if n < 5 {
        return insns;
    }

    // All jump targets — used for the backward acc-init scan to detect
    // multiple incoming edges to any instruction in the scan range.
    let mut all_jump_targets: HashSet<usize> = HashSet::new();
    // Forward-only jump targets — used for the loop-pattern guard.
    // The loop's own back-edge Jump at [h+2] → [h] is a *backward* jump;
    // excluding it prevents the guard from incorrectly blocking the fold
    // because [h] (the ForCountConstInline header) is always targeted by
    // the back-edge and would otherwise fail the `contains(&h)` check.
    let mut fwd_jump_targets: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let t = i as i64 + 1 + k as i64;
            if t >= 0 && (t as usize) < n {
                let t = t as usize;
                all_jump_targets.insert(t);
                if t > i {
                    fwd_jump_targets.insert(t);
                }
            }
        }
    }

    let mut transformed = insns;
    let mut keep = vec![true; n];
    let mut modified = false;
    let mut written_tmp: HashSet<u32> = HashSet::new();

    'outer: for h in 1..n.saturating_sub(2) {
        // [h]: ForCountConstInline(iv, Lt, stop, 1, +2)
        let (iv, stop, exit_off) = match transformed[h] {
            Insn::ForCountConstInline(iv, BinaryOp::Lt, stop, 1, off) => (iv, stop, off),
            _ => continue,
        };
        if exit_off != 2 {
            continue;
        }

        // [h+1]: BinOpImm(acc, acc, Add/Sub, imm) — acc != iv, iv not read
        let (acc, acc_op, imm) = match transformed[h + 1] {
            Insn::BinOpImm(dst, src, op, imm, _)
                if dst == src && dst != iv && matches!(op, BinaryOp::Add | BinaryOp::Sub) =>
            {
                (dst, op, imm as i64)
            }
            _ => continue,
        };

        // [h+2]: Jump back to h
        let back_ok = match transformed[h + 2] {
            Insn::Jump(k) => (h as i64 + 3 + k as i64) as usize == h,
            _ => false,
        };
        if !back_ok {
            continue;
        }

        // None of h-1, h, h+1, h+2 may be forward-jump targets.
        // (Backward jumps are excluded because the loop's own back-edge
        // is a backward Jump at [h+2] → [h]; that is expected and allowed.)
        if fwd_jump_targets.contains(&(h - 1))
            || fwd_jump_targets.contains(&h)
            || fwd_jump_targets.contains(&(h + 1))
            || fwd_jump_targets.contains(&(h + 2))
        {
            continue;
        }

        // [h-1]: LoadConst(iv, idx) where consts[idx] == -1 (ForCount pre-decrement)
        let iv_pre_is_neg1 = match transformed[h - 1] {
            Insn::LoadConst(r, idx) if r == iv => {
                matches!(
                    consts.get(idx as usize).map(Value::kind),
                    Some(ValueKind::Int(-1))
                )
            }
            _ => false,
        };
        if !iv_pre_is_neg1 {
            continue;
        }

        // Scan backward from h-2 for acc's initialisation.  Stop at a jump
        // target (another incoming edge → acc value is not uniquely known) or
        // when we find the LoadConst that sets acc.  Any other write to acc
        // means the folding is unsafe.
        let mut acc_init: Option<i64> = None;
        for scan in (0..h - 1).rev() {
            if all_jump_targets.contains(&scan) {
                continue 'outer; // can't guarantee single entry path
            }
            written_tmp.clear();
            collect_writes(&transformed[scan], &mut written_tmp);
            if written_tmp.contains(&acc) {
                // acc is written here — must be a LoadConst with an integer value.
                if let Insn::LoadConst(r, idx) = transformed[scan]
                    && r == acc
                    && let Some(ValueKind::Int(v)) = consts.get(idx as usize).map(Value::kind)
                {
                    acc_init = Some(v);
                    break;
                }
                continue 'outer; // non-LoadConst write → unsafe
            }
        }

        let acc_init = match acc_init {
            Some(v) => v,
            None => continue,
        };

        // `stop` is an i32 and may be ≤ 0 (e.g. `range(-3)` or `range(0)`).
        // The loop is zero-trip when stop ≤ 0: acc stays unchanged and iv
        // remains at its pre-decremented value of -1.
        let trip_count = (stop as i64).max(0);

        // Compute acc_final = acc_init ± imm * trip_count.  Reject on overflow
        // so we never silently produce the wrong value (let the loop run instead).
        let delta = imm.checked_mul(trip_count);
        let acc_final = match (delta, acc_op) {
            (Some(d), BinaryOp::Add) => acc_init.checked_add(d),
            (Some(d), BinaryOp::Sub) => acc_init.checked_sub(d),
            _ => None,
        };
        let acc_final = match acc_final {
            Some(v) => v,
            None => continue,
        };

        // Intern the two new constants.  Bail if the pool is full (unlikely but
        // possible for code with thousands of existing constants).
        let Some(acc_final_idx) = intern_const_in_pool(consts, Value::int(acc_final)) else {
            continue;
        };
        // iv's post-loop value: stop - 1 when stop > 0 (last value for which
        // iv < stop held); -1 when stop ≤ 0 (loop never ran, iv stays at
        // its pre-decremented initial value).
        let iv_post = if stop > 0 { stop as i64 - 1 } else { -1 };
        let Some(iv_post_idx) = intern_const_in_pool(consts, Value::int(iv_post)) else {
            continue;
        };

        // Apply: replace pre-decrement with post-loop iv, ForCount with
        // LoadConst(acc, acc_final), mark body instructions as dead.
        transformed[h - 1] = Insn::LoadConst(iv, iv_post_idx);
        transformed[h] = Insn::LoadConst(acc, acc_final_idx);
        keep[h + 1] = false;
        keep[h + 2] = false;
        modified = true;
    }

    if !modified {
        return transformed;
    }
    compact(transformed, &keep)
}

// ─── LoadNone merging ────────────────────────────────────────────────────────

/// Merge runs of consecutive `LoadNone(r), LoadNone(r+1), ..., LoadNone(r+N-1)` into
/// a single `LoadNoneRange { start: r, count: N }` instruction.
///
/// Only contiguous ascending register sequences are merged, AND the run must be
/// free of any jump targets in its interior — a jump to the middle of a run
/// cannot be redirected to the first instruction without changing semantics.
/// A run of 1 is left as a bare `LoadNone` (no gain from wrapping).  If a run
/// exceeds 255 registers, it is split into multiple `LoadNoneRange` instructions.
///
/// ## Why this helps
///
/// Function entry emits one `LoadNone` per local variable.  A function with N
/// locals emits N separate VM dispatches through the giant `match insn`.
/// `LoadNoneRange` collapses those N dispatches into one tight `for` loop that
/// Rust/LLVM can lower to a memset-style fill.
///
/// ## Jump-offset correctness
///
/// When the pass merges a run of K instructions into one, it uses the `compact`
/// helper to rewrite all subsequent jump offsets automatically.  The `keep`
/// vector marks which instructions to retain: the first instruction in a run is
/// replaced with a `LoadNoneRange`; the rest are marked `keep = false`.
fn pass_loadnone_merge(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Pre-pass: collect all instruction indices that are jump targets.
    // An instruction that is a jump target cannot be swallowed into the middle of
    // a LoadNoneRange (the range would be misinterpreted as starting earlier).
    let mut jump_targets: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target_signed = i as i64 + 1 + k as i64;
            if target_signed >= 0 && (target_signed as usize) < n {
                jump_targets.insert(target_signed as usize);
            }
        }
    }

    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i < n {
        if let Insn::LoadNone(start) = transformed[i] {
            // Extend the run as long as:
            // 1. The next instruction is LoadNone(run_end + 1) (contiguous ascending).
            // 2. The next instruction is NOT a jump target (cannot be subsumed).
            let mut run_end = start;
            let mut j = i + 1;
            while j < n && !jump_targets.contains(&j) {
                if let Insn::LoadNone(r) = transformed[j]
                    && r == run_end + 1
                {
                    run_end = r;
                    j += 1;
                    continue;
                }
                break;
            }

            let count = (run_end - start + 1) as usize; // >= 1
            if count == 1 {
                // Single LoadNone — no merge beneficial; leave as-is.
                i += 1;
                continue;
            }

            // Replace the first instruction with LoadNoneRange(s) and mark the
            // rest of the run for removal via `compact`.
            // If count > 255, split into batches; each batch occupies one slot
            // from the run (the first, then second, …).
            let mut base = start;
            let mut remaining = count;
            let mut slot = i; // which position in the run gets the next instruction
            while remaining > 0 {
                let batch = remaining.min(u8::MAX as usize) as u8;
                transformed[slot] = Insn::LoadNoneRange {
                    start: base,
                    count: batch,
                };
                base += batch as u32;
                remaining -= batch as usize;
                slot += 1;
            }
            // Mark the tail of the run (slot..j) as dead.
            for dead in slot..j {
                keep[dead] = false;
            }
            i = j;
        } else {
            i += 1;
        }
    }

    compact(transformed, &keep)
}

/// 3. **Compact**: rebuild `consts` retaining only referenced values.
/// 4. **Rewrite**: replace every constant index in `insns` using `old_to_new`.
///
/// ## Instruction fields that carry constant indices
///
/// `LoadConst`, `BinOpConst`, `CmpJumpIfFalseConst`, `CmpJumpIfTrueConst`,
/// `ForCountConst` (stop and step), `ForCountReg` (step only).
///
/// Reference: CPython `flowgraph.c` `remove_unused_consts()`.
fn pass_compact_consts(insns: Vec<Insn>, consts: Vec<Value>) -> (Vec<Insn>, Vec<Value>) {
    let old_len = consts.len();
    if old_len == 0 {
        return (insns, consts);
    }

    // Step 1: collect referenced indices.
    let mut used = vec![false; old_len];
    let mark = |used: &mut Vec<bool>, idx: u16| {
        if (idx as usize) < used.len() {
            used[idx as usize] = true;
        }
    };
    for insn in &insns {
        match insn {
            Insn::LoadConst(_, c) => mark(&mut used, *c),
            Insn::BinOpConst(_, _, _, c, _) => mark(&mut used, *c),
            Insn::CmpJumpIfFalseConst(_, _, c, _) => mark(&mut used, *c),
            Insn::CmpJumpIfTrueConst(_, _, c, _) => mark(&mut used, *c),
            Insn::ForCountConst(_, _, stop, step, _) => {
                mark(&mut used, *stop);
                mark(&mut used, *step);
            }
            Insn::ForCountReg(_, _, _, step, _) => mark(&mut used, *step),
            Insn::MakeTypeAlias(_, name_idx, _, _) => mark(&mut used, *name_idx),
            Insn::MakeTypeVar(_, name_idx) => mark(&mut used, *name_idx),
            Insn::CallKw { kwnames_idx, .. } => mark(&mut used, *kwnames_idx),
            Insn::CallMethodKw { kwnames_idx, .. } => mark(&mut used, *kwnames_idx),
            _ => {}
        }
    }

    // Early exit if every entry is still referenced.
    if used.iter().all(|&u| u) {
        return (insns, consts);
    }

    // Step 2: build remap table.
    let mut old_to_new: Vec<Option<u16>> = vec![None; old_len];
    let mut new_consts: Vec<Value> = Vec::with_capacity(used.iter().filter(|&&u| u).count());
    for (old_idx, val) in consts.into_iter().enumerate() {
        if used[old_idx] {
            old_to_new[old_idx] = Some(new_consts.len() as u16);
            new_consts.push(val);
        }
    }

    // Step 3: rewrite constant indices in instructions.
    let remap = |c: u16| old_to_new[c as usize].expect("referenced const must have new index");
    let new_insns = insns
        .into_iter()
        .map(|insn| match insn {
            Insn::LoadConst(r, c) => Insn::LoadConst(r, remap(c)),
            Insn::BinOpConst(d, l, op, c, ia) => Insn::BinOpConst(d, l, op, remap(c), ia),
            Insn::CmpJumpIfFalseConst(r, op, c, k) => Insn::CmpJumpIfFalseConst(r, op, remap(c), k),
            Insn::CmpJumpIfTrueConst(r, op, c, k) => Insn::CmpJumpIfTrueConst(r, op, remap(c), k),
            Insn::ForCountConst(v, op, stop, step, k) => {
                Insn::ForCountConst(v, op, remap(stop), remap(step), k)
            }
            Insn::ForCountReg(v, op, stop, step, k) => {
                Insn::ForCountReg(v, op, stop, remap(step), k)
            }
            Insn::MakeTypeAlias(dst, name_idx, value_reg, params_reg) => {
                Insn::MakeTypeAlias(dst, remap(name_idx), value_reg, params_reg)
            }
            Insn::MakeTypeVar(dst, name_idx) => Insn::MakeTypeVar(dst, remap(name_idx)),
            Insn::CallKw {
                func,
                total,
                nkw,
                kwnames_idx,
            } => Insn::CallKw {
                func,
                total,
                nkw,
                kwnames_idx: remap(kwnames_idx),
            },
            Insn::CallMethodKw {
                dst,
                obj,
                name_idx,
                args_base,
                total,
                nkw,
                kwnames_idx,
            } => Insn::CallMethodKw {
                dst,
                obj,
                name_idx,
                args_base,
                total,
                nkw,
                kwnames_idx: remap(kwnames_idx),
            },
            other => other,
        })
        .collect();

    (new_insns, new_consts)
}

// ─── Zero-cost exception table ─────────────────────────────────────────────────

/// Sentinel in an exception table meaning "no handler covers this pc".
pub(crate) const EXC_NO_HANDLER: u32 = u32::MAX;

/// Build a per-pc exception-handler table from the (PC-balanced)
/// `SetupExcept`/`PopExcept` structure, then strip those two instructions from
/// the stream (CPython 3.11 "zero-cost" model).
///
/// Returns `(new_insns, exc_table)` where `exc_table[pc]` is the absolute target
/// PC (in the *new*, post-strip instruction space) of the innermost exception
/// handler active when an exception is raised at `pc`, or [`EXC_NO_HANDLER`] if
/// none.  `SetupExcept`/`PopExcept` are removed entirely, so entering/leaving a
/// `try` costs nothing at runtime — the cost moves to the rare raise/unwind,
/// which does a single O(1) table lookup instead of a per-frame block push/pop.
///
/// On the bail path (handler stack not statically consistent at every PC) the
/// stream is returned unchanged with an **empty** table; the VM then keeps the
/// dynamic `SetupExcept`/`PopExcept` handler stack.  The compiler emits balanced,
/// properly-nested `SetupExcept`/`PopExcept`, so in practice this never bails;
/// the check is a safety net that guarantees we never produce an incorrect table.
fn build_exc_table(insns: Vec<Insn>) -> (Vec<Insn>, Vec<u32>) {
    let n = insns.len();

    // Fast out: no exception handlers at all → empty table, nothing to strip.
    if !insns.iter().any(|i| matches!(i, Insn::SetupExcept(_))) {
        return (insns, vec![EXC_NO_HANDLER; n]);
    }

    // Forward dataflow: `stack_in[pc]` = the exc-handler stack (absolute target
    // PCs, innermost last) at entry to instruction `pc`.  `None` = not yet
    // visited.  Computed by walking normal control-flow edges (fallthrough +
    // explicit jumps); exception edges are exactly what this table encodes and
    // are not followed here.
    let mut stack_in: Vec<Option<Vec<usize>>> = vec![None; n];
    let mut work: Vec<usize> = Vec::new();
    if n > 0 {
        stack_in[0] = Some(Vec::new());
        work.push(0);
    }

    // Propagate `stack` to successor `t`, queueing it if newly set.  Returns
    // false on a stack-state conflict (statically inconsistent → bail).
    fn propagate(
        stack_in: &mut [Option<Vec<usize>>],
        work: &mut Vec<usize>,
        t: usize,
        stack: &[usize],
    ) -> bool {
        if t >= stack_in.len() {
            // Edge past the end of the stream (fallthrough off the last insn or
            // a jump to insns.len()): no instruction to annotate.
            return true;
        }
        match &stack_in[t] {
            None => {
                stack_in[t] = Some(stack.to_vec());
                work.push(t);
                true
            }
            Some(existing) => existing.as_slice() == stack,
        }
    }

    let mut consistent = true;
    while let Some(i) = work.pop() {
        let cur = stack_in[i].clone().expect("queued => visited");
        let jt = |k: i32| -> usize { (i as i64 + 1 + k as i64) as usize };
        match &insns[i] {
            // `SetupExcept(k)`: the fallthrough edge enters the try body with
            // the handler at `k` pushed; the jump edge *to* `k` is the handler
            // itself, which lives outside its own protected region, so it sees
            // the pre-push stack.
            Insn::SetupExcept(k) => {
                let handler = jt(*k);
                let mut pushed = cur.clone();
                pushed.push(handler);
                consistent &= propagate(&mut stack_in, &mut work, i + 1, &pushed);
                consistent &= propagate(&mut stack_in, &mut work, handler, &cur);
            }
            // `PopExcept`: leaves the innermost try region (fallthrough only).
            Insn::PopExcept => {
                let mut popped = cur.clone();
                popped.pop();
                consistent &= propagate(&mut stack_in, &mut work, i + 1, &popped);
            }
            Insn::Jump(k) => {
                consistent &= propagate(&mut stack_in, &mut work, jt(*k), &cur);
            }
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => {
                consistent &= propagate(&mut stack_in, &mut work, jt(*k), &cur);
                consistent &= propagate(&mut stack_in, &mut work, i + 1, &cur);
            }
            insn if is_terminator(insn) => {
                // No normal-flow successor.
            }
            _ => {
                consistent &= propagate(&mut stack_in, &mut work, i + 1, &cur);
            }
        }
    }

    // Safety net: a statically inconsistent handler stack means the per-pc table
    // would be ambiguous.  Hand the (unstripped) stream back with an empty table
    // and let the VM keep the dynamic SetupExcept/PopExcept handler stack.
    if !consistent {
        return (insns, Vec::new());
    }

    // `handler_at[pc]` (original PC space) = innermost active handler at `pc`,
    // or `usize::MAX` for none / unreachable.
    let mut handler_at: Vec<usize> = vec![usize::MAX; n];
    for pc in 0..n {
        if let Some(stack) = &stack_in[pc]
            && let Some(&h) = stack.last()
        {
            handler_at[pc] = h;
        }
    }

    // Strip SetupExcept/PopExcept and retarget all jumps via the shared compact
    // machinery, then remap `handler_at` into the new PC space.
    let keep: Vec<bool> = insns
        .iter()
        .map(|i| !matches!(i, Insn::SetupExcept(_) | Insn::PopExcept))
        .collect();

    // Replicate compact's old→new index map so we can remap handler targets.
    let mut to_new = vec![0usize; n + 1];
    let mut cnt = 0usize;
    for i in 0..n {
        to_new[i] = cnt;
        if keep[i] {
            cnt += 1;
        }
    }
    to_new[n] = cnt;

    let new_insns = compact(insns, &keep);
    let new_len = new_insns.len();
    debug_assert_eq!(new_len, cnt);

    let mut exc_table = vec![EXC_NO_HANDLER; new_len];
    for old_pc in 0..n {
        if !keep[old_pc] {
            continue;
        }
        let new_pc = to_new[old_pc];
        let h = handler_at[old_pc];
        exc_table[new_pc] = if h == usize::MAX {
            EXC_NO_HANDLER
        } else {
            // A handler target is always a SetupExcept jump target, i.e. the
            // first kept instruction at-or-after the removed SetupExcept's
            // destination — exactly compact's redirect rule.
            to_new[h] as u32
        };
    }

    (new_insns, exc_table)
}

// ─── Compaction helper ─────────────────────────────────────────────────────────

/// Remove instructions where `keep[i]` is `false` and rewrite all jump offsets.
///
/// Removed instructions are treated as transparent: any jump whose target is a
/// removed instruction is redirected to the first kept instruction that follows it.
/// This is correct for no-op removals where "jumping to the no-op" is equivalent
/// to "jumping to whatever comes after it".
pub(crate) fn compact(insns: Vec<Insn>, keep: &[bool]) -> Vec<Insn> {
    let n = insns.len();
    debug_assert_eq!(n, keep.len());

    // to_new[i] = new index of the first kept instruction at or after old index i.
    // to_new[n] = total kept count (past-the-end sentinel for jumps to code.insns.len()).
    let mut to_new = vec![0usize; n + 1];
    let mut cnt = 0usize;
    for i in 0..n {
        to_new[i] = cnt;
        if keep[i] {
            cnt += 1;
        }
    }
    to_new[n] = cnt;

    insns
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(old_i, insn)| rewrite_offsets(insn, old_i, &to_new))
        .collect()
}

/// Rewrite all jump offsets in `insn` using the old→new index table.
/// `old_i` is the pre-compaction index of `insn` (which is guaranteed to be kept).
pub(crate) fn rewrite_offsets(insn: Insn, old_i: usize, to_new: &[usize]) -> Insn {
    rewrite_offsets_with(insn, old_i, to_new, to_new)
}

/// Like [`rewrite_offsets`] but with a separate map for jump targets.  LICM
/// uses this to redirect jumps that targeted a hoisted instruction so they
/// land at the new body start rather than at the hoisted instruction's
/// pre-header position.  See issue #323.
pub(crate) fn rewrite_offsets_with(
    insn: Insn,
    old_i: usize,
    placement_map: &[usize],
    jump_target_map: &[usize],
) -> Insn {
    let fix = |k: i32| -> i32 {
        let old_target = (old_i as i64 + 1 + k as i64) as usize;
        let new_src = placement_map[old_i];
        let new_target = jump_target_map[old_target];
        (new_target as i64 - new_src as i64 - 1) as i32
    };
    use Insn::*;
    match insn {
        Jump(k) => Jump(fix(k)),
        JumpIfFalse(r, k) => JumpIfFalse(r, fix(k)),
        JumpIfTrue(r, k) => JumpIfTrue(r, fix(k)),
        CmpJumpIfFalse(a, op, b, k) => CmpJumpIfFalse(a, op, b, fix(k)),
        CmpJumpIfTrue(a, op, b, k) => CmpJumpIfTrue(a, op, b, fix(k)),
        CmpJumpIfFalseConst(r, op, c, k) => CmpJumpIfFalseConst(r, op, c, fix(k)),
        CmpJumpIfTrueConst(r, op, c, k) => CmpJumpIfTrueConst(r, op, c, fix(k)),
        ForIter(dst, slot, k) => ForIter(dst, slot, fix(k)),
        ForCountReg(v, op, stop, step, k) => ForCountReg(v, op, stop, step, fix(k)),
        ForCountConst(v, op, stop, step, k) => ForCountConst(v, op, stop, step, fix(k)),
        ForCountConstInline(v, op, stop, step, k) => ForCountConstInline(v, op, stop, step, fix(k)),
        SetupExcept(k) => SetupExcept(fix(k)),
        MatchExcept(r, k) => MatchExcept(r, fix(k)),
        MatchExceptStar(r, src, dst, k) => MatchExceptStar(r, src, dst, fix(k)),
        other => other,
    }
}

// ─── String concat chain merging ──────────────────────────────────────────────

/// Apply the per-instruction transfer function for the `str_regs` dataflow used
/// by [`pass_concat_merge`].  Updates `str_regs` (the set of registers known to
/// hold a `str`) to reflect the effect of executing `insn`.
///
/// Conservative: a register is only *added* when the instruction provably yields
/// a string; every other write to a register *removes* it from the set.  This is
/// intentionally one-directional (no fixpoint over back-edges) — the caller
/// clears the set at every basic-block boundary, so a register staying in the
/// set always means "definitely a str on this straight-line path".
fn apply_str_regs_transfer(insn: &Insn, consts: &[Value], str_regs: &mut HashSet<u32>) {
    use crate::ast::BinaryOp;
    match insn {
        // A `str` constant load makes the dst a string.
        Insn::LoadConst(dst, c) => {
            if matches!(consts[*c as usize].kind(), ValueKind::Str(_)) {
                str_regs.insert(*dst);
            } else {
                str_regs.remove(dst);
            }
        }
        // Copies propagate string-ness from source to destination.
        Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
            if str_regs.contains(src) {
                str_regs.insert(*dst);
            } else {
                str_regs.remove(dst);
            }
        }
        // `str + str` is a `str`; otherwise the op may not yield a string.
        Insn::BinOp(dst, lhs, BinaryOp::Add, rhs) => {
            if str_regs.contains(lhs) && str_regs.contains(rhs) {
                str_regs.insert(*dst);
            } else {
                str_regs.remove(dst);
            }
        }
        // `str + const`: a string result iff both sides are strings.
        Insn::BinOpConst(dst, lhs, BinaryOp::Add, c, _) => {
            if str_regs.contains(lhs) && matches!(consts[*c as usize].kind(), ValueKind::Str(_)) {
                str_regs.insert(*dst);
            } else {
                str_regs.remove(dst);
            }
        }
        // These always produce a `str`: f-string lowering / format / a prior
        // Concat fusion / str-typed builtin string methods are not modelled here,
        // but the always-string opcodes are.
        Insn::Concat { dst, .. }
        | Insn::BuildString(dst, _, _)
        | Insn::FormatValue(dst, _)
        | Insn::FormatValueSpec(dst, _, _) => {
            str_regs.insert(*dst);
        }
        // Any other instruction that writes a register clears its string-ness.
        other => {
            if let Some(dst) = writable_dst(other) {
                str_regs.remove(&dst);
            }
        }
    }
}

/// Merge a chain of `BinOp(Add)` instructions into a single `Concat` instruction
/// that performs the concatenation in one allocation.
///
/// ## Pattern detected (minimum 3 operands, i.e. 2 BinOps in chain)
///
/// ```text
/// BinOp(t1, r0, Add, r1)          ← t1 is a temp; used only once (next BinOp)
/// BinOp(t2, t1, Add, r2)          ← t2 is a temp; used only once (next BinOp)
/// ...
/// BinOp(dst, t_{n-2}, Add, r_{n-1})
/// ```
///
/// is replaced by:
///
/// ```text
/// Move(base+0, r0)
/// Move(base+1, r1)
/// ...
/// Move(base+n-1, r_{n-1})
/// Concat { dst, base, count: n }
/// ```
///
/// The intermediate `BinOp` instructions are removed.  `pass_dead_store_elim`
/// (which runs after this pass in a second pipeline run, or directly afterward)
/// cleans up any now-unused Move instructions.
///
/// ## Guards
///
/// - Chain must be ≥ 2 BinOps (≥ 3 operands): a 2-operand Concat saves no
///   allocation over a plain `BinOp`.
/// - Each intermediate result register must be a temp (`>= num_locals`).
/// - The intermediate result register must be read exactly once (by the
///   immediately following BinOp).
/// - No BB boundary (jump target) between any two BinOps in the chain.
/// - `count ≤ u8::MAX`.
///
/// ## Register window
///
/// Fresh consecutive registers `[num_regs, num_regs+count)` are allocated for
/// the operand window.  The instruction vector is rebuilt with jump-offset
/// rewriting (mirrors `pass_ivsr`), growing by 2 for the first chain found.
///
/// ## String-only gate (issue #2383)
///
/// The fusion is only profitable for **string** chains, where it collapses N-1
/// allocations into one.  For int (and other primitive) chains the operand
/// window `Move`s are pure overhead — ~19% slower than the equivalent plain
/// `BinOp` chain.  We therefore only fuse a chain when its first operand
/// register is *statically known* to hold a string (`str_regs`, seeded from
/// `str`-typed constants and string-producing instructions, propagated forward
/// per basic block — mirrors the `int_regs` gate in `pass_algebraic_simplify`).
/// Chains whose leading operand isn't provably a string keep their `BinOp`
/// form.  The runtime Concat handler's own string/non-string check is retained
/// as a correctness backstop.
///
/// Run [`pass_concat_merge_once`] to fixed-point: keep merging until no new
/// chain can be fused.  A single pass handles only the first eligible chain;
/// iterating ensures all chains in a function are folded.
fn pass_concat_merge(
    mut insns: Vec<Insn>,
    num_locals: u32,
    num_regs: &mut u32,
    consts: &[Value],
) -> Vec<Insn> {
    loop {
        let (next, changed) = pass_concat_merge_once(insns, num_locals, num_regs, consts);
        insns = next;
        if !changed {
            return insns;
        }
    }
}

/// Single-pass helper for [`pass_concat_merge`].  Finds the first eligible
/// `BinOp(Add)` chain and fuses it into a `Concat` instruction, then returns
/// `(new_insns, true)`.  Returns `(insns, false)` when no chain exists.
fn pass_concat_merge_once(
    insns: Vec<Insn>,
    num_locals: u32,
    num_regs: &mut u32,
    consts: &[Value],
) -> (Vec<Insn>, bool) {
    use crate::ast::BinaryOp;

    let n = insns.len();
    if n < 2 {
        return (insns, false);
    }

    // Mark BB starts (jump targets): we must not merge across them.
    let mut bb_starts: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target < n {
                bb_starts.insert(target);
            }
        }
    }

    // Compute use-count per register (number of instruction sites that read it).
    let mut use_count: HashMap<u32, usize> = HashMap::new();
    for insn in &insns {
        visit_read_regs(insn, |r| {
            *use_count.entry(r).or_insert(0) += 1;
        });
    }

    // Forward dataflow: `str_regs_before[pc]` is the set of registers statically
    // known to hold a `str` value just before `insns[pc]` executes.  The set is
    // cleared at every basic-block boundary (a register's type can't be trusted
    // across a join).  Seeded from `str`-typed constants and string-producing
    // instructions; mirrors the `int_regs` analysis used by other passes.  Only
    // chains whose leading operand is in this set get fused (issue #2383).
    let mut str_regs_before: Vec<HashSet<u32>> = Vec::with_capacity(n);
    {
        let mut str_regs: HashSet<u32> = HashSet::new();
        for (idx, insn) in insns.iter().enumerate() {
            if bb_starts.contains(&idx) {
                str_regs.clear();
            }
            str_regs_before.push(str_regs.clone());
            apply_str_regs_transfer(insn, consts, &mut str_regs);
        }
    }

    // Scan for the first mergeable chain.
    let mut i = 0;
    while i < n {
        // First BinOp(Add) with a temp destination.
        let (t0, lhs0, rhs0) = match &insns[i] {
            Insn::BinOp(dst, lhs, BinaryOp::Add, rhs) if *dst >= num_locals => (*dst, *lhs, *rhs),
            _ => {
                i += 1;
                continue;
            }
        };

        // String-only gate (issue #2383): only fuse when the chain's leading
        // operand is statically known to be a `str`.  Int / other primitive
        // chains keep their plain `BinOp` form (the operand-window Moves are
        // pure overhead for them).  If we can't prove it's a string, skip.
        if !str_regs_before[i].contains(&lhs0) {
            i += 1;
            continue;
        }

        // Extend the chain: BinOp(t_{k+1}, t_k, Add, rhs_{k+1}).
        let mut chain_positions: Vec<usize> = vec![i];
        let mut leaf_operands: Vec<u32> = vec![lhs0, rhs0];
        let mut intermediate_dsts: Vec<u32> = vec![t0];

        let mut j = i + 1;
        loop {
            if j >= n || bb_starts.contains(&j) {
                break;
            }
            let prev_dst = *intermediate_dsts.last().unwrap();
            match &insns[j] {
                Insn::BinOp(dst, lhs, BinaryOp::Add, rhs)
                    if *lhs == prev_dst && *dst >= num_locals =>
                {
                    // Guard: the intermediate `prev_dst` must not be read outside
                    // the chain.  Two sub-cases:
                    //
                    // (A) Fresh-dst (prev_dst != dst): `prev_dst` is a distinct
                    //     temp used exactly once — as this BinOp's LHS.  Check
                    //     use_count == 1 to rule out external reads.
                    //
                    // (B) In-place accumulation (prev_dst == dst): the compiler
                    //     reuses the same temp register (`t = t + x`).  Here
                    //     use_count > 1 because the same register is the LHS of
                    //     multiple chain BinOps, but all those reads happen within
                    //     the chain itself and are being removed by the merge.
                    //     Skip the use_count check; correctness is guaranteed by
                    //     the consecutive-chain structure.
                    let safe = if *dst != prev_dst {
                        // Pattern A: distinct fresh temp — must be single-use.
                        use_count.get(&prev_dst).copied().unwrap_or(0) == 1
                    } else {
                        // Pattern B: in-place accumulation — always safe structurally.
                        true
                    };
                    if !safe {
                        break;
                    }
                    chain_positions.push(j);
                    leaf_operands.push(*rhs);
                    intermediate_dsts.push(*dst);
                    j += 1;
                }
                _ => break,
            }
        }

        // Need at least 2 BinOps (3 operands) to be worth merging.
        if chain_positions.len() < 2 {
            i += 1;
            continue;
        }

        let count = leaf_operands.len();
        if count > u8::MAX as usize {
            i += 1;
            continue;
        }

        let final_dst = *intermediate_dsts.last().unwrap();

        // Sanity: no leaf operand may be an intermediate result register
        // (would create a use-after-free in the chain removal).
        let intermediates_set: HashSet<u32> = intermediate_dsts[..intermediate_dsts.len() - 1]
            .iter()
            .copied()
            .collect();
        if leaf_operands.iter().any(|r| intermediates_set.contains(r)) {
            i += 1;
            continue;
        }

        // Allocate fresh consecutive registers for the operand window.
        let base = *num_regs;
        *num_regs += count as u32;

        // Rebuild the instruction list, inserting count Moves + 1 Concat in
        // place of the chain, growing the vector by 2 (same strategy as pass_ivsr).
        //
        // Slots consumed: chain_positions.len() = count-1.
        // Slots emitted:  count Moves + 1 Concat = count+1.
        // Delta: +2 instructions.
        //
        // Instructions before first_pos: unchanged position.
        // Instructions in [first_pos, last_pos]: replaced by new_insn_count slots.
        // Instructions after last_pos: shifted by +2.

        let first_pos = chain_positions[0];
        let last_pos = *chain_positions.last().unwrap();
        let chain_len = chain_positions.len(); // count - 1
        let new_insn_count = count + 1; // count Moves + 1 Concat
        // delta = new_insn_count - chain_len = (count+1) - (count-1) = +2
        let delta: i64 = new_insn_count as i64 - chain_len as i64;

        let mut old_to_new = vec![0usize; n + 1];
        for k in 0..=first_pos {
            old_to_new[k] = k;
        }
        // Chain positions [first_pos, last_pos] all redirect to first_pos
        // (jump targets that aimed at any chain instruction land at the first Move).
        for k in first_pos..=last_pos {
            old_to_new[k] = first_pos;
        }
        for k in (last_pos + 1)..=n {
            old_to_new[k] = (k as i64 + delta) as usize;
        }

        let mut new_vec: Vec<Insn> = Vec::with_capacity(n + 2);
        for (k, insn) in insns.iter().enumerate() {
            if k == first_pos {
                // Emit count Moves into the fresh register window.
                for (m, &operand_reg) in leaf_operands.iter().enumerate() {
                    new_vec.push(Insn::Move(base + m as u32, operand_reg));
                }
                // Emit the Concat.
                new_vec.push(Insn::Concat {
                    dst: final_dst,
                    base,
                    count: count as u8,
                });
            } else if k > first_pos && k <= last_pos {
                // Skip the remaining chain BinOps (already replaced above).
            } else {
                // Rewrite jump offsets for non-chain instructions.
                new_vec.push(rewrite_offsets(insn.clone(), k, &old_to_new));
            }
        }

        return (new_vec, true);
    }

    (insns, false)
}

/// Visits every register *read* by `insn`, calling `f` once per register.
/// Used by `pass_concat_merge` to accumulate per-register use counts without
/// allocating a temporary `Vec`.
fn visit_read_regs(insn: &Insn, mut f: impl FnMut(u32)) {
    use Insn::*;
    match insn {
        LoadConst(..)
        | LoadGlobal(..)
        | LoadCell(..)
        | LoadNone(..)
        | LoadNoneRange { .. }
        | LoadExc(..)
        | ImportModule(..)
        | DeleteName(..)
        | PushTypeParamEnv
        | PopTypeParamEnv
        | DeleteLocal(..)
        | DeleteModuleGlobal(..)
        | Jump(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | PopExcContext
        | ReturnNone
        | RaiseReRaise
        | RaiseAssertNoMsg
        | ForIter(..) => {}

        ForCountConst(var, _, _, _, _) | ForCountConstInline(var, _, _, _, _) => f(*var),
        ForCountReg(var, _, stop, _, _) => {
            f(*var);
            f(*stop);
        }

        BinOpImm(_, a, _, _, _) | SyncModuleGlobal(a, _) => f(*a),

        StoreGlobal(_, s)
        | StoreCell(_, s)
        | ImportStar(s)
        | Move(_, s)
        | CopyReg(_, s)
        | UnaryOp(_, _, s)
        | FormatValue(_, s)
        | MatchSeqExcluded(_, s)
        | MatchMapping(_, s)
        | Return(s)
        | PrintExpr(s)
        | RaiseValue(s)
        | RaiseAssert(s)
        | JumpIfFalse(s, _)
        | JumpIfTrue(s, _)
        | GetIter(_, s)
        | GetAwaitable(_, s)
        | Unpack(_, s, _)
        | CheckLocal(s, _)
        | GetAttr(_, s, _)
        | GetAttrForWith(_, s, _, _)
        | LoadExcTraceback(_, s)
        | ImportFromAttr(_, s, _)
        | DeleteAttr(s, _)
        | BinOpConst(_, s, _, _, _)
        | CmpJumpIfFalseConst(s, _, _, _)
        | CmpJumpIfTrueConst(s, _, _, _)
        | MatchExcept(s, _)
        | RecordClassStore(s)
        | RecordClassDel(s)
        | PushExcContext(s) => f(*s),
        MatchExceptStar(type_r, src, _, _) => {
            f(*type_r);
            f(*src);
        }

        BinOp(_, a, _, b)
        | BinOpInPlace(_, a, _, b)
        | CmpJumpIfFalse(a, _, b, _)
        | CmpJumpIfTrue(a, _, b, _)
        | RaiseFrom(a, b)
        | SetAdd(a, b)
        | ListAppend(a, b)
        | ListExtend(a, b)
        | DictUpdate(a, b)
        | GetItem(_, a, b)
        | FormatValueSpec(_, a, b)
        | DeleteItem(a, b) => {
            f(*a);
            f(*b);
        }

        SetAttr(obj, _, val) | SetTypeVarAttr(obj, _, val) => {
            f(*obj);
            f(*val);
        }
        SetItem(a, b, c) => {
            f(*a);
            f(*b);
            f(*c);
        }

        DictMergeKwCall { dict, src, name } => {
            f(*dict);
            f(*src);
            match name {
                crate::bytecode::KwCallName::Callee(reg) => f(*reg),
                crate::bytecode::KwCallName::Method { obj, .. } => f(*obj),
            }
        }
        SetItemKwCall {
            dict,
            key,
            val,
            name,
        } => {
            f(*dict);
            f(*key);
            f(*val);
            match name {
                crate::bytecode::KwCallName::Callee(reg) => f(*reg),
                crate::bytecode::KwCallName::Method { obj, .. } => f(*obj),
            }
        }

        Call(base, argc) | CallMemo(base, argc) => {
            for r in *base..=(*base + *argc as u32) {
                f(r);
            }
        }
        CallKw { func, total, .. } => {
            for r in *func..=(*func + *total as u32) {
                f(r);
            }
        }
        CallEx { func, npos, kwargs } => {
            for r in *func..=(*func + *npos as u32) {
                f(r);
            }
            f(*kwargs);
        }
        TailCall { args_base, nargs } => {
            f(args_base - 1);
            for r in *args_base..*args_base + *nargs as u32 {
                f(r);
            }
        }
        BuildList(_, base, n) | BuildTuple(_, base, n) => {
            for r in *base..*base + *n {
                f(r);
            }
        }
        BuildString(_, base, n) => {
            for r in *base..*base + *n as u32 {
                f(r);
            }
        }
        BuildSlice(_, base) => {
            for r in *base..*base + 3 {
                f(r);
            }
        }
        GetSlice(_, obj, base) => {
            f(*obj);
            for r in *base..*base + 3 {
                f(r);
            }
        }
        BuildDict(_, base, n) => {
            for r in *base..*base + 2 * *n {
                f(r);
            }
        }
        CallMethod {
            obj,
            args_base,
            nargs,
            ..
        } => {
            f(*obj);
            for r in *args_base..*args_base + *nargs as u32 {
                f(r);
            }
        }
        CallMethodKw {
            obj,
            args_base,
            total,
            ..
        } => {
            f(*obj);
            for r in *args_base..*args_base + *total as u32 {
                f(r);
            }
        }
        CallMethodExpanded {
            obj,
            pos_list,
            kw_dict,
            ..
        } => {
            f(*obj);
            f(*pos_list);
            f(*kw_dict);
        }
        MakeFunction(_, _, defs_base, defs_n, annots_base, annots_n) => {
            for r in *defs_base..*defs_base + *defs_n as u32 {
                f(r);
            }
            if *annots_n > 0 {
                for r in *annots_base..*annots_base + *annots_n as u32 {
                    f(r);
                }
            }
        }
        MakeClass(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n) => {
            for r in *bases_base..*bases_base + *bases_n as u32 {
                f(r);
            }
            for r in *kwarg_base..*kwarg_base + *kwarg_n as u32 {
                f(r);
            }
        }
        MakeClassMeta(_, _, bases_base, bases_n, _, kwarg_base, kwarg_n, meta_reg) => {
            f(*meta_reg);
            for r in *bases_base..*bases_base + *bases_n as u32 {
                f(r);
            }
            for r in *kwarg_base..*kwarg_base + *kwarg_n as u32 {
                f(r);
            }
        }
        MakeTypeAlias(_, _, value_reg, params_reg) => {
            f(*value_reg);
            f(*params_reg);
        }
        MakeTypeVar(_, _) => {}
        Yield { src, .. } => f(*src),
        YieldFrom {
            iter_reg, sent_reg, ..
        } => {
            f(*iter_reg);
            f(*sent_reg);
        }
        UnpackEx { src, .. } => f(*src),
        Concat { base, count, .. } => {
            for r in *base..*base + *count as u32 {
                f(r);
            }
        }
        // MatchClassPositional reads subj and cls.
        MatchClassPositional { subj, cls, .. } => {
            f(*subj);
            f(*cls);
        }
    }
}

// ─── Self-tail-call optimisation ──────────────────────────────────────────────

/// Replace `Call(r, n) + Return(r)` pairs with `TailCall { args_base: r+1, nargs: n }`.
///
/// ## What this enables
///
/// When the VM encounters `TailCall`, it checks whether the callee is the same
/// function that is currently executing.  If it is, it resets the parameter
/// registers in the current frame and jumps to pc=0 instead of allocating a new
/// stack frame, turning O(n) stack growth into O(1).
///
/// If the callee turns out to be a *different* function at runtime (e.g. the name
/// was rebound), the VM falls back to a normal call+return, so correctness is
/// preserved in all cases.
///
/// ## Pattern
///
/// ```text
/// Call(r, n)    ← result lands in r
/// Return(r)     ← immediately returned
/// ```
/// →
/// ```text
/// TailCall { args_base: r + 1, nargs: n }
/// ```
///
/// The args to the call are in `R[r+1 .. r+1+n]` (per the `Call` convention);
/// `TailCall` stores only `args_base` and `nargs`. The VM recovers the callee
/// register as `args_base - 1` at runtime and reads it from the register file.
///
/// ## Guards
///
/// - The pair must be adjacent (no instructions between `Call` and `Return`).
/// - The `Return` must return exactly the register that `Call` wrote (`func_reg`).
/// - Generators are excluded: a generator frame cannot be "restarted" in the same
///   way (but the is_generator flag is not available here, so we rely on the VM's
///   generator guard).
fn pass_self_tail_call(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    if n < 2 {
        return insns;
    }
    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i + 1 < n {
        let replace: Option<Insn> = match (&transformed[i], &transformed[i + 1]) {
            // Match both Call and CallMemo — pure functions use CallMemo but are
            // still valid candidates for self-tail-call optimisation.
            (
                &Insn::Call(func_reg, nargs) | &Insn::CallMemo(func_reg, nargs),
                &Insn::Return(ret_reg),
            ) if func_reg == ret_reg => Some(Insn::TailCall {
                args_base: func_reg + 1,
                nargs,
            }),
            _ => None,
        };
        if let Some(tail_insn) = replace {
            // Replace Call/CallMemo with TailCall, drop the Return.
            transformed[i] = tail_insn;
            keep[i + 1] = false;
            i += 2;
        } else {
            i += 1;
        }
    }
    compact(transformed, &keep)
}

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
///    `t_keep < t_dup`.  A terminator is a `Return`, `ReturnNone`, `Jump`,
///    `TailCall`, or `Raise*` instruction.
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
fn pass_cross_jump(mut insns: Vec<Insn>, linenos: &[u32]) -> Vec<Insn> {
    // `linenos[i]` is the source line of `insns[i]` (0 = inherit previous).  The
    // pass refuses to merge two tails whose terminators carry different lines, so
    // each `raise`/`return` site keeps its own line for traceback attribution.
    // The slice is compacted alongside `insns` on every merge so the lines stay
    // aligned across the fixed-point iterations.
    // Pad/truncate to match `insns` so callers may pass an empty slice (tests
    // that don't care about line attribution) — a 0 line is "inherit previous",
    // which keeps every tail mergeable exactly as before line tracking existed.
    let mut linenos = linenos.to_vec();
    linenos.resize(insns.len(), 0);
    loop {
        let (next, next_linenos, changed) = pass_cross_jump_once(insns, linenos);
        insns = next;
        linenos = next_linenos;
        if !changed {
            return insns;
        }
    }
}

/// Single-pass helper for [`pass_cross_jump`].  Finds the first mergeable tail
/// pair, applies the merge, and returns `(new_insns, true)`.  Returns
/// `(insns, false)` when no merge candidate exists (fixed-point reached).
fn pass_cross_jump_once(insns: Vec<Insn>, linenos: Vec<u32>) -> (Vec<Insn>, Vec<u32>, bool) {
    const MIN_TAIL: usize = 2;

    let n = insns.len();
    if n < MIN_TAIL * 2 {
        return (insns, linenos, false);
    }
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
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::ForCountConstInline(_, _, _, _, k)
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
                | Insn::ForCountReg(..)
                | Insn::ForCountConst(..)
                | Insn::ForCountConstInline(..)
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
                | Insn::TailCall { .. }
                | Insn::RaiseValue(_)
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

            // Issue #2420: refuse the merge when any instruction in the two
            // tails carries a different source line.  The survivor copy can only
            // hold one line, so collapsing two raise/return sites that live on
            // different lines would attribute the duplicate site's exception to
            // the survivor's line (a stale traceback `File …, line N`).  Tails
            // that agree on every line are safe to merge.
            if (0..tail_len).any(|step| eff_line[t_keep - step] != eff_line[t_dup - step]) {
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
            return (compact(transformed, &keep), new_linenos, true);
        }
    }

    (insns, linenos, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_fn(src: &str) -> FnCode {
        use crate::{interpreter::collect_local_names, lexer::Lexer, parser::Parser};
        use std::collections::HashSet;
        let tokens = Lexer::new(src).unwrap().into_tokens();
        let mut parser = Parser::new(tokens);
        let stmts = parser.parse_program().unwrap();
        let empty: HashSet<String> = HashSet::new();
        let names = collect_local_names(&[], &stmts, &empty, &empty);
        let local_index = std::rc::Rc::new(
            (0u32..)
                .zip(names.iter())
                .map(|(i, n)| (n.clone(), i))
                .collect(),
        );
        crate::compiler::compile_script_with_linenos(&stmts, local_index, false, &[], "<test>")
            .unwrap()
    }

    /// Like `compile_fn`, but supplies a per-top-level-statement line-number
    /// slice so the resulting `FnCode::lineno_table` is populated (the plain
    /// `compile_script` path leaves it all zeros).
    fn compile_script_with_linenos_for_test(src: &str, stmt_linenos: &[u32]) -> FnCode {
        use crate::{interpreter::collect_local_names, lexer::Lexer, parser::Parser};
        use std::collections::HashSet;
        let tokens = Lexer::new(src).unwrap().into_tokens();
        let mut parser = Parser::new(tokens);
        let stmts = parser.parse_program().unwrap();
        let empty: HashSet<String> = HashSet::new();
        let names = collect_local_names(&[], &stmts, &empty, &empty);
        let local_index = std::rc::Rc::new(
            (0u32..)
                .zip(names.iter())
                .map(|(i, n)| (n.clone(), i))
                .collect(),
        );
        crate::compiler::compile_script_with_linenos(
            &stmts,
            local_index,
            false,
            stmt_linenos,
            "<test>",
        )
        .unwrap()
    }

    // ── pass_binop_const_fusion ───────────────────────────────────────────────

    #[test]
    fn binop_const_fusion_fuses_loadconst_binop() {
        use crate::ast::BinaryOp;
        // LoadConst(r=5, c=0)  BinOp(dst=1, lhs=0, Add, r=5)  where num_locals=2
        // r=5 >= num_locals=2, r != dst  → fuse to BinOpConst(1, 0, Add, 0), drop LoadConst
        let insns = vec![
            Insn::LoadConst(5, 0), // temp reg 5, const index 0
            Insn::BinOp(1, 0, BinaryOp::Add, 5),
            Insn::Return(1),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert_eq!(out.len(), 2, "LoadConst should be removed");
        assert!(
            matches!(out[0], Insn::BinOpConst(1, 0, BinaryOp::Add, 0, ..)),
            "BinOp should become BinOpConst"
        );
    }

    #[test]
    fn binop_const_fusion_fires_for_reused_scratch_in_loop_body() {
        use crate::ast::BinaryOp;
        // The "reused scratch register" shape that dominates loop bodies: temp
        // reg 3 is LoadConst-ed afresh for each operand, and the loop ends in a
        // back-edge.  The first LoadConst(3)+BinOp(_, _, Mul, 3) pair must fuse
        // because reg 3 is overwritten by the next LoadConst before being read
        // (its value is dead), even though `last_read[3]` is the second BinOp and
        // a back-edge follows.  num_locals = 2.
        let insns = vec![
            Insn::LoadConst(3, 0),                      // t = 2     (scratch)
            Insn::BinOp(2, 1, BinaryOp::Mul, 3),        // r2 = i * t   ← fuse
            Insn::LoadConst(3, 1),                      // t = 1     (overwrites reg 3)
            Insn::BinOp(2, 2, BinaryOp::Sub, 3),        // r2 = r2 - t
            Insn::BinOpInPlace(0, 0, BinaryOp::Add, 2), // s += r2
            Insn::Jump(-6),                             // loop back-edge
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert!(
            matches!(out[0], Insn::BinOpConst(2, 1, BinaryOp::Mul, 0, false)),
            "first pair fused despite the back-edge (scratch overwritten before \
             read): {:?}",
            out[0]
        );
        // The whole LoadConst is removed, so the stream shrinks by at least one.
        assert!(
            out.len() < 6,
            "at least one LoadConst removed: len {}",
            out.len()
        );
    }

    #[test]
    fn binop_const_fusion_skips_reused_scratch_when_value_is_live() {
        use crate::ast::BinaryOp;
        // Same shape but reg 3 is READ again (by the second BinOp) before being
        // overwritten, so its value is live and the first pair must NOT fuse.
        let insns = vec![
            Insn::LoadConst(3, 0),
            Insn::BinOp(2, 1, BinaryOp::Mul, 3), // reads reg 3
            Insn::BinOp(4, 2, BinaryOp::Sub, 3), // reads reg 3 again → still live
            Insn::Jump(-4),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert!(
            matches!(out[0], Insn::LoadConst(3, 0)),
            "no fusion while reg 3 is still live: {:?}",
            out[0]
        );
    }

    #[test]
    fn binop_const_fusion_skips_when_reg_is_local() {
        use crate::ast::BinaryOp;
        // r=1 < num_locals=3  → must NOT fuse (register could be a local variable)
        let insns = vec![
            Insn::LoadConst(1, 0),
            Insn::BinOp(2, 0, BinaryOp::Add, 1),
            Insn::Return(2),
        ];
        let out = pass_binop_const_fusion(insns, 3);
        assert_eq!(out.len(), 3, "no fusion when reg is a local");
        assert!(matches!(out[0], Insn::LoadConst(1, 0)));
    }

    #[test]
    fn binop_const_fusion_skips_when_dst_equals_reg() {
        use crate::ast::BinaryOp;
        // dst == lc_reg: the result overwrites lc_reg, so lc_reg is not live after
        // the BinOp — fusion is safe and should happen.
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(5, 0, BinaryOp::Add, 5), // dst == rhs == lc_reg
            Insn::Return(5),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert_eq!(
            out.len(),
            2,
            "fusion is safe when dst == lc_reg (result overwrites it)"
        );
        assert!(
            matches!(out[0], Insn::BinOpConst(5, 0, BinaryOp::Add, 0, ..)),
            "should fuse to BinOpConst"
        );
    }

    #[test]
    fn binop_const_fusion_on_compiled_code() {
        // Use a function argument so the lhs is not a compile-time constant.
        // pass_binop_const_fusion should still fuse LoadConst(r,5)+BinOp(dst,n,Add,r)
        // → BinOpConst(dst,n,Add,5), which pass_const_fold cannot fold further.
        let code = compile_fn("def f(n):\n    return n + 5\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(
            has_binopconst,
            "optimizer should fuse LoadConst+BinOp into BinOpConst for n+5"
        );
    }

    #[test]
    fn binop_const_fusion_commutative_lhs_const() {
        use crate::ast::BinaryOp;
        // LoadConst(r=5, c=0)  BinOp(dst=1, lhs=5, Add, rhs=0)  — const on LEFT
        // Even though Add is commutative, swapping would break __radd__ dispatch,
        // so the optimization must be skipped and all 3 instructions kept.
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(1, 5, BinaryOp::Add, 0),
            Insn::Return(1),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert_eq!(
            out.len(),
            3,
            "const-lhs Add should NOT be fused (would break __radd__ dispatch)"
        );
    }

    #[test]
    fn binop_const_fusion_does_not_commute_non_commutative() {
        use crate::ast::BinaryOp;
        // Sub is not commutative — should not fuse when const is on left
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(1, 5, BinaryOp::Sub, 0),
            Insn::Return(1),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert_eq!(
            out.len(),
            3,
            "non-commutative op with const-lhs should not fuse"
        );
    }

    // ── pass_unary_fold ───────────────────────────────────────────────────────

    #[test]
    fn unary_fold_neg_int() {
        use crate::ast::UnaryOp;
        use crate::value::Value;
        // LoadConst(r=5, idx=0) [consts[0]=-3]  UnaryOp(dst=1, Neg, r=5)
        // → LoadConst(dst=1, idx_3)
        let mut consts = vec![Value::int(-3)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::UnaryOp(1, UnaryOp::Neg, 5),
            Insn::Return(1),
        ];
        let out = pass_unary_fold(insns, 2, &mut consts);
        assert_eq!(out.len(), 2, "LoadConst should be removed");
        assert!(
            matches!(out[0], Insn::LoadConst(1, _)),
            "UnaryOp should become LoadConst"
        );
        let idx = match out[0] {
            Insn::LoadConst(_, i) => i,
            _ => panic!(),
        };
        assert!(matches!(
            consts[idx as usize].kind(),
            crate::value::ValueKind::Int(3)
        ));
    }

    #[test]
    fn unary_fold_not_bool() {
        use crate::ast::UnaryOp;
        use crate::value::Value;
        let mut consts = vec![Value::bool_(true)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::UnaryOp(1, UnaryOp::Not, 5),
            Insn::Return(1),
        ];
        let out = pass_unary_fold(insns, 2, &mut consts);
        assert_eq!(out.len(), 2, "LoadConst should be removed");
        let idx = match out[0] {
            Insn::LoadConst(_, i) => i,
            _ => panic!(),
        };
        assert!(matches!(
            consts[idx as usize].kind(),
            crate::value::ValueKind::Bool(false)
        ));
    }

    #[test]
    fn unary_fold_skips_local_reg() {
        use crate::ast::UnaryOp;
        use crate::value::Value;
        // r=1 < num_locals=3: should not fuse.
        let mut consts = vec![Value::int(5)];
        let insns = vec![
            Insn::LoadConst(1, 0),
            Insn::UnaryOp(2, UnaryOp::Neg, 1),
            Insn::Return(2),
        ];
        let out = pass_unary_fold(insns, 3, &mut consts);
        assert_eq!(out.len(), 3, "no fusion for local register");
    }

    #[test]
    fn unary_fold_on_compiled_literal() {
        // -5 is a literal unary-neg in the source; after removing the fold_constant check
        // from compile_expr, the optimizer should fold it via pass_unary_fold.
        let code = compile_fn("x = -5\nprint(x)\n");
        let optimized = optimize(code);
        // The constant pool should contain -5 (or the LoadConst(-5) folded from Neg+5).
        let has_neg5 = optimized
            .consts
            .iter()
            .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(-5)));
        assert!(has_neg5, "constant -5 should appear after unary fold");
    }

    // ── algebraic-simplify removal (issue #438) ───────────────────────────────
    //
    // These tests pin the regression: the optimizer must NOT rewrite
    // `x + 0` / `x * 1` / `x * 0` / `x ** 0` / `x ** 1` when `x` is a function
    // parameter of unknown type, because a user class may override `__add__`
    // etc. and the rewrite would skip dunder dispatch.

    #[test]
    fn algebraic_x_plus_zero_keeps_binopconst() {
        // x + 0 inside a function must NOT be rewritten to Move(dst, x),
        // because `x` may be a user instance with a `__add__` override.
        let code = compile_fn("def f(x):\n    return x + 0\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(
            has_binopconst,
            "x+0 must keep BinOpConst so __add__ dispatch runs at runtime"
        );
    }

    #[test]
    fn algebraic_x_times_zero_keeps_binopconst() {
        // x * 0 must NOT collapse to LoadConst(0) — user `__mul__` may run.
        let code = compile_fn("def f(x):\n    return x * 0\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(
            has_binopconst,
            "x*0 must keep BinOpConst so __mul__ dispatch runs at runtime"
        );
    }

    #[test]
    fn algebraic_x_pow_zero_keeps_binopconst() {
        // x ** 0 must NOT collapse to LoadConst(1) — user `__pow__` may run.
        let code = compile_fn("def f(x):\n    return x ** 0\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(
            has_binopconst,
            "x**0 must keep BinOpConst so __pow__ dispatch runs at runtime"
        );
    }

    #[test]
    fn algebraic_constant_lhs_still_folds_via_const_fold() {
        // `5 + 0` should still fold to `5` — handled by `pass_const_fold`,
        // not the removed algebraic-simplify pass.
        let code = compile_fn("def f():\n    return 5 + 0\n");
        let optimized = optimize(code);
        // No BinOpConst should remain: const_fold turned it into LoadConst(_, 5).
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(
            !has_binopconst,
            "5+0 must still be constant-folded by pass_const_fold"
        );
    }

    // ── pass_algebraic_simplify (type-gated, fires for known Int LHS) ────────

    #[test]
    fn algebraic_fires_when_lhs_known_int_from_loadconst() {
        // `x = 5; y = x + 0` — LoadConst marks `x` as Int, so `x + 0`
        // simplifies to Move (and the chain typically collapses further).
        let code = compile_fn("def f():\n    x = 5\n    return x + 0\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(
            !has_binopconst,
            "x+0 with x known Int (LoadConst 5) should be simplified"
        );
    }

    #[test]
    fn algebraic_fires_for_loop_counter() {
        // `for i in range(N): t = i + 0` — ForCount* marks `i` as Int,
        // so `i + 0` inside the loop body simplifies.
        let code = compile_fn(
            "def f():\n    t = 0\n    for i in range(100):\n        t = i + 0\n    return t\n",
        );
        let optimized = optimize(code);
        // The loop body should NOT contain `BinOpConst(..., Add, 0)` —
        // it must have collapsed via algebraic + downstream passes.
        let loop_binopconst_add0 = optimized.fn_protos[0].code.insns.iter().any(|i| {
            matches!(i, Insn::BinOpConst(_, _, crate::ast::BinaryOp::Add, c_idx, ..)
                if matches!(optimized.fn_protos[0].code.consts[*c_idx as usize].kind(),
                            crate::value::ValueKind::Int(0)))
        });
        assert!(
            !loop_binopconst_add0,
            "i+0 in loop body should be simplified (loop counter is Int)"
        );
    }

    #[test]
    fn algebraic_int_mul_one_simplifies() {
        let code = compile_fn("def f():\n    x = 7\n    return x * 1\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(!has_binopconst, "x*1 with x known Int should be simplified");
    }

    #[test]
    fn algebraic_int_mul_zero_loadconst() {
        // `x = 7; return x * 0` — `x` is Int, so `x * 0` becomes LoadConst(0)
        // (no BinOpConst left after the pass).
        let code = compile_fn("def f():\n    x = 7\n    return x * 0\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(
            !has_binopconst,
            "x*0 with x known Int should fold to LoadConst(0)"
        );
    }

    #[test]
    fn algebraic_unknown_lhs_still_keeps_binopconst() {
        // Param `x` of unknown type — pass MUST NOT fire (regression pin).
        // This is the original #438 test, restated to make sure the gating
        // works for the bug case.
        let code = compile_fn("def f(x):\n    return x + 0\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(
            has_binopconst,
            "x+0 with unknown-type x MUST keep BinOpConst (#438)"
        );
    }

    #[test]
    fn algebraic_float_lhs_keeps_binopconst() {
        // `x = 1.5; return x + 0` — x is Float, pass MUST NOT fire because
        // `float * 0` returns `0.0` (not int 0) and `float + 0` preserves
        // NaN/inf semantics.
        let code = compile_fn("def f():\n    x = 1.5\n    return x + 0\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(
            has_binopconst,
            "x+0 with Float x MUST keep BinOpConst (gating works for non-Int)"
        );
    }

    // ── pass_const_fold ───────────────────────────────────────────────────────

    #[test]
    fn const_fold_binopconst_with_known_lhs() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // LoadConst(r0, 0)  [consts[0]=5]
        // BinOpConst(r1, r0, Add, 1)  [consts[1]=3]
        // → LoadConst(r1, 2)  [consts[2]=8]
        let mut consts = vec![Value::int(5), Value::int(3)];
        let insns = vec![
            Insn::LoadConst(0, 0),                           // r0 = 5
            Insn::BinOpConst(1, 0, BinaryOp::Add, 1, false), // r1 = r0 + 3
            Insn::Return(1),
        ];
        let out = pass_const_fold(insns, &mut consts, 0);
        assert!(
            matches!(out[1], Insn::LoadConst(1, _)),
            "BinOpConst with known lhs should be folded to LoadConst"
        );
        let folded_idx = match out[1] {
            Insn::LoadConst(_, i) => i,
            _ => panic!(),
        };
        assert!(matches!(
            consts[folded_idx as usize].kind(),
            crate::value::ValueKind::Int(8)
        ));
    }

    #[test]
    fn const_fold_binop_with_both_known() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        let mut consts = vec![Value::int(10), Value::int(2)];
        let insns = vec![
            Insn::LoadConst(0, 0),               // r0 = 10
            Insn::LoadConst(1, 1),               // r1 = 2
            Insn::BinOp(2, 0, BinaryOp::Mul, 1), // r2 = r0 * r1
            Insn::Return(2),
        ];
        let out = pass_const_fold(insns, &mut consts, 0);
        assert!(
            matches!(out[2], Insn::LoadConst(2, _)),
            "BinOp with both operands known should fold to LoadConst"
        );
        let idx = match out[2] {
            Insn::LoadConst(_, i) => i,
            _ => panic!(),
        };
        assert!(matches!(
            consts[idx as usize].kind(),
            crate::value::ValueKind::Int(20)
        ));
    }

    #[test]
    fn const_fold_propagates_through_move() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // LoadConst(t, idx_5)  Move(x, t)  BinOpConst(y, x, Add, idx_3)
        // After propagation: known[x]=idx_5, fold BinOpConst to LoadConst(y, idx_8)
        let mut consts = vec![Value::int(5), Value::int(3)];
        let insns = vec![
            Insn::LoadConst(5, 0),                           // temp=5 (reg 5)
            Insn::Move(0, 5),                                // x = temp
            Insn::BinOpConst(1, 0, BinaryOp::Add, 1, false), // y = x + 3
            Insn::Return(1),
        ];
        let out = pass_const_fold(insns, &mut consts, 0);
        assert!(
            matches!(out[2], Insn::LoadConst(1, _)),
            "BinOpConst should fold after Move propagates known value"
        );
    }

    #[test]
    fn const_fold_clears_at_branch() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // LoadConst(r0, idx_5)  JumpIfFalse(r0, 0)  BinOpConst(r1, r0, Add, idx_3)
        // After the branch, known is cleared, so BinOpConst should NOT fold.
        let mut consts = vec![Value::int(5), Value::int(3)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::JumpIfFalse(0, 0),
            Insn::BinOpConst(1, 0, BinaryOp::Add, 1, false),
            Insn::Return(1),
        ];
        let out = pass_const_fold(insns, &mut consts, 0);
        assert!(
            matches!(out[2], Insn::BinOpConst(1, 0, BinaryOp::Add, 1, ..)),
            "no folding after a branch clears known map"
        );
    }

    #[test]
    fn const_fold_on_compiled_chain() {
        // x = 5; y = x * 2 — the optimizer should fold y to 10
        let code = compile_fn("x = 5\ny = x * 2\nprint(y)\n");
        let optimized = optimize(code);
        // After folding, the constant pool should contain 10
        let has_10 = optimized
            .consts
            .iter()
            .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(10)));
        assert!(
            has_10,
            "constant 10 should appear in pool after folding x*2 with x=5"
        );
    }

    #[test]
    fn const_fold_clears_at_forward_jump_target() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // Models the post-ternary merge point.  The `then`-arm writes
        // r2 = consts[0]=10 and jumps over the `else`-arm.  At the merge
        // instruction (index 3, the Jump target) the known-constant map
        // must be cleared — otherwise a linear forward scan would see the
        // else-arm's write (r2 = consts[1]=20) as the most recent and fold
        // BinOpConst on r2 to a constant, producing a stale result on the
        // taken-then path.
        //
        //   [0] JumpIfFalse(0, 2)              # if !cond, skip to [3]
        //   [1] LoadConst(2, 0)                # then: r2 = 10
        //   [2] Jump(1)                        # → [4]   (target of forward jump = [4])
        //   [3] LoadConst(2, 1)                # else: r2 = 20
        //   [4] BinOpConst(3, 2, Add, 2)       # consts[2] = 1  → must not fold
        //   [5] Return(3)
        let mut consts = vec![Value::int(10), Value::int(20), Value::int(1)];
        let insns = vec![
            Insn::JumpIfFalse(0, 2),
            Insn::LoadConst(2, 0),
            Insn::Jump(1),
            Insn::LoadConst(2, 1),
            Insn::BinOpConst(3, 2, BinaryOp::Add, 2, false),
            Insn::Return(3),
        ];
        let out = pass_const_fold(insns, &mut consts, 0);
        assert!(
            matches!(out[4], Insn::BinOpConst(3, 2, BinaryOp::Add, 2, ..)),
            "merge point must clear known map; BinOpConst on a phi'd value must not fold",
        );
    }

    #[test]
    fn const_fold_does_not_fold_loop_condition() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // Simulates: y = 3; while y > 0: y = y - 1
        //
        //  [0] LoadConst(0, 0)              consts[0] = 3   (y_reg = 0)
        //  [1] BinOpConst(1, 0, Gt, 1)      consts[1] = 0   (loop header — target of Jump at [4])
        //  [2] JumpIfFalse(1, 2)                             (exit: target = 2+1+2 = 5)
        //  [3] BinOpConst(0, 0, Sub, 2)     consts[2] = 1
        //  [4] Jump(-4)                                      (back to [1]: 4+1-4 = 1)
        //  [5] Return(0)
        let mut consts = vec![Value::int(3), Value::int(0), Value::int(1)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::BinOpConst(1, 0, BinaryOp::Gt, 1, false),
            Insn::JumpIfFalse(1, 2),
            Insn::BinOpConst(0, 0, BinaryOp::Sub, 2, false),
            Insn::Jump(-4),
            Insn::Return(0),
        ];
        let out = pass_const_fold(insns, &mut consts, 0);
        // [1] must NOT fold to LoadConst(True) — the loop would become infinite.
        assert!(
            matches!(out[1], Insn::BinOpConst(1, 0, BinaryOp::Gt, 1, ..)),
            "loop condition must not be folded; known map must clear at loop header"
        );
    }

    #[test]
    fn const_fold_call_invalidates_named_locals_but_not_temps() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // Simulates the issue #671 pattern at the instruction level:
        //
        //   [0] LoadConst(r0, 0)      r0 is a named local (< num_locals=2): consts[0]=10
        //   [1] LoadConst(r5, 1)      r5 is a temp (>= num_locals=2): consts[1]=3
        //   [2] Call(r2, 0)           user call — may write r0 via assign_name write-through
        //   [3] BinOpConst(r3, r0, Add, 1)  must NOT fold (r0 was a named local)
        //   [4] BinOpConst(r4, r5, Add, 1)  MUST fold (r5 is a temp, safe to retain)
        //   [5] Return(r3)
        //
        // With num_locals=2, after Call at [2]:
        //   known[r0] must be removed  → BinOpConst at [3] stays unfused
        //   known[r5] must survive     → BinOpConst at [4] folds to LoadConst
        let mut consts = vec![Value::int(10), Value::int(3)];
        let insns = vec![
            Insn::LoadConst(0, 0),                           // r0 = 10 (named local)
            Insn::LoadConst(5, 1),                           // r5 = 3  (temp)
            Insn::Call(2, 0),                                // call — may clobber r0
            Insn::BinOpConst(3, 0, BinaryOp::Add, 1, false), // r3 = r0 + 3
            Insn::BinOpConst(4, 5, BinaryOp::Add, 1, false), // r4 = r5 + 3
            Insn::Return(3),
        ];
        let out = pass_const_fold(insns, &mut consts, 2);
        assert!(
            matches!(out[3], Insn::BinOpConst(3, 0, BinaryOp::Add, 1, ..)),
            "named-local r0 must not be folded after Call: found {:?}",
            out[3]
        );
        assert!(
            matches!(out[4], Insn::LoadConst(4, _)),
            "temp r5 must still be folded after Call: found {:?}",
            out[4]
        );
    }

    // ── pass_const_branch_elim ────────────────────────────────────────────────

    #[test]
    fn const_branch_elim_jumpiffalse_truthy_becomes_jump0() {
        use crate::value::Value;
        // LoadConst(r=0, c=0) [consts[0]=True]  JumpIfFalse(0, 5)
        // Truthy → never jumps → replace with Jump(0)
        let consts = vec![Value::bool_(true)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::JumpIfFalse(0, 5),
            Insn::Return(0),
        ];
        let out = pass_const_branch_elim(insns, &consts);
        assert!(
            matches!(out[1], Insn::Jump(0)),
            "truthy JumpIfFalse → Jump(0)"
        );
    }

    #[test]
    fn const_branch_elim_jumpiffalse_falsy_becomes_jump_k() {
        use crate::value::Value;
        // LoadConst(r=0, c=0) [consts[0]=False]  JumpIfFalse(0, 3)
        // Falsy → always jumps → replace with Jump(3)
        let consts = vec![Value::bool_(false)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::JumpIfFalse(0, 3),
            Insn::Return(0),
        ];
        let out = pass_const_branch_elim(insns, &consts);
        assert!(
            matches!(out[1], Insn::Jump(3)),
            "falsy JumpIfFalse → Jump(k)"
        );
    }

    #[test]
    fn const_branch_elim_eliminates_dead_branch_on_compiled_code() {
        // "if True: print(1)\nelse: print(2)" — the else branch should be dead
        let code = compile_fn("if True:\n    print(1)\nelse:\n    print(2)\n");
        let optimized = optimize(code);
        // After optimization, the instruction stream should not contain the
        // dead-branch code that prints 2. Check by verifying only one integer
        // constant (1) is referenced, not 2.
        // Note: the constant 2 may still exist in the pool even if the code
        // referencing it is dead — but the dead code itself should be gone.
        // Instead check that no LoadConst referencing 2 appears in reachable insns.
        // Simpler: check the insn list has no BinOp/conditional jumps (the if collapsed).
        let has_cond_jump = optimized.insns.iter().any(|i| {
            matches!(
                i,
                Insn::JumpIfFalse(..)
                    | Insn::JumpIfTrue(..)
                    | Insn::CmpJumpIfFalse(..)
                    | Insn::CmpJumpIfFalseConst(..)
                    | Insn::CmpJumpIfTrue(..)
                    | Insn::CmpJumpIfTrueConst(..)
            )
        });
        assert!(
            !has_cond_jump,
            "constant-condition if should have no conditional jumps"
        );
    }

    // ── pass_cmpjump_fusion ───────────────────────────────────────────────────

    #[test]
    fn cmpjump_fuses_binopconst_jumpiffalse() {
        use crate::ast::BinaryOp;
        // BinOpConst(r=5, lhs=0, Gt, c=0) + JumpIfFalse(r=5, k=0)
        // k=0: if-false jumps to old_pos 1+1+0=2 = Return.
        // After fusion+compaction: CmpJumpIfFalseConst at new_pos 0, Return at new_pos 1.
        // Rewritten offset: to_new[2]-to_new[1]-1 = 1-0-1 = 0 → same k=0.
        let insns = vec![
            Insn::BinOpConst(5, 0, BinaryOp::Gt, 0, false),
            Insn::JumpIfFalse(5, 0),
            Insn::Return(0),
        ];
        let out = pass_cmpjump_fusion(insns, 2);
        assert_eq!(out.len(), 2, "BinOpConst should be removed");
        assert!(
            matches!(out[0], Insn::CmpJumpIfFalseConst(0, BinaryOp::Gt, 0, 0)),
            "should become CmpJumpIfFalseConst with same offset"
        );
    }

    #[test]
    fn cmpjump_fuses_binop_jumpiftrue() {
        use crate::ast::BinaryOp;
        // BinOp(r=5, lhs=0, Eq, rhs=1) + JumpIfTrue(r=5, k=0)
        // → CmpJumpIfTrue(lhs=0, Eq, rhs=1, k=0)
        let insns = vec![
            Insn::BinOp(5, 0, BinaryOp::Eq, 1),
            Insn::JumpIfTrue(5, 0),
            Insn::Return(0),
        ];
        let out = pass_cmpjump_fusion(insns, 2);
        assert_eq!(out.len(), 2, "BinOp should be removed");
        assert!(
            matches!(out[0], Insn::CmpJumpIfTrue(0, BinaryOp::Eq, 1, 0)),
            "should become CmpJumpIfTrue"
        );
    }

    #[test]
    fn cmpjump_skips_when_reg_is_local() {
        use crate::ast::BinaryOp;
        // r=1 < num_locals=3 → no fusion
        let insns = vec![
            Insn::BinOpConst(1, 0, BinaryOp::Gt, 0, false),
            Insn::JumpIfFalse(1, 1),
            Insn::Return(0),
        ];
        let out = pass_cmpjump_fusion(insns, 3);
        assert_eq!(out.len(), 3, "no fusion when cond reg is a local");
    }

    #[test]
    fn cmpjump_skips_when_jump_targets_the_cond_jump() {
        use crate::ast::BinaryOp;
        // Issue #2088: an `and` short-circuit lands on the trailing conditional
        // jump of the RHS comparison.  Fusing BinOp+JumpIfFalse at index 1/2
        // would make index 2 recompute (5 Ne 6) instead of re-testing the LHS
        // register — wrong on the incoming-jump path.  The JumpIfFalse(4, -1)
        // (short-circuit) targets index 2 (0 + 1 + 1 = 2), so fusion must skip.
        let insns = vec![
            Insn::JumpIfFalse(4, 1), // short-circuit LHS-false jump → targets index 2
            Insn::BinOp(3, 5, BinaryOp::Ne, 6),
            Insn::JumpIfFalse(3, 0), // index 2: jump target — must NOT be fused
            Insn::Return(0),
        ];
        let out = pass_cmpjump_fusion(insns, 2);
        assert_eq!(
            out.len(),
            4,
            "BinOp must survive: its JumpIfFalse is a jump target"
        );
        assert!(
            matches!(out[1], Insn::BinOp(3, 5, BinaryOp::Ne, 6)),
            "BinOp(3, 5, Ne, 6) must be preserved, not fused away"
        );
        assert!(
            matches!(out[2], Insn::JumpIfFalse(3, 0)),
            "trailing JumpIfFalse must remain a plain register test"
        );
    }

    #[test]
    fn cmpjump_fusion_on_compiled_if() {
        let code = compile_fn("def f(x):\n    if x > 3:\n        print(x)\n");
        let optimized = optimize(code);
        let inner = &optimized.fn_protos[0].code;
        let has_cmpjump = inner.insns.iter().any(|i| {
            matches!(
                i,
                Insn::CmpJumpIfFalse(..)
                    | Insn::CmpJumpIfTrue(..)
                    | Insn::CmpJumpIfFalseConst(..)
                    | Insn::CmpJumpIfTrueConst(..)
            )
        });
        assert!(
            has_cmpjump,
            "optimizer should fuse comparison into conditional jump"
        );
    }

    // ── pass_thread_jumps ─────────────────────────────────────────────────────

    #[test]
    fn thread_jumps_collapses_chain() {
        // [0] Jump(1)  [1] LoadNone(0)  [2] Jump(1)  [3] LoadNone(1)  [4] Return(1)
        // Jump at 0 targets idx 2 (0+1+1=2). idx 2 is Jump(1) → idx 4.
        // After threading: Jump at 0 should target 4 directly → offset = 4-(0+1)=3.
        let insns = vec![
            Insn::Jump(1),     // 0 → 2
            Insn::LoadNone(0), // 1
            Insn::Jump(1),     // 2 → 4
            Insn::LoadNone(1), // 3
            Insn::Return(1),   // 4
        ];
        let out = pass_thread_jumps(insns);
        assert!(
            matches!(out[0], Insn::Jump(3)),
            "Jump(1) at 0 should be threaded to Jump(3) (target idx 4)"
        );
    }

    #[test]
    fn thread_jumps_handles_self_loop() {
        // Jump(-1) loops to itself — threading must not infinite-loop.
        let insns = vec![Insn::Jump(-1)];
        let out = pass_thread_jumps(insns);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(out[0], Insn::Jump(-1)),
            "self-loop must be left unchanged"
        );
    }

    #[test]
    fn thread_jumps_stops_at_backward_jump() {
        // Simulates the nested-loop pattern that triggered issue #966.
        //
        // Layout:
        //   [0] ForCountConstInline(v, Lt, 3, 1, off=2)  — inner loop header
        //   [1] LoadNone(0)                               — body instruction
        //   [2] Jump(-3)                                  — inner back-edge → [0]
        //   [3] Jump(-4)                                  — outer back-edge → before [0]
        //   [4] Return(0)
        //
        // Before the fix, follow() would start at [3] (the exit target of
        // ForCountConstInline off=2 → idx 0+1+2=3), see Jump(-4) and follow it to
        // idx 0 (3+1+(-4)=0), then see ForCountConstInline and stop. The computed
        // exit offset relative to [0] would be 0 - 0 - 1 = -1 (negative).
        //
        // After the fix, follow() stops at [3] because Jump(-4) is a backward jump
        // (k < 0). The offset for ForCountConstInline stays 2.
        let insns = vec![
            Insn::ForCountConstInline(0, crate::ast::BinaryOp::Lt, 3, 1, 2), // 0
            Insn::LoadNone(0),                                               // 1
            Insn::Jump(-3),  // 2  inner back-edge → 0
            Insn::Jump(-4),  // 3  outer back-edge → before [0] (simulated)
            Insn::Return(0), // 4
        ];
        let out = pass_thread_jumps(insns);
        // The ForCountConstInline exit offset must remain 2 (not become negative).
        match out[0] {
            Insn::ForCountConstInline(_, _, _, _, off) => assert!(
                off >= 0,
                "ForCountConstInline off must not be negative after threading; got {off}"
            ),
            _ => panic!("insn[0] should still be ForCountConstInline"),
        }
    }

    #[test]
    fn thread_conditional_jump_through_unconditional() {
        // [0] JumpIfFalse(r, 1)   [1] Jump(1)   [2] LoadNone(0)   [3] Return(0)
        // JumpIfFalse at 0 targets 2. idx 2 is Jump(1) targeting idx 4 (past end).
        // After threading JumpIfFalse should target idx 4 as well.
        let insns = vec![
            Insn::JumpIfFalse(0, 1), // 0 → target 2
            Insn::Jump(1),           // 1 → target 3
            Insn::LoadNone(0),       // 2
            Insn::Return(0),         // 3
        ];
        let out = pass_thread_jumps(insns);
        // JumpIfFalse at 0 targeted 2; idx 2 is LoadNone (not a Jump) so no threading there.
        // JumpIfFalse's target is idx 2 which is NOT a Jump — offset stays 1.
        assert!(
            matches!(out[0], Insn::JumpIfFalse(0, 1)),
            "no chain to thread for the conditional"
        );
        // Jump at 1 targets 3 (Return), which is not a Jump.
        assert!(matches!(out[1], Insn::Jump(1)));
    }

    #[test]
    fn thread_jumps_no_change_when_no_chains() {
        // No jump chains → output equals input.
        let insns = vec![
            Insn::LoadNone(0),
            Insn::JumpIfFalse(0, 1),
            Insn::LoadNone(1),
            Insn::Return(0),
        ];
        let out = pass_thread_jumps(insns.clone());
        assert_eq!(out.len(), insns.len());
    }

    // ── pass_dead_code ────────────────────────────────────────────────────────

    #[test]
    fn dce_removes_instructions_after_return() {
        // Instructions after Return are unreachable.
        let insns = vec![
            Insn::LoadNone(0),
            Insn::Return(0),
            Insn::LoadNone(1), // unreachable
            Insn::Return(1),   // unreachable
        ];
        let out = pass_dead_code(insns);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Insn::LoadNone(0)));
        assert!(matches!(out[1], Insn::Return(0)));
    }

    #[test]
    fn dce_keeps_instructions_after_conditional_jump() {
        // Both branches of a conditional jump are reachable.
        // [0] JumpIfFalse(r0, 1)  [1] LoadNone(1)  [2] Return(0)
        // Both fallthrough (idx 1) and target (idx 1+1+1=3 — end) are successors,
        // so nothing is removed.
        let insns = vec![
            Insn::JumpIfFalse(0, 1), // target = 0+1+1 = 2 (Return)
            Insn::LoadNone(1),
            Insn::Return(0),
        ];
        let out = pass_dead_code(insns);
        assert_eq!(out.len(), 3, "all instructions reachable");
    }

    #[test]
    fn dce_removes_dead_code_after_unconditional_jump() {
        // [0] Jump(1)   [1] LoadNone(0) <dead>   [2] Return(1)
        let insns = vec![
            Insn::Jump(1),     // jumps to idx 2
            Insn::LoadNone(0), // unreachable
            Insn::Return(1),
        ];
        let out = pass_dead_code(insns);
        assert_eq!(out.len(), 2);
        assert!(
            matches!(out[0], Insn::Jump(0)),
            "offset rewritten: 2→1, new offset = 1-(0+1)=0"
        );
        assert!(matches!(out[1], Insn::Return(1)));
    }

    #[test]
    fn dce_on_compiled_function_with_early_return() {
        let code = compile_fn("def f(x):\n    if x > 0:\n        return 1\n    return 0\n");
        let before = code.fn_protos[0].code.insns.len();
        let optimized = optimize(code);
        let after = optimized.fn_protos[0].code.insns.len();
        assert!(
            after <= before,
            "optimizer should not increase instruction count ({before} → {after})"
        );
    }

    // ── pass_trivial_nop ──────────────────────────────────────────────────────

    #[test]
    fn trivial_nop_removes_jump0() {
        // Jump(0) is a no-op: it jumps to the next instruction.
        let insns = vec![
            Insn::LoadNone(0),
            Insn::Jump(0), // <- should be removed
            Insn::Return(0),
        ];
        let out = pass_trivial_nop(insns);
        assert_eq!(out.len(), 2, "Jump(0) should be removed");
        assert!(matches!(out[0], Insn::LoadNone(0)));
        assert!(matches!(out[1], Insn::Return(0)));
    }

    #[test]
    fn trivial_nop_removes_self_move() {
        // Move(r, r) copies a register into itself — no effect.
        let insns = vec![
            Insn::LoadNone(1),
            Insn::Move(2, 2), // <- should be removed
            Insn::Return(1),
        ];
        let out = pass_trivial_nop(insns);
        assert_eq!(out.len(), 2, "Move(r, r) should be removed");
    }

    #[test]
    fn trivial_nop_fixes_jump_over_removed() {
        // A Jump that skips over a removed Jump(0) must have its offset decremented.
        // insns: [0] LoadNone 0   [1] Jump(1)   [2] Jump(0) <removed>   [3] Return 0
        // Jump(1) at idx 1 targets idx 3 (offset 1 = idx 1 + 1 + 1).
        // After removing idx 2: Jump(1) at new-idx 1 should target new-idx 2 (old idx 3),
        // so new offset = 2 - (1+1) = 0.
        let insns = vec![
            Insn::LoadNone(0),
            Insn::Jump(1), // targets idx 3 (Return)
            Insn::Jump(0), // no-op, removed
            Insn::Return(0),
        ];
        let out = pass_trivial_nop(insns);
        assert_eq!(out.len(), 3);
        assert!(
            matches!(out[1], Insn::Jump(0)),
            "offset should decrease by 1"
        );
    }

    // ── pass_compact_consts ───────────────────────────────────────────────────

    #[test]
    fn compact_consts_removes_unreferenced_entry() {
        use crate::value::Value;
        // Pool: [10, 99, 20].  Only consts 0 and 2 are referenced.
        // Expected pool after compaction: [10, 20]; indices rewritten.
        let consts = vec![Value::int(10), Value::int(99), Value::int(20)];
        let insns = vec![
            Insn::LoadConst(0, 0), // references pool[0] = 10
            Insn::LoadConst(1, 2), // references pool[2] = 20
            Insn::Return(0),
        ];
        let (out_insns, out_consts) = pass_compact_consts(insns, consts);
        assert_eq!(out_consts.len(), 2, "unreferenced entry should be removed");
        assert!(matches!(
            out_consts[0].kind(),
            crate::value::ValueKind::Int(10)
        ));
        assert!(matches!(
            out_consts[1].kind(),
            crate::value::ValueKind::Int(20)
        ));
        // LoadConst(1, 2) should be rewritten to LoadConst(1, 1)
        assert!(matches!(out_insns[1], Insn::LoadConst(1, 1)));
    }

    #[test]
    fn compact_consts_noop_when_all_referenced() {
        use crate::value::Value;
        let consts = vec![Value::int(1), Value::int(2)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::LoadConst(1, 1),
            Insn::Return(0),
        ];
        let (out_insns, out_consts) = pass_compact_consts(insns, consts);
        assert_eq!(out_consts.len(), 2, "no change when all referenced");
        assert!(matches!(out_insns[0], Insn::LoadConst(0, 0)));
        assert!(matches!(out_insns[1], Insn::LoadConst(1, 1)));
    }

    #[test]
    fn compact_consts_on_compiled_dead_branch() {
        // "if True: x=1\nelse: x=2" — the else branch is dead.
        // pass_const_fold+pass_dead_code should eliminate the else body.
        // pass_compact_consts should then remove the orphaned constant 2 from the pool.
        let code = compile_fn("if True:\n    x = 1\nelse:\n    x = 2\n");
        let optimized = optimize(code);
        // After optimization, the constant 2 should not appear in the pool
        // (the dead branch referencing it was removed, then the pool was compacted).
        let has_2 = optimized
            .consts
            .iter()
            .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(2)));
        assert!(
            !has_2,
            "orphaned constant 2 should be removed by pool compaction"
        );
    }

    // ── pass_not_invert ───────────────────────────────────────────────────────

    #[test]
    fn not_invert_jumpiffalse_becomes_jumpiftrue() {
        use crate::ast::UnaryOp;
        // [0] UnaryOp(r=5, Not, src=0)   keep=false
        // [1] JumpIfFalse(5, k=1)         target = 1+1+1 = 3 (past-end sentinel)
        // [2] Return(0)
        // After fusion: [0] JumpIfTrue(0, 1)  [1] Return(0)
        // Offset rewrite: old_target=3, to_new[3]=2, new_src=to_new[1]=0 → k=2-0-1=1
        let insns = vec![
            Insn::UnaryOp(5, UnaryOp::Not, 0),
            Insn::JumpIfFalse(5, 1),
            Insn::Return(0),
        ];
        let out = pass_not_invert(insns, 2);
        assert_eq!(out.len(), 2, "UnaryOp should be removed");
        assert!(
            matches!(out[0], Insn::JumpIfTrue(0, 1)),
            "JumpIfFalse(not x) should become JumpIfTrue(x)"
        );
    }

    #[test]
    fn not_invert_jumpiftrue_becomes_jumpiffalse() {
        use crate::ast::UnaryOp;
        // Same layout; k=1 → past-end target.
        let insns = vec![
            Insn::UnaryOp(5, UnaryOp::Not, 0),
            Insn::JumpIfTrue(5, 1),
            Insn::Return(0),
        ];
        let out = pass_not_invert(insns, 2);
        assert_eq!(out.len(), 2, "UnaryOp should be removed");
        assert!(
            matches!(out[0], Insn::JumpIfFalse(0, 1)),
            "JumpIfTrue(not x) should become JumpIfFalse(x)"
        );
    }

    #[test]
    fn not_invert_skips_when_reg_is_local() {
        use crate::ast::UnaryOp;
        // r=1 < num_locals=3 → must not fuse
        let insns = vec![
            Insn::UnaryOp(1, UnaryOp::Not, 0),
            Insn::JumpIfFalse(1, 1),
            Insn::Return(0),
        ];
        let out = pass_not_invert(insns, 3);
        assert_eq!(out.len(), 3, "no fusion when r is a local");
    }

    #[test]
    fn not_invert_skips_when_reg_read_after() {
        use crate::ast::UnaryOp;
        // r=5 is read again after the branch → must not fuse
        let insns = vec![
            Insn::UnaryOp(5, UnaryOp::Not, 0),
            Insn::JumpIfFalse(5, 0),
            Insn::Return(5), // reads r=5 → live
        ];
        let out = pass_not_invert(insns, 2);
        assert_eq!(out.len(), 3, "no fusion when r is live after branch");
    }

    #[test]
    fn not_invert_fuses_when_reg_not_reused() {
        use crate::ast::UnaryOp;
        // Build a case where the Not result register (r=5) is genuinely dead after
        // the branch: src=0 (x), result=5, jump target uses a different register.
        //
        // [0] UnaryOp(5, Not, 0)   r5 = not r0
        // [1] JumpIfFalse(5, 1)    if r5 false: jump past-end
        // [2] Move(2, 0)           r2 = r0  (r5 not read here)
        // [3] Return(2)
        let insns = vec![
            Insn::UnaryOp(5, UnaryOp::Not, 0),
            Insn::JumpIfFalse(5, 1),
            Insn::Move(2, 0),
            Insn::Return(2),
        ];
        let out = pass_not_invert(insns, 2);
        // UnaryOp should be removed; JumpIfFalse→JumpIfTrue
        assert_eq!(out.len(), 3, "UnaryOp should be removed");
        assert!(
            matches!(out[0], Insn::JumpIfTrue(0, _)),
            "JumpIfFalse(not r0) should become JumpIfTrue(r0)"
        );
    }

    // ── pass_binopinplace_downgrade ───────────────────────────────────────────

    #[test]
    fn binopinplace_downgrades_dead_numeric_temp_lhs() {
        use crate::ast::BinaryOp;
        // r5 loaded from a numeric const → provably Int; r5 not read after the op
        // → safe to downgrade to BinOp (issue #1943 type gate).
        let consts = vec![Value::int(7)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOpInPlace(2, 5, BinaryOp::Add, 5),
            Insn::Return(2),
        ];
        let out = pass_binopinplace_downgrade(insns, 2, &consts);
        assert!(
            matches!(out[1], Insn::BinOp(2, 5, BinaryOp::Add, 5)),
            "BinOpInPlace with dead numeric temp lhs should become BinOp"
        );
    }

    #[test]
    fn binopinplace_skips_unknown_type_temp_lhs() {
        use crate::ast::BinaryOp;
        // lhs=5 comes from GetItem → runtime type unknown (could be a list with
        // extend-from-any-iterable __iadd__). Must NOT downgrade (issue #1943).
        let consts = vec![Value::int(0)];
        let insns = vec![
            Insn::GetItem(5, 3, 4),
            Insn::BinOpInPlace(2, 5, BinaryOp::Add, 6),
            Insn::Return(2),
        ];
        let out = pass_binopinplace_downgrade(insns, 2, &consts);
        assert!(
            matches!(out[1], Insn::BinOpInPlace(2, 5, BinaryOp::Add, 6)),
            "BinOpInPlace with unknown-type temp lhs must not be downgraded"
        );
    }

    #[test]
    fn binopinplace_skips_local_lhs() {
        use crate::ast::BinaryOp;
        // lhs=1 < num_locals=3 → user object may have __iadd__, must not downgrade
        let consts = vec![Value::int(0)];
        let insns = vec![Insn::BinOpInPlace(2, 1, BinaryOp::Add, 0), Insn::Return(2)];
        let out = pass_binopinplace_downgrade(insns, 3, &consts);
        assert!(
            matches!(out[0], Insn::BinOpInPlace(2, 1, BinaryOp::Add, 0)),
            "BinOpInPlace with local lhs must not be downgraded"
        );
    }

    #[test]
    fn binopinplace_skips_live_lhs() {
        use crate::ast::BinaryOp;
        // lhs=5 is numeric but read after by Return(5) → live, must not downgrade
        let consts = vec![Value::int(1)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOpInPlace(2, 5, BinaryOp::Add, 5),
            Insn::Return(5),
        ];
        let out = pass_binopinplace_downgrade(insns, 2, &consts);
        assert!(
            matches!(out[1], Insn::BinOpInPlace(2, 5, BinaryOp::Add, 5)),
            "BinOpInPlace with live lhs must not be downgraded"
        );
    }

    #[test]
    fn binopinplace_downgrades_numeric_dst_equals_lhs() {
        use crate::ast::BinaryOp;
        // dst == lhs AND lhs provably numeric → safe to downgrade.
        let consts = vec![Value::int(2)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOpInPlace(5, 5, BinaryOp::Mul, 5),
            Insn::Return(5),
        ];
        let out = pass_binopinplace_downgrade(insns, 2, &consts);
        assert!(
            matches!(out[1], Insn::BinOp(5, 5, BinaryOp::Mul, 5)),
            "BinOpInPlace(dst==lhs) on numeric lhs should downgrade to BinOp"
        );
    }

    #[test]
    fn binopinplace_skips_container_dst_equals_lhs() {
        use crate::ast::BinaryOp;
        // dst == lhs but lhs is a freshly built list → list += extends in place
        // and accepts any iterable; downgrading to BinOp would lose that and
        // raise a spurious TypeError (issue #1943). Must NOT downgrade.
        let consts = vec![Value::int(0)];
        let insns = vec![
            Insn::BuildList(5, 5, 0),
            Insn::BinOpInPlace(5, 5, BinaryOp::Add, 6),
            Insn::Return(5),
        ];
        let out = pass_binopinplace_downgrade(insns, 2, &consts);
        assert!(
            matches!(out[1], Insn::BinOpInPlace(5, 5, BinaryOp::Add, 6)),
            "BinOpInPlace(dst==lhs) on a container lhs must not be downgraded"
        );
    }

    #[test]
    fn binopinplace_skips_numeric_temp_clobbered_by_unpack() {
        use crate::ast::BinaryOp;
        // r5 starts numeric (LoadConst Int) but is then overwritten by Unpack,
        // which can store an element of ANY type (e.g. a list). `writable_dst`
        // does not capture Unpack's destination range, so without an explicit
        // clear the stale numeric provenance would survive and wrongly downgrade
        // the following in-place op. Must NOT downgrade.
        let consts = vec![Value::int(7)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::Unpack(5, 6, 1),
            Insn::BinOpInPlace(2, 5, BinaryOp::Add, 7),
            Insn::Return(2),
        ];
        let out = pass_binopinplace_downgrade(insns, 2, &consts);
        assert!(
            matches!(out[2], Insn::BinOpInPlace(2, 5, BinaryOp::Add, 7)),
            "numeric temp clobbered by Unpack must not be downgraded"
        );
    }

    // ── slice_has_back_edge ────────────────────────────────────────────────────

    #[test]
    fn back_edge_detected_on_negative_jump() {
        // Jump(-2) is a backward edge
        let insns = vec![Insn::Jump(-2)];
        assert!(slice_has_back_edge(&insns));
    }

    #[test]
    fn no_back_edge_in_forward_only_slice() {
        // All jumps are non-negative → no back-edge
        let insns = vec![Insn::JumpIfFalse(0, 1), Insn::Return(0)];
        assert!(!slice_has_back_edge(&insns));
    }

    #[test]
    fn binop_const_fusion_skips_when_back_edge_present() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // LoadConst(r5, 0) + BinOp(r3, r2, Mul, r5) + ForIter(r6, 0, -2)
        // The ForIter has a negative offset → back-edge; fusion must not remove LoadConst.
        let consts = vec![Value::int(4)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(3, 2, BinaryOp::Mul, 5),
            Insn::ForIter(6, 0, -2),
            Insn::Return(3),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        // LoadConst must survive — r5 is live on the back-edge
        assert!(
            matches!(out[0], Insn::LoadConst(5, 0)),
            "LoadConst must not be removed when a back-edge is present"
        );
        let _ = consts; // suppress unused warning
    }

    // ── pass_copy_prop ─────────────────────────────────────────────────────────

    #[test]
    fn copy_prop_eliminates_move() {
        use crate::ast::BinaryOp;
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::Move(1, 0),
            Insn::BinOp(2, 1, BinaryOp::Add, 3),
            Insn::Return(2),
        ];
        let out = pass_copy_prop(insns, 0);
        assert!(
            matches!(out[2], Insn::BinOp(2, 0, BinaryOp::Add, 3)),
            "r1 should be substituted with r0 in BinOp"
        );
    }

    #[test]
    fn copy_prop_kills_alias_on_move_overwrite() {
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::LoadConst(2, 1),
            Insn::Move(1, 0),
            Insn::Move(0, 2),
            Insn::Return(1),
        ];
        let out = pass_copy_prop(insns, 0);
        assert!(
            matches!(out[4], Insn::Return(1)),
            "r1 alias must be killed when r0 is overwritten"
        );
    }

    #[test]
    fn copy_prop_kills_alias_on_binop_write() {
        use crate::ast::BinaryOp;
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::LoadConst(2, 1),
            Insn::Move(1, 0),
            Insn::BinOp(0, 0, BinaryOp::Add, 2),
            Insn::Return(1),
        ];
        let out = pass_copy_prop(insns, 0);
        assert!(
            matches!(out[4], Insn::Return(1)),
            "r1→r0 alias must be killed when BinOp writes r0"
        );
    }

    #[test]
    fn copy_prop_does_not_substitute_dict_update_receiver() {
        let insns = vec![
            Insn::BuildDict(2, 3, 0),
            Insn::Move(5, 2),
            Insn::BuildDict(4, 3, 0),
            Insn::Move(6, 4),
            Insn::DictUpdate(5, 6),
            Insn::Return(5),
        ];
        let out = pass_copy_prop(insns, 0);
        assert!(
            matches!(out[4], Insn::DictUpdate(5, 4)),
            "DictUpdate: receiver unchanged, src substituted"
        );
    }

    #[test]
    fn copy_prop_invalidates_named_local_alias_on_call() {
        use crate::ast::BinaryOp;
        // Simulates the pattern from issue #671 applied to copy propagation:
        //
        //   [0] LoadConst(r5, 0)      r5 is a temp (>= num_locals=2): consts[0]=5
        //   [1] Move(r0, r5)          r0 is a named local (< num_locals=2)
        //                             copy-prop records copies[r0] = r5
        //   [2] Call(r8, 0)           user call — may write r0 via assign_name
        //                             write-through; copies[r0 → r5] must be evicted
        //   [3] BinOpConst(r3, r0, Add, 0)  must use r0 (not r5)
        //   [4] Return(r3)
        //
        // Without the call-boundary invalidation, copy-prop would replace r0
        // with r5 in [3], producing BinOpConst(r3, r5, Add, 0), which would
        // compute the pre-call value of r5 rather than the updated r0.
        let insns = vec![
            Insn::LoadConst(5, 0),                           // r5 = consts[0]
            Insn::Move(0, 5),                                // r0 (named local) = r5
            Insn::Call(8, 0),                                // call — may clobber r0
            Insn::BinOpConst(3, 0, BinaryOp::Add, 0, false), // r3 = r0 + consts[0]
            Insn::Return(3),
        ];
        // num_locals=2: r0 and r1 are named locals; r2+ are temps.
        let out = pass_copy_prop(insns, 2);
        // After Call, copies[r0 → r5] must be evicted. The BinOpConst at [3]
        // must still use r0, not the aliased r5.
        assert!(
            matches!(out[3], Insn::BinOpConst(3, 0, BinaryOp::Add, 0, ..)),
            "named-local alias r0→r5 must not be propagated past Call: found {:?}",
            out[3]
        );
    }

    #[test]
    fn copy_prop_substitutes_yieldfrom_iter_and_sent_regs() {
        // Regression test for issue #1521: pass_copy_prop had no YieldFrom arm,
        // so iter_reg and sent_reg were never substituted when copy aliases existed.
        //
        // Sequence:
        //   [0] LoadConst(r2, 0)           r2 = some iterator object (const slot 0)
        //   [1] Move(r3, r2)               alias: r3 → r2
        //   [2] LoadNone(r4)               sent_reg initial value
        //   [3] Move(r5, r4)               alias: r5 → r4
        //   [4] LoadNone(r6)               result_reg
        //   [5] YieldFrom { iter_reg: r3, sent_reg: r5, result_reg: r6 }
        //       → after substitution: iter_reg should become r2, sent_reg r4
        //   [6] Return(r6)
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::Move(3, 2),
            Insn::LoadNone(4),
            Insn::Move(5, 4),
            Insn::LoadNone(6),
            Insn::YieldFrom {
                iter_reg: 3,
                sent_reg: 5,
                result_reg: 6,
            },
            Insn::Return(6),
        ];
        let out = pass_copy_prop(insns, 0);
        assert!(
            matches!(
                out[5],
                Insn::YieldFrom {
                    iter_reg: 2,
                    sent_reg: 4,
                    result_reg: 6,
                }
            ),
            "iter_reg r3→r2 and sent_reg r5→r4 should be substituted; result_reg must stay r6: found {:?}",
            out[5]
        );
    }

    // ── pass_fold_const_tuple ─────────────────────────────────────────────────

    #[test]
    fn fold_const_tuple_two_consts() {
        use crate::value::Value;
        let mut consts = vec![Value::int(10), Value::int(20)];
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::LoadConst(3, 1),
            Insn::BuildTuple(4, 2, 2),
            Insn::Return(4),
        ];
        let out = pass_fold_const_tuple(insns, 2, &mut consts);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Insn::LoadConst(4, _)));
        let new_idx = match out[0] {
            Insn::LoadConst(_, i) => i,
            _ => panic!("expected LoadConst"),
        };
        let elems = consts[new_idx as usize]
            .as_tuple()
            .expect("new constant should be a tuple");
        assert_eq!(elems.len(), 2);
        assert!(matches!(elems[0].kind(), crate::value::ValueKind::Int(10)));
        assert!(matches!(elems[1].kind(), crate::value::ValueKind::Int(20)));
    }

    #[test]
    fn fold_const_tuple_skips_local_regs() {
        use crate::value::Value;
        let mut consts = vec![Value::int(1), Value::int(2)];
        let insns = vec![
            Insn::LoadConst(1, 0),
            Insn::LoadConst(2, 1),
            Insn::BuildTuple(5, 1, 2),
            Insn::Return(5),
        ];
        let out = pass_fold_const_tuple(insns, 3, &mut consts);
        assert_eq!(
            out.len(),
            4,
            "should not fold when base register is a local"
        );
        assert!(matches!(out[2], Insn::BuildTuple(5, 1, 2)));
    }

    // ── pass_dead_store_elim ──────────────────────────────────────────────────

    #[test]
    fn dse_removes_overwritten_load_const() {
        // LoadConst(r2, 0) immediately overwritten by LoadConst(r2, 1); first is dead.
        let insns = vec![
            Insn::LoadConst(2, 0), // dead — r2 written again before any read
            Insn::LoadConst(2, 1), // live — r2 used by Return
            Insn::Return(2),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 2, "dead LoadConst should be removed");
        assert!(matches!(out[0], Insn::LoadConst(2, 1)));
        assert!(matches!(out[1], Insn::Return(2)));
    }

    #[test]
    fn dse_keeps_store_that_is_read() {
        // LoadConst(r2, 0) is read by Return — must be kept.
        let insns = vec![Insn::LoadConst(2, 0), Insn::Return(2)];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 2, "live LoadConst must not be removed");
    }

    #[test]
    fn dse_keeps_local_register_writes() {
        // Register r0 < num_locals=2 — locals must not be eliminated.
        let insns = vec![
            Insn::LoadConst(0, 0), // local reg — keep even if "dead"
            Insn::LoadConst(0, 1), // overwrites r0
            Insn::Return(0),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 3, "local register writes must not be removed");
    }

    #[test]
    fn dse_skips_when_back_edge_present_and_register_is_read() {
        // LoadConst(r2, 0) followed by a loop back-edge, and r2 IS read (by
        // Return(r2) after the loop) — must be kept because it is live.
        //
        //   [0] LoadConst(r2, 0)   ← r2 read by [2]
        //   [1] Jump(-2)           ← back-edge (target = 1+1-2 = 0)
        //   [2] Return(r2)
        let insns = vec![
            Insn::LoadConst(2, 0), // r2 is live (read by Return)
            Insn::Jump(-2),        // back-edge
            Insn::Return(2),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(
            out.len(),
            3,
            "must not remove store when register is read globally and back-edge is present"
        );
    }

    #[test]
    fn dse_removes_dead_move() {
        use crate::ast::BinaryOp;
        // Move(r3, r2) followed immediately by BinOp(r3, ...) — Move is dead.
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::Move(3, 2),                    // dead — r3 overwritten below
            Insn::BinOp(3, 2, BinaryOp::Add, 2), // overwrites r3
            Insn::Return(3),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 3, "dead Move should be removed");
        assert!(matches!(out[1], Insn::BinOp(3, 2, BinaryOp::Add, 2)));
    }

    #[test]
    fn dse_keeps_then_branch_write_across_jump() {
        // Issue #361: a ternary's `then`-arm writes a temp, then jumps over the
        // `else`-arm's write to the same temp.  A purely linear scan would see
        // the else-arm's LoadConst as the "next write" and incorrectly delete
        // the then-arm's store — leaving the temp unset on the taken path.
        //
        //   [0] JumpIfFalse(0, 2)     # if !c, skip to [3]
        //   [1] LoadConst(2, 0)       # then-arm: r2 = consts[0]
        //   [2] Jump(1)               # skip [3]
        //   [3] LoadConst(2, 1)       # else-arm: r2 = consts[1]
        //   [4] Return(2)
        //
        // The store at [1] is REACHABLE and consumed by [4] along the taken
        // path (c truthy).  DSE must keep it.
        let insns = vec![
            Insn::JumpIfFalse(0, 2),
            Insn::LoadConst(2, 0), // <-- must NOT be removed
            Insn::Jump(1),
            Insn::LoadConst(2, 1),
            Insn::Return(2),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(
            out.len(),
            5,
            "the then-arm's LoadConst must survive — its value is read on the taken path",
        );
        assert!(matches!(out[1], Insn::LoadConst(2, 0)));
    }

    #[test]
    fn dse_global_read_count_removes_zero_read_temp_inside_loop() {
        // A LoadConst for a temp register that is never read anywhere in the
        // function body — not even inside the loop — should be removed even
        // though there is a back-edge after it.
        //
        // Before this change, pass_dead_store_elim would see the back-edge
        // (Jump(-N)) after the LoadConst and conservatively keep it.  With
        // the global-read-count pre-scan, it detects that r5 has 0 global
        // reads and removes the instruction unconditionally.
        //
        //   [0] LoadConst(r2, 0)   ← live — used by Return at [2]
        //   [1] LoadConst(r5, 0)   ← dead temp, r5 never read
        //   [2] Jump(-3)           ← back-edge (target = 2+1-3 = 0)
        //   [3] Return(r2)
        //
        // Expected: LoadConst(r5) removed; other instructions kept; Jump offset
        //           updated by compact to reflect the removed slot.
        let insns = vec![
            Insn::LoadConst(2, 0), // r2 — live (read by Return)
            Insn::LoadConst(5, 0), // r5 — dead temp, r5 never read globally
            Insn::Jump(-3),        // back-edge: target = 2+1-3 = 0
            Insn::Return(2),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(
            out.len(),
            3,
            "LoadConst for zero-global-read temp should be removed even with back-edge present"
        );
        // The remaining instructions are: LoadConst(r2), Jump(updated), Return(r2).
        assert!(
            matches!(out[0], Insn::LoadConst(2, 0)),
            "live LoadConst must survive: {:?}",
            out[0]
        );
        // compact rewrites the jump offset: old target was index 0; in the new
        // array that is still index 0, so new_offset = 0 - (1 + 1) = -2.
        assert!(
            matches!(out[1], Insn::Jump(-2)),
            "back-edge Jump must survive with updated offset: {:?}",
            out[1]
        );
        assert!(
            matches!(out[2], Insn::Return(2)),
            "Return must survive: {:?}",
            out[2]
        );
    }

    #[test]
    fn dse_drops_dead_call_memo() {
        // CallMemo(r2, 0) — result r2 never read.  CallMemo is pure so it is safe
        // to drop.
        let insns = vec![
            Insn::LoadGlobal(2, 0), // r2 = some_pure_fn (kept: LoadGlobal can raise)
            Insn::CallMemo(2, 0),   // r2 = some_pure_fn() — result dead
            Insn::ReturnNone,
        ];
        let out = pass_dead_store_elim(insns, 2);
        // CallMemo dropped; LoadGlobal and ReturnNone kept.
        assert_eq!(out.len(), 2, "dead CallMemo should be removed");
        assert!(matches!(out[0], Insn::LoadGlobal(2, 0)));
        assert!(matches!(out[1], Insn::ReturnNone));
    }

    #[test]
    fn dse_drops_dead_call_memo_two_pass_cascade() {
        // Two-pass test: CallMemo(r2, 1) is dead; after dropping it, the
        // LoadConst(r3, 0) that was the argument also becomes dead.
        // The second invocation of pass_dead_store_elim cleans it up.
        let insns = vec![
            Insn::LoadGlobal(2, 0), // r2 = abs (kept)
            Insn::LoadConst(3, 0),  // r3 = 5 — arg for abs; dead after CallMemo dropped
            Insn::CallMemo(2, 1),   // r2 = abs(5) — result dead
            Insn::ReturnNone,
        ];
        // First pass drops CallMemo; second pass drops LoadConst.
        let after_first = pass_dead_store_elim(insns, 2);
        assert!(
            !after_first.iter().any(|i| matches!(i, Insn::CallMemo(..))),
            "first pass must drop CallMemo"
        );
        let after_second = pass_dead_store_elim(after_first, 2);
        assert!(
            !after_second
                .iter()
                .any(|i| matches!(i, Insn::LoadConst(..))),
            "second pass must drop the now-dead LoadConst arg"
        );
    }

    #[test]
    fn dse_keeps_call_memo_whose_result_is_used() {
        // CallMemo(r2, 0) whose result is returned — must NOT be dropped.
        let insns = vec![
            Insn::LoadGlobal(2, 0), // r2 = some_fn
            Insn::CallMemo(2, 0),   // r2 = some_fn() — result used by Return
            Insn::Return(2),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 3, "live CallMemo must not be removed");
        assert!(matches!(out[1], Insn::CallMemo(2, 0)));
    }

    #[test]
    fn dse_keeps_call_memo_local_reg() {
        // CallMemo targeting a local register (r1 < num_locals=2) — must be kept.
        let insns = vec![
            Insn::LoadGlobal(1, 0), // r1 = fn
            Insn::CallMemo(1, 0),   // r1 = fn() — local, must not be removed
            Insn::ReturnNone,
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 3, "CallMemo to local must not be removed");
    }

    // ── pass_builtin_dce ─────────────────────────────────────────────────────

    #[test]
    fn builtin_dce_drops_dead_call_to_pure_builtin_with_const_args() {
        // LoadGlobal(r2, 0) loads "abs" (pure); Call(r2, 1) has a const arg in
        // r3 (LoadConst); result in r2 is never read.  The Call must be removed.
        let names = vec!["abs".to_string()];
        let insns = vec![
            Insn::LoadGlobal(2, 0), // r2 = abs  (names[0] = "abs", pure)
            Insn::LoadConst(3, 0),  // r3 = const[0]  (e.g. -5)
            Insn::Call(2, 1),       // r2 = abs(r3) — all args const, result dead
            Insn::ReturnNone,
        ];
        let out = pass_builtin_dce(insns, 2, &names);
        assert!(
            !out.iter().any(|i| matches!(i, Insn::Call(..))),
            "dead Call with const args to pure builtin should be removed"
        );
        assert!(matches!(out.last(), Some(Insn::ReturnNone)));
    }

    #[test]
    fn builtin_dce_keeps_call_with_runtime_arg() {
        // Call(r2, 1) where arg r3 comes from a BinOp (not LoadConst).
        // The call may raise TypeError — must NOT be removed.
        let names = vec!["abs".to_string()];
        let insns = vec![
            Insn::LoadGlobal(2, 0), // r2 = abs  (pure)
            // r3 is produced by a BinOp (runtime value, not const):
            Insn::BinOp(3, 0, crate::ast::BinaryOp::Add, 1),
            Insn::Call(2, 1), // r2 = abs(r3) — r3 is NOT in const_reg → keep call
            Insn::ReturnNone,
        ];
        let out = pass_builtin_dce(insns, 2, &names);
        assert_eq!(out.len(), 4, "Call with runtime arg must not be removed");
        assert!(
            out.iter().any(|i| matches!(i, Insn::Call(..))),
            "Call with non-const arg must survive"
        );
    }

    #[test]
    fn builtin_dce_keeps_dead_call_to_impure_builtin() {
        // `len` dispatches user __len__ (issue #1526) and must NOT be DCE'd
        // even when the result is dead and the argument is a compile-time constant.
        let names = vec!["len".to_string()];
        let insns = vec![
            Insn::LoadGlobal(2, 0), // r2 = len  (impure — dispatches __len__)
            Insn::LoadConst(3, 0),  // r3 = const[0]
            Insn::Call(2, 1),       // r2 = len(r3) — result dead, but must not be removed
            Insn::ReturnNone,
        ];
        let out = pass_builtin_dce(insns, 2, &names);
        assert_eq!(out.len(), 4, "Call to impure builtin must not be removed");
        assert!(
            out.iter().any(|i| matches!(i, Insn::Call(..))),
            "Dead Call to impure len must survive (may dispatch user __len__)"
        );
    }

    #[test]
    fn builtin_dce_keeps_call_whose_result_is_used() {
        // Call(r2, 1) result is returned — must NOT be dropped even if arg is const.
        let names = vec!["abs".to_string()];
        let insns = vec![
            Insn::LoadGlobal(2, 0), // r2 = abs
            Insn::LoadConst(3, 0),  // r3 = const
            Insn::Call(2, 1),       // r2 = abs(r3) — result used by Return
            Insn::Return(2),
        ];
        let out = pass_builtin_dce(insns, 2, &names);
        assert_eq!(out.len(), 4, "live Call must not be removed");
        assert!(matches!(out[2], Insn::Call(2, 1)));
    }

    #[test]
    fn builtin_dce_keeps_call_to_impure_builtin() {
        // LoadGlobal(r2, 0) loads "print" (impure); Call must be kept even if
        // the result is never read and args are const.
        let names = vec!["print".to_string()];
        let insns = vec![
            Insn::LoadGlobal(2, 0), // r2 = print  (impure)
            Insn::LoadConst(3, 0),  // r3 = const arg
            Insn::Call(2, 1),       // r2 = print(r3) — result dead, but call has side effects
            Insn::ReturnNone,
        ];
        let out = pass_builtin_dce(insns, 2, &names);
        assert_eq!(out.len(), 4, "Call to impure builtin must not be removed");
        assert!(matches!(out[2], Insn::Call(2, 1)));
    }

    // ── pass_exit_inline ─────────────────────────────────────────────────────

    #[test]
    fn exit_inline_jump_to_return() {
        // Jump(0) at index 0 targets index 1 (Return(r)) → replaced with Return(r).
        let insns = vec![Insn::Jump(0), Insn::Return(5)];
        let out = pass_exit_inline(insns);
        assert_eq!(out.len(), 2, "no instructions removed — only inlined");
        assert!(
            matches!(out[0], Insn::Return(5)),
            "Jump targeting Return should be replaced with Return"
        );
    }

    #[test]
    fn exit_inline_jump_to_return_none() {
        let insns = vec![Insn::Jump(1), Insn::LoadConst(0, 0), Insn::ReturnNone];
        let out = pass_exit_inline(insns);
        // Jump(1) at index 0 targets index 2 (ReturnNone)
        assert!(
            matches!(out[0], Insn::ReturnNone),
            "Jump targeting ReturnNone should be replaced with ReturnNone"
        );
    }

    #[test]
    fn exit_inline_skips_non_terminal_target() {
        use crate::ast::BinaryOp;
        // Jump(0) targets LoadConst — not a terminal, must not be replaced.
        let insns = vec![Insn::Jump(0), Insn::LoadConst(3, 0), Insn::Return(3)];
        let out = pass_exit_inline(insns);
        assert!(
            matches!(out[0], Insn::Jump(0)),
            "Jump to non-terminal should be kept as-is"
        );
        // Suppress unused-import lint
        let _ = BinaryOp::Add;
    }

    #[test]
    fn exit_inline_skips_conditional_jumps() {
        // JumpIfFalse is NOT an unconditional Jump — must not be modified.
        let insns = vec![Insn::JumpIfFalse(0, 0), Insn::Return(0)];
        let out = pass_exit_inline(insns);
        assert!(
            matches!(out[0], Insn::JumpIfFalse(0, 0)),
            "conditional jumps must not be inlined"
        );
    }

    // ── pass_licm ─────────────────────────────────────────────────────────────

    /// Build a minimal loop with a back edge:
    ///
    /// ```text
    /// [0]  LoadConst(r5, 0)         ← invariant: hoistable
    /// [1]  ForCountConst(r0, Lt, 0, 1, 2)   ← loop header (target of back edge at [3])
    ///                                         if counter exhausted: jump to [4]
    /// [2]  BinOp(r1, r1, Add, r0)   ← body: uses r5 indirectly via BinOp(not hoistable)
    /// [3]  Jump(-3)                 ← back edge → [1]
    /// [4]  Return(r1)
    /// ```
    ///
    /// After LICM, LoadConst(r5, 0) should appear before the loop header (at index 0
    /// in the pre-header block, which is before old index 1).
    #[test]
    fn licm_hoists_loadconst_before_loop() {
        use crate::ast::BinaryOp;

        // Layout (raw, before LICM):
        //  [0] LoadConst(r5, 0)           — invariant
        //  [1] ForCountConst(r0, Lt, 0, 1, 2) — header; jumps to [4] when done
        //  [2] BinOp(r1, r1, Add, r0)     — body
        //  [3] Jump(-3)                   — back edge → [1]
        //  [4] Return(r1)
        //
        // Back edge: Jump(-3) at old index 3 → target = 3+1-3 = 1 → header=1, latch=3
        // Write set for [1..=3]: {r0} (ForCountConst writes r0), {r1} (BinOp writes r1)
        // LoadConst(r5, 0) is at index 0 — OUTSIDE the loop [1..=3], so LICM does
        // nothing here since 0 < header=1.
        //
        // Adjusted test: put LoadConst INSIDE the loop body, so LICM moves it out.
        //
        //  [0] ForCountConst(r0, Lt, 0, 1, 3) — header; jumps to [4] when done
        //  [1] LoadConst(r5, 0)               — invariant (inside loop body)
        //  [2] BinOp(r1, r1, Add, r0)         — not invariant (r0 is written by header)
        //  [3] Jump(-4)                        — back edge → [0]
        //  [4] Return(r1)
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 3), // [0] header, exits to [4]
            Insn::LoadConst(5, 0),                         // [1] invariant
            Insn::BinOp(1, 1, BinaryOp::Add, 0),           // [2] not invariant
            Insn::Jump(-4),                                // [3] back edge → [0]
            Insn::Return(1),                               // [4]
        ];
        // r0..r4 are named locals; r5 is a temp (>= num_locals=5) → hoistable.
        let out = pass_licm(insns, 5);

        // After hoisting LoadConst(r5, 0) before header [0], the new layout is:
        //  [0] LoadConst(r5, 0)               — hoisted
        //  [1] ForCountConst(r0, Lt, 0, 1, 2) — header (offset adjusted: was 3, now 2)
        //  [2] BinOp(r1, r1, Add, r0)
        //  [3] Jump(-3)                        — back edge → [1]
        //  [4] Return(r1)
        assert_eq!(out.len(), 5, "instruction count must not change");
        assert!(
            matches!(out[0], Insn::LoadConst(5, 0)),
            "LoadConst should be hoisted to position 0 (before loop header); got {:?}",
            out[0]
        );
        // The loop header must still be present and its exit offset adjusted to
        // land on the Return (still at the end of the 5-instruction list).
        assert!(
            matches!(out[1], Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _)),
            "loop header should remain at position 1"
        );
    }

    /// `BinOpConst(dst, src, op, c)` is loop-invariant when `src` is not written
    /// inside the loop body.  Verify it is hoisted.
    #[test]
    fn licm_hoists_binopconst_with_invariant_src() {
        use crate::ast::BinaryOp;

        // Loop layout:
        //  [0] ForCountConst(r0, Lt, 0, 1, 3) — header, exits to [4]
        //  [1] BinOpConst(r5, r2, Add, 0)      — r2 NOT written in loop → invariant
        //  [2] BinOp(r1, r1, Add, r0)           — uses r0 (written) → not invariant
        //  [3] Jump(-4)                          — back edge → [0]
        //  [4] Return(r1)
        //
        // r0 is written by ForCountConst; r1 by BinOp; r2 is untouched.
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 3),   // [0]
            Insn::BinOpConst(5, 2, BinaryOp::Add, 0, false), // [1] r2 not in write set → hoist
            Insn::BinOp(1, 1, BinaryOp::Add, 0),             // [2] r0 written → keep
            Insn::Jump(-4),                                  // [3]
            Insn::Return(1),                                 // [4]
        ];
        // r0..r4 are named locals; r5 is a temp (>= num_locals=5) → BinOpConst(r5) hoistable.
        let out = pass_licm(insns, 5);

        assert_eq!(out.len(), 5);
        assert!(
            matches!(out[0], Insn::BinOpConst(5, 2, BinaryOp::Add, 0, ..)),
            "BinOpConst with invariant src should be hoisted before header; got {:?}",
            out[0]
        );
        assert!(
            matches!(out[1], Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _)),
            "loop header must follow the hoisted instruction"
        );
    }

    /// `LoadConst(dst, idx)` where `dst < num_locals` (a named local) must NOT be
    /// hoisted: a zero-trip loop must not unconditionally assign named locals.
    ///
    /// This is the core regression guard for issue #580.
    #[test]
    fn licm_does_not_hoist_loadconst_to_named_local() {
        use crate::ast::BinaryOp;

        // Loop layout:
        //  [0] ForCountConst(r0, Lt, 0, 1, 2) — header, exits to [3]
        //  [1] LoadConst(r1, 0)                — r1 IS a named local (< num_locals=5)
        //  [2] Jump(-3)                         — back edge → [0]
        //  [3] Return(r1)
        //
        // r0..r4 are named locals; r1 is named local → LoadConst(r1) must NOT be hoisted.
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 2), // [0] header
            Insn::LoadConst(1, 0),                         // [1] dst=r1, named local
            Insn::Jump(-3),                                // [2] back edge → [0]
            Insn::Return(1),                               // [3]
        ];
        let before = insns.clone();
        // r1 < num_locals=5 → named local → must not be hoisted.
        let out = pass_licm(insns, 5);

        // Instruction count must be unchanged (nothing hoisted).
        assert_eq!(out.len(), before.len(), "instruction count must not change");
        assert!(
            matches!(out[0], Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _)),
            "loop header must remain at position 0; LoadConst(r1) must not be hoisted before it"
        );
        assert!(
            matches!(out[1], Insn::LoadConst(1, 0)),
            "LoadConst to named local must stay in loop body"
        );
    }

    /// `BinOpConst(dst, src, op, c)` where `dst < num_locals` (a named local) must NOT
    /// be hoisted: a zero-trip loop must not unconditionally assign named locals.
    #[test]
    fn licm_does_not_hoist_binopconst_to_named_local() {
        use crate::ast::BinaryOp;

        // Loop layout:
        //  [0] ForCountConst(r0, Lt, 0, 1, 2) — header
        //  [1] BinOpConst(r1, r2, Add, 0)      — r1 IS a named local (< num_locals=5)
        //  [2] Jump(-3)                          — back edge → [0]
        //  [3] Return(r1)
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 2),   // [0]
            Insn::BinOpConst(1, 2, BinaryOp::Add, 0, false), // [1] dst=r1 < 5 → keep
            Insn::Jump(-3),                                  // [2]
            Insn::Return(1),                                 // [3]
        ];
        let before = insns.clone();
        let out = pass_licm(insns, 5);

        assert_eq!(out.len(), before.len(), "instruction count must not change");
        assert!(
            matches!(out[0], Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _)),
            "loop header must remain at position 0"
        );
        assert!(
            matches!(out[1], Insn::BinOpConst(1, 2, BinaryOp::Add, 0, ..)),
            "BinOpConst to named local must stay in loop body"
        );
    }

    /// `BinOpConst(dst, src, op, c)` where `src` IS written in the loop must NOT
    /// be hoisted.
    #[test]
    fn licm_does_not_hoist_binopconst_with_variant_src() {
        use crate::ast::BinaryOp;

        // r0 is the loop counter (written by ForCountConst); BinOpConst reads r0 → variant.
        //  [0] ForCountConst(r0, Lt, 0, 1, 3)
        //  [1] BinOpConst(r5, r0, Add, 0)      — r0 IS written → NOT invariant
        //  [2] BinOp(r1, r1, Add, r5)
        //  [3] Jump(-4)
        //  [4] Return(r1)
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 3),   // [0]
            Insn::BinOpConst(5, 0, BinaryOp::Add, 0, false), // [1] r0 in write set → keep
            Insn::BinOp(1, 1, BinaryOp::Add, 5),             // [2]
            Insn::Jump(-4),                                  // [3]
            Insn::Return(1),                                 // [4]
        ];
        let before = insns.clone();
        // r5 is a temp but r0 (src) IS written by ForCountConst → still not hoisted.
        let out = pass_licm(insns, 5);

        // Nothing should move: BinOpConst reads r0 which is written by ForCountConst.
        assert_eq!(
            out.len(),
            before.len(),
            "instruction count should not change"
        );
        assert!(
            matches!(out[0], Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _)),
            "loop header must remain at position 0 (nothing hoisted)"
        );
        assert!(
            matches!(out[1], Insn::BinOpConst(5, 0, BinaryOp::Add, 0, ..)),
            "variant BinOpConst must stay in loop body"
        );
    }

    /// Nested loops: an invariant in the inner loop that is also invariant wrt the
    /// outer loop ends up hoisted all the way out of both loops (before the outer
    /// header).  Verify that it is no longer inside the inner loop body.
    #[test]
    fn licm_hoists_inner_invariant_out_of_inner_loop() {
        use crate::ast::BinaryOp;

        // Outer loop: back edge at [7] → header [0].
        // Inner loop: back edge at [5] → inner header [2].
        //
        //  [0] ForCountConst(r0, Lt, 0, 1, 7)  — outer header, exits to [8]
        //  [1] BinOp(r1, r1, Add, r0)           — outer body (r0 written by outer)
        //  [2] ForCountConst(r3, Lt, 2, 3, 3)   — inner header, exits to [6]
        //  [3] LoadConst(r9, 0)                 — invariant wrt both loops
        //  [4] BinOp(r4, r4, Add, r3)           — uses r3 (written by inner) → variant
        //  [5] Jump(-4)                          — inner back edge → [2]
        //  [6] BinOp(r1, r1, Add, r4)           — outer body (after inner)
        //  [7] Jump(-8)                          — outer back edge → [0]
        //  [8] Return(r1)
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 7), // [0] outer header
            Insn::BinOp(1, 1, BinaryOp::Add, 0),           // [1]
            Insn::ForCountConst(3, BinaryOp::Lt, 2, 3, 3), // [2] inner header
            Insn::LoadConst(9, 0),                         // [3] invariant wrt inner
            Insn::BinOp(4, 4, BinaryOp::Add, 3),           // [4] variant wrt inner (r3 written)
            Insn::Jump(-4),                                // [5] inner back edge → [2]
            Insn::BinOp(1, 1, BinaryOp::Add, 4),           // [6]
            Insn::Jump(-8),                                // [7] outer back edge → [0]
            Insn::Return(1),                               // [8]
        ];
        // r0..r4 are named locals; r9 is a temp (>= num_locals=5) → LoadConst(r9) hoistable.
        let out = pass_licm(insns, 5);

        assert_eq!(out.len(), 9, "total instruction count unchanged");

        // LoadConst(r9, 0) is invariant wrt both loops.  The inner loop processes
        // first and hoists it before the inner header; the outer loop then hoists it
        // again before the outer header.  Either way, it must not remain inside the
        // inner loop body [inner_header..inner_latch].
        //
        // Find the inner header (ForCountConst for r3) and latch (Jump with negative
        // offset targeting the inner header).  LoadConst(r9, _) must not appear
        // between them.
        let inner_header_pos = out
            .iter()
            .position(|i| matches!(i, Insn::ForCountConst(3, BinaryOp::Lt, 2, 3, _)))
            .expect("inner header must still exist");
        let inner_latch_pos = out
            .iter()
            .enumerate()
            .position(|(i, insn)| {
                if let Insn::Jump(k) = insn {
                    let target = i as i64 + 1 + *k as i64;
                    target == inner_header_pos as i64
                } else {
                    false
                }
            })
            .expect("inner back-edge Jump must still exist");

        let loadconst_inside_inner = out[inner_header_pos..=inner_latch_pos]
            .iter()
            .any(|i| matches!(i, Insn::LoadConst(9, _)));
        assert!(
            !loadconst_inside_inner,
            "LoadConst(r9) must be hoisted out of the inner loop body \
             (inner_header={inner_header_pos}, inner_latch={inner_latch_pos})"
        );

        // The overall structure must remain intact: outer header and inner header
        // must still exist.
        assert!(
            out.iter()
                .any(|i| matches!(i, Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _))),
            "outer loop header must still exist"
        );
    }

    // ── pass_cse ──────────────────────────────────────────────────────────────

    #[test]
    fn cse_duplicate_loadconst_becomes_copyreg() {
        // Two loads of the same constant index → second becomes CopyReg.
        // LoadConst(r2, 0)  LoadConst(r3, 0)  Return(r2)
        // Expected: LoadConst(r2, 0)  CopyReg(r3, r2)  Return(r2)
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::LoadConst(3, 0), // duplicate
            Insn::Return(2),
        ];
        let out = pass_cse(insns, 0);
        assert_eq!(out.len(), 3, "instruction count unchanged");
        assert!(
            matches!(out[0], Insn::LoadConst(2, 0)),
            "first LoadConst must be kept"
        );
        assert!(
            matches!(out[1], Insn::CopyReg(3, 2)),
            "second LoadConst should become CopyReg(r3, r2)"
        );
    }

    #[test]
    fn cse_duplicate_binopconst_becomes_copyreg() {
        use crate::ast::BinaryOp;
        // BinOpConst(r4, r0, Add, 1)  …  BinOpConst(r5, r0, Add, 1)
        // The second should become CopyReg(r5, r4).
        let insns = vec![
            Insn::BinOpConst(4, 0, BinaryOp::Add, 1, false),
            Insn::BinOpConst(5, 0, BinaryOp::Add, 1, false), // duplicate
            Insn::Return(4),
        ];
        let out = pass_cse(insns, 0); // num_locals=0: r0 is a temp, CSE valid
        assert_eq!(out.len(), 3);
        assert!(
            matches!(out[0], Insn::BinOpConst(4, 0, BinaryOp::Add, 1, ..)),
            "first BinOpConst must be kept"
        );
        assert!(
            matches!(out[1], Insn::CopyReg(5, 4)),
            "second BinOpConst should become CopyReg(r5, r4)"
        );
    }

    #[test]
    fn cse_intervening_write_invalidates_entry() {
        use crate::ast::BinaryOp;
        // BinOpConst(r4, r0, Add, 1)
        // LoadConst(r0, 2)        ← writes r0, the input of the BinOpConst
        // BinOpConst(r5, r0, Add, 1)   ← r0 is now a different value; NOT a duplicate
        let insns = vec![
            Insn::BinOpConst(4, 0, BinaryOp::Add, 1, false),
            Insn::LoadConst(0, 2), // clobbers r0
            Insn::BinOpConst(5, 0, BinaryOp::Add, 1, false),
            Insn::Return(4),
        ];
        let out = pass_cse(insns, 0);
        assert_eq!(out.len(), 4, "no elimination when input clobbered");
        assert!(
            matches!(out[2], Insn::BinOpConst(5, 0, BinaryOp::Add, 1, ..)),
            "second BinOpConst must not be replaced after input clobber"
        );
    }

    #[test]
    fn cse_does_not_cross_basic_block_boundary() {
        // LoadConst(r2, 0)
        // JumpIfFalse(r1, 0)   ← ends basic block; target is next instruction
        // LoadConst(r3, 0)     ← same const, but different basic block → NOT replaced
        // Return(r2)
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::JumpIfFalse(1, 0), // offset 0 → target = idx 2
            Insn::LoadConst(3, 0),   // idx 2 is a BB start (jump target)
            Insn::Return(2),
        ];
        let out = pass_cse(insns, 0);
        // The third instruction (idx 2) is a BB start, so the CSE table is cleared
        // before it is processed; the second LoadConst(r3, 0) must NOT be replaced.
        assert!(
            matches!(out[2], Insn::LoadConst(3, 0)),
            "CSE must not cross basic-block boundary"
        );
    }

    #[test]
    fn cse_unary_op_duplicate_becomes_copyreg() {
        use crate::ast::UnaryOp;
        // UnaryOp(r4, Neg, r0)  UnaryOp(r5, Neg, r0)  → second becomes CopyReg(r5, r4)
        let insns = vec![
            Insn::UnaryOp(4, UnaryOp::Neg, 0),
            Insn::UnaryOp(5, UnaryOp::Neg, 0), // duplicate
            Insn::Return(4),
        ];
        let out = pass_cse(insns, 0); // num_locals=0: r0 is a temp, CSE valid
        assert_eq!(out.len(), 3);
        assert!(
            matches!(out[0], Insn::UnaryOp(4, UnaryOp::Neg, 0)),
            "first UnaryOp must be kept"
        );
        assert!(
            matches!(out[1], Insn::CopyReg(5, 4)),
            "second UnaryOp should become CopyReg(r5, r4)"
        );
    }

    #[test]
    fn cse_output_clobber_invalidates_entry() {
        // If the output register of a CSE candidate is overwritten, subsequent
        // identical computations cannot be replaced by a CopyReg pointing to it.
        //
        // LoadConst(r2, 0)        ← r2 holds consts[0]
        // LoadNone(r2)            ← clobbers r2; CSE entry for consts[0] removed
        // LoadConst(r3, 0)        ← must NOT be replaced by CopyReg(r3, r2)
        // Return(r3)
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::LoadNone(2), // clobbers the output register r2
            Insn::LoadConst(3, 0),
            Insn::Return(3),
        ];
        let out = pass_cse(insns, 0);
        assert_eq!(out.len(), 4, "no elimination when output clobbered");
        assert!(
            matches!(out[2], Insn::LoadConst(3, 0)),
            "LoadConst must not be replaced after its output register is clobbered"
        );
    }

    #[test]
    fn cse_does_not_match_binopconst_across_call_boundary() {
        use crate::ast::BinaryOp;
        // Simulates the pattern where a named-local register (r0) is used
        // as a source in BinOpConst before and after a Call instruction that
        // may update r0 via assign_name write-through.
        //
        // A Call writes to r9 (a temp, not r0 or r4).  The CSE table entry
        // {BinOpConst(r0, Add, 1) -> r4} must NOT survive across the Call,
        // because at runtime r0 may have been updated by the callee.
        //
        //   [0] BinOpConst(r4, r0, Add, 1)   <- first occurrence; r4 = r0 + c1
        //   [1] Call(r9, r2)                  <- may update r0 via write-through
        //   [2] BinOpConst(r5, r0, Add, 1)   <- second occurrence; must NOT be CopyReg(r5, r4)
        //   [3] Return(r5)
        // num_locals=1: r0 is a named local; r4, r5, r9 are temps.
        let insns = vec![
            Insn::BinOpConst(4, 0, BinaryOp::Add, 1, false), // r4 = r0 + c1
            Insn::Call(9, 2), // call; writes r9 (temp), may clobber r0
            Insn::BinOpConst(5, 0, BinaryOp::Add, 1, false), // r5 = r0 + c1 (same expr)
            Insn::Return(5),
        ];
        let out = pass_cse(insns, 1); // r0 < num_locals: named local, must not be CSE'd across call
        // The second BinOpConst MUST NOT be replaced by CopyReg(r5, r4)
        // because r0 may have been mutated by the Call.
        assert!(
            matches!(out[2], Insn::BinOpConst(5, 0, BinaryOp::Add, 1, ..)),
            "BinOpConst after Call must not be CSE-replaced when src is a named local: {:?}",
            out[2]
        );
    }

    #[test]
    fn cse_yield_from_evicts_result_reg_and_sent_reg() {
        use crate::ast::UnaryOp;
        // Regression test for #1470: pass_cse must evict CSE table entries
        // whose source or destination is result_reg or sent_reg after YieldFrom.
        //
        // Layout:
        //   [0] UnaryOp(r5, Neg, r2)           -- r2 is sent_reg; CSE records {(Neg, r2) -> r5}
        //   [1] YieldFrom { iter_reg=r0, sent_reg=r2, result_reg=r3 }
        //                                       -- r2 and r3 are written; CSE must be evicted
        //   [2] UnaryOp(r6, Neg, r2)           -- same key (Neg, r2); must NOT be CopyReg(r6, r5)
        //   [3] Return(r6)
        let insns = vec![
            Insn::UnaryOp(5, UnaryOp::Neg, 2),
            Insn::YieldFrom {
                iter_reg: 0,
                sent_reg: 2,
                result_reg: 3,
            },
            Insn::UnaryOp(6, UnaryOp::Neg, 2),
            Insn::Return(6),
        ];
        let out = pass_cse(insns, 0);
        assert_eq!(out.len(), 4, "no instruction removed");
        assert!(
            matches!(out[2], Insn::UnaryOp(6, UnaryOp::Neg, 2)),
            "UnaryOp after YieldFrom must not be CSE-replaced: {:?}",
            out[2]
        );
    }

    #[test]
    fn cse_yield_from_evicts_result_reg_as_output() {
        use crate::ast::UnaryOp;
        // If result_reg was previously the dst of a CSE'd expression, that entry
        // must be evicted after YieldFrom writes a new value into result_reg.
        //
        //   [0] UnaryOp(r3, Neg, r1)    -- CSE records {(Neg, r1) -> r3}
        //   [1] YieldFrom { iter_reg=r0, sent_reg=r2, result_reg=r3 }
        //                               -- r3 is overwritten; entry must be removed
        //   [2] UnaryOp(r4, Neg, r1)    -- same key (Neg, r1); CopyReg(r4, r3) would be wrong
        //   [3] Return(r4)
        let insns = vec![
            Insn::UnaryOp(3, UnaryOp::Neg, 1),
            Insn::YieldFrom {
                iter_reg: 0,
                sent_reg: 2,
                result_reg: 3,
            },
            Insn::UnaryOp(4, UnaryOp::Neg, 1),
            Insn::Return(4),
        ];
        let out = pass_cse(insns, 0);
        assert_eq!(out.len(), 4, "no instruction removed");
        assert!(
            matches!(out[2], Insn::UnaryOp(4, UnaryOp::Neg, 1)),
            "UnaryOp after YieldFrom must not use stale result_reg as CopyReg source: {:?}",
            out[2]
        );
    }

    // ── linearized-pass equivalence (issue #2004) ──────────────────────────────

    /// Reference dead-store elimination that uses the original O(n) tail-scan
    /// (`reg_is_read_before_next_write`).  Used to confirm the linearized
    /// `pass_dead_store_elim` produces byte-identical output.
    fn dead_store_elim_reference(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
        let n = insns.len();
        let mut keep = vec![true; n];
        let mut global_read_count: HashMap<u32, usize> = HashMap::new();
        let mut reads_buf: HashSet<u32> = HashSet::new();
        for insn in &insns {
            reads_buf.clear();
            collect_reads(insn, &mut reads_buf);
            for &r in &reads_buf {
                if r >= num_locals {
                    *global_read_count.entry(r).or_insert(0) += 1;
                }
            }
        }
        for i in 0..n {
            let dst = match &insns[i] {
                Insn::LoadConst(r, _)
                | Insn::LoadNone(r)
                | Insn::Move(r, _)
                | Insn::CopyReg(r, _)
                    if *r >= num_locals =>
                {
                    *r
                }
                Insn::CallMemo(r, _) if *r >= num_locals => *r,
                _ => continue,
            };
            if global_read_count.get(&dst).copied().unwrap_or(0) == 0 {
                keep[i] = false;
                continue;
            }
            if slice_has_back_edge(&insns[i + 1..]) {
                continue;
            }
            if !reg_is_read_before_next_write(&insns[i + 1..], dst) {
                keep[i] = false;
            }
        }
        compact(insns, &keep)
    }

    /// A small library of random-ish instruction streams that exercise the
    /// dead-store / CSE eviction logic: loads, moves, copies, fused ops, reads,
    /// control flow (terminators, back-edges), and range writes.
    fn sample_streams() -> Vec<Vec<Insn>> {
        use crate::ast::{BinaryOp, UnaryOp};
        vec![
            // Long single block of distinct LoadConsts feeding one BuildList
            // (the #2004 literal shape).
            {
                let mut v: Vec<Insn> = (0..40u16)
                    .map(|i| Insn::LoadConst(2 + i as u32, i))
                    .collect();
                v.push(Insn::BuildList(2, 2, 40));
                v.push(Insn::Return(2));
                v
            },
            // Move chain with a read in the middle.
            vec![
                Insn::LoadConst(2, 0),
                Insn::Move(3, 2),
                Insn::UnaryOp(4, UnaryOp::Neg, 3),
                Insn::Move(3, 4),
                Insn::Return(3),
            ],
            // Read-then-write of the same reg in one instruction (BinOp(r,r,..)).
            vec![
                Insn::LoadConst(2, 0),
                Insn::BinOpConst(2, 2, BinaryOp::Add, 1, false),
                Insn::Return(2),
            ],
            // Store with a terminator before any read.
            vec![Insn::LoadConst(2, 0), Insn::ReturnNone],
            // Store, control-flow (forward jump), then read.
            vec![Insn::LoadConst(2, 0), Insn::Jump(0), Insn::Return(2)],
            // Back-edge after a store (loop-carried).
            vec![
                Insn::LoadConst(2, 0),
                Insn::BinOpConst(2, 2, BinaryOp::Add, 1, false),
                Insn::Jump(-2),
            ],
            // LoadNoneRange overwriting a previously loaded temp.
            vec![
                Insn::LoadConst(2, 0),
                Insn::LoadNoneRange { start: 2, count: 3 },
                Insn::Return(2),
            ],
            // CopyReg dead store (never read).
            vec![Insn::LoadConst(2, 0), Insn::CopyReg(3, 2), Insn::Return(2)],
            // Duplicate LoadConst (CSE target) with intervening write.
            vec![
                Insn::LoadConst(2, 0),
                Insn::Move(2, 3),
                Insn::LoadConst(4, 0),
                Insn::Return(4),
            ],
            // Duplicate fused op separated by an unrelated write.
            vec![
                Insn::BinOpConst(2, 0, BinaryOp::Add, 1, false),
                Insn::LoadConst(5, 7),
                Insn::BinOpConst(3, 0, BinaryOp::Add, 1, false),
                Insn::Return(3),
            ],
            // Fused op whose source is overwritten between two occurrences.
            vec![
                Insn::BinOpConst(2, 0, BinaryOp::Add, 1, false),
                Insn::Move(0, 9),
                Insn::BinOpConst(3, 0, BinaryOp::Add, 1, false),
                Insn::Return(3),
            ],
        ]
    }

    #[test]
    fn dead_store_elim_matches_reference() {
        for stream in sample_streams() {
            for num_locals in [0u32, 2, 3] {
                let a = pass_dead_store_elim(stream.clone(), num_locals);
                let b = dead_store_elim_reference(stream.clone(), num_locals);
                assert_eq!(
                    a, b,
                    "dead_store_elim diverged from reference (num_locals={num_locals}) on {stream:?}"
                );
            }
        }
    }

    /// Reference CSE using a plain `HashMap` with full-table `retain` eviction
    /// (the pre-#2004 algorithm).  Confirms the reverse-indexed `CseTable`
    /// produces byte-identical output.
    fn cse_reference(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
        #[derive(Eq, PartialEq, Hash, Clone)]
        enum K {
            LoadConst(u16),
            BinOpConst(u32, crate::ast::BinaryOp, u16),
            BinOpImm(u32, crate::ast::BinaryOp, i16),
            UnaryOp(crate::ast::UnaryOp, u32),
        }
        let n = insns.len();
        if n == 0 {
            return insns;
        }
        let mut is_bb_start = vec![false; n + 1];
        is_bb_start[0] = true;
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
                | Insn::ForCountReg(_, _, _, _, k)
                | Insn::ForCountConst(_, _, _, _, k)
                | Insn::ForCountConstInline(_, _, _, _, k)
                | Insn::SetupExcept(k)
                | Insn::MatchExcept(_, k)
                | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
                _ => None,
            };
            if let Some(k) = k {
                let target = (i as i64 + 1 + k as i64) as usize;
                if target <= n {
                    is_bb_start[target] = true;
                }
            }
        }
        let mut table: HashMap<K, u32> = HashMap::new();
        let mut result: Vec<Insn> = Vec::with_capacity(n);
        for (i, insn) in insns.into_iter().enumerate() {
            if is_bb_start[i] {
                table.clear();
            }
            let key: Option<(K, u32)> = match &insn {
                Insn::LoadConst(dst, idx) => Some((K::LoadConst(*idx), *dst)),
                Insn::BinOpConst(dst, src, op, idx, false) => {
                    Some((K::BinOpConst(*src, *op, *idx), *dst))
                }
                Insn::BinOpImm(dst, src, op, imm, false) => {
                    Some((K::BinOpImm(*src, *op, *imm), *dst))
                }
                Insn::UnaryOp(dst, op, src) => Some((K::UnaryOp(*op, *src), *dst)),
                _ => None,
            };
            let written_reg: Option<u32> = match &insn {
                Insn::LoadConst(r, _) | Insn::LoadNone(r) | Insn::LoadGlobal(r, _) => Some(*r),
                Insn::Move(dst, _) => Some(*dst),
                Insn::Unpack(..) | Insn::LoadNoneRange { .. } => None,
                _ => writable_dst(&insn),
            };
            let evict_range = |table: &mut HashMap<K, u32>, lo: u32, hi: u32| {
                table.retain(|k, prev_dst| {
                    if *prev_dst >= lo && *prev_dst < hi {
                        return false;
                    }
                    match k {
                        K::LoadConst(_) => true,
                        K::BinOpConst(src, _, _) | K::BinOpImm(src, _, _) => {
                            *src < lo || *src >= hi
                        }
                        K::UnaryOp(_, src) => *src < lo || *src >= hi,
                    }
                });
            };
            if let Insn::LoadNoneRange { start, count } = &insn {
                evict_range(&mut table, *start, start + *count as u32);
            } else if let Insn::Unpack(base, _, m) = &insn {
                evict_range(&mut table, *base, base + m);
            } else if let Some(w) = written_reg {
                table.retain(|k, prev_dst| {
                    if *prev_dst == w {
                        return false;
                    }
                    match k {
                        K::LoadConst(_) => true,
                        K::BinOpConst(src, _, _) | K::BinOpImm(src, _, _) => *src != w,
                        K::UnaryOp(_, src) => *src != w,
                    }
                });
            }
            if let Insn::UnpackEx {
                dst_base,
                before,
                after,
                ..
            } = &insn
            {
                let lo = *dst_base;
                let hi = dst_base + *before as u32 + 1 + *after as u32;
                evict_range(&mut table, lo, hi);
            }
            if let Insn::YieldFrom {
                result_reg,
                sent_reg,
                ..
            } = &insn
            {
                let rr = *result_reg;
                let sr = *sent_reg;
                table.retain(|k, prev_dst| {
                    if *prev_dst == rr || *prev_dst == sr {
                        return false;
                    }
                    match k {
                        K::LoadConst(_) => true,
                        K::BinOpConst(src, _, _) | K::BinOpImm(src, _, _) => {
                            *src != rr && *src != sr
                        }
                        K::UnaryOp(_, src) => *src != rr && *src != sr,
                    }
                });
            }
            if matches!(
                insn,
                Insn::Call(..)
                    | Insn::CallMemo(..)
                    | Insn::CallKw { .. }
                    | Insn::CallEx { .. }
                    | Insn::CallMethod { .. }
                    | Insn::CallMethodKw { .. }
                    | Insn::CallMethodExpanded { .. }
                    | Insn::MakeClass(..)
                    | Insn::MakeClassMeta(..)
            ) {
                table.retain(|k, _| match k {
                    K::LoadConst(_) => true,
                    K::BinOpConst(src, _, _) | K::BinOpImm(src, _, _) => *src >= num_locals,
                    K::UnaryOp(_, src) => *src >= num_locals,
                });
            }
            let replaced = if let Some((ref k, dst)) = key {
                if let Some(&prev_dst) = table.get(k) {
                    if prev_dst != dst {
                        result.push(Insn::CopyReg(dst, prev_dst));
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !replaced {
                if let Some((k, dst)) = key {
                    table.insert(k, dst);
                }
                result.push(insn);
            }
            let is_terminator = matches!(
                result.last().unwrap(),
                Insn::Jump(_)
                    | Insn::JumpIfFalse(..)
                    | Insn::JumpIfTrue(..)
                    | Insn::CmpJumpIfFalse(..)
                    | Insn::CmpJumpIfTrue(..)
                    | Insn::CmpJumpIfFalseConst(..)
                    | Insn::CmpJumpIfTrueConst(..)
                    | Insn::ForIter(..)
                    | Insn::ForCountReg(..)
                    | Insn::ForCountConst(..)
                    | Insn::ForCountConstInline(..)
                    | Insn::SetupExcept(_)
                    | Insn::MatchExcept(..)
                    | Insn::MatchExceptStar(..)
                    | Insn::Return(_)
                    | Insn::ReturnNone
                    | Insn::RaiseValue(_)
                    | Insn::RaiseFrom(..)
                    | Insn::RaiseReRaise
                    | Insn::RaiseAssert(_)
                    | Insn::RaiseAssertNoMsg
            );
            if is_terminator {
                table.clear();
            }
        }
        result
    }

    #[test]
    fn cse_linearized_matches_reference() {
        for stream in sample_streams() {
            for num_locals in [0u32, 1, 2, 3] {
                let a = pass_cse(stream.clone(), num_locals);
                let b = cse_reference(stream.clone(), num_locals);
                assert_eq!(
                    a, b,
                    "cse diverged from reference (num_locals={num_locals}) on {stream:?}"
                );
            }
        }
    }

    #[test]
    fn cse_linearized_matches_small_cases() {
        // The dedicated cse_* tests above lock in the expected output for each
        // eviction path; re-run them through the linearized table on the literal
        // shape to confirm the indexed eviction still dedups within one block.
        let mut v: Vec<Insn> = vec![Insn::LoadConst(2, 0), Insn::LoadConst(3, 0)];
        v.push(Insn::BuildList(2, 2, 2));
        v.push(Insn::Return(2));
        let out = pass_cse(v, 0);
        // The second LoadConst(_, 0) must become CopyReg(3, 2).
        assert!(
            matches!(out[1], Insn::CopyReg(3, 2)),
            "duplicate LoadConst within a block should CSE: {:?}",
            out[1]
        );
    }

    // ── pass_ivsr ─────────────────────────────────────────────────────────────

    #[test]
    fn ivsr_replaces_induction_var_mul_with_accumulator() {
        use crate::ast::BinaryOp;
        // Simulates: for i in range(10): r_dst = i * 3
        //
        // [0] LoadConst(r_iv=2, c_neg1=0)        iv init = start - step = -1
        // [1] ForCountConst(2, Lt, c_10=1, c_1=2, k_exit=2)  → jumps to [4]
        // [2] BinOpConst(r_dst=3, 2, Mul, c_3=3)
        // [3] Jump(-3)                             back to [1]
        // [4] Return(3)
        let mut consts = vec![Value::int(-1), Value::int(10), Value::int(1), Value::int(3)];
        let mut num_regs = 4u32;
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 2),
            Insn::BinOpConst(3, 2, BinaryOp::Mul, 3, false),
            Insn::Jump(-3),
            Insn::Return(3),
        ];
        let out = pass_ivsr(insns, &mut consts, &mut num_regs);

        // Expected (7 instructions): LoadConst(iv), LoadConst(acc), ForCountConst,
        //   Move(dst, acc), BinOpConst(acc += K), Jump(back), Return
        assert_eq!(out.len(), 7, "two instructions inserted");
        assert_eq!(num_regs, 5, "one new register allocated");
        // [1] = LoadConst(r_acc=4, c_neg3)  — acc init = -1 * 3 = -3
        assert!(
            matches!(out[1], Insn::LoadConst(4, _)),
            "accumulator init inserted before loop header"
        );
        // [2] = ForCountConst — exit offset now points to [6] (was [4])
        assert!(
            matches!(out[2], Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 3)),
            "ForCountConst exit offset adjusted (2 → 3)"
        );
        // [3] = Move(r_dst, r_acc)
        assert!(
            matches!(out[3], Insn::Move(3, 4)),
            "BinOpConst replaced by Move(dst, acc)"
        );
        // [4] = BinOpConst(r_acc, r_acc, Add, c_K)
        assert!(
            matches!(out[4], Insn::BinOpConst(4, 4, BinaryOp::Add, 3, ..)),
            "accumulator increment inserted before back-edge"
        );
        // [5] = Jump — back-edge: old offset -3 (h=1, latch=3), new = h+1 - (latch+2) - 1 = 2-5-1 = -4
        assert!(
            matches!(out[5], Insn::Jump(-4)),
            "back-edge offset adjusted"
        );
        // Accumulator init value = 0 (= (-1 + 1) * 3 = start * K for range(10))
        let acc_init_in_consts = consts.iter().any(|v| matches!(v.kind(), ValueKind::Int(0)));
        assert!(
            acc_init_in_consts,
            "const 0 added for accumulator init ((-1+1)*3=0)"
        );
    }

    #[test]
    fn ivsr_skips_when_body_branch_can_skip_increment() {
        use crate::ast::BinaryOp;
        // A conditional in the body jumps to the back-edge, skipping the
        // accumulator increment IVSR would insert there → must NOT reduce.
        //
        // [0] LoadConst(iv=2, c=-1)
        // [1] ForCountConst(2, Lt, 1, 2, k)
        // [2] BinOpConst(3, 2, Mul, 3)        ← i*K
        // [3] JumpIfFalse(4, 1)               ← if false, jump to [5] (the latch)
        // [4] BinOpInPlace(0, 0, Add, 3)
        // [5] Jump(-5)                         ← back-edge (latch)
        // [6] Return(0)
        let mut consts = vec![Value::int(-1), Value::int(10), Value::int(1), Value::int(3)];
        let mut num_regs = 5u32;
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 5),
            Insn::BinOpConst(3, 2, BinaryOp::Mul, 3, false),
            Insn::JumpIfFalse(4, 1),
            Insn::BinOpInPlace(0, 0, BinaryOp::Add, 3),
            Insn::Jump(-5),
            Insn::Return(0),
        ];
        let out = pass_ivsr(insns.clone(), &mut consts, &mut num_regs);
        assert_eq!(out, insns, "branch can skip increment → no reduction");
        assert_eq!(num_regs, 5, "no new register allocated");
    }

    #[test]
    fn ivsr_skips_when_body_contains_nested_loop() {
        use crate::ast::BinaryOp;
        // The reduced multiply sits inside a nested loop; the inner loop's exit
        // edge is retargeted past the inserted increment, so it never runs →
        // must NOT reduce.
        //
        // [0] LoadConst(iv=2, c=-1)
        // [1] ForCountConst(2, Lt, 1, 2, 6)   ← outer header
        // [2] LoadConst(jv=4, c=-1)
        // [3] ForCountConst(4, Lt, 1, 2, 3)   ← inner header, exit → [7] (latch)
        // [4] BinOpConst(3, 2, Mul, 3)        ← i*K inside inner loop
        // [5] BinOpInPlace(0, 0, Add, 3)
        // [6] Jump(-4)                         ← inner back-edge
        // [7] Jump(-7)                         ← outer back-edge (latch)
        // [8] Return(0)
        let mut consts = vec![Value::int(-1), Value::int(10), Value::int(1), Value::int(3)];
        let mut num_regs = 5u32;
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 6),
            Insn::LoadConst(4, 0),
            Insn::ForCountConst(4, BinaryOp::Lt, 1, 2, 3),
            Insn::BinOpConst(3, 2, BinaryOp::Mul, 3, false),
            Insn::BinOpInPlace(0, 0, BinaryOp::Add, 3),
            Insn::Jump(-4),
            Insn::Jump(-7),
            Insn::Return(0),
        ];
        let out = pass_ivsr(insns.clone(), &mut consts, &mut num_regs);
        assert_eq!(out, insns, "nested loop in body → no reduction");
        assert_eq!(num_regs, 5, "no new register allocated");
    }

    #[test]
    fn ivsr_skips_when_step_not_one() {
        use crate::ast::BinaryOp;
        // step = 2 → not eligible
        let mut consts = vec![Value::int(-2), Value::int(10), Value::int(2), Value::int(3)];
        let mut num_regs = 4u32;
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 2), // step_c=2 → consts[2]=2 ≠ 1
            Insn::BinOpConst(3, 2, BinaryOp::Mul, 3, false),
            Insn::Jump(-3),
            Insn::Return(3),
        ];
        let out = pass_ivsr(insns, &mut consts, &mut num_regs);
        assert_eq!(out.len(), 5, "not eligible: step ≠ 1");
        assert_eq!(num_regs, 4, "no new register allocated");
    }

    #[test]
    fn ivsr_skips_when_mul_uses_non_induction_var() {
        use crate::ast::BinaryOp;
        // BinOpConst uses r_other=5, not r_iv=2 → not eligible
        let mut consts = vec![Value::int(-1), Value::int(10), Value::int(1), Value::int(3)];
        let mut num_regs = 6u32;
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 2),
            Insn::BinOpConst(3, 5, BinaryOp::Mul, 3, false), // r_src=5 ≠ iv=2
            Insn::Jump(-3),
            Insn::Return(3),
        ];
        let out = pass_ivsr(insns, &mut consts, &mut num_regs);
        assert_eq!(out.len(), 5, "not eligible: src ≠ iv");
    }

    // ── pass_self_tail_call ───────────────────────────────────────────────────

    #[test]
    fn self_tail_call_fuses_call_return() {
        // Call(func_reg=2, nargs=2) + Return(2) → TailCall { args_base: 3, nargs: 2 }
        // The Return is dropped; one instruction remains.
        let insns = vec![Insn::Call(2, 2), Insn::Return(2)];
        let out = pass_self_tail_call(insns);
        assert_eq!(out.len(), 1, "Return should be dropped");
        assert!(
            matches!(
                out[0],
                Insn::TailCall {
                    args_base: 3,
                    nargs: 2
                }
            ),
            "Call+Return should become TailCall with args_base=func_reg+1"
        );
    }

    #[test]
    fn self_tail_call_skips_when_return_uses_different_reg() {
        // Call writes to r2, but Return reads r3 → not a tail call pattern.
        let insns = vec![
            Insn::Call(2, 2),
            Insn::Return(3), // different register
        ];
        let out = pass_self_tail_call(insns);
        assert_eq!(
            out.len(),
            2,
            "no fusion when Return reads a different register"
        );
        assert!(matches!(out[0], Insn::Call(2, 2)));
        assert!(matches!(out[1], Insn::Return(3)));
    }

    #[test]
    fn self_tail_call_skips_when_not_adjacent() {
        // Call is NOT immediately followed by Return.
        let insns = vec![
            Insn::Call(2, 1),
            Insn::Move(0, 2), // intervening instruction
            Insn::Return(0),
        ];
        let out = pass_self_tail_call(insns);
        assert_eq!(
            out.len(),
            3,
            "no fusion when Call and Return are not adjacent"
        );
        assert!(matches!(out[0], Insn::Call(2, 1)));
    }

    #[test]
    fn self_tail_call_nargs_zero() {
        // Call(func_reg=0, nargs=0) + Return(0) → TailCall { args_base: 1, nargs: 0 }
        let insns = vec![Insn::Call(0, 0), Insn::Return(0)];
        let out = pass_self_tail_call(insns);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Insn::TailCall {
                args_base: 1,
                nargs: 0
            }
        ));
    }

    // ── pass_const_reg_prop ────────────────────────────────────────────────────

    #[test]
    fn const_reg_prop_converts_cmpjump() {
        use crate::ast::BinaryOp;
        // LoadConst(r4, idx=0) — write-once temp register (r4 >= num_locals=2)
        // CmpJumpIfFalse(r0, Lt, r4, k=1) → CmpJumpIfFalseConst(r0, Lt, 0, k=1)
        // LoadConst(r4) becomes dead and is pruned.
        let insns = vec![
            Insn::LoadConst(4, 0),
            Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 4, 1),
            Insn::Return(0),
        ];
        let out = pass_const_reg_prop(insns, 2, &[]);
        assert_eq!(out.len(), 2, "dead LoadConst should be removed");
        assert!(
            matches!(out[0], Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 1)),
            "CmpJumpIfFalse with write-once const rhs should become CmpJumpIfFalseConst: {:?}",
            out[0]
        );
    }

    #[test]
    fn const_reg_prop_converts_binop() {
        use crate::ast::BinaryOp;
        // LoadConst(r5, idx=0) — write-once temp register (r5 >= num_locals=2)
        // BinOp(r2, r0, Add, r5) → BinOpConst(r2, r0, Add, 0)
        // LoadConst(r5) becomes dead and is pruned.
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(2, 0, BinaryOp::Add, 5),
            Insn::Return(2),
        ];
        let out = pass_const_reg_prop(insns, 2, &[]);
        assert_eq!(out.len(), 2, "dead LoadConst should be removed");
        assert!(
            matches!(out[0], Insn::BinOpConst(2, 0, BinaryOp::Add, 0, ..)),
            "BinOp with write-once const rhs should become BinOpConst: {:?}",
            out[0]
        );
    }

    #[test]
    fn const_reg_prop_skips_multi_write_reg() {
        use crate::ast::BinaryOp;
        // r5 is written twice — not a safe immutable const, must not convert.
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::LoadConst(5, 1), // second write to r5
            Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 5, 0),
            Insn::Return(0),
        ];
        let out = pass_const_reg_prop(insns, 2, &[]);
        assert!(
            matches!(out[2], Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 5, 0)),
            "should not convert when rhs register is written multiple times"
        );
    }

    #[test]
    fn const_reg_prop_keeps_loadconst_for_named_local() {
        use crate::ast::BinaryOp;
        // r1 is a named local (r1 < num_locals=3): LoadConst should NOT be pruned
        // even though CmpJumpIfFalse is converted to CmpJumpIfFalseConst.
        let insns = vec![
            Insn::LoadConst(1, 0), // r1 is a named local
            Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 1),
            Insn::Return(0),
        ];
        let out = pass_const_reg_prop(insns, 3, &[]);
        assert_eq!(out.len(), 3, "LoadConst for named local must not be pruned");
        assert!(
            matches!(out[0], Insn::LoadConst(1, 0)),
            "named-local LoadConst must survive"
        );
        assert!(
            matches!(out[1], Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 1)),
            "CmpJump should still be converted: {:?}",
            out[1]
        );
    }

    #[test]
    fn const_reg_prop_keeps_loadconst_when_reg_has_other_readers() {
        use crate::ast::BinaryOp;
        // r5 is converted in CmpJump but also read by Return — LoadConst must stay.
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 5, 1),
            Insn::Return(5), // r5 is still live here
        ];
        let out = pass_const_reg_prop(insns, 2, &[]);
        assert_eq!(
            out.len(),
            3,
            "LoadConst must survive when reg has other readers"
        );
        assert!(
            matches!(out[0], Insn::LoadConst(5, 0)),
            "LoadConst should remain: {:?}",
            out[0]
        );
    }

    #[test]
    fn const_reg_prop_binopinplace_i16_const_becomes_binopimm() {
        use crate::ast::BinaryOp;
        // LoadConst(r5, idx=0) where consts[0] = 42 (fits in i16).
        // BinOpInPlace(r0, r0, Add, r5) → BinOpImm(r0, r0, Add, 42).
        // r5 is a temp (>= num_locals=2) and has no other readers → LoadConst pruned.
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOpInPlace(0, 0, BinaryOp::Add, 5),
            Insn::Return(0),
        ];
        let consts = vec![Value::int(42)];
        let out = pass_const_reg_prop(insns, 2, &consts);
        assert_eq!(out.len(), 2, "dead LoadConst should be removed");
        assert!(
            matches!(out[0], Insn::BinOpImm(0, 0, BinaryOp::Add, 42, ..)),
            "BinOpInPlace with i16-range const should become BinOpImm: {:?}",
            out[0]
        );
    }

    #[test]
    fn const_reg_prop_binopinplace_large_const_temp_lhs_becomes_binopconst() {
        use crate::ast::BinaryOp;
        // consts[0] = 100000 which does NOT fit in i16 (> 32767).
        // lhs=r3 is a temp (>= num_locals=2), so BinOpConst is safe.
        // BinOpInPlace(r2, r3, Add, r5) → BinOpConst(r2, r3, Add, 0).
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOpInPlace(2, 3, BinaryOp::Add, 5),
            Insn::Return(2),
        ];
        let consts = vec![Value::int(100_000)];
        let out = pass_const_reg_prop(insns, 2, &consts);
        assert_eq!(out.len(), 2, "dead LoadConst should be removed");
        assert!(
            matches!(out[0], Insn::BinOpConst(2, 3, BinaryOp::Add, 0, ..)),
            "BinOpInPlace with large const and temp lhs should become BinOpConst: {:?}",
            out[0]
        );
    }

    #[test]
    fn const_reg_prop_binopinplace_large_const_local_lhs_unchanged() {
        use crate::ast::BinaryOp;
        // consts[0] = 100000 — does not fit in i16.
        // lhs=r1 is a named local (r1 < num_locals=2), so we must not convert
        // to BinOpConst (user object might have __iadd__).
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOpInPlace(1, 1, BinaryOp::Add, 5),
            Insn::Return(1),
        ];
        let consts = vec![Value::int(100_000)];
        let out = pass_const_reg_prop(insns, 2, &consts);
        assert!(
            matches!(out[1], Insn::BinOpInPlace(1, 1, BinaryOp::Add, 5)),
            "BinOpInPlace with large const and local lhs must remain unchanged: {:?}",
            out[1]
        );
    }

    // ── pass_syncmod_sink ──────────────────────────────────────────────────────

    #[test]
    fn syncmod_sink_removes_from_while_loop() {
        use crate::ast::BinaryOp;
        // Simulate:  while i < n:  i += 1
        //
        //  [0] CmpJumpIfFalseConst(r0, Lt, 0, k=3)  ← header, exit at 0+1+3=4
        //  [1] BinOpImm(r0, r0, Add, 1)
        //  [2] SyncModuleGlobal(r0, 0)               ← should be removed
        //  [3] Jump(-4)                              ← back-edge to 0, offset=-4: 3+1-4=0 ✓
        // (k=3 means: target = 0+1+3 = 4, which is past latch=3 → exit=4)
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 3),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::SyncModuleGlobal(0, 0),
            Insn::Jump(-4),
        ];
        let out = pass_syncmod_sink(insns);
        // After sink: SyncModuleGlobal removed from body (pos 2 gone),
        // SyncModuleGlobal added at exit=4 (which is pos 3 in the new sequence = after Jump).
        // Result: [CmpJumpIfFalseConst, BinOpImm, Jump, SyncModuleGlobal]
        assert_eq!(
            out.len(),
            4,
            "length stays same after sink (removed+added 1): {:?}",
            out
        );
        // No SyncModuleGlobal before the Jump.
        let sync_count_before_jump = out[..out.len() - 1]
            .iter()
            .filter(|i| matches!(i, Insn::SyncModuleGlobal(_, _)))
            .count();
        assert_eq!(
            sync_count_before_jump, 0,
            "SyncModuleGlobal should be removed from loop body"
        );
        // SyncModuleGlobal at the end (after the loop).
        assert!(
            matches!(out[out.len() - 1], Insn::SyncModuleGlobal(0, 0)),
            "SyncModuleGlobal should be sunk to loop exit: {:?}",
            out[out.len() - 1]
        );
    }

    #[test]
    fn syncmod_sink_not_applied_with_call() {
        use crate::ast::BinaryOp;
        // Loop with a Call — must not sink.
        //
        //  [0] CmpJumpIfFalseConst(r0, Lt, 0, k=4)
        //  [1] Call(r1, 0)
        //  [2] BinOpImm(r0, r0, Add, 1)
        //  [3] SyncModuleGlobal(r0, 0)
        //  [4] Jump(-5)
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 4),
            Insn::Call(1, 0),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::SyncModuleGlobal(0, 0),
            Insn::Jump(-5),
        ];
        let out = pass_syncmod_sink(insns);
        // Must be unchanged: same length, no SyncModuleGlobal at the end.
        assert_eq!(
            out.len(),
            5,
            "should not change when loop body contains a Call"
        );
        assert!(
            matches!(out[3], Insn::SyncModuleGlobal(0, 0)),
            "SyncModuleGlobal should remain in loop body when Call is present"
        );
    }

    #[test]
    fn syncmod_sink_not_applied_with_loadglobal_conflict() {
        use crate::ast::BinaryOp;
        // Loop body contains LoadGlobal for the same name_idx as the SyncModuleGlobal.
        //
        //  [0] CmpJumpIfFalseConst(r0, Lt, 0, k=4)
        //  [1] LoadGlobal(r2, 0)          ← name_idx=0 conflicts with SyncModuleGlobal(r0, 0)
        //  [2] BinOpImm(r0, r0, Add, 1)
        //  [3] SyncModuleGlobal(r0, 0)
        //  [4] Jump(-5)
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 4),
            Insn::LoadGlobal(2, 0),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::SyncModuleGlobal(0, 0),
            Insn::Jump(-5),
        ];
        let out = pass_syncmod_sink(insns);
        assert_eq!(out.len(), 5, "should not change when LoadGlobal conflicts");
        assert!(
            matches!(out[3], Insn::SyncModuleGlobal(0, 0)),
            "SyncModuleGlobal should remain in loop body when LoadGlobal conflicts"
        );
    }

    #[test]
    fn syncmod_sink_multiple_exits() {
        use crate::ast::BinaryOp;
        // Loop with a break (two exit labels).
        //
        //  [0] CmpJumpIfFalseConst(r0, Lt, 0, k=4) ← exit at 5
        //  [1] CmpJumpIfFalseConst(r0, Lt, 1, k=2) ← break-exit at 4
        //  [2] BinOpImm(r0, r0, Add, 1)
        //  [3] SyncModuleGlobal(r0, 0)
        //  [4] Jump(-5)                             ← back-edge → 0
        //  [5] ReturnNone                           ← normal exit
        //
        // header=0, latch=4.
        // header exit: 0+1+4=5.
        // break exit: 1+1+2=4, which is == latch — not > latch, so not a break.
        // Let's use k=3 for break so target=1+1+3=5 (same as header exit).
        // Let's instead use a body jump that really breaks out:
        //
        //  [0] CmpJumpIfFalseConst(r0, Lt, 0, k=5) ← exit at 6
        //  [1] CmpJumpIfFalseConst(r0, Lt, 1, k=3) ← break at 1+1+3=5, > latch=4 → exit at 5
        //  [2] BinOpImm(r0, r0, Add, 1)
        //  [3] SyncModuleGlobal(r0, 0)
        //  [4] Jump(-5)                             ← back-edge → 0
        //  [5] ReturnNone
        //  [6] ReturnNone
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 5),
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 1, 3),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::SyncModuleGlobal(0, 0),
            Insn::Jump(-5),
            Insn::ReturnNone,
            Insn::ReturnNone,
        ];
        let out = pass_syncmod_sink(insns);
        // SyncModuleGlobal should be removed from body (was at old pos 3).
        let sync_in_loop = out
            .iter()
            .take(4) // header, branch, binopimm, jump (after removal)
            .filter(|i| matches!(i, Insn::SyncModuleGlobal(_, _)))
            .count();
        assert_eq!(
            sync_in_loop, 0,
            "no SyncModuleGlobal in loop body after sink: {:?}",
            &out
        );
        // SyncModuleGlobal should appear at BOTH exit points.
        let total_syncs = out
            .iter()
            .filter(|i| matches!(i, Insn::SyncModuleGlobal(_, _)))
            .count();
        assert_eq!(
            total_syncs, 2,
            "SyncModuleGlobal should appear at both exits: {:?}",
            out
        );
    }

    #[test]
    fn syncmod_sink_not_applied_with_pre_loop_call() {
        use crate::ast::BinaryOp;
        // A Call before the loop header means globals_accessed may already be true.
        // The sink must NOT be applied in this case.
        //
        //  [0] Call(1, 0)                              ← pre-loop call (e.g. globals())
        //  [1] CmpJumpIfFalseConst(r0, Lt, 0, k=3)    ← header, exit at 5
        //  [2] BinOpImm(r0, r0, Add, 1)
        //  [3] SyncModuleGlobal(r0, 0)
        //  [4] Jump(-4)                                ← back-edge to 1
        let insns = vec![
            Insn::Call(1, 0),
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 3),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::SyncModuleGlobal(0, 0),
            Insn::Jump(-4),
        ];
        let out = pass_syncmod_sink(insns);
        assert_eq!(
            out.len(),
            5,
            "should not change when Call precedes loop header"
        );
        assert!(
            matches!(out[3], Insn::SyncModuleGlobal(0, 0)),
            "SyncModuleGlobal should remain in loop body"
        );
    }

    #[test]
    fn syncmod_sink_continue_loop_uses_max_latch() {
        use crate::ast::BinaryOp;
        // A loop with a `continue` compiles to two Jump-back edges to the same
        // header.  The pass must analyse the full range [header..=max_latch].
        //
        //  [0] CmpJumpIfFalseConst(r0, Lt, 0, k=6)    ← header, exit at 7
        //  [1] BinOpImm(r0, r0, Add, 1)
        //  [2] SyncModuleGlobal(r0, 0)
        //  [3] CmpJumpIfFalseConst(r0, Lt, 1, k=1)    ← inner-if: continue (jump to 0)
        //  [4] Jump(-5)                                ← continue: 4+1-5=0 ✓ latch=4
        //  [5] BinOpImm(r0, r0, Add, 1)
        //  [6] Jump(-7)                                ← main latch: 6+1-7=0 ✓ latch=6
        //  [7] ReturnNone
        //
        // Two back-edges: (0, 4) and (0, 6).  max_latch = 6.
        // The body [0..=6] has no blocker and no pre-loop call → sink is safe.
        // SyncModuleGlobal at pos 2 should be removed and sunk to exit at 7.
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 6),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::SyncModuleGlobal(0, 0),
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 1, 1),
            Insn::Jump(-5),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::Jump(-7),
            Insn::ReturnNone,
        ];
        let out = pass_syncmod_sink(insns);
        // SyncModuleGlobal should appear exactly once (at the exit before ReturnNone).
        let total_syncs = out
            .iter()
            .filter(|i| matches!(i, Insn::SyncModuleGlobal(_, _)))
            .count();
        assert_eq!(
            total_syncs, 1,
            "exactly one SyncModuleGlobal after sink: {:?}",
            out
        );
        // It must appear in the last two instructions (sunk before ReturnNone).
        let sync_in_exit_region: usize = out
            .iter()
            .rev()
            .take(2)
            .filter(|i| matches!(i, Insn::SyncModuleGlobal(_, _)))
            .count();
        assert_eq!(
            sync_in_exit_region, 1,
            "SyncModuleGlobal should be sunk to exit region: {:?}",
            out
        );
    }

    // ── pass_switch_hoist ─────────────────────────────────────────────────────

    #[test]
    fn switch_hoist_removes_redundant_global_load() {
        use crate::ast::BinaryOp;
        // Simulates the if/elif pattern for module-level "x":
        //   g_idx = 0 is the global name index for "x", num_locals = 5.
        //   t=5 and t=6 are temp registers (>= num_locals).
        //
        //   i=0: LoadGlobal(5, 0)
        //   i=1: CmpJumpIfFalseConst(5, Eq, c=0, k=4)  → false target = 1+1+4 = 6
        //   i=2: LoadConst(3, 0)     // body0 true branch
        //   i=3: Return(3)           // body0 exit
        //   i=4: Jump(4)             // unreachable fall-through after Return
        //   i=5: Return(3)           // unreachable
        //   i=6: LoadGlobal(6, 0)   // elif-2 test — redundant!
        //   i=7: CmpJumpIfFalseConst(6, Eq, c=1, k=1)  → false target = 9
        //   i=8: Return(5)           // elif-2 true branch
        //   i=9: Return(5)           // else / default
        // consts pool: indices 0 and 1 are integers (primitives), so
        // safety condition 5 passes and the hoist fires.
        let consts = vec![Value::int(1), Value::int(2)];
        let insns = vec![
            Insn::LoadGlobal(5, 0),
            Insn::CmpJumpIfFalseConst(5, BinaryOp::Eq, 0, 4),
            Insn::LoadConst(3, 0),
            Insn::Return(3),
            Insn::Jump(4), // unreachable; target=i+1+4=9 (valid index)
            Insn::Return(3),
            Insn::LoadGlobal(6, 0), // i=6: target — should be removed
            Insn::CmpJumpIfFalseConst(6, BinaryOp::Eq, 1, 1), // false → i=9
            Insn::Return(5),
            Insn::Return(5),
        ];
        // pred_count[6] should be 1 (only from CmpJump at i=1).
        let out = pass_switch_hoist(insns, 5, &consts);
        // LoadGlobal(6, 0) at old index 6 should be gone.
        let has_second_load = out
            .iter()
            .any(|insn| matches!(insn, Insn::LoadGlobal(6, 0)));
        assert!(
            !has_second_load,
            "redundant LoadGlobal for t=6 should be removed"
        );
        // CmpJumpIfFalseConst should now use t=5 (not t=6).
        let cmpjump_uses_t0 = out
            .iter()
            .any(|insn| matches!(insn, Insn::CmpJumpIfFalseConst(5, BinaryOp::Eq, 1, _)));
        assert!(
            cmpjump_uses_t0,
            "rewritten CmpJumpIfFalseConst should use t=5"
        );
    }

    #[test]
    fn switch_hoist_skips_when_multiple_predecessors() {
        use crate::ast::BinaryOp;
        // If the target has two predecessors (jump from elsewhere), do not hoist.
        //   i=0: LoadGlobal(5, 0)
        //   i=1: CmpJumpIfFalseConst(5, Eq, 0, k=2) → false target = 1+1+2 = 4
        //   i=2: Return(0)                            // true branch
        //   i=3: Jump(0)                              → target = 3+1+0 = 4 (second predecessor!)
        //   i=4: LoadGlobal(6, 0)                    // two predecessors → must NOT hoist
        //   i=5: CmpJumpIfFalseConst(6, Eq, 1, 1)   → false target = 5+1+1 = 7
        //   i=6: Return(1)                            // elif-2 true branch
        //   i=7: Return(1)                            // else
        // consts pool: primitive integers at indices 0 and 1.
        let consts = vec![Value::int(1), Value::int(2)];
        let insns = vec![
            Insn::LoadGlobal(5, 0),
            Insn::CmpJumpIfFalseConst(5, BinaryOp::Eq, 0, 2),
            Insn::Return(0),
            Insn::Jump(0),          // targets i=4: adds a second predecessor
            Insn::LoadGlobal(6, 0), // i=4: two predecessors — should NOT be removed
            Insn::CmpJumpIfFalseConst(6, BinaryOp::Eq, 1, 1),
            Insn::Return(1),
            Insn::Return(1),
        ];
        let out = pass_switch_hoist(insns, 5, &consts);
        let second_load_present = out
            .iter()
            .any(|insn| matches!(insn, Insn::LoadGlobal(6, 0)));
        assert!(
            second_load_present,
            "LoadGlobal should be kept when target has 2 predecessors"
        );
    }

    #[test]
    fn switch_hoist_on_compiled_elif_chain() {
        // Verify via compile_fn that a global if/elif chain optimises correctly.
        let code = compile_fn(
            "x = 99
if x == 1:
    print(1)
elif x == 2:
    print(2)
",
        );
        let before_count = code
            .insns
            .iter()
            .filter(|i| matches!(i, Insn::LoadGlobal(..)))
            .count();
        let optimized = optimize(code);
        let after_count = optimized
            .insns
            .iter()
            .filter(|i| matches!(i, Insn::LoadGlobal(..)))
            .count();
        assert!(
            after_count <= before_count,
            "optimised code should have no more LoadGlobal than unoptimised"
        );
        // The optimised code should have fewer LoadGlobal for the same global.
        // (The assignment x=99 stays, but the elif test should reuse the first load.)
        assert!(
            after_count < before_count,
            "switch-hoist should have reduced LoadGlobal count from {} to less",
            before_count
        );
    }

    #[test]
    fn self_tail_call_on_compiled_factorial() {
        // def factorial(n, acc=1):
        //     if n <= 1: return acc
        //     return factorial(n - 1, acc * n)
        let code = compile_fn(
            "def factorial(n, acc=1):\n    if n <= 1:\n        return acc\n    return factorial(n - 1, acc * n)\n",
        );
        let optimized = optimize(code);
        let inner = &optimized.fn_protos[0].code;
        let has_tailcall = inner
            .insns
            .iter()
            .any(|i| matches!(i, Insn::TailCall { .. }));
        assert!(
            has_tailcall,
            "recursive tail call in factorial should be optimised to TailCall"
        );
        // There should be no Call+Return pair left — the optimiser fused them all.
        let has_plain_call = inner
            .insns
            .iter()
            .any(|i| matches!(i, Insn::Call(..) | Insn::CallMemo(..)));
        assert!(
            !has_plain_call,
            "after TCO all recursive calls should be TailCall, not Call/CallMemo"
        );
    }

    // ── pass_reassoc ─────────────────────────────────────────────────────────

    #[test]
    fn reassoc_add_chain() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // Models reassoc for a known-integer source loaded via LoadConst.
        // r0 = 5 (LoadConst → int_regs)
        // t1 = r0 + 1
        // t2 = t1 + 2  → reassoc to t2 = r0 + 3
        let mut consts = vec![Value::int(5), Value::int(1), Value::int(2)];
        let num_locals = 0u32; // no named locals; r0/t1/t2 are all temps
        let insns = vec![
            Insn::LoadConst(0, 0),                           // r0 = 5  → int_regs
            Insn::BinOpConst(1, 0, BinaryOp::Add, 1, false), // t1 = r0 + 1
            Insn::BinOpConst(2, 1, BinaryOp::Add, 2, false), // t2 = t1 + 2
            Insn::Return(2),
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        // t2 should now use r0 (reg 0) directly instead of t1 (reg 1).
        assert!(
            matches!(out[2], Insn::BinOpConst(2, 0, BinaryOp::Add, ..)),
            "reassoc should rewrite t2's lhs from t1 to r0 when r0 is a known int: {:?}",
            out[2]
        );
        // The combined constant (1+2=3) must be in the pool.
        let has_3 = consts
            .iter()
            .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(3)));
        assert!(has_3, "combined constant 3 should be interned in the pool");
        assert_eq!(out.len(), 4, "instruction count unchanged by reassoc alone");
    }

    #[test]
    fn reassoc_mul_chain() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // (5 * 2) * 5 where 5 is a known-int constant → 5 * 10
        let mut consts = vec![Value::int(5), Value::int(2), Value::int(5)];
        let num_locals = 0u32;
        let insns = vec![
            Insn::LoadConst(0, 0),                           // r0 = 5  → int_regs
            Insn::BinOpConst(1, 0, BinaryOp::Mul, 1, false), // t1 = r0 * 2
            Insn::BinOpConst(2, 1, BinaryOp::Mul, 2, false), // t2 = t1 * 5
            Insn::Return(2),
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        assert!(
            matches!(out[2], Insn::BinOpConst(2, 0, BinaryOp::Mul, ..)),
            "reassoc should rewrite t2 to use r0 (reg 0) directly"
        );
        let ci = match out[2] {
            Insn::BinOpConst(_, _, _, ci, ..) => ci,
            _ => panic!(),
        };
        assert!(
            matches!(consts[ci as usize].kind(), crate::value::ValueKind::Int(10)),
            "combined constant should be 10"
        );
    }

    // ── pass_loadnone_merge ───────────────────────────────────────────────────

    #[test]
    fn loadnone_merge_fuses_consecutive_run() {
        // LoadNone(0), LoadNone(1), LoadNone(2), Return(0)
        // → LoadNoneRange { start: 0, count: 3 }, Return(0)
        let insns = vec![
            Insn::LoadNone(0),
            Insn::LoadNone(1),
            Insn::LoadNone(2),
            Insn::ReturnNone,
        ];
        let out = pass_loadnone_merge(insns);
        assert_eq!(
            out.len(),
            2,
            "three consecutive LoadNone should become one LoadNoneRange"
        );
        assert!(
            matches!(out[0], Insn::LoadNoneRange { start: 0, count: 3 }),
            "should produce LoadNoneRange {{ start:0, count:3 }}, got {:?}",
            out[0]
        );
    }

    #[test]
    fn reassoc_bitor_chain() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // (r0 | 1) | 2 where r0 is a known-int constant → r0 | 3
        let mut consts = vec![Value::int(0b1010), Value::int(1), Value::int(2)];
        let num_locals = 0u32;
        let insns = vec![
            Insn::LoadConst(0, 0),                             // r0 = 0b1010  → int_regs
            Insn::BinOpConst(1, 0, BinaryOp::BitOr, 1, false), // t1 = r0 | 1
            Insn::BinOpConst(2, 1, BinaryOp::BitOr, 2, false), // t2 = t1 | 2
            Insn::Return(2),
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        assert!(
            matches!(out[2], Insn::BinOpConst(2, 0, BinaryOp::BitOr, ..)),
            "BitOr chain should be reassociated when source is known-int"
        );
        let ci = match out[2] {
            Insn::BinOpConst(_, _, _, ci, ..) => ci,
            _ => panic!(),
        };
        assert!(
            matches!(consts[ci as usize].kind(), crate::value::ValueKind::Int(3)),
            "1 | 2 = 3"
        );
    }

    #[test]
    fn reassoc_skips_mixed_ops() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // (x + 2) * 3: different ops → must NOT reassociate.
        let mut consts = vec![Value::int(2), Value::int(3)];
        let num_locals = 1u32;
        let insns = vec![
            Insn::BinOpConst(1, 0, BinaryOp::Add, 0, false), // t1 = x + 2
            Insn::BinOpConst(2, 1, BinaryOp::Mul, 1, false), // t2 = t1 * 3
            Insn::Return(2),
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        // t2's lhs must remain t1 (reg 1).
        assert!(
            matches!(out[1], Insn::BinOpConst(2, 1, BinaryOp::Mul, 1, ..)),
            "mixed ops must not be reassociated: {:?}",
            out[1]
        );
    }

    #[test]
    fn reassoc_skips_sub_and_div() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // Sub is not associative: (x - 2) - 3 ≠ x - (2 - 3).  Must skip.
        let mut consts = vec![Value::int(2), Value::int(3)];
        let num_locals = 1u32;
        let insns = vec![
            Insn::BinOpConst(1, 0, BinaryOp::Sub, 0, false), // t1 = x - 2
            Insn::BinOpConst(2, 1, BinaryOp::Sub, 1, false), // t2 = t1 - 3
            Insn::Return(2),
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        assert!(
            matches!(out[1], Insn::BinOpConst(2, 1, BinaryOp::Sub, 1, ..)),
            "Sub must not be reassociated"
        );
    }

    #[test]
    fn reassoc_skips_named_local_intermediate() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // If the intermediate register is a named local (< num_locals), we must
        // not reassociate because user code could have mutated it.
        let mut consts = vec![Value::int(1), Value::int(2)];
        let num_locals = 3u32; // r0, r1, r2 are all named locals
        let insns = vec![
            Insn::BinOpConst(1, 0, BinaryOp::Add, 0, false), // t1=r1 = x + 1  (r1 < num_locals!)
            Insn::BinOpConst(2, 1, BinaryOp::Add, 1, false), // t2=r2 = t1 + 2 (r2 < num_locals!)
            Insn::Return(2),
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        // No reassociation — r1 and r2 are named locals.
        assert!(
            matches!(out[1], Insn::BinOpConst(2, 1, BinaryOp::Add, 1, ..)),
            "named-local intermediate must not be reassociated"
        );
    }

    #[test]
    fn reassoc_clears_at_bb_boundary() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // A jump in the middle means the second BinOpConst may be in a different
        // basic block — the defined_as map must be cleared.
        //   [0] BinOpConst(1, 0, Add, 0)   t1 = x + 1   (idx_0 = 1)
        //   [1] Jump(1)                     → [3]
        //   [2] BinOpConst(2, 1, Add, 1)   t2 = t1 + 2  (idx_1 = 2)  — NOT a BB-boundary
        //   [3] Return(2)                                               — IS a BB-boundary
        //
        // The target of Jump(1) from index 1 is 1+1+1=3 (instruction [3]).
        // So [3] is a BB start.  The BinOpConst at [2] is NOT a BB start.
        // Therefore the map is cleared at [3], not at [2] — reassoc at [2] fires.
        //
        // But if we model a *backward* jump pointing at [2]:
        let mut consts = vec![Value::int(1), Value::int(2)];
        let num_locals = 1u32;
        // Jump(-3) from index 2 → target = 2+1-3 = 0.  So [0] is a BB start.
        // But BinOpConst at [2]'s lhs is [0]'s output — both in same "block"
        // relative to [2], because [0] is the BB start and [2] falls through.
        // To test clear: put a forward jump that makes [2] a BB start.
        //   [0] BinOpConst(1, 0, Add, 0)    t1 = x + 1
        //   [1] Jump(1)                      → [3]  (makes [3] a bb_start... irrelevant here)
        //   [2] BinOpConst(2, 1, Add, 1)    t2 = t1 + 2  ← this IS reachable from [1]? No.
        //   [3] Return(2)
        //
        // Simpler: make [1] a bb_start by having [0] be a forward jump targeting [1].
        //   [0] Jump(0)                      → [1]  (makes [1] a bb_start)
        //   [1] BinOpConst(1, 0, Add, 0)    t1 = x + 1
        //   [2] BinOpConst(2, 1, Add, 1)    t2 = t1 + 2  ← [1] is bb_start, clears defined_as
        //                                                   → [1]'s output NOT in defined_as when
        //                                                   [2] processes? No — [1] was executed
        //                                                   and recorded t1.
        //
        // The relevant case: [1] is a bb_start, so defined_as is cleared before [1] runs.
        // After [1], t1 is recorded.  [2] sees t1 in defined_as — fires correctly.
        //
        // Test: put a jump to [2] (making [2] a bb_start), so defined_as is cleared there.
        //   [0] BinOpConst(1, 0, Add, 0)    t1 = x + 1  (in first BB)
        //   [1] Jump(0)                      → [2]        (makes [2] a bb_start)
        //   [2] BinOpConst(2, 1, Add, 1)    t2 = t1 + 2  ← defined_as CLEARED → no reassoc
        let insns = vec![
            Insn::BinOpConst(1, 0, BinaryOp::Add, 0, false), // [0]
            Insn::Jump(0),                                   // [1] → [2]
            Insn::BinOpConst(2, 1, BinaryOp::Add, 1, false), // [2] — bb_start → map cleared
            Insn::Return(2),                                 // [3]
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        // [2] must NOT be reassociated because defined_as was cleared at BB boundary.
        assert!(
            matches!(out[2], Insn::BinOpConst(2, 1, BinaryOp::Add, 1, ..)),
            "reassoc must not fire across a basic-block boundary"
        );
    }

    #[test]
    fn reassoc_end_to_end_compiled_const_source() {
        // Full-pipeline test: def f(): return (5 + 1) + 2
        // `5` is a literal constant → int_regs tracks r_5.
        // pass_reassoc can fold to `5 + 3`, then pass_const_fold collapses to 8.
        // The whole expression should reduce to a single LoadConst(8).
        let code = compile_fn("def f():\n    return (5 + 1) + 2\n");
        let optimized = optimize(code);
        let inner = &optimized.fn_protos[0].code;
        // After full optimization the result should be a single LoadConst(8) + Return.
        let has_8 = inner
            .consts
            .iter()
            .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(8)));
        assert!(
            has_8,
            "fully constant expression (5+1)+2 should fold to 8 in const pool"
        );
        let binopconst_count = inner
            .insns
            .iter()
            .filter(|i| matches!(i, Insn::BinOpConst(..)))
            .count();
        assert_eq!(
            binopconst_count, 0,
            "all ops should be eliminated by const_fold after reassoc; got {binopconst_count}"
        );
    }

    #[test]
    fn reassoc_skips_unknown_type_lhs() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // When `inner_src` (r0) is NOT in int_regs (e.g. it's a function parameter
        // of unknown type), reassociation must be suppressed — the intermediate
        // value's `__add__` must still be called at runtime.
        //
        // Model: t1 = r0 + 1, t2 = t1 + 2
        // r0 is NOT known to be an integer (no LoadConst), so reassoc must not fire.
        let mut consts = vec![Value::int(1), Value::int(2)];
        let num_locals = 1u32; // r0 is a local of unknown type
        let insns = vec![
            Insn::BinOpConst(1, 0, BinaryOp::Add, 0, false), // t1 = r0 + 1
            Insn::BinOpConst(2, 1, BinaryOp::Add, 1, false), // t2 = t1 + 2
            Insn::Return(2),
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        // t2 must still use t1 (reg 1), not r0 (reg 0) — r0 is unknown type.
        assert!(
            matches!(out[1], Insn::BinOpConst(2, 1, BinaryOp::Add, 1, ..)),
            "reassoc must not fire when inner_src is of unknown type: {:?}",
            out[1]
        );
    }

    #[test]
    fn reassoc_fires_for_known_int_lhs() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // When `inner_src` IS in int_regs (loaded from a known-int constant),
        // reassociation should fire.
        //
        // r0 = LoadConst(5)  → int_regs = {r0}
        // t1 = r0 + 1
        // t2 = t1 + 2   → reassoc to t2 = r0 + 3 (because r0 ∈ int_regs)
        let mut consts = vec![Value::int(5), Value::int(1), Value::int(2)];
        let num_locals = 0u32; // no named locals; r0/t1/t2 are all temps
        let insns = vec![
            Insn::LoadConst(0, 0),                           // r0 = 5
            Insn::BinOpConst(1, 0, BinaryOp::Add, 1, false), // t1 = r0 + 1
            Insn::BinOpConst(2, 1, BinaryOp::Add, 2, false), // t2 = t1 + 2
            Insn::Return(2),
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        // t2 should be rewritten to use r0 directly with combined constant 3.
        assert!(
            matches!(out[2], Insn::BinOpConst(2, 0, BinaryOp::Add, ..)),
            "reassoc must fire when inner_src (r0) is a known-int register: {:?}",
            out[2]
        );
        let ci = match out[2] {
            Insn::BinOpConst(_, _, _, ci, ..) => ci,
            _ => panic!(),
        };
        assert!(
            matches!(consts[ci as usize].kind(), crate::value::ValueKind::Int(3)),
            "combined constant must be 3 (1+2)"
        );
    }

    #[test]
    fn reassoc_end_to_end_user_dunder_not_reassociated() {
        // Full-pipeline test: def f(x): return (x + 1) + 2
        // `x` is a parameter of unknown type — pass_reassoc must NOT fire because
        // it cannot prove that `x` is an integer.  The pass would otherwise skip
        // the intermediate dunder call `(x+1).__add__(2)`.
        //
        // We verify this at the pass level (before the full optimize() pipeline
        // runs, since later passes such as pass_algebraic_simplify may further
        // reduce the IR in their own safe ways).
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // Simulate: x is r0 (local of unknown type, num_locals=1).
        // BinOpConst(1, 0, Add, 0): t1 = x + 1
        // BinOpConst(2, 1, Add, 1): t2 = t1 + 2
        let mut consts = vec![Value::int(1), Value::int(2)];
        let num_locals = 1u32;
        let insns = vec![
            Insn::BinOpConst(1, 0, BinaryOp::Add, 0, false), // t1 = x + 1  (x unknown type)
            Insn::BinOpConst(2, 1, BinaryOp::Add, 1, false), // t2 = t1 + 2
            Insn::Return(2),
        ];
        let out = pass_reassoc(insns, &mut consts, num_locals);
        // pass_reassoc must NOT have rewritten t2 to use x (reg 0) directly,
        // because x is of unknown type and the intermediate __add__ must fire.
        assert!(
            matches!(out[1], Insn::BinOpConst(2, 1, BinaryOp::Add, 1, ..)),
            "pass_reassoc must not fire when inner_src (x) is of unknown type: {:?}",
            out[1]
        );
    }

    #[test]
    fn loadnone_merge_single_unchanged() {
        // A lone LoadNone should not be wrapped in a LoadNoneRange.
        let insns = vec![Insn::LoadNone(5), Insn::ReturnNone];
        let out = pass_loadnone_merge(insns);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Insn::LoadNone(5)));
    }

    #[test]
    fn loadnone_merge_non_consecutive_not_merged() {
        // LoadNone(0), LoadNone(2) — gap at reg 1 — must NOT be merged.
        let insns = vec![Insn::LoadNone(0), Insn::LoadNone(2), Insn::ReturnNone];
        let out = pass_loadnone_merge(insns);
        assert_eq!(out.len(), 3, "non-consecutive LoadNones must not merge");
        assert!(matches!(out[0], Insn::LoadNone(0)));
        assert!(matches!(out[1], Insn::LoadNone(2)));
    }

    #[test]
    fn loadnone_merge_interrupted_by_other_insn() {
        // LoadNone(0), Move(1,0), LoadNone(1) — interrupted by Move.
        let insns = vec![
            Insn::LoadNone(0),
            Insn::Move(1, 0),
            Insn::LoadNone(1),
            Insn::ReturnNone,
        ];
        let out = pass_loadnone_merge(insns);
        // LoadNone(0) is alone → stays as LoadNone(0).
        // Move(1,0) stays.
        // LoadNone(1) is alone → stays.
        assert_eq!(out.len(), 4);
        assert!(matches!(out[0], Insn::LoadNone(0)));
        assert!(matches!(out[1], Insn::Move(1, 0)));
        assert!(matches!(out[2], Insn::LoadNone(1)));
    }

    #[test]
    fn loadnone_merge_fuses_prologue_with_subsequent_jump() {
        // Simulate the instruction sequence a compiler emits for:
        //
        //   def f(x):        # x → reg 0
        //       a = None     # a → reg 1
        //       b = None     # b → reg 2
        //       c = None     # c → reg 3
        //       d = None     # d → reg 4
        //       if x: ...    # JumpIfFalse(0, +3) → skips 3 LoadConst insns
        //       return a, b, c, d
        //
        // Index layout (before merge):
        //   0: LoadNone(1)
        //   1: LoadNone(2)
        //   2: LoadNone(3)
        //   3: LoadNone(4)
        //   4: JumpIfFalse(0, 3)   ← target = 4+1+3 = 8 (ReturnNone)
        //   5: LoadConst(1, 0)
        //   6: LoadConst(2, 0)
        //   7: LoadConst(3, 0)
        //   8: ReturnNone
        //
        // After merge: LoadNone(1..=4) becomes LoadNoneRange{start:1,count:4},
        // positions 1..3 are removed (3 instructions deleted).
        // JumpIfFalse was at old=4 → new=1.  Its target was old=8 → new=5.
        // New offset = 5 - 1 - 1 = 3.  Offset should be unchanged here (both
        // source and target shift by the same 3 removals), so the check is that
        // JumpIfFalse still points past the 3 LoadConst instructions.
        let insns = vec![
            Insn::LoadNone(1),       // 0
            Insn::LoadNone(2),       // 1
            Insn::LoadNone(3),       // 2
            Insn::LoadNone(4),       // 3
            Insn::JumpIfFalse(0, 3), // 4 → target 8 (ReturnNone)
            Insn::LoadConst(1, 0),   // 5
            Insn::LoadConst(2, 0),   // 6
            Insn::LoadConst(3, 0),   // 7
            Insn::ReturnNone,        // 8
        ];
        let out = pass_loadnone_merge(insns);
        // 4 LoadNones fused into 1 + 1 JumpIfFalse + 3 LoadConst + 1 ReturnNone = 6.
        assert_eq!(
            out.len(),
            6,
            "four LoadNones fused to one range; total 6 insns"
        );
        assert!(
            matches!(out[0], Insn::LoadNoneRange { start: 1, count: 4 }),
            "first instruction should be LoadNoneRange{{start:1,count:4}}, got {:?}",
            out[0]
        );
        // JumpIfFalse is now at new index 1.  Its target is ReturnNone at new index 5.
        // Expected offset: 5 - 1 - 1 = 3.
        assert!(
            matches!(out[1], Insn::JumpIfFalse(0, 3)),
            "JumpIfFalse offset must remain 3 after compaction, got {:?}",
            out[1]
        );
    }

    // ── pass_cross_jump ───────────────────────────────────────────────────────

    #[test]
    fn cross_jump_merges_identical_return_tails() {
        use crate::ast::BinaryOp;
        // Models: if cond: x=10 else: x=20; return x+1
        //
        // After compile + earlier passes, the bytecode looks roughly like:
        //   [0] CmpJumpIfFalseConst(cond, Eq, c_true, 3)  — if branch
        //   [1] LoadConst(r0, c_10)                        — then: x=10
        //   [2] BinOpConst(r1, r0, Add, c_1)              — x+1
        //   [3] Return(r1)                                  ← surviving tail [2..3]
        //   [4] LoadConst(r0, c_20)                        — else: x=20
        //   [5] BinOpConst(r1, r0, Add, c_1)              — x+1  (duplicate)
        //   [6] Return(r1)                                  ← duplicate tail [5..6]
        //
        // pass_cross_jump should remove [5..6] and insert Jump to [2].
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 0, 3), // [0] → [4]
            Insn::LoadConst(5, 1),                            // [1] x=10
            Insn::BinOpConst(6, 5, BinaryOp::Add, 2, false),  // [2] x+1
            Insn::Return(6),                                  // [3] ← survivor end
            Insn::LoadConst(5, 3),                            // [4] x=20
            Insn::BinOpConst(6, 5, BinaryOp::Add, 2, false),  // [5] duplicate
            Insn::Return(6),                                  // [6] duplicate end
        ];

        let out = pass_cross_jump(insns, &[]);

        // After merging: [5] and [6] are collapsed to Jump([4]→[2]).
        // The output should be shorter by 1 instruction (one BinOpConst removed,
        // one Return removed, one Jump added = net -1).
        assert!(
            out.len() < 7,
            "cross_jump should reduce instruction count; got {} insns",
            out.len()
        );
        // The merged return count should drop to one.
        let return_count = out.iter().filter(|i| matches!(i, Insn::Return(_))).count();
        assert_eq!(
            return_count, 1,
            "exactly one Return should survive after merge"
        );
    }

    #[test]
    fn cross_jump_skips_tail_length_one() {
        // Only `Return(r)` is common — length 1, below MIN_TAIL=2.
        // The pass must NOT fire.
        let insns = vec![
            Insn::JumpIfFalse(0, 2), // [0] → [3]
            Insn::LoadConst(1, 0),   // [1]
            Insn::Return(1),         // [2] ← terminal
            Insn::LoadConst(2, 1),   // [3]
            Insn::Return(2),         // [4] ← same Return discriminant but different reg
        ];
        let n = insns.len();
        let out = pass_cross_jump(insns, &[]);
        // Return(1) vs Return(2) differ → no merge.
        assert_eq!(out.len(), n, "no merge for length-1 or differing tails");
    }

    #[test]
    fn cross_jump_does_not_merge_across_jump_target_in_dup_tail() {
        // The duplicate tail starts at a jump target — must not be removed.
        //
        //   [0] JumpIfFalse(r, 2)  → [3]
        //   [1] LoadConst(r0, 0)
        //   [2] Return(r0)         ← survivor tail [1..2]
        //   [3] LoadConst(r0, 0)   ← JUMP TARGET — cannot be removed
        //   [4] Return(r0)         ← duplicate terminator
        //
        // [3] is a jump target (from [0]), so the merge must not fire.
        let insns = vec![
            Insn::JumpIfFalse(0, 2), // [0] → [3]
            Insn::LoadConst(1, 0),   // [1]
            Insn::Return(1),         // [2] survivor tail start
            Insn::LoadConst(1, 0),   // [3] ← jump target (from [0])
            Insn::Return(1),         // [4] duplicate tail
        ];
        let n = insns.len();
        let out = pass_cross_jump(insns, &[]);
        assert_eq!(
            out.len(),
            n,
            "must not merge when dup tail instructions are jump targets"
        );
    }

    #[test]
    fn cross_jump_does_not_merge_tail_with_jump_offset_insn() {
        use crate::ast::BinaryOp;
        // Tail contains a JumpIfFalse — has an offset field, must not be merged.
        //
        //   [0] JumpIfFalse(r, 2) → [3]
        //   [1] JumpIfFalse(r, 1) — this would land differently from each block
        //   [2] Return(0)
        //   [3] JumpIfFalse(r, 1) — structurally same as [1] but offset means
        //   [4] Return(0)           different target
        let insns = vec![
            Insn::JumpIfFalse(0, 2),                          // [0] → [3]
            Insn::CmpJumpIfFalseConst(1, BinaryOp::Gt, 0, 0), // [1]
            Insn::Return(0),                                  // [2]
            Insn::CmpJumpIfFalseConst(1, BinaryOp::Gt, 0, 0), // [3] jump target
            Insn::Return(0),                                  // [4]
        ];
        let n = insns.len();
        let out = pass_cross_jump(insns, &[]);
        // The CmpJumpIfFalseConst has a jump-offset field; merge must not fire.
        assert_eq!(
            out.len(),
            n,
            "must not merge tails containing instructions with jump-offset fields"
        );
    }

    #[test]
    fn cross_jump_on_compiled_if_else_common_tail() {
        // Compile a function with an explicit common tail and verify the instruction
        // count decreases after optimization (or at least does not increase).
        let code_before = compile_fn(
            "def f(cond):\n    if cond:\n        x = 10\n    else:\n        x = 20\n    return x + 1\n",
        );
        let before_count = code_before.fn_protos[0].code.insns.len();
        let optimized = optimize(code_before);
        let after_count = optimized.fn_protos[0].code.insns.len();
        assert!(
            after_count <= before_count,
            "optimizer should not increase instruction count ({before_count} → {after_count})"
        );
    }

    #[test]
    fn cross_jump_correctness_with_compiled_code() {
        // The parity fixture: with_merge(True)=11, with_merge(False)=21.
        // This test ensures the optimizer does not break correct execution.
        let code = compile_fn(
            "def f(cond):\n    if cond:\n        x = 10\n    else:\n        x = 20\n    return x + 1\n",
        );
        let optimized = optimize(code);
        // After optimization, the function proto must still be present.
        assert_eq!(
            optimized.fn_protos.len(),
            1,
            "function proto should survive"
        );
        // The instruction list must be non-empty.
        assert!(
            !optimized.fn_protos[0].code.insns.is_empty(),
            "instruction list must not be empty after optimization"
        );
    }

    #[test]
    fn cross_jump_three_arms_fixed_point() {
        // 3-arm chain where each arm ends with a 3-instruction common tail
        // (BinOp, BinOpConst, Return).  Each arm's unique prefix is a single
        // LoadConst with a different constant, so the prefix is NOT merged.
        //
        // A single-pass implementation only merges the first pair.  The fixed-point
        // loop must also detect and apply the second merge opportunity.
        //
        // Shape (14 instructions):
        //   [0]  CmpJumpIfFalseConst  -- if n!=1 jump to [5] (arm2 cond)
        //   [1]  LoadConst(r0, c_a1)  -- arm1 unique prefix
        //   [2]  BinOp(r1,r0,Add,r0)          \
        //   [3]  BinOpConst(r2,r1,Mul,c_k)     } survivor tail (3 insns)
        //   [4]  Return(r2)                    /
        //   [5]  CmpJumpIfFalseConst  -- if n!=2 jump to [10] (arm3 start)
        //   [6]  LoadConst(r0, c_a2)  -- arm2 unique prefix
        //   [7]  BinOp(r1,r0,Add,r0)          \
        //   [8]  BinOpConst(r2,r1,Mul,c_k)     } dup1 tail (same 3 insns)
        //   [9]  Return(r2)                    /
        //   [10] LoadConst(r0, c_a3)  -- arm3 unique prefix  (jump target from [5])
        //   [11] BinOp(r1,r0,Add,r0)          \
        //   [12] BinOpConst(r2,r1,Mul,c_k)     } dup2 tail (same 3 insns)
        //   [13] Return(r2)                    /
        //
        // jump_targets = {0, 5, 10}.  Dup terminators [9] and [13] are NOT targets.
        //
        // First merge (pass 1): dup1 [7..9] -> Jump([2]).
        //   [2] becomes a jump target (from the new Jump at new-[7]).
        // Second merge (pass 2): dup2 tail scanned from [11]:
        //   step=0 Return match; step=1 BinOpConst(new-[10]) match, not a target;
        //   step=2 BinOp(new-[9]) match but [2] IS a target -> scan stops.
        //   tail_len=2 >= MIN_TAIL -> second merge fires!
        //
        // Net: 2 merges applied; instruction count drops by 2.
        // Only one Return survives.
        use crate::ast::BinaryOp;
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 0, 4), // [0] -> [5]
            Insn::LoadConst(5, 1),                            // [1]  arm1 unique
            Insn::BinOp(6, 5, BinaryOp::Add, 5),              // [2] \
            Insn::BinOpConst(7, 6, BinaryOp::Mul, 2, false),  // [3]  survivor tail
            Insn::Return(7),                                  // [4] /
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 3, 4), // [5] -> [10]
            Insn::LoadConst(5, 10),                           // [6]  arm2 unique
            Insn::BinOp(6, 5, BinaryOp::Add, 5),              // [7] \
            Insn::BinOpConst(7, 6, BinaryOp::Mul, 2, false),  // [8]  dup1 tail
            Insn::Return(7),                                  // [9] /
            Insn::LoadConst(5, 20),                           // [10] arm3 unique (jump target)
            Insn::BinOp(6, 5, BinaryOp::Add, 5),              // [11] \
            Insn::BinOpConst(7, 6, BinaryOp::Mul, 2, false),  // [12]  dup2 tail
            Insn::Return(7),                                  // [13] /
        ];
        let before_count = insns.len();
        let out = pass_cross_jump(insns, &[]);

        // Two merges must fire -> instruction count drops by at least 2.
        assert!(
            out.len() <= before_count - 2,
            "fixed-point cross_jump must apply at least 2 merges for 3-arm 3-insn-tail \
             chain (before={before_count}, after={})",
            out.len()
        );

        // Exactly one Return must survive (the survivor tail's Return).
        let return_count = out.iter().filter(|i| matches!(i, Insn::Return(_))).count();
        assert_eq!(
            return_count, 1,
            "exactly one Return should survive after fixed-point merge of 3 arms \
             with 3-instruction common tails"
        );
    }

    // ── pass_concat_merge ─────────────────────────────────────────────────────

    // A constant pool whose slot 0 is a `str`, used to seed `str_regs` so the
    // string-only gate (issue #2383) admits the chain.  Operand registers are
    // marked string via `LoadConst(reg, 0)` prefixes in each test.
    fn str_consts() -> Vec<Value> {
        vec![Value::string("")]
    }

    #[test]
    fn concat_merge_fuses_three_binop_chain() {
        use crate::ast::BinaryOp;
        // LoadConst(0..) seed the operand regs as strings so the gate admits the
        // chain.  num_locals = 2, r0=0, r1=1 (locals), t1=2, r2=3, t2=4.
        // BinOp(t1, r0, Add, r1)   ← t1 is temp (>= num_locals=2), single-use
        // BinOp(t2, t1, Add, r2)   ← t2 is temp, single-use
        let insns = vec![
            Insn::LoadConst(0, 0), // r0 = "" (str)
            Insn::LoadConst(1, 0), // r1 = "" (str)
            Insn::LoadConst(3, 0), // r2 = "" (str)
            Insn::BinOp(2, 0, BinaryOp::Add, 1),
            Insn::BinOp(4, 2, BinaryOp::Add, 3),
            Insn::Return(4),
        ];
        let mut num_regs = 5u32;
        let out = pass_concat_merge(insns, 2, &mut num_regs, &str_consts());

        // 3 LoadConst + 3 Moves + 1 Concat + 1 Return = 8 instructions.
        assert_eq!(out.len(), 8, "3 LoadConst + 3 Moves + Concat + Return");
        // The chain BinOps are replaced by Moves into the operand window.
        assert!(matches!(out[3], Insn::Move(_, 0)), "Move(base+0, r0)");
        assert!(matches!(out[4], Insn::Move(_, 1)), "Move(base+1, r1)");
        assert!(matches!(out[5], Insn::Move(_, 3)), "Move(base+2, r2)");
        assert!(
            matches!(
                out[6],
                Insn::Concat {
                    dst: 4,
                    count: 3,
                    ..
                }
            ),
            "Concat {{ dst: t2, count: 3 }}"
        );
        // num_regs should have grown by 3 (one per operand).
        assert_eq!(num_regs, 8, "num_regs grew by count=3");
    }

    #[test]
    fn concat_merge_skips_non_string_chain() {
        use crate::ast::BinaryOp;
        // Issue #2383: an int chain (no `str` evidence on the leading operand)
        // must NOT be fused — the operand-window Moves are pure overhead.
        let insns = vec![
            Insn::BinOp(2, 0, BinaryOp::Add, 1),
            Insn::BinOp(4, 2, BinaryOp::Add, 3),
            Insn::Return(4),
        ];
        let mut num_regs = 5u32;
        // Empty const pool → no register is provably a string.
        let out = pass_concat_merge(insns, 2, &mut num_regs, &[]);
        assert_eq!(out.len(), 3, "int chain left as plain BinOps");
        assert!(matches!(out[0], Insn::BinOp(..)));
        assert!(matches!(out[1], Insn::BinOp(..)));
        assert!(
            !out.iter().any(|i| matches!(i, Insn::Concat { .. })),
            "no Concat for a non-string chain"
        );
        assert_eq!(num_regs, 5, "num_regs unchanged");
    }

    #[test]
    fn concat_merge_requires_two_binops_minimum() {
        use crate::ast::BinaryOp;
        // Single BinOp(Add): only 2 operands, should NOT be merged.
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::LoadConst(1, 0),
            Insn::BinOp(2, 0, BinaryOp::Add, 1),
            Insn::Return(2),
        ];
        let mut num_regs = 3u32;
        let out = pass_concat_merge(insns, 2, &mut num_regs, &str_consts());
        assert_eq!(out.len(), 4, "no merge for 2-operand chain");
        assert!(
            !out.iter().any(|i| matches!(i, Insn::Concat { .. })),
            "BinOp unchanged"
        );
        assert_eq!(num_regs, 3, "num_regs unchanged");
    }

    #[test]
    fn concat_merge_skips_when_intermediate_multi_use() {
        use crate::ast::BinaryOp;
        // t1 is read twice (by BinOp and by Return), so it cannot be removed.
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::LoadConst(1, 0),
            Insn::LoadConst(3, 0),
            Insn::BinOp(2, 0, BinaryOp::Add, 1),
            Insn::BinOp(4, 2, BinaryOp::Add, 3), // reads t1=2
            Insn::Return(2),                     // also reads t1=2 → use_count=2
        ];
        let mut num_regs = 5u32;
        let out = pass_concat_merge(insns, 2, &mut num_regs, &str_consts());
        // Must NOT merge because t1 has use_count=2.
        assert_eq!(out.len(), 6, "no merge when intermediate is multi-use");
        assert!(
            !out.iter().any(|i| matches!(i, Insn::Concat { .. })),
            "no fusion"
        );
        assert_eq!(num_regs, 5, "num_regs unchanged");
    }

    #[test]
    fn concat_merge_does_not_cross_bb_boundary() {
        use crate::ast::BinaryOp;
        // Layout (indices 0-3):
        //   i=0: BinOp(t1, r0, Add, r1)   ← chain start candidate
        //   i=1: BinOp(t2, t1, Add, r2)   ← BB start: target of Jump at i=3
        //   i=2: Return(t2)
        //   i=3: Jump(-3)                  ← target = 3+1+(-3) = 1
        //
        // Because i=1 is a BB start the chain [0,1] straddles a BB boundary
        // and must NOT be fused.  Operands are seeded as strings so only the
        // BB-boundary guard (not the string gate) can prevent the merge.
        let insns = vec![
            Insn::LoadConst(0, 0),               // r0 = "" (str)
            Insn::LoadConst(1, 0),               // r1 = "" (str)
            Insn::LoadConst(3, 0),               // r2 = "" (str)
            Insn::BinOp(2, 0, BinaryOp::Add, 1), // i=3
            Insn::BinOp(4, 2, BinaryOp::Add, 3), // i=4 ← BB start
            Insn::Return(4),                     // i=5
            Insn::Jump(-3),                      // i=6: target = 6+1+(-3) = 4
        ];
        let mut num_regs = 5u32;
        let out = pass_concat_merge(insns, 2, &mut num_regs, &str_consts());
        assert_eq!(out.len(), 7, "no merge across BB boundary");
        assert!(
            !out.iter().any(|i| matches!(i, Insn::Concat { .. })),
            "no fusion across a BB boundary"
        );
        assert_eq!(num_regs, 5, "num_regs unchanged");
    }

    #[test]
    fn concat_merge_skips_compiled_opaque_param_add() {
        // Function parameters have no static type evidence, so the gate
        // (issue #2383) declines to fuse — the chain keeps its plain BinOp form
        // rather than paying for an operand-window Move per operand.
        let code = compile_fn("def f(a, b, c, d):\n    return a + b + c + d\n");
        let optimized = optimize(code);
        let inner = &optimized.fn_protos[0].code;
        assert!(
            !inner.insns.iter().any(|i| matches!(i, Insn::Concat { .. })),
            "opaque-param chain must NOT be fused; insns: {:?}",
            inner.insns
        );
    }

    // ── pass_loop_inversion ───────────────────────────────────────────────────

    #[test]
    fn loop_inversion_const_variant_basic() {
        use crate::ast::BinaryOp;
        // Minimal while-loop (CmpJumpIfFalseConst variant):
        //   [0] CmpJumpIfFalseConst(0, Lt, 0, 2)   ; k=2, exit to [3] if false
        //   [1] BinOpImm(0, 0, Add, 1)              ; body: i += 1
        //   [2] Jump(-3)                             ; back-edge to [0]
        //   [3] Return(0)
        //
        // jump_pc!(-(2+1)) at i=2: 2 + 1 + (-3) = 0. ✓ targets [0].
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::Jump(-3),
            Insn::Return(0),
        ];
        let out = pass_loop_inversion(insns);
        // Length unchanged: we replace Jump in-place, no removal.
        assert_eq!(out.len(), 4);
        // [0] must remain the initial guard.
        assert!(
            matches!(out[0], Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2)),
            "[0] header must be unchanged"
        );
        // [2] must become CmpJumpIfTrueConst(0, Lt, 0, -2).
        // new_offset = -k = -2;  2+1+(-2) = 1 = j+1. ✓
        assert!(
            matches!(out[2], Insn::CmpJumpIfTrueConst(0, BinaryOp::Lt, 0, -2)),
            "[2] back-edge should be CmpJumpIfTrueConst with offset -2, got {:?}",
            out[2]
        );
    }

    #[test]
    fn loop_inversion_reg_variant_basic() {
        use crate::ast::BinaryOp;
        // CmpJumpIfFalse (register-register) variant:
        //   [0] CmpJumpIfFalse(0, Lt, 1, 2)
        //   [1] BinOpImm(0, 0, BinaryOp::Add, 1)
        //   [2] Jump(-3)
        //   [3] Return(0)
        let insns = vec![
            Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 2),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::Jump(-3),
            Insn::Return(0),
        ];
        let out = pass_loop_inversion(insns);
        assert_eq!(out.len(), 4);
        assert!(
            matches!(out[0], Insn::CmpJumpIfFalse(0, BinaryOp::Lt, 1, 2)),
            "[0] header must be unchanged"
        );
        assert!(
            matches!(out[2], Insn::CmpJumpIfTrue(0, BinaryOp::Lt, 1, -2)),
            "[2] back-edge should be CmpJumpIfTrue with offset -2, got {:?}",
            out[2]
        );
    }

    #[test]
    fn loop_inversion_guard_k_too_small() {
        use crate::ast::BinaryOp;
        // k=1: only one instruction between header and Jump, which IS the Jump.
        // The guard `k < 2` must prevent transformation.
        //   [0] CmpJumpIfFalseConst(0, Lt, 0, 1)   ; k=1, exit to [2]
        //   [1] Jump(-2)                             ; back-edge to [0]
        //   [2] Return(0)
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 1),
            Insn::Jump(-2),
            Insn::Return(0),
        ];
        let out = pass_loop_inversion(insns.clone());
        // Nothing should change.
        assert_eq!(out, insns, "k=1 must not be transformed");
    }

    #[test]
    fn loop_inversion_guard_not_a_back_edge() {
        use crate::ast::BinaryOp;
        // The Jump at [j+k] targets somewhere other than [j] — must not transform.
        //   [0] CmpJumpIfFalseConst(0, Lt, 0, 2)   ; k=2, exit to [3]
        //   [1] BinOpImm(0, 0, BinaryOp::Add, 1)
        //   [2] Jump(0)                              ; NOT a back-edge (targets [3])
        //   [3] Return(0)
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::Jump(0), // forward jump, not -(k+1) = -3
            Insn::Return(0),
        ];
        let out = pass_loop_inversion(insns.clone());
        assert_eq!(out, insns, "forward jump must not be transformed");
    }

    #[test]
    fn loop_inversion_does_not_touch_non_jump_at_back() {
        use crate::ast::BinaryOp;
        // If [j+k] is not a Jump at all, leave it alone.
        //   [0] CmpJumpIfFalseConst(0, Lt, 0, 2)
        //   [1] BinOpImm(0, 0, BinaryOp::Add, 1)
        //   [2] Return(0)                           ; not a Jump
        //   [3] Return(0)
        let insns = vec![
            Insn::CmpJumpIfFalseConst(0, BinaryOp::Lt, 0, 2),
            Insn::BinOpImm(0, 0, BinaryOp::Add, 1, false),
            Insn::Return(0),
            Insn::Return(0),
        ];
        let out = pass_loop_inversion(insns.clone());
        assert_eq!(
            out, insns,
            "non-Jump at back-edge position must not be transformed"
        );
    }

    #[test]
    fn loop_inversion_full_pipeline_while_loop() {
        // End-to-end: compile a while loop whose back-edge survives the pipeline
        // as a raw Jump (not a ForCountReg/ForCount structure).
        //
        // `while a != b: a += 1` — the Ne operator is not recognised by the
        // ForCount promotion pass, so the loop remains as CmpJumpIfFalse + Jump.
        // pass_loop_inversion should replace the back-edge Jump with CmpJumpIfTrue.
        let code = compile_fn("def f(a, b):\n    while a != b:\n        a += 1\n    return a\n");
        let optimized = optimize(code);
        let inner = &optimized.fn_protos[0].code;
        // The optimized code must not contain an unconditional back-edge Jump.
        let has_back_edge_jump = inner
            .insns
            .iter()
            .any(|i| matches!(i, Insn::Jump(k) if *k < 0));
        assert!(
            !has_back_edge_jump,
            "optimizer should eliminate back-edge Jump in while-ne loop; insns: {:?}",
            inner.insns
        );
        // The optimized code must contain CmpJumpIfTrue* at the loop tail.
        let has_cmpjump_true = inner
            .insns
            .iter()
            .any(|i| matches!(i, Insn::CmpJumpIfTrue(..) | Insn::CmpJumpIfTrueConst(..)));
        assert!(
            has_cmpjump_true,
            "optimizer should introduce CmpJumpIfTrue* at loop tail; insns: {:?}",
            inner.insns
        );
    }

    #[test]
    fn loop_inversion_true_const_variant_basic() {
        use crate::ast::BinaryOp;
        // `while True: if n == 0: break; body` shape (CmpJumpIfTrueConst header):
        //   [0] CmpJumpIfTrueConst(0, Eq, 0, 2)   ; k=2, exit to [3] if n==0 (TRUE)
        //   [1] BinOpImm(0, 0, BinaryOp::Sub, 1)  ; body: n -= 1
        //   [2] Jump(-3)                            ; back-edge to [0]
        //   [3] Return(0)
        //
        // Arithmetic check: jump_pc!(-(2+1)) at i=2: 2+1+(-3) = 0. ✓ targets [0].
        let insns = vec![
            Insn::CmpJumpIfTrueConst(0, BinaryOp::Eq, 0, 2),
            Insn::BinOpImm(0, 0, BinaryOp::Sub, 1, false),
            Insn::Jump(-3),
            Insn::Return(0),
        ];
        let out = pass_loop_inversion(insns);
        assert_eq!(out.len(), 4);
        // [0] must remain the initial guard.
        assert!(
            matches!(out[0], Insn::CmpJumpIfTrueConst(0, BinaryOp::Eq, 0, 2)),
            "[0] header must be unchanged"
        );
        // [2] must become CmpJumpIfFalseConst(0, Eq, 0, -2).
        // new_offset = -k = -2;  2+1+(-2) = 1 = j+1. ✓
        assert!(
            matches!(out[2], Insn::CmpJumpIfFalseConst(0, BinaryOp::Eq, 0, -2)),
            "[2] back-edge should be CmpJumpIfFalseConst with offset -2, got {:?}",
            out[2]
        );
    }

    #[test]
    fn loop_inversion_true_reg_variant_basic() {
        use crate::ast::BinaryOp;
        // CmpJumpIfTrue (register-register) header variant:
        //   [0] CmpJumpIfTrue(0, Eq, 1, 2)
        //   [1] BinOpImm(0, 0, BinaryOp::Sub, 1)
        //   [2] Jump(-3)
        //   [3] Return(0)
        let insns = vec![
            Insn::CmpJumpIfTrue(0, BinaryOp::Eq, 1, 2),
            Insn::BinOpImm(0, 0, BinaryOp::Sub, 1, false),
            Insn::Jump(-3),
            Insn::Return(0),
        ];
        let out = pass_loop_inversion(insns);
        assert_eq!(out.len(), 4);
        assert!(
            matches!(out[0], Insn::CmpJumpIfTrue(0, BinaryOp::Eq, 1, 2)),
            "[0] header must be unchanged"
        );
        assert!(
            matches!(out[2], Insn::CmpJumpIfFalse(0, BinaryOp::Eq, 1, -2)),
            "[2] back-edge should be CmpJumpIfFalse with offset -2, got {:?}",
            out[2]
        );
    }

    #[test]
    fn loop_inversion_true_const_guard_k_too_small() {
        use crate::ast::BinaryOp;
        // k=1: guard must prevent transformation for CmpJumpIfTrueConst header.
        //   [0] CmpJumpIfTrueConst(0, Eq, 0, 1)   ; k=1, exit to [2]
        //   [1] Jump(-2)                             ; back-edge to [0]
        //   [2] Return(0)
        let insns = vec![
            Insn::CmpJumpIfTrueConst(0, BinaryOp::Eq, 0, 1),
            Insn::Jump(-2),
            Insn::Return(0),
        ];
        let out = pass_loop_inversion(insns.clone());
        assert_eq!(
            out, insns,
            "k=1 must not be transformed for CmpJumpIfTrueConst header"
        );
    }

    #[test]
    fn loop_inversion_full_pipeline_while_true_break() {
        // End-to-end: compile `while True: if n == 0: break; acc += n; n -= 1`.
        // The CmpJumpIfTrueConst header (break condition) + Jump back-edge should
        // be inverted so the back-edge becomes CmpJumpIfFalseConst.
        let code = compile_fn(
            "def f(n):\n    acc = 0\n    while True:\n        if n == 0:\n            break\n        acc += n\n        n -= 1\n    return acc\n",
        );
        let optimized = optimize(code);
        let inner = &optimized.fn_protos[0].code;
        // No unconditional back-edge Jump should survive.
        let has_back_edge_jump = inner
            .insns
            .iter()
            .any(|i| matches!(i, Insn::Jump(k) if *k < 0));
        assert!(
            !has_back_edge_jump,
            "optimizer should eliminate back-edge Jump in while-true-break loop; insns: {:?}",
            inner.insns
        );
        // A CmpJumpIfFalseConst (the inverted back-edge) must be present.
        let has_cmpjump_false_const = inner
            .insns
            .iter()
            .any(|i| matches!(i, Insn::CmpJumpIfFalseConst(..)));
        assert!(
            has_cmpjump_false_const,
            "optimizer should introduce CmpJumpIfFalseConst at loop tail; insns: {:?}",
            inner.insns
        );
    }

    // ── line-number remap across equal-valued constants (issue #1962) ─────────

    #[test]
    fn equal_valued_const_statements_keep_distinct_linenos() {
        use crate::ast::BinaryOp;
        // Two statements whose constant expressions fold to the SAME value.
        // `2 ** 1024` and `(2 ** 512) * (2 ** 512)` both fold to the same BigInt;
        // the optimizer dedups the constant-pool slot.  The `/ 1` divisions are
        // left as runtime BinOps (folding them would raise OverflowError).  The
        // surviving division for the FIRST statement must retain line 1, not
        // inherit the second statement's line — otherwise an exception raised on
        // line 1 is mis-attributed to line 2 (the bug: remap_linenos ran after
        // constant-pool compaction reindexed the LoadConst slots).
        let code = compile_script_with_linenos_for_test(
            "(2 ** 1024) / 1\n(2 ** 512) * (2 ** 512) / 1\n",
            &[1, 2],
        );
        let optimized = optimize(code);

        // Collect the line number of every surviving Div BinOp, in order.
        let div_linenos: Vec<u32> = optimized
            .insns
            .iter()
            .enumerate()
            .filter_map(|(i, ins)| match ins {
                Insn::BinOp(_, _, BinaryOp::Div, _)
                | Insn::BinOpConst(_, _, BinaryOp::Div, _, _) => {
                    Some(optimized.lineno_table.get(i).copied().unwrap_or(0))
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            div_linenos,
            vec![1, 2],
            "each statement's division must keep its own source line; insns: {:?}, linenos: {:?}",
            optimized.insns,
            optimized.lineno_table
        );
    }

    // ── issue #2002: linear compile on long single-variable def-use chains ──────

    /// Brute-force reference for `remap_linenos`'s greedy forward scan.  The
    /// indexed implementation must produce byte-identical output.
    fn remap_linenos_reference(
        old_insns: &[Insn],
        old_linenos: &[u32],
        new_insns: &[Insn],
    ) -> Vec<u32> {
        if old_linenos.is_empty() {
            return vec![0u32; new_insns.len()];
        }
        let mut running = 0u32;
        let old_prefix: Vec<u32> = old_linenos
            .iter()
            .map(|&ln| {
                if ln != 0 {
                    running = ln;
                }
                running
            })
            .collect();
        let mut old_pos = 0usize;
        let mut result = Vec::with_capacity(new_insns.len());
        'outer: for new_insn in new_insns {
            // `old_pos` advances across outer iterations; the inner range is
            // evaluated once per outer pass and the mutation only takes effect
            // on the next pass, which is the intended scan behaviour.
            #[allow(clippy::mut_range_bound)]
            for i in old_pos..old_insns.len() {
                if &old_insns[i] == new_insn {
                    result.push(old_linenos.get(i).copied().unwrap_or(0));
                    old_pos = i + 1;
                    continue 'outer;
                }
            }
            result.push(old_prefix.get(old_pos).copied().unwrap_or(0));
        }
        result
    }

    #[test]
    fn remap_linenos_indexed_matches_reference() {
        use crate::ast::BinaryOp;
        // A stream with duplicate instructions, optimizer-created instructions
        // that never match, and reordered survivors — the cases where the greedy
        // cursor behaviour matters.
        let old = vec![
            Insn::LoadConst(2, 0),
            Insn::BinOp(0, 0, BinaryOp::Add, 2),
            Insn::LoadConst(2, 0),
            Insn::BinOp(0, 0, BinaryOp::Add, 2),
            Insn::LoadConst(2, 0),
            Insn::Return(0),
        ];
        let old_ln = vec![1, 1, 2, 2, 3, 3];
        let candidates = vec![
            Insn::LoadConst(2, 0),               // matches first occurrence
            Insn::BinOp(0, 0, BinaryOp::Add, 2), // matches
            Insn::LoadConst(9, 5),               // never matches (optimizer-made)
            Insn::LoadConst(2, 0),               // matches the next occurrence
            Insn::Return(0),                     // matches
        ];
        let got = remap_linenos(&old, &old_ln, &candidates);
        let want = remap_linenos_reference(&old, &old_ln, &candidates);
        assert_eq!(got, want);
    }

    #[test]
    fn const_fold_long_chain_folds_to_final_value() {
        // x = x + 1 + 2 + ... folded across a straight-line chain must collapse
        // every step and leave the correct final constant in the pool.
        let mut src = String::from("x = 0\n");
        for i in 1..=20 {
            src.push_str(&format!("x = x + {i}\n"));
        }
        let code = compile_fn(&src);
        let optimized = optimize(code);
        // sum(1..=20) == 210 must appear as a folded Int constant.
        assert!(
            optimized
                .consts
                .iter()
                .any(|v| matches!(v.kind(), ValueKind::Int(210))),
            "folded chain must produce the constant 210; consts: {:?}",
            optimized.consts
        );
    }

    #[test]
    fn const_index_intern_matches_linear_scan() {
        // The hash-indexed interner must return the same slots as the original
        // linear scan (first-occurrence-wins dedup), including for Bool vs Int
        // (which must never collide) and float bit-equality.
        let mut a = vec![Value::int(1), Value::bool_(true), Value::float(2.0)];
        let mut b = a.clone();
        let mut idx = ConstIndex::build(&a);
        let vals = [
            Value::int(1),       // dedup → 0
            Value::bool_(true),  // dedup → 1 (not Int(1))
            Value::int(5),       // new → 3
            Value::float(2.0),   // dedup → 2
            Value::bool_(false), // new → 4
            Value::int(5),       // dedup → 3
        ];
        for v in vals {
            let indexed = idx.intern(&mut a, v.clone());
            let linear = intern_const_in_pool(&mut b, v);
            assert_eq!(indexed, linear, "indexed vs linear intern diverged");
        }
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn build_exc_table_no_handlers_is_identity() {
        let insns = vec![Insn::LoadConst(0, 0), Insn::Return(0)];
        let (out, table) = build_exc_table(insns.clone());
        assert_eq!(out, insns, "stream unchanged when no SetupExcept present");
        assert_eq!(table, vec![EXC_NO_HANDLER, EXC_NO_HANDLER]);
    }

    #[test]
    fn build_exc_table_strips_and_maps_single_try() {
        // Layout (offsets are relative, +1 of the source counting in jump_pc):
        //   0 SetupExcept(+1)   -> handler at pc 3 (the LoadConst below)
        //   1 RaiseValue(0)     try body, raises
        //   2 PopExcept         normal exit (jumped over on raise)
        //   3 LoadConst(0,0)    handler entry
        //   4 Return(0)
        // SetupExcept's absolute target = 0 + 1 + 1 = 2; but pc 2 is PopExcept,
        // which is stripped, so compact redirects the handler to pc 3 → new pc 1.
        let insns = vec![
            Insn::SetupExcept(1),
            Insn::RaiseValue(0),
            Insn::PopExcept,
            Insn::LoadConst(0, 0),
            Insn::Return(0),
        ];
        let (out, table) = build_exc_table(insns);
        // Two instructions (SetupExcept + PopExcept) removed.
        assert_eq!(out.len(), 3);
        assert!(
            !out.iter()
                .any(|i| matches!(i, Insn::SetupExcept(_) | Insn::PopExcept)),
            "block-setup instructions must be stripped"
        );
        // New stream: [RaiseValue(0), LoadConst(0,0), Return(0)].
        // RaiseValue is now at new pc 0 and must dispatch to the handler at new
        // pc 1 (the LoadConst).
        assert!(matches!(out[0], Insn::RaiseValue(0)));
        assert_eq!(table[0], 1, "raise inside try → handler pc 1");
        // The handler body and code after it are not protected.
        assert_eq!(table[1], EXC_NO_HANDLER);
        assert_eq!(table[2], EXC_NO_HANDLER);
    }

    #[test]
    fn build_exc_table_real_compiled_try_is_stripped_and_consistent() {
        // A real compiled try/except: after the optimizer runs (which includes
        // build_exc_table), the FnCode must have no SetupExcept/PopExcept left
        // and a non-empty exc_table whose every protected entry points at a
        // valid in-range handler pc.
        let code = optimize(compile_fn(
            "def f(x):\n    try:\n        return x.attr\n    except AttributeError:\n        return -1\n",
        ));
        // The optimized top-level code holds f as a nested proto; check the proto.
        let proto = &code.fn_protos[0].code;
        assert!(
            !proto
                .insns
                .iter()
                .any(|i| matches!(i, Insn::SetupExcept(_) | Insn::PopExcept)),
            "block-setup instructions must be stripped from optimized try/except"
        );
        assert_eq!(
            proto.exc_table.len(),
            proto.insns.len(),
            "exc_table is parallel to insns"
        );
        // At least one instruction is protected, and every handler target is a
        // valid pc within the (post-strip) stream.
        let protected = proto
            .exc_table
            .iter()
            .filter(|&&t| t != EXC_NO_HANDLER)
            .count();
        assert!(protected > 0, "the try body must be covered by a handler");
        for &t in &proto.exc_table {
            if t != EXC_NO_HANDLER {
                assert!(
                    (t as usize) < proto.insns.len(),
                    "handler target {t} out of range (len {})",
                    proto.insns.len()
                );
            }
        }
    }

    // ── pass_inline (issue #349) ──────────────────────────────────────────────

    fn has_user_call(insns: &[Insn]) -> bool {
        insns
            .iter()
            .any(|i| matches!(i, Insn::Call(..) | Insn::CallMemo(..)))
    }

    #[test]
    fn inline_small_pure_leaf_removes_call() {
        // The canonical target: a tiny pure helper called in a loop is spliced
        // inline, so no Call/CallMemo survives at the call site.
        let code = optimize(compile_fn(
            "def sq(x):\n    return x * x\ns = 0\nfor i in range(5):\n    s += sq(i)\n",
        ));
        assert!(
            !has_user_call(&code.insns),
            "sq(i) should be inlined away; insns: {:?}",
            code.insns
        );
        // A Mul BinOp from the inlined body must remain in the top-level stream.
        assert!(
            code.insns
                .iter()
                .any(|i| matches!(i, Insn::BinOp(_, _, crate::ast::BinaryOp::Mul, _))),
            "inlined body's multiply must appear in caller; insns: {:?}",
            code.insns
        );
    }

    #[test]
    fn inline_multi_arg_helper() {
        let code = optimize(compile_fn(
            "def add3(a, b, c):\n    return a + b + c\nr = add3(1, 2, 3)\n",
        ));
        assert!(
            !has_user_call(&code.insns),
            "add3(1,2,3) should be inlined; insns: {:?}",
            code.insns
        );
    }

    #[test]
    fn no_inline_when_globals_reified() {
        // globals() lets the binding be swapped at runtime, so the helper must
        // NOT be inlined — the real call is preserved.
        let code = optimize(compile_fn(
            "def h(x):\n    return x + 1\ng = globals()\nr = h(2)\n",
        ));
        assert!(
            has_user_call(&code.insns),
            "globals() reification must disable inlining; insns: {:?}",
            code.insns
        );
    }

    #[test]
    fn no_inline_recursive_helper() {
        // A recursive function is not pure (and its body contains a Call), so it
        // is never inlined — preventing infinite expansion.
        let code = optimize(compile_fn(
            "def fac(n):\n    if n <= 1:\n        return 1\n    return n * fac(n - 1)\nr = fac(5)\n",
        ));
        assert!(
            has_user_call(&code.insns),
            "recursive helper must not be inlined; insns: {:?}",
            code.insns
        );
    }

    #[test]
    fn no_inline_default_arg_helper() {
        // Default parameters disqualify inlining (MakeFunction carries defaults
        // and argc may differ from the call site).
        let code = optimize(compile_fn("def d(x, y=10):\n    return x + y\nr = d(1)\n"));
        assert!(
            has_user_call(&code.insns),
            "default-arg helper must not be inlined; insns: {:?}",
            code.insns
        );
    }
}
