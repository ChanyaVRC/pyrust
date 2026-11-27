//! Zero-copy register coalescing heuristics for canonical locals, `ForIter`
//! loop variables, named-local phi parameters, and `Call` callee-slot reuse.

use smallvec::SmallVec;

use crate::bytecode::{Insn, Reg};

use crate::mir::ir::{MirFn, MirOp, Terminator, Val};
use crate::mir::verify::DominatorTree;

use super::liveness::{LivenessInfo, mir_op_may_invalidate_locals};

/// Pre-assigned register hints computed before linear-scan allocation.
pub(super) struct CoalescePlan {
    pub(super) val_to_reg: Vec<Reg>,
    pub(super) preassigned: Vec<bool>,
    pub(super) call_reused_base: Vec<Option<Val>>,
}

#[inline]
fn inst_clobbers_local(op: &MirOp, r: Reg, aliased_locals: bool) -> bool {
    match op {
        MirOp::StoreLocal { reg, .. }
        | MirOp::SideEffectInsn {
            insn: Insn::DeleteLocal(reg, _, _),
            ..
        } => *reg == r,
        other => aliased_locals && mir_op_may_invalidate_locals(other),
    }
}

/// Verify that canonical local `R[r]` is never clobbered before a subsequent
/// use of `v` in `origin_bb` (starting at `origin_start_ii`) or in any other
/// block where `v` is live-in.
fn is_local_unclobbered_while_live(
    func: &MirFn,
    dom: &DominatorTree,
    liveness: &LivenessInfo,
    v: Val,
    r: Reg,
    origin_bb: usize,
    origin_start_ii: usize,
) -> bool {
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        let start_ii = if bi == origin_bb {
            origin_start_ii
        } else if liveness.live_in[bi].contains(&v) {
            0
        } else {
            continue;
        };
        let block = &func.blocks[bi];
        let mut clobbered = false;
        for inst in &block.instrs[start_ii..] {
            let mut used_here = false;
            inst.for_each_use(|u| {
                if u == v {
                    used_here = true;
                }
            });
            if used_here && clobbered {
                return false;
            }
            if inst_clobbers_local(&inst.op, r, liveness.aliased_locals) {
                clobbered = true;
            }
        }
        if clobbered {
            let mut used_in_term = false;
            block.term.for_each_use(|u| {
                if u == v {
                    used_in_term = true;
                }
            });
            if used_in_term || liveness.live_out[bi].contains(&v) {
                return false;
            }
        }
    }
    true
}

/// Compute pre-assigned canonical register coalescing and `Call` callee-reuse
/// hints before linear-scan allocation.
pub(super) fn compute_coalescing(
    func: &MirFn,
    dom: &DominatorTree,
    liveness: &LivenessInfo,
) -> CoalescePlan {
    let nvals = func.next_val as usize;
    let mut val_to_reg: Vec<Reg> = vec![func.num_locals; nvals];
    let mut preassigned: Vec<bool> = vec![false; nvals];

    // 1. Coalesce `LoadLocal(r)` directly into canonical `R[r]` whenever `R[r]`
    //    cannot be clobbered while the loaded value is live.
    let mut load_local_def_loc: Vec<Option<(usize, usize, Reg)>> = vec![None; nvals];
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        for (ii, inst) in func.blocks[bi].instrs.iter().enumerate() {
            if let (Some(def), MirOp::LoadLocal(r)) = (inst.def, &inst.op) {
                load_local_def_loc[def.0 as usize] = Some((bi, ii, *r));
            }
        }
    }

    for (vi, def_loc) in load_local_def_loc.into_iter().enumerate() {
        let Some((def_bi, def_ii, r)) = def_loc else {
            continue;
        };
        if (r as usize) >= liveness.local_write_positions.len() {
            continue;
        }
        let v = Val(vi as u32);
        let never_written = liveness.local_write_positions[r as usize].is_empty()
            && liveness.barrier_positions.is_empty();
        if never_written
            || is_local_unclobbered_while_live(func, dom, liveness, v, r, def_bi, def_ii + 1)
        {
            val_to_reg[vi] = r;
            preassigned[vi] = true;
        }
    }

    // 2. Coalesce `ForIter::dst` directly into the loop variable when `body_bb`
    //    immediately stores it via `StoreLocal { reg, val: dst }` as its sole use.
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        if let Terminator::ForIter { dst, body_bb, .. } = func.blocks[bi].term
            && liveness.use_count[dst.0 as usize] == 1
            && let Some(first_inst) = func.blocks[body_bb.0 as usize].instrs.first()
            && let MirOp::StoreLocal { reg, val } = first_inst.op
            && val == dst
        {
            val_to_reg[dst.0 as usize] = reg;
            preassigned[dst.0 as usize] = true;
        }
    }

    // 3. Coalesce named-local block parameters (and their single-block defining
    //    edge args) directly into canonical `R[r]` when every incoming edge
    //    already writes `StoreLocal { reg: r, val: arg }` and `r` is never
    //    clobbered while the param is live.
    let mut stored_to_local: Vec<Option<Reg>> = vec![None; nvals];
    for &bb_id in &dom.rpo {
        for inst in &func.blocks[bb_id.0 as usize].instrs {
            if let MirOp::StoreLocal { reg, val } = inst.op {
                stored_to_local[val.0 as usize] = Some(reg);
            }
        }
    }
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        for (pi, &param) in func.blocks[bi].params.iter().enumerate() {
            let mut common_reg: Option<Reg> = None;
            let mut all_match = true;
            let mut edge_args: SmallVec<[Val; 4]> = SmallVec::new();
            for &pred in &func.blocks[bi].preds {
                func.blocks[pred.0 as usize]
                    .term
                    .for_each_edge(|target, args| {
                        if target == bb_id
                            && let Some(&arg) = args.get(pi)
                        {
                            if arg == param {
                                return;
                            }
                            edge_args.push(arg);
                            match (common_reg, stored_to_local[arg.0 as usize]) {
                                (None, Some(r)) => common_reg = Some(r),
                                (Some(prev), Some(r)) if prev == r => {}
                                _ => all_match = false,
                            }
                        }
                    });
            }
            if !all_match {
                continue;
            }
            let Some(r) = common_reg else {
                continue;
            };
            if (r as usize) >= liveness.local_write_positions.len() {
                continue;
            }
            if is_local_unclobbered_while_live(func, dom, liveness, param, r, bi, 0) {
                val_to_reg[param.0 as usize] = r;
                preassigned[param.0 as usize] = true;
                for arg in edge_args {
                    if !preassigned[arg.0 as usize] && liveness.use_count[arg.0 as usize] == 2 {
                        val_to_reg[arg.0 as usize] = r;
                        preassigned[arg.0 as usize] = true;
                    }
                }
            }
        }
    }

    // 4. Record a result-reuse preference when the callee (`Call`) or the
    //    receiver (`CallMethod` / `CallMethodKw`, read before the result is
    //    written) dies right at the call. Writing the result over that dead
    //    temp also releases it as the call returns, like CPython popping its
    //    stack, so e.g. `open(p, "w", encoding=e).write(s)` flushes the file
    //    at the end of the statement (#3318).
    let mut call_reused_base: Vec<Option<Val>> = vec![None; nvals];
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        let block = &func.blocks[bi];
        for (ii, inst) in block.instrs.iter().enumerate() {
            let Some(def) = inst.def else {
                continue;
            };
            let reused = match &inst.op {
                MirOp::Call { callee, .. } => *callee,
                MirOp::SideEffectInsn {
                    insn: Insn::CallMethod { .. } | Insn::CallMethodKw(_) | Insn::CallSuperMethod(_),
                    uses,
                    defs,
                } if defs.len() == 1 => match uses.first() {
                    Some(&receiver) => receiver,
                    None => continue,
                },
                _ => continue,
            };
            let pos = liveness.block_start[bi] + 1 + ii;
            if liveness.end_pos[reused.0 as usize] <= pos && !preassigned[reused.0 as usize] {
                call_reused_base[def.0 as usize] = Some(reused);
            }
        }
    }

    CoalescePlan {
        val_to_reg,
        preassigned,
        call_reused_base,
    }
}

/// True when `op` can write directly into a target canonical local register
/// (eliminating a trailing single-use `StoreLocal`).
pub(super) fn can_coalesce_op_into_reg(op: &MirOp) -> bool {
    match op {
        MirOp::Const(_)
        | MirOp::Copy(_)
        | MirOp::BinOp { .. }
        | MirOp::UnaryOp { .. }
        | MirOp::LoadLocal(_)
        | MirOp::LoadGlobal { .. }
        | MirOp::LoadCell { .. }
        | MirOp::GetAttr { .. }
        | MirOp::GetItem { .. }
        | MirOp::Call { .. }
        | MirOp::BuildTuple(_)
        | MirOp::BuildList(_) => true,
        MirOp::SideEffectInsn { defs, .. } => defs.len() == 1,
        _ => false,
    }
}
