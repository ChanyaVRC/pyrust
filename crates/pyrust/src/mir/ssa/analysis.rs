//! Dataflow analyses (`definitely_bound`, backward register liveness,
//! pinned-register discovery) and Cytron-style block-parameter phi placement.

use crate::bytecode::{Insn, InsnDesc, Reg};

use crate::mir::insn_info::{insn_may_invalidate_named_locals, visit_ssa_reads, visit_ssa_writes};
use crate::mir::ir::{BlockId, MirBlock, MirFn, Terminator, Val};
use crate::mir::verify::DominatorTree;

use super::cfg::{RawBlock, RawEdgeKind, RegBits};

/// Precomputed SSA construction metadata for the dominator-tree renamer.
pub(super) struct SsaAnalysis {
    pub(super) pinned: RegBits,
    pub(super) def_in: Vec<RegBits>,
    pub(super) def_blocks: Vec<Vec<usize>>,
    pub(super) phi_regs: Vec<Vec<Reg>>,
    pub(super) phi_vals: Vec<Vec<Val>>,
    pub(super) dom: DominatorTree,
}

/// Run `definitely_bound` and backward liveness dataflow on `compact_raw`,
/// populate skeleton [`MirBlock`]s on `mir`, and place block-parameter phis
/// on dominance frontiers.
pub(super) fn analyze_and_place_phis(
    compact_raw: &[RawBlock],
    insns: &[Insn],
    descs: &[InsnDesc],
    num_locals: Reg,
    num_regs: usize,
    aliased_locals: bool,
    mir: &mut MirFn,
) -> SsaAnalysis {
    let nb = compact_raw.len();

    // All named locals (`0..num_locals`) are write-through pinned so `locals()`,
    // `f_locals`, tracebacks, `SyncModuleGlobal`, `RecordClassStore`,
    // `CheckLocal`, and exception handlers always see canonical `R[0..num_locals]`.
    let mut pinned = RegBits::empty(num_regs);
    for r in 0..num_locals {
        pinned.insert(r);
    }

    let def_in = compute_definitely_bound(
        compact_raw,
        insns,
        descs,
        num_locals,
        num_regs,
        aliased_locals,
        &mut pinned,
    );
    let (def_blocks, live_in) = compute_liveness_and_defs(
        compact_raw,
        insns,
        descs,
        num_locals,
        num_regs,
        aliased_locals,
    );

    // Build skeleton MirBlocks so DominatorTree can compute dominance frontiers.
    for (bi, rb) in compact_raw.iter().enumerate() {
        let dummy_term = match rb.succs.as_slice() {
            [] => Terminator::Unreachable,
            [e0] => Terminator::Jump {
                target: BlockId(e0.target as u32),
                args: Vec::new(),
            },
            [e0, e1] => Terminator::Branch {
                cond: Val(0),
                then_bb: BlockId(e0.target as u32),
                then_args: Vec::new(),
                else_bb: BlockId(e1.target as u32),
                else_args: Vec::new(),
            },
            _ => unreachable!("raw block has at most 2 successors"),
        };
        mir.blocks.push(MirBlock {
            id: BlockId(bi as u32),
            params: Vec::new(),
            instrs: Vec::new(),
            term: dummy_term,
            term_lineno: 0,
            term_col_span: (0, 0, 0, 0),
            preds: rb.preds.iter().map(|&p| BlockId(p as u32)).collect(),
            exc_depth: rb.exc_depth,
        });
    }

    let dom = DominatorTree::compute(mir);
    let df = dom.dominance_frontiers(mir);

    // Place block-parameter phis on dominance frontiers.
    let mut phi_regs: Vec<Vec<Reg>> = vec![Vec::new(); nb];
    let mut phi_vals: Vec<Vec<Val>> = vec![Vec::new(); nb];

    for (reg_usize, blocks_defining) in def_blocks.iter().enumerate() {
        let r = reg_usize as Reg;
        if blocks_defining.is_empty() {
            continue;
        }
        let mut has_phi = vec![false; nb];
        let mut work: Vec<usize> = blocks_defining.clone();
        while let Some(b) = work.pop() {
            for &y_id in &df[b] {
                let y = y_id.0 as usize;
                if y == 0 || has_phi[y] {
                    continue;
                }
                // Soundness gate: only place a phi when `r` is BOTH live at
                // `y`'s entry AND guaranteed definitely bound along every
                // incoming edge!
                if def_in[y].contains(r) && live_in[y].contains(r) {
                    has_phi[y] = true;
                    let param_val = mir.fresh_val();
                    phi_regs[y].push(r);
                    phi_vals[y].push(param_val);
                    mir.blocks[y].params.push(param_val);
                    work.push(y);
                }
            }
        }
    }

    SsaAnalysis {
        pinned,
        def_in,
        def_blocks,
        phi_regs,
        phi_vals,
        dom,
    }
}

/// Compute `definitely_bound_in` per block and pin any temp register read at a
/// point where it is not definitely bound.
fn compute_definitely_bound(
    compact_raw: &[RawBlock],
    insns: &[Insn],
    descs: &[InsnDesc],
    num_locals: Reg,
    num_regs: usize,
    aliased_locals: bool,
    pinned: &mut RegBits,
) -> Vec<RegBits> {
    let nb = compact_raw.len();
    let mut def_in: Vec<RegBits> = (0..nb)
        .map(|bi| {
            if bi == 0 {
                RegBits::empty(num_regs)
            } else {
                RegBits::all(num_regs)
            }
        })
        .collect();
    let mut def_out: Vec<RegBits> = (0..nb).map(|_| RegBits::all(num_regs)).collect();

    let apply_block_def_transfer = |bi: usize, mut bits: RegBits| -> RegBits {
        let rb = &compact_raw[bi];
        if rb.is_handler {
            // An exception can enter a handler from any raising op inside the
            // protected try block, so named locals reload from canonical R[r].
            bits.clear_below(num_locals);
        }
        for insn in &insns[rb.start_pc..rb.body_end] {
            if let Insn::DeleteLocal(r, _, _) = insn {
                bits.remove(*r);
                continue;
            }
            visit_ssa_writes(insn, descs, |w| bits.insert(w));
            if aliased_locals && insn_may_invalidate_named_locals(insn) {
                bits.clear_below(num_locals);
            }
        }
        bits
    };

    for _ in 0..(nb * 4 + 16) {
        let mut changed = false;
        for bi in 0..nb {
            let next_in = if bi == 0 {
                RegBits::empty(num_regs)
            } else {
                let mut acc = RegBits::all(num_regs);
                for &p in &compact_raw[bi].preds {
                    let mut edge_bits = def_out[p].clone();
                    if let Some(tpc) = compact_raw[p].term_pc {
                        let edge_kind = compact_raw[p]
                            .succs
                            .iter()
                            .find(|e| e.target == bi)
                            .map(|e| e.kind)
                            .unwrap_or(RawEdgeKind::Normal);
                        match (&insns[tpc], edge_kind) {
                            (Insn::ForIter(dst, _, _), RawEdgeKind::ForIterBody) => {
                                edge_bits.insert(*dst);
                            }
                            (
                                Insn::MatchExceptStar(desc_idx, _),
                                RawEdgeKind::ExceptStarMatched,
                            ) => {
                                let desc = descs[*desc_idx as usize].as_match_except_star();
                                edge_bits.insert(desc.matched_dst);
                                edge_bits.insert(desc.src_group);
                            }
                            _ => {}
                        }
                    }
                    acc.intersect_with(&edge_bits);
                }
                if compact_raw[bi].is_handler {
                    acc.clear_below(num_locals);
                }
                acc
            };
            if next_in != def_in[bi] {
                def_in[bi] = next_in;
                changed = true;
            }
            let next_out = apply_block_def_transfer(bi, def_in[bi].clone());
            if next_out != def_out[bi] {
                def_out[bi] = next_out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Any temp register read where it is not `definitely_bound` gets pinned so
    // its writes also go through canonical `R[r]`.
    for bi in 0..nb {
        let rb = &compact_raw[bi];
        let mut cur = def_in[bi].clone();
        for insn in &insns[rb.start_pc..rb.body_end] {
            visit_ssa_reads(insn, descs, |r| {
                if r >= num_locals && !cur.contains(r) {
                    pinned.insert(r);
                }
            });
            if let Insn::DeleteLocal(r, _, _) = insn {
                cur.remove(*r);
            } else {
                visit_ssa_writes(insn, descs, |w| cur.insert(w));
                if aliased_locals && insn_may_invalidate_named_locals(insn) {
                    cur.clear_below(num_locals);
                }
            }
        }
        if let Some(tpc) = rb.term_pc {
            visit_ssa_reads(&insns[tpc], descs, |r| {
                if r >= num_locals && !cur.contains(r) {
                    pinned.insert(r);
                }
            });
        }
    }

    def_in
}

/// Compute per-register defining blocks and backward register `live_in` sets.
fn compute_liveness_and_defs(
    compact_raw: &[RawBlock],
    insns: &[Insn],
    descs: &[InsnDesc],
    num_locals: Reg,
    num_regs: usize,
    aliased_locals: bool,
) -> (Vec<Vec<usize>>, Vec<RegBits>) {
    let nb = compact_raw.len();
    let mut use_before_def: Vec<RegBits> = Vec::with_capacity(nb);
    let mut kill_set: Vec<RegBits> = Vec::with_capacity(nb);
    let mut def_blocks: Vec<Vec<usize>> = vec![Vec::new(); num_regs];

    let record_edge_def = |def_blocks: &mut [Vec<usize>], r: Reg, bi: usize, target_bb: usize| {
        if (r as usize) < num_regs {
            if !def_blocks[r as usize].contains(&target_bb) {
                def_blocks[r as usize].push(target_bb);
            }
            if compact_raw[target_bb].preds.len() > 1 && !def_blocks[r as usize].contains(&bi) {
                def_blocks[r as usize].push(bi);
            }
        }
    };

    for (bi, rb) in compact_raw.iter().enumerate() {
        let mut used = RegBits::empty(num_regs);
        let mut killed = RegBits::empty(num_regs);
        if rb.is_handler {
            for r in 0..num_locals {
                killed.insert(r);
            }
        }
        for insn in &insns[rb.start_pc..rb.body_end] {
            visit_ssa_reads(insn, descs, |r| {
                if !killed.contains(r) {
                    used.insert(r);
                }
            });
            if let Insn::DeleteLocal(r, _, _) = insn {
                killed.insert(*r);
            } else {
                visit_ssa_writes(insn, descs, |w| {
                    killed.insert(w);
                    if (w as usize) < num_regs && def_blocks[w as usize].last() != Some(&bi) {
                        def_blocks[w as usize].push(bi);
                    }
                });
                if aliased_locals && insn_may_invalidate_named_locals(insn) {
                    for r in 0..num_locals {
                        killed.insert(r);
                    }
                }
            }
        }
        if let Some(tpc) = rb.term_pc {
            let insn = &insns[tpc];
            visit_ssa_reads(insn, descs, |r| {
                if !killed.contains(r) {
                    used.insert(r);
                }
            });
            match insn {
                Insn::ForIter(dst, _, _) => {
                    record_edge_def(&mut def_blocks, *dst, bi, rb.succs[0].target);
                }
                Insn::MatchExceptStar(desc_idx, _) => {
                    let desc = descs[*desc_idx as usize].as_match_except_star();
                    let matched_bb = rb.succs[0].target;
                    record_edge_def(&mut def_blocks, desc.matched_dst, bi, matched_bb);
                    record_edge_def(&mut def_blocks, desc.src_group, bi, matched_bb);
                }
                _ => {}
            }
        }
        use_before_def.push(used);
        kill_set.push(killed);
    }

    let mut live_in: Vec<RegBits> = (0..nb).map(|_| RegBits::empty(num_regs)).collect();
    let mut live_out: Vec<RegBits> = (0..nb).map(|_| RegBits::empty(num_regs)).collect();
    for _ in 0..(nb * 4 + 16) {
        let mut changed = false;
        for bi in (0..nb).rev() {
            let mut out_bits = RegBits::empty(num_regs);
            for edge in &compact_raw[bi].succs {
                out_bits.union_with(&live_in[edge.target]);
            }
            live_out[bi] = out_bits;
            let mut in_bits = use_before_def[bi].clone();
            for r in live_out[bi].iter() {
                if !kill_set[bi].contains(r) {
                    in_bits.insert(r);
                }
            }
            if in_bits != live_in[bi] {
                live_in[bi] = in_bits;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    (def_blocks, live_in)
}
