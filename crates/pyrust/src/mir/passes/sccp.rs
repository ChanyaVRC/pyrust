//! Sparse Conditional Constant Propagation (Wegman-Zadeck) over SSA MIR.
//!
//! Operates directly on the SSA `Val` lattice (`Top` / `Const(Value)` / `Bottom`)
//! and explicit block-parameter phi edges.  Delegates constant arithmetic and
//! unary evaluation to canonical `pyrust_core::NumericOps` via
//! [`eval_mir_binop`] / [`eval_mir_unary`], structurally preventing `#421`
//! overflow bugs.

use std::collections::HashSet;

use crate::value::Value;

use super::super::ir::{
    BlockId, MirFn, MirOp, Terminator, Val, eval_mir_binop, eval_mir_unary, infer_mir_types,
    is_mir_pure_const, mir_const_exact_eq, mir_const_truthy,
};
use super::prune_unreachable_blocks;

#[derive(Debug, Clone)]
enum Lattice {
    Top,
    Const(Value),
    Bottom,
}

impl Lattice {
    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (Lattice::Top, x) | (x, Lattice::Top) => x.clone(),
            (Lattice::Bottom, _) | (_, Lattice::Bottom) => Lattice::Bottom,
            (Lattice::Const(a), Lattice::Const(b)) => {
                if mir_const_exact_eq(a, b) {
                    Lattice::Const(a.clone())
                } else {
                    Lattice::Bottom
                }
            }
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        match (self, other) {
            (Lattice::Top, Lattice::Top) | (Lattice::Bottom, Lattice::Bottom) => true,
            (Lattice::Const(a), Lattice::Const(b)) => mir_const_exact_eq(a, b),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum UseSite {
    Inst(BlockId, usize),
    Term(BlockId),
}

/// Run Sparse Conditional Constant Propagation on `func`.
pub fn mir_sccp(func: &mut MirFn) {
    let nblocks = func.blocks.len();
    let nvals = func.next_val as usize;
    if nblocks == 0 {
        return;
    }

    // Build SSA use-def index so updating a Val immediately re-queues only its
    // dependent instructions and terminators.
    let mut uses: Vec<Vec<UseSite>> = vec![Vec::new(); nvals];
    for block in &func.blocks {
        for (i_idx, inst) in block.instrs.iter().enumerate() {
            inst.for_each_use(|u| {
                if (u.0 as usize) < nvals {
                    uses[u.0 as usize].push(UseSite::Inst(block.id, i_idx));
                }
            });
        }
        block.term.for_each_use(|u| {
            if (u.0 as usize) < nvals {
                uses[u.0 as usize].push(UseSite::Term(block.id));
            }
        });
    }

    let mut lattice: Vec<Lattice> = vec![Lattice::Top; nvals];
    let mut block_exec: Vec<bool> = vec![false; nblocks];
    let mut exec_edges: HashSet<(BlockId, BlockId)> = HashSet::new();
    let mut cfg_worklist: Vec<(Option<BlockId>, BlockId)> = vec![(None, func.entry)];
    let mut ssa_worklist: Vec<Val> = Vec::new();

    let update_val =
        |v: Val, next: Lattice, lattice: &mut [Lattice], ssa_worklist: &mut Vec<Val>| {
            let idx = v.0 as usize;
            if idx < lattice.len() && !lattice[idx].same_as(&next) {
                lattice[idx] = next;
                ssa_worklist.push(v);
            }
        };

    let eval_inst = |bb: BlockId,
                     i_idx: usize,
                     func: &MirFn,
                     lattice: &mut [Lattice],
                     ssa_worklist: &mut Vec<Val>| {
        let inst = &func.blocks[bb.0 as usize].instrs[i_idx];
        let next = match &inst.op {
            MirOp::Const(v) => {
                if is_mir_pure_const(v) {
                    Lattice::Const(v.clone())
                } else {
                    Lattice::Bottom
                }
            }
            MirOp::Copy(src) => lattice[src.0 as usize].clone(),
            MirOp::BinOp {
                op,
                lhs,
                rhs,
                is_inplace: _,
            } => match (&lattice[lhs.0 as usize], &lattice[rhs.0 as usize]) {
                (Lattice::Top, _) | (_, Lattice::Top) => Lattice::Top,
                (Lattice::Const(lv), Lattice::Const(rv)) => match eval_mir_binop(lv, *op, rv) {
                    Some(folded) if is_mir_pure_const(&folded) => Lattice::Const(folded),
                    _ => Lattice::Bottom,
                },
                _ => Lattice::Bottom,
            },
            MirOp::UnaryOp { op, arg } => match &lattice[arg.0 as usize] {
                Lattice::Top => Lattice::Top,
                Lattice::Const(av) => match eval_mir_unary(*op, av) {
                    Some(folded) if is_mir_pure_const(&folded) => Lattice::Const(folded),
                    _ => Lattice::Bottom,
                },
                Lattice::Bottom => Lattice::Bottom,
            },
            _ => Lattice::Bottom,
        };
        if let Some(def) = inst.def {
            update_val(def, next, lattice, ssa_worklist);
        }
        if let MirOp::SideEffectInsn { defs, .. } = &inst.op {
            for &d in defs {
                if Some(d) != inst.def {
                    update_val(d, Lattice::Bottom, lattice, ssa_worklist);
                }
            }
        }
    };

    let eval_term = |bb: BlockId,
                     func: &MirFn,
                     lattice: &mut [Lattice],
                     exec_edges: &mut HashSet<(BlockId, BlockId)>,
                     cfg_worklist: &mut Vec<(Option<BlockId>, BlockId)>,
                     ssa_worklist: &mut Vec<Val>| {
        let block = &func.blocks[bb.0 as usize];
        block.term.for_each_def(|d| {
            update_val(d, Lattice::Bottom, lattice, ssa_worklist);
        });
        let mut mark_edge = |target: BlockId| {
            if exec_edges.insert((bb, target)) {
                cfg_worklist.push((Some(bb), target));
            }
        };
        match &block.term {
            Terminator::Jump { target, .. } => mark_edge(*target),
            Terminator::Branch {
                cond,
                then_bb,
                else_bb,
                ..
            } => match &lattice[cond.0 as usize] {
                Lattice::Top => {}
                Lattice::Const(v) => match mir_const_truthy(v) {
                    Some(true) => mark_edge(*then_bb),
                    Some(false) => mark_edge(*else_bb),
                    None => {
                        mark_edge(*then_bb);
                        mark_edge(*else_bb);
                    }
                },
                Lattice::Bottom => {
                    mark_edge(*then_bb);
                    mark_edge(*else_bb);
                }
            },
            Terminator::ForIter {
                body_bb, exit_bb, ..
            } => {
                mark_edge(*body_bb);
                mark_edge(*exit_bb);
            }
            Terminator::SetupExcept {
                try_bb, handler_bb, ..
            } => {
                mark_edge(*try_bb);
                mark_edge(*handler_bb);
            }
            Terminator::MatchExcept {
                matched_bb,
                nomatch_bb,
                ..
            }
            | Terminator::MatchExceptStar {
                matched_bb,
                nomatch_bb,
                ..
            } => {
                mark_edge(*matched_bb);
                mark_edge(*nomatch_bb);
            }
            Terminator::Return(_)
            | Terminator::Raise(_)
            | Terminator::RaiseSpecial { .. }
            | Terminator::Unreachable => {}
        }
    };

    let eval_block_params = |target: BlockId,
                             func: &MirFn,
                             exec_edges: &HashSet<(BlockId, BlockId)>,
                             lattice: &mut [Lattice],
                             ssa_worklist: &mut Vec<Val>| {
        let tb = &func.blocks[target.0 as usize];
        if tb.params.is_empty() {
            return;
        }
        let mut acc: Vec<Lattice> = vec![Lattice::Top; tb.params.len()];
        for &pred in &tb.preds {
            if !exec_edges.contains(&(pred, target)) {
                continue;
            }
            func.blocks[pred.0 as usize]
                .term
                .for_each_edge(|succ, args| {
                    if succ == target {
                        for (slot, &arg) in acc.iter_mut().zip(args.iter()) {
                            *slot = slot.meet(&lattice[arg.0 as usize]);
                        }
                    }
                });
        }
        for (&p, next) in tb.params.iter().zip(acc) {
            update_val(p, next, lattice, ssa_worklist);
        }
    };

    while !cfg_worklist.is_empty() || !ssa_worklist.is_empty() {
        while let Some((_from, bb)) = cfg_worklist.pop() {
            let bi = bb.0 as usize;
            let first_visit = !block_exec[bi];
            block_exec[bi] = true;
            eval_block_params(bb, func, &exec_edges, &mut lattice, &mut ssa_worklist);
            if first_visit {
                let n_instrs = func.blocks[bi].instrs.len();
                for i_idx in 0..n_instrs {
                    eval_inst(bb, i_idx, func, &mut lattice, &mut ssa_worklist);
                }
                eval_term(
                    bb,
                    func,
                    &mut lattice,
                    &mut exec_edges,
                    &mut cfg_worklist,
                    &mut ssa_worklist,
                );
            }
        }

        while let Some(v) = ssa_worklist.pop() {
            for &site in &uses[v.0 as usize] {
                match site {
                    UseSite::Inst(bb, i_idx) => {
                        if block_exec[bb.0 as usize] {
                            eval_inst(bb, i_idx, func, &mut lattice, &mut ssa_worklist);
                        }
                    }
                    UseSite::Term(bb) => {
                        if block_exec[bb.0 as usize] {
                            eval_term(
                                bb,
                                func,
                                &mut lattice,
                                &mut exec_edges,
                                &mut cfg_worklist,
                                &mut ssa_worklist,
                            );
                            // Also re-evaluate block parameters of any executable
                            // successor whose incoming edge passes `v`.
                            let succs = func.blocks[bb.0 as usize].term.successors();
                            for succ in succs {
                                if exec_edges.contains(&(bb, succ)) {
                                    eval_block_params(
                                        succ,
                                        func,
                                        &exec_edges,
                                        &mut lattice,
                                        &mut ssa_worklist,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Rewrite phase ────────────────────────────────────────────────────────
    // 1. Replace foldable pure instructions whose `def` resolved to `Const(v)`
    //    with `MirOp::Const(v)` (keeping source line & PEP 657 caret span).
    for (bi, block) in func.blocks.iter_mut().enumerate() {
        if !block_exec[bi] {
            continue;
        }
        for inst in &mut block.instrs {
            let is_foldable_op = matches!(inst.op, MirOp::BinOp { .. } | MirOp::UnaryOp { .. });
            if is_foldable_op
                && let Some(def) = inst.def
                && let Lattice::Const(ref val) = lattice[def.0 as usize]
            {
                inst.op = MirOp::Const(val.clone());
            }
        }

        // 2. Replace constant-condition `Branch` terminators with `Jump`.
        if let Terminator::Branch {
            cond,
            then_bb,
            then_args,
            else_bb,
            else_args,
        } = &block.term
            && let Lattice::Const(ref val) = lattice[cond.0 as usize]
            && let Some(taken) = mir_const_truthy(val)
        {
            let (target, args) = if taken {
                (*then_bb, then_args.clone())
            } else {
                (*else_bb, else_args.clone())
            };
            block.term = Terminator::Jump { target, args };
        }
    }

    // 3. Replace any block parameter that resolved to `Const(v)` by inserting
    //    `MirOp::Const(v)` for a fresh copy — even cleaner: replace all uses of
    //    a constant block parameter with a `Const(v)` defined at the top of its
    //    block, then `mir_copy_prop` / `mir_dce` removes the dead block param!
    let mut param_const_replacements: Vec<(BlockId, Val, Value)> = Vec::new();
    for (bi, block) in func.blocks.iter().enumerate() {
        if !block_exec[bi] {
            continue;
        }
        for &p in &block.params {
            if let Lattice::Const(ref val) = lattice[p.0 as usize] {
                param_const_replacements.push((block.id, p, val.clone()));
            }
        }
    }
    if !param_const_replacements.is_empty() {
        let mut remap: Vec<Val> = (0..func.next_val).map(Val).collect();
        for (bb, param_val, const_val) in param_const_replacements {
            let fresh = func.fresh_val();
            remap.push(fresh);
            remap[param_val.0 as usize] = fresh;
            func.blocks[bb.0 as usize].instrs.insert(
                0,
                super::super::ir::MirInst {
                    def: Some(fresh),
                    op: MirOp::Const(const_val),
                    lineno: 0,
                    col_span: (0, 0, 0, 0),
                },
            );
        }
        for block in &mut func.blocks {
            for inst in &mut block.instrs {
                inst.remap_uses(|v| remap.get(v.0 as usize).copied().unwrap_or(v));
            }
            block
                .term
                .remap_uses(|v| remap.get(v.0 as usize).copied().unwrap_or(v));
        }
    }

    prune_unreachable_blocks(func);
    infer_mir_types(func);
}
