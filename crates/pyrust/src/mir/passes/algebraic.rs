//! Type-proven SSA algebraic simplification pass (`mir_algebraic_simplify`).
//!
//! Strictly guarded by SSA [`MirType`] provenance: simplifies algebraic
//! identities (`x + 0 -> x`, `x - 0 -> x`, `x * 1 -> x`, `x * 0 -> 0`,
//! `x | 0 -> x`, `x ^ 0 -> x`, `x & 0 -> 0`, `x - x -> 0`, `x // 1 -> x`,
//! `x << 0 -> x`, `x >> 0 -> x`) ONLY when the operand is proven exact
//! `MirType::Int` (never `Unknown` user objects and never `Bool`), plus
//! IEEE-754-exact float identities (`x - 0.0 -> x`, `x * 1.0 -> x`,
//! `x / 1.0 -> x`) when proven `MirType::Float`.
//!
//! Structurally eliminates `#438`-class dunder bypass bugs.

use crate::ast::BinaryOp;
use crate::value::{Value, ValueKind};

use super::super::ir::{MirFn, MirOp, infer_mir_types};

#[inline]
fn as_exact_i64(val: &Value) -> Option<i64> {
    match val.kind() {
        ValueKind::Int(n) => Some(n),
        _ => None,
    }
}

#[inline]
fn is_pos_zero_f64(val: &Value) -> bool {
    matches!(val.kind(), ValueKind::Float(f) if f.to_bits() == 0.0f64.to_bits())
}

#[inline]
fn is_one_f64(val: &Value) -> bool {
    matches!(val.kind(), ValueKind::Float(f) if f == 1.0)
}

/// Apply type-proven algebraic simplifications to `MirOp::BinOp` instructions.
pub fn mir_algebraic_simplify(func: &mut MirFn) {
    infer_mir_types(func);

    let types = func.val_types.clone();
    let mut changed = false;

    for block in &mut func.blocks {
        for inst in &mut block.instrs {
            let MirOp::BinOp {
                op,
                lhs,
                rhs,
                is_inplace: _,
            } = inst.op
            else {
                continue;
            };
            let lt = &types[lhs.0 as usize];
            let rt = &types[rhs.0 as usize];

            // Exact `int` identities (never `bool`, never `Unknown`).
            if lt.is_int() && rt.is_int() {
                let lc = lt.as_const().and_then(as_exact_i64);
                let rc = rt.as_const().and_then(as_exact_i64);
                let simplified: Option<MirOp> = match op {
                    BinaryOp::Add => {
                        if rc == Some(0) {
                            Some(MirOp::Copy(lhs))
                        } else if lc == Some(0) {
                            Some(MirOp::Copy(rhs))
                        } else {
                            None
                        }
                    }
                    BinaryOp::Sub => {
                        if rc == Some(0) {
                            Some(MirOp::Copy(lhs))
                        } else if lhs == rhs {
                            Some(MirOp::Const(Value::int(0)))
                        } else {
                            None
                        }
                    }
                    BinaryOp::Mul => {
                        if rc == Some(1) {
                            Some(MirOp::Copy(lhs))
                        } else if lc == Some(1) {
                            Some(MirOp::Copy(rhs))
                        } else if rc == Some(0) || lc == Some(0) {
                            Some(MirOp::Const(Value::int(0)))
                        } else {
                            None
                        }
                    }
                    BinaryOp::FloorDiv => {
                        if rc == Some(1) {
                            Some(MirOp::Copy(lhs))
                        } else {
                            None
                        }
                    }
                    BinaryOp::BitOr => {
                        if rc == Some(0) {
                            Some(MirOp::Copy(lhs))
                        } else if lc == Some(0) {
                            Some(MirOp::Copy(rhs))
                        } else if lhs == rhs {
                            Some(MirOp::Copy(lhs))
                        } else {
                            None
                        }
                    }
                    BinaryOp::BitXor => {
                        if rc == Some(0) {
                            Some(MirOp::Copy(lhs))
                        } else if lc == Some(0) {
                            Some(MirOp::Copy(rhs))
                        } else if lhs == rhs {
                            Some(MirOp::Const(Value::int(0)))
                        } else {
                            None
                        }
                    }
                    BinaryOp::BitAnd => {
                        if rc == Some(0) || lc == Some(0) {
                            Some(MirOp::Const(Value::int(0)))
                        } else if lhs == rhs {
                            Some(MirOp::Copy(lhs))
                        } else {
                            None
                        }
                    }
                    BinaryOp::LShift | BinaryOp::RShift => {
                        if rc == Some(0) {
                            Some(MirOp::Copy(lhs))
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some(new_op) = simplified {
                    inst.op = new_op;
                    changed = true;
                    continue;
                }
            }

            // IEEE-754-bit-exact `float` identities only (preserves `-0.0`,
            // `NaN`, and `inf` bit-for-bit!).
            if lt.is_float() && rt.is_float() {
                let lc = lt.as_const();
                let rc = rt.as_const();
                let simplified: Option<MirOp> = match op {
                    BinaryOp::Sub if rc.is_some_and(is_pos_zero_f64) => Some(MirOp::Copy(lhs)),
                    BinaryOp::Mul if rc.is_some_and(is_one_f64) => Some(MirOp::Copy(lhs)),
                    BinaryOp::Mul if lc.is_some_and(is_one_f64) => Some(MirOp::Copy(rhs)),
                    BinaryOp::Div if rc.is_some_and(is_one_f64) => Some(MirOp::Copy(lhs)),
                    _ => None,
                };
                if let Some(new_op) = simplified {
                    inst.op = new_op;
                    changed = true;
                }
            }
        }
    }

    if changed {
        infer_mir_types(func);
    }
}
