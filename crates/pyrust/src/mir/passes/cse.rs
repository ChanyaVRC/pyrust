//! Dominator-tree-scoped Global Value Numbering / Common Subexpression
//! Elimination (`mir_cse`).
//!
//! Walks basic blocks in dominator-tree DFS preorder, deduplicating strictly
//! pure, proven non-raising SSA expressions (`Const`, type-proven non-raising
//! `BinOp`, and type-proven non-raising `UnaryOp`) across dominated blocks.
//! Matches are replaced with `MirOp::Copy(prev_val)` so [`super::mir_copy_prop`]
//! eliminates them cleanly.

use std::collections::HashMap;

use crate::ast::{BinaryOp, UnaryOp};

use super::super::ir::{
    BlockId, MirFn, MirOp, MirType, Val, infer_mir_types, is_proven_non_raising_binop,
    is_proven_non_raising_unary,
};
use super::super::verify::DominatorTree;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum GvnKey {
    BinOp(BinaryOp, Val, Val),
    UnaryOp(UnaryOp, Val),
}

fn gvn_key_for_op(op: &MirOp, types: &[MirType]) -> Option<GvnKey> {
    // Deliberately do NOT CSE `MirOp::Const`: cheap constants are fused into
    // `BinOpImm` / `BinOpConst` / `CmpJump*Const` by the bytecode backend, and
    // deduplicating them across blocks extends register live ranges and blocks
    // immediate fusion.
    match op {
        MirOp::BinOp {
            op,
            lhs,
            rhs,
            is_inplace: false,
        } => {
            let lt = &types[lhs.0 as usize];
            let rt = &types[rhs.0 as usize];
            if !is_proven_non_raising_binop(*op, lt, rt) {
                return None;
            }
            // Canonicalize commutative operations on exact integers.
            let (a, b) = if lt.is_int()
                && rt.is_int()
                && matches!(
                    op,
                    BinaryOp::Add
                        | BinaryOp::Mul
                        | BinaryOp::BitAnd
                        | BinaryOp::BitOr
                        | BinaryOp::BitXor
                        | BinaryOp::Eq
                        | BinaryOp::Ne
                ) {
                if lhs.0 <= rhs.0 {
                    (*lhs, *rhs)
                } else {
                    (*rhs, *lhs)
                }
            } else {
                (*lhs, *rhs)
            };
            Some(GvnKey::BinOp(*op, a, b))
        }
        MirOp::UnaryOp { op, arg } => {
            let at = &types[arg.0 as usize];
            if !is_proven_non_raising_unary(*op, at) {
                return None;
            }
            Some(GvnKey::UnaryOp(*op, *arg))
        }
        _ => None,
    }
}

/// Dominator-tree-scoped GVN / CSE over pure, proven non-raising SSA ops.
pub fn mir_cse(func: &mut MirFn) {
    if func.blocks.is_empty() {
        return;
    }
    infer_mir_types(func);
    let dom = DominatorTree::compute(func);
    let types = func.val_types.clone();

    let mut table: HashMap<GvnKey, Val> = HashMap::new();

    fn walk(
        bb: BlockId,
        func: &mut MirFn,
        dom: &DominatorTree,
        types: &[MirType],
        table: &mut HashMap<GvnKey, Val>,
    ) {
        let mut inserted: Vec<GvnKey> = Vec::new();
        let bi = bb.0 as usize;
        for inst in &mut func.blocks[bi].instrs {
            let Some(def) = inst.def else {
                continue;
            };
            if let Some(key) = gvn_key_for_op(&inst.op, types) {
                if let Some(&prev_val) = table.get(&key) {
                    inst.op = MirOp::Copy(prev_val);
                } else {
                    table.insert(key.clone(), def);
                    inserted.push(key);
                }
            }
        }

        for &child in &dom.children[bi] {
            walk(child, func, dom, types, table);
        }

        for key in inserted {
            table.remove(&key);
        }
    }

    walk(func.entry, func, &dom, &types, &mut table);
}
