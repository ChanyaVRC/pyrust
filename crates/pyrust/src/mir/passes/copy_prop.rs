//! SSA copy propagation and trivial block-parameter (phi) elimination.
//!
//! 1. Eliminates `MirOp::Copy(src)` by substituting every use of its `def` with
//!    `resolve(src)` and removing the dead `Copy` instruction.
//! 2. Eliminates trivial block parameters (Braun et al.): any block parameter
//!    `p` whose incoming edge arguments are all either `p` itself or the same
//!    single value `u` is replaced with `u` and removed from `bb.params` and
//!    every incoming predecessor edge.

use smallvec::SmallVec;

use super::super::ir::{BlockId, MirFn, MirOp, Val, infer_mir_types};

/// Resolve a chain of SSA value substitutions with cycle protection.
#[inline]
fn resolve(subst: &[Val], mut v: Val) -> Val {
    for _ in 0..64 {
        let next = subst[v.0 as usize];
        if next == v {
            break;
        }
        v = next;
    }
    v
}

/// Eliminate `MirOp::Copy` instructions and trivial block parameters to fixpoint.
pub fn mir_copy_prop(func: &mut MirFn) {
    let nvals = func.next_val as usize;
    if nvals == 0 || func.blocks.is_empty() {
        return;
    }

    let mut any_changed = false;
    for _ in 0..16 {
        let mut subst: Vec<Val> = (0..nvals as u32).map(Val).collect();
        let mut round_changed = false;

        // 1. Collect all `MirOp::Copy(src)` substitutions.
        for block in &func.blocks {
            for inst in &block.instrs {
                if let (Some(def), MirOp::Copy(src)) = (inst.def, &inst.op)
                    && def != *src
                {
                    subst[def.0 as usize] = *src;
                    round_changed = true;
                }
            }
        }

        // 2. Detect trivial block parameters: every incoming argument is either
        //    the param itself or the same unique value `u` (which must not be
        //    the param itself).
        let nb = func.blocks.len();
        let mut incoming_args: Vec<Vec<SmallVec<[Val; 4]>>> = func
            .blocks
            .iter()
            .map(|b| vec![SmallVec::new(); b.params.len()])
            .collect();
        for block in &func.blocks {
            block.term.for_each_edge(|target, args| {
                let ti = target.0 as usize;
                for (pi, &arg) in args.iter().enumerate() {
                    if pi < incoming_args[ti].len() {
                        incoming_args[ti][pi].push(resolve(&subst, arg));
                    }
                }
            });
        }

        let mut drop_param: Vec<Vec<bool>> = (0..nb)
            .map(|bi| vec![false; func.blocks[bi].params.len()])
            .collect();
        for bi in 0..nb {
            for (pi, &param) in func.blocks[bi].params.iter().enumerate() {
                let mut unique: Option<Val> = None;
                let mut trivial = true;
                for &arg in &incoming_args[bi][pi] {
                    if arg == param {
                        continue;
                    }
                    match unique {
                        None => unique = Some(arg),
                        Some(prev) if prev == arg => {}
                        Some(_) => {
                            trivial = false;
                            break;
                        }
                    }
                }
                if trivial && let Some(u) = unique {
                    subst[param.0 as usize] = u;
                    drop_param[bi][pi] = true;
                    round_changed = true;
                }
            }
        }

        if !round_changed {
            break;
        }
        any_changed = true;

        // Flatten substitution chains.
        let flat: Vec<Val> = (0..nvals as u32).map(|i| resolve(&subst, Val(i))).collect();

        // 3. Apply substitutions across all instructions and terminators, and
        //    drop eliminated `MirOp::Copy` instructions.
        for block in &mut func.blocks {
            block.instrs.retain_mut(|inst| {
                if let (Some(def), MirOp::Copy(_)) = (inst.def, &inst.op)
                    && flat[def.0 as usize] != def
                {
                    return false;
                }
                inst.remap_uses(|v| flat[v.0 as usize]);
                true
            });
            block.term.remap_uses(|v| flat[v.0 as usize]);
        }

        // 4. Remove trivial block parameters and their matching edge args.
        for bi in 0..nb {
            if drop_param[bi].iter().any(|&d| d) {
                let mut keep_idx = 0usize;
                func.blocks[bi].params.retain(|_| {
                    let keep = !drop_param[bi][keep_idx];
                    keep_idx += 1;
                    keep
                });
            }
        }
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
    }

    if any_changed {
        infer_mir_types(func);
    }
}
