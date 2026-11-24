//! Type-proven SSA constant reassociation (parked optimization `#357`).
//!
//! Because Python integer arithmetic over `int` (`Int` / `BigInt`) is exact,
//! associative, and commutative over $\mathbb{Z}$ (overflow promotes to
//! `BigInt`, never wraps), when SSA [`MirType`] provenance proves `x: Int`:
//! - `(x + c1) + c2` -> `x + (c1 + c2)`
//! - `(x + c1) - c2` -> `x + (c1 - c2)`
//! - `(x - c1) + c2` -> `x + (c2 - c1)`
//! - `(x - c1) - c2` -> `x - (c1 + c2)`
//! - `(x * c1) * c2` -> `x * (c1 * c2)`
//! - `(x & c1) & c2` -> `x & (c1 & c2)`
//! - `(x | c1) | c2` -> `x | (c1 | c2)`
//! - `(x ^ c1) ^ c2` -> `x ^ (c1 ^ c2)`
//!
//! whenever the combined constant fits in `i64` without overflow.
//!
//! Strictly guarded on `MirType::Int` (never fires on `Unknown` user objects,
//! `Float`, or `Bool`).

use crate::ast::BinaryOp;
use crate::value::{Value, ValueKind};

use super::super::ir::{MirFn, MirInst, MirOp, MirType, Val, infer_mir_types};

#[derive(Clone, Copy)]
struct IntBinDef {
    op: BinaryOp,
    lhs: Val,
    rhs: Val,
}

#[inline]
fn exact_i64(ty: &MirType) -> Option<i64> {
    match ty.as_const().map(Value::kind) {
        Some(ValueKind::Int(n)) => Some(n),
        _ => None,
    }
}

/// Extract `(non_const_int_operand, const_i64)` from an associative/commutative
/// binary def `a op b` where one operand is a constant `i64` and the other is
/// proven `MirType::Int`.
fn split_commutative_int_const(
    op: BinaryOp,
    lhs: Val,
    rhs: Val,
    types: &[MirType],
) -> Option<(Val, i64)> {
    let lt = &types[lhs.0 as usize];
    let rt = &types[rhs.0 as usize];
    if !lt.is_int() || !rt.is_int() {
        return None;
    }
    if let Some(c) = exact_i64(rt) {
        return Some((lhs, c));
    }
    if matches!(
        op,
        BinaryOp::Add | BinaryOp::Mul | BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor
    ) && let Some(c) = exact_i64(lt)
    {
        return Some((rhs, c));
    }
    None
}

/// Reassociate constant integer chains `(x op c1) op c2 -> x op (c1 op c2)`.
pub fn mir_reassociate(func: &mut MirFn) {
    if func.blocks.is_empty() {
        return;
    }
    infer_mir_types(func);

    let nvals = func.next_val as usize;
    let mut bin_defs: Vec<Option<IntBinDef>> = vec![None; nvals];
    for block in &func.blocks {
        for inst in &block.instrs {
            if let (
                Some(def),
                MirOp::BinOp {
                    op,
                    lhs,
                    rhs,
                    is_inplace: false,
                },
            ) = (inst.def, &inst.op)
            {
                bin_defs[def.0 as usize] = Some(IntBinDef {
                    op: *op,
                    lhs: *lhs,
                    rhs: *rhs,
                });
            }
        }
    }

    let mut types = func.val_types.clone();
    let mut changed = false;

    for bi in 0..func.blocks.len() {
        let mut new_instrs: Vec<MirInst> = Vec::with_capacity(func.blocks[bi].instrs.len());
        let old_instrs = std::mem::take(&mut func.blocks[bi].instrs);

        for mut inst in old_instrs {
            let rewritten: Option<(BinaryOp, Val, i64)> = if let MirOp::BinOp {
                op,
                lhs,
                rhs,
                is_inplace: false,
            } = inst.op
            {
                if let Some((inner_val, c2)) = split_commutative_int_const(op, lhs, rhs, &types)
                    && let Some(inner) = bin_defs.get(inner_val.0 as usize).copied().flatten()
                    && let Some((x, c1)) =
                        split_commutative_int_const(inner.op, inner.lhs, inner.rhs, &types)
                {
                    match (inner.op, op) {
                        (BinaryOp::Add, BinaryOp::Add) => {
                            c1.checked_add(c2).map(|c| (BinaryOp::Add, x, c))
                        }
                        (BinaryOp::Add, BinaryOp::Sub) => {
                            c1.checked_sub(c2).map(|c| (BinaryOp::Add, x, c))
                        }
                        (BinaryOp::Sub, BinaryOp::Add) => {
                            c2.checked_sub(c1).map(|c| (BinaryOp::Add, x, c))
                        }
                        (BinaryOp::Sub, BinaryOp::Sub) => {
                            c1.checked_add(c2).map(|c| (BinaryOp::Sub, x, c))
                        }
                        (BinaryOp::Mul, BinaryOp::Mul) => {
                            c1.checked_mul(c2).map(|c| (BinaryOp::Mul, x, c))
                        }
                        (BinaryOp::BitAnd, BinaryOp::BitAnd) => {
                            Some((BinaryOp::BitAnd, x, c1 & c2))
                        }
                        (BinaryOp::BitOr, BinaryOp::BitOr) => Some((BinaryOp::BitOr, x, c1 | c2)),
                        (BinaryOp::BitXor, BinaryOp::BitXor) => {
                            Some((BinaryOp::BitXor, x, c1 ^ c2))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some((new_op, x, combined_c)) = rewritten {
                let c_ty = MirType::ExactConst(Value::int(combined_c));
                let c_val = func.fresh_val_typed(c_ty.clone());
                if types.len() <= c_val.0 as usize {
                    types.resize(c_val.0 as usize + 1, MirType::Unknown);
                }
                types[c_val.0 as usize] = c_ty;
                new_instrs.push(MirInst {
                    def: Some(c_val),
                    op: MirOp::Const(Value::int(combined_c)),
                    lineno: inst.lineno,
                    col_span: inst.col_span,
                });
                inst.op = MirOp::BinOp {
                    op: new_op,
                    lhs: x,
                    rhs: c_val,
                    is_inplace: false,
                };
                if let Some(def) = inst.def {
                    if bin_defs.len() <= def.0 as usize {
                        bin_defs.resize(def.0 as usize + 1, None);
                    }
                    bin_defs[def.0 as usize] = Some(IntBinDef {
                        op: new_op,
                        lhs: x,
                        rhs: c_val,
                    });
                }
                changed = true;
            }
            new_instrs.push(inst);
        }

        func.blocks[bi].instrs = new_instrs;
    }

    if changed {
        infer_mir_types(func);
    }
}
