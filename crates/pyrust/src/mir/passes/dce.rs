//! Dead SSA instruction, dead block-parameter, and unreachable-block
//! elimination (`mir_dce`).
//!
//! Removes any pure, proven non-raising `MirInst` whose `def` has zero uses in
//! the SSA use-def graph (iterating to fixpoint to catch cascading dead chains),
//! strips unused block parameters, and compacts unreachable basic blocks.

use super::super::ir::{
    BlockId, MirFn, MirOp, MirType, infer_mir_types, is_proven_non_raising_binop,
    is_proven_non_raising_unary,
};
use super::prune_unreachable_blocks;

/// True when `op` is pure, non-reentrant, and incapable of raising an exception
/// so removing it when its `def` has 0 uses preserves exact Python semantics.
fn is_pure_removable_op(op: &MirOp, types: &[MirType]) -> bool {
    match op {
        MirOp::Const(_) | MirOp::Copy(_) | MirOp::BuildTuple(_) | MirOp::BuildList(_) => true,
        MirOp::BinOp {
            op,
            lhs,
            rhs,
            is_inplace: false,
        } => {
            let lt = &types[lhs.0 as usize];
            let rt = &types[rhs.0 as usize];
            is_proven_non_raising_binop(*op, lt, rt)
        }
        MirOp::UnaryOp { op, arg } => {
            let at = &types[arg.0 as usize];
            is_proven_non_raising_unary(*op, at)
        }
        _ => false,
    }
}

/// Prune unreachable blocks, dead pure SSA instructions, and dead block params.
pub fn mir_dce(func: &mut MirFn) {
    if func.blocks.is_empty() {
        return;
    }
    prune_unreachable_blocks(func);
    infer_mir_types(func);

    let nvals = func.next_val as usize;
    if nvals == 0 {
        return;
    }

    for _ in 0..16 {
        let mut use_count = vec![0u32; nvals];
        for block in &func.blocks {
            for inst in &block.instrs {
                inst.for_each_use(|u| use_count[u.0 as usize] += 1);
            }
            block.term.for_each_use(|u| use_count[u.0 as usize] += 1);
        }

        let types = &func.val_types;
        let mut changed = false;

        // 1. Remove pure dead instructions.
        for block in &mut func.blocks {
            let before = block.instrs.len();
            block.instrs.retain(|inst| {
                if let Some(def) = inst.def
                    && use_count[def.0 as usize] == 0
                    && is_pure_removable_op(&inst.op, types)
                {
                    return false;
                }
                true
            });
            if block.instrs.len() != before {
                changed = true;
            }
        }

        // 2. Remove unused block parameters (0 uses anywhere in instructions or
        //    terminators).
        let nb = func.blocks.len();
        let mut drop_param: Vec<Vec<bool>> = (0..nb)
            .map(|bi| {
                func.blocks[bi]
                    .params
                    .iter()
                    .map(|&p| use_count[p.0 as usize] == 0)
                    .collect()
            })
            .collect();

        // Only drop params that have 0 uses (including in outgoing edge args).
        let mut any_param_dropped = false;
        for bi in 0..nb {
            if drop_param[bi].iter().any(|&d| d) {
                any_param_dropped = true;
                let mut idx = 0usize;
                func.blocks[bi].params.retain(|_| {
                    let keep = !drop_param[bi][idx];
                    idx += 1;
                    keep
                });
            }
        }
        if any_param_dropped {
            changed = true;
            for block in &mut func.blocks {
                block.term.for_each_edge_mut(|target: &mut BlockId, args| {
                    let ti = target.0 as usize;
                    if drop_param[ti].iter().any(|&d| d) {
                        let mut idx = 0usize;
                        args.retain(|_| {
                            let keep = !drop_param[ti][idx];
                            idx += 1;
                            keep
                        });
                    }
                });
            }
            // Clear `drop_param` reference cleanly before next iteration.
            drop_param.clear();
        }

        if !changed {
            break;
        }
    }
}
