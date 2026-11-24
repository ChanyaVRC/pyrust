//! Parallel-copy de-SSA sequentialization, critical-edge trampoline planning,
//! and relative jump offset patching.

use smallvec::SmallVec;

use crate::ast::CaretSpan;
use crate::bytecode::{Insn, Reg};

use crate::mir::insn_info::jump_offset_mut;
use crate::mir::ir::{BlockId, MirFn, Terminator, Val};
use crate::mir::verify::DominatorTree;

/// Resolve a set of parallel register copies `(dst, src)` into a cycle-safe
/// sequence of `Insn::Move` instructions using `cycle_tmp` to break swap cycles
/// (e.g. `a, b = b, a`).
pub(crate) fn emit_parallel_copies(
    copies: &[(Reg, Reg)],
    cycle_tmp: Reg,
    out: &mut Vec<Insn>,
    linenos: &mut Vec<u32>,
    cols: &mut Vec<CaretSpan>,
    lineno: u32,
    col_span: CaretSpan,
) {
    let mut pending: SmallVec<[(Reg, Reg); 8]> =
        copies.iter().copied().filter(|&(d, s)| d != s).collect();
    let mut push_move = |dst: Reg, src: Reg| {
        if dst != src {
            out.push(Insn::Move(dst, src));
            linenos.push(lineno);
            cols.push(col_span);
        }
    };

    while !pending.is_empty() {
        // Find a copy whose `dst` is NOT read as `src` by any other pending copy.
        if let Some(idx) = pending
            .iter()
            .position(|&(d, _)| !pending.iter().any(|&(_, s)| s == d))
        {
            let (d, s) = pending.swap_remove(idx);
            push_move(d, s);
            continue;
        }
        // Remaining copies form one or more cycles: break one using `cycle_tmp`.
        let (_d, s) = pending[0];
        push_move(cycle_tmp, s);
        for &mut (_, ref mut src) in &mut pending {
            if *src == s {
                *src = cycle_tmp;
            }
        }
    }
}

pub(super) struct JumpPatch {
    pub(super) insn_pc: usize,
    pub(super) target: PatchTarget,
}

pub(super) enum PatchTarget {
    Block(BlockId),
    Trampoline(usize),
}

pub(super) struct EdgeTrampoline {
    pub(super) copies: Vec<(Reg, Reg)>,
    pub(super) target: BlockId,
    pub(super) lineno: u32,
    pub(super) col_span: CaretSpan,
}

/// Trampoline placement tables ensuring forward edges remain strictly forward
/// jumps and back-edge trampolines stay inside their loop / exception region.
pub(super) struct TrampolinePlan {
    pub(super) rpo_pos: Vec<usize>,
    pub(super) has_fwd_tramp_before: Vec<bool>,
    pub(super) has_back_tramp_after: Vec<bool>,
    pub(super) fwd_tramps_before: Vec<Vec<(usize, EdgeTrampoline)>>,
    pub(super) tramp_pc: Vec<usize>,
}

/// Compute the non-trivial parallel register copies required along a CFG edge
/// targeting `target` with block-parameter `args`.
pub(super) fn edge_copies(
    func: &MirFn,
    val_to_reg: &[Reg],
    target: BlockId,
    args: &[Val],
) -> Vec<(Reg, Reg)> {
    func.blocks[target.0 as usize]
        .params
        .iter()
        .zip(args.iter())
        .map(|(&p, &a)| (val_to_reg[p.0 as usize], val_to_reg[a.0 as usize]))
        .filter(|&(d, s)| d != s)
        .collect()
}

/// Pre-mark blocks that may receive a forward trampoline right before them or
/// emit a back-edge trampoline right after them.
///
/// Placing forward trampolines directly before their target keeps every forward
/// MIR edge a strictly forward bytecode jump (preventing spurious backward
/// jumps that downstream passes like `pass_licm` could mistake for loops), and
/// placing back-edge trampolines directly after their source keeps them inside
/// their loop and exception region.
pub(super) fn plan_trampolines(
    func: &MirFn,
    dom: &DominatorTree,
    val_to_reg: &[Reg],
) -> TrampolinePlan {
    let nb = func.blocks.len();
    let mut rpo_pos: Vec<usize> = vec![usize::MAX; nb];
    for (idx, &bb_id) in dom.rpo.iter().enumerate() {
        rpo_pos[bb_id.0 as usize] = idx;
    }

    let mut has_fwd_tramp_before = vec![false; nb];
    let mut has_back_tramp_after = vec![false; nb];

    for (order_idx, &bb_id) in dom.rpo.iter().enumerate() {
        let bi = bb_id.0 as usize;
        let mut note_tramp = |target: BlockId, args: &[Val]| {
            if !edge_copies(func, val_to_reg, target, args).is_empty() {
                if rpo_pos[target.0 as usize] > order_idx {
                    has_fwd_tramp_before[target.0 as usize] = true;
                } else {
                    has_back_tramp_after[bi] = true;
                }
            }
        };
        match &func.blocks[bi].term {
            Terminator::Branch {
                then_bb,
                then_args,
                else_bb,
                else_args,
                ..
            } => {
                note_tramp(*else_bb, else_args);
                if dom.rpo.get(order_idx + 1) == Some(else_bb) {
                    note_tramp(*then_bb, then_args);
                }
            }
            Terminator::ForIter {
                exit_bb, exit_args, ..
            } => note_tramp(*exit_bb, exit_args),
            Terminator::SetupExcept {
                handler_bb,
                handler_args,
                ..
            } => note_tramp(*handler_bb, handler_args),
            Terminator::MatchExcept {
                nomatch_bb,
                nomatch_args,
                ..
            }
            | Terminator::MatchExceptStar {
                nomatch_bb,
                nomatch_args,
                ..
            } => note_tramp(*nomatch_bb, nomatch_args),
            _ => {}
        }
    }

    TrampolinePlan {
        rpo_pos,
        has_fwd_tramp_before,
        has_back_tramp_after,
        fwd_tramps_before: (0..nb).map(|_| Vec::new()).collect(),
        tramp_pc: Vec::new(),
    }
}

/// Patch all relative jump offsets once every block and trampoline PC is known.
pub(super) fn patch_jumps(
    insns: &mut [Insn],
    patches: Vec<JumpPatch>,
    block_pc: &[usize],
    tramp_pc: &[usize],
) {
    for patch in patches {
        let target_pc = match patch.target {
            PatchTarget::Block(bb) => block_pc[bb.0 as usize],
            PatchTarget::Trampoline(tid) => tramp_pc[tid],
        };
        let off = target_pc as i32 - (patch.insn_pc as i32 + 1);
        if let Some(slot) = jump_offset_mut(&mut insns[patch.insn_pc]) {
            *slot = off;
        } else {
            unreachable!("non-jump in JumpPatch: {:?}", insns[patch.insn_pc]);
        }
    }
}
