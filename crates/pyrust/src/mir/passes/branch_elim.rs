//! Constant and redundant branch elimination (`mir_const_branch_elim`).
//!
//! Replaces any `Terminator::Branch` whose condition is a compile-time constant
//! (or whose `then` and `else` edges are identical and whose condition is a
//! pure primitive) with an unconditional `Terminator::Jump`, pruning any blocks
//! that become unreachable.

use super::super::ir::{MirFn, Terminator, infer_mir_types, mir_const_truthy};
use super::prune_unreachable_blocks;

/// Simplify constant and degenerate conditional branches and prune unreachable
/// blocks.
pub fn mir_const_branch_elim(func: &mut MirFn) {
    if func.blocks.is_empty() {
        return;
    }
    infer_mir_types(func);
    let types = func.val_types.clone();
    let mut changed = false;

    for block in &mut func.blocks {
        if let Terminator::Branch {
            cond,
            then_bb,
            then_args,
            else_bb,
            else_args,
        } = &block.term
        {
            let cond_ty = &types[cond.0 as usize];
            if let Some(const_val) = cond_ty.as_const()
                && let Some(taken) = mir_const_truthy(const_val)
            {
                let (target, args) = if taken {
                    (*then_bb, then_args.clone())
                } else {
                    (*else_bb, else_args.clone())
                };
                block.term = Terminator::Jump { target, args };
                changed = true;
            } else if then_bb == else_bb
                && then_args == else_args
                && cond_ty.is_pure_truthy_primitive()
            {
                block.term = Terminator::Jump {
                    target: *then_bb,
                    args: then_args.clone(),
                };
                changed = true;
            }
        }
    }

    if changed {
        prune_unreachable_blocks(func);
    }
}
