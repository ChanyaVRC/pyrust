//! Direct SSA constant folding pass (`mir_const_fold`).
//!
//! Because every [`Val`] in SSA MIR has a single defining operation, constant
//! folding is a single RPO scan over basic blocks with zero ad-hoc register
//! invalidation.  Delegates arithmetic/unary evaluation to canonical
//! `pyrust_core::NumericOps` via [`eval_mir_binop`] / [`eval_mir_unary`],
//! preserving exact `i64` -> `BigInt` overflow promotion (`#421`).

use crate::value::Value;

use super::super::ir::{
    MirFn, MirOp, eval_mir_binop, eval_mir_unary, infer_mir_types, is_mir_pure_const,
};
use super::super::verify::DominatorTree;

/// Fold `MirOp::BinOp` and `MirOp::UnaryOp` whose SSA operands are defined by
/// `MirOp::Const`.
pub fn mir_const_fold(func: &mut MirFn) {
    let nvals = func.next_val as usize;
    if func.blocks.is_empty() || nvals == 0 {
        return;
    }

    let dom = DominatorTree::compute(func);
    let mut use_count: Vec<u32> = vec![0; nvals];
    for block in &func.blocks {
        for inst in &block.instrs {
            inst.for_each_use(|u| use_count[u.0 as usize] += 1);
        }
        block.term.for_each_use(|u| use_count[u.0 as usize] += 1);
    }
    let mut known_const: Vec<Option<Value>> = vec![None; nvals];
    let mut direct_const: Vec<bool> = vec![false; nvals];

    // Seed with existing `MirOp::Const` definitions in RPO order so chains like
    // `(1 + 2) * (3 + 4)` fold completely in a single linear pass.
    for &bb in &dom.rpo {
        let block = &mut func.blocks[bb.0 as usize];
        for inst in &mut block.instrs {
            match &inst.op {
                MirOp::Const(v) if is_mir_pure_const(v) => {
                    if let Some(def) = inst.def {
                        known_const[def.0 as usize] = Some(v.clone());
                        direct_const[def.0 as usize] = true;
                    }
                }
                MirOp::Copy(src) => {
                    if let (Some(def), Some(v)) = (inst.def, known_const[src.0 as usize].clone()) {
                        known_const[def.0 as usize] = Some(v);
                    }
                }
                MirOp::BinOp {
                    op,
                    lhs,
                    rhs,
                    is_inplace: _,
                } => {
                    if let (Some(def), Some(lv), Some(rv)) = (
                        inst.def,
                        &known_const[lhs.0 as usize],
                        &known_const[rhs.0 as usize],
                    ) && let Some(folded) = eval_mir_binop(lv, *op, rv)
                        && is_mir_pure_const(&folded)
                    {
                        inst.op = MirOp::Const(folded.clone());
                        known_const[def.0 as usize] = Some(folded);
                        direct_const[def.0 as usize] = true;
                    }
                }
                MirOp::UnaryOp { op, arg } => {
                    if let (Some(def), Some(av)) = (inst.def, &known_const[arg.0 as usize])
                        && let Some(folded) = eval_mir_unary(*op, av)
                        && is_mir_pure_const(&folded)
                    {
                        inst.op = MirOp::Const(folded.clone());
                        known_const[def.0 as usize] = Some(folded);
                        direct_const[def.0 as usize] = true;
                    }
                }
                MirOp::BuildTuple(elems) => {
                    // Only fold literal constant tuples whose elements are
                    // single-use direct constants (not named locals like
                    // `a = 10; b = "hi"; return a, b`) so `co_consts` matches
                    // CPython's peephole tuple-constant folding exactly.
                    if let Some(def) = inst.def
                        && (1..=16).contains(&elems.len())
                        && elems.iter().all(|e| {
                            direct_const[e.0 as usize]
                                && use_count[e.0 as usize] == 1
                                && known_const[e.0 as usize].is_some()
                        })
                    {
                        let vals: Vec<Value> = elems
                            .iter()
                            .map(|e| known_const[e.0 as usize].clone().unwrap())
                            .collect();
                        let tuple_val = Value::tuple(vals);
                        inst.op = MirOp::Const(tuple_val.clone());
                        known_const[def.0 as usize] = Some(tuple_val);
                        direct_const[def.0 as usize] = true;
                    }
                }
                _ => {}
            }
        }
    }

    infer_mir_types(func);
}
