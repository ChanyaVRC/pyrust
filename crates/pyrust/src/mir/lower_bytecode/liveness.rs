//! Exact backward SSA liveness and RPO linear live-interval computation.

use std::collections::HashSet;

use crate::bytecode::Insn;

use crate::mir::ir::{MirFn, MirOp, Val};
use crate::mir::verify::DominatorTree;

/// Exact SSA liveness, RPO live intervals, use counts, and local write/barrier
/// positions needed by coalescing and linear-scan register allocation.
pub(super) struct LivenessInfo {
    pub(super) live_in: Vec<HashSet<Val>>,
    pub(super) live_out: Vec<HashSet<Val>>,
    pub(super) start_pos: Vec<usize>,
    pub(super) end_pos: Vec<usize>,
    pub(super) block_start: Vec<usize>,
    pub(super) use_count: Vec<u32>,
    pub(super) local_write_positions: Vec<Vec<usize>>,
    pub(super) barrier_positions: Vec<usize>,
    pub(super) aliased_locals: bool,
}

#[inline]
pub(super) fn mir_op_may_invalidate_locals(op: &MirOp) -> bool {
    !matches!(
        op,
        MirOp::Const(_)
            | MirOp::Copy(_)
            | MirOp::LoadLocal(_)
            | MirOp::LoadGlobal { .. }
            | MirOp::LoadCell { .. }
            | MirOp::BuildTuple(_)
            | MirOp::BuildList(_)
    )
}

/// Compute exact backward SSA liveness (`live_in`/`live_out`) and RPO linear
/// `[start_pos, end_pos]` intervals (extended across block boundaries and loop
/// back-edges).
pub(super) fn compute_liveness_and_intervals(func: &MirFn, dom: &DominatorTree) -> LivenessInfo {
    let nvals = func.next_val as usize;
    let nb = func.blocks.len();

    // 1. Exact backward SSA liveness per block.
    let mut live_in: Vec<HashSet<Val>> = vec![HashSet::new(); nb];
    let mut live_out: Vec<HashSet<Val>> = vec![HashSet::new(); nb];

    for _ in 0..(nb * 4 + 16) {
        let mut changed = false;
        for &bb_id in dom.rpo.iter().rev() {
            let bi = bb_id.0 as usize;
            let block = &func.blocks[bi];

            // Live-out = union over successors of (live_in[succ] \ params[succ])
            // plus all edge args passed from `bi` to `succ`.
            let mut out_set: HashSet<Val> = HashSet::new();
            block.term.for_each_edge(|succ, args| {
                let si = succ.0 as usize;
                let succ_params: HashSet<Val> = func.blocks[si].params.iter().copied().collect();
                for &v in &live_in[si] {
                    if !succ_params.contains(&v) {
                        out_set.insert(v);
                    }
                }
                for &a in args {
                    out_set.insert(a);
                }
            });
            live_out[bi] = out_set.clone();

            // Walk backward through terminator and instructions.
            let mut cur = out_set;
            block.term.for_each_def(|d| {
                cur.remove(&d);
            });
            block.term.for_each_use(|u| {
                cur.insert(u);
            });
            for inst in block.instrs.iter().rev() {
                inst.for_each_def(|d| {
                    cur.remove(&d);
                });
                inst.for_each_use(|u| {
                    cur.insert(u);
                });
            }
            // Block params ARE live-in to `bi` (needed for incoming edge copies).
            for &p in &block.params {
                cur.insert(p);
            }

            if cur != live_in[bi] {
                live_in[bi] = cur;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // 2. Assign linear positions in RPO order, compute `[start, end]` intervals,
    //    and record every write position to canonical local registers `0..num_locals`.
    let mut start_pos: Vec<usize> = vec![usize::MAX; nvals];
    let mut end_pos: Vec<usize> = vec![0; nvals];
    let mut block_start: Vec<usize> = vec![0; nb];
    let mut block_end: Vec<usize> = vec![0; nb];
    let mut use_count: Vec<u32> = vec![0; nvals];
    let mut local_write_positions: Vec<Vec<usize>> = vec![Vec::new(); func.num_locals as usize];
    let mut barrier_positions: Vec<usize> = Vec::new();
    let aliased_locals = !func.is_function_scope;

    let mut cursor = 0usize;
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        let block = &func.blocks[bi];
        block_start[bi] = cursor;
        for &p in &block.params {
            start_pos[p.0 as usize] = start_pos[p.0 as usize].min(cursor);
            end_pos[p.0 as usize] = end_pos[p.0 as usize].max(cursor);
        }
        cursor += 1;

        for inst in &block.instrs {
            let pos = cursor;
            inst.for_each_use(|u| {
                use_count[u.0 as usize] += 1;
                end_pos[u.0 as usize] = end_pos[u.0 as usize].max(pos);
            });
            inst.for_each_def(|d| {
                start_pos[d.0 as usize] = start_pos[d.0 as usize].min(pos);
                end_pos[d.0 as usize] = end_pos[d.0 as usize].max(pos);
            });
            match &inst.op {
                MirOp::LoadLocal(_) => {}
                MirOp::StoreLocal { reg, .. }
                | MirOp::SideEffectInsn {
                    insn: Insn::DeleteLocal(reg, _, _),
                    ..
                } => {
                    if (*reg as usize) < local_write_positions.len() {
                        local_write_positions[*reg as usize].push(pos);
                    }
                }
                op => {
                    if aliased_locals && mir_op_may_invalidate_locals(op) {
                        barrier_positions.push(pos);
                    }
                }
            }
            cursor += 1;
        }

        let tpos = cursor;
        block.term.for_each_use(|u| {
            use_count[u.0 as usize] += 1;
            end_pos[u.0 as usize] = end_pos[u.0 as usize].max(tpos);
        });
        block.term.for_each_def(|d| {
            start_pos[d.0 as usize] = start_pos[d.0 as usize].min(tpos);
            end_pos[d.0 as usize] = end_pos[d.0 as usize].max(tpos);
        });
        block_end[bi] = tpos;
        cursor += 1;
    }

    // Extend intervals for every value live across block boundaries (handles
    // loops, back-edges, and arbitrary merges cleanly).
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        for &v in &live_out[bi] {
            let vi = v.0 as usize;
            if start_pos[vi] != usize::MAX {
                end_pos[vi] = end_pos[vi].max(block_end[bi]);
            }
        }
        for &v in &live_in[bi] {
            let vi = v.0 as usize;
            if start_pos[vi] != usize::MAX {
                start_pos[vi] = start_pos[vi].min(block_start[bi]);
                end_pos[vi] = end_pos[vi].max(block_start[bi]);
            }
        }
    }

    LivenessInfo {
        live_in,
        live_out,
        start_pos,
        end_pos,
        block_start,
        use_count,
        local_write_positions,
        barrier_positions,
        aliased_locals,
    }
}
