//! SSA-based MIR optimizer pass suite (issue #457).
//!
//! Includes 7 ported core passes plus 1 previously parked optimization (`#357`):
//! 1. [`mir_sccp`] — Sparse Conditional Constant Propagation over SSA lattice
//! 2. [`mir_const_fold`] — Direct SSA constant folding (`#421`-safe)
//! 3. [`mir_algebraic_simplify`] — Type-proven algebraic identities (`#438`-safe)
//! 4. [`mir_copy_prop`] — Copy & trivial block-parameter elimination
//! 5. [`mir_cse`] — Dominator-tree-scoped Global Value Numbering / CSE
//! 6. [`mir_const_branch_elim`] — Constant / redundant branch elimination
//! 7. [`mir_dce`] — Dead SSA instruction & unreachable-block elimination
//! 8. [`mir_reassociate`] — Type-proven SSA constant reassociation (`#357`)

pub mod algebraic;
pub mod branch_elim;
pub mod const_fold;
pub mod copy_prop;
pub mod cse;
pub mod dce;
pub mod reassociate;
pub mod sccp;

pub use algebraic::mir_algebraic_simplify;
pub use branch_elim::mir_const_branch_elim;
pub use const_fold::mir_const_fold;
pub use copy_prop::mir_copy_prop;
pub use cse::mir_cse;
pub use dce::mir_dce;
pub use reassociate::mir_reassociate;
pub use sccp::mir_sccp;

use super::ir::{BlockId, MirFn, infer_mir_types};

/// Remove all basic blocks unreachable from `func.entry`, compacting
/// [`BlockId`]s and rebuilding `preds`.
pub(crate) fn prune_unreachable_blocks(func: &mut MirFn) {
    let n = func.blocks.len();
    if n == 0 {
        return;
    }
    let mut reachable = vec![false; n];
    let mut work = vec![func.entry];
    reachable[func.entry.0 as usize] = true;
    while let Some(bb) = work.pop() {
        for succ in func.blocks[bb.0 as usize].term.successors() {
            let si = succ.0 as usize;
            if si < n && !reachable[si] {
                reachable[si] = true;
                work.push(succ);
            }
        }
    }
    if reachable.iter().all(|&r| r) {
        func.rebuild_preds();
        return;
    }

    let mut old_to_new: Vec<Option<BlockId>> = vec![None; n];
    let mut new_blocks = Vec::with_capacity(n);
    for (old_idx, mut block) in func.blocks.drain(..).enumerate() {
        if reachable[old_idx] {
            let new_id = BlockId(new_blocks.len() as u32);
            old_to_new[old_idx] = Some(new_id);
            block.id = new_id;
            new_blocks.push(block);
        }
    }
    func.entry = old_to_new[func.entry.0 as usize].expect("entry block must remain reachable");
    for block in &mut new_blocks {
        block.term.for_each_edge_mut(|target, _| {
            *target = old_to_new[target.0 as usize].expect("reachable edge target must survive");
        });
    }
    func.blocks = new_blocks;
    func.rebuild_preds();
}

/// Run the full SSA MIR optimization pipeline on `func`.
pub fn run_mir_passes(func: &mut MirFn) {
    mir_sccp(func);
    mir_const_fold(func);
    mir_copy_prop(func);
    infer_mir_types(func);
    mir_reassociate(func);
    mir_const_fold(func);
    infer_mir_types(func);
    mir_algebraic_simplify(func);
    mir_copy_prop(func);
    infer_mir_types(func);
    mir_cse(func);
    mir_copy_prop(func);
    mir_const_branch_elim(func);
    mir_dce(func);
    mir_copy_prop(func);
    mir_dce(func);
    infer_mir_types(func);
}
