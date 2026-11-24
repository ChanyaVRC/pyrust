//! Linear-scan register allocator for SSA MIR values.

use crate::bytecode::Reg;

use crate::mir::ir::{MirFn, Val};
use crate::mir::verify::DominatorTree;

use super::coalesce::compute_coalescing;
use super::liveness::compute_liveness_and_intervals;

/// Register assignment and liveness metadata produced by [`allocate_ssa_registers`].
pub(super) struct RegAllocResult {
    pub(super) val_to_reg: Vec<Reg>,
    pub(super) end_pos: Vec<usize>,
    pub(super) use_count: Vec<u32>,
    pub(super) ssa_reg_end: Reg,
}

/// Allocate physical registers (`num_locals..`) for all SSA [`Val`]s via
/// exact liveness, canonical-local coalescing, and linear scan.
pub(super) fn allocate_ssa_registers(func: &MirFn, dom: &DominatorTree) -> RegAllocResult {
    let nvals = func.next_val as usize;
    let liveness = compute_liveness_and_intervals(func, dom);
    let mut plan = compute_coalescing(func, dom, &liveness);

    // Linear-scan allocation over all non-preassigned `[start, end]` intervals.
    let mut intervals: Vec<(usize, usize, Val)> = Vec::new();
    for vi in 0..nvals {
        if liveness.start_pos[vi] != usize::MAX && !plan.preassigned[vi] {
            intervals.push((liveness.start_pos[vi], liveness.end_pos[vi], Val(vi as u32)));
        }
    }
    intervals.sort_unstable_by_key(|&(s, e, v)| (s, e, v.0));

    let mut active: Vec<(usize, Reg)> = Vec::new(); // (end, reg)
    let mut free_regs: Vec<Reg> = Vec::new();
    let mut next_reg: Reg = func.num_locals;

    for (s, e, v) in intervals {
        // Expire intervals that ended strictly before `s`.
        let mut i = 0;
        while i < active.len() {
            if active[i].0 < s {
                let (_, r) = active.swap_remove(i);
                free_regs.push(r);
            } else {
                i += 1;
            }
        }
        // If `v` is the result of a `Call` whose `callee` died right at `s`,
        // prefer `callee`'s register (`ae == s` is `callee` itself, which is
        // read before `Call` writes its return value into `cr`).
        let preferred = plan.call_reused_base[v.0 as usize].and_then(|callee| {
            let cr = plan.val_to_reg[callee.0 as usize];
            if cr >= func.num_locals && !active.iter().any(|&(ae, ar)| ar == cr && ae > s) {
                Some(cr)
            } else {
                None
            }
        });
        let reg = if let Some(pref) = preferred {
            // Remove any expiring `(s, pref)` entry so `pref` is not double-freed.
            active.retain(|&(ae, ar)| !(ar == pref && ae <= s));
            if let Some(pos) = free_regs.iter().position(|&r| r == pref) {
                free_regs.swap_remove(pos);
            }
            pref
        } else if let Some((best_idx, &best_r)) =
            free_regs.iter().enumerate().min_by_key(|&(_, &r)| r)
        {
            free_regs.swap_remove(best_idx);
            best_r
        } else {
            let r = next_reg;
            next_reg += 1;
            r
        };
        plan.val_to_reg[v.0 as usize] = reg;
        active.push((e, reg));
    }

    RegAllocResult {
        val_to_reg: plan.val_to_reg,
        end_pos: liveness.end_pos,
        use_count: liveness.use_count,
        ssa_reg_end: next_reg,
    }
}
